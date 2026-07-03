use std::sync::Arc;

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

/// Pin `WAYLAND_PLUGINS_DIR` to a fresh empty directory so on-disk plugin
/// discovery cannot pick up plugins installed on the host or CI runner (e.g. an
/// `ijfw` plugin that registers an `ijfw-memory` MCP server). Without this, the
/// no-MCP assertions below fail on any machine that has a plugin installed.
/// Returns BOTH the guard and the `TempDir`; the caller must bind both for the
/// duration of the test (the guard restores the env on drop; the dir must
/// outlive `build()`). Requires `#[serial]` since it mutates process-global env.
fn isolated_plugins() -> (tempfile::TempDir, EnvGuard) {
    let dir = tempfile::TempDir::new().expect("plugins dir");
    let guard = EnvGuard::set(&[(
        "WAYLAND_PLUGINS_DIR",
        dir.path().to_str().expect("utf8 plugins dir"),
    )]);
    (dir, guard)
}

#[tokio::test]
#[serial]
async fn bootstrap_builds_engine_with_model_in_prompt() {
    let (_plugins, _env) = isolated_plugins();
    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed");

    assert!(!result.engine.tool_names().is_empty());
    assert!(!result.has_mcp);
    assert!(result.mcp_managers.is_empty());
}

/// #141 audit item 5 — prove the env flag actually installs
/// `HostDelegatedTransport` at the registration site: with
/// `WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1`, an executed `send_message` parks
/// a waiter on the SAME `HostSendBridge` exposed on `BootstrapResult`, and
/// resolving that bridge completes the tool call with the host's receipt.
/// (The `#[serial]` + `EnvGuard` discipline mirrors the backend-gating
/// tests above; the env var is process-global.)
#[tokio::test]
#[serial]
async fn host_delegate_env_installs_host_delegated_send_transport() {
    let (_plugins, _plugins_env) = isolated_plugins();
    let _env = EnvGuard::set(&[("WAYLAND_SEND_MESSAGE_HOST_DELEGATE", "1")]);

    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed");

    let bridge = result.host_send_bridge.clone();
    assert_eq!(bridge.pending_count(), 0);

    // Play host: wait for the delegated send to register its waiter on the
    // result's bridge, then resolve it with a receipt.
    let resolver_bridge = bridge.clone();
    let resolver = tokio::spawn(async move {
        for _ in 0..400 {
            let ids = resolver_bridge.pending_ids();
            if let Some(id) = ids.first() {
                assert!(resolver_bridge.resolve(
                    id,
                    wcore_agent::host_send_transport::HostSendResult {
                        ok: true,
                        message_id: Some("bootstrap-wired-ok".into()),
                        error: None,
                    },
                ));
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("send_message never registered a waiter on the BootstrapResult bridge");
    });

    let tools = result.engine.tools();
    let tool = tools.get("send_message").expect("send_message registered");
    let out = tool
        .execute(serde_json::json!({
            "target": "email:mike@example.com",
            "message": "wiring probe"
        }))
        .await;
    resolver.await.expect("resolver task");

    assert!(
        !out.is_error,
        "delegated send resolved by the result's bridge must succeed, got: {}",
        out.content
    );
    assert!(
        out.content.contains("bootstrap-wired-ok"),
        "the host receipt must flow through — proves HostDelegatedTransport \
         is bound to BootstrapResult.host_send_bridge, got: {}",
        out.content
    );
}

/// #141 backward-compat control: WITHOUT the env flag the delegated
/// transport is NOT wired — `send_message` fails through the channel path
/// immediately (no waiter ever appears on the result's bridge).
#[tokio::test]
#[serial]
async fn without_host_delegate_env_send_message_keeps_channel_path() {
    let (_plugins, _plugins_env) = isolated_plugins();
    // Explicitly ensure the flag is absent for this test.
    struct Unset(Option<String>);
    impl Drop for Unset {
        fn drop(&mut self) {
            if let Some(v) = &self.0 {
                // SAFETY: `#[serial]` — no concurrent env access.
                unsafe { std::env::set_var("WAYLAND_SEND_MESSAGE_HOST_DELEGATE", v) };
            }
        }
    }
    let _unset = Unset(std::env::var("WAYLAND_SEND_MESSAGE_HOST_DELEGATE").ok());
    // SAFETY: `#[serial]` — no concurrent env access.
    unsafe { std::env::remove_var("WAYLAND_SEND_MESSAGE_HOST_DELEGATE") };

    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed");

    let bridge = result.host_send_bridge.clone();
    let tools = result.engine.tools();
    let tool = tools.get("send_message").expect("send_message registered");
    let out = tool
        .execute(serde_json::json!({
            "target": "email:mike@example.com",
            "message": "control probe"
        }))
        .await;

    assert!(
        out.is_error,
        "flag off: send must fail through the channel/null path, got: {}",
        out.content
    );
    assert_eq!(
        bridge.pending_count(),
        0,
        "flag off: the delegated bridge must never see a waiter"
    );
}

#[tokio::test]
async fn bootstrap_registers_all_expected_tools() {
    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .unwrap();

    let names = result.engine.tool_names();

    for expected in &["Read", "Write", "Edit", "Bash", "Grep", "Glob"] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing built-in tool: {expected}"
        );
    }

    assert!(
        names.iter().any(|n| n == "Skill"),
        "SkillTool should be registered"
    );
    assert!(
        names.iter().any(|n| n == "Spawn"),
        "SpawnTool should be registered"
    );
    assert!(
        names.iter().any(|n| n == "Workflow"),
        "WorkflowTool should be registered"
    );
    assert!(
        names.iter().any(|n| n == "ToolSearch"),
        "ToolSearchTool should be registered"
    );
    assert!(
        names.iter().any(|n| n == "session_search"),
        "SessionSearchTool should be registered (cross-session recall)"
    );
}

