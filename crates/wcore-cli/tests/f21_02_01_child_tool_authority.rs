//! F21-02-01 LIVE — a delegated child must not obtain a tool the PARENT
//! session does not itself hold.
//!
//! # Why this drives the real binary
//!
//! An in-process assertion on `build_tool_registry` proves the seam computes an
//! intersection. It does not prove that anything in production ever gives that
//! seam a narrowed parent — and this repository has already shipped an entire
//! permission crate whose `check` had no caller. So this file spawns the real
//! `wayland-core` binary, narrows the parent by the only production mechanism
//! that is drivable from outside the process (a persona `allowed_tools` roster
//! over `acp serve --enable-agent-selection`), and observes the child's Bash
//! effect on disk.
//!
//! ## The scenario
//!
//! 1. A persona `narrowed` declares `allowed_tools: [Delegate, Read]`. Bootstrap
//!    `retain`s the parent registry down to those two, so the parent session has
//!    NO Bash.
//! 2. The parent's scripted turn issues `Delegate` with `toolsets: ["Bash"]`.
//! 3. The delegated child's scripted turn issues `Bash`, writing a probe file.
//!
//! Before the fix, `build_tool_registry` honoured the REQUEST and registered
//! Bash — the probe file appeared. After it, the request is intersected with the
//! parent's authority, Bash is never registered, and no probe file exists.
//!
//! ## Anti-vacuity
//!
//! A missing probe file is only evidence if a child actually existed and took
//! its own provider turn. The mock routes by the first user message (the L1 goal
//! marker), exactly as `child_authority_corpus/live.rs` does, and the test
//! asserts a child turn was served before it reads the verdict. Without that,
//! "no Bash effect" would also be the reading for "no child ever ran".
//!
//! ## Hermetic by construction
//!
//! `WAYLAND_HOME` + `HOME` point at a throwaway tempdir, every provider
//! credential env var is stripped, and the provider `base_url` is a local
//! wiremock. No network call leaves the machine and the developer's real config
//! is never read.

// Unix-only. Not because the DEFECT is unix-only — it is not, and the
// seam-level regressions in `wcore-agent::spawner` run on every target — but
// because this driver spawns a real server that runs a real `Bash` command
// under the platform sandbox, and shipping a live Windows driver that has never
// been observed to run would be a green with nothing behind it.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;
use tempfile::TempDir;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

#[path = "support/mod.rs"]
mod support;

/// Marker prefixed onto the delegated goal. A provider request whose FIRST user
/// message carries it was made by the CHILD, not the parent — the only reliable
/// way to answer each generation with its own script.
const CHILD_GOAL: &str = "F2101CHILDGOAL";

/// The command the child's scripted `Bash` call runs. Its output is irrelevant
/// to the verdict (see [`child_tool_registries`]); it exists so the child takes
/// a real tool-calling turn rather than a bare text turn.
const BASH_COMMAND: &str = "printf 'F2101_%s_PROBE' RAN";

/// The tool registry the CHILD was actually built with, once per child provider
/// request, read out of the `tools` array the engine serialises into every call.
///
/// # Why this is the observable, and not an effect on disk
///
/// This reads the product's answer to the exact question the finding asks —
/// *which tools did `build_tool_registry` put in the child's registry* — out of
/// the real binary's own wire traffic. Nothing else in the pipeline can forge
/// it: the array is built from the live `ToolRegistry`, so a name present here
/// was registered and a name absent here was not.
///
/// Two earlier revisions of this test graded an on-disk probe instead, and the
/// CONTROL run rejected both. The first wrote to an absolute path outside the
/// child's `SandboxedFs` jail. The second wrote relative — and the transcript
/// showed the real reason nothing appeared: `bwrap: Can't mkdir
/// …/workspace/.git: Read-only file system`, a sandbox-mount condition on the
/// test host that has nothing whatever to do with tool authority. In BOTH runs
/// the child's registry visibly contained `Bash`. Had the narrowed run been
/// graded on the probe alone it would have reported a PASS produced by the
/// sandbox, not by the fix — an engineered green. The control is what caught
/// that, which is precisely why it is here.
fn child_tool_registries(mock: &MockServer, rt: &tokio::runtime::Runtime) -> Vec<Vec<String>> {
    rt.block_on(async {
        mock.received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter_map(|r| {
                let body: serde_json::Value = serde_json::from_slice(&r.body).ok()?;
                if !body
                    .get("messages")?
                    .get(0)?
                    .to_string()
                    .contains(CHILD_GOAL)
                {
                    return None;
                }
                Some(
                    body.get("tools")
                        .and_then(|t| t.as_array())
                        .map(|tools| {
                            tools
                                .iter()
                                .filter_map(|t| Some(t.get("name")?.as_str()?.to_owned()))
                                .collect()
                        })
                        .unwrap_or_default(),
                )
            })
            .collect()
    })
}

