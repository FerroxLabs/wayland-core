//! #162 — regression guard for per-assistant MCP scoping on the DEFERRED
//! (wayland#551) config-MCP connect path.
//!
//! #111 added `only_for_assistant` scoping for config MCP servers. The filter
//! (`McpConfig::servers_for_assistant`) is applied at TWO injection choke
//! points: `AgentBootstrap::build()` and the wayland#551 deferred background
//! connect inside `run_json_stream_mode`. The bootstrap choke point is pinned
//! by `crates/wcore-agent/tests/bootstrap_test.rs`; the deferred path had NO
//! test. If the one-line filter on the deferred path
//! (`config.mcp.servers_for_assistant(assistant.as_deref())`, main.rs) were
//! reverted in a refactor, a scoped server would leak to every assistant and
//! nothing in the suite would notice. This test closes that gap.
//!
//! ## How "was the server dialed?" is observed
//!
//! The deferred path background-connects the scoped servers and then emits one
//! `mcp_ready`/`mcp_failed` json-stream event PER SERVER it attempted (see
//! `integrate_deferred_mcp` / `mcp_failed_events_for`). `McpManager::connect_all`
//! records per-server health and always returns `Ok`, so a server pointed at a
//! non-existent command settles as `Failed` and still emits a
//! `{"type":"mcp_failed","name":...}` event naming it. That event's PRESENCE is
//! therefore a reliable, fully cross-platform proof that the server was in the
//! scoped set and a connect was attempted — no real MCP handshake needed.
//!
//! The config declares two stdio servers, both pointed at a bogus command:
//!   * `open_diag` — UNSCOPED (global). It is dialed for every assistant, so
//!     seeing its event is a synchronization barrier proving the deferred phase
//!     actually ran (guards against a false negative where the phase never
//!     executed and the scoped server "isn't dialed" for the wrong reason).
//!   * `diag` — scoped `only_for_assistant = ["concierge"]`. It must be dialed
//!     ONLY for `--assistant concierge`, and never for a non-matching or absent
//!     assistant (fail-closed).
//!
//! ## Hermetic by construction
//!
//! Mirrors `acp_gate_d012.rs`: every child points `WAYLAND_HOME` + `HOME` at a
//! throwaway tempdir and strips the full provider-credential env set. No LLM
//! turn is driven (no message frame is sent) — the deferred connect settles in
//! the pre-message phase — so no provider is ever contacted.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Path to the debug binary under test (Cargo wires this env var).
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Provider-credential env vars every spawned child must NOT inherit, so a run
/// can neither read the developer's real keys nor auto-detect a stray dev
/// credential. Mirrors `acp_gate_d012.rs::STRIPPED_PROVIDER_ENV`.
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
    // Do not let a real active assistant leak in from the environment and
    // change scoping under the test.
    "WAYLAND_ASSISTANT",
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

/// Seed `<home>/config.toml` with a bootable `[default]` and the two deferred
/// stdio MCP servers (one global, one scoped to the `concierge` assistant).
/// Both point at a non-existent command so the connect settles `Failed` fast
/// and emits a naming `mcp_failed` event without any real MCP server.
fn write_config(home: &Path) {
    // A non-existent command name: the connect attempt fails fast (the server
    // reaches `Failed` health) on every platform, which is exactly the signal
    // this test keys on. The point is that a connect was ATTEMPTED, not that it
    // succeeded.
    const PROBE_CMD: &str = "wayland_mcp_scoping_probe_absent_cmd";
    let toml = format!(
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\
         \n[providers.anthropic]\napi_key = \"diagnostics-redaction-sentinel\"\n\
         base_url = \"http://127.0.0.1:9/unused\"\n\
         \n[mcp.servers.open_diag]\ntransport = \"stdio\"\ncommand = \"{PROBE_CMD}\"\n\
         args = [\"--noop\"]\n\
         \n[mcp.servers.diag]\ntransport = \"stdio\"\ncommand = \"{PROBE_CMD}\"\n\
         args = [\"--noop\"]\nonly_for_assistant = [\"concierge\"]\n"
    );
    std::fs::write(home.join("config.toml"), toml).expect("write config.toml");
}

/// Which deferred MCP servers were dialed (observed via a per-server
/// `mcp_ready`/`mcp_failed` event) for a given `--assistant`.
struct DialObs {
    /// The unscoped `open_diag` server — dialed for every assistant. Used as a
    /// barrier: `false` means the deferred phase never ran.
    saw_open: bool,
    /// The scoped `diag` server — dialed only for a matching assistant.
    saw_diag: bool,
    diagnostics: Option<serde_json::Value>,
    unsupported: Option<serde_json::Value>,
}

