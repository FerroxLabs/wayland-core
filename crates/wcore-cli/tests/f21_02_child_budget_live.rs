//! F21-02 LIVE — a delegated child runs inside the envelope its delegator
//! sub-allocated to it, on the shipped `wayland-core` binary.
//!
//! # Why this file exists
//!
//! F21-02 ("nested children cannot exceed parent depth, fan-out, concurrency,
//! token, cost, or time reservations") was graded NOT MET three times because it
//! was VACUOUSLY true: no shipped surface carried a child-fillable budget field,
//! so the property held because nothing could ask rather than because anything
//! refused. A suite written against that state passes green forever and
//! distinguishes "refused" from "never requested" not at all.
//!
//! This drives the real binary over `acp serve`, has the PARENT'S OWN MODEL fill
//! the `budget` object on a `Delegate` call — the request is authored by the LLM,
//! not injected by trusted harness code — and reads the consequence off the
//! provider wire.
//!
//! # The differential, which is the whole point
//!
//! Two runs, byte-identical except for the presence of the `budget` object in
//! the parent's `Delegate` input:
//!
//! * NARROWED — the delegator asks for `max_tokens_in: 900`. The child is cut
//!   off partway through its script.
//! * CONTROL  — no `budget` object. The same child script runs to completion
//!   against the session root's 100_000.
//!
//! Neither run's number means anything alone. A low count in the narrowed run
//! could just as easily be a broken harness, and that is precisely how a
//! vacuous suite fools a reader. It is the GAP between them that can only exist
//! if the request was carried, resolved and enforced. Delete the channel, or
//! revert the spawn seam to `sub_budget(None)`, and the two runs converge and
//! this test goes red.
//!
//! # Anti-vacuity
//!
//! The child must have EXISTED and taken its own provider turns, or "the child
//! stopped early" is a statement about the harness. Both runs assert a non-zero
//! child turn count before any verdict is read, and the control asserts the
//! child ran to the END of its script — so a blanket refusal, or a harness that
//! never reaches the spawn seam, fails the control rather than passing the
//! narrowed case for the wrong reason.
//!
//! Hermetic: `WAYLAND_HOME`/`HOME` are a throwaway tempdir, every provider
//! credential env var is stripped, and the provider `base_url` is a local
//! wiremock. Nothing leaves the machine.

// Unix-only for the same reason as `f21_02_01_child_tool_authority.rs`: this
// spawns a real server. The seam-level regressions in `wcore-agent::spawner`
// and `wcore-budget::execution` are platform-neutral and run everywhere.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;
use tempfile::TempDir;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

#[path = "support/mod.rs"]
mod support;

/// Marker prefixed onto the delegated goal, so a provider request whose FIRST
/// user message carries it is known to be the CHILD's.
const CHILD_GOAL: &str = "F2102CHILDGOAL";

/// Input tokens the mock charges for every child turn.
const CHILD_TOKENS_PER_TURN: u64 = 400;

/// The envelope the parent's model sub-allocates. Two turns fit (800); the
/// third (1200) does not.
const NARROWED_TOKENS_IN: u64 = 900;

/// The session root, set in config.toml. Loose enough that it cannot be what
/// stops the narrowed child.
const ROOT_TOKENS_IN: u64 = 100_000;

/// How many turns the child's script would take if nothing stopped it.
const CHILD_SCRIPT_TURNS: usize = 8;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// One assistant turn that charges `input_tokens`, either a tool call or text.
///
/// The shared `support::mock_llm` helpers hardcode a usage of 1 input token,
/// which is too small to exercise a token envelope in a bounded number of
/// turns. This is the same SSE framing with the usage made explicit.
fn turn_sse(input_tokens: u64, tool: Option<(&str, &serde_json::Value)>) -> String {
    let (block_start, deltas, stop_reason) = match tool {
        Some((name, input)) => {
            let payload = serde_json::to_string(input).expect("serialize tool input");
            (
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use", "id": "toolu_f2102", "name": name, "input": {}
                    }
                }),
                json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": { "type": "input_json_delta", "partial_json": payload }
                }),
                "tool_use",
            )
        }
        None => (
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" }
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "done" }
            }),
            "end_turn",
        ),
    };
    let message_start = json!({
        "type": "message_start",
        "message": {
            "id": "msg_f2102",
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": "claude-mock",
            "stop_reason": serde_json::Value::Null,
            "stop_sequence": serde_json::Value::Null,
            "usage": { "input_tokens": input_tokens, "output_tokens": 1 }
        }
    });
    let message_delta = json!({
        "type": "message_delta",
        "delta": { "stop_reason": stop_reason, "stop_sequence": serde_json::Value::Null },
        "usage": { "output_tokens": 1 }
    });
    format!(
        "event: message_start\ndata: {message_start}\n\n\
         event: content_block_start\ndata: {block_start}\n\n\
         event: content_block_delta\ndata: {deltas}\n\n\
         event: content_block_stop\ndata: {}\n\n\
         event: message_delta\ndata: {message_delta}\n\n\
         event: message_stop\ndata: {}\n\n",
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({ "type": "message_stop" }),
    )
}

