//! Cross-platform "is this pid a live process?" probe that does **not**
//! mistake a corpse for a live process.
//!
//! # Why this module exists
//!
//! The obvious probes are all wrong in the same direction:
//!
//! * `kill(pid, 0) == 0` returns **0 for a Linux/macOS zombie** — a process
//!   that has already exited and is only waiting for its parent to reap it.
//! * `Path::new("/proc/<pid>").exists()` is **true for a zombie** — the
//!   `/proc` entry survives until the corpse is reaped.
//! * `OpenProcess(pid)` **succeeds for an exited Windows process** for as long
//!   as anything still holds a handle to it, because the pid stays reserved.
//!
//! On a normal host you rarely notice, because PID 1 (`systemd`, `launchd`)
//! adopts orphans and reaps them within milliseconds. In a container started
//! **without** a reaping init, PID 1 is whatever command was run; Rust's
//! `Child::wait()` issues `waitpid(<specific pid>)` and never `wait(-1)`, so
//! PID 1 cannot incidentally reap an adopted orphan, and the corpse stays a
//! zombie indefinitely. Thirteen descendant-containment tests in this
//! workspace read that zombie as a surviving process and went red on exactly
//! that shape. See `.planning/CI-IMAGE.md` §2 and `.planning/ZOMBIE-PROBE.md`.
//!
//! Process containment is a security property, so the probe that measures it
//! has to be able to tell the two apart on **every** platform, not just on
//! hosts that happen to have a reaping init.
//!
//! # Why the answer is three-valued
//!
//! A probe that answers "dead" for everything makes every containment test
//! pass. A probe that answers "alive" for everything wedges every lock guard.
//! Neither failure is visible in a two-valued `bool`, so the real answer is
//! [`ProcessLiveness`], and "I could not tell" is a *distinct* third state
//! rather than a silent lean in either direction.
//!
//! [`process_is_alive`] is the convenience wrapper for callers that must
//! collapse it: it maps [`ProcessLiveness::Indeterminate`] to `true`, which is
//! the conservative direction for both of this workspace's caller shapes —
//! a containment probe fails **loud** rather than declaring success it cannot
//! see, and a resource guard refuses rather than stealing a lock from a
//! process it merely cannot observe.
//!
//! # Platform coverage
//!
//! | platform | mechanism | corpse looks like |
//! |---|---|---|
//! | Linux | `/proc/<pid>/stat` field 3 | `Z` (zombie) or `X`/`x` (dead) |
//! | macOS | `sysctl` `KERN_PROC_PID` → `kinfo_proc.kp_proc.p_stat` | `SZOMB` |
//! | Windows | `OpenProcess` + `WaitForSingleObject(h, 0)` | `WAIT_OBJECT_0` |
//! | other unix | `kill(pid, 0)` only | **indistinguishable** → `Indeterminate` |
//!
//! Adding a platform means adding one arm here, not editing every probe.

/// What an external observer can say about a process id.
///
/// This is deliberately **not** a `bool`. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessLiveness {
    /// The process exists and has not exited.
    Live,
    /// The pid names no process at all, or names one that has already exited
    /// and is merely awaiting reaping (a Unix zombie, or a Windows process
    /// whose pid is still reserved by an open handle).
    Dead,
    /// The platform did not give an answer precise enough to separate a live
    /// process from an unreaped corpse — for example `EPERM` because the pid
    /// belongs to another user, or a Unix without a zombie-aware probe here.
    ///
    /// Callers must not silently read this as either `Live` or `Dead`.
    Indeterminate,
}

impl ProcessLiveness {
    /// `true` only for [`ProcessLiveness::Live`].
    pub fn is_live(self) -> bool {
        matches!(self, ProcessLiveness::Live)
    }

    /// `true` only for [`ProcessLiveness::Dead`].
    pub fn is_dead(self) -> bool {
        matches!(self, ProcessLiveness::Dead)
    }
}

/// Probe `pid` and report what can actually be established about it.
///
/// A zombie / already-exited process reports [`ProcessLiveness::Dead`].
pub fn process_liveness(pid: u32) -> ProcessLiveness {
    // POSIX defines `kill(0, sig)` as "every process in the CALLER'S process
    // group", so a pid-0 probe answers a different question than it looks
    // like it asks and would report the caller itself as alive. Windows pid 0
    // is the System Idle Process. Neither is ever a process a caller here
    // holds, so refuse it up front on every platform.
    if pid == 0 {
        return ProcessLiveness::Dead;
    }
    platform::liveness(pid)
}

/// Collapse [`process_liveness`] to a `bool`, leaning conservative.
///
/// [`ProcessLiveness::Indeterminate`] maps to `true`. A zombie maps to
/// `false` — that is the whole point of this module.
pub fn process_is_alive(pid: u32) -> bool {
    !process_liveness(pid).is_dead()
}

