//! The parent-death channel that binds a supervisor-spawned `acp serve` child
//! to the supervisor process (FerroxLabs/wayland#1156).
//!
//! # The defect this closes
//!
//! [`crate::profile_router::CliProfileRouter`] spawns each isolated profile as
//! its own `acp serve --profile <name>` process with `process_group(0)` AND
//! `Stdio::null()` for stdin. Those two together leave the child with NO handle
//! that closes when the supervisor dies: the new process group detaches it from
//! every signal the supervisor's group receives, and a null stdin never reports
//! EOF. The supervisor's own reaping — its signal handler, its `Drop`, its
//! per-session reap — covers only the exits it gets to observe. A SIGKILLed,
//! panicking or OOM-killed supervisor observes nothing, and the child then runs
//! forever, reparented to PPID 1, still bound to its loopback port and still
//! holding that profile's credentials. Nine such orphans were measured on one
//! host, the oldest 24 hours old.
//!
//! # The mechanism
//!
//! Hand the child the read end of an anonymous pipe as its stdin and keep the
//! write end in the supervisor for exactly the child's registered lifetime. The
//! kernel closes that write end when the supervisor exits by ANY means, which
//! is the property no supervisor-side cleanup can have. The child parks a
//! thread on the handle and leaves only when
//! [`wcore_sandbox::backends::process_tree::sentinel_park_decision_io`] says the
//! channel is really gone — the same fail-closed rule the sandbox's
//! process-group sentinels park on, so an interrupted read cannot pass for an
//! orphaning and shut a healthy server down.
//!
//! An inherited handle is closed on process death on Unix and Windows alike, so
//! the channel itself is portable; only the shutdown it triggers is per-platform.

use std::ffi::OsStr;
use std::io::Read;
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::time::Duration;

use wcore_sandbox::backends::process_tree::{SentinelPark, sentinel_park_decision_io};

/// Env var a supervisor sets on the child naming where the parent channel
/// arrived. `stdin` is the only defined value; its absence means this process
/// was NOT supervised and must keep ordinary stdin semantics — a hand-run
/// `wayland-core acp serve` must not exit because a terminal sent EOF.
pub const PARENT_CHANNEL_ENV: &str = "WAYLAND_ACP_PARENT_CHANNEL";

/// The only defined value of [`PARENT_CHANNEL_ENV`].
const STDIN: &str = "stdin";

/// How long the orphaned child gives its own tree to stop before killing it.
#[cfg(unix)]
const ORPHAN_GRACE: Duration = Duration::from_secs(5);

/// Exit code of a child that outlived its supervisor: 128 + SIGTERM, matching
/// the signal it raises on itself on Unix.
const ORPHANED_EXIT_CODE: i32 = 143;

/// Wire a parent-death channel into `cmd` and return the supervisor's end.
///
/// KEEP THE RETURNED WRITER ALIVE for as long as the child should live —
/// dropping it closes the channel, which is precisely the signal the child
/// exits on. The supervisor never writes to it; the kernel closing it on
/// supervisor death is the entire message.
pub fn attach(cmd: &mut Command) -> std::io::Result<std::io::PipeWriter> {
    let (child_end, supervisor_end) = std::io::pipe()?;
    cmd.stdin(Stdio::from(child_end))
        .env(PARENT_CHANNEL_ENV, STDIN);
    Ok(supervisor_end)
}

/// Whether `value` names a parent channel this process must park on.
fn is_parent_channel(value: Option<&OsStr>) -> bool {
    value.is_some_and(|v| v == OsStr::new(STDIN))
}

/// Park a background thread on the inherited parent channel, if there is one.
///
/// A no-op for an unsupervised process, so this is safe to call unconditionally
/// at `acp serve` startup.
pub fn watch_for_orphaning() {
    if !is_parent_channel(std::env::var_os(PARENT_CHANNEL_ENV).as_deref()) {
        return;
    }
    let parked = std::thread::Builder::new()
        .name("parent-channel".to_string())
        .spawn(|| {
            park_until_closed(std::io::stdin());
            eprintln!(
                "wayland-core acp: the supervisor channel closed — this profile child has been \
                 orphaned. Shutting down rather than outliving the supervisor that owns it."
            );
            exit_orphaned();
        });
    if let Err(e) = parked {
        // Say so on stderr, not through `warn!`: with RUST_LOG unset only ERROR
        // reaches an operator, and without this thread the process IS the orphan
        // #1156 reports.
        eprintln!(
            "wayland-core acp: could not park on the supervisor channel ({e}); this process \
             would outlive its supervisor instead of exiting with it."
        );
    }
}

/// Block until the parent channel is really gone.
///
/// Split out from [`watch_for_orphaning`] so the park loop is testable against
/// an ordinary pipe with no process involved.
pub fn park_until_closed<R: Read>(mut channel: R) {
    // Nothing is ever written to this channel in production; the buffer exists
    // so a stray byte is consumed rather than re-waking the park forever.
    let mut scratch = [0u8; 64];
    loop {
        if sentinel_park_decision_io(&channel.read(&mut scratch)) == SentinelPark::Release {
            return;
        }
    }
}

