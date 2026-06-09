use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use wcore_config::config::Config;
use wcore_mcp::manager::McpManager;
use wcore_observability::sink::SpanSink;
use wcore_plugin_api::registry::providers::PluginProvider;
use wcore_providers::{CircuitConfig, LlmProvider, ResilientProvider};
// E-H2: `CircuitReporter` / `NoOpCircuitReporter` are referenced by
// fully-qualified path in the resilience wiring below.

use crate::budget::{ExecutionBudget, ExecutionBudgetView};
use crate::cancel::{BudgetGuard, CancellationToken, budget_linked_with_callback};
use crate::engine::AgentEngine;
use crate::output::OutputSink;
use crate::session::Session;

/// Result of bootstrapping an agent engine with all features initialized.
pub struct BootstrapResult {
    pub engine: AgentEngine,
    pub provider: Arc<dyn LlmProvider>,
    pub mcp_managers: Vec<Arc<McpManager>>,
    pub has_mcp: bool,
    /// True iff at least one plugin successfully registered during boot
    /// (design spec §5.17 — flips `Capabilities.plugins` in the Ready event).
    pub has_plugins: bool,
    /// W8c.3 H.2: plugin-derived capability set carried into the
    /// `Capabilities` advertisement. Built from the live plugin loader
    /// so flags like `browser_suite` and `computer_use` flip when
    /// `wayland-browser` / `wayland-cua` actually loaded. Pre-W8c.3
    /// consumers can ignore this field (`has_plugins` is unchanged).
    pub plugin_capabilities: crate::output::protocol_sink::PluginCapabilitySet,
    /// W8c.3 H.2: the canonical list of loaded plugin names, as
    /// emitted by the loader. Stable order = inventory order. Empty
    /// when no plugins discovered.
    pub loaded_plugin_names: Vec<String>,
    /// W8a A.6: session-root execution budget view. Constructed from
    /// `Config.budget` via `ExecutionBudget::from(&BudgetConfig)`. All
    /// per-tool `ToolContext.budget` views are sub-budgets of this root.
    pub budget: ExecutionBudgetView,
    /// W8a A.6: session-root cancellation handle. Linked to `budget`
    /// via `budget_linked` so any cap trip fires the wrapped token
    /// (which propagates to in-flight tools through
    /// `ToolContext.cancel`). Cancel the session by calling
    /// `cancel_root.cancel()`; orchestration races every tool dispatch
    /// against this token (A.4 dispatcher route through
    /// `execute_with_ctx`).
    ///
    /// Wave RC (audit MAJOR #8) — this is now a [`BudgetGuard`] RAII
    /// handle rather than a bare `CancellationToken`. Dropping the
    /// `BootstrapResult` aborts the per-session budget-watcher tokio
    /// task. The guard derefs to the underlying token so existing
    /// `.is_cancelled()` / `.cancel()` / `.cancelled()` calls keep
    /// working without churn.
    pub cancel_root: BudgetGuard,
    /// v0.8.1 U5 — channel runtime. `ChannelManager` is constructed
    /// at boot and seeded by
    /// `wcore_channels_registry::auto_register_from_user_config`,
    /// which scans `~/.wayland/channels/*.toml` and registers every
    /// adapter whose `platform` field maps to a known factory.
    ///
    /// F-014 + F-050 (CRIT + MED): lifted to `Arc<tokio::sync::Mutex<ChannelManager>>`
    /// so the cron `channel_sink` and any CLI inbound-subscription path can
    /// hold a clone. `start_all` is called inside `AgentBootstrap::build`
    /// before this result is returned, so polling is already active by the
    /// time the caller receives the handle. Uses tokio Mutex because the
    /// cron handler's `send_to` path is async (the guard must be held across
    /// an await point in `ChannelManager::send_to`).
    pub channel_manager: std::sync::Arc<tokio::sync::Mutex<wcore_channels::ChannelManager>>,
    /// v0.8.1 U5 — count of channels successfully auto-registered
    /// during boot.
    pub channels_auto_registered: usize,
    /// v0.8.1 U7 — background cron runner. `Drop` on the `CronRunner`
    /// signals the runner's shutdown watch channel and aborts the
    /// background tokio task. `Option` so the boot path can leave it
    /// `None` when the store path can't be resolved.
    pub cron_runner: Option<wcore_cron::CronRunner>,
}

/// Wave OL: plugin-provider router. Called after plugin discovery + init
/// with the live list of registered `Arc<dyn PluginProvider>` handles. The
/// router inspects `model_str` (typically `Config.model`) and may downcast
/// a plugin provider into a concrete `wcore_providers::LlmProvider`, e.g.
/// the `wayland-ollama` route when `model_str.starts_with("ollama:")`.
///
/// Returning `None` falls through to the built-in `create_provider(&config)`
/// path. The downcast itself lives in the binary crate (`wcore-cli`) — it's
/// the only crate that links both `wcore-providers` and `wayland-ollama`.
pub type PluginProviderRouter =
    Box<dyn Fn(&str, &[Arc<dyn PluginProvider>]) -> Option<Arc<dyn LlmProvider>> + Send + Sync>;

/// Builder for creating a fully-initialized `AgentEngine`.
///
/// Encapsulates the complete initialization pipeline so all consumers
/// (CLI, backend, sub-agents) get consistent behavior:
///
/// - System prompt always includes model identity, working directory, date
/// - Tool usage guidance is always injected
/// - AGENTS.md is loaded from the workspace hierarchy
/// - Skills, MCP, plan mode, spawn are enabled based on `Config` fields
pub struct AgentBootstrap {
    config: Config,
    workspace: String,
    output: Arc<dyn OutputSink>,
    provider: Option<Arc<dyn LlmProvider>>,
    resume_session: Option<Session>,
    extra_skill_dirs: Vec<PathBuf>,
    /// Wave OL: optional resolver invoked after plugin init to route
    /// model strings like `ollama:llama3` through a plugin-supplied
    /// provider.
    plugin_provider_router: Option<PluginProviderRouter>,
    /// M5.bootstrap-wiring: optional `SpanSink` for trace + budget +
    /// memory-op observability. When set, bootstrap wires:
    ///
    /// - `ObservabilityMemoryTraceBridge` into the `Memory` instance so
    ///   M3.3 memory-op events reach the JSON span channel (the bridge
    ///   was already implemented but had no production install path).
    /// - `ObservabilityBudgetEventBridge` into the per-session
    ///   `BudgetTracker` (when `config.session_cap` is also set) so
    ///   `BudgetEvent::Charge` / `CapWarn` / `CapBlock` fire to the same
    ///   sink.
    ///
    /// Default `None` keeps pre-M5 behaviour: both bridges stay dormant.
    span_sink: Option<Arc<dyn SpanSink>>,
}

impl AgentBootstrap {
    pub fn new(config: Config, workspace: impl Into<String>, output: Arc<dyn OutputSink>) -> Self {
        Self {
            config,
            workspace: workspace.into(),
            output,
            provider: None,
            resume_session: None,
            extra_skill_dirs: Vec::new(),
            plugin_provider_router: None,
            span_sink: None,
        }
    }

    /// M5.bootstrap-wiring — install an `Arc<dyn SpanSink>` that bootstrap
    /// will use to back the memory-trace bridge (M3.3) and budget-event
    /// bridge (M5.3). Without this, both bridges stay un-instantiated and
    /// the corresponding event channels never fire in production — the
    /// CLI / host is responsible for calling this with whichever sink
    /// (`InMemorySink`, `JsonStdoutSink`, `OtlpSink`) makes sense for the
    /// runtime.
    pub fn with_span_sink(mut self, sink: Arc<dyn SpanSink>) -> Self {
        self.span_sink = Some(sink);
        self
    }

    /// Use a pre-created provider instead of creating one from config.
    pub fn provider(mut self, provider: Arc<dyn LlmProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Wave OL: install a plugin-provider router. Invoked after plugin
    /// init; if it returns `Some(provider)`, that provider replaces the
    /// default `wcore_providers::create_provider(&config)` path. Used by
    /// `wcore-cli` to route `--model ollama:*` through the loaded
    /// `wayland-ollama` plugin's `OllamaProvider`. No-op when an
    /// explicit `.provider(...)` was already supplied.
    pub fn plugin_provider_router(mut self, router: PluginProviderRouter) -> Self {
        self.plugin_provider_router = Some(router);
        self
    }

    /// Resume from a previously saved session.
    pub fn resume(mut self, session: Session) -> Self {
        self.resume_session = Some(session);
        self
    }

    /// Add extra directories to scan for skills.
    pub fn extra_skill_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.extra_skill_dirs = dirs;
        self
    }