/// Boot `--json-stream` with the given active assistant and observe which of the
/// two deferred config-MCP servers get dialed (emit a per-server
/// `mcp_ready`/`mcp_failed` event). No LLM turn is driven.
fn observe_deferred_dials(assistant: Option<&str>) -> DialObs {
    let home = TempDir::new().expect("tempdir");
    write_config(home.path());

    let mut args = vec!["--json-stream", "--provider", "anthropic"];
    if let Some(name) = assistant {
        args.push("--assistant");
        args.push(name);
    }
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

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut saw_open = false;
    let mut saw_diag = false;
    let mut diagnostics = None;
    let mut unsupported = None;
    let mut diagnostics_requested = false;
    let mut unsupported_requested = false;
    // The deferred connect settles in the pre-message phase, so the events
    // arrive within a second or two. The window is generous to tolerate the
    // slower connect-timeout path on a loaded CI runner.
    let deadline = Instant::now() + Duration::from_secs(20);
    // Once the barrier server's event is seen, the whole deferred batch has been
    // emitted in one burst; drain a short grace to capture the scoped event if
    // it is in the same burst, then stop.
    let mut grace_until: Option<Instant> = None;
    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200))
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&line)
        {
            let ty = v
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if ty == "mcp_ready" || ty == "mcp_failed" {
                match v.get("name").and_then(|n| n.as_str()) {
                    Some("open_diag") => {
                        saw_open = true;
                        grace_until.get_or_insert(Instant::now() + Duration::from_millis(800));
                    }
                    Some("diag") => saw_diag = true,
                    _ => {}
                }
            }
            if ty == "runtime_diagnostics_snapshot"
                && v.get("request_id").and_then(|id| id.as_str()) == Some("pre-message")
            {
                diagnostics = Some(v);
            } else if ty == "runtime_diagnostics_unavailable"
                && v.get("request_id").and_then(|id| id.as_str()) == Some("unsupported")
            {
                unsupported = Some(v);
            }
        }
        if saw_open && !diagnostics_requested {
            writeln!(
                stdin,
                "{{\"type\":\"get_runtime_diagnostics\",\"diagnostics_version\":1,\"request_id\":\"pre-message\"}}"
            )
            .expect("request pre-message diagnostics");
            diagnostics_requested = true;
        }
        if diagnostics.is_some() && !unsupported_requested {
            writeln!(
                stdin,
                "{{\"type\":\"get_runtime_diagnostics\",\"diagnostics_version\":2,\"request_id\":\"unsupported\"}}"
            )
            .expect("request unsupported diagnostics version");
            unsupported_requested = true;
        }
        if let Some(g) = grace_until
            && Instant::now() >= g
            && diagnostics.is_some()
            && unsupported.is_some()
        {
            break;
        }
    }

    let _ = writeln!(stdin, "{{\"type\":\"stop\"}}");
    let _ = child.kill();
    let _ = child.wait();

    DialObs {
        saw_open,
        saw_diag,
        diagnostics,
        unsupported,
    }
}

fn assert_runtime_diagnostics(obs: &DialObs, scoped_connection: &str) {
    let diagnostics = obs
        .diagnostics
        .as_ref()
        .expect("pre-message diagnostics response");
    let servers = diagnostics["snapshot"]["mcp_servers"]
        .as_array()
        .expect("MCP diagnostics array");
    let scoped = servers
        .iter()
        .find(|server| server["name"] == "diag")
        .expect("scoped declaration remains visible");
    assert_eq!(scoped["connection"], scoped_connection);
    let remediation = scoped["remediation"]
        .as_array()
        .expect("diagnostics remediation array");
    if scoped_connection == "failed" {
        assert_eq!(scoped["executable_readiness"], "not_found");
        for expected in [
            "install_executable",
            "fix_gui_launch_path",
            "restart_desktop",
        ] {
            assert!(
                remediation.iter().any(|code| code == expected),
                "missing actionable readiness remediation {expected}"
            );
        }
    } else {
        assert_eq!(scoped["executable_readiness"], "unchecked");
        assert!(
            remediation
                .iter()
                .any(|code| code == "check_assistant_scope"),
            "a scoped-out server must explain why it was not inspected"
        );
    }

    let serialized = serde_json::to_string(diagnostics).unwrap();
    for canary in ["diagnostics-redaction-sentinel", "--noop", "127.0.0.1:9"] {
        assert!(!serialized.contains(canary), "diagnostics leaked {canary}");
    }

    let unsupported = obs
        .unsupported
        .as_ref()
        .expect("unsupported version receives terminal response");
    assert_eq!(unsupported["type"], "runtime_diagnostics_unavailable");
    assert_eq!(unsupported["diagnostics_version"], 2);
    assert_eq!(unsupported["supported_version"], 1);
    assert_eq!(unsupported["reason"], "unsupported_version");
}

/// Matching assistant: the scoped `diag` server MUST be dialed on the deferred
/// path (alongside the always-global `open_diag`).
#[test]
fn matching_assistant_dials_scoped_deferred_server() {
    let obs = observe_deferred_dials(Some("concierge"));
    assert!(
        obs.saw_open,
        "the deferred phase must run: the global open_diag server should always be dialed"
    );
    assert!(
        obs.saw_diag,
        "a server scoped to `concierge` MUST be dialed on the deferred path for --assistant concierge"
    );
    assert_runtime_diagnostics(&obs, "failed");
}

/// Non-matching assistant: the scoped `diag` server must NOT be dialed, while
/// the global `open_diag` still is (proving the phase ran and the scoping is
/// what excluded `diag`, not a dead deferred phase).
#[test]
fn nonmatching_assistant_does_not_dial_scoped_deferred_server() {
    let obs = observe_deferred_dials(Some("some_other_assistant"));
    assert!(
        obs.saw_open,
        "the global open_diag server should still be dialed for a non-matching assistant"
    );
    assert!(
        !obs.saw_diag,
        "a server scoped to `concierge` must NOT be dialed on the deferred path for a \
         non-matching assistant"
    );
    assert_runtime_diagnostics(&obs, "skipped");
}

/// Absent assistant (bare `--json-stream`, no `--assistant`): fail-closed — the
/// scoped `diag` server must NOT be dialed; the global `open_diag` still is.
#[test]
fn absent_assistant_does_not_dial_scoped_deferred_server() {
    let obs = observe_deferred_dials(None);
    assert!(
        obs.saw_open,
        "the global open_diag server should still be dialed when no assistant is set"
    );
    assert!(
        !obs.saw_diag,
        "fail-closed: a scoped server must NOT be dialed on the deferred path when the active \
         assistant is unset"
    );
    assert_runtime_diagnostics(&obs, "skipped");
}
