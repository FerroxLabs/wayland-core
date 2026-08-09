//! W-C — a dedicated window station and desktop for AppContainer children.
//!
//! # The defect
//!
//! `STARTUPINFOW::lpDesktop` used to be left NULL, so every sandboxed child
//! inherited the *engine's* window station and desktop. `USER32.dll` connects
//! the process to that window station during its process-attach; if the
//! AppContainer token cannot open the object, `DllMain` fails and the loader
//! returns `STATUS_DLL_INIT_FAILED` (`0xC0000142`) before a single byte of the
//! image's own code runs.
//!
//! Measured on SEANDESKTOP (Windows 10.0.26200), over an OpenSSH session:
//!
//! ```text
//! CURRENT-WINSTA name=Service-0x1-11905c7a$
//! CURRENT-WINSTA sddl=O:BAG:...D:(A;;DCLCSWWPDTSDRCWDWO;;;S-1-5-21-...-1001)
//!                       (A;OINPIO;CCDCLCSWDTLOSDRCWDWO;;;S-1-5-21-...-1001)
//!                       (A;;CR;;;BA)(A;OINPIO;CCDTLO;;;BA)
//! WINSTA0      sddl=...(A;OICIIO;GXGR;;;AC)(A;NP;0x20327;;;AC)
//! ```
//!
//! The service window station a non-interactive engine inherits carries **no**
//! `ALL APPLICATION PACKAGES` (`AC`, S-1-15-2-1) ACE; the interactive
//! `WinSta0` carries two. That is the whole of the difference between "the
//! same binary at the same commit runs `git`" and "it dies at image init",
//! and it is why the failure sorted exactly by USER32 linkage: `cmd`,
//! `hostname`, `attrib` and `find` do not link USER32 and ran; `where`,
//! `git`, `cargo` and `node` do and died.
//!
//! # The repair, and why it TIGHTENS isolation
//!
//! Rather than granting the child access to the station the engine happens to
//! sit on — which on an interactive session is `WinSta0`, the real desktop,
//! with the real clipboard and every other window on it — the engine creates
//! its **own** window station and desktop, empty, ACLed for app containers,
//! and names it in `lpDesktop`. This is Chromium's model. A sandboxed child
//! moves *off* the interactive desktop, so shatter attacks, clipboard reads
//! and screen reads against the operator's session stop being possible on the
//! path that previously worked.
//!
//! # The grants, one line of justification each
//!
//! | Principal | Object | Access | What an attacker gains |
//! |---|---|---|---|
//! | current user | station + desktop | `GENERIC_ALL` | nothing new — it is the creating identity, which already owns the objects |
//! | `BA` (Administrators) | station + desktop | `GENERIC_ALL` | nothing new — an administrator can already open any user object |
//! | `AC` (ALL APPLICATION PACKAGES) | station | the two ACEs `WinSta0` already carries verbatim (`(A;OICIIO;GXGR;;;AC)` + `(A;NP;0x20327;;;AC)`) | another AppContainer on the machine could enumerate an empty private station; on `WinSta0` it could do the same to the *interactive* one, so this is a strict reduction |
//! | `AC` | desktop | `GENERIC_ALL` | a sandboxed child could hook or post messages to other sandboxed children on this desktop — which are already its own siblings inside one job object, and the job's UI restrictions (`JOB_OBJECT_UILIMIT_HANDLES`) block USER handles crossing the job boundary |
//!
//! **Named residual.** The `AC` grant is not narrowed to the per-execution
//! AppContainer package SID. It cannot be without a per-execution DACL write
//! and an ACE merge/teardown lifecycle on an object shared by concurrent
//! executions, which is a larger correctness risk than the exposure it would
//! remove — and the exposure is bounded by an empty station that this process
//! destroys on exit.
//!
//! # The `SetProcessWindowStation` window
//!
//! `CreateDesktopW` always creates the desktop on the *calling process's*
//! window station, so the engine must switch to the new station for the
//! duration of that one call and switch back. That is a process-global
//! mutation. It is bounded three ways: it runs exactly once per engine
//! process (`OnceLock`, which also serialises it), the original station is
//! restored immediately afterwards, and `SetProcessWindowStation` only
//! affects threads that have not already been assigned a desktop — which for
//! this engine is every thread, because nothing in it uses USER32. If any
//! step fails the engine keeps the inherited station and logs, so the
//! interactive path that already worked cannot be made worse by this code.