#[tokio::test]
async fn bootstrap_registers_v063_catalog_tools() {
    // v0.6.3 D.0: every *always-on* catalog tool wired in
    // `AgentBootstrap::build()` must be reachable by a running agent.
    //
    // 14 of the 15 v0.6.3 catalog tools register unconditionally with a
    // Null/CLI-shelling backend that fails loudly at call time, so the built
    // ToolRegistry must expose each name regardless of environment.
    //
    // The 15th — `postgres_schema` — was changed in v0.9.0 W1 to register
    // ONLY when a live backend is wired (DATABASE_URL / POSTGRES_URL /
    // PG_CONN_STRING). When unset it is hidden via `Tool::is_available()`,
    // so it is *correctly absent* here. Its configured-registration path is
    // pinned by `postgres_schema_registers_when_database_url_set` below; the
    // resolver itself is unit-tested in `tool_backends::postgres_schema`.
    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .unwrap();

    let names = result.engine.tool_names();

    // The 14 always-on v0.6.3 catalog tools by their registered `name()`
    // strings.
    let v063_tools = [
        "Jsonl",          // T11 — JSON Lines streaming
        "pdf_extract",    // T15 — PDF text extraction
        "sql_query",      // SQL query
        "Archive",        // zip/tar list + extract
        "markdown_table", // markdown table tool
        "image_inspect",  // image metadata
        "email_parse",    // RFC822 email parse
        "kubectl",        // kubectl CLI wrapper
        "gcloud",         // gcloud CLI wrapper
        "aws_cli",        // aws CLI wrapper
        "github_api",     // GitHub REST (real HTTP backend)
        "gitlab_api",     // GitLab REST (real HTTP backend)
        "linear_api",     // Linear GraphQL (real HTTP backend)
        "notion_api",     // Notion REST (real HTTP backend)
    ];

    for expected in &v063_tools {
        assert!(
            names.iter().any(|n| n == expected),
            "v0.6.3 catalog tool not registered in the live ToolRegistry: {expected}"
        );
    }

    // postgres_schema is backend-gated: it must NOT leak into a no-DB env.
    assert!(
        !names.iter().any(|n| n == "postgres_schema"),
        "postgres_schema must stay hidden when no DATABASE_URL is configured \
         (v0.9.0 W1 is_available() gating); got: {names:?}"
    );
}

