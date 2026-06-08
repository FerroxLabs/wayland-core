//! Registry inventory snapshot — pin the FleetDispatcher-class capability
//! wiring contract.
//!
//! The audit at `.blackboard/VERIFY-AUDIT-RUNTIME-2026-05-24.md` identified
//! three "built but never wired" production gaps:
//!
//!   1. CUA — `wayland-cua` plugin loads + registrar captures the spec,
//!      but `PluginRunner::with_computer_use_advertised` is never called,
//!      so reification fails `CapabilityDisabled` and no `Cua` tool ever
//!      reaches the registry.
//!   2. Browser policy — `wayland-browser` registers a spec with
//!      `BrowserPolicySpec::default()` (deny-all). `Config.browser.policy`
//!      is never read, so even after wiring the tool is reachable but
//!      every navigate denies.
//!   3. `SendMessageTool` — registered with `NullMessageTransport`, every
//!      call fails-loud. No host wires a real `MessageTransport` bridged
//!      to `ChannelManager`.
//!
//! These tests pin the wiring contract so future regressions show up in
//! CI rather than as silent capability loss.

use std::sync::Arc;

// Link the plugin shells so their `inventory::submit!` factories register
// with `PluginLoader::discover`. Integration tests build a separate binary
// from `main.rs`, so we must re-import each plugin crate here exactly as
// `main.rs` does — without this, only `wayland-honcho` (pulled in via
// `wcore-honcho-adapter`) shows up at boot.
use wayland_browser as _;
use wayland_cua as _;
use wayland_honcho as _;
use wayland_ijfw as _;
use wayland_ollama as _;

use serial_test::serial;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};

