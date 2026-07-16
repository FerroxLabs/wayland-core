//! P0 SMOKE HARNESS — the pre-release gate (Wave 0, Task 0.B + 0.C).
//!
//! This is the live smoke suite from
//! `.planning/audit/UX-AUDIT-AND-TEST-PLAN.md` §4: the ordered checks a new
//! user hits in the first ten minutes. **A release is blocked if any HARD-GATE
//! check fails.**
//!
//! ## Hard gate — what actually EXISTS as a test here
//!
//! The original SMOKE §4 rule named checks `6, 7, 8, 9, 11, 14, 15, 17, 18, 19`
//! as the aspirational hard gate (the confirmed `/model`-class phantoms). Only a
//! subset is implemented today; this doc and `scripts/smoke.sh` name ONLY the
//! checks that exist as real tests, so the gate never implies coverage it does
//! not have:
//!
//! | Check | Status | Test fn |
//! |-------|--------|---------|
//! | #6  | GREEN gate | `smoke_06_first_prompt_uses_configured_provider_and_key` |
//! | #10 | GREEN gate | `smoke_10_model_override_reaches_outgoing_request` |
//! | #17 | GREEN gate | `smoke_17_force_posture_auto_approves_mutating_tool_in_engine` (binary-level Force seam; the `/config`-PTF leg is the Config wave's — see below) |
//! | #24 | GREEN gate | `smoke_24_quit_exits_cleanly` (PTY anchor) |
//! | #15 | interactive-pending | `smoke_15_*` (`#[ignore]`, D040) |
//! | #22 | interactive-pending | `smoke_22_*` (`#[ignore]`, D038) |
//! | #23 | interactive-pending | `smoke_23_*` (`#[ignore]`, D039) |
//! | #7, #8, #9, #11, #14, #18, #19 | **TODO — uncovered** | no test yet; pending their remediation wave. NOT claimed as covered. |
//!
//! Checks `7, 8, 9, 11, 14, 18, 19` are deliberately NOT in the hard-gate runner
//! list because no test asserts them yet; listing them would be a coverage
//! overclaim. They land as their waves wire the behavior.
//!
//! Two classes of check live here:
//!
//! 1. **Engine-behavior checks** — provable by driving the REAL `wayland-core`
//!    binary against the scriptable [`support::mock_llm::MockLlm`] and asserting
//!    on the OUTGOING request the binary actually sent (model / system / the
//!    `x-api-key` header), read back via
//!    [`support::mock_llm::received_requests`], or on the engine's tool-gating
//!    behavior. These settle #6 (first-prompt-uses-entered-key), #10 (`/model`
//!    literal switch), and #17 (a Force approval posture auto-approves a
//!    mutating tool in the live engine — the binary-level leg; the interactive
//!    `/config` PTY leg is the Config wave's) with zero provider spend.
//!
//! 2. **GAP checks** — the P0 defects the smoke suite did NOT yet cover
//!    (`D002, D009, D010, D011, D012, D015`, per
//!    `MASTER-DEFECT-LEDGER.md` → "P0 defects with NO smoke check"). Each is
//!    written to assert the FIXED behavior, so it FAILS NOW - proving the gap
//!    exists. A later remediation wave turns each one green. They are named
//!    `gap_dNNN_*` and tagged `#[ignore = "GAP: ..."]` so the default lane stays
//!    green while the gate runner (`scripts/smoke.sh`) runs them with
//!    `--run-ignored all` (or `--ignored`) and reports them as currently-RED /
//!    uncovered rather than silently skipped.
//!
//! ## Hermetic by construction
//!
//! Every check points `WAYLAND_HOME` (and `HOME`) at a throwaway tempdir (the
//! F-010 hermetic-sandbox override honoured by
//! `wcore_config::wayland_config_dir()`) and strips the full provider-credential
//! env set — exactly the vars listed in `STRIPPED_PROVIDER_ENV` (`API_KEY`,
//! `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `GOOGLE_API_KEY`,
//! `OPENROUTER_API_KEY`, `DEEPSEEK_API_KEY`, `GROQ_API_KEY`, the concrete
//! `AWS_*` Bedrock vars, and the Vertex vars) — from the child env via the one
//! shared `harden_child_env` helper (and the matching loop in the PTY spawn),
//! so a run can neither read nor mutate the developer's real config/keys, and
//! onboarding never auto-detects a stray dev key.
//!
//! ## Why the PTY half is `#[cfg(unix)]`
//!
//! `portable_pty`'s Windows ConPTY backend does not surface the spawned
//! binary's stdout to the master end in the headless GHA runner (documented at
//! length in `harness_tui_flow.rs`), so the full-screen TUI harness is
//! Unix-only. The headless GAP checks (`-p` one-shot / `--json-stream`) run on
//! every platform.
//!
//! ## One real-key happy path
//!
//! Set `SMOKE_LIVE=1` to additionally run the single end-to-end real-provider
//! check (`live_real_key_first_prompt_round_trip`, gated + `#[ignore]`): one
//! real key on the cheapest model, one short turn. CI runs everything else
//! hermetically.

#[path = "support/mod.rs"]
mod support;

use std::path::Path;

use support::mock_llm::{MockLlm, RecordedRequest, received_requests};
use tempfile::TempDir;
use wiremock::MockServer;

// ===========================================================================
// Shared hermetic helpers (cross-platform).
// ===========================================================================

/// Path to the debug binary under test (Cargo wires this env var).
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Seed `<home>/config.toml` for a provider/model, optionally routing the
/// provider `base_url` at a local mock. `model: None` writes NO model line —
/// the exact catalog-provider shape the D002 GAP check needs.
fn write_config(home: &Path, provider: &str, model: Option<&str>, base_url: Option<&str>) {
    let mut toml = format!("[default]\nprovider = \"{provider}\"\n");
    if let Some(m) = model {
        toml.push_str(&format!("model = \"{m}\"\n"));
    }
    toml.push_str(&format!(
        "\n[providers.{provider}]\napi_key = \"sk-ant-harness-not-real-key-0000000000\"\n"
    ));
    if let Some(url) = base_url {
        toml.push_str(&format!("base_url = \"{url}\"\n"));
    }
    std::fs::write(home.join("config.toml"), toml).expect("write config.toml");
}

/// Start a [`MockServer`] on a held tokio runtime, returning both so the caller
/// keeps them alive for the whole test (the spawned binary POSTs to it over
/// real loopback). Drop order: server then runtime, at end of scope.
fn start_mock(mock: MockLlm) -> (tokio::runtime::Runtime, MockServer) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(mock.start());
    (rt, server)
}

