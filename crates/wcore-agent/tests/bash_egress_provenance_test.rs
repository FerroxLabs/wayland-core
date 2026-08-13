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
//! * **SEC-13** — meanwhile the operator had no trusted lever at all: nothing
//!   in the config file could give a `Contained` Bash session network.
//!
//!   The first repair welded that lever onto `[security] egress_allow`, and
//!   that was a worse trade than the defect. `egress_allow` is a PER-HOST
//!   permit for the in-process HTTP gate; the `Contained` branch is the DEFAULT
//!   for any repo the operator has not fingerprint-trusted; and no sandbox
//!   backend can filter an arbitrary shell's egress by host, so the shell grant
//!   is all-or-nothing. Measured on that draft: `egress_allow = ["docs.rs"]`
//!   opened `127.0.0.1:44755`, a host the operator never listed. So permitting
//!   one host for one subsystem handed an untrusted cloned repository's shell
//!   arbitrary outbound TCP. The lever is now its own operator-owned switch,
//!   `[security] allow_sandboxed_shell_network`, default false, read from the
//!   trusted (global) config layer alone — the same shape as
//!   `[security] enabled`.
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
use wcore_agent::channel_tools::ChannelToolScope;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_channels::ChannelToolPosture;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_sandbox::{NetworkPolicy, SandboxRegistry};
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

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
fn contained_config(egress_allow: &[&str], allow_sandboxed_shell_network: bool) -> Config {
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
    config.security.allow_sandboxed_shell_network = allow_sandboxed_shell_network;
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
    curl_through_bootstrapped_bash_inner(config, workdir, port, network_override, false).await
}

async fn curl_through_bootstrapped_bash_inner(
    config: Config,
    workdir: &std::path::Path,
    port: u16,
    network_override: Option<NetworkPolicy>,
    channel_attached: bool,
) -> (String, NetworkPolicy) {
    let installed = bootstrapped_policy(config, workdir, channel_attached).await;
    // The override exists ONLY for the fired positive control below, which has
    // to prove this observer is reachable at all from inside the sandbox.
    let policy = match network_override {
        Some(net) => Arc::new((*installed).clone().with_network(net)),
        None => installed,
    };
    curl_under_policy(policy, workdir, port).await
}

/// The workspace policy the REAL bootstrap installs for `config`.
async fn bootstrapped_policy(
    config: Config,
    workdir: &std::path::Path,
    channel_attached: bool,
) -> Arc<WorkspacePolicy> {
    let workspace = workdir.to_str().expect("utf8 workdir").to_string();
    let mut bootstrap =
        AgentBootstrap::new(config, workspace, null_output()).without_channels(true);
    if channel_attached {
        bootstrap = bootstrap.channel_tool_posture(ChannelToolScope {
            posture: ChannelToolPosture::Full,
            workspace_root: workdir.to_path_buf(),
        });
    }
    let result = bootstrap.build().await.expect("bootstrap should succeed");
    result
        .engine
        .tools()
        .workspace_policy()
        .expect("bootstrap installs one workspace policy per session")
}

/// Run one `curl` at the observer through the real BashTool under `policy`.
async fn curl_under_policy(
    policy: Arc<WorkspacePolicy>,
    workdir: &std::path::Path,
    port: u16,
) -> (String, NetworkPolicy) {
    let network = policy.network();
    let command = format!(
        "cd {} && curl -sS --max-time 5 http://127.0.0.1:{}/probe; echo",
        shell_quote(workdir),
        port
    );
    (bash_under_policy(policy, &command).await, network)
}

/// Run one shell command through the real BashTool under `policy`, inside the
/// real sandbox backend. Returns the tool result content.
async fn bash_under_policy(policy: Arc<WorkspacePolicy>, command: &str) -> String {
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

    BashTool
        .execute_with_ctx(
            serde_json::json!({ "command": command, "timeout": 20000 }),
            &ctx,
        )
        .await
        .content
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
        contained_config(&[], false),
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

    let (content, network) = curl_through_bootstrapped_bash(
        contained_config(&[], false),
        workdir.path(),
        obs.port,
        None,
    )
    .await;

    assert_eq!(
        obs.accept_count(),
        0,
        "SEC-11: a bare WAYLAND_BASH_ALLOW_NETWORK=1 in the ENVIRONMENT opened \
         the sandboxed shell's egress. Privilege may only be raised by \
         operator-owned config, never by an inherited env var. \
         policy={network:?} tool_result={content:?}"
    );
}