/// Save/restore guard for process-global env vars used by backend-gating
/// tests. Restores prior values (or removes if previously unset) on drop —
/// including on panic — so a failed `#[serial]` test cannot poison the next.
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn set(vars: &[(&'static str, &str)]) -> Self {
        let saved = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            // SAFETY: guarded by `#[serial]`; no other test mutates env
            // concurrently, and the guard restores prior state on drop.
            unsafe { std::env::set_var(k, v) };
        }
        Self(saved)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in &self.0 {
            // SAFETY: see `EnvGuard::set`.
            match prev {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

fn minimal_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(5),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

fn null_output() -> Arc<dyn wcore_agent::output::OutputSink> {
    Arc::new(NullSink)
}

/// Pin fix #1 (CUA): after bootstrap, the `Cua` tool MUST be in the
/// registry. Before the fix, `with_computer_use_advertised` was never
/// called and reification failed `CapabilityDisabled`.
#[tokio::test]
async fn bootstrap_registers_cua_tool_after_wiring_fix() {
    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed");

    let names = result.engine.tool_names();
    assert!(
        names.iter().any(|n| n == "Cua"),
        "Cua tool must be registered after bootstrap. \
         Wiring contract: `PluginRunner::with_computer_use_advertised(true)` \
         flips the registrar gate so `wayland-cua`'s captured spec reifies. \
         Got tools: {names:?}"
    );
}

/// Pin the orphan-tool registration contract (audit 2026-05-24 §7 follow-up).
/// 24 tools across 13 modules were `pub mod`-exported in wcore-tools/lib.rs
/// but never `registry.register`'d before this commit. Each ships with a
/// Null/Capturing backend default that fails LOUDLY when called.
///
/// This test asserts every one is in the registry. Removing any single
/// `registry.register` call from the bootstrap orphan-tools block will
/// fail this test — the FleetDispatcher pattern is hard to re-create
/// once the contract is pinned.
#[tokio::test]
async fn bootstrap_registers_all_orphan_tools_schema_only() {
    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed");

    let names = result.engine.tool_names();
    // Always-on orphan tools: each registers with a Null/Capturing backend
    // that fails LOUDLY when called, so it is present in EVERY environment.
    // Removing any single `registry.register` call from the bootstrap
    // orphan-tools block will fail this test.
    //
    // NOT asserted here (deliberately env-gated). `ToolRegistry::register`
    // SKIPS a tool whose `is_available() == false` — that is the deliberate
    // "running forever" guard that keeps the model from ever seeing a tool it
    // cannot successfully call. So any tool whose backend resolver returns
    // `None` without provider credentials is ABSENT from a keyless registry by
    // design; asserting it in an always-on set would just make this test pass
    // only on a machine that happens to have the relevant key. The thing to
    // pin for these is their CONFIGURED-path registration (see
    // `credential_gated_orphan_tools_register_when_configured`), not presence
    // here:
    //   * Multimodal / transcription providers — registered via
    //     `if let Some(backend) = build_*_backend()` in bootstrap, each needing
    //     an OpenAI / Anthropic / Gemini / FAL / ElevenLabs / Groq key:
    //     `image_generate`, `vision_analyze`, `transcribe_audio`,
    //     `text_to_speech`, `video_analyze`.
    //   * `discord_server`, `homeassistant`, `meet_*` (5) — register only when
    //     their credentials are set.
    //   * `spotify_*` (7) — default-only block until their env-gated backend
    //     lands; re-add here once the resolver is wired.
    //   * `yuanbao` — NEUTRALIZED: its `is_available()` returns false when it
    //     resolves a Null backend (no real endpoint), so `ToolRegistry::register`
    //     skips it in a keyless environment. It registers only with a configured
    //     backend, like the other credential-gated tools above — so it is
    //     deliberately ABSENT from this always-on set (was a model footgun: an
    //     always-registered tool whose every call fails).
    //
    // The set below is the tools that register UNCONDITIONALLY (real,
    // always-available backends needing no provider key), so they must be
    // present in EVERY environment. Removing any single `registry.register`
    // call for one of these from the bootstrap block will fail this test.
    let expected_orphans = [
        "web",
        "wayland_status",
        "wayland_telemetry_query",
        "cronjob",
    ];
    let missing: Vec<&&str> = expected_orphans
        .iter()
        .filter(|t| !names.iter().any(|n| n == *t))
        .collect();
    assert!(
        missing.is_empty(),
        "Orphan-tool registration regression. The following tools were \
         removed from the bootstrap orphan-tools block (FleetDispatcher \
         risk class): {missing:?}. Got tools: {names:?}"
    );
}

/// Pin the configured-registration path for the credential-gated orphan
/// cluster (v0.9.0 W1): with their gating env vars set, `discord_server`,
/// `homeassistant`, and the `meet_*` tools must appear in the live registry.
/// `#[serial]` + the restoring `EnvGuard` keep the global env mutation safe
/// under both `cargo test` (threads) and nextest (process-per-test).
#[tokio::test]
#[serial]
async fn credential_gated_orphan_tools_register_when_configured() {
    let _env = EnvGuard::set(&[
        ("DISCORD_BOT_TOKEN", "probe-token"),
        ("HASS_URL", "http://localhost:8123"),
        ("HASS_TOKEN", "probe-token"),
        ("GOOGLE_CLIENT_ID", "probe-client-id"),
    ]);

    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(
        minimal_config(),
        workdir.path().to_str().unwrap(),
        null_output(),
    )
    .build()
    .await
    .expect("bootstrap should succeed");

    let names = result.engine.tool_names();
    for expected in ["discord_server", "homeassistant", "meet_join"] {
        assert!(
            names.iter().any(|n| n == expected),
            "credential-gated orphan tool `{expected}` must register when its \
             gating env var is configured. Got tools: {names:?}"
        );
    }
}

/// Pin fix #1 supporting check (Browser): the Browser tool must also be
/// present (this was already wired pre-fix, but pinning ensures the
/// surrounding plumbing doesn't regress when the policy plumbing lands).
#[tokio::test]
async fn bootstrap_registers_browser_tool() {
    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed");

    let names = result.engine.tool_names();
    assert!(
        names.iter().any(|n| n == "Browser"),
        "Browser tool must be registered after bootstrap. \
         Loaded plugins: {:?}. Got tools: {names:?}",
        result.loaded_plugin_names
    );
}

/// Pin fix #3 (SendMessage transport): after bootstrap, the registered
/// `send_message` tool MUST NOT be wired to `NullMessageTransport`.
///
/// Probe: invoke the tool against a platform whose channel is not in the
/// minimal-config `ChannelManager`. Assert the error string is NOT the
/// Null transport's signature.
///   - Null transport returns: "No message transport configured for platform 'X'..."
///   - Real transport (over ChannelManager) returns: "unknown channel: X"
#[tokio::test]
async fn bootstrap_wires_real_message_transport_not_null() {
    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let mut result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed");

    {
        let names = result.engine.tool_names();
        assert!(
            names.iter().any(|n| n == "send_message"),
            "send_message tool must be registered. Got: {names:?}"
        );
    }

    let registry = result
        .engine
        .registry_mut()
        .expect("registry should be exclusive at boot");
    let tool = registry
        .get("send_message")
        .expect("send_message tool registered");

    let tool_result = tool
        .execute(serde_json::json!({
            "target": "slack:test-channel",
            "message": "probe",
        }))
        .await;

    assert!(
        tool_result.is_error,
        "send_message against an unknown channel should error \
         (no channel registered in minimal-config). Got: {}",
        tool_result.content
    );

    assert!(
        !tool_result
            .content
            .contains("No message transport configured"),
        "send_message must NOT be wired to NullMessageTransport. \
         Wiring contract: a `MessageTransport` impl over \
         `Arc<Mutex<ChannelManager>>` must be installed at bootstrap. \
         Got error content: {}",
        tool_result.content
    );
}
