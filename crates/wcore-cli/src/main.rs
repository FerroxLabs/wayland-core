use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

use clap::{Parser, Subcommand, ValueEnum};
// `doctor` lives in the `wcore_cli` lib so the TUI diagnostics surface can
// share it; the binary re-imports it here for the `--doctor` CLI flag.
use wcore_cli::budget_grants::BudgetGrantLedger;
use wcore_cli::doctor;
// B3: the exit-code contract. `ShutdownSignal` names which signal ended the
// process so the code can be 128+N instead of a blanket SUCCESS.
use wcore_cli::exit_code::ShutdownSignal;
use wcore_cli::log_rotate;
use wcore_cli::packaged_runtime::{
    LocalExecutionSelection, audit_unix_time_millis, resolve_local_execution,
};
use wcore_cli::runtime_diagnostics::RuntimeDiagnosticsState;
// Wave OL: typed import — `OllamaProvider` is downcast from
// `Arc<dyn PluginProvider>` in `make_plugin_provider_router` below to
// route `--model ollama:*` through the wayland-ollama plugin. The
// `inventory::submit!` factory still fires at static init for plugin
// discovery just like the other `as _` re-exports do.
use wayland_ollama::OllamaProvider;

use wcore_agent::bootstrap::{AgentBootstrap, PluginProviderRouter};
use wcore_agent::late_mcp::LateMcpBinder;
use wcore_agent::mcp_lifecycle::{
    McpConfigIdentity, McpConnectionReservation, McpLifecycleCatalog, McpLifecycleState,
    McpReservationOutcome,
};
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_agent::session;
use wcore_agent::slash::{Dispatcher as SlashDispatcher, SlashError, SlashOutcome};
use wcore_config::config::{self, CliArgs, Config, McpServerConfig, TransportType};
use wcore_mcp::manager::{McpManager, McpServerHealth};
use wcore_mcp::tool_proxy::register_single_server_tools;
use wcore_protocol::commands::{
    MCP_LIFECYCLE_VERSION, ProtocolCommand, RemoveMcpServerCommand, ResumeTurnAction,
};
use wcore_protocol::events::{
    BudgetGrantRefusalReason, FinishReason, GrantRefusalReason, GrantSurface, McpRemovalOutcome,
    ProtocolEvent, RecoveryLifecycle, RecoveryReconcileReason, RecoveryUnavailableReason,
};
use wcore_protocol::execution_policy::{
    ExecutionPolicyChangeReason, ExecutionPolicySequence, ExecutionPolicySequenceError,
    ExecutionPolicySnapshot,
};
use wcore_protocol::reader::spawn_stdin_reader;
use wcore_protocol::writer::{ProtocolEmitter, ProtocolWriter};
use wcore_protocol::{ToolApprovalManager, ToolApprovalResult};
use wcore_providers::LlmProvider;
use wcore_skills::refs::SkillRef;
use wcore_types::execution_policy::{ApprovalPolicy, DEFAULT_DANGEROUS_SESSION_TTL_SECS};

// v0.8.0 N.1+N.2+N.3 — slash-runtime dispatch helpers.
//
// The slash dispatcher is constructed once per session via
// `build_slash_dispatcher`, then driven for every user-input line via
// `handle_slash_or_run`. Slash commands short-circuit; non-slash input
// flows through to `engine.run()`.

/// Outcome of a single user-input line that may or may not be a slash command.
enum SlashOrRun {
    /// Recognised slash command — handled in-process, no engine call needed.
    Slash,
    /// `/exit` (or another Exit-returning handler) was dispatched; caller
    /// should break out of its loop or return from main.
    Exit,
    /// Not a slash command — `engine.run()` was invoked. Carries the engine's
    /// result so the caller can render the streamed output the same way it
    /// did before the slash layer was inserted.
    Engine(Result<wcore_agent::engine::AgentResult, wcore_agent::engine::AgentError>),
}

/// Construct the per-session slash dispatcher with Runtime-variant handlers
/// reaching the engine's wired-up MemoryApi, plugin runtime handles, and
/// SkillCatalog. When the engine doesn't yet carry a catalog (cold start
/// before bootstrap finishes), the skill handler falls back to its Stub
/// variant — that's the documented `with_runtime(.., None)` behaviour.
fn build_slash_dispatcher(engine: &wcore_agent::engine::AgentEngine) -> SlashDispatcher {
    let memory_api = engine.memory_api().clone();
    let plugin_handles = engine.plugin_runtime_handles_arc();
    let skill_catalog = engine.skill_catalog().cloned();
    let mut dispatcher = SlashDispatcher::with_runtime(memory_api, plugin_handles, skill_catalog);
    // 23B-C3: register `/usermodel` only when bootstrap actually opened a
    // correction store, so the command is absent rather than present-and-inert.
    if let Some((store, user_id)) = engine.user_correction_store() {
        let mut handler =
            wcore_agent::slash::usermodel::UserModelHandler::new(store.clone(), user_id);
        if let Some(backend) = engine.user_model_backend() {
            handler = handler.with_backend(backend.clone());
        }
        dispatcher.register(std::sync::Arc::new(handler));
    }
    dispatcher
}

/// Pre-process one input line through the slash dispatcher, falling through
/// to `engine.run()` when the line is not a known slash command. Handler
/// output is emitted via the `OutputSink`'s info channel so it threads
/// through both terminal and protocol sinks uniformly.
async fn handle_slash_or_run(
    dispatcher: &SlashDispatcher,
    engine: &mut wcore_agent::engine::AgentEngine,
    input: &str,
    msg_id: &str,
    output: &dyn OutputSink,
) -> SlashOrRun {
    if let Some(inv) = wcore_agent::slash::parse(input) {
        match dispatcher.try_dispatch(&inv) {
            Ok(SlashOutcome::Handled { output: Some(text) }) => {
                output.emit_info(&text);
                return SlashOrRun::Slash;
            }
            Ok(SlashOutcome::Handled { output: None }) => {
                return SlashOrRun::Slash;
            }
            Ok(SlashOutcome::SetStyle(directive)) => {
                engine.inject_history(directive);
                output.emit_info("style updated");
                return SlashOrRun::Slash;
            }
            Ok(SlashOutcome::ClearConversation) => {
                engine.clear_conversation();
                // ED+H: clear the scrollback and home the cursor.
                output.emit_info("\x1b[2J\x1b[H(conversation cleared)");
                return SlashOrRun::Slash;
            }
            Ok(SlashOutcome::NotImplemented { message }) => {
                output.emit_info(&message);
                return SlashOrRun::Slash;
            }
            Ok(SlashOutcome::Exit) => {
                return SlashOrRun::Exit;
            }
            Err(SlashError::Unknown(_)) => {
                // Not a registered slash command — fall through to engine.
            }
            Err(SlashError::Bad(reason)) => {
                output.emit_error(
                    &format!("bad slash invocation: {reason}"),
                    false,
                    wcore_protocol::events::FailureCategory::LocalWayland,
                );
                return SlashOrRun::Slash;
            }
        }
    }
    SlashOrRun::Engine(engine.run(input, msg_id).await)
}

/// Wave OL: plugin-provider router. Detects model strings that begin with
/// `ollama:` and downcasts the loaded `wayland-ollama` plugin's
/// `Arc<dyn PluginProvider>` to the concrete `OllamaProvider`, returning
/// it as the engine's `Arc<dyn LlmProvider>`.
///
/// Lives here (not in `wcore-agent`) because `wcore-agent` deliberately
/// doesn't depend on `wayland-ollama` — plugin crates flow from binary
/// into the engine via inventory, not via direct dep edges. The
/// `wcore-cli` binary is the one place that links both `wayland-ollama`
/// AND `wcore-providers`, so this is where the downcast must live.
///
/// Returning `None` lets `AgentBootstrap` fall back to the built-in
/// `wcore_providers::create_provider(&config)` path.
fn make_plugin_provider_router() -> PluginProviderRouter {
    Box::new(
        |model: &str,
         providers: &[Arc<dyn wcore_plugin_api::registry::providers::PluginProvider>]|
         -> Option<Arc<dyn LlmProvider>> {
            if !model.starts_with("ollama:") {
                return None;
            }
            let plugin_provider = providers.iter().find(|p| p.provider_name() == "ollama")?;
            // Downcast through `as_any` to recover the concrete plugin type.
            // The `Arc<dyn PluginProvider>` wraps a value of type
            // `OllamaProvider` (registered by `wayland_ollama::WaylandOllama`
            // in `Plugin::initialize`), so `downcast_ref` succeeds in the
            // happy path.
            let _ollama_ref: &OllamaProvider =
                plugin_provider.as_any().downcast_ref::<OllamaProvider>()?;
            // We can't move out of the Arc<dyn PluginProvider> to get an
            // Arc<OllamaProvider>, and `Arc::downcast` requires `Arc<dyn Any>`.
            // Construct a fresh `OllamaProvider` with the same defaults and
            // hand THAT out as the LlmProvider — for now we just clone the
            // configuration from the plugin's registered instance. (The
            // plugin-side instance was constructed in `Plugin::initialize`
            // with a hardcoded base URL + model; the route honours
            // `--model ollama:<name>` via the prefix-strip inside
            // `OllamaProvider::stream`, and OLLAMA_BASE_URL env override
            // can re-target the endpoint.)
            //
            // Long-term: switch `PluginProvider::as_any` to
            // `as_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>`
            // so we can `Arc::downcast` directly without re-construction.
            // For the v0.2.x ship this approach is sufficient: the
            // OllamaProvider is stateless modulo its reqwest::Client, so
            // re-constructing is cheap and observationally equivalent.
            let base_url = std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434/api/chat".to_string());
            // Strip the `ollama:` prefix so the bare model name reaches Ollama.
            let model_name = model.strip_prefix("ollama:").unwrap_or(model).to_string();
            Some(Arc::new(OllamaProvider::new(base_url, model_name)))
        },
    )
}

/// Rank 47: apply the `--no-memory` flag to a resolved [`Config`].
///
/// One-directional, mirroring `--online-evolution`: when `no_memory` is set
/// the run becomes stateless (`memory.enabled = false`); when it is unset the
/// config's own `memory.enabled` (file/default-driven) is left untouched, so
/// the flag can only turn memory off, never on. Called from `main` after the
/// config is fully resolved and before it reaches `AgentBootstrap`.
fn apply_no_memory_flag(config: &mut Config, no_memory: bool) {
    if no_memory {
        config.memory.enabled = false;
    }
}

/// Consume an evaluator-supplied credential file exactly once. The path may
/// appear in argv; the credential itself never does, and the file is removed
/// before provider bootstrap can spawn hooks, tools, plugins, or MCP servers.
fn read_one_use_api_key(path: &Path) -> anyhow::Result<String> {
    const MAX_CREDENTIAL_BYTES: u64 = 16 * 1024;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("--api-key-file must name a regular non-symlink file");
    }
    if metadata.len() == 0 || metadata.len() > MAX_CREDENTIAL_BYTES {
        let _ = std::fs::remove_file(path);
        anyhow::bail!("--api-key-file must contain 1..={MAX_CREDENTIAL_BYTES} bytes");
    }
    let read = std::fs::read(path);
    let removed = std::fs::remove_file(path);
    let bytes = read.map_err(|error| anyhow::anyhow!("read --api-key-file: {error}"))?;
    removed.map_err(|error| anyhow::anyhow!("remove --api-key-file: {error}"))?;
    String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("--api-key-file was not valid UTF-8"))
}

fn read_one_use_eval_egress_key(path: &Path) -> anyhow::Result<ed25519_dalek::SigningKey> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != 32 {
        let _ = std::fs::remove_file(path);
        anyhow::bail!("--eval-egress-key-file must contain exactly 32 bytes in a regular file");
    }
    let read = std::fs::read(path);
    let removed = std::fs::remove_file(path);
    let mut bytes =
        read.map_err(|error| anyhow::anyhow!("read --eval-egress-key-file: {error}"))?;
    if bytes.len() != 32 {
        bytes.fill(0);
        removed.map_err(|error| anyhow::anyhow!("remove --eval-egress-key-file: {error}"))?;
        anyhow::bail!("--eval-egress-key-file must contain exactly 32 bytes");
    }
    let mut seed = [0_u8; 32];
    seed.copy_from_slice(&bytes);
    bytes.fill(0);
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    seed.fill(0);
    removed.map_err(|error| anyhow::anyhow!("remove --eval-egress-key-file: {error}"))?;
    Ok(key)
}

#[derive(Parser)]
#[command(
    name = "wayland-core",
    about = "A multi-provider AI agent CLI with tool orchestration support",
    version
)]
struct Cli {
    /// Provider: "anthropic" or "openai"
    #[arg(short, long, env = "PROVIDER")]
    provider: Option<String>,

    // #685 — deliberately NOT `env = "API_KEY"`. Clap's env fallback made the
    // bare, provider-agnostic `API_KEY` the TOP rung of the credential ladder,
    // outranking the config file and the credentials store, and it did so
    // invisibly: the ladder's own gate in `resolve_api_key_from_env` was never
    // reached because the value had already arrived as a "CLI" argument. The
    // variable is still honoured — at the env rung, behind
    // `WAYLAND_ALLOW_BARE_API_KEY`, which is the order `docs/getting-started.md`
    // has always documented. A `///` here would print all of this in `--help`.
    /// API key
    #[arg(short = 'k', long)]
    api_key: Option<String>,

    /// Internal one-use credential transport for isolated hosts/evaluators.
    #[arg(long, hide = true, value_name = "PATH", conflicts_with = "api_key")]
    api_key_file: Option<PathBuf>,

    /// Internal one-use evaluator evidence signing key transport.
    #[arg(long, hide = true, value_name = "PATH")]
    eval_egress_key_file: Option<PathBuf>,

    /// Base URL for the API
    #[arg(short, long, env = "BASE_URL")]
    base_url: Option<String>,

    /// Model name
    #[arg(short, long, env = "MODEL")]
    model: Option<String>,

    /// Built-in agent persona to inherit (e.g. `architect`, `debugger`).
    /// Loads system_prompt + max_turns from the bundled agent pack
    /// unless an explicit `--system-prompt` / `--max-turns` is also set.
    /// Run `wayland-core --list-agents` to see all built-ins.
    #[arg(long, value_name = "NAME")]
    agent: Option<String>,

    /// The host's active assistant identity, for per-assistant MCP scoping.
    /// A config MCP server marked `only_for_assistant` is injected only when
    /// this matches its allow-list (fail-closed otherwise). Distinct from
    /// `--agent` (persona/system-prompt); the desktop host sets this when
    /// spawning the json-stream engine.
    #[arg(long, value_name = "NAME", env = "WAYLAND_ASSISTANT")]
    assistant: Option<String>,

    /// List built-in agent personas and exit.
    #[arg(long)]
    list_agents: bool,

    /// Max output tokens per response
    #[arg(long)]
    max_tokens: Option<u32>,

    /// Max agent loop turns
    #[arg(long)]
    max_turns: Option<usize>,

    /// Custom system prompt
    #[arg(long)]
    system_prompt: Option<String>,

    /// Named profile from config file
    #[arg(long)]
    profile: Option<String>,

    /// Auto-approve all tool executions (skip confirmation)
    #[arg(long)]
    auto_approve: bool,

    /// TIER 1 — bypass approval prompts ONLY. Every tool call is approved
    /// without asking and the OS sandbox STAYS ON. `--force` and `--yolo` are
    /// aliases of this one flag, so they are the same tier by construction and
    /// cannot drift apart. Only use for trusted, scripted runs: there is NO
    /// interactive permission gate once this is set. The TUI surfaces a
    /// `· FORCE` badge in the bottom status bar so the mode is impossible to
    /// forget. To bypass the sandbox as well, use the tier-2 superset
    /// `--dangerously-skip-permissions-and-sandbox`.
    #[arg(
        long = "dangerously-skip-permissions",
        visible_aliases = ["force", "yolo"],
        conflicts_with = "dangerous"
    )]
    dangerously_skip_permissions: bool,

    /// TIER 2, a superset of tier 1 — bypass approvals AND the OS sandbox
    /// until a time-bounded lease expires. Cannot be activated by config,
    /// environment, protocol, ACP, TUI commands, resumed state, or child
    /// agents: argv on a local launch is the only provenance that mints the
    /// lease. `--dangerous` is still accepted as a DEPRECATED alias.
    #[arg(
        long = "dangerously-skip-permissions-and-sandbox",
        visible_alias = "dangerous",
        conflicts_with = "dangerously_skip_permissions"
    )]
    dangerous: bool,

    /// Lease lifetime in seconds for
    /// `--dangerously-skip-permissions-and-sandbox` (maximum one hour).
    #[arg(long, value_name = "SECONDS", requires = "dangerous")]
    dangerous_ttl_secs: Option<u64>,

    /// Project directory to load .wayland-core.toml from (defaults to CWD)
    #[arg(long)]
    project_dir: Option<std::path::PathBuf>,

    /// Trust the current repository's executable configuration fingerprint in
    /// Core's external trust store, then start the session. Material changes
    /// to hooks, MCP config or project skills automatically revoke eligibility.
    #[arg(long, conflicts_with = "untrust_workspace")]
    trust_workspace: bool,

    /// Remove the current repository from Core's external trust store, then
    /// start with the strict untrusted-workspace profile.
    #[arg(long, conflicts_with = "trust_workspace")]
    untrust_workspace: bool,

    /// Permit this JSON-stream host to approve read-only, process-lifetime
    /// developer capabilities. This launch opt-in does not permit writes,
    /// untrusted/remote grants, or sandbox bypass.
    #[arg(long, requires = "json_stream")]
    allow_host_workspace_grants: bool,

    /// Permit this JSON-stream host to grant read-only access to folders
    /// outside the workspace ("always allow this folder").
    ///
    /// A separate opt-in from `--allow-host-workspace-grants` because it
    /// answers a different question: that one widens what the session may
    /// RUN, this one widens what it may READ. A launcher entitled to one is
    /// not automatically entitled to the other.
    ///
    /// Deliberately a launch flag and not an environment variable. An env var
    /// set once on every spawn cannot express "this session may, that one may
    /// not"; a flag can, and it fails closed when absent.
    #[arg(long, requires = "json_stream")]
    allow_host_path_grants: bool,

    /// Permit this local JSON-stream host to grant additional provider spend
    /// after Core has emitted a budget-exceeded receipt. Default-deny; managed
    /// sessions still refuse grants.
    #[arg(long, requires = "json_stream")]
    allow_host_budget_grants: bool,

    /// Resume a previous session
    #[arg(long)]
    resume: Option<String>,

    /// Resume the most-recent session (the latest by creation time).
    /// A convenience shortcut for `--resume <latest-id>` — mutually
    /// exclusive with `--resume` and `--session-id`.
    #[arg(short = 'c', long = "continue", conflicts_with_all = ["resume", "session_id"])]
    continue_latest: bool,

    /// Use a specific session ID (instead of auto-generating one)
    #[arg(long)]
    session_id: Option<String>,

    /// List saved sessions
    #[arg(long)]
    list_sessions: bool,

    /// Disable colored output
    #[arg(long)]
    no_color: bool,

    /// Enable JSON streaming mode for host client integration
    #[arg(long)]
    json_stream: bool,

    /// Host-supplied engine-mode evidence for local runtime diagnostics.
    #[arg(long, value_enum, requires = "json_stream", hide = true)]
    runtime_engine_mode: Option<RuntimeEngineModeArg>,

    /// Host-supplied workspace-role evidence for local runtime diagnostics.
    #[arg(long, value_enum, requires = "json_stream", hide = true)]
    runtime_workspace_kind: Option<RuntimeWorkspaceKindArg>,

    /// Disable the ratatui TUI — fall back to the line-based REPL even
    /// on an interactive terminal. The TUI is the default for
    /// `wayland-core` on a TTY with no prompt; this is the escape hatch
    /// for users who prefer the bare REPL (or for terminals it cannot
    /// drive). `--json-stream` and `-p`/headless modes are unaffected.
    #[arg(long)]
    no_tui: bool,

    /// Generate a default config file
    #[arg(long)]
    init_config: bool,

    /// Print config file path and exit
    #[arg(long)]
    config_path: bool,

    /// Print build provenance (version + embedded source git SHA) and exit.
    /// Catches the stale-build class: the SHA must match the HEAD the binary
    /// was compiled from. Used by the build-provenance integration test.
    #[arg(long)]
    build_info: bool,

    /// Print skill directory paths and exit
    #[arg(long)]
    skills_path: bool,

    /// Run the system-dependency doctor. Probes external binaries
    /// (`wlrctl`, `grim`, `chromium`, `ollama`), environment signals
    /// (`WAYLAND_DISPLAY`, `DISPLAY`, `BROWSERBASE_API_KEY`,
    /// `OLLAMA_BASE_URL`), and surfaces missing dependencies with
    /// per-distro install hints. Exit code `1` if any required check
    /// fails on the current platform, otherwise `0`.
    #[arg(long)]
    doctor: bool,

    /// When running --doctor, actually CONNECT-TEST each declared MCP
    /// server (spawns stdio commands / dials URLs) instead of only listing
    /// them. Off by default so bare --doctor stays side-effect-free.
    #[arg(long, requires = "doctor")]
    probe_mcp: bool,

    // FerroxLabs/wayland#1079: without this the doctor can say WHICH key an
    // invocation selected but not whether it works, so "let me pass the key
    // explicitly to rule it out" had no answer. Kept as a plain comment, NOT
    // a doc comment: `///` here becomes user-facing `--help` text, and
    // `help_no_internal_ids` rejects issue numbers there — correctly.
    /// When running --doctor, actually AUTHENTICATE the resolved credential
    /// against the provider's key-validation endpoint, instead of only
    /// reporting that one resolved. One read-only request; it spends no
    /// tokens and never prints the key.
    ///
    /// Off by default, symmetric with --probe-mcp, so bare --doctor stays
    /// side-effect-free and never makes an authenticated network call the
    /// user did not ask for. The request goes to the provider's own
    /// endpoint, so it does not cover a proxy set with --base-url; the
    /// doctor says so when the two differ.
    #[arg(long, requires = "doctor")]
    probe_provider: bool,

    /// macOS only: ask the OS to show the TCC consent prompts that
    /// computer-use needs (Accessibility, Screen Recording), then print
    /// the resulting state.
    ///
    /// This is the ONLY path that raises a system dialog. `--doctor`
    /// and every agent run use the non-prompting probe instead, so a
    /// user is never surprised by a consent sheet mid-task. Off macOS
    /// it prints that there is nothing to grant and exits 0.
    #[arg(long)]
    request_permissions: bool,

    /// Run the skills audit. Writes JSON to .wayland-core/skills-audit.json
    /// and renders Markdown to stdout.
    #[arg(long)]
    skills_audit: bool,

    /// Override the staleness threshold (days) used by --skills-audit.
    // F-072: `requires` ensures clap rejects --skills-audit-stale-days
    // when --skills-audit is absent, matching --replay-diff behaviour.
    #[arg(long, default_value_t = 180, requires = "skills_audit")]
    skills_audit_stale_days: u64,

    /// Promote a drafted skill from `Staged` to `Active`.
    ///
    /// Accepts a skill NAME or a procedure UUID. The UUID form is what
    /// anyone who scripted the historical flag passes; the name form is
    /// what `--skills-govern` prints. Reads and writes this project's memory
    /// DB (`wcore_memory::paths::project_db_path`). Promotion is governed:
    /// the grant is bound to a content digest, revoked artifacts are refused,
    /// and every outcome is journalled.
    #[arg(long, value_name = "SKILL_OR_PROCEDURE_ID")]
    skills_promote: Option<String>,

    /// Archive a drafted skill. Accepts either a `Staged` or an `Active`
    /// row — `Staged → Archived` is allowed directly, so losing drafts can
    /// be dismissed without a detour through Active. Pinned rows are NOT
    /// archivable from the CLI: promote then archive, or unpin them through
    /// the curator UI first.
    #[arg(long, value_name = "PROCEDURE_ID")]
    skills_archive: Option<String>,

    // ---- governed skill lifecycle (one contiguous additive block) ----
    /// Revoke an installed skill. Retains every byte first, then removes it,
    /// then suppresses re-drafting, so the auto-draft loop cannot silently
    /// recreate what you removed. Undo with `--skills-rollback`.
    #[arg(long, value_name = "SKILL")]
    skills_revoke: Option<String>,

    /// Restore a revoked skill byte for byte and clear its suppression. The
    /// argument is the revocation id printed by `--skills-revoke` and listed
    /// by `--skills-govern`.
    #[arg(long, value_name = "REVOCATION_ID")]
    skills_rollback: Option<String>,

    /// List installed skills with their promotion status, every revocation in
    /// force, and the append-only governance journal.
    #[arg(long)]
    skills_govern: bool,

    /// Dump the memory state for a given session id. Prints all episodes
    /// scoped to that session at the session+project tiers, plus all
    /// project-tier facts and procedures. Intended for human inspection; the
    /// format is a plain text table (not JSON) and may change between
    /// releases. Exits 0 even if the session has no recorded data so scripts
    /// can probe state without try/catch.
    #[arg(long, value_name = "SESSION_ID")]
    memory_show: Option<String>,

    /// Replay a session trace JSON file. Validates the schema and the
    /// version-skew guard (refuses traces recorded by a different
    /// wcore-core build unless --replay-force-version-skew is set).
    /// Prints the event count for the session. Combine with
    /// --replay-diff to surface the first divergence against another
    /// trace.
    #[arg(long, value_name = "TRACE_PATH")]
    replay: Option<std::path::PathBuf>,

    /// Compare the trace passed to --replay against this second trace and
    /// print the changed/added/removed entries.
    #[arg(long, value_name = "OTHER_TRACE_PATH", requires = "replay")]
    replay_diff: Option<std::path::PathBuf>,

    /// Skip the wcore-version guard in --replay (use only when inspecting
    /// traces from another release on purpose).
    #[arg(long, requires = "replay")]
    replay_force_version_skew: bool,

    /// Output compaction level: off, safe (default), full
    #[arg(long)]
    compaction: Option<String>,

    /// Enable TOON encoding for JSON arrays (session-level, cannot change mid-conversation)
    #[arg(long)]
    toon: bool,

    /// Enable live online evolution. At session-end the engine emits one
    /// `evolution_event` and applies the Paraphrase mutator to successful
    /// trajectories (≥50% of turns had tool calls). Evolved system-prompt
    /// variants are persisted to `$WAYLAND_HOME/evolved/`. Equivalent to
    /// `[observability] online_evolution = true` in config.
    #[arg(long)]
    online_evolution: bool,

    /// Run a stateless session: disable long-term memory for this run.
    /// Sets `memory.enabled = false` before the engine boots, so no
    /// MemoryManager is created — GEPA, SkillRouter seeds, SkillDrafter,
    /// and user-model write-back are all inert. Equivalent to
    /// `[memory] enabled = false` in wcore.toml, but scoped to this
    /// invocation only. Merge is one-directional: the flag can only turn
    /// memory off, never on.
    #[arg(long)]
    no_memory: bool,

    /// FluxRouter web_search grounding: attach a server-side
    /// `web_search` tool to every turn so Flux grounds the answer via
    /// Perplexity Sonar and renders citations. Only fires when the active
    /// model is a Flux tier alias (`flux-auto` / `flux-fast` / `flux-standard`
    /// / `flux-reasoning`) — a no-op on other models. Paid-only on Flux.
    #[arg(long)]
    search: bool,

    /// Initial prompt (if omitted, enters interactive REPL mode)
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,

    /// Optional subcommand. When present this short-circuits the agent/REPL
    /// path and runs the subcommand dispatcher instead. Kept optional so every
    /// existing flag-driven invocation (`wayland-core --doctor`,
    /// `wayland-core "prompt"`, REPL, json-stream) keeps working unchanged.
    #[command(subcommand)]
    command: Option<TopCmd>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RuntimeEngineModeArg {
    Standard,
    Raw,
}

fn runtime_engine_mode(
    value: Option<RuntimeEngineModeArg>,
) -> wcore_protocol::diagnostics::RuntimeEngineMode {
    match value {
        Some(RuntimeEngineModeArg::Standard) => {
            wcore_protocol::diagnostics::RuntimeEngineMode::Standard
        }
        Some(RuntimeEngineModeArg::Raw) => wcore_protocol::diagnostics::RuntimeEngineMode::Raw,
        None => wcore_protocol::diagnostics::RuntimeEngineMode::Unknown,
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RuntimeWorkspaceKindArg {
    None,
    Project,
    Temporary,
    ProfileHome,
}

fn runtime_workspace_kind(
    value: Option<RuntimeWorkspaceKindArg>,
) -> wcore_protocol::diagnostics::RuntimeWorkspaceKind {
    match value {
        Some(RuntimeWorkspaceKindArg::None) => {
            wcore_protocol::diagnostics::RuntimeWorkspaceKind::None
        }
        Some(RuntimeWorkspaceKindArg::Project) => {
            wcore_protocol::diagnostics::RuntimeWorkspaceKind::Project
        }
        Some(RuntimeWorkspaceKindArg::Temporary) => {
            wcore_protocol::diagnostics::RuntimeWorkspaceKind::Temporary
        }
        Some(RuntimeWorkspaceKindArg::ProfileHome) => {
            wcore_protocol::diagnostics::RuntimeWorkspaceKind::ProfileHome
        }
        None => wcore_protocol::diagnostics::RuntimeWorkspaceKind::Unknown,
    }
}

/// M5.4: top-level subcommands. We add new subcommands here as the CLI
/// grows.
#[derive(Subcommand)]
enum TopCmd {
    /// Browse the bundled model catalog.
    Models {
        #[command(subcommand)]
        cmd: ModelsCmd,
    },
    /// Manage installed plugins (install / list / available / remove).
    Plugin(wcore_cli::plugin::PluginArgs),
    /// Serve the engine's tool registry as an MCP server (stdio or SSE
    /// transport), so external MCP clients such as Claude Desktop or mcp-cli
    /// can call wayland-core's tools.
    McpServe(wcore_cli::mcp_serve::McpServeArgs),
    /// Dispatch a worktree-isolated worker swarm.
    Swarm(wcore_cli::swarm::SwarmArgs),
    /// Operate on saved sessions — list, search, show, checkpoint, rewind,
    /// retry, fork, export, retain, reconcile and cancel. Every operation
    /// prints a machine-readable `F23_SESSION=` token to STDOUT and uses the
    /// exit-code map documented in `session_cmd`.
    Session(wcore_cli::session_cmd::SessionArgs),
    /// Manage the persistent repository index — `build`, `status`, `search`
    /// and `verify`. Every verb prints greppable `F23_INDEX=` lines to STDOUT;
    /// `verify` exits 6 when the store disagrees with the working tree.
    Index(wcore_cli::index_cmd::IndexArgs),
    /// Inspect the cache + compaction ledger — `report`, `list`, `show` and
    /// `verify` over what the prompt cache and the compactor actually did.
    /// Every verb prints greppable `F23_CACHE=` lines; `verify` exits 7 when
    /// the session's cost is not fully priced (the USD figure is then a floor,
    /// not spend) and 8 when there is no ledger to check.
    Cache(wcore_cli::cache_cmd::CacheArgs),
    /// ForgeFlows: validate / list / run saved `.ron` workflows from
    /// `.wayland/workflows/`.
    #[command(visible_alias = "forgeflows")]
    Workflow {
        #[command(subcommand)]
        cmd: wcore_cli::workflow::WorkflowCmd,
    },
    /// Crucible (Mixture-of-Providers): run the cross-provider council over a
    /// task. N proposers (each pinned to its own provider from `[providers]`)
    /// answer in parallel; a fenced, read-only aggregator fuses them. Requires
    /// `[crucible] enabled = true` with a `proposers` roster in your config.
    Crucible {
        /// The task for the council to work.
        task: String,
        /// Gate the council: a cheap classifier decides whether the task
        /// warrants convening (high-stakes / complex) or can be answered with a
        /// single direct call (trivial). Without this flag the council always
        /// convenes.
        #[arg(long)]
        auto: bool,
        /// Auto mode: pin the candidate pool to these specs (comma-separated).
        #[arg(long, value_delimiter = ',')]
        council: Vec<String>,
        /// Auto mode: pin the aggregator to this spec.
        #[arg(long)]
        judge: Option<String>,
        /// Auto mode: force a single direct answer.
        #[arg(long)]
        direct: bool,
        /// Auto mode: force convening a council regardless of the gate.
        #[arg(long)]
        force_council: bool,
        /// Auto mode: treat the task as High stakes (widest roster, top judge).
        #[arg(long)]
        deep: bool,
        /// Auto mode: exclude these provider families (comma-separated).
        #[arg(long, value_delimiter = ',')]
        deny: Vec<String>,
        /// Inject the council synthesis into the normal trusted agent loop as
        /// private guidance (the agent then reasons/acts/uses tools on it),
        /// instead of printing the fused answer and stopping. Overrides config.
        #[arg(long)]
        advisor: bool,
        /// Force terminal (print-and-stop) mode, overriding `[crucible].mode`.
        #[arg(long)]
        terminal: bool,
    },
    /// Anvil (gated forge) — forge a candidate that passes a REAL executable
    /// gate (tests / build / lint), then stamp a verified receipt. Requires
    /// ON by default; `[anvil] enabled = false` is the kill-switch. Empty gate config auto-detects the workspace suite.
    Forge(wcore_cli::anvil::ForgeArgs),
    /// Print resolved project context from WAYLAND.md / AGENTS.md /
    /// .wayland/context.md / CLAUDE.md, walking up from the current directory.
    ProjectContext,
    /// Scaffold .wayland/config.toml + WAYLAND.md in the current directory.
    Init(wcore_cli::init::InitArgs),
    /// ACP server + client surface. `acp serve` binds the HTTP/SSE transport;
    /// `acp request` drives a one-shot session/message round-trip.
    Acp(wcore_cli::acp::AcpArgs),
    /// Manage user-defined agents (create, list, show, edit, delete).
    /// Built-ins from the bundled pack are read-only.
    Agent {
        #[command(subcommand)]
        cmd: wcore_cli::agent_cmd::AgentCmd,
    },
    /// Manage scheduled cron jobs (add / list / remove / enable / disable).
    /// Persists to `$WAYLAND_HOME/cron/jobs.json`; the background runner
    /// spawned at session boot picks up changes on its next tick.
    Cron {
        #[command(subcommand)]
        cmd: wcore_cli::cron::CronCmd,
    },
    /// Update wayland-core to the latest signed release from
    /// `FerroxLabs/wayland-core`. Verifies the `.sig` artifact against the
    /// pinned marketplace pubkey (ed25519) before atomic swap. Use
    /// `--check-only` to print versions without installing.
    SelfUpdate {
        /// Print current vs. latest version and exit without installing.
        #[arg(long)]
        check_only: bool,
    },
    /// CLI surface: launch the TUI on the Onboarding (connect/configure)
    /// surface regardless of whether a config already exists. Onboarding
    /// handles an existing config gracefully via an Overwrite/Keep
    /// choice. The plain `wayland-core` launch only opens Onboarding on a
    /// true first run; `setup` is the explicit re-entry point.
    Setup,
    /// CLI surface: manage provider API keys (list / add / remove)
    /// directly in the global `config.toml` — the lightweight
    /// alternative to the full onboarding flow.
    Auth {
        #[command(subcommand)]
        cmd: wcore_cli::auth::AuthCmd,
    },
    /// FluxRouter image generation (`POST /v1/images/generations`).
    /// Writes the decoded image to `--out` (or stdout when piped). A
    /// free / paid-but-uncleared Flux key returns a `premium_locked`
    /// message — image generation is a paid-only capability.
    Image(wcore_cli::image::ImageArgs),
    /// FluxRouter web_fetch (`POST /v1/fetch`): fetch a URL and print it
    /// as markdown. `--render` selects the JS-rendered premium arm. A
    /// free / paid-but-uncleared Flux key returns an `upgrade_required`
    /// message — web_fetch is a paid-only capability.
    Fetch(wcore_cli::fetch::FetchArgs),
    /// Manage execution backends — list / probe / run / cancel / orphans /
    /// receipt verify / diff across local, container, ssh and cloud.
    Backend(wcore_cli::backend::BackendArgs),
    /// Manage the persistent gateway runtime — install / uninstall / start /
    /// stop / restart / status / drain, plus the `run` verb every generated
    /// launchd, systemd and scheduled-task unit invokes.
    Gateway(wcore_cli::gateway::GatewayArgs),
    /// Manage paired nodes — pair / list / show / probe / revoke / submit /
    /// attribution across machines that host execution backends.
    Node(wcore_cli::node::NodeArgs),
    /// Manage durable Goals and their Fleet task ledger — open / task / run /
    /// status / exec-task. `run` recovers a killed Goal, revokes expired claim
    /// leases, drains completions the dead parent never observed, and drives the
    /// remaining tasks through the Fleet dispatcher.
    Goal(wcore_cli::goal_cmd::GoalArgs),
    /// Manage channel adapters — list / probe / health / reload. `probe` asks
    /// the platform and needs no gateway; `health` reports only what a RUNNING
    /// gateway has observed and refuses otherwise.
    Channel(wcore_cli::channel::ChannelArgs),
    /// Manage isolated profiles — each is an independent `WAYLAND_HOME`-rooted
    /// home with its own config, credentials, memory, and skills.
    Profile {
        #[command(subcommand)]
        cmd: wcore_cli::profile::ProfileCmd,
    },
    /// Import an existing agent setup (Hermes) into wayland-core profiles.
    Migrate {
        #[command(subcommand)]
        cmd: wcore_cli::migrate::MigrateCmd,
    },
    /// Archive, verify, restore and recover a Wayland home.
    Backup {
        #[command(subcommand)]
        cmd: wcore_cli::backup::BackupCmd,
    },
    /// Inspect platform containment — `status` reports the selected sandbox
    /// backend and its properties; `exec` runs a command through the agent's
    /// own shell tool so you can observe, from the child's own output, what
    /// the sandbox actually applied rather than merely that it was available.
    ///
    /// The properties differ by platform and some of them are `false`. Read
    /// `confines_filesystem` for "can a command write outside my workspace" —
    /// it is `false` on the Windows default, where a Job Object bounds process
    /// lifetime but does not filter the filesystem.
    Sandbox(wcore_cli::sandbox_cmd::SandboxArgs),
}

/// `models` sub-subcommands.
#[derive(Subcommand)]
enum ModelsCmd {
    /// List known models from the bundled pricing catalog.
    /// Prints `provider/model_id` one per line. When `--provider` is
    /// omitted the full catalog across every built-in provider is shown.
    List {
        /// Filter to a specific provider (e.g. `openai`, `anthropic`).
        #[arg(long, value_name = "PROVIDER")]
        provider: Option<String>,
    },
}

/// F-089: print known models from the bundled pricing catalog.
/// When `provider` is Some, only that provider's models are shown.
/// Format: `provider/model_id` one per line, sorted alphabetically.
fn print_known_models(provider: Option<&str>) {
    let catalog = match wcore_pricing::PricingCatalog::load_default() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("models: failed to load pricing catalog: {e}");
            return;
        }
    };

    let mut lines: Vec<String> = catalog
        .providers
        .iter()
        .filter(|(prov, _)| provider.is_none_or(|p| prov.eq_ignore_ascii_case(p)))
        .flat_map(|(prov, models)| models.keys().map(move |model| format!("{prov}/{model}")))
        .collect();

    if lines.is_empty() {
        if let Some(p) = provider {
            eprintln!("models: no known models for provider '{p}'");
        }
        return;
    }

    lines.sort_unstable();
    for line in lines {
        println!("{line}");
    }
}

/// v0.9.1 W2 cycle-2 HIGH 2: bind the tracing log file for append.
/// Lives under `$WAYLAND_HOME/logs/wayland-core.log`, with `~/.wayland/logs/`
/// as the platform default. Any error is surfaced to the caller, which prints
/// [`log_rotate::LOG_FALLBACK_NOTICE`] and falls back to stderr (better than
/// no traces at all).
///
/// #932: neither the directory nor the file is created here. This runs BEFORE
/// the subcommand short-circuit, so `$WAYLAND_HOME` may well be the directory
/// the subcommand is about to refuse to touch — see [`log_rotate::RotatingLog`]
/// for the two measured failures that caused. The file appears with the first
/// record.
///
/// The writer is size-bounded — see [`log_rotate`]. It was not, and on a
/// gateway host that is a file which grows for as long as the host runs.
fn open_tui_log_file() -> std::io::Result<log_rotate::RotatingLog> {
    let base = if let Some(home) = std::env::var_os("WAYLAND_HOME") {
        std::path::PathBuf::from(home)
    } else if let Some(home) = std::env::var_os("HOME") {
        std::path::PathBuf::from(home).join(".wayland")
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no $WAYLAND_HOME or $HOME for log file",
        ));
    };
    log_rotate::RotatingLog::open(
        base.join("logs").join("wayland-core.log"),
        log_rotate::MAX_LOG_BYTES,
    )
}

/// 3A / D3 fail-closed guard for `--json-stream` host mode.
///
/// A desktop host spawns `wayland-core --json-stream` and may drive a
/// multi-profile UI. If it signals profile intent (`--profile`) but does NOT
/// materialize the isolated home (`WAYLAND_HOME` unset — meaning
/// [`wcore_config::profile::activate_for_launch`] could not resolve the profile
/// to an existing home), refusing is the only safe choice: silently falling
/// through to the SHARED default home would cross-write another account's
/// credentials and memory — the credential/memory corruption bug reproduced at the host
/// boundary. Interactive CLI/TUI use is intentionally tolerant (it warns and
/// falls through); only the host protocol is held to the strict contract.
///
/// Returns the loud error string when the run must be refused, else `Ok(())`.
fn json_stream_profile_guard(
    json_stream: bool,
    profile: Option<&str>,
    wayland_home_set: bool,
) -> Result<(), String> {
    if json_stream && profile.is_some() && !wayland_home_set {
        return Err(format!(
            "refusing to start --json-stream with --profile {:?} but no WAYLAND_HOME \
             set. A host that drives profiles must set WAYLAND_HOME to the profile's \
             isolated home before spawning the engine; otherwise the engine would \
             write the shared default home and cross-write another profile's \
             credentials and memory. Create the home with `wayland-core profile \
             create`, or set WAYLAND_HOME per spawn.",
            profile.unwrap_or("")
        ));
    }
    Ok(())
}

/// Own the process-level bundled reference tree outside the cancellable root
/// future. On signal shutdown, `run_until_shutdown` first drops that future
/// (releasing session/catalog handles), then this guard removes the exact root
/// while the entry thread unwinds normally.
struct BundledSkillTmpCleanup;

impl Drop for BundledSkillTmpCleanup {
    fn drop(&mut self) {
        wcore_skills::bundled::cleanup_bundled_skill_extract_dir();
    }
}

/// Await a shutdown signal and report WHICH one arrived.
///
/// B3: the caller needs the identity, not just the fact — an interrupted run
/// must exit 130 and a terminated one 143, the codes a shell already
/// understands. Returning `()` is what forced the old `Ok(ExitCode::SUCCESS)`
/// below: with no signal to name, the only honest code was the wrong one.
async fn shutdown_signal() -> ShutdownSignal {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler install");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler install");
        let mut hup = signal(SignalKind::hangup()).expect("SIGHUP handler install");
        tokio::select! {
            _ = term.recv() => ShutdownSignal::Terminate,
            _ = int.recv()  => ShutdownSignal::Interrupt,
            _ = hup.recv()  => ShutdownSignal::Hangup,
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("Ctrl+C handler install");
        ShutdownSignal::Interrupt
    }
}

async fn run_until_shutdown<R, S>(run_future: R, signal_future: S) -> anyhow::Result<ExitCode>
where
    R: std::future::Future<Output = anyhow::Result<ExitCode>>,
    S: std::future::Future<Output = ShutdownSignal>,
{
    let mut run_future = Box::pin(run_future);
    tokio::select! {
        result = &mut run_future => result,
        signal = signal_future => {
            // Explicitly drop all bootstrap/session state before the outer
            // cleanup guard runs. This also releases every Windows capability
            // handle clone so the no-delete process root can be removed.
            drop(run_future);
            wcore_cli::profile_router::reap_all_children_blocking();
            // B3: an interrupted run did NOT succeed. This used to return
            // `ExitCode::SUCCESS`, so `kill -INT` mid-tool produced empty
            // output and exit 0 — a caller checking `$?` could not tell a
            // cancelled run from a completed one. 128+signal is the shell
            // convention every other Unix program already follows.
            Ok(ExitCode::from(signal.exit_code()))
        }
    }
}

fn main() -> anyhow::Result<ExitCode> {
    // Resolve the active isolated profile ONCE, here at process entry, and
    // materialize it into WAYLAND_HOME (C2). This MUST precede
    // load_wayland_env_file() below — that reads $WAYLAND_HOME/.env, so the home
    // must be settled first — and runs while main() is still single-threaded
    // (the Tokio runtime is built later, on the entry thread), so the set_var
    // inside is sound. After this returns, WAYLAND_HOME is the sole source of
    // truth; the active pointer is never read again.
    wcore_config::profile::activate_for_launch();

    // Load ~/.wayland/.env (or $WAYLAND_HOME/.env) into the process environment
    // before ANY threads spawn. The Config TUI writes provider keys there
    // (surfaces/config.rs save); without this they never reach credential
    // resolution on the next launch. main() is single-threaded at this point —
    // the Tokio runtime is built later, on the entry thread — so set_var is
    // sound here. Existing exported vars win (never clobbered).
    wcore_config::env_file::load_wayland_env_file();

    // Windows defaults the main-thread stack to 1 MiB. wcore-cli's root future
    // (this large `async` entry plus the full clap command tree built by
    // `Cli::parse`) exceeds it and the process aborts with STATUS_STACK_OVERFLOW
    // (0xC00000FD) before any command runs — even `--help`. Unix defaults to an
    // 8 MiB main stack, which is why this only bites on Windows. Run the entire
    // entry on a dedicated thread with a generous explicit stack so the binary
    // behaves identically on every platform. The Tokio runtime is built INSIDE
    // that thread, so `block_on` drives the root future on the large stack
    // (a `#[tokio::main]` would instead drive it on the 1 MiB main thread).
    const ENTRY_STACK_SIZE: usize = 32 * 1024 * 1024;
    let entry = std::thread::Builder::new()
        .name("wcore-main".into())
        .stack_size(ENTRY_STACK_SIZE)
        .spawn(|| {
            // Declared before runtime/bootstrap state. Normal reverse drop
            // order shuts down the runtime first; signal shutdown additionally
            // drops the root future before returning from block_on.
            let _bundled_skill_cleanup = BundledSkillTmpCleanup;
            // The entry thread above carries a large explicit stack, but the
            // runtime built inside it spawns WORKER threads at the platform
            // default, so every `tokio::spawn`ed task runs on a 2 MiB stack on
            // Windows. `WorktreeManager::create_isolated_checkout` is reached in
            // production from the agent's spawner, its durable-launch path, the
            // anvil forge and the child-transaction gates, and it drives the same
            // chain of large futures that overflows in tests.
            //
            // HONEST FRAMING: the production path was NOT observed to overflow.
            // What WAS measured is that the nearest path's headroom is under
            // 256 KiB over this same 2 MiB default (a full-suite sweep: unset
            // (2 MiB) -> 4 aborts, 2359296 (2.25 MiB) -> 0). This is
            // defense-in-depth against a measured-narrow margin, NOT a repair for
            // a reproduced production crash. 8 MiB matches the unix main-thread
            // default and stays small enough that a genuinely unbounded recursion
            // still overflows, so it cannot mask a runaway. The cost is virtual
            // address-space RESERVE, not commit — negligible on 64-bit.
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(8 * 1024 * 1024)
                .build()?;
            let outcome = runtime.block_on(run_until_shutdown(run(), shutdown_signal()));
            // The startup-refusal chokepoint. Any error escaping `run()` before
            // the `ready` frame went out is a refusal the `--json-stream` host
            // must hear about on STDOUT — anyhow would otherwise print it to
            // stderr, which the protocol consumer does not read, leaving the
            // desktop app with a pipe that opened and closed carrying nothing.
            // Runs while `runtime` is still alive and is a no-op for non-
            // protocol runs, for runs that reached `ready`, and for the
            // pre-existing #186 sites that already reported.
            if let Err(error) = &outcome {
                wcore_cli::startup_error::report_startup_refusal(error);
            }
            // Row B-3 — a finished run must END the process.
            //
            // Dropping a Tokio runtime WAITS, without bound, for every task on
            // the blocking pool to return, and `spawn_blocking` tasks cannot be
            // aborted. The email channel runs its IMAP poll loop there
            // (`wcore_channel_email::imap::imap_poll_blocking`), and that loop
            // only returns when its shutdown watch flips. Measured: a one-shot
            // run whose work was DONE (last stdout byte 03:02:31) sat here
            // polling IMAP every 10s until the harness SIGKILLed it at 03:16:07
            // — 13m36s of hang scored as `exit_code:-9, timed_out:true`, which
            // is how every positive run of corpus row B-3 has ever ended.
            //
            // The exit paths below stop the channel manager explicitly, so in
            // the ordinary case this returns at once. This is the backstop that
            // makes "the process exits" a property of the entry point rather
            // than of five exit paths each remembering to clean up: any future
            // leaked blocking task costs a bounded delay, never the whole run.
            runtime.shutdown_timeout(std::time::Duration::from_secs(5));
            outcome
        })
        .map_err(|e| anyhow::anyhow!("failed to spawn wcore-cli entry thread: {e}"))?;
    entry
        .join()
        .map_err(|_| anyhow::anyhow!("wcore-cli entry thread panicked"))?
}

/// Build a clear, actionable reason string for an engine init failure (#186).
///
/// Without this, a `--json-stream` host (the Wayland desktop app) only sees a
/// bare non-zero exit code and renders a generic "wcore exited with code 1
/// during init" — the real cause never surfaces. When the failure is a
/// [`MissingApiKey`](wcore_config::config::MissingApiKey), the message also
/// points local-model users at the `ollama:` prefix fix, since that is the
/// common case behind this symptom (a local model selected without the prefix
/// falls back to the default keyed provider).
fn init_failure_message(err: &anyhow::Error, provider_label: &str) -> String {
    let mut msg = format!("Engine failed to start during init: {err:#}");
    let is_missing_api_key = err
        .downcast_ref::<wcore_config::config::MissingApiKey>()
        .is_some()
        || err
            .chain()
            .any(|c| c.is::<wcore_config::config::MissingApiKey>());
    if is_missing_api_key {
        msg.push('\n');
        msg.push_str(&format!(
            "Provider '{provider_label}' requires an API key. To use a LOCAL model \
             with Ollama, select a model id prefixed with `ollama:` (e.g. \
             `ollama:qwen3-coder:30b`) — no API key is needed. Otherwise add a key \
             via onboarding or set ANTHROPIC_API_KEY / OPENAI_API_KEY."
        ));
    }
    msg
}

/// The ONE wiring point from parsed argv to the two danger tiers, returning
/// `(approval_bypass, dangerous_launch)`.
///
/// Tier 1 (`--dangerously-skip-permissions`, aliases `--force` / `--yolo`)
/// bypasses approvals and leaves the OS sandbox REQUIRED. Tier 2
/// (`--dangerously-skip-permissions-and-sandbox`, deprecated alias
/// `--dangerous`) additionally bypasses the sandbox, under a lease that only a
/// local argv launch can mint. Tier 2 does not need `approval_bypass`: its
/// grant already resolves approvals to `Bypass`.
///
/// `run()` and the tier-regression test both read the wiring from here, so an
/// edit that moves a tier-1 alias into tier 2 cannot pass the test.
/// The directory that identifies the workspace this session operates against:
/// `--project-dir` when given, else the process CWD.
///
/// #693 — every workspace-keyed decision must resolve this the SAME way.
/// `--project-dir` moves the config, the project skills, the MCP servers and
/// the workspace-trust entry without moving the CWD, so two sessions launched
/// from one shell against two different projects have one CWD and two
/// workspaces. The durable learned-permission grants key off this too; keying
/// them off the CWD alone let a grant made against project A auto-approve the
/// same tool name against project B.
fn workspace_root(project_dir: Option<&std::path::Path>) -> anyhow::Result<std::path::PathBuf> {
    Ok(match project_dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    })
}

fn danger_tiers(cli: &Cli) -> (bool, bool) {
    (cli.dangerously_skip_permissions, cli.dangerous)
}

/// The advice printed when stdin is not a TTY and no prompt was given.
///
/// Lifted out of `run()` for the same reason as `danger_tiers` above: the
/// product and the test read the SAME bytes, so the test cannot pass against
/// a stale copy of the message.
///
/// UAT-W1: this used to end `pass a prompt with -p`. `-p` is the short form of
/// `--provider` (see the `Cli` derive), so a user who followed the product's
/// own advice got `Unknown provider: '<their prompt>'`. The prompt is a
/// trailing positional, not a flag.
const NON_TTY_NO_PROMPT_ADVICE: &str = "wayland-core: stdin is not a terminal and no prompt was given.\n\
     Use --json-stream for headless/piped use, or pass the prompt as an\n\
     argument: wayland-core \"your prompt here\".";

/// Added when the user asked to RESUME and still got the advice above.
///
/// UAT-UXA2: recovering a crash-interrupted session begins with
/// `wayland-core --resume <id>`, and the generic refusal above is all the
/// product said — it never mentioned that resuming takes a message too, and
/// never mentioned the reconcile/cancel path the engine's own interrupted-turn
/// refusal names. A user who reads it learns nothing about the state they are
/// actually in.
const RESUME_NO_PROMPT_ADVICE: &str = "Resuming needs a message as well as the session:\n\
     wayland-core --resume <id> \"your next message\"\n\
     If that then refuses because the session was interrupted mid-turn, close the\n\
     interrupted turn first — it prints the exact command for anything it cannot\n\
     decide itself:\n\
     wayland-core session cancel <id>";

async fn run() -> anyhow::Result<ExitCode> {
    let mut cli = Cli::parse();
    // Record protocol mode before ANY fallible startup work, so every refusal
    // from here to the `ready` frame reaches the host as an error frame rather
    // than as stderr text the protocol consumer never reads. The emit itself
    // happens once, at the process-exit chokepoint in `main`.
    if cli.json_stream {
        wcore_cli::startup_error::mark_json_stream_active();
    }
    let (approval_bypass, dangerous_launch) = danger_tiers(&cli);
    let dangerous_ttl_secs = cli
        .dangerous_ttl_secs
        .unwrap_or(DEFAULT_DANGEROUS_SESSION_TTL_SECS);
    // The compatibility notice stays scoped to the danger-NAMED tier-1
    // spelling, exactly as before the rename, so existing `--force` / `--yolo`
    // runs keep byte-identical stderr. All three spellings are one clap field
    // and therefore one tier; only this advisory text is spelling-scoped.
    if cli.dangerously_skip_permissions
        && std::env::args().any(|arg| arg == "--dangerously-skip-permissions")
    {
        eprintln!(
            "wayland-core: --dangerously-skip-permissions bypasses approval prompts only; \
             the OS sandbox remains required. Use \
             --dangerously-skip-permissions-and-sandbox for an explicit, time-bounded \
             local sandbox bypass."
        );
    }
    let eval_egress_key = cli
        .eval_egress_key_file
        .take()
        .map(|path| read_one_use_eval_egress_key(&path))
        .transpose()?;
    wcore_agent::egress::install_eval_egress_observer(eval_egress_key)?;
    if let Some(path) = cli.api_key_file.take() {
        cli.api_key = Some(read_one_use_api_key(&path)?);
    }

    // v0.9.1 W2 cycle-2 HIGH 2: when the binary will enter the
    // alt-screen TUI, route INFO/WARN traces and startup notices to a
    // log file so they don't leak as pre-alt-screen TTY noise (the
    // crash-sentinel warning was alarming users with their home path).
    // Headless modes (`-p`, `--json-stream`, `--no-tui`, piped stdout)
    // keep the previous stderr behaviour so CI logs and `--help` still
    // print where users expect.
    //
    // The TUI predicate mirrors the dispatch at the bottom of main()
    // (`prompt.is_empty() && !cli.no_tui && tui_capable && !json_stream`).
    // It's a best-effort heuristic computed BEFORE the engine boots so
    // the subscriber installs once with the right writer; if the actual
    // dispatch path later falls back to REPL the stderr fallback below
    // is still acceptable (the alt-screen is never entered).
    let prompt_guess = cli.prompt.join(" ");
    let tui_capable = std::io::IsTerminal::is_terminal(&std::io::stdout())
        && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true);
    let will_enter_tui = prompt_guess.is_empty() && !cli.no_tui && tui_capable && !cli.json_stream;

    // F-001: install the tracing subscriber so every
    // tracing::info!/warn!/error!/debug! reaches a sink. EnvFilter honours
    // RUST_LOG at runtime; default is "info" when RUST_LOG is unset or
    // unparseable. try_init() is a no-op if something else already
    // initialised (e.g. tests); the let _ = swallows the Err in that case.
    //
    // v0.9.1 W2 cycle-2: TUI mode writes to `$WAYLAND_HOME/logs/wayland-core.log`
    // so trace output never lands on the alt-screen-bound stdio. Failure
    // to open the file degrades silently to stderr — we'd rather have
    // visible traces than none.
    //
    // ── lane fix-tui-noise: quiet by default, diagnostics never lost ────────
    // UAT-TUI-UNIX F4 / UAT-TUI-WINDOWS F3: a single trivial headless turn
    // printed 42 lines of engine internals to stderr before the answer
    // (measured at e7bc6d88: 20 INFO + 19 WARN — capability advisories about
    // Spotify, Postgres, TTS, ffmpeg and browser backends the user never
    // invoked). TUI mode was already fixed by routing traces to a file; every
    // other mode still wrote them to the terminal.
    //
    // The rule now is: `RUST_LOG` is authoritative and UNCHANGED when set —
    // `RUST_LOG=info wayland-core "…"` reproduces the previous behaviour
    // byte for byte, on stderr, in every mode. When `RUST_LOG` is UNSET the
    // full INFO record goes to `$WAYLAND_HOME/logs/wayland-core.log` and only
    // ERROR reaches stderr. That is strictly MORE diagnosable than before:
    // headless runs previously kept no log at all, so the record was lost the
    // moment the terminal scrolled.
    //
    // No new flag is introduced. `RUST_LOG` already existed, already worked and
    // is already the documented lever.
    //
    // ── B2.2: should a one-shot headless run write a trace file at all? ─────
    // DECISION: yes, it keeps writing one, and the defect was the missing
    // bound, not the file. `log_rotate` supplies the bound.
    //
    // Justified against the gateway case specifically, because that is the
    // case that makes the question sharp: a host answering channel messages
    // runs headless CONTINUOUSLY, so "headless writes a trace by default" is at
    // its most expensive there. It is also at its most necessary there. That
    // host has no terminal anyone is watching, no TUI to route traces to, and
    // it is the one deployment whose failures (a channel that stopped polling,
    // a credential that expired, a delivery abandoned at 04:00) are discovered
    // hours later from the record or not at all. Defaulting headless to no file
    // would leave the gateway as the only mode of the product with no
    // diagnostics whatsoever — the exact "a trace record existing nowhere"
    // state the fix-tui-noise change was made to end.
    //
    // The cost it was actually challenged on — unbounded growth on a
    // continuously-running host — is answered by bounding the file at
    // 2 × MAX_LOG_BYTES rather than by removing it. A gateway now holds at most
    // 10 MiB of its own most recent diagnostics, forever.
    //
    // The short-lived one-shot run is the cheap case, not the expensive one:
    // ~7 kB, into a file that is now capped. `RUST_LOG` remains the lever for
    // anyone who wants the old stderr behaviour instead.
    let rust_log_set = std::env::var_os("RUST_LOG").is_some();
    let log_to_file = will_enter_tui || !rust_log_set;
    let tui_log_file: Option<log_rotate::RotatingLog> = if log_to_file {
        match open_tui_log_file() {
            Ok(f) => Some(f),
            Err(e) => {
                // B2.3: the fallback must be OBSERVABLE. A product that cannot
                // open its log must still run — and must say so, or "it exited
                // 0" is equally consistent with logging being dead, disabled,
                // or never attempted. This line is the degraded-mode marker the
                // integration test asserts on.
                eprintln!("{}: {e}", log_rotate::LOG_FALLBACK_NOTICE);
                None
            }
        }
    } else {
        None
    };
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // v0.9.x perf (2026-05-31 teardown §2.1): route TUI-mode file logging
    // through `tracing_appender::non_blocking` so a `tracing::info!`/`warn!`/
    // `debug!` on the Tokio runtime only enqueues the line — a dedicated worker
    // thread owns the `write()`+`flush()` syscalls. The previous `Mutex<File>`
    // writer did the blocking write inline on whatever async worker logged,
    // which under `RUST_LOG=debug` (the diagnostic loop) stalled the engine.
    // `_log_guard` MUST stay alive for the whole process: dropping it flushes
    // the buffer and stops the worker, so it is parked in `main`'s frame.
    let mut _log_guard: Option<tracing_appender::non_blocking::WorkerGuard> = None;
    if let Some(file) = tui_log_file {
        let (non_blocking, guard) = tracing_appender::non_blocking(file);
        _log_guard = Some(guard);
        if will_enter_tui {
            // Unchanged: the alt-screen owns the terminal, so NOTHING may reach
            // stdio — not even an error.
            let _ = fmt()
                .with_env_filter(env_filter)
                .with_writer(non_blocking)
                .with_target(false)
                .try_init();
        } else {
            // Headless / REPL / json-stream with RUST_LOG unset: tee. The file
            // takes everything the filter admits; stderr takes ERROR only, so a
            // genuine failure is still visible without opening a log.
            use tracing_subscriber::fmt::writer::MakeWriterExt;
            let writer = non_blocking.and(std::io::stderr.with_max_level(tracing::Level::ERROR));
            let _ = fmt()
                .with_env_filter(env_filter)
                .with_writer(writer)
                .with_target(false)
                .try_init();
        }
    } else {
        let _ = fmt()
            .with_env_filter(env_filter)
            .with_writer(std::io::stderr)
            .with_target(false)
            .try_init();
    }

    // T1-E2: dirty-death crash sentinel. Probe + arm BEFORE any other work
    // so the flag survives across subcommand short-circuits, doctor runs,
    // and the full agent boot path alike. The guard is held in `main`'s
    // stack frame for the rest of the run; `Drop` removes the flag on
    // clean exit and intentionally leaves it behind during a panic so
    // the next start can detect the unclean shutdown.
    //
    // v0.9.1 W2 cycle-2 HIGH 2: in TUI mode the warning is emitted via
    // `tracing::warn!` so it lands in the log file (not on the
    // alt-screen-bound TTY). Non-TUI keeps `eprintln!` for parity with
    // existing CI scrapers.
    let mut _sentinel_guard = {
        let sentinel_path = wcore_cli::crash_sentinel::CrashSentinel::default_path();
        // #181: the sentinel is scoped per-process (`.dirty-death.<pid>`).
        // The scan reports ONLY flags whose owning pid is dead (plus the
        // legacy un-scoped flag, once, for migration) — a live sibling
        // engine's flag is not a crash. Reported flags are reaped by the
        // scan so each dirty death fires exactly once.
        let dead_sentinels = wcore_cli::crash_sentinel::CrashSentinel::scan_dead_sentinels(
            &wcore_cli::crash_sentinel::CrashSentinel::default_dir(),
        );
        for dead_path in &dead_sentinels {
            if will_enter_tui {
                tracing::warn!(
                    path = %dead_path.display(),
                    "previous run did not shut down cleanly (crash sentinel found)"
                );
            } else {
                eprintln!(
                    "wayland-core: warning: previous run did not shut down cleanly \
                     (crash sentinel found at {})",
                    dead_path.display()
                );
            }
        }
        match wcore_cli::crash_sentinel::CrashSentinel::new(sentinel_path.clone()) {
            Ok(guard) => Some(guard),
            Err(e) => {
                if will_enter_tui {
                    tracing::warn!(
                        path = %sentinel_path.display(),
                        error = %e,
                        "could not arm crash sentinel"
                    );
                } else {
                    eprintln!(
                        "wayland-core: warning: could not arm crash sentinel at {}: {}",
                        sentinel_path.display(),
                        e
                    );
                }
                None
            }
        }
    };

    // M5.4: subcommand short-circuit. Subcommands run before any of the
    // flag-driven modes (doctor, REPL, etc.) so a user who runs
    // `wayland-core plugin install ...` never hits the agent bootstrap.
    if let Some(cmd) = cli.command {
        return match cmd {
            // F-089: model catalog subcommand.
            TopCmd::Models { cmd } => {
                match cmd {
                    ModelsCmd::List { provider } => {
                        print_known_models(provider.as_deref());
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
            TopCmd::Plugin(args) => match wcore_cli::plugin::run(args) {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("error: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            // v0.6.4 Task 2.4 + 2.5: serve the engine's tool registry over
            // MCP, gated by a real `PolicyGate`. We seed the registry with
            // the read-only built-ins (Read/Grep/Glob) — same safe set used
            // by `AgentBootstrap::build_for_test` — and the gate with one
            // `Invoke` grant per advertised tool. The grant set is the
            // sole authority on what an MCP client may invoke; widening
            // the registry without widening the gate causes a deliberate
            // POLICY_DENIED rather than a silent broadening of the
            // over-the-wire surface.
            TopCmd::McpServe(args) => {
                let mut registry = wcore_tools::registry::ToolRegistry::new();
                registry.register(Box::new(wcore_tools::read::ReadTool::new(None)));
                registry.register(Box::new(wcore_tools::grep::GrepTool));
                registry.register(Box::new(wcore_tools::glob::GlobTool));

                // Task 2.5: build the policy gate with Invoke grants for
                // exactly the tools we registered above. `Actor::User
                // ("mcp-serve")` is the gate's default actor — every
                // incoming `tools/call` is attributed to it (the MCP
                // server has no sub-agent attribution path).
                let mut engine = wcore_permissions::PolicyEngine::new();
                let actor = wcore_permissions::Actor::User("mcp-serve".into());
                for tool_name in ["Read", "Grep", "Glob"] {
                    engine.grant(wcore_permissions::Permission {
                        actor: actor.clone(),
                        resource: wcore_permissions::Resource::Tool(tool_name.into()),
                        action: wcore_permissions::Action::Invoke,
                    });
                }
                let gate =
                    wcore_agent::policy_gate::PolicyGate::new(std::sync::Arc::new(engine), actor);

                match wcore_cli::mcp_serve::run(args, registry, gate).await {
                    Ok(()) => Ok(ExitCode::SUCCESS),
                    Err(e) => {
                        eprintln!("mcp-serve error: {e:#}");
                        Ok(ExitCode::FAILURE)
                    }
                }
            }
            TopCmd::Swarm(args) => match wcore_cli::swarm::run(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("error: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            // F23-02: dispatched here, alongside the other subcommands and
            // before `Config::resolve`, so listing and searching sessions work
            // for a first-run user with no provider key — the same contract the
            // root `--list-sessions` flag already honours.
            TopCmd::Session(args) => wcore_cli::session_cmd::run(args),
            // F23-06: dispatched beside `session` and before `Config::resolve`
            // for the same reason — indexing and searching a checkout needs no
            // provider credential.
            TopCmd::Index(args) => wcore_cli::index_cmd::run(args),
            // F23-04: dispatched beside `index` and before `Config::resolve` —
            // reading a ledger a past session already wrote needs no provider
            // credential, and refusing to report on a session because the
            // current environment has no key would be absurd.
            TopCmd::Cache(args) => wcore_cli::cache_cmd::run(args),
            TopCmd::Workflow { cmd } => match wcore_cli::workflow::run(cmd).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core workflow: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Crucible {
                task,
                auto,
                council,
                judge,
                direct,
                force_council,
                deep,
                deny,
                advisor,
                terminal,
            } => {
                let args = wcore_cli::crucible::CrucibleArgs {
                    task,
                    auto,
                    council: (!council.is_empty()).then_some(council),
                    judge,
                    direct,
                    force_council,
                    deep,
                    deny,
                    advisor,
                    terminal,
                };
                match wcore_cli::crucible::run_crucible(args).await {
                    Ok(()) => Ok(ExitCode::SUCCESS),
                    Err(e) => {
                        eprintln!("wayland-core crucible: {e:#}");
                        Ok(ExitCode::FAILURE)
                    }
                }
            }
            TopCmd::Forge(args) => match wcore_cli::anvil::run_forge(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core forge: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            // methodology #27: production caller for project_context::scan
            // (v0.7.0 Task 1.C.1).
            TopCmd::ProjectContext => {
                match wcore_agent::project_context::scan(std::path::Path::new(".")) {
                    Ok(ctx) => match ctx.rendered() {
                        Some(body) => {
                            print!("{body}");
                            Ok(ExitCode::SUCCESS)
                        }
                        None => {
                            eprintln!("no project context files found in cwd or ancestors");
                            Ok(ExitCode::SUCCESS)
                        }
                    },
                    Err(e) => {
                        eprintln!("project-context error: {e:#}");
                        Ok(ExitCode::FAILURE)
                    }
                }
            }
            TopCmd::Init(args) => match wcore_cli::init::run(args) {
                Ok(outcome) => {
                    wcore_cli::init::print_summary(&outcome);
                    Ok(ExitCode::SUCCESS)
                }
                Err(e) => {
                    eprintln!("wayland-core: init failed: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Acp(args) => match wcore_cli::acp::run(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core acp: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Agent { cmd } => match wcore_cli::agent_cmd::run(cmd) {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core agent: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Cron { cmd } => match wcore_cli::cron::run(cmd).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core cron: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            // v0.8.1 U9: production caller for `self_update::run`. Pulls
            // the latest signed release from FerroxLabs/wayland-core,
            // verifies the .sig against the pinned marketplace pubkey,
            // and atomically swaps the running binary (self_replace).
            TopCmd::SelfUpdate { check_only } => {
                match wcore_cli::self_update::run(check_only).await {
                    Ok(()) => Ok(ExitCode::SUCCESS),
                    Err(e) => {
                        eprintln!("wayland-core self-update: {e:#}");
                        Ok(ExitCode::FAILURE)
                    }
                }
            }
            // CLI surface: `setup` launches the TUI on the Onboarding
            // surface regardless of whether a config already exists.
            // F-012 / F-018 fix: do NOT call Config::resolve here — that
            // would bail with "No API key found" on a fresh install, which
            // is exactly the state the user is trying to fix. Use
            // Config::default() so the onboarding TUI can open and walk
            // the user through provider + key entry.
            TopCmd::Setup => {
                let config = Config::default();
                let cwd = std::env::current_dir()?.to_string_lossy().to_string();
                // Setup subcommand never honours --force: the onboarding
                // flow makes no tool calls. web_search is irrelevant here.
                let execution = resolve_local_execution(
                    &config,
                    false,
                    false,
                    DEFAULT_DANGEROUS_SESSION_TTL_SECS,
                    false,
                )?;
                let tui_workspace = workspace_root(cli.project_dir.as_deref())?;
                run_tui_mode(
                    config,
                    &cwd,
                    &tui_workspace,
                    None,
                    None,
                    None,
                    true,
                    execution,
                    false,
                )
                .await?;
                // B3: explicitly disarm the crash sentinel on normal TUI
                // exit so it isn't present if the process is still alive
                // during post-TUI cleanup (MCP shutdown, etc.) and then
                // dies unexpectedly. The Drop impl also disarms, but this
                // call fires earlier — at the earliest known-clean point.
                if let Some(ref mut g) = _sentinel_guard {
                    let _ = g.disarm();
                }
                Ok(ExitCode::SUCCESS)
            }
            // CLI surface: `auth` manages provider API keys (list / add /
            // remove) and subscription OAuth logins (login / logout / status)
            // for the global config.toml + token store. Awaited on the
            // existing runtime — the OAuth verbs are async (a nested runtime
            // would panic).
            TopCmd::Auth { cmd } => match wcore_cli::auth::run(cmd).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core auth: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Image(args) => match wcore_cli::image::run(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core image: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Fetch(args) => match wcore_cli::fetch::run(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core fetch: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Backend(args) => match wcore_cli::backend::run(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core backend: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Gateway(args) => match wcore_cli::gateway::run(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core gateway: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Node(args) => match wcore_cli::node::run(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core node: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Goal(args) => match wcore_cli::goal_cmd::run(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core goal: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Channel(args) => match wcore_cli::channel::run(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core channel: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            TopCmd::Sandbox(args) => match wcore_cli::sandbox_cmd::run_sandbox(args).await {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core sandbox: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            // `profile::run` is synchronous — no `.await` (mirrors `TopCmd::Plugin`).
            TopCmd::Profile { cmd } => match wcore_cli::profile::run(cmd) {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core profile: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            // `migrate::run` is synchronous — no `.await` (mirrors `TopCmd::Profile`).
            TopCmd::Migrate { cmd } => match wcore_cli::migrate::run(cmd) {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core migrate: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
            // `backup::run` is synchronous — no `.await` (mirrors `TopCmd::Migrate`).
            TopCmd::Backup { cmd } => match wcore_cli::backup::run(cmd) {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core backup: {e:#}");
                    Ok(ExitCode::FAILURE)
                }
            },
        };
    }

    if cli.resume.is_some() && cli.session_id.is_some() {
        anyhow::bail!("Cannot use --resume and --session-id together");
    }

    // W5 (A.5): doctor is the only path that returns a non-zero exit
    // code without raising an `anyhow::Error`. Run it before any other
    // mode so a misconfigured environment can be diagnosed without
    // touching config files, OAuth, or the engine bootstrap.
    if cli.doctor {
        // #1079: doctor resolves config for two of its sections (the declared
        // MCP list and the durable-sessions verdict). It used to do that with
        // `CliArgs::default()`, which threw away `--profile` and
        // `--project-dir` -- so it reported on a config the same invocation
        // would never have used -- and could fail with `MissingApiKey` on a
        // host where the user's own `--provider`/`--api-key` resolve fine.
        // Hand it the same args a real run gets.
        //
        // `max_turns` and `system_prompt` are the raw flags rather than the
        // `effective_*` values computed at `:2005`: those are derived AFTER
        // this early return, and neither one participates in selecting or
        // loading a config file, so the extra hop would buy nothing.
        let doctor_args = CliArgs {
            provider: cli.provider.clone(),
            api_key: cli.api_key.clone(),
            base_url: cli.base_url.clone(),
            model: cli.model.clone(),
            max_tokens: cli.max_tokens,
            max_turns: cli.max_turns,
            system_prompt: cli.system_prompt.clone(),
            profile: cli.profile.clone(),
            auto_approve: cli.auto_approve,
            project_dir: cli.project_dir.clone(),
        };
        return Ok(doctor::run(cli.probe_mcp, cli.probe_provider, &doctor_args).await);
    }

    // Issue #114: the explicit, user-initiated TCC prompt. Kept next to
    // --doctor so it runs before config/OAuth/engine bootstrap — a user
    // fixing permissions must not have to get past anything else first.
    if cli.request_permissions {
        return Ok(request_permissions());
    }

    // Handle --build-info: print version + embedded source SHA and exit.
    if cli.build_info {
        println!(
            "wayland-core {} (source {})",
            env!("CARGO_PKG_VERSION"),
            env!("WAYLAND_SOURCE_SHA")
        );
        return Ok(ExitCode::SUCCESS);
    }

    // Handle --config-path
    if cli.config_path {
        println!("{}", config::global_config_path().display());
        return Ok(ExitCode::SUCCESS);
    }

    // Handle --skills-path
    if cli.skills_path {
        print_skills_paths();
        return Ok(ExitCode::SUCCESS);
    }

    // W4 F19: --skills-audit
    if cli.skills_audit {
        run_skills_audit(cli.skills_audit_stale_days).await?;
        return Ok(ExitCode::SUCCESS);
    }

    // W9.1 T4 (T11): skills lifecycle subcommands. Mutually-exclusive
    // with each other and with --skills-audit/--skills-path so a single
    // invocation does exactly one curator action.
    if let Some(id) = cli.skills_promote.as_deref() {
        run_skills_promote(id).await?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(id) = cli.skills_archive.as_deref() {
        run_skills_archive(id).await?;
        return Ok(ExitCode::SUCCESS);
    }
    // ---- 23A-C1 governed skill lifecycle (one contiguous additive block) ----
    if let Some(name) = cli.skills_revoke.as_deref() {
        wcore_cli::skill_govern::run_revoke(name)?;
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(id) = cli.skills_rollback.as_deref() {
        wcore_cli::skill_govern::run_rollback(id)?;
        return Ok(ExitCode::SUCCESS);
    }
    if cli.skills_govern {
        wcore_cli::skill_govern::run_list()?;
        return Ok(ExitCode::SUCCESS);
    }

    // M3.4: dump memory state for a given session.
    if let Some(session) = cli.memory_show.as_deref() {
        run_memory_show(session).await?;
        return Ok(ExitCode::SUCCESS);
    }

    // M5.2: replay a session trace (with optional diff against a
    // second trace). Surfaces the version-skew guard error verbatim
    // unless --replay-force-version-skew was passed.
    if let Some(trace_path) = cli.replay.as_deref() {
        run_replay(
            trace_path,
            cli.replay_diff.as_deref(),
            cli.replay_force_version_skew,
        )?;
        return Ok(ExitCode::SUCCESS);
    }

    // v0.7.0 Task 3.A.1: --list-agents prints built-in agent personas.
    if cli.list_agents {
        for m in wcore_agents_pack::AgentPack::list() {
            println!("{:24}  {}", m.name, m.description);
        }
        return Ok(ExitCode::SUCCESS);
    }

    // Handle --init-config
    if cli.init_config {
        config::init_config()?;
        return Ok(ExitCode::SUCCESS);
    }

    // lane fix-tui-noise: a prompt on argv means exactly one answer then exit,
    // so the sink drops the `⏺ `/`* ` speaker marker and terminates stdout with
    // a newline (UAT-TUI-WINDOWS F4/F5, UAT-TUI-UNIX F8). Interactive REPL runs
    // — where the marker separates turns — are unaffected.
    let terminal = Arc::new(if cli.prompt.is_empty() {
        TerminalSink::new(cli.no_color)
    } else {
        TerminalSink::new(cli.no_color).one_shot()
    });
    let output: Arc<dyn OutputSink> = terminal.clone();

    // v0.7.0 Task 3.A.1: resolve --agent overlay so the built-in's
    // system_prompt + max_turns fill in unless explicit overrides are set.
    let agent_overlay = cli
        .agent
        .as_deref()
        .map(|name| {
            wcore_agents_pack::AgentPack::get(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown built-in agent '{}'. Run --list-agents for the full list.",
                    name
                )
            })
        })
        .transpose()?;

    let effective_system_prompt = cli
        .system_prompt
        .clone()
        .or_else(|| agent_overlay.as_ref().map(|m| m.system_prompt.clone()));
    let effective_max_turns = cli.max_turns.or_else(|| {
        agent_overlay
            .as_ref()
            .and_then(|m| m.max_turns.map(|n| n as usize))
    });

    // F-018: --list-sessions short-circuit BEFORE Config::resolve.
    // Listing saved sessions does not require a provider API key — a
    // first-run user should be able to see (empty) session history. We
    // try a full resolve first to honour any custom session.directory
    // from the config file, and fall back to Config::default() if that
    // fails (e.g. "No API key found").
    if cli.list_sessions {
        let session_dir_config = Config::resolve(&CliArgs {
            provider: None,
            api_key: None,
            base_url: None,
            model: None,
            max_tokens: None,
            max_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: cli.project_dir.clone(),
        })
        .unwrap_or_default();
        let session_mgr = session::SessionManager::new(
            session_dir_config.session.directory.clone().into(),
            session_dir_config.session.max_sessions,
        );
        let sessions = session_mgr.list()?;
        // The session table is this flag's ANSWER, not a diagnostic, so it goes
        // to STDOUT. It used to go to stderr, which left
        // `wayland-core --list-sessions | grep <id>` silently matching nothing
        // while the table scrolled past on the terminal. `--list-agents` above
        // already prints its answer to stdout, and so does the
        // `session list` subcommand, whose doc comment recorded this flag as
        // the outlier; it no longer is.
        if sessions.is_empty() {
            println!("No saved sessions.");
        } else {
            println!(
                "{:<8} {:<12} {:<30} {:>5}  Summary",
                "ID", "Date", "Model", "Msgs"
            );
            for s in &sessions {
                println!(
                    "{:<8} {:<12} {:<30} {:>5}  {}",
                    s.id,
                    s.created_at.format("%Y-%m-%d"),
                    s.model,
                    s.message_count,
                    s.summary
                );
            }
        }
        return Ok(ExitCode::SUCCESS);
    }

    // #186: capture the requested provider label before `cli.provider` is
    // moved into `CliArgs`, so an init-failure error event (below) can name
    // the provider even when config resolution fails. Falls back to the
    // engine default ("anthropic") when no provider was requested.
    let provider_label_for_error = cli
        .provider
        .clone()
        .unwrap_or_else(|| "anthropic".to_string());

    // 3A / D3: fail closed BEFORE `cli.profile` is moved into CliArgs below. If
    // the host signaled profile intent but no isolated home was materialized
    // (WAYLAND_HOME was resolved once at process entry by activate_for_launch),
    // refuse rather than silently writing the shared default home.
    if let Err(msg) = json_stream_profile_guard(
        cli.json_stream,
        cli.profile.as_deref(),
        std::env::var_os("WAYLAND_HOME").is_some(),
    ) {
        anyhow::bail!(msg);
    }

    // Resolve config from files + CLI args + env vars
    let workspace_for_trust = workspace_root(cli.project_dir.as_deref())?;
    let trust_store = wcore_config::workspace_trust::WorkspaceTrustStore::for_current_home();
    if cli.trust_workspace {
        let fingerprint = trust_store.grant(&workspace_for_trust)?;
        eprintln!(
            "Trusted workspace executable fingerprint {} for {}",
            &fingerprint.digest[..12],
            fingerprint.root.display()
        );
    } else if cli.untrust_workspace {
        let removed = trust_store.revoke(&workspace_for_trust)?;
        eprintln!(
            "{} workspace trust for {}",
            if removed { "Removed" } else { "No stored" },
            workspace_for_trust.display()
        );
    }

    let cli_args = CliArgs {
        provider: cli.provider,
        api_key: cli.api_key,
        base_url: cli.base_url,
        model: cli.model,
        max_tokens: cli.max_tokens,
        max_turns: effective_max_turns,
        system_prompt: effective_system_prompt,
        profile: cli.profile,
        auto_approve: cli.auto_approve,
        project_dir: cli.project_dir,
    };

    // B2: one-shot migration from legacy ~/.wayland/config.yaml → canonical
    // config.toml, run here (in the binary) so it doesn't affect tests that
    // call Config::resolve directly with a temp project_dir. Idempotent.
    wcore_config::config::migrate_legacy_yaml_if_needed();

    let resolved_config = match Config::resolve_with_provenance(&cli_args) {
        Ok(resolved) => resolved,
        Err(resolution_error) => {
            let e = resolution_error.source;
            // T0-1: On a true first run (no global config yet) where the user
            // just typed `wayland-core` to open the interactive TUI, a missing
            // API key must route into the Onboarding surface — not crash to
            // stderr and exit non-zero. This mirrors the `setup` subcommand,
            // which uses Config::default() so onboarding can walk the user
            // through provider + key entry. Without this, the very first launch
            // on a fresh machine dies before the TUI ever starts (the first-run
            // gate lives inside run_tui_mode, which this `?` never reached).
            //
            // D002: the same in-app recovery must also catch a RETURNING user
            // whose config exists but resolves to no credential — e.g. a
            // catalog/keyless provider with no api_key and no env var. Before
            // this, `first_run` was false (the file exists) so the recovery was
            // skipped and the binary crashed to stderr with "run wayland-core
            // setup", forcing a quit-to-shell. We additionally route when the
            // resolve error is specifically a `MissingApiKey` — a recoverable
            // "needs setup" condition — so the user lands in Onboarding in-app.
            // A corrupt-config `ConfigLoadError::ParseFailed` (D011) is NOT a
            // `MissingApiKey`, so it must NOT be swallowed into a fresh-install
            // walkthrough. The earlier gate keyed the swallow on `first_run`,
            // which inspects ONLY the global file — so a corrupt PROJECT
            // `.wayland-core.toml` on a machine with no global config (common in
            // CI scaffolds and first-use-in-a-repo) was silently routed into
            // onboarding, discarding the user's real-but-malformed config
            // (D011 dataloss, reachable under an interactive TUI launch). Gate
            // the swallow on the ERROR CLASS instead: always propagate a
            // `ConfigLoadError` (its only variant is `ParseFailed`) with the
            // file-named message BEFORE the onboarding branch, even under a TUI
            // launch, so a corrupt global OR project file aborts visibly.
            if e.downcast_ref::<wcore_config::config::ConfigLoadError>()
                .is_some()
            {
                return Err(e);
            }
            let prompt_empty = cli.prompt.join(" ").is_empty();
            let tui_capable = std::io::IsTerminal::is_terminal(&std::io::stdout())
                && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true);
            let would_open_tui = !cli.json_stream && prompt_empty && !cli.no_tui && tui_capable;
            // `first_run` inspects only the global file, so it cannot stand alone
            // as the onboarding trigger: a populated project repo on a machine
            // with no global config is NOT a fresh install. Onboarding is only
            // correct for a recoverable `MissingApiKey` (D002 keyless-catalog
            // recovery) OR a genuine fresh install with NO config file at all
            // (neither global nor project). `ParseFailed` is already returned
            // above, so the only resolve errors reaching here are credential/
            // alias/profile errors, never a corrupt file.
            let first_run = !config::global_config_path().exists();
            let missing_credentials = e
                .downcast_ref::<wcore_config::config::MissingApiKey>()
                .is_some();
            if would_open_tui
                && !dangerous_launch
                && (missing_credentials || (first_run && !project_config_exists()))
            {
                let cwd = std::env::current_dir()?.to_string_lossy().to_string();
                // B8-1: install the process-global egress policy BEFORE the
                // onboarding session runs. The normal install site (below, after
                // full config resolution) is never reached on this branch — it
                // early-returns SUCCESS here — so without this call onboarding's
                // outbound probes (key validation, Ollama reachability) would run
                // with NO global policy installed. `Config::default()` yields the
                // enforcing default posture (`[security] enabled = true`), and the
                // install is one-shot/first-call-wins, so this is the policy the
                // onboarding session sees and a later call is a guarded no-op.
                let onboarding_config = Config::default();
                wcore_agent::egress::install_egress_policy(&onboarding_config);
                let execution = resolve_local_execution(
                    &onboarding_config,
                    approval_bypass,
                    false,
                    dangerous_ttl_secs,
                    false,
                )?;
                run_tui_mode(
                    onboarding_config,
                    &cwd,
                    &workspace_for_trust,
                    None,
                    cli.session_id.clone(),
                    cli.assistant.clone(),
                    true,
                    execution,
                    cli.search,
                )
                .await?;
                if let Some(ref mut g) = _sentinel_guard {
                    let _ = g.disarm();
                }
                return Ok(ExitCode::SUCCESS);
            }
            if cli.json_stream && wcore_cli::startup_error::claim_startup_error_emission() {
                // #186: a json-stream host (desktop app) otherwise sees only a bare exit
                // code and shows a generic "wcore exited with code 1 during init". Emit a
                // structured error event so the real, actionable reason reaches the host UI.
                // The claim above keeps this more specific message and stands the
                // process-exit chokepoint down, so the host is told exactly once.
                let w = wcore_protocol::writer::ProtocolWriter::new();
                let _ = w.emit(&wcore_protocol::events::ProtocolEvent::Error {
                    msg_id: None,
                    error: wcore_protocol::events::ErrorInfo {
                        code: "init_failed".to_string(),
                        message: init_failure_message(&e, &provider_label_for_error),
                        retryable: false,
                        // A startup failure is this process refusing to
                        // proceed on its own account -- FailureCategory's
                        // LocalWayland names "a startup failure" outright.
                        category: wcore_protocol::events::FailureCategory::LocalWayland,
                    },
                });
            }
            return Err(e);
        }
    };
    let mut config = resolved_config.value;
    let config_provenance = resolved_config.provenance;

    if let Some(ref level_str) = cli.compaction {
        match level_str.parse::<wcore_compact::CompactionLevel>() {
            Ok(level) => config.compact.compaction = level,
            Err(e) => anyhow::bail!("Invalid --compaction value: {e}"),
        }
    }
    if cli.toon {
        config.compact.toon = true;
    }
    // F-092 (W7-N): --online-evolution CLI flag overrides (enables) the
    // config gate. Merging is OR-based — the flag can only turn the feature
    // on, not off; users who want it always-on use the config file.
    if cli.online_evolution {
        config.observability.online_evolution = true;
    }
    // Rank 47: --no-memory forces a stateless run by disabling long-term
    // memory before `config` reaches `AgentBootstrap`. OR-based like
    // --online-evolution: the flag can only turn memory off, never on.
    apply_no_memory_flag(&mut config, cli.no_memory);

    // B2 — install the process-global egress policy now that `config` is fully
    // resolved (base_url/provider/`[security]` are finalized above; the mutations
    // between here and dispatch only touch compaction/toon/online-evolution).
    // This is the chokepoint for every in-process run-path that follows:
    // json-stream/host mode, the interactive TUI, and headless/REPL. The install
    // is one-shot and idempotent, so doing it once here — before any agent
    // egress — is exactly right. Subcommands that early-return above (acp/swarm/
    // workflow/agent) never reach here: workflow installs from its own resolved
    // config; swarm runs workers as subprocesses that self-install on boot.
    wcore_agent::egress::install_egress_policy(&config);

    let cwd = std::env::current_dir()?.to_string_lossy().to_string();

    // Resolve the effective resume id. `--continue` (`-c`) picks the
    // most-recent session and feeds it through the exact same resume
    // path as an explicit `--resume <id>`; `clap`'s `conflicts_with_all`
    // already guarantees only one of `--resume` / `--continue` is set.
    let resume = resolve_resume(cli.resume.clone(), cli.continue_latest, &config)?;
    let execution = resolve_local_execution(
        &config,
        approval_bypass,
        dangerous_launch,
        dangerous_ttl_secs,
        cli.json_stream,
    )?;

    // Branch to JSON stream mode
    if cli.json_stream {
        let run_result = run_json_stream_mode(
            config,
            config_provenance,
            &cwd,
            resume,
            cli.session_id,
            execution,
            cli.assistant.clone(),
            cli.allow_host_workspace_grants,
            cli.allow_host_path_grants,
            cli.allow_host_budget_grants,
            runtime_engine_mode(cli.runtime_engine_mode),
            runtime_workspace_kind(cli.runtime_workspace_kind),
        )
        .await;
        let evidence_result = wcore_agent::egress::finalize_eval_egress_observer();
        run_result?;
        evidence_result?;
        return Ok(ExitCode::SUCCESS);
    }

    // Default-mode dispatch (T2.3): `wayland-core` on an interactive
    // terminal with no prompt opens the ratatui TUI. `--json-stream`
    // (handled above) and `-p`/headless (`prompt` non-empty, below) keep
    // their exact prior behaviour — the merge surface is just this
    // branch plus the new `tui/` module. The TUI is skipped when:
    //   * `--no-tui` was passed (explicit escape hatch), or
    //   * stdout is not a TTY (piped / redirected), or
    //   * `TERM=dumb` (a terminal that cannot drive a full-screen UI).
    // In every skipped case the existing line-based `repl_loop` runs.
    let prompt = cli.prompt.join(" ");
    let tui_capable = std::io::IsTerminal::is_terminal(&std::io::stdout())
        && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(true);
    if prompt.is_empty() && !cli.no_tui && tui_capable {
        run_tui_mode(
            config,
            &cwd,
            &workspace_for_trust,
            resume,
            cli.session_id,
            cli.assistant,
            false,
            execution,
            cli.search,
        )
        .await?;
        // B3: disarm the crash sentinel at the earliest known-clean point.
        // The Drop impl on `_sentinel_guard` also fires when `main` returns,
        // but an explicit early disarm closes the window between TUI exit
        // and any post-TUI cleanup (MCP shutdown etc.) before the outer signal
        // race has a chance to cancel the root future.
        if let Some(ref mut g) = _sentinel_guard {
            let _ = g.disarm();
        }
        return Ok(ExitCode::SUCCESS);
    }

    // F-028: bare REPL on non-TTY stdin hangs forever waiting for input
    // that never comes (piped/CI use). Detect and bail early with a clear
    // message rather than silently blocking. A non-empty prompt (headless
    // one-shot mode) is fine — it reads from the provided argument, not
    // from stdin. `--no-tui` on a non-TTY is also fine because the caller
    // explicitly opted into the line-REPL (they know what they're doing).
    // `--json-stream` is handled before this point and never hits here.
    if prompt.is_empty() && !cli.no_tui && !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        // Any flag this advice names is checked against the real clap
        // definition by `non_tty_advice_names_only_flags_that_do_what_it_says`.
        eprintln!("{NON_TTY_NO_PROMPT_ADVICE}");
        if cli.resume.is_some() || cli.continue_latest {
            eprintln!("{RESUME_NO_PROMPT_ADVICE}");
        }
        return Ok(ExitCode::FAILURE);
    }

    let provider_name = config.provider_label.clone();

    // Bootstrap engine with full feature initialization. Phase 1B-2 — this
    // build backs both the long-running interactive line-REPL and the
    // headless one-shot `-p` path; opt into inbound channel dispatch so the
    // primary interactive session listens to configured channels (the
    // short-lived one-shot simply aborts the subscriber on exit).
    let mut bootstrap = execution
        .apply(AgentBootstrap::new(config, &cwd, output.clone()))
        .plugin_provider_router(make_plugin_provider_router())
        .enable_inbound_dispatch(true);

    if let Some(resume_id) = &resume {
        let cfg = bootstrap.config();
        let session_mgr = session::SessionManager::new(
            cfg.session.directory.clone().into(),
            cfg.session.max_sessions,
        );
        let session = session_mgr.load_for_run(resume_id)?;
        terminal.formatter().session_info(&format!(
            "Resumed session {} ({} messages, {} model)",
            session.session.id,
            session.session.messages.len(),
            session.session.model
        ));
        bootstrap = bootstrap.resume(session);
    }

    let result = bootstrap.build().await?;
    let mut engine = result.engine;

    // FluxRouter web_search grounding (contract §5): enable per-turn grounding
    // when `--search` is set. A no-op unless the active model is a Flux tier
    // alias (the provider guards on `is_flux_tier_alias`).
    if cli.search {
        engine.set_web_search(true);
    }

    if resume.is_none() {
        engine.init_session(&provider_name, &cwd, cli.session_id.as_deref())?;
    }

    // A resumed session may carry a turn a crash cut in half. Nothing on this
    // path used to consult the recovery plan, so the next message hit
    // `AgentEngine::run`'s fail-closed gate — and every remedy that gate names
    // was unreachable from here (`session reconcile` only lists the item,
    // `session cancel` refuses because it is outstanding), which left a killed
    // job unresumable for the rest of its life. That is job corpus row B-1:
    // ten kill boundaries, ten losses, zero duplication. Settle the
    // interrupted turn first, say out loud what was in flight, and carry the
    // same account into the model's next turn so it verifies the world instead
    // of assuming it. The TUI and `--json-stream` paths return above this
    // point and keep driving recovery through their own explicit surfaces.
    let interruption_briefing = if resume.is_some() {
        match engine.settle_interrupted_turn_for_resume().await {
            Ok(Some(report)) => {
                let briefing = report.briefing();
                terminal.formatter().session_info(&briefing);
                Some(briefing)
            }
            Ok(None) => None,
            Err(error) => {
                output.emit_error(
                    &format!(
                        "the interrupted turn from the previous run could not be settled: {error}"
                    ),
                    false,
                    wcore_protocol::events::FailureCategory::LocalWayland,
                );
                None
            }
        }
    } else {
        None
    };
    // The briefing is part of what the model is asked this turn, not a note
    // printed beside it: a resumed job that is never told it was interrupted
    // has no reason to re-check anything.
    let prompt = match &interruption_briefing {
        Some(briefing) if !prompt.is_empty() => format!("{briefing}\n\n{prompt}"),
        _ => prompt,
    };
    // Move session-tier memory off the bootstrap "boot" DB onto the real
    // per-session file, now that the session id is known.
    engine.rebind_memory_session().await;
    // Fire SessionStart plugin hooks once, now that the session is initialized.
    engine.run_session_start_hooks().await;

    // v0.8.0 N.1+N.2+N.3 — construct the runtime slash dispatcher once
    // per session. The dispatcher reaches the engine's wired-up
    // MemoryApi, plugin runtime handles, and SkillCatalog so /memory,
    // /plugin, /skill all hit real surfaces (not the old v0.7.0
    // placeholder strings).
    let slash_dispatcher = build_slash_dispatcher(&engine);

    // `prompt` is resolved above the TUI dispatch (the TUI-capable path
    // early-returns). Reaching here means either a non-empty prompt
    // (headless) or the TUI was skipped — both fall through to the REPL /
    // headless paths.
    // v0.6.4 Task 4.3: track the one-shot path's exit code so a styled error
    // from the engine surfaces through `OutputSink::emit_error` (Red Bold
    // `✗ Error: …` with anyhow chain handling — spec §3.5) instead of
    // anyhow's default `Debug` print on `main`'s `Result::Err`. The REPL
    // path already routes errors through `output.emit_error` so this brings
    // the one-shot path to parity.
    // ── F22C (Phase 22 Success Criterion 3): Direct's canonical transition ──
    //
    // The fifth loop owner. Attachment is by environment
    // (`WAYLAND_GOAL_ID` + `WAYLAND_GOAL_JOURNAL`) rather than by flag,
    // deliberately: this file is the shared multi-lane fence, and a flag pair
    // would mean a second, non-contiguous edit to add it to `Cli`. The env
    // route is already how this codebase hands a Goal's identity to a child
    // process (`goal_cmd::ENV_GOAL`), so it is the existing mechanism.
    //
    // Opt-in and headless-only: with no Goal in the environment, or in the
    // REPL, nothing below changes. `engine.run` here is THE production Direct
    // invocation — the closure wraps it, and its return type is
    // `StrategyTermination`, so there is no path out that terminates the Goal
    // any other way and none that terminates it zero times.
    if !prompt.is_empty()
        && let Some((driver, goal_id)) = wcore_cli::goal_cmd::GoalAttachArgs::default().resolve()?
    {
        use wcore_agent::goal::{DirectOutcome, StrategyTermination};
        // #946: this arm ended `return Ok(ExitCode::SUCCESS)`, so the exit-code
        // contract did not exist for a Goal-attached headless run AT ALL — a
        // turn-cap stop, a provider error and a run that answered nothing all
        // reported 0. The code is decided inside the closure (that is where the
        // run result lives) and read out after `run_direct` returns. Atomic
        // rather than a `Cell` because the closure's future must stay `Send`.
        let goal_exit_code =
            std::sync::Arc::new(std::sync::atomic::AtomicU8::new(wcore_cli::exit_code::OK));
        let exit_sink = std::sync::Arc::clone(&goal_exit_code);
        let cursor = driver
            .run_direct(&goal_id, |owner| async {
                // Bound to a `let` so the `&mut engine` borrow held by the
                // future ends here; the arms below need `&engine` for the
                // human latch.
                let run_outcome = engine.run(&prompt, "").await;
                let awaiting_human = engine.awaiting_human();
                match run_outcome {
                    Ok(run_result) => {
                        output.emit_stream_end(
                            "",
                            run_result.turns,
                            run_result.usage.input_tokens,
                            run_result.usage.output_tokens,
                            run_result.usage.cache_creation_tokens,
                            run_result.usage.cache_read_tokens,
                            run_result.finish_reason,
                        );
                        exit_sink.store(
                            wcore_cli::exit_code::for_run_outcome(
                                run_result.stop_reason,
                                run_result.finish_reason,
                                &run_result.text,
                                run_result.ended_on_unrecovered_tool_failure,
                                awaiting_human,
                            ),
                            std::sync::atomic::Ordering::SeqCst,
                        );
                        // A completed Direct run is UNCHECKED — Direct has no
                        // verification owner — so the adapter maps it to
                        // `NeedsEscalation`, not to a success category.
                        if run_result.stop_reason == wcore_types::message::StopReason::MaxTurns {
                            StrategyTermination::from_direct(
                                owner,
                                DirectOutcome::TurnLimitReached {
                                    turns: run_result.turns as u64,
                                },
                            )
                        } else {
                            StrategyTermination::from_direct(owner, DirectOutcome::Completed)
                        }
                    }
                    Err(error) => {
                        output.emit_error(
                            &format!("{error:#}"),
                            false,
                            wcore_protocol::events::FailureCategory::Unknown,
                        );
                        exit_sink.store(
                            wcore_cli::exit_code::FAILURE,
                            std::sync::atomic::Ordering::SeqCst,
                        );
                        StrategyTermination::from_direct(owner, DirectOutcome::Failed(&error))
                    }
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("goal {} did not terminate: {e}", goal_id.as_str()))?;
        wcore_cli::goal_cmd::print_canonical_transition(&driver, &goal_id, "direct", &cursor);
        engine.run_stop_hooks().await;
        shut_down_channels(&result.channel_manager).await;
        for mgr in &result.mcp_managers {
            mgr.shutdown().await;
        }
        return Ok(ExitCode::from(
            goal_exit_code.load(std::sync::atomic::Ordering::SeqCst),
        ));
    }

    let exit_code = if prompt.is_empty() {
        repl_loop(&mut engine, &terminal, &output, &slash_dispatcher).await?;
        ExitCode::SUCCESS
    } else {
        // v0.8.0 N.* — pre-process via the slash dispatcher first; only
        // forward to the engine when the input is NOT a known slash command.
        match handle_slash_or_run(&slash_dispatcher, &mut engine, &prompt, "", output.as_ref())
            .await
        {
            SlashOrRun::Slash => ExitCode::SUCCESS,
            SlashOrRun::Exit => ExitCode::SUCCESS,
            SlashOrRun::Engine(Ok(run_result)) => {
                output.emit_stream_end(
                    "",
                    run_result.turns,
                    run_result.usage.input_tokens,
                    run_result.usage.output_tokens,
                    run_result.usage.cache_creation_tokens,
                    run_result.usage.cache_read_tokens,
                    run_result.finish_reason,
                );
                // Row B-3 — say out loud that this stop is waiting on a person,
                // and how to pick it up. Without this the run is silent about
                // the one thing the operator has to do, and its exit code is
                // the only clue.
                let awaiting_human = engine.awaiting_human();
                if awaiting_human {
                    terminal
                        .formatter()
                        .session_info(&awaiting_human_notice(engine.current_session_id()));
                }
                // B3: the one-shot path used to return SUCCESS for every
                // completed `engine.run`, so a run stopped by the turn cap and
                // one that gave up on a failing tool both reported 0. The
                // contract lives in `wcore_cli::exit_code`.
                // #946: `stop_reason` alone is blind to a turn that ended in a
                // provider error and to a run that answered with nothing at
                // all, so both used to exit 0. Both are now inputs.
                ExitCode::from(wcore_cli::exit_code::for_run_outcome(
                    run_result.stop_reason,
                    run_result.finish_reason,
                    &run_result.text,
                    run_result.ended_on_unrecovered_tool_failure,
                    awaiting_human,
                ))
            }
            SlashOrRun::Engine(Err(e)) => {
                // Render the full anyhow chain (`{e:#}` flattens causes onto
                // `\nCaused by: …` lines which the formatter recognises).
                output.emit_error(
                    &format!("{e:#}"),
                    false,
                    wcore_protocol::events::FailureCategory::Unknown,
                );
                ExitCode::from(wcore_cli::exit_code::FAILURE)
            }
        }
    };

    engine.run_stop_hooks().await;

    shut_down_channels(&result.channel_manager).await;

    for mgr in &result.mcp_managers {
        mgr.shutdown().await;
    }

    Ok(exit_code)
}

/// Stop every configured channel before this path returns.
///
/// Row B-3. `AgentBootstrap::enable_inbound_dispatch(true)` starts inbound
/// polling for the one-shot and REPL paths, but only `gateway.rs` ever stopped
/// it again. An email channel polls IMAP from a `spawn_blocking` task that
/// returns only when its shutdown watch flips, and a blocking task that never
/// returns holds the whole runtime open at drop — so a run whose work was
/// finished never exited and was eventually killed. `stop_all` flips that
/// watch; the poll loop re-checks it every 100ms, so this is fast, and each
/// channel is bounded by its own grace period regardless.
///
/// Failures are logged, not propagated: this runs after the exit code is
/// already decided, and a channel that will not shut down cleanly must not
/// change the verdict on the work that was done.
async fn shut_down_channels(
    channels: &std::sync::Arc<
        tokio::sync::RwLock<wcore_channels_registry::wcore_channels::ChannelManager>,
    >,
) {
    if let Err(error) = channels.write().await.stop_all().await {
        tracing::warn!(
            target: "wcore_cli",
            %error,
            "channel shutdown reported an error; continuing to exit"
        );
    }
}

/// The operator-facing line for a run that stopped needing a human (row B-3).
///
/// Names the two things a person woken by this has to know: what the process
/// is waiting for, and the exact command that carries the work on. The session
/// is already durable — this reuses the ordinary `--resume` path rather than
/// introducing a second kind of saved state.
fn awaiting_human_notice(session_id: Option<String>) -> String {
    let resume = session_id.map_or_else(
        || "wayland-core --continue \"<your reply>\"".to_string(),
        |id| format!("wayland-core --resume {id} \"<your reply>\""),
    );
    format!(
        "Stopped: this run needs a person and the outbound route to one is \
         down, so nothing that changes the world was allowed to run. The work \
         so far is saved. Fix the channel or answer directly, then resume:\n  \
         {resume}"
    )
}

async fn repl_loop(
    engine: &mut wcore_agent::engine::AgentEngine,
    terminal: &Arc<TerminalSink>,
    output: &Arc<dyn OutputSink>,
    slash_dispatcher: &SlashDispatcher,
) -> anyhow::Result<()> {
    use std::io::{self, BufRead};

    loop {
        terminal.formatter().repl_prompt();

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() || input == "/quit" {
            break;
        }

        // v0.8.0 N.* — pre-process slash commands BEFORE the engine sees
        // the input. `/exit` is handled by the ExitHandler returning
        // SlashOutcome::Exit, which we surface as a break out of the loop.
        match handle_slash_or_run(slash_dispatcher, engine, input, "", output.as_ref()).await {
            SlashOrRun::Slash => {}
            SlashOrRun::Exit => break,
            SlashOrRun::Engine(Ok(result)) => {
                output.emit_stream_end(
                    "",
                    result.turns,
                    result.usage.input_tokens,
                    result.usage.output_tokens,
                    result.usage.cache_creation_tokens,
                    result.usage.cache_read_tokens,
                    result.finish_reason,
                );
            }
            SlashOrRun::Engine(Err(e)) => {
                output.emit_error(
                    &format!("{e:#}"),
                    false,
                    wcore_protocol::events::FailureCategory::Unknown,
                );
            }
        }
    }

    Ok(())
}

/// D011 boot-recovery helper: does a project-local config file exist in the
/// current working directory? Checks BOTH accepted layout forms the resolver
/// honours — the canonical file form `.wayland-core.toml` and the eval-harness
/// dir form `.wayland-core/config.toml` (see `wcore_config::config`'s private
/// `project_config_path`). Used to keep the onboarding swallow honest: a
/// populated project repo on a machine with no global config is NOT a fresh
/// install, so its (non-parse) resolve errors must not route to onboarding.
fn project_config_exists() -> bool {
    std::path::Path::new(".wayland-core.toml").exists()
        || std::path::Path::new(".wayland-core")
            .join("config.toml")
            .exists()
}

/// Resolve the effective resume id from the `--resume` / `--continue`
/// flags.
///
/// `--resume <id>` is returned verbatim. `--continue` looks up the
/// most-recent session — the one with the latest `updated_at` — and
/// returns its id; with no saved sessions it is a hard error so the user
/// is not silently dropped into a fresh session. Neither flag set
/// returns `None` (a new session). `clap`'s `conflicts_with_all` already
/// guarantees the two flags are never both set.
fn resolve_resume(
    resume: Option<String>,
    continue_latest: bool,
    config: &Config,
) -> anyhow::Result<Option<String>> {
    if let Some(id) = resume {
        return Ok(Some(id));
    }
    if !continue_latest {
        return Ok(None);
    }
    let session_mgr = session::SessionManager::new(
        config.session.directory.clone().into(),
        config.session.max_sessions,
    );
    let latest = session_mgr
        .list()?
        .into_iter()
        .max_by_key(|s| s.updated_at)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--continue: no saved sessions to resume. Start a session first, \
                 or run `wayland-core --list-sessions`."
            )
        })?;
    Ok(Some(latest.id))
}

/// T2.3 — default-mode dispatch: run the ratatui TUI against a live
/// `AgentEngine`.
///
/// Structurally a sibling of [`run_json_stream_mode`]: it bootstraps the
/// engine with `AgentBootstrap`, installs the approval manager, and wires
/// the engine's event surfaces. The only difference is the event
/// destination — instead of a stdout `ProtocolWriter`, both halves
/// (`OutputSink` and the `protocol_writer`) forward into an in-process
/// `mpsc` channel the TUI drains:
///
///   * the engine's `OutputSink` is a `ChannelSink` (streaming events),
///   * the engine's `protocol_writer` is a `ChannelEmitter` (tool-
///     lifecycle + approval events) — installed via `set_protocol_writer`.
///
/// `engine.run` is then driven by the TUI's `TuiEngine` controller on a
/// background task, exactly as `run_json_stream_mode` drives it from its
/// command loop. The TUI owns the render loop until the user quits.
///
/// `force_onboarding` makes the TUI start on the Onboarding surface even
/// when a config already exists (the `wayland-core setup` re-entry
/// point). When `false` the first-run gate decides: Onboarding on a true
/// first run, Workspace otherwise.
fn approval_policy_to_session(policy: ApprovalPolicy) -> wcore_protocol::commands::SessionMode {
    use wcore_protocol::commands::SessionMode;
    match policy {
        ApprovalPolicy::Prompt => SessionMode::Default,
        ApprovalPolicy::AutoEdit => SessionMode::AutoEdit,
        ApprovalPolicy::Bypass => SessionMode::Force,
    }
}

enum WireModeChange {
    /// wayland#1088 — the local-opt-in gate turned the request down. Carries
    /// the mode that is STILL in force, because that is the half a host cannot
    /// derive: the refusal leaves the session where it was, and a host that
    /// assumed its request landed attributes the resulting all-categories gate
    /// storm to the engine.
    Rejected {
        effective: wcore_protocol::commands::SessionMode,
    },
    Unchanged,
    Changed(ExecutionPolicySnapshot),
}

/// Apply one untrusted wire mode request through the live authority gate and
/// advance the output-only producer sequence only when effective approvals
/// actually changed.
fn apply_wire_mode_change(
    approval_manager: &ToolApprovalManager,
    sequence: &mut ExecutionPolicySequence,
    mode: wcore_protocol::commands::SessionMode,
    effective_at_unix_ms: u64,
) -> Result<WireModeChange, ExecutionPolicySequenceError> {
    if !approval_manager.set_mode_from_wire(mode) {
        return Ok(WireModeChange::Rejected {
            effective: approval_manager.session_mode(),
        });
    }
    let policy = sequence
        .current()
        .policy
        .with_runtime_approvals(approval_manager.current_approval_policy());
    match sequence.advance_if_changed(
        policy,
        ExecutionPolicyChangeReason::ModeChange,
        effective_at_unix_ms,
    )? {
        Some(snapshot) => Ok(WireModeChange::Changed(snapshot.clone())),
        None => Ok(WireModeChange::Unchanged),
    }
}

/// GHSA-8r7g: the `WAYLAND_ALLOW_WIRE_FORCE` env opt-in that lets a protocol
/// peer request `SessionMode::Force`. Mirrors the env-based operator opt-ins
/// elsewhere (e.g. `WAYLAND_ALLOW_NO_SANDBOX`). Truthy = `1` / `true`.
fn wire_force_opt_in_env() -> bool {
    std::env::var("WAYLAND_ALLOW_WIRE_FORCE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)] // One explicit argument per host-controlled launch input.
async fn run_tui_mode(
    config: Config,
    cwd: &str,
    workspace_root: &std::path::Path,
    resume: Option<String>,
    session_id: Option<String>,
    active_assistant: Option<String>,
    force_onboarding: bool,
    execution: LocalExecutionSelection,
    web_search: bool,
) -> anyhow::Result<()> {
    use wcore_cli::tui;

    // Eager model-cache warm: fetch live model lists for connected providers at
    // startup so the FIRST `/model` open is already fresh — the lazy on-open
    // refresh only helps the *next* open. Run it on a DEDICATED OS thread with
    // its own current-thread runtime, NOT a `tokio::spawn` on the engine
    // runtime: a slow or blocked engine boot (e.g. a host with strict egress
    // filtering that stalls a boot-time connect) must not starve the warm, and
    // the warm must not compete with boot for the engine's worker threads.
    // Uses the already-resolved `config` (cloned before it moves into the
    // bootstrap) so there is no redundant re-resolution. Best-effort: a thread-
    // spawn or HTTP failure simply leaves the cache as-is.
    {
        let warm_cfg = config.clone();
        let _ = std::thread::Builder::new()
            .name("model-warm".into())
            .spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(wcore_providers::model_catalog::refresh_connected(&warm_cfg));
                }
            });
    }

    // The status-bar snapshot is taken from the resolved config before
    // it is moved into the bootstrap. Keep approval-bypass launch authority
    // for rebinds; the status bar renders active posture from `App::mode`.
    let approval_policy = execution.approvals();
    let approval_bypass = matches!(approval_policy, ApprovalPolicy::Bypass);
    let mut config_view = tui::config_view_from(&config);
    config_view.force = approval_bypass;
    let context_view = tui::context_view_from(&config);
    let provider_name = config.provider_label.clone();

    // Snapshot the registered hooks BEFORE `config` is moved into the
    // bootstrap below. `/hooks` reads this immutable list (the dispatch is
    // synchronous; the live config is consumed by `AgentBootstrap`).
    let hooks_snapshot: Vec<tui::HookInfo> = {
        let h = &config.hooks;
        h.pre_tool_use
            .iter()
            .map(|d| tui::HookInfo {
                name: d.name.clone(),
                trigger: "pre-tool-use",
            })
            .chain(h.post_tool_use.iter().map(|d| tui::HookInfo {
                name: d.name.clone(),
                trigger: "post-tool-use",
            }))
            .chain(h.stop.iter().map(|d| tui::HookInfo {
                name: d.name.clone(),
                trigger: "stop",
            }))
            .collect()
    };
    // Snapshot the session store (also before `config` moves) so `/resume`
    // can list saved sessions from the same directory the engine persists to.
    let session_store_dir: std::path::PathBuf = config.session.directory.clone().into();
    let session_store_max = config.session.max_sessions;

    // First-run gate — see `tui::is_first_run` for why the config file alone is
    // not enough to answer this (UAT-TUI-UNIX F1).
    let first_run = tui::is_first_run(
        config::global_config_path().exists(),
        &config.api_key,
        &config.model,
    );

    // The single engine→TUI event channel. Three producers forward onto
    // it — the `ChannelSink` (streaming events), the `ChannelEmitter`
    // (tool-lifecycle + approval events), and the `TuiEngine` itself (the
    // synthetic `StreamEnd` after a turn). The TUI's bridge task drains
    // `rx`. The channel is unbounded so an event burst during a turn
    // never back-pressures the engine.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let output: Arc<dyn OutputSink> = Arc::new(tui::ChannelSink::new(tx.clone()));
    let approval_manager = Arc::new(ToolApprovalManager::new());
    // GHSA-8r7g: a protocol peer may escalate to Force only when this local
    // operator opted in at launch (--force or WAYLAND_ALLOW_WIRE_FORCE).
    approval_manager.set_allow_wire_force(approval_bypass || wire_force_opt_in_env());
    // Seed the initial approval posture from config (`[default] approval_mode`,
    // editable via /config); `--force` overrides to Force. When Force, the
    // TUI's approval modal never opens (no `ApprovalRequired` event) and the
    // status bar renders the live mode so later de-escalation is visible.
    approval_manager.set_mode(approval_policy_to_session(approval_policy));
    // #693 — replay the "always allow <tool>" grants the user made in earlier
    // sessions IN THIS WORKSPACE. The manager's always-allow set is in-memory,
    // so without this the durable grant `TuiEngine::approve` writes would never
    // be read back and the user would be re-prompted for a tool they already
    // answered. The policy file is user-global, so the workspace filter is what
    // keeps a grant made in another checkout from applying here; the key comes
    // from the same helper `TuiEngine` stamps a grant with, so the write and
    // the read cannot disagree.
    let learned_workspace = wcore_permissions::LearnedPolicy::workspace_key(workspace_root);
    match wcore_permissions::LearnedPolicy::default_path() {
        Ok(path) => tui::restore_always_allows(&approval_manager, &path, &learned_workspace),
        Err(error) => tracing::warn!(
            %error,
            "cannot resolve the permissions path; always-allow grants not restored"
        ),
    }

    // Phase 1B-2 — the interactive TUI is a primary long-running session, so
    // opt into inbound channel dispatch (the InboundSubscriber turns admitted
    // channel messages into agent turns for the lifetime of this session).
    let mut bootstrap = execution
        .apply(AgentBootstrap::new(config, cwd, output.clone()))
        // The ONLY production waiver of the MCP dial notice. The alt screen is
        // entered immediately below, precisely so the dial runs behind a
        // branded splash — this surface has already told the user, on a
        // surface built to say it, and a second line into a live frame is
        // noise. Every other entry point announces by default.
        .without_mcp_dial_notice(true)
        .active_assistant(active_assistant.clone())
        .with_approval_manager(approval_manager.clone())
        .plugin_provider_router(make_plugin_provider_router())
        .enable_inbound_dispatch(true);

    if let Some(resume_id) = &resume {
        let cfg = bootstrap.config();
        let session_mgr = session::SessionManager::new(
            cfg.session.directory.clone().into(),
            cfg.session.max_sessions,
        );
        let session = session_mgr.load_for_run(resume_id)?;
        bootstrap = bootstrap.resume(session);
    }

    // Enter the TUI terminal up front so the engine build — which connects
    // every configured + installed-plugin MCP server (bounded per-server) on
    // the boot critical path — runs behind a branded splash instead of a blank
    // terminal. Single alt-screen entry; the SAME terminal is handed to
    // `run_attached` below (entering it twice would corrupt the screen). The
    // RAII guard restores the terminal on any `?`-early-return between here and
    // `run_attached`.
    let mcp_count = bootstrap.config().mcp.servers.len();
    let (mut boot_terminal, boot_guard) = tui::enter()?;
    let result = tui::splash_while(&mut boot_terminal, mcp_count, bootstrap.build()).await?;
    let startup_capability_activations = result.capability_activations.clone();
    let effective_execution_policy = result.effective_execution_policy.clone();
    let execution_policy_sequence = if resume.is_some() {
        ExecutionPolicySequence::resume(effective_execution_policy, audit_unix_time_millis()?)
    } else {
        ExecutionPolicySequence::launch(effective_execution_policy, audit_unix_time_millis()?)
    };
    let workspace_policy_receipt = result.workspace_policy_receipt.clone();
    let mut engine = result.engine;

    // FluxRouter web_search grounding (contract §5): honour `--search`. A no-op
    // unless the active model is a Flux tier alias (provider-side guard).
    if web_search {
        engine.set_web_search(true);
    }

    // L2 / D016 boot parity: fold the `[default] user` display name into the
    // boot system prompt BEFORE the first turn, using the SAME helper +
    // name-block wording the rebind path uses (`build_rebind_system_prompt`).
    // Without this the name only reached the wire AFTER a rebind, so the very
    // first turn addressed the user anonymously.
    //
    // Wave-6 #5: install the name block as the rebind OVERLAY via
    // `set_system_prompt` (not `inject_history`). At this point the engine's
    // retained rebind base is the pure bootstrap-enriched prompt
    // (Constitution / persona / skills / config prompt). `set_system_prompt`
    // re-prepends the name overlay onto that retained base, so the boot prompt
    // is byte-identical to what a later `/config` rebind installs for the same
    // name — and a subsequent rebind REPLACES this overlay rather than stacking
    // a second name block (which a prepend-via-`inject_history` would do, since
    // it would also pollute the retained base with the name). A blank name
    // yields an empty overlay and is skipped.
    if let Some(name) = wcore_config::config::global_user_display_name() {
        let name_block = tui::build_rebind_system_prompt(None, Some(&name));
        if !name_block.trim().is_empty() {
            engine.set_system_prompt(name_block);
        }
    }

    if resume.is_none() {
        engine.init_session(&provider_name, cwd, session_id.as_deref())?;
    }
    // Move session-tier memory off the bootstrap "boot" DB onto the real
    // per-session file, now that the session id is known.
    engine.rebind_memory_session().await;
    // Fire SessionStart plugin hooks once, before the TUI loop begins.
    engine.run_session_start_hooks().await;

    // Resume repaint: when resuming, rebuild the restored conversation into
    // view models NOW (while the engine is still owned here) so the TUI can
    // seed its transcript and the user sees their history instead of a blank
    // screen. `conversation_messages()` is the restored session history
    // (`resume_with_provider` populated it). Empty for a fresh session.
    let (restored_turns, restored_tool_cards) = if resume.is_some() {
        tui::hydrate_history(engine.conversation_messages())
    } else {
        (Vec::new(), Vec::new())
    };

    // Install the approval manager + the channel-backed protocol writer.
    // The engine REQUIRES a protocol writer once an approval manager is
    // set (the per-turn `ApprovalChannel` emits `ToolRequest` /
    // `ApprovalRequired` through it) — so both must be wired together.
    // Wave 6 #24 — use the dedupe variant so a self-gating engine site (the
    // live-workflow gate emits `ToolRequest` + its own `ApprovalRequired`)
    // yields exactly ONE gate frame, not the synthesized + explicit pair (which
    // double-rang the terminal bell on the TUI and is malformed on ACP).
    engine.set_protocol_writer(Arc::new(tui::ChannelEmitter::with_dedupe(
        tx.clone(),
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        Some(engine.approval_bridge().clone()),
    )));
    for activation in startup_capability_activations {
        let _ = tx.send(wcore_protocol::events::ProtocolEvent::CapabilityActivation { activation });
    }
    let _ = tx.send(wcore_protocol::events::ProtocolEvent::ExecutionPolicy {
        snapshot: execution_policy_sequence.current().clone(),
    });
    let _ = tx.send(wcore_protocol::events::ProtocolEvent::WorkspacePolicy {
        policy: workspace_policy_receipt,
    });

    // Snapshot the loaded skills + MCP servers for the `/skills` and `/mcp`
    // listings. Taken here while `engine` and `result.mcp_managers` are still
    // owned (the engine moves into the controller on the next line). The
    // dispatch path is synchronous, so it reads this rather than locking the
    // engine's async mutex on the render thread.
    let skills_snapshot: Vec<tui::SkillInfo> = engine
        .skill_catalog()
        .map(|cat| {
            cat.visible()
                .into_iter()
                .map(|r| tui::SkillInfo {
                    name: r.name,
                    description: r.description,
                    user_invocable: r.user_invocable,
                })
                .collect()
        })
        .unwrap_or_default();
    // Snapshot EVERY attempted server (from `health()`), not just the live ones
    // (`server_names()`): a server that failed or timed out at connect has no
    // live entry but the user still needs to see why in `/mcp` and `/doctor`.
    let mut mcp_snapshot: Vec<tui::McpServerInfo> = Vec::new();
    for mgr in &result.mcp_managers {
        for (name, health) in mgr.health() {
            mcp_snapshot.push(tui::McpServerInfo {
                name: name.clone(),
                health: health.clone(),
            });
        }
    }
    // A4c: servers dropped by the pre-connect reachability gate never reach a
    // manager's `health()`, so surface them here as a distinct skipped (⊘) row.
    for (name, reason) in &result.skipped_mcp_servers {
        mcp_snapshot.push(tui::McpServerInfo {
            name: name.clone(),
            health: wcore_mcp::manager::McpServerHealth::Skipped {
                reason: reason.clone(),
            },
        });
    }

    // The `TuiEngine` controller keeps the last `tx` clone so it can
    // synthesize the `StreamEnd` the engine never emits itself.
    let mut tui_engine = tui::TuiEngine::new(engine, approval_manager, tx)
        .with_learned_policy_workspace(learned_workspace);
    tui_engine.set_active_assistant(active_assistant);
    tui_engine.set_inventory(tui::EngineInventory {
        skills: skills_snapshot,
        mcp_servers: mcp_snapshot,
        hooks: hooks_snapshot,
    });
    // The project root `/repomap` scans — the same cwd the engine bootstrapped in.
    tui_engine.set_repo_root(std::path::PathBuf::from(cwd));
    // The session store `/resume` lists from — the engine's own persist dir.
    tui_engine.set_session_store(session_store_dir, session_store_max);

    // Hand the TUI everything it needs: the engine controller, the event
    // receiver, and the status-bar snapshot.
    let session = tui::TuiSession {
        engine: tui_engine,
        events: rx,
        config: config_view,
        initial_mode: approval_policy_to_session(approval_policy),
        context: context_view,
        first_run,
        force_onboarding,
        restored_turns,
        restored_tool_cards,
    };
    // Hand the splash terminal (already in the alt-screen) + its guard to the
    // main loop — no second alt-screen entry.
    tui::run_attached(boot_terminal, boot_guard, Some(session)).await?;

    // The TUI has exited — shut MCP servers down cleanly.
    for mgr in &result.mcp_managers {
        mgr.shutdown().await;
    }
    Ok(())
}

/// W4 F19: run the skills audit. Loads the catalog from the current working
/// directory, computes findings, writes the JSON report to
/// `.wayland-core/skills-audit.json`, and renders the Markdown summary to
/// stdout.
async fn run_skills_audit(stale_after_days: u64) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let refs = wcore_skills::loader::load_catalog(&cwd, &[], false, None).await;
    let opts = wcore_skills::audit::AuditOpts {
        stale_after_days,
        ..Default::default()
    };
    let report = wcore_skills::audit::audit_corpus(&refs, &opts);

    // JSON file in .wayland-core/skills-audit.json (machine readable).
    let json_dir = cwd.join(".wayland-core");
    std::fs::create_dir_all(&json_dir)?;
    let json_path = json_dir.join("skills-audit.json");
    let json = serde_json::to_string_pretty(&report)?;
    wcore_config::atomic_write(&json_path, json.as_bytes())?;

    // Markdown to stdout (human readable).
    let md = wcore_skills::audit::render_markdown(&report);
    println!("{md}");

    Ok(())
}

/// 23A-C1: governed promotion. Delegates to `wcore_cli::skill_govern`, which binds one
/// reviewed artifact to one content digest, refuses revoked artifacts, and journals the
/// outcome — the transaction whose absence is why this function used to be a `bail!`.
async fn run_skills_promote(id: &str) -> anyhow::Result<()> {
    wcore_cli::skill_govern::run_promote(id).await
}

/// W9.1 T4 (T11): archive a Staged or Active procedure. The state-
/// machine (W9 T0.5 amendment) allows `Staged → Archived` directly so
/// curators can dismiss losing drafts without a detour through Active.
async fn run_skills_archive(id: &str) -> anyhow::Result<()> {
    transition_procedure(
        id,
        wcore_memory::v2_types::ProcedureStatus::Archived,
        "archive",
    )
    .await
}

/// Shared backend for `--skills-promote` and `--skills-archive`. Keeps
/// the open-memory + lookup + transition + upsert sequence in one
/// place so both commands report the same error shapes.
async fn transition_procedure(
    id_str: &str,
    next_status: wcore_memory::v2_types::ProcedureStatus,
    verb: &str,
) -> anyhow::Result<()> {
    use wcore_memory::v2_types::{AccessToken, ProcedureId, Tier};

    let parsed = uuid::Uuid::parse_str(id_str)
        .map_err(|e| anyhow::anyhow!("invalid procedure id '{id_str}': not a valid UUID ({e})"))?;
    let target_id = ProcedureId(parsed);

    let cwd = std::env::current_dir()?;
    // Session id is irrelevant for project-tier procedures — they're
    // stored in the project DB which is keyed solely on `project_root`.
    // Use a constant sentinel so repeated invocations share session-db
    // state on disk; the CLI doesn't read session-scoped procedures.
    let mem = wcore_memory::Memory::open(&cwd, "cli-skills-cmd")
        .await
        .map_err(|e| anyhow::anyhow!("failed to open project memory: {e}"))?;

    let procs = mem
        .api()
        .list_procedures(Tier::Project, AccessToken::System)
        .await
        .map_err(|e| anyhow::anyhow!("failed to list procedures: {e}"))?;
    let target = procs
        .into_iter()
        .find(|p| p.id == target_id)
        .ok_or_else(|| anyhow::anyhow!("no procedure with id '{id_str}' found at Tier::Project"))?;

    if !target.status.can_transition_to(next_status) {
        anyhow::bail!(
            "cannot {verb} procedure '{}' (id={id_str}): \
             {} → {} is not a valid transition",
            target.name,
            target.status.as_str(),
            next_status.as_str()
        );
    }

    let mut updated = target.clone();
    updated.status = next_status;
    mem.api()
        .upsert_procedure(updated, AccessToken::System)
        .await
        .map_err(|e| anyhow::anyhow!("failed to upsert procedure: {e}"))?;

    println!(
        "{verb}d procedure '{name}' (id={id_str}): {prev} → {next}",
        name = target.name,
        prev = target.status.as_str(),
        next = next_status.as_str()
    );
    Ok(())
}

/// M5.2: load a session trace from disk + optionally compare against a
/// second trace. The version-skew guard refuses traces recorded by a
/// different `wcore-core` build unless `force_version_skew` is `true`.
/// Output is plain text intended for human inspection.
fn run_replay(
    trace_path: &std::path::Path,
    diff_path: Option<&std::path::Path>,
    force_version_skew: bool,
) -> anyhow::Result<()> {
    // F-048: include the file path in I/O errors so users see which file
    // failed, not just a generic "couldn't open file: permission denied".
    let trace = wcore_replay::Trace::load_from_path(trace_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to load trace from '{}': {e:#}",
            trace_path.display()
        )
    })?;
    let runtime_version = env!("CARGO_PKG_VERSION");
    let replayer = wcore_replay::Replayer { force_version_skew };
    let events = replayer.dry_run(&trace, runtime_version)?;
    println!(
        "trace ok: {} events from session {}",
        events.len(),
        trace.session_id
    );
    if let Some(other_path) = diff_path {
        let other = wcore_replay::Trace::load_from_path(other_path)?;
        let diffs = wcore_replay::Differ::compare(&trace, &other);
        let changed = diffs
            .iter()
            .filter(|d| d.kind != wcore_replay::DiffKind::Unchanged)
            .count();
        println!("{} diff entries ({} changed)", diffs.len(), changed);
        for d in diffs
            .iter()
            .filter(|d| d.kind != wcore_replay::DiffKind::Unchanged)
        {
            println!(
                "  [{:?}] event #{}: {:?}",
                d.kind,
                d.index,
                d.left.as_ref().or(d.right.as_ref())
            );
        }
    }
    Ok(())
}

/// M3.4: dump the memory state for a given session id. Prints procedures
/// at project tier and user-model entries from the core partition. Always
/// echoes the session id so scripts can distinguish output even when the
/// session has no recorded data. Exits 0 in all success cases — the
/// format is plain text intended for human inspection and may change
/// between releases.
async fn run_memory_show(session: &str) -> anyhow::Result<()> {
    use wcore_memory::v2_types::{AccessToken, Tier};

    let cwd = std::env::current_dir()?;
    let mem = wcore_memory::Memory::open(&cwd, session)
        .await
        .map_err(|e| anyhow::anyhow!("failed to open memory for session '{session}': {e}"))?;

    println!("Session: {session}");
    println!();

    // Procedures (project tier). Episodes don't have a public list-by-
    // session API yet; M3.4 v1 ships procedures + user-model only and
    // a follow-up wave can extend with episodes once an `EpisodicPartition::list_by_session`
    // landing surface is agreed (see `crates/wcore-memory/src/api.rs`).
    let procs = mem
        .api()
        .list_procedures(Tier::Project, AccessToken::System)
        .await
        .map_err(|e| anyhow::anyhow!("failed to list procedures: {e}"))?;
    println!("Procedures (project tier): {} entries", procs.len());
    for p in &procs {
        println!(
            "  - {name} [{status}]  uses={success}/{total}",
            name = p.name,
            status = p.status.as_str(),
            success = p.success_count,
            total = p.use_count
        );
    }
    println!();

    // User model (Core partition).
    let user_model = mem
        .api()
        .user_model(AccessToken::System)
        .await
        .map_err(|e| anyhow::anyhow!("failed to read user model: {e}"))?;
    println!("User model: {} entries", user_model.entries.len());
    for entry in &user_model.entries {
        println!("  - {} = {}", entry.key, entry.value);
    }

    Ok(())
}

fn print_skills_paths() {
    use wcore_skills::paths::{
        project_commands_dirs, project_skills_dirs, user_commands_dir, user_skills_dir,
    };

    fn status(p: &Path) -> &'static str {
        if p.is_dir() { "exists" } else { "not found" }
    }

    // User-level
    match user_skills_dir() {
        Some(dir) => println!("User:    {}  ({})", dir.display(), status(&dir)),
        None => println!("User:    <unable to determine config directory>"),
    }

    // Project-level
    let cwd = std::env::current_dir().unwrap_or_default();
    let project_dirs = project_skills_dirs(&cwd);
    if project_dirs.is_empty() {
        println!("Project: <none found>");
    } else {
        for dir in &project_dirs {
            println!("Project: {}  ({})", dir.display(), status(dir));
        }
    }

    // Legacy commands
    let mut has_legacy = false;
    if let Some(dir) = user_commands_dir()
        && dir.is_dir()
    {
        println!("Legacy:  {}  ({})", dir.display(), status(&dir));
        has_legacy = true;
    }
    for dir in project_commands_dirs(&cwd) {
        println!("Legacy:  {}  ({})", dir.display(), status(&dir));
        has_legacy = true;
    }
    if !has_legacy {
        println!("Legacy:  <none found>");
    }
}

/// W6 B.7: build one `McpReady` event per server in an `McpManager`.
///
/// Used at boot to surface MCP server health to hosts. The dynamic
/// `AddMcpServer` path already emits these one-by-one; this helper
/// covers the boot-time path that previously emitted nothing,
/// regardless of which LLM provider the session uses.
///
/// Pure function — no IO, no protocol writer — so the boot-time
/// emission can be regression-tested without spinning up a full CLI
/// harness. Server iteration order is sorted by name so the event
/// sequence is deterministic for fixture-based tests and golden
/// streams.
fn registered_mcp_tool_names(
    registry: &wcore_tools::registry::ToolRegistry,
    server_name: &str,
) -> Vec<String> {
    let mut names: Vec<String> = registry
        .to_tool_defs()
        .into_iter()
        .filter(|tool| tool.server.as_deref() == Some(server_name))
        .map(|tool| tool.name)
        .collect();
    names.sort();
    names
}

fn mcp_ready_events_for(
    mgr: &McpManager,
    registry: &wcore_tools::registry::ToolRegistry,
) -> Vec<ProtocolEvent> {
    let mut per_server: HashMap<String, Vec<String>> = HashMap::new();
    // Include every connected server so tool-less servers still produce an
    // empty-tools `McpReady`, matching the dynamic AddMcpServer path.
    for server_name in mgr.server_names() {
        per_server.insert(
            server_name.clone(),
            registered_mcp_tool_names(registry, &server_name),
        );
    }
    let mut names: Vec<String> = per_server.keys().cloned().collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let tools = per_server.remove(&name).unwrap_or_default();
            ProtocolEvent::McpReady {
                name,
                tools,
                already_connected: false,
            }
        })
        .collect()
}

/// wayland#551 — companion to [`mcp_ready_events_for`]: one `McpFailed`
/// event per server whose connect failed or timed out, so a host whose
/// config MCP servers were dialed in the background still learns WHY a
/// server's tools never appeared (parity with the dynamic `AddMcpServer`
/// path's failure emit). Pure and name-sorted for deterministic tests.
fn mcp_failed_events_for(mgr: &McpManager) -> Vec<ProtocolEvent> {
    let mut entries: Vec<(&String, &McpServerHealth)> = mgr.health().iter().collect();
    entries.sort_by_key(|(name, _)| name.as_str());
    entries
        .into_iter()
        .filter_map(|(name, health)| {
            let reason = mcp_server_failure_reason(health)?;
            Some(ProtocolEvent::McpFailed {
                name: name.clone(),
                reason,
            })
        })
        .collect()
}

fn mcp_server_failure_reason(health: &McpServerHealth) -> Option<String> {
    match health {
        McpServerHealth::Failed { reason } | McpServerHealth::Skipped { reason } => {
            Some(reason.clone())
        }
        McpServerHealth::TimedOut {
            after,
            cleanup_error,
        } => {
            let cleanup = cleanup_error
                .as_ref()
                .map(|error| format!("; cleanup unverified: {error}"))
                .unwrap_or_default();
            Some(format!(
                "connect timed out after {}s{cleanup}",
                after.as_secs()
            ))
        }
        McpServerHealth::Ready { .. } => None,
    }
}

fn emit_runtime_diagnostics(
    command: &wcore_protocol::diagnostics::GetRuntimeDiagnosticsCommand,
    state: &RuntimeDiagnosticsState,
    lifecycle: &McpLifecycleCatalog,
    boot_managers: &[Arc<McpManager>],
    dynamic_managers: &[Arc<McpManager>],
    registry: &wcore_tools::registry::ToolRegistry,
    writer: &dyn ProtocolEmitter,
) {
    if let Some(event) = runtime_diagnostics_admission_rejection(command) {
        let _ = writer.emit(&event);
        return;
    }
    let managers: Vec<_> = boot_managers
        .iter()
        .chain(dynamic_managers)
        .cloned()
        .collect();
    let snapshot = state.snapshot(lifecycle, &managers, registry);
    let _ = writer.emit(&ProtocolEvent::RuntimeDiagnosticsSnapshot {
        diagnostics_version: command.diagnostics_version,
        request_id: command.request_id.clone(),
        snapshot,
    });
}

fn runtime_diagnostics_admission_rejection(
    command: &wcore_protocol::diagnostics::GetRuntimeDiagnosticsCommand,
) -> Option<ProtocolEvent> {
    let reason = if wcore_protocol::diagnostics::validate_runtime_diagnostics_version(
        command.diagnostics_version,
    )
    .is_err()
    {
        Some(wcore_protocol::diagnostics::RuntimeDiagnosticsUnavailableReason::UnsupportedVersion)
    } else if wcore_protocol::diagnostics::validate_runtime_diagnostics_request_id(
        &command.request_id,
    )
    .is_err()
    {
        Some(wcore_protocol::diagnostics::RuntimeDiagnosticsUnavailableReason::InvalidRequest)
    } else {
        None
    };
    reason.map(|reason| ProtocolEvent::RuntimeDiagnosticsUnavailable {
        diagnostics_version: command.diagnostics_version,
        supported_version: wcore_protocol::diagnostics::RUNTIME_DIAGNOSTICS_VERSION,
        request_id: command.request_id.clone(),
        reason,
    })
}

fn mcp_removal_receipt(
    command: &RemoveMcpServerCommand,
    outcome: McpRemovalOutcome,
    removed_tools: Vec<String>,
) -> ProtocolEvent {
    ProtocolEvent::McpRemovalResult {
        lifecycle_version: command.lifecycle_version,
        request_id: bounded_mcp_receipt_field(&command.request_id, MAX_MCP_REQUEST_ID_LEN),
        name: bounded_mcp_receipt_field(&command.name, MAX_MCP_SERVER_NAME_LEN),
        outcome,
        removed_tools,
    }
}

fn bounded_mcp_receipt_field(value: &str, max_bytes: usize) -> String {
    if !value.trim().is_empty() && value.len() <= max_bytes {
        value.to_string()
    } else {
        "<invalid>".to_string()
    }
}

fn mcp_removal_request_rejection(command: &RemoveMcpServerCommand) -> Option<McpRemovalOutcome> {
    if command.lifecycle_version != MCP_LIFECYCLE_VERSION {
        Some(McpRemovalOutcome::UnsupportedVersion)
    } else if mcp_removal_request_id_invalid(command)
        || command.name.trim().is_empty()
        || command.name.len() > MAX_MCP_SERVER_NAME_LEN
    {
        Some(McpRemovalOutcome::InvalidRequest)
    } else {
        None
    }
}

fn mcp_removal_request_id_invalid(command: &RemoveMcpServerCommand) -> bool {
    command.request_id.trim().is_empty() || command.request_id.len() > MAX_MCP_REQUEST_ID_LEN
}

#[derive(Default)]
struct McpRemovalLedger {
    receipts: std::collections::HashMap<String, (RemoveMcpServerCommand, ProtocolEvent)>,
}

const MAX_MCP_REMOVAL_RECEIPTS: usize = 4096;
const MAX_MCP_REQUEST_ID_LEN: usize = 256;
const MAX_MCP_SERVER_NAME_LEN: usize = 256;
const MAX_MCP_CONFIG_VALUE_LEN: usize = 8 * 1024;
const MAX_MCP_CONFIG_ENTRIES: usize = 256;

#[allow(clippy::too_many_arguments)]
fn mcp_add_request_rejection(
    name: &str,
    transport: &str,
    command: Option<&str>,
    args: Option<&[String]>,
    env: Option<&HashMap<String, String>>,
    url: Option<&str>,
    headers: Option<&HashMap<String, String>>,
) -> Option<&'static str> {
    if name.trim().is_empty() || name.len() > MAX_MCP_SERVER_NAME_LEN {
        return Some("server name is empty or too long");
    }
    if transport.trim().is_empty() || transport.len() > 32 {
        return Some("transport is empty or too long");
    }
    if command.is_some_and(|value| value.len() > MAX_MCP_CONFIG_VALUE_LEN)
        || url.is_some_and(|value| value.len() > MAX_MCP_CONFIG_VALUE_LEN)
    {
        return Some("command or URL is too long");
    }
    if args.is_some_and(|values| {
        values.len() > MAX_MCP_CONFIG_ENTRIES
            || values
                .iter()
                .any(|value| value.len() > MAX_MCP_CONFIG_VALUE_LEN)
    }) {
        return Some("argument list exceeds the MCP request limit");
    }
    if [env, headers].into_iter().flatten().any(|values| {
        values.len() > MAX_MCP_CONFIG_ENTRIES
            || values.iter().any(|(key, value)| {
                key.len() > MAX_MCP_SERVER_NAME_LEN || value.len() > MAX_MCP_CONFIG_VALUE_LEN
            })
    }) {
        return Some("environment or header map exceeds the MCP request limit");
    }
    None
}

impl McpRemovalLedger {
    fn is_full_for_new(&self, request_id: &str) -> bool {
        !self.receipts.contains_key(request_id) && self.receipts.len() >= MAX_MCP_REMOVAL_RECEIPTS
    }

    fn replay_or_conflict(&self, command: &RemoveMcpServerCommand) -> Option<ProtocolEvent> {
        let (bound_command, receipt) = self.receipts.get(&command.request_id)?;
        if bound_command == command {
            Some(receipt.clone())
        } else {
            Some(mcp_removal_receipt(
                command,
                McpRemovalOutcome::RequestIdConflict,
                Vec::new(),
            ))
        }
    }

    fn record(&mut self, command: &RemoveMcpServerCommand, receipt: &ProtocolEvent) {
        if !command.request_id.trim().is_empty()
            && !command.name.trim().is_empty()
            && command.request_id.len() <= MAX_MCP_REQUEST_ID_LEN
            && command.name.len() <= MAX_MCP_SERVER_NAME_LEN
        {
            self.receipts
                .entry(command.request_id.clone())
                .or_insert_with(|| (command.clone(), receipt.clone()));
        }
    }
}

fn emit_mcp_removal_receipt(
    command: &RemoveMcpServerCommand,
    outcome: McpRemovalOutcome,
    removed_tools: Vec<String>,
    ledger: &mut McpRemovalLedger,
    writer: &dyn ProtocolEmitter,
) {
    let receipt = mcp_removal_receipt(command, outcome, removed_tools);
    ledger.record(command, &receipt);
    let _ = writer.emit(&receipt);
}

fn mcp_removal_cleanup_outcome(cleanup_failures: &[String]) -> McpRemovalOutcome {
    if cleanup_failures.is_empty() {
        McpRemovalOutcome::Removed
    } else {
        McpRemovalOutcome::CleanupUnverified
    }
}

/// Withdraw a runtime-added MCP server from the live catalogue refresh.
///
/// FerroxLabs/wayland#1213 c4. `McpCatalogRefresh` is what turns a server's
/// `notifications/tools/list_changed` back into registered tools, and until
/// wayland#1175 that machinery was a no-op for SSE and Streamable HTTP because
/// neither transport reported the notification. Now that both do, an entry the
/// operator removed but that stayed in the refresh is a resurrection path: the
/// tools of a server the host explicitly took away get re-registered into the
/// live registry on its next announcement.
///
/// `McpManager::close_server` marks the transport dead, and
/// `refresh_signalled_tools` skips dead transports, so today the withdrawal is
/// belt to that braces. It is done anyway because the criterion asks for it and
/// because the belt is the one that does not depend on cleanup having
/// succeeded: on the `CleanupUnverified` arm the manager is left in place, and
/// a transport whose `close()` could not be verified is exactly the one whose
/// liveness nobody should be trusting. It also stops the refresh accumulating a
/// dead entry, and its config, per add/remove cycle.
///
/// Called from every path that withdraws a runtime declaration; the pairing is
/// graded by `every_runtime_mcp_withdrawal_leaves_the_catalog_refresh`.
fn withdraw_runtime_mcp_from_refresh(engine: &wcore_agent::engine::AgentEngine, name: &str) {
    if let Some(refresh) = engine.mcp_catalog_refresh() {
        refresh.forget_runtime_server(name);
    }
}

/// wayland#1165 — the teardown half of `AddMcpServer { replace: true }`.
///
/// wayland#605 deliberately made a duplicate add of a READY server a no-op:
/// re-adding must never silently mutate a live connection, because a retry, a
/// reconnect or two hosts racing the same add would then tear a working server
/// down as a side effect. That guarantee is unchanged and is still the default.
/// This is the path for the operator who genuinely wants the new configuration
/// and said so, and it works by RELEASING the name — the caller then reserves a
/// fresh generation through the ordinary [`McpLifecycleCatalog::reserve`], so
/// the catalog's "a ready name keeps its generation" invariant is never
/// weakened, only routed around by an explicit remove-then-add.
///
/// Returns `Ok(())` when the name is free to reserve again — including when
/// there was nothing to tear down, which makes `replace` on an unknown name a
/// plain add. Returns `Err(reason)` when the caller must refuse the add
/// instead: a connect or a removal already in flight is interrupted by nobody,
/// and an unverified prior cleanup still owns the name.
async fn teardown_runtime_mcp_for_replace(
    name: &str,
    runtime_diagnostics: &mut RuntimeDiagnosticsState,
    lifecycle: &McpLifecycleCatalog,
    engine: &mut wcore_agent::engine::AgentEngine,
    dynamic_managers: &mut Vec<Arc<McpManager>>,
) -> Result<(), String> {
    // Nothing this process introduced is under this name: the add that follows
    // is an ordinary first connect.
    if !runtime_diagnostics.has_runtime_declaration(name) {
        return Ok(());
    }
    match lifecycle.snapshot(name) {
        None => return Ok(()),
        Some(snapshot) => match snapshot.state {
            McpLifecycleState::Ready | McpLifecycleState::Failed { .. } => {}
            McpLifecycleState::Connecting => {
                return Err(
                    "server is still connecting; replace would race the dial in flight".to_string(),
                );
            }
            McpLifecycleState::Stopping => {
                return Err("server is stopping; retry the replace once it has".to_string());
            }
            McpLifecycleState::CleanupUnverified { .. } => {
                return Err(
                    "prior transport cleanup is unverified; retry remove before replacing"
                        .to_string(),
                );
            }
        },
    }

    let defer_cold = engine.defer_cold_config();
    let Some(registry) = engine.registry_mut() else {
        return Err("registry busy".to_string());
    };
    let _ = lifecycle.mark_stopping(name);
    let _removed_tools = registry.remove_mcp_server(name);
    registry.refresh_tool_search_catalog(&defer_cold);

    let matching: Vec<_> = dynamic_managers
        .iter()
        .filter(|manager| manager.hosts_server(name) || manager.health().contains_key(name))
        .cloned()
        .collect();
    let mut cleanup_failures = Vec::new();
    for manager in &matching {
        if let Err(error) = manager.close_server(name).await {
            cleanup_failures.push(error.to_string());
        }
    }
    if !cleanup_failures.is_empty() {
        // The old transport may still be alive, so the name stays reserved and
        // the replace is refused rather than connecting a second child beside a
        // process nobody proved dead.
        let reason = format!(
            "MCP transport cleanup could not be verified: {}",
            cleanup_failures.join("; ")
        );
        let _ = lifecycle.mark_cleanup_unverified(name, reason.clone());
        // wayland#1234 -- WITHDRAW HERE TOO, on the arm the tree used to skip.
        //
        // The old rationale was "on CleanupUnverified the manager is left in
        // place and the name stays reserved, so nothing is withdrawn either".
        // It does not survive the state this arm actually leaves: the tools
        // were ALREADY taken out of the live registry above and are NOT put
        // back. CleanupUnverified means `close_server` could not be verified,
        // i.e. the transport may still be ALIVE -- so the manager stays in
        // McpCatalogRefresh, the server announces `tools/list_changed`, and the
        // tools the operator just removed are re-registered. That is #1234's
        // resurrection shape, on the one arm most likely to have a live
        // transport. Withdrawing here makes the refresh state agree with the
        // registry state on BOTH arms.
        withdraw_runtime_mcp_from_refresh(engine, name);
        return Err(reason);
    }
    dynamic_managers
        .retain(|manager| !(manager.hosts_server(name) || manager.health().contains_key(name)));
    runtime_diagnostics.remove_runtime_declaration(name);
    withdraw_runtime_mcp_from_refresh(engine, name);
    let _ = lifecycle.complete_stopping(name);
    Ok(())
}

/// Remove only a server introduced through the current process's host command.
/// Config/plugin declarations and profile-scoped OAuth state are outside this
/// authority and are never touched.
async fn remove_runtime_mcp_server(
    command: RemoveMcpServerCommand,
    removal_ledger: &mut McpRemovalLedger,
    runtime_diagnostics: &mut RuntimeDiagnosticsState,
    lifecycle: &McpLifecycleCatalog,
    engine: &mut wcore_agent::engine::AgentEngine,
    dynamic_managers: &mut Vec<Arc<McpManager>>,
    writer: &dyn ProtocolEmitter,
) {
    if mcp_removal_request_id_invalid(&command) {
        let _ = writer.emit(&mcp_removal_receipt(
            &command,
            McpRemovalOutcome::InvalidRequest,
            Vec::new(),
        ));
        return;
    }
    if let Some(receipt) = removal_ledger.replay_or_conflict(&command) {
        let _ = writer.emit(&receipt);
        return;
    }
    if removal_ledger.is_full_for_new(&command.request_id) {
        let _ = writer.emit(&mcp_removal_receipt(
            &command,
            McpRemovalOutcome::CapacityExceeded,
            Vec::new(),
        ));
        return;
    }
    if let Some(outcome) = mcp_removal_request_rejection(&command) {
        emit_mcp_removal_receipt(&command, outcome, Vec::new(), removal_ledger, writer);
        return;
    }

    let admission = if runtime_diagnostics.has_non_runtime_declaration(&command.name) {
        Some(McpRemovalOutcome::NotRuntimeManaged)
    } else if !runtime_diagnostics.has_runtime_declaration(&command.name) {
        Some(McpRemovalOutcome::AlreadyAbsent)
    } else {
        None
    };
    if let Some(outcome) = admission {
        emit_mcp_removal_receipt(&command, outcome, Vec::new(), removal_ledger, writer);
        return;
    }

    let defer_cold = engine.defer_cold_config();
    let Some(registry) = engine.registry_mut() else {
        emit_mcp_removal_receipt(
            &command,
            McpRemovalOutcome::RegistryBusy,
            Vec::new(),
            removal_ledger,
            writer,
        );
        return;
    };

    let _ = lifecycle.mark_stopping(&command.name);
    let removed_tools = registry.remove_mcp_server(&command.name);
    registry.refresh_tool_search_catalog(&defer_cold);

    let matching: Vec<_> = dynamic_managers
        .iter()
        .filter(|manager| {
            manager.hosts_server(&command.name) || manager.health().contains_key(&command.name)
        })
        .cloned()
        .collect();
    let mut cleanup_failures = Vec::new();
    for manager in &matching {
        if let Err(error) = manager.close_server(&command.name).await {
            cleanup_failures.push(error.to_string());
        }
    }
    let cleanup_outcome = mcp_removal_cleanup_outcome(&cleanup_failures);
    if cleanup_outcome != McpRemovalOutcome::Removed {
        let reason = format!(
            "MCP transport cleanup could not be verified: {}",
            cleanup_failures.join("; ")
        );
        let _ = lifecycle.mark_cleanup_unverified(&command.name, reason);
        // wayland#1234 -- WITHDRAW HERE TOO, on the arm the tree used to skip.
        //
        // The old rationale was "on CleanupUnverified the manager is left in
        // place and the name stays reserved, so nothing is withdrawn either".
        // It does not survive the state this arm actually leaves: the tools
        // were ALREADY taken out of the live registry above and are NOT put
        // back. CleanupUnverified means `close_server` could not be verified,
        // i.e. the transport may still be ALIVE -- so the manager stays in
        // McpCatalogRefresh, the server announces `tools/list_changed`, and the
        // tools the operator just removed are re-registered. That is #1234's
        // resurrection shape, on the one arm most likely to have a live
        // transport. Withdrawing here makes the refresh state agree with the
        // registry state on BOTH arms.
        withdraw_runtime_mcp_from_refresh(engine, &command.name);
        emit_mcp_removal_receipt(
            &command,
            cleanup_outcome,
            removed_tools,
            removal_ledger,
            writer,
        );
        return;
    }
    dynamic_managers.retain(|manager| {
        !(manager.hosts_server(&command.name) || manager.health().contains_key(&command.name))
    });
    runtime_diagnostics.remove_runtime_declaration(&command.name);
    withdraw_runtime_mcp_from_refresh(engine, &command.name);
    let _ = lifecycle.complete_stopping(&command.name);

    emit_mcp_removal_receipt(
        &command,
        McpRemovalOutcome::Removed,
        removed_tools,
        removal_ledger,
        writer,
    );
}

/// wayland#551 — integrate a background-connected config-MCP manager into
/// the LIVE engine. Registers the manager's tools (same collision handling
/// as boot via [`wcore_mcp::tool_proxy::register_mcp_tools`]), emits one
/// `McpReady` per connected server and one `McpFailed` per failed server,
/// and parks the manager in `dynamic_managers` so it stays alive.
///
/// wayland#551 — absorb a settled background config-MCP connect: on
/// success, integrate into the live engine (returning the parked pair when
/// the registry is momentarily borrowed so the caller can retry between
/// turns); on failure, surface the error with bootstrap's inline-connect
/// wording. Shared by the loop-top non-blocking poll and the select arm.
async fn note_deferred_mcp_connect(
    result: DeferredMcpConnectResult,
    runtime_diagnostics: Option<&mut RuntimeDiagnosticsState>,
    engine: &mut wcore_agent::engine::AgentEngine,
    writer: &ProtocolWriter,
    output: &Arc<dyn OutputSink>,
    dynamic_managers: &mut Vec<Arc<McpManager>>,
    late_mcp: &mut LateMcpBinder,
) -> Option<PendingDeferredMcp> {
    let DeferredMcpConnectResult {
        outcome,
        resolved,
        mut reservations,
    } = result;
    match outcome {
        Ok(mgr) => {
            if let Some(state) = runtime_diagnostics {
                for (name, evidence) in mgr.executable_readiness() {
                    state.record_executable_readiness(
                        wcore_protocol::diagnostics::McpDeclarationOrigin::EffectiveConfig,
                        name,
                        *evidence,
                    );
                }
            }
            let mgr = Arc::new(mgr);
            // wayland#562 — the `skill://` read is the only ASYNC part of
            // late-binding, so it happens here, before the sync integrate
            // step whose "registry borrowed, retry between turns" contract
            // must stay sync. The refs ride along on `PendingDeferredMcp` so a
            // retry never re-reads the server.
            let mut skill_refs = LateMcpBinder::skill_refs_for(&mgr).await;
            if integrate_deferred_mcp(
                engine,
                mgr.clone(),
                &resolved,
                &mut reservations,
                writer,
                dynamic_managers,
                late_mcp,
                &mut skill_refs,
            ) {
                None
            } else {
                Some(PendingDeferredMcp {
                    manager: mgr,
                    resolved,
                    reservations,
                    skill_refs,
                })
            }
        }
        Err(e) => {
            let reason = format!("MCP initialization error: {e}");
            for (_, reservation) in reservations {
                reservation.complete_failed(reason.clone());
            }
            output.emit_error(
                &reason,
                false,
                wcore_protocol::events::FailureCategory::LocalWayland,
            );
            None
        }
    }
}

/// Returns `false` when the registry Arc is currently borrowed (a turn is
/// mid-flight) — the caller parks the manager and retries at the next
/// between-turns boundary.
#[allow(clippy::too_many_arguments)]
fn integrate_deferred_mcp(
    engine: &mut wcore_agent::engine::AgentEngine,
    mgr: Arc<McpManager>,
    resolved_servers: &HashMap<String, McpServerConfig>,
    reservations: &mut HashMap<String, McpConnectionReservation>,
    writer: &ProtocolWriter,
    dynamic_managers: &mut Vec<Arc<McpManager>>,
    late_mcp: &mut LateMcpBinder,
    skill_refs: &mut Vec<SkillRef>,
) -> bool {
    let builtin_names = engine.tool_names();
    let defer_cold = engine.defer_cold_config();
    // wayland#1174 — this is the ONLY manager a `defer_config_mcp` session
    // ever has for its config-declared servers. Boot left the refresh empty,
    // so without this the whole session runs with `tools/list_changed`
    // ignored for every one of them. Taken before `registry_mut` borrows the
    // engine.
    let catalog_refresh = engine.mcp_catalog_refresh();
    let Some(reg) = engine.registry_mut() else {
        return false;
    };
    wcore_mcp::tool_proxy::register_mcp_tools(
        reg,
        &mgr,
        &builtin_names,
        resolved_servers,
        &defer_cold,
    );
    reg.refresh_tool_search_catalog(&defer_cold);
    for (name, reservation) in reservations.drain() {
        match mgr.health().get(&name).and_then(mcp_server_failure_reason) {
            Some(reason) => {
                reservation.complete_failed(reason);
            }
            None if mgr.health().contains_key(&name) => {
                reservation.complete_ready();
            }
            None => {
                reservation.complete_failed("connect outcome missing from MCP health report");
            }
        }
    }
    for event in mcp_ready_events_for(&mgr, reg) {
        let _ = writer.emit(&event);
    }
    for event in mcp_failed_events_for(&mgr) {
        let _ = writer.emit(&event);
    }
    // wayland#562 — tools are only one of the three boot-time consumers of a
    // config MCP manager. Bind the other two now: merge the server's
    // `skill://` skills into the live shared catalog (+ prompt listing) and
    // re-resolve the plugin hook dispatcher over the widened manager set.
    // Taken (not cloned) because everything above this point has committed —
    // the only early return is the borrowed-registry check at the top.
    let report = late_mcp.bind(engine, mgr.clone(), std::mem::take(skill_refs));
    if !report.skills_added.is_empty() {
        tracing::info!(
            target: "wcore_cli::mcp",
            skills = ?report.skills_added,
            prompt_updated = report.prompt_updated,
            "deferred config MCP: late-bound skills into the live session"
        );
    }
    if let Some(refresh) = catalog_refresh {
        refresh.register_runtime_server(&mgr, resolved_servers);
    }
    dynamic_managers.push(mgr);
    true
}

struct DeferredMcpConnectResult {
    outcome: Result<McpManager, wcore_mcp::transport::McpError>,
    resolved: HashMap<String, McpServerConfig>,
    reservations: HashMap<String, McpConnectionReservation>,
}

struct PendingDeferredMcp {
    manager: Arc<McpManager>,
    resolved: HashMap<String, McpServerConfig>,
    reservations: HashMap<String, McpConnectionReservation>,
    /// wayland#562 — `skill://` refs already read off `manager`. Held across
    /// the retry so a borrowed registry never costs a second server round
    /// trip (and never loses the skills).
    skill_refs: Vec<SkillRef>,
}

type DeferredMcpReceiver = tokio::sync::oneshot::Receiver<DeferredMcpConnectResult>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionCommandReadiness {
    Immediate,
    SettleDeferredMcp,
}

/// Classify the between-turn readiness boundary for a protocol command.
/// Setup and control commands remain immediate; only a provider-bound Message
/// must wait for the already-running configured-MCP handshake.
fn session_command_readiness(command: &ProtocolCommand) -> SessionCommandReadiness {
    if matches!(command, ProtocolCommand::Message { .. }) {
        SessionCommandReadiness::SettleDeferredMcp
    } else {
        SessionCommandReadiness::Immediate
    }
}

/// Settle the configured-MCP connect task at the actual provider-turn
/// boundary. Hosts may send setup commands (`InitHistory`, `SetMode`, etc.)
/// before their first `Message`; those commands must not let the message race
/// ahead of the already-running MCP handshake.
/// Wait for the deferred MCP dial to settle, and say so if it takes a while.
///
/// The session loop's readiness boundary reaches this; the boot dial in
/// `wcore_agent::bootstrap` reaches the same notice from the other side. One
/// notice, one budget, one deadline read from `wcore_mcp` — see
/// [`wcore_agent::mcp_dial_notice::announce_slow_mcp_dial`] for why a bounded
/// wait still has to be announced and why it cannot be a `tracing` line.
async fn await_deferred_mcp_connect(
    rx: DeferredMcpReceiver,
    output: &Arc<dyn OutputSink>,
) -> Result<DeferredMcpConnectResult, tokio::sync::oneshot::error::RecvError> {
    wcore_agent::mcp_dial_notice::announce_slow_mcp_dial(rx, output).await
}

#[allow(clippy::too_many_arguments)]
async fn settle_deferred_mcp_before_message(
    deferred_mcp_rx: &mut Option<DeferredMcpReceiver>,
    pending_deferred_mcp: &mut Option<PendingDeferredMcp>,
    engine: &mut wcore_agent::engine::AgentEngine,
    writer: &ProtocolWriter,
    output: &Arc<dyn OutputSink>,
    dynamic_managers: &mut Vec<Arc<McpManager>>,
    runtime_diagnostics: Option<&mut RuntimeDiagnosticsState>,
    late_mcp: &mut LateMcpBinder,
) -> bool {
    if let Some(rx) = deferred_mcp_rx.take()
        && let Ok(result) = await_deferred_mcp_connect(rx, output).await
    {
        *pending_deferred_mcp = note_deferred_mcp_connect(
            result,
            runtime_diagnostics,
            engine,
            writer,
            output,
            dynamic_managers,
            late_mcp,
        )
        .await;
    }

    if let Some(mut pending) = pending_deferred_mcp.take()
        && !integrate_deferred_mcp(
            engine,
            pending.manager.clone(),
            &pending.resolved,
            &mut pending.reservations,
            writer,
            dynamic_managers,
            late_mcp,
            &mut pending.skill_refs,
        )
    {
        *pending_deferred_mcp = Some(pending);
    }

    pending_deferred_mcp.is_none()
}

// One explicit argument per `add_mcp_server` wire field, so a field the host
// sends cannot be dropped on the way into the config without this signature
// changing. Collapsing them into a struct would hide exactly that.
#[allow(clippy::too_many_arguments)]
fn to_mcp_server_config(
    transport: &str,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    url: Option<String>,
    headers: Option<HashMap<String, String>>,
    allow_local: bool,
    allowed_tools: Option<Vec<String>>,
) -> Result<McpServerConfig, String> {
    let transport_type = match transport {
        "stdio" => TransportType::Stdio,
        "sse" => TransportType::Sse,
        "streamable-http" | "streamable_http" => TransportType::StreamableHttp,
        other => return Err(format!("unknown transport: {other}")),
    };
    Ok(McpServerConfig {
        transport: transport_type,
        command,
        args,
        env,
        url,
        headers,
        deferred: Some(false),
        allow_local,
        only_for_assistant: None,
        allowed_tools,
    })
}

fn scope_host_runtime_mcp(
    config: McpServerConfig,
    active_assistant: Option<&str>,
) -> Result<McpServerConfig, &'static str> {
    let active_assistant = active_assistant
        .filter(|assistant| !assistant.trim().is_empty())
        .ok_or("active assistant identity is required for a runtime MCP declaration")?;
    Ok(config.scoped_to_assistant(Some(active_assistant)))
}

fn resolve_live_mcp_credential_references(config: &mut McpServerConfig) -> Result<(), String> {
    if !wcore_config::mcp_cred_refs::server_has_credential_references(config) {
        return Ok(());
    }
    let resolved = Config::resolve(&CliArgs::default())
        .map_err(|error| format!("credentials config unavailable: {error}"))?;
    let store = resolved
        .open_credentials_store()
        .map_err(|error| format!("credentials store unavailable: {error}"))?;
    wcore_config::mcp_cred_refs::resolve_server_credential_refs(config, &*store)
        .map_err(|error| error.to_string())
}

/// Pending config fields: (model, thinking, thinking_budget, effort)
type PendingConfig = (
    Option<String>,
    Option<String>,
    Option<u32>,
    Option<String>,
    Option<String>,
);

/// D012 (P0 security) — a [`ProtocolEmitter`] that wraps the stdout
/// [`ProtocolWriter`] and makes the json-stream approval gate observable to a
/// host.
///
/// The engine's orchestration approval path
/// (`execute_tool_calls_with_approval`) emits a `ToolRequest` ONLY when a tool
/// actually needs human approval — auto-approved tools/grants and allow-listed
/// read-only tools skip the request entirely, and under a Force posture no
/// `ToolRequest` is emitted at all. So a `ToolRequest` reaching this writer
/// unambiguously means "the engine is parked on
/// `approval_manager.request_approval` for this call_id, awaiting a decision."
/// But `ToolRequest` serializes as `{"type":"tool_request",...}` — it carries
/// none of the approval vocabulary a host (or the D012 gate) looks for, so over
/// the bare stdout writer the gate was invisible: the tool was correctly parked
/// (fail-closed) yet the host could not tell a gated call from an
/// already-approved one. The TUI path got this via
/// `tui::ChannelEmitter`'s identical synthesis; the json-stream path used the
/// bare `ProtocolWriter` and did not.
///
/// This wrapper synthesizes a `ProtocolEvent::ApprovalRequired` right after
/// each `ToolRequest`, mirroring `ChannelEmitter`, so the host receives the
/// `approval_required` gate frame BEFORE the tool runs. Engine sites that
/// already emit an explicit `ApprovalRequired` for a call_id (the ForgeFlow
/// confirm gate in `engine.rs`) are de-duplicated: the synthesized id is
/// recorded so the engine's own subsequent `ApprovalRequired` for the same
/// call_id is suppressed, leaving exactly one gate frame per call.
struct GatingProtocolWriter {
    inner: Arc<dyn ProtocolEmitter>,
    /// The live approval posture. The gate frame is synthesized ONLY when the
    /// tool will actually be parked (`!is_auto_approved`); under Force (or for
    /// an auto-approved tool/grant) the engine auto-runs the tool, so emitting an
    /// `ApprovalRequired` would be a false gate the host would wait on forever.
    approval: Arc<ToolApprovalManager>,
    /// call_ids for which this writer already synthesized an
    /// `ApprovalRequired`, so a later explicit one from the engine for the same
    /// call is not double-emitted.
    synthesized: std::sync::Mutex<std::collections::HashSet<String>>,
    /// GHSA-8r7g: sync correlation→secret lookup so the synthesized gate frame
    /// carries the unguessable resume_token for bridge-backed approvals
    /// (crucible/egress), and EMPTY for a regular tool (no bridge entry).
    approval_bridge: Option<Arc<wcore_agent::approval::ApprovalBridge>>,
}

impl GatingProtocolWriter {
    fn new(
        inner: Arc<dyn ProtocolEmitter>,
        approval: Arc<ToolApprovalManager>,
        approval_bridge: Option<Arc<wcore_agent::approval::ApprovalBridge>>,
    ) -> Self {
        Self {
            inner,
            approval,
            synthesized: std::sync::Mutex::new(std::collections::HashSet::new()),
            approval_bridge,
        }
    }
}

impl ProtocolEmitter for GatingProtocolWriter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        // Suppress a duplicate explicit `ApprovalRequired` we already
        // synthesized for this call_id (the ForgeFlow gate emits one inline).
        if let ProtocolEvent::ApprovalRequired { call_id, .. } = event
            && self
                .synthesized
                .lock()
                .map(|s| s.contains(call_id))
                .unwrap_or(false)
        {
            return Ok(());
        }

        self.inner.emit(event)?;

        // Synthesize the host-visible gate frame after the `ToolRequest`.
        if let ProtocolEvent::ToolRequest {
            msg_id: _,
            call_id,
            tool,
        } = event
        {
            let reason = match tool.category {
                wcore_protocol::events::ToolCategory::Edit => "edit",
                wcore_protocol::events::ToolCategory::Exec => "exec",
                wcore_protocol::events::ToolCategory::Mcp => "mcp",
                wcore_protocol::events::ToolCategory::Info => "info",
            };
            // Only synthesize the gate when the tool will actually be parked.
            // Under Force (or an auto-approved tool/grant) the engine auto-runs
            // the tool, so a gate frame here would be a false gate.
            //
            // wayland#1195: the posture check alone is NOT the parked-ness
            // predicate. `execute_tool_calls_with_approval` parks on two
            // further grounds that no approval posture can lift — an
            // `AskUserQuestion` (which needs an answer, not a permission) and
            // a call the path-boundary classifier escalated (#1099) — and it
            // emits `ToolRequest` for both. Suppressing the gate frame there
            // left the engine parked on a request the host was never shown:
            // measured under `force`, an `AskUserQuestion` produced a
            // `tool_request` and then silence for the life of the turn. So the
            // suppression is skipped for exactly the reasons the engine parks
            // on regardless of posture. The TUI's `ChannelEmitter` never had
            // this hole; it synthesizes unconditionally.
            let parks_regardless_of_posture =
                tool.name == "AskUserQuestion" || tool.escalation.is_some();
            if parks_regardless_of_posture
                || !self.approval.is_auto_approved_tool_cmd(
                    reason,
                    Some(&tool.name),
                    tool.args.get("command").and_then(|value| value.as_str()),
                )
            {
                if let Ok(mut seen) = self.synthesized.lock() {
                    seen.insert(call_id.clone());
                }
                // Crucible Stage 4: the typed proposal card rides the
                // ToolRequest's `tool.args` (the explicit ApprovalRequired{plan}
                // is suppressed by the dedupe above), so carry it into the
                // host-visible synthesized frame. None for every other tool.
                let plan = tool.args.get("plan").and_then(|v| {
                    serde_json::from_value::<wcore_types::crucible::CruciblePlan>(v.clone()).ok()
                });
                // GHSA-8r7g: stamp the secret bridge token for bridge-backed
                // approvals (crucible/egress); EMPTY for a regular tool that
                // has no bridge entry and is resolved via ToolApprovalManager.
                let resume_token = self
                    .approval_bridge
                    .as_ref()
                    .and_then(|b| b.secret_for_correlation(call_id))
                    .unwrap_or_default();
                self.inner.emit(&ProtocolEvent::ApprovalRequired {
                    call_id: call_id.clone(),
                    resume_token,
                    correlation_id: call_id.clone(),
                    reason: reason.to_string(),
                    context: tool.description.clone(),
                    plan,
                })?;
            }
        }

        Ok(())
    }
}

/// The host's `grant_path` request, as received.
struct PathGrantRequest {
    grant_id: String,
    root: String,
    access: wcore_protocol::commands::PathGrantAccess,
    expires_at_ms: Option<u64>,
}

fn emit_path_grant(
    launch_authorized: bool,
    policy: &wcore_tools::workspace_policy::WorkspacePolicy,
    receipt: &mut wcore_types::workspace_trust::WorkspacePolicyReceipt,
    request: PathGrantRequest,
    writer: &dyn ProtocolEmitter,
) {
    let PathGrantRequest {
        grant_id,
        root,
        access,
        expires_at_ms,
    } = request;
    // #314 c5. This handler has exactly ONE refusal exit. Both causes -- the
    // launcher opt-in and the policy's own rejection -- are folded into the
    // `Err` of a single `Result` that only `emit_grant_refusal` consumes, so a
    // third cause added later cannot reach the wire as prose without going
    // through the typed frame. This replaced two hand-written `Info` refusals
    // that a host could only branch on by matching our English.
    let write = matches!(access, wcore_protocol::commands::PathGrantAccess::Write);
    let expires_at =
        expires_at_ms.map(|ms| std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms));
    let correlation = grant_id.clone();
    let outcome: Result<std::path::PathBuf, (GrantRefusalReason, String)> = if !launch_authorized {
        Err((
            GrantRefusalReason::LocalOptInRequired,
            "the local launcher did not opt in with --allow-host-path-grants".to_string(),
        ))
    } else {
        policy
            .grant_session_read_root_full(&root, write, Some(grant_id), expires_at)
            .map_err(|error| (GrantRefusalReason::PolicyRejected, error.to_string()))
    };
    match outcome {
        Ok(granted) => {
            // #314 D-1. The receipt is documented as unconditional after a
            // `grant_path` (json-stream-protocol.md 2.3.3) and
            // `emit_path_revoke` already emits it in BOTH its arms.
            emit_workspace_policy_receipt(policy, receipt, writer);
            // #1104: the parenthetical is the grant's own access, not a fixed
            // string. A write grant announced as "read-only" would be the
            // button-that-lies this whole surface exists to avoid, in the one
            // frame the user actually reads.
            let access = if write { "read and write" } else { "read-only" };
            let _ = writer.emit(&ProtocolEvent::Info {
                msg_id: String::new(),
                message: format!(
                    "folder granted for this session: {} ({access}; sandbox remains active)",
                    granted.display()
                ),
            });
        }
        Err((reason, detail)) => emit_grant_refusal(
            GrantSurface::Path,
            reason,
            Some(correlation),
            detail,
            policy,
            receipt,
            writer,
        ),
    }
}

/// #314 c5. The ONE place a host grant refusal reaches the wire.
///
/// Order matters and is the same order every other grant/revoke exit uses:
/// the `workspace_policy` receipt first (#314 c4 -- "what can this chat
/// reach"), then the typed `grant_refused` frame, then the human line. The
/// human line is BUILT FROM the typed frame rather than written beside it, so
/// the refusal prose no longer exists as a literal anywhere in this file and
/// a new refusal site has nothing to copy: it must construct the `Err` these
/// handlers return, and this function is its only consumer.
fn emit_grant_refusal(
    surface: GrantSurface,
    reason: GrantRefusalReason,
    grant_id: Option<String>,
    detail: String,
    policy: &wcore_tools::workspace_policy::WorkspacePolicy,
    receipt: &mut wcore_types::workspace_trust::WorkspacePolicyReceipt,
    writer: &dyn ProtocolEmitter,
) {
    emit_workspace_policy_receipt(policy, receipt, writer);
    let _ = writer.emit(&ProtocolEvent::GrantRefused {
        grant_id,
        surface,
        reason,
        detail: detail.clone(),
    });
    let _ = writer.emit(&ProtocolEvent::Info {
        msg_id: String::new(),
        message: format!("{}: {detail}", surface.refusal_prefix()),
    });
}

fn emit_path_revoke(
    policy: &wcore_tools::workspace_policy::WorkspacePolicy,
    receipt: &mut wcore_types::workspace_trust::WorkspacePolicyReceipt,
    grant_id: &str,
    writer: &dyn ProtocolEmitter,
) {
    // Deliberately NOT gated on the launcher opt-in. Taking authority away is
    // always allowed; requiring permission to revoke would mean a host that
    // launched without the flag could not clean up a grant it somehow held.
    let message = match policy.revoke_session_read_root(grant_id) {
        Some(root) => format!("folder access withdrawn: {}", root.display()),
        None => format!("no folder grant with id {grant_id} is held by this session"),
    };
    emit_workspace_policy_receipt(policy, receipt, writer);
    let _ = writer.emit(&ProtocolEvent::Info {
        msg_id: String::new(),
        message,
    });
}

/// Re-publish the policy receipt so a host's "what can this chat reach" view
/// is authoritative after every change to it, rather than only at startup.
fn emit_workspace_policy_receipt(
    policy: &wcore_tools::workspace_policy::WorkspacePolicy,
    receipt: &mut wcore_types::workspace_trust::WorkspacePolicyReceipt,
    writer: &dyn ProtocolEmitter,
) {
    receipt.readable_roots = policy
        .readable_roots()
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    receipt.capabilities = policy.developer_capabilities();
    let _ = writer.emit(&ProtocolEvent::WorkspacePolicy {
        policy: receipt.clone(),
    });
}

fn emit_workspace_capability_grant(
    launch_authorized: bool,
    policy: &wcore_tools::workspace_policy::WorkspacePolicy,
    receipt: &mut wcore_types::workspace_trust::WorkspacePolicyReceipt,
    executable: &str,
    writer: &dyn ProtocolEmitter,
) {
    // #314 c5 -- one refusal exit, as in `emit_path_grant`.
    let outcome = if !launch_authorized {
        Err((
            GrantRefusalReason::LocalOptInRequired,
            "the local launcher did not opt in with --allow-host-workspace-grants".to_string(),
        ))
    } else {
        policy
            .grant_session_capability(executable)
            .map_err(|error| (GrantRefusalReason::PolicyRejected, error.to_string()))
    };
    match outcome {
        Ok(capability) => {
            // Same receipt shape as every other grant/revoke exit -- built by
            // one helper so the four paths cannot drift apart.
            emit_workspace_policy_receipt(policy, receipt, writer);
            let _ = writer.emit(&ProtocolEvent::Info {
                msg_id: String::new(),
                message: format!(
                    "workspace capability granted for this session: {} (read-only; sandbox remains active)",
                    capability.executable
                ),
            });
        }
        Err((reason, detail)) => emit_grant_refusal(
            GrantSurface::WorkspaceCapability,
            reason,
            None,
            detail,
            policy,
            receipt,
            writer,
        ),
    }
}

const RECOVERY_PROTOCOL_VERSION: u16 = 1;

fn emit_recovery_unavailable(
    writer: &dyn ProtocolEmitter,
    request_id: String,
    session_id: String,
    reason: RecoveryUnavailableReason,
) {
    let _ = writer.emit(&ProtocolEvent::SessionRecoveryUnavailable {
        recovery_version: RECOVERY_PROTOCOL_VERSION,
        request_id,
        session_id,
        reason,
    });
}

fn handle_session_resync(
    engine: &wcore_agent::engine::AgentEngine,
    writer: &dyn ProtocolEmitter,
    recovery_version: u16,
    request_id: String,
    session_id: String,
    after: Option<wcore_protocol::events::RecoveryCursor>,
) {
    if recovery_version != RECOVERY_PROTOCOL_VERSION {
        emit_recovery_unavailable(
            writer,
            request_id,
            session_id,
            RecoveryUnavailableReason::UnsupportedVersion,
        );
        return;
    }
    if engine.current_session_id().as_deref() != Some(session_id.as_str()) {
        emit_recovery_unavailable(
            writer,
            request_id,
            session_id,
            RecoveryUnavailableReason::SessionNotFound,
        );
        return;
    }
    let (plan, replay) = match after.as_ref() {
        Some(cursor) => {
            let plan = match engine.recovery_plan_at(cursor) {
                Ok(plan) => plan,
                Err(reason) => {
                    emit_recovery_unavailable(writer, request_id, session_id, reason);
                    return;
                }
            };
            let items = match engine.recovery_replay_after(cursor) {
                Ok(items) => items,
                Err(reason) => {
                    emit_recovery_unavailable(writer, request_id, session_id, reason);
                    return;
                }
            };
            (plan, Some(items))
        }
        None => match engine.recovery_plan() {
            Ok(plan) => (plan, None),
            Err(_) => {
                emit_recovery_unavailable(
                    writer,
                    request_id,
                    session_id,
                    RecoveryUnavailableReason::JournalCorrupt,
                );
                return;
            }
        },
    };
    let (lifecycle, pending_turn) = plan.protocol_projection();
    let cursor = plan.cursor();
    let _ = writer.emit(&ProtocolEvent::SessionRecoverySnapshot {
        recovery_version: RECOVERY_PROTOCOL_VERSION,
        request_id: request_id.clone(),
        session_id: session_id.clone(),
        cursor: cursor.clone(),
        state_digest: plan.state_digest,
        lifecycle,
        pending_turn,
        budget: plan.budget,
    });
    if let Some(items) = replay {
        let through = items
            .last()
            .map(|item| item.cursor.clone())
            .or_else(|| after.clone())
            .unwrap_or_else(|| cursor.clone());
        let _ = writer.emit(&ProtocolEvent::SessionRecoveryReplay {
            recovery_version: RECOVERY_PROTOCOL_VERSION,
            request_id,
            session_id,
            from: after,
            through,
            items,
        });
    }
}

/// FerroxLabs/wayland#1070 — reason stamped on approvals denied because the
/// host's command stream reached EOF while they were parked. Distinct from the
/// TTL reaper's "approval timed out (no host response)", which means the host
/// is still connected but silent.
const HOST_EOF_DENY_REASON: &str = "host closed the command stream while this approval was pending";

/// FerroxLabs/wayland#1070 — the command stream reached EOF. No approval
/// decision can ever arrive after this, so every parked approval is resolved
/// as denied NOW rather than stalling for the rest of the 5-minute approval
/// TTL (live UAT measured a 330-second stall). Fails closed, matching the
/// reaper.
///
/// FerroxLabs/wayland#1083 — there are TWO approval stores, and #1070 drained
/// only one. `ToolApprovalManager` gates ordinary tool calls;
/// `wcore_agent::approval::ApprovalBridge` is a separate store, with its own
/// `by_token`/`by_corr` maps, that backs the egress-consent doorbell and the
/// Crucible proposal card. A bridge approval parked at EOF had no bulk escape
/// at all — only the TTL reaper, on a 30-second tick, needing the entry to
/// expire. A Crucible card is minted with `CRUCIBLE_APPROVAL_TTL` = 86,400s,
/// so the real symptom was not the ticket's 300-second stall but a
/// TWENTY-FOUR HOUR one. Both stores are drained here, together, so the
/// two can never drift apart again.
async fn deny_pending_approvals_on_host_eof(
    approval_manager: &ToolApprovalManager,
    approval_bridge: &wcore_agent::approval::ApprovalBridge,
) {
    let denied = approval_manager.deny_all_pending(HOST_EOF_DENY_REASON);
    if denied > 0 {
        tracing::warn!(
            denied,
            "host closed the command stream with approvals pending; denied them immediately"
        );
    }
    // #1083 criterion 3: the bridge does NOT reuse `HOST_EOF_DENY_REASON`. The
    // cause is stamped onto the outcome the waiter receives (it used to be a
    // free-form string that was logged and dropped), and
    // `ApprovalCancelCause::HostStreamClosed` owns the one reason string that
    // goes with it — distinct from this manager's EOF reason above and from the
    // TTL reason either store's reaper uses.
    let cancelled = approval_bridge
        .cancel_all_pending(wcore_agent::approval::ApprovalCancelCause::HostStreamClosed)
        .await;
    if cancelled > 0 {
        tracing::warn!(
            cancelled,
            "host closed the command stream with bridge approvals pending; \
             cancelled them immediately"
        );
    }
}

enum ActiveRecoveryOutcome<T> {
    Finished(T),
    Stopped(T),
}

/// Drive a recovered turn while preserving the same host approval boundary as
/// an ordinary active turn. Recovery can encounter more than one gated tool;
/// parking only on the engine future would strand later approval commands in
/// `cmd_rx` and deadlock the turn.
///
/// #1083: `approval_bridge` is the engine's shared
/// [`wcore_agent::approval::ApprovalBridge`], cloned out BEFORE `future` takes
/// its `&mut` borrow of the engine. It is a required parameter rather than an
/// `Option` on purpose — an EOF that drains one store and not the other is
/// exactly the defect this closes, so there is no caller shape that may skip
/// it.
async fn drive_active_recovery<F, T, C>(
    future: F,
    cmd_rx: &mut tokio::sync::mpsc::Receiver<ProtocolCommand>,
    approval_manager: &ToolApprovalManager,
    approval_bridge: &wcore_agent::approval::ApprovalBridge,
    writer: &dyn ProtocolEmitter,
    cancel_active_turn: &C,
) -> ActiveRecoveryOutcome<T>
where
    F: Future<Output = T>,
    C: Fn(),
{
    tokio::pin!(future);
    let mut commands_open = true;
    let mut stop_requested = false;
    loop {
        tokio::select! {
            biased;
            command = cmd_rx.recv(), if commands_open => match command {
                Some(ProtocolCommand::ToolApprove { call_id, scope, answer }) if !stop_requested => {
                    approval_manager.approve(&call_id, scope, answer);
                }
                Some(ProtocolCommand::ToolDeny { call_id, reason }) if !stop_requested => {
                    approval_manager.resolve(
                        &call_id,
                        ToolApprovalResult::Denied { reason },
                    );
                }
                Some(ProtocolCommand::Stop) if !stop_requested => {
                    cancel_active_turn();
                    stop_requested = true;
                }
                Some(ProtocolCommand::Ping) => {
                    let _ = writer.emit(&ProtocolEvent::Pong);
                }
                // wayland#896 — quiescence is a PROCESS-level operation, not a
                // turn-level one, so it is answered at every command site
                // rather than only between turns. A host that could take a
                // recovery point only while Core happened to be idle would be
                // back to guessing at quiescence, which is what this contract
                // removes.
                Some(quiesce_command @ (ProtocolCommand::QuiesceAcquire(_)
                | ProtocolCommand::QuiesceRelease(_)
                | ProtocolCommand::QuiesceStatus(_))) => {
                    for event in
                        wcore_cli::quiesce_control::handle_quiesce_control(&quiesce_command)
                    {
                        let _ = writer.emit(&event);
                    }
                }
                Some(ProtocolCommand::SessionResync(command)) => {
                    emit_recovery_unavailable(
                        writer,
                        command.request_id,
                        command.session_id,
                        RecoveryUnavailableReason::SnapshotUnavailable,
                    );
                }
                Some(ProtocolCommand::ResumeTurn(command)) => {
                    emit_recovery_unavailable(
                        writer,
                        command.request_id,
                        command.session_id,
                        RecoveryUnavailableReason::UnknownCriticalState,
                    );
                }
                Some(ProtocolCommand::ResolveInterruptedApproval(command)) => {
                    emit_recovery_unavailable(
                        writer,
                        command.request_id,
                        command.session_id,
                        RecoveryUnavailableReason::UnknownCriticalState,
                    );
                }
                Some(ProtocolCommand::ResolveUnknownToolEffect(resolution)) => {
                    let _ = writer.emit(&ProtocolEvent::Error {
                        msg_id: Some(resolution.tool_execution_id),
                        error: wcore_protocol::events::ErrorInfo {
                            code: "recovery_busy".to_string(),
                            message: "resolve_unknown_tool_effect refused while another recovery action is active; resync and retry".to_string(),
                            retryable: true,
                            // A refused host command. Nothing upstream is
                            // implicated: this process declined it.
                            category: wcore_protocol::events::FailureCategory::LocalWayland,
                        },
                    });
                }
                Some(_) => {
                    eprintln!("[protocol] Ignoring uncorrelated command during active recovery");
                }
                None => {
                    commands_open = false;
                    deny_pending_approvals_on_host_eof(approval_manager, approval_bridge).await;
                }
            },
            result = &mut future => {
                return if stop_requested {
                    ActiveRecoveryOutcome::Stopped(result)
                } else {
                    ActiveRecoveryOutcome::Finished(result)
                };
            }
        }
    }
}

fn emit_recovered_terminal(output: &dyn OutputSink, request_id: &str, finish_reason: FinishReason) {
    output.emit_stream_end(request_id, 0, 0, 0, 0, 0, finish_reason);
}

#[allow(clippy::too_many_arguments)] // Explicit wire fields remain visible at the authority gate.
async fn handle_resume_turn<C>(
    engine: &mut wcore_agent::engine::AgentEngine,
    writer: &dyn ProtocolEmitter,
    output: &dyn OutputSink,
    cmd_rx: &mut tokio::sync::mpsc::Receiver<ProtocolCommand>,
    approval_manager: &ToolApprovalManager,
    cancel_active_turn: &C,
    recovery_version: u16,
    request_id: String,
    session_id: String,
    turn_id: String,
    cursor: wcore_protocol::events::RecoveryCursor,
    action: ResumeTurnAction,
) where
    C: Fn(),
{
    if recovery_version != RECOVERY_PROTOCOL_VERSION {
        emit_recovery_unavailable(
            writer,
            request_id.clone(),
            session_id,
            RecoveryUnavailableReason::UnsupportedVersion,
        );
        emit_recovered_terminal(output, &request_id, FinishReason::Error);
        return;
    }
    if engine.current_session_id().as_deref() != Some(session_id.as_str()) {
        emit_recovery_unavailable(
            writer,
            request_id.clone(),
            session_id,
            RecoveryUnavailableReason::SessionNotFound,
        );
        emit_recovered_terminal(output, &request_id, FinishReason::Error);
        return;
    }
    let plan = match engine.recovery_plan() {
        Ok(plan) => plan,
        Err(_) => {
            emit_recovery_unavailable(
                writer,
                request_id.clone(),
                session_id,
                RecoveryUnavailableReason::JournalCorrupt,
            );
            emit_recovered_terminal(output, &request_id, FinishReason::Error);
            return;
        }
    };
    // `Abandon` is exempt, and this exemption is the whole feature. The cursor
    // gate refuses a host whose view of the session head has drifted, which is
    // correct for every action that goes on to ACT on that head. Abandon acts on
    // nothing: it exists precisely for the case where the host believes a turn
    // is running that the engine no longer holds, and that belief is what makes
    // the cursor stale in the first place. Gating it here would make the verb
    // unreachable in the only situation it was added for (#326), which is the
    // state Desktop cannot currently escape without restarting the app.
    //
    // The version and session gates above are NOT relaxed: those are about
    // addressing the right process at all, not about agreeing on turn state.
    if action != ResumeTurnAction::Abandon && plan.cursor() != cursor {
        emit_recovery_unavailable(
            writer,
            request_id.clone(),
            session_id,
            RecoveryUnavailableReason::CursorDigestMismatch,
        );
        emit_recovered_terminal(output, &request_id, FinishReason::Error);
        return;
    }

    let result = match action {
        ResumeTurnAction::Continue => {
            // #1083: clone the shared bridge handle BEFORE `future` takes
            // its `&mut` borrow of the engine.
            let approval_bridge = engine.approval_bridge().clone();
            let future = engine.resume_interrupted_turn(&turn_id, &cursor, &request_id);
            match drive_active_recovery(
                future,
                cmd_rx,
                approval_manager,
                &approval_bridge,
                writer,
                cancel_active_turn,
            )
            .await
            {
                ActiveRecoveryOutcome::Finished(result) => result.map(|result| {
                    emit_recovered_stream_end(output, &request_id, &result);
                    RecoveryLifecycle::Completed
                }),
                ActiveRecoveryOutcome::Stopped(result) => {
                    let terminal_if_ready = match result {
                        Ok(_) => RecoveryLifecycle::Completed,
                        Err(wcore_agent::engine::AgentError::UserAborted) => {
                            RecoveryLifecycle::Cancelled
                        }
                        Err(error) => {
                            output.emit_error(
                                &format!("resume_turn refused: {error}"),
                                false,
                                error.failure_category(),
                            );
                            emit_recovered_terminal(output, &request_id, FinishReason::Error);
                            emit_recovery_unavailable(
                                writer,
                                request_id,
                                session_id,
                                RecoveryUnavailableReason::UnknownCriticalState,
                            );
                            return;
                        }
                    };
                    emit_recovered_terminal(output, &request_id, FinishReason::Stop);
                    let next = match engine.recovery_plan() {
                        Ok(plan) => plan,
                        Err(_) => {
                            emit_recovery_unavailable(
                                writer,
                                request_id,
                                session_id,
                                RecoveryUnavailableReason::JournalCorrupt,
                            );
                            return;
                        }
                    };
                    let (lifecycle, reconcile_reason) =
                        interrupted_action_lifecycle(&next, terminal_if_ready);
                    let _ = writer.emit(&ProtocolEvent::TurnRecoveryLifecycle {
                        recovery_version: RECOVERY_PROTOCOL_VERSION,
                        session_id,
                        turn_id,
                        cursor: next.cursor(),
                        lifecycle,
                        reconcile_reason,
                    });
                    return;
                }
            }
        }
        ResumeTurnAction::Reconcile => engine
            .reconcile_interrupted_turn(&turn_id, &cursor)
            .await
            .map(|_| {
                emit_recovered_terminal(output, &request_id, FinishReason::Error);
                RecoveryLifecycle::Failed
            }),
        ResumeTurnAction::Cancel => {
            engine
                .cancel_interrupted_turn(&turn_id, &cursor)
                .await
                .map(|_| {
                    emit_recovered_terminal(output, &request_id, FinishReason::Stop);
                    RecoveryLifecycle::Cancelled
                })
        }
        // `cursor` is deliberately NOT forwarded. The command still carries
        // one because every recovery command does, but gating on it would
        // refuse precisely the case this verb exists for — a host and an
        // engine that disagree about the session head.
        ResumeTurnAction::Abandon => engine.abandon_interrupted_turn(&turn_id).await.map(|_| {
            emit_recovered_terminal(output, &request_id, FinishReason::Stop);
            RecoveryLifecycle::Cancelled
        }),
    };
    match result {
        Ok(lifecycle) => {
            let next = match engine.recovery_plan() {
                Ok(plan) => plan,
                Err(_) => {
                    emit_recovery_unavailable(
                        writer,
                        request_id,
                        session_id,
                        RecoveryUnavailableReason::JournalCorrupt,
                    );
                    return;
                }
            };
            let _ = writer.emit(&ProtocolEvent::TurnRecoveryLifecycle {
                recovery_version: RECOVERY_PROTOCOL_VERSION,
                session_id,
                turn_id,
                cursor: next.cursor(),
                lifecycle,
                reconcile_reason: None,
            });
        }
        Err(error) => {
            output.emit_error(
                &format!("resume_turn refused: {error}"),
                false,
                error.failure_category(),
            );
            let finish_reason = if matches!(error, wcore_agent::engine::AgentError::UserAborted) {
                FinishReason::Stop
            } else {
                FinishReason::Error
            };
            emit_recovered_terminal(output, &request_id, finish_reason);
            if matches!(action, ResumeTurnAction::Continue)
                && let Ok(next) = engine.recovery_plan()
                && next.cursor() != cursor
                && matches!(
                    next.disposition,
                    wcore_agent::recovery::RecoveryDisposition::Ready
                )
            {
                let lifecycle = if matches!(error, wcore_agent::engine::AgentError::UserAborted) {
                    RecoveryLifecycle::Cancelled
                } else {
                    RecoveryLifecycle::Failed
                };
                let _ = writer.emit(&ProtocolEvent::TurnRecoveryLifecycle {
                    recovery_version: RECOVERY_PROTOCOL_VERSION,
                    session_id,
                    turn_id,
                    cursor: next.cursor(),
                    lifecycle,
                    reconcile_reason: None,
                });
                return;
            }
            emit_recovery_unavailable(
                writer,
                request_id,
                session_id,
                RecoveryUnavailableReason::UnknownCriticalState,
            );
        }
    }
}

fn emit_recovered_stream_end(
    output: &dyn OutputSink,
    request_id: &str,
    result: &wcore_agent::engine::AgentResult,
) {
    output.emit_stream_end_full(
        request_id,
        result.turns,
        result.usage.input_tokens,
        result.usage.output_tokens,
        result.usage.cache_creation_tokens,
        result.usage.cache_read_tokens,
        result.finish_reason,
        result.active_window_percent,
        result.agent_run_id.as_deref(),
        Some(&result.usage_delta),
    );
}

fn interrupted_action_lifecycle(
    plan: &wcore_agent::recovery::RecoveryPlan,
    terminal_if_ready: RecoveryLifecycle,
) -> (RecoveryLifecycle, Option<RecoveryReconcileReason>) {
    let (durable_lifecycle, pending_turn) = plan.protocol_projection();
    if matches!(
        plan.disposition,
        wcore_agent::recovery::RecoveryDisposition::Ready
    ) {
        (terminal_if_ready, None)
    } else {
        (
            durable_lifecycle,
            pending_turn.and_then(|turn| turn.reconcile_reason),
        )
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_recovered_approval<C>(
    engine: &mut wcore_agent::engine::AgentEngine,
    writer: &dyn ProtocolEmitter,
    output: &dyn OutputSink,
    cmd_rx: &mut tokio::sync::mpsc::Receiver<ProtocolCommand>,
    approval_manager: &ToolApprovalManager,
    cancel_active_turn: &C,
    recovery_version: u16,
    request_id: String,
    session_id: String,
    turn_id: String,
    cursor: wcore_protocol::events::RecoveryCursor,
    approval_id: String,
    decision: wcore_protocol::commands::RecoveredApprovalDecision,
    answer: Option<String>,
) where
    C: Fn(),
{
    if recovery_version != wcore_protocol::commands::RECOVERED_APPROVAL_VERSION {
        emit_recovery_unavailable(
            writer,
            request_id.clone(),
            session_id,
            RecoveryUnavailableReason::UnsupportedVersion,
        );
        emit_recovered_terminal(output, &request_id, FinishReason::Error);
        return;
    }
    if engine.current_session_id().as_deref() != Some(session_id.as_str()) {
        emit_recovery_unavailable(
            writer,
            request_id.clone(),
            session_id,
            RecoveryUnavailableReason::SessionNotFound,
        );
        emit_recovered_terminal(output, &request_id, FinishReason::Error);
        return;
    }
    // #1083: clone the shared bridge handle BEFORE `future` takes its `&mut`
    // borrow of the engine.
    let approval_bridge = engine.approval_bridge().clone();
    let future = engine.resolve_interrupted_approval(
        &turn_id,
        &cursor,
        &approval_id,
        decision,
        answer.as_deref(),
        &request_id,
    );
    let result = match drive_active_recovery(
        future,
        cmd_rx,
        approval_manager,
        &approval_bridge,
        writer,
        cancel_active_turn,
    )
    .await
    {
        ActiveRecoveryOutcome::Finished(result) => result,
        ActiveRecoveryOutcome::Stopped(result) => {
            let terminal_if_ready = match result {
                Ok(_) => RecoveryLifecycle::Completed,
                Err(wcore_agent::engine::AgentError::UserAborted) => RecoveryLifecycle::Cancelled,
                Err(error) => {
                    output.emit_error(
                        &format!("resolve_interrupted_approval refused: {error}"),
                        false,
                        error.failure_category(),
                    );
                    emit_recovered_terminal(output, &request_id, FinishReason::Error);
                    emit_recovery_unavailable(
                        writer,
                        request_id,
                        session_id,
                        RecoveryUnavailableReason::UnknownCriticalState,
                    );
                    return;
                }
            };
            emit_recovered_terminal(output, &request_id, FinishReason::Stop);
            let next = match engine.recovery_plan() {
                Ok(plan) => plan,
                Err(_) => {
                    emit_recovery_unavailable(
                        writer,
                        request_id,
                        session_id,
                        RecoveryUnavailableReason::JournalCorrupt,
                    );
                    return;
                }
            };
            let (lifecycle, reconcile_reason) =
                interrupted_action_lifecycle(&next, terminal_if_ready);
            let _ = writer.emit(&ProtocolEvent::TurnRecoveryLifecycle {
                recovery_version: RECOVERY_PROTOCOL_VERSION,
                session_id,
                turn_id,
                cursor: next.cursor(),
                lifecycle,
                reconcile_reason,
            });
            return;
        }
    };
    match result {
        Ok(result) => {
            emit_recovered_stream_end(output, &request_id, &result);
            let next = match engine.recovery_plan() {
                Ok(plan) => plan,
                Err(_) => {
                    emit_recovery_unavailable(
                        writer,
                        request_id,
                        session_id,
                        RecoveryUnavailableReason::JournalCorrupt,
                    );
                    return;
                }
            };
            let _ = writer.emit(&ProtocolEvent::TurnRecoveryLifecycle {
                recovery_version: RECOVERY_PROTOCOL_VERSION,
                session_id,
                turn_id,
                cursor: next.cursor(),
                lifecycle: RecoveryLifecycle::Completed,
                reconcile_reason: None,
            });
        }
        Err(error) => {
            if matches!(
                decision,
                wcore_protocol::commands::RecoveredApprovalDecision::Deny
            ) && matches!(error, wcore_agent::engine::AgentError::UserAborted)
                && let Ok(next) = engine.recovery_plan()
                && next.cursor() != cursor
                && matches!(
                    next.disposition,
                    wcore_agent::recovery::RecoveryDisposition::Ready
                )
            {
                emit_recovered_terminal(output, &request_id, FinishReason::Stop);
                let _ = writer.emit(&ProtocolEvent::TurnRecoveryLifecycle {
                    recovery_version: RECOVERY_PROTOCOL_VERSION,
                    session_id,
                    turn_id,
                    cursor: next.cursor(),
                    lifecycle: RecoveryLifecycle::Cancelled,
                    reconcile_reason: None,
                });
                return;
            }
            output.emit_error(
                &format!("resolve_interrupted_approval refused: {error}"),
                false,
                error.failure_category(),
            );
            let finish_reason = if matches!(error, wcore_agent::engine::AgentError::UserAborted) {
                FinishReason::Stop
            } else {
                FinishReason::Error
            };
            emit_recovered_terminal(output, &request_id, finish_reason);
            emit_recovery_unavailable(
                writer,
                request_id,
                session_id,
                RecoveryUnavailableReason::UnknownCriticalState,
            );
        }
    }
}

fn handle_operator_tool_effect_resolution(
    engine: &wcore_agent::engine::AgentEngine,
    writer: &dyn ProtocolEmitter,
    output: &dyn OutputSink,
    command: ProtocolCommand,
) {
    let ProtocolCommand::ResolveUnknownToolEffect(_) = &command else {
        output.emit_error(
            "resolve_unknown_tool_effect refused: wrong command at dispatcher boundary",
            false,
            wcore_protocol::events::FailureCategory::LocalWayland,
        );
        return;
    };

    let ProtocolCommand::ResolveUnknownToolEffect(resolution) = command else {
        unreachable!("operator-resolution command was checked above");
    };
    if let Err(error) = engine.resolve_operator_tool_effect(&resolution) {
        output.emit_error(
            &format!("resolve_unknown_tool_effect refused: {error}"),
            false,
            error.failure_category(),
        );
        return;
    }

    let _ = writer.emit(&ProtocolEvent::UnknownToolEffectResolved { resolution });
}

#[allow(clippy::too_many_arguments)] // One explicit boundary argument per host-controlled posture.
async fn run_json_stream_mode(
    config: Config,
    config_provenance: wcore_config::resolution_provenance::ConfigResolutionProvenance,
    cwd: &str,
    resume: Option<String>,
    session_id: Option<String>,
    execution: LocalExecutionSelection,
    assistant: Option<String>,
    allow_host_workspace_grants: bool,
    allow_host_path_grants: bool,
    allow_host_budget_grants: bool,
    runtime_engine_mode: wcore_protocol::diagnostics::RuntimeEngineMode,
    runtime_workspace_kind: wcore_protocol::diagnostics::RuntimeWorkspaceKind,
) -> anyhow::Result<()> {
    let writer = Arc::new(ProtocolWriter::new());
    let approval_policy = execution.approvals();
    let approval_bypass = matches!(approval_policy, ApprovalPolicy::Bypass);

    // F-009: pre-compute cost_attribution from the config compat rows BEFORE
    // config is moved into AgentBootstrap. Bootstrap applies the same gate
    // (bootstrap.rs:1093-1097) but the result stays buried inside the engine.
    // The ProtocolSink gate at protocol_sink.rs:713-715 reads its own internal
    // `advertised` Arc — not the engine's — so it always saw `false` because
    // `with_advertised_capabilities` was never called here.
    //
    // Mirror the bootstrap gate exactly: cost_attribution = true iff the
    // active ProviderCompat has at least one non-None cost row. This is
    // evaluated before bootstrap so it applies to OpenAI/Anthropic (inline
    // cost rows) but NOT to openai-compat secondaries (F-026 fixes that gate
    // in bootstrap; the sink will receive the updated value once F-026 lands).
    let pre_bootstrap_cost_attribution = config.compat.cost_per_input_token.is_some()
        || config.compat.cost_per_output_token.is_some();
    let advertised_for_sink = Arc::new(wcore_config::tools::AdvertisedCapabilitiesConfig {
        cost_attribution: pre_bootstrap_cost_attribution,
        // F-092 (W7-N): mirror online_evolution into the sink's advertised
        // capabilities so the Ready event reflects the flag before bootstrap
        // runs (mirrors the cost_attribution pre-bootstrap pattern above).
        online_evolution: config.observability.online_evolution,
        ..Default::default()
    });

    // W1 Task 10: opt-in trace_event emission via [observability]
    // structured_traces. Default off so hosts that haven't learned about
    // the variant remain undisturbed (W0 host decoder contract).
    let protocol_sink = Arc::new(wcore_cli::json_stream_sink::build_json_stream_sink(
        writer.clone(),
        config.observability.structured_traces,
        advertised_for_sink,
    ));
    let approval_manager = Arc::new(ToolApprovalManager::new());
    // GHSA-8r7g: a protocol peer may escalate to Force only when this local
    // operator opted in at launch (--force or WAYLAND_ALLOW_WIRE_FORCE).
    approval_manager.set_allow_wire_force(approval_bypass || wire_force_opt_in_env());
    // wayland#241: seed the initial approval posture from config
    // (`[default] approval_mode`) via the shared `initial_session_mode`
    // helper, exactly like `run_tui_mode`. The json-stream path previously
    // only honored `--force`, so a config `approval_mode = "auto-edit"` /
    // `"force"` was silently ignored for the desktop host — every mutating
    // tool then waited on an approval the host never sent. `--force` still
    // overrides to Force (F-002).
    approval_manager.set_mode(approval_policy_to_session(approval_policy));
    let output: Arc<dyn OutputSink> = protocol_sink.clone();

    let provider_name = config.provider_label.clone();
    let mut runtime_diagnostics = RuntimeDiagnosticsState::from_launch(
        &config,
        &config_provenance,
        assistant.as_deref(),
        runtime_engine_mode,
        runtime_workspace_kind,
    );

    // wayland#551 — config-declared MCP connects must NOT gate the `ready`
    // frame: a slow/hung server eats up to the full 30s per-server connect
    // budget INSIDE bootstrap, and hosts (the desktop app) time out waiting
    // for ready at 30s — the chat never opens. Capture + cred-resolve the
    // servers now (config is moved into bootstrap next), tell bootstrap to
    // skip them, and dial them in the background right after ready goes out.
    // Resolution happens here on a clone, mirroring bootstrap's own connect
    // boundary: the long-lived config keeps the literal `${cred:...}`.
    // #111 — apply per-assistant MCP scoping to the DEFERRED path too (the
    // second injection choke point per the #613 completeness guardrail): a
    // server marked `only_for_assistant` must not be background-connected for a
    // non-matching assistant. Fail-closed when `assistant` is None.
    let scoped_config_servers = config.mcp.servers_for_assistant(assistant.as_deref());
    let (deferred_mcp_servers, credential_skips) = if scoped_config_servers.is_empty() {
        (None, Vec::new())
    } else {
        let resolution = match config.open_credentials_store() {
            Ok(store) => wcore_config::mcp_cred_refs::resolve_servers_for_connect_with_report(
                &scoped_config_servers,
                &*store,
            ),
            Err(_) => wcore_config::mcp_cred_refs::without_credential_references_with_report(
                &scoped_config_servers,
            ),
        };
        (Some(resolution.connectable), resolution.skipped)
    };
    for (name, _) in &credential_skips {
        runtime_diagnostics.record_preconnect_failure(
            wcore_protocol::diagnostics::McpDeclarationOrigin::EffectiveConfig,
            name,
            wcore_protocol::diagnostics::McpFailureCode::AuthenticationRequired,
        );
    }

    // Bootstrap engine with full feature initialization. Phase 1B-2 —
    // json-stream is a primary long-running host session (e.g. the Wayland
    // desktop app), so
    // opt into inbound channel dispatch.
    let mut bootstrap = execution
        .apply(AgentBootstrap::new(config, cwd, output.clone()))
        .with_approval_manager(approval_manager.clone())
        .plugin_provider_router(make_plugin_provider_router())
        .enable_inbound_dispatch(true)
        .active_assistant(assistant.clone())
        .defer_config_mcp(deferred_mcp_servers.is_some());

    if let Some(resume_id) = &resume {
        let cfg = bootstrap.config();
        let session_mgr = session::SessionManager::new(
            cfg.session.directory.clone().into(),
            cfg.session.max_sessions,
        );
        let session = session_mgr.load_for_run(resume_id)?;
        bootstrap = bootstrap.resume(session);
    }

    let result = match bootstrap.build().await {
        Ok(r) => r,
        Err(e) => {
            // #186: surface init failure to the json-stream host instead of a bare exit.
            // The claim keeps this specific message and stands the process-exit
            // chokepoint down, so the host receives exactly one error frame.
            if wcore_cli::startup_error::claim_startup_error_emission() {
                output.emit_error(
                    &init_failure_message(&e, &provider_name),
                    false,
                    wcore_protocol::events::FailureCategory::LocalWayland,
                );
            }
            return Err(e);
        }
    };
    let startup_capability_activations = result.capability_activations.clone();
    let mut execution_policy_sequence = if resume.is_some() {
        ExecutionPolicySequence::resume(
            result.effective_execution_policy.clone(),
            audit_unix_time_millis()?,
        )
    } else {
        ExecutionPolicySequence::launch(
            result.effective_execution_policy.clone(),
            audit_unix_time_millis()?,
        )
    };
    let session_control = result.cancel_root.clone();
    runtime_diagnostics.record_plugin_declarations(&result.plugin_mcp_declarations);
    // wayland#562 — the late-bind seam for config MCP servers deferred out of
    // `build()`. Moved out of `result` (which stays alive for its other
    // fields) so the command loop can hand each settled background manager to
    // it.
    let mut late_mcp = result.late_mcp;
    let mut engine = result.engine;
    let session_egress_policy = engine.egress_policy();
    let workspace_policy = engine
        .tools()
        .workspace_policy()
        .expect("bootstrap installs a workspace policy");
    let mut workspace_policy_receipt = result.workspace_policy_receipt.clone();
    // wayland#551 — declared-but-still-connecting servers count as MCP
    // capability on the ready frame; their tools register shortly after.
    let initial_has_mcp = result.has_mcp
        || deferred_mcp_servers
            .as_ref()
            .is_some_and(|servers| !servers.is_empty());
    let initial_has_plugins = result.has_plugins;
    // W8c.3 H.2: snapshot the plugin-derived capability set so the
    // protocol sink advertises `browser_suite` / `computer_use` flags
    // alongside `plugins` whenever the corresponding plugin shells
    // loaded during bootstrap.
    let initial_plugin_caps = result.plugin_capabilities.clone();

    if resume.is_none() {
        engine.init_session(&provider_name, cwd, session_id.as_deref())?;
    }
    // Move session-tier memory off the bootstrap "boot" DB onto the real
    // per-session file, now that the session id is known.
    engine.rebind_memory_session().await;
    // Fire SessionStart plugin hooks once, before the JSON-stream loop begins.
    engine.run_session_start_hooks().await;

    // v0.8.0 N.1+N.2+N.3 — wire the runtime slash dispatcher for the
    // protocol path. The protocol loop pre-processes incoming
    // `ProtocolCommand::Message` content through this dispatcher; only
    // non-slash input reaches `engine.run()`.
    let slash_dispatcher = build_slash_dispatcher(&engine);

    // F-093: surface the resolved user-model backend tag in the ready
    // event's capabilities so hosts and the desktop app can display it.
    if let Some(backend) = engine.user_model_backend() {
        protocol_sink.set_user_model_backend(backend.backend_tag());
    }
    let sid = engine.current_session_id();
    protocol_sink.emit_ready_with_plugins_and_policy(
        engine.compat(),
        initial_has_mcp,
        sid,
        &approval_manager.current_mode(),
        initial_has_plugins,
        &initial_plugin_caps,
        engine.advertised_capabilities(),
        Some(execution_policy_sequence.current().clone()),
    );
    // Startup succeeded and the host has its `ready`. Everything after this
    // belongs to the live session, whose errors the protocol sink reports, so
    // the startup-refusal chokepoint stands down here.
    wcore_cli::startup_error::mark_ready_emitted();
    for (name, reason) in &credential_skips {
        let _ = writer.emit(&ProtocolEvent::McpFailed {
            name: name.clone(),
            reason: reason.message().to_string(),
        });
    }
    let _ = writer.emit(&ProtocolEvent::ExecutionPolicy {
        snapshot: execution_policy_sequence.current().clone(),
    });
    let _ = writer.emit(&ProtocolEvent::WorkspacePolicy {
        policy: workspace_policy_receipt.clone(),
    });
    for activation in startup_capability_activations {
        output.emit_capability_activation(&activation);
    }

    // W6 B.7: emit McpReady for each boot-time MCP server. Previously
    // only the dynamic `AddMcpServer` command path (below) emitted this
    // event, so hosts running sessions with MCP servers configured at
    // boot — common for Gemini deployments where servers ship in the
    // user's wayland config — never saw MCP health for the boot set.
    // Provider-agnostic by design: nothing in this loop branches on
    // which LLM the session uses; the gap was uniform across providers
    // and showed up most visibly on Gemini because Gemini hosts rely on
    // the boot path more heavily.
    for mgr in &result.mcp_managers {
        for event in mcp_ready_events_for(mgr, &engine.tools()) {
            let _ = writer.emit(&event);
        }
    }

    let mcp_lifecycle = McpLifecycleCatalog::new();
    for mgr in &result.mcp_managers {
        for name in mgr.server_names() {
            if !mcp_lifecycle.seed_ready(name.clone(), McpConfigIdentity::UNKNOWN) {
                let _ = writer.emit(&ProtocolEvent::McpFailed {
                    name,
                    reason: "MCP lifecycle capacity exceeded; runtime management unavailable"
                        .to_string(),
                });
            }
        }
    }

    // wayland#551 — dial the deferred config MCP servers in the background,
    // AFTER ready is on the wire. The command loop integrates the manager
    // into the live engine between turns (see the select below) and emits
    // McpReady / McpFailed per server, exactly like the dynamic
    // `AddMcpServer` path. The host can open immediately; the first real
    // message queues behind this already-running handshake so its provider
    // request cannot race ahead with an incomplete configured-tool set.
    let mut deferred_mcp_rx = deferred_mcp_servers.and_then(|resolved| {
        let mut reserved_configs = HashMap::new();
        let mut reservations = HashMap::new();
        for (name, config) in resolved {
            let identity = McpConfigIdentity::for_server(&config);
            if let McpReservationOutcome::Acquired(reservation) =
                mcp_lifecycle.reserve(name.clone(), identity)
            {
                reserved_configs.insert(name.clone(), config);
                reservations.insert(name, reservation);
            }
        }
        if reserved_configs.is_empty() {
            return None;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let egress_policy = session_egress_policy.clone();
        tokio::spawn(async move {
            let outcome =
                McpManager::connect_all_with_policy(&reserved_configs, egress_policy).await;
            let _ = tx.send(DeferredMcpConnectResult {
                outcome,
                resolved: reserved_configs,
                reservations,
            });
        });
        Some(rx)
    });

    // D012 (P0 security): install the gating writer as the engine's
    // tool-lifecycle emitter so a gated mutating tool emits a host-visible
    // `ApprovalRequired` frame before it runs (the engine's orchestration gate
    // emits only `ToolRequest`, which carries no approval vocabulary). The raw
    // `writer` is still used directly below for the loop's own emissions
    // (McpReady / Info / ApprovalResume) — those are never `ToolRequest`, so
    // they need no synthesis.
    let gating_writer: Arc<dyn ProtocolEmitter> = Arc::new(GatingProtocolWriter::new(
        writer.clone(),
        approval_manager.clone(),
        Some(engine.approval_bridge().clone()),
    ));
    engine.set_protocol_writer(gating_writer);

    // W7.1 S4-3.2: capture a clone of the engine's shared ApprovalBridge so
    // the `ApprovalResume` command arm below can call `bridge.resolve(...)`
    // on the same instance the registered ScriptTool is awaiting against.
    // Bootstrap builds one bridge and hands it to both engine + ScriptTool.
    let approval_bridge = engine.approval_bridge().clone();

    // #537/#141: the host-delegated send_message correlation bridge. The
    // `HostSendMessageResult` command arms below (top-level AND mid-turn)
    // resolve through this handle so a `send_message` tool call parked on
    // `HostDelegatedTransport::send` unblocks with the host's outcome.
    let host_send_bridge = result.host_send_bridge.clone();

    // Wave SC SECURITY MAJOR: share the bridge's active-token redactor
    // with the protocol sink. The sink was built before the bridge
    // existed; `share_with` swaps the inner Arc<RwLock> pointer so
    // both sides observe the same set going forward. Streaming tool
    // output now has in-flight approval correlation ids redacted as
    // defense-in-depth.
    protocol_sink.share_token_redactor_with(&approval_bridge.redactor());

    let mut cmd_rx = spawn_stdin_reader(writer.clone());

    // --- Pre-message phase: accept AddMcpServer commands ---
    let mut dynamic_managers: Vec<Arc<McpManager>> = Vec::new();
    let mut mcp_removal_ledger = McpRemovalLedger::default();
    let mut first_cmd: Option<ProtocolCommand> = None;

    // wayland#551 — a deferred-MCP manager whose integration found the
    // registry borrowed; retried at the next between-turns boundary.
    let mut pending_deferred_mcp: Option<PendingDeferredMcp> = None;
    let mut budget_grants = BudgetGrantLedger::default();

    loop {
        // wayland#551 — the pre-message phase can park in recv() forever on
        // a quiet host, so the background connect result must be settled
        // here too, or McpReady/McpFailed health would wait on host
        // activity (live-verify caught exactly that).
        let cmd = tokio::select! {
            c = cmd_rx.recv() => match c {
                Some(c) => c,
                None => break,
            },
            res = async { deferred_mcp_rx.as_mut().expect("guarded by is_some").await },
                if deferred_mcp_rx.is_some() =>
            {
                deferred_mcp_rx = None;
                // Err = connect task dropped without sending (panic).
                if let Ok(result) = res {
                    pending_deferred_mcp = note_deferred_mcp_connect(
                        result,
                        Some(&mut runtime_diagnostics),
                        &mut engine,
                        &writer,
                        &output,
                        &mut dynamic_managers,
                        &mut late_mcp,
                    )
                    .await;
                }
                continue;
            }
        };
        match cmd {
            ProtocolCommand::GetRuntimeDiagnostics(command) => {
                emit_runtime_diagnostics(
                    &command,
                    &runtime_diagnostics,
                    &mcp_lifecycle,
                    &result.mcp_managers,
                    &dynamic_managers,
                    &engine.tools(),
                    writer.as_ref(),
                );
            }
            // wayland#896 — see the note at the recovery-loop arm. Available
            // before the first Message too: Desktop's recovery capture must not
            // have to start a turn to reach it.
            ref quiesce_command @ (ProtocolCommand::QuiesceAcquire(_)
            | ProtocolCommand::QuiesceRelease(_)
            | ProtocolCommand::QuiesceStatus(_)) => {
                for event in wcore_cli::quiesce_control::handle_quiesce_control(quiesce_command) {
                    let _ = writer.emit(&event);
                }
            }
            ProtocolCommand::AddMcpServer {
                name,
                transport,
                command,
                args,
                env,
                url,
                headers,
                allow_local,
                allowed_tools,
                replace,
            } => {
                if let Some(reason) = mcp_add_request_rejection(
                    &name,
                    &transport,
                    command.as_deref(),
                    args.as_deref(),
                    env.as_ref(),
                    url.as_deref(),
                    headers.as_ref(),
                ) {
                    output.emit_error(
                        &format!("AddMcpServer rejected: invalid request ({reason})"),
                        false,
                        wcore_protocol::events::FailureCategory::LocalWayland,
                    );
                    let safe_name = if name.len() <= MAX_MCP_SERVER_NAME_LEN {
                        name
                    } else {
                        "<invalid>".to_string()
                    };
                    let _ = writer.emit(&ProtocolEvent::McpFailed {
                        name: safe_name,
                        reason: format!("invalid request: {reason}"),
                    });
                    continue;
                }
                eprintln!(
                    "[mcp] AddMcpServer received: name={name}, transport={transport}, command_present={}",
                    command.is_some()
                );
                let mut config = match to_mcp_server_config(
                    &transport,
                    command,
                    args,
                    env,
                    url,
                    headers,
                    allow_local,
                    allowed_tools,
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        output.emit_error(
                            &format!("AddMcpServer '{name}': {e}"),
                            false,
                            wcore_protocol::events::FailureCategory::LocalWayland,
                        );
                        continue;
                    }
                };
                config = match scope_host_runtime_mcp(config, assistant.as_deref()) {
                    Ok(config) => config,
                    Err(reason) => {
                        output.emit_error(
                            &format!("AddMcpServer '{name}': {reason}"),
                            false,
                            wcore_protocol::events::FailureCategory::LocalWayland,
                        );
                        let _ = writer.emit(&ProtocolEvent::McpFailed {
                            name: name.clone(),
                            reason: reason.to_string(),
                        });
                        continue;
                    }
                };
                if let Err(error) = resolve_live_mcp_credential_references(&mut config) {
                    let reason = format!("credential resolution failed: {error}");
                    output.emit_error(
                        &format!("AddMcpServer '{name}': {reason}"),
                        false,
                        wcore_protocol::events::FailureCategory::LocalWayland,
                    );
                    let _ = writer.emit(&ProtocolEvent::McpFailed {
                        name: name.clone(),
                        reason,
                    });
                    continue;
                }
                if runtime_diagnostics.has_non_runtime_declaration(&name) {
                    output.emit_error(
                        &format!(
                            "AddMcpServer '{name}': name collides with an effective config declaration"
                        ),
                        false,
                    wcore_protocol::events::FailureCategory::LocalWayland);
                    let _ = writer.emit(&ProtocolEvent::McpFailed {
                        name,
                        reason: "name collides with an effective config declaration".to_string(),
                    });
                    continue;
                }

                // wayland#1165 — the EXPLICIT opt-in. Without it the
                // reservation below returns `Existing` for a ready name and the
                // add is the #605 no-op; with it, release the name FIRST so the
                // reservation mints a fresh generation for the new
                // configuration. Refusing here (rather than reserving and then
                // discovering the old transport is still up) keeps a failed
                // replace from leaving two children under one name.
                if replace {
                    match teardown_runtime_mcp_for_replace(
                        &name,
                        &mut runtime_diagnostics,
                        &mcp_lifecycle,
                        &mut engine,
                        &mut dynamic_managers,
                    )
                    .await
                    {
                        Ok(()) => {
                            eprintln!("[mcp] replace: released '{name}' before reconnecting");
                        }
                        Err(reason) => {
                            output.emit_error(
                                &format!("AddMcpServer '{name}' (replace): {reason}"),
                                false,
                                wcore_protocol::events::FailureCategory::LocalWayland,
                            );
                            let _ = writer.emit(&ProtocolEvent::McpFailed { name, reason });
                            continue;
                        }
                    }
                }

                let config_identity = McpConfigIdentity::for_server(&config);
                let reservation = match mcp_lifecycle.reserve(name.clone(), config_identity) {
                    McpReservationOutcome::Acquired(reservation) => reservation,
                    McpReservationOutcome::Existing(snapshot) => {
                        if snapshot.config_identity != config_identity {
                            let reason = "same-name MCP server is already owned by a different configuration; remove it before re-adding".to_string();
                            output.emit_error(
                                &format!("AddMcpServer '{name}': {reason}"),
                                false,
                                wcore_protocol::events::FailureCategory::LocalWayland,
                            );
                            let _ = writer.emit(&ProtocolEvent::McpFailed { name, reason });
                            continue;
                        }
                        match snapshot.state {
                            McpLifecycleState::Ready => {
                                let tools = registered_mcp_tool_names(&engine.tools(), &name);
                                // wayland#605: annotate the skip so a host can
                                // tell this apart from a real reconnect.
                                let _ = writer.emit(&ProtocolEvent::McpReady {
                                    name,
                                    tools,
                                    already_connected: true,
                                });
                            }
                            McpLifecycleState::Connecting => {
                                eprintln!(
                                    "[mcp] '{name}' is already connecting; ignoring duplicate add"
                                );
                            }
                            McpLifecycleState::Stopping => {
                                output.emit_error(
                                    &format!("AddMcpServer '{name}': server is stopping"),
                                    true,
                                    wcore_protocol::events::FailureCategory::LocalWayland,
                                );
                            }
                            McpLifecycleState::CleanupUnverified { .. } => {
                                output.emit_error(
                                    &format!(
                                        "AddMcpServer '{name}': prior transport cleanup is unverified; retry remove first"
                                    ),
                                    false,
                                wcore_protocol::events::FailureCategory::LocalWayland);
                            }
                            McpLifecycleState::Failed { .. } => {
                                unreachable!("failed lifecycle entries are retryable")
                            }
                        }
                        continue;
                    }
                    McpReservationOutcome::CapacityExceeded => {
                        output.emit_error(
                            "AddMcpServer refused: session MCP lifecycle capacity exceeded",
                            false,
                            wcore_protocol::events::FailureCategory::LocalWayland,
                        );
                        continue;
                    }
                };
                let declaration_recorded =
                    runtime_diagnostics.record_runtime_declaration(&name, &config);
                debug_assert!(declaration_recorded);

                let mut single_configs = HashMap::new();
                single_configs.insert(name.clone(), config.clone());
                eprintln!("[mcp] Connecting to '{name}'...");
                let connect_outcome = McpManager::connect_all_with_policy(
                    &single_configs,
                    session_egress_policy.clone(),
                )
                .await;
                match connect_outcome {
                    Ok(mgr) => {
                        if let Some(evidence) = mgr.executable_readiness().get(&name).copied() {
                            runtime_diagnostics.record_executable_readiness(
                                wcore_protocol::diagnostics::McpDeclarationOrigin::RuntimeCommand,
                                &name,
                                evidence,
                            );
                        }
                        let mgr_arc = Arc::new(mgr);
                        let failure_reason = match mgr_arc.health().get(&name) {
                            Some(health) => mcp_server_failure_reason(health),
                            None => {
                                Some("connect outcome missing from MCP health report".to_string())
                            }
                        };
                        if let Some(reason) = failure_reason {
                            reservation.complete_failed(reason.clone());
                            // Retain the typed health outcome for local runtime
                            // diagnostics even though no live tools exist.
                            dynamic_managers.push(mgr_arc);
                            eprintln!("[mcp] connect failed for '{name}': {reason}");
                            output.emit_error(
                                &format!("AddMcpServer '{name}' failed: {reason}"),
                                false,
                                wcore_protocol::events::FailureCategory::ToolRuntime,
                            );
                            let _ = writer.emit(&ProtocolEvent::McpFailed {
                                name: name.clone(),
                                reason,
                            });
                            continue;
                        }
                        let discovered_tool_count = mgr_arc.all_tools().len();
                        eprintln!("[mcp] Connected to '{name}': {discovered_tool_count} tools");
                        let builtin_names = engine.tool_names();
                        let defer_cold = engine.defer_cold_config();
                        // Wave OR: `registry_mut` returns `Option` because
                        // the registry is now Arc-shared. At this CLI boot
                        // site the engine is not running so the refcount
                        // is 1 and `Arc::get_mut` succeeds. Defensive log
                        // (not panic) keeps the dynamic-MCP add-path
                        // resilient if a future change leaks a clone.
                        match engine.registry_mut() {
                            Some(reg) => {
                                register_single_server_tools(
                                    reg,
                                    &mgr_arc,
                                    &name,
                                    &builtin_names,
                                    config.deferred.unwrap_or(true),
                                    config.allowed_tools.as_deref(),
                                    &defer_cold,
                                );
                            }
                            None => {
                                reservation.complete_failed("tool registry is busy");
                                eprintln!(
                                    "[mcp] cannot register tools for '{name}': registry is currently borrowed"
                                );
                                // "registry busy" is a transient lock-contention condition — a
                                // re-issue moments later can succeed, so this one IS retryable.
                                output.emit_error(
                                    &format!("AddMcpServer '{name}': registry busy"),
                                    true,
                                    wcore_protocol::events::FailureCategory::LocalWayland,
                                );
                                continue;
                            }
                        }
                        let tool_names = registered_mcp_tool_names(&engine.tools(), &name);
                        // wayland#1175 — join the live catalogue refresh, with
                        // this server's config, so a `tools/list_changed` from
                        // a runtime-added server is honoured and the operator's
                        // per-tool allowlist (#998) is carried across it.
                        if let Some(refresh) = engine.mcp_catalog_refresh() {
                            refresh.register_runtime_server(&mgr_arc, &single_configs);
                        }
                        dynamic_managers.push(mgr_arc);
                        reservation.complete_ready();
                        let _ = writer.emit(&ProtocolEvent::McpReady {
                            name,
                            tools: tool_names,
                            already_connected: false,
                        });
                    }
                    Err(e) => {
                        eprintln!("[mcp] connect_one failed for '{name}': {e:#}");
                        let reason = format!("{e:#}");
                        reservation.complete_failed(reason.clone());
                        output.emit_error(
                            &format!("AddMcpServer '{name}' failed: {reason}"),
                            false,
                            wcore_protocol::events::FailureCategory::ToolRuntime,
                        );
                        // Companion to the McpReady success emit: tell the host /
                        // TUI *why* this server's tools never appeared so /doctor
                        // can surface it, instead of the failure only hitting stderr.
                        let _ = writer.emit(&ProtocolEvent::McpFailed {
                            name: name.clone(),
                            reason,
                        });
                    }
                }
            }
            ProtocolCommand::RemoveMcpServer(command) => {
                remove_runtime_mcp_server(
                    command,
                    &mut mcp_removal_ledger,
                    &mut runtime_diagnostics,
                    &mcp_lifecycle,
                    &mut engine,
                    &mut dynamic_managers,
                    writer.as_ref(),
                )
                .await;
            }
            ProtocolCommand::Stop => return Ok(()),
            other => {
                // Configured MCP is connected after `ready` so desktop boot is
                // never gated by a slow server. A user message is a stronger
                // boundary: processing it before the connect task settles
                // gives the provider an incomplete tool registry for the
                // entire first turn. Await only the task already in flight;
                // `McpManager` bounds every server handshake independently.
                if matches!(&other, ProtocolCommand::Message { .. })
                    && let Some(rx) = deferred_mcp_rx.take()
                    && let Ok(result) = await_deferred_mcp_connect(rx, &output).await
                {
                    pending_deferred_mcp = note_deferred_mcp_connect(
                        result,
                        Some(&mut runtime_diagnostics),
                        &mut engine,
                        &writer,
                        &output,
                        &mut dynamic_managers,
                        &mut late_mcp,
                    )
                    .await;
                }
                first_cmd = Some(other);
                break;
            }
        }
    }

    let has_mcp = initial_has_mcp || !dynamic_managers.is_empty();
    let mut pending_cmd = first_cmd;

    'session: loop {
        // wayland#551 — settle deferred MCP at every between-turns boundary,
        // BEFORE the next command is processed, so a message that arrives
        // after the connects finished runs WITH the MCP tools. Two sources:
        // a non-blocking poll of the connect task (the blocking wait lives
        // in the select below) and a parked integration whose earlier
        // attempt found the registry borrowed.
        if let Some(rx) = deferred_mcp_rx.as_mut()
            && let Ok(result) = rx.try_recv()
        {
            deferred_mcp_rx = None;
            pending_deferred_mcp = note_deferred_mcp_connect(
                result,
                Some(&mut runtime_diagnostics),
                &mut engine,
                &writer,
                &output,
                &mut dynamic_managers,
                &mut late_mcp,
            )
            .await;
        }
        if let Some(mut pending) = pending_deferred_mcp.take()
            && !integrate_deferred_mcp(
                &mut engine,
                pending.manager.clone(),
                &pending.resolved,
                &mut pending.reservations,
                &writer,
                &mut dynamic_managers,
                &mut late_mcp,
                &mut pending.skill_refs,
            )
        {
            pending_deferred_mcp = Some(pending);
        }

        let cmd = if let Some(c) = pending_cmd.take() {
            c
        } else {
            loop {
                tokio::select! {
                    c = cmd_rx.recv() => match c {
                        Some(c) => break c,
                        None => break 'session,
                    },
                    // wayland#551 — background config-MCP connect settled;
                    // integrate into the live engine and keep waiting for
                    // the next command. Guarded so a consumed receiver is
                    // never polled again.
                    res = async { deferred_mcp_rx.as_mut().expect("guarded by is_some").await },
                        if deferred_mcp_rx.is_some() =>
                    {
                        deferred_mcp_rx = None;
                        // Err = connect task dropped without sending (panic);
                        // nothing to integrate.
                        if let Ok(result) = res {
                            pending_deferred_mcp = note_deferred_mcp_connect(
                                result,
                                Some(&mut runtime_diagnostics),
                                &mut engine,
                                &writer,
                                &output,
                                &mut dynamic_managers,
                                &mut late_mcp,
                            )
                            .await;
                        }
                    }
                }
            }
        };

        if session_command_readiness(&cmd) == SessionCommandReadiness::SettleDeferredMcp {
            // Configured MCP is connected after `ready` so desktop boot is
            // never gated by a slow server. The first actual provider turn
            // is the stronger boundary: await the already-running,
            // per-server-bounded handshake here even when setup commands
            // caused the pre-message loop to exit earlier.
            let ready = settle_deferred_mcp_before_message(
                &mut deferred_mcp_rx,
                &mut pending_deferred_mcp,
                &mut engine,
                &writer,
                &output,
                &mut dynamic_managers,
                Some(&mut runtime_diagnostics),
                &mut late_mcp,
            )
            .await;
            if !ready {
                // A per-turn registry reader has not released its Arc yet.
                // Keep the exact Message parked and retry after yielding;
                // executing it now would give the provider an incomplete
                // tool manifest for the whole turn.
                pending_cmd = Some(cmd);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                continue;
            }
        }

        match cmd {
            ProtocolCommand::Message {
                msg_id,
                content,
                files,
            } => {
                // F-079: thread the active turn id into the protocol sink so
                // any emit_info calls during this turn carry the right msg_id
                // instead of the empty string. Must happen before any slash
                // dispatch or engine.run() call so the first info event is
                // already correlated.
                protocol_sink.set_current_msg_id(&msg_id);

                // v0.8.0 N.* — pre-process slash commands BEFORE the
                // tokio::select loop. When the input is a known slash,
                // emit the rendered output via the protocol sink (as
                // an Info event) and synthesize an empty stream_end so
                // the host's UX doesn't hang.
                if let Some(inv) = wcore_agent::slash::parse(&content) {
                    match slash_dispatcher.try_dispatch(&inv) {
                        Ok(SlashOutcome::Handled { output: Some(text) }) => {
                            output.emit_info(&text);
                            output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Stop);
                            continue;
                        }
                        Ok(SlashOutcome::Handled { output: None }) => {
                            output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Stop);
                            continue;
                        }
                        Ok(SlashOutcome::SetStyle(directive)) => {
                            engine.inject_history(directive);
                            output.emit_info("style updated");
                            output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Stop);
                            continue;
                        }
                        Ok(SlashOutcome::ClearConversation) => {
                            engine.clear_conversation();
                            output.emit_info("conversation cleared");
                            output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Stop);
                            continue;
                        }
                        Ok(SlashOutcome::NotImplemented { message }) => {
                            output.emit_info(&message);
                            output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Stop);
                            continue;
                        }
                        Ok(SlashOutcome::Exit) => {
                            // Host-driven exit via /exit slash command.
                            output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Stop);
                            return Ok(());
                        }
                        Err(SlashError::Unknown(_)) => {
                            // Not a known slash command — fall through
                            // to the normal engine path below.
                        }
                        Err(SlashError::Bad(reason)) => {
                            output.emit_error(
                                &format!("bad slash invocation: {reason}"),
                                false,
                                wcore_protocol::events::FailureCategory::LocalWayland,
                            );
                            output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Error);
                            continue;
                        }
                    }
                }

                let attachment_content = match wcore_cli::attachments::load_composer_images(&files)
                {
                    Ok(content) => content,
                    Err(error) => {
                        protocol_sink.emit_correlated_error(
                            &msg_id,
                            &format!("composer attachment rejected: {error}"),
                            false,
                            wcore_protocol::events::FailureCategory::LocalWayland,
                        );
                        output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Error);
                        continue;
                    }
                };

                let mut stopped = false;
                // #1070: latched false once the host's command stream reaches
                // EOF, so the select never polls a closed receiver again.
                let mut commands_open = true;
                let mut pending_config: Option<PendingConfig> = None;
                let mut mode_changed = false;
                // CORE-2: an errored run may still have consumed provider
                // round-trips (total_usage/run_usage grew before the Err),
                // but `run()` returned no AgentResult to read them from.
                // Defer the error-path terminal stream_end to AFTER this
                // block — `engine_fut` borrows `engine` until then — so it
                // can carry the engine's usage snapshot instead of zeros.
                let mut run_failed = false;

                {
                    let engine_fut = engine.run_with_content(&content, attachment_content, &msg_id);
                    tokio::pin!(engine_fut);

                    loop {
                        tokio::select! {
                            result = &mut engine_fut => {
                                match result {
                                    Ok(result) => {
                                        if result.finish_reason == FinishReason::Error {
                                            // FerroxLabs/wayland#200: a turn can end Ok with finish_reason=Error when
                                            // the provider returned a Done event carrying an unrecognized/empty
                                            // finish_reason (mapped to FinishReason::Error) — e.g. an OpenAI model
                                            // whose finish_reason string the engine doesn't map yet. The engine
                                            // classifies that as success, so without this the host would emit a
                                            // contentless stream_end and the turn would fail SILENTLY. Surface it.
                                            output.emit_error(
                                                "The model ended the turn with an error and no output (finish_reason=error). \
                                                 The provider likely returned an empty response or an unrecognized completion status. \
                                                 Check the engine log for an 'unrecognized finish_reason' warning, and verify the model name and provider.",
                                                false,
                                            wcore_protocol::events::FailureCategory::Unknown);
                                        }
                                        output.emit_stream_end_full(
                                            &msg_id,
                                            result.turns,
                                            result.usage.input_tokens,
                                            result.usage.output_tokens,
                                            result.usage.cache_creation_tokens,
                                            result.usage.cache_read_tokens,
                                            result.finish_reason,
                                            result.active_window_percent,
                                            result.agent_run_id.as_deref(),
                                            Some(&result.usage_delta),
                                        );
                                    }
                                    Err(e) => {
                                        output.emit_error(&format!("{e:#}"), false, wcore_protocol::events::FailureCategory::Unknown);
                                        // stream_end deferred (see run_failed
                                        // above): emitted after this block with
                                        // the engine's usage snapshot.
                                        run_failed = true;
                                    }
                                }
                                break;
                            }
                            maybe_cmd = cmd_rx.recv(), if commands_open => {
                                // #1070: `Some(sub_cmd) = ...` silently
                                // disabled this branch on EOF, leaving a
                                // parked approval to wait out its full TTL.
                                let Some(sub_cmd) = maybe_cmd else {
                                    commands_open = false;
                                    // #1083: drains the ApprovalBridge too — a
                                    // Crucible card parked here otherwise waits
                                    // out its 24h TTL.
                                    deny_pending_approvals_on_host_eof(
                                        &approval_manager,
                                        &approval_bridge,
                                    )
                                    .await;
                                    continue;
                                };
                                match sub_cmd {
                                    ProtocolCommand::ToolApprove { call_id, scope, answer } => {
                                        // v0.9.4 W1.3 (F7): was resolve() ignoring scope. Use
                                        // approve() so Always/AlwaysPrefix registers the rule.
                                        approval_manager.approve(&call_id, scope, answer);
                                    }
                                    ProtocolCommand::ToolDeny { call_id, reason } => {
                                        approval_manager.resolve(&call_id, ToolApprovalResult::Denied { reason });
                                    }
                                    ProtocolCommand::Stop => {
                                        // wayland#403 fix-3: Stop CANCELS THE ACTIVE TURN — it must
                                        // NOT end the session. Fire the engine-owned active-turn
                                        // token before dropping `engine_fut`; we emit `stream_end`
                                        // (FinishReason::Stop) for this msg_id so the host's turn-loop
                                        // gets its terminator and doesn't hang. `stopped` then makes
                                        // the outer loop `continue` (keep reading commands) instead of
                                        // breaking — the pre-fix `break` stranded the session
                                        // ("new chat required") after any mid-turn Stop. Only EOF and
                                        // `/exit` end a json-stream session, matching the TUI (Esc
                                        // cancels the turn, never closes the session).
                                        session_control.cancel_active_turn();
                                        output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Stop);
                                        stopped = true;
                                        break;
                                    }
                                    ProtocolCommand::SetConfig { model, thinking, thinking_budget, effort, compaction } => {
                                        pending_config = Some((model, thinking, thinking_budget, effort, compaction));
                                        let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                                            msg_id: String::new(),
                                            message: "set_config: queued, will apply after current response".to_string(),
                                        });
                                    }
                                    ProtocolCommand::ContinueWithBudget(command) => {
                                        let refusal = if allow_host_budget_grants {
                                            BudgetGrantRefusalReason::TurnInProgress
                                        } else {
                                            BudgetGrantRefusalReason::HostNotAuthorized
                                        };
                                        let emission = budget_grants.complete(command, |_| Err(refusal));
                                        let _ = writer.emit(&ProtocolEvent::BudgetGrantResult {
                                            result: emission.into_result(),
                                        });
                                    }
                                    ProtocolCommand::SetMode { mode } => {
                                        // GHSA-8r7g: a wire peer may not escalate to
                                        // an auto-approving mode (Force or AutoEdit)
                                        // without a local-operator opt-in.
                                        let mode_str = format!("{mode:?}").to_lowercase();
                                        match apply_wire_mode_change(
                                            &approval_manager,
                                            &mut execution_policy_sequence,
                                            mode,
                                            audit_unix_time_millis()?,
                                        )? {
                                            WireModeChange::Changed(snapshot) => {
                                                mode_changed = true;
                                                let _ = writer.emit(&ProtocolEvent::ExecutionPolicy {
                                                    snapshot,
                                                });
                                                let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                                                    msg_id: String::new(),
                                                    message: format!("mode updated: {}", approval_manager.current_mode()),
                                                });
                                            }
                                            WireModeChange::Unchanged => {
                                                let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                                                    msg_id: String::new(),
                                                    message: format!("mode unchanged: {}", approval_manager.current_mode()),
                                                });
                                            }
                                            WireModeChange::Rejected { effective } => {
                                                // The typed nack FIRST: a host branches on this.
                                                // The `info` below stays for hosts that only
                                                // render prose (wayland#1088).
                                                let _ = writer.emit(&ProtocolEvent::SetModeRefused {
                                                    requested: mode,
                                                    effective,
                                                    reason: wcore_protocol::events::SetModeRefusalReason::LocalOptInRequired,
                                                });
                                                let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                                                    msg_id: String::new(),
                                                    message: format!("set_mode: '{mode_str}' refused — an auto-approving mode (auto_edit/force) requires a local-operator opt-in (launch with --force or WAYLAND_ALLOW_WIRE_FORCE=1)"),
                                                });
                                            }
                                        }
                                    }
                                    ProtocolCommand::GrantWorkspaceCapability { executable } => {
                                        emit_workspace_capability_grant(
                                            allow_host_workspace_grants,
                                            &workspace_policy,
                                            &mut workspace_policy_receipt,
                                            &executable,
                                            writer.as_ref(),
                                        );
                                    }
                                    ProtocolCommand::GrantPath {
                                        grant_id,
                                        root,
                                        access,
                                        expires_at_ms,
                                    } => {
                                        emit_path_grant(
                                            allow_host_path_grants,
                                            &workspace_policy,
                                            &mut workspace_policy_receipt,
                                            PathGrantRequest {
                                                grant_id,
                                                root,
                                                access,
                                                expires_at_ms,
                                            },
                                            writer.as_ref(),
                                        );
                                    }
                                    ProtocolCommand::RevokePath { grant_id } => {
                                        emit_path_revoke(
                                            &workspace_policy,
                                            &mut workspace_policy_receipt,
                                            &grant_id,
                                            writer.as_ref(),
                                        );
                                    }
                                    ProtocolCommand::SessionResync(command) => {
                                        let reason = if command.recovery_version
                                            != RECOVERY_PROTOCOL_VERSION
                                        {
                                            RecoveryUnavailableReason::UnsupportedVersion
                                        } else {
                                            RecoveryUnavailableReason::SnapshotUnavailable
                                        };
                                        emit_recovery_unavailable(
                                            writer.as_ref(),
                                            command.request_id,
                                            command.session_id,
                                            reason,
                                        );
                                    }
                                    ProtocolCommand::ResumeTurn(command) => {
                                        let reason = if command.recovery_version
                                            != RECOVERY_PROTOCOL_VERSION
                                        {
                                            RecoveryUnavailableReason::UnsupportedVersion
                                        } else {
                                            RecoveryUnavailableReason::UnknownCriticalState
                                        };
                                        emit_recovery_unavailable(
                                            writer.as_ref(),
                                            command.request_id,
                                            command.session_id,
                                            reason,
                                        );
                                    }
                                    ProtocolCommand::ResolveInterruptedApproval(command) => {
                                        let reason = if command.recovery_version
                                            != wcore_protocol::commands::RECOVERED_APPROVAL_VERSION
                                        {
                                            RecoveryUnavailableReason::UnsupportedVersion
                                        } else {
                                            RecoveryUnavailableReason::UnknownCriticalState
                                        };
                                        emit_recovery_unavailable(
                                            writer.as_ref(),
                                            command.request_id,
                                            command.session_id,
                                            reason,
                                        );
                                    }
                                    ProtocolCommand::ResolveUnknownToolEffect(_) => {
                                        // The live engine is mutably borrowed by `engine_fut` and
                                        // its durable cursor may still advance. Never queue or
                                        // apply an operator claim against that moving authority;
                                        // the host must resync and reissue it between turns.
                                        output.emit_error(
                                            "resolve_unknown_tool_effect refused during active turn; resync and retry after the turn stops",
                                            false,
                                        wcore_protocol::events::FailureCategory::LocalWayland);
                                    }
                                    ProtocolCommand::RemoveMcpServer(command) => {
                                        if mcp_removal_request_id_invalid(&command) {
                                            let _ = writer.emit(&mcp_removal_receipt(
                                                &command,
                                                McpRemovalOutcome::InvalidRequest,
                                                Vec::new(),
                                            ));
                                        } else if let Some(receipt) = mcp_removal_ledger
                                            .replay_or_conflict(&command)
                                        {
                                            let _ = writer.emit(&receipt);
                                        } else if mcp_removal_ledger
                                            .is_full_for_new(&command.request_id)
                                        {
                                            let _ = writer.emit(&mcp_removal_receipt(
                                                &command,
                                                McpRemovalOutcome::CapacityExceeded,
                                                Vec::new(),
                                            ));
                                        } else if let Some(outcome) =
                                            mcp_removal_request_rejection(&command)
                                        {
                                            emit_mcp_removal_receipt(
                                                &command,
                                                outcome,
                                                Vec::new(),
                                                &mut mcp_removal_ledger,
                                                writer.as_ref(),
                                            );
                                        } else {
                                            emit_mcp_removal_receipt(
                                                &command,
                                                McpRemovalOutcome::TurnInProgress,
                                                Vec::new(),
                                                &mut mcp_removal_ledger,
                                                writer.as_ref(),
                                            );
                                        }
                                    }
                                    ProtocolCommand::Ping => {
                                        let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Pong);
                                    }
                                    // wayland#896 — see the note at the
                                    // recovery-loop arm. MID-turn matters most
                                    // of all: a long turn is exactly when a
                                    // host wants a recovery point, and it is
                                    // also exactly when Core is most likely to
                                    // be writing profile state.
                                    ref quiesce_command @ (ProtocolCommand::QuiesceAcquire(_)
                                    | ProtocolCommand::QuiesceRelease(_)
                                    | ProtocolCommand::QuiesceStatus(_)) => {
                                        for event in
                                            wcore_cli::quiesce_control::handle_quiesce_control(
                                                quiesce_command,
                                            )
                                        {
                                            let _ = writer.emit(&event);
                                        }
                                    }
                                    ProtocolCommand::ApprovalResume {
                                        resume_token,
                                        approved,
                                        modifications,
                                    } => {
                                        // GHSA-8r7g: a bridge-backed approval (the
                                        // Crucible council card, an egress consent) parks
                                        // DURING an active turn, so its ApprovalResume MUST
                                        // be handled here — the top-level arm only runs
                                        // BETWEEN turns and would never see it, leaving the
                                        // council/consent unresolvable (hang until the TTL)
                                        // on a JSON-stream host (the desktop app). Route the
                                        // secret resume_token through the shared bridge.
                                        wcore_cli::approval_resume::handle_approval_resume(
                                            &approval_bridge,
                                            writer.as_ref(),
                                            resume_token,
                                            approved,
                                            modifications,
                                        )
                                        .await;
                                    }
                                    ProtocolCommand::HostSendMessageResult { call_id, ok, message_id, error } => {
                                        // #537/#141: a host-delegated send_message parks
                                        // DURING the active turn (the tool call awaits the
                                        // host's result), so this arm is the one that
                                        // actually unblocks it — mirroring the
                                        // ApprovalResume mid-turn handling above
                                        // (GHSA-8r7g pattern). An unknown call_id resolves
                                        // nothing (stale reply after timeout, or a peer
                                        // guessing ids) and is surfaced as Info.
                                        let resolved = host_send_bridge.resolve(
                                            &call_id,
                                            wcore_agent::host_send_transport::HostSendResult {
                                                ok,
                                                message_id,
                                                error,
                                            },
                                        );
                                        if !resolved {
                                            let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                                                msg_id: String::new(),
                                                message: format!(
                                                    "host_send_message_result received for unknown call_id: {call_id} (stale or timed-out send?)"
                                                ),
                                            });
                                        }
                                    }
                                    _ => {
                                        eprintln!("[protocol] Ignoring command during active message processing");
                                    }
                                }
                            }
                        }
                    }
                }

                if run_failed {
                    // CORE-2: the failed run's terminal stream_end. When the
                    // run consumed provider round-trips before dying, report
                    // the cumulative usage + this run's delta from the
                    // engine's snapshot (the counters already grew and will
                    // be persisted); otherwise keep the legacy zero-usage
                    // emission byte-identical.
                    let (total, delta) = engine.usage_snapshot();
                    let delta_nonzero = delta.input_tokens > 0
                        || delta.output_tokens > 0
                        || delta.cache_creation_tokens > 0
                        || delta.cache_read_tokens > 0;
                    if delta_nonzero {
                        output.emit_stream_end_full(
                            &msg_id,
                            0,
                            total.input_tokens,
                            total.output_tokens,
                            total.cache_creation_tokens,
                            total.cache_read_tokens,
                            FinishReason::Error,
                            None,
                            None,
                            Some(&delta),
                        );
                    } else {
                        output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Error);
                    }
                }

                if let Some((model, thinking, thinking_budget, effort, compaction)) =
                    pending_config.take()
                {
                    let changes = engine.apply_config_update(
                        model,
                        thinking,
                        thinking_budget,
                        effort,
                        compaction,
                    );
                    if !changes.is_empty() {
                        let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                            msg_id: String::new(),
                            message: format!("config applied: {}", changes.join(", ")),
                        });
                    }
                    protocol_sink.emit_config_changed_with_plugins(
                        engine.compat(),
                        has_mcp,
                        &approval_manager.current_mode(),
                        initial_has_plugins,
                        &initial_plugin_caps,
                        engine.advertised_capabilities(),
                    );
                } else if mode_changed {
                    protocol_sink.emit_config_changed_with_plugins(
                        engine.compat(),
                        has_mcp,
                        &approval_manager.current_mode(),
                        initial_has_plugins,
                        &initial_plugin_caps,
                        engine.advertised_capabilities(),
                    );
                }
                if stopped {
                    // wayland#403 fix-3: the mid-turn Stop cancelled the turn and
                    // already emitted its `stream_end`; resume the command loop so
                    // the next Message streams. Pre-fix this was `break`, which
                    // ended the whole session.
                    continue;
                }
            }
            // F22-C1 — host CONTROL of a durable Goal. One arm for all five
            // commands; the decision logic lives in
            // `wcore_agent::goal::control` so this fenced file gains a single
            // contiguous block rather than five handlers.
            //
            // `handle_goal_control` NEVER returns an empty vec for a Goal
            // command: an accepted one answers with `goal_snapshot`, a refused
            // one with `goal_control_refused`. That is what keeps this arm from
            // being a surface that accepts and silently does nothing — the
            // failure mode the catch-all arm below would otherwise produce.
            goal_command @ (ProtocolCommand::GoalOpen(_)
            | ProtocolCommand::GoalDeclareTask(_)
            | ProtocolCommand::GoalAdvance(_)
            | ProtocolCommand::GoalCancel(_)
            | ProtocolCommand::GoalResync(_)) => {
                let live_session_id = engine.current_session_id();
                let goal_events = wcore_agent::goal::handle_goal_control(
                    engine.session_journal(),
                    live_session_id.as_deref(),
                    &wcore_agent::goal::GoalParentEnvelope::local_session_default(),
                    audit_unix_time_millis()?,
                    &goal_command,
                )
                .unwrap_or_default();
                for event in &goal_events {
                    let _ = writer.emit(event);
                }
            }
            ProtocolCommand::SessionResync(command) => {
                handle_session_resync(
                    &engine,
                    writer.as_ref(),
                    command.recovery_version,
                    command.request_id,
                    command.session_id,
                    command.after,
                );
            }
            ProtocolCommand::ResumeTurn(command) => {
                protocol_sink.set_current_msg_id(&command.request_id);
                handle_resume_turn(
                    &mut engine,
                    writer.as_ref(),
                    output.as_ref(),
                    &mut cmd_rx,
                    approval_manager.as_ref(),
                    &|| session_control.cancel_active_turn(),
                    command.recovery_version,
                    command.request_id,
                    command.session_id,
                    command.turn_id,
                    command.cursor,
                    command.action,
                )
                .await;
            }
            ProtocolCommand::ResolveInterruptedApproval(command) => {
                protocol_sink.set_current_msg_id(&command.request_id);
                handle_recovered_approval(
                    &mut engine,
                    writer.as_ref(),
                    output.as_ref(),
                    &mut cmd_rx,
                    approval_manager.as_ref(),
                    &|| session_control.cancel_active_turn(),
                    command.recovery_version,
                    command.request_id,
                    command.session_id,
                    command.turn_id,
                    command.cursor,
                    command.approval_id,
                    command.decision,
                    command.answer,
                )
                .await;
            }
            command @ ProtocolCommand::ResolveUnknownToolEffect(_) => {
                handle_operator_tool_effect_resolution(
                    &engine,
                    writer.as_ref(),
                    output.as_ref(),
                    command,
                );
            }
            ProtocolCommand::Stop => {
                // wayland#403 fix-3: a Stop that arrives with no active turn (or
                // just as one finishes) is a no-op — it must not close the
                // session. Keep reading commands; EOF / `/exit` are the
                // terminators.
                continue;
            }
            ProtocolCommand::ToolApprove {
                call_id,
                scope,
                answer,
            } => {
                // v0.9.4 W1.3 (F7): was a stub that ignored scope and called
                // resolve(). Use approve() so Always/AlwaysPrefix persists.
                approval_manager.approve(&call_id, scope, answer);
            }
            ProtocolCommand::ToolDeny { call_id, reason } => {
                approval_manager.resolve(&call_id, ToolApprovalResult::Denied { reason });
            }
            ProtocolCommand::InitHistory { text } => {
                // F-003: route init_history text into the engine's system
                // prompt so Constitution + skills index + persona sent by
                // the app actually reach the model. Previously this was a
                // silent eprintln!-drop — the root cause of "no deliverables"
                // in the customer flow.
                tracing::info!(
                    target: "wcore_cli::protocol",
                    chars = text.len(),
                    "init_history injected into engine system prompt"
                );
                engine.inject_history(text);
            }
            ProtocolCommand::SetMode { mode } => {
                let mode_str = format!("{mode:?}").to_lowercase();
                // GHSA-8r7g: a wire peer may not escalate to an auto-approving
                // mode (Force or AutoEdit) without a local-operator opt-in.
                match apply_wire_mode_change(
                    &approval_manager,
                    &mut execution_policy_sequence,
                    mode,
                    audit_unix_time_millis()?,
                )? {
                    WireModeChange::Rejected { effective } => {
                        // The typed nack FIRST: a host branches on this. The
                        // `info` below stays for hosts that only render prose
                        // (wayland#1088).
                        let _ = writer.emit(&ProtocolEvent::SetModeRefused {
                            requested: mode,
                            effective,
                            reason:
                                wcore_protocol::events::SetModeRefusalReason::LocalOptInRequired,
                        });
                        let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                            msg_id: String::new(),
                            message: format!("set_mode: '{mode_str}' refused — an auto-approving mode (auto_edit/force) requires a local-operator opt-in (launch with --force or WAYLAND_ALLOW_WIRE_FORCE=1)"),
                        });
                        eprintln!("[protocol] SetMode refused ({mode_str}, no local opt-in)");
                    }
                    WireModeChange::Unchanged => {
                        let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                            msg_id: String::new(),
                            message: format!("mode unchanged: {}", approval_manager.current_mode()),
                        });
                    }
                    WireModeChange::Changed(snapshot) => {
                        let _ = writer.emit(&ProtocolEvent::ExecutionPolicy { snapshot });
                        let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                            msg_id: String::new(),
                            message: format!("mode updated: {}", approval_manager.current_mode()),
                        });
                        protocol_sink.emit_config_changed_with_plugins(
                            engine.compat(),
                            has_mcp,
                            &approval_manager.current_mode(),
                            initial_has_plugins,
                            &initial_plugin_caps,
                            engine.advertised_capabilities(),
                        );
                        eprintln!("[protocol] SetMode applied: {mode_str}");
                    }
                }
            }
            ProtocolCommand::SetConfig {
                model,
                thinking,
                thinking_budget,
                effort,
                compaction,
            } => {
                let changes = engine.apply_config_update(
                    model,
                    thinking,
                    thinking_budget,
                    effort,
                    compaction,
                );
                // F-061: only emit config_changed when something actually
                // changed. When changes is empty the host already has the
                // current state; an extra emission would send the full
                // 13-key capabilities blob unnecessarily.
                if changes.is_empty() {
                    let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                        msg_id: String::new(),
                        message: "set_config: no changes".to_string(),
                    });
                } else {
                    let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                        msg_id: String::new(),
                        message: format!("config updated: {}", changes.join(", ")),
                    });
                    protocol_sink.emit_config_changed_with_plugins(
                        engine.compat(),
                        has_mcp,
                        &approval_manager.current_mode(),
                        initial_has_plugins,
                        &initial_plugin_caps,
                        engine.advertised_capabilities(),
                    );
                }
            }
            ProtocolCommand::ContinueWithBudget(command) => {
                let emission = if allow_host_budget_grants {
                    budget_grants.complete(command, |command| {
                        engine.continue_with_additional_budget(
                            &command.request_id,
                            command.additional_tokens,
                            command.additional_cost_usd,
                        )
                    })
                } else {
                    budget_grants.complete(command, |_| {
                        Err(BudgetGrantRefusalReason::HostNotAuthorized)
                    })
                };
                let _ = writer.emit(&ProtocolEvent::BudgetGrantResult {
                    result: emission.into_result(),
                });
            }
            ProtocolCommand::GetRuntimeDiagnostics(command) => {
                emit_runtime_diagnostics(
                    &command,
                    &runtime_diagnostics,
                    &mcp_lifecycle,
                    &result.mcp_managers,
                    &dynamic_managers,
                    &engine.tools(),
                    writer.as_ref(),
                );
            }
            ProtocolCommand::AddMcpServer { name, .. } => {
                output.emit_error(
                    &format!("AddMcpServer '{name}': rejected — only allowed before first Message"),
                    false,
                    wcore_protocol::events::FailureCategory::LocalWayland,
                );
            }
            ProtocolCommand::RemoveMcpServer(command) => {
                remove_runtime_mcp_server(
                    command,
                    &mut mcp_removal_ledger,
                    &mut runtime_diagnostics,
                    &mcp_lifecycle,
                    &mut engine,
                    &mut dynamic_managers,
                    writer.as_ref(),
                )
                .await;
            }
            ProtocolCommand::GrantWorkspaceCapability { executable } => {
                emit_workspace_capability_grant(
                    allow_host_workspace_grants,
                    &workspace_policy,
                    &mut workspace_policy_receipt,
                    &executable,
                    writer.as_ref(),
                );
            }
            ProtocolCommand::GrantPath {
                grant_id,
                root,
                access,
                expires_at_ms,
            } => {
                emit_path_grant(
                    allow_host_path_grants,
                    &workspace_policy,
                    &mut workspace_policy_receipt,
                    PathGrantRequest {
                        grant_id,
                        root,
                        access,
                        expires_at_ms,
                    },
                    writer.as_ref(),
                );
            }
            ProtocolCommand::RevokePath { grant_id } => {
                emit_path_revoke(
                    &workspace_policy,
                    &mut workspace_policy_receipt,
                    &grant_id,
                    writer.as_ref(),
                );
            }
            ProtocolCommand::Ping => {
                let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Pong);
            }
            // wayland#896 — see the note at the recovery-loop arm.
            ref quiesce_command @ (ProtocolCommand::QuiesceAcquire(_)
            | ProtocolCommand::QuiesceRelease(_)
            | ProtocolCommand::QuiesceStatus(_)) => {
                for event in wcore_cli::quiesce_control::handle_quiesce_control(quiesce_command) {
                    let _ = writer.emit(&event);
                }
            }
            ProtocolCommand::ApprovalResume {
                resume_token,
                approved,
                modifications,
            } => {
                // W7.1 S4-3.2: route the host's resume decision through the
                // shared `ApprovalBridge` so the awaiting `ScriptTool` step
                // unblocks and continues (or aborts) under the same outcome.
                // We still emit the `ApprovalResume` event so host UI can
                // clear its pending-approval state; the diagnostic `Info` is
                // emitted only when the token is unknown (stale resume).
                wcore_cli::approval_resume::handle_approval_resume(
                    &approval_bridge,
                    writer.as_ref(),
                    resume_token,
                    approved,
                    modifications,
                )
                .await;
            }
            ProtocolCommand::HostSendMessageResult {
                call_id,
                ok,
                message_id,
                error,
            } => {
                // #537/#141: a delegated send awaits its result DURING the
                // turn (handled by the mid-turn arm above); a result arriving
                // here, between turns, is almost always stale — the send
                // already timed out into a tool error. Still attempt the
                // resolve (harmless if the id is gone) and surface unknown
                // ids as Info, mirroring the ApprovalResume arm.
                let resolved = host_send_bridge.resolve(
                    &call_id,
                    wcore_agent::host_send_transport::HostSendResult {
                        ok,
                        message_id,
                        error,
                    },
                );
                if !resolved {
                    let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                        msg_id: String::new(),
                        message: format!(
                            "host_send_message_result received for unknown call_id: {call_id} (stale or timed-out send?)"
                        ),
                    });
                }
            }
        }
    }

    engine.run_stop_hooks().await;
    for mgr in &result.mcp_managers {
        mgr.shutdown().await;
    }
    for mgr in &dynamic_managers {
        mgr.shutdown().await;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// `wayland-core --request-permissions` — the single explicit path that
/// is allowed to raise a macOS TCC consent dialog (issue #114).
///
/// Everything else in the binary uses `wcore_cua::permissions::probe`,
/// which never shows UI. Accessibility consent is not granted
/// in-process: macOS opens the Settings pane and the grant takes effect
/// only after the user adds the binary and restarts it, so a still-
/// denied result here is the expected first-run answer and NOT an
/// error — the exit code stays 0 either way so a first run is not
/// reported as a failure.
fn request_permissions() -> ExitCode {
    use wcore_cua::permissions::{TccCapability, TccStatus, prime};

    if !cfg!(target_os = "macos") {
        println!("No permissions to request on this platform — TCC is macOS-only.");
        return ExitCode::SUCCESS;
    }

    println!("Requesting macOS computer-use permissions. Approve the prompts that appear.\n");
    for capability in TccCapability::ALL {
        match prime(capability) {
            TccStatus::Granted => println!("[GRANTED] {}", capability.settings_pane()),
            TccStatus::Denied => {
                println!("[PENDING] {}", capability.settings_pane());
                println!("          {}", capability.remediation());
            }
            TccStatus::NotApplicable => {
                println!(
                    "[SKIP]    {} — not applicable here",
                    capability.settings_pane()
                );
            }
        }
    }
    println!("\nRe-run `wayland-core --doctor` to confirm the grants took effect.");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wayland#562 — an inert `LateMcpBinder` for tests that only exercise the
    /// tool-registration half of deferred-MCP integration: empty catalog, no
    /// plugin hooks, no boot managers. `bind` still runs against it, so these
    /// tests also prove late-binding cannot panic or disturb tool registration
    /// when there is nothing to bind.
    fn inert_late_binder() -> LateMcpBinder {
        LateMcpBinder::new(
            Arc::new(wcore_skills::refs::SkillCatalog::from_refs(Vec::new())),
            &[],
            Vec::new(),
            true,
        )
    }
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::time::Duration;
    use wcore_types::execution_policy::{BaselineExecutionPolicy, PolicySource};

    /// #693 — `--project-dir` moves the workspace without moving the CWD, so
    /// a durable grant keyed off the CWD alone is shared by two sessions
    /// pointed at two different projects.
    ///
    /// The scenario is the one the flag makes trivial: ONE shell, two
    /// `wayland-core --project-dir ...` invocations, two projects. The
    /// same-workspace arm is the positive control — without it this would
    /// pass just as well against a `restore_always_allows` that restored
    /// nothing at all.
    #[test]
    fn a_grant_under_one_project_dir_does_not_restore_under_another() {
        use wcore_cli::tui::restore_always_allows;
        use wcore_permissions::learning::{LearnedDecision, LearnedPolicy};

        let tmp = tempfile::tempdir().expect("tempdir");
        let repo_a = tmp.path().join("repo-a");
        let repo_b = tmp.path().join("repo-b");
        std::fs::create_dir_all(&repo_a).expect("mkdir a");
        std::fs::create_dir_all(&repo_b).expect("mkdir b");
        let path = tmp.path().join("permissions.toml");

        // The CWD is whatever the test binary runs in — identical for both,
        // which is exactly the point.
        let ws_a =
            LearnedPolicy::workspace_key(&workspace_root(Some(&repo_a)).expect("workspace root a"));
        let ws_b =
            LearnedPolicy::workspace_key(&workspace_root(Some(&repo_b)).expect("workspace root b"));
        assert_ne!(
            ws_a, ws_b,
            "two --project-dir values from one shell must not collapse to one \
             permissions key"
        );

        let mut policy = LearnedPolicy::new();
        policy.record_in("Write", None, LearnedDecision::AllowAlways, &ws_a);
        policy.save_to(&path).expect("save");

        let session_a = ToolApprovalManager::new();
        restore_always_allows(&session_a, &path, &ws_a);
        assert!(
            session_a.is_tool_name_auto_approved("Write"),
            "control: the grant must still restore in the project it was made in"
        );

        let session_b = ToolApprovalManager::new();
        restore_always_allows(&session_b, &path, &ws_b);
        assert!(
            !session_b.is_tool_name_auto_approved("Write"),
            "a grant made against {} auto-approved Write against {} — \
             --project-dir is the workspace, and the permissions key must \
             follow it just as the trust store already does",
            repo_a.display(),
            repo_b.display()
        );
    }

    /// The no-flag arm: `workspace_root(None)` is the CWD, which is what the
    /// trust store resolved before this existed.
    #[test]
    fn workspace_root_without_the_flag_is_the_cwd() {
        assert_eq!(
            workspace_root(None).expect("workspace root"),
            std::env::current_dir().expect("cwd")
        );
    }

    /// UAT-W1 — the product's own error advised the wrong flag.
    ///
    /// The non-TTY message told the user to `pass a prompt with -p`. `-p` is
    /// the short form of `--provider`, so obeying it yields
    /// `Unknown provider: 'Reply'`. The prompt is a trailing positional.
    ///
    /// This fails if the advice names ANY flag that does not do what the
    /// advice says. It resolves each flag token against the REAL clap
    /// definition (`Cli::command()`), never against a copy of the message, so
    /// it cannot drift and cannot pass tautologically.
    #[test]
    fn non_tty_advice_names_only_flags_that_do_what_it_says() {
        use clap::CommandFactory;

        /// Resolve a `--long` / `-s` token to the clap argument id it
        /// actually binds to, or `None` if no such argument exists.
        fn resolve(cmd: &clap::Command, token: &str) -> Option<String> {
            let name = token.trim_start_matches('-');
            cmd.get_arguments()
                .find(|a| {
                    if token.starts_with("--") {
                        a.get_long() == Some(name)
                            || a.get_all_aliases().is_some_and(|v| v.contains(&name))
                    } else {
                        name.chars().count() == 1 && a.get_short() == name.chars().next()
                    }
                })
                .map(|a| a.get_id().to_string())
        }

        let cmd = Cli::command();

        // ── Controls, BOTH directions, so the resolver is proven alive ──
        // Known-positive: a flag that exists resolves to its id.
        assert_eq!(
            resolve(&cmd, "--json-stream").as_deref(),
            Some("json_stream"),
            "resolver is dead: it cannot even find --json-stream"
        );
        // The exact token that carried the bug. This documents the trap: if
        // `-p` is ever put back in the advice, the loop below reports it as
        // `provider`, not as a way to pass a prompt.
        assert_eq!(
            resolve(&cmd, "-p").as_deref(),
            Some("provider"),
            "`-p` is expected to be --provider; the advice must not offer it \
             as a way to pass a prompt"
        );
        // Known-negative: a flag that does not exist resolves to nothing.
        assert_eq!(
            resolve(&cmd, "--definitely-not-a-real-flag"),
            None,
            "resolver is not discriminating: it matched a nonexistent flag"
        );
        // The prompt really is a positional, so no flag can ever be the right
        // answer. If this changes, the advice should change with it.
        assert!(
            cmd.get_arguments().any(|a| a.get_id() == "prompt"
                && a.get_long().is_none()
                && a.get_short().is_none()),
            "`prompt` is no longer a bare positional — revisit the advice"
        );

        // ── The assertion itself ──
        let tokens: Vec<&str> = [NON_TTY_NO_PROMPT_ADVICE, RESUME_NO_PROMPT_ADVICE]
            .concat()
            .leak()
            .split(|c: char| c.is_whitespace() || c == '"' || c == ',')
            .map(|t| t.trim_end_matches('.'))
            .filter(|t| t.starts_with('-') && t.len() > 1)
            .collect();

        // Vacuity guard: a message naming no flags at all would pass the loop
        // below without checking anything.
        assert!(
            !tokens.is_empty(),
            "no flag tokens extracted from the advice — the extractor is dead, \
             or the advice stopped naming any flag"
        );

        // Every token the advice may name, pinned to the ONE clap argument it
        // must bind to. A map, not a set: a token that exists but binds to
        // something else still fails, so the `-p` trap above stays live --
        // `-p` resolves to `provider`, is absent from this map, and is
        // reported as not doing what the advice would be claiming.
        let expected: &[(&str, &str)] = &[
            // "Use --json-stream for headless/piped use".
            ("--json-stream", "json_stream"),
            // UAT-UXA2: `wayland-core --resume <id> "your next message"`.
            // `--resume` is clap `resume: Option<String>` -- "Resume a
            // previous session" -- and this second paragraph only prints when
            // the user ALREADY passed `--resume`/`--continue`, so the sentence
            // names the flag that does exactly what the sentence says.
            ("--resume", "resume"),
        ];

        // The allow-map may not carry an entry the advice no longer names, or
        // it silently pre-authorises a flag that nothing actually checks.
        for (token, _) in expected {
            assert!(
                tokens.contains(token),
                "`{token}` is allow-listed but the advice no longer names it"
            );
        }

        for token in tokens {
            let id = resolve(&cmd, token).unwrap_or_else(|| {
                panic!("the advice names `{token}`, which is not an argument at all")
            });
            let want = expected
                .iter()
                .find(|(t, _)| *t == token)
                .map(|(_, want)| *want)
                .unwrap_or_else(|| {
                    panic!(
                        "the advice names `{token}` (clap argument `{id}`), which this \
                         message is not allowed to offer. The prompt is a trailing \
                         positional: wayland-core \"your prompt\"."
                    )
                });
            assert_eq!(
                id, want,
                "the advice names `{token}`, which is clap argument `{id}`, not \
                 `{want}` -- it does not do what the advice says"
            );
        }
    }

    #[test]
    fn host_runtime_mcp_requires_an_immutable_assistant_scope() {
        let config = to_mcp_server_config(
            "stdio",
            Some("example-mcp".into()),
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .expect("valid MCP config");

        assert!(scope_host_runtime_mcp(config.clone(), None).is_err());
        assert!(scope_host_runtime_mcp(config.clone(), Some(" ")).is_err());

        let scoped = scope_host_runtime_mcp(config, Some("research"))
            .expect("identified host session must be scoped");
        assert!(scoped.is_visible_to_assistant(Some("research")));
        assert!(!scoped.is_visible_to_assistant(Some("operations")));
        assert!(!scoped.is_visible_to_assistant(None));
    }

    #[test]
    fn cleanup_failure_cannot_produce_removed_receipt() {
        assert_eq!(mcp_removal_cleanup_outcome(&[]), McpRemovalOutcome::Removed);
        assert_eq!(
            mcp_removal_cleanup_outcome(&["injected close failure".into()]),
            McpRemovalOutcome::CleanupUnverified
        );
    }

    #[test]
    fn removal_admission_is_versioned_bounded_and_replay_safe_during_turns() {
        let unsupported = RemoveMcpServerCommand {
            lifecycle_version: MCP_LIFECYCLE_VERSION + 1,
            request_id: String::new(),
            name: String::new(),
        };
        assert_eq!(
            mcp_removal_request_rejection(&unsupported),
            Some(McpRemovalOutcome::UnsupportedVersion)
        );

        let blank = RemoveMcpServerCommand {
            lifecycle_version: MCP_LIFECYCLE_VERSION,
            request_id: " ".into(),
            name: "server".into(),
        };
        assert_eq!(
            mcp_removal_request_rejection(&blank),
            Some(McpRemovalOutcome::InvalidRequest)
        );

        let command = RemoveMcpServerCommand {
            lifecycle_version: MCP_LIFECYCLE_VERSION,
            request_id: "active-turn-1".into(),
            name: "server".into(),
        };
        let receipt = mcp_removal_receipt(&command, McpRemovalOutcome::TurnInProgress, Vec::new());
        let mut ledger = McpRemovalLedger::default();
        ledger.record(&command, &receipt);
        assert_eq!(
            serde_json::to_value(
                ledger
                    .replay_or_conflict(&command)
                    .expect("same request must replay its terminal receipt")
            )
            .unwrap(),
            serde_json::to_value(receipt).unwrap()
        );

        let conflict = RemoveMcpServerCommand {
            name: "different-server".into(),
            ..command
        };
        let conflict = ledger
            .replay_or_conflict(&conflict)
            .expect("same request id with a different name must terminate as conflict");
        assert_eq!(
            serde_json::to_value(conflict).unwrap()["outcome"],
            "request_id_conflict"
        );
    }

    #[test]
    fn removal_ledger_and_add_request_are_hard_bounded() {
        let receipt = ProtocolEvent::Pong;
        let mut ledger = McpRemovalLedger::default();
        for index in 0..MAX_MCP_REMOVAL_RECEIPTS {
            let request_id = format!("request-{index}");
            ledger.receipts.insert(
                request_id.clone(),
                (
                    RemoveMcpServerCommand {
                        lifecycle_version: MCP_LIFECYCLE_VERSION,
                        request_id,
                        name: "server".into(),
                    },
                    receipt.clone(),
                ),
            );
        }
        assert!(ledger.is_full_for_new("new-request"));
        assert!(!ledger.is_full_for_new("request-0"));

        let oversized_name = "n".repeat(MAX_MCP_SERVER_NAME_LEN + 1);
        assert_eq!(
            mcp_add_request_rejection(&oversized_name, "stdio", None, None, None, None, None),
            Some("server name is empty or too long")
        );
        let oversized_value = "v".repeat(MAX_MCP_CONFIG_VALUE_LEN + 1);
        assert_eq!(
            mcp_add_request_rejection(
                "server",
                "stdio",
                Some(&oversized_value),
                None,
                None,
                None,
                None,
            ),
            Some("command or URL is too long")
        );
        let too_many_args = vec![String::new(); MAX_MCP_CONFIG_ENTRIES + 1];
        assert_eq!(
            mcp_add_request_rejection(
                "server",
                "stdio",
                None,
                Some(&too_many_args),
                None,
                None,
                None,
            ),
            Some("argument list exceeds the MCP request limit")
        );

        let mut invalid_ledger = McpRemovalLedger::default();
        for command in [
            RemoveMcpServerCommand {
                lifecycle_version: MCP_LIFECYCLE_VERSION,
                request_id: "r".repeat(MAX_MCP_REQUEST_ID_LEN + 1),
                name: "server".into(),
            },
            RemoveMcpServerCommand {
                lifecycle_version: MCP_LIFECYCLE_VERSION,
                request_id: "bounded-id".into(),
                name: "n".repeat(MAX_MCP_SERVER_NAME_LEN + 1),
            },
        ] {
            invalid_ledger.record(&command, &receipt);
        }
        assert!(invalid_ledger.receipts.is_empty());

        let oversized = RemoveMcpServerCommand {
            lifecycle_version: MCP_LIFECYCLE_VERSION,
            request_id: "r".repeat(MAX_MCP_REQUEST_ID_LEN + 1),
            name: "n".repeat(MAX_MCP_SERVER_NAME_LEN + 1),
        };
        let value = serde_json::to_value(mcp_removal_receipt(
            &oversized,
            McpRemovalOutcome::InvalidRequest,
            Vec::new(),
        ))
        .unwrap();
        assert_eq!(value["request_id"], "<invalid>");
        assert_eq!(value["name"], "<invalid>");
    }

    #[test]
    fn removal_ledger_binds_full_command_and_never_overwrites() {
        let original = RemoveMcpServerCommand {
            lifecycle_version: MCP_LIFECYCLE_VERSION,
            request_id: "stable-request".into(),
            name: "server".into(),
        };
        let original_receipt = mcp_removal_receipt(
            &original,
            McpRemovalOutcome::Removed,
            vec!["server_tool".into()],
        );
        let mut ledger = McpRemovalLedger::default();
        ledger.record(&original, &original_receipt);

        for conflicting in [
            RemoveMcpServerCommand {
                lifecycle_version: MCP_LIFECYCLE_VERSION + 1,
                ..original.clone()
            },
            RemoveMcpServerCommand {
                name: "different-server".into(),
                ..original.clone()
            },
            RemoveMcpServerCommand {
                name: String::new(),
                ..original.clone()
            },
        ] {
            let conflict = ledger
                .replay_or_conflict(&conflicting)
                .expect("a reused request id must terminate");
            assert_eq!(
                serde_json::to_value(&conflict).unwrap()["outcome"],
                "request_id_conflict"
            );
            ledger.record(&conflicting, &conflict);
        }

        assert_eq!(
            serde_json::to_value(
                ledger
                    .replay_or_conflict(&original)
                    .expect("original exact command must remain replayable")
            )
            .unwrap(),
            serde_json::to_value(original_receipt).unwrap()
        );
    }

    #[test]
    fn unsupported_runtime_diagnostics_request_gets_correlated_terminal_event() {
        let event = runtime_diagnostics_admission_rejection(
            &wcore_protocol::diagnostics::GetRuntimeDiagnosticsCommand {
                diagnostics_version: 2,
                request_id: "diagnostics-request-2".into(),
            },
        )
        .expect("unsupported version must be rejected");
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "runtime_diagnostics_unavailable");
        assert_eq!(value["diagnostics_version"], 2);
        assert_eq!(value["supported_version"], 1);
        assert_eq!(value["request_id"], "diagnostics-request-2");
        assert_eq!(value["reason"], "unsupported_version");
    }

    fn lifecycle_reservations(
        configs: &HashMap<String, McpServerConfig>,
    ) -> HashMap<String, McpConnectionReservation> {
        let catalog = McpLifecycleCatalog::new();
        configs
            .iter()
            .map(|(name, config)| {
                let reservation =
                    match catalog.reserve(name.clone(), McpConfigIdentity::for_server(config)) {
                        McpReservationOutcome::Acquired(reservation) => reservation,
                        McpReservationOutcome::Existing(_) => {
                            panic!("each fixture server name must reserve once")
                        }
                        McpReservationOutcome::CapacityExceeded => {
                            panic!("fixture must stay below lifecycle capacity")
                        }
                    };
                (name.clone(), reservation)
            })
            .collect()
    }

    fn lifecycle_test_plan(
        disposition: wcore_agent::recovery::RecoveryDisposition,
    ) -> wcore_agent::recovery::RecoveryPlan {
        wcore_agent::recovery::RecoveryPlan {
            session_id: "f14-lifecycle".into(),
            journal_sequence: Some(7),
            journal_digest: "a".repeat(64),
            state_digest: "b".repeat(64),
            budget: wcore_protocol::events::RecoveryBudgetSnapshot {
                tokens_used: 0,
                token_limit: None,
                cost_used_usd: 0.0,
                cost_limit_usd: None,
            },
            disposition,
        }
    }

    #[test]
    fn interrupted_action_lifecycle_reports_durable_post_stop_state_f14() {
        use wcore_agent::recovery::{RecoveryBlocker, RecoveryDisposition};

        assert_eq!(
            interrupted_action_lifecycle(
                &lifecycle_test_plan(RecoveryDisposition::Ready),
                RecoveryLifecycle::Cancelled,
            ),
            (RecoveryLifecycle::Cancelled, None)
        );
        assert_eq!(
            interrupted_action_lifecycle(
                &lifecycle_test_plan(RecoveryDisposition::Blocked {
                    turn_id: "turn-provider".into(),
                    reason: RecoveryBlocker::ProviderOutcomeUnknown,
                }),
                RecoveryLifecycle::Cancelled,
            ),
            (
                RecoveryLifecycle::Suspended,
                Some(RecoveryReconcileReason::ProviderOutcomeUnknown),
            )
        );
        assert_eq!(
            interrupted_action_lifecycle(
                &lifecycle_test_plan(RecoveryDisposition::ReconciliationRequired {
                    turn_id: "turn-tool".into(),
                    tool_execution_ids: vec!["tool-1".into()],
                }),
                RecoveryLifecycle::Cancelled,
            ),
            (
                RecoveryLifecycle::ReconciliationRequired,
                Some(RecoveryReconcileReason::ToolOutcomeUnknown),
            )
        );
    }

    // --- #314 D-1: the workspace_policy receipt on the REFUSAL paths ------
    //
    // `docs/json-stream-protocol.md` 2.3.3 promises the receipt after ANY
    // grant_path / revoke_path, and `emit_path_revoke` already honours that in
    // both its found and not-found arms. `emit_path_grant` and
    // `emit_workspace_capability_grant` skipped it on their two refusal exits
    // each, so a host could only detect a refusal by the ABSENCE of a frame --
    // which is indistinguishable from a frame that has not arrived yet.

    fn grant_test_receipt() -> wcore_types::workspace_trust::WorkspacePolicyReceipt {
        use wcore_types::workspace_trust::{
            AuthoritySource, EffectiveWorkspaceTrust, WorkspaceSandboxProfile,
        };
        wcore_types::workspace_trust::WorkspacePolicyReceipt {
            trust: EffectiveWorkspaceTrust::untrusted(
                AuthoritySource::LocalSession,
                "test-fingerprint",
                "test",
            ),
            profile: WorkspaceSandboxProfile::Strict,
            backend: "test".to_string(),
            writable_roots: Vec::new(),
            readable_roots: Vec::new(),
            capabilities: Vec::new(),
        }
    }

    fn receipt_count(writer: &CapturingProtocolEmitter) -> usize {
        writer
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, ProtocolEvent::WorkspacePolicy { .. }))
            .count()
    }

    fn info_messages(writer: &CapturingProtocolEmitter) -> Vec<String> {
        writer
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                ProtocolEvent::Info { message, .. } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    /// Refusal 1 of 2: the launcher never opted in. The host still gets the
    /// receipt, so "what can this chat reach" stays answerable.
    #[test]
    fn path_grant_refused_by_the_launcher_still_emits_the_policy_receipt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = wcore_tools::workspace_policy::WorkspacePolicy::trusted_local(dir.path())
            .with_local_operator_principal();
        let mut receipt = grant_test_receipt();
        let writer = CapturingProtocolEmitter::default();

        emit_path_grant(
            false,
            &policy,
            &mut receipt,
            PathGrantRequest {
                grant_id: "g1".to_string(),
                root: dir.path().to_string_lossy().into_owned(),
                access: wcore_protocol::commands::PathGrantAccess::Read,
                expires_at_ms: None,
            },
            &writer,
        );

        assert_eq!(
            receipt_count(&writer),
            1,
            "a launcher-refused grant_path must still publish workspace_policy"
        );
        assert!(
            info_messages(&writer)
                .iter()
                .any(|m| m.contains("--allow-host-path-grants")),
            "the refusal reason must still be named: {:?}",
            info_messages(&writer)
        );
        // The receipt is the FIRST frame, matching the success path's order, so
        // a host that reads state-then-message sees them in one order always.
        assert!(matches!(
            writer.events.lock().unwrap().first(),
            Some(ProtocolEvent::WorkspacePolicy { .. })
        ));
    }

    /// Refusal 2 of 2: the launcher opted in but the POLICY refused the folder.
    #[test]
    fn path_grant_refused_by_policy_still_emits_the_policy_receipt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = wcore_tools::workspace_policy::WorkspacePolicy::trusted_local(dir.path())
            .with_local_operator_principal();
        let mut receipt = grant_test_receipt();
        let writer = CapturingProtocolEmitter::default();

        // A path that cannot be canonicalized -> PathGrantError::Resolve.
        let missing = dir.path().join("no-such-folder");
        emit_path_grant(
            true,
            &policy,
            &mut receipt,
            PathGrantRequest {
                grant_id: "g2".to_string(),
                root: missing.to_string_lossy().into_owned(),
                access: wcore_protocol::commands::PathGrantAccess::Read,
                expires_at_ms: None,
            },
            &writer,
        );

        assert!(
            info_messages(&writer)
                .iter()
                .any(|m| m.starts_with("path grant refused:")),
            "expected a policy refusal, got {:?}",
            info_messages(&writer)
        );
        assert_eq!(
            receipt_count(&writer),
            1,
            "a policy-refused grant_path must still publish workspace_policy"
        );
    }

    /// The capability grant has the same two exits and the same defect.
    #[test]
    fn workspace_capability_grant_refused_by_the_launcher_still_emits_the_receipt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = wcore_tools::workspace_policy::WorkspacePolicy::trusted_local(dir.path())
            .with_local_operator_principal();
        let mut receipt = grant_test_receipt();
        let writer = CapturingProtocolEmitter::default();

        emit_workspace_capability_grant(false, &policy, &mut receipt, "cargo", &writer);

        assert_eq!(receipt_count(&writer), 1);
        assert!(
            info_messages(&writer)
                .iter()
                .any(|m| m.contains("--allow-host-workspace-grants")),
            "{:?}",
            info_messages(&writer)
        );
    }

    #[test]
    fn workspace_capability_grant_refused_by_policy_still_emits_the_receipt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = wcore_tools::workspace_policy::WorkspacePolicy::trusted_local(dir.path())
            .with_local_operator_principal();
        let mut receipt = grant_test_receipt();
        let writer = CapturingProtocolEmitter::default();

        emit_workspace_capability_grant(
            true,
            &policy,
            &mut receipt,
            "wayland-core-no-such-executable",
            &writer,
        );

        assert!(
            info_messages(&writer)
                .iter()
                .any(|m| m.starts_with("workspace capability grant refused:")),
            "expected a policy refusal, got {:?}",
            info_messages(&writer)
        );
        assert_eq!(receipt_count(&writer), 1);
    }

    /// Every `grant_refused` frame this writer saw, as
    /// `(grant_id, surface, reason)`. #314 c5.
    fn refusals(
        writer: &CapturingProtocolEmitter,
    ) -> Vec<(Option<String>, GrantSurface, GrantRefusalReason)> {
        writer
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                ProtocolEvent::GrantRefused {
                    grant_id,
                    surface,
                    reason,
                    ..
                } => Some((grant_id.clone(), *surface, *reason)),
                _ => None,
            })
            .collect()
    }

    /// #314 c5. All FOUR refusal exits, in ONE test, each asserted on the
    /// TYPED frame rather than on prose.
    ///
    /// The observable is deliberately NOT the `info` line the old tests above
    /// grade: those pass unchanged while a host still has nothing to branch on,
    /// which is exactly why c5 outlived c4. What is graded here is that a host
    /// which never reads English can tell WHICH surface refused and WHY.
    ///
    /// The two causes are distinguished, not merged: `LocalOptInRequired` never
    /// reached the policy, `PolicyRejected` did. A fix that reported one reason
    /// for both would satisfy "a typed frame exists" and still leave the host
    /// unable to tell "turn on the flag" from "pick a different folder".
    #[test]
    fn every_grant_refusal_exit_is_machine_readable_not_prose() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = wcore_tools::workspace_policy::WorkspacePolicy::trusted_local(dir.path())
            .with_local_operator_principal();
        let missing = dir.path().join("no-such-folder");

        let path_request = |grant_id: &str, root: &std::path::Path| PathGrantRequest {
            grant_id: grant_id.to_string(),
            root: root.to_string_lossy().into_owned(),
            access: wcore_protocol::commands::PathGrantAccess::Read,
            expires_at_ms: None,
        };

        // Exit 1 of 4 -- grant_path, launcher never opted in.
        let w = CapturingProtocolEmitter::default();
        let mut receipt = grant_test_receipt();
        emit_path_grant(
            false,
            &policy,
            &mut receipt,
            path_request("g-launcher", &missing),
            &w,
        );
        assert_eq!(
            refusals(&w),
            vec![(
                Some("g-launcher".to_string()),
                GrantSurface::Path,
                GrantRefusalReason::LocalOptInRequired
            )],
            "grant_path refused by the launcher must publish a typed frame \
             carrying the host's own grant_id"
        );

        // Exit 2 of 4 -- grant_path, launcher opted in, POLICY refused.
        let w = CapturingProtocolEmitter::default();
        let mut receipt = grant_test_receipt();
        emit_path_grant(
            true,
            &policy,
            &mut receipt,
            path_request("g-policy", &missing),
            &w,
        );
        assert_eq!(
            refusals(&w),
            vec![(
                Some("g-policy".to_string()),
                GrantSurface::Path,
                GrantRefusalReason::PolicyRejected
            )],
            "a POLICY refusal must be a different reason from a LAUNCHER \
             refusal, or the host cannot tell the two remedies apart"
        );

        // Exit 3 of 4 -- capability, launcher never opted in. No grant_id
        // exists on this command, so the correlation key is null rather than
        // an invented value.
        let w = CapturingProtocolEmitter::default();
        let mut receipt = grant_test_receipt();
        emit_workspace_capability_grant(false, &policy, &mut receipt, "cargo", &w);
        assert_eq!(
            refusals(&w),
            vec![(
                None,
                GrantSurface::WorkspaceCapability,
                GrantRefusalReason::LocalOptInRequired
            )]
        );

        // Exit 4 of 4 -- capability, launcher opted in, POLICY refused.
        let w = CapturingProtocolEmitter::default();
        let mut receipt = grant_test_receipt();
        emit_workspace_capability_grant(
            true,
            &policy,
            &mut receipt,
            "wayland-core-no-such-executable",
            &w,
        );
        assert_eq!(
            refusals(&w),
            vec![(
                None,
                GrantSurface::WorkspaceCapability,
                GrantRefusalReason::PolicyRejected
            )]
        );
    }

    /// NEGATIVE CONTROL for #314 c5 -- blocks an always-fires fix.
    ///
    /// A refusal event emitted on the SUCCESS path would satisfy the test
    /// above (which only ever inspects refusal runs) while telling every host
    /// that a granted folder was denied. The receipt and the human line must
    /// still be there, so this also proves the restructure did not drop the
    /// success path on the floor.
    #[test]
    fn a_granted_path_emits_no_refusal_frame() {
        let dir = tempfile::tempdir().expect("tempdir");
        let grantable = dir.path().join("shared");
        std::fs::create_dir_all(&grantable).expect("mkdir");
        let policy = wcore_tools::workspace_policy::WorkspacePolicy::trusted_local(dir.path())
            .with_local_operator_principal();
        let mut receipt = grant_test_receipt();
        let writer = CapturingProtocolEmitter::default();

        emit_path_grant(
            true,
            &policy,
            &mut receipt,
            PathGrantRequest {
                grant_id: "g-ok".to_string(),
                root: grantable.to_string_lossy().into_owned(),
                access: wcore_protocol::commands::PathGrantAccess::Read,
                expires_at_ms: None,
            },
            &writer,
        );

        assert_eq!(
            refusals(&writer),
            vec![],
            "a GRANTED folder must not publish a refusal"
        );
        assert_eq!(receipt_count(&writer), 1);
        assert!(
            info_messages(&writer)
                .iter()
                .any(|m| m.starts_with("folder granted for this session:")),
            "the success line must survive the c5 restructure: {:?}",
            info_messages(&writer)
        );
    }

    /// NEGATIVE CONTROL -- passes in BOTH arms. A GRANTED path still emits
    /// exactly one receipt: the fix must not double-publish on success.
    #[test]
    fn a_granted_path_still_emits_exactly_one_receipt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let grantable = dir.path().join("shared");
        std::fs::create_dir_all(&grantable).expect("mkdir");
        let policy = wcore_tools::workspace_policy::WorkspacePolicy::trusted_local(dir.path())
            .with_local_operator_principal();
        let mut receipt = grant_test_receipt();
        let writer = CapturingProtocolEmitter::default();

        emit_path_grant(
            true,
            &policy,
            &mut receipt,
            PathGrantRequest {
                grant_id: "g3".to_string(),
                root: grantable.to_string_lossy().into_owned(),
                access: wcore_protocol::commands::PathGrantAccess::Read,
                expires_at_ms: None,
            },
            &writer,
        );

        let msgs = info_messages(&writer);
        assert!(
            msgs.iter().any(|m| m.starts_with("folder granted")),
            "expected a successful grant, got {msgs:?}"
        );
        assert_eq!(receipt_count(&writer), 1);
    }

    /// NEGATIVE CONTROL -- passes in BOTH arms. `revoke_path` was already
    /// unconditional in both arms and must stay that way.
    #[test]
    fn revoke_emits_the_receipt_whether_or_not_the_grant_existed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let policy = wcore_tools::workspace_policy::WorkspacePolicy::trusted_local(dir.path())
            .with_local_operator_principal();
        let mut receipt = grant_test_receipt();
        let writer = CapturingProtocolEmitter::default();

        emit_path_revoke(&policy, &mut receipt, "never-granted", &writer);

        assert_eq!(receipt_count(&writer), 1);
        assert!(
            info_messages(&writer)
                .iter()
                .any(|m| m.contains("no folder grant with id never-granted")),
            "{:?}",
            info_messages(&writer)
        );
    }

    #[derive(Default)]
    struct CapturingProtocolEmitter {
        events: std::sync::Mutex<Vec<ProtocolEvent>>,
    }

    impl ProtocolEmitter for CapturingProtocolEmitter {
        fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
            self.events.lock().unwrap().push(event.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct CapturingOutputSink {
        errors: std::sync::Mutex<Vec<String>>,
        stream_ends: std::sync::Mutex<Vec<(String, FinishReason)>>,
    }

    impl OutputSink for CapturingOutputSink {
        fn emit_text_delta(&self, _text: &str, _msg_id: &str) {}
        fn emit_thinking(&self, _text: &str, _msg_id: &str) {}
        fn emit_tool_call(&self, _name: &str, _input: &str) {}
        fn emit_tool_result(&self, _name: &str, _is_error: bool, _content: &str) {}
        fn emit_stream_start(&self, _msg_id: &str) {}
        fn emit_stream_end(
            &self,
            msg_id: &str,
            _turns: usize,
            _input_tokens: u64,
            _output_tokens: u64,
            _cache_creation_tokens: u64,
            _cache_read_tokens: u64,
            finish_reason: wcore_types::message::FinishReason,
        ) {
            self.stream_ends
                .lock()
                .unwrap()
                .push((msg_id.to_owned(), finish_reason));
        }
        fn emit_error(
            &self,
            msg: &str,
            _retryable: bool,
            _category: wcore_protocol::events::FailureCategory,
        ) {
            self.errors.lock().unwrap().push(msg.to_owned());
        }
        fn emit_info(&self, _msg: &str) {}
    }

    fn recovery_engine(
        session_id: &str,
    ) -> (
        tempfile::TempDir,
        wcore_agent::engine::AgentEngine,
        wcore_agent::session_journal::SessionJournal,
        wcore_protocol::events::RecoveryCursor,
    ) {
        let directory = tempfile::tempdir().expect("recovery tempdir");
        let manager = wcore_agent::session::SessionManager::new(directory.path().into(), 10);
        let active = manager
            .create_for_run("test", "test-model", "/tmp", Some(session_id))
            .expect("create recovery session");
        active
            .journal
            .append(wcore_agent::session_journal::SessionEvent::TurnStarted {
                turn_id: "turn-recovery".into(),
                user_message: "content must stay out of recovery frames".into(),
            })
            .expect("append interrupted turn");
        let cursor = wcore_agent::recovery::RecoveryPlan::from_journal(&active.journal)
            .expect("plan recovery")
            .cursor();
        let journal = active.journal.clone();
        let mut config = wcore_config::config::Config::default();
        config.session.enabled = true;
        config.session.directory = directory.path().to_string_lossy().into_owned();
        let engine = wcore_agent::engine::AgentEngine::resume_active(
            config,
            wcore_tools::registry::ToolRegistry::new(),
            Arc::new(wcore_agent::output::null_sink::NullSink),
            active,
        );
        (directory, engine, journal, cursor)
    }

    fn unknown_tool_recovery_engine(
        session_id: &str,
    ) -> (
        tempfile::TempDir,
        wcore_agent::engine::AgentEngine,
        wcore_agent::session_journal::SessionJournal,
        wcore_protocol::events::RecoveryCursor,
        String,
    ) {
        let directory = tempfile::tempdir().expect("operator recovery tempdir");
        let manager = wcore_agent::session::SessionManager::new(directory.path().into(), 10);
        let active = manager
            .create_for_run("test", "test-model", "/tmp", Some(session_id))
            .expect("create operator recovery session");
        active
            .journal
            .append(wcore_agent::session_journal::SessionEvent::TurnStarted {
                turn_id: "turn-operator-recovery".into(),
                user_message: "interrupted".into(),
            })
            .expect("append interrupted turn");
        let unknown =
            wcore_agent::journal_effects::JournalEffectCoordinator::new(active.journal.clone())
                .for_turn("turn-operator-recovery")
                .prepare_tool(
                    "provider-tool-call",
                    0,
                    "OpaqueRemote",
                    json!({}),
                    json!({}),
                )
                .expect("prepare unknown tool")
                .start()
                .expect("start unknown tool")
                .unknown(
                    wcore_agent::session_journal::ToolUnknownReason::AmbiguousFailure {
                        error: "remote outcome unavailable".into(),
                    },
                    json!({"adapter": "opaque"}),
                )
                .expect("record unknown tool effect");
        let tool_execution_id = unknown.id().to_owned();
        drop(unknown);
        let cursor = wcore_agent::recovery::RecoveryPlan::from_journal(&active.journal)
            .expect("plan operator recovery")
            .cursor();
        let journal = active.journal.clone();
        let mut config = wcore_config::config::Config::default();
        config.session.enabled = true;
        config.session.directory = directory.path().to_string_lossy().into_owned();
        let engine = wcore_agent::engine::AgentEngine::resume_active(
            config,
            wcore_tools::registry::ToolRegistry::new(),
            Arc::new(wcore_agent::output::null_sink::NullSink),
            active,
        );
        (directory, engine, journal, cursor, tool_execution_id)
    }

    fn operator_resolution_command(
        session_id: &str,
        cursor: wcore_protocol::events::RecoveryCursor,
        tool_execution_id: &str,
    ) -> ProtocolCommand {
        ProtocolCommand::ResolveUnknownToolEffect(
            wcore_protocol::events::OperatorToolEffectResolution {
                recovery_version: RECOVERY_PROTOCOL_VERSION,
                session_id: session_id.into(),
                turn_id: "turn-operator-recovery".into(),
                cursor,
                tool_execution_id: tool_execution_id.into(),
                outcome: wcore_protocol::events::OperatorToolEffectOutcome::Succeeded,
                operator_id: "operator-7".into(),
                evidence: wcore_protocol::events::OperatorResolutionEvidence {
                    source:
                        wcore_protocol::events::OperatorResolutionEvidenceSource::ExternalSystemRecord,
                    reference_id: "record-11".into(),
                    observed_at_unix_ms: 1_721_000_003_000,
                    digest: format!("sha256:{}", "b".repeat(64)),
                },
            },
        )
    }

    #[test]
    fn json_recovery_resync_emits_contract_reducible_non_empty_replay() {
        let (_directory, engine, journal, cursor) = recovery_engine("f14a0001");
        journal
            .append(wcore_agent::session_journal::SessionEvent::TurnCancelled {
                turn_id: "turn-recovery".into(),
            })
            .expect("advance recovery journal beyond host cursor");
        let writer = CapturingProtocolEmitter::default();

        handle_session_resync(
            &engine,
            &writer,
            RECOVERY_PROTOCOL_VERSION,
            "request-snapshot".into(),
            "f14a0001".into(),
            Some(cursor),
        );

        let events = writer.events.lock().unwrap();
        let [
            ProtocolEvent::SessionRecoverySnapshot {
                cursor: snapshot_cursor,
                state_digest,
                lifecycle: RecoveryLifecycle::Suspended,
                pending_turn: Some(_),
                ..
            },
            ProtocolEvent::SessionRecoveryReplay {
                from: Some(replay_from),
                through,
                items,
                ..
            },
        ] = events.as_slice()
        else {
            panic!("expected one snapshot followed by one non-empty replay");
        };
        assert_eq!(snapshot_cursor, replay_from);
        let is_raw_digest = |digest: &str| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        };
        assert!(is_raw_digest(&snapshot_cursor.journal_digest));
        assert!(is_raw_digest(state_digest));
        assert_eq!(items.len(), 1);
        assert_eq!(&items[0].cursor, through);
        assert!(items[0].cursor.journal_sequence > replay_from.journal_sequence);

        let wire = serde_json::to_value(&*events).unwrap();
        assert_eq!(wire[0]["cursor"], wire[1]["from"]);
        let round_trip: wcore_protocol::events::RecoveryCursor =
            serde_json::from_value(wire[0]["cursor"].clone()).unwrap();
        assert_eq!(&round_trip, snapshot_cursor);
        assert_eq!(wire[1]["through"], wire[1]["items"][0]["cursor"]);
        assert_eq!(wire[1]["items"][0]["kind"], "turn_cancelled");
        let encoded = serde_json::to_string(&wire).unwrap();
        assert!(!encoded.contains("content must stay out of recovery frames"));
    }

    #[test]
    fn json_recovery_resync_rejects_stale_digest_without_snapshot() {
        let (_directory, engine, _journal, mut cursor) = recovery_engine("f14a0002");
        cursor.journal_digest = "stale".into();
        let writer = CapturingProtocolEmitter::default();

        handle_session_resync(
            &engine,
            &writer,
            RECOVERY_PROTOCOL_VERSION,
            "request-stale".into(),
            "f14a0002".into(),
            Some(cursor),
        );

        assert!(matches!(
            writer.events.lock().unwrap().as_slice(),
            [ProtocolEvent::SessionRecoveryUnavailable {
                reason: RecoveryUnavailableReason::CursorDigestMismatch,
                ..
            }]
        ));
    }

    #[tokio::test]
    async fn json_recovery_cancel_is_durable_and_cursor_bound() {
        let (_directory, mut engine, _journal, cursor) = recovery_engine("f14a0003");
        let writer = CapturingProtocolEmitter::default();
        let output = CapturingOutputSink::default();
        let approval_manager = ToolApprovalManager::new();
        let (_cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(1);

        handle_resume_turn(
            &mut engine,
            &writer,
            &output,
            &mut cmd_rx,
            &approval_manager,
            &|| {},
            RECOVERY_PROTOCOL_VERSION,
            "request-cancel".into(),
            "f14a0003".into(),
            "turn-recovery".into(),
            cursor,
            ResumeTurnAction::Cancel,
        )
        .await;

        assert!(matches!(
            writer.events.lock().unwrap().as_slice(),
            [ProtocolEvent::TurnRecoveryLifecycle {
                lifecycle: RecoveryLifecycle::Cancelled,
                ..
            }]
        ));
        assert!(matches!(
            engine.recovery_plan().unwrap().disposition,
            wcore_agent::recovery::RecoveryDisposition::Ready
        ));
        assert_eq!(
            *output.stream_ends.lock().unwrap(),
            vec![("request-cancel".into(), FinishReason::Stop)]
        );
    }

    /// Helper: drive `handle_resume_turn` once and hand back what the host saw.
    #[allow(clippy::too_many_arguments)]
    async fn drive_resume(
        engine: &mut wcore_agent::engine::AgentEngine,
        session: &str,
        request_id: &str,
        turn_id: &str,
        cursor: wcore_protocol::events::RecoveryCursor,
        action: ResumeTurnAction,
    ) -> (CapturingProtocolEmitter, CapturingOutputSink) {
        let writer = CapturingProtocolEmitter::default();
        let output = CapturingOutputSink::default();
        let approval_manager = ToolApprovalManager::new();
        let (_cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(1);
        handle_resume_turn(
            engine,
            &writer,
            &output,
            &mut cmd_rx,
            &approval_manager,
            &|| {},
            RECOVERY_PROTOCOL_VERSION,
            request_id.into(),
            session.into(),
            turn_id.into(),
            cursor,
            action,
        )
        .await;
        (writer, output)
    }

    fn lifecycles(writer: &CapturingProtocolEmitter) -> Vec<RecoveryLifecycle> {
        writer
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                ProtocolEvent::TurnRecoveryLifecycle { lifecycle, .. } => Some(*lifecycle),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn json_recovery_abandon_terminates_the_turn() {
        let (_directory, mut engine, _journal, cursor) = recovery_engine("f14a0010");
        let (writer, output) = drive_resume(
            &mut engine,
            "f14a0010",
            "request-abandon",
            "turn-recovery",
            cursor,
            ResumeTurnAction::Abandon,
        )
        .await;

        assert_eq!(lifecycles(&writer), vec![RecoveryLifecycle::Cancelled]);
        assert!(matches!(
            engine.recovery_plan().unwrap().disposition,
            wcore_agent::recovery::RecoveryDisposition::Ready
        ));
        // The host must be able to settle its UI from a frame, not from silence.
        assert_eq!(
            *output.stream_ends.lock().unwrap(),
            vec![("request-abandon".into(), FinishReason::Stop)]
        );
    }

    #[tokio::test]
    async fn json_recovery_abandon_is_idempotent() {
        let (_directory, mut engine, _journal, cursor) = recovery_engine("f14a0011");
        let (first, _) = drive_resume(
            &mut engine,
            "f14a0011",
            "abandon-1",
            "turn-recovery",
            cursor.clone(),
            ResumeTurnAction::Abandon,
        )
        .await;
        assert_eq!(lifecycles(&first), vec![RecoveryLifecycle::Cancelled]);

        // Second click. The turn is gone, the cursor the host still holds is now
        // stale, and BOTH of those are the ordinary case for this verb.
        let (second, output) = drive_resume(
            &mut engine,
            "f14a0011",
            "abandon-2",
            "turn-recovery",
            cursor,
            ResumeTurnAction::Abandon,
        )
        .await;
        assert_eq!(
            lifecycles(&second),
            vec![RecoveryLifecycle::Cancelled],
            "a second abandon must succeed, not error"
        );
        assert_eq!(
            *output.stream_ends.lock().unwrap(),
            vec![("abandon-2".into(), FinishReason::Stop)],
            "and must still emit a terminal frame so the UI settles"
        );
    }

    #[tokio::test]
    async fn json_recovery_abandon_survives_a_stale_cursor_where_cancel_refuses() {
        // ANTI-VACUITY: the same stale cursor is driven through Cancel FIRST. If
        // this arm did not refuse, the "abandon tolerates it" assertion below
        // would be passing on a cursor that was never actually stale.
        let (_dir_a, mut cancel_engine, _j_a, mut stale) = recovery_engine("f14a0012");
        stale.journal_digest = "stale".into();
        let (cancel_writer, _) = drive_resume(
            &mut cancel_engine,
            "f14a0012",
            "cancel-stale",
            "turn-recovery",
            stale.clone(),
            ResumeTurnAction::Cancel,
        )
        .await;
        assert!(
            !lifecycles(&cancel_writer).contains(&RecoveryLifecycle::Cancelled),
            "control: cancel must REFUSE a stale cursor, or this test grades nothing"
        );
        assert!(
            !matches!(
                cancel_engine.recovery_plan().unwrap().disposition,
                wcore_agent::recovery::RecoveryDisposition::Ready
            ),
            "control: the turn must still be interrupted after a refused cancel"
        );

        // Same stale cursor, same starting state, abandon instead.
        let (_dir_b, mut engine, _j_b, mut also_stale) = recovery_engine("f14a0013");
        also_stale.journal_digest = "stale".into();
        let (writer, output) = drive_resume(
            &mut engine,
            "f14a0013",
            "abandon-stale",
            "turn-recovery",
            also_stale,
            ResumeTurnAction::Abandon,
        )
        .await;
        assert_eq!(
            lifecycles(&writer),
            vec![RecoveryLifecycle::Cancelled],
            "abandon exists for the case where host and engine disagree"
        );
        assert!(matches!(
            engine.recovery_plan().unwrap().disposition,
            wcore_agent::recovery::RecoveryDisposition::Ready
        ));
        assert_eq!(
            *output.stream_ends.lock().unwrap(),
            vec![("abandon-stale".into(), FinishReason::Stop)]
        );
    }

    #[tokio::test]
    async fn json_recovery_abandon_of_a_turn_the_engine_never_had_is_a_no_op() {
        let (_directory, mut engine, _journal, cursor) = recovery_engine("f14a0014");
        let (writer, output) = drive_resume(
            &mut engine,
            "f14a0014",
            "abandon-ghost",
            "turn-the-engine-never-heard-of",
            cursor,
            ResumeTurnAction::Abandon,
        )
        .await;

        assert_eq!(
            lifecycles(&writer),
            vec![RecoveryLifecycle::Cancelled],
            "a turn the engine does not hold is already over; that is success"
        );
        assert_eq!(
            *output.stream_ends.lock().unwrap(),
            vec![("abandon-ghost".into(), FinishReason::Stop)]
        );
        // ANTI-VACUITY: the REAL interrupted turn must be untouched. A no-op that
        // silently terminated whatever happened to be in flight would pass every
        // assertion above.
        assert!(
            matches!(
                engine.recovery_plan().unwrap().disposition,
                wcore_agent::recovery::RecoveryDisposition::ContinueTurnStart { .. }
                    | wcore_agent::recovery::RecoveryDisposition::ContinueCheckpoint { .. }
                    | wcore_agent::recovery::RecoveryDisposition::AwaitApproval { .. }
                    | wcore_agent::recovery::RecoveryDisposition::ReconciliationRequired { .. }
                    | wcore_agent::recovery::RecoveryDisposition::Blocked { .. }
            ),
            "abandoning an unknown turn must not terminate the one that IS in flight"
        );
    }

    #[test]
    fn abandon_is_accepted_on_the_wire() {
        let cmd: wcore_protocol::commands::ResumeTurnCommand = serde_json::from_value(json!({
            "recovery_version": 1,
            "request_id": "r",
            "session_id": "s",
            "turn_id": "t",
            "cursor": {"journal_sequence": 1, "journal_digest": "d"},
            "action": "abandon"
        }))
        .expect("`abandon` must deserialize");
        assert_eq!(cmd.action, ResumeTurnAction::Abandon);
        // Control: an action that does NOT exist must still be refused, or the
        // assertion above would pass against a permissive deserializer.
        assert!(
            serde_json::from_value::<wcore_protocol::commands::ResumeTurnCommand>(json!({
                "recovery_version": 1,
                "request_id": "r",
                "session_id": "s",
                "turn_id": "t",
                "cursor": {"journal_sequence": 1, "journal_digest": "d"},
                "action": "give_up"
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn recovered_turn_driver_resolves_multiple_approval_commands() {
        use wcore_protocol::commands::ApprovalScope;
        use wcore_protocol::events::ToolCategory;

        let approval_manager = Arc::new(ToolApprovalManager::new());
        let future_manager = approval_manager.clone();
        let (ready_tx, mut ready_rx) = tokio::sync::mpsc::unbounded_channel();
        let future = async move {
            let first =
                future_manager.request_approval("recovery-call-1", &ToolCategory::Exec, "Bash");
            ready_tx.send("recovery-call-1").unwrap();
            let first = first.await.unwrap();
            let second =
                future_manager.request_approval("recovery-call-2", &ToolCategory::Exec, "Write");
            ready_tx.send("recovery-call-2").unwrap();
            let second = second.await.unwrap();
            (first, second)
        };
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(2);
        let host = tokio::spawn(async move {
            assert_eq!(ready_rx.recv().await, Some("recovery-call-1"));
            cmd_tx
                .send(ProtocolCommand::ToolApprove {
                    call_id: "recovery-call-1".into(),
                    scope: ApprovalScope::Once,
                    answer: None,
                })
                .await
                .unwrap();
            assert_eq!(ready_rx.recv().await, Some("recovery-call-2"));
            cmd_tx
                .send(ProtocolCommand::ToolDeny {
                    call_id: "recovery-call-2".into(),
                    reason: "operator denied second recovered tool".into(),
                })
                .await
                .unwrap();
        });
        let writer = CapturingProtocolEmitter::default();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            drive_active_recovery(
                future,
                &mut cmd_rx,
                approval_manager.as_ref(),
                &empty_bridge(),
                &writer,
                &|| {},
            ),
        )
        .await
        .expect("multiple recovered approvals must not deadlock");
        host.await.unwrap();

        let ActiveRecoveryOutcome::Finished((first, second)) = outcome else {
            panic!("recovery driver stopped unexpectedly");
        };
        assert!(matches!(
            first,
            ToolApprovalResult::Approved { answer: None }
        ));
        assert!(matches!(
            second,
            ToolApprovalResult::Denied { reason }
                if reason == "operator denied second recovered tool"
        ));
    }

    // -----------------------------------------------------------------
    // FerroxLabs/wayland#1083 -- host EOF does not cancel approvals parked on
    // the OTHER approval store.
    //
    // The ticket's anchor is wrong. `deny_all_pending` is
    // `ToolApprovalManager::deny_all_pending`
    // (`wcore-protocol/src/lib.rs:340`), it is already wired to both EOF sites
    // by #1070, and the two tests above pin it. Nothing in `approval.rs` is
    // involved in that path.
    //
    // The store that IS stranded is `wcore_agent::approval::ApprovalBridge` --
    // a different crate with a different store (`by_token` + `by_corr`) and NO
    // bulk-deny of any kind. It backs the egress consent doorbell and the
    // Crucible proposal card. `deny_pending_approvals_on_host_eof` takes only a
    // `&ToolApprovalManager`, so a bridge approval parked when the host goes
    // away is untouched.
    //
    // Its ONLY escape is `reap_expired`, which needs the entry's TTL to have
    // elapsed and runs on a 30s reaper tick. A Crucible card is minted with
    // `CRUCIBLE_APPROVAL_TTL`, which is 86,400s. So the real symptom is not
    // the ticket's 300-second stall -- it is a TWENTY-FOUR HOUR one.
    //
    // The 2-second timeout is the assertion: if the reaper is what eventually
    // unblocks this, the test fails instead of hanging CI for a day.

    /// A bridge with nothing parked on it, for the #1070 tests that exercise
    /// only the `ToolApprovalManager` half of the EOF drain. Draining an empty
    /// bridge is a no-op, so these keep asserting exactly what they did before
    /// `drive_active_recovery` grew the parameter.
    fn empty_bridge() -> wcore_agent::approval::ApprovalBridge {
        wcore_agent::approval::ApprovalBridge::new()
    }

    fn bridge_request(call_id: &str) -> wcore_agent::approval::ApprovalRequest {
        wcore_agent::approval::ApprovalRequest {
            call_id: call_id.to_string(),
            reason: "crucible proposal card".to_string(),
            context: "multi-vendor cost decision".to_string(),
        }
    }

    /// RED ARM. A Crucible-TTL approval parked on the `ApprovalBridge` must be
    /// resolved when the host's command stream reaches EOF, exactly as a
    /// `ToolApprovalManager` approval already is.
    #[tokio::test]
    async fn host_command_stream_eof_cancels_a_parked_bridge_approval() {
        let approval_manager = Arc::new(ToolApprovalManager::new());
        let bridge = Arc::new(wcore_agent::approval::ApprovalBridge::new());

        // Park a Crucible card on the bridge -- 24h TTL, so the reaper is not
        // a plausible rescuer inside this test's timeout.
        let (_secret, parked_rx) = bridge
            .request_with_id_and_ttl(
                "crucible:eof-card".to_string(),
                bridge_request("crucible:eof-card"),
                wcore_agent::approval::CRUCIBLE_APPROVAL_TTL,
            )
            .await;

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let future = async move {
            ready_tx.send(()).unwrap();
            parked_rx.await
        };

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(1);
        let host = tokio::spawn(async move {
            ready_rx.await.unwrap();
            // The host process went away with the card still on screen.
            drop(cmd_tx);
        });
        let writer = CapturingProtocolEmitter::default();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            drive_active_recovery(
                future,
                &mut cmd_rx,
                approval_manager.as_ref(),
                bridge.as_ref(),
                &writer,
                &|| {},
            ),
        )
        .await
        .expect(
            "#1083: host EOF left an ApprovalBridge approval parked. \
             deny_pending_approvals_on_host_eof drains only the \
             ToolApprovalManager, and the bridge's sole escape is the TTL \
             reaper -- 86,400s for a Crucible card. The agent stalls for 24h.",
        );
        host.await.unwrap();

        let ActiveRecoveryOutcome::Finished(result) = outcome else {
            panic!("recovery driver stopped unexpectedly");
        };
        let result = result.expect("the parked bridge approval must resolve, not be dropped");
        assert!(
            !result.approved,
            "EOF must fail CLOSED: no approval decision can arrive after the \
             command stream closed, so the card must resolve as NOT approved"
        );
    }

    /// NEGATIVE CONTROL for the test above -- passes today and must keep
    /// passing. With the command stream still open, the same parked bridge
    /// approval is answered by the operator and delivers THAT decision.
    ///
    /// This proves the red arm's failure is the missing EOF drain and not a
    /// broken harness: parking on the bridge, awaiting it inside
    /// `drive_active_recovery`, and resolving it all work.
    #[tokio::test]
    async fn an_open_command_stream_still_answers_a_parked_bridge_approval() {
        let approval_manager = Arc::new(ToolApprovalManager::new());
        let bridge = Arc::new(wcore_agent::approval::ApprovalBridge::new());

        let (_secret, parked_rx) = bridge
            .request_with_id_and_ttl(
                "crucible:open-card".to_string(),
                bridge_request("crucible:open-card"),
                wcore_agent::approval::CRUCIBLE_APPROVAL_TTL,
            )
            .await;

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let future = async move {
            ready_tx.send(()).unwrap();
            parked_rx.await
        };

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(1);
        let resolver = bridge.clone();
        let host = tokio::spawn(async move {
            ready_rx.await.unwrap();
            resolver
                .resolve_by_correlation(
                    "crucible:open-card",
                    wcore_agent::approval::ApprovalOutcome {
                        approved: true,
                        modifications: None,
                        cancellation: None,
                    },
                )
                .await;
            // Held open past the decision, then closed so the driver returns.
            drop(cmd_tx);
        });
        let writer = CapturingProtocolEmitter::default();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            drive_active_recovery(
                future,
                &mut cmd_rx,
                approval_manager.as_ref(),
                bridge.as_ref(),
                &writer,
                &|| {},
            ),
        )
        .await
        .expect("an answered bridge approval must not deadlock");
        host.await.unwrap();

        let ActiveRecoveryOutcome::Finished(result) = outcome else {
            panic!("recovery driver stopped unexpectedly");
        };
        let result = result.expect("the parked bridge approval must resolve");
        assert!(
            result.approved,
            "an open stream must deliver the operator's APPROVAL, not an EOF \
             cancellation -- otherwise the red arm above would pass for the \
             wrong reason (everything cancels)"
        );
        // #1083 criterion 3 control: a decision the operator actually made
        // carries NO cancellation cause. If it did, the cause below would be
        // stamped on everything and would discriminate nothing.
        assert_eq!(
            result.cancellation, None,
            "an operator's answer is not a bridge cancellation"
        );
    }

    /// FerroxLabs/wayland#1083 criterion 3, END TO END. The EOF drain must not
    /// merely resolve the parked bridge approval — it must tell the waiter WHY,
    /// so a host disconnect is distinguishable from the TTL simply running out.
    ///
    /// At released v0.13.5 the waiter got `ApprovalOutcome { approved: false,
    /// modifications: None }` in both cases, byte for byte; the only
    /// discriminator was a `tracing::warn!` that never reaches stderr with
    /// `RUST_LOG` unset. Asserting only `!approved` (as the red arm above does)
    /// passes for a TTL expiry too, which is why that arm does not cover this.
    ///
    /// The 24h `CRUCIBLE_APPROVAL_TTL` and the 2-second timeout are the same
    /// guards the red arm uses: the reaper cannot be what resolves this.
    #[tokio::test]
    async fn host_eof_tells_the_bridge_waiter_that_the_stream_closed() {
        let approval_manager = Arc::new(ToolApprovalManager::new());
        let bridge = Arc::new(wcore_agent::approval::ApprovalBridge::new());

        let (_secret, parked_rx) = bridge
            .request_with_id_and_ttl(
                "crucible:eof-reason-card".to_string(),
                bridge_request("crucible:eof-reason-card"),
                wcore_agent::approval::CRUCIBLE_APPROVAL_TTL,
            )
            .await;

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let future = async move {
            ready_tx.send(()).unwrap();
            parked_rx.await
        };

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(1);
        let host = tokio::spawn(async move {
            ready_rx.await.unwrap();
            drop(cmd_tx);
        });
        let writer = CapturingProtocolEmitter::default();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            drive_active_recovery(
                future,
                &mut cmd_rx,
                approval_manager.as_ref(),
                bridge.as_ref(),
                &writer,
                &|| {},
            ),
        )
        .await
        .expect("the parked bridge approval must resolve on EOF, not after the 24h TTL");
        host.await.unwrap();

        let ActiveRecoveryOutcome::Finished(result) = outcome else {
            panic!("recovery driver stopped unexpectedly");
        };
        let result = result.expect("the parked bridge approval must resolve");
        assert!(!result.approved, "EOF must still fail CLOSED");
        assert_eq!(
            result.cancellation,
            Some(wcore_agent::approval::ApprovalCancelCause::HostStreamClosed),
            "the waiter must be told the HOST WENT AWAY. \
             Some(Expired) means the EOF path is reusing the TTL cause and a \
             consumer still cannot tell a disconnect from a slow operator; \
             None means no cause reaches the waiter at all"
        );
        assert_eq!(
            result.cancel_reason(),
            Some(wcore_agent::approval::ApprovalCancelCause::HostStreamClosed.reason())
        );
        assert_ne!(
            result.cancel_reason(),
            Some(HOST_EOF_DENY_REASON),
            "#1083 asked the bridge not to reuse #1070's string verbatim"
        );
    }

    /// FerroxLabs/wayland#1180 -- every `ApprovalResume` arm is WIRED.
    ///
    /// `crates/wcore-cli/tests/approval_resume_active_turn.rs` grades what
    /// `handle_approval_resume` DOES, driving it with the real egress-consent
    /// doorbell. It cannot see whether `main.rs` still calls it. An arm that
    /// stopped calling it would leave that suite green while a bridge-backed
    /// approval parked mid-turn waited out its TTL -- which is the exact shape
    /// of #1180 in the first place, and the reason its acceptance criterion is
    /// a mutation on the CALL SITE.
    ///
    /// Comment lines are stripped first: a scan that matches its own
    /// documentation grades nothing.
    #[test]
    fn every_approval_resume_arm_routes_through_the_shared_handler() {
        let source = include_str!("main.rs");
        let code: Vec<&str> = source
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//"))
            .collect();

        let arms: Vec<usize> = code
            .iter()
            .enumerate()
            .filter(|(_, line)| **line == "ProtocolCommand::ApprovalResume {")
            .map(|(i, _)| i)
            .collect();

        // POSITIVE CONTROL: both arms (mid-turn and between-turn) exist today.
        // If the match arm is ever spelled differently this scan finds nothing
        // and would pass while grading nothing at all.
        assert_eq!(
            arms.len(),
            2,
            "expected the 2 known ApprovalResume arms (mid-turn and \
             between-turn), found {} -- the pattern this scan looks for must \
             have changed, and the check below is now vacuous",
            arms.len()
        );

        for arm in arms {
            let window = code[arm..code.len().min(arm + 12)].join(" ");
            assert!(
                window.contains("handle_approval_resume("),
                "the ApprovalResume arm at code line {arm} (of comment-stripped \
                 source) does not route through the shared handler, so nothing \
                 resolves the parked approval and it waits out its TTL. \
                 Context: {window}"
            );
        }
    }

    /// FerroxLabs/wayland#1083 criterion 1 -- "awaited at EVERY EOF site".
    ///
    /// The tests around this one grade the shared drain HELPER. None of them
    /// grades the WIRING: a third `commands_open = false` site that forgot to
    /// call the helper would leave every one of them green while a parked
    /// Crucible card went back to waiting out its 24h TTL. Only one of the two
    /// existing sites (`drive_active_recovery`) has a behavioural test at all;
    /// the `run_json_stream_mode` inner loop has no harness of its own, so this
    /// is what stands between it and a silent regression.
    ///
    /// Comment lines are stripped first, so the prose above (which names both
    /// markers) cannot satisfy the check -- a scan that matches its own
    /// documentation grades nothing.
    #[test]
    fn every_command_stream_eof_site_drains_the_approval_bridge() {
        let source = include_str!("main.rs");
        let code: Vec<&str> = source
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//"))
            .collect();

        let eof_sites: Vec<usize> = code
            .iter()
            .enumerate()
            // Exact match, not `contains`: this test's own body mentions the
            // marker in a string literal, and a scan that finds ITSELF is a
            // scan that grades nothing.
            .filter(|(_, line)| **line == "commands_open = false;")
            .map(|(i, _)| i)
            .collect();

        // POSITIVE CONTROL: if the marker is ever renamed, the scan below finds
        // nothing and would pass vacuously. Both sites exist today.
        assert!(
            eof_sites.len() >= 2,
            "expected at least the 2 known host-EOF sites, found {} -- the \
                marker this scan looks for must have been renamed, and the check \
                below is now vacuous",
            eof_sites.len()
        );

        for site in eof_sites {
            let window = code[site..code.len().min(site + 8)].join(" ");
            assert!(
                window.contains("deny_pending_approvals_on_host_eof("),
                "the host-EOF site at code line {site} (of comment-stripped \
                    source) does not drain the approval stores. Every \
                    `commands_open = false` site must call \
                    deny_pending_approvals_on_host_eof, or a bridge approval \
                    parked there waits out CRUCIBLE_APPROVAL_TTL (86,400s). Context: {window}"
            );
        }
    }

    /// The EOF drain is invoked from BOTH `commands_open = false` sites -- the
    /// recovery driver (covered end to end above) and the `run_json_stream_mode`
    /// inner loop, which has no test harness of its own. This grades the shared
    /// helper both sites call: one approval parked on EACH store, drained
    /// together, each carrying its own store's distinct reason.
    #[tokio::test]
    async fn the_eof_drain_empties_both_stores_with_reasons_that_differ() {
        use wcore_protocol::events::ToolCategory;

        let approval_manager = ToolApprovalManager::new();
        let bridge = wcore_agent::approval::ApprovalBridge::new();

        let manager_parked =
            approval_manager.request_approval("eof-both", &ToolCategory::Exec, "Bash");
        let (_secret, bridge_parked) = bridge
            .request_with_id_and_ttl(
                "crucible:both-card".to_string(),
                bridge_request("crucible:both-card"),
                wcore_agent::approval::CRUCIBLE_APPROVAL_TTL,
            )
            .await;

        deny_pending_approvals_on_host_eof(&approval_manager, &bridge).await;

        let manager_result =
            tokio::time::timeout(std::time::Duration::from_secs(2), manager_parked)
                .await
                .expect("the manager approval must resolve on EOF")
                .expect("the manager approval must not be dropped");
        let bridge_result = tokio::time::timeout(std::time::Duration::from_secs(2), bridge_parked)
            .await
            .expect("the bridge approval must resolve on EOF")
            .expect("the bridge approval must not be dropped");

        assert!(
            matches!(manager_result, ToolApprovalResult::Denied { ref reason } if reason == HOST_EOF_DENY_REASON),
            "the ToolApprovalManager half must keep #1070's reason"
        );
        assert!(!bridge_result.approved, "the bridge half must fail closed");
        let bridge_reason = bridge_result
            .cancel_reason()
            .expect("the bridge half must carry a reason the waiter can read");
        assert_eq!(
            bridge_reason,
            wcore_agent::approval::ApprovalCancelCause::HostStreamClosed.reason()
        );
        assert_ne!(
            bridge_reason, HOST_EOF_DENY_REASON,
            "the two stores must not report EOF with the same string (#1083)"
        );
    }

    /// FerroxLabs/wayland#1070 (b) — the host's command stream reaching EOF
    /// while a tool is parked on its approval must resolve that approval
    /// immediately (denied), not after the approval TTL.
    ///
    /// Pre-fix, EOF only stopped the driver from reading commands; the parked
    /// `rx.await` then sat until the background reaper fired. Live UAT of the
    /// v0.13.1 candidate measured a 330-second stall before the correct
    /// `tool_cancelled` finally arrived.
    ///
    /// The manager keeps its DEFAULT 300-second TTL here on purpose, and the
    /// 1-second timeout is the assertion: if the reaper is what unblocks this,
    /// the test fails instead of hanging CI.
    #[tokio::test]
    async fn host_command_stream_eof_denies_a_parked_approval_immediately() {
        use wcore_protocol::events::ToolCategory;

        let approval_manager = Arc::new(ToolApprovalManager::new());
        let future_manager = approval_manager.clone();
        let (parked_tx, parked_rx) = tokio::sync::oneshot::channel();
        let future = async move {
            let parked = future_manager.request_approval("eof-call", &ToolCategory::Exec, "Bash");
            parked_tx.send(()).unwrap();
            parked.await.expect("the parked approval must resolve")
        };
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(1);
        let host = tokio::spawn(async move {
            parked_rx.await.unwrap();
            // The host process went away mid-approval.
            drop(cmd_tx);
        });
        let writer = CapturingProtocolEmitter::default();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            drive_active_recovery(
                future,
                &mut cmd_rx,
                approval_manager.as_ref(),
                &empty_bridge(),
                &writer,
                &|| {},
            ),
        )
        .await
        .expect("stdin EOF must resolve a parked approval promptly, not after the approval TTL");
        host.await.unwrap();

        let ActiveRecoveryOutcome::Finished(result) = outcome else {
            panic!("recovery driver stopped unexpectedly");
        };
        assert!(
            matches!(result, ToolApprovalResult::Denied { reason } if reason == HOST_EOF_DENY_REASON),
            "EOF must fail CLOSED, and say EOF was the cause rather than reusing the TTL reason"
        );
    }

    /// CONTROL for the test above: with the command stream still OPEN, the
    /// same parked approval is answered by the host and resolves as the
    /// operator decided — the EOF denial is not firing on every approval.
    #[tokio::test]
    async fn an_open_command_stream_still_answers_a_parked_approval() {
        use wcore_protocol::commands::ApprovalScope;
        use wcore_protocol::events::ToolCategory;

        let approval_manager = Arc::new(ToolApprovalManager::new());
        let future_manager = approval_manager.clone();
        let (parked_tx, parked_rx) = tokio::sync::oneshot::channel();
        let future = async move {
            let parked = future_manager.request_approval("open-call", &ToolCategory::Exec, "Bash");
            parked_tx.send(()).unwrap();
            parked.await.expect("the parked approval must resolve")
        };
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(1);
        let host = tokio::spawn(async move {
            parked_rx.await.unwrap();
            cmd_tx
                .send(ProtocolCommand::ToolApprove {
                    call_id: "open-call".into(),
                    scope: ApprovalScope::Once,
                    answer: None,
                })
                .await
                .unwrap();
            // Held open past the decision, then closed so the driver returns.
            drop(cmd_tx);
        });
        let writer = CapturingProtocolEmitter::default();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            drive_active_recovery(
                future,
                &mut cmd_rx,
                approval_manager.as_ref(),
                &empty_bridge(),
                &writer,
                &|| {},
            ),
        )
        .await
        .expect("an answered approval must not deadlock");
        host.await.unwrap();

        let ActiveRecoveryOutcome::Finished(result) = outcome else {
            panic!("recovery driver stopped unexpectedly");
        };
        assert!(
            matches!(result, ToolApprovalResult::Approved { answer: None }),
            "an open stream must deliver the operator's decision, not an EOF denial"
        );
    }

    #[tokio::test]
    async fn recovered_turn_stop_waits_until_engine_future_observes_cancellation() {
        let cancellation = wcore_agent::cancel::CancellationToken::new();
        let observed = cancellation.clone();
        let provider_dispatches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let future_dispatches = provider_dispatches.clone();
        let future = async move {
            if !observed.is_cancelled() {
                future_dispatches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            observed.cancelled().await;
            "engine-observed-cancellation"
        };
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(1);
        cmd_tx.send(ProtocolCommand::Stop).await.unwrap();
        let writer = CapturingProtocolEmitter::default();
        let approval_manager = ToolApprovalManager::new();

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            drive_active_recovery(
                future,
                &mut cmd_rx,
                &approval_manager,
                &empty_bridge(),
                &writer,
                &|| cancellation.cancel(),
            ),
        )
        .await
        .expect("Stop must be observed by the recovery future before it is dropped");

        assert!(matches!(
            outcome,
            ActiveRecoveryOutcome::Stopped("engine-observed-cancellation")
        ));
        assert_eq!(
            provider_dispatches.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a queued Stop must be applied before the recovered future's first poll"
        );
    }

    #[tokio::test]
    async fn active_recovery_answers_every_correlated_command_once() {
        let cancellation = wcore_agent::cancel::CancellationToken::new();
        let observed = cancellation.clone();
        let future = async move {
            observed.cancelled().await;
        };
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(5);
        for command in [
            r#"{"type":"session_resync","recovery_version":1,"request_id":"r1","session_id":"s1"}"#,
            r#"{"type":"resume_turn","recovery_version":1,"request_id":"r2","session_id":"s1","turn_id":"t1","cursor":{"journal_sequence":1,"journal_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"action":"continue"}"#,
            r#"{"type":"resolve_interrupted_approval","recovery_version":1,"request_id":"r3","session_id":"s1","turn_id":"t1","cursor":{"journal_sequence":1,"journal_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"approval_id":"a1","decision":"deny"}"#,
            r#"{"type":"resolve_unknown_tool_effect","recovery_version":1,"session_id":"s1","turn_id":"t1","cursor":{"journal_sequence":1,"journal_digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"tool_execution_id":"tool-1","outcome":"not_started","operator_id":"operator-1","evidence":{"source":"external_system_record","reference_id":"ref-1","observed_at_unix_ms":1,"digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}"#,
        ] {
            cmd_tx
                .send(serde_json::from_str(command).unwrap())
                .await
                .unwrap();
        }
        cmd_tx.send(ProtocolCommand::Stop).await.unwrap();
        let writer = CapturingProtocolEmitter::default();
        let approval_manager = ToolApprovalManager::new();

        let outcome = drive_active_recovery(
            future,
            &mut cmd_rx,
            &approval_manager,
            &empty_bridge(),
            &writer,
            &|| cancellation.cancel(),
        )
        .await;

        assert!(matches!(outcome, ActiveRecoveryOutcome::Stopped(())));
        let events = writer.events.lock().unwrap();
        assert_eq!(events.len(), 4);
        assert!(matches!(
            &events[0],
            ProtocolEvent::SessionRecoveryUnavailable { request_id, .. } if request_id == "r1"
        ));
        assert!(matches!(
            &events[1],
            ProtocolEvent::SessionRecoveryUnavailable { request_id, .. } if request_id == "r2"
        ));
        assert!(matches!(
            &events[2],
            ProtocolEvent::SessionRecoveryUnavailable { request_id, .. } if request_id == "r3"
        ));
        assert!(matches!(
            &events[3],
            ProtocolEvent::Error { msg_id: Some(msg_id), error }
                if msg_id == "tool-1" && error.code == "recovery_busy"
        ));
    }

    #[tokio::test]
    async fn recovered_approval_error_emits_exactly_one_terminal_stream_end() {
        let (_directory, mut engine, _journal, cursor) = recovery_engine("f14a0005");
        let writer = CapturingProtocolEmitter::default();
        let output = CapturingOutputSink::default();
        let approval_manager = ToolApprovalManager::new();
        let (_cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel(1);

        handle_recovered_approval(
            &mut engine,
            &writer,
            &output,
            &mut cmd_rx,
            &approval_manager,
            &|| {},
            wcore_protocol::commands::RECOVERED_APPROVAL_VERSION,
            "request-approval-error".into(),
            "f14a0005".into(),
            "turn-recovery".into(),
            cursor,
            "absent-approval".into(),
            wcore_protocol::commands::RecoveredApprovalDecision::Approve,
            None,
        )
        .await;

        assert_eq!(output.errors.lock().unwrap().len(), 1);
        assert_eq!(
            *output.stream_ends.lock().unwrap(),
            vec![("request-approval-error".into(), FinishReason::Error)]
        );
        assert!(matches!(
            writer.events.lock().unwrap().as_slice(),
            [ProtocolEvent::SessionRecoveryUnavailable { .. }]
        ));
    }

    #[test]
    fn json_operator_resolution_persists_exact_authority_and_emits_receipt() {
        let (_directory, engine, journal, cursor, tool_execution_id) =
            unknown_tool_recovery_engine("f14a0004");
        let writer = CapturingProtocolEmitter::default();
        let output = CapturingOutputSink::default();
        let command = operator_resolution_command("f14a0004", cursor.clone(), &tool_execution_id);

        handle_operator_tool_effect_resolution(&engine, &writer, &output, command);

        assert_eq!(*output.errors.lock().unwrap(), Vec::<String>::new());
        assert!(
            engine
                .tool_effects_requiring_reconciliation()
                .unwrap()
                .is_empty()
        );
        let state = journal.state().unwrap();
        let tool = &state.tools[&tool_execution_id];
        assert_eq!(
            tool.result
                .as_ref()
                .and_then(|result| result["content"].as_str()),
            Some("Operator evidence confirms the interrupted tool effect succeeded")
        );
        assert_eq!(
            tool.result
                .as_ref()
                .and_then(|result| result["is_error"].as_bool()),
            Some(false)
        );
        assert_eq!(
            tool.resolution_source,
            Some(
                wcore_agent::session_journal::ToolResolutionSource::Operator {
                    operator_id: "operator-7".into(),
                }
            )
        );
        assert_eq!(
            tool.resolution_evidence,
            Some(json!({
                "source": "external_system_record",
                "reference_id": "record-11",
                "observed_at_unix_ms": 1_721_000_003_000_u64,
                "digest": format!("sha256:{}", "b".repeat(64)),
            }))
        );
        assert!(matches!(
            writer.events.lock().unwrap().as_slice(),
            [ProtocolEvent::UnknownToolEffectResolved { resolution }]
                if resolution.session_id == "f14a0004"
                    && resolution.turn_id == "turn-operator-recovery"
                    && resolution.cursor == cursor
                    && resolution.tool_execution_id == tool_execution_id
        ));
    }

    #[test]
    fn json_failed_operator_resolution_persists_canonical_error_result() {
        let (_directory, engine, journal, cursor, tool_execution_id) =
            unknown_tool_recovery_engine("f14a0006");
        let writer = CapturingProtocolEmitter::default();
        let output = CapturingOutputSink::default();
        let mut command = operator_resolution_command("f14a0006", cursor, &tool_execution_id);
        let ProtocolCommand::ResolveUnknownToolEffect(resolution) = &mut command else {
            unreachable!()
        };
        resolution.outcome = wcore_protocol::events::OperatorToolEffectOutcome::Failed;

        handle_operator_tool_effect_resolution(&engine, &writer, &output, command);

        let state = journal.state().unwrap();
        let result = state.tools[&tool_execution_id]
            .result
            .as_ref()
            .expect("failed operator evidence must preserve a provider-visible result");
        assert_eq!(
            result["content"],
            "Operator evidence confirms the interrupted tool effect failed"
        );
        assert_eq!(result["is_error"], true);
        assert!(result.get("operator_resolution_evidence").is_some());
    }

    #[test]
    fn json_operator_resolution_rejects_stale_cursor_without_mutation_or_receipt() {
        let (_directory, engine, journal, mut cursor, tool_execution_id) =
            unknown_tool_recovery_engine("f14a0005");
        cursor.journal_digest = format!("sha256:{}", "c".repeat(64));
        let writer = CapturingProtocolEmitter::default();
        let output = wcore_agent::output::null_sink::NullSink;
        let command = operator_resolution_command("f14a0005", cursor, &tool_execution_id);

        handle_operator_tool_effect_resolution(&engine, &writer, &output, command);

        assert_eq!(
            engine.tool_effects_requiring_reconciliation().unwrap(),
            vec![tool_execution_id.clone()]
        );
        assert!(matches!(
            journal.state().unwrap().tools[&tool_execution_id].effect,
            wcore_agent::session_journal::ToolEffectState::Unknown { .. }
        ));
        assert!(writer.events.lock().unwrap().is_empty());
    }

    async fn pending_bundled_reference_session(
        extracted_root: Arc<std::sync::Mutex<Option<PathBuf>>>,
        ready: tokio::sync::oneshot::Sender<()>,
    ) -> anyhow::Result<ExitCode> {
        use wcore_skills::bundled::{BundledSkillCatalog, BundledSkillEntry};

        let mut catalog = BundledSkillCatalog::new();
        catalog.register(BundledSkillEntry {
            name: "signal-cleanup".into(),
            description: "signal cleanup fixture".into(),
            when_to_use: None,
            argument_hint: None,
            allowed_tools: Vec::new(),
            model: None,
            disable_model_invocation: false,
            user_invocable: false,
            context: None,
            agent: None,
            files: vec![("reference.txt".into(), "exact reference bytes".into())],
            content: "fixture".into(),
        });
        let skills = catalog.prepare_bundled_skills().await;
        let skill_root = PathBuf::from(
            skills[0]
                .skill_root
                .as_deref()
                .expect("reference extraction must succeed"),
        );
        let process_root = skill_root
            .parent()
            .and_then(Path::parent)
            .expect("skill root must be nested under catalog and process roots")
            .to_owned();
        assert_eq!(
            std::fs::read(skill_root.join("reference.txt")).expect("read reference bytes"),
            b"exact reference bytes"
        );
        *extracted_root.lock().expect("record extraction root") = Some(process_root);
        ready.send(()).expect("signal trigger must be waiting");
        std::future::pending().await
    }

    #[cfg(unix)]
    fn raise_native_shutdown_signal(kind: &str) {
        let signal = match kind {
            "sigint" => libc::SIGINT,
            "sigterm" => libc::SIGTERM,
            "sighup" => libc::SIGHUP,
            other => panic!("unsupported native test signal: {other}"),
        };
        // SAFETY: raise delivers a known signal constant to this subprocess;
        // shutdown_signal installed the matching Tokio handler before the
        // extraction future sends its ready notification.
        assert_eq!(unsafe { libc::raise(signal) }, 0);
    }

    #[cfg(windows)]
    fn raise_native_shutdown_signal(kind: &str) {
        use windows_sys::Win32::System::Console::{CTRL_C_EVENT, GenerateConsoleCtrlEvent};

        assert_eq!(kind, "ctrl-c");
        // SAFETY: the parent launches this helper with CREATE_NEW_CONSOLE, so
        // group zero targets only this subprocess's console. Tokio's Ctrl+C
        // handler is installed before the extraction future signals ready.
        assert_ne!(unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) }, 0);
    }

    #[tokio::test]
    #[ignore = "native signal subprocess helper"]
    async fn signal_shutdown_native_subprocess() {
        let kind = std::env::var("WCORE_TEST_SHUTDOWN_SIGNAL")
            .expect("native signal kind supplied by parent test");
        let extracted_root = Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let session = pending_bundled_reference_session(extracted_root.clone(), ready_tx);
        let raised = kind.clone();
        let trigger = tokio::spawn(async move {
            ready_rx
                .await
                .expect("reference extraction reaches signal point");
            tokio::task::yield_now().await;
            raise_native_shutdown_signal(&raised);
        });

        let cleanup = BundledSkillTmpCleanup;
        let status = run_until_shutdown(session, shutdown_signal())
            .await
            .expect("native signal shutdown must complete cleanly");
        trigger.await.expect("native signal trigger task");
        // B3: a signalled shutdown reports 128+signal, not SUCCESS. This
        // assertion previously demanded `ExitCode::SUCCESS`, which is what let
        // `kill -INT` mid-run look identical to a completed one.
        let expected = match kind.as_str() {
            "sigint" | "ctrl-c" => ShutdownSignal::Interrupt,
            "sigterm" => ShutdownSignal::Terminate,
            "sighup" => ShutdownSignal::Hangup,
            other => panic!("unsupported native test signal: {other}"),
        };
        assert_eq!(status, ExitCode::from(expected.exit_code()));
        let process_root = extracted_root
            .lock()
            .expect("read extraction root")
            .clone()
            .expect("subprocess records extraction root");
        assert!(process_root.exists());
        drop(cleanup);
        assert!(
            !process_root.exists(),
            "native signal shutdown must remove the exact UUID root"
        );
    }

    #[tokio::test]
    async fn native_shutdown_signals_remove_exact_bundled_root() {
        #[cfg(unix)]
        let signals = ["sigint", "sigterm", "sighup"];
        #[cfg(windows)]
        let signals = ["ctrl-c"];

        let current_exe = std::env::current_exe().expect("current test executable");
        let current_exe = current_exe.to_string_lossy().into_owned();
        for signal in signals {
            let mut child = wcore_config::shell::shell_command_argv(
                &current_exe,
                &[
                    "--exact",
                    "tests::signal_shutdown_native_subprocess",
                    "--ignored",
                    "--nocapture",
                ],
            );
            child.kill_on_drop(true);
            child.env("WCORE_TEST_SHUTDOWN_SIGNAL", signal);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt as _;
                use windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE;

                child.as_std_mut().creation_flags(CREATE_NEW_CONSOLE);
            }
            let output = tokio::time::timeout(std::time::Duration::from_secs(60), child.output())
                .await
                .unwrap_or_else(|_| panic!("native {signal} cleanup subprocess timed out"))
                .expect("run native signal subprocess");
            assert!(
                output.status.success(),
                "native {signal} cleanup subprocess failed; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn json_stream_guard_blocks_profile_intent_without_home() {
        // Host passed --profile but no WAYLAND_HOME materialized → refuse.
        let err = json_stream_profile_guard(true, Some("work"), false)
            .expect_err("must refuse profile intent without WAYLAND_HOME in json-stream");
        assert!(
            err.contains("WAYLAND_HOME"),
            "message must name the fix: {err}"
        );
    }

    #[test]
    fn json_stream_guard_allows_when_home_set() {
        // The host correctly set WAYLAND_HOME per spawn → proceed.
        assert!(json_stream_profile_guard(true, Some("work"), true).is_ok());
    }

    #[test]
    fn json_stream_guard_allows_without_profile_intent() {
        // No --profile → default home is the legacy single-home contract.
        assert!(json_stream_profile_guard(true, None, false).is_ok());
    }

    #[test]
    fn json_stream_guard_is_noop_outside_json_stream() {
        // Interactive CLI/TUI tolerates profile fall-through; only the host
        // protocol is strict. Same inputs that block above must pass here.
        assert!(json_stream_profile_guard(false, Some("work"), false).is_ok());
    }
    use wcore_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolDef};
    use wcore_mcp::transport::{McpError, McpTransport};

    /// No-op transport stub for test-only McpManager construction.
    /// W6 B.7: we never call into the transport because the helper
    /// under test (`mcp_ready_events_for`) only reads pre-discovered
    /// tools — no JSON-RPC traffic is involved.
    /// A server whose advertised tool list grows once, announced by raising
    /// the `tools/list_changed` flag the real stdio reader raises off the
    /// wire. wayland#1174 / #1175.
    struct GrowingTestTransport {
        tools: std::sync::Mutex<Vec<String>>,
        tools_changed: std::sync::atomic::AtomicBool,
        /// wayland#1234 — model the `CleanupUnverified` arm: `close()` fails,
        /// so the removal path cannot prove the transport dead, and the
        /// transport goes on answering `tools/list` and announcing changes.
        close_fails: bool,
    }

    impl GrowingTestTransport {
        fn new(initial: &[&str]) -> Self {
            Self {
                tools: std::sync::Mutex::new(initial.iter().map(|n| n.to_string()).collect()),
                tools_changed: std::sync::atomic::AtomicBool::new(false),
                close_fails: false,
            }
        }

        /// A transport whose close cannot be verified — the arm on which a
        /// live server is MOST likely, and the one the withdrawal used to skip.
        fn new_refusing_close(initial: &[&str]) -> Self {
            Self {
                close_fails: true,
                ..Self::new(initial)
            }
        }

        fn register_and_announce(&self, name: &str) {
            self.tools.lock().unwrap().push(name.to_string());
            self.tools_changed
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl McpTransport for GrowingTestTransport {
        async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            let tools: Vec<Value> = self
                .tools
                .lock()
                .unwrap()
                .iter()
                .map(|name| json!({"name": name, "description": name, "inputSchema": {}}))
                .collect();
            Ok(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(json!({ "tools": tools })),
                error: None,
            })
        }
        async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
            Ok(())
        }
        async fn close(&self) -> Result<(), McpError> {
            if self.close_fails {
                return Err(McpError::Transport(
                    "transport refused to close (test fixture)".to_string(),
                ));
            }
            Ok(())
        }
        fn take_tools_changed(&self) -> bool {
            self.tools_changed
                .swap(false, std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Lets the test keep observing the fixture the manager owns.
    struct SharedTransport(Arc<GrowingTestTransport>);

    #[async_trait]
    impl McpTransport for SharedTransport {
        async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            self.0.request(req).await
        }
        async fn notify(&self, req: &JsonRpcRequest) -> Result<(), McpError> {
            self.0.notify(req).await
        }
        async fn close(&self) -> Result<(), McpError> {
            self.0.close().await
        }
        fn take_tools_changed(&self) -> bool {
            self.0.take_tools_changed()
        }
    }

    struct NoopTransport;

    #[async_trait]
    impl McpTransport for NoopTransport {
        async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
            Ok(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: Some(1),
                result: Some(json!(null)),
                error: None,
            })
        }
        async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
            Ok(())
        }
        async fn close(&self) -> Result<(), McpError> {
            Ok(())
        }
    }

    fn tool(name: &str) -> McpToolDef {
        let json_value: Value = serde_json::from_str(&format!(
            r#"{{"name":"{name}","description":null,"inputSchema":{{}}}}"#
        ))
        .unwrap();
        serde_json::from_value(json_value).unwrap()
    }

    /// W6 B.7 regression: boot-time McpReady emission must produce one
    /// event per connected server with the server's discovered tools.
    /// Pre-fix the boot path emitted nothing — only the dynamic
    /// AddMcpServer path emitted. Hosts running Gemini-routed sessions
    /// with MCP servers in the user's wayland config saw no MCP
    /// health, breaking the MCP-server status UI for that path.
    #[test]
    fn test_mcp_ready_events_for_emits_one_per_server_with_tools() {
        let mgr = Arc::new(McpManager::new_for_test_with_tools(vec![
            (
                "fs",
                false,
                Box::new(NoopTransport) as Box<dyn McpTransport>,
                vec![tool("read_file"), tool("write_file")],
            ),
            (
                "search",
                false,
                Box::new(NoopTransport) as Box<dyn McpTransport>,
                vec![tool("grep")],
            ),
        ]));
        let mut registry = wcore_tools::registry::ToolRegistry::new();
        wcore_mcp::tool_proxy::register_mcp_tools(
            &mut registry,
            &mgr,
            &[],
            &HashMap::new(),
            &wcore_config::tools::DeferColdConfig::default(),
        );

        let events = mcp_ready_events_for(&mgr, &registry);
        assert_eq!(events.len(), 2, "expected one McpReady per server");

        // Helper sorts servers by name, so order is deterministic: fs, search.
        match &events[0] {
            ProtocolEvent::McpReady { name, tools, .. } => {
                assert_eq!(name, "fs");
                let mut sorted = tools.clone();
                sorted.sort();
                assert_eq!(sorted, vec!["read_file".to_string(), "write_file".into()]);
            }
            other => panic!("expected McpReady, got {other:?}"),
        }
        match &events[1] {
            ProtocolEvent::McpReady { name, tools, .. } => {
                assert_eq!(name, "search");
                assert_eq!(tools, &vec!["grep".to_string()]);
            }
            other => panic!("expected McpReady, got {other:?}"),
        }
    }

    /// Empty manager (no MCP servers configured) must produce no events.
    /// Guards against accidental `McpReady` spam when MCP is disabled.
    #[test]
    fn test_mcp_ready_events_for_empty_manager_emits_nothing() {
        let mgr = McpManager::new_for_test_with_tools(vec![]);
        let registry = wcore_tools::registry::ToolRegistry::new();
        let events = mcp_ready_events_for(&mgr, &registry);
        assert!(events.is_empty(), "no MCP servers => no McpReady events");
    }

    /// Server with no discovered tools still produces an `McpReady` with
    /// an empty tools list — matches the dynamic `AddMcpServer` path,
    /// which always emits one event per successfully-connected server
    /// regardless of tool count. Hosts use the event itself as the
    /// "server connected" signal; the empty `tools` array just means
    /// the server exposed no tools.
    #[test]
    fn test_mcp_ready_events_for_server_with_no_tools_still_emits() {
        let mgr = McpManager::new_for_test_with_tools(vec![(
            "introspect",
            false,
            Box::new(NoopTransport) as Box<dyn McpTransport>,
            vec![],
        )]);
        let registry = wcore_tools::registry::ToolRegistry::new();
        let events = mcp_ready_events_for(&mgr, &registry);
        assert_eq!(events.len(), 1, "tool-less servers must still emit");
        match &events[0] {
            ProtocolEvent::McpReady { name, tools, .. } => {
                assert_eq!(name, "introspect");
                assert!(tools.is_empty());
            }
            other => panic!("expected McpReady, got {other:?}"),
        }
    }

    /// wayland#551: background-connect failure surfacing — one `McpFailed`
    /// per Failed/TimedOut/Skipped server, none for Ready, name-sorted.
    /// Pre-fix the deferred path would have reported connect failures
    /// nowhere (bootstrap's inline emit no longer runs for these servers).
    #[test]
    fn test_mcp_failed_events_for_reports_failures_only() {
        use wcore_mcp::manager::McpServerHealth;
        let mgr = McpManager::new_for_test_with_health(vec![
            (
                "slow",
                McpServerHealth::TimedOut {
                    after: std::time::Duration::from_secs(30),
                    cleanup_error: None,
                },
            ),
            ("okay", McpServerHealth::Ready { tool_count: 3 }),
            (
                "broken",
                McpServerHealth::Failed {
                    reason: "handshake refused".into(),
                },
            ),
        ]);
        let events = mcp_failed_events_for(&mgr);
        assert_eq!(events.len(), 2, "Ready servers must not emit McpFailed");
        match &events[0] {
            ProtocolEvent::McpFailed { name, reason } => {
                assert_eq!(name, "broken");
                assert!(reason.contains("handshake refused"), "reason = {reason}");
            }
            other => panic!("expected McpFailed, got {other:?}"),
        }
        match &events[1] {
            ProtocolEvent::McpFailed { name, reason } => {
                assert_eq!(name, "slow");
                assert!(reason.contains("30"), "reason = {reason}");
            }
            other => panic!("expected McpFailed, got {other:?}"),
        }
    }

    /// wayland#551/#562: deferred-MCP integration must register the manager's
    /// tools into a LIVE engine (post-boot), refresh the real registered
    /// ToolSearch catalog, emit per-server events, and park the manager alive
    /// in `dynamic_managers`. Merely adding the proxy to the registry is not
    /// enough: ToolSearch snapshots the catalog during bootstrap, so a late
    /// proxy is otherwise callable by name but undiscoverable to the model.
    #[tokio::test]
    async fn integrate_deferred_mcp_registers_tools_into_live_engine() {
        let config = wcore_config::config::Config::default();
        let defer_cold = config.builtin_tools.defer_cold.clone();
        let (mut engine, _sink) =
            wcore_agent::bootstrap::AgentBootstrap::build_for_test(config, vec![]);
        // `build_for_test` deliberately omits ToolSearch; seed it through the
        // same live-registry helper production bootstrap uses.
        engine
            .registry_mut()
            .expect("idle fixture registry must be mutable")
            .refresh_tool_search_catalog(&defer_cold);
        let before = engine.tool_names().len();
        let mgr = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "quick",
            false,
            Box::new(NoopTransport) as Box<dyn McpTransport>,
            vec![tool("quick_echo")],
        )]));
        let writer = ProtocolWriter::new();
        let mut dynamic_managers = Vec::new();
        // Mark the server itself non-deferred to prove the refresh reapplies
        // the global cold policy before ToolSearch snapshots the live tools.
        let resolved = HashMap::from([(
            "quick".to_string(),
            to_mcp_server_config(
                "stdio",
                Some("unused-test-command".to_string()),
                None,
                None,
                None,
                None,
                false,
                None,
            )
            .expect("valid test server config"),
        )]);
        let mut reservations = lifecycle_reservations(&resolved);
        assert!(
            integrate_deferred_mcp(
                &mut engine,
                mgr,
                &resolved,
                &mut reservations,
                &writer,
                &mut dynamic_managers,
                &mut inert_late_binder(),
                &mut Vec::new(),
            ),
            "integration must succeed on an idle engine"
        );
        assert!(
            engine.tool_names().len() > before
                && engine.tool_names().iter().any(|n| n.contains("quick_echo")),
            "deferred server's tools must be registered; got {:?}",
            engine.tool_names()
        );
        let registry = engine.tools();
        let search = registry
            .get("ToolSearch")
            .expect("bootstrap must register the real ToolSearch tool");
        let result = search.execute(json!({"query": "quick_echo"})).await;
        assert!(
            result.content.contains("\"name\": \"quick_echo\"")
                && result.content.contains("\"parameters\""),
            "late MCP tool must be discoverable through the registered ToolSearch; got {}",
            result.content
        );
        drop(registry);

        // A second live refresh replaces rather than duplicates ToolSearch,
        // and the catalog snapshot must never index ToolSearch itself.
        engine
            .registry_mut()
            .expect("registry must be mutable after dropping the read handle")
            .refresh_tool_search_catalog(&defer_cold);
        assert_eq!(
            engine
                .tool_names()
                .iter()
                .filter(|name| name.as_str() == "ToolSearch")
                .count(),
            1,
            "repeated refresh must leave exactly one registered ToolSearch"
        );
        let registry = engine.tools();
        let self_search = registry
            .get("ToolSearch")
            .expect("ToolSearch must survive repeated refresh")
            .execute(json!({"query": "ToolSearch"}))
            .await;
        assert!(
            !self_search.content.contains("\"name\": \"ToolSearch\""),
            "ToolSearch must not index or hydrate itself; got {}",
            self_search.content
        );
        assert_eq!(dynamic_managers.len(), 1, "manager must be kept alive");
    }

    /// wayland#562 — WIRING gate. `LateMcpBinder` being correct in isolation
    /// is not enough: `integrate_deferred_mcp` is the single site the deferred
    /// config-MCP path funnels through, and it must actually CALL the binder.
    /// The sibling tests above pass an inert binder, so none of them can
    /// notice the `late_mcp.bind(..)` call being deleted; this one can.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: delete the `late_mcp.bind(..)`
    /// call from `integrate_deferred_mcp` and the assertions below fail by
    /// name ("never reached the live catalog", "never rebound the plugin hook
    /// dispatcher") rather than by a compile error.
    #[tokio::test]
    async fn integrate_deferred_mcp_late_binds_skills_and_hooks() {
        let config = wcore_config::config::Config::default();
        let defer_cold = config.builtin_tools.defer_cold.clone();
        let (mut engine, _sink) =
            wcore_agent::bootstrap::AgentBootstrap::build_for_test(config, vec![]);
        engine
            .registry_mut()
            .expect("idle fixture registry must be mutable")
            .refresh_tool_search_catalog(&defer_cold);

        // Deferral's boot state: an empty catalog, a plugin hook that resolved
        // to nothing, and no MCP manager at all.
        let catalog = Arc::new(wcore_skills::refs::SkillCatalog::from_refs(Vec::new()));
        engine.set_skill_catalog(Arc::clone(&catalog));
        let hooks = vec![wcore_agent::plugins::runner::PluginHook {
            plugin: "demo-plugin".to_string(),
            phase: wcore_plugin_api::registry::hooks::HookPhase::SessionStart,
            name: "demo_contribution".to_string(),
        }];
        engine.register_plugin_hooks(hooks.clone());
        assert!(
            !engine
                .hook_engine()
                .expect("bootstrap installs a HookEngine")
                .has_dispatcher(),
            "precondition: with config MCP deferred, boot binds no dispatcher"
        );
        let mut late_mcp = LateMcpBinder::new(Arc::clone(&catalog), &hooks, Vec::new(), true);

        // The deferred server: advertises the plugin's hook tool, and (via the
        // refs the async half already read off it) serves one skill.
        let mgr = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "late-srv",
            false,
            Box::new(NoopTransport) as Box<dyn McpTransport>,
            vec![tool("demo_contribution")],
        )]));
        let mut skill_refs = vec![SkillRef {
            name: "late-srv:remote-helper".to_string(),
            display_name: None,
            description: "RESOURCE_SERVED_SKILL".to_string(),
            when_to_use: None,
            paths: Vec::new(),
            source: wcore_skills::types::SkillSource::Project,
            loaded_from: wcore_skills::types::LoadedFrom::Skills,
            file_path: std::path::PathBuf::from("<mcp:late-srv>"),
            skill_root: None,
            content_length_hint: 0,
            user_invocable: true,
            disable_model_invocation: false,
            has_artifacts: false,
            inline_content: Some(
                "---\nname: remote-helper\ndescription: RESOURCE_SERVED_SKILL\n---\nbody\n"
                    .to_string(),
            ),
        }];

        let resolved = HashMap::from([(
            "late-srv".to_string(),
            to_mcp_server_config(
                "stdio",
                Some("unused-test-command".to_string()),
                None,
                None,
                None,
                None,
                false,
                None,
            )
            .expect("valid test server config"),
        )]);
        let mut reservations = lifecycle_reservations(&resolved);
        let writer = ProtocolWriter::new();
        let mut dynamic_managers = Vec::new();
        assert!(
            integrate_deferred_mcp(
                &mut engine,
                mgr,
                &resolved,
                &mut reservations,
                &writer,
                &mut dynamic_managers,
                &mut late_mcp,
                &mut skill_refs,
            ),
            "integration must succeed on an idle engine"
        );

        // Gap 1 at the call site: the skill reached the SHARED catalog Arc and
        // the model was told about it.
        assert!(
            catalog.find("late-srv:remote-helper").is_some(),
            "the deferred server's skill never reached the live catalog through \
             integrate_deferred_mcp; catalog = {:?}",
            catalog.visible_names()
        );
        assert!(
            engine.system_prompt().contains("RESOURCE_SERVED_SKILL"),
            "integrate_deferred_mcp never told the model about the late skill"
        );
        assert!(
            skill_refs.is_empty(),
            "the refs must be consumed, not left to be merged twice on a retry"
        );

        // Gap 2 at the call site: the plugin hook dispatcher was rebound over
        // the widened manager set.
        assert!(
            engine.hook_engine().expect("HookEngine").has_dispatcher(),
            "integrate_deferred_mcp never rebound the plugin hook dispatcher"
        );

        // The already-closed tool-discovery defects must stay closed on this
        // path: the late tool is still registered AND ToolSearch-discoverable.
        let registry = engine.tools();
        let result = registry
            .get("ToolSearch")
            .expect("ToolSearch must be registered")
            .execute(json!({"query": "demo_contribution"}))
            .await;
        assert!(
            result.content.contains("demo_contribution"),
            "late-binding must not disturb late tool discovery; got {}",
            result.content
        );
    }

    /// #562: `ToolSearch` is a reserved built-in name before boot MCP proxy
    /// delivery and is refreshed after live additions. A server exporting that
    /// literal name must remain callable under the deterministic MCP namespace,
    /// while host health reports the same display name the catalog exposes.
    #[tokio::test]
    async fn literal_tool_search_mcp_is_namespaced_preserved_and_health_aligned() {
        let config = wcore_config::config::Config::default();
        let defer_cold = config.builtin_tools.defer_cold.clone();
        let (mut engine, _sink) =
            wcore_agent::bootstrap::AgentBootstrap::build_for_test(config, vec![]);
        engine
            .registry_mut()
            .expect("idle fixture registry must be mutable")
            .refresh_tool_search_catalog(&defer_cold);

        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "collision",
            false,
            Box::new(NoopTransport) as Box<dyn McpTransport>,
            vec![tool("ToolSearch")],
        )]));
        let resolved = HashMap::from([(
            "collision".to_string(),
            to_mcp_server_config(
                "stdio",
                Some("unused-test-command".to_string()),
                None,
                None,
                None,
                None,
                false,
                None,
            )
            .expect("valid test server config"),
        )]);
        let writer = ProtocolWriter::new();
        let mut dynamic_managers = Vec::new();
        let mut reservations = lifecycle_reservations(&resolved);

        assert!(integrate_deferred_mcp(
            &mut engine,
            manager.clone(),
            &resolved,
            &mut reservations,
            &writer,
            &mut dynamic_managers,
            &mut inert_late_binder(),
            &mut Vec::new(),
        ));

        let registry = engine.tools();
        let names = registry.tool_names();
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == "ToolSearch")
                .count(),
            1,
            "the built-in ToolSearch must remain singular"
        );
        assert!(
            names
                .iter()
                .any(|name| name == "mcp__collision__ToolSearch"),
            "the colliding MCP proxy must be preserved under its namespace: {names:?}"
        );
        let catalog_result = registry
            .get("ToolSearch")
            .expect("built-in ToolSearch must remain installed")
            .execute(json!({"query": "mcp__collision__ToolSearch"}))
            .await;
        assert!(
            catalog_result
                .content
                .contains("\"name\": \"mcp__collision__ToolSearch\""),
            "built-in catalog must expose the preserved proxy: {}",
            catalog_result.content
        );

        let events = mcp_ready_events_for(&manager, &registry);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProtocolEvent::McpReady { name, tools, .. } => {
                assert_eq!(name, "collision");
                assert_eq!(tools, &["mcp__collision__ToolSearch".to_string()]);
            }
            other => panic!("expected McpReady, got {other:?}"),
        }
        assert_eq!(dynamic_managers.len(), 1, "manager must remain alive");
    }

    /// The dial notice is ON by default, and exactly one surface waives it.
    ///
    /// This is a COUNT, not an opinion: the whole failure mode being closed is
    /// a wait that some surface takes silently, so the thing worth pinning is
    /// how many surfaces are allowed to. Walks the crate's real sources rather
    /// than one file, because a waiver added in `acp_engine.rs` or `tui/` would
    /// be exactly the drift this exists to catch and a single-file gate would
    /// never see it.
    #[test]
    fn the_mcp_dial_notice_is_waived_only_where_a_splash_already_covers_it() {
        fn rust_sources(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
            for entry in std::fs::read_dir(dir).expect("crate sources must be readable") {
                let path = entry.expect("readable dir entry").path();
                if path.is_dir() {
                    rust_sources(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).expect("readable source");
                    // Cut the test modules off. THIS test quotes the waiver
                    // string three times, so a gate that searched whole files
                    // would report main.rs as a waiver site whether or not the
                    // production line still existed — it would be matching its
                    // own text and could never fail in the deleted direction.
                    let production = match text.find("\n#[cfg(test)]\n") {
                        Some(at) => text[..at].to_string(),
                        None => text,
                    };
                    out.push((path, production));
                }
            }
        }
        let mut sources = Vec::new();
        rust_sources(
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
            &mut sources,
        );

        // Known-positive control: prove the walk actually read this crate,
        // so "zero waivers" can never be produced by an empty scan.
        let bootstraps: usize = sources
            .iter()
            .map(|(_, text)| text.matches("AgentBootstrap::new").count())
            .sum();
        assert!(
            bootstraps >= 3,
            "the source walk found only {bootstraps} AgentBootstrap::new sites — it is not \
             reading this crate, so every count below it is meaningless"
        );

        let waivers: Vec<&std::path::Path> = sources
            .iter()
            .filter(|(_, text)| text.contains(".without_mcp_dial_notice(true)"))
            .map(|(path, _)| path.as_path())
            .collect();
        assert_eq!(
            waivers.len(),
            1,
            "exactly one surface may take the MCP dial silently — the TUI, whose splash covers \
             this window. Waivers found in: {waivers:?}"
        );

        let (path, text) = sources
            .iter()
            .find(|(_, text)| text.contains(".without_mcp_dial_notice(true)"))
            .expect("asserted present above");
        assert!(
            path.ends_with("main.rs"),
            "the one waiver moved out of main.rs, to {path:?}"
        );
        let waiver = text
            .find(".without_mcp_dial_notice(true)")
            .expect("asserted present above");
        let enclosing = text[..waiver]
            .rfind("async fn ")
            .expect("the waiver must sit inside a function");
        assert!(
            text[enclosing..].starts_with("async fn run_tui_mode("),
            "the waiver must stay in the splash-covered TUI entry point, found it in {:?}",
            &text[enclosing..enclosing + 40]
        );
    }

    /// A turn held open by the MCP dial must SAY it is being held open.
    ///
    /// Measured on 0.13.8 with one stdio server that never speaks
    /// (`command = "sleep"`): the host got NOTHING for 30.3 s — no event of
    /// any kind — then `mcp_failed`, then `stream_start`. Identical on
    /// 0.13.7, so this is not a regression; it is a hole in what the turn
    /// discloses about itself, and thirty seconds of a blank turn reads as a
    /// dead app whatever caused it.
    ///
    /// Grades the real call site (`settle_deferred_mcp_before_message`, the
    /// session loop's readiness boundary), not the notice helper in
    /// isolation. Deleting the `sleep` arm from the `select!` inside
    /// `await_deferred_mcp_connect` turns this red.
    ///
    /// `start_paused` so the clock is driven by the runtime rather than by
    /// the wall: the fixture's dial settles one second short of the real
    /// per-server deadline, which is the window the notice exists to fill,
    /// and the whole test still runs in milliseconds.
    #[tokio::test(start_paused = true)]
    async fn a_slow_mcp_dial_announces_itself_instead_of_showing_a_blank_turn() {
        let config = wcore_config::config::Config::default();
        let defer_cold = config.builtin_tools.defer_cold.clone();
        let (mut engine, _engine_sink) =
            wcore_agent::bootstrap::AgentBootstrap::build_for_test(config, vec![]);
        engine
            .registry_mut()
            .expect("idle fixture registry must be mutable")
            .refresh_tool_search_catalog(&defer_cold);

        let resolved = HashMap::from([(
            "slow".to_string(),
            to_mcp_server_config(
                "stdio",
                Some("unused-test-command".to_string()),
                None,
                None,
                None,
                None,
                false,
                None,
            )
            .expect("valid test server config"),
        )]);
        let reservations = lifecycle_reservations(&resolved);
        let manager = McpManager::new_for_test_with_tools(vec![(
            "slow",
            false,
            Box::new(NoopTransport) as Box<dyn McpTransport>,
            vec![tool("slow_after_dial")],
        )]);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            tokio::time::sleep(wcore_mcp::manager::CONNECT_TIMEOUT - Duration::from_secs(1)).await;
            let _ = tx.send(DeferredMcpConnectResult {
                outcome: Ok(manager),
                resolved,
                reservations,
            });
        });

        let sink = wcore_agent::test_utils::TestSink::new();
        let events = sink.handle();
        let output: Arc<dyn OutputSink> = Arc::new(sink);
        let writer = ProtocolWriter::new();
        let mut deferred_mcp_rx = Some(rx);
        let mut pending_deferred_mcp = None;
        let mut dynamic_managers = Vec::new();
        let mut late_mcp = inert_late_binder();

        let ready = settle_deferred_mcp_before_message(
            &mut deferred_mcp_rx,
            &mut pending_deferred_mcp,
            &mut engine,
            &writer,
            &output,
            &mut dynamic_managers,
            None,
            &mut late_mcp,
        )
        .await;

        assert!(
            ready,
            "the notice must not change the outcome — the dial still settles"
        );
        let notices: Vec<String> = events
            .snapshot()
            .iter()
            .filter(|e| e["type"].as_str() == Some("info"))
            .filter_map(|e| e["message"].as_str().map(str::to_string))
            .filter(|m| m.contains("Still waiting on MCP servers"))
            .collect();
        assert_eq!(
            notices.len(),
            1,
            "a host must be told once that the turn is waiting on MCP, got {notices:?}"
        );
        assert!(
            notices[0].contains(&format!(
                "{}s",
                wcore_mcp::manager::CONNECT_TIMEOUT.as_secs()
            )),
            "the notice must name the deadline it is counting towards, got {:?}",
            notices[0]
        );
        assert!(
            engine
                .tools()
                .to_tool_defs()
                .iter()
                .any(|def| def.name == "slow_after_dial"),
            "the settled dial must still have registered its tools"
        );
    }

    /// #562 structural ordering regression: execute the same readiness seam
    /// used by the session loop across `InitHistory -> Message`. Setup remains
    /// immediate; the delayed manager becomes provider-visible exactly at the
    /// Message boundary.
    #[tokio::test]
    async fn session_readiness_preserves_init_history_then_settles_mcp_for_message() {
        let config = wcore_config::config::Config::default();
        let defer_cold = config.builtin_tools.defer_cold.clone();
        let (mut engine, _sink) =
            wcore_agent::bootstrap::AgentBootstrap::build_for_test(config, vec![]);
        engine
            .registry_mut()
            .expect("idle fixture registry must be mutable")
            .refresh_tool_search_catalog(&defer_cold);

        let resolved = HashMap::from([(
            "delayed".to_string(),
            to_mcp_server_config(
                "stdio",
                Some("unused-test-command".to_string()),
                None,
                None,
                None,
                None,
                false,
                None,
            )
            .expect("valid test server config"),
        )]);
        let reservations = lifecycle_reservations(&resolved);
        let manager = McpManager::new_for_test_with_tools(vec![(
            "delayed",
            false,
            Box::new(NoopTransport) as Box<dyn McpTransport>,
            vec![tool("late_after_init")],
        )]);
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = tx.send(DeferredMcpConnectResult {
                outcome: Ok(manager),
                resolved,
                reservations,
            });
        });

        let writer = ProtocolWriter::new();
        let output: Arc<dyn OutputSink> = Arc::new(wcore_agent::output::null_sink::NullSink);
        let mut deferred_mcp_rx = Some(rx);
        let mut pending_deferred_mcp = None;
        let mut dynamic_managers = Vec::new();

        let commands = [
            ProtocolCommand::InitHistory {
                text: "desktop init-history sentinel".to_string(),
            },
            ProtocolCommand::Message {
                msg_id: "first-message".to_string(),
                content: "use the delayed tool".to_string(),
                files: Vec::new(),
            },
        ];
        let mut readiness_trace = Vec::new();
        let mut init_history_applied = false;
        let mut late_mcp = inert_late_binder();

        for command in commands {
            let readiness = session_command_readiness(&command);
            readiness_trace.push(readiness);
            if readiness == SessionCommandReadiness::SettleDeferredMcp {
                let ready = tokio::time::timeout(
                    Duration::from_secs(1),
                    settle_deferred_mcp_before_message(
                        &mut deferred_mcp_rx,
                        &mut pending_deferred_mcp,
                        &mut engine,
                        &writer,
                        &output,
                        &mut dynamic_managers,
                        None,
                        &mut late_mcp,
                    ),
                )
                .await
                .expect("the message readiness boundary must remain bounded");
                assert!(ready, "an idle registry must be ready before Message");
            }

            match command {
                ProtocolCommand::InitHistory { text } => {
                    engine.inject_history(text);
                    init_history_applied = true;
                    assert!(
                        deferred_mcp_rx.is_some(),
                        "setup commands must not wait for configured MCP"
                    );
                    assert!(
                        !engine
                            .tools()
                            .to_tool_defs()
                            .iter()
                            .any(|def| def.name == "late_after_init"),
                        "delayed tool must remain absent before the Message boundary"
                    );
                }
                ProtocolCommand::Message { .. } => {
                    assert!(
                        init_history_applied,
                        "InitHistory must execute before the immediate Message"
                    );
                    assert!(
                        engine
                            .tools()
                            .to_tool_defs()
                            .iter()
                            .any(|def| def.name == "late_after_init"),
                        "provider-visible registry must be ready before Message processing"
                    );
                }
                _ => unreachable!("fixture contains only InitHistory and Message"),
            }
        }

        assert_eq!(
            readiness_trace,
            [
                SessionCommandReadiness::Immediate,
                SessionCommandReadiness::SettleDeferredMcp,
            ],
            "only Message may cross the configured-MCP readiness boundary"
        );

        assert!(deferred_mcp_rx.is_none());
        assert!(pending_deferred_mcp.is_none());
        assert_eq!(dynamic_managers.len(), 1, "manager must be kept alive");
        assert!(
            engine
                .tools()
                .to_tool_defs()
                .iter()
                .any(|def| def.name == "late_after_init"),
            "provider-visible tool registry must include the delayed MCP tool"
        );
        let registry = engine.tools();
        let result = registry
            .get("ToolSearch")
            .expect("ToolSearch must remain registered")
            .execute(json!({"query": "late_after_init"}))
            .await;
        assert!(
            result.content.contains("\"name\": \"late_after_init\""),
            "the delayed tool must be discoverable before the provider turn; got {}",
            result.content
        );
    }

    /// wayland#551: while the registry Arc is borrowed (as during a turn),
    /// integration must decline — no partial registration — so the caller
    /// parks the manager and retries at the next between-turns boundary.
    #[tokio::test]
    async fn integrate_deferred_mcp_parks_while_registry_is_borrowed() {
        let (mut engine, _sink) = wcore_agent::bootstrap::AgentBootstrap::build_for_test(
            wcore_config::config::Config::default(),
            vec![],
        );
        let hold = engine.tools(); // second Arc ref → registry_mut() is None
        let mgr = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "quick",
            false,
            Box::new(NoopTransport) as Box<dyn McpTransport>,
            vec![tool("quick_echo")],
        )]));
        let writer = ProtocolWriter::new();
        let mut dynamic_managers = Vec::new();
        let mut reservations = HashMap::new();
        assert!(
            !integrate_deferred_mcp(
                &mut engine,
                mgr,
                &HashMap::new(),
                &mut reservations,
                &writer,
                &mut dynamic_managers,
                &mut inert_late_binder(),
                &mut Vec::new(),
            ),
            "integration must decline while the registry is borrowed"
        );
        assert!(dynamic_managers.is_empty());
        assert!(
            !engine.tool_names().iter().any(|n| n.contains("quick_echo")),
            "no tools may be registered on a declined integration"
        );
        drop(hold);
    }

    /// F17 review regression: the Message boundary must report that it is not
    /// ready while a registry reader is retained. The session loop uses this
    /// result to park and retry the exact command instead of running a turn
    /// with a partial tool manifest.
    #[tokio::test]
    async fn message_readiness_fails_closed_while_registry_is_borrowed() {
        let (mut engine, _sink) = wcore_agent::bootstrap::AgentBootstrap::build_for_test(
            wcore_config::config::Config::default(),
            vec![],
        );
        let hold = engine.tools();
        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "held",
            false,
            Box::new(NoopTransport) as Box<dyn McpTransport>,
            vec![tool("held_echo")],
        )]));
        let mut deferred_mcp_rx = None;
        let mut pending_deferred_mcp = Some(PendingDeferredMcp {
            manager,
            resolved: HashMap::new(),
            reservations: HashMap::new(),
            skill_refs: Vec::new(),
        });
        let writer = ProtocolWriter::new();
        let output: Arc<dyn OutputSink> = Arc::new(wcore_agent::output::null_sink::NullSink);
        let mut dynamic_managers = Vec::new();
        let mut late_mcp = inert_late_binder();

        assert!(
            !settle_deferred_mcp_before_message(
                &mut deferred_mcp_rx,
                &mut pending_deferred_mcp,
                &mut engine,
                &writer,
                &output,
                &mut dynamic_managers,
                None,
                &mut late_mcp,
            )
            .await,
            "a retained registry reader must block the provider boundary"
        );
        assert!(
            pending_deferred_mcp.is_some(),
            "integration must stay parked"
        );
        assert!(dynamic_managers.is_empty());

        drop(hold);
        assert!(
            settle_deferred_mcp_before_message(
                &mut deferred_mcp_rx,
                &mut pending_deferred_mcp,
                &mut engine,
                &writer,
                &output,
                &mut dynamic_managers,
                None,
                &mut late_mcp,
            )
            .await,
            "the parked manager must integrate after the reader is released"
        );
        assert!(pending_deferred_mcp.is_none());
        assert_eq!(dynamic_managers.len(), 1);
    }

    /// Rank 47 regression: `--no-memory` must parse and flip
    /// `config.memory.enabled` to `false`, giving users an accessible way to
    /// run a stateless session. Pre-fix the flag did not exist (only a TODO
    /// in wcore-config), so there was no CLI path to disable memory per-run.
    /// wayland#241: the boot approval posture both entry points seed
    /// through. Config drives the mode; `--force` overrides to Force. The
    /// json-stream path previously skipped the config seed, so a user's
    /// `approval_mode` was ignored — this helper is now the single source
    /// both paths use.
    #[test]
    fn initial_session_mode_honors_config_and_force_override() {
        use wcore_config::config::ApprovalMode;
        use wcore_protocol::commands::SessionMode;

        // Config posture is honored when --force is absent.
        for (mode, expected) in [
            (ApprovalMode::Default, SessionMode::Default),
            (ApprovalMode::AutoEdit, SessionMode::AutoEdit),
            (ApprovalMode::Force, SessionMode::Force),
        ] {
            let config = Config {
                approval_mode: mode,
                ..Default::default()
            };
            assert_eq!(
                approval_policy_to_session(
                    resolve_local_execution(
                        &config,
                        false,
                        false,
                        DEFAULT_DANGEROUS_SESSION_TTL_SECS,
                        false,
                    )
                    .unwrap()
                    .approvals(),
                ),
                expected,
                "config {mode:?} must reach the session"
            );
        }

        // --force overrides every config posture to Force.
        for m in [
            ApprovalMode::Default,
            ApprovalMode::AutoEdit,
            ApprovalMode::Force,
        ] {
            let config = Config {
                approval_mode: m,
                ..Default::default()
            };
            assert_eq!(
                approval_policy_to_session(
                    resolve_local_execution(
                        &config,
                        true,
                        false,
                        DEFAULT_DANGEROUS_SESSION_TTL_SECS,
                        false,
                    )
                    .unwrap()
                    .approvals(),
                ),
                SessionMode::Force,
                "--force must override config {m:?} to Force"
            );
        }

        let legacy = Config {
            tools: wcore_config::config::ToolsConfig {
                auto_approve: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            approval_policy_to_session(
                resolve_local_execution(
                    &legacy,
                    false,
                    false,
                    DEFAULT_DANGEROUS_SESSION_TTL_SECS,
                    false,
                )
                .unwrap()
                .approvals(),
            ),
            SessionMode::Force,
            "legacy auto-approve must converge with the typed policy"
        );
        let view = wcore_cli::tui::config_view_from(&legacy);
        assert_eq!(view.approval, "force");
        assert!(view.tools_auto_approve);
    }

    /// RETAINED FROM BEFORE THE RENAME, deliberately unmodified.
    ///
    /// This is the pre-rename guard. It is kept for two reasons: deleting a
    /// passing test to install a replacement is not a trade this lane is
    /// allowed to make, and it serves as the live demonstration of WHY the
    /// replacement below was needed. It hardcodes the two tier arguments
    /// (`resolve_local_execution(&cfg, /*approval_bypass*/ true,
    /// /*dangerous*/ false, ..)`) instead of deriving them from the parsed
    /// `Cli`, so it stays GREEN through a rewiring that moves `--force` into
    /// tier 2 — the exact privilege escalation this lane exists to prevent.
    /// Measured, not asserted: see the lane summary's known-negative run.
    #[test]
    fn foreign_dangerous_alias_is_approval_only() {
        use clap::Parser as _;
        use wcore_types::execution_policy::SandboxPolicy;

        let cli = Cli::try_parse_from(["wayland-core", "--dangerously-skip-permissions"])
            .expect("compatibility alias must remain accepted");
        assert!(cli.dangerously_skip_permissions);
        assert!(!cli.dangerous);

        let selection = resolve_local_execution(
            &Config::default(),
            true,
            false,
            DEFAULT_DANGEROUS_SESSION_TTL_SECS,
            false,
        )
        .unwrap();
        assert_eq!(selection.approvals(), ApprovalPolicy::Bypass);
        assert_eq!(selection.baseline().sandbox(), SandboxPolicy::Required);
        assert!(selection.dangerous_grant().is_none());
    }

    /// TIER REGRESSION GUARD — the artifact that makes the danger-flag rename
    /// safe forever.
    ///
    /// For every accepted danger spelling, assert WHICH TIER it lands in, and
    /// in particular that the OS sandbox is still required for every tier-1
    /// spelling. If a future edit makes `--force` or `--yolo` an alias of the
    /// tier-2 flag, every existing script and CI job using them silently loses
    /// its OS sandbox on upgrade — a privilege escalation delivered by a
    /// rename, invisible in the caller's own diff.
    ///
    /// This derives BOTH tier arguments from the parsed `Cli` through
    /// `danger_tiers` — the same wiring `run()` uses. The test it replaces
    /// (`foreign_dangerous_alias_is_approval_only`) hardcoded
    /// `resolve_local_execution(&cfg, /*approval_bypass*/ true,
    /// /*dangerous*/ false, ..)`, so rewiring a tier-1 alias into tier 2 would
    /// have left it green.
    #[test]
    fn danger_spellings_never_change_tier() {
        use clap::Parser as _;
        use wcore_types::execution_policy::{
            EffectiveExecutionPolicy, MAX_DANGEROUS_SESSION_TTL_SECS, SandboxPolicy,
        };

        // (argv spelling, does this spelling bypass the OS sandbox?)
        let cases: [(&str, bool); 5] = [
            ("--force", false),
            ("--yolo", false),
            ("--dangerously-skip-permissions", false),
            ("--dangerously-skip-permissions-and-sandbox", true),
            ("--dangerous", true),
        ];

        for (spelling, expect_sandbox_bypass) in cases {
            let cli = Cli::try_parse_from(["wayland-core", spelling])
                .unwrap_or_else(|e| panic!("{spelling} must remain accepted: {e}"));
            let (approval_bypass, dangerous_launch) = danger_tiers(&cli);

            let selection = resolve_local_execution(
                &Config::default(),
                approval_bypass,
                dangerous_launch,
                DEFAULT_DANGEROUS_SESSION_TTL_SECS,
                false,
            )
            .unwrap_or_else(|e| panic!("{spelling} must resolve an execution selection: {e}"));

            // The half both tiers share.
            assert_eq!(
                selection.approvals(),
                ApprovalPolicy::Bypass,
                "{spelling} must bypass approval prompts"
            );

            // The half that must NEVER move for a tier-1 spelling.
            let sandbox_bypassed = selection.dangerous_grant().is_some();
            assert_eq!(
                sandbox_bypassed, expect_sandbox_bypass,
                "{spelling} CHANGED TIER: sandbox_bypassed={sandbox_bypassed}, expected \
                 {expect_sandbox_bypass}. A tier-1 spelling that gains a dangerous grant \
                 strips the OS sandbox from every existing script that uses it."
            );

            // The baseline is Required for BOTH tiers; only a resolver-minted
            // lease may override it. This catches a change that weakened the
            // baseline itself rather than the tier wiring.
            //
            // NOTE this assertion ALONE would be a permanently-green gate:
            // `BaselineExecutionPolicy::smart()` hardcodes `Required`, so it
            // can never redden for a tier change. The authoritative assertion
            // is the EFFECTIVE posture below — the same projection the protocol
            // emits to hosts, and the one that actually moves between tiers.
            assert_eq!(
                selection.baseline().sandbox(),
                SandboxPolicy::Required,
                "{spelling}: the baseline sandbox must stay Required"
            );

            let effective = match selection.dangerous_grant() {
                Some(grant) => EffectiveExecutionPolicy::dangerous(grant),
                None => EffectiveExecutionPolicy::baseline(selection.baseline()),
            };
            let expected_sandbox = if expect_sandbox_bypass {
                SandboxPolicy::Bypass
            } else {
                SandboxPolicy::Required
            };
            assert_eq!(
                effective.sandbox(),
                expected_sandbox,
                "{spelling}: TIER CHANGE. The EFFECTIVE OS-sandbox posture for this \
                 spelling moved. If deliberate, every existing caller of {spelling} \
                 just gained or lost containment — that is a privilege change, not \
                 a rename."
            );

            match selection.dangerous_grant() {
                Some(grant) => {
                    assert!(
                        expect_sandbox_bypass,
                        "{spelling}: a tier-1 spelling must not mint a lease"
                    );
                    assert!(
                        grant.ttl_millis() > 0
                            && grant.ttl_millis() <= MAX_DANGEROUS_SESSION_TTL_SECS * 1_000,
                        "{spelling}: lease must be bounded within the one-hour cap, got {}ms",
                        grant.ttl_millis()
                    );
                }
                None => assert!(
                    !expect_sandbox_bypass,
                    "{spelling}: a tier-2 spelling must mint a bounded lease"
                ),
            }
        }
    }

    /// The two tiers are a superset relationship, not a stack: asking for both
    /// at once is a parse error in every spelling combination.
    #[test]
    fn the_two_tiers_refuse_to_stack_in_every_spelling() {
        use clap::Parser as _;

        for tier1 in ["--force", "--yolo", "--dangerously-skip-permissions"] {
            for tier2 in ["--dangerously-skip-permissions-and-sandbox", "--dangerous"] {
                assert!(
                    Cli::try_parse_from(["wayland-core", tier1, tier2]).is_err(),
                    "{tier1} {tier2} must be refused: the tiers do not stack"
                );
                // Both orders — a one-sided `conflicts_with` that only fired
                // in one direction would pass the check above.
                assert!(
                    Cli::try_parse_from(["wayland-core", tier2, tier1]).is_err(),
                    "{tier2} {tier1} must be refused too (reversed order)"
                );
            }
            // CONTROL IN THE PASS DIRECTION. Without this, every assertion
            // above would also be satisfied by a spelling that simply does not
            // parse at all — a gate that cannot pass proves as little as one
            // that cannot fail.
            assert!(
                Cli::try_parse_from(["wayland-core", tier1]).is_ok(),
                "{tier1} alone must still parse"
            );
        }
        for tier2 in ["--dangerously-skip-permissions-and-sandbox", "--dangerous"] {
            assert!(
                Cli::try_parse_from(["wayland-core", tier2]).is_ok(),
                "{tier2} alone must still parse"
            );
        }
    }

    /// `--auto-approve` is deliberately NOT a tier-1 alias, and this locks the
    /// measured reason in: it is the CLI face of the `[tools] auto_approve`
    /// config key, it has no conflict relationship with the danger flags, and
    /// `--dangerous --auto-approve` parses today. Aliasing it would start
    /// rejecting an invocation that works, which is the same class of silent
    /// behaviour change this module's tier guard exists to prevent.
    #[test]
    fn auto_approve_is_not_a_danger_tier_alias() {
        use clap::Parser as _;

        let cli = Cli::try_parse_from([
            "wayland-core",
            "--dangerously-skip-permissions-and-sandbox",
            "--auto-approve",
        ])
        .expect("--auto-approve must keep composing with the tier-2 flag");
        assert!(cli.auto_approve);
        let (approval_bypass, dangerous_launch) = danger_tiers(&cli);
        assert!(
            !approval_bypass,
            "--auto-approve must not feed the tier-1 approval-bypass wiring"
        );
        assert!(dangerous_launch);

        // And on its own it selects NEITHER tier.
        let alone =
            Cli::try_parse_from(["wayland-core", "--auto-approve"]).expect("parses standalone");
        assert_eq!(danger_tiers(&alone), (false, false));
    }

    /// The lease lifetime attaches to tier 2 under BOTH spellings, and still
    /// cannot imply the authority on its own.
    #[test]
    fn dangerous_ttl_requires_an_explicit_dangerous_launch() {
        use clap::Parser as _;

        assert!(
            Cli::try_parse_from(["wayland-core", "--dangerous-ttl-secs", "30"]).is_err(),
            "a lease lifetime cannot imply Dangerous authority"
        );
        for spelling in ["--dangerously-skip-permissions-and-sandbox", "--dangerous"] {
            let cli = Cli::try_parse_from(["wayland-core", spelling, "--dangerous-ttl-secs", "30"])
                .unwrap_or_else(|e| panic!("{spelling} must accept a lease lifetime: {e}"));
            assert_eq!(cli.dangerous_ttl_secs, Some(30));
        }
    }

    #[test]
    fn managed_deny_refuses_local_dangerous_launch() {
        use wcore_types::execution_policy::ManagedDangerousPolicy;

        let config = Config {
            execution_policy: BaselineExecutionPolicy::managed(
                ApprovalPolicy::Prompt,
                ManagedDangerousPolicy::Deny,
            ),
            ..Default::default()
        };
        let error = resolve_local_execution(&config, false, true, 30, false)
            .err()
            .expect("Managed deny must reject even a local launch");
        assert!(error.to_string().contains("disabled by managed policy"));
    }

    #[test]
    fn desktop_dangerous_launch_is_source_bound() {
        let selection = resolve_local_execution(&Config::default(), false, true, 30, true)
            .expect("Desktop process launch is an allowed local source");
        let grant = selection
            .dangerous_grant()
            .expect("Dangerous must produce a resolver-owned lease");
        assert_eq!(grant.source(), PolicySource::DesktopLocalLaunch);
        assert_eq!(grant.ttl_millis(), 30_000);
    }

    #[test]
    fn wire_mode_changes_advance_only_for_accepted_effective_changes() {
        use wcore_protocol::commands::SessionMode;
        use wcore_types::execution_policy::EffectiveExecutionPolicy;

        let manager = ToolApprovalManager::new();
        manager.set_allow_wire_force(true);
        let policy = EffectiveExecutionPolicy::baseline(&BaselineExecutionPolicy::smart(
            ApprovalPolicy::Prompt,
            PolicySource::DesktopLocalLaunch,
        ));
        let mut sequence = ExecutionPolicySequence::launch(policy, 10);

        assert!(matches!(
            apply_wire_mode_change(&manager, &mut sequence, SessionMode::Default, 11).unwrap(),
            WireModeChange::Unchanged
        ));
        assert_eq!(sequence.current().revision, 0);

        let changed =
            apply_wire_mode_change(&manager, &mut sequence, SessionMode::AutoEdit, 12).unwrap();
        assert!(matches!(changed, WireModeChange::Changed(_)));
        assert_eq!(sequence.current().revision, 1);
        assert_eq!(
            sequence.current().policy.approvals(),
            ApprovalPolicy::AutoEdit
        );

        assert!(matches!(
            apply_wire_mode_change(&manager, &mut sequence, SessionMode::AutoEdit, 13).unwrap(),
            WireModeChange::Unchanged
        ));
        assert_eq!(sequence.current().revision, 1);
    }

    #[test]
    fn rejected_wire_mode_does_not_advance_or_mutate_policy() {
        use wcore_protocol::commands::SessionMode;
        use wcore_types::execution_policy::EffectiveExecutionPolicy;

        let manager = ToolApprovalManager::new();
        let policy = EffectiveExecutionPolicy::baseline(&BaselineExecutionPolicy::smart(
            ApprovalPolicy::Prompt,
            PolicySource::DesktopLocalLaunch,
        ));
        let mut sequence = ExecutionPolicySequence::launch(policy, 10);

        assert!(matches!(
            apply_wire_mode_change(&manager, &mut sequence, SessionMode::Force, 11).unwrap(),
            WireModeChange::Rejected {
                effective: SessionMode::Default
            }
        ));
        assert_eq!(sequence.current().revision, 0);
        assert_eq!(
            sequence.current().policy.approvals(),
            ApprovalPolicy::Prompt
        );
        assert_eq!(manager.session_mode(), SessionMode::Default);
    }

    #[test]
    fn auto_edit_gate_visibility_is_tool_name_aware() {
        use wcore_protocol::commands::SessionMode;
        use wcore_protocol::events::{ToolCategory, ToolInfo};

        let manager = Arc::new(ToolApprovalManager::new());
        manager.set_mode(SessionMode::AutoEdit);
        let capture = Arc::new(CapturingProtocolEmitter::default());
        let inner: Arc<dyn ProtocolEmitter> = capture.clone();
        let writer = GatingProtocolWriter::new(inner, manager, None);

        for (call_id, name) in [("remote-edit", "Notion"), ("local-write", "Write")] {
            writer
                .emit(&ProtocolEvent::ToolRequest {
                    msg_id: "m".into(),
                    call_id: call_id.into(),
                    tool: ToolInfo {
                        name: name.into(),
                        category: ToolCategory::Edit,
                        args: json!({}),
                        description: String::new(),
                        escalation: None,
                    },
                })
                .unwrap();
        }

        let events = capture.events.lock().unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            ProtocolEvent::ApprovalRequired { call_id, .. } if call_id == "remote-edit"
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            ProtocolEvent::ApprovalRequired { call_id, .. } if call_id == "local-write"
        )));
    }

    /// FerroxLabs/wayland#1070 (c) — what `approval_required.resume_token`
    /// carries, in both of its two cases.
    ///
    /// The `"resume_token": ""` seen in UAT is the DESIGNED value for an
    /// ordinary tool gate (GHSA-8r7g): that gate has no bridge entry, so there
    /// is no secret to mint or leak, and the host answers it with
    /// `tool_approve` / `tool_deny` keyed by `call_id`. A gate that IS
    /// bridge-backed must carry a non-empty token — and non-empty alone would
    /// not prove it is USABLE, so this resolves the parked approval with the
    /// exact token that went out on the wire.
    #[tokio::test]
    async fn a_bridge_backed_gate_carries_a_resume_token_that_round_trips() {
        use wcore_agent::approval::{ApprovalBridge, ApprovalOutcome, ApprovalRequest};
        use wcore_protocol::commands::ApprovalScope;
        use wcore_protocol::events::{ToolCategory, ToolInfo};

        let manager = Arc::new(ToolApprovalManager::new());
        let bridge = Arc::new(ApprovalBridge::new());
        let (_minted, parked) = bridge
            .request_with_id(
                "bridged-call".to_string(),
                ApprovalRequest {
                    call_id: "bridged-call".into(),
                    reason: "egress".into(),
                    context: String::new(),
                },
            )
            .await;
        let capture = Arc::new(CapturingProtocolEmitter::default());
        let inner: Arc<dyn ProtocolEmitter> = capture.clone();
        let writer = GatingProtocolWriter::new(inner, manager.clone(), Some(bridge.clone()));

        for call_id in ["bridged-call", "plain-call"] {
            writer
                .emit(&ProtocolEvent::ToolRequest {
                    msg_id: "m".into(),
                    call_id: call_id.into(),
                    tool: ToolInfo {
                        name: "Bash".into(),
                        category: ToolCategory::Exec,
                        args: json!({}),
                        description: String::new(),
                        escalation: None,
                    },
                })
                .unwrap();
        }

        let gates: Vec<(String, String, String)> = capture
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                ProtocolEvent::ApprovalRequired {
                    call_id,
                    resume_token,
                    correlation_id,
                    ..
                } => Some((
                    call_id.clone(),
                    resume_token.clone(),
                    correlation_id.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(gates.len(), 2, "both tools must be gated: {gates:?}");

        let bridged = gates
            .iter()
            .find(|gate| gate.0 == "bridged-call")
            .expect("the bridge-backed gate");
        assert!(
            !bridged.1.is_empty(),
            "a bridge-backed gate must carry the minted secret: {bridged:?}"
        );
        assert!(
            bridge
                .resolve(
                    &bridged.1,
                    ApprovalOutcome {
                        approved: true,
                        modifications: None,
                        cancellation: None,
                    },
                )
                .await,
            "the token on the wire must be the one the bridge accepts — non-empty is not enough"
        );
        let outcome = parked.await.expect("the parked bridge approval resolves");
        assert!(outcome.approved);

        // CONTROL: the ordinary tool gate is the empty-token case, and it is
        // not a dead end — `correlation_id` is the non-empty handle, and the
        // manager answers it by `call_id`.
        let plain = gates
            .iter()
            .find(|gate| gate.0 == "plain-call")
            .expect("the ordinary tool gate");
        assert!(
            plain.1.is_empty(),
            "an ordinary tool gate must mint NO bridge secret: {plain:?}"
        );
        assert_eq!(plain.2, "plain-call");
        let rx = manager.request_approval("plain-call", &ToolCategory::Exec, "Bash");
        assert!(
            manager.resolve_host("plain-call", true, ApprovalScope::Once, None),
            "the empty-token gate must be answerable by call_id"
        );
        assert!(matches!(
            rx.await.expect("the tool gate resolves"),
            ToolApprovalResult::Approved { .. }
        ));
    }

    #[test]
    fn one_use_api_key_is_consumed_and_removed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("credential");
        std::fs::write(&path, b"secret-canary").expect("write credential");

        let secret = read_one_use_api_key(&path).expect("consume credential");

        assert_eq!(secret, "secret-canary");
        assert!(
            !path.exists(),
            "credential file must be removed before boot"
        );
    }

    #[test]
    fn oversized_one_use_api_key_is_rejected_and_removed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("credential");
        std::fs::write(&path, vec![b'x'; 16 * 1024 + 1]).expect("write credential");

        let error = read_one_use_api_key(&path).expect_err("oversized credential must fail");

        assert!(error.to_string().contains("1..=16384 bytes"));
        assert!(!path.exists(), "rejected credential file must be removed");
    }

    #[test]
    fn one_use_eval_egress_key_is_consumed_and_removed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("egress-signing-key");
        let seed = [7_u8; 32];
        std::fs::write(&path, seed).expect("write signing key");

        let key = read_one_use_eval_egress_key(&path).expect("consume signing key");

        assert_eq!(key.to_bytes(), seed);
        assert!(!path.exists(), "signing key must be removed before boot");
    }

    #[test]
    fn invalid_one_use_eval_egress_key_is_rejected_and_removed() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("egress-signing-key");
        std::fs::write(&path, [7_u8; 31]).expect("write invalid signing key");

        let error = read_one_use_eval_egress_key(&path).expect_err("short key must fail");

        assert!(error.to_string().contains("exactly 32 bytes"));
        assert!(!path.exists(), "rejected signing key must be removed");
    }

    #[test]
    fn test_no_memory_flag_disables_memory() {
        let cli = Cli::parse_from(["wayland-core", "--no-memory", "hello"]);
        assert!(cli.no_memory, "--no-memory must parse to true");

        let mut config = Config::default();
        assert!(
            config.memory.enabled,
            "default config must have memory enabled"
        );
        apply_no_memory_flag(&mut config, cli.no_memory);
        assert!(
            !config.memory.enabled,
            "--no-memory must set memory.enabled = false"
        );
    }

    /// Rank 47: absence of `--no-memory` is one-directional — it must leave an
    /// already-enabled config untouched (the flag can only turn memory off,
    /// never on).
    #[test]
    fn test_no_memory_flag_absent_preserves_enabled() {
        let cli = Cli::parse_from(["wayland-core", "hello"]);
        assert!(!cli.no_memory, "flag defaults to false when omitted");

        let mut config = Config::default();
        apply_no_memory_flag(&mut config, cli.no_memory);
        assert!(
            config.memory.enabled,
            "without --no-memory the config's memory.enabled must survive"
        );
    }

    /// #186: a plain (non-MissingApiKey) error yields the base init-failure
    /// message and must NOT append the Ollama hint.
    #[test]
    fn test_init_failure_message_plain_error_has_no_ollama_hint() {
        let err = anyhow::anyhow!("network unreachable");
        let msg = init_failure_message(&err, "openai");
        assert!(
            msg.starts_with("Engine failed to start during init:"),
            "must lead with the base reason, got: {msg}"
        );
        assert!(
            !msg.contains("ollama:"),
            "non-MissingApiKey errors must not carry the local-model hint, got: {msg}"
        );
    }

    /// #186: a MissingApiKey error must append the actionable Ollama hint and
    /// name the provider label.
    #[test]
    fn test_init_failure_message_missing_api_key_has_ollama_hint() {
        let err = anyhow::Error::new(wcore_config::config::MissingApiKey);
        let msg = init_failure_message(&err, "anthropic");
        assert!(
            msg.contains("ollama:"),
            "MissingApiKey must surface the local-model hint, got: {msg}"
        );
        assert!(
            msg.contains("anthropic"),
            "hint must name the provider label, got: {msg}"
        );
    }

    /// #186: the hint must also fire when MissingApiKey is buried in the error
    /// chain via `.context(...)`, not only at the top level.
    #[test]
    fn test_init_failure_message_missing_api_key_in_chain() {
        use anyhow::Context;
        let err = Err::<(), _>(wcore_config::config::MissingApiKey)
            .context("resolving provider credentials")
            .unwrap_err();
        let msg = init_failure_message(&err, "anthropic");
        assert!(
            msg.contains("ollama:"),
            "a chained MissingApiKey must still surface the hint, got: {msg}"
        );
    }

    /// FluxRouter web_search grounding (contract §5): `--search` parses to a
    /// bool and defaults to false when omitted.
    #[test]
    fn test_search_flag_parses() {
        let cli = Cli::parse_from(["wayland-core", "--search", "latest JWST news"]);
        assert!(cli.search, "--search must parse to true");

        let cli = Cli::parse_from(["wayland-core", "hello"]);
        assert!(!cli.search, "--search defaults to false when omitted");
    }

    /// wayland#1174 — the `defer_config_mcp` path, which is the mode the
    /// Wayland Desktop host runs in.
    ///
    /// Boot leaves `McpCatalogRefresh` EMPTY under that flag (no config MCP is
    /// dialled), and `set_mcp_catalog_refresh` used to return early on
    /// `is_empty()`. The session therefore had no refresher at all and a
    /// `notifications/tools/list_changed` from ANY server was ignored for its
    /// whole life. This drives the real `integrate_deferred_mcp` and asserts a
    /// tool the server registers afterwards becomes callable.
    #[tokio::test]
    async fn deferred_config_mcp_still_honours_a_late_tools_list_changed() {
        let config = wcore_config::config::Config::default();
        let defer_cold = config.builtin_tools.defer_cold.clone();
        let (mut engine, _sink) =
            wcore_agent::bootstrap::AgentBootstrap::build_for_test(config, vec![]);

        // Exactly what bootstrap installs when `defer_config_mcp` is set:
        // no managers, no server configs.
        let refresh = Arc::new(wcore_mcp::tool_proxy::McpCatalogRefresh::new(
            Vec::new(),
            engine.tool_names(),
            HashMap::new(),
        ));
        engine.set_mcp_catalog_refresh(refresh);
        assert!(
            engine.mcp_catalog_refresh().is_some(),
            "an empty refresh must still be installed: the deferred connect fills it later"
        );

        let fixture = Arc::new(GrowingTestTransport::new(&["warehouse_reserve"]));
        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "warehouse",
            false,
            Box::new(SharedTransport(fixture.clone())) as Box<dyn McpTransport>,
            vec![tool("warehouse_reserve")],
        )]));
        let resolved = HashMap::from([(
            "warehouse".to_string(),
            to_mcp_server_config(
                "stdio",
                Some("unused-test-command".to_string()),
                None,
                None,
                None,
                None,
                false,
                None,
            )
            .expect("valid test server config"),
        )]);
        let writer = ProtocolWriter::new();
        let mut dynamic_managers = Vec::new();
        let mut reservations = lifecycle_reservations(&resolved);
        assert!(integrate_deferred_mcp(
            &mut engine,
            manager.clone(),
            &resolved,
            &mut reservations,
            &writer,
            &mut dynamic_managers,
            &mut inert_late_binder(),
            &mut Vec::new(),
        ));
        assert!(engine.tools().get("warehouse_reserve").is_some());
        assert!(engine.tools().get("warehouse_audit_export").is_none());

        // The server registers a tool mid-session and says so.
        fixture.register_and_announce("warehouse_audit_export");

        let refresh = engine
            .mcp_catalog_refresh()
            .expect("the deferred connect must leave a live refresh behind");
        let registry = engine
            .registry_mut()
            .expect("idle fixture registry must be mutable");
        let refreshed = refresh.apply(registry, &defer_cold).await;
        assert_eq!(
            refreshed,
            vec!["warehouse".to_string()],
            "a deferred-config server's tools/list_changed must be honoured"
        );
        assert!(
            engine.tools().get("warehouse_audit_export").is_some(),
            "the late tool must be callable"
        );
    }

    /// FerroxLabs/wayland#1234 c1/c2 — the SAME resurrection, on the
    /// `CleanupUnverified` arm, which is the arm this tree used to skip.
    ///
    /// WHY THIS IS A SEPARATE TEST AND NOT A VARIANT SPELLING. The sibling
    /// below drives the `Removed` arm, where `close_server` succeeded and the
    /// withdrawal has landed since #1213 c4. This drives the arm where
    /// `close_server` FAILS. Until wayland#1234 the handler returned early on
    /// that arm — after it had already taken the tools out of the live
    /// registry — so the manager stayed in `McpCatalogRefresh` while its
    /// transport was, by the definition of the arm, NOT proven dead. The next
    /// `notifications/tools/list_changed` re-registered the tools the operator
    /// had just removed.
    ///
    /// The criterion says "regardless of what its transport reports about
    /// liveness", so the fixture is alive in both directions: it refuses to
    /// close AND it goes on serving `tools/list`. Nothing here is carried by
    /// a dead-transport skip in `refresh_signalled_tools`.
    ///
    /// RED ARM (recorded, re-runnable): delete the
    /// `withdraw_runtime_mcp_from_refresh(engine, &command.name);` line on the
    /// cleanup-unverified arm of `remove_runtime_mcp_server`, `touch`
    /// `main.rs`, rebuild — this test fails on the resurrection assertion
    /// while the `Removed`-arm sibling stays green.
    #[tokio::test]
    async fn an_unverified_removal_still_withdraws_so_the_server_cannot_resurrect() {
        let config = wcore_config::config::Config::default();
        let defer_cold = config.builtin_tools.defer_cold.clone();
        let (mut engine, _sink) =
            wcore_agent::bootstrap::AgentBootstrap::build_for_test(config, vec![]);
        engine.set_mcp_catalog_refresh(Arc::new(wcore_mcp::tool_proxy::McpCatalogRefresh::new(
            Vec::new(),
            engine.tool_names(),
            HashMap::new(),
        )));

        let fixture = Arc::new(GrowingTestTransport::new_refusing_close(&[
            "warehouse_reserve",
        ]));
        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "warehouse",
            false,
            Box::new(SharedTransport(fixture.clone())) as Box<dyn McpTransport>,
            vec![tool("warehouse_reserve")],
        )]));
        let server_config = to_mcp_server_config(
            "stdio",
            Some("unused-test-command".to_string()),
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .expect("valid test server config");
        let resolved = HashMap::from([("warehouse".to_string(), server_config.clone())]);
        let writer = ProtocolWriter::new();
        let mut dynamic_managers = Vec::new();
        let mut reservations = lifecycle_reservations(&resolved);
        assert!(integrate_deferred_mcp(
            &mut engine,
            manager.clone(),
            &resolved,
            &mut reservations,
            &writer,
            &mut dynamic_managers,
            &mut inert_late_binder(),
            &mut Vec::new(),
        ));
        assert!(
            engine.tools().get("warehouse_reserve").is_some(),
            "precondition: the runtime-added server's tool is live"
        );

        let mut diagnostics = RuntimeDiagnosticsState::from_launch(
            &wcore_config::config::Config::default(),
            &wcore_config::resolution_provenance::ConfigResolutionProvenance::default(),
            None,
            wcore_protocol::diagnostics::RuntimeEngineMode::Unknown,
            wcore_protocol::diagnostics::RuntimeWorkspaceKind::Unknown,
        );
        assert!(diagnostics.record_runtime_declaration("warehouse", &server_config));
        let lifecycle = McpLifecycleCatalog::new();
        // Seed the name READY so the removal has a lifecycle entry to move.
        // Without this mark_stopping is a no-op, mark_cleanup_unverified
        // records nothing, and the arm control below cannot observe which arm
        // ran -- which is exactly what it caught on the first attempt.
        assert!(lifecycle.seed_ready(
            "warehouse",
            wcore_agent::mcp_lifecycle::McpConfigIdentity::for_server(&server_config),
        ));
        let mut removal_ledger = McpRemovalLedger::default();
        remove_runtime_mcp_server(
            RemoveMcpServerCommand {
                lifecycle_version: MCP_LIFECYCLE_VERSION,
                request_id: "removal-unverified-1".to_string(),
                name: "warehouse".to_string(),
            },
            &mut removal_ledger,
            &mut diagnostics,
            &lifecycle,
            &mut engine,
            &mut dynamic_managers,
            &writer,
        )
        .await;

        // CONTROL ON THE ARM. Without this the test could be driving the
        // ordinary `Removed` path and grading nothing new: the whole point is
        // that cleanup was NOT verified here.
        assert!(
            matches!(
                lifecycle.snapshot("warehouse").map(|s| s.state),
                Some(McpLifecycleState::CleanupUnverified { .. })
            ),
            "control: this test must exercise the CleanupUnverified arm, and \
             the lifecycle says it took a different one: {:?}",
            lifecycle.snapshot("warehouse").map(|s| s.state)
        );
        // The tools are gone from the live registry on this arm too — which is
        // exactly why leaving the manager in the refresh is a resurrection and
        // not a harmless no-op.
        assert!(
            engine.tools().get("warehouse_reserve").is_none(),
            "precondition: an unverified removal still drops the server's \
             tools from the live registry"
        );

        // The server the operator detached goes on talking.
        fixture.register_and_announce("warehouse_audit_export");
        let refresh = engine
            .mcp_catalog_refresh()
            .expect("the refresh outlives the removal");
        let registry = engine
            .registry_mut()
            .expect("idle fixture registry must be mutable");
        let refreshed = refresh.apply(registry, &defer_cold).await;
        assert!(
            refreshed.is_empty(),
            "wayland#1234: a server removed on the CleanupUnverified arm was \
             still polled by McpCatalogRefresh. Refreshed: {refreshed:?}"
        );
        assert!(
            engine.tools().get("warehouse_audit_export").is_none(),
            "wayland#1234: the removed server's NEW tool was re-registered \
             into the live registry after an unverified removal — the operator \
             took the server away and it came back"
        );
    }

    /// FerroxLabs/wayland#1213 c4 — the resurrection the ticket names.
    ///
    /// #1213 fixed `take_tools_changed` for SSE and Streamable HTTP, which is
    /// what made this reachable: before it, `refresh_signalled_tools` could
    /// never fire for a URL transport, so a stale entry in
    /// `McpCatalogRefresh` was inert. c4 says the withdrawal must land in the
    /// SAME change, and this is the observable it names — an operator removed
    /// the server, the server announces a tool anyway, and NOTHING is
    /// re-registered.
    ///
    /// The fixture transport deliberately does not go dead on `close()`.
    /// `McpManager::close_server` marks the three real transports dead and
    /// `refresh_signalled_tools` skips dead transports, so a fixture that
    /// died on close would test that second mechanism instead of this one and
    /// would pass with the withdrawal removed. Holding it alive isolates the
    /// withdrawal, and it is the honest model of the arm where cleanup could
    /// not be verified: there, the manager is left in place and nobody has
    /// proved the transport dead.
    #[tokio::test]
    async fn a_removed_runtime_server_is_not_resurrected_by_a_later_list_changed() {
        let config = wcore_config::config::Config::default();
        let defer_cold = config.builtin_tools.defer_cold.clone();
        let (mut engine, _sink) =
            wcore_agent::bootstrap::AgentBootstrap::build_for_test(config, vec![]);
        engine.set_mcp_catalog_refresh(Arc::new(wcore_mcp::tool_proxy::McpCatalogRefresh::new(
            Vec::new(),
            engine.tool_names(),
            HashMap::new(),
        )));

        let fixture = Arc::new(GrowingTestTransport::new(&["warehouse_reserve"]));
        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "warehouse",
            false,
            Box::new(SharedTransport(fixture.clone())) as Box<dyn McpTransport>,
            vec![tool("warehouse_reserve")],
        )]));
        let server_config = to_mcp_server_config(
            "stdio",
            Some("unused-test-command".to_string()),
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .expect("valid test server config");
        let resolved = HashMap::from([("warehouse".to_string(), server_config.clone())]);
        let writer = ProtocolWriter::new();
        let mut dynamic_managers = Vec::new();
        let mut reservations = lifecycle_reservations(&resolved);
        assert!(integrate_deferred_mcp(
            &mut engine,
            manager.clone(),
            &resolved,
            &mut reservations,
            &writer,
            &mut dynamic_managers,
            &mut inert_late_binder(),
            &mut Vec::new(),
        ));
        assert!(
            engine.tools().get("warehouse_reserve").is_some(),
            "precondition: the runtime-added server's tool is live"
        );

        // The operator removes it through the host command.
        let mut diagnostics = RuntimeDiagnosticsState::from_launch(
            &wcore_config::config::Config::default(),
            &wcore_config::resolution_provenance::ConfigResolutionProvenance::default(),
            None,
            wcore_protocol::diagnostics::RuntimeEngineMode::Unknown,
            wcore_protocol::diagnostics::RuntimeWorkspaceKind::Unknown,
        );
        assert!(diagnostics.record_runtime_declaration("warehouse", &server_config));
        let lifecycle = McpLifecycleCatalog::new();
        let mut removal_ledger = McpRemovalLedger::default();
        remove_runtime_mcp_server(
            RemoveMcpServerCommand {
                lifecycle_version: MCP_LIFECYCLE_VERSION,
                request_id: "removal-1".to_string(),
                name: "warehouse".to_string(),
            },
            &mut removal_ledger,
            &mut diagnostics,
            &lifecycle,
            &mut engine,
            &mut dynamic_managers,
            &writer,
        )
        .await;
        assert!(
            engine.tools().get("warehouse_reserve").is_none(),
            "precondition: removal drops the server's tools from the live registry"
        );
        assert!(!diagnostics.has_runtime_declaration("warehouse"));

        // The removed server announces a tool anyway — a hosted server the
        // operator detached does not stop talking on request.
        fixture.register_and_announce("warehouse_audit_export");
        let refresh = engine
            .mcp_catalog_refresh()
            .expect("the refresh outlives the removal");
        let registry = engine
            .registry_mut()
            .expect("idle fixture registry must be mutable");
        let refreshed = refresh.apply(registry, &defer_cold).await;
        assert!(
            refreshed.is_empty(),
            "a removed server must not be refreshed at all; refreshed {refreshed:?}"
        );
        assert!(
            engine.tools().get("warehouse_audit_export").is_none(),
            "the removed server's NEW tool was registered into the live registry"
        );
        assert!(
            engine.tools().get("warehouse_reserve").is_none(),
            "the removed server's OLD tools came back with the refresh"
        );
    }

    /// Negative control for the test above, and the #998 guard on the new
    /// door: an operator allowlist of `Some([])` means "disable every tool on
    /// this server" — and must still mean that after a `list_changed` on the
    /// deferred-config path. A manager admitted WITHOUT its config would hit
    /// the `config == None -> allow-all` read in `tool_proxy` and restore the
    /// server's full tool set.
    #[tokio::test]
    async fn a_deferred_servers_empty_allowlist_survives_the_refresh() {
        let config = wcore_config::config::Config::default();
        let defer_cold = config.builtin_tools.defer_cold.clone();
        let (mut engine, _sink) =
            wcore_agent::bootstrap::AgentBootstrap::build_for_test(config, vec![]);
        engine.set_mcp_catalog_refresh(Arc::new(wcore_mcp::tool_proxy::McpCatalogRefresh::new(
            Vec::new(),
            engine.tool_names(),
            HashMap::new(),
        )));

        let fixture = Arc::new(GrowingTestTransport::new(&["locked_tool"]));
        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "locked",
            false,
            Box::new(SharedTransport(fixture.clone())) as Box<dyn McpTransport>,
            vec![tool("locked_tool")],
        )]));
        let mut server_config = to_mcp_server_config(
            "stdio",
            Some("unused-test-command".to_string()),
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .expect("valid test server config");
        server_config.allowed_tools = Some(Vec::new());
        let resolved = HashMap::from([("locked".to_string(), server_config)]);
        let writer = ProtocolWriter::new();
        let mut dynamic_managers = Vec::new();
        let mut reservations = lifecycle_reservations(&resolved);
        assert!(integrate_deferred_mcp(
            &mut engine,
            manager.clone(),
            &resolved,
            &mut reservations,
            &writer,
            &mut dynamic_managers,
            &mut inert_late_binder(),
            &mut Vec::new(),
        ));
        assert!(
            engine.tools().get("locked_tool").is_none(),
            "boot: an empty allowlist disables every tool"
        );

        fixture.register_and_announce("locked_late_tool");
        let refresh = engine.mcp_catalog_refresh().expect("refresh installed");
        let registry = engine.registry_mut().expect("registry must be mutable");
        assert_eq!(
            refresh.apply(registry, &defer_cold).await,
            vec!["locked".to_string()]
        );
        assert!(
            engine.tools().get("locked_late_tool").is_none(),
            "the refresh must not restore a tool the operator disabled"
        );
        assert!(engine.tools().get("locked_tool").is_none());
    }

    /// The wiring, as a CLASS rather than a census.
    ///
    /// A helper nothing calls is not a fix, and the failure this guards is
    /// specifically a runtime-add site left bare — which is what #1175
    /// reports for all three of the sites that existed when it was written.
    ///
    /// ROUND 1 counted `register_runtime_server(` and compared it to a
    /// HARDCODED 2. The 0.13.12 close-sweep refuted that in its own words: "a
    /// FOURTH bare runtime-add path in main.rs would leave the count at 2 and
    /// pass". It would — the needle it counts is the FIX, so a path that
    /// never applies the fix subtracts nothing from the total.
    ///
    /// The count is now DERIVED from the DEFECT instead: every file under
    /// `wcore-cli/src` that CONSTRUCTS an `McpManager` must carry at least as
    /// many refresh registrations as constructions. A fourth bare path adds a
    /// construction and no registration, and the file goes red. Registration
    /// is allowed to happen in a different function from the construction —
    /// the #551 deferred path genuinely does that — so this is a per-FILE
    /// pairing, not a per-function one.
    #[test]
    fn every_runtime_mcp_add_joins_the_catalog_refresh() {
        // Needles are assembled at compile time from fragments so that this
        // test's own source, which `include_str!`/the walk below both read,
        // never matches itself. (The round-1 lint got its count of 3 from its
        // own assertion strings.)
        let constructs = concat!("McpManager::", "connect");
        let registers = concat!("register_runtime_", "server(");

        // The one construction that legitimately never registers, named with
        // its reason. This is an ALLOWLIST and is stated as one: `wayland
        // doctor` dials the config-declared servers to print a health table
        // and drops the manager at the end of the match arm. There is no
        // engine and no live session for it to register into. Anything else
        // that wants an exemption has to be added here, in the open.
        const EXEMPT: &[(&str, &str)] = &[(
            "doctor/mod.rs",
            "throwaway health probe; no engine, manager dropped at the arm",
        )];

        let mut constructing: Vec<(String, usize, usize)> = Vec::new();
        for (path, source) in wcore_cli_production_sources() {
            let built = source.matches(constructs).count();
            if built == 0 {
                continue;
            }
            constructing.push((path, built, source.matches(registers).count()));
        }

        // POSITIVE CONTROL on the walk. If it silently found nothing — wrong
        // root, renamed constructor, a `use` alias — every assertion below
        // would vacuously pass and this would grade an empty set.
        for known in ["src/main.rs", "tui/engine_bridge.rs", "doctor/mod.rs"] {
            assert!(
                constructing
                    .iter()
                    .any(|(path, _, _)| path.ends_with(known)),
                "the McpManager construction walk did not find {known} — \
                 discovery is broken and this lint grades an empty set. \
                 Found: {constructing:?}"
            );
        }

        for (path, built, registered) in &constructing {
            if let Some((_, why)) = EXEMPT.iter().find(|(file, _)| path.ends_with(file)) {
                // A stale exemption is worse than none: it silently covers
                // whatever the file grows into next.
                assert!(
                    *registered == 0,
                    "{path} is exempted ({why}) but now registers with the \
                     catalog refresh — drop the exemption"
                );
                continue;
            }
            assert!(
                registered >= built,
                "{path} constructs {built} McpManager(s) but joins the catalog \
                 refresh {registered} time(s). A runtime-add path that never \
                 reaches McpCatalogRefresh has its tools/list_changed ignored \
                 for the life of the session (FerroxLabs/wayland#1175). Add the \
                 registration, or add the file to EXEMPT with the reason."
            );
        }

        // #1174 — unchanged, and unrelated to the count above.
        let engine_src = include_str!("../../wcore-agent/src/engine.rs");
        assert!(
            engine_src.contains("fn set_mcp_catalog_refresh"),
            "known-positive control: the setter this asserts about still exists"
        );
        assert!(
            !engine_src.contains("if refresh.is_empty() {"),
            "set_mcp_catalog_refresh must not skip an empty refresh again (#1174): \
             empty is the state defer_config_mcp produces, and the deferred connect \
             fills it afterwards"
        );
    }

    /// The withdrawal half, FerroxLabs/wayland#1213 c4.
    ///
    /// #1213 c4 is explicit that implementing `take_tools_changed` for the URL
    /// transports without this is a live resurrection bug. The add side is
    /// counted per file above because construction and registration can sit in
    /// different functions; withdrawal cannot — the function that takes a
    /// runtime server away is the function that owns the name at that instant.
    /// So this is graded per FUNCTION, which catches a fifth withdrawal path in
    /// a brand-new function that a count over the whole file would not.
    ///
    /// THE FILE SET WAS THE HOLE, and it is the reason this reads the tree
    /// instead of one file. Round 1 graded `include_str!("main.rs")` and
    /// nothing else. Two consequences, both measured on 2026-08-30: the TUI's
    /// `/mcp add` rollback was ungraded, and — the part that was not a guard
    /// regression but a live defect — `TuiEngine::remove_tui_runtime_mcp`, the
    /// function behind the documented interactive `/mcp remove`, dropped the
    /// registry entry and left the `McpCatalogRefresh` entry behind. A guard
    /// scoped to one file cannot fail on a second file, ever, so the set now
    /// comes from `wcore_cli_production_sources()` — the same set the add side
    /// walks — and a withdrawal path in a brand-new file is graded the day it
    /// is written.
    ///
    /// GAP, recorded rather than implied away: the DEFECT needles below are
    /// still a spelling set (`.remove_runtime_declaration(` and
    /// `.remove_mcp_server(`, receiver-agnostic but method-named). A removal
    /// spelled some third way is invisible to them. That is why the control at
    /// the end pins the exact `(file, fn)` set rather than a count: a rename
    /// that hides a site drops a pair and reddens here instead of passing
    /// quietly.
    #[test]
    fn every_runtime_mcp_withdrawal_leaves_the_catalog_refresh() {
        // Fragment-assembled for the same reason as the add side: this test's
        // own source is inside the tree the walk reads.
        let drops = [
            concat!(".remove_runtime_", "declaration("),
            concat!(".remove_mcp_", "server("),
        ];
        let withdraws = [
            concat!("withdraw_runtime_mcp_from_", "refresh("),
            concat!("forget_runtime_", "server("),
        ];

        // The one removal-shaped call that is a DISPATCH rather than a
        // removal, named with its reason. `/mcp remove` in the surface layer
        // hands the name to `TuiEngine::remove_mcp_server`, which spawns
        // `remove_tui_runtime_mcp` — the function that really takes the server
        // away, and which IS graded below. Anything else wanting an exemption
        // has to be added here, in the open.
        const EXEMPT: &[(&str, &str, &str)] = &[(
            "tui/surfaces/mod.rs",
            "dispatch_command",
            "dispatches to TuiEngine::remove_mcp_server; the real removal is \
             remove_tui_runtime_mcp, graded below",
        )];

        let mut graded: Vec<(String, String)> = Vec::new();
        let mut exercised_exemptions = 0usize;
        for (path, source) in wcore_cli_production_sources() {
            for (name, body) in fn_blocks(&source) {
                if !drops.iter().any(|needle| body.contains(needle)) {
                    continue;
                }
                if EXEMPT
                    .iter()
                    .any(|(file, func, _)| path.ends_with(file) && *func == name)
                {
                    exercised_exemptions += 1;
                    continue;
                }
                assert!(
                    withdraws.iter().any(|needle| body.contains(needle)),
                    "fn {name} ({path}) takes a runtime MCP server away but \
                     leaves it in McpCatalogRefresh. Its tools are re-registered \
                     into the live registry on the next \
                     notifications/tools/list_changed — an operator-removed \
                     server resurrected (FerroxLabs/wayland#1213 c4)"
                );
                graded.push((path.clone(), name));
            }
        }

        // POSITIVE CONTROL, on the SET rather than a count. A walk that found
        // nothing, a splitter that returned nothing, or a renamed receiver that
        // hid one site would all pass the loop above vacuously.
        let mut found: Vec<String> = graded
            .iter()
            .map(|(path, name)| {
                let file = path.rsplit_once("/src/").map_or(path.as_str(), |(_, r)| r);
                format!("{file}::{name}")
            })
            .collect();
        found.sort();
        assert_eq!(
            found,
            vec![
                "main.rs::remove_runtime_mcp_server".to_string(),
                "main.rs::teardown_runtime_mcp_for_replace".to_string(),
                "tui/engine_bridge.rs::connect_and_register_mcp".to_string(),
                "tui/engine_bridge.rs::remove_tui_runtime_mcp".to_string(),
            ],
            "the withdrawal walk graded a different set of functions than the \
             four known runtime-MCP removal paths. A new one is fine — grade it \
             and add it here. One GOING MISSING is the failure this control \
             exists for: the needle no longer matches that site and it is now \
             ungraded."
        );

        // A stale exemption silently covers whatever the file grows into next,
        // so the exemption must still be exercised by something.
        assert_eq!(
            exercised_exemptions,
            EXEMPT.len(),
            "an EXEMPT entry matched nothing — drop it rather than leave a \
             blanket over {} file(s)",
            EXEMPT.len()
        );
    }

    /// Every `.rs` file under `wcore-cli/src`, as `(display path, production
    /// source)` — inline `#[cfg(test)]` items and `//` comments removed.
    ///
    /// THE FILE SET IS THE CLASS, and it is decided here, once, for both
    /// pairing guards in this module. A pairing lint is only ever as complete
    /// as the set of files it reads, and a set written down by hand cannot
    /// fail on a file that is not in it. The withdrawal guard's set was
    /// literally `include_str!("main.rs")`; `tui/engine_bridge.rs` was
    /// therefore ungraded, and a live #1213 c4 defect sat in it. Deriving the
    /// set from the tree at test time is what makes "a new file escapes"
    /// impossible.
    ///
    /// A TEST FIXTURE IS NOT A CALL SITE. `a_comment_is_not_a_registration`
    /// holds the string "refresh.register_runtime_server(&mgr, &configs);" as
    /// a fixture, twice, in this very file. Counting raw text scored those as
    /// two production registrations and gave main.rs two registrations of
    /// SLACK — enough that deleting the real AddMcpServer registration left
    /// the count passing. MEASURED: with the AddMcpServer call replaced by
    /// `let _unused = refresh;`, the add-side guard was still green. That is
    /// the #1175 residual verbatim ("a FOURTH bare runtime-add path would
    /// leave the count at 2 and pass"), reintroduced by the guard's own
    /// fixtures. Production code is what ships, so inline test modules are
    /// removed before anything is counted — on BOTH sides, since a
    /// construction in a test is not a runtime-add path either.
    ///
    /// A COMMENT IS NOT A CALL. Counting raw text let the add-side negative
    /// control be satisfied by `// refresh.register_runtime_server(...)`,
    /// which is the same trap the #1175 transport guard closed on its own
    /// side.
    fn wcore_cli_production_sources() -> Vec<(String, String)> {
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![src_dir];
        let mut sources = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)
                .expect("read wcore-cli/src")
                .flatten()
            {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("read rust source");
                let source = strip_cfg_test_modules(&source);
                let source = strip_line_comments(&source);
                // wayland-core#409 c3/c4. Every comparison downstream of this
                // walk is written with `/` -- `ends_with("src/main.rs")`, the
                // `EXEMPT` table's `"tui/surfaces/mod.rs"`, and the positive
                // control's `rsplit_once("/src/")`. `Path::display` emits the
                // PLATFORM separator, so on Windows all three stopped matching
                // at once: the add-side lint asserted "discovery is broken and
                // this lint grades an empty set" WHILE PRINTING the file it
                // said it could not find, and the withdrawal-side lint lost its
                // exemption and accused `dispatch_command` of a defect it does
                // not have. Both were correct on Linux and red on the
                // self-hosted runner, which is why nothing caught it until a
                // Windows leg ran for the first time.
                //
                // Normalised once here rather than at three call sites, because
                // a fourth comparison would inherit the bug. Guarded on
                // `cfg!(windows)`: a backslash is a legal character in a Unix
                // filename, and rewriting it there would be a different bug.
                let display = path.display().to_string();
                let display = if cfg!(windows) {
                    display.replace('\\', "/")
                } else {
                    display
                };
                sources.push((display, source));
            }
        }
        sources
    }

    /// One line with its `//`-to-end-of-line comment removed.
    fn code_before_comment(line: &str) -> &str {
        match line.find("//") {
            Some(at) => &line[..at],
            None => line,
        }
    }

    /// `source` with every `//`-to-end-of-line comment removed.
    ///
    /// Both guards in this module count CALLS, and a comment naming a call is
    /// not a call — the trap that made the #1175 transport guard accept
    /// `// we do not need fn take_tools_changed here`. Naive on purpose: a
    /// `//` inside a string literal (a URL) truncates that line early, which
    /// can only ever HIDE a needle, never invent one, so the guard's error is
    /// toward a false red on the add side. Block comments are not stripped;
    /// that gap is recorded in the #1175 ledger rather than implied away.
    fn strip_line_comments(source: &str) -> String {
        source
            .lines()
            .map(code_before_comment)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `source` with every column-zero `#[cfg(test)]` item removed.
    ///
    /// Brace-counted from the item's own opening brace, so a nested `mod` or a
    /// braced string inside the test module cannot end the strip early. An
    /// UNBALANCED brace would run the strip to end-of-file, which deletes more
    /// than it should and can only ever make the add-side guard MORE likely to
    /// go red — the safe direction for a lint.
    ///
    /// Column zero is the discriminator, matching the rest of this module: an
    /// inline unit-test module in this tree is written at the top level of its
    /// file. A `#[cfg(test)]` on an indented item is left in place, which is a
    /// named gap rather than a claim.
    fn strip_cfg_test_modules(source: &str) -> String {
        let lines: Vec<&str> = source.lines().collect();
        let mut kept: Vec<&str> = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            if lines[i].trim_end() != "#[cfg(test)]" {
                kept.push(lines[i]);
                i += 1;
                continue;
            }
            // Skip forward to the item's body and then past its close.
            let mut depth = 0i32;
            let mut opened = false;
            let mut j = i;
            while j < lines.len() {
                let code = match lines[j].find("//") {
                    Some(at) => &lines[j][..at],
                    None => lines[j],
                };
                depth += code.matches('{').count() as i32;
                depth -= code.matches('}').count() as i32;
                if code.contains('{') {
                    opened = true;
                }
                if opened && depth <= 0 {
                    break;
                }
                // A `#[cfg(test)] use ...;` has no body at all.
                if !opened && code.trim_end().ends_with(';') && j > i {
                    break;
                }
                j += 1;
            }
            i = j + 1;
        }
        kept.join("\n")
    }

    /// The stripper decides what the add-side guard is allowed to see, so it
    /// is graded directly rather than trusted.
    #[test]
    fn a_test_fixture_is_not_a_production_call_site() {
        let needle = concat!("register_runtime_", "server(");
        let source = "\
fn production() {
    refresh.register_runtime_server(&mgr, &configs);
}

#[cfg(test)]
mod tests {
    #[test]
    fn fixture() {
        let real = \"    refresh.register_runtime_server(&mgr, &configs);\";
        assert!(real.contains(\"x\"));
    }
}
";
        let stripped = strip_cfg_test_modules(source);
        assert_eq!(
            stripped.matches(needle).count(),
            1,
            "the test module's fixture was counted as a production call site, \
             which is the slack that let a deleted registration pass: {stripped}"
        );
        // POSITIVE CONTROL: the production call must SURVIVE, or the stripper
        // satisfies the count above by deleting everything.
        assert!(
            stripped.contains("fn production"),
            "the stripper ate production code: {stripped}"
        );
        // NEGATIVE CONTROL: a file with no test module is returned intact.
        let plain = "fn only() {\n    refresh.register_runtime_server(&m, &c);\n}\n";
        assert_eq!(strip_cfg_test_modules(plain).matches(needle).count(), 1);
        // A `#[cfg(test)]` on a non-module item must not swallow the rest of
        // the file.
        let attr_use = "#[cfg(test)]\nuse std::io;\n\nfn after() {\n    refresh.register_runtime_server(&m, &c);\n}\n";
        assert_eq!(
            strip_cfg_test_modules(attr_use).matches(needle).count(),
            1,
            "a `#[cfg(test)] use` swallowed the production code after it"
        );
    }

    #[test]
    fn a_comment_is_not_a_registration() {
        let needle = concat!("register_runtime_", "server(");
        let commented = "    // refresh.register_runtime_server(&mgr, &configs);\n";
        assert!(
            !strip_line_comments(commented).contains(needle),
            "a commented-out registration satisfies the add-side guard"
        );
        // POSITIVE CONTROL: the real call still counts, or the check above is
        // satisfied by a stripper that deletes everything.
        let real = "    refresh.register_runtime_server(&mgr, &configs);\n";
        assert!(
            strip_line_comments(real).contains(needle),
            "a real registration is not recognised"
        );
        // And a trailing comment must not eat the call on the same line.
        let both = "    refresh.register_runtime_server(&mgr, &configs); // wayland#1175\n";
        assert!(strip_line_comments(both).contains(needle));
    }

    /// Every `fn` in `source`, at ANY indentation, as `(name, body)`.
    ///
    /// Indentation used to be the discriminator — column zero only — and that
    /// was a second file-set hole wearing a different hat: every runtime-MCP
    /// path in `tui/engine_bridge.rs` is an inherent method on `TuiEngine`, so
    /// a column-zero splitter returns NOTHING for that file and every
    /// assertion over it passes vacuously. Free functions and methods are
    /// graded alike now.
    ///
    /// Blocks do not overlap: the scan resumes after a function's closing
    /// brace, so a `fn` nested inside another is absorbed into its parent's
    /// body rather than graded separately. That is a stated gap — a nested
    /// helper that removed a server while its parent withdrew would pass — and
    /// there is no such nesting on these paths today.
    /// A BODYLESS `fn` — a trait method declaration, which ends in `;` before
    /// any `{` — is skipped rather than brace-counted. `tui/surfaces/mod.rs`
    /// has one (`fn render(..);`), and brace-counting from it ran to the end
    /// of the enclosing trait and past it, swallowing the real functions that
    /// followed. That is the exact shape of a silently-vacuous guard, so it is
    /// its own case with its own test.
    fn fn_blocks(source: &str) -> Vec<(String, String)> {
        let lines: Vec<&str> = source.lines().collect();
        let mut blocks = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let Some(name) = associated_fn_name(code_before_comment(lines[i])) else {
                i += 1;
                continue;
            };
            // Walk to whichever comes first: the `{` that opens the body, or
            // the `;` that ends a declaration. A rustfmt-wrapped signature puts
            // either of them several lines down.
            let mut k = i;
            let mut opens_a_body = false;
            while k < lines.len() {
                let code = code_before_comment(lines[k]);
                let brace = code.find('{');
                let semi = code.find(';');
                match (brace, semi) {
                    (Some(b), Some(s)) if s < b => break,
                    (Some(_), _) => {
                        opens_a_body = true;
                        break;
                    }
                    (None, Some(_)) => break,
                    (None, None) => k += 1,
                }
            }
            if !opens_a_body {
                i = k + 1;
                continue;
            }
            // Brace-count to the end of the body. Line comments are stripped
            // first so a `//` mentioning a brace cannot skew the depth.
            let mut depth = 0i32;
            let mut body = String::new();
            let mut k = i;
            let mut opened = false;
            while k < lines.len() {
                let code = code_before_comment(lines[k]);
                depth += code.matches('{').count() as i32;
                depth -= code.matches('}').count() as i32;
                if code.contains('{') {
                    opened = true;
                }
                body.push_str(code);
                body.push('\n');
                if opened && depth <= 0 {
                    break;
                }
                k += 1;
            }
            blocks.push((name, body));
            i = k + 1;
        }
        blocks
    }

    /// The splitter is the thing that can silently stop finding functions, so
    /// it is graded directly rather than trusted.
    #[test]
    fn the_fn_splitter_scopes_a_body_to_its_own_function() {
        let source = "fn good() {\n    withdraw();\n}\n\nasync fn bad(x: u8) -> u8 {\n    0\n}\n\nimpl T for U {\n    pub(crate) async fn method(\n        &self,\n    ) -> u8 {\n        withdraw();\n        0\n    }\n}\n";
        let blocks = fn_blocks(source);
        assert_eq!(
            blocks.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["good", "bad", "method"],
            "an indented method IS a withdrawal path — every runtime-MCP path \
             in tui/engine_bridge.rs is one, and a column-zero-only splitter \
             grades that whole file vacuously: {blocks:?}"
        );
        assert!(blocks[0].1.contains("withdraw();"));
        assert!(
            blocks[2].1.contains("withdraw();"),
            "a rustfmt-wrapped method signature must not lose its body"
        );
        // NEGATIVE CONTROL: the second body must NOT borrow the first's call,
        // or a file with one compliant fn grades every other fn green.
        assert!(
            !blocks[1].1.contains("withdraw();"),
            "the second fn body swallowed the first one's call"
        );

        // A BODYLESS trait method must not be brace-counted: doing so runs to
        // the end of the enclosing trait and swallows the functions after it.
        let with_declaration = "trait S {\n    fn render(&mut self, f: &mut F);\n}\n\nfn after() {\n    withdraw();\n}\n";
        let blocks = fn_blocks(with_declaration);
        assert_eq!(
            blocks.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["after"],
            "a `fn ...;` declaration has no body to grade, and must not eat the \
             functions that follow it: {blocks:?}"
        );
        assert!(blocks[0].1.contains("withdraw();"));
    }

    /// The add-side needle's SPELLING SET, closed against `wcore-mcp`.
    ///
    /// `every_runtime_mcp_add_joins_the_catalog_refresh` finds construction
    /// sites by the text `McpManager::connect`. Deriving the count from the
    /// defect closed the "a fourth bare path leaves the count at 2" hole, but
    /// it left a second one of exactly the same shape one level down: the
    /// needle is an ALLOWLIST OF SPELLINGS. A fifth constructor called
    /// `McpManager::from_configs` would be matched by nothing, every file that
    /// used it would count zero constructions, `built == 0` would `continue`,
    /// and the lint would stay green while the path it added had its
    /// tools/list_changed ignored for the life of the session — the original
    /// defect, reached through a rename.
    ///
    /// So the needle is not ASSERTED complete, it is CHECKED against the type
    /// it searches for: every associated function of `McpManager` that can
    /// hand a caller a new one must either be matched by the needle or be a
    /// `new_for_test*` fixture constructor. Adding a production constructor
    /// under any other name reddens this test, which is the point — the
    /// author is then made to extend the needle rather than silently escape
    /// it.
    ///
    /// THE PREDICATE IS INVERTED, and that is the whole point of round 4.
    /// Round 3 collected constructors by asking "does this signature RETURN a
    /// new one?", implemented as "is there a `Self` token right of the `->`".
    /// That question is not decidable over a closed alphabet: `-> McpManager`,
    /// `-> Result<McpManager, McpError>`, `-> Arc<Self>`, a type alias, are
    /// all ordinary Rust spellings of the same thing, and the first of them
    /// already occurs in this tree (`fn make_manager_with_servers(...) ->
    /// McpManager`). A verifier measured the escape directly: the round-3
    /// parser returned `[]` for an impl block whose two constructors named
    /// their own return type. So a fifth constructor spelled that way was
    /// invisible, every file using it counted zero constructions, `built == 0`
    /// would `continue`, and the add-side lint stayed green — the original
    /// defect reached through a rename, one level down from the last one.
    ///
    /// The question asked now is "does this associated fn take a `self`
    /// RECEIVER?", which is decidable and total: the receiver grammar is
    /// closed by the Rust language itself (`self`, `&self`, `&'a self`,
    /// `&mut self`, `mut self`, `self: Arc<Self>`), and it is written at the
    /// call site of the definition, not inferred from a type name. Everything
    /// in `impl McpManager` without one is an associated function, and every
    /// associated function must be matched by the needle, be a
    /// `new_for_test*` fixture, or be named here with its reason. Return
    /// types no longer enter into it, so no spelling of one can escape.
    ///
    /// GAP, recorded rather than implied away: this closes constructors on
    /// `McpManager` itself. It does not see a helper in another crate that
    /// builds a manager and hands it to `wcore-cli` already made, because the
    /// needle would then live in that crate's file and the walk is scoped to
    /// `wcore-cli/src`. That is residual, and it is stated in the #1175
    /// ledger.
    #[test]
    fn the_construction_needle_matches_every_way_to_get_an_mcp_manager() {
        let manager_src = include_str!("../../wcore-mcp/src/manager.rs");
        // Same fragment assembly as the walk, and the same reason.
        let needle_suffix = concat!("conn", "ect");

        let associated = receiverless_associated_fns(manager_src, "McpManager");

        // POSITIVE CONTROL on the parse. If the block finder or the receiver
        // detection silently stopped matching, `associated` would be empty and
        // the loop below would grade nothing.
        for known in ["connect_all", "connect_all_with_policy", "new_for_test"] {
            assert!(
                associated.iter().any(|name| name == known),
                "the McpManager associated-fn parse did not find {known} — it is \
                 grading an empty or truncated set. Found: {associated:?}"
            );
        }
        // NEGATIVE CONTROL on the same parse: a `&self` method must NOT be
        // collected, or "every associated fn is a constructor" is trivially
        // true of the whole impl and the assertion below means nothing.
        assert!(
            !associated.iter().any(|name| name == "server_names"),
            "server_names(&self) is a method, not an associated fn — the \
             receiver test is not discriminating. Found: {associated:?}"
        );

        for name in &associated {
            assert!(
                name.starts_with(needle_suffix) || name.starts_with("new_for_test"),
                "McpManager::{name} takes no self receiver, so it is a way to \
                 GET a manager, and it is not matched by the \
                 `McpManager::{needle_suffix}` needle that \
                 every_runtime_mcp_add_joins_the_catalog_refresh counts \
                 constructions with. A runtime-add path using it would count \
                 zero constructions and the lint would pass while its \
                 tools/list_changed was ignored for the life of the session \
                 (FerroxLabs/wayland#1175). Rename it, or widen the needle in \
                 both places."
            );
        }
    }

    /// Every associated fn of `impl <type>` in `source` that takes no `self`
    /// receiver, by name.
    ///
    /// Scoped to the inherent `impl <type> {` blocks at column zero — all of
    /// them, not just the first — so a trait impl or a different type's
    /// functions cannot be mistaken for this type's, and an inline
    /// `#[cfg(test)] mod tests` (indented) is invisible.
    fn receiverless_associated_fns(source: &str, type_name: &str) -> Vec<String> {
        let header = format!("impl {type_name} {{");
        let lines: Vec<&str> = source.lines().collect();
        let mut names = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            if lines[index].trim_end() != header {
                index += 1;
                continue;
            }
            let mut depth = 0i32;
            // The fn signature may wrap across lines, so the parameter list is
            // read from the header joined up to the line that opens the body.
            let mut pending: Option<(String, String)> = None;
            let mut cursor = index;
            while cursor < lines.len() {
                let line = lines[cursor];
                let code = code_before_comment(line);
                depth += code.matches('{').count() as i32;
                depth -= code.matches('}').count() as i32;

                if let Some((name, mut header)) = pending.take() {
                    header.push(' ');
                    header.push_str(code.trim());
                    if code.contains('{') || code.trim_end().ends_with(';') {
                        if !takes_self_receiver(&header) {
                            names.push(name);
                        }
                    } else {
                        pending = Some((name, header));
                    }
                } else if let Some(name) = associated_fn_name(code) {
                    let header = code.trim().to_string();
                    if code.contains('{') || code.trim_end().ends_with(';') {
                        if !takes_self_receiver(&header) {
                            names.push(name);
                        }
                    } else {
                        pending = Some((name, header));
                    }
                }

                if depth <= 0 && code.contains('}') {
                    break;
                }
                cursor += 1;
            }
            index = cursor + 1;
        }
        names
    }

    /// True when a joined fn signature declares a `self` receiver.
    ///
    /// Every `(` in the signature is tried rather than only the parameter
    /// list's, so a generic bound spelled `<F: Fn(u8)>` cannot shift the
    /// parameter list out from under this — no bound is followed by `self`.
    /// The receiver grammar itself is closed by the language, so this is
    /// total: `self`, `&self`, `&'a self`, `&mut self`, `&'a mut self`,
    /// `mut self`, `self: Arc<Self>`.
    fn takes_self_receiver(header: &str) -> bool {
        header.match_indices('(').any(|(at, _)| {
            let mut rest = header[at + 1..].trim_start();
            if let Some(stripped) = rest.strip_prefix('&') {
                rest = stripped.trim_start();
                if let Some(after_tick) = rest.strip_prefix('\'') {
                    rest = after_tick
                        .trim_start_matches(|ch: char| ch.is_alphanumeric() || ch == '_')
                        .trim_start();
                }
            }
            if let Some(stripped) = rest.strip_prefix("mut ") {
                rest = stripped.trim_start();
            }
            let token: String = rest
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                .collect();
            token == "self"
        })
    }

    /// The fn name on a `fn` item line, whatever visibility/asyncness it
    /// carries. `None` for anything that is not an fn item.
    fn associated_fn_name(line: &str) -> Option<String> {
        let mut rest = line.trim_start();
        loop {
            let mut advanced = false;
            for modifier in [
                "pub(crate)",
                "pub(super)",
                "pub",
                "async",
                "unsafe",
                "const",
            ] {
                if let Some(stripped) = rest.strip_prefix(modifier)
                    && stripped.starts_with(char::is_whitespace)
                {
                    rest = stripped.trim_start();
                    advanced = true;
                }
            }
            if !advanced {
                break;
            }
        }
        let rest = rest.strip_prefix("fn ")?;
        let name: String = rest
            .chars()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    }

    /// The parser is the thing that can silently stop finding constructors,
    /// so it is graded directly rather than trusted.
    ///
    /// The two `McpManager`-returning spellings in this fixture are the
    /// verifier's measured escape from round 3: against them, the old
    /// `-> Self` parse returned `[]`. They are kept as the first two
    /// constructors here so a reversion to a return-type test reddens.
    #[test]
    fn the_constructor_parse_sees_a_renamed_constructor() {
        let source = "\
impl McpManager {
    pub async fn from_parts(c: &C) -> Result<McpManager, McpError> {
        todo!()
    }

    pub fn build_it(c: &C) -> McpManager {
        todo!()
    }

    pub async fn connect_all(configs: &C) -> Result<Self, McpError> {
        todo!()
    }

    pub async fn from_configs(
        configs: &C,
        policy: P,
    ) -> Result<Self, McpError> {
        todo!()
    }

    pub fn new_for_test(entries: Vec<E>) -> Self {
        todo!()
    }

    pub fn server_names(&self) -> Vec<String> {
        todo!()
    }

    pub async fn call_tool<F: Fn(u8) -> u8>(&self, f: F) -> u8 {
        todo!()
    }

    pub fn take(mut self) -> Vec<E> {
        todo!()
    }
}
";
        let found = receiverless_associated_fns(source, "McpManager");
        assert_eq!(
            found,
            vec![
                "from_parts",
                "build_it",
                "connect_all",
                "from_configs",
                "new_for_test"
            ],
            "the parse must find a rustfmt-WRAPPED signature and EVERY return \
             spelling — `-> McpManager` and `-> Result<McpManager, _>` are the \
             two the `-> Self` parse it replaced returned nothing for — and \
             must not mistake a receiver-taking method for a constructor"
        );
        // The fixture must actually EXERCISE the failing branch, or the
        // guard above is graded against a source that could never redden it.
        assert!(
            !found
                .iter()
                .all(|name| name.starts_with("connect") || name.starts_with("new_for_test")),
            "control: the synthetic source must contain a constructor the \
             needle misses, or this proves nothing about the real check"
        );

        // NEGATIVE CONTROL on the block scoping: another type's constructors
        // must not be collected as McpManager's, or the guard grades the
        // wrong impl and a renamed McpManager constructor slips through.
        let other = "\
impl SomethingElse {
    pub fn build() -> Self {
        todo!()
    }
}
";
        assert!(
            receiverless_associated_fns(other, "McpManager").is_empty(),
            "a different type's impl block was collected"
        );

        // A SECOND inherent impl block of the same type must also be read —
        // splitting an impl in two is a refactor, not an escape hatch.
        let split = "\
impl McpManager {
    pub fn first() -> Self {
        todo!()
    }
}

impl McpManager {
    pub fn second() -> Self {
        todo!()
    }
}
";
        assert_eq!(
            receiverless_associated_fns(split, "McpManager"),
            vec!["first", "second"],
            "only the first `impl McpManager` block was read"
        );

        // NEGATIVE CONTROL on `takes_self_receiver`: every spelling of the
        // receiver grammar, and a bound whose parentheses come first.
        for method in [
            "pub fn health(&self) -> &HashMap<String, H> {",
            "pub fn take(self) -> Vec<E> {",
            "pub fn take(mut self) -> Vec<E> {",
            "pub fn edit(&mut self) {",
            "pub fn borrow<'a>(&'a self) -> &'a E {",
            "pub fn borrow_mut<'a>(&'a mut self) -> &'a mut E {",
            "pub fn shared(self: Arc<Self>) {",
            "pub async fn call<F: Fn(u8) -> u8>(&self, f: F) -> u8 {",
        ] {
            assert!(
                takes_self_receiver(method),
                "a receiver was missed, so this method would be graded as a \
                 constructor: {method}"
            );
        }
        for constructor in [
            "pub fn new_for_test(e: Vec<E>) -> Self {",
            "pub async fn connect_all(c: &C) -> Result<Self, McpError> {",
            "pub fn build_it(c: &C) -> McpManager {",
            "pub fn from_selfish(selfish: Selfish) -> McpManager {",
        ] {
            assert!(
                !takes_self_receiver(constructor),
                "a constructor was read as taking a receiver, so it would be \
                 invisible to the guard: {constructor}"
            );
        }
    }
}
