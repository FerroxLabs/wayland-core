//! SEC-11 / SEC-13 — WHO is allowed to open the sandboxed shell's network.
//!
//! The pairing is the whole point, and the measured polarity was backwards:
//!
//! * **SEC-11** — a bare `WAYLAND_BASH_ALLOW_NETWORK=1` in the ENVIRONMENT
//!   re-opened the sandboxed shell's egress on Linux (driver-owned listener
//!   `accept_count=1`, reproduced 3/3 by the W2/W3 conformance gate). The
//!   environment is UNTRUSTED provenance: it is inherited from whatever
//!   launched the process (a CI job, a parent agent, a `direnv` file that
//!   travels with a cloned repository), which is exactly the supply-chain
//!   hazard `SecurityConfig::enabled` is already documented against —
//!   *"Disabling is config-file only (never a bare env var — supply-chain
//!   hazard, C8)"*. Raising a boundary from the environment is the same class
//!   of mistake as lowering one from it.
//!
//! * **SEC-13** — meanwhile the operator's TRUSTED `[security] egress_allow`
//!   had no effect on Bash at all: `egress_allow = ["127.0.0.1"]` still
//!   recorded `accept_count=0`. Every allowlist any operator has ever written
//!   was decorative for the shell.
//!
//! These tests are graded from a listener the TEST owns — a real
//! `TcpListener` whose completed-`accept()` count no product log line can
//! change — driven through the REAL `AgentBootstrap::build()` seam, so they
//! measure the policy the product actually installs rather than a helper in
//! isolation.
//!
//! Linux-only because bubblewrap is the only backend in this repo that both
//! honours [`NetworkPolicy`] and is available on the CI Linux runner; the
//! measured defect was Linux-specific (macOS already returned "Operation not
//! permitted" for the identical variable).

#![cfg(target_os = "linux")]

use std::io::Write;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serial_test::serial;
use tokio_util::sync::CancellationToken;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_sandbox::{NetworkPolicy, SandboxRegistry};
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;

// ── the driver-owned observer ────────────────────────────────────────────────