use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{GetLastError, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, CloseWindowStation, CreateDesktopW, CreateWindowStationW,
    GetProcessWindowStation, SetProcessWindowStation,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::handles::OwnedHandle;

/// `WINSTA_ALL_ACCESS | STANDARD_RIGHTS_REQUIRED` from `winuser.h`. Not
/// re-exported by windows-sys 0.59.
const WINSTA_ALL: u32 = 0x000F_037F;
/// `DESKTOP_*` full mask | `STANDARD_RIGHTS_REQUIRED` from `winuser.h`.
const DESKTOP_ALL: u32 = 0x000F_01FF;

/// The station's DACL. The two `AC` ACEs are copied verbatim from the
/// measured `WinSta0` descriptor on Windows 10.0.26200, so the access an
/// AppContainer gets here is exactly the access it already has on the
/// interactive station and no more.
fn station_sddl(user_sid: &str) -> String {
    format!("D:(A;;GA;;;{user_sid})(A;;GA;;;BA)(A;OICIIO;GXGR;;;AC)(A;NP;0x20327;;;AC)")
}

/// The desktop's DACL. See the module table for why `AC` gets `GENERIC_ALL`
/// on an empty desktop that only ever holds this engine's own children.
fn desktop_sddl(user_sid: &str) -> String {
    format!("D:(A;;GA;;;{user_sid})(A;;GA;;;BA)(A;;GA;;;AC)")
}

fn widen(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A `LocalFree`-owned security descriptor, kept alive for as long as the
/// `SECURITY_ATTRIBUTES` that points at it is in use.
struct OwnedSd(*mut core::ffi::c_void);

impl Drop for OwnedSd {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0 as _) };
        }
    }
}

/// Build a self-relative security descriptor from an SDDL string.
fn security_descriptor(sddl: &str) -> Option<OwnedSd> {
    let sddl_w = widen(sddl);
    let mut psd: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `sddl_w` is a NUL-terminated UTF-16 buffer that outlives the
    // call; `psd` receives a LocalAlloc'd descriptor which `OwnedSd` frees.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_w.as_ptr(),
            SDDL_REVISION_1,
            &mut psd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || psd.is_null() {
        tracing::warn!(
            target: "wcore_sandbox",
            sddl = %sddl,
            last_err = format!("{:#x}", unsafe { GetLastError() }),
            "ConvertStringSecurityDescriptorToSecurityDescriptorW failed"
        );
        return None;
    }
    Some(OwnedSd(psd))
}

/// The string SID of the identity this process runs as, for the DACL.
fn current_user_sid() -> Option<String> {
    // SAFETY: each call below is checked; every out-parameter is initialised
    // before it is read, and both allocations are released on every path.
    unsafe {
        let mut raw: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) == 0 {
            tracing::warn!(
                target: "wcore_sandbox",
                last_err = format!("{:#x}", GetLastError()),
                "OpenProcessToken(TOKEN_QUERY) failed; sandbox window station not created"
            );
            return None;
        }
        let token = OwnedHandle::new(raw);
        let mut needed: u32 = 0;
        GetTokenInformation(
            token.as_raw(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 {
            return None;
        }
        let mut buf = vec![0u8; needed as usize];
        if GetTokenInformation(
            token.as_raw(),
            TokenUser,
            buf.as_mut_ptr() as _,
            needed,
            &mut needed,
        ) == 0
        {
            tracing::warn!(
                target: "wcore_sandbox",
                last_err = format!("{:#x}", GetLastError()),
                "GetTokenInformation(TokenUser) failed; sandbox window station not created"
            );
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut out: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut out) == 0 || out.is_null() {
            return None;
        }
        let mut len = 0usize;
        while *out.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(out, len));
        LocalFree(out as _);
        Some(s)
    }
}

/// An owned `HWINSTA`. Stored as `usize` so the containing struct is `Send +
/// `Sync` for the process-wide `OnceLock`; a window station handle is not
/// thread-affine, so sharing it across threads is sound.
struct OwnedWinsta(usize);

impl Drop for OwnedWinsta {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from `CreateWindowStationW` and is closed once.
        unsafe { CloseWindowStation(self.0 as _) };
    }
}

/// An owned `HDESK`, stored as `usize` for the same reason as [`OwnedWinsta`].
struct OwnedDesk(usize);