/// How many provider turns the CHILD was served, read off the real wire.
fn child_turn_count(mock: &MockServer, rt: &tokio::runtime::Runtime) -> usize {
    rt.block_on(async {
        mock.received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|r| {
                serde_json::from_slice::<serde_json::Value>(&r.body)
                    .ok()
                    .and_then(|body| Some(body.get("messages")?.get(0)?.to_string()))
                    .is_some_and(|first| first.contains(CHILD_GOAL))
            })
            .count()
    })
}

/// Route by generation, not by queue order.
///
/// `budget` is the object the PARENT'S MODEL puts on its `Delegate` call. In the
/// control it is absent. That is the only difference between the two runs.
async fn start_routed_mock(budget: Option<serde_json::Value>) -> MockServer {
    let server = MockServer::start().await;

    let mut delegate_input = json!({
        "goal": format!("{CHILD_GOAL}: read the fixture repeatedly"),
        "toolsets": ["Read"],
    });
    if let Some(budget) = budget {
        delegate_input["budget"] = budget;
    }
    let parent: Vec<String> = vec![
        turn_sse(1, Some(("Delegate", &delegate_input))),
        turn_sse(1, None),
    ];

    // The child reads a file it is allowed to read, over and over, so it takes
    // real provider turns that charge real tokens. The final turn is text so an
    // unbounded child terminates on its own rather than on `max_turns`.
    let read_input = json!({ "file_path": "README.f2102" });
    let mut child: Vec<String> = (0..CHILD_SCRIPT_TURNS - 1)
        .map(|_| turn_sse(CHILD_TOKENS_PER_TURN, Some(("Read", &read_input))))
        .collect();
    child.push(turn_sse(CHILD_TOKENS_PER_TURN, None));

    let cursors = std::sync::Arc::new(std::sync::Mutex::new([0usize; 2]));
    let responder = move |request: &wiremock::Request| {
        let is_child = serde_json::from_slice::<serde_json::Value>(&request.body)
            .ok()
            .and_then(|body| Some(body.get("messages")?.get(0)?.to_string()))
            .is_some_and(|first| first.contains(CHILD_GOAL));
        let generation = usize::from(is_child);
        let script = if is_child { &child } else { &parent };
        let mut cursor = cursors.lock().expect("cursor lock");
        let index = cursor[generation].min(script.len().saturating_sub(1));
        cursor[generation] = (cursor[generation] + 1).min(script.len());
        ResponseTemplate::new(200).set_body_raw(script[index].clone(), "text/event-stream")
    };

    Mock::given(matchers::method("POST"))
        .and(matchers::path("/v1/messages"))
        .respond_with(responder)
        .mount(&server)
        .await;
    server
}

/// Config with an explicit session-root token envelope. The root is what the
/// sub-allocation is intersected AGAINST, and what the control child runs under.
fn write_config_with_root_budget(home: &Path, base_url: &str) {
    let toml = format!(
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\n\
         [providers.anthropic]\napi_key = \"sk-ant-harness-not-real-key-0000000000\"\n\
         base_url = \"{base_url}\"\n\n\
         [budget]\nmax_tokens_in = {ROOT_TOKENS_IN}\nmax_agent_depth = 4\n"
    );
    std::fs::write(home.join("config.toml"), toml).expect("write config.toml");
}

fn init_workspace(home: &Path) -> std::path::PathBuf {
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).expect("create the workspace");
    std::fs::write(
        workspace.join("README.f2102"),
        b"f21-02 budget fixture: the file the delegated child reads.\n",
    )
    .expect("write the fixture the child reads");
    workspace
}

/// How long [`AcpServer::post`] may spend reading one prompt response.
///
/// A DELIBERATE CAP, set BELOW nextest's kill rather than above the operation
/// it reads. Two measured facts put it there:
///
///   * A prompt turn contains at least one tool dispatch, and
///     `tool_dispatch_timeout` (crates/wcore-agent/src/orchestration/mod.rs)
///     allows a `ToolCategory::Exec` dispatch 600s. A turn is that plus model
///     latency, so NO read budget can be proved to cover one. This can only be
///     a cap that says so when it binds — which is what `post` now does.
///   * nextest hard-kills this binary at 180s under `--profile ci`
///     (`[profile.ci] slow-timeout = { period = "90s", terminate-after = 2 }`)
///     and at 60s under the default profile. The previous 180s budget was
///     therefore UNREACHABLE under ci: the prompt read starts seconds into the
///     test, so nextest always won the race and the budget never fired.
///     Raising it to 600s to "match" the Exec ceiling would deepen that — a
///     number that cannot be reached certifies nothing.
///
/// 150s leaves ~30s of headroom under the ci kill, so the diagnostic in `post`
/// is reachable on the profile CI actually runs.
const PROMPT_READ_BUDGET: Duration = Duration::from_secs(150);

