//! #657 — the local-Bash network grant, pinned at the REAL bootstrap seam.
//!
//! WHY THIS FILE EXISTS
//!
//! The product behaviour of #657 ("a local CLI Bash session can reach the
//! network — npm/pip/cargo installs, curl, git fetch just work") is delivered
//! by exactly ONE production line, in `AgentBootstrap::build`:
//!
//! ```ignore
//! wcore_tools::workspace_policy::WorkspacePolicy::trusted_local(&workspace)
//!     .with_network(wcore_tools::workspace_policy::local_bash_network(false))
//! ```
//!
//! Everything else about the grant was already covered — but only at the
//! HELPER layer. `crates/wcore-tools/src/workspace_policy/tests.rs` proves
//! `local_bash_network(false) == Inherit` and that `with_network` overrides the
//! fail-safe default; `crates/wcore-tools/src/bash/tests.rs` proves the Deny
//! default. Not one of them touches the bootstrap wiring that CALLS the helper,
//! so all of them stay green with the call site deleted. That is the gap: the
//! helper is tested, the grant is not.
//!
//! These tests drive the real `AgentBootstrap::new(..).build()` path — the same
//! path a local CLI/TUI/json-stream session takes — and read the network
//! posture off the workspace policy the bootstrap actually installed into the
//! engine's tool registry.
//!
//! HOW THIS FAILS IF THE DEFECT RETURNS
//!
//! Delete (or neuter) the `.with_network(wcore_tools::workspace_policy::
//! local_bash_network(false))` line in `crates/wcore-agent/src/bootstrap.rs`
//! (the `else` arm of the `strict_workspace` branch, ~line 2970) and
//! `local_trusted_session_is_bootstrapped_with_network_inherit` reddens: the
//! bare `WorkspacePolicy::trusted_local` constructor is deliberately fail-safe
//! and seeds `network` from `default_bash_network_policy()`, so the assertion
//! sees `NetworkPolicy::Deny` where it demands `NetworkPolicy::Inherit` — which
//! is the exact #657 symptom (local Bash loses egress).
//!
//! The two negative tests are the other half of the pin. A blanket grant —
//! moving `.with_network(..)` above the `if`, or hardcoding `Inherit` inside
//! `WorkspacePolicy::contained` — would satisfy the positive test alone while
//! handing every remote channel sender and every untrusted repository a
//! networked shell. Those two reddens keep the grant narrow.
//!
//! ENV HERMETICITY
//!
//! `default_bash_network_policy()` returns `Inherit` when
//! `WAYLAND_BASH_ALLOW_NETWORK=1`, which would make the Deny expectations (and
//! therefore the positive/negative discrimination) collapse on a host that sets
//! it. Every test here scrubs that variable through an `EnvGuard` and is
//! `#[serial]` because the variable is process-global.

use std::sync::Arc;

use serial_test::serial;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::channel_tools::ChannelToolScope;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_channels::ChannelToolPosture;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_sandbox::NetworkPolicy;
use wcore_tools::workspace_policy::WorkspaceTrust;

/// Save/restore guard for the process-global env vars these tests pin.
/// `Some(v)` sets, `None` removes. Prior values are restored on drop —
/// including on panic — so a failed `#[serial]` test cannot poison the next.
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

/// Scrub `WAYLAND_BASH_ALLOW_NETWORK` (see the ENV HERMETICITY note above) and
/// point plugin discovery at a fresh empty directory so a plugin installed on
/// the host or CI runner cannot influence what bootstrap installs. Returns BOTH
/// the guard and the `TempDir`; the caller must bind both for the duration of
/// the test.
fn hermetic_env() -> (tempfile::TempDir, EnvGuard) {
    let dir = tempfile::TempDir::new().expect("plugins dir");
    let guard = EnvGuard::apply(&[
        ("WAYLAND_BASH_ALLOW_NETWORK", None),
        (
            "WAYLAND_PLUGINS_DIR",
            Some(dir.path().to_str().expect("utf8 plugins dir")),
        ),
    ]);
    (dir, guard)
}

/// Dead base URL: `build()` never connects — these tests only inspect the
/// workspace policy the bootstrap installed.
fn bootstrap_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(1),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

/// A config carrying a real fingerprint-bound local trust grant — the only
/// authority that can make `config.workspace_trust.is_trusted()` true and so
/// the only way to reach the non-strict bootstrap branch. `Config::default()`
/// is deliberately UNTRUSTED.
fn trusted_config() -> Config {
    let mut config = bootstrap_config();
    config.workspace_trust = wcore_types::workspace_trust::resolve_workspace_trust(
        "pin-657-fingerprint",
        [wcore_types::workspace_trust::WorkspaceTrustInput::grant(
            wcore_types::workspace_trust::AuthoritySource::LocalSession,
        )],
    );
    config
}

