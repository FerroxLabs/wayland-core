//! Issue #338 — an untrusted plugin install must not be able to prompt the
//! user for credentials on the terminal it was launched from.
//!
//! The route that matters is `open("/dev/tty")`: it needs none of the stdio we
//! hand the child and reads no environment, so it survives every
//! `GIT_TERMINAL_PROMPT`-shaped fix. A process can only open `/dev/tty` when it
//! has a CONTROLLING terminal, so the property under test is that a quarantine
//! child has none.
//!
//! That property is only OBSERVABLE from a process that itself has a
//! controlling terminal. A test binary run under `cargo nextest` over ssh or in
//! CI has none, so a naive in-process assertion would pass vacuously against
//! completely unhardened code. This test therefore re-executes itself inside a
//! real PTY (which makes it a session leader with that PTY as its ctty) and
//! runs both arms there:
//!
//! * `PLAIN=OPEN` — an UNHARDENED child reaches `/dev/tty`. This is the
//!   negative control. It must hold in both mutation arms; if it ever reports
//!   `DENIED` the environment cannot exhibit the defect and the other arms
//!   prove nothing.
//! * `HARDENED=DENIED` — the same child, spawned through
//!   `harden_against_credential_prompt`, cannot.
//! * `PRODUCTION_GIT=DENIED` — a real `git` built by `build_git_command`, the
//!   builder every quarantine spawn uses, cannot either. This one grades the
//!   wiring.
//! * `GIT_STILL_RUNS=true` — liveness control, also required in both arms: a
//!   guard that refuses everything is not a fix.

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

mod support;
use support::owned_tree::OwnedTree;

/// Set on the re-executed copy of this binary; its presence switches this test
/// from driver to probe.
const PROBE_ENV: &str = "WCORE_QUARANTINE_TTY_PROBE";

const TEST_NAME: &str = "quarantine_child_cannot_reach_the_controlling_terminal";

/// Opens the controlling terminal and immediately drops it.
///
/// Wrapped in a subshell so a failing `exec` redirection (which POSIX says
/// terminates a non-interactive shell) kills only the subshell, and opened
/// write-side so nothing can block on a read that never arrives.
const TTY_PROBE: &str = "if ( exec 3>/dev/tty ) 2>/dev/null; then echo OPEN; else echo DENIED; fi";

fn probe(harden: bool) -> String {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(TTY_PROBE)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if harden {
        wcore_cli::plugin::quarantine::harden_against_credential_prompt(&mut cmd);
    }
    match cmd.output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(e) => format!("SPAWN_FAILED({e})"),
    }
}

/// Run the tty probe as a `git` alias through the PRODUCTION command builder,
/// so the wiring is graded and not only the hardening function.
fn probe_through_production_git() -> String {
    let alias = format!("alias.ttyprobe=!{TTY_PROBE}");
    let mut cmd =
        wcore_cli::plugin::quarantine::build_git_command(&["-c", alias.as_str(), "ttyprobe"], None);
    match cmd.output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(e) => format!("SPAWN_FAILED({e})"),
    }
}

/// The half that runs inside the PTY.
fn run_as_probe() {
    println!("PLAIN={}", probe(false));
    println!("HARDENED={}", probe(true));
    println!("PRODUCTION_GIT={}", probe_through_production_git());

    // Liveness control: hardening must not simply break `git`. A guard that
    // refuses everything is not a fix, and would otherwise make every
    // DENIED above meaningless.
    let version = wcore_cli::plugin::quarantine::build_git_command(&["--version"], None)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|e| format!("SPAWN_FAILED({e})"));
    println!("GIT_STILL_RUNS={}", version.starts_with("git version"));
}

#[test]
fn quarantine_child_cannot_reach_the_controlling_terminal() {
    if std::env::var_os(PROBE_ENV).is_some() {
        run_as_probe();
        return;
    }

    let exe = std::env::current_exe().expect("current test binary");
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open PTY");

    let mut cmd = CommandBuilder::new(exe);
    cmd.arg(TEST_NAME);
    cmd.arg("--exact");
    cmd.arg("--nocapture");
    cmd.arg("--test-threads=1");
    cmd.env(PROBE_ENV, "1");
    cmd.env("TERM", "xterm-256color");

    // #352: the tree guard, not a bare Child — if this test panics or returns
    // early the PTY probe and anything it spawned must still be reaped.
    let mut child = OwnedTree::new(pair.slave.spawn_command(cmd).expect("spawn probe in PTY"));
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("clone PTY reader");
    let drain = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });

    let status = child.child_mut().wait().expect("probe exited");
    drop(pair.master);
    let output = drain.join().expect("drain thread");

    assert!(
        status.success(),
        "the probe run must pass on its own terms; output was:\n{output}"
    );

    // NEGATIVE CONTROL, asserted first. Without this the test below can pass
    // simply because nothing in this environment can open /dev/tty at all.
    assert!(
        output.contains("PLAIN=OPEN"),
        "an unhardened child must be able to open /dev/tty here, otherwise this \
         environment cannot exhibit the defect and the assertion below is \
         vacuous; output was:\n{output}"
    );

    assert!(
        output.contains("PRODUCTION_GIT=DENIED"),
        "the command builder every quarantine git spawn uses reached the \
         controlling terminal — the hardening exists but is not wired in \
         (#338); output was:\n{output}"
    );

    // Liveness control, also required in BOTH arms: the hardened path still
    // runs git. Without this a `harden` that made every spawn fail would read
    // as a pass.
    assert!(
        output.contains("GIT_STILL_RUNS=true"),
        "the hardened command builder must still be able to run git; \
         output was:\n{output}"
    );

    assert!(
        output.contains("HARDENED=DENIED"),
        "a quarantine-hardened child reached the user's controlling terminal — \
         a catalog-controlled plugin source can prompt for credentials inside \
         the TUI alt screen (#338); output was:\n{output}"
    );
}