/// Read back every JSON request the mock received, on the runtime the server
/// was started on. The rebind-class checks assert on these.
fn recorded(rt: &tokio::runtime::Runtime, server: &MockServer) -> Vec<RecordedRequest> {
    rt.block_on(received_requests(server))
}

/// The full provider-credential env-var set every spawned child must NOT
/// inherit, so a run can neither read the developer's real keys nor have
/// onboarding auto-detect a stray dev credential. ONE source of truth used by
/// `run_headless`, the PTY spawn, and the `--json-stream` child — keeps the
/// strip set honest and uniform (M6). `AWS_*` / `VERTEX*` are stripped by name
/// (the concrete vars Bedrock/Vertex auth reads), not by glob.
const STRIPPED_PROVIDER_ENV: &[&str] = &[
    "API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    // AWS (Bedrock) — concrete vars the provider auth chain reads.
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    // Google Vertex.
    "VERTEX_PROJECT",
    "VERTEX_LOCATION",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// Apply the hermetic child env uniformly: point `WAYLAND_HOME` + `HOME` at the
/// throwaway tempdir, set a deterministic `TERM`, and strip every credential in
/// [`STRIPPED_PROVIDER_ENV`]. The single place that defines "hermetic child
/// env" so the headless / PTY / json-stream spawns can never drift apart (M6).
fn harden_child_env(cmd: &mut std::process::Command, home: &Path) -> support::vault::VaultGuard {
    cmd.env("WAYLAND_HOME", home)
        .env("HOME", home)
        // Headless / json-stream children get a deterministic non-TTY term. The
        // PTY spawn (which needs a real terminal type) sets its own TERM and
        // does NOT route through this helper.
        .env("TERM", "dumb");
    for key in STRIPPED_PROVIDER_ENV {
        cmd.env_remove(key);
    }
    support::vault::configure_process(cmd)
}

/// Spawn the binary headless (no TUI) against a hermetic home. The prompt is a
/// TRAILING POSITIONAL argument — `wayland-core` has no `-p` flag (`-p` is the
/// short for `--provider`); an unknown trailing word is swallowed as the prompt
/// (`prompt: Vec<String>` with `trailing_var_arg`, main.rs). Strips the full
/// provider-key set (see [`STRIPPED_PROVIDER_ENV`]), sets `WAYLAND_HOME` +
/// `TERM`, runs in `home` as cwd. Returns (status, stdout, stderr). One-shot:
/// the process runs the prompt and exits.
fn run_headless(home: &Path, args: &[&str]) -> (std::process::ExitStatus, String, String) {
    let mut cmd = std::process::Command::new(binary());
    cmd.args(args).current_dir(home);
    let vault = harden_child_env(&mut cmd, home);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = cmd.spawn();
    drop(vault);
    let out = child
        .expect("spawn wayland-core headless")
        .wait_with_output()
        .expect("wait for wayland-core headless");
    (
        out.status,
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// ===========================================================================
// ENGINE-BEHAVIOR CHECKS (hermetic, mock provider, request-recorder).
//
// These drive the REAL binary headless and assert on the recorded OUTGOING
// request. They settle the rebind-class gates without any provider spend.
// ===========================================================================

/// SMOKE #6 (HARD GATE) — the first prompt actually uses the configured
/// provider/key, NOT a boot default. (Defect D001.)
///
/// Headless `-p` boots the engine from the seeded config and runs one turn
/// against the mock. We assert the mock RECEIVED a request and that it carried
/// the configured model + the configured `x-api-key` — i.e. the entered
/// provider/key reached the live engine. If onboarding/boot resolved to a
/// default (D001), the request would carry a different model or no key.
#[test]
fn smoke_06_first_prompt_uses_configured_provider_and_key() {
    let home = TempDir::new().expect("tempdir");
    let (rt, server) = start_mock(MockLlm::new().text("ok from mock"));
    write_config(
        home.path(),
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(&server.uri()),
    );

    let (status, stdout, stderr) = run_headless(home.path(), &["--no-tui", "say", "hello"]);
    assert!(
        status.success(),
        "headless first prompt should exit 0; stderr: {stderr}\nstdout: {stdout}"
    );

    let reqs = recorded(&rt, &server);
    assert!(
        !reqs.is_empty(),
        "the engine must have sent at least one request to the configured provider; \
         stdout: {stdout}\nstderr: {stderr}"
    );
    let first = &reqs[0];
    assert_eq!(
        first.model(),
        Some("claude-sonnet-4-20250514"),
        "first prompt must carry the CONFIGURED model, not a boot default; got {:?}",
        first.model()
    );
    assert_eq!(
        first.api_key.as_deref(),
        Some("sk-ant-harness-not-real-key-0000000000"),
        "first prompt must carry the CONFIGURED api key (proof it reached the live engine); \
         got {:?}",
        first.api_key
    );
}

/// SMOKE #10 — `/model <id>` literal switch: the outgoing request carries the
/// new id. Driven headless via `--model` (the same resolution path a `/model`
/// pick feeds the engine builder), asserting the recorded request body.
#[test]
fn smoke_10_model_override_reaches_outgoing_request() {
    let home = TempDir::new().expect("tempdir");
    let (rt, server) = start_mock(MockLlm::new().text("ok"));
    // Config says sonnet; the explicit --model override must win on the wire.
    write_config(
        home.path(),
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(&server.uri()),
    );

    let (status, stdout, stderr) = run_headless(
        home.path(),
        &["--no-tui", "--model", "claude-haiku-4-5", "hi", "there"],
    );
    assert!(
        status.success(),
        "exit 0 expected; stderr: {stderr}\nstdout: {stdout}"
    );

    let reqs = recorded(&rt, &server);
    assert!(!reqs.is_empty(), "expected a recorded request");
    assert_eq!(
        reqs[0].model(),
        Some("claude-haiku-4-5"),
        "the --model override must reach the outgoing request, not the config model"
    );
}

/// SMOKE #17 (HARD GATE) — a Force approval posture reaches the LIVE engine: a
/// mutating tool runs auto-approved instead of blocking on an approval that
/// never arrives. Driven through the REAL binary over `--json-stream --force`,
/// with the mock scripting a `Write` tool call. We assert the engine actually
/// executed the Write (the probe file appears) — proof the Force posture is wired
/// all the way into the engine's `ToolApprovalManager` gate, not just persisted.
///
/// ## What this covers vs. what it does not
///
/// This is the strongest HERMETIC binary-level check of the Force seam. It pins
/// the *engine-side* contract: when the runtime approval posture is Force, a
/// mutating tool auto-approves end to end through the spawned binary
/// (`--force` flips `ToolApprovalManager` to `SessionMode::Force` before boot —
/// main.rs json-stream arm). A non-Force session would instead hang waiting for
/// an approval that headless never sends (the F-002 timeout the `--force` wiring
/// fixed), so a written file is a genuine Force-vs-not delta.
///
/// It deliberately does NOT drive a full PTY `/config` → save → rebind flow:
/// that interactive Config-surface → live-rebind mapping is pinned at the
/// library seam by `engine_rebind.rs::rebind_applies_force_approval_mode_to_live_session`
/// (owned by the rebind-seam suite), which this check does not duplicate. The
/// `/config`-surface PTY leg is tracked under the Config wave; see the module
/// doc's hard-gate TODO list.
#[test]
fn smoke_17_force_posture_auto_approves_mutating_tool_in_engine() {
    use std::io::{BufRead, BufReader, Write};
    use std::time::{Duration, Instant};

    let home = TempDir::new().expect("tempdir");
    let probe = home.path().join("force_probe.txt");
    let probe_arg = probe.to_str().expect("utf-8 path").to_string();

    // Turn 1: a mutating Write call. Turn 2: closing text (only reached if the
    // tool was allowed to run). Under Force the Write must auto-approve.
    let (_rt, server) = start_mock(
        MockLlm::new()
            .tool_use(
                "Write",
                serde_json::json!({ "file_path": probe_arg, "content": "FORCE_APPLIED" }),
            )
            .text("done"),
    );
    write_config(
        home.path(),
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(&server.uri()),
    );

    let mut cmd = std::process::Command::new(binary());
    cmd.args(["--json-stream", "--force", "--provider", "anthropic"])
        .current_dir(home.path());
    let vault = harden_child_env(&mut cmd, home.path());
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
        "{{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"write the probe\"}}"
    )
    .expect("write message");

    // Drain stdout in the background so the child never blocks on a full pipe.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    // Wait for the engine to execute the Write (the probe appears) within a
    // bounded window. Under Force the tool auto-approves; without the posture
    // reaching the engine it would block and the file would never appear.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut wrote = false;
    while Instant::now() < deadline {
        if probe.exists() {
            wrote = true;
            break;
        }
        // Keep the channel drained (ignore the content; we assert on the file).
        let _ = rx.recv_timeout(Duration::from_millis(200));
    }

    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        wrote,
        "under --force the engine must auto-approve and execute the mutating Write \
         (probe file must exist); it did not - the Force posture did not reach the live \
         engine gate. probe={}",
        probe.display()
    );
    let body = std::fs::read_to_string(&probe).unwrap_or_default();
    assert!(
        body.contains("FORCE_APPLIED"),
        "the auto-approved Write must have written the scripted content; got {body:?}"
    );
}

/// wayland#403 fix-3 regression: a mid-turn `Stop` must CANCEL THE TURN but
/// NOT strand the json-stream session — a subsequent Message must still
/// stream. Pre-fix, the mid-turn Stop broke the outer command loop, so after
/// any Stop the session was dead ("new chat required") — reproduced here
/// deterministically: turn 1 issues a Bash tool call which, under the default
/// approval posture, PAUSES on approval (a deterministic mid-turn state);
/// while paused we send Stop, then send a second Message. The fix requires
/// the second Message to reach its `stream_end`.
#[test]
fn stop_mid_turn_does_not_strand_json_stream_session() {
    use std::io::{BufRead, BufReader, Write};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    // Turn 1: a Bash (exec-category) tool call — under the default posture the
    // engine emits ToolRequest and BLOCKS on approval. Turn 2 (a fresh LLM
    // call after Stop cancels turn 1): plain text.
    let (_rt, server) = start_mock(
        MockLlm::new()
            .tool_use("Bash", serde_json::json!({ "command": "echo repro-403" }))
            .text("SESSION-ALIVE"),
    );
    // Default posture (NO --force): the Bash call must pause on approval.
    let home = TempDir::new().expect("tempdir");
    write_config(
        home.path(),
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(&server.uri()),
    );

    let mut cmd = std::process::Command::new(binary());
    cmd.args(["--json-stream", "--provider", "anthropic"])
        .current_dir(home.path());
    let vault = harden_child_env(&mut cmd, home.path());
    let child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    drop(vault);
    let mut child = child.expect("spawn --json-stream");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    // Collect every stdout line so the test can look for specific frames.
    let frames: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let frames = frames.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                frames.lock().unwrap().push(line);
            }
        });
    }

    // Frame predicate: any collected line whose JSON `type` is one of `types`
    // and (when `msg_id` is Some) matches that msg_id.
    let seen = |types: &[&str], msg_id: Option<&str>| -> bool {
        frames.lock().unwrap().iter().any(|ln| {
            let Ok(j) = serde_json::from_str::<serde_json::Value>(ln) else {
                return false;
            };
            let ty = j.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let mid_ok =
                msg_id.is_none_or(|want| j.get("msg_id").and_then(|m| m.as_str()) == Some(want));
            types.contains(&ty) && mid_ok
        })
    };
    let wait_until = |pred: &dyn Fn() -> bool, secs: u64| -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if pred() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        false
    };

    // Turn 1 — forces the Bash tool call, which pauses on approval.
    writeln!(
        stdin,
        "{{\"type\":\"message\",\"msg_id\":\"m1\",\"content\":\"run bash\"}}"
    )
    .expect("write m1");

    assert!(
        wait_until(
            &|| seen(&["tool_request", "approval_required"], Some("m1")),
            20
        ),
        "turn 1 must pause on the Bash approval gate (deterministic mid-turn state); \
         frames={:?}",
        frames.lock().unwrap()
    );

    // Stop mid-turn — cancels turn 1; the session must survive.
    writeln!(stdin, "{{\"type\":\"stop\"}}").expect("write stop");
    assert!(
        wait_until(&|| seen(&["stream_end"], Some("m1")), 10),
        "the mid-turn Stop must emit a stream_end for m1 so the host isn't left hanging"
    );

    // Turn 2 — the actual regression check: a new Message must still stream.
    writeln!(
        stdin,
        "{{\"type\":\"message\",\"msg_id\":\"m2\",\"content\":\"say hi\"}}"
    )
    .expect("write m2 (broken pipe here would already prove the session died)");

    let m2_streamed = wait_until(&|| seen(&["stream_end"], Some("m2")), 20);

    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        m2_streamed,
        "after a mid-turn Stop the session must keep streaming — m2 produced no \
         stream_end, so Stop stranded the session (the #403 fix-3 regression). \
         frames={:?}",
        frames.lock().unwrap()
    );
}

