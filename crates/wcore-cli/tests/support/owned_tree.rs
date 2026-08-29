//! Harness OWNERSHIP of a spawned process TREE — FerroxLabs/wayland#1156.
//!
//! Nine `wayland-core acp serve` processes were found alive on one build host
//! long after the tests that started them had finished; most had `PPID 1`, the
//! oldest was 24 hours old, they still held listening loopback ports and they
//! pinned 160 GB of target directories. Two of them were surviving CHILDREN of
//! surviving servers, so a whole process TREE outlived its test, not a leaf.
//!
//! The product half of that (a supervisor-spawned `acp serve --profile` child
//! now dies with its supervisor through `wcore_cli::parent_channel`) shipped in
//! v0.13.10. This module is the OTHER half the ticket asked for: the test
//! supervisor owning what it spawned.
//!
//! ## Why a `Drop` guard and not a trailing kill
//!
//! Every leaking site ended its test body with `let _ = child.kill();`. That
//! line runs on exactly one exit path — a normal return. An assertion failure,
//! a `panic!`, a `#[should_panic]` body, an early `return`, or a `?` all skip
//! it, the test binary exits, and the server reparents to init. That is not
//! ownership; it is a hope. `OwnedTree` moves the kill into `Drop`, which runs
//! while unwinding, so the only way to leak is to `std::mem::forget` it.
//!
//! ## Why the tree and not just the leaf
//!
//! The profile router spawns each child with `process_group(0)` (setsid), so
//! the child gets its OWN process group and a `kill(-pgid)` aimed at the
//! supervisor's group never reaches it. Descendants are therefore enumerated
//! explicitly and killed by pid.
//!
//! ## Why this does not shell out to `pgrep` on Linux
//!
//! It must not. The `CI (linux-containerized)` job's image ships without
//! procps, so `Command::new("pgrep")` fails there with
//! `Os { code: 2, kind: NotFound }`. Linux exposes the parent of every process
//! in `/proc/<pid>/status`, which needs no external binary. There is
//! deliberately NO fallback to `pgrep` on Linux: a silent fallback would let
//! the procps dependency creep back in unnoticed, which is how it got here.

#![allow(dead_code)] // Shared module: not every test binary uses every helper.

use std::process::Child;

/// Direct child pids of `parent`, read straight out of `/proc`.
///
/// A process can exit between the `readdir` and the `status` read; that is not
/// an error, it just means it is not a child any more. `PPid:` is on its own
/// line, so this cannot be confused by a `comm` containing spaces or
/// parentheses the way parsing `/proc/<pid>/stat` can.
#[cfg(target_os = "linux")]
pub fn child_pids(parent: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue; // /proc also holds non-numeric entries
        };
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("PPid:")
                && rest.trim().parse::<u32>() == Ok(parent)
            {
                out.push(pid);
                break;
            }
        }
    }
    out
}

/// macOS and other Unixes have no `/proc`. `pgrep` is part of the base system
/// there, so it is a safe dependency in a way it is not inside a minimal Linux
/// container image.
#[cfg(all(unix, not(target_os = "linux")))]
pub fn child_pids(parent: u32) -> Vec<u32> {
    let Ok(out) = std::process::Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect()
}

/// Windows has no cheap parent-pid walk without a `windows`/`sysinfo`
/// dependency this crate does not carry, so the guard degrades to killing the
/// direct child only — exactly what the leaking sites already did there. The
/// leak this module closes was measured on Linux; the Windows tree case is
/// NOT covered here and is deliberately left visible rather than faked.
#[cfg(windows)]
pub fn child_pids(_parent: u32) -> Vec<u32> {
    Vec::new()
}

/// Every descendant of `root`, breadth-first. Bounded by a visited set so a
/// pid that is somehow its own ancestor cannot loop forever.
pub fn descendants(root: u32) -> Vec<u32> {
    let mut seen = vec![root];
    let mut queue = vec![root];
    let mut out = Vec::new();
    while let Some(pid) = queue.pop() {
        for kid in child_pids(pid) {
            if seen.contains(&kid) {
                continue;
            }
            seen.push(kid);
            queue.push(kid);
            out.push(kid);
        }
    }
    out
}

