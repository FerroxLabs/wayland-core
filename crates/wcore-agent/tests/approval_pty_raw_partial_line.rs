//! #1131 — the approval budget must cover the ANSWER, not just its first byte.
//!
//! #946 bounded the approval prompt by waiting for stdin *readiness* before the
//! blocking `read_line`. Readiness is not completeness. In a **canonical**
//! terminal the line discipline holds a partial line back, so a keystroke with
//! no Enter behind it never makes stdin readable and the budget fires; that arm
//! is already graded in `approval_pty_no_reader.rs`. In a **non-canonical (raw)**
//! terminal the byte is delivered the moment it is typed: the readiness wait
//! reports ready, `read_line` is handed a stdin holding `y` and no newline, and
//! it parks for the life of the process waiting for a terminator that never
//! comes. Same hang class as #946, narrower trigger.
//!
//! Raw mode is not hypothetical for this binary: the process inherits whatever
//! termios its controlling terminal already has, so any parent that took the tty
//! out of canonical mode (an embedding TUI, `stty raw`, a driver harness) puts
//! `check_for` in exactly this state.
//!
//! Every arm drives the REAL `ToolConfirmer` in a child process whose stdin is a
//! real pty slave put into raw mode with `cfmakeraw`, so the wiring is graded and
//! not just the predicate. The child reports its verdict through a FILE, never
//! the terminal: writing it to the pty would put the instrument on the channel
//! the subject reads back.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Set on the child only; names the file the child writes its verdict to.
const HELPER_OUT_ENV: &str = "WCORE_1131_HELPER_VERDICT";
/// The `#[test]` the child re-executes as its whole body.
const HELPER_TEST: &str = "helper_child_answers_one_raw_confirmation";
/// Approval budget the child runs with — short so an arm finishes fast.
const CHILD_BUDGET_SECS: u64 = 2;
/// How long the harness waits before calling the child hung.
const ARM_BUDGET: Duration = Duration::from_secs(20);
/// #1309: how long the harness waits, AFTER the child has been reaped, for the
/// pty reader to reach a terminal condition.
///
/// A child exit does not mean the transcript is complete. The reader is a
/// separate thread blocked in `read` on a dup of the pty master, and the bytes
/// the child wrote on its way out sit in the pty buffer until that thread is
/// scheduled again. Cloning the transcript at the moment the child is reaped
/// samples whatever happens to have been copied by then, which is why a
/// missing denial reason and a read that returned early produce the SAME
/// string. Every slave dup is closed once the child is reaped, so the master
/// reaches EOF (or `EIO`, which is how Linux spells it) after handing over the
/// last byte -- a terminal condition the harness can WAIT for instead of
/// sampling. Five seconds is far past what that costs; it is a bound so a
/// wedged reader cannot hang the suite, not a race to be tuned.
const DRAIN_BUDGET: Duration = Duration::from_secs(5);

/// Re-execution target, not a test of its own: with `HELPER_OUT_ENV` unset
/// (every ordinary suite run) it returns immediately.
#[test]
fn helper_child_answers_one_raw_confirmation() {
    let Ok(out) = std::env::var(HELPER_OUT_ENV) else {
        return;
    };
    let mut confirmer = wcore_agent::confirm::ToolConfirmer::new(false, vec![]);
    let verdict = confirmer.check("Bash", "rm -rf /var/tmp/wcore-1131");
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
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(rc, 0, "openpty failed: {}", std::io::Error::last_os_error());
    // SAFETY: `openpty` returned 0, so both fds are fresh and owned by us.
    unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
}

/// Put the slave into raw mode. This is the ONLY difference between this file
/// and the canonical-mode arms in `approval_pty_no_reader.rs`: same harness,
/// same child, same budget, only the termios differing.
fn make_raw(slave: &OwnedFd) {
    // SAFETY: `termios` is fully initialised by `tcgetattr` before use, and
    // `slave` is a live fd for the pty slave we just opened.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        assert_eq!(
            libc::tcgetattr(slave.as_raw_fd(), &mut termios),
            0,
            "tcgetattr on the pty slave failed: {}",
            std::io::Error::last_os_error()
        );
        libc::cfmakeraw(&mut termios);
        assert_eq!(
            libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &termios),
            0,
            "tcsetattr on the pty slave failed: {}",
            std::io::Error::last_os_error()
        );
        // Prove the mode really changed rather than trusting the call: an arm
        // run against a still-canonical tty would be a green from the wrong
        // terminal.
        let mut check: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(slave.as_raw_fd(), &mut check), 0);
        assert_eq!(
            check.c_lflag & libc::ICANON,
            0,
            "the slave must really be non-canonical for this arm to mean anything"
        );
    }
}

