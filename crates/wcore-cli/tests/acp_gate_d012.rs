//! D012 (P0 security) — the `--json-stream` protocol front-end must GATE a
//! mutating tool: a host must observe an approval/gate event BEFORE the tool
//! executes when the session runs in the default (non-Force) approval posture,
//! and must NOT see that gate under `--force` (the operator opted into
//! auto-approval).
//!
//! This is the dedicated D012 integration check. It is deliberately separate
//! from `smoke_p0.rs::gap_d012` (the release-gate smoke harness the
//! orchestrator un-ignores after the fix) so the two cannot drift: this file
//! pins BOTH legs of the contract (gated-emits-approval AND
//! force-does-not-emit) and the file-write ordering, while the smoke gate pins
//! the single end-to-end release assertion.
//!
//! ## Hermetic by construction
//!
//! Mirrors the smoke harness: every child points `WAYLAND_HOME` + `HOME` at a
//! throwaway tempdir and strips the full provider-credential env set, so a run
//! can neither read nor mutate the developer's real config/keys. The mock
//! provider scripts a `Write` tool call so no real provider is contacted.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{Duration, Instant};

#[path = "support/mod.rs"]
mod support;

use support::mock_llm::MockLlm;
use tempfile::TempDir;

/// Path to the debug binary under test (Cargo wires this env var).
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// The provider-credential env-var set every spawned child must NOT inherit,
/// so a run can neither read the developer's real keys nor auto-detect a stray
/// dev credential. Mirrors `smoke_p0.rs::STRIPPED_PROVIDER_ENV`.
const STRIPPED_PROVIDER_ENV: &[&str] = &[
    "API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "VERTEX_PROJECT",
    "VERTEX_LOCATION",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// Apply the hermetic child env uniformly: point `WAYLAND_HOME` + `HOME` at the
/// throwaway tempdir, set a deterministic `TERM`, and strip every credential.
fn harden_child_env(cmd: &mut std::process::Command, home: &Path) {
    cmd.env("WAYLAND_HOME", home)
        .env("HOME", home)
        .env("TERM", "dumb");
    for key in STRIPPED_PROVIDER_ENV {
        cmd.env_remove(key);
    }
}

/// Seed `<home>/config.toml` for a provider/model routed at the mock.
fn write_config(home: &Path, base_url: &str) {
    let toml = format!(
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\
         \n[providers.anthropic]\napi_key = \"sk-ant-harness-not-real-key-0000000000\"\n\
         base_url = \"{base_url}\"\n"
    );
    std::fs::write(home.join("config.toml"), toml).expect("write config.toml");
}

/// Outcome of driving one `--json-stream` Write turn: whether a gate
/// (`approval`/`permission`/`gate`) event line was observed before the probe
/// file appeared, and whether the probe was ever written.
struct GateProbe {
    saw_gate: bool,
    file_written: bool,
}

/// Drive a single mutating-`Write` turn over `--json-stream` with the supplied
/// extra args (e.g. `["--force"]`). Returns whether an approval/gate event was
/// observed BEFORE the probe file was written, and whether it was written at
/// all. Fails closed in the assertion sense: if neither a gate nor a write
/// happens within the window, `saw_gate` and `file_written` are both false.
fn drive_write_turn(extra_args: &[&str]) -> GateProbe {
    let home = TempDir::new().expect("tempdir");
    let probe = home.path().join("d012_probe.txt");
    let probe_arg = probe.to_str().expect("utf-8 path").to_string();

    let mock = MockLlm::new()
        .tool_use(
            "Write",
            serde_json::json!({ "file_path": probe_arg, "content": "D012_PROBE" }),
        )
        .text("done");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(mock.start());
    write_config(home.path(), &server.uri());

    let mut args = vec!["--json-stream", "--provider", "anthropic"];
    args.extend_from_slice(extra_args);
    let mut cmd = std::process::Command::new(binary());
    cmd.args(&args).current_dir(home.path());
    harden_child_env(&mut cmd, home.path());
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn --json-stream");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    writeln!(
        stdin,
        "{{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"write the probe\"}}"
    )
    .expect("write message");

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    // The security ordering: a gate event must reach the host BEFORE the tool
    // writes the file. We watch both and record which came first.
    //
    // Match the genuine gate EVENT (`{"type":"approval_required",...}`), not a
    // loose `contains("approval")` substring. The `Ready` frame emitted at
    // startup advertises `"tool_approval":true` in its capabilities — a bare
    // substring scan matches that capability advertisement in BOTH postures,
    // so it cannot distinguish a real gate from the unconditional capability
    // banner (it false-passed Default for the wrong reason and false-failed
    // the Force leg, which emits no gate at all). Keying on the event `type`
    // pins the actual security control: an `approval_required` event.
    let mut saw_gate = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let l = line.to_lowercase();
                if l.contains("\"type\":\"approval_required\"") {
                    saw_gate = true;
                    break;
                }
                // If the tool already executed (file present) before any gate
                // line, the gate did not precede execution — stop and report.
                if probe.exists() {
                    break;
                }
            }
            Err(_) => {
                if probe.exists() {
                    break;
                }
            }
        }
    }
    // Under --force the tool auto-approves: no gate is ever emitted, so the
    // file appears instead. Give it a brief settle window so the force leg can
    // observe the write deterministically.
    if !saw_gate {
        let settle = Instant::now() + Duration::from_secs(5);
        while Instant::now() < settle && !probe.exists() {
            let _ = rx.recv_timeout(Duration::from_millis(100));
        }
    }

    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();

    GateProbe {
        saw_gate,
        file_written: probe.exists(),
    }
}

/// D012 PART A — the gated (default posture, no `--force`) `--json-stream` path
/// MUST emit an `approval_required` event for a mutating `Write` BEFORE the
/// tool runs. The engine's orchestration gate emits only `ToolRequest`
/// (`tool_request` — no approval vocabulary); the `GatingProtocolWriter`
/// installed on the json-stream path synthesizes the host-visible
/// `approval_required` gate frame right after it, so a host sees the control.
#[test]
fn d012_json_stream_gated_write_emits_approval_before_execution() {
    let probe = drive_write_turn(&[]);
    assert!(
        probe.saw_gate,
        "a mutating Write over --json-stream (default posture) must emit an \
         approval/gate event BEFORE executing; none seen (the gate is invisible \
         to the host — D012). file_written={}",
        probe.file_written
    );
}

/// D012 PART A (negative leg) — under `--force` the operator opted into
/// auto-approval, so the engine must NOT emit an approval gate; the Write rides
/// straight through and the probe file is written. This pins that the gate is
/// posture-driven (a genuine Force-vs-default delta), not unconditional.
#[test]
fn d012_json_stream_force_does_not_gate_write() {
    let probe = drive_write_turn(&["--force"]);
    assert!(
        !probe.saw_gate,
        "under --force the engine must auto-approve and emit NO approval gate \
         for the Write; a gate event was seen"
    );
    assert!(
        probe.file_written,
        "under --force the auto-approved Write must execute (probe file must \
         exist); it did not"
    );
}