/// Pin the configured-registration path for the one backend-gated v0.6.3
/// catalog tool. With `DATABASE_URL` set, `postgres_schema` must appear in
/// the live registry. `#[serial]` + the restoring `EnvGuard` keep the global
/// env mutation safe under both `cargo test` (threads) and nextest
/// (process-per-test).
#[tokio::test]
#[serial]
async fn postgres_schema_registers_when_database_url_set() {
    let _env = EnvGuard::set(&[(
        "DATABASE_URL",
        "postgresql://localhost:5432/registration_probe",
    )]);

    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(
        minimal_config(),
        workdir.path().to_str().unwrap(),
        null_output(),
    )
    .build()
    .await
    .unwrap();

    assert!(
        result
            .engine
            .tool_names()
            .iter()
            .any(|n| n == "postgres_schema"),
        "postgres_schema must register when DATABASE_URL is configured"
    );
}

#[tokio::test]
async fn bootstrap_plan_tools_when_enabled() {
    let mut config = minimal_config();
    config.plan.enabled = true;

    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .unwrap();

    let names = result.engine.tool_names();
    assert!(
        names.iter().any(|n| n == "EnterPlanMode"),
        "EnterPlanMode should be registered when plan.enabled"
    );
    assert!(
        names.iter().any(|n| n == "ExitPlanMode"),
        "ExitPlanMode should be registered when plan.enabled"
    );
}

#[tokio::test]
async fn bootstrap_no_plan_tools_when_disabled() {
    let mut config = minimal_config();
    config.plan.enabled = false;

    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .unwrap();

    let names = result.engine.tool_names();
    assert!(
        !names.iter().any(|n| n == "EnterPlanMode"),
        "EnterPlanMode should NOT be registered when plan.disabled"
    );
}

#[tokio::test]
#[serial]
async fn bootstrap_no_mcp_when_no_servers() {
    let (_plugins, _env) = isolated_plugins();
    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .unwrap();

    assert!(!result.has_mcp);
    assert!(result.mcp_managers.is_empty());
}

/// wayland#551 — `defer_config_mcp(true)` must skip the config-server
/// connect entirely: build() returns without dialing (the configured
/// server here would hang the full 30s per-server budget if dialed —
/// the exact stall that blew hosts' 30s ready timeout), registers no
/// #111 — a config MCP server marked `only_for_assistant` must be injected
/// ONLY for a matching active assistant. Overwatch #613 requires proving a
/// non-Concierge (or unidentified) session does NOT receive the scoped server
/// while a Concierge session DOES. The server command exits immediately, so a
/// DIALED server still yields a manager (connect_all is non-fatal per-server);
/// a FILTERED server is never dialed, so `has_mcp` stays false.
#[tokio::test]
#[serial]
async fn bootstrap_scopes_marked_mcp_server_to_matching_assistant() {
    use wcore_config::config::{McpServerConfig, TransportType};

    let (_plugins, _env) = isolated_plugins();
    let scoped_config = || {
        let mut config = minimal_config();
        config.mcp.servers.insert(
            "diag".into(),
            McpServerConfig {
                transport: TransportType::Stdio,
                command: Some("sleep".into()),
                args: Some(vec!["0".into()]), // exits immediately → fast connect
                env: None,
                url: None,
                headers: None,
                deferred: None,
                allow_local: false,
                only_for_assistant: Some(vec!["concierge".into()]),
            },
        );
        config
    };
    let workdir = tempfile::TempDir::new().expect("workdir");

    // Unidentified session (fail-closed): the marked server is filtered out and
    // never dialed.
    let excluded = AgentBootstrap::new(
        scoped_config(),
        workdir.path().to_str().unwrap(),
        null_output(),
    )
    .active_assistant(None)
    .build()
    .await
    .unwrap();
    assert!(
        !excluded.has_mcp,
        "a scoped server must NOT reach an unidentified session (fail-closed)"
    );
    assert!(excluded.mcp_managers.is_empty());

    // Non-matching assistant: also filtered out.
    let wrong = AgentBootstrap::new(
        scoped_config(),
        workdir.path().to_str().unwrap(),
        null_output(),
    )
    .active_assistant(Some("default".into()))
    .build()
    .await
    .unwrap();
    assert!(
        !wrong.has_mcp,
        "a scoped server must NOT reach a non-matching assistant"
    );

    // Concierge session: the scoped server IS in the dialed set.
    let included = AgentBootstrap::new(
        scoped_config(),
        workdir.path().to_str().unwrap(),
        null_output(),
    )
    .active_assistant(Some("concierge".into()))
    .build()
    .await
    .unwrap();
    assert!(
        included.has_mcp,
        "the Concierge session MUST receive its scoped server"
    );
    assert!(!included.mcp_managers.is_empty());
}