/// How long to keep looking for the delegated child's provider request after
/// the parent's stream has ended.
const CHILD_TURN_DEADLINE: Duration = Duration::from_secs(30);

/// [`child_tool_registries`], polled until a child provider turn has been
/// served or `within` elapses.
///
/// # Why a poll, and not the bare read this replaced
///
/// The delegation itself is synchronous end to end — `DelegateTool::execute`
/// `join_all`s its children, `spawn_durable` awaits `engine.run`, and the ACP
/// `done` frame is only emitted after the whole parent turn returns — so on a
/// CLEAN end of stream the child's request is already recorded and the first
/// poll returns it.
///
/// The hazard is that `AcpServer::post` cannot tell a clean end of stream from
/// a socket read timeout: it ends on `let _ = stream.read_to_end(..)` and
/// returns whatever accumulated. Its 180s read budget is shorter than the 600s
/// `ToolCategory::Exec` ceiling the `Delegate` dispatch runs under, so on a
/// loaded runner the read can end while the child is still mid-turn. The bare
/// read then saw an empty registry and fired the anti-vacuity guard below —
/// correctly reporting that the run measured nothing, but for a scheduling
/// reason rather than a product one.
///
/// The deadline is what keeps the guard's teeth. When no child turn is ever
/// served this still returns empty, and the caller's assertion fails with its
/// own message.
fn await_child_tool_registries(
    mock: &MockServer,
    rt: &tokio::runtime::Runtime,
    within: Duration,
) -> Vec<Vec<String>> {
    let deadline = std::time::Instant::now() + within;
    loop {
        let registries = child_tool_registries(mock, rt);
        if !registries.is_empty() || std::time::Instant::now() >= deadline {
            return registries;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The persona id, and the two tools it is allowed. `Bash` is deliberately
/// absent: that is what makes the parent narrower than the child's request.
const PERSONA: &str = "narrowed";

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Make the run's working directory a REAL git repository one level below the
/// home, and return it.
///
/// This is not incidental setup. A `Delegate` naming `toolsets: ["Bash"]`
/// classifies as [`RequestedChildWorkspace::IsolatedMutation`], whose checkout
/// root is derived under `<WAYLAND_HOME>/sessions`. With `cwd == home` that root
/// overlaps the repository and `WorktreeManager::new_with_workspace_root`
/// refuses with `durable child workspace preparation failed: worktree io:
/// orchestrator worktree root must not overlap repository` — so NO mutating
/// child is ever created and the probe's absence would be a workspace-
/// preparation failure wearing the costume of an authority decision. That is
/// precisely the confound that made 21-02's tool dimension unmeasurable, and
/// this test's anti-vacuity gate exists to refuse to grade such a run.
///
/// Argv mode throughout; identity is passed per-invocation with `-c` so nothing
/// reads or writes a global git config.
fn init_workspace_repo(home: &Path) -> std::io::Result<std::path::PathBuf> {
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace)?;
    let run = |args: &[&str]| -> std::io::Result<()> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&workspace)
            .output()?;
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    };
    run(&["init", "--initial-branch=f2101"])?;
    std::fs::write(
        workspace.join("README.f2101"),
        b"f21-02-01 fixture repository",
    )?;
    // The binary writes per-workspace state under `.wayland-core/`, and an
    // isolated-mutation dispatch refuses on a dirty checkout. Ignoring it is
    // what keeps the repository clean enough for the child to exist at all.
    std::fs::write(workspace.join(".gitignore"), b".wayland-core/\n")?;
    run(&["add", "README.f2101", ".gitignore"])?;
    run(&[
        "-c",
        "user.email=f2101@example.invalid",
        "-c",
        "user.name=f2101",
        "commit",
        "-m",
        "f21-02-01 fixture",
    ])?;
    Ok(workspace)
}

/// Write the operator-authored global persona YAML the ACP roster loads from
/// `wayland_config_dir()/agents` (which is `WAYLAND_HOME`-aware).
fn write_persona(home: &Path) {
    let dir = home.join("agents");
    std::fs::create_dir_all(&dir).expect("create the global agents dir");
    let yaml = format!(
        "name: {PERSONA}\n\
         description: a parent narrowed to Delegate + Read\n\
         system_prompt: You delegate work.\n\
         allowed_tools:\n  - Delegate\n  - Read\n"
    );
    std::fs::write(dir.join(format!("{PERSONA}.yaml")), yaml).expect("write the persona yaml");
}