    /// Read-only access to the config (for session management before build).
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Build the fully-initialized engine.
    pub async fn build(mut self) -> anyhow::Result<BootstrapResult> {
        let cwd = &self.workspace;
        let cwd_path = std::path::Path::new(cwd);

        // Wave OL: provider construction is now DEFERRED until after
        // plugin init, so the `plugin_provider_router` (if any) can
        // inspect the live `Vec<Arc<dyn PluginProvider>>` and route
        // model strings like `ollama:llama3` through a plugin-supplied
        // `LlmProvider`. We materialize `primary_provider` after the
        // plugin runner has settled (see post-init block below). The
        // original ResilientProvider wrap also moves after that point.

        let memory_dir = wcore_memory::paths::auto_memory_dir(cwd_path);

        let file_cache = if self.config.file_cache.enabled {
            let cache = Arc::new(std::sync::RwLock::new(
                wcore_tools::file_cache::FileStateCache::new(&self.config.file_cache),
            ));
            // Token-opt (diff-resend): enable client-side diff answers only on
            // routes that optimize client-side. Router-optimized routes leave
            // this off and send full reads. Set once here from the resolved
            // compat (config is moved into the engine below, so it must happen
            // before construction).
            if let Ok(mut c) = cache.write() {
                c.set_optimize_reads(self.config.compat.input_optimization() == "client");
            }
            Some(cache)
        } else {
            None
        };

        // F13 (Task 8): keep a clone of file_cache so the Script dispatcher
        // can mint a parallel Read/Write/Edit set that shares the same cache.
        let file_cache_for_script = file_cache.clone();
        // Token-opt (diff-resend): keep a clone for the engine so it can bump the
        // cache's compaction generation after each compaction pass (the original
        // `file_cache` is moved into EditTool below).
        let file_cache_for_engine = file_cache.clone();

        let mut registry = wcore_tools::registry::ToolRegistry::new();

        // W2.5: plugin discovery + initialization. PluginsConfig is
        // intentionally empty this wave; full ~/.wayland-core/plugins.toml
        // load lands in W4 alongside the permission-grant UX. Built-in
        // plugins discovered via inventory work today regardless. Per
        // design spec §5.17: one bad plugin must not crash session boot —
        // every plugin error logs via tracing::warn and continues.
        let plugins_config = wcore_config::plugins_config::PluginsConfig::default();
        let mut plugin_loader = crate::plugins::PluginLoader::discover(&plugins_config);
        let captured = plugin_loader.validate_all().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "plugin validation failed; continuing without plugins");
            Vec::new()
        });
        // FleetDispatcher-class fix (audit 2026-05-24): without flipping
        // `computer_use_advertised`, every captured `CuaToolSpec` errors
        // `CapabilityDisabled` at reify-time and no CUA tool ever reaches
        // the registry. Per-OS reification still self-gates restricted
        // platforms (e.g. wlroots-only Wayland compositors via
        // `compositor_allows_background_input`), so unconditionally `true`
        // here is safe — the runtime check refuses on unsupported hosts.
        let mut plugin_runner =
            crate::plugins::PluginRunner::new().with_computer_use_advertised(true);
        let mut plugin_outcome = match plugin_runner.initialize_all(&captured).await {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(error = %e, "plugin initialization failed; continuing without plugins");
                crate::plugins::InitializeOutcome::default()
            }
        };
        for (name, err) in &plugin_outcome.errors {
            tracing::warn!(plugin = %name, error = %err, "plugin initialization error (continuing)");
        }

        // Wave 6A.1 — on-disk plugin discovery. The inventory `discover()`
        // above only finds statically-linked plugins. Path-based WASM /
        // subprocess / MCP-bridge plugins live at
        // `$WAYLAND_PLUGINS_DIR` (override) or
        // `PluginIdentity::default_plugin_root()`. We walk that root,
        // dispatch each manifest to the matching runner, then synthesize
        // `InitializeOutcome`s from the loaded handles and merge them into
        // `plugin_outcome` BEFORE `apply_initialize_outcome` runs. Keepalive
        // handles for spawned subprocess / mcp-bridge / wasm plugins are
        // stashed on the engine via `set_plugin_runtime_handles` so the
        // closures inside the synthesized tools keep working for the
        // session's lifetime.
        let wasm_plugin_runner = match wcore_plugin_wasm::WasmPluginRunner::new() {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!(error = %e, "WasmPluginRunner init failed; on-disk WASM plugins will be skipped");
                None
            }
        };
        let plugin_gate = Arc::new(wcore_plugin_api::PluginAccessGate);
        plugin_loader
            .discover_on_disk(
                &plugin_runner,
                wasm_plugin_runner.as_ref(),
                plugin_gate.clone(),
            )
            .await;
        let mut plugin_runtime_keepalives: Vec<crate::plugins::LoadedRuntimeHandle> = Vec::new();
        for record in plugin_loader.take_on_disk_dispatches() {
            if let Err(reason) = &record.load_result {
                tracing::warn!(
                    plugin = %record.plugin_name,
                    manifest = %record.manifest_path.display(),
                    error = %reason,
                    "on-disk plugin load failed (continuing)"
                );
                continue;
            }
            let crate::plugins::loader::OnDiskDispatchRecord {
                plugin_name,
                tool_namespace,
                handle,
                ..
            } = record;
            match handle {
                crate::plugins::LoadedRuntimeHandle::Wasm(loaded) => {
                    let tools = loaded.tools().await;
                    let synth = crate::plugins::synthesize_initialize_outcome_wasm(
                        loaded.clone(),
                        &plugin_name,
                        &tool_namespace,
                        tools,
                    );
                    plugin_outcome.tools.extend(synth.tools);
                    plugin_runtime_keepalives
                        .push(crate::plugins::LoadedRuntimeHandle::Wasm(loaded));
                }
                crate::plugins::LoadedRuntimeHandle::Subprocess(loaded) => {
                    let synth = crate::plugins::synthesize_initialize_outcome_subprocess(
                        loaded.clone(),
                        &plugin_name,
                        &tool_namespace,
                    );
                    plugin_outcome.tools.extend(synth.tools);
                    plugin_runtime_keepalives
                        .push(crate::plugins::LoadedRuntimeHandle::Subprocess(loaded));
                }
                crate::plugins::LoadedRuntimeHandle::McpBridge(loaded) => {
                    // The mcp-bridge synthesizer consumes `loaded` by value
                    // via `into_parts`; the closures inside the tools hold
                    // their own `Arc<McpBridgePluginRunner>` reference, so
                    // there is no separate keepalive to stash here.
                    let synth = crate::plugins::synthesize_initialize_outcome_mcp_bridge(
                        loaded,
                        &plugin_name,
                        &tool_namespace,
                    );
                    plugin_outcome.tools.extend(synth.tools);
                }
                crate::plugins::LoadedRuntimeHandle::None => {}
            }
        }
        let has_plugins = plugin_outcome.has_any_registered();
        // W8c.3 H.2: snapshot the loaded plugin names so the protocol
        // sink can flip per-plugin capability flags
        // (`browser_suite` when `wayland-browser` loaded,
        // `computer_use` when `wayland-cua` loaded). `captured` is the
        // post-validation list; anything that initialize-errored is in
        // `plugin_outcome.errors` but still counts as "discovered" for
        // the wire-capability flag (matches the established pattern of
        // advertising the surface so the host can render a useful
        // error path).
        //
        // Wave SC SECURITY MAJOR fix: pair every loaded plugin with a
        // verified `PluginIdentity`. All inventory-discovered plugins
        // are `Static` (compile-time symbol-anchored); a malicious
        // crate cannot impersonate the real `wayland-browser` /
        // `wayland-cua` because the inventory registry is populated by
        // `inventory::submit!` macros that the engine's own build
        // links in. `from_verified` consumes the `(name, identity)`
        // pairs so the capability advertisement is gated on that
        // proof of origin.
        let verified_plugins: Vec<(String, wcore_plugin_api::PluginIdentity)> = captured
            .iter()
            .map(|d| {
                (
                    d.name.clone(),
                    wcore_plugin_api::PluginIdentity::from_static(&d.name),
                )
            })
            .collect();
        let plugin_capabilities =
            crate::output::protocol_sink::PluginCapabilitySet::from_verified(&verified_plugins);
        // Backwards-compat alias for any consumer that still expects
        // the raw name list (handler-side log lines etc.).
        let loaded_plugin_names: Vec<String> =
            verified_plugins.iter().map(|(n, _)| n.clone()).collect();

        // Wave OL: provider resolution. Precedence:
        //   1. Explicit `.provider(...)` injection wins (test override).
        //   2. `plugin_provider_router` invoked on `config.model` — if it
        //      returns Some, that's the provider (e.g. `ollama:llama3`
        //      routed through `wayland-ollama`'s `OllamaProvider`).
        //   3. Built-in `wcore_providers::create_provider(&config)` for
        //      the four core providers (anthropic/openai/bedrock/vertex).
        let routed_provider: Option<Arc<dyn LlmProvider>> = if self.provider.is_none() {
            if let Some(router) = self.plugin_provider_router.as_ref() {
                let providers: &[Arc<dyn PluginProvider>] = &plugin_runner.providers.registered;
                router(&self.config.model, providers)
            } else {
                None
            }
        } else {
            None
        };

        // E-H2: resilience (circuit breaker) is now ON by default for every
        // provider path.
        //
        //  - An explicitly injected `.provider(...)` (test override) or a
        //    plugin-routed provider is wrapped HERE, so it too gets the
        //    breaker — and, when `provider_chain.enabled`, the protocol-aware
        //    `ProtocolCircuitReporter` (emits `provider_circuit_event`).
        //  - The built-in path uses `create_native_provider` + a wrap here,
        //    rather than `create_provider` (which would wrap with a NoOp
        //    reporter), so circuit transitions are observable on the JSON
        //    stream when chain reporting is enabled.
        //
        // The wrap carries no fallback chain — a single configured provider
        // has no alternate — but fail-fast circuit-breaking is live for all.
        let injected_or_routed = self.provider.take().or(routed_provider);
        let primary_provider: Arc<dyn LlmProvider> = injected_or_routed
            .unwrap_or_else(|| wcore_providers::create_native_provider(&self.config));

        let cfg = CircuitConfig {
            fail_threshold: self.config.provider_chain.failure_threshold as usize,
            window: Duration::from_secs(self.config.provider_chain.recovery_timeout_secs),
            cooldown: Duration::from_secs(self.config.provider_chain.recovery_timeout_secs),
        };
        // Protocol reporter when chain reporting is opted in; NoOp otherwise.
        let reporter: Arc<dyn wcore_providers::CircuitReporter> =
            if self.config.provider_chain.enabled {
                Arc::new(crate::resilient_reporter::ProtocolCircuitReporter::new(
                    self.output.clone(),
                ))
            } else {
                Arc::new(wcore_providers::NoOpCircuitReporter)
            };
        let provider: Arc<dyn LlmProvider> = Arc::new(ResilientProvider::new(
            self.config.provider_label.clone(),
            primary_provider,
            Vec::new(),
            cfg,
            reporter,
        ));

        registry.register(Box::new(wcore_tools::read::ReadTool::new(
            file_cache.clone(),
        )));
        registry.register(Box::new(wcore_tools::write::WriteTool::new(
            file_cache.clone(),
        )));
        registry.register(Box::new(wcore_tools::edit::EditTool::new(file_cache)));
        registry.register(Box::new(wcore_tools::bash::BashTool));
        registry.register(Box::new(wcore_tools::grep::GrepTool));
        registry.register(Box::new(wcore_tools::glob::GlobTool));
        // T11: JsonlTool — large-file-friendly JSON Lines streaming tool.
        registry.register(Box::new(wcore_tools::jsonl_tool::JsonlTool::default()));
        // T3-3.1.1: ClarifyTool — structured user-clarification prompt
        // (ported from wayland-hermes). The host layer intercepts
        // tool calls named `clarify` to perform the real UI interaction.
        registry.register(Box::new(wcore_tools::clarify::ClarifyTool::new()));
        // v0.9.3 W0.4: AskUserQuestionTool — structured multi-choice question.
        // Sibling to ClarifyTool, but the user's answer routes back via the
        // approval channel (ProtocolCommand::ToolApprove `answer` field,
        // W0.1) and is synthesized into the tool result by orchestration
        // at `orchestration::mod.rs:911` (W0.3). `execute()` is a loud-
        // defensive fallback; the happy path never dispatches.
        registry.register(Box::new(
            wcore_tools::ask_user_question::AskUserQuestionTool::new(),
        ));
        // T3-3.1.2: TodoTool — in-memory planning/task list ported from
        // wayland-hermes. State is per-session (one `TodoTool` instance
        // per bootstrap → one list per agent session).
        registry.register(Box::new(wcore_tools::todo::TodoTool::new()));
        // T3-3.1.4: SendMessageTool — registered with the fail-loud
        // `NullMessageTransport` default so the tool is schema-visible to
        // the LLM but every send call fails loudly until a host wires a
        // real transport (Telegram/Discord/Slack/etc.). Mirrors the
        // conditional-registration precedent set by RepoMapTool below.
        registry.register(Box::new(
            wcore_tools::send_message::SendMessageTool::default(),
        ));
        // W6 A1: GitTool — typed wrapper over git ops. Read-only ops route
        // through the concurrency-safe path automatically via
        // `is_concurrency_safe(input)`. Mutating ops (add/commit/checkout/stash)
        // require an explicit user-facing call from the LLM.
        registry.register(Box::new(wcore_tools::git::GitTool));
        // T15: PdfTool — read-only PDF text extraction. Always registered;
        // degrades to an honest error when wcore-tools is built without
        // the default-on `pdf` feature.
        registry.register(Box::new(wcore_tools::pdf_tool::PdfTool::new()));

        // v0.6.3 D.0: wire the remaining catalog tools into the live
        // registry. Until D.0 these shipped as `pub mod` code unreachable
        // by a running agent. Eleven are self-constructing (no host
        // dependency); the four API-seam tools (github/gitlab/linear/
        // notion) are bound to real HTTP backends below.
        //
        // Self-constructing data/file/CLI tools — all read-only or
        // CLI-shelling, each degrades to an honest error when its
        // optional feature or external binary is absent.
        registry.register(Box::new(wcore_tools::sql_query_tool::SqlQueryTool::new()));
        // v0.9.0 W1 B-Postgres — live tokio-postgres backend gated on
        // POSTGRES_URL / DATABASE_URL. Resolver is async (exposes a sync
        // validate path; the Config::connect run-path is async). `.await`
        // is legal here because `build()` is async.
        if let Some(b) =
            crate::tool_backends::postgres_schema::build_postgres_schema_backend().await
        {
            registry.register(Box::new(
                wcore_tools::postgres_schema_tool::PostgresSchemaTool::new(b),
            ));
        }
        registry.register(Box::new(wcore_tools::archive_tool::ArchiveTool));
        registry.register(Box::new(
            wcore_tools::markdown_tool::MarkdownTableTool::new(),
        ));
        registry.register(Box::new(
            wcore_tools::image_inspect_tool::ImageInspectTool::new(),
        ));
        registry.register(Box::new(
            wcore_tools::email_parse_tool::EmailParseTool::new(),
        ));
        registry.register(Box::new(wcore_tools::kubectl_tool::KubectlTool::new()));
        registry.register(Box::new(wcore_tools::gcloud_tool::GcloudTool));
        wcore_tools::aws_cli_tool::register_aws_cli_tool(&mut registry);

        // API-seam tools — bound to real HTTP backends over
        // `wcore-providers::http_client` (see `crate::tool_backends`).
        // The tools resolve their own auth tokens (input arg or env var)
        // into the request; a missing credential surfaces as a clean
        // upstream `401`/`403`, never a silent stub.
        let api_backends = crate::tool_backends::build_api_tool_backends();
        wcore_tools::github_tool::register_github_tool(&mut registry, api_backends.github);
        wcore_tools::gitlab_tool::register_gitlab_tool(
            &mut registry,
            api_backends.gitlab,
            None,
            None,
        );
        wcore_tools::linear_tool::register_linear_tool(&mut registry, api_backends.linear);
        wcore_tools::notion_tool::register_notion_tool(&mut registry, api_backends.notion);

        // Wave RC (2026-05-23): wire WebFetch. The Browser tool requires a
        // Camoufox / Chromium sidecar that ISN'T installed by default on a
        // fresh wayland-core, so a model asked "fetch github.com/trending"
        // used to call Browser, hit the missing sidecar, and watch a 60s
        // spinner. WebFetch is a plain HTTP GET (real reqwest backend
        // from crate::tool_backends) + readability extraction for HTML
        // pages — works on every machine without any extra install.
        // Registered BEFORE the `builtin_names` snapshot below so MCP
        // collision detection treats it as a builtin.
        wcore_tools::web_fetch::register_web_fetch_tool(
            &mut registry,
            crate::tool_backends::build_fetch_backend(),
        );

        // FleetDispatcher-class fix #2 (audit 2026-05-24 §7): 24 orphan
        // tools across 13 files were `pub mod`-exported in `wcore-tools/lib.rs`
        // but never `registry.register`'d, so the LLM had zero visibility
        // into them. Each one ships with a Null/Capturing backend default
        // that fails LOUDLY when called — schema is now LLM-callable, but a
        // missing real backend surfaces as a typed error instead of silent
        // capability loss. Real backends ship in follow-up commits per
        // category (Spotify needs OAuth, Discord needs bot token, etc).
        //
        // Excluded from this commit: `moa::MoaTool` (requires Vec<ProposerSpec>
        // config — not a wire-presence fix; needs Config schema design).
        //
        // v0.9.0 W1 (2026-05-27): the 24 orphan tools below have had their
        // null-backed `::default()` registrations swapped for real env-gated
        // backends. Tools without their gating env var return `None` from
        // their resolver and are hidden via `Tool::is_available() == false`.
        // The Spotify cluster (7 tools) is the one remaining default-only
        // block — wired in v0.9.1.
        // v0.9.0 W1 B-Discord — gated on DISCORD_BOT_TOKEN. Resolver returns
        // None when missing/empty; tool then hidden via `is_available()`.
        if let Some(b) = crate::tool_backends::discord::build_discord_backend() {
            registry.register(Box::new(wcore_tools::discord_tool::DiscordTool::new(b)));
        }
        registry.register(Box::new(
            wcore_tools::spotify_tool::SpotifyPlaybackTool::default(),
        ));
        registry.register(Box::new(
            wcore_tools::spotify_tool::SpotifyDevicesTool::default(),
        ));
        registry.register(Box::new(
            wcore_tools::spotify_tool::SpotifyQueueTool::default(),
        ));
        registry.register(Box::new(
            wcore_tools::spotify_tool::SpotifySearchTool::default(),
        ));
        registry.register(Box::new(
            wcore_tools::spotify_tool::SpotifyPlaylistsTool::default(),
        ));
        registry.register(Box::new(
            wcore_tools::spotify_tool::SpotifyAlbumsTool::default(),
        ));
        registry.register(Box::new(
            wcore_tools::spotify_tool::SpotifyLibraryTool::default(),
        ));
        // v0.9.0 W1 B3 — homeassistant: HTTP backend gated on
        // HOME_ASSISTANT_URL + HOME_ASSISTANT_TOKEN. Resolver returns None
        // when either is absent; tool then hidden via `is_available()`.
        if let Some(b) = crate::tool_backends::homeassistant::build_homeassistant_backend() {
            registry.register(Box::new(
                wcore_tools::homeassistant_tool::HomeAssistantTool::new(b),
            ));
        }
        // v0.9.0 W1 B4 — google_meet: 5 tools share one OAuth-backed
        // HttpGoogleMeetBackend. PKCE-S256 is the default; the resolver
        // returns None when `GOOGLE_CLIENT_ID` is missing/empty (R-H2),
        // hiding all 5 tools. MeetSayTool registers but returns
        // MeetApiCapabilityError at runtime because Meet REST v2 has no
        // in-call TTS endpoint.
        if let Some(b) = crate::tool_backends::google_meet::build_google_meet_backend() {
            let b: std::sync::Arc<dyn wcore_tools::google_meet_tool::GoogleMeetBackend> =
                std::sync::Arc::new(b);
            registry.register(Box::new(wcore_tools::google_meet_tool::MeetJoinTool::new(
                b.clone(),
            )));
            registry.register(Box::new(
                wcore_tools::google_meet_tool::MeetStatusTool::new(b.clone()),
            ));
            registry.register(Box::new(
                wcore_tools::google_meet_tool::MeetTranscriptTool::new(b.clone()),
            ));
            registry.register(Box::new(wcore_tools::google_meet_tool::MeetLeaveTool::new(
                b.clone(),
            )));
            registry.register(Box::new(wcore_tools::google_meet_tool::MeetSayTool::new(b)));
        }
        registry.register(Box::new(wcore_tools::yuanbao_tools::YuanbaoTool::default()));
        // v0.9.0 W1 B1 — image_gen: real backends (OpenAI DALL-E 3 / FAL /
        // Gemini Imagen / HF) gated on env keys. The `allow_pollinations`
        // arg defaults to `false` (opt-in only); a future config field at
        // `builtin_tools.image_gen.allow_pollinations_fallback` will surface
        // it to users without recompiling.
        if let Some(b) = crate::tool_backends::image_gen::build_image_gen_backend(false) {
            registry.register(Box::new(
                wcore_tools::image_generation_tool::ImageGenerationTool::with_backend(b),
            ));
        }
        // `web` tool — wired to a real search backend so the model
        // gets actual results, not a "no backend configured" 404 wall.
        // [`build_web_search_backend`] picks the best available:
        //   - TAVILY_API_KEY → Tavily   (paid; best LLM-tuned)
        //   - BRAVE_SEARCH_API_KEY → Brave (free tier 2k/mo)
        //   - default → DuckDuckGo HTML scrape (free, no key)
        // Extract/crawl operations still error out cleanly on the free
        // backend with a "set FIRECRAWL_API_KEY" message — the model
        // can fall back to `WebFetch` for single-URL reads.
        registry.register(Box::new(wcore_tools::web_tools::WebTool::new(
            crate::tool_backends::build_web_search_backend(),
        )));
        // `vision_analyze` tool — wired to the user's existing LLM key
        // (Anthropic preferred, OpenAI / Gemini auto-fallback). If
        // NONE of the three keys is set the resolver returns None and
        // the tool stays hidden via `Tool::is_available() == false`.
        if let Some(vision_backend) = crate::tool_backends::build_vision_backend() {
            registry.register(Box::new(wcore_tools::vision_tools::VisionAnalyzeTool::new(
                vision_backend,
                crate::tool_backends::build_image_fetcher(),
            )));
        }
        // `transcribe_audio` — Groq Whisper free tier preferred,
        // OpenAI Whisper fallback. If neither key is set the tool
        // hides itself via `Tool::is_available()`.
        if let Some(stt_backend) = crate::tool_backends::build_transcription_backend() {
            registry.register(Box::new(
                wcore_tools::transcription_tools::TranscribeAudioTool::new(
                    stt_backend,
                    crate::tool_backends::build_audio_fetcher(),
                ),
            ));
        }
        // v0.9.0 W1 B2 — tts: OpenAI > ElevenLabs > (feature-gated piper).
        // Resolver returns None when no provider is configured; tool is then
        // hidden via `is_available() == false`.
        if let Some(b) = crate::tool_backends::tts::build_tts_backend() {
            registry.register(Box::new(wcore_tools::tts_tool::TtsTool::with_backend(b)));
        }
        // v0.9.0 W1 B10 — voice_mode: cpal-backed recorder + STT bridge.
        // Resolver returns None when cpal can't bind a default input device
        // (CI, container, headless SSH); tool is then hidden via
        // `is_available() == false`. Registered AFTER the TTS/STT block so
        // the LLM observability surfaces are wired before the voice loop
        // is reachable.
        if let Some(vm) = crate::tool_backends::voice_mode::build_voice_mode_backend() {
            registry.register(Box::new(wcore_tools::voice_mode::VoiceModeTool::new(vm)));
        }
        // v0.9.0 W1 B5 — video_analyze: async ffmpeg probe + LLM vision
        // backend. Resolver is `pub async fn build_video_analyze_backend()`
        // because the ffmpeg probe spawns a child process cached in a
        // tokio::sync::OnceCell; `.await` is legal here because `build()`
        // is async.
        if let Some(b) = crate::tool_backends::video_analyze::build_video_analyze_backend().await {
            registry.register(Box::new(
                wcore_tools::video_analyze_tool::VideoAnalyzeTool::with_backend(b),
            ));
        }
        // v0.9.0 W1 B7 — wayland_introspection: two tools share one backend.
        // The backend reads in-process session state (no env keys, no
        // network); writer-side wiring of the same Arc into the engine is
        // a deferred follow-up so the tools surface zeroes for now.
        let state_reader: std::sync::Arc<dyn crate::session_state::SessionStateReader> =
            std::sync::Arc::new(crate::session_state::InMemorySessionState::new(
                self.config.model.clone(),
            ));
        let intro_backend =
            crate::tool_backends::introspection::build_introspection_backend(state_reader.clone());
        registry.register(Box::new(
            wcore_tools::wayland_introspection::WaylandStatusTool::new(intro_backend.clone()),
        ));
        registry.register(Box::new(
            wcore_tools::wayland_introspection::WaylandTelemetryQueryTool::new(intro_backend),
        ));
        // v0.9.0 W1 B6 — cronjob: wire WaylandCronScheduler over FileCronStore.
        // Adapter constructs its own FileCronStore over the default path; the
        // runner at bootstrap.rs:~1900 owns a separate FileCronStore over the
        // same path. Both writers serialise inside the store's internal mutex;
        // reads + tempfile+rename writes are atomic so the two-instance
        // pattern is safe (see tool_backends/cron.rs module doc).
        if let Some(b) = crate::tool_backends::cron::build_cron_backend() {
            registry.register(Box::new(wcore_tools::cronjob_tools::CronJobTool::new(b)));
        }

        // W3→W4 hand-off (Task 9.5): register RepoMap when enabled. Default
        // on per RepoMapToolConfig::default() — read-only and shape-bounded.
        if self.config.builtin_tools.repomap.enabled {
            registry.register(Box::new(wcore_tools::repomap::RepoMapTool::new(
                cwd_path.to_path_buf(),
            )));
        }

        let builtin_names: Vec<String> = registry.tool_names();

        // v0.6.4 Task 1.7 — deliver every captured plugin capability.
        //
        // ORDERING (§6 of the Phase 1 design notes):
        //   - tools: `apply_initialize_outcome` registers plugin tools into
        //     `registry` HERE — *after* the builtin block — so a plugin tool
        //     whose name collides with a builtin is logged + skipped
        //     (builtins win). It runs before the MCP pass so the
        //     `builtin_names` snapshot above is the pure-builtin set.
        //   - agents: returned in `applied.agent_registry`, threaded into
        //     `SpawnTool` + the engine after construction.
        //   - skills: `applied.plugin_skills` registered via
        //     `register_bundled_skill` BEFORE `load_catalog` (below).
        //   - rules: `applied.plugin_rules` passed to `build_system_prompt`.
        //   - hooks: `applied.plugin_hooks` handed to the engine setter
        //     after construction.
        //   - mcp: `applied.plugin_mcp_servers` connected via the
        //     `connect_plugin_mcp_servers` second pass below.
        //   - user-models: `applied.plugin_user_models` is a carrier only at
        //     v0.6.4 Task 2.2. v0.6.4 Task 2.3 will reify each
        //     `CapturedUserModel` into a live `wayland_honcho::HonchoClient`
        //     (or other backend) and thread it into the engine via the
        //     `UserModel` injection point.
        //
        // FleetDispatcher-class fix #3 (audit 2026-05-24 §3): apply the
        // operator's `Config.browser.policy` to every captured
        // `BrowserToolSpec.policy` BEFORE the host registrar reifies them.
        // The plugin shell registers a `BrowserPolicySpec::default()`
        // (deny-all) — without this copy, every navigate from the LLM
        // denies regardless of what the user configured in
        // `[browser.policy]` in their config.toml. v0.8.4's fix wired the
        // schema; this completes the loop by feeding it through to the
        // reify step.
        let policy = &self.config.browser.policy;
        for spec in &mut plugin_runner.browser.specs {
            spec.policy.default_action = policy.default_action.clone();
            spec.policy.allowed_origins = policy.allowed_origins.clone();
            spec.policy.denied_origins = policy.denied_origins.clone();
        }

        // v0.6.5 Task 1.4 — browser/cua plugin tools now reify INSIDE
        // `apply_initialize_outcome` (see `apply.rs::deliver_browser_tools`
        // and `deliver_cua_tools`). Bootstrap moves each registrar out of
        // `plugin_runner` by value; the runner's slot is replaced with a
        // fresh empty registrar so it stays well-formed for any later
        // inspection.
        let browser_registrar = std::mem::take(&mut plugin_runner.browser);
        let cua_registrar = std::mem::take(&mut plugin_runner.cua);
        let applied = crate::plugins::apply_initialize_outcome(
            plugin_outcome,
            &mut registry,
            browser_registrar,
            cua_registrar,
        );

        let mut mcp_managers: Vec<Arc<McpManager>> = Vec::new();
        let mcp_manager = if !self.config.mcp.servers.is_empty() {
            match McpManager::connect_all(&self.config.mcp.servers).await {
                Ok(mgr) => {
                    let mgr = Arc::new(mgr);
                    wcore_mcp::tool_proxy::register_mcp_tools(
                        &mut registry,
                        &mgr,
                        &builtin_names,
                        &self.config.mcp.servers,
                    );
                    mcp_managers.push(mgr.clone());
                    Some(mgr)
                }
                Err(e) => {
                    self.output
                        .emit_error(&format!("MCP initialization error: {e}"), false);
                    None
                }
            }
        } else {
            None
        };

        // v0.6.4 Task 1.5/1.7 — plugin MCP second pass. Runs `connect_all` +
        // `register_mcp_tools` for plugin-supplied servers, reusing the
        // pre-MCP `builtin_names` snapshot for collision detection. Non-fatal:
        // a failed plugin MCP connect logs and returns `None` (one bad plugin
        // cannot crash boot).
        if let Some(plugin_mcp_mgr) = crate::plugins::mcp_delivery::connect_plugin_mcp_servers(
            &applied.plugin_mcp_servers,
            &mut registry,
            &builtin_names,
        )
        .await
        {
            mcp_managers.push(plugin_mcp_mgr);
        }

        let has_mcp = mcp_manager.is_some() || !mcp_managers.is_empty();

        // C1 / Task A1 — build the host hook dispatcher. Plugin lifecycle hooks
        // (e.g. SessionStart) can pull a contribution from an MCP tool of the
        // same NAME on the plugin's MCP server. The dispatcher is framework-
        // blind (see `crate::hooks::mcp_dispatcher`); the only IJFW-or-anything
        // knowledge is the `plugin -> mcp server` map built here from registry
        // state — no plugin name is hardcoded.
        //
        // Provenance gap: `McpServerSpec` carries no originating-plugin field
        // and `HostMcpRegistrar` stores a flat list, so we cannot read the map
        // directly. Instead we resolve generically: each plugin that registered
        // a hook is matched to the connected MCP server whose advertised tool
        // list contains one of that plugin's hook names. Tools are discovered
        // eagerly at connect (`tools/list`), so the live `all_tools()` view is
        // populated by the time we get here.
        //
        // First-match assumption: this binds a plugin to the FIRST server (in
        // `all_tools()` iteration order, which is not deterministic across a
        // HashMap) advertising any of its hook tool names. That is unambiguous
        // only while hook-tool names are unique across servers — the case today
        // (one memory-style plugin → one server). If two servers ever advertise
        // a tool sharing a plugin's hook name, the bind is arbitrary; the real
        // fix is plugin→server provenance on `HostMcpRegistrar` (see A4/A5).
        //
        // Gated by `config.hooks.dispatch_enabled` (default ON). When off, or
        // when no plugin hook resolves to a server, no dispatcher is wired and
        // plugin hooks stay log-only (the legacy behavior).
        let hook_dispatcher: Option<Arc<dyn crate::hooks::HookDispatcher>> =
            if self.config.hooks.dispatch_enabled
                && !applied.plugin_hooks.is_empty()
                && !mcp_managers.is_empty()
            {
                // plugin name -> set of its hook tool names
                let mut hooks_by_plugin: std::collections::HashMap<&str, Vec<&str>> =
                    std::collections::HashMap::new();
                for h in &applied.plugin_hooks {
                    hooks_by_plugin
                        .entry(h.plugin.as_str())
                        .or_default()
                        .push(h.name.as_str());
                }
                // For each plugin, find the server advertising one of its hook tools.
                let mut server_for_plugin: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                for (plugin, hook_names) in &hooks_by_plugin {
                    'mgr: for mgr in &mcp_managers {
                        for (server_name, tool) in mgr.all_tools() {
                            if hook_names.iter().any(|hn| *hn == tool.name) {
                                server_for_plugin
                                    .insert((*plugin).to_string(), server_name.to_string());
                                break 'mgr;
                            }
                        }
                    }
                    if !server_for_plugin.contains_key(*plugin) {
                        tracing::debug!(
                            target: "wcore_agent::hooks",
                            plugin = %plugin,
                            "no MCP server advertises this plugin's hook tools; hooks stay log-only"
                        );
                    }
                }
                if server_for_plugin.is_empty() {
                    None
                } else {
                    let caller =
                        Arc::new(crate::hooks::McpManagerCaller::new(mcp_managers.clone()));
                    Some(Arc::new(crate::hooks::McpHookDispatcher::new(
                        caller,
                        server_for_plugin,
                    )))
                }
            } else {
                None
            };

        // M3.6.2 — build memory_api BEFORE skill_refs so the prioritizer
        // can reorder the catalog at session start. Moved up from its
        // original location (post-engine-construction) for sequencing.
        // The engine setters at the bottom (`engine.set_memory_api`,
        // `engine.push_decay_handle`) still consume these values.
        //
        // W7 Pre-flight 0.0b + M3.2: build a real `Memory` when
        // `cfg.memory.enabled` OR `observability.skills_lifecycle` is on.
        // When BOTH gates are on we share one `Memory` instance so the
        // same DB backs the skills wiring and the scheduler. When only
        // `skills_lifecycle` is on the scheduler stays unspawned (the dev
        // flag is observability-only). When only `memory.enabled` is on we
        // open `Memory` + spawn the scheduler. When neither is on we stay
        // on `NullMemory`.
        let want_memory = self.config.memory.enabled || self.config.observability.skills_lifecycle;
        let mut decay_handle: Option<tokio::task::JoinHandle<()>> = None;
        // v0.8.1 U1 — capture the `Arc<Db>` handle from the opened
        // `Memory` so we can hand it to `wcore_evolve::PromptStore::new`
        // when seeding the per-turn `SkillRouter` below. `MemoryApi` is
        // a trait object and doesn't expose the underlying connection,
        // so we keep this typed handle alongside `memory_api`.
        let mut mem_db_for_router: Option<Arc<wcore_memory::db::Db>> = None;
        let memory_api: Arc<dyn wcore_memory::MemoryApi> = if want_memory {
            // Session id is not yet known at build() time — the engine
            // initialises sessions later via `init_session()`. Use a
            // synthetic "boot" id; the W5 v2 Memory uses one DB per
            // session_id, so this stays isolated from real session data.
            // Tests that need true session-scoped memory will call
            // `engine.set_memory_api()` after `init_session()` with the
            // real id.
            // R2 fix C1 — NOTE(v0.6.3+): if KG + staleness are wired into
            // production bootstrap here, `init_kg()` MUST run before
            // `init_staleness()`. The staleness table carries a FK reference
            // to `kg_nodes(id)`, so the schemas must be created in that
            // order or the staleness `CREATE TABLE` fails. v0.6.2 wires
            // neither; this note exists to bank the ordering constraint
            // before the wiring lands.
            match wcore_memory::Memory::open(cwd_path, "boot").await {
                Ok(mem) => {
                    if self.config.observability.skills_lifecycle {
                        tracing::info!(
                            "W7 Pre-0: skills_lifecycle ON — real Memory wired \
                             (session-scope id='boot'; rebind on init_session)"
                        );
                    }
                    // M5.bootstrap-wiring — if a SpanSink was installed,
                    // attach the M3.3 `ObservabilityMemoryTraceBridge`
                    // so memory-op events reach the JSON span channel.
                    // Without this hook, the bridge type ships but
                    // nothing in production instantiates it (the M3.3
                    // gap this task closes).
                    //
                    // R2 fix (D.2) — the trace sink MUST be attached BEFORE
                    // `spawn_decay_scheduler` below. `spawn_decay_scheduler`
                    // captures `self.dispatcher.clone()` at call time; if the
                    // decay task is spawned first, its dispatcher clone
                    // predates the `with_trace_sink` rebind and every
                    // decay-cycle memory op silently bypasses the trace
                    // bridge. Attach the sink, then spawn the scheduler on the
                    // trace-sink-bearing `Memory`.
                    let mem = if let Some(sink) = self.span_sink.as_ref() {
                        let bridge = Arc::new(
                            wcore_observability::sink::ObservabilityMemoryTraceBridge::new(
                                sink.clone(),
                            ),
                        );
                        mem.with_trace_sink(bridge)
                    } else {
                        mem
                    };
                    // M3.2 — spawn the decay scheduler iff the user opted
                    // into memory. The scheduler ticks `decay()` every
                    // `cfg.memory.decay_interval_secs` and is aborted by
                    // `AgentEngine::Drop` on shutdown. Spawned AFTER the
                    // trace-sink rebind above so decay-cycle ops emit spans.
                    if self.config.memory.enabled {
                        let interval =
                            std::time::Duration::from_secs(self.config.memory.decay_interval_secs);
                        decay_handle = Some(mem.spawn_decay_scheduler(interval));
                        tracing::info!(
                            interval_secs = self.config.memory.decay_interval_secs,
                            "M3.2: memory.enabled ON — decay scheduler spawned"
                        );
                    }
                    // W2 v0.6.3 — initialize the knowledge-graph schema in
                    // the session-tier connection if KG is enabled. Closes
                    // the v0.6.2 SCAFFOLDED gap where `kg::init` shipped but
                    // was never invoked on the production Memory instance.
                    //
                    // `init_kg` is synchronous and operates on a raw rusqlite
                    // `Connection` via `parking_lot::Mutex`. The session
                    // tier is the canonical owner of the per-run KG; failure
                    // is non-fatal — we warn and continue so memory-only
                    // flows aren't blocked by KG-schema issues on first boot.
                    if wcore_memory::kg::kg_enabled() {
                        if let Some(tier_conn) = mem.db.tier(wcore_memory::v2_types::Tier::Session)
                        {
                            let conn = tier_conn.conn.lock();
                            if let Err(e) = wcore_memory::kg::init_kg(&conn) {
                                tracing::warn!(
                                    target: "wcore_agent",
                                    error = %e,
                                    "W2: KG schema init failed (continuing without KG)"
                                );
                            } else {
                                tracing::debug!(
                                    target: "wcore_agent",
                                    "W2: KG schema initialized in session tier"
                                );
                                // W4 v0.6.3 — create the `kg_node_staleness`
                                // table immediately after `init_kg` (the FK
                                // target `kg_nodes` must exist first). Closes
                                // the gap where every production `upsert_node`
                                // called `propagate_staleness` against a
                                // missing table, erroring and swallowing it.
                                if let Err(e) = wcore_memory::staleness::init_staleness(&conn) {
                                    tracing::warn!(
                                        target: "wcore_agent",
                                        error = %e,
                                        "W4: staleness table init failed \
                                         (propagation will be a no-op)"
                                    );
                                } else {
                                    tracing::debug!(
                                        target: "wcore_agent",
                                        "W4: staleness table initialized in session tier"
                                    );
                                }
                            }
                        } else {
                            tracing::warn!(
                                target: "wcore_agent",
                                "W2: no session tier available for KG init; skipping"
                            );
                        }
                    }
                    // v0.8.1 U1 — clone the Db handle for the
                    // `PromptStore` bridge (used to seed `SkillRouter`
                    // below).
                    mem_db_for_router = Some(mem.db.clone());
                    Arc::new(mem) as Arc<dyn wcore_memory::MemoryApi>
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Memory::open failed under skills_lifecycle / memory.enabled; \
                         falling back to NullMemory (no decay scheduler)"
                    );
                    Arc::new(wcore_memory::NullMemory)
                }
            }
        } else {
            Arc::new(wcore_memory::NullMemory)
        };

        // F-036 (HIGH, Aud-2b/Aud-10): call `init_bundled_skills` in
        // production bootstrap. Previously this was only called in tests, so
        // fresh installs had zero invokable native skills (the `hello` skill
        // was invisible). Must run BEFORE plugin registration below so the
        // global registry is cleared+initialised before plugin skills are
        // appended. `init_bundled_skills` is idempotent (clear+register).
        wcore_skills::bundled::init_bundled_skills();
        tracing::debug!(
            target: "wcore_agent::bootstrap",
            "F-036: bundled skills initialised (hello registered)"
        );

        // v0.6.4 Task 1.6/1.7 — register plugin-contributed skills into the
        // process-global bundled-skill registry BEFORE `load_catalog` runs.
        // `load_catalog` reads `bundled::get_bundled_skills()` first
        // (highest priority), so a skill registered here surfaces in the
        // catalog and, transitively, in the system prompt. Each spec is
        // leaked to `&'static str` fields via `spec_to_static_definition`
        // (plugin lifetime == process lifetime — leak is correct, see
        // `skill_delivery.rs`).
        for skill_spec in applied.plugin_skills {
            let name = skill_spec.name.clone();
            wcore_skills::bundled::register_bundled_skill(
                crate::plugins::skill_delivery::spec_to_static_definition(skill_spec),
            );
            tracing::debug!(skill = %name, "plugin skill registered into bundled-skill registry");
        }

        // X1 (Task 5): load the catalog lazily. Bodies are NOT pinned in
        // memory — SkillCatalog::resolve() reads them on demand on first
        // activation, with a 32-entry LRU thereafter.
        let mut skill_refs = wcore_skills::loader::load_catalog(
            cwd_path,
            &self.extra_skill_dirs,
            false,
            mcp_manager.as_deref(),
        )
        .await;

        // F-067 (MED): warn when a user- or project-level skill would shadow a
        // bundled skill. Bundled skills win dedup inside `load_catalog` (spliced
        // to index 0), so the user's copy is silently dropped. This warn surfaces
        // the collision so operators know their local skill is inactive.
        //
        // Strategy: collect bundled names into a HashSet<String>, then scan the
        // skill_refs that came from disk (identified by wcore_skills::types::SkillSource).
        // load_catalog already deduped, so all surviving refs for bundled names ARE the
        // bundled copies — the disk copies are gone. We detect potential collisions by
        // checking whether any disk-sourced skill dir contains a subdirectory matching
        // a bundled name. Cheap: we only walk the dir listing, not the file bodies.
        //
        // NOTE: to flip precedence, add `skills.user_overrides_bundled = true` to wcore.toml
        // and change the splice order in `wcore_skills::loader::load_catalog` (W3-G crate).
        {
            let bundled_names: std::collections::HashSet<String> =
                wcore_skills::bundled::get_bundled_skills()
                    .iter()
                    .map(|d| d.name.to_string())
                    .collect();
            if !bundled_names.is_empty() {
                // Scan disk skill dirs for any directory whose name matches a bundled skill.
                let mut dirs_to_check: Vec<std::path::PathBuf> = Vec::new();
                if let Some(d) = wcore_skills::paths::user_skills_dir() {
                    dirs_to_check.push(d);
                }
                dirs_to_check.extend(wcore_skills::paths::project_skills_dirs(cwd_path));
                dirs_to_check.extend_from_slice(&self.extra_skill_dirs);

                for dir in dirs_to_check {
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if bundled_names.contains(&name) {
                                let dir_display = dir.display().to_string();
                                tracing::warn!(
                                    target: "wcore_agent::bootstrap",
                                    skill = %name,
                                    dir = %dir_display,
                                    "F-067: skill '{name}' in {dir_display} is shadowed by the \
                                     bundled version; bundled wins. To override, flip splice order \
                                     in wcore_skills::loader::load_catalog (see W3-G / \
                                     skills.user_overrides_bundled config key — not yet supported)."
                                );
                            }
                        }
                    }
                }
            }
        }

        // M3.6.2 — reorder skill_refs by procedural-partition success when
        // memory is enabled. Falls back to load order on any memory error
        // (the prioritizer itself swallows errors and returns input). The
        // reorder also flows through the system_prompt below so the prompt
        // introduces skills in their prioritized order.
        if self.config.memory.enabled {
            use std::collections::HashMap;
            let names: Vec<String> = skill_refs.iter().map(|r| r.name.clone()).collect();
            let prioritizer =
                wcore_skills::prioritizer::SkillPrioritizer::new(Arc::clone(&memory_api));
            let ordered = prioritizer.priority_order(&names, 64).await;
            let rank: HashMap<&str, usize> = ordered
                .iter()
                .enumerate()
                .map(|(i, n)| (n.as_str(), i))
                .collect();
            skill_refs.sort_by_key(|r| rank.get(r.name.as_str()).copied().unwrap_or(usize::MAX));
        }

        let mut prompt_cache = crate::context::SystemPromptCache::new();
        let system_prompt = crate::context::build_system_prompt(
            &mut prompt_cache,
            self.config.system_prompt.as_deref(),
            cwd,
            &self.config.model,
            &skill_refs,
            None,
            memory_dir.as_deref(),
            false,
            self.config.compact.toon,
            // v0.6.4 Task 1.7 — plugin-contributed rules. `RuleScope::Universal`
            // fragments are appended to the prompt; `ProjectScoped` ones are
            // gated on cwd inside `build_system_prompt` (Task 1.4).
            &applied.plugin_rules,
            // Output-side opt (Part B): inject the terseness directive only when
            // the route optimizes client-side. Router-optimized routes already
            // trim output server-side, so the directive is omitted there.
            self.config.compat.input_optimization() == "client",
        );
        // G1 slice 4: when the user enabled `[plan] plan_first` (via /config),
        // append a standing preference so the agent enters plan mode on its
        // own for large/risky changes. Plan-mode-*active* instructions are
        // still injected per-turn separately; this is only the base-prompt
        // nudge that makes the agent reach for planning unprompted.
        let system_prompt = if self.config.plan.plan_first {
            format!(
                "{system_prompt}\n\n{}",
                crate::plan::prompt::plan_first_instruction()
            )
        } else {
            system_prompt
        };
        // v0.7.0 2.B.4: append user-context block when memory is on.
        // F-093: backend is selected by `WAYLAND_USER_MODEL_BACKEND`.
        // When the env var is ABSENT, auto-detect: use `honcho` if
        // `HONCHO_API_KEY` is set in the environment, else fall back to
        // `local` and emit a one-time hint so users discover the option.
        // When the env var IS set explicitly, honor it exactly (preserves
        // operator control and existing `local`/`honcho` override paths).
        //
        // The `local` path keeps the same on-disk JSON persistence as
        // before. The `honcho` path round-trips through a live Honcho
        // deployment. Failures degrade silently — telemetry is best-effort,
        // never blocks bootstrap.
        let user_id = "default";
        // v0.8.0 Task M — hoist the `UserModelBackend` out of the
        // `user_ctx_block` scope so it can ALSO be installed on the
        // engine for per-turn write-back. v0.7.0 read-only at
        // bootstrap; v0.8.0 closes that loop so the backend learns
        // continuously from every user turn.
        let user_model_backend: Option<std::sync::Arc<dyn wcore_user_model::UserModelBackend>> =
            if want_memory {
                // F-093: resolve the effective backend name. When the env var
                // is absent, auto-detect from HONCHO_API_KEY presence.
                let explicit = std::env::var("WAYLAND_USER_MODEL_BACKEND").ok();
                let backend_choice: String = match &explicit {
                    Some(v) => v.clone(),
                    None => {
                        if std::env::var("HONCHO_API_KEY").is_ok() {
                            // Honcho key present — select honcho automatically.
                            tracing::info!(
                                target: "wcore_agent::bootstrap",
                                "HONCHO_API_KEY detected; auto-selecting honcho user-model backend"
                            );
                            "honcho".to_string()
                        } else {
                            // No key present — fall back to local and surface
                            // a one-line hint so users can opt in easily.
                            tracing::info!(
                                target: "wcore_agent::bootstrap",
                                "user-model: using local backend \
                                 (set HONCHO_API_KEY or WAYLAND_USER_MODEL_BACKEND=honcho \
                                 for deeper Honcho dialectic user modeling)"
                            );
                            "local".to_string()
                        }
                    }
                };
                if backend_choice == "local" {
                    // Preserve the existing on-disk persistence path for
                    // the default backend — the env-var selector only
                    // matters when an operator opts into Honcho.
                    let path = wcore_memory::paths::auto_memory_dir(cwd_path)
                        .map(|d| d.join("user-model.json"))
                        .unwrap_or_else(|| cwd_path.join(".wayland").join("user-model.json"));
                    match wcore_user_model::LocalBackend::with_persistence(&path) {
                        Ok(b) => Some(std::sync::Arc::new(b)),
                        Err(e) => {
                            tracing::warn!(
                                target: "wcore_agent::bootstrap",
                                error = %e,
                                "user-model local backend init failed; skipping context block"
                            );
                            None
                        }
                    }
                } else {
                    // Honcho (or any future backend) — delegate to the
                    // adapter's env-driven selector.
                    match wcore_honcho_adapter::select_backend_from_env() {
                        Ok(b) => Some(b),
                        Err(e) => {
                            tracing::warn!(
                                target: "wcore_agent::bootstrap",
                                error = %e,
                                backend = %backend_choice,
                                "user-model backend init failed; falling back to local"
                            );
                            // Graceful fallback: construct local backend
                            // rather than leaving the user without any
                            // user-model context.
                            let path = wcore_memory::paths::auto_memory_dir(cwd_path)
                                .map(|d| d.join("user-model.json"))
                                .unwrap_or_else(|| {
                                    cwd_path.join(".wayland").join("user-model.json")
                                });
                            wcore_user_model::LocalBackend::with_persistence(&path)
                                .ok()
                                .map(|b| {
                                    std::sync::Arc::new(b)
                                        as std::sync::Arc<dyn wcore_user_model::UserModelBackend>
                                })
                        }
                    }
                }
            } else {
                None
            };
        let user_ctx_block = if let Some(b) = user_model_backend.as_ref() {
            let brief = b.brief(user_id).await.unwrap_or_default();
            let prefs = b.preferences(user_id).await.unwrap_or_default();
            crate::user_context::render_user_context_block(&brief, &prefs)
        } else {
            None
        };
        let mut system_prompt = system_prompt;
        if let Some(block) = user_ctx_block {
            system_prompt.push_str(&block);
        }
        self.config.system_prompt = Some(system_prompt);

        // W6 — opt the catalog into cross-project skill resolution. The
        // current project's parent directory holds sibling projects; a
        // `resolve()` miss widens to their `.wayland-core/skills/` dirs.
        // Degrades to single-project behaviour when cwd has no parent.
        let mut catalog = wcore_skills::refs::SkillCatalog::from_refs(skill_refs);
        if let Some(siblings_root) = cwd_path.parent() {
            catalog = catalog.with_cross_project_root(siblings_root);
        }
        let catalog = Arc::new(catalog);

        // v0.8.1 U1 — build the per-turn `SkillRouter` now that the
        // catalog is finalised. Seeds layer in priority:
        //   1. GEPA winners from `evolved_prompts` (via
        //      `PromptStore::seed_pairs_for` with scorer="bench") —
        //      strongest prior, capped at 5 simulated successes per
        //      arm. Skipped silently when memory is on the `NullMemory`
        //      fallback (no Db handle captured above) or the table is
        //      empty.
        //   1b. Auto-drafted skills (scorer="auto_drafter"). The U6
        //      `SkillDrafter` records each on-disk draft into
        //      `evolved_prompts` with AUTO_DRAFT_SCORE (0.7 → 4 simulated
        //      successes) precisely so the skill learned in session 1 gets
        //      a router head-start in session 2. Without this pass the
        //      draft lands on disk + in the catalog but the router never
        //      receives its intended prior — the closed-loop weight was
        //      written but never read. `restore_seeds` is idempotent on
        //      names a `bench` GEPA winner already seeded, so proven
        //      winners still outrank fresh auto-drafts.
        //   2. Session-start prioritizer ranking — head-start of
        //      `seed_from_prioritizer` (3 for top quartile, fading to
        //      0 at the tail). `restore_seeds` is idempotent on names
        //      already seeded by GEPA, so this layer only fills the
        //      gaps.
        // Installed on the engine via `set_skill_router` post-
        // construction below.
        let skill_router_to_install: wcore_skills::SkillRouter = {
            let mut sk_router = wcore_skills::SkillRouter::new();
            let candidate_names: Vec<String> = catalog.refs().map(|r| r.name.clone()).collect();
            // Layer 1 — GEPA winners. Requires a real Db handle; the
            // `NullMemory` fallback skips this branch entirely.
            if let Some(db_arc) = mem_db_for_router.clone() {
                let store = wcore_evolve::prompt_store::PromptStore::new(db_arc);
                match store.seed_pairs_for(&candidate_names, "bench", 1) {
                    Ok(pairs) => {
                        let n = sk_router.restore_seeds(pairs);
                        tracing::debug!(
                            target: "wcore_agent::bootstrap",
                            seeded = n,
                            "skill_router: hydrated from GEPA `evolved_prompts` (bench scorer)"
                        );
                    }
                    Err(e) => tracing::warn!(
                        target: "wcore_agent::bootstrap",
                        error = %e,
                        "skill_router: GEPA seed hydration failed (continuing with prioritizer-only seeds)"
                    ),
                }
                // Layer 1b — auto-drafted skills (scorer="auto_drafter").
                // Closes the U6 read-back: the SkillDrafter writes this row
                // in session 1; here in session 2 the router consumes it so
                // the freshly-learned skill is preferred. Idempotent against
                // the `bench` pass above — a real GEPA winner keeps priority.
                match store.seed_pairs_for(&candidate_names, "auto_drafter", 1) {
                    Ok(pairs) => {
                        let n = sk_router.restore_seeds(pairs);
                        tracing::debug!(
                            target: "wcore_agent::bootstrap",
                            seeded = n,
                            "skill_router: hydrated auto-drafted skills (auto_drafter scorer)"
                        );
                    }
                    Err(e) => tracing::warn!(
                        target: "wcore_agent::bootstrap",
                        error = %e,
                        "skill_router: auto-draft seed hydration failed (continuing)"
                    ),
                }
            }
            // Layer 2 — prioritizer-based head-start. Always runs; the
            // call is cheap and idempotent on names already seeded
            // above, so it only credits arms GEPA didn't touch.
            sk_router.seed_from_prioritizer(&candidate_names);
            sk_router
        };

        let skill_checker = wcore_skills::permissions::SkillPermissionChecker::new(
            self.config.tools.skills.deny.clone(),
            self.config.tools.skills.allow.clone(),
            self.config.tools.auto_approve,
        );
        // F-013: capture permission-checker config for the cron skill_sink
        // closure before self.config moves into AgentEngine::new_with_provider
        // below (~line 1147). The cron sink builds a fresh SkillPermissionChecker
        // from these values for each fire — identical policy to the session sink.
        let cron_skill_deny_rules = self.config.tools.skills.deny.clone();
        let cron_skill_allow_rules = self.config.tools.skills.allow.clone();
        let cron_skill_auto_approve = self.config.tools.auto_approve;
        // v0.7.0 1.D.5 — wire ProceduralSkillTelemetrySink when memory
        // is enabled so SkillTool invocations feed the procedural-memory
        // loop (M3.5). Without this, the prior wiring path had a sink
        // trait + impl shipped but no producer — telemetry events were
        // never emitted. Mock memory + skills-lifecycle-only flows fall
        // through to the SkillTool default `NullTelemetrySink`.
        let skill_telemetry_sink: Arc<dyn wcore_skills::telemetry::SkillTelemetrySink> =
            if want_memory {
                Arc::new(wcore_skills::telemetry::ProceduralSkillTelemetrySink::new(
                    Arc::clone(&memory_api),
                ))
            } else {
                Arc::new(wcore_skills::telemetry::NullTelemetrySink)
            };
        registry.register(Box::new(
            crate::skill_tool::SkillTool::new(Arc::clone(&catalog), cwd.to_string(), skill_checker)
                .with_telemetry_sink(skill_telemetry_sink),
        ));

        // T3-3.1.7: SessionSearchTool — cross-session conversation recall via
        // the same `memory_api` handle the engine and SkillPrioritizer use.
        // Always registered: with real memory it searches past episodes; with
        // `NullMemory` (want_memory=false) it returns empty results rather than
        // erroring, so the tool name is always visible to the model.
        registry.register(Box::new(
            wcore_tools::session_search::SessionSearchTool::new(Arc::clone(&memory_api)),
        ));

        // Memory write tools — let the agent deliberately store durable memory
        // (the read side is `session_search` above). Gated on `want_memory`:
        // with `NullMemory` the writes are no-ops, so registering them then
        // would advertise capabilities the model can't actually exercise. When
        // memory is real these pair with the session-start recall + the memory
        // prompt section so the loop (store now → recall next session) closes.
        if want_memory {
            registry.register(Box::new(
                wcore_tools::record_episode::RecordEpisodeTool::new(Arc::clone(&memory_api)),
            ));
            registry.register(Box::new(wcore_tools::assert_fact::AssertFactTool::new(
                Arc::clone(&memory_api),
            )));
        }

        // v0.8.1 U2 — wire the production `AgentBus` so sub-agent
        // lifecycle events have a real channel and a real subscriber.
        // The bus is attached to the spawner via `with_bus(...)`; the
        // observer is spawned below once the engine + its `OutputSink`
        // are available, and its `JoinHandle` is parked on the engine's
        // background-task vec so `Drop for AgentEngine` aborts it on
        // session shutdown.
        let agent_bus = Arc::new(crate::agents::bus::AgentBus::new(256));
        let spawner = Arc::new(
            crate::spawner::AgentSpawner::new(provider.clone(), self.config.clone())
                .with_bus(Arc::clone(&agent_bus)),
        );
        // v0.6.4 Task 1.2/1.7 — share ONE `AgentRegistry` between the
        // `SpawnTool` (so the LLM can spawn plugin-contributed named agents)
        // and the engine (`set_agent_registry`, after construction). The
        // registry is pre-loaded with every plugin `AgentManifest` by
        // `apply_initialize_outcome` above.
        let plugin_agent_registry = Arc::new(applied.agent_registry);
        // #269 — wire FleetDispatcher when the loaded agent registry is
        // large enough that sharding actually helps. Default
        // `Topology::Spawn` (cap 5) and `Topology::Mesh` (cap 50) cover
        // small/medium cases; only flip to `Topology::Fleet` (cap 100,
        // sharded) when the user has more than DEFAULT_SHARD_SIZE
        // registered agents — that's when hierarchical reduction beats
        // a flat fan-out. This is the production trigger for the
        // FleetDispatcher wire path; the `fleet_dispatcher_wired_test.rs`
        // wire-presence test exercises the same code path with a
        // synthetic 11-task SpawnTool invocation.
        let agent_count = plugin_agent_registry.list().len();
        // v0.9.4 W1.2: wire parent_output BEFORE self.output moves into the
        // engine at ~1492. Arc::clone is cheap; the engine holds the primary ref.
        let mut spawn_tool = crate::spawn_tool::SpawnTool::new(Arc::clone(&spawner))
            .with_registry(Arc::clone(&plugin_agent_registry))
            .with_parent_output(Arc::clone(&self.output));
        // v0.9.4 C8: only flip to Fleet when parent_output is NOT wired (Fleet
        // hardcodes channel_sink: None). With parent_output wired, per-task
        // relay handles any fan-out size; Fleet is left for the unmonitored path.
        if agent_count > wcore_swarm::DEFAULT_SHARD_SIZE {
            spawn_tool = spawn_tool.with_topology(wcore_swarm::Topology::Fleet);
        }
        registry.register(Box::new(spawn_tool));
        // B1 — WorkflowTool: LLM-facing dynamic-workflow surface. Sibling to
        // SpawnTool; shares the same `AgentSpawner` (the runner borrows it per
        // call). Registered before `spawner` is moved into DelegateTool below,
        // so clone the Arc here.
        registry.register(Box::new(crate::workflow_tool::WorkflowTool::new(
            Arc::clone(&spawner),
        )));
        // T3-3.1.3: DelegateTool — focused single-task / batch delegation
        // surface ported from wayland-hermes. Sibling to SpawnTool (the
        // existing registry-aware multi-agent fan-out): Delegate provides
        // structured-JSON output + per-task `toolsets` whitelist + max
        // turns 50 default, while Spawn exposes registry-resolved named
        // agents + OutputSink relay. Both share AgentSpawner via the
        // `wcore_types::spawner::Spawner` trait so wcore-tools stays
        // below wcore-agent in the dep graph.
        registry.register(Box::new(wcore_tools::delegate::DelegateTool::new(spawner)));

        let plan_active_flag = Arc::new(AtomicBool::new(false));
        if self.config.plan.enabled {
            registry.register(Box::new(crate::plan::tools::EnterPlanModeTool::new(
                Arc::clone(&plan_active_flag),
            )));
            registry.register(Box::new(crate::plan::tools::ExitPlanModeTool::new(
                Arc::clone(&plan_active_flag),
            )));
        }

        // X1/F13 (Task 8): conditionally register ScriptTool.
        //
        // ScriptTool dispatches against a dedicated mini-registry built
        // from fresh copies of the allow-listed built-ins (no Spawn, no
        // Script, no MCP). This sidesteps the Arc-cycle problem
        // (registry → ScriptTool → dispatcher → registry) while still
        // satisfying §5.4: Script invokes the same `Tool` impls as a
        // direct tool call would. The mini-registry is kept in sync with
        // the main registry by mirroring construction here — if a new
        // allow-listed tool lands, add it to BOTH places (the W4 audit
        // calls this out as a known concern; the integration test in
        // script_e2e.rs verifies the mirror).
        //
        // Pre-decided dispatcher shape (audit HIGH-2): Arc<dyn ToolDispatcher>.
        //
        // W7.1 S4-3.2: build one `ApprovalBridge` here and hand it to both
        // `ScriptTool` (via `.with_approval(...)`) and the engine (via
        // `engine.set_approval_bridge(...)` after construction). The CLI's
        // `ApprovalResume` arm calls `engine.approval_bridge().resolve(...)`
        // on the same instance, unblocking the script step's awaiting
        // future end-to-end.
        let approval_bridge = Arc::new(crate::approval::ApprovalBridge::new());
        // Wave SC SECURITY MAJOR fix: spawn the TTL reaper so abandoned
        // approvals auto-resolve as Cancelled instead of leaking
        // `oneshot::Sender`s + holding the session in Suspend forever.
        // The reaper task lives for the engine's lifetime; tokio aborts
        // it when the runtime shuts down.
        let _reaper_handle = approval_bridge.spawn_reaper(crate::approval::DEFAULT_REAP_INTERVAL);

        // B2.5 — attach the consent doorbell to the process-global egress policy
        // (if one was installed at CLI entry). The policy rings this on an `Ask`
        // verdict (a data-less read to a new domain) to prompt once/always/no
        // through the same approval bridge + output sink the ScriptTool HITL
        // path uses. `installed_policy()` is `None` in tests / headless / when
        // security is off, so this is a cheap no-op there (no allocation, no
        // boot-cost — unlike the policy *install*, which is deliberately kept at
        // CLI entry, not here).
        if let Some(policy) = crate::egress::installed_policy() {
            let doorbell = std::sync::Arc::new(crate::egress::BridgeConsentDoorbell::new(
                approval_bridge.clone(),
                self.output.clone(),
            ));
            policy.set_doorbell(doorbell);
        }

        if self.config.builtin_tools.script.enabled {
            use wcore_tools::dispatcher::ToolDispatcher;

            let dispatch_reg = build_script_dispatcher_registry(
                file_cache_for_script.clone(),
                cwd_path,
                self.config.builtin_tools.repomap.enabled,
            );
            let shared = Arc::new(tokio::sync::RwLock::new(dispatch_reg));
            let dispatcher_handle = Arc::clone(&shared);
            // W8b.2.A — route through the ctx-aware closure shape so
            // ScriptTool sub-steps inherit the parent's ToolContext
            // (vfs, cancel, file_write_notifier). The non-ctx
            // `dispatch` entry point still works via a test_default
            // ctx for any legacy caller.
            let dispatcher: Arc<dyn ToolDispatcher> =
                Arc::new(wcore_tools::dispatcher::ClosureDispatcher::new_with_ctx(
                    Box::new(move |tool, input, ctx| {
                        let reg = Arc::clone(&dispatcher_handle);
                        Box::pin(async move {
                            let guard = reg.read().await;
                            match guard.get(&tool) {
                                Some(t) => t.execute_with_ctx(input, ctx).await,
                                None => wcore_types::tool::ToolResult {
                                    content: format!("tool '{tool}' not in registry"),
                                    is_error: true,
                                },
                            }
                        })
                    }),
                ));
            // W7.1 S4-3.2: wire the shared bridge + an OutputSink adapter into
            // `ScriptTool` so `approval_required: true` steps actually request
            // approval and emit `ApprovalRequired` + `Suspend` over the host
            // stream. The CLI resolves the request via `bridge.resolve(...)`.
            let script_bridge: Arc<dyn wcore_tools::script::ApprovalProducer> =
                approval_bridge.clone();
            let script_sink: Arc<dyn wcore_tools::script::ScriptOutputSink> = Arc::new(
                crate::approval::OutputSinkScriptAdapter::new(self.output.clone()),
            );
            registry.register(Box::new(
                wcore_tools::script::ScriptTool::new(dispatcher)
                    .with_approval(script_bridge, script_sink),
            ));
            // Keep the Arc alive for the lifetime of the engine by leaking
            // it. The dispatcher closure already holds a strong ref; this
            // is only here to make the ownership obvious and to silence
            // any "unused variable" warnings if the closure is the sole
            // owner.
            std::mem::forget(shared);
            self.config.advertised_capabilities.rpc_tool_script = true;
        }

        // W6 F7: any non-None cost row on the active ProviderCompat means we
        // can report at least a per-provider list-price estimate, so advertise
        // cost_attribution to the host. Audit rev-2 finding 5: this is the
        // SINGLE source for the cost gate; ProtocolSink reads advertised
        // directly and emits SessionCost iff the flag is true.
        if self.config.compat.cost_per_input_token.is_some()
            || self.config.compat.cost_per_output_token.is_some()
        {
            self.config.advertised_capabilities.cost_attribution = true;
        }

        // F-092 (W7-N): mirror online_evolution config gate into the
        // advertised capability surface so the host sees it on Ready.
        if self.config.observability.online_evolution {
            self.config.advertised_capabilities.online_evolution = true;
        }

        let tool_defs_snapshot = registry.to_tool_defs();
        registry.register(Box::new(wcore_tools::tool_search::ToolSearchTool::new(
            tool_defs_snapshot,
        )));

        // M3.6.2: memory_api + decay_handle are constructed earlier in this
        // function (before skill_refs) so the SkillPrioritizer can use them.
        // See the M3.6.2 block above. The engine setters below consume the
        // values from that earlier block.

        // W8a A.6: capture the BudgetConfig before self.config is moved
        // into AgentEngine. The BudgetConfig is Clone, so a one-time copy
        // here is cheap; the ExecutionBudgetView is built after the engine
        // is fully wired so plugin/MCP boot-time failures don't allocate
        // a watcher task that would then need teardown. Clone the
        // OutputSink Arc too so the budget watcher's emit callback can
        // hold a handle after `self.output` is moved into the engine.
        let budget_cfg = self.config.budget.clone();
        let sink_for_budget = self.output.clone();

        // M5.bootstrap-wiring — capture session_cap + span_sink BEFORE
        // self.config is moved into AgentEngine so we can install a
        // BudgetTracker after engine construction. None ⇒ skip install
        // and leave engine.budget_tracker = None (pre-M5.3 behaviour).
        let session_cap_cfg = self.config.session_cap.clone();
        let span_sink_for_budget = self.span_sink.clone();

        // v0.8.1 U5 — open the credentials store BEFORE self.config is
        // moved into AgentEngine so the channel auto-registration block
        // below can hand the same store to every adapter's factory.
        // Failure here means we skip channel auto-registration; the
        // engine itself starts fine.
        let channel_credentials: Option<
            std::sync::Arc<dyn wcore_config::credentials::CredentialsStore>,
        > = match self.config.open_credentials_store() {
            Ok(store) => Some(std::sync::Arc::from(store)),
            Err(e) => {
                tracing::warn!(
                    target: "wcore_agent::bootstrap",
                    error = %e,
                    "credentials store open failed; channel auto-registration will be skipped"
                );
                None
            }
        };

        let mut engine = if let Some(session) = self.resume_session {
            AgentEngine::resume_with_provider(
                provider.clone(),
                self.config,
                registry,
                self.output,
                session,
            )
        } else {
            AgentEngine::new_with_provider(provider.clone(), self.config, registry, self.output)
        };
        engine.set_plan_active_flag(plan_active_flag);
        // Token-opt (diff-resend): give the engine the shared file cache so it
        // can bump the compaction generation after each compaction pass,
        // invalidating read bases the model can no longer see.
        if let Some(fc) = file_cache_for_engine {
            engine.set_file_cache(fc);
        }
        // Wave 6A.1 — hand the on-disk plugin runtime keepalives to the
        // engine so they outlive the registered tool closures.
        engine.set_plugin_runtime_handles(plugin_runtime_keepalives);
        // v0.6.4 Task 1.2/1.7 — install the shared plugin `AgentRegistry`
        // (the same instance the `SpawnTool` was built with) so the engine
        // and the spawner resolve plugin agents identically.
        engine.set_agent_registry(plugin_agent_registry);
        // v0.6.4 Task 1.3/1.7 — forward plugin-contributed hooks into the
        // engine's `HookEngine` (constructed inside `new_with_provider` /
        // `resume_with_provider`, so this must happen post-construction).
        engine.register_plugin_hooks(applied.plugin_hooks);
        // C1 / Task A1 — wire the host hook dispatcher built above (gated by
        // `config.hooks.dispatch_enabled`). `None` ⇒ plugin hooks stay
        // log-only. Set after `register_plugin_hooks` so the engine already
        // knows which hooks the dispatcher will be asked to resolve.
        if let Some(dispatcher) = hook_dispatcher {
            engine.set_hook_dispatcher(dispatcher);
        }
        // v0.6.5 Wave 6A.2 — install plugin-reified user-model backends.
        // The session-end PUM path mirrors every inferred delta to each
        // backend (e.g. Honcho via `learn_preference`) in addition to the
        // local `MemoryApi::update_user_model` write. Empty slice is a
        // no-op — byte-identical to pre-6A.2 behaviour when no plugin
        // reified a user-model.
        engine.set_plugin_user_models(applied.plugin_reified_user_models);
        // W7 Pre-flight 0.0b: install the MemoryApi handle resolved above.
        engine.set_memory_api(memory_api);
        // v0.8.0 N.3 — hand the resolved skill catalog to the engine so the
        // `/skill` slash-command handler's Runtime variant observes the
        // same `Arc<SkillCatalog>` the `SkillTool` was constructed with.
        engine.set_skill_catalog(Arc::clone(&catalog));
        // v0.8.1 U1 — install the per-turn `SkillRouter` built above.
        // From this point `engine.run()` calls `choose()` against the
        // catalog every turn and `observe()` credits the pick on exit.
        // Closes the v0.7.0/v0.8.0 dead-chain where `SkillRouter` +
        // `restore_seeds` + `PromptStore::seed_pairs_for` shipped without
        // a production caller.
        engine.set_skill_router(skill_router_to_install);
        // F-024 (HIGH, Aud-4): install the TemplateRouter so Thompson-sampled
        // learned orchestration actually runs. Previously `set_template_router`
        // had zero production callers — every turn fell through to the
        // deterministic IntentClassifier. The router is default-initialised
        // (all five Template arms, random seed). Seeding from PromptStore
        // (parity with SkillRouter GEPA path) is deferred: the TemplateRouter
        // arms are Template variants, not skill names, so the seed_pairs_for
        // schema would need extending. Default Thompson arms are fine for v0.8.2.
        engine.set_template_router(std::sync::Arc::new(std::sync::Mutex::new(
            wcore_dispatch::TemplateRouter::default(),
        )));
        tracing::debug!(
            target: "wcore_agent::bootstrap",
            "F-024: TemplateRouter installed (Thompson-sampled orchestration active)"
        );
        // v0.8.1 U6 — install the autonomous `SkillDrafter`. After N=3
        // consecutive successful turns on the same task signature,
        // `engine::observe_auto_skill` writes a candidate skill to
        // `$WAYLAND_HOME/skills/auto/` and records into GEPA's
        // `PromptStore` so the next session's U1 `SkillRouter` hydrates
        // the new skill as a seed pair. Only installed when a real
        // `Db` is available — without one we have no PromptStore and the
        // closed-loop seed pathway is dead. The bucketer itself is always
        // live on the engine; without a drafter it just observes.
        if let Some(db_arc) = mem_db_for_router.clone() {
            // `$WAYLAND_HOME` resolution: prefer the explicit env var,
            // fall back to `~/.wayland`. Matches the pattern used elsewhere
            // in the project for user-facing on-disk artifacts.
            let wayland_home = std::env::var("WAYLAND_HOME")
                .map(std::path::PathBuf::from)
                .ok()
                .or_else(|| dirs::home_dir().map(|h| h.join(".wayland")))
                .unwrap_or_else(|| std::path::PathBuf::from(".wayland"));
            let skill_dir = wayland_home.join("skills").join("auto");
            let store = Arc::new(wcore_evolve::prompt_store::PromptStore::new(db_arc));
            let drafter = Arc::new(crate::auto_skill::SkillDrafter::new(skill_dir, Some(store)));
            engine.set_skill_drafter(drafter);
            tracing::debug!(
                target: "wcore_agent::bootstrap",
                "v0.8.1 U6: SkillDrafter installed; auto-skill loop active"
            );
        }
        // v0.8.0 Task M — install the `UserModelBackend` on the engine
        // so every `run()` invocation derives a style fingerprint from
        // the user input and calls `backend.observe(user_id, …)`. The
        // bootstrap path read the brief ONCE above; this setter closes
        // the loop so the backend keeps learning each turn. The engine
        // and bootstrap share the same `Arc<dyn UserModelBackend>` —
        // observations land in the store the next bootstrap reads from.
        if let Some(backend) = user_model_backend {
            engine.set_user_model_backend(backend);
        }
        // M3.2: hand the decay-scheduler handle (if any) to the engine so
        // its `Drop` impl aborts the task on shutdown. No-op when memory
        // is disabled.
        if let Some(handle) = decay_handle {
            engine.push_decay_handle(handle);
        }

        // v0.8.1 U2 — spawn the production subscriber for `AgentBus`
        // lifecycle events. Forwards every `Spawned` / `FirstMessage` /
        // `Completed` / `Errored` to tracing + the engine's
        // `OutputSink::emit_info` so JSON-stream protocol clients see
        // sub-agent lifecycle on the same channel as everything else.
        // Park the `JoinHandle` on the engine's `decay_handles` vec so
        // `Drop for AgentEngine` aborts it on session shutdown.
        let bus_observer =
            crate::agents::AgentBusObserver::spawn(Arc::clone(&agent_bus), sink_for_budget.clone());
        engine.push_decay_handle(bus_observer.into_join_handle());

        // W8b.2.B Task 7: mount a FileWatcher on the session root for
        // external-edit detection (D.3). The watcher is best-effort —
        // platforms that lack a recommended notify backend (or where
        // arming fails for any reason) degrade silently so sessions
        // boot regardless. The watcher + FileWatcherNotifier adapter
        // are stored on the engine (accessible via
        // `engine.current_tool_context()`), and the per-turn drain
        // in `engine.rs::run` consumes external events for D.3 injection.
        // The orchestration dispatcher at `orchestration/mod.rs:398`
        // still mints its per-call `ToolContext` via `test_default()`;
        // threading `tool_write_notifier` into that production dispatch
        // path is the W8b.2.B.1 chain edge (alongside the NodeExecutor
        // adapter for Task 5).
        // v0.8.1 — `FileWatcher::new` arms the OS file-notification backend
        // (FSEvents on macOS); on a host whose `fseventsd` is backed up the
        // arming call can block for tens of seconds. Phase 0 "eventual
        // install": the watcher arms on a detached `std::thread` the runtime
        // never joins (the hang-guard against a wedged backend), and installs
        // itself plus the paired notifier into the engine's interior-mutable
        // slots whenever that thread finishes — see
        // `AgentEngine::install_file_watcher_eventually`. There is no grace
        // window and nothing is ever built-then-dropped, so a contended host
        // can no longer silently lose external-edit tracking by missing a
        // timing budget; a genuinely wedged backend simply never installs
        // (the same best-effort contract this block always had).
        engine.install_file_watcher_eventually(cwd_path.to_path_buf());
        // F-039 (HIGH, Aud-10): wire SkillWatcher hot-reload into bootstrap.
        // Previously `wcore_skills::watcher::SkillWatcher` shipped with zero
        // production callers — skills added mid-session were invisible until
        // the next boot. The watcher monitors the same dirs `load_catalog`
        // reads from (user + project + extra); on any change it reloads the
        // catalog and installs it on the engine.
        //
        // Best-effort: if the watcher can't arm (FSEvents degraded, no dirs,
        // etc.) the session continues without hot-reload — same contract as
        // FileWatcher above. The watcher's JoinHandle is parked on the
        // engine's decay handles so it's aborted on session shutdown.
        {
            let skill_dirs: Vec<std::path::PathBuf> = {
                let mut dirs = Vec::new();
                if let Some(d) = wcore_skills::paths::user_skills_dir()
                    && d.is_dir()
                {
                    dirs.push(d);
                }
                for d in wcore_skills::paths::project_skills_dirs(cwd_path) {
                    if d.is_dir() {
                        dirs.push(d);
                    }
                }
                for d in &self.extra_skill_dirs {
                    if d.is_dir() {
                        dirs.push(d.clone());
                    }
                }
                dirs
            };

            match wcore_skills::watcher::SkillWatcher::new() {
                Ok((mut skill_watcher, mut version_rx)) => {
                    if let Err(e) = skill_watcher.start(skill_dirs) {
                        tracing::warn!(
                            error = %e,
                            "F-039: SkillWatcher::start failed; continuing without skill hot-reload"
                        );
                    } else {
                        let catalog_for_reload = Arc::clone(&catalog);
                        let engine_catalog_setter = {
                            // We cannot hand an `&mut AgentEngine` into the
                            // spawn closure, but `set_skill_catalog` takes an
                            // `Arc<SkillCatalog>` and the engine is `!Send`.
                            // The watcher fires on a tokio task on the same
                            // thread; we deliver the reload via a one-shot
                            // channel that the session main-loop drains.
                            // For now, log the version bump. Full in-session
                            // reload requires a reload_tx channel threaded into
                            // the orchestration loop (future W3-G coordination).
                            // TODO(W3-B-follow-on): thread reload_tx into
                            // engine so set_skill_catalog is called mid-session.
                            Arc::clone(&catalog_for_reload)
                        };
                        let reload_handle = tokio::spawn(async move {
                            while version_rx.changed().await.is_ok() {
                                let version = *version_rx.borrow();
                                tracing::info!(
                                    target: "wcore_agent::bootstrap",
                                    version,
                                    "F-039: skill catalog changed (version={version}); \
                                     reload will apply on next session start \
                                     (in-session hot-swap: TODO W3-B-follow-on)"
                                );
                                let _ = &engine_catalog_setter; // keep Arc alive
                            }
                        });
                        // Park watcher + reload task so Drop shuts them down.
                        // The watcher itself is kept alive by holding it in a
                        // Box that we park via push_decay_handle on a
                        // synthetic task that never resolves (the watcher's
                        // own tokio task does the real work via version_rx).
                        engine.push_decay_handle(reload_handle);
                        // The SkillWatcher must stay alive; its Drop calls
                        // stop() which aborts the OS watcher. We keep it alive
                        // by leaking it into a Box held by the engine via a
                        // dedicated boxed-handle approach.
                        engine.push_decay_handle(tokio::spawn(async move {
                            // Hold skill_watcher alive for the session.
                            let _watcher = skill_watcher;
                            // Park here forever; aborted by engine Drop.
                            std::future::pending::<()>().await;
                        }));
                        tracing::debug!(
                            target: "wcore_agent::bootstrap",
                            "F-039: SkillWatcher armed (skill hot-reload active)"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "F-039: SkillWatcher::new failed; continuing without skill hot-reload"
                    );
                }
            }
        }

        // W7.1 S4-3.2: install the same `ApprovalBridge` the ScriptTool was
        // wired with so `engine.approval_bridge()` and the registered
        // ScriptTool share one instance. Without this, the CLI's
        // `ApprovalResume` arm would resolve on a different (empty) bridge
        // and never unblock the awaiting script step.
        engine.set_approval_bridge(approval_bridge);

        // M5.bootstrap-wiring — install a per-session `BudgetTracker`
        // when `Config.session_cap` is set. When the bootstrap also has
        // a `SpanSink` installed, wire `ObservabilityBudgetEventBridge`
        // so `BudgetEvent::{Charge, CapWarn, CapBlock}` reach the JSON
        // span channel. Without `session_cap`, the engine's
        // `budget_tracker` stays `None` and the per-turn charge call
        // in `engine.rs::run` is a no-op (matches pre-M5.3 behaviour).
        if let Some(cap_cfg) = session_cap_cfg.as_ref() {
            let cap: wcore_budget::BudgetCap = cap_cfg.into();
            let mut tracker = wcore_budget::BudgetTracker::new(cap);
            if let Some(sink) = span_sink_for_budget.as_ref() {
                let bridge = Arc::new(
                    wcore_observability::sink::ObservabilityBudgetEventBridge::new(sink.clone()),
                );
                tracker.set_event_sink(bridge);
            }
            engine.set_budget_tracker(Arc::new(parking_lot::Mutex::new(tracker)));
        }

        // W8a A.6/A.7: build the session-root ExecutionBudgetView from
        // config and pair it with a cancellation token. The
        // `budget_linked_with_callback` form additionally emits
        // `BudgetExceeded` over the protocol sink the instant the first
        // cap trips — singular per session, host-tolerated per audit F5.
        // Default-config sessions have every cap = None and the
        // watcher's callback never fires.
        let exec_budget: ExecutionBudget = (&budget_cfg).into();
        let budget = exec_budget.start_root();
        let cancel_root =
            budget_linked_with_callback(CancellationToken::new(), budget.clone(), move |payload| {
                sink_for_budget.emit_budget_exceeded(
                    &payload.reason,
                    &payload.observed,
                    &payload.limit,
                );
            });

        // F-014 (CRIT, Aud-4/Aud-11): construct ChannelManager, auto-register
        // adapters, call start_all to spawn per-channel inbound poll tasks, and
        // lift the manager to Arc<Mutex<ChannelManager>> so the cron handler
        // (and any future inbound-event subscriber in the CLI) can hold a clone.
        //
        // Previously bootstrap stopped at `auto_register_from_user_config` and
        // never called `start_all`, so inbound messages were never polled on
        // any configured channel.
        //
        // start_all is idempotent (already-started channels skip re-start).
        // Errors from individual channel start() calls are surfaced as
        // ChannelError and bubble up; non-fatal channels that fail independently
        // already return Ok from their start() impl — this is the same contract
        // as the channels crate manager tests.
        let mut channel_manager_inner = wcore_channels::ChannelManager::new();
        let channels_auto_registered = if let Some(creds) = channel_credentials {
            match wcore_channels_registry::auto_register_from_user_config(
                &mut channel_manager_inner,
                creds,
            )
            .await
            {
                Ok(count) => {
                    tracing::info!(
                        target: "wcore_agent::bootstrap",
                        count,
                        "F-014: channels auto-registered from ~/.wayland/channels"
                    );
                    count
                }
                Err(e) => {
                    tracing::warn!(
                        target: "wcore_agent::bootstrap",
                        error = %e,
                        "F-014: channel auto-register failed; continuing with empty ChannelManager"
                    );
                    0
                }
            }
        } else {
            0
        };
        // Call start_all to arm inbound poll tasks. Best-effort: if start_all
        // returns an error we warn and continue (session still works, channels
        // just won't deliver inbound messages).
        if let Err(e) = channel_manager_inner.start_all().await {
            tracing::warn!(
                target: "wcore_agent::bootstrap",
                error = %e,
                "F-014: channel_manager.start_all() failed; inbound polling may be partial"
            );
        } else {
            tracing::info!(
                target: "wcore_agent::bootstrap",
                "F-014: channel_manager.start_all() complete — inbound polling active"
            );
        }
        // Lift to Arc<tokio::sync::Mutex<>> so the cron handler and future
        // CLI inbound subscribers can hold a clone across async boundaries.
        // tokio Mutex is required because ChannelManager::send_to is async
        // and the guard must survive an await point in the cron channel arm.
        let channel_manager: std::sync::Arc<tokio::sync::Mutex<wcore_channels::ChannelManager>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(channel_manager_inner));

        // FleetDispatcher-class fix (audit 2026-05-24): SendMessageTool was
        // registered above with the boot-default `NullMessageTransport` so
        // its schema reaches the LLM from the start of the session. Now that
        // `channel_manager` is lifted to `Arc<Mutex<>>`, replace the tool's
        // transport with the real `ChannelManagerTransport` adapter so the
        // LLM's `send_message` calls route through user-configured channels
        // (Telegram/Discord/Slack/Email/etc.) instead of returning the Null
        // transport's loud "no transport configured" error on every call.
        if let Some(reg) = engine.registry_mut() {
            let transport =
                std::sync::Arc::new(crate::channel_send_transport::ChannelManagerTransport::new(
                    std::sync::Arc::clone(&channel_manager),
                ));
            reg.replace_by_name(Box::new(wcore_tools::send_message::SendMessageTool::new(
                transport,
            )));
        } else {
            tracing::warn!(
                target: "wcore_agent::bootstrap",
                "send_message transport upgrade skipped: engine.registry_mut() \
                 returned None (a stale Arc clone of the tools registry is held \
                 somewhere). send_message will continue to use NullMessageTransport \
                 and fail loudly on every call."
            );
        }

        // v0.8.1 U7 — spawn the cron runner. Errors resolving the
        // default store path (no $HOME, no $WAYLAND_HOME) are non-fatal:
        // session boot continues without a runner.
        //
        // F-013 fix (CRIT, Aud-4/Aud-10/Aud-11): wire a real skill_sink so
        // --skill cron targets actually invoke the skill. The sink builds a
        // transient SkillTool from the session's catalog + a fresh permission
        // checker and calls SkillTool::execute on it directly — no engine
        // session required, no LLM spend, just skill body execution.
        //
        // F-014 follow-up (W2-K comment seam): replace the None channel_sink
        // placeholder with a real sink now that channel_manager is lifted to
        // Arc<Mutex<ChannelManager>>. The sink acquires the lock, looks up the
        // channel by name, and calls send() on it.
        //
        // slash_sink: deferred to a follow-up (TRIAGE.md F-013 slash arm).
        // Cross-session dispatcher problem — slash commands need an active
        // engine session; None here means slash cron fires log+stage
        // as before.
        let cron_skill_sink: crate::cron::SkillSink = {
            let catalog_for_cron = Arc::clone(&catalog);
            let deny_rules = cron_skill_deny_rules;
            let allow_rules = cron_skill_allow_rules;
            let auto_approve = cron_skill_auto_approve;
            let cwd_for_cron = cwd.to_string();
            Arc::new(move |skill_name: String, args: serde_json::Value| {
                let catalog = Arc::clone(&catalog_for_cron);
                let checker = wcore_skills::permissions::SkillPermissionChecker::new(
                    deny_rules.clone(),
                    allow_rules.clone(),
                    auto_approve,
                );
                let cwd = cwd_for_cron.clone();
                Box::pin(async move {
                    // Aud-12 / M-18 (+ B8 follow-up): the cron runner's
                    // pre-dispatch `scan_target` only inspects the Skill
                    // target's name + raw args. The text that actually executes
                    // unattended is the skill BODY (`!shell:` directives run via
                    // sh -c) AFTER argument substitution. A benign-looking `args`
                    // value can splice a denylisted payload into a `!shell:`
                    // body line that only becomes dangerous post-substitution.
                    //
                    // Scan the EXACT post-substitution string the shell will
                    // receive: `render_shell_input` is the same function
                    // `prepare_inline_content` (inside `SkillTool::execute`)
                    // runs to compose the shell input, so the scanned bytes are
                    // byte-identical to the executed bytes. The sink builds a
                    // `SkillTool::new` (session_id = None) and passes `args`
                    // through unchanged as the tool's `args` param, whose
                    // `as_str()` is what the executor reads — so we mirror both
                    // here. `resolve` hits the catalog LRU that
                    // `SkillTool::execute` reuses, so this is not a second disk
                    // read in the common case.
                    if let Ok(skill) = catalog.resolve(&skill_name).await {
                        let args_str = args.as_str();
                        let composed =
                            wcore_skills::executor::render_shell_input(&skill, args_str, None);
                        // Cheap raw-args scan retained: catches payloads that
                        // never reach a `!shell:` line (e.g. injected into a
                        // non-shell body region) but are still attacker-supplied.
                        let raw_args = serde_json::to_string(&args).unwrap_or_default();
                        for chunk in [composed.as_str(), raw_args.as_str()] {
                            if let Some(reason) = wcore_cron::runner::scan_target_text(chunk) {
                                tracing::warn!(
                                    target: "wcore_agent::cron",
                                    skill = %skill_name,
                                    reason = %reason,
                                    "cron skill blocked: substituted body/args failed \
                                     execution-boundary scan"
                                );
                                return Err(format!(
                                    "cron skill '{skill_name}' blocked before dispatch: {reason}"
                                ));
                            }
                        }
                    }
                    let tool = crate::skill_tool::SkillTool::new(catalog, cwd, checker);
                    let input = serde_json::json!({ "skill": skill_name, "args": args });
                    let result = wcore_tools::Tool::execute(&tool, input).await;
                    if result.is_error {
                        Err(result.content)
                    } else {
                        Ok(())
                    }
                })
            })
        };
        let cron_runner = match wcore_cron::FileCronStore::from_default_path() {
            Ok(store) => {
                let store: std::sync::Arc<dyn wcore_cron::CronStore> = std::sync::Arc::new(store);
                let handler: std::sync::Arc<dyn wcore_cron::JobHandler> =
                    std::sync::Arc::new(crate::cron::EngineJobHandler::new(
                        // channel_sink: F-014 follow-up (W2-K seam) — now that
                        // channel_manager is Arc<tokio::sync::Mutex<ChannelManager>>
                        // (same type as EngineJobHandler::channels), we can pass
                        // the clone directly. The handler's Channel arm calls
                        // send_to() on the locked manager.
                        Some(std::sync::Arc::clone(&channel_manager)),
                        // slash_sink: deferred — cross-session dispatcher (F-013 slash arm)
                        None,
                        // skill_sink: wired — F-013 fix (skill arm)
                        Some(cron_skill_sink),
                    ));
                // F-065: use spawn_with_history so every fire is recorded
                // in history.jsonl (parallel to jobs.json). The history
                // file is the backing store for `cron history` and
                // `cron logs` subcommands.
                match wcore_cron::default_history_path() {
                    Some(hp) => Some(wcore_cron::CronRunner::spawn_with_history(
                        store,
                        handler,
                        wcore_cron::runner::TICK_INTERVAL,
                        hp,
                    )),
                    None => Some(wcore_cron::CronRunner::spawn(
                        store,
                        handler,
                        wcore_cron::runner::TICK_INTERVAL,
                    )),
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "wcore_agent::bootstrap",
                    error = %e,
                    "cron runner not started — store path could not be resolved"
                );
                None
            }
        };

        Ok(BootstrapResult {
            engine,
            provider,
            mcp_managers,
            has_mcp,
            has_plugins,
            plugin_capabilities,
            loaded_plugin_names,
            budget,
            cancel_root,
            channel_manager,
            channels_auto_registered,
            cron_runner,
        })
    }
}

