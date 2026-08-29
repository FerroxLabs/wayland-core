//! A `--json-stream` startup refusal must reach the HOST, not just stderr.
//!
//! These tests drive the real `wayland-core` binary and read its **stdout**
//! exactly as the Wayland desktop app does — one JSON object per line. Reading
//! the reason from stderr would not count: stderr is where the reason already
//! was, and the protocol consumer does not read it. That is the whole defect.
//!
//! Measured before the fix (hetzner-dsm, debug build, five conditions):
//! a plaintext-credentials refusal exited rc=1 having written **0 bytes** to
//! stdout, with a 6015-byte stderr carrying the real reason.
//!
//! # Why there is a positive control here
//!
//! A previous probe of this exact area produced a false HIGH: it reported that
//! `ready` was never emitted, having actually reproduced its own harness's
//! isolation bug. A refusal is indistinguishable from a broken invocation
//! unless the same harness is shown to observe a healthy start, so
//! [`healthy_start_still_emits_ready`] is a load-bearing part of this file
//! rather than a nicety. It also blocks the "manufacture a green by making
//! nothing start" failure: if the fix broke startup universally, every
//! refusal assertion would still pass and only this test would catch it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

/// A dummy key so `Config::resolve` SUCCEEDS and the run reaches the later
/// startup stages under test. It authenticates nothing and contacts nothing.
const DUMMY_KEY: &str = "sk-ant-not-a-real-key-000000000000000000";

struct Capture {
    stdout: String,
    stderr: String,
    status: Option<i32>,
}

impl Capture {
    /// Parse stdout the way the host does: JSON Lines, one frame per line.
    fn frames(&self) -> Vec<serde_json::Value> {
        self.stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l)
                    .unwrap_or_else(|e| panic!("host cannot parse stdout line as JSON: {e}\n{l}"))
            })
            .collect()
    }

    fn frames_of_type(&self, ty: &str) -> Vec<serde_json::Value> {
        self.frames()
            .into_iter()
            .filter(|f| f.get("type").and_then(|t| t.as_str()) == Some(ty))
            .collect()
    }
}

/// Write an isolated profile home and run the binary over `--json-stream`.
///
/// Isolation is via `WAYLAND_HOME`, which is the product's own sanctioned
/// mechanism and works identically on every platform. `HOME`/`USERPROFILE` are
/// also redirected because `HOME` alone does NOT isolate config on Windows —
/// `dirs::home_dir()` reads `USERPROFILE` there, and a previous probe of this
/// area reproduced its own harness bug that way and graded it a HIGH.
fn run_json_stream(case_dir: &Path, config_toml: &str, with_key: bool, extra: &[&str]) -> Capture {
    let home = case_dir.join("home");
    let fake_home = case_dir.join("fakehome");
    let proj = case_dir.join("proj");
    std::fs::create_dir_all(&home).expect("create WAYLAND_HOME");
    std::fs::create_dir_all(&fake_home).expect("create fake HOME");
    std::fs::create_dir_all(&proj).expect("create project dir");
    std::fs::write(home.join("config.toml"), config_toml).expect("write config.toml");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wayland-core"));
    cmd.arg("--json-stream")
        .arg("--project-dir")
        .arg(&proj)
        .args(extra)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("API_KEY")
        .env_remove("WAYLAND_VAULT_PASSPHRASE")
        .env_remove("WAYLAND_VAULT_PASSPHRASE_FD")
        .env("WAYLAND_HOME", &home)
        .env("HOME", &fake_home)
        .env("USERPROFILE", &fake_home)
        .env("TERM", "dumb")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if with_key {
        cmd.env("ANTHROPIC_API_KEY", DUMMY_KEY);
    }

    let mut child = OwnedTree::new(cmd.spawn().expect("spawn wayland-core --json-stream"));
    // Close stdin immediately: the protocol loop ends when the host hangs up,
    // so a healthy run emits `ready` and then exits cleanly.
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait for wayland-core");
    Capture {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        status: out.status.code(),
    }
}

fn case_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wl-jsr-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create case dir");
    dir
}

fn config(session_enabled: bool, backend: &str) -> String {
    format!(
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\n\
         [storage.credentials]\nbackend = \"{backend}\"\n\n\
         [session]\nenabled = {session_enabled}\n"
    )
}

/// Assert a single startup-refusal error frame is present, and return its
/// message so the caller can prove the host can NAME the reason.
fn assert_single_error_frame(cap: &Capture, case: &str) -> String {
    let errors = cap.frames_of_type("error");
    assert_eq!(
        errors.len(),
        1,
        "{case}: host must receive exactly one error frame, got {}. stdout was:\n{}",
        errors.len(),
        cap.stdout
    );
    let err = &errors[0]["error"];
    assert_eq!(
        err["retryable"], false,
        "{case}: a startup refusal is not retryable"
    );
    assert!(
        err["code"].as_str().is_some_and(|c| !c.is_empty()),
        "{case}: error frame must carry a code"
    );
    err["message"]
        .as_str()
        .unwrap_or_else(|| panic!("{case}: error frame message must be a string"))
        .to_string()
}

