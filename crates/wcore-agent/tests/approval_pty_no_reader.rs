//! #946 — an approval prompt on a pty nobody answers must fail closed.
//!
//! `ToolConfirmer::check_for` guards the *non*-terminal case
//! (`!io::stdin().is_terminal()`), but a detached tmux/screen pane, a `script`
//! wrapper, or CI that allocated a tty nobody types into all leave stdin a
//! REAL terminal with nothing on the other end. `is_terminal()` stays true, the
//! guard never fires, and `read_line` blocks for the life of the process: the
//! turn stops with no output and no way to answer.
//!
//! Both arms drive the real `ToolConfirmer` in a child process whose stdin is a
//! real pty slave, so the wiring is graded and not just the predicate:
//!
//! * nothing written to the master -> the child must exit `Denied` inside the
//!   budget (on unfixed code it never exits at all);
//! * `y\n` written to the master   -> the child must still exit `Approved`
//!   (negative control — a blanket deny would pass the first arm alone).
//!
//! The child reports its verdict through a FILE, never the terminal: writing it
//! to the pty would put the instrument on the channel the subject reads back.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Set on the child only; names the file the child writes its verdict to.
const HELPER_OUT_ENV: &str = "WCORE_946_HELPER_VERDICT";
/// The `#[test]` the child re-executes as its whole body.
const HELPER_TEST: &str = "helper_child_answers_one_confirmation";
/// Approval budget the child runs with — short so an arm finishes fast.
const CHILD_BUDGET_SECS: u64 = 2;
/// How long the harness waits before calling the child hung.
const ARM_BUDGET: Duration = Duration::from_secs(30);

/// Re-execution target, not a test of its own: with `HELPER_OUT_ENV` unset
/// (every ordinary suite run) it returns immediately.
#[test]
fn helper_child_answers_one_confirmation() {
    let Ok(out) = std::env::var(HELPER_OUT_ENV) else {
        return;
    };
    let mut confirmer = wcore_agent::confirm::ToolConfirmer::new(false, vec![]);
    let verdict = confirmer.check("Bash", "rm -rf /tmp/wcore-946");
    std::fs::write(out, format!("{verdict:?}")).expect("child writes its verdict");
}

/// A fresh pty pair. Returned as owned fds so neither end leaks.
fn open_pty() -> (OwnedFd, OwnedFd) {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: both out-params are valid `c_int` slots; the three optional
    // pointers are null, which `openpty(3)` documents as "use the defaults".
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(rc, 0, "openpty failed: {}", std::io::Error::last_os_error());
    // SAFETY: `openpty` returned 0, so both fds are fresh and owned by us.
    unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
}

/// Run one arm. `answer` is what (if anything) gets typed at the prompt.
/// Returns the child's verdict, everything the pty master saw, and how long
/// the child took to exit.
fn run_arm(answer: Option<&str>) -> (String, String, Duration) {
    let dir = tempfile::tempdir().expect("tempdir");
    let verdict_path = dir.path().join("verdict");
    let (master, slave) = open_pty();

    let mut child = Command::new(std::env::current_exe().expect("test binary path"))
        .args([HELPER_TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(HELPER_OUT_ENV, &verdict_path)
        .env(
            "WAYLAND_APPROVAL_TIMEOUT_SECS",
            CHILD_BUDGET_SECS.to_string(),
        )
        .stdin(Stdio::from(slave.try_clone().expect("dup slave for stdin")))
        .stderr(Stdio::from(
            slave.try_clone().expect("dup slave for stderr"),
        ))
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn helper child");
    let started = Instant::now();
    // The child holds its own slave dups, so releasing ours neither closes the
    // child's stdin (an EOF there would trip the *other* fail-closed path and
    // mask this defect) nor leaves the master unable to see the child exit.
    drop(slave);

    // Drain the master so a full pty buffer can never be what stalls the child,
    // and so the arm can assert the prompt was really printed.
    let mut reader = std::fs::File::from(master.try_clone().expect("dup master to read"));
    let seen = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock()
                .expect("transcript mutex")
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });

    if let Some(answer) = answer {
        let mut writer = std::fs::File::from(master.try_clone().expect("dup master to write"));
        writer
            .write_all(answer.as_bytes())
            .expect("type the answer");
        writer.flush().expect("flush the answer");
    }

    let status = loop {
        match child.try_wait().expect("poll helper child") {
            Some(status) => break status,
            None if started.elapsed() > ARM_BUDGET => {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "the approval prompt never returned: the child was still \
                     blocked on a pty nobody answered after {}s (the approval \
                     budget was {CHILD_BUDGET_SECS}s)",
                    ARM_BUDGET.as_secs()
                );
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };
    let elapsed = started.elapsed();
    let transcript = seen.lock().expect("transcript mutex").clone();
    assert!(
        status.success(),
        "helper child exited {status}; pty saw: {transcript:?}"
    );
    let verdict = std::fs::read_to_string(&verdict_path)
        .unwrap_or_else(|e| panic!("child wrote no verdict ({e}); pty saw: {transcript:?}"));
    (verdict, transcript, elapsed)
}

/// The defect: stdin is a terminal, so the `!is_terminal()` guard does not
/// fire, and nobody is on the other end to type. Before the fix this test does
/// not fail — it never finishes.
#[test]
fn pty_with_no_reader_denies_instead_of_blocking_forever() {
    let (verdict, transcript, elapsed) = run_arm(None);
    assert!(
        transcript.contains("Allow?"),
        "the child must have reached the interactive prompt; pty saw: {transcript:?}"
    );
    assert_eq!(
        verdict, "Denied",
        "an approval nobody can answer must fail closed, never auto-approve"
    );
    assert!(
        elapsed < ARM_BUDGET,
        "the denial must land on the approval budget, not on the harness kill"
    );
}

/// Negative control. Without this arm the fix is unfalsifiable: a confirmer
/// that denied unconditionally would pass the arm above.
#[test]
fn pty_with_a_reader_still_prompts_and_honours_the_answer() {
    let (verdict, transcript, _elapsed) = run_arm(Some("y\n"));
    assert!(
        transcript.contains("Allow?"),
        "a pty WITH a reader must still be prompted; pty saw: {transcript:?}"
    );
    assert_eq!(
        verdict, "Approved",
        "an answered prompt must still be honoured; pty saw: {transcript:?}"
    );
}
