//! `--list-sessions` is a QUERY: its answer belongs on STDOUT.
//!
//! Measured on `wayland-core 0.12.26`: `wayland-core --list-sessions > out 2> err`
//! produced **0 bytes on stdout**, the whole table on stderr, and exit 0 — so
//! `wayland-core --list-sessions | grep <id>` matched nothing while the table
//! scrolled past on the terminal, and the failure was silent because the exit
//! code said success. `--list-agents`, three short-circuits above it in
//! `main.rs`, already printed its answer to stdout, and so did the
//! `session list` subcommand, whose own doc comment recorded the root flag as
//! the outlier.
//!
//! Both legs matter and neither is redundant:
//!
//! 1. **The table reaches stdout.** Alone, this passes if the code printed the
//!    table to BOTH streams — which still breaks nothing but also fixes
//!    nothing for anyone reading a terminal.
//! 2. **The table does NOT reach stderr.** The leg that refuses the
//!    print-to-both non-fix.
//!
//! The assertion is on the seeded MODEL string, not on the `ID`/`Date` header:
//! the header is printed by a different statement from the rows, so a matcher
//! that only saw the header would score a run whose ROWS still went to stderr
//! as green.

use std::path::Path;
use std::process::Command;

use wcore_agent::session::SessionManager;

/// Distinctive enough that it cannot collide with anything else the binary
/// prints on either stream.
const MODEL: &str = "uxb-list-sessions-stream-probe-model";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Seed one listed session into `<home>/sessions`, which is where
/// `default_session_dir()` resolves once `WAYLAND_HOME` points at `home`.
fn seed_one_session(home: &Path) {
    let manager = SessionManager::new(home.join("sessions"), 20);
    let session = manager
        .create("openai", MODEL, "/tmp", None)
        .expect("create session");
    manager.save(&session).expect("save session");
    manager
        .update_index_for(&session)
        .expect("index the session");
}

#[test]
fn list_sessions_writes_the_table_to_stdout_not_stderr() {
    let home = tempfile::tempdir().expect("tempdir");
    seed_one_session(home.path());

    let out = Command::new(binary())
        .arg("--list-sessions")
        .env("WAYLAND_HOME", home.path())
        .env("HOME", home.path())
        .env_remove("API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .output()
        .expect("run --list-sessions");

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    assert!(
        out.status.success(),
        "--list-sessions must exit 0: {:?}\nstderr: {stderr}",
        out.status
    );
    assert!(
        stdout.contains(MODEL),
        "the session table is this flag's answer and belongs on stdout.\n\
         stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        !stderr.contains(MODEL),
        "the table must not ALSO go to stderr — printing to both streams is \
         not a fix.\nstdout: {stdout}\nstderr: {stderr}"
    );
}
