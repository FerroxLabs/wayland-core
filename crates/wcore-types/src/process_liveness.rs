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

/// Extract `(state, pgrp)` from a Linux `/proc/<pid>/stat` line.
///
/// Same right-to-left scan as [`proc_stat_state_char`], for the same reason:
/// `comm` is the executable name and may contain spaces and `)`. After the
/// last `)` the fields are positional — `state ppid pgrp …` — so `pgrp` is
/// the third whitespace-separated token.
///
/// Exposed (and compiled) on every platform so the parser can be unit-tested
/// without a Linux host.
pub fn proc_stat_state_and_pgrp(stat: &str) -> Option<(char, i64)> {
    let (_, after_comm) = stat.rsplit_once(')')?;
    let mut fields = after_comm.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let _ppid = fields.next()?;
    let pgrp = fields.next()?.parse().ok()?;
    Some((state, pgrp))
}

/// How many *live* processes a Unix process group still contains.
///
/// This is deliberately **not** a `usize`. A census that cannot see is not a
/// census of zero — and "zero" is the answer that lets a containment proof
/// succeed, so it is the answer an unreliable instrument must never be able to
/// produce by accident. See [`ProcessLiveness`] for the same argument applied
/// to a single pid.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessGroupCensus {
    /// Exactly this many members exist and have not exited. Unreaped corpses
    /// are **not** counted: a zombie holds no resources and cannot execute, so
    /// a group whose only remaining member is the anchor zombie is `Live(0)`.
    Live(usize),
    /// The group could not be enumerated. Callers must not read this as zero.
    Indeterminate(String),
}

