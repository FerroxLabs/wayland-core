//! Residual (a) — the browser sidecar WIRING, graded end to end.
//!
//! Three tests already grade the supervisor's launch FUNCTION, and all three
//! hand-build a `SupervisorConfig` and call `BrowserSupervisor::ensure_ready`
//! directly:
//!
//!   * `wcore-browser/tests/camoufox_provisioning_wiring_test.rs`
//!   * `wcore-browser/tests/missing_sidecar_diagnosis_test.rs`
//!   * `wcore-agent/tests/browser_e2e_test.rs` — which builds the tool it
//!     drives inline ("We replicate the wiring inline here"), with a
//!     `BrowserSupervisor::new()` whose `sidecar_program` is `None`, so its
//!     `ensure_ready` returns on its first line and the launch path is never
//!     entered.
//!
//! What none of them touches is the LINKS between those functions, so all
//! three stay green while any of these is severed:
//!
//!   1. `wcore_browser::adapter::from_spec` building
//!      `SupervisorConfig::local_camoufox` for the camoufox backend — the only
//!      production code that ever sets `sidecar_program` to something. Cut it
//!      and nothing is ever launched.
//!   2. the same function attaching `egress_policy` (gh#1117) — cut it and the
//!      sidecar starts with no PROXY_* in its environment and resolves its own
//!      DNS, silently.
//!   3. `BrowserTool::ensure_session` calling `supervisor.ensure_ready()` — cut
//!      it and the tool talks to a sidecar port nobody ever started.
//!
//! This file drives the real chain — plugin-registered `BrowserToolSpec` →
//! `HostBrowserRegistrar::reify_all` → `spec_to_core` → `from_spec` →
//! `BrowserTool::execute_with_ctx` → `ensure_session` → `ensure_ready` →
//! `launch_camoufox_program` — and asserts on the LAUNCHED PROCESS: a stub
//! sidecar that records the environment it was handed before it answers the
//! health poll. Nothing is mocked below the tool surface, no network is
//! contacted, and `@askjo/camofox-browser` does not have to be installed.
//!
//! **Unix only** — the stub sidecar is a `#!`-script and the executable bit is
//! a Unix concept, exactly as in `camoufox_provisioning_wiring_test`. Windows
//! is a gap here, not a pass.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar;
use wcore_browser::sidecar_prefs::{LOOPBACK_PROXY_PREF, PREF_FILE_NAME};
use wcore_browser::tool::BrowserTool;
use wcore_plugin_api::browser_spec::{BrowserPolicySpec, BrowserProviderHint, BrowserToolSpec};
use wcore_plugin_api::registry::browser::BrowserToolRegistrar;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;

/// A literal public IP rather than a hostname: `BrowserPolicy`'s resolution
/// gate fails closed on a host that cannot resolve, and a hostname fixture
/// would make this test require working DNS on the runner.
const TARGET: &str = "https://93.184.216.34/";

/// The file Firefox itself installs in the directory the loopback pref has to
/// be written into. `sidecar_prefs` locates the install BY this marker, so the
/// fake install below has to carry it.
const FIREFOX_PREF_MARKER: &str = "channel-prefs.js";

// ── environment ───────────────────────────────────────────────────────────