/// POSITIVE CONTROL. Proves the harness can observe a healthy start, and that
/// the fix did not "succeed" by making nothing start at all.
#[test]
fn healthy_start_still_emits_ready() {
    let dir = case_dir("healthy");
    let cap = run_json_stream(&dir, &config(false, "plaintext"), true, &[]);

    let ready = cap.frames_of_type("ready");
    assert_eq!(
        ready.len(),
        1,
        "healthy start must emit exactly one ready frame. rc={:?} stdout:\n{}\nstderr tail:\n{}",
        cap.status,
        cap.stdout,
        cap.stderr
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(cap.status, Some(0), "healthy start must exit 0");
    assert!(
        cap.frames_of_type("error").is_empty(),
        "healthy start must not emit an error frame"
    );
    // The control must also prove the stream is substantive, not a lone frame.
    assert!(
        cap.frames().len() > 1,
        "healthy start should emit more than just ready"
    );
}

/// THE MEASURED DEFECT: durable sessions on a credentials backend that cannot
/// hold the recovery key. Pre-fix this wrote zero bytes to stdout.
#[test]
fn plaintext_credentials_refusal_reaches_the_host() {
    let dir = case_dir("plaintext");
    let cap = run_json_stream(&dir, &config(true, "plaintext"), true, &[]);

    assert_ne!(cap.status, Some(0), "this condition must refuse to start");
    assert!(
        cap.frames_of_type("ready").is_empty(),
        "a refused start must not claim ready"
    );
    let message = assert_single_error_frame(&cap, "plaintext-credentials");

    // The host must be able to NAME the reason, not merely observe that
    // something failed. These are the actionable nouns in the refusal.
    assert!(
        message.contains("plaintext"),
        "host must learn WHICH backend was refused; got: {message}"
    );
    assert!(
        message.contains("session") || message.contains("recovery"),
        "host must learn the refusal concerns durable session recovery; got: {message}"
    );
}

/// A corrupt `config.toml` returns above the pre-existing #186 emit site, so
/// this path emitted nothing even though config failure was believed covered.
#[test]
fn corrupt_config_refusal_reaches_the_host() {
    let dir = case_dir("parse");
    let cap = run_json_stream(&dir, "[default\nnot valid toml = = =\n", true, &[]);

    assert_ne!(cap.status, Some(0), "a corrupt config must refuse to start");
    let message = assert_single_error_frame(&cap, "corrupt-config");
    assert!(
        message.contains("config") || message.contains("toml") || message.contains("parse"),
        "host must learn the config file failed to load; got: {message}"
    );
}

/// `--profile` with no isolated home: a fail-closed guard that bails long
/// before any pre-existing emit site.
#[test]
fn profile_guard_refusal_reaches_the_host() {
    let dir = case_dir("profile");
    let home = dir.join("home");
    let fake_home = dir.join("fakehome");
    let proj = dir.join("proj");
    for d in [&home, &fake_home, &proj] {
        std::fs::create_dir_all(d).expect("create dir");
    }
    std::fs::write(
        home.join("config.toml"),
        "[default]\nprovider = \"anthropic\"\n",
    )
    .expect("write config");

    // WAYLAND_HOME deliberately NOT set -- that is the condition under test.
    let mut child = OwnedTree::new(
        Command::new(env!("CARGO_BIN_EXE_wayland-core"))
            .args(["--json-stream", "--profile", "work", "--project-dir"])
            .arg(&proj)
            .env_remove("WAYLAND_HOME")
            .env("ANTHROPIC_API_KEY", DUMMY_KEY)
            .env("HOME", &fake_home)
            .env("USERPROFILE", &fake_home)
            .env("TERM", "dumb")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn wayland-core"),
    );
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    let cap = Capture {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        status: out.status.code(),
    };

    assert_ne!(cap.status, Some(0), "profile guard must refuse to start");
    let message = assert_single_error_frame(&cap, "profile-guard");
    assert!(
        message.contains("WAYLAND_HOME") || message.contains("profile"),
        "host must learn the profile home was missing; got: {message}"
    );
}

/// The pre-existing #186 path must keep working and must NOT double-report now
/// that a chokepoint also exists. `assert_single_error_frame` is what pins it.
#[test]
fn missing_api_key_still_reports_exactly_once() {
    let dir = case_dir("nokey");
    let cap = run_json_stream(
        &dir,
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n",
        false,
        &[],
    );

    assert_ne!(cap.status, Some(0), "no credential must refuse to start");
    let message = assert_single_error_frame(&cap, "missing-api-key");
    assert!(
        message.contains("API key") || message.contains("api_key"),
        "host must learn a credential is missing; got: {message}"
    );
}

/// The refusal must arrive on STDOUT specifically. Asserting only "a frame
/// exists somewhere" would pass on the pre-fix binary if the harness merged
/// the two streams -- the exact mistake that produced the earlier false HIGH.
#[test]
fn refusal_is_on_stdout_not_merely_somewhere() {
    let dir = case_dir("stream-separation");
    let cap = run_json_stream(&dir, &config(true, "plaintext"), true, &[]);

    assert!(
        !cap.stdout.trim().is_empty(),
        "stdout must not be empty on a refusal -- that is the defect"
    );
    // And the frame must be parseable from stdout ALONE.
    assert_eq!(
        cap.frames_of_type("error").len(),
        1,
        "exactly one error frame must be readable from stdout alone"
    );
    // stderr keeps the human-facing text; we assert only that stdout no longer
    // depends on it, never that stderr changed.
    assert!(
        !cap.stderr.is_empty(),
        "stderr should still carry the operator-facing message"
    );
}