/// wayland#241 regression: config `[default] approval_mode = "auto-edit"`
/// must reach the json-stream session WITHOUT `--force`. Pre-fix
/// `run_json_stream_mode` only seeded `--force` and ignored the config
/// posture, so an edit-class tool blocked on an approval the headless host
/// never sends. This drives the REAL binary (not the isolated helper) so it
/// pins the call site the #149 cross-audit flagged as unpinned: a Write
/// (edit category, auto-approved under AutoEdit) must execute with only the
/// config seed. A plain `[default]` (no approval_mode) would hang here.
#[test]
fn approval_mode_auto_edit_from_config_reaches_json_stream_session() {
    use std::io::{BufRead, BufReader, Write};
    use std::time::{Duration, Instant};

    let home = TempDir::new().expect("tempdir");
    let probe = home.path().join("auto_edit_probe.txt");
    let probe_arg = probe.to_str().expect("utf-8 path").to_string();

    let (_rt, server) = start_mock(
        MockLlm::new()
            .tool_use(
                "Write",
                serde_json::json!({ "file_path": probe_arg, "content": "AUTO_EDIT_APPLIED" }),
            )
            .text("done"),
    );

    // Config posture is the ONLY approval signal — no `--force` flag below.
    let toml = format!(
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\
         approval_mode = \"auto-edit\"\n\n\
         [providers.anthropic]\napi_key = \"sk-ant-harness-not-real-key-0000000000\"\n\
         base_url = \"{}\"\n",
        server.uri()
    );
    std::fs::write(home.path().join("config.toml"), toml).expect("write config.toml");

    let mut cmd = std::process::Command::new(binary());
    // NOTE: no --force. The auto-edit posture must come from config alone.
    cmd.args(["--json-stream", "--provider", "anthropic"])
        .current_dir(home.path());
    let vault = harden_child_env(&mut cmd, home.path());
    let child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    drop(vault);
    let mut child = child.expect("spawn --json-stream");

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

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut wrote = false;
    while Instant::now() < deadline {
        if probe.exists() {
            wrote = true;
            break;
        }
        let _ = rx.recv_timeout(Duration::from_millis(200));
    }

    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        wrote,
        "config approval_mode=auto-edit must auto-approve the edit-class Write in \
         json-stream WITHOUT --force (the #241 regression); the probe never appeared, \
         so the config posture did not reach the session. probe={}",
        probe.display()
    );
    let body = std::fs::read_to_string(&probe).unwrap_or_default();
    assert!(
        body.contains("AUTO_EDIT_APPLIED"),
        "the auto-approved Write must have written the scripted content; got {body:?}"
    );
}