/// Restores every variable it set, including back to *unset*.
struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn set(&mut self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) {
        self.0.push((key, std::env::var_os(key)));
        unsafe { std::env::set_var(key, value) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prior) in self.0.drain(..).rev() {
            match prior {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

// ── fixtures ──────────────────────────────────────────────────────────────

/// The same question `BrowserSupervisor::resolve_sidecar_program` asks of the
/// configured program, minus the `which` crate (not a dependency of this
/// crate). Only used as a precondition, never as the thing under test.
fn resolves_on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).is_file())
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// A stand-in sidecar. `launch_camoufox_program` passes NO arguments, so the
/// port and the marker path are baked in.
///
/// It records the PROXY_* environment it was handed **before** it binds its
/// socket, so a health poll that succeeds implies the record is already on
/// disk — the assertion can never race the process.
fn write_stub_sidecar(dir: &Path, port: u16, marker: &Path) -> PathBuf {
    let script = format!(
        r#"#!/usr/bin/env python3
import http.server, json, os, socketserver
with open({marker:?}, "w") as fh:
    json.dump({{k: v for k, v in os.environ.items() if k.startswith("PROXY_")}}, fh)
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.send_header("Content-Length", "2"); self.end_headers()
        self.wfile.write(b"ok")
    def log_message(self, *a): pass
socketserver.TCPServer.allow_reuse_address = True
socketserver.TCPServer(("127.0.0.1", {port}), H).serve_forever()
"#,
        marker = marker.to_str().unwrap(),
    );
    let path = dir.join("stub-camofox-browser");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A directory shaped like a Camoufox install: the marker file `sidecar_prefs`
/// searches for, plus the executable an operator would point
/// `CAMOUFOX_EXECUTABLE_PATH` at. Lets the gh#1117 loopback containment step
/// run for real without a 300 MB browser on the runner.
fn fake_camoufox_install(root: &Path) -> PathBuf {
    let pref_dir = root.join("defaults").join("pref");
    std::fs::create_dir_all(&pref_dir).unwrap();
    std::fs::write(pref_dir.join(FIREFOX_PREF_MARKER), "// stand-in\n").unwrap();
    let exe = root.join("camoufox-bin");
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    exe
}

/// The spec the `wayland-browser` plugin registers, with the operator's policy
/// already copied onto it. Camoufox is forced because the camoufox branch of
/// `from_spec` is what is under test.
///
/// ORDERING TRAP: `BrowserTool::execute_with_ctx` runs `policy_check` BEFORE
/// `ensure_session`, so a deny-by-default policy would make every assertion
/// below pass or fail for a reason that has nothing to do with the sidecar.
/// The policy here allows the navigation target so the op reaches the wiring.
fn plugin_spec() -> BrowserToolSpec {
    BrowserToolSpec {
        tool_namespace: "Browser".into(),
        preferred_provider: BrowserProviderHint::Camoufox,
        policy: BrowserPolicySpec {
            default_action: "allow".into(),
            allowed_origins: vec!["93.184.216.34".into()],
            denied_origins: vec![],
            loopback: Default::default(),
        },
        allow_cloud: false,
    }
}

/// The production reification path: exactly what `AgentBootstrap` calls after
/// `PluginRunner::initialize_all` returns.
fn reify_from_plugin_registration() -> Arc<BrowserTool> {
    let mut host = HostBrowserRegistrar::default();
    host.host_register(plugin_spec()).unwrap();
    let mut tools = host.reify_all();
    assert_eq!(tools.len(), 1, "one registered spec must reify to one tool");
    tools.pop().unwrap()
}

async fn navigate(tool: &Arc<BrowserTool>) -> wcore_types::tool::ToolResult {
    tool.execute_with_ctx(
        json!({ "op": { "kind": "navigate", "url": TARGET } }),
        &ToolContext::test_default(),
    )
    .await
}

// ── arms ──────────────────────────────────────────────────────────────────

/// THE WIRING. A browser op on a tool built the way the engine builds it
/// launches the configured sidecar, behind the egress proxy, with loopback
/// contained.
///
/// The assertion is the launched PROCESS — a record only the child can have
/// written — not a return value, because every link under test is one whose
/// removal still returns `Ok`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial(browser_sidecar_wiring_env)]
async fn a_browser_op_launches_the_configured_sidecar_through_the_real_adapter() {
    let tmp = tempfile::tempdir().unwrap();
    let port = free_port();
    let marker = tmp.path().join("sidecar-launched.json");
    let stub = write_stub_sidecar(tmp.path(), port, &marker);
    let install = tmp.path().join("camoufox-install");
    let camoufox_exe = fake_camoufox_install(&install);
    let pref_file = install.join("defaults").join("pref").join(PREF_FILE_NAME);

    let mut env = EnvGuard::new();
    env.set("WAYLAND_HOME", tmp.path());
    env.set("WAYLAND_CAMOUFOX_URL", format!("http://127.0.0.1:{port}"));
    env.set("WAYLAND_CAMOUFOX_BIN", &stub);
    env.set("CAMOUFOX_EXECUTABLE_PATH", &camoufox_exe);
    // Pin the fail-closed posture: containment below must genuinely succeed,
    // not be waived. Otherwise the pref assertion could pass vacuously.
    env.set("WAYLAND_BROWSER_ALLOW_UNPROXIED_SIDECAR", "0");

    // PRECONDITIONS. Without these the launch assertion could be satisfied by
    // a sidecar somebody else started, or by a stale file.
    assert!(
        !marker.exists(),
        "the launch record exists before the test ran"
    );
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "port {port} is already served; a reused external sidecar would make this vacuous"
    );
    assert!(
        !pref_file.exists(),
        "the loopback pref exists before the test ran"
    );

    let tool = reify_from_plugin_registration();
    let result = navigate(&tool).await;

    // 1. IT LAUNCHED. Only `launch_camoufox_program` can produce this file,
    //    and it is only reached if `from_spec` set `sidecar_program` AND
    //    `ensure_session` called `ensure_ready`.
    assert!(
        marker.is_file(),
        "the configured sidecar was never launched — no launch record at {}. \
         Tool result was: {}",
        marker.display(),
        result.content
    );

    // 2. IT LAUNCHED BEHIND CORE'S EGRESS GATE (gh#1117). PROXY_* reaches the
    //    child only when `from_spec` attached `egress_policy`, which is what
    //    makes the supervisor start a proxy at all.
    let recorded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
    let proxy_host = recorded.get("PROXY_HOST").and_then(|v| v.as_str());
    assert_eq!(
        proxy_host,
        Some("127.0.0.1"),
        "the sidecar was started without Core's egress proxy in its environment: {recorded}"
    );
    let proxy_port: u16 = recorded
        .get("PROXY_PORT")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);
    assert_ne!(proxy_port, 0, "PROXY_PORT is not a live port: {recorded}");
    assert_ne!(
        proxy_port, port,
        "PROXY_PORT points at the sidecar itself, not at Core's proxy: {recorded}"
    );

    // 3. AND WITH LOOPBACK CONTAINED — the launch-path half of gh#1117, which
    //    only runs because `egress_policy` was attached upstream.
    let pref = std::fs::read_to_string(&pref_file).unwrap_or_else(|e| {
        panic!(
            "the loopback pref was not written to {}: {e}",
            pref_file.display()
        )
    });
    assert!(
        pref.contains(LOOPBACK_PROXY_PREF),
        "the pref file does not set {LOOPBACK_PROXY_PREF}: {pref}"
    );

    // 4. And the op got PAST both gates it had to pass to get here, so none of
    //    the above was reached by an error path.
    assert!(
        !result.content.contains("policy:"),
        "the op was policy-denied, so it never reached the sidecar wiring: {}",
        result.content
    );
    assert!(
        !result
            .content
            .contains(wcore_browser::install::CAMOUFOX_SIDECAR_PACKAGE),
        "the op was answered with the not-installed message: {}",
        result.content
    );

    drop(tool);
}