/// Extract the state character from a Linux `/proc/<pid>/stat` line.
///
/// The line is `<pid> (<comm>) <state> <ppid> ...` and `comm` may contain
/// both spaces **and** `)`, so the state character is found by scanning from
/// the RIGHT for the last `)` — none of the numeric fields that follow it can
/// contain a parenthesis. Returns `None` for a line that does not have that
/// shape at all, which callers must treat as "could not tell", never as
/// "alive".
///
/// Exposed (and compiled) on every platform so the parser can be unit-tested
/// without a Linux host.
pub fn proc_stat_state_char(stat: &str) -> Option<char> {
    let (_, after_comm) = stat.rsplit_once(')')?;
    after_comm.trim_start().chars().next()
}

/// Does a Linux `/proc/<pid>/stat` state character mean the process is a
/// corpse? `Z` is a zombie; `X`/`x` are the kernel's "dead" states.
pub fn proc_stat_state_is_corpse(state: char) -> bool {
    matches!(state, 'Z' | 'X' | 'x')
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod platform {
    use super::{ProcessLiveness, proc_stat_state_char, proc_stat_state_is_corpse};

    pub(super) fn liveness(pid: u32) -> ProcessLiveness {
        // Step 1: does the pid resolve to anything at all?
        //
        // SAFETY: signal 0 delivers nothing; it performs only the kernel's
        // existence + permission check.
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
            return match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::ESRCH) => ProcessLiveness::Dead,
                // EPERM: the process exists but belongs to someone else, so we
                // cannot read its state. Anything else is equally unreadable.
                _ => ProcessLiveness::Indeterminate,
            };
        }

        // Step 2: it resolves — but `kill` says 0 for a zombie too, so the
        // state character is the part that actually answers the question.
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => match proc_stat_state_char(&stat) {
                Some(state) if proc_stat_state_is_corpse(state) => ProcessLiveness::Dead,
                Some(_) => ProcessLiveness::Live,
                // Malformed stat line. Two hand-rolled copies of this probe
                // used to exist in this workspace and they disagreed here —
                // one guessed "gone", the other guessed "alive". Neither
                // guess is a measurement.
                None => ProcessLiveness::Indeterminate,
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Reaped between the two syscalls.
                ProcessLiveness::Dead
            }
            // `hidepid`, a restricted /proc mount, or a PID namespace we
            // cannot see into: the process exists, its state does not.
            Err(_) => ProcessLiveness::Indeterminate,
        }
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
mod platform {
    use super::ProcessLiveness;

    /// `SZOMB` from `<sys/proc.h>`: `SIDL 1, SRUN 2, SSLEEP 3, SSTOP 4,
    /// SZOMB 5`. Not re-exported by the `libc` crate for Apple targets, so it
    /// is spelled out here with its source rather than assumed.
    const SZOMB: libc::c_char = 5;

    pub(super) fn liveness(pid: u32) -> ProcessLiveness {
        // macOS has no /proc. `kill(pid, 0)` succeeds for a zombie exactly as
        // it does on Linux, so it cannot be the probe either. The kernel's
        // own answer is `kinfo_proc.kp_proc.p_stat`.
        let mut mib: [libc::c_int; 4] = [
            libc::CTL_KERN,
            libc::KERN_PROC,
            libc::KERN_PROC_PID,
            pid as libc::c_int,
        ];
        let mut info: libc::kinfo_proc = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of::<libc::kinfo_proc>();

        // SAFETY: `mib` is a 4-element array matching the `namelen` argument,
        // `info` is a live, correctly sized `kinfo_proc`, and `size` is its
        // byte length in/out. No pointer outlives this call.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                4,
                (&mut info as *mut libc::kinfo_proc).cast::<libc::c_void>(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };

        if rc != 0 {
            return match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::ESRCH) => ProcessLiveness::Dead,
                _ => ProcessLiveness::Indeterminate,
            };
        }
        // A successful sysctl that filled nothing in means "no such process".
        if size == 0 {
            return ProcessLiveness::Dead;
        }
        if info.kp_proc.p_stat == SZOMB {
            ProcessLiveness::Dead
        } else {
            ProcessLiveness::Live
        }
    }
}

// ---------------------------------------------------------------------------
// Other unix (FreeBSD, illumos, …) — honest about what it cannot see
// ---------------------------------------------------------------------------
#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
mod platform {
    use super::ProcessLiveness;