/// Start a provider mock that answers by GENERATION, not by queue order. A
/// single ordered queue would answer the parent's second turn with the child's
/// script, and any on-disk effect would then be unattributable.
async fn start_routed_mock() -> MockServer {
    let server = MockServer::start().await;
    let parent: Vec<String> = vec![
        support::mock_llm::tool_use_turn_sse(
            "Delegate",
            &json!({
                "goal": format!("{CHILD_GOAL}: write the probe file with Bash"),
                "toolsets": ["Bash"]
            }),
        ),
        support::mock_llm::text_turn_sse("parent done"),
    ];
    let child: Vec<String> = vec![
        support::mock_llm::tool_use_turn_sse("Bash", &json!({ "command": BASH_COMMAND })),
        support::mock_llm::text_turn_sse("child done"),
    ];
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

/// A live `acp serve` process plus the address it actually bound.
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
    /// Holds the parent end of the inherited vault-passphrase descriptor open
    /// for the server's whole life. Dropping it at the end of `spawn_acp` would
    /// race the child's read of the passphrase.
    _vault: support::vault::VaultGuard,
}

impl AcpServer {
    /// One authenticated `POST` against the REST surface, spoken over a raw
    /// socket. Deliberately dependency-free: `reqwest` is behind wcore-cli's
    /// optional `remote-registry` feature, and adding a dev-dependency to
    /// prove a security fix widens the blast radius for no benefit. Returns the
    /// full response body (the SSE stream, for a prompt).
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

/// Split an HTTP response into (headers, body) and parse the body as JSON.
fn json_body(response: &str) -> serde_json::Value {
    let body = response
        .split_once("\r\n\r\n")
        .map_or(response, |(_, rest)| rest);
    // A `Connection: close` response is not chunked on this server's JSON
    // routes, so the body is the raw JSON document.
    serde_json::from_str(body.trim())
        .unwrap_or_else(|e| panic!("response body was not json ({e}): {response}"))
}

impl Drop for AcpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn `wayland-core acp serve` on an ephemeral port and wait until it
/// reports the address it bound. The server key is injected through
/// `WAYLAND_ACP_SERVER_KEY` so the run never touches the developer's keychain.
fn spawn_acp(home: &Path, cwd: &Path) -> AcpServer {
    let key = "f2101-live-server-key".to_owned();
    let mut cmd = Command::new(binary());
    cmd.args([
        "acp",
        "serve",
        "--bind",
        "127.0.0.1:0",
        "--enable-agent-selection",
        // The child must actually RUN its tool call; without this the turn
        // stops at an approval frame and the probe's absence would prove
        // nothing about tool authority.
        "--allow-all-tools",
    ])
    // The governed repository, NOT the home — see `init_workspace_repo`.
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

/// Create a session (optionally bound to a persona) and drive one prompt
/// through it. Returns the raw SSE body so the caller can report what ran.
fn drive_one_turn(server: &AcpServer, agent: Option<&str>) -> String {
    let create_body = match agent {
        Some(id) => json!({ "agent": id }),
        None => json!({}),
    };
    let created = json_body(&server.post("/v1/sessions", &create_body, Duration::from_secs(60)));
    let session_id = created
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("session/create did not return a session id: {created}"))
        .to_owned();

    server.post(
        &format!("/v1/sessions/{session_id}/prompt"),
        &json!({ "text": "delegate the probe write" }),
        PROMPT_READ_BUDGET,
    )
}

#[test]
fn f21_02_01_delegated_child_cannot_obtain_a_tool_the_parent_lacks() {
    let home = TempDir::new().expect("hermetic home");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mock = rt.block_on(start_routed_mock());

    let workspace = init_workspace_repo(home.path()).expect("git fixture repository");
    write_persona(home.path());
    support::pty::write_config(
        home.path(),
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(&mock.uri()),
    );

    let server = spawn_acp(home.path(), &workspace);
    let transcript = drive_one_turn(&server, Some(PERSONA));

    // Anti-vacuity: a child must have EXISTED and taken its own provider turn,
    // or "the child had no Bash" is a statement about the harness rather than
    // about the product.
    let registries = await_child_tool_registries(&mock, &rt, CHILD_TURN_DEADLINE);
    assert!(
        !registries.is_empty(),
        "no delegated child provider turn was served, so this run measures nothing about child \
         tool authority. transcript: {transcript}"
    );

    // The verdict, read out of the child's own registry.
    let granted: Vec<&Vec<String>> = registries
        .iter()
        .filter(|names| names.iter().any(|n| n == "Bash"))
        .collect();
    assert!(
        granted.is_empty(),
        "a delegated child was BUILT WITH Bash — a tool its parent session (persona {PERSONA}, \
         allowed_tools = [Delegate, Read]) does not itself hold. The child's provider requests \
         advertise these registries: {registries:?}. transcript: {transcript}"
    );
}

/// A control: the SAME script against a parent with NO persona narrowing must
/// still deliver Bash to the child. Without this, a blanket denial (or a
/// harness that never reaches the seam at all) would pass the assertion above
/// for entirely the wrong reason.
#[test]
fn f21_02_01_control_unnarrowed_parent_still_delegates_bash() {
    let home = TempDir::new().expect("hermetic home");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mock = rt.block_on(start_routed_mock());

    // No persona yaml, and the session selects no agent: the parent keeps its
    // full toolset, so the intersection is a no-op.
    let workspace = init_workspace_repo(home.path()).expect("git fixture repository");
    support::pty::write_config(
        home.path(),
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(&mock.uri()),
    );

    let server = spawn_acp(home.path(), &workspace);
    let transcript = drive_one_turn(&server, None);

    let registries = await_child_tool_registries(&mock, &rt, CHILD_TURN_DEADLINE);
    let granted = registries
        .iter()
        .any(|names| names.iter().any(|n| n == "Bash"));
    assert!(
        granted,
        "the control run's child was never built with Bash either ({} child turn(s), registries \
         {registries:?}), so the narrowed run above cannot be attributed to the tool-authority \
         intersection rather than to some unrelated refusal. transcript: {transcript}",
        registries.len()
    );
}

// ── The read guard in `AcpServer::post` ──────────────────────────────────
//
// `post` used to end on `let _ = stream.read_to_end(&mut raw)`, which makes a
// budget expiry indistinguishable from a clean EOF: it returned the PREFIX it
// had managed to read as an ordinary `String`. Nothing downstream can tell —
// `json_body` blames the server for invalid JSON, and a prompt assertion
// blames the product for an SSE frame the harness never read. So a harness
// timeout was reported as a product defect.
//
// Graded HERE, at the call site, and not on an extracted helper: the thing
// that has to stay true is that `post` itself refuses to return a prefix.

/// The frame the stalling listener emits only AFTER its stall. Its presence is
/// what separates a whole body from a truncated one.
const TERMINAL_FRAME: &str = "event: turn_complete\ndata: {\"done\":true}\n\n";

/// A listener that writes the head of an SSE body immediately and the terminal
/// frame only after `stall`. That is the wire shape of a turn whose tool
/// dispatch is slow: headers and early frames are on the wire, completion is
/// not. Returns the bound address.
fn stalling_listener(stall: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stalling listener");
    let addr = listener
        .local_addr()
        .expect("the stalling listener bound an address")
        .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut conn = stream;
                let mut scratch = [0u8; 4096];
                // The request is not parsed — only its arrival matters.
                let _ = conn.read(&mut scratch);
                let _ = conn.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                      Connection: close\r\n\r\nevent: tool_call\ndata: {}\n\n",
                );
                let _ = conn.flush();
                std::thread::sleep(stall);
                let _ = conn.write_all(TERMINAL_FRAME.as_bytes());
                let _ = conn.flush();
                // Dropping `conn` closes the socket, which is the EOF the
                // control arm below reads to completion.
            });
        }
    });
    addr
}