/// Why the pty reader stopped.
///
/// #1309: the transcript is evidence about the PRODUCT only once the reader has
/// reached `Terminal`. Before that it is evidence about the harness, and the
/// two must never be reported as the same thing.
#[derive(Debug)]
enum DrainEnd {
    /// The master handed over its last byte and then reported EOF (`Ok(0)`) or
    /// the `EIO` that Linux returns once every slave dup is closed. Whatever
    /// the child wrote, the transcript now holds.
    Terminal(String),
    /// The reader was still blocked in `read` when `DRAIN_BUDGET` ran out. The
    /// transcript is a truncated read and grades nothing.
    StillReading,
}

/// One arm's outcome. `exited` separates "the child decided" from "the harness
/// killed it", which is the whole distinction this file grades. `drain`
/// separates "the child never wrote it" from "the harness never read it",
/// which is the distinction #1309 was filed for.
struct Arm {
    exited: bool,
    verdict: String,
    transcript: String,
    elapsed: Duration,
    drain: DrainEnd,
    drain_wait: Duration,
}

/// Run one arm. `answer` is what (if anything) gets typed at the prompt.
fn run_arm(answer: Option<&str>) -> Arm {
    let dir = tempfile::tempdir().expect("tempdir");
    let verdict_path = dir.path().join("verdict");
    let (master, slave) = open_pty();
    make_raw(&slave);

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
    // The reader announces the terminal condition it reached, so the harness can
    // WAIT for one rather than sample the transcript at an arbitrary moment.
    // Sending only after the last `push_str` is what makes the announcement
    // mean "every byte is in the transcript" and not merely "I am leaving".
    let (drained_tx, drained_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 1024];
        let end = loop {
            match reader.read(&mut buf) {
                Ok(0) => break "EOF".to_string(),
                Ok(n) => sink
                    .lock()
                    .expect("transcript mutex")
                    .push_str(&String::from_utf8_lossy(&buf[..n])),
                // Linux reports "the last slave dup closed" as EIO rather than
                // EOF. It is the same terminal condition and it arrives after
                // the same last byte.
                Err(e) => break format!("{e}"),
            }
        };
        let _ = drained_tx.send(end);
    });

    if let Some(answer) = answer {
        let mut writer = std::fs::File::from(master.try_clone().expect("dup master to write"));
        writer
            .write_all(answer.as_bytes())
            .expect("type the answer");
        writer.flush().expect("flush the answer");
    }

    let mut exited = true;
    loop {
        match child.try_wait().expect("poll helper child") {
            Some(_) => break,
            None if started.elapsed() > ARM_BUDGET => {
                let _ = child.kill();
                let _ = child.wait();
                exited = false;
                break;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let elapsed = started.elapsed();
    // #1309. The child is reaped, so every slave dup it held is closed and the
    // master is on its way to a terminal condition. Wait for the reader to say
    // it got there before reading the transcript: the alternative is a sample
    // whose truncation is indistinguishable from a product that stayed silent.
    let drain_started = Instant::now();
    let drain = match drained_rx.recv_timeout(DRAIN_BUDGET) {
        Ok(end) => DrainEnd::Terminal(end),
        Err(_) => DrainEnd::StillReading,
    };
    let drain_wait = drain_started.elapsed();
    let transcript = seen.lock().expect("transcript mutex").clone();
    let verdict = std::fs::read_to_string(&verdict_path).unwrap_or_else(|e| format!("<none: {e}>"));
    Arm {
        exited,
        verdict,
        transcript,
        elapsed,
        drain,
        drain_wait,
    }
}

/// Report an arm the way the issue reports it, so a red arm is quotable.
fn report(name: &str, arm: &Arm) {
    println!(
        "[{name}] exited={} elapsed={:.3}s verdict={:?} prompt_seen={} \
         drain={:?} drain_wait={:.3}s transcript_bytes={}",
        arm.exited,
        arm.elapsed.as_secs_f64(),
        arm.verdict,
        arm.transcript.contains("Allow?"),
        arm.drain,
        arm.drain_wait.as_secs_f64(),
        arm.transcript.len()
    );
}

/// THE DEFECT. A single keystroke with no Enter behind it. In raw mode the byte
/// is readable immediately, so the #946 readiness wait reports ready and hands a
/// partial line to a blocking read. Before the fix this arm does not fail — it
/// never finishes, and only the harness kill ends it.
#[test]
fn raw_mode_partial_line_denies_on_the_budget_instead_of_parking() {
    let arm = run_arm(Some("y"));
    report("raw/partial-line", &arm);
    assert!(
        arm.transcript.contains("Allow?"),
        "the child must have reached the interactive prompt; pty saw: {:?}",
        arm.transcript
    );
    assert!(
        arm.exited,
        "the approval prompt never returned: the child was still parked on a \
         partial line in raw mode after {}s (the approval budget was \
         {CHILD_BUDGET_SECS}s). elapsed={:.3}s verdict={:?}",
        ARM_BUDGET.as_secs(),
        arm.elapsed.as_secs_f64(),
        arm.verdict
    );
    assert_eq!(
        arm.verdict, "Denied",
        "an answer that never completes must fail closed, never auto-approve"
    );
    assert!(
        arm.elapsed < ARM_BUDGET,
        "the denial must land on the approval budget, not on the harness kill"
    );
}

/// Negative control #1. Without it the fix is unfalsifiable: a confirmer that
/// denied unconditionally in raw mode would pass the arm above. Enter in a raw
/// terminal sends CR, not LF — there is no ICRNL to translate it — so this arm
/// also grades that a raw-mode user who DOES answer is heard.
#[test]
fn raw_mode_completed_answer_is_still_honoured() {
    let arm = run_arm(Some("y\r"));
    report("raw/answered", &arm);
    assert!(
        arm.exited,
        "an answered prompt must return; elapsed={:.3}s pty saw: {:?}",
        arm.elapsed.as_secs_f64(),
        arm.transcript
    );
    assert_eq!(
        arm.verdict, "Approved",
        "a completed raw-mode answer must still be honoured; pty saw: {:?}",
        arm.transcript
    );
}

/// Negative control #2. `n` must still deny for the reason the user gave, and a
/// completed answer must not be mistaken for the timeout path.
#[test]
fn raw_mode_explicit_refusal_is_still_a_refusal() {
    let arm = run_arm(Some("n\r"));
    report("raw/refused", &arm);
    assert!(arm.exited, "an answered prompt must return");
    assert_eq!(arm.verdict, "Denied");
    assert!(
        !arm.transcript.contains("No answer after"),
        "an explicit refusal must not be reported as an expired budget; pty \
         saw: {:?}",
        arm.transcript
    );
}

/// Positive control on the harness itself: the #946 guard still fires in raw
/// mode when nothing at all is typed. If this arm ever goes red the raw setup,
/// not the fix, is what broke.
#[test]
fn raw_mode_with_nothing_typed_still_denies() {
    let arm = run_arm(None);
    report("raw/silent", &arm);
    assert!(
        arm.exited,
        "the #946 budget must still fire in raw mode; elapsed={:.3}s",
        arm.elapsed.as_secs_f64()
    );
    assert_eq!(arm.verdict, "Denied");
    // #1309 c1 -- the two readings, told apart and reported apart.
    //
    // A transcript that stops at the `> ` prompt has two causes and the old
    // shape could not name either: the product denied without saying why, or
    // the harness read the pty before the reason was written. This assertion
    // fires FIRST and only on the second cause, so a truncated capture can
    // never be reported as a product defect (nor a product defect excused as a
    // truncated capture).
    assert!(
        matches!(arm.drain, DrainEnd::Terminal(_)),
        "CAPTURE INCOMPLETE -- this grades the harness, not the product. The \
         child was reaped (exited={}, elapsed={:.3}s) but the pty reader was \
         still blocked in read after a further {:.3}s, so the transcript below \
         is whatever had been copied by then and says NOTHING about whether \
         the denial reason was written. pty saw so far: {:?}",
        arm.exited,
        arm.elapsed.as_secs_f64(),
        arm.drain_wait.as_secs_f64(),
        arm.transcript
    );
    assert!(
        arm.transcript.contains("No answer after"),
        "the operator must be told why it was denied. THE CAPTURE IS COMPLETE: \
         the pty reader reached {:?} {:.3}s after the child was reaped, so \
         every byte the child ever wrote is in this transcript and the reason \
         is genuinely ABSENT rather than merely unread; pty saw: {:?}",
        arm.drain,
        arm.drain_wait.as_secs_f64(),
        arm.transcript
    );
}