// ===========================================================================
// PTY-DRIVEN CHECKS (Unix-only). Drive the real full-screen TUI through a
// pseudo-terminal so the assertions hit the RENDERED screen, and every key
// goes through the real Router (never a surface handle_key directly).
// ===========================================================================

#[cfg(unix)]
mod pty {
    use super::*;
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

    /// A minimal PTY harness — the proven shape from `harness_tui_flow.rs`,
    /// re-derived here because integration test files compile as separate
    /// binaries and cannot share a non-`support` module.
    pub struct Pty {
        writer: Box<dyn Write + Send>,
        parser: std::sync::Arc<std::sync::Mutex<vt100::Parser>>,
        _master: Box<dyn MasterPty + Send>,
        child: Box<dyn portable_pty::Child + Send + Sync>,
        _reader: std::thread::JoinHandle<()>,
    }

    impl Pty {
        pub fn spawn(home: &Path) -> Self {
            let pty = native_pty_system()
                .openpty(PtySize {
                    rows: 40,
                    cols: 120,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("open PTY");

            let mut cmd = CommandBuilder::new(binary());
            cmd.env("HOME", home);
            cmd.env("WAYLAND_HOME", home);
            // The TUI needs a real terminal type (not "dumb") to render; the
            // hermetic key-strip set is shared with the headless/json-stream
            // spawns via STRIPPED_PROVIDER_ENV (M6).
            cmd.env("TERM", "xterm-256color");
            for key in STRIPPED_PROVIDER_ENV {
                cmd.env_remove(key);
            }
            let vault = support::vault::configure_pty(&mut cmd);
            cmd.cwd(home);
            let child = pty.slave.spawn_command(cmd);
            drop(vault);
            let child = child.expect("spawn wayland-core");

            let mut reader = pty.master.try_clone_reader().expect("clone PTY reader");
            let parser = std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(40, 120, 0)));
            let parser_for_thread = std::sync::Arc::clone(&parser);
            let reader_handle = std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Ok(mut p) = parser_for_thread.lock() {
                                p.process(&buf[..n]);
                            }
                        }
                        Err(_) => break,
                    }
                }
            });

            let writer = pty.master.take_writer().expect("take PTY writer");
            Self {
                writer,
                parser,
                _master: pty.master,
                child,
                _reader: reader_handle,
            }
        }

        pub fn screen_text(&self) -> String {
            let parser = self.parser.lock().expect("parser lock");
            parser.screen().contents()
        }

        pub fn wait_for<F: Fn(&str) -> bool>(&self, predicate: F, timeout: Duration, what: &str) {
            let deadline = Instant::now() + timeout;
            let mut last = String::new();
            while Instant::now() < deadline {
                last = self.screen_text();
                if predicate(&last) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            panic!(
                "timed out after {:?} waiting for {what}.\n--- last screen ---\n{}\n--- end ---",
                timeout, last
            );
        }

        pub fn send(&mut self, bytes: &[u8]) {
            self.writer.write_all(bytes).expect("write to PTY");
            self.writer.flush().ok();
        }

        pub fn wait_for_exit(&mut self, timeout: Duration) -> Option<portable_pty::ExitStatus> {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                match self.child.try_wait() {
                    Ok(Some(status)) => return Some(status),
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => return None,
                }
            }
            None
        }

        /// Clean shutdown via the proven palette quit path.
        pub fn quit(&mut self) {
            self.send(b"/");
            std::thread::sleep(Duration::from_millis(300));
            self.send(b"exit\r");
            let _ = self.wait_for_exit(Duration::from_secs(8));
        }
    }

    impl Drop for Pty {
        fn drop(&mut self) {
            if let Ok(None) = self.child.try_wait() {
                let _ = self.child.kill();
            }
        }
    }

    /// Boot the TUI to the Workspace surface (chrome wordmark + tab painted).
    pub fn boot(home: &Path) -> Pty {
        let h = Pty::spawn(home);
        h.wait_for(
            |s| s.contains("WAYLAND") && s.contains("Workspace"),
            Duration::from_secs(60),
            "TUI to render the chrome wordmark and Workspace tab",
        );
        h
    }

    // -------------------------------------------------------------------
    // SMOKE #24 — `/quit` clean exit (the always-on sanity anchor). Cheap,
    // proves the harness itself boots the real TUI and exits cleanly.
    // -------------------------------------------------------------------
    #[test]
    fn smoke_24_quit_exits_cleanly() {
        let home = TempDir::new().expect("tempdir");
        write_config(
            home.path(),
            "anthropic",
            Some("claude-sonnet-4-20250514"),
            None,
        );
        let mut h = boot(home.path());

        h.send(b"/");
        h.wait_for(
            |s| s.contains("command") || s.contains("/exit"),
            Duration::from_secs(3),
            "palette overlay to open",
        );
        h.send(b"exit");
        std::thread::sleep(Duration::from_millis(300));
        h.send(b"\r");
        let status = h
            .wait_for_exit(Duration::from_secs(8))
            .expect("wayland-core did not exit within 8s of /exit");
        assert!(status.success(), "expected a clean exit; got {status:?}");
    }

    // -------------------------------------------------------------------
    // SMOKE #15 (HARD GATE) — AskUserQuestion card: arrows move the choice,
    // Enter selects, and the footer must NOT advertise y/a/n (which are dead).
    // Defect D040. This is INTERACTIVE: the card is raised by a model turn,
    // so it needs a scripted mock that issues an AskUserQuestion tool call.
    //
    // Scaffolded as an interactive-pending TODO: raising the card requires the
    // engine to surface the AskUserQuestion surface, which the remediation wave
    // wires. Marked #[ignore] so the gate runner reports it as
    // interactive-pending (NOT silently skipped). When the wave lands, replace
    // the body with: script a mock AskUserQuestion turn, assert arrows move the
    // rendered selection marker and the footer omits "[y]"/"[a]"/"[n]".
    // -------------------------------------------------------------------
    #[test]
    #[ignore = "INTERACTIVE-PENDING (SMOKE #15 / D040): drive AskUserQuestion card via \
                scripted mock; assert RENDERED arrow-move + Enter-select and footer omits y/a/n"]
    fn smoke_15_askuser_card_arrows_and_no_yan_footer() {
        // Interactive-pending: see the module doc + the #[ignore] reason. The
        // gate runner surfaces this as uncovered until the wave wires it.
        panic!("SMOKE #15 not yet wired - interactive AskUserQuestion driving pending");
    }

    // -------------------------------------------------------------------
    // SMOKE #23 — `@`-completion popup: Tab inserts the candidate (does NOT
    // switch tabs). Defect D039. Interactive: needs a workspace file to
    // complete against and the @-popup open. Scaffolded interactive-pending.
    // The RENDERED assertion (per TUI discipline): after Tab, the composer
    // shows the inserted candidate AND the active surface is still Workspace
    // (the global Tab interceptor did NOT steal it to Sub-Agents).
    // -------------------------------------------------------------------
    #[test]
    #[ignore = "INTERACTIVE-PENDING (SMOKE #23 / D039): open @-popup, press Tab through the \
                Router; assert RENDERED candidate inserted AND surface stayed Workspace"]
    fn smoke_23_at_completion_tab_inserts_candidate() {
        panic!("SMOKE #23 not yet wired - interactive @-popup Tab driving pending");
    }

    // -------------------------------------------------------------------
    // SMOKE #22 — `?` on a non-Workspace surface shows a help overlay.
    // Defect D038 (keybind.rs Keymap + `?`-help overlay is dead code).
    // Driven through the real Router: Tab to a non-Workspace surface, press
    // `?`, assert a help overlay RENDERS. Currently dead → this fails now, so
    // it is grouped with the GAP checks (D038 is P1, but #22 is part of the
    // ordered smoke suite). Marked #[ignore] so default lane stays green.
    // -------------------------------------------------------------------
    #[test]
    #[ignore = "INTERACTIVE-PENDING (SMOKE #22 / D038): press `?` on a non-Workspace surface \
                through the Router; assert a help overlay RENDERS (dead code today)"]
    fn smoke_22_question_mark_shows_help_overlay() {
        let home = TempDir::new().expect("tempdir");
        write_config(
            home.path(),
            "anthropic",
            Some("claude-sonnet-4-20250514"),
            None,
        );
        let mut h = boot(home.path());

        // Tab off Workspace to Config (a surface where `?` claims help).
        h.send(b"\t\t\t"); // Workspace -> Sub-Agents -> Plan -> Config
        std::thread::sleep(Duration::from_millis(400));
        // Press `?` through the real Router (raw keystroke, not a surface call).
        h.send(b"?");
        // RENDERED assertion: a help overlay must appear. It does not today.
        h.wait_for(
            |s| {
                let l = s.to_lowercase();
                l.contains("help") && (l.contains("key") || l.contains("press"))
            },
            Duration::from_secs(5),
            "a `?` help overlay to render on a non-Workspace surface",
        );
        h.quit();
    }

    // ===================================================================
    // GAP CHECKS — PTY-driven (currently RED, prove the gap exists).
    // ===================================================================

    /// GAP D015 (HARD-GATE amplifier) — driving `//` through the REAL Router
    /// must NOT panic, must NOT leave the terminal bricked, and must NOT
    /// re-open the palette. The render/input loop `.expect()` on the App mutex
    /// does not recover poison today, so a panic under the lock aborts the
    /// process. We drive `/` then `/` (the documented palette mis-handle
    /// amplifier) and assert the TUI stays alive and exits cleanly afterward.
    ///
    /// Currently RED: until the loop uses `unwrap_or_else(|e| e.into_inner())`
    /// and `catch_unwind`, a poison panic strands the raw terminal. We assert the
    /// process is STILL responsive (can still quit cleanly) after `//`.
    #[test]
    fn gap_d015_double_slash_does_not_poison_or_brick() {
        let home = TempDir::new().expect("tempdir");
        write_config(
            home.path(),
            "anthropic",
            Some("claude-sonnet-4-20250514"),
            None,
        );
        let mut h = boot(home.path());

        // `/` opens the palette; a second `/` is the documented mis-handle.
        h.send(b"/");
        std::thread::sleep(Duration::from_millis(200));
        h.send(b"/");
        std::thread::sleep(Duration::from_millis(400));

        // The chrome must still be coherent — a poison abort would have killed
        // the child and frozen the vt100 grid.
        let screen = h.screen_text();
        assert!(
            screen.contains("WAYLAND") && screen.contains("Workspace"),
            "after `//` the TUI must still be alive and painting; screen:\n{screen}"
        );

        // And the process must still accept input and exit cleanly — proof the
        // event loop did not abort and the mutex is not poisoned.
        h.send(b"\x1b"); // Esc to close the palette
        std::thread::sleep(Duration::from_millis(200));
        h.quit();
        // If we reached here without a panic-driven kill, the loop survived.
    }

    /// GAP D009 — input dies after a turn completes under a large transcript.
    /// We stream a large (108KB) assistant turn via the mock, then assert input
    /// stays responsive: a composer keystroke is reflected on screen within a
    /// tight budget.
    ///
    /// Root cause (found by instrumenting the live render/input loop): NOT a
    /// render livelock. The windowed transcript render is O(viewport) and fast
    /// (the full 108KB build+wrap measured ~5ms; every frame ~3ms). The real
    /// defect was input delivery: the idle loop raced
    /// `spawn_blocking(poll_input)` inside a `select!` against a `wake` signal,
    /// and the bridge's post-turn `wake` permit made that arm win and DROP the
    /// poll future — orphaning a blocking `event::read()` that then consumed and
    /// discarded the user's next keystroke, so after a turn settled the TUI
    /// silently stopped accepting input. The fix moves the crossterm read onto a
    /// dedicated reader thread feeding a channel the loop consumes via a
    /// cancel-safe `recv()`; a dropped recv future no longer eats a keystroke.
    #[test]
    fn gap_d009_large_transcript_keeps_input_responsive() {
        // A single very long assistant turn that settles in one shot — after it
        // completes, the post-turn `wake` is what used to orphan the input poll.
        let big = "lorem ipsum dolor sit amet ".repeat(4000); // ~108 KB of text
        let (_rt, server) = start_mock(MockLlm::new().text(big.as_str()));
        let home = TempDir::new().expect("tempdir");
        write_config(
            home.path(),
            "anthropic",
            Some("claude-sonnet-4-20250514"),
            Some(&server.uri()),
        );
        let mut h = boot(home.path());

        h.send(b"dump a lot\r");
        h.wait_for(
            |s| s.contains("lorem ipsum"),
            Duration::from_secs(30),
            "the large transcript to stream in",
        );

        // Now type a sentinel char and assert it lands on screen within a tight
        // budget. With the orphaned-poll bug the keystroke was eaten by the
        // discarded blocking read and never rendered at all.
        let start = Instant::now();
        h.send(b"Zxq"); // an unlikely sentinel in the composer
        h.wait_for(
            |s| s.contains("Zxq"),
            Duration::from_secs(2),
            "a composer keystroke to render within budget under a large transcript",
        );
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(750),
            "input latency under a large transcript must stay under budget; took {elapsed:?}"
        );
        h.quit();
    }

    /// GAP D010 — huge paste stalls every frame (O(n) line-count + full-buffer
    /// realloc per keystroke/frame). We bracket-paste a few hundred KB into the
    /// composer, then assert the composer either caps it with a toast or stays
    /// responsive. Currently RED: neither cap nor cached line-count exists.
    #[test]
    fn gap_d010_huge_paste_is_capped_or_responsive() {
        let home = TempDir::new().expect("tempdir");
        write_config(
            home.path(),
            "anthropic",
            Some("claude-sonnet-4-20250514"),
            None,
        );
        let mut h = boot(home.path());

        // Bracketed paste: ESC[200~ <payload> ESC[201~ — how a terminal
        // delivers a paste so the app's handle_paste path (not per-key) runs.
        let payload = "x".repeat(400_000);
        let mut bytes = Vec::with_capacity(payload.len() + 16);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(payload.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        h.send(&bytes);
        std::thread::sleep(Duration::from_millis(400));

        // After the paste, a tiny keystroke must still register quickly OR the
        // composer must show a cap/toast. Either is acceptable; an unbounded
        // composer that hangs is not.
        let start = Instant::now();
        h.send(b"Q");
        h.wait_for(
            |s| {
                s.contains('Q')
                    || s.contains("Paste was")
                    || s.to_lowercase().contains("too large")
                    || s.contains("capped")
            },
            Duration::from_secs(2),
            "a keystroke to register (or a cap/toast) after a huge paste",
        );
        assert!(
            start.elapsed() < Duration::from_millis(900),
            "composer must stay responsive (or cap) after a huge paste"
        );
        h.quit();
    }

    /// GAP D002 — catalog-provider onboarding writes NO model, so the first
    /// prompt dead-ends: the Workspace submit blocks and renders the no-model
    /// banner whose only remedy is "run `wayland-core setup`" or hand-edit
    /// `config.toml` (workspace.rs no-model guard). There is NO in-app `/model`
    /// affordance to recover without leaving the session. This is a TUI-path
    /// behavior (the dead-end lives in the Workspace submit, not the headless
    /// one-shot path — a `--no-tui` run would instead POST an empty-model request
    /// to the catalog endpoint and never show the banner), so it is driven
    /// through the REAL TUI via the PTY and asserts on the RENDERED screen.
    ///
    /// Seeds a REAL bundled catalog id (`novita-ai`) with NO model AND no
    /// `[providers.novita-ai]` overlay: the data-driven catalog fallthrough is
    /// guarded by `!providers.contains_key(requested)` (config.rs
    /// `resolve_provider_alias`), so an overlay would instead trip the "alias
    /// requires a 'provider' field" error. With no overlay it resolves with
    /// `catalog_entry.is_some()` → ProviderType::OpenAI and the empty-model branch
    /// (config.rs ~1062) yields `String::new()` — `config.model.is_empty()` is
    /// true and the banner fires. Seeding `anthropic` (non-catalog) would pick
    /// `default_model_for(provider)` and never dead-end, hiding the gap.
    ///
    /// Currently RED: the dead-end banner renders with only setup/hand-edit
    /// guidance and NO actionable in-app `/model` recovery. The fixed behavior
    /// (onboarding prompts for a model, or the banner offers an in-app `/model`
    /// picker) turns this green. Hermetic: the submit blocks BEFORE any provider
    /// call, so no network is touched.
    #[test]
    fn gap_d002_catalog_provider_no_credential_recovers_in_app_not_crash() {
        // Catalog id, NO model, NO key, NO `[providers.novita-ai]` overlay. The
        // catalog provider resolves to ProviderType::OpenAI but no api_key is
        // found by any source (CLI / config / store / env), so
        // `Config::resolve` returns a typed `MissingApiKey`.
        let home = TempDir::new().expect("tempdir");
        std::fs::write(
            home.path().join("config.toml"),
            "[default]\nprovider = \"novita-ai\"\n",
        )
        .expect("write catalog-no-credential config");

        // The defect (D002): this config EXISTS, so the old first-run-only
        // recovery gate was skipped and the binary CRASHED to stderr ("No API
        // key found ...") and exited non-zero BEFORE the TUI ever rendered — a
        // quit-to-shell dead-end. The fix routes a
        // `MissingApiKey` resolve error on an interactive launch into the
        // Onboarding surface for in-app recovery. We assert the RENDERED
        // onboarding chrome ("Connect a provider" card title) appears, proving
        // the binary did NOT crash and the user can finish setup in-session.
        //
        // We spawn directly (not via `boot()`, which waits for Workspace chrome)
        // because the recovery lands on Onboarding, not Workspace.
        let mut h = Pty::spawn(home.path());
        h.wait_for(
            |s| s.contains("Connect a provider"),
            Duration::from_secs(60),
            "in-app Onboarding recovery (the `Connect a provider` card) to render \
             for a keyless catalog config instead of crashing to stderr",
        );
        h.quit();
    }

    /// GAP D011 (project layer) — a corrupt PROJECT `.wayland-core.toml` under an
    /// INTERACTIVE TUI launch must surface the file-named parse error and refuse
    /// the silent downgrade, exactly like the headless `gap_d011` check does for
    /// the GLOBAL file under `--no-tui`.
    ///
    /// The dataloss this pins: the boot recovery gate used to swallow ANY resolve
    /// error into onboarding when `first_run` was true, and `first_run` inspects
    /// ONLY the global file. So a returning user with NO global config but a
    /// corrupt project config (a bad hand-edit; common in a fresh-global +
    /// populated-repo CI scaffold) booted into a fresh-install walkthrough — their
    /// real config ignored, the corrupt file never named — instead of a visible
    /// parse error. A `--no-tui` headless run can NOT exercise this: with stdout
    /// piped, `would_open_tui` is false and the error always propagates, so the
    /// swallow branch is unreachable. We MUST drive a real PTY (where
    /// `is_terminal(stdout)` is true and `would_open_tui` flips on) to reach it.
    ///
    /// Fixed contract (D011): a `ConfigLoadError::ParseFailed` is propagated
    /// BEFORE the onboarding branch even under a TUI launch, so the process exits
    /// non-zero with anyhow's `Error: failed to parse .wayland-core.toml: ...`
    /// printed to the terminal — never the onboarding chrome. The TUI never opens,
    /// so we assert on the terminal output + the exit status, not rendered chrome.
    ///
    /// Hermetic: no global config and no provider env (the PTY spawn strips the
    /// full `STRIPPED_PROVIDER_ENV` set), and the parse failure aborts boot before
    /// any provider call, so nothing touches the network.
    #[test]
    fn gap_d011_corrupt_project_config_under_tui_surfaces_parse_error() {
        let home = TempDir::new().expect("tempdir");
        // NO global config.toml — only a corrupt PROJECT file in the cwd. The PTY
        // spawn runs with cwd == home, so `.wayland-core.toml` here is the
        // project-local config the resolver loads after the (absent) global one.
        // A stray trailing comma / dangling bracket — invalid TOML.
        std::fs::write(
            home.path().join(".wayland-core.toml"),
            "[default\nprovider = \"anthropic\",,\nmodel = \n",
        )
        .expect("write corrupt project config");
        assert!(
            !home.path().join("config.toml").exists(),
            "test invariant: NO global config must exist for this gap"
        );

        let mut h = Pty::spawn(home.path());

        // The fix makes boot abort visibly: the process must exit non-zero rather
        // than swallow the corrupt project config into an onboarding walkthrough.
        let status = h.wait_for_exit(Duration::from_secs(60)).expect(
            "corrupt project config under a TUI launch must abort boot (process exit), \
             not hang in an onboarding walkthrough",
        );
        let screen = h.screen_text();

        // L3: assert the SPECIFIC refusal contract on the SAME terminal line — the
        // parse-class wording and the `.wayland-core.toml` filename together, so a
        // stray filename mention plus an unrelated "invalid" word cannot pass.
        let parse_error_names_project_file = screen.lines().any(|line| {
            let l = line.to_lowercase();
            l.contains(".wayland-core.toml")
                && (l.contains("parse") || l.contains("invalid") || l.contains("malformed"))
        });
        assert!(
            parse_error_names_project_file && !status.success(),
            "corrupt PROJECT config under a TUI launch must surface a visible parse error that \
             NAMES .wayland-core.toml and refuse the silent onboarding downgrade (non-zero exit); \
             got exit success={}\n--- terminal ---\n{screen}\n--- end ---",
            status.success()
        );
    }
}