/// SIGKILL one pid, ignoring "already gone".
#[cfg(unix)]
fn sigkill(pid: u32) {
    // SAFETY: `kill` takes no pointers and delivers nothing but the signal
    // number; the pid is one this harness spawned or descends from one.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
}

/// Never reached in practice: `child_pids` is empty on Windows, so the
/// descendant list this would be called over is always empty there.
#[cfg(windows)]
fn sigkill(_pid: u32) {}

/// A spawned process this harness can kill and reap.
///
/// Implemented for `std::process::Child` and (on Unix) for the boxed
/// `portable_pty` child, so one guard covers both spawn surfaces.
pub trait Reapable {
    /// The pid, if the process has one.
    fn pid(&self) -> Option<u32>;
    /// Best-effort kill. Already-exited is not an error.
    fn kill_direct(&mut self);
    /// Reap, so the process cannot linger as a zombie.
    fn wait_direct(&mut self);
}

impl Reapable for Child {
    fn pid(&self) -> Option<u32> {
        Some(self.id())
    }
    fn kill_direct(&mut self) {
        let _ = Child::kill(self);
    }
    fn wait_direct(&mut self) {
        let _ = Child::wait(self);
    }
}

#[cfg(unix)]
impl Reapable for Box<dyn portable_pty::Child + Send + Sync> {
    fn pid(&self) -> Option<u32> {
        self.process_id()
    }
    fn kill_direct(&mut self) {
        let _ = (**self).kill();
    }
    fn wait_direct(&mut self) {
        let _ = (**self).wait();
    }
}

/// RAII ownership of a spawned process and everything it spawned.
///
/// Dropping it — on a normal return, an early return, an assertion failure or
/// any other panic — kills the whole tree and reaps the direct child.
pub struct OwnedTree<C: Reapable> {
    child: C,
    /// Descendants observed at any point in this guard's life. Snapshotted
    /// BEFORE the direct child is killed, because a descendant reparents to
    /// init the instant its parent dies and the parent link is then gone.
    known: Vec<u32>,
}

impl<C: Reapable> OwnedTree<C> {
    /// Take ownership of an already-spawned process.
    pub fn new(child: C) -> Self {
        Self {
            child,
            known: Vec::new(),
        }
    }

    /// The direct child's pid.
    ///
    /// # Panics
    /// If the process has no pid.
    pub fn id(&self) -> u32 {
        self.child.pid().expect("owned process has no pid")
    }

    /// The direct child, for callers that need the real handle
    /// (`stderr.take()`, `try_wait`, ...).
    pub fn child_mut(&mut self) -> &mut C {
        &mut self.child
    }

    /// Record the current descendant set, unioned with everything already
    /// recorded. Called before every kill so the list survives the moment the
    /// parent link disappears.
    pub fn snapshot(&mut self) {
        let Some(pid) = self.child.pid() else { return };
        for kid in descendants(pid) {
            if !self.known.contains(&kid) {
                self.known.push(kid);
            }
        }
    }

    /// Kill and reap the DIRECT child only, leaving its descendants running.
    ///
    /// For the one test that deliberately observes whether a child dies with
    /// its killed parent. The descendants are still snapshotted, so `Drop`
    /// remains a real safety net for them even though this call spares them.
    pub fn kill_direct_only(&mut self) {
        self.snapshot();
        self.child.kill_direct();
        self.child.wait_direct();
    }

    /// Kill the whole tree and reap the direct child. Idempotent.
    pub fn reap(&mut self) {
        // Snapshot first: after the parent dies its children reparent to init
        // and no parent-pid walk can find them again.
        self.snapshot();
        // Parent first, so it cannot spawn anything else while we work.
        self.child.kill_direct();
        for pid in &self.known {
            sigkill(*pid);
        }
        // Reap, so the direct child does not linger as a zombie.
        self.child.wait_direct();
    }
}

impl<C: Reapable> Drop for OwnedTree<C> {
    fn drop(&mut self) {
        self.reap();
    }
}
