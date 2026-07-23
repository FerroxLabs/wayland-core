//! AppContainer OS handle, Job Object, SID and pipe/attribute primitives (F20-03 Task 1A split of `windows_impl`).
#![allow(unused_imports)]

use super::super::super::SandboxBackend;
use super::super::appcontainer_acl_lease::ExecutionIdentity;
use super::super::{NEGATIVE_PROBE_TTL, ProbeCache};
use crate::error::{Result, SandboxError};
use crate::manifest::{NetworkPolicy, SandboxManifest};
use crate::{ResourceLimitEnforcement, SandboxCommand, SandboxOutput};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::mem;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::ptr;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::{
    AllocateAndInitializeSid, CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, FreeSid, GetLengthSid,
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, SECURITY_ATTRIBUTES,
    SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES, SID_IDENTIFIER_AUTHORITY, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};

use super::command::*;
use super::process::*;
use super::*;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PRIORITY_CLASS,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_PROCESS_TIME,
    JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    BELOW_NORMAL_PRIORITY_CLASS, CREATE_SUSPENDED, CreateProcessAsUserW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    OpenProcessToken, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

/// RAII helper that closes a HANDLE on drop. Skips closing if the handle
/// is null or `INVALID_HANDLE_VALUE`.
pub(super) struct OwnedHandle(HANDLE);
impl OwnedHandle {
    pub(super) fn new(h: HANDLE) -> Self {
        Self(h)
    }
    pub(super) fn as_raw(&self) -> HANDLE {
        self.0
    }
}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                CloseHandle(self.0);
            }
        }
    }
}

/// Cross-thread ownership of the AppContainer Job Object. Windows kernel
/// handles are process-wide and may be used from any thread; this wrapper
/// makes that guarantee explicit for the blocking-worker cancellation path.
pub(super) struct SharedJob(OwnedHandle);

// SAFETY: HANDLE values are process-wide kernel object references. This
// wrapper closes its handle exactly once through OwnedHandle and exposes
// only thread-safe Job Object operations.
unsafe impl Send for SharedJob {}
// SAFETY: see the Send justification above; concurrent termination of the
// same Job Object is documented and idempotent for this use.
unsafe impl Sync for SharedJob {}

impl SharedJob {
    /// Wrap an owned Job Object handle. The handle is closed exactly once
    /// when the last `Arc<SharedJob>` drops (single-close invariant).
    pub(super) fn new(handle: OwnedHandle) -> Self {
        Self(handle)
    }

    pub(super) fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.0.as_raw(), 1);
        }
    }

    pub(super) fn as_raw(&self) -> HANDLE {
        self.0.as_raw()
    }
}

#[derive(Default)]
pub(super) struct JobControlState {
    cancelled: bool,
    job: Option<Arc<SharedJob>>,
}

/// Shared cancellation state between the async caller and the Win32
/// blocking worker. The worker publishes its Job Object here before spawn;
/// dropping the async execution future can then kill the complete tree.
#[derive(Default)]
pub(super) struct JobControl {
    state: Mutex<JobControlState>,
}

impl JobControl {
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, JobControlState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn ensure_active(&self) -> Result<()> {
        if self.lock().cancelled {
            Err(SandboxError::Timeout)
        } else {
            Ok(())
        }
    }

    pub(super) fn install(&self, job: Arc<SharedJob>) -> Result<()> {
        let cancelled = {
            let mut state = self.lock();
            state.job = Some(Arc::clone(&job));
            state.cancelled
        };
        if cancelled {
            job.terminate();
            Err(SandboxError::Timeout)
        } else {
            Ok(())
        }
    }

    pub(super) fn cancel(&self) {
        let job = {
            let mut state = self.lock();
            state.cancelled = true;
            state.job.clone()
        };
        if let Some(job) = job {
            job.terminate();
        }
    }