struct AcpServer {
    child: std::process::Child,
    addr: String,
    key: String,
    _vault: support::vault::VaultGuard,
}

impl AcpServer {
    fn post(&self, path: &str, body: &serde_json::Value, read_for: Duration) -> String {
        let mut stream = TcpStream::connect(&self.addr).expect("connect to acp serve");
        stream
            .set_read_timeout(Some(read_for))
            .expect("set the read timeout");
        let payload = body.to_string();
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nX-API-Key: {}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            self.addr,
            self.key,
            payload.len()
        );
        stream
            .write_all(request.as_bytes())
            .expect("write the request");
        stream.flush().expect("flush the request");
        let mut raw = Vec::new();
        let started = Instant::now();
        // NEVER `let _ =` here. `read_to_end` returns `Ok` only on a clean
        // EOF, which IS the normal path: the server closes a finished response
        // (`Connection: close`). Any error means `raw` holds a PREFIX, and a
        // prefix returned as a plain `String` is indistinguishable from a
        // whole response — the session-create call then dies inside
        // `json_body` blaming the server for invalid JSON, and the prompt call
        // fails an assertion blaming the product for a frame the harness
        // simply never read. Both misattribute a harness timeout to the code
        // under test, which is worse than a red test.
        if let Err(err) = stream.read_to_end(&mut raw) {
            // Read-timeout expiry is `WouldBlock` on Unix and `TimedOut` on
            // Windows (`TcpStream::set_read_timeout`); CI runs both.
            let timed_out = matches!(
                err.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            );
            panic!(
                "POST {path} was not read to completion after {:?} (budget \
                 {read_for:?}, timed out: {timed_out}, io error: {err}). The {} \
                 bytes received are a PREFIX of the response, not the response. \
                 A single `ToolCategory::Exec` dispatch is allowed 600s \
                 (wcore_agent tool_dispatch_timeout), so a legitimately slow \
                 turn can outlast this budget: raise the budget at the call \
                 site and this binary's nextest slow-timeout TOGETHER, rather \
                 than reading the partial body as a result. Partial body: {:?}",
                started.elapsed(),
                raw.len(),
                String::from_utf8_lossy(&raw),
            );
        }
        String::from_utf8_lossy(&raw).into_owned()
    }
}

impl Drop for AcpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn json_body(response: &str) -> serde_json::Value {
    let body = response
        .split_once("\r\n\r\n")
        .map_or(response, |(_, rest)| rest);
    serde_json::from_str(body.trim())
        .unwrap_or_else(|e| panic!("response body was not json ({e}): {response}"))
}

fn spawn_acp(home: &Path, cwd: &Path) -> AcpServer {
    let key = "f2102-live-server-key".to_owned();
    let mut cmd = Command::new(binary());
    cmd.args([
        "acp",
        "serve",
        "--bind",
        "127.0.0.1:0",
        // The child must actually RUN its Read calls; without this every turn
        // stops at an approval frame and no tokens are ever charged.
        "--allow-all-tools",
    ])
    .current_dir(cwd)
    .env("WAYLAND_ACP_SERVER_KEY", &key)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    support::pty::harden_child_env(&mut cmd, home);
    let vault = support::vault::configure_process(&mut cmd);
    let mut child = cmd.spawn().expect("spawn acp serve");

    let stderr = child.stderr.take().expect("acp serve stderr");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            eprintln!("[acp serve] {line}");
            if let Some(rest) = line.split("serving on http://").nth(1) {
                let addr: String = rest
                    .chars()
                    .take_while(|c| !c.is_whitespace())
                    .collect::<String>()
                    .trim_end_matches(['.', ','])
                    .to_owned();
                let _ = tx.send(addr);
            }
        }
    });
    let addr = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("acp serve reported its bound address");
    AcpServer {
        child,
        addr,
        key,
        _vault: vault,
    }
}

fn drive_one_turn(server: &AcpServer) -> String {
    let created = json_body(&server.post("/v1/sessions", &json!({}), Duration::from_secs(60)));
    let session_id = created
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("session/create did not return a session id: {created}"))
        .to_owned();
    server.post(
        &format!("/v1/sessions/{session_id}/prompt"),
        &json!({ "text": "delegate the read loop" }),
        PROMPT_READ_BUDGET,
    )
}