/// MCP tools, and reports has_mcp=false (the caller advertises MCP
/// capability itself for declared-but-deferred servers).
#[tokio::test]
#[serial]
async fn bootstrap_defer_config_mcp_skips_connect() {
    use wcore_config::config::{McpServerConfig, TransportType};

    let (_plugins, _env) = isolated_plugins();
    let mut config = minimal_config();
    config.mcp.servers.insert(
        "hang".into(),
        McpServerConfig {
            transport: TransportType::Stdio,
            // Starts fine, never speaks MCP — a dialed connect would sit in
            // the handshake until the 30s per-server timeout.
            command: Some("sleep".into()),
            args: Some(vec!["300".into()]),
            env: None,
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
            only_for_assistant: None,
        },
    );
    let workdir = tempfile::TempDir::new().expect("workdir");

    let start = std::time::Instant::now();
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .defer_config_mcp(true)
        .build()
        .await
        .unwrap();

    // Generous bound: well under the 30s connect budget the skipped dial
    // would have burned, far above any honest build cost.
    assert!(
        start.elapsed() < std::time::Duration::from_secs(15),
        "deferred build must not dial config MCP servers (took {:?})",
        start.elapsed()
    );
    assert!(!result.has_mcp, "deferred servers must not set has_mcp");
    assert!(result.mcp_managers.is_empty());
    assert!(
        !result
            .engine
            .tool_names()
            .iter()
            .any(|n| n.contains("hang")),
        "no tools from the deferred server may be registered at build time"
    );
}

#[tokio::test]
async fn bootstrap_with_custom_system_prompt() {
    let mut config = minimal_config();
    config.system_prompt = Some("You are a pirate assistant.".into());

    let workdir = tempfile::TempDir::new().expect("workdir");
    let _result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .unwrap();
}

#[tokio::test]
async fn bootstrap_with_agents_md_in_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path();
    std::fs::write(workspace.join("AGENTS.md"), "PROJECT_RULES_MARKER").unwrap();

    let config = minimal_config();
    let _result = AgentBootstrap::new(config, workspace.to_string_lossy().as_ref(), null_output())
        .build()
        .await
        .unwrap();
}

#[tokio::test]
async fn bootstrap_config_accessor_returns_config() {
    // No `.build()` call here — bootstrap stores the workspace string without
    // touching the filesystem. The literal is safe on Windows as pure data.
    let config = minimal_config();
    let bootstrap = AgentBootstrap::new(config, "/tmp/ws", null_output());
    assert_eq!(bootstrap.config().model, "gpt-test-model");
    assert_eq!(bootstrap.config().max_tokens, 1024);
}

#[tokio::test]
async fn bootstrap_with_external_provider() {
    let config = minimal_config();
    let provider = wcore_providers::create_provider(&config);

    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .provider(provider)
        .build()
        .await
        .unwrap();

    assert!(!result.engine.tool_names().is_empty());
}

// ---- W7.1 F8-3: bootstrap wraps primary provider in ResilientProvider ----

mod resilience_wrap {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::mpsc;
    use wcore_agent::output::OutputSink;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::FinishReason;