fn null_output() -> Arc<dyn OutputSink> {
    Arc::new(NullSink)
}

/// POSITIVE DIRECTION. A genuinely-local session — no channel posture,
/// non-Managed execution policy, fingerprint-trusted workspace — must come out
/// of the real bootstrap with `NetworkPolicy::Inherit` on its workspace policy.
///
/// This is the assertion that reddens when the `.with_network(..)` call at
/// `crates/wcore-agent/src/bootstrap.rs` ~2970 is deleted.
#[tokio::test]
#[serial]
async fn local_trusted_session_is_bootstrapped_with_network_inherit() {
    let (_plugins, _env) = hermetic_env();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let workspace = workdir.path().to_str().expect("utf8 workdir").to_string();

    let result = AgentBootstrap::new(trusted_config(), workspace, null_output())
        .without_channels(true)
        .build()
        .await
        .expect("bootstrap should succeed");

    let policy = result
        .engine
        .tools()
        .workspace_policy()
        .expect("bootstrap installs one workspace policy per session");

    // Guard rail: prove we actually took the non-strict branch. Without this a
    // future regression that forces every session Contained would turn the
    // network assertion below into a differently-caused failure with a
    // misleading message.
    assert_eq!(
        policy.trust(),
        WorkspaceTrust::Trusted,
        "a fingerprint-trusted, non-channel, non-Managed session must take the \
         trusted_local branch"
    );

    assert_eq!(
        policy.network(),
        NetworkPolicy::Inherit,
        "#657: a genuinely-local Bash session must be bootstrapped with network \
         egress. Deny here means the `.with_network(local_bash_network(false))` \
         grant in AgentBootstrap::build was lost"
    );
}

/// NEGATIVE DIRECTION (remote sender). A channel-attached session is a remote
/// sender even at `Full` posture: it takes the `contained` branch and must NOT
/// receive the local network grant. Reddens if the grant is hoisted above the
/// `strict_workspace` branch or baked into `WorkspacePolicy::contained`.
#[tokio::test]
#[serial]
async fn channel_attached_session_is_denied_the_local_network_grant() {
    let (_plugins, _env) = hermetic_env();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let workspace = workdir.path().to_str().expect("utf8 workdir").to_string();

    // Trusted config on purpose: the ONLY thing that must deny the grant here
    // is the attached channel posture.
    let result = AgentBootstrap::new(trusted_config(), workspace, null_output())
        .without_channels(true)
        .channel_tool_posture(ChannelToolScope {
            posture: ChannelToolPosture::Full,
            workspace_root: workdir.path().to_path_buf(),
        })
        .build()
        .await
        .expect("bootstrap should succeed");

    let policy = result
        .engine
        .tools()
        .workspace_policy()
        .expect("bootstrap installs one workspace policy per session");

    assert_eq!(
        policy.trust(),
        WorkspaceTrust::Contained,
        "any channel-attached session must take the contained branch"
    );
    assert_eq!(
        policy.network(),
        NetworkPolicy::Deny,
        "#657: a channel-attached (remote-sender) session must stay on the \
         pre-#657 lockdown even at Full posture — the local grant is not blanket"
    );
}

/// NEGATIVE DIRECTION (untrusted repository). No channel posture, but no
/// fingerprint trust grant either: repository content can never select the
/// networked branch. Reddens if the grant stops being conditioned on
/// `config.workspace_trust.is_trusted()`.
#[tokio::test]
#[serial]
async fn untrusted_workspace_is_denied_the_local_network_grant() {
    let (_plugins, _env) = hermetic_env();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let workspace = workdir.path().to_str().expect("utf8 workdir").to_string();

    // `bootstrap_config()` leaves `workspace_trust` at the untrusted default.
    let result = AgentBootstrap::new(bootstrap_config(), workspace, null_output())
        .without_channels(true)
        .build()
        .await
        .expect("bootstrap should succeed");

    let policy = result
        .engine
        .tools()
        .workspace_policy()
        .expect("bootstrap installs one workspace policy per session");

    assert_eq!(
        policy.trust(),
        WorkspaceTrust::Contained,
        "a workspace without a current fingerprint grant must stay strict"
    );
    assert_eq!(
        policy.network(),
        NetworkPolicy::Deny,
        "#657: an untrusted workspace must not receive the local network grant"
    );
}