/// SEC-11 at the CONSTRUCTOR seam, and the test that actually pins the deleted
/// env read.
///
/// The bootstrap seam is now belt-and-braces: it applies an explicit
/// `with_network(..)` that overrides whatever the constructor seeded, so the
/// test above stays green even if `default_bash_network_policy` starts reading
/// the environment again. Three production sites build
/// `WorkspacePolicy::contained()` and apply NO override — `sandbox_context` in
/// `wcore_cli::sandbox_cmd` (`wayland-core sandbox exec`),
/// `wcore_agent::channel_tools::apply_posture`, and `AgentSpawner` — and at
/// those seams `default_bash_network_policy()` is the ONLY decider. This test
/// takes that shape, and it is the one that reddens if the env read returns.
#[tokio::test]
#[serial]
async fn a_bare_env_var_cannot_open_a_directly_constructed_contained_policy() {
    let (_plugins, _env) = hermetic_env(Some("1"));
    let workdir = tempfile::TempDir::new().expect("workdir");
    let obs = Observer::start();

    let policy = Arc::new(WorkspacePolicy::contained(workdir.path()));
    let (content, network) = curl_under_policy(policy, workdir.path(), obs.port).await;

    assert_eq!(
        obs.accept_count(),
        0,
        "SEC-11: `WorkspacePolicy::contained()` seeded its network policy from \
         the environment, so `wayland-core sandbox exec`, the channel posture \
         installer and AgentSpawner all hand out a networked shell to anyone \
         who can set WAYLAND_BASH_ALLOW_NETWORK. \
         policy={network:?} tool_result={content:?}"
    );
}

// ── SEC-13 ───────────────────────────────────────────────────────────────────

/// SEC-13, the OVER-GRANT direction — the reason this half was reworked.
///
/// `egress_allow` is a per-host permit for the in-process HTTP gate. Listing a
/// host there must not give the sandboxed shell network, because the grant the
/// backends can actually enforce is the WHOLE host network: the observer below
/// is `127.0.0.1`, a host the operator never listed.
///
/// The `Contained` branch is the default for any repo the operator has not
/// fingerprint-trusted, so on the first draft of this fix every operator with
/// any `egress_allow` entry — the ordinary configuration for anyone using the
/// HTTP gate at all — gave a cloned repository's shell arbitrary outbound TCP.
/// Measured on that draft: `accept_count=1`, `policy=Inherit`.
#[tokio::test]
#[serial]
async fn an_egress_allow_permit_does_not_open_the_sandboxed_shell() {
    let (_plugins, _env) = hermetic_env(None);
    let workdir = tempfile::TempDir::new().expect("workdir");
    let obs = Observer::start();

    let (content, network) = curl_through_bootstrapped_bash(
        contained_config(&["docs.rs"], false),
        workdir.path(),
        obs.port,
        None,
    )
    .await;

    assert_eq!(
        obs.accept_count(),
        0,
        "SEC-13 OVER-GRANT: `egress_allow = [\"docs.rs\"]` — one host, for the \
         HTTP gate — opened the sandboxed shell's network to 127.0.0.1:{}, a \
         host the operator never listed. A per-host permit for one subsystem \
         must not be a whole-host-network switch for another. \
         policy={network:?} tool_result={content:?}",
        obs.port
    );
}

/// SEC-13, the positive direction. The operator's own switch,
/// `[security] allow_sandboxed_shell_network = true`, must actually reach the
/// sandboxed shell — otherwise the documented escape hatch is decorative and
/// the operator has no trusted lever at all, which was the original defect.
///
/// Same `egress_allow = ["docs.rs"]` as the test above: the ONE variable
/// between the two is the new boolean.
#[tokio::test]
#[serial]
async fn the_operator_switch_opens_the_sandboxed_shell() {
    let (_plugins, _env) = hermetic_env(None);
    let workdir = tempfile::TempDir::new().expect("workdir");
    let obs = Observer::start();

    let (content, network) = curl_through_bootstrapped_bash(
        contained_config(&["docs.rs"], true),
        workdir.path(),
        obs.port,
        None,
    )
    .await;

    assert!(
        obs.accept_count() >= 1,
        "SEC-13: [security] allow_sandboxed_shell_network = true did not reach \
         the sandboxed shell — the operator's opt-in is decorative. \
         policy={network:?} tool_result={content:?}"
    );
}

/// SEC-13, the closed default. Nothing set anywhere must still deny. Pairs
/// with the test above so the fix cannot be "always grant"; pairs with the
/// fired control above so `accept_count == 0` is a real observation.
#[tokio::test]
#[serial]
async fn the_default_configuration_denies_the_sandboxed_shell() {
    let (_plugins, _env) = hermetic_env(None);
    let workdir = tempfile::TempDir::new().expect("workdir");
    let obs = Observer::start();

    let (content, network) = curl_through_bootstrapped_bash(
        contained_config(&[], false),
        workdir.path(),
        obs.port,
        None,
    )
    .await;

    assert_eq!(
        obs.accept_count(),
        0,
        "the default [security] block must leave the sandboxed shell with \
         no egress. policy={network:?} tool_result={content:?}"
    );
}

