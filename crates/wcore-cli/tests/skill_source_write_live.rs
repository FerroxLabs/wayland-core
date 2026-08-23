//! FerroxLabs/wayland#1096, suggested direction 2 — LIVE binary leg.
//!
//! The 2026-08-19 UAT symptom, reproduced through the REAL `wayland-core`
//! binary instead of a hand-assembled VFS stack: a skill produces an HTML
//! report and the model writes it next to the skill's own `SKILL.md`, which
//! lives in the global config dir — outside the session workspace entirely.
//! The file lands, the producing session cannot read it back, and nothing said
//! anything.
//!
//! Why a binary-level test and not only the `wcore-agent` one: the guard is
//! installed by `bootstrap.rs`, and a guard that is correct but not installed
//! grades exactly like a guard that is absent. This drives the same seam
//! `smoke_p0.rs::smoke_17_*` uses — `--json-stream --force` against the
//! scriptable `MockLlm` — so the Write really is dispatched by the live engine
//! through the live tool registry, with zero provider spend.

#[path = "support/mod.rs"]
mod support;

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use support::mock_llm::MockLlm;
use tempfile::TempDir;
use wiremock::MockServer;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Credential env every spawned child must NOT inherit, so the run can neither
/// read the developer's real keys nor have onboarding auto-detect a stray one.
/// Same set and same reason as `smoke_p0.rs::STRIPPED_PROVIDER_ENV`.
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

fn write_config(home: &Path, base_url: &str) {
    let toml = format!(
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\n\
         [providers.anthropic]\napi_key = \"sk-ant-harness-not-real-key-0000000000\"\n\
         base_url = \"{base_url}\"\n"
    );
    std::fs::write(home.join("config.toml"), toml).expect("write config.toml");
}

fn start_mock(mock: MockLlm) -> (tokio::runtime::Runtime, MockServer) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(mock.start());
    (rt, server)
}

/// Drive one scripted `Write` through the live binary and return everything the
/// json-stream said. Turn 1 is the tool call; turn 2 is closing text, which the
/// engine reaches whether the tool succeeded or was refused — so waiting for it
/// is a turn-completion signal in BOTH arms, not a success signal.
fn run_scripted_write(home: &Path, cwd: &Path, target: &Path, body: &str) -> String {
    let (_rt, server) = start_mock(
        MockLlm::new()
            .tool_use(
                "Write",
                serde_json::json!({
                    "file_path": target.to_string_lossy(),
                    "content": body,
                }),
            )
            .text("TURN-DONE"),
    );
    write_config(home, &server.uri());

    let mut cmd = std::process::Command::new(binary());
    cmd.args([
        "--json-stream",
        "--force",
        "--trust-workspace",
        "--provider",
        "anthropic",
    ])
    .current_dir(cwd)
    .env("WAYLAND_HOME", home)
    .env("HOME", home)
    .env("TERM", "dumb");
    for key in STRIPPED_PROVIDER_ENV {
        cmd.env_remove(key);
    }
    let vault = support::vault::configure_process(&mut cmd);
    let child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    drop(vault);
    let mut child = child.expect("spawn --json-stream --force");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    writeln!(
        stdin,
        "{{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"run the skill and save the report\"}}"
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

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut transcript = String::new();
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let done = line.contains("TURN-DONE");
                transcript.push_str(&line);
                transcript.push('\n');
                if done {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();
    transcript
}

/// THE UAT PATH, live. `market-open-report` produced `morning-brief.html` and
/// put it in its own SOURCE directory under the global config dir.
#[test]
fn the_live_binary_refuses_a_report_written_into_the_user_skills_dir() {
    let home = TempDir::new().expect("home");
    let workspace = TempDir::new().expect("workspace");
    let target = home
        .path()
        .join("skills")
        .join("market-open-report")
        .join("morning-brief.html");

    let transcript = run_scripted_write(
        home.path(),
        workspace.path(),
        &target,
        "<html>morning brief</html>",
    );

    assert!(
        !target.exists(),
        "the live engine wrote the report into the skill's own SOURCE directory \
         ({}) — outside the session workspace ({}), where the producing session \
         cannot read it back. transcript:\n{transcript}",
        target.display(),
        workspace.path().display(),
    );
    assert!(
        transcript.contains(".wayland-out") || transcript.contains("WCORE_SKILL_OUTPUT_DIR"),
        "the refusal must SAY where the file belongs, not merely deny. transcript:\n{transcript}"
    );
}

/// KNOWN-POSITIVE CONTROL for the assertion above. The same harness, the same
/// scripted tool call, one directory different: a write into the session
/// workspace must LAND. Without this, `!target.exists()` would also pass if the
/// binary never started, the mock was never reached, or the Write was never
/// dispatched at all.
#[test]
fn the_same_harness_writes_a_report_into_the_workspace() {
    let home = TempDir::new().expect("home");
    let workspace = TempDir::new().expect("workspace");
    let target = workspace.path().join("morning-brief.html");

    let transcript = run_scripted_write(
        home.path(),
        workspace.path(),
        &target,
        "<html>morning brief</html>",
    );

    assert!(
        target.exists(),
        "control failed: the harness could not write even into the workspace, so \
         a missing file in the sibling test proves nothing. transcript:\n{transcript}"
    );
}