/// Take this orphaned process — and the tree it started — down.
#[cfg(unix)]
fn exit_orphaned() -> ! {
    // The supervisor spawns us with `process_group(0)` (setsid), so our process
    // group holds exactly our own tree: signalling it stops this server AND the
    // MCP servers and tool processes it started, in one act. Signalling group 0
    // while we are NOT its leader would reach processes that are not ours (a
    // hand-run server shares its shell's group), so that case signals only self.
    //
    // SAFETY: `getpgrp`, `getpid` and `kill` take no pointers, and the pid
    // arguments are this process's own or the group-self sentinel 0.
    let leads_own_group = unsafe { libc::getpgrp() } == unsafe { libc::getpid() };
    let target = if leads_own_group {
        0
    } else {
        unsafe { libc::getpid() }
    };
    // SIGTERM first, and it is delivered to us too, so `run_until_shutdown`'s
    // handler runs the orderly shutdown that reaps what the process tracks.
    unsafe { libc::kill(target, libc::SIGTERM) };
    std::thread::sleep(ORPHAN_GRACE);
    // Still alive: the orderly path did not finish, and there is nothing left
    // to be graceful about — an orphan that lingers is the whole defect.
    unsafe { libc::kill(target, libc::SIGKILL) };
    std::process::exit(ORPHANED_EXIT_CODE);
}

/// Windows (and any other non-Unix host) has no process group to signal.
/// Exiting closes every handle this process holds, including the kill-on-close
/// Job Objects that own the child processes it started
/// (`wcore_types::job_object`), so the tree goes down with it.
#[cfg(not(unix))]
fn exit_orphaned() -> ! {
    std::process::exit(ORPHANED_EXIT_CODE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;
    use std::time::Duration;

    /// How long a park is watched before it is called "still parked". Long
    /// enough that a released park would have reported, short enough to keep
    /// the test cheap.
    const SETTLE: Duration = Duration::from_millis(200);
    /// Budget for an event that must happen promptly once the channel closes.
    const BUDGET: Duration = Duration::from_secs(10);

    /// Park on a channel on another thread; the receiver reports the release.
    fn park_in_background<R: Read + Send + 'static>(channel: R) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            park_until_closed(channel);
            let _ = tx.send(());
        });
        rx
    }

    /// Both halves of the contract in one test: the park must NOT release while
    /// the supervisor end is open (a release there kills a healthy server), and
    /// it MUST release once that end is dropped (no release is the orphan).
    #[test]
    fn the_park_releases_only_when_the_supervisor_end_is_dropped() {
        let (child_end, supervisor_end) = std::io::pipe().expect("pipe");
        let released = park_in_background(child_end);

        std::thread::sleep(SETTLE);
        assert!(
            released.try_recv().is_err(),
            "the park released while the supervisor still held its end of the channel"
        );

        drop(supervisor_end);
        released
            .recv_timeout(BUDGET)
            .expect("the park must release once the supervisor end is closed");
    }

    /// A wakeup carrying data is not the channel closing. Nothing writes to this
    /// channel in production, so a park that exited on one would be an orphan
    /// killer triggered by noise.
    #[test]
    fn a_byte_on_the_channel_is_not_the_channel_closing() {
        let (child_end, mut supervisor_end) = std::io::pipe().expect("pipe");
        let released = park_in_background(child_end);

        supervisor_end.write_all(b"x").expect("write one byte");
        std::thread::sleep(SETTLE);
        assert!(
            released.try_recv().is_err(),
            "a byte on the channel released the park; only EOF may"
        );

        drop(supervisor_end);
        released
            .recv_timeout(BUDGET)
            .expect("the park must still release on the real close");
    }

    /// The gate is exact. Anything but the one defined value leaves stdin alone,
    /// so a hand-run `acp serve` cannot be shut down by a terminal EOF.
    #[test]
    fn only_the_defined_channel_value_arms_the_park() {
        assert!(is_parent_channel(Some(OsStr::new(STDIN))));
        assert!(!is_parent_channel(None));
        assert!(!is_parent_channel(Some(OsStr::new(""))));
        assert!(!is_parent_channel(Some(OsStr::new("1"))));
        assert!(!is_parent_channel(Some(OsStr::new("STDIN"))));
        assert!(!is_parent_channel(Some(OsStr::new("stdin\n"))));
    }

    /// End-to-end on a real spawned process: `attach` must deliver BOTH halves
    /// of the contract — the env marker that arms the child's park, and a stdin
    /// that stays open while the parent holds its end and reports EOF when it
    /// does not. The child refuses (exit 9) if the marker is missing, so this
    /// cannot pass on the stdin wiring alone.
    #[cfg(unix)]
    #[test]
    fn attach_gives_a_spawned_child_an_armed_channel_that_closes() {
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "test \"$WAYLAND_ACP_PARENT_CHANNEL\" = stdin || exit 9; cat >/dev/null",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        let supervisor_end = attach(&mut cmd).expect("attach a parent channel");
        let mut child = cmd.spawn().expect("spawn the probe child");

        std::thread::sleep(SETTLE);
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "the child exited while the parent still held the channel (a missing env marker \
             exits 9 here)"
        );

        drop(supervisor_end);
        let deadline = std::time::Instant::now() + BUDGET;
        let status = loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                break status;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the child outlived the closed parent channel — this is the #1156 orphan");
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        assert!(
            status.success(),
            "the child must reach EOF on stdin and exit cleanly, got {status:?}"
        );
    }
}