/// Count the live members of the Unix process group `pgid`.
///
/// # Why this exists
///
/// `kill(-pgid, 0)` looks like the cheap way to ask "is this group empty?" and
/// **is not discriminating**: on Darwin it returns `EPERM` — not `ESRCH` —
/// when the group's only remaining member is an unreaped zombie, which is
/// exactly the state a correctly-cleaned group is in. Signal-0 therefore
/// cannot separate "everything is dead" from "something survived and I am not
/// allowed to signal it", and those two must never be confused: the first is
/// containment succeeding and the second is containment failing.
///
/// So this enumerates rather than probes.
#[cfg(unix)]
pub fn process_group_census(pgid: u32) -> ProcessGroupCensus {
    if pgid == 0 {
        // `kill(0, …)` addresses the CALLER'S group; a pgid-0 census would
        // silently answer a different question than the one asked.
        return ProcessGroupCensus::Indeterminate(
            "process group 0 means \"the caller's own group\", never a specific group".into(),
        );
    }
    platform::group_census(pgid)
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod platform {
    use super::{
        ProcessGroupCensus, ProcessLiveness, proc_stat_state_and_pgrp, proc_stat_state_char,
        proc_stat_state_is_corpse,
    };

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

    pub(super) fn group_census(pgid: u32) -> ProcessGroupCensus {
        let entries = match std::fs::read_dir("/proc") {
            Ok(entries) => entries,
            Err(error) => {
                return ProcessGroupCensus::Indeterminate(format!("cannot read /proc: {error}"));
            }
        };

        // A weak self-test, kept for the PID-namespace case but NOT relied on
        // for permission failures.
        //
        // It does not catch `hidepid`, and an earlier version of this comment
        // claimed it did. `hidepid` hides OTHER users' processes; your own
        // stay visible, so `saw_self` is green while the scan is blind to
        // exactly the members that matter. That gap is closed by the
        // per-entry error handling below, not here.
        let me = std::process::id();
        let mut saw_self = false;

        let mut live = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let stat = match std::fs::read_to_string(format!("/proc/{name}/stat")) {
                Ok(stat) => stat,
                // Exited and was reaped between `read_dir` and this read. A
                // process that no longer exists is not a containment failure.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                // ANY other error is "could not look", and must never be
                // recorded as "nothing was there".
                //
                // This is the exact shape the whole module exists to refuse.
                // Under `hidepid=1`, reading a different uid's
                // `/proc/<pid>/stat` fails with EACCES. A group member owned
                // by another uid is also precisely the member `kill(-pgid,
                // SIGKILL)` answers EPERM for — the genuine containment
                // failure. Skipping it here would return `Live(0)`, and the
                // EPERM arm in `UnixProcessGroup::kill` would read that as
                // proof the group was already empty. A live survivor would be
                // recorded as a clean teardown.
                //
                // `liveness()` above already distinguishes NotFound from other
                // errors; this census used to contradict its own module.
                Err(error) => {
                    return ProcessGroupCensus::Indeterminate(format!(
                        "/proc/{name}/stat could not be read ({error}); a census that skips \
                         unreadable entries can only undercount, and undercounting is the \
                         direction that fakes an empty group"
                    ));
                }
            };
            let Some((state, group)) = proc_stat_state_and_pgrp(&stat) else {
                // Skipping an unparseable entry could only ever UNDERcount,
                // and undercounting is the direction that fakes success.
                return ProcessGroupCensus::Indeterminate(format!(
                    "/proc/{name}/stat did not parse as `… ) <state> <ppid> <pgrp> …`"
                ));
            };
            if name.parse::<u32>() == Ok(me) {
                saw_self = true;
            }
            if group == i64::from(pgid) && !proc_stat_state_is_corpse(state) {
                live += 1;
            }
        }

        if !saw_self {
            // Catches a /proc from a foreign PID namespace, or `hidepid=2`
            // hiding everything including us. It does NOT catch hidepid
            // hiding only other users — see the comment above.
            return ProcessGroupCensus::Indeterminate(format!(
                "/proc did not list this process (pid {me}); the enumeration cannot see \
                 itself, so a count of {live} for group {pgid} is not a measurement"
            ));
        }
        ProcessGroupCensus::Live(live)
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
mod platform {
    use super::{ProcessGroupCensus, ProcessLiveness};

    /// `SZOMB` from `<sys/proc.h>`: `SIDL 1, SRUN 2, SSLEEP 3, SSTOP 4,
    /// SZOMB 5`. The `libc` crate does export `libc::SZOMB` for Apple, but as
    /// a `u32`; the byte read out of the kernel buffer below is a `c_char`, so
    /// the comparison is done on the value with its source named.
    const SZOMB: i8 = 5;

    /// Byte offset of `kinfo_proc.kp_proc.p_stat`.
    ///
    /// The `libc` crate does **not** define `kinfo_proc` for Apple targets
    /// (measured: three `E0425`s from
    /// `cargo check -p wcore-types --target aarch64-apple-darwin`, libc
    /// 0.2.186), so the two fields this probe needs are read out of the raw
    /// kernel buffer. The offsets are not guessed — they were printed by
    /// `offsetof` on real hardware; see
    /// `.planning/evidence/zombie-probe/MACOS-PROBE-RESULT.txt`:
    ///
    /// ```text
    /// ABI sizeof(struct kinfo_proc)  = 648
    /// ABI offsetof(kp_proc.p_stat)   = 36
    /// ABI offsetof(kp_proc.p_pid)    = 40
    /// ```
    ///
    /// Both fields sit in the fixed prefix of `struct extern_proc`
    /// (`p_un[16] | p_vmspace(8) | p_sigacts(8) | p_flag(4) | p_stat(1)`),
    /// which is identical on `x86_64` and `aarch64` because both are LP64.
    const P_STAT_OFFSET: usize = 36;
    /// Byte offset of `kinfo_proc.kp_proc.p_pid`. Read back purely as a
    /// self-check on the two offsets above — see [`liveness`].
    const P_PID_OFFSET: usize = 40;

    pub(super) fn liveness(pid: u32) -> ProcessLiveness {
        // macOS has no /proc, and `kill(pid, 0)` returns 0 for a macOS zombie
        // exactly as it does on Linux — measured, ARM A of the C probe above.
        // It is also wrong in the OTHER direction here: `kill(1, 0)` fails
        // with EPERM for launchd, so the old shape reported a live process as
        // dead (ARM D). `sysctl KERN_PROC_PID` is right on both arms.
        //
        // `proc_pidinfo(PROC_PIDTBSDINFO)` was the obvious alternative and is
        // DISQUALIFIED: it fails with EPERM for a live process owned by
        // another user, indistinguishably from its ESRCH failure for a corpse,
        // so "libproc failed => dead" is universal denial.
        let mut mib: [libc::c_int; 4] = [
            libc::CTL_KERN,
            libc::KERN_PROC,
            libc::KERN_PROC_PID,
            pid as libc::c_int,
        ];

        // Ask for the size first rather than hardcoding 648, so a future
        // kernel that grows the struct does not silently truncate.
        let mut needed: libc::size_t = 0;
        // SAFETY: `mib` has exactly the 4 elements declared by `namelen`; a
        // null `oldp` with a live `oldlenp` is the documented size query.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                4,
                std::ptr::null_mut(),
                &mut needed,
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
        if needed == 0 {
            // A successful query that needs zero bytes means "no such
            // process" — this is the shape a fully reaped pid produces
            // (ARM C), and it does NOT come back as ESRCH.
            return ProcessLiveness::Dead;
        }

        let mut buffer = vec![0u8; needed];
        let mut size = needed;
        // SAFETY: `buffer` is `size` bytes long and stays borrowed for the
        // duration of the call; `size` is the in/out length.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                4,
                buffer.as_mut_ptr().cast::<libc::c_void>(),
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
        if size == 0 {
            return ProcessLiveness::Dead;
        }
        if size < P_PID_OFFSET + 4 {
            // Too short to contain the fields — cannot tell, do not guess.
            return ProcessLiveness::Indeterminate;
        }

        // Self-check on the hardcoded offsets: read `p_pid` back and require
        // it to be the pid we asked about. If Apple ever moves these fields,
        // this probe degrades to `Indeterminate` (which `process_is_alive`
        // renders as the conservative "assume alive") instead of silently
        // reading some unrelated byte as a process state.
        let readback = i32::from_ne_bytes([
            buffer[P_PID_OFFSET],
            buffer[P_PID_OFFSET + 1],
            buffer[P_PID_OFFSET + 2],
            buffer[P_PID_OFFSET + 3],
        ]);
        if readback != pid as i32 {
            return ProcessLiveness::Indeterminate;
        }

        if buffer[P_STAT_OFFSET] as i8 == SZOMB {
            ProcessLiveness::Dead
        } else {
            ProcessLiveness::Live
        }
    }

    /// One `sysctl` round trip into a correctly-sized buffer, truncated to the
    /// bytes the kernel actually wrote.
    fn sysctl_proc(mib: &mut [libc::c_int; 4]) -> Result<Vec<u8>, String> {
        let mut needed: libc::size_t = 0;
        // SAFETY: `mib` has exactly the 4 elements declared by `namelen`; a
        // null `oldp` with a live `oldlenp` is the documented size query.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                4,
                std::ptr::null_mut(),
                &mut needed,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(Vec::new());
            }
            return Err(format!("sysctl size query failed: {error}"));
        }
        if needed == 0 {
            return Ok(Vec::new());
        }
        let mut buffer = vec![0u8; needed];
        let mut size = needed;
        // SAFETY: `buffer` is `size` bytes long and stays borrowed for the
        // duration of the call; `size` is the in/out length.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                4,
                buffer.as_mut_ptr().cast::<libc::c_void>(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(Vec::new());
            }
            return Err(format!("sysctl data query failed: {error}"));
        }
        buffer.truncate(size);
        Ok(buffer)
    }

    /// `sizeof(struct kinfo_proc)`, measured from the running kernel.
    ///
    /// It has to be measured because `libc` does not define `kinfo_proc` for
    /// Apple targets (see [`P_STAT_OFFSET`]), and it must NOT be taken from
    /// the null-buffer size query: **XNU deliberately over-reports that
    /// number**, so a process forked during the call cannot overflow the
    /// caller's buffer. Measured on this hardware (macOS 15, aarch64):
    ///
    /// ```text
    /// KERN_PROC_PID size query (null oldp) = 3888   <- 6x, NOT a struct size
    /// KERN_PROC_PID data query (real oldp) =  648   <- sizeof(kinfo_proc)
    /// KERN_PROC_PGRP data query, 5 members = 3240 = 5 * 648
    /// ```
    ///
    /// Using 3888 as a stride would read one entry out of every six and
    /// silently undercount a group by 83% — an undercount is exactly the
    /// direction that fakes a successful containment proof.
    ///
    /// A `KERN_PROC_PID` query for a pid that exists writes exactly ONE
    /// struct, and for a non-null buffer the kernel reports back the bytes it
    /// actually wrote — so that number IS the stride.
    fn kinfo_proc_stride() -> Result<usize, String> {
        let me = std::process::id();
        let mut mib: [libc::c_int; 4] = [
            libc::CTL_KERN,
            libc::KERN_PROC,
            libc::KERN_PROC_PID,
            me as libc::c_int,
        ];
        let buffer = sysctl_proc(&mut mib)?;
        if buffer.len() < P_PID_OFFSET + 4 {
            return Err(format!(
                "KERN_PROC_PID for this very process (pid {me}) returned {} bytes, too few to \
                 hold a kinfo_proc; an instrument that cannot see itself cannot be trusted to \
                 see a survivor",
                buffer.len()
            ));
        }
        // Self-check the hardcoded offsets against real data BEFORE the length
        // is trusted as a stride. If Apple moves these fields, every caller
        // degrades to `Indeterminate` rather than reading an unrelated byte as
        // a process state.
        let readback = i32::from_ne_bytes([
            buffer[P_PID_OFFSET],
            buffer[P_PID_OFFSET + 1],
            buffer[P_PID_OFFSET + 2],
            buffer[P_PID_OFFSET + 3],
        ]);
        if readback != me as i32 {
            return Err(format!(
                "kinfo_proc p_pid offset self-check failed: asked the kernel for pid {me}, read \
                 back {readback} at offset {P_PID_OFFSET}"
            ));
        }
        Ok(buffer.len())
    }

    pub(super) fn group_census(pgid: u32) -> ProcessGroupCensus {
        let stride = match kinfo_proc_stride() {
            Ok(stride) => stride,
            Err(why) => return ProcessGroupCensus::Indeterminate(why),
        };
        let mut mib: [libc::c_int; 4] = [
            libc::CTL_KERN,
            libc::KERN_PROC,
            libc::KERN_PROC_PGRP,
            pgid as libc::c_int,
        ];
        let buffer = match sysctl_proc(&mut mib) {
            Ok(buffer) => buffer,
            Err(why) => return ProcessGroupCensus::Indeterminate(why),
        };
        if buffer.is_empty() {
            // No such group at all: every member has been reaped.
            return ProcessGroupCensus::Live(0);
        }
        if buffer.len() % stride != 0 {
            return ProcessGroupCensus::Indeterminate(format!(
                "KERN_PROC_PGRP for group {pgid} returned {} bytes, not a whole multiple of the \
                 {stride}-byte kinfo_proc measured from this process",
                buffer.len()
            ));
        }

        let mut live = 0usize;
        for entry in buffer.chunks_exact(stride) {
            let pid = i32::from_ne_bytes([
                entry[P_PID_OFFSET],
                entry[P_PID_OFFSET + 1],
                entry[P_PID_OFFSET + 2],
                entry[P_PID_OFFSET + 3],
            ]);
            if pid <= 0 {
                // Per-entry restatement of the offset self-check: a valid row
                // always carries a positive pid, so a non-positive one means
                // the stride or the layout is wrong and the count below it is
                // meaningless.
                return ProcessGroupCensus::Indeterminate(format!(
                    "a KERN_PROC_PGRP entry for group {pgid} carried a non-positive pid ({pid}); \
                     the kinfo_proc layout this census depends on has moved"
                ));
            }
            if entry[P_STAT_OFFSET] as i8 != SZOMB {
                live += 1;
            }
        }
        ProcessGroupCensus::Live(live)
    }
}

