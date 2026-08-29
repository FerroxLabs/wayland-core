//! F21-04-01 live wire proof — a nested budget event must be attributable to
//! the child that raised it ON THE SHIPPED `--json-stream` WIRE.
//!
//! This drives the real `wayland-core` binary, not a seam. A parent turn calls
//! `Spawn`; the spawned child's first provider reservation trips the session's
//! output-token cap (the parent's own turn already consumed part of it), so the
//! child engine raises `budget_exceeded` on its `OutputSink` — which, for a
//! spawned child, is the `ChannelSink`.
//!
//! Before the repair the sink had no `emit_budget_exceeded` override, so the
//! trait's empty default body discarded it and the only thing a host saw was
//! the free-form `error` string that follows the cap site. After the repair the
//! structured event rides the existing `sub_agent_event` envelope and carries
//! the raising child's `parent_call_id` + `agent_name`.
//!
//! **Anti-vacuity gate.** The assertion on the structured event is only reached
//! once the run has PROVED that a child actually hit the cap (its relayed error
//! names the cap) and that the run really landed in host-protocol mode (the
//! `ready` frame). An absent child is recorded as an inconclusive run rather
//! than being read as a missing observable — Phase 21 lost a whole corpus to
//! exactly that confusion.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;
use support::{mock_llm, pty, vault};

/// Marker planted in the child's prompt so the mock can tell parent from child.
const CHILD_MARKER: &str = "F210401CHILD";
const CHILD_NAME: &str = "capped";
/// Session output-token cap. The parent reserves `max_tokens` (50) and settles
/// at the mock's 25 output tokens, both under the cap; the child then reserves
/// `DEFAULT_SUB_AGENT_MAX_TOKENS` (4096), which cannot fit.
const SESSION_OUTPUT_TOKEN_CAP: u64 = 100;
const PARENT_MAX_TOKENS: u32 = 50;
const RUN_BUDGET: Duration = Duration::from_secs(45);

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

fn write_config(home: &Path, base_url: &str) {
    let toml = format!(
        "[default]\n\
         provider = \"anthropic\"\n\
         model = \"claude-sonnet-4-20250514\"\n\
         max_tokens = {PARENT_MAX_TOKENS}\n\
         \n[providers.anthropic]\n\
         api_key = \"sk-ant-harness-not-real-key-0000000000\"\n\
         base_url = \"{base_url}\"\n\
         \n[budget]\n\
         max_tokens_out = {SESSION_OUTPUT_TOKEN_CAP}\n"
    );
    std::fs::write(home.join("config.toml"), toml).expect("write config.toml");
}

/// Parent: one `Spawn` call, then a closing line. Child: never expected to be
/// served (it is refused admission before its provider call), but scripted so a
/// run that DOES reach the provider still terminates instead of hanging.
fn start_mock(rt: &tokio::runtime::Runtime) -> MockServer {
    let server = rt.block_on(MockServer::start());
    let parent: Vec<String> = vec![
        mock_llm::tool_use_turn_sse(
            "Spawn",
            &json!({
                "tasks": [{
                    "name": CHILD_NAME,
                    "prompt": format!("{CHILD_MARKER}: report your result"),
                }]
            }),
        ),
        mock_llm::text_turn_sse("parent finished"),
    ];
    let child: Vec<String> = vec![mock_llm::text_turn_sse("child finished")];
    let cursors = std::sync::Arc::new(std::sync::Mutex::new([0usize; 2]));

    let responder = move |request: &wiremock::Request| {
        let who = serde_json::from_slice::<Value>(&request.body)
            .ok()
            .and_then(|body| {
                let first = body.get("messages")?.get(0)?.to_string();
                Some(usize::from(first.contains(CHILD_MARKER)))
            })
            .unwrap_or(0);
        let script = if who == 1 { &child } else { &parent };
        let mut cursor = cursors.lock().expect("cursor lock");
        let index = cursor[who].min(script.len().saturating_sub(1));
        cursor[who] = (cursor[who] + 1).min(script.len());
        ResponseTemplate::new(200).set_body_raw(script[index].clone(), "text/event-stream")
    };

    rt.block_on(
        Mock::given(matchers::method("POST"))
            .and(matchers::path("/v1/messages"))
            .respond_with(responder)
            .mount(&server),
    );
    server
}

#[derive(Default)]
struct WireRun {
    transcript: String,
    saw_ready: bool,
    /// Relayed child frames: (parent_call_id, agent_name, inner).
    sub_agent_frames: Vec<(String, String, Value)>,
}