/// The grant must stay narrow. A channel-attached session is a REMOTE sender
/// even at `Full` posture (#657, Overwatch ruling): the operator's switch
/// widens the operator's own shell, never a remote sender's, or a
/// prompt-injected `curl --data-binary @secret` gets its egress back through
/// the new lever. Reddens if `operator_bash_network` is applied unconditionally
/// in the `strict_workspace` branch.
#[tokio::test]
#[serial]
async fn a_channel_remote_session_never_receives_the_operator_grant() {
    let (_plugins, _env) = hermetic_env(None);
    let workdir = tempfile::TempDir::new().expect("workdir");
    let obs = Observer::start();

    let (content, network) = curl_through_bootstrapped_bash_inner(
        contained_config(&[], true),
        workdir.path(),
        obs.port,
        None,
        true,
    )
    .await;

    assert_eq!(
        obs.accept_count(),
        0,
        "a channel-attached (remote-sender) session must stay on the absolute \
         lockdown even when the operator set allow_sandboxed_shell_network. \
         policy={network:?} tool_result={content:?}"
    );
}

// ── the grant has to be USABLE ───────────────────────────────────────────────

/// A network with no name resolution is not a network. The operator's opt-in
/// must give the sandboxed shell a working resolver, not just a route.
///
/// Found while re-checking the SEC-11/SEC-13 residuals: under the `Contained`
/// profile with `NetworkPolicy::Inherit`, `curl https://example.com` exited 6
/// ("Could not resolve host") inside the sandbox while the identical `curl` on
/// the host returned HTTP 200 — so the documented escape hatch worked for raw
/// IP literals only.
///
/// Root cause is in the bwrap backend, not in the policy: on systemd
/// distributions `/etc/resolv.conf` is a symlink into `/run`
/// (`../run/systemd/resolve/stub-resolv.conf` on Ubuntu 24.04), the sandbox
/// namespace binds `/etc` but not `/run`, so the symlink dangles and every
/// lookup fails with EAI_NONAME. Reproduced at the bwrap layer directly:
/// `cat /etc/resolv.conf` inside → "No such file or directory".
///
/// The assertion grades the resolver file the C library actually reads, from
/// inside the sandbox, against the host's — bytes on disk, not curl's opinion.
#[tokio::test]
#[serial]
async fn the_operator_grant_gives_the_sandboxed_shell_a_resolver() {
    let (_plugins, _env) = hermetic_env(None);
    let workdir = tempfile::TempDir::new().expect("workdir");
    let nameserver = host_nameserver();

    let policy = bootstrapped_policy(contained_config(&[], true), workdir.path(), false).await;
    assert_eq!(
        policy.network(),
        NetworkPolicy::Inherit,
        "precondition: the operator switch must have granted network"
    );

    let inside = bash_under_policy(policy, "cat /etc/resolv.conf; echo CHILD_RAN").await;
    assert!(
        inside.contains(&nameserver),
        "the sandboxed shell was granted network but has no resolver: the \
         host's {nameserver:?} is not visible inside the namespace, so \
         every hostname lookup fails while raw-IP connections work. \
         tool_result={inside:?}"
    );
}

/// The resolver bind is GATED on the grant, and this is the test that keeps
/// the gate honest. A `Deny` namespace has no network, so it must not acquire
/// a readable host file it never had: the default posture stays exactly the
/// posture it was before this fix.
///
/// `echo CHILD_RAN` is the liveness control. Without it this negative
/// assertion passes just as happily when bwrap fails to build the namespace at
/// all and every child exits 1 with empty stdout — the exact shape that let a
/// completely broken sandbox report three green containment tests.
#[tokio::test]
#[serial]
async fn the_default_deny_posture_gains_no_resolver() {
    let (_plugins, _env) = hermetic_env(None);
    let workdir = tempfile::TempDir::new().expect("workdir");
    let nameserver = host_nameserver();

    let policy = bootstrapped_policy(contained_config(&[], false), workdir.path(), false).await;
    assert_eq!(
        policy.network(),
        NetworkPolicy::Deny,
        "precondition: the default posture must be Deny"
    );

    let inside = bash_under_policy(policy, "cat /etc/resolv.conf; echo CHILD_RAN").await;
    assert!(
        inside.contains("CHILD_RAN"),
        "INSTRUMENT BLIND: the child did not run at all, so the negative \
         assertion below would pass on a completely broken sandbox. \
         tool_result={inside:?}"
    );
    assert!(
        !inside.contains(&nameserver),
        "the network-Deny posture must not carry the host resolver — the bind \
         is supposed to be gated on NetworkPolicy::Inherit. \
         tool_result={inside:?}"
    );
}

/// The host's first `nameserver` line. Panics loudly rather than returning an
/// empty needle: on a host with no resolver, "the sandbox cannot resolve
/// names" is not a statement about the sandbox, and `contains("")` is true for
/// every string — so both tests above would be vacuous.
fn host_nameserver() -> String {
    let host = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    host.lines()
        .find(|line| line.trim_start().starts_with("nameserver "))
        .map(|line| line.trim().to_string())
        .unwrap_or_else(|| {
            panic!(
                "INSTRUMENT BLIND: this host's /etc/resolv.conf declares no \
                 nameserver. host_resolv_conf={host:?}"
            )
        })
}