// ===========================================================================
// GAP CHECKS — headless (cross-platform, currently RED, prove the gap).
// ===========================================================================

/// GAP D011 — a corrupt `config.toml` silently discards the entire user config
/// and behaves like a fresh install; the only signal is a `Warning:` line on
/// stderr, hidden behind the alt-screen. We launch headless with a malformed
/// config and assert the parse failure is surfaced as a VISIBLE, file-named
/// error that refuses the silent downgrade — not swallowed into defaults.
///
/// Currently RED: `load_config_file` does `eprintln!("Warning: ...")` then
/// returns `ConfigFile::default()` — a silent dataloss-class downgrade.
#[test]
fn gap_d011_corrupt_config_surfaces_parse_error_not_silent_default() {
    let home = TempDir::new().expect("tempdir");
    // A stray trailing comma / dangling bracket — invalid TOML.
    std::fs::write(
        home.path().join("config.toml"),
        "[default\nprovider = \"anthropic\",,\nmodel = \n",
    )
    .expect("write corrupt config");

    let (status, stdout, stderr) = run_headless(home.path(), &["--no-tui", "hi", "there"]);
    let combined = format!("{stdout}\n{stderr}");

    // Fixed behavior: a clear, file-named error that does NOT silently downgrade.
    // The defect today: at most a soft "Warning:" then default() (a fresh-install
    // downgrade), and the run otherwise proceeds as if no config existed.
    //
    // L3: assert the SPECIFIC refusal contract, not mere co-occurrence — the
    // parse-class wording and the `config.toml` filename must appear on the SAME
    // output line (a real "failed to parse config.toml ..." message), so a stray
    // "config.toml" mention elsewhere plus an unrelated "invalid" word cannot
    // accidentally pass. And the run must refuse the downgrade (non-zero exit).
    let parse_error_names_file = combined.lines().any(|line| {
        let l = line.to_lowercase();
        l.contains("config.toml")
            && (l.contains("parse") || l.contains("invalid") || l.contains("malformed"))
    });
    assert!(
        parse_error_names_file && !status.success(),
        "corrupt config must surface a single visible parse error that NAMES config.toml and \
         refuse a silent downgrade (non-zero exit); got status {status:?}\n--- output ---\n{combined}"
    );
}

