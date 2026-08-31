//! FerroxLabs/wayland#1266 c1 — an engine-classified IN-BAND error reaches the
//! HOST with a category other than `unknown`.
//!
//! # Why this file exists
//!
//! c1's evidence clause asks for "a test that an engine-classified in-band
//! error arrives **at the host** with a category other than `unknown`". At the
//! point this was written, the tree proved that claim in two halves with two
//! different instruments and never joined them:
//!
//! * `crates/wcore-agent/tests/issue_1266_in_band_category_test.rs` drives a
//!   real `AgentEngine::run` but stops at `CatSink` — a test double defined in
//!   the test file. Its own name says so:
//!   `..._reaches_the_SINK_as_context_limit`.
//! * `ProtocolSink`, which builds the frame a host actually reads, was never
//!   driven by the engine with a non-`Unknown` category anywhere:
//!   `protocol_sink.rs`'s own unit test passes `Unknown`,
//!   `golden_v0_1_21.rs` pins `"unknown"`, and the shipped
//!   `contracts/desktop/v1/events/error.json` is
//!   `{"error":{"category":"unknown", ...}}`.
//!
//! `issue_1266_c2_frame_test.rs` closed most of that distance — real engine,
//! real `ProtocolSink`, assertions on encoded JSON bytes — but its emitter is
//! still an in-process recorder, so the last hop (`OutputPump` -> the process's
//! stdout) is not covered there. This file covers it, by reading the bytes out
//! of a REAL `wayland-core --json-stream` process's stdout exactly as the
//! Wayland desktop app does: one JSON object per line.
//!
//! Nothing in here is in-process. There is no sink of ours anywhere in the
//! path: the engine classifies at its own call site, hands the category to
//! `ProtocolSink`, which serializes to a `ProtocolWriter`, which pumps bytes
//! to fd 1, which this test reads back over a pipe and parses as the host
//! does.
//!
//! # Why the refusal is a context ceiling and not a provider fault
//!
//! `[compact] context_window = 1024` is below core's own baseline turn, so the
//! engine refuses IN BAND — the run returns normally, the process stays up and
//! exits 0 at stdin EOF — and it does so **before any provider call**. That
//! makes this test hermetic: no network, no key, no timing.
//!
//! # Why there is a positive control
//!
//! A refusal frame is indistinguishable from a broken invocation unless the
//! same harness is shown to observe a healthy start.
//! [`the_harness_observes_a_healthy_start_and_reports_an_opaque_failure_as_unknown`] is therefore
//! load-bearing, not a nicety: without it, a change that broke startup
//! universally would make the refusal assertions pass for the wrong reason.
//! It doubles as the c2-control shape at this layer — a run that reaches the
//! provider over a dead endpoint reports `unknown`, so the binary is not
//! shipping one hardcoded category.
//!
//! # RED ARM (recorded, re-runnable, and MEASURED)
//!
//! In `crates/wcore-agent/src/engine.rs`, change the unworkable-window
//! refusal's `FailureCategory::ContextLimit` to `FailureCategory::Unknown`,
//! `touch` the file and rebuild. The mutation was asserted to have landed on
//! the line and `cargo check -p wcore-cli --tests` was RC=0 before the run was
//! believed, so this is not a mutation that merely failed to compile.
//!
//! Result: **1 failed, 1 passed**.
//! [`the_context_ceiling_refusal_reaches_the_real_host_as_context_limit`] went
//! RED with the frame arriving off the process's stdout as
//! `{"category":"unknown", ... "core cannot operate in a window that small"
//! ...}`, while
//! [`the_harness_observes_a_healthy_start_and_reports_an_opaque_failure_as_unknown`]
//! stayed green — which is the shape that matters: the mutation moved exactly
//! the classification under test and nothing else.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