/// A real listening socket THIS TEST owns. `accepted` counts completed
/// `accept()`s, so "did a TCP connection happen?" is an external observation
/// and not something the product can report about itself.
struct Observer {
    port: u16,
    accepted: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Observer {
    fn start() -> Self {
        let srv = TcpListener::bind("127.0.0.1:0").expect("bind loopback observer");
        let port = srv.local_addr().expect("observer addr").port();
        srv.set_nonblocking(true).expect("nonblocking observer");
        let accepted = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (a, s) = (Arc::clone(&accepted), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !s.load(Ordering::SeqCst) {
                match srv.accept() {
                    Ok((mut c, _)) => {
                        a.fetch_add(1, Ordering::SeqCst);
                        let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nREACHD");
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(_) => return,
                }
            }
        });
        Self {
            port,
            accepted,
            stop,
        }
    }

    fn accept_count(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

// ── env hermeticity ──────────────────────────────────────────────────────────

/// Save/restore guard for the process-global env vars these tests pin.
/// `Some(v)` sets, `None` removes; prior values are restored on drop, panic
/// included, so a failed `#[serial]` test cannot poison the next one.
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn apply(vars: &[(&'static str, Option<&str>)]) -> Self {
        let saved = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            // SAFETY: every test in this binary is `#[serial]`, so no other
            // thread mutates or reads the environment concurrently, and the
            // guard restores prior state on drop.
            match v {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        Self(saved)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in &self.0 {
            // SAFETY: see `EnvGuard::apply`.
            match prev {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

/// Point plugin discovery at an empty directory (so a plugin installed on the
/// runner cannot influence bootstrap) and set `WAYLAND_BASH_ALLOW_NETWORK` to
/// the value this test wants to measure.
fn hermetic_env(bash_allow_network: Option<&str>) -> (tempfile::TempDir, EnvGuard) {
    let dir = tempfile::TempDir::new().expect("plugins dir");
    let guard = EnvGuard::apply(&[
        ("WAYLAND_BASH_ALLOW_NETWORK", bash_allow_network),
        (
            "WAYLAND_PLUGINS_DIR",
            Some(dir.path().to_str().expect("utf8 plugins dir")),
        ),
    ]);
    (dir, guard)
}

// ── product seam ─────────────────────────────────────────────────────────────

/// Dead base URL: `build()` never connects — these tests only take the
/// workspace policy the bootstrap installed and then drive Bash under it.
/// `workspace_trust` is left at the UNTRUSTED default, which is what selects
/// the strict (`contained`) bootstrap branch — the branch the conformance gate
/// measured and the branch every remote/channel session also lands on.
fn contained_config(egress_allow: &[&str]) -> Config {
    let mut config = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(1),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    config.security.egress_allow = egress_allow.iter().map(|s| (*s).to_string()).collect();
    config
}

fn null_output() -> Arc<dyn OutputSink> {
    Arc::new(NullSink)
}

/// Run one `curl` at the observer through the REAL BashTool, under the
/// workspace policy the real bootstrap installed for `config`. Returns the
/// tool result content (context only — the verdict is the accept count).
async fn curl_through_bootstrapped_bash(
    config: Config,
    workdir: &std::path::Path,
    port: u16,
    network_override: Option<NetworkPolicy>,
) -> (String, NetworkPolicy) {
    let workspace = workdir.to_str().expect("utf8 workdir").to_string();
    let result = AgentBootstrap::new(config, workspace, null_output())
        .without_channels(true)
        .build()
        .await
        .expect("bootstrap should succeed");

    let installed = result
        .engine
        .tools()
        .workspace_policy()
        .expect("bootstrap installs one workspace policy per session");

    // The override exists ONLY for the fired positive control below, which has
    // to prove this observer is reachable at all from inside the sandbox.
    let policy = match network_override {
        Some(net) => Arc::new((*installed).clone().with_network(net)),
        None => installed,
    };
    let network = policy.network();

    let registry =
        Arc::new(SandboxRegistry::required_for_session(None).expect("select a sandbox backend"));
    let ctx = ToolContext::new(
        "sec11-sec13",
        CancellationToken::new(),
        Arc::new(wcore_tools::vfs::RealFs),
        None,
        Arc::new(wcore_tools::NullToolOutputSink),
    )
    .with_workspace(policy)
    .with_sandbox(registry);

    let out = BashTool
        .execute_with_ctx(
            serde_json::json!({
                "command": format!(
                    "cd {} && curl -sS --max-time 5 http://127.0.0.1:{}/probe; echo",
                    shell_quote(workdir), port
                ),
                "timeout": 20000,
            }),
            &ctx,
        )
        .await;
    (out.content, network)
}

fn shell_quote(p: &std::path::Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', r"'\''"))
}

// ── FIRED CONTROL ────────────────────────────────────────────────────────────

/// Without this the two `accept_count == 0` assertions below mean nothing: a
/// listener nobody can ever reach records zero connections whatever the policy
/// is. Same observer, same `curl`, same sandbox backend — the ONE variable is
/// that the workspace policy carries `NetworkPolicy::Inherit`.
#[tokio::test]
#[serial]
async fn control_the_observer_is_reachable_when_the_policy_allows_it() {
    let (_plugins, _env) = hermetic_env(None);
    let workdir = tempfile::TempDir::new().expect("workdir");
    let obs = Observer::start();

    let (content, network) = curl_through_bootstrapped_bash(
        contained_config(&[]),
        workdir.path(),
        obs.port,
        Some(NetworkPolicy::Inherit),
    )
    .await;

    assert_eq!(network, NetworkPolicy::Inherit, "control must run Inherit");
    assert!(
        obs.accept_count() >= 1,
        "INSTRUMENT BLIND: the observer recorded no connection even with the \
         sandbox network policy set to Inherit, so every accept_count==0 \
         assertion in this file is vacuous. port={} tool_result={content:?}",
        obs.port
    );
}

// ── SEC-11 ───────────────────────────────────────────────────────────────────

/// SEC-11. The environment is untrusted provenance and must not be able to
/// RAISE the sandboxed shell's network privilege.
///
/// Before the fix `default_bash_network_policy()` read
/// `WAYLAND_BASH_ALLOW_NETWORK` and returned `NetworkPolicy::Inherit`, which
/// `WorkspacePolicy::contained()` seeded straight into the manifest — so this
/// records `accept_count=1`.
#[tokio::test]
#[serial]
async fn a_bare_env_var_cannot_open_the_sandboxed_shell_network() {
    let (_plugins, _env) = hermetic_env(Some("1"));
    let workdir = tempfile::TempDir::new().expect("workdir");
    let obs = Observer::start();

    let (content, network) =
        curl_through_bootstrapped_bash(contained_config(&[]), workdir.path(), obs.port, None).await;

    assert_eq!(
        obs.accept_count(),
        0,
        "SEC-11: a bare WAYLAND_BASH_ALLOW_NETWORK=1 in the ENVIRONMENT opened \
         the sandboxed shell's egress. Privilege may only be raised by \
         operator-owned config, never by an inherited env var. \
         policy={network:?} tool_result={content:?}"
    );
}

// ── SEC-13 ───────────────────────────────────────────────────────────────────

/// SEC-13, the direction that was inert. The operator's TRUSTED
/// `[security] egress_allow` must actually govern the sandboxed shell.
///
/// Before the fix `WorkspacePolicy::contained()` never saw the config at all,
/// so this records `accept_count=0` — the allowlist was decorative.
#[tokio::test]
#[serial]
async fn operator_egress_allow_governs_the_sandboxed_shell() {
    let (_plugins, _env) = hermetic_env(None);
    let workdir = tempfile::TempDir::new().expect("workdir");
    let obs = Observer::start();

    let (content, network) = curl_through_bootstrapped_bash(
        contained_config(&["127.0.0.1"]),
        workdir.path(),
        obs.port,
        None,
    )
    .await;

    assert!(
        obs.accept_count() >= 1,
        "SEC-13: the operator's [security] egress_allow did not reach the \
         sandboxed shell — the allowlist is decorative. \
         policy={network:?} tool_result={content:?}"
    );
}

/// SEC-13, the closed direction. An EMPTY allowlist must still deny. Pairs
/// with the test above so the fix cannot be "always grant"; pairs with the
/// fired control above so `accept_count == 0` is a real observation.
#[tokio::test]
#[serial]
async fn an_empty_egress_allow_still_denies_the_sandboxed_shell() {
    let (_plugins, _env) = hermetic_env(None);
    let workdir = tempfile::TempDir::new().expect("workdir");
    let obs = Observer::start();

    let (content, network) =
        curl_through_bootstrapped_bash(contained_config(&[]), workdir.path(), obs.port, None).await;

    assert_eq!(
        obs.accept_count(),
        0,
        "an empty [security] egress_allow must leave the sandboxed shell with \
         no egress. policy={network:?} tool_result={content:?}"
    );
}