// ---------------------------------------------------------------------------
// Other unix (FreeBSD, illumos, …) — honest about what it cannot see
// ---------------------------------------------------------------------------
#[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
mod platform {
    use super::{ProcessGroupCensus, ProcessLiveness};

    pub(super) fn group_census(pgid: u32) -> ProcessGroupCensus {
        // There is no zombie-aware enumeration for this target, and a census
        // that cannot separate a corpse from a survivor cannot answer the only
        // question callers ask it ("is this group empty?"). Say so. The
        // alternative — falling back to `kill(-pgid, 0)` — is precisely the
        // non-discriminating probe this function exists to replace.
        ProcessGroupCensus::Indeterminate(format!(
            "no process-group census is implemented for target_os=\"{}\", so group {pgid} \
             cannot be enumerated",
            std::env::consts::OS
        ))
    }

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

    // -- the group census ---------------------------------------------------

    #[test]
    fn stat_pgrp_is_read_positionally_after_the_last_paren() {
        // `<pid> (<comm>) <state> <ppid> <pgrp> …`
        assert_eq!(
            proc_stat_state_and_pgrp("1234 (sleep) S 1 4321 4321 0 -1"),
            Some(('S', 4321))
        );
        assert_eq!(
            proc_stat_state_and_pgrp("1234 (weird ) name) Z 1 777 777 0"),
            Some(('Z', 777)),
            "a ')' inside comm must not shift the pgrp field"
        );
        // The state and the pgrp must come from the SAME parse. If these two
        // ever disagree, one of the two readers is off by a field.
        let line = "9 (a) b) c) R 3 555 555 0";
        assert_eq!(proc_stat_state_and_pgrp(line).map(|(s, _)| s), Some('R'));
        assert_eq!(proc_stat_state_char(line), Some('R'));
    }