/// A syntactically valid key so `Config::resolve` SUCCEEDS and the run reaches
/// the turn loop. It authenticates nothing: the arm under test refuses before
/// any provider call, and the control deliberately points at a dead port.
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

    /// Every `error` frame's `category` as it appears ON THE WIRE.
    ///
    /// `<absent>` rather than a default when the key is missing: a frame that
    /// omits the field entirely is a different defect from one carrying
    /// `"unknown"`, and collapsing them would hide it.
    fn error_categories(&self) -> Vec<String> {
        self.frames_of_type("error")
            .iter()
            .map(|f| {
                f["error"]
                    .get("category")
                    .and_then(|c| c.as_str())
                    .unwrap_or("<absent>")
                    .to_string()
            })
            .collect()
    }
}

/// Write an isolated profile home, run the real binary over `--json-stream`,
/// send one `message` command, and capture what the host would have read.
///
/// Isolation is via `WAYLAND_HOME` — the product's own sanctioned mechanism.
/// `HOME`/`USERPROFILE` are also redirected because `HOME` alone does NOT
/// isolate config on Windows (`dirs::home_dir()` reads `USERPROFILE` there),
/// and a previous probe of this area reproduced its own harness bug that way.
fn run_one_turn(case_dir: &Path, config_toml: &str) -> Capture {
    let home = case_dir.join("home");
    let fake_home = case_dir.join("fakehome");
    let proj = case_dir.join("proj");
    for d in [&home, &fake_home, &proj] {
        std::fs::create_dir_all(d).expect("create case dir");
    }
    std::fs::write(home.join("config.toml"), config_toml).expect("write config.toml");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wayland-core"));
    cmd.arg("--json-stream")
        .arg("--project-dir")
        .arg(&proj)
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        // #1266-adjacent hygiene: a bare `API_KEY` set for an unrelated
        // service is honoured as a provider credential, so an inherited one
        // would make this test contact a real endpoint.
        .env_remove("API_KEY")
        .env_remove("WAYLAND_VAULT_PASSPHRASE")
        .env_remove("WAYLAND_VAULT_PASSPHRASE_FD")
        .env("WAYLAND_HOME", &home)
        .env("HOME", &fake_home)
        .env("USERPROFILE", &fake_home)
        .env("TERM", "dumb")
        .env("ANTHROPIC_API_KEY", DUMMY_KEY)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = OwnedTree::new(cmd.spawn().expect("spawn wayland-core --json-stream"));
    {
        let mut stdin = child.stdin.take().expect("child stdin");
        // One real host command. `ProtocolCommand::Message` is `#[serde(tag =
        // "type", rename_all = "snake_case")]`, so this is the exact line the
        // desktop app writes.
        stdin
            .write_all(b"{\"type\":\"message\",\"msg_id\":\"m-1266\",\"content\":\"hello\"}\n")
            .expect("write message command");
        stdin.flush().expect("flush message command");
        // Hang up: the protocol loop ends at EOF, so the process exits on its
        // own once the turn is done and we never need a timeout.
    }
    let out = child.wait_with_output().expect("wait for wayland-core");
    Capture {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        status: out.status.code(),
    }
}

fn case_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wl-1266-c1-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create case dir");
    dir
}

/// A config that starts cleanly and CANNOT reach a real provider.
///
/// The endpoint override goes in `[providers.anthropic]`, not `[default]`.
/// This was measured, not assumed: an earlier draft put `base_url` under
/// `[default]`, where it is silently ignored, and the "unreachable endpoint"
/// control quietly contacted `api.anthropic.com` and graded a real 401. The
/// assertions still passed, which is exactly why the manual frame dump that
/// caught it is worth more than the green run that hid it.
fn base_config(extra: &str) -> String {
    format!(
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\n\
         [providers.anthropic]\nprovider = \"anthropic\"\n\
         base_url = \"http://127.0.0.1:1\"\n\n\
         [storage.credentials]\nbackend = \"plaintext\"\n\n\
         [session]\nenabled = false\n\n{extra}"
    )
}

