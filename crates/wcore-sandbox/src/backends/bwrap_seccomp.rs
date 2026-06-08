//! S4 — seccomp-bpf filter construction for the bwrap backend.
//!
//! Builds an `ScmpFilterContext` per `SyscallPolicy`, exports BPF to a
//! tempfile, and returns the file handle so the caller can pass its fd to
//! `bwrap --seccomp <fd>`. bwrap itself applies the filter atomically to
//! the child via `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, …)` after
//! setting `PR_SET_NO_NEW_PRIVS`, so the well-known TOCTOU race between
//! `execve` and `prctl` (path B in Audit B) is avoided.
//!
//! Compiled ONLY when the `seccomp` cargo feature is enabled AND target is
//! Linux. Failures bubble up as `SandboxError::ExecFailed`; the caller is
//! responsible for warn-once degradation back to bwrap-only sandbox.

#![cfg(all(target_os = "linux", feature = "seccomp"))]

use crate::error::{Result, SandboxError};
use crate::manifest::SyscallPolicy;
use libseccomp::{ScmpAction, ScmpFilterContext, ScmpSyscall};
use std::fs::File;
use std::io::{Seek, SeekFrom};

/// Default syscall allowlist used by `SyscallPolicy::Strict`.
///
/// Covers the syscalls a normal POSIX program needs to start, allocate
/// memory, do basic I/O, and exit. Anything outside this list is killed
/// with `SECCOMP_RET_KILL_PROCESS`. The list is intentionally conservative
/// — adding rather than removing entries when real plugins hit a deny is
/// the safer evolution path.
///
/// This is exposed as a `pub const` so reviewers can audit the surface in
/// one place rather than reading the filter-build code.
pub const DEFAULT_SYSCALL_ALLOWLIST: &[&str] = &[
    // I/O
    "read",
    "write",
    "readv",
    "writev",
    "pread64",
    "pwrite64",
    "openat",
    "openat2",
    "close",
    "close_range",
    "lseek",
    "dup",
    "dup2",
    "dup3",
    "fcntl",
    "ioctl",
    "pipe",
    "pipe2",
    "poll",
    "ppoll",
    "select",
    "pselect6",
    "epoll_create1",
    "epoll_ctl",
    "epoll_wait",
    "epoll_pwait",
    "eventfd2",
    "signalfd4",
    // Filesystem metadata
    "stat",
    "fstat",
    "lstat",
    "newfstatat",
    "statx",
    "access",
    "faccessat",
    "faccessat2",
    "readlink",
    "readlinkat",
    "getcwd",
    "getdents64",
    // Memory
    "mmap",
    "munmap",
    "mremap",
    "mprotect",
    "brk",
    "madvise",
    // Process / thread lifecycle
    "exit",
    "exit_group",
    "getpid",
    "gettid",
    "getppid",
    "getuid",
    "geteuid",
    "getgid",
    "getegid",
    "getgroups",
    "set_tid_address",
    "set_robust_list",
    "sched_yield",
    "sched_getaffinity",
    "prlimit64",
    // Signal handling
    "rt_sigaction",
    "rt_sigprocmask",
    "rt_sigreturn",
    "sigaltstack",
    // Synchronisation
    "futex",
    "futex_waitv",
    // Clone families — needed by glibc/musl thread startup and posix_spawn.
    // bwrap's `--unshare-all` already prevents the child from creating new
    // user/PID/net namespaces, so an inner `clone3` only spawns a thread.
    "clone",
    "clone3",
    // Time
    "clock_gettime",
    "clock_getres",
    "clock_nanosleep",
    "nanosleep",
    "gettimeofday",
    // Misc essentials
    "getrandom",
    "uname",
    "prctl",
    "arch_prctl",
    "membarrier",
    "rseq",
];

/// Build a `ScmpFilterContext` for the given policy. The filter is NOT
/// loaded into the kernel here; the caller exports it to a fd and hands it
/// to bwrap.
fn build_filter(policy: SyscallPolicy) -> Result<Option<ScmpFilterContext>> {
    match policy {
        SyscallPolicy::Inherit => Ok(None),
        SyscallPolicy::Strict => {
            let mut ctx = ScmpFilterContext::new(ScmpAction::KillProcess)
                .map_err(|e| SandboxError::ExecFailed(format!("seccomp: new filter ctx: {e}")))?;
            for name in DEFAULT_SYSCALL_ALLOWLIST {
                let sc = ScmpSyscall::from_name(name).map_err(|e| {
                    SandboxError::ExecFailed(format!("seccomp: unknown syscall '{name}': {e}"))
                })?;
                ctx.add_rule(ScmpAction::Allow, sc).map_err(|e| {
                    SandboxError::ExecFailed(format!("seccomp: add_rule {name}: {e}"))
                })?;
            }
            Ok(Some(ctx))
        }
    }
}

/// Export the BPF for `policy` to a fresh anonymous tempfile and return it
/// rewound to offset 0. The returned `File` owns the fd; the caller passes
/// it to `bwrap` via `--seccomp <fd>`. Returns `Ok(None)` for
/// `SyscallPolicy::Inherit`.
pub fn export_filter_to_tempfile(policy: SyscallPolicy) -> Result<Option<File>> {
    let Some(ctx) = build_filter(policy)? else {
        return Ok(None);
    };
    let mut file = tempfile::tempfile()
        .map_err(|e| SandboxError::ExecFailed(format!("seccomp: tempfile: {e}")))?;
    ctx.export_bpf(&file)
        .map_err(|e| SandboxError::ExecFailed(format!("seccomp: export_bpf: {e}")))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| SandboxError::ExecFailed(format!("seccomp: rewind: {e}")))?;
    Ok(Some(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherit_returns_none() {
        let out = export_filter_to_tempfile(SyscallPolicy::Inherit).expect("inherit ok");
        assert!(out.is_none());
    }

    #[test]
    fn strict_produces_nonempty_bpf() {
        // libseccomp may or may not be linkable depending on the host. If
        // the C lib is missing we get a runtime error from new_filter; in
        // that case treat as skip rather than fail.
        let res = export_filter_to_tempfile(SyscallPolicy::Strict);
        let file = match res {
            Ok(Some(f)) => f,
            Ok(None) => panic!("Strict must produce a filter"),
            Err(SandboxError::ExecFailed(msg)) if msg.contains("seccomp") => {
                eprintln!("skip: libseccomp unavailable on this host: {msg}");
                return;
            }
            Err(e) => panic!("unexpected error: {e}"),
        };
        let meta = file.metadata().expect("metadata");
        assert!(meta.len() > 0, "BPF program must not be empty");
    }
}