/// GAP D012 — the ACP/REST front-end runs the engine ungated (no approval /
/// plan / tool-gating vocabulary). We drive a mutating tool over the
/// `--json-stream` protocol front-end and assert the host sees an approval /
/// gate event before the tool executes. Today the protocol path has no approval
/// channel, so the tool runs with no gate — RED.
///
/// Driven headless via `--json-stream`: feed a user message that the mock
/// answers with a gated Write tool call, then assert the emitted event stream
/// contains an approval-request event (not a bare tool-executed event).
#[test]
fn gap_d012_acp_protocol_gates_mutating_tools() {
    use std::io::{BufRead, BufReader, Write};
    use std::time::Duration;

    let home = TempDir::new().expect("tempdir");
    let target = home.path().join("acp_gate_probe.txt");
    let target_arg = target.to_str().expect("utf-8 path").to_string();

    // Turn 1: a gated Write call. Turn 2: closing text (only reached if the
    // tool was allowed to proceed).
    let (_rt, server) = start_mock(
        MockLlm::new()
            .tool_use(
                "Write",
                serde_json::json!({ "file_path": target_arg, "content": "ACP_UNGATED" }),
            )
            .text("done"),
    );
    write_config(
        home.path(),
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(&server.uri()),
    );

    let mut cmd = std::process::Command::new(binary());
    cmd.args(["--json-stream", "--provider", "anthropic"])
        .current_dir(home.path());
    let vault = harden_child_env(&mut cmd, home.path());
    let child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    drop(vault);
    let mut child = child.expect("spawn --json-stream");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    // Send the REAL `message` command over the protocol. `ProtocolCommand`
    // (wcore-protocol/src/commands.rs, serde tag="type" snake_case) has a
    // `Message { msg_id, content }` variant and NO `user_message` variant — the
    // old `{"type":"user_message",...}` frame deserialized to nothing, the reader
    // dropped it, and no turn ran, so `saw_approval` could never flip even once
    // ACP gating lands (a false-RED that can never go green). With the real frame
    // the Write turn actually runs ungated today (RED for the right reason) and
    // the same assertion goes green once the ACP path gates the tool.
    writeln!(
        stdin,
        "{{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"write the probe\"}}"
    )
    .expect("write message");

    // Collect emitted event lines for a short window.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut saw_approval = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                let l = line.to_lowercase();
                // Match the genuine gate EVENT, not any line containing
                // "approval" — the startup Ready frame advertises
                // `"tool_approval":true` in its capabilities, which would
                // false-match a loose substring scan in BOTH postures.
                if l.contains("approval_required") {
                    saw_approval = true;
                    break;
                }
            }
            Err(_) => {
                if target.exists() {
                    // The tool already wrote the file with no gate — the defect.
                    break;
                }
            }
        }
    }
    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        saw_approval,
        "a mutating tool over --json-stream must emit an approval/gate event before executing; \
         none seen (ACP runs ungated — D012). file_written={}",
        target.exists()
    );
}

