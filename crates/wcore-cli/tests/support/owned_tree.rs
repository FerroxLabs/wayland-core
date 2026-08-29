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
//! ## Why Windows is a Job Object and not a parent-pid walk
//!
//! Windows has no process group and no `/proc`. It also does not reparent an
//! orphan, so a parent-pid walk is *possible* there — but it is the weaker
//! mechanism: it races pid reuse, it can only ever kill what it managed to
//! observe, and a descendant created between the snapshot and the kill is
//! missed. The kernel-backed equivalent of `kill(-pgid)` is a Job Object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, which a descendant cannot leave the
//! way a Unix child can `setsid` out of its group. The workspace already owns
//! exactly that primitive — [`wcore_types::job_object::WindowsJobObject`],
//! used by `wcore-mcp`'s stdio transport and `wcore-sandbox`'s
//! `ProcessTreeGuard` — so this guard uses it rather than growing a second
//! mechanism (FerroxLabs/wayland-core#358).
//!
//! The two arms are therefore NOT the same shape, and the fields say so:
//! `known` (the snapshotted descendant pid list) is a Unix-only concept and is
//! always empty on Windows, where the job — not a list — is what owns the
//! tree.
//!
//! What the Windows arm still cannot promise: [`OwnedTree::new`] is handed a
//! process that is ALREADY RUNNING, so it assigns it to the job after the
//! fact. Descendants created in the microseconds between `CreateProcess`
//! returning and the assignment landing are outside the job forever. See
//! [`wcore_types::job_object::WindowsJobObject::attach_running`] for the
//! precise statement of that window and the constructor that closes it.
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

/// Every descendant of `root`, breadth-first. Bounded by a visited set so a
/// pid that is somehow its own ancestor cannot loop forever.
///
/// Unix-only, and deliberately not stubbed out on Windows: a `Vec::new()`
/// Windows arm is a walk that reports no descendants for a tree that has
/// them, which is how the guard came to look present there while owning
/// nothing. The Windows arm owns the tree through a Job Object instead — see
/// the module docs.
#[cfg(unix)]
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

/// SIGKILL every pid in `known`.
///
/// Windows has no arm here and does not need one: `known` is always empty
/// there (see the field docs), and the job — not this list — is what kills the
/// descendants. The `debug_assert` is the guard against that invariant
/// quietly changing and this becoming a silent no-op over a non-empty list.
#[cfg(unix)]
fn kill_all(known: &[u32]) {
    for pid in known {
        sigkill(*pid);
    }
}

#[cfg(windows)]
fn kill_all(known: &[u32]) {
    debug_assert!(
        known.is_empty(),
        "the descendant pid list must stay empty on Windows — the Job Object \
         owns the tree there, and a non-empty list means some walk is filling \
         it that nothing acts on"
    );
}

/// Put `pid` in a fresh kill-on-close Job Object, or fail the test loudly.
///
/// There is deliberately no fallback. The Linux arm has none either: it reads
/// `/proc` and refuses to degrade to `pgrep`. A Windows arm that swallowed
/// this error would hand back a guard that owns the leaf and silently leaks
/// every descendant — the exact state FerroxLabs/wayland-core#358 was filed
/// about, and one that no assertion anywhere can see.
#[cfg(windows)]
fn own_windows_tree(pid: u32) -> wcore_types::job_object::WindowsJobObject {
    wcore_types::job_object::WindowsJobObject::attach_running(pid).unwrap_or_else(|error| {
        panic!(
            "could not assign child pid {pid} to a kill-on-close Job Object ({error}); \
             on Windows that job is the ONLY thing this guard has that reaches a \
             descendant, so continuing would leak the process TREE \
             (FerroxLabs/wayland-core#358)"
        )
    })
}

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