impl Drop for OwnedDesk {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from `CreateDesktopW` and is closed once.
        unsafe { CloseDesktop(self.0 as _) };
    }
}

/// The engine's private window station and desktop. Both handles stay open
/// for the life of the process — a window station or desktop is destroyed
/// when its last handle closes, so dropping them would take the child's
/// desktop with it.
pub(super) struct SandboxStation {
    /// `"<station>\<desktop>"`, ready for `STARTUPINFOW::lpDesktop`.
    qualified: Vec<u16>,
    _winsta: OwnedWinsta,
    _desktop: OwnedDesk,
}

// SAFETY: the only interior state is two kernel handles (as `usize`) and an
// immutable UTF-16 buffer. Window station and desktop handles are not thread
// affine and are only ever read from here.
unsafe impl Send for SandboxStation {}
unsafe impl Sync for SandboxStation {}

static SANDBOX_STATION: OnceLock<Option<SandboxStation>> = OnceLock::new();

/// The `lpDesktop` value for a sandboxed child, or `None` if the engine could
/// not create its own station — in which case the caller leaves `lpDesktop`
/// NULL and the child inherits, exactly as it did before this module existed.
pub(super) fn sandbox_desktop() -> Option<&'static [u16]> {
    SANDBOX_STATION
        .get_or_init(create_sandbox_station)
        .as_ref()
        .map(|s| s.qualified.as_slice())
}

fn create_sandbox_station() -> Option<SandboxStation> {
    let user_sid = current_user_sid()?;
    let station_sd = security_descriptor(&station_sddl(&user_sid))?;
    let desktop_sd = security_descriptor(&desktop_sddl(&user_sid))?;

    // Per-PID so two engines on one desktop never contend for one station,
    // and so a crashed engine's station is not silently reused by the next.
    let station_name = format!("WaylandCoreSbx-{}", std::process::id());
    let desktop_name = "Sandbox";

    let mut station_sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: station_sd.0,
        bInheritHandle: 0,
    };
    let mut desktop_sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: desktop_sd.0,
        bInheritHandle: 0,
    };

    // SAFETY: every handle is checked before use; the original window station
    // is restored on every path out of the switched region, and the two
    // security descriptors outlive the calls that read them.
    unsafe {
        let name_w = widen(&station_name);
        let hsta = CreateWindowStationW(name_w.as_ptr(), 0, WINSTA_ALL, &mut station_sa);
        if hsta.is_null() {
            tracing::warn!(
                target: "wcore_sandbox",
                station = %station_name,
                last_err = format!("{:#x}", GetLastError()),
                "CreateWindowStationW failed; sandboxed children keep the inherited \
                 window station and USER32-linked images may fail with 0xC0000142"
            );
            return None;
        }
        let winsta = OwnedWinsta(hsta as usize);

        let original = GetProcessWindowStation();
        if SetProcessWindowStation(hsta) == 0 {
            tracing::warn!(
                target: "wcore_sandbox",
                last_err = format!("{:#x}", GetLastError()),
                "SetProcessWindowStation onto the sandbox station failed; keeping \
                 the inherited station"
            );
            return None;
        }

        let desk_w = widen(desktop_name);
        let hdesk = CreateDesktopW(
            desk_w.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            DESKTOP_ALL,
            &mut desktop_sa,
        );
        let create_err = GetLastError();

        // Restore BEFORE inspecting `hdesk`, so no early return can leave the
        // engine parked on the sandbox station.
        if !original.is_null() && SetProcessWindowStation(original) == 0 {
            tracing::error!(
                target: "wcore_sandbox",
                last_err = format!("{:#x}", GetLastError()),
                "failed to restore the engine's original window station"
            );
        }

        if hdesk.is_null() {
            tracing::warn!(
                target: "wcore_sandbox",
                last_err = format!("{create_err:#x}"),
                "CreateDesktopW on the sandbox station failed; keeping the \
                 inherited station"
            );
            return None;
        }

        let qualified = widen(&format!("{station_name}\\{desktop_name}"));
        tracing::debug!(
            target: "wcore_sandbox",
            station = %station_name,
            desktop = %desktop_name,
            "sandbox window station created"
        );
        Some(SandboxStation {
            qualified,
            _winsta: winsta,
            _desktop: OwnedDesk(hdesk as usize),
        })
    }
}