// ===========================================================================
// ONE REAL-KEY HAPPY PATH (opt-in via SMOKE_LIVE=1).
// ===========================================================================

/// The single genuine end-to-end real-provider check: one real key, the
/// cheapest model, one short turn, asserting the engine completes a turn
/// against the live endpoint. Gated behind `SMOKE_LIVE=1` AND `#[ignore]` so CI
/// runs everything else hermetically. Run with:
///
/// ```text
/// SMOKE_LIVE=1 ANTHROPIC_API_KEY=sk-... \
///   cargo test --package wcore-cli --test smoke_p0 -- --ignored \
///   live_real_key_first_prompt_round_trip
/// ```
#[test]
#[ignore = "LIVE: requires SMOKE_LIVE=1 + a real ANTHROPIC_API_KEY; one cheap real turn"]
fn live_real_key_first_prompt_round_trip() {
    if std::env::var("SMOKE_LIVE").ok().as_deref() != Some("1") {
        eprintln!("[smoke_p0] SMOKE_LIVE != 1 — skipping the real-key happy path");
        return;
    }
    let key = std::env::var("ANTHROPIC_API_KEY")
        .expect("SMOKE_LIVE=1 requires a real ANTHROPIC_API_KEY in the env");

    let home = TempDir::new().expect("tempdir");
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "[default]\nprovider = \"anthropic\"\nmodel = \"claude-haiku-4-5\"\n\
             \n[providers.anthropic]\napi_key = \"{key}\"\n"
        ),
    )
    .expect("write config");

    // Note: do NOT strip ANTHROPIC_API_KEY here — this is the live path.
    let mut cmd = std::process::Command::new(binary());
    cmd.args(["--no-tui", "Reply with exactly: SMOKE_LIVE_OK"])
        .current_dir(home.path())
        .env("WAYLAND_HOME", home.path())
        .env("HOME", home.path())
        .env("TERM", "dumb")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let vault = support::vault::configure_process(&mut cmd);
    let child = cmd.spawn();
    drop(vault);
    let out = child
        .expect("spawn live wayland-core")
        .wait_with_output()
        .expect("wait for live wayland-core");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "live real-key turn must exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("SMOKE_LIVE_OK"),
        "live turn must surface the model's reply; stdout: {stdout}"
    );
}