/// `tokio::process::Child`.
///
/// The ~40 sites this guard was extended to cover (FerroxLabs/wayland-core#352)
/// are split between `std::process::Command` and `tokio::process::Command`, and
/// the tokio half is where the ad-hoc leaf-only `Drop` the ticket names actually
/// lives: `.kill_on_drop(true)` kills the DIRECT child when the handle drops and
/// the runtime reaps it, which is exactly the leaf-only ownership #1156 measured
/// as insufficient. Wrapping the handle keeps that leaf behaviour and adds the
/// descendant walk on top of it.
///
/// `kill()` and `wait()` are `async` on this type and `Drop` is not, so the
/// synchronous halves are used: `start_kill` delivers the signal without
/// awaiting the exit, and `try_wait` reaps it once it has gone. Anything still
/// unreaped is collected by the runtime orphan queue when the inner handle drops
/// immediately afterwards.
impl Reapable for tokio::process::Child {
    fn pid(&self) -> Option<u32> {
        self.id()
    }
    fn kill_direct(&mut self) {
        let _ = self.start_kill();
    }
    fn wait_direct(&mut self) {
        let _ = self.try_wait();
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
    /// `None` only after [`OwnedTree::wait_with_output`] has moved the handle
    /// out to consume it. Every other path keeps it populated for `Drop`.
    child: Option<C>,
    /// Descendants observed at any point in this guard's life. Snapshotted
    /// BEFORE the direct child is killed, because a descendant reparents to
    /// init the instant its parent dies and the parent link is then gone.
    ///
    /// Unix only. On Windows it is constructed empty and never written to:
    /// the job below is what owns the tree there, and a pid list that is
    /// always empty must not be mistaken for one that found nothing.
    known: Vec<u32>,
    /// The kill-on-close Job Object holding the direct child and every
    /// descendant it goes on to create.
    ///
    /// `None` only when the handle had no pid to assign — a process that had
    /// already exited and been reaped before the guard was built, which owns
    /// nothing by definition. Any other failure to create the job PANICS in
    /// [`OwnedTree::new`] rather than degrading quietly.
    #[cfg(windows)]
    job: Option<wcore_types::job_object::WindowsJobObject>,
}

impl<C: Reapable> OwnedTree<C> {
    /// Take ownership of an already-spawned process.
    ///
    /// On Windows this is also where the tree becomes owned: the child is
    /// assigned to a fresh kill-on-close Job Object, so every process it
    /// creates from here on is inside that job and dies with it. See the
    /// module docs for the one window this cannot close.
    pub fn new(child: C) -> Self {
        #[cfg(windows)]
        let job = child.pid().map(own_windows_tree);
        Self {
            child: Some(child),
            known: Vec::new(),
            #[cfg(windows)]
            job,
        }
    }

    /// The Job Object owning this guard's tree.
    ///
    /// Exposed so an ownership test can ask the KERNEL whether a descendant
    /// actually landed inside the job before it kills anything — without it,
    /// the only available evidence that the job contains the grandchild is the
    /// grandchild dying, which is the claim under test and therefore not
    /// evidence.
    #[cfg(windows)]
    pub fn job(&self) -> Option<&wcore_types::job_object::WindowsJobObject> {
        self.job.as_ref()
    }

    /// The direct child's pid.
    ///
    /// # Panics
    /// If the process has no pid.
    pub fn id(&self) -> u32 {
        self.child
            .as_ref()
            .and_then(Reapable::pid)
            .expect("owned process has no pid")
    }

    /// The direct child, for callers that need the real handle
    /// (`stderr.take()`, `try_wait`, ...).
    ///
    /// [`Deref`](std::ops::Deref) reaches the same handle, so most call sites
    /// need neither this nor any other edit beyond the wrap itself. It is kept
    /// for the sites that say `child_mut()` explicitly.
    ///
    /// # Panics
    /// After `wait_with_output` has consumed the guard, which cannot be
    /// observed: that method takes `self` by value.
    pub fn child_mut(&mut self) -> &mut C {
        self.child.as_mut().expect("the child was already consumed")
    }

    /// Record the current descendant set, unioned with everything already
    /// recorded. Called before every kill so the list survives the moment the
    /// parent link disappears.
    #[cfg(unix)]
    pub fn snapshot(&mut self) {
        // RED ARM for FerroxLabs/wayland-core#352 c5, macOS arm. This branch is a
        // throwaway instrument, never merged: the guard is reduced to owning the
        // LEAF only, which is what every swept site did before #1156 and what the
        // Windows arm still does. `black_box` keeps the compiler from folding the
        // condition, so the rest of the body is not `unreachable_code` and clippy
        // -D warnings still passes -- the previous macOS red-arm attempt died on
        // an unrelated lint before it ever reached the tests.
        if std::hint::black_box(true) {
            return;
        }
        let Some(pid) = self.child.as_ref().and_then(Reapable::pid) else {
            return;
        };
        for kid in descendants(pid) {
            if !self.known.contains(&kid) {
                self.known.push(kid);
            }
        }
    }