    #[test]
    fn a_stat_line_too_short_to_hold_a_pgrp_yields_none() {
        // Truncation must not be read as "pgrp 0", which would silently make
        // every process a member of group 0.
        assert_eq!(proc_stat_state_and_pgrp("1234 (sleep) S 1"), None);
        assert_eq!(proc_stat_state_and_pgrp("1234 (sleep) S"), None);
        assert_eq!(proc_stat_state_and_pgrp("no parens"), None);
        assert_eq!(
            proc_stat_state_and_pgrp("1234 (sleep) S 1 notanumber"),
            None,
            "a non-numeric pgrp is a parse failure, not a default"
        );
    }

    #[cfg(unix)]
    #[test]
    fn our_own_process_group_contains_at_least_us() {
        // THE positive control for the whole census. Without it, an
        // implementation that answered `Live(0)` for every group would satisfy
        // every containment proof in the workspace — which is exactly the
        // false-success this module exists to prevent.
        //
        // SAFETY: `getpgrp` takes no arguments and only reads the caller's own
        // process group.
        let mine = unsafe { libc::getpgrp() };
        assert!(mine > 0, "getpgrp returned {mine}");
        match process_group_census(mine as u32) {
            ProcessGroupCensus::Live(n) => assert!(
                n >= 1,
                "the census reported {n} live members of our OWN process group ({mine}); this \
                 test process is one of them, so any count below 1 means the census cannot see"
            ),
            ProcessGroupCensus::Indeterminate(why) => panic!(
                "the census could not enumerate our own process group ({mine}) on this \
                 supported platform: {why}"
            ),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_group_whose_every_member_is_gone_censuses_as_empty() {
        // The negative direction. A census that answered `Indeterminate` for
        // everything would never let a containment proof succeed, wedging
        // every clean shutdown — the opposite failure, equally invisible in a
        // bool.
        //
        // The group is made genuinely empty rather than guessed at. An earlier
        // version picked the "unused" id 4_000_000, which is BELOW Linux's
        // default pid_max of 4194304 — on a high-churn host that id can name a
        // real group, and the test would flake red for a correct census.
        use std::os::unix::process::CommandExt;

        let mut command = std::process::Command::new("true");
        command.process_group(0);
        let mut leader = command.spawn().expect("spawn group leader");
        let pgid = leader.id();
        leader.wait().expect("reap group leader");

        match process_group_census(pgid) {
            ProcessGroupCensus::Live(0) => {}
            other => {
                panic!("a group whose only member was reaped must census as Live(0), got {other:?}")
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn group_zero_is_refused_because_it_means_the_caller() {
        // `kill(0, …)` addresses the caller's own group. A census that
        // accepted 0 would answer a question nobody asked.
        assert!(matches!(
            process_group_census(0),
            ProcessGroupCensus::Indeterminate(_)
        ));
    }

    #[test]
    fn pid_zero_is_never_alive() {
        // `kill(0, 0)` addresses the caller's own process group and would
        // report success; pid 0 must be refused before it reaches the probe.
        assert_eq!(process_liveness(0), ProcessLiveness::Dead);
        assert!(!process_is_alive(0));
    }
}