impl AgentBootstrap {
    /// W7 Pre-flight 0.0d: synchronous fixture builder for tests.
    ///
    /// Constructs an `AgentEngine` wired with:
    /// - a `ScriptedProvider` that replays the supplied event script,
    /// - a `TestSink` whose buffer is exposed via
    ///   `engine.captured_protocol_events()`,
    /// - a `NullMemory` `MemoryApi` (in-memory, no disk I/O),
    /// - a `ToolRegistry` containing **only** the read-only built-ins
    ///   (Read, Grep, Glob) — no Bash, no Spawn, no MCP, no plugins.
    ///
    /// Suitable for unit/integration tests that need to drive the engine
    /// turn loop end-to-end without touching the filesystem or network.
    /// Returns the engine plus the test sink handle so callers can also
    /// pull captured events outside the engine if desired.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn build_for_test(
        config: Config,
        script: Vec<wcore_types::llm::LlmEvent>,
    ) -> (AgentEngine, crate::test_utils::TestSinkHandle) {
        use std::sync::Arc as StdArc;

        let sink = crate::test_utils::TestSink::new();
        let handle = sink.handle();
        let sink_arc: StdArc<dyn OutputSink> = StdArc::new(sink);

        let provider: StdArc<dyn LlmProvider> =
            StdArc::new(crate::test_utils::ScriptedProvider::new(script));

        // Read-only built-ins only. Skip Bash (spawns processes),
        // Spawn (recursive engine), Script (depends on dispatcher),
        // and MCP (network). Read/Grep/Glob are safe in-process.
        let mut registry = wcore_tools::registry::ToolRegistry::new();
        registry.register(Box::new(wcore_tools::read::ReadTool::new(None)));
        registry.register(Box::new(wcore_tools::grep::GrepTool));
        registry.register(Box::new(wcore_tools::glob::GlobTool));

        let mut engine = AgentEngine::new_with_provider(provider, config, registry, sink_arc);
        engine.set_test_sink_handle(handle.clone());
        (engine, handle)
    }
}