/// Run one live scenario and return `(child turns served, transcript)`.
fn run_live(budget: Option<serde_json::Value>) -> (usize, String) {
    let home = TempDir::new().expect("hermetic home");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mock = rt.block_on(start_routed_mock(budget));

    let workspace = init_workspace(home.path());
    write_config_with_root_budget(home.path(), &mock.uri());

    let server = spawn_acp(home.path(), &workspace);
    let transcript = drive_one_turn(&server);
    (child_turn_count(&mock, &rt), transcript)
}

/// THE LIVE VERDICT.
///
/// Both scenarios run in one test so the comparison is between two runs of the
/// same binary in the same session, and so the differential can never be
/// reported from a stale or absent counterpart.
#[test]
fn f21_02_a_delegated_child_is_bound_by_the_envelope_its_delegator_sub_allocated() {
    let (control_turns, control_transcript) = run_live(None);
    let (narrowed_turns, narrowed_transcript) = run_live(Some(json!({
        "max_tokens_in": NARROWED_TOKENS_IN
    })));

    // Anti-vacuity gate 1 — a child existed and took its own provider turns in
    // BOTH runs. Without this, "the narrowed child stopped early" is a claim
    // about the harness rather than about the product.
    assert!(
        control_turns > 0,
        "no delegated child provider turn was served in the CONTROL run, so this \
         measures nothing. transcript: {control_transcript}"
    );
    assert!(
        narrowed_turns > 0,
        "no delegated child provider turn was served in the NARROWED run, so its \
         low turn count would be indistinguishable from the child never running \
         at all — exactly the confound this phase keeps hitting. transcript: \
         {narrowed_transcript}"
    );

    // Anti-vacuity gate 2 — the control child ran its script to completion
    // against the 100_000 root. A blanket refusal, or a harness that never
    // reaches the spawn seam, fails HERE rather than silently making the
    // narrowed assertion below pass for the wrong reason.
    assert_eq!(
        control_turns, CHILD_SCRIPT_TURNS,
        "the CONTROL child was served {control_turns} of {CHILD_SCRIPT_TURNS} scripted \
         turns under a {ROOT_TOKENS_IN}-token root. Something other than the \
         sub-allocation is cutting children short, so the narrowed run cannot be \
         attributed to the envelope. transcript: {control_transcript}"
    );

    // THE VERDICT. The delegator asked for 900 input tokens; the child charges
    // 400 a turn, so it may take two and is refused the third.
    let permitted = (NARROWED_TOKENS_IN / CHILD_TOKENS_PER_TURN + 1) as usize;
    assert!(
        narrowed_turns <= permitted,
        "the NARROWED child was served {narrowed_turns} turns at {CHILD_TOKENS_PER_TURN} \
         input tokens each, against a sub-allocated envelope of {NARROWED_TOKENS_IN}. \
         The envelope its delegator requested did not bind it. transcript: \
         {narrowed_transcript}"
    );
    assert!(
        narrowed_turns < control_turns,
        "the narrowed child ({narrowed_turns} turns) ran as long as the control \
         child ({control_turns} turns). The two runs differ ONLY by the `budget` \
         object on the parent's Delegate call, so identical counts mean the \
         request was never carried to the spawn seam and F21-02 is satisfied by \
         the ABSENCE of a request channel — the exact vacuity this test exists \
         to detect."
    );

    eprintln!(
        "F21-02 LIVE: control child served {control_turns} turns, narrowed child \
         served {narrowed_turns} turns under a {NARROWED_TOKENS_IN}-token \
         sub-allocation of a {ROOT_TOKENS_IN}-token root."
    );
}

/// A delegator cannot buy itself a bigger envelope by asking for one.
///
/// Stated honestly: this leg is the WEAKER of the two. `sub_budget(Some(wider))`
/// could never amplify consumption in the first place, because the ancestor
/// chain is consulted by every admission path regardless of what the child's own
/// caps say. What this pins is that exposing a request field to an untrusted
/// delegating actor did not change that — the number it names is clamped to the
/// root rather than installed.
#[test]
fn f21_02_a_delegator_cannot_request_a_wider_envelope_than_the_session_root() {
    let (turns, transcript) = run_live(Some(json!({
        "max_tokens_in": 999_999_999u64,
        "max_cost_usd": 1_000_000.0,
        "max_agent_depth": 999
    })));

    assert!(
        turns > 0,
        "no delegated child provider turn was served, so nothing about the \
         widening request was measured. transcript: {transcript}"
    );
    assert_eq!(
        turns, CHILD_SCRIPT_TURNS,
        "the child neither gained nor lost turns from a widening request: it must \
         run exactly its script, bounded by the {ROOT_TOKENS_IN}-token root it \
         inherited. transcript: {transcript}"
    );
}