    /// Hold the state lock across the final cancellation check and resume.
    /// This removes the check/resume TOCTOU window: cancellation either
    /// wins before resume, or terminates the already-resumed Job tree.
    pub(super) fn resume_if_active(&self, thread: HANDLE) -> Result<()> {
        let state = self.lock();
        if state.cancelled {
            return Err(SandboxError::Timeout);
        }
        if unsafe { ResumeThread(thread) } == u32::MAX {
            return Err(SandboxError::ExecFailed(format!(
                "ResumeThread: {:#x}",
                unsafe { GetLastError() }
            )));
        }
        Ok(())
    }
}

pub(super) struct JobCancellationGuard {
    control: Arc<JobControl>,
    armed: bool,
}

impl JobCancellationGuard {
    pub(super) fn new(control: Arc<JobControl>) -> Self {
        Self {
            control,
            armed: true,
        }
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for JobCancellationGuard {
    fn drop(&mut self) {
        if self.armed {
            self.control.cancel();
        }
    }
}

/// RAII for a SID allocated with `AllocateAndInitializeSid`.
pub(super) struct OwnedSid(*mut core::ffi::c_void);
impl OwnedSid {
    pub(super) fn as_psid(&self) -> *mut core::ffi::c_void {
        self.0
    }
}
impl Drop for OwnedSid {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                FreeSid(self.0 as _);
            }
        }
    }
}

pub(super) fn allocate_sid(authority: [u8; 6], subauthorities: &[u32]) -> Result<OwnedSid> {
    let auth = SID_IDENTIFIER_AUTHORITY { Value: authority };
    let mut sub = [0u32; 8];
    for (i, s) in subauthorities.iter().enumerate().take(8) {
        sub[i] = *s;
    }
    let mut sid: *mut core::ffi::c_void = ptr::null_mut();
    let ok = unsafe {
        AllocateAndInitializeSid(
            &auth,
            subauthorities.len() as u8,
            sub[0],
            sub[1],
            sub[2],
            sub[3],
            sub[4],
            sub[5],
            sub[6],
            sub[7],
            &mut sid,
        )
    };
    if ok == 0 || sid.is_null() {
        return Err(SandboxError::ExecFailed(format!(
            "AllocateAndInitializeSid: {:#x}",
            unsafe { GetLastError() }
        )));
    }
    Ok(OwnedSid(sid))
}

pub(super) unsafe fn drain_pipe(h: HANDLE, output_bytes: Arc<AtomicUsize>) -> (Vec<u8>, bool) {
    let mut out: Vec<u8> = Vec::new();
    let mut exceeded = false;
    let mut buf = [0u8; 4096];
    loop {
        let mut read: u32 = 0;
        let ok = unsafe {
            ReadFile(
                h,
                buf.as_mut_ptr() as _,
                buf.len() as u32,
                &mut read,
                ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            break;
        }
        let read = read as usize;
        if super::super::super::reserve_output(&output_bytes, read) {
            out.extend_from_slice(&buf[..read]);
        } else {
            // Keep draining after the ceiling is hit so the child does not
            // deadlock on a full pipe. Discarding the excess bounds host
            // memory while still allowing the owned job to exit normally.
            exceeded = true;
        }
    }
    (out, exceeded)
}

pub(super) fn clamp_timeout_ms(d: Duration) -> u32 {
    let ms = d.as_millis();
    if ms >= INFINITE as u128 - 1 {
        INFINITE - 1
    } else {
        ms as u32
    }
}

/// RAII: delete the proc-thread attribute list.
pub(super) struct AttrListGuard {
    // `pub(super)` so the `process` sibling module can construct the guard via
    // struct literal (E0451: the field was private, matching the SharedJob fix).
    pub(super) list: LPPROC_THREAD_ATTRIBUTE_LIST,
}
impl Drop for AttrListGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.list.is_null() {
                DeleteProcThreadAttributeList(self.list);
            }
        }
    }
}