/// c1's host half, end to end.
///
/// The engine classified this exit itself — it refused precisely BECAUSE the
/// window is unworkable — and the classification must survive all the way to
/// the bytes on the host's pipe.
#[test]
fn the_context_ceiling_refusal_reaches_the_real_host_as_context_limit() {
    let dir = case_dir("ceiling");
    let cap = run_one_turn(&dir, &base_config("[compact]\ncontext_window = 1024\n"));

    // Control: this must be an IN-BAND error, not a startup refusal. A startup
    // refusal never emits `ready`, and grading one would prove nothing about
    // the in-band seam c1 is about.
    assert_eq!(
        cap.frames_of_type("ready").len(),
        1,
        "control: the process must have STARTED and only then refused in band. \
         rc={:?}\nstdout:\n{}\nstderr tail:\n{}",
        cap.status,
        cap.stdout,
        cap.stderr
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .join("\n")
    );

    let errors = cap.frames_of_type("error");
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one in-band error frame, got {}. stdout:\n{}",
        errors.len(),
        cap.stdout
    );
    let error = &errors[0]["error"];
    // Control: the frame under test is the refusal, not some unrelated error
    // that happened to arrive first.
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|m| m.contains("cannot operate in a window that small")),
        "the graded frame must be the unworkable-window refusal: {error}"
    );
    assert_eq!(
        error["category"].as_str(),
        Some("context_limit"),
        "wayland#1266 c1: the engine knew this was a context ceiling and the \
         HOST must be told so — not handed prose with `unknown`. Frame was: \
         {error}"
    );
}

/// POSITIVE CONTROL, two jobs in one run.
///
/// 1. The harness can observe a healthy start, so the refusal above is a
///    refusal and not a broken invocation.
/// 2. The binary is not shipping one hardcoded category: this run reaches the
///    provider (a closed port), which is an opaque upstream core cannot
///    classify, and the host is told `unknown` — the honest answer #1237 c4
///    requires. Two runs of the same binary therefore put two DIFFERENT
///    categories on the wire.
#[test]
fn the_harness_observes_a_healthy_start_and_reports_an_opaque_failure_as_unknown() {
    let dir = case_dir("healthy");
    let cap = run_one_turn(&dir, &base_config(""));

    assert_eq!(
        cap.frames_of_type("ready").len(),
        1,
        "a healthy start must emit exactly one ready frame. rc={:?}\nstdout:\n{}\n\
         stderr tail:\n{}",
        cap.status,
        cap.stdout,
        cap.stderr
            .lines()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        cap.frames().len() > 1,
        "control: the stream must be substantive, not a lone frame:\n{}",
        cap.stdout
    );

    // Whatever this run reported, it must NOT have reported a context ceiling:
    // nothing here is near one. If it did, the arm above would be passing on a
    // constant rather than on a classification.
    let categories = cap.error_categories();
    // Vacuity guard. The loop below is an `assert_eq` over `categories`, which
    // passes trivially on an empty set. Port 1 on loopback refuses the
    // connection deterministically, so this run MUST have produced an opaque
    // upstream failure; if it did not, the control graded nothing.
    assert!(
        !categories.is_empty(),
        "control: the dead-endpoint run must have produced at least one error \
         frame, or the `unknown` assertion below proves nothing.\n{}",
        cap.stdout
    );
    assert!(
        !categories.iter().any(|c| c == "context_limit"),
        "a run with no window problem reported `context_limit`, so the seam is \
         carrying a constant: {categories:?}\n{}",
        cap.stdout
    );
    // And an opaque upstream must arrive as `unknown` rather than being given
    // a plausible-looking value on the user's behalf.
    for category in &categories {
        assert_eq!(
            category, "unknown",
            "an unreachable endpoint is the #1184 split core cannot decide, and \
             it must say `unknown` rather than guess: {categories:?}\n{}",
            cap.stdout
        );
    }
}