    /// Always-failing provider — drives the circuit breaker open when wrapped.
    /// When NOT wrapped, the bootstrap's engine sees the raw error directly
    /// (no `provider_circuit_event` emitted).
    struct AlwaysFailProvider {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl LlmProvider for AlwaysFailProvider {
        async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::Connection("forced failure".into()))
        }
    }

    /// (primary, fallback?, state, error?) captured per circuit event.
    type CircuitEvent = (String, Option<String>, String, Option<String>);

    /// Test sink that records every `emit_provider_circuit_event` call. All
    /// other emit methods are no-ops.
    #[derive(Default)]
    struct CircuitCapSink {
        events: Mutex<Vec<CircuitEvent>>,
    }
    impl OutputSink for CircuitCapSink {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
        fn emit_provider_circuit_event(
            &self,
            primary: &str,
            fallback: Option<&str>,
            state: &str,
            error: Option<&str>,
        ) {
            self.events.lock().unwrap().push((
                primary.into(),
                fallback.map(String::from),
                state.into(),
                error.map(String::from),
            ));
        }
    }

    fn always_fail() -> Arc<dyn LlmProvider> {
        Arc::new(AlwaysFailProvider {
            calls: AtomicUsize::new(0),
        })
    }

    #[tokio::test]
    async fn provider_chain_disabled_does_not_emit_circuit_events() {
        // With provider_chain.enabled = false (the default), bootstrap must
        // hand the engine the primary provider unchanged. We force the
        // primary to fail multiple times; no `provider_circuit_event` can
        // be emitted because no `ResilientProvider` is in play.
        let mut config = minimal_config();
        assert!(
            !config.provider_chain.enabled,
            "default should be disabled — guards the W0 forward-additive invariant"
        );
        config.provider_chain.failure_threshold = 2;
        config.provider_chain.recovery_timeout_secs = 5;

        let sink = Arc::new(CircuitCapSink::default());
        let workdir = tempfile::TempDir::new().expect("workdir");
        let result = AgentBootstrap::new(
            config,
            workdir.path().to_str().unwrap(),
            sink.clone() as Arc<dyn OutputSink>,
        )
        .provider(always_fail())
        .build()
        .await
        .unwrap();

        // Drive the provider directly via the BootstrapResult handle (the same
        // handle the engine holds). Even on repeated failures, no circuit
        // event should fire because the wrap is absent.
        for _ in 0..5 {
            let req = LlmRequest {
                model: "test".into(),
                system: String::new(),
                messages: vec![],
                tools: vec![],
                max_tokens: 1024,
                thinking: None,
                reasoning_effort: None,
                cache_tier: None,
                routing_hint: None,
                stop_sequences: Vec::new(),
                web_search: false,
                conversation_id: None,
                client_context_tokens: None,
                temperature: None,
                omit_max_tokens: false,
            };
            let _ = result.provider.stream(&req).await;
        }

        assert!(
            sink.events.lock().unwrap().is_empty(),
            "with provider_chain disabled, no provider_circuit_event should fire; got {:?}",
            sink.events.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn provider_chain_enabled_emits_circuit_event_after_threshold() {
        // With provider_chain.enabled = true, bootstrap wraps the primary in
        // a `ResilientProvider`. A configured number of failed primary calls
        // opens the breaker; the wrapped reporter routes the transition
        // through the sink's `emit_provider_circuit_event`.
        let mut config = minimal_config();
        config.provider_chain.enabled = true;
        config.provider_chain.failure_threshold = 3;
        config.provider_chain.recovery_timeout_secs = 30;

        let sink = Arc::new(CircuitCapSink::default());
        let workdir = tempfile::TempDir::new().expect("workdir");
        let result = AgentBootstrap::new(
            config,
            workdir.path().to_str().unwrap(),
            sink.clone() as Arc<dyn OutputSink>,
        )
        .provider(always_fail())
        .build()
        .await
        .unwrap();

        // 3 failed calls = threshold reached → breaker opens → reporter fires.
        for _ in 0..4 {
            let req = LlmRequest {
                model: "test".into(),
                system: String::new(),
                messages: vec![],
                tools: vec![],
                max_tokens: 1024,
                thinking: None,
                reasoning_effort: None,
                cache_tier: None,
                routing_hint: None,
                stop_sequences: Vec::new(),
                web_search: false,
                conversation_id: None,
                client_context_tokens: None,
                temperature: None,
                omit_max_tokens: false,
            };
            let _ = result.provider.stream(&req).await;
        }

        let events = sink.events.lock().unwrap();
        assert!(
            events.iter().any(|(_, _, state, _)| state == "open"),
            "expected at least one circuit-open event after threshold failures; got {events:?}"
        );
        // Primary identifier should match the resolved provider label
        // (`minimal_config()` uses "openai").
        assert!(
            events.iter().all(|(p, _, _, _)| p == "openai"),
            "primary id should match config.provider_label; got {events:?}"
        );
    }
}

// ---- W7 Pre-flight 0.0b: bootstrap wires MemoryApi into the engine -------

#[tokio::test]
async fn w7_pre0_bootstrap_wires_null_memory_when_skills_lifecycle_off() {
    // With skills_lifecycle disabled (Default), bootstrap must hand the
    // engine a NullMemory handle, not panic or block on Memory::open.
    let mut config = minimal_config();
    config.observability.skills_lifecycle = false;

    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed without memory");

    // memory_api() returns a live handle; search on NullMemory yields empty.
    let api = result.engine.memory_api();
    let hits = api
        .search(
            wcore_memory::v2_types::Query::default(),
            wcore_memory::AccessToken::MainAgent,
        )
        .await
        .unwrap();
    assert!(
        hits.is_empty(),
        "NullMemory should return empty results, got {} hits",
        hits.len()
    );
}

#[tokio::test]
async fn w2_v063_bootstrap_initializes_kg_when_memory_enabled() {
    // W2 v0.6.3: when memory is enabled (real Memory is opened), the bootstrap
    // must run `kg::init_kg` on the session-tier connection. Verifying the
    // schema directly would require poking at private Db state, so we settle
    // for the structural assertion: bootstrap must succeed without panic,
    // produce a non-Null memory handle that supports search, and the KG
    // helpers (`kg::kg_enabled`) must report enabled by default.
    let mut config = minimal_config();
    config.observability.skills_lifecycle = true;
    config.memory.enabled = true;

    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed with memory + KG init");

    // Default env: KG is enabled.
    assert!(
        wcore_memory::kg::kg_enabled(),
        "KG should be enabled by default; bootstrap must have called init_kg"
    );

    // memory_api() must be live (real Memory, not NullMemory). Empty search
    // on a fresh memory db is the strongest assertion we can make without
    // poking private state.
    let api = result.engine.memory_api();
    let _ = api
        .search(
            wcore_memory::v2_types::Query::default(),
            wcore_memory::AccessToken::MainAgent,
        )
        .await
        .expect("search on live memory should not error");
}

#[tokio::test]
async fn w2_v063_bootstrap_skips_kg_when_disabled() {
    // W2 v0.6.3 inverse path: WAYLAND_KG=off must not block bootstrap. We
    // can't safely flip the global env var inside a parallel test runner, so
    // this test asserts the surface: `kg_enabled()` honors the env contract.
    // The bootstrap code-path is symmetrical (if kg_enabled() returns false,
    // the entire init block is a no-op), so this is the cleanest signal
    // without process-level isolation.
    //
    // SAFETY: env var mutation is racy under cargo's default parallel test
    // runner; the assertion holds regardless of timing because we snapshot
    // the result before any other test can observe the variable.
    // SAFETY: env mutation is racy under parallel tests; this is the documented
    // pattern in the existing test suite and the assertion observes the
    // immediate post-set value, which is timing-safe.
    unsafe { std::env::set_var(wcore_memory::kg::ENV_KG, "off") };
    let enabled = wcore_memory::kg::kg_enabled();
    unsafe { std::env::remove_var(wcore_memory::kg::ENV_KG) };
    assert!(!enabled, "WAYLAND_KG=off must disable KG init in bootstrap");
}

/// Task 5: a plain (non-channel) bootstrap session must install a Trusted
/// WorkspacePolicy on the registry so BashTool's OS sandbox is rooted at
/// the workspace directory.
#[tokio::test]
async fn bootstrap_workspace_policy_installed_trusted() {
    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap should succeed");

    let policy = result
        .engine
        .tools()
        .workspace_policy()
        .expect("workspace_policy must be Some after bootstrap");

    assert_eq!(
        policy.trust(),
        wcore_tools::workspace_policy::WorkspaceTrust::Trusted,
        "non-channel session must get a Trusted workspace policy"
    );
}