/// F13 (Task 8): build a parallel mini-registry for ScriptTool to dispatch
/// against. Mirrors the built-ins registered in `build()` minus SpawnTool,
/// MCP, plan-mode helpers, ToolSearch, and Script itself. The mini-registry
/// shares the FileStateCache so file ops stay coherent with direct tool calls.
///
/// **Sync rule:** any new tool added to `ScriptTool::ALLOW_LIST` must be
/// added BOTH to the main registry's built-in block AND to this helper.
fn build_script_dispatcher_registry(
    file_cache: Option<std::sync::Arc<std::sync::RwLock<wcore_tools::file_cache::FileStateCache>>>,
    cwd: &std::path::Path,
    include_repomap: bool,
) -> wcore_tools::registry::ToolRegistry {
    let mut reg = wcore_tools::registry::ToolRegistry::new();
    reg.register(Box::new(wcore_tools::read::ReadTool::new(
        file_cache.clone(),
    )));
    reg.register(Box::new(wcore_tools::write::WriteTool::new(
        file_cache.clone(),
    )));
    reg.register(Box::new(wcore_tools::edit::EditTool::new(file_cache)));
    reg.register(Box::new(wcore_tools::bash::BashTool));
    reg.register(Box::new(wcore_tools::grep::GrepTool));
    reg.register(Box::new(wcore_tools::glob::GlobTool));
    if include_repomap {
        reg.register(Box::new(wcore_tools::repomap::RepoMapTool::new(
            cwd.to_path_buf(),
        )));
    }
    reg
}