    /// No-op on Windows, and not a degradation: the job already holds every
    /// descendant, so there is nothing to record and no moment at which the
    /// membership could be lost. Kept as a method so the call sites below read
    /// the same on both platforms.
    #[cfg(windows)]
    pub fn snapshot(&mut self) {}

    /// Kill and reap the DIRECT child only, leaving its descendants running.
    ///
    /// For the one test that deliberately observes whether a child dies with
    /// its killed parent. The descendants are still snapshotted, so `Drop`
    /// remains a real safety net for them even though this call spares them.
    pub fn kill_direct_only(&mut self) {
        self.snapshot();
        if let Some(child) = self.child.as_mut() {
            child.kill_direct();
            child.wait_direct();
        }
    }

    /// Kill the whole tree and reap the direct child. Idempotent.
    pub fn reap(&mut self) {
        // Snapshot first: after the parent dies its children reparent to init
        // and no parent-pid walk can find them again.
        self.snapshot();
        // Parent first, so it cannot spawn anything else while we work.
        if let Some(child) = self.child.as_mut() {
            child.kill_direct();
        }
        kill_all(&self.known);
        // Windows: the job is what reaches the descendants, and it reaches the
        // ones no walk could have seen. Idempotent, so a second `reap` and the
        // `Drop` that follows are both harmless.
        #[cfg(windows)]
        if let Some(job) = self.job.as_ref() {
            job.terminate();
        }
        // Reap, so the direct child does not linger as a zombie.
        if let Some(child) = self.child.as_mut() {
            child.wait_direct();
        }
    }

    /// Take the handle out, recording its descendants first.
    ///
    /// The one way the guard stops owning the direct child: the `*_with_output`
    /// helpers below have to pass it by value. The descendant list is kept and
    /// killed by the caller, so the TREE is still owned across the wait.
    fn take_for_output(&mut self) -> (C, Vec<u32>) {
        self.snapshot();
        let child = self.child.take().expect("the child was already consumed");
        (child, std::mem::take(&mut self.known))
    }
}

/// Reach the wrapped handle transparently.
///
/// Deliberate, and the reason the #352 sweep is a wrap rather than a rewrite of
/// 44 call sequences: `guard.stdin`, `guard.stdout.take()`, `guard.try_wait()`
/// and `guard.wait()` all keep working verbatim, so the diff at each site is the
/// spawn expression and nothing else. A sweep that had to restate every use of
/// every child would not have been done, which is how these sites got here.
///
/// `id()` is the one name that shadows: the inherent method wins, and it returns
/// the same pid the handle would.
impl<C: Reapable> std::ops::Deref for OwnedTree<C> {
    type Target = C;
    fn deref(&self) -> &C {
        self.child.as_ref().expect("the child was already consumed")
    }
}

impl<C: Reapable> std::ops::DerefMut for OwnedTree<C> {
    fn deref_mut(&mut self) -> &mut C {
        self.child.as_mut().expect("the child was already consumed")
    }
}

impl OwnedTree<Child> {
    /// `Child::wait_with_output`, with the tree still owned.
    ///
    /// Many sites end in this call, which takes the handle BY VALUE and so
    /// cannot go through `Deref`. Waiting for a natural exit is not ownership on
    /// its own: the descendants are what outlive the test, and after the direct
    /// child exits its parent links are gone, so they are snapshotted BEFORE the
    /// wait and killed after it.
    pub fn wait_with_output(mut self) -> std::io::Result<std::process::Output> {
        let (child, known) = self.take_for_output();
        let out = child.wait_with_output();
        kill_all(&known);
        out
    }
}

impl OwnedTree<tokio::process::Child> {
    /// `tokio::process::Child::wait_with_output`, with the tree still owned.
    /// The async twin of the method above; same reasoning.
    pub async fn wait_with_output(mut self) -> std::io::Result<std::process::Output> {
        let (child, known) = self.take_for_output();
        let out = child.wait_with_output().await;
        kill_all(&known);
        out
    }
}

impl<C: Reapable> Drop for OwnedTree<C> {
    fn drop(&mut self) {
        self.reap();
    }
}