/// CONTROL, and the known-positive for the launch-record query above.
///
/// Identical wiring, one variable changed: the configured sidecar program
/// cannot resolve. The launch record must then be ABSENT and the tool must
/// answer with the missing-dependency message — which is itself a second,
/// independent reading of the same chain (the message is built inside
/// `ensure_ready`, so the tool can only be quoting it if `ensure_session`
/// called it). Without this arm, arm 1 would also pass against an
/// implementation that wrote the record for any reason at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial(browser_sidecar_wiring_env)]
async fn an_unresolvable_sidecar_program_launches_nothing_and_is_reported_as_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let port = free_port();
    let marker = tmp.path().join("sidecar-launched.json");
    // Written but NOT installed anywhere `which` can find it, and configured
    // under a name that has no directory separator, so `which` must search
    // PATH and fail.
    let _stub = write_stub_sidecar(tmp.path(), port, &marker);
    let absent = "wcore-br-wiring-camofox-browser-that-does-not-exist";
    assert!(
        !resolves_on_path(absent),
        "{absent} resolves on this host; this arm would be vacuous"
    );

    let mut env = EnvGuard::new();
    env.set("WAYLAND_HOME", tmp.path());
    env.set("WAYLAND_CAMOUFOX_URL", format!("http://127.0.0.1:{port}"));
    env.set("WAYLAND_CAMOUFOX_BIN", absent);
    env.set("WAYLAND_BROWSER_ALLOW_UNPROXIED_SIDECAR", "0");

    let tool = reify_from_plugin_registration();
    let result = navigate(&tool).await;

    assert!(
        !marker.exists(),
        "nothing resolvable was configured, yet a sidecar was launched"
    );
    assert!(
        result.is_error,
        "expected an error, got: {}",
        result.content
    );
    assert!(
        result
            .content
            .contains(wcore_browser::install::CAMOUFOX_SIDECAR_PACKAGE),
        "the tool must surface the supervisor's missing-dependency remedy; got: {}",
        result.content
    );

    drop(tool);
}