impl WireRun {
    /// Did a child actually hit the cap? Read from the free-form error that
    /// every `emit_budget_exceeded` site pairs with, which was already relayed
    /// before this repair — so this gate is independent of the fix under test.
    fn child_hit_the_cap(&self) -> bool {
        self.sub_agent_frames.iter().any(|(_, _, inner)| {
            inner["type"] == "error"
                && inner["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("budget cap"))
        })
    }

    fn budget_frames(&self) -> Vec<&(String, String, Value)> {
        self.sub_agent_frames
            .iter()
            .filter(|(_, _, inner)| inner["type"] == "budget_exceeded")
            .collect()
    }
}

fn drive(home: &Path) -> WireRun {
    let mut command = Command::new(binary());
    command
        .args(["--json-stream", "--provider", "anthropic"])
        .current_dir(home);
    pty::harden_child_env(&mut command, home);
    // Without an ephemeral encrypted vault the binary refuses to start a session
    // under a hermetic WAYLAND_HOME, so the turn never reaches a provider and
    // every observation would be an absence rather than a verdict.
    let guard = vault::configure_process(&mut command);
    let mut child = OwnedTree::new(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the shipped binary must spawn"),
    );
    drop(guard);

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let stderr_sink = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let stderr_writer = std::sync::Arc::clone(&stderr_sink);
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Ok(mut buffer) = stderr_writer.lock() {
                buffer.push_str(&line);
                buffer.push('\n');
            }
        }
    });
    let _ = writeln!(
        stdin,
        "{{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"spawn the capped task\"}}"
    );

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut run = WireRun::default();
    let mut saw_parent_close = false;
    let deadline = Instant::now() + RUN_BUDGET;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => {
                run.transcript.push_str(&line);
                run.transcript.push('\n');
                let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match frame["type"].as_str() {
                    Some("ready") => run.saw_ready = true,
                    // Answer the gate the way a real host does. A driver that
                    // never answers parks the delegation forever and turns every
                    // downstream observation into an absence — inconclusive in
                    // the direction that looks like correctness.
                    Some("approval_required") => {
                        if let Some(call_id) = frame["call_id"].as_str() {
                            let _ = writeln!(
                                stdin,
                                "{{\"type\":\"tool_approve\",\"call_id\":\"{call_id}\"}}"
                            );
                        }
                    }
                    Some("sub_agent_event") => run.sub_agent_frames.push((
                        frame["parent_call_id"].as_str().unwrap_or_default().into(),
                        frame["agent_name"].as_str().unwrap_or_default().into(),
                        frame["inner"].clone(),
                    )),
                    Some("text_delta")
                        if frame["text"]
                            .as_str()
                            .is_some_and(|text| text.contains("parent finished")) =>
                    {
                        saw_parent_close = true;
                    }
                    _ => {}
                }
                // `stream_end` fires per assistant STREAM, not per turn, so the
                // parent's very first response already carries one while the
                // child is still working. End on the parent's closing text,
                // which can only follow the Spawn tool returning.
                if saw_parent_close {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();
    if let Ok(buffer) = stderr_sink.lock() {
        run.transcript.push_str("--- stderr ---\n");
        run.transcript.push_str(&buffer);
    }
    run
}

#[test]
fn child_budget_event_is_attributable_on_the_shipped_json_stream_wire() {
    let home = TempDir::new().expect("hermetic home");
    let rt = runtime();
    let server = start_mock(&rt);
    write_config(home.path(), &server.uri());

    let run = drive(home.path());

    // Mode gate: the `ready` frame is emitted only by the json-stream
    // front-end, so its presence proves the run did not fall through to the
    // line REPL and record a REPL absence as a protocol absence.
    assert!(
        run.saw_ready,
        "run never proved host-protocol mode; transcript:\n{}",
        run.transcript
    );

    // Anti-vacuity gate: a child must have existed AND hit the cap. This reads
    // the free-form error that pre-dates the repair, so it cannot be satisfied
    // by the repair itself.
    assert!(
        run.child_hit_the_cap(),
        "no child reached the budget cap, so this run cannot decide whether the \
         per-child observable exists; transcript:\n{}",
        run.transcript
    );

    let budget = run.budget_frames();
    assert!(
        !budget.is_empty(),
        "F21-04-01: a child hit the budget cap but the host protocol carried no \
         per-child `budget_exceeded` observable; transcript:\n{}",
        run.transcript
    );

    for (parent_call_id, agent_name, inner) in budget {
        assert!(
            parent_call_id.starts_with("spawn:"),
            "the nested budget event must name the child that raised it, got \
             parent_call_id={parent_call_id:?}"
        );
        assert_eq!(agent_name, CHILD_NAME);
        assert!(
            inner["reason"].is_string()
                && inner["observed"].is_string()
                && inner["limit"].is_string(),
            "the relayed event must keep its structured triple, got {inner}"
        );
    }
}