/// A real `AcpServer` aimed at `addr` instead of a spawned `acp serve`. The
/// `child` field needs a genuine `Child`, so a short-lived `--version` run of
/// the same binary fills it; `Drop` kills and reaps it either way.
fn server_pointed_at(addr: String) -> AcpServer {
    let mut cmd = Command::new(binary());
    cmd.arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let vault = support::vault::configure_process(&mut cmd);
    let child = cmd.spawn().expect("spawn the short-lived stand-in child");
    AcpServer {
        child,
        addr,
        key: "read-guard-test-key".to_owned(),
        _vault: vault,
    }
}

/// Pull a readable message out of a caught panic payload.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "<non-string panic payload>".to_owned())
}

#[test]
fn post_reports_a_read_that_ran_out_of_budget_instead_of_returning_a_prefix() {
    let stall = Duration::from_secs(2);
    let server = server_pointed_at(stalling_listener(stall));

    // CONTROL FIRST. With a budget above the stall the whole body arrives, so
    // the red arm below cannot pass merely because this listener never emits a
    // terminal frame, and a clean EOF is proved to still be a success.
    let whole = server.post("/control", &json!({}), stall * 8);
    assert!(
        whole.contains("turn_complete"),
        "control: a budget above the stall must return the WHOLE body and a \
         clean EOF must stay a success — got {whole:?}"
    );

    // RED ARM. A budget below the stall must fail, not return the prefix that
    // the control arm just proved is only part of the response.
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        server.post("/truncated", &json!({}), stall / 4)
    }))
    .expect_err(
        "a read that ran out of budget returned a body as an ordinary success — \
         that is the defect this test exists to hold closed",
    );
    let message = panic_text(payload.as_ref());
    assert!(
        message.contains("PREFIX of the response"),
        "the failure must name the truncation so it is not misread as a product \
         defect — got: {message}"
    );
    assert!(
        message.contains("timed out: true"),
        "the failure must report that the budget, not the peer, ended the read \
         — got: {message}"
    );
}