    pub(super) fn liveness(pid: u32) -> ProcessLiveness {
        // SAFETY: signal 0 delivers nothing.
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
            return match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::ESRCH) => ProcessLiveness::Dead,
                _ => ProcessLiveness::Indeterminate,
            };
        }
        // The pid resolves, but there is no zombie-aware arm for this target,
        // so a corpse and a live process are indistinguishable here. Say so
        // rather than guessing "Live" — `process_is_alive` still returns true,
        // which is the pre-existing conservative behaviour, but the enum does
        // not claim a measurement that was not taken.
        ProcessLiveness::Indeterminate
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod platform {
    use super::ProcessLiveness;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, FALSE, GetLastError, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    pub(super) fn liveness(pid: u32) -> ProcessLiveness {
        // Windows has no zombie in the Unix sense, but it has the same
        // observable hazard: while ANY handle to an exited process is still
        // open, the pid stays reserved and `OpenProcess` keeps succeeding. A
        // parent that holds a `std::process::Child` it never waited on is
        // exactly that case, and it is the direct analogue of the Linux
        // zombie this module exists for.
        //
        // `WaitForSingleObject(h, 0)` is preferred over
        // `GetExitCodeProcess != STILL_ACTIVE`: a live process cannot
        // signal its own handle, whereas a process whose genuine exit code
        // happens to be 259 is indistinguishable from a running one under
        // `STILL_ACTIVE`.
        //
        // SAFETY: Win32 FFI. `OpenProcess` returns NULL on failure; the
        // handle is closed on every path that obtained one.
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                FALSE,
                pid,
            )
        };
        if handle.is_null() {
            // SAFETY: reads the calling thread's last-error slot.
            let code = unsafe { GetLastError() };
            return if code == ERROR_INVALID_PARAMETER {
                // Documented as "no such process id".
                ProcessLiveness::Dead
            } else {
                // ERROR_ACCESS_DENIED and friends: it may well exist.
                ProcessLiveness::Indeterminate
            };
        }

        // SAFETY: `handle` is a valid process handle with SYNCHRONIZE rights.
        let wait = unsafe { WaitForSingleObject(handle, 0) };
        // SAFETY: `handle` came from a successful `OpenProcess` and is not
        // used again after this call.
        unsafe { CloseHandle(handle) };

        match wait {
            WAIT_OBJECT_0 => ProcessLiveness::Dead,
            WAIT_TIMEOUT => ProcessLiveness::Live,
            _ => ProcessLiveness::Indeterminate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- the /proc stat parser, testable on every platform ------------------

    #[test]
    fn state_char_is_read_from_the_right_because_comm_can_contain_parens() {
        // Real kernel shapes. The comm field is attacker-influenced (it is the
        // executable name), so a left-to-right parse is exploitable into
        // misreading the state.
        assert_eq!(
            proc_stat_state_char("1234 (sleep) S 1 1234 1234 0 -1"),
            Some('S')
        );
        assert_eq!(
            proc_stat_state_char("1234 (sleep) Z 1 1234 1234 0 -1"),
            Some('Z')
        );
        assert_eq!(
            proc_stat_state_char("1234 (weird ) name) Z 1 1234"),
            Some('Z'),
            "a ')' inside comm must not shift the state field"
        );
        assert_eq!(
            proc_stat_state_char("1234 (a) b) c) R 1 1234"),
            Some('R'),
            "multiple ')' inside comm must not shift the state field"
        );
    }

    #[test]
    fn malformed_stat_yields_none_rather_than_a_guess() {
        assert_eq!(proc_stat_state_char(""), None);
        assert_eq!(proc_stat_state_char("no parens at all"), None);
        assert_eq!(
            proc_stat_state_char("1234 (sleep)"),
            None,
            "truncated after comm"
        );
        assert_eq!(
            proc_stat_state_char("1234 (sleep)   "),
            None,
            "only whitespace after comm"
        );
    }

    #[test]
    fn corpse_states_are_z_and_x_only() {
        for state in ['Z', 'X', 'x'] {
            assert!(
                proc_stat_state_is_corpse(state),
                "{state} must read as a corpse"
            );
        }
        // R running, S sleeping, D uninterruptible, T stopped, t traced,
        // I idle, K wakekill, W waking, P parked.
        for state in ['R', 'S', 'D', 'T', 't', 'I', 'K', 'W', 'P'] {
            assert!(
                !proc_stat_state_is_corpse(state),
                "{state} is a LIVE state and must not read as a corpse"
            );
        }
    }

    // -- the probe itself ---------------------------------------------------

    #[test]
    fn this_process_reads_as_live() {
        // The positive direction. Without this assertion a probe that answered
        // "dead" for everything would satisfy every containment test in the
        // workspace.
        let me = std::process::id();
        assert_eq!(
            process_liveness(me),
            ProcessLiveness::Live,
            "the running test process (pid {me}) must read as Live"
        );
        assert!(process_is_alive(me));
    }

    #[test]
    fn pid_zero_is_never_alive() {
        // `kill(0, 0)` addresses the caller's own process group and would
        // report success; pid 0 must be refused before it reaches the probe.
        assert_eq!(process_liveness(0), ProcessLiveness::Dead);
        assert!(!process_is_alive(0));
    }
}
