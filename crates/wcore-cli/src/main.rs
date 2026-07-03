use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

use clap::{Parser, Subcommand};
// `doctor` lives in the `wcore_cli` lib so the TUI diagnostics surface can
// share it; the binary re-imports it here for the `--doctor` CLI flag.
use wcore_cli::doctor;

// Force-link the plugin crates so their `inventory::submit!` factories
// fire at static-init time and `PluginLoader::discover` can find them.
// Without these `use ... as _;` references Rust's linker dead-code-strips
// the entire crate — including the `link_section` static items inventory
// relies on — because nothing in this binary names a symbol from them.
// Each crate registers itself via inventory; we never need a typed import.
// Removing any line silently disables the corresponding plugin in the
// shipped binary. Covered by `tests/plugin_discovery_e2e.rs`.
use wayland_browser as _;
use wayland_cua as _;
use wayland_honcho as _;
// Wave OL: typed import — `OllamaProvider` is downcast from
// `Arc<dyn PluginProvider>` in `make_plugin_provider_router` below to
// route `--model ollama:*` through the wayland-ollama plugin. The
// `inventory::submit!` factory still fires at static init for plugin
// discovery just like the other `as _` re-exports do.
use wayland_ollama::OllamaProvider;

use wcore_agent::bootstrap::{AgentBootstrap, PluginProviderRouter};
use wcore_agent::output::OutputSink;
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_agent::session;
use wcore_agent::slash::{Dispatcher as SlashDispatcher, SlashError, SlashOutcome};
use wcore_config::config::{self, CliArgs, Config, McpServerConfig, TransportType};
use wcore_mcp::manager::McpManager;
use wcore_mcp::tool_proxy::register_single_server_tools;
use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::events::{FinishReason, ProtocolEvent};
use wcore_protocol::reader::spawn_stdin_reader;
use wcore_protocol::writer::{ProtocolEmitter, ProtocolWriter};
use wcore_protocol::{ToolApprovalManager, ToolApprovalResult};
use wcore_providers::LlmProvider;

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
    SlashDispatcher::with_runtime(memory_api, plugin_handles, skill_catalog)
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
                output.emit_error(&format!("bad slash invocation: {reason}"), false);
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

    /// API key
    #[arg(short = 'k', long, env = "API_KEY")]
    api_key: Option<String>,

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

    /// #111 — the host's active assistant identity, for per-assistant MCP
    /// scoping. A config MCP server marked `only_for_assistant` is injected
    /// only when this matches its allow-list (fail-closed otherwise). Distinct
    /// from `--agent` (persona/system-prompt); the desktop host sets this when
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

    /// Force mode: approve every tool call without prompting. Only use for
    /// trusted, scripted runs — there is NO interactive permission gate
    /// once this is set. Equivalent to flipping the engine's session
    /// approval mode to `Force` for the entire run. The TUI surfaces a
    /// `· FORCE` badge in the bottom status bar so the mode is impossible
    /// to forget. Aliases: `--yolo`, `--dangerously-skip-permissions`.
    #[arg(long = "force", aliases = ["yolo", "dangerously-skip-permissions"])]
    force: bool,

    /// Project directory to load .wayland-core.toml from (defaults to CWD)
    #[arg(long)]
    project_dir: Option<std::path::PathBuf>,

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

    /// Print skill directory paths and exit
    #[arg(long)]
    skills_path: bool,

    /// W5 (A.5): run the system-dependency doctor. Probes external
    /// binaries (`wlrctl`, `grim`, `chromium`, `ollama`), environment
    /// signals (`WAYLAND_DISPLAY`, `DISPLAY`, `BROWSERBASE_API_KEY`,
    /// `OLLAMA_BASE_URL`), and surfaces missing dependencies with
    /// per-distro install hints. Exit code `1` if any required check
    /// fails on the current platform, otherwise `0`.
    #[arg(long)]
    doctor: bool,

    /// A4b: when running --doctor, actually CONNECT-TEST each declared MCP
    /// server (spawns stdio commands / dials URLs) instead of only listing
    /// them. Off by default so bare --doctor stays side-effect-free.
    #[arg(long, requires = "doctor")]
    probe_mcp: bool,

    /// W4 F19: run the skills audit. Writes JSON to
    /// .wayland-core/skills-audit.json and renders Markdown to stdout.
    #[arg(long)]
    skills_audit: bool,

    /// Override the staleness threshold (days) used by --skills-audit.
    // F-072: `requires` ensures clap rejects --skills-audit-stale-days
    // when --skills-audit is absent, matching --replay-diff behaviour.
    #[arg(long, default_value_t = 180, requires = "skills_audit")]
    skills_audit_stale_days: u64,

    /// W9.1 T4 (T11): promote a P4 procedure (drafted skill) from
    /// `Staged` → `Active`. The argument is the procedure's UUID as
    /// emitted in `skill_drafted` TraceEvents (or as listed by
    /// internal tooling). Reads + writes the project's
    /// `.wayland-core/memory/memory.db`.
    #[arg(long, value_name = "PROCEDURE_ID")]
    skills_promote: Option<String>,

    /// W9.1 T4 (T11): archive a P4 procedure. Accepts either a
    /// `Staged` or `Active` row (W9 T0.5 amendment to the
    /// state-machine allows `Staged → Archived` directly so curators
    /// can dismiss losing drafts without a detour through Active).
    /// Pinned rows are NOT archivable from the CLI — promote → archive
    /// or unpin them through the curator UI first.
    #[arg(long, value_name = "PROCEDURE_ID")]
    skills_archive: Option<String>,

    /// M3.4: dump the memory state for a given session id. Prints all
    /// episodes scoped to that session at the session+project tiers,
    /// plus all project-tier facts and procedures. Intended for human
    /// inspection; the format is a plain text table (not JSON) and may
    /// change between releases. Exits 0 even if the session has no
    /// recorded data so scripts can probe state without try/catch.
    #[arg(long, value_name = "SESSION_ID")]
    memory_show: Option<String>,

    /// M5.2: replay a session trace JSON file. Validates schema + the
    /// version-skew guard (refuses traces recorded by a different
    /// wcore-core build unless --replay-force-version-skew is set).
    /// Prints the event count for the session. Combine with
    /// --replay-diff to surface the first divergence against another
    /// trace.
    #[arg(long, value_name = "TRACE_PATH")]
    replay: Option<std::path::PathBuf>,

    /// M5.2: compare the trace passed to --replay against this second
    /// trace and print the changed/added/removed entries.
    #[arg(long, value_name = "OTHER_TRACE_PATH", requires = "replay")]
    replay_diff: Option<std::path::PathBuf>,

    /// M5.2: skip the wcore-version guard in --replay (use only when
    /// inspecting traces from another release on purpose).
    #[arg(long, requires = "replay")]
    replay_force_version_skew: bool,

    /// Output compaction level: off, safe (default), full
    #[arg(long)]
    compaction: Option<String>,

    /// Enable TOON encoding for JSON arrays (session-level, cannot change mid-conversation)
    #[arg(long)]
    toon: bool,

    /// F-092 (W7-N): enable live online evolution. At session-end the engine
    /// emits one `evolution_event` and applies the Paraphrase mutator to
    /// successful trajectories (≥50% of turns had tool calls). Evolved
    /// system-prompt variants are persisted to `$WAYLAND_HOME/evolved/`.
    /// Equivalent to `[observability] online_evolution = true` in config.
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

    /// FluxRouter web_search grounding (contract §5): attach a server-side
    /// `web_search` tool to every turn so Flux grounds the answer via
    /// Perplexity Sonar and renders citations. Only fires when the active
    /// model is a Flux tier alias (`flux-auto` / `flux-fast` / `flux-standard`
    /// / `flux-reasoning`) — a no-op on other models. Paid-only on Flux.
    #[arg(long)]
    search: bool,

    /// Initial prompt (if omitted, enters interactive REPL mode)
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,

    /// M5.4: optional subcommand (currently `plugin`). When present
    /// this short-circuits the agent/REPL path and runs the subcommand
    /// dispatcher instead. Kept optional so every existing flag-driven
    /// invocation (`wayland-core --doctor`, `wayland-core "prompt"`,
    /// REPL, json-stream) keeps working unchanged.
    #[command(subcommand)]
    command: Option<TopCmd>,
}

/// M5.4: top-level subcommands. We add new subcommands here as the CLI
/// grows.
#[derive(Subcommand)]
enum TopCmd {
    /// F-089: model catalog commands.
    Models {
        #[command(subcommand)]
        cmd: ModelsCmd,
    },
    /// Manage installed plugins (install / list / available / remove).
    Plugin(wcore_cli::plugin::PluginArgs),
    /// v0.6.4 Task 2.4: serve the engine's tool registry as an MCP server
    /// (stdio or SSE transport). Used by external MCP clients like Claude
    /// Desktop, mcp-cli, etc. to call wayland-core's tools.
    McpServe(wcore_cli::mcp_serve::McpServeArgs),
    /// v0.6.4 Task 2.6: dispatch a worktree-isolated worker swarm.
    Swarm(wcore_cli::swarm::SwarmArgs),
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
    /// v0.7.0 Task 1.C.1: print resolved project context from WAYLAND.md /
    /// AGENTS.md / .wayland/context.md / CLAUDE.md walking up from cwd.
    ProjectContext,
    /// v0.7.0 1.B.2: scaffold .wayland/config.toml + WAYLAND.md in cwd.
    Init(wcore_cli::init::InitArgs),
    /// v0.7.0 1.A.10: ACP server + client surface. `acp serve`
    /// binds the HTTP/SSE transport; `acp request` drives a one-shot
    /// session/message round-trip.
    Acp(wcore_cli::acp::AcpArgs),
    /// v0.7.0 3.B.2: manage user-defined agents (create, list, show,
    /// edit, delete). Built-ins from the bundled pack are read-only.
    Agent {
        #[command(subcommand)]
        cmd: wcore_cli::agent_cmd::AgentCmd,
    },
    /// v0.8.1 U7: manage scheduled cron jobs (add / list / remove /
    /// enable / disable). Persists to `$WAYLAND_HOME/cron/jobs.json`;
    /// the background runner spawned at session boot picks up changes
    /// on its next tick.
    Cron {
        #[command(subcommand)]
        cmd: wcore_cli::cron::CronCmd,
    },
    /// v0.8.1 U9: update wayland-core to the latest signed release
    /// from `FerroxLabs/wayland-core`. Verifies the `.sig` artifact
    /// against the pinned marketplace pubkey (ed25519) before atomic
    /// swap. Use `--check-only` to print versions without installing.
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
    /// Manage isolated profiles — each is an independent `WAYLAND_HOME`-rooted
    /// home with its own config, credentials, memory, and skills.
    Profile {
        #[command(subcommand)]
        cmd: wcore_cli::profile::ProfileCmd,
    },
}

/// F-089: `models` sub-subcommands.
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

/// v0.9.1 W2 cycle-2 HIGH 2: open the TUI-mode tracing log file in
/// append mode. Lives under `$WAYLAND_HOME/logs/wayland-core.log`, with
/// `~/.wayland/logs/` as the platform default. The parent directory is
/// created lazily; any error is surfaced to the caller which falls back
/// to stderr (better than no traces at all).
fn open_tui_log_file() -> std::io::Result<std::fs::File> {
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
    let log_dir = base.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("wayland-core.log"))
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
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            runtime.block_on(run())
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

async fn run() -> anyhow::Result<ExitCode> {
    let cli = Cli::parse();

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
    let tui_log_file: Option<std::fs::File> = if will_enter_tui {
        open_tui_log_file().ok()
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
        let _ = fmt()
            .with_env_filter(env_filter)
            .with_writer(non_blocking)
            .with_target(false)
            .try_init();
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
        // R2 fix A5: probe + arm via a SINGLE fs::write (was: probe via
        // arm() then write again via new() — double-write that could
        // discard a successful first write on second-write failure).
        let was_dirty = wcore_cli::crash_sentinel::CrashSentinel::check_dirty(&sentinel_path);
        if was_dirty {
            if will_enter_tui {
                tracing::warn!(
                    path = %sentinel_path.display(),
                    "previous run did not shut down cleanly (crash sentinel found)"
                );
            } else {
                eprintln!(
                    "wayland-core: warning: previous run did not shut down cleanly \
                     (crash sentinel found at {})",
                    sentinel_path.display()
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

    // F-062: install signal handlers (SIGTERM / SIGINT / SIGHUP) that
    // remove the crash sentinel before exit so the next restart does NOT
    // falsely report a crash. Without these handlers, SIGTERM from the OS
    // (e.g. `kill`, systemd, launchd) bypasses Drop and leaves the flag
    // behind, causing every restart to claim the prior run crashed.
    //
    // Implementation: spawn a background task that waits for any of the
    // three signals, removes the sentinel file best-effort, and calls
    // std::process::exit(0). The tokio runtime is shut down by exit() so
    // all other tasks are cancelled; the sentinel file removal is the
    // only ordering-critical step.
    #[cfg(unix)]
    {
        let sentinel_path_for_sig = wcore_cli::crash_sentinel::CrashSentinel::default_path();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler install");
            let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler install");
            let mut hup = signal(SignalKind::hangup()).expect("SIGHUP handler install");
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv()  => {}
                _ = hup.recv()  => {}
            }
            // Best-effort: remove the sentinel so the next restart
            // doesn't falsely report a crash.
            let _ = std::fs::remove_file(&sentinel_path_for_sig);
            std::process::exit(0);
        });
    }
    // Audit W-2 fix (E2E-WINDOWS-ADDENDUM-2026-05-24 §2.2):
    // On Windows there is no SIGTERM/SIGINT/SIGHUP — the previous code had
    // no #[cfg(not(unix))] branch, so the sentinel was never cleaned up when
    // the Electron app (or OS) killed the engine process via TerminateProcess.
    // This caused every Windows restart to falsely report a crash.
    // Ctrl+C is the closest Windows equivalent an external manager can send
    // before falling back to TerminateProcess.
    #[cfg(not(unix))]
    {
        let sentinel_path_for_sig = wcore_cli::crash_sentinel::CrashSentinel::default_path();
        tokio::spawn(async move {
            // On Windows, tokio::signal::ctrl_c() fires on Ctrl+C (CTRL_C_EVENT
            // and CTRL_BREAK_EVENT). This is what the Electron wrapper sends
            // before a hard kill.
            let _ = tokio::signal::ctrl_c().await;
            let _ = std::fs::remove_file(&sentinel_path_for_sig);
            std::process::exit(0);
        });
    }

    // F-086: register a cleanup guard for the per-process bundled-skill
    // extraction directory (`$TMPDIR/wayland-core-bundled-skills-{pid}/`).
    // The guard's Drop impl calls `cleanup_bundled_skill_extract_dir()` so
    // the temp directory is removed on both graceful exit and panic unwind,
    // preventing accumulation across restarts.
    struct BundledSkillTmpCleanup;
    impl Drop for BundledSkillTmpCleanup {
        fn drop(&mut self) {
            wcore_skills::bundled::cleanup_bundled_skill_extract_dir();
        }
    }
    let _bundled_skill_cleanup = BundledSkillTmpCleanup;

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
                run_tui_mode(config, &cwd, None, None, true, false, false).await?;
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
            // `profile::run` is synchronous — no `.await` (mirrors `TopCmd::Plugin`).
            TopCmd::Profile { cmd } => match wcore_cli::profile::run(cmd) {
                Ok(()) => Ok(ExitCode::SUCCESS),
                Err(e) => {
                    eprintln!("wayland-core profile: {e:#}");
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
        return Ok(doctor::run(cli.probe_mcp).await);
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

    let terminal = Arc::new(TerminalSink::new(cli.no_color));
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
        if sessions.is_empty() {
            eprintln!("No saved sessions.");
        } else {
            eprintln!(
                "{:<8} {:<12} {:<30} {:>5}  Summary",
                "ID", "Date", "Model", "Msgs"
            );
            for s in &sessions {
                eprintln!(
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

    let mut config = match Config::resolve(&cli_args) {
        Ok(c) => c,
        Err(e) => {
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
            if would_open_tui && (missing_credentials || (first_run && !project_config_exists())) {
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
                run_tui_mode(
                    onboarding_config,
                    &cwd,
                    None,
                    cli.session_id.clone(),
                    true,
                    cli.force,
                    cli.search,
                )
                .await?;
                if let Some(ref mut g) = _sentinel_guard {
                    let _ = g.disarm();
                }
                return Ok(ExitCode::SUCCESS);
            }
            if cli.json_stream {
                // #186: a json-stream host (desktop app) otherwise sees only a bare exit
                // code and shows a generic "wcore exited with code 1 during init". Emit a
                // structured error event so the real, actionable reason reaches the host UI.
                let w = wcore_protocol::writer::ProtocolWriter::new();
                let _ = w.emit(&wcore_protocol::events::ProtocolEvent::Error {
                    msg_id: None,
                    error: wcore_protocol::events::ErrorInfo {
                        code: "init_failed".to_string(),
                        message: init_failure_message(&e, &provider_label_for_error),
                        retryable: false,
                    },
                });
            }
            return Err(e);
        }
    };

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

    // Branch to JSON stream mode
    if cli.json_stream {
        run_json_stream_mode(
            config,
            &cwd,
            resume,
            cli.session_id,
            cli.force,
            cli.assistant.clone(),
        )
        .await?;
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
            resume,
            cli.session_id,
            false,
            cli.force,
            cli.search,
        )
        .await?;
        // B3: disarm the crash sentinel at the earliest known-clean point.
        // The Drop impl on `_sentinel_guard` also fires when `main` returns,
        // but an explicit early disarm closes the window between TUI exit
        // and any post-TUI cleanup (MCP shutdown etc.) where a signal-based
        // `process::exit` could bypass Drop.
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
        eprintln!(
            "wayland-core: stdin is not a terminal and no prompt was given.\n\
             Use --json-stream for headless/piped use, or pass a prompt with -p."
        );
        return Ok(ExitCode::FAILURE);
    }

    let provider_name = config.provider_label.clone();

    // Bootstrap engine with full feature initialization. Phase 1B-2 — this
    // build backs both the long-running interactive line-REPL and the
    // headless one-shot `-p` path; opt into inbound channel dispatch so the
    // primary interactive session listens to configured channels (the
    // short-lived one-shot simply aborts the subscriber on exit).
    let mut bootstrap = AgentBootstrap::new(config, &cwd, output.clone())
        .plugin_provider_router(make_plugin_provider_router())
        .enable_inbound_dispatch(true);

    if let Some(resume_id) = &resume {
        let cfg = bootstrap.config();
        let session_mgr = session::SessionManager::new(
            cfg.session.directory.clone().into(),
            cfg.session.max_sessions,
        );
        let session = session_mgr.load(resume_id)?;
        terminal.formatter().session_info(&format!(
            "Resumed session {} ({} messages, {} model)",
            session.id,
            session.messages.len(),
            session.model
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
                ExitCode::SUCCESS
            }
            SlashOrRun::Engine(Err(e)) => {
                // Render the full anyhow chain (`{e:#}` flattens causes onto
                // `\nCaused by: …` lines which the formatter recognises).
                output.emit_error(&format!("{e:#}"), false);
                ExitCode::FAILURE
            }
        }
    };

    engine.run_stop_hooks().await;

    for mgr in &result.mcp_managers {
        mgr.shutdown().await;
    }

    Ok(exit_code)
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
                output.emit_error(&format!("{e:#}"), false);
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
/// Map the persisted config approval posture to the engine's runtime
/// `SessionMode`. Keeps `wcore-config` decoupled from `wcore-protocol`.
fn approval_mode_to_session(
    mode: wcore_config::config::ApprovalMode,
) -> wcore_protocol::commands::SessionMode {
    use wcore_protocol::commands::SessionMode;
    match mode {
        wcore_config::config::ApprovalMode::Default => SessionMode::Default,
        wcore_config::config::ApprovalMode::AutoEdit => SessionMode::AutoEdit,
        wcore_config::config::ApprovalMode::Force => SessionMode::Force,
    }
}

/// Boot-time approval posture from local trust sources: the config's
/// `[default] approval_mode`, with `--force` (or `WAYLAND_ALLOW_WIRE_FORCE`,
/// surfaced by the caller as `force`) overriding to `Force`. This is the
/// LOCAL operator's posture — distinct from a wire-originated mode change,
/// which goes through the GHSA-8r7g-gated `set_mode_from_wire`.
///
/// wayland#241: both the TUI (`run_tui_mode`) and json-stream
/// (`run_json_stream_mode`) paths seed through this one helper so they can't
/// drift — the json-stream path previously skipped the config seed entirely,
/// silently ignoring a user's `approval_mode = "auto-edit"` / `"force"`.
fn initial_session_mode(
    config_mode: wcore_config::config::ApprovalMode,
    force: bool,
) -> wcore_protocol::commands::SessionMode {
    if force {
        wcore_protocol::commands::SessionMode::Force
    } else {
        approval_mode_to_session(config_mode)
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

async fn run_tui_mode(
    config: Config,
    cwd: &str,
    resume: Option<String>,
    session_id: Option<String>,
    force_onboarding: bool,
    force: bool,
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
    // it is moved into the bootstrap. `--force` is reflected on the view
    // so the status bar can paint the `· FORCE` badge.
    let mut config_view = tui::config_view_from(&config);
    config_view.force = force;
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

    // First-run gate: a true first run has no global config file yet, so
    // the TUI opens on the Onboarding surface. A returning user lands on
    // the Workspace.
    let first_run = !config::global_config_path().exists();

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
    approval_manager.set_allow_wire_force(force || wire_force_opt_in_env());
    // Seed the initial approval posture from config (`[default] approval_mode`,
    // editable via /config); `--force` overrides to Force. When Force, the
    // TUI's approval modal never opens (no `ApprovalRequired` event) and the
    // status bar renders the FORCE badge so the mode is visible.
    approval_manager.set_mode(initial_session_mode(config.approval_mode, force));

    // Phase 1B-2 — the interactive TUI is a primary long-running session, so
    // opt into inbound channel dispatch (the InboundSubscriber turns admitted
    // channel messages into agent turns for the lifetime of this session).
    let mut bootstrap = AgentBootstrap::new(config, cwd, output.clone())
        .plugin_provider_router(make_plugin_provider_router())
        .enable_inbound_dispatch(true);

    if let Some(resume_id) = &resume {
        let cfg = bootstrap.config();
        let session_mgr = session::SessionManager::new(
            cfg.session.directory.clone().into(),
            cfg.session.max_sessions,
        );
        let session = session_mgr.load(resume_id)?;
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
    engine.set_approval_manager(approval_manager.clone());
    // Wave 6 #24 — use the dedupe variant so a self-gating engine site (the
    // live-workflow gate emits `ToolRequest` + its own `ApprovalRequired`)
    // yields exactly ONE gate frame, not the synthesized + explicit pair (which
    // double-rang the terminal bell on the TUI and is malformed on ACP).
    engine.set_protocol_writer(Arc::new(tui::ChannelEmitter::with_dedupe(
        tx.clone(),
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        Some(engine.approval_bridge().clone()),
    )));

    // Snapshot the loaded skills + MCP servers for the `/skills` and `/mcp`
    // listings. Taken here while `engine` and `result.mcp_managers` are still
    // owned (the engine moves into the controller on the next line). The
    // dispatch path is synchronous, so it reads this rather than locking the
    // engine's async mutex on the render thread.
    let skills_snapshot: Vec<tui::SkillInfo> = engine
        .skill_catalog()
        .map(|cat| {
            cat.visible()
                .map(|r| tui::SkillInfo {
                    name: r.name.clone(),
                    description: r.description.clone(),
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
    let mut tui_engine = tui::TuiEngine::new(engine, approval_manager, tx);
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

/// W9.1 T4 (T11): promote a Staged or Pinned procedure to Active. Opens
/// the project's `.wayland-core/memory/memory.db` rooted at CWD, looks
/// up the procedure by id at `Tier::Project`, validates the transition
/// against the state-machine, and writes the new status via
/// `upsert_procedure` (same row, same id, same artifact — only `status`
/// changes).
///
/// F-069: after the DB transition, also attempts to copy the on-disk auto-
/// draft from `<config_dir>/wayland-core/skills/<name>/` (where
/// `SkillDrafter` writes it) to `<config_dir>/wayland-core/skills/<name>/`
/// (already there — the draft IS at the loader-visible path post F-038).
/// For backwards-compat with old drafts written to WAYLAND_HOME, we also
/// check `<WAYLAND_HOME>/skills/auto/<name>/SKILL.md` and copy it to the
/// loader-visible path if the config-dir copy is missing.
async fn run_skills_promote(id: &str) -> anyhow::Result<()> {
    transition_procedure(
        id,
        wcore_memory::v2_types::ProcedureStatus::Active,
        "promote",
    )
    .await?;

    // F-069: best-effort migration of old WAYLAND_HOME-format drafts into
    // the loader-visible config-dir path. Runs after the DB transition so
    // a DB failure does not trigger a misleading disk move.
    try_migrate_draft_to_loader_path(id).await;
    Ok(())
}

/// F-069: if a draft exists at the legacy `$WAYLAND_HOME/skills/auto/<name>/SKILL.md`
/// location but not at `<config_dir>/wayland-core/skills/<name>/SKILL.md`,
/// copy it to the loader-visible path. Best-effort — failure is logged, not
/// returned.
async fn try_migrate_draft_to_loader_path(_procedure_id: &str) {
    // Derive candidate skill name from procedure id (heuristic: auto-<sig>).
    // We check both patterns in case the operator passed a UUID vs. a name.
    let Some(config_dir) = wcore_config::config::app_config_dir() else {
        return;
    };
    let wayland_home = std::env::var("WAYLAND_HOME")
        .map(std::path::PathBuf::from)
        .ok()
        .or_else(|| dirs::home_dir().map(|h| h.join(".wayland")))
        .unwrap_or_else(|| std::path::PathBuf::from(".wayland"));

    let legacy_auto_dir = wayland_home.join("skills").join("auto");
    if !legacy_auto_dir.is_dir() {
        return;
    }

    // Walk legacy auto dir looking for a directory whose name contains the
    // procedure_id suffix (or starts with "auto-").
    let read_dir = match std::fs::read_dir(&legacy_auto_dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for entry in read_dir.flatten() {
        let skill_name = entry.file_name().to_string_lossy().into_owned();
        let src_skill_md = entry.path().join("SKILL.md");
        if !src_skill_md.exists() {
            continue;
        }
        let dst_skill_dir = config_dir.join("skills").join(&skill_name);
        let dst_skill_md = dst_skill_dir.join("SKILL.md");
        if dst_skill_md.exists() {
            // Already at loader-visible path — nothing to migrate.
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(&dst_skill_dir)
            .and_then(|_| std::fs::copy(&src_skill_md, &dst_skill_md).map(|_| ()))
        {
            tracing::warn!(
                "F-069: failed to migrate draft {} to loader path: {}",
                skill_name,
                e
            );
        } else {
            println!(
                "info: migrated draft '{skill_name}' to loader-visible path at {}",
                dst_skill_dir.display()
            );
        }
    }
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
fn mcp_ready_events_for(mgr: &McpManager) -> Vec<ProtocolEvent> {
    let mut per_server: HashMap<String, Vec<String>> = HashMap::new();
    // Seed the map with every connected server so tool-less servers
    // still produce an empty-tools `McpReady`, matching the dynamic
    // `AddMcpServer` path (which always emits one event per
    // successfully-connected server regardless of tool count).
    for name in mgr.server_names() {
        per_server.entry(name).or_default();
    }
    for (server_name, tool) in mgr.all_tools() {
        per_server
            .entry(server_name.to_string())
            .or_default()
            .push(tool.name.clone());
    }
    let mut names: Vec<String> = per_server.keys().cloned().collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let tools = per_server.remove(&name).unwrap_or_default();
            ProtocolEvent::McpReady { name, tools }
        })
        .collect()
}

/// wayland#551 — companion to [`mcp_ready_events_for`]: one `McpFailed`
/// event per server whose connect failed or timed out, so a host whose
/// config MCP servers were dialed in the background still learns WHY a
/// server's tools never appeared (parity with the dynamic `AddMcpServer`
/// path's failure emit). Pure and name-sorted for deterministic tests.
fn mcp_failed_events_for(mgr: &McpManager) -> Vec<ProtocolEvent> {
    use wcore_mcp::manager::McpServerHealth;
    let mut entries: Vec<(&String, &McpServerHealth)> = mgr.health().iter().collect();
    entries.sort_by_key(|(name, _)| name.as_str());
    entries
        .into_iter()
        .filter_map(|(name, health)| {
            let reason = match health {
                McpServerHealth::Failed { reason } => reason.clone(),
                McpServerHealth::TimedOut { after } => {
                    format!("connect timed out after {}s", after.as_secs())
                }
                McpServerHealth::Skipped { reason } => reason.clone(),
                McpServerHealth::Ready { .. } => return None,
            };
            Some(ProtocolEvent::McpFailed {
                name: name.clone(),
                reason,
            })
        })
        .collect()
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
fn note_deferred_mcp_connect(
    outcome: Result<McpManager, wcore_mcp::transport::McpError>,
    resolved: HashMap<String, McpServerConfig>,
    engine: &mut wcore_agent::engine::AgentEngine,
    writer: &ProtocolWriter,
    output: &Arc<dyn OutputSink>,
    dynamic_managers: &mut Vec<Arc<McpManager>>,
) -> Option<(Arc<McpManager>, HashMap<String, McpServerConfig>)> {
    match outcome {
        Ok(mgr) => {
            let mgr = Arc::new(mgr);
            if integrate_deferred_mcp(engine, mgr.clone(), &resolved, writer, dynamic_managers) {
                None
            } else {
                Some((mgr, resolved))
            }
        }
        Err(e) => {
            output.emit_error(&format!("MCP initialization error: {e}"), false);
            None
        }
    }
}

/// Returns `false` when the registry Arc is currently borrowed (a turn is
/// mid-flight) — the caller parks the manager and retries at the next
/// between-turns boundary.
fn integrate_deferred_mcp(
    engine: &mut wcore_agent::engine::AgentEngine,
    mgr: Arc<McpManager>,
    resolved_servers: &HashMap<String, McpServerConfig>,
    writer: &ProtocolWriter,
    dynamic_managers: &mut Vec<Arc<McpManager>>,
) -> bool {
    let builtin_names = engine.tool_names();
    let Some(reg) = engine.registry_mut() else {
        return false;
    };
    wcore_mcp::tool_proxy::register_mcp_tools(reg, &mgr, &builtin_names, resolved_servers);
    for event in mcp_ready_events_for(&mgr) {
        let _ = writer.emit(&event);
    }
    for event in mcp_failed_events_for(&mgr) {
        let _ = writer.emit(&event);
    }
    dynamic_managers.push(mgr);
    true
}

fn to_mcp_server_config(
    transport: &str,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<HashMap<String, String>>,
    url: Option<String>,
    headers: Option<HashMap<String, String>>,
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
        allow_local: false,
        only_for_assistant: None,
    })
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
/// actually needs human approval — auto-approved categories and allow-listed
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
    inner: Arc<ProtocolWriter>,
    /// The live approval posture. The gate frame is synthesized ONLY when the
    /// tool will actually be parked (`!is_auto_approved`); under Force (or for
    /// an auto-approved category) the engine auto-runs the tool, so emitting an
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
        inner: Arc<ProtocolWriter>,
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
            // Under Force (or an auto-approved category) the engine auto-runs
            // the tool, so a gate frame here would be a false gate.
            if !self.approval.is_auto_approved(reason) {
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

async fn run_json_stream_mode(
    config: Config,
    cwd: &str,
    resume: Option<String>,
    session_id: Option<String>,
    force: bool,
    assistant: Option<String>,
) -> anyhow::Result<()> {
    let writer = Arc::new(ProtocolWriter::new());

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
    let protocol_sink = Arc::new(
        ProtocolSink::new(writer.clone())
            .with_structured_traces(config.observability.structured_traces)
            .with_advertised_capabilities(advertised_for_sink)
            // v0.9.4 W1.2 (F2): enable sub-agent event relay to the Desktop
            // host. Harmless when no sub-agents spawn (no-op emission path).
            .with_sub_agent_traces(true),
    );
    let approval_manager = Arc::new(ToolApprovalManager::new());
    // GHSA-8r7g: a protocol peer may escalate to Force only when this local
    // operator opted in at launch (--force or WAYLAND_ALLOW_WIRE_FORCE).
    approval_manager.set_allow_wire_force(force || wire_force_opt_in_env());
    // wayland#241: seed the initial approval posture from config
    // (`[default] approval_mode`) via the shared `initial_session_mode`
    // helper, exactly like `run_tui_mode`. The json-stream path previously
    // only honored `--force`, so a config `approval_mode = "auto-edit"` /
    // `"force"` was silently ignored for the desktop host — every mutating
    // tool then waited on an approval the host never sent. `--force` still
    // overrides to Force (F-002).
    approval_manager.set_mode(initial_session_mode(config.approval_mode, force));
    let output: Arc<dyn OutputSink> = protocol_sink.clone();

    let provider_name = config.provider_label.clone();

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
    let deferred_mcp_servers = if scoped_config_servers.is_empty() {
        None
    } else {
        Some(match config.open_credentials_store() {
            Ok(store) => wcore_config::mcp_cred_refs::resolve_servers_for_connect(
                &scoped_config_servers,
                &*store,
            ),
            Err(_) => scoped_config_servers,
        })
    };

    // Bootstrap engine with full feature initialization. Phase 1B-2 —
    // json-stream is a primary long-running host session (e.g. the Wayland
    // desktop app), so
    // opt into inbound channel dispatch.
    let mut bootstrap = AgentBootstrap::new(config, cwd, output.clone())
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
        let session = session_mgr.load(resume_id)?;
        bootstrap = bootstrap.resume(session);
    }

    let result = match bootstrap.build().await {
        Ok(r) => r,
        Err(e) => {
            // #186: surface init failure to the json-stream host instead of a bare exit.
            output.emit_error(&init_failure_message(&e, &provider_name), false);
            return Err(e);
        }
    };
    let mut engine = result.engine;
    // wayland#551 — declared-but-still-connecting servers count as MCP
    // capability on the ready frame; their tools register shortly after.
    let initial_has_mcp = result.has_mcp || deferred_mcp_servers.is_some();
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
    protocol_sink.emit_ready_with_plugins(
        engine.compat(),
        initial_has_mcp,
        sid,
        &approval_manager.current_mode(),
        initial_has_plugins,
        &initial_plugin_caps,
        engine.advertised_capabilities(),
    );

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
        for event in mcp_ready_events_for(mgr) {
            let _ = writer.emit(&event);
        }
    }

    // wayland#551 — dial the deferred config MCP servers in the background,
    // AFTER ready is on the wire. The command loop integrates the manager
    // into the live engine between turns (see the select below) and emits
    // McpReady / McpFailed per server, exactly like the dynamic
    // `AddMcpServer` path. A turn that starts before the connects settle
    // simply runs without the MCP tools — the old behavior was a chat that
    // never opened at all.
    let mut deferred_mcp_rx = deferred_mcp_servers.map(|resolved| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let outcome = McpManager::connect_all(&resolved).await;
            let _ = tx.send((outcome, resolved));
        });
        rx
    });

    engine.set_approval_manager(approval_manager.clone());
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

    let mut cmd_rx = spawn_stdin_reader();

    // --- Pre-message phase: accept AddMcpServer commands ---
    let mut dynamic_managers: Vec<Arc<McpManager>> = Vec::new();
    let mut first_cmd: Option<ProtocolCommand> = None;

    // wayland#551 — a deferred-MCP manager whose integration found the
    // registry borrowed; retried at the next between-turns boundary.
    let mut pending_deferred_mcp: Option<(Arc<McpManager>, HashMap<String, McpServerConfig>)> =
        None;

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
                if let Ok((outcome, resolved)) = res {
                    pending_deferred_mcp = note_deferred_mcp_connect(
                        outcome,
                        resolved,
                        &mut engine,
                        &writer,
                        &output,
                        &mut dynamic_managers,
                    );
                }
                continue;
            }
        };
        match cmd {
            ProtocolCommand::AddMcpServer {
                name,
                transport,
                command,
                args,
                env,
                url,
                headers,
            } => {
                eprintln!(
                    "[mcp] AddMcpServer received: name={name}, transport={transport}, command={command:?}"
                );
                // #135: idempotency — re-adding an already-connected server (boot
                // set or a prior dynamic add) does not spawn a duplicate. Re-emit
                // the existing tools so the host's view stays consistent, then skip
                // the reconnect. NOTE: a re-add with changed config is ignored (the
                // existing connection is kept) — remove then add to reconfigure.
                if engine.mcp_server_connected(&name) {
                    let existing: Vec<String> = engine
                        .tools()
                        .to_tool_defs()
                        .iter()
                        .filter(|t| t.server.as_deref() == Some(name.as_str()))
                        .map(|t| t.name.clone())
                        .collect();
                    eprintln!(
                        "[mcp] '{name}' already connected ({} tools); keeping existing \
                         connection, ignoring re-add (remove then add to change config)",
                        existing.len()
                    );
                    let _ = writer.emit(&ProtocolEvent::McpReady {
                        name,
                        tools: existing,
                    });
                    continue;
                }
                let config =
                    match to_mcp_server_config(&transport, command, args, env, url, headers) {
                        Ok(c) => c,
                        Err(e) => {
                            output.emit_error(&format!("AddMcpServer '{name}': {e}"), false);
                            continue;
                        }
                    };

                let mut single_configs = HashMap::new();
                single_configs.insert(name.clone(), config.clone());
                eprintln!("[mcp] Connecting to '{name}'...");
                match McpManager::connect_all(&single_configs).await {
                    Ok(mgr) => {
                        let tool_names: Vec<String> = mgr
                            .all_tools()
                            .iter()
                            .map(|(_, t)| t.name.clone())
                            .collect();
                        eprintln!("[mcp] Connected to '{name}': {} tools", tool_names.len());
                        let mgr_arc = Arc::new(mgr);
                        let builtin_names = engine.tool_names();
                        // Wave OR: `registry_mut` returns `Option` because
                        // the registry is now Arc-shared. At this CLI boot
                        // site the engine is not running so the refcount
                        // is 1 and `Arc::get_mut` succeeds. Defensive log
                        // (not panic) keeps the dynamic-MCP add-path
                        // resilient if a future change leaks a clone.
                        match engine.registry_mut() {
                            Some(reg) => register_single_server_tools(
                                reg,
                                &mgr_arc,
                                &name,
                                &builtin_names,
                                config.deferred.unwrap_or(true),
                            ),
                            None => {
                                eprintln!(
                                    "[mcp] cannot register tools for '{name}': registry is currently borrowed"
                                );
                                // "registry busy" is a transient lock-contention condition — a
                                // re-issue moments later can succeed, so this one IS retryable.
                                output.emit_error(
                                    &format!("AddMcpServer '{name}': registry busy"),
                                    true,
                                );
                                continue;
                            }
                        }
                        dynamic_managers.push(mgr_arc);
                        let _ = writer.emit(&ProtocolEvent::McpReady {
                            name,
                            tools: tool_names,
                        });
                    }
                    Err(e) => {
                        eprintln!("[mcp] connect_one failed for '{name}': {e:#}");
                        let reason = format!("{e:#}");
                        output
                            .emit_error(&format!("AddMcpServer '{name}' failed: {reason}"), false);
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
            ProtocolCommand::Stop => return Ok(()),
            other => {
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
            && let Ok((outcome, resolved)) = rx.try_recv()
        {
            deferred_mcp_rx = None;
            pending_deferred_mcp = note_deferred_mcp_connect(
                outcome,
                resolved,
                &mut engine,
                &writer,
                &output,
                &mut dynamic_managers,
            );
        }
        if let Some((mgr, resolved)) = pending_deferred_mcp.take()
            && !integrate_deferred_mcp(
                &mut engine,
                mgr.clone(),
                &resolved,
                &writer,
                &mut dynamic_managers,
            )
        {
            pending_deferred_mcp = Some((mgr, resolved));
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
                        if let Ok((outcome, resolved)) = res {
                            pending_deferred_mcp = note_deferred_mcp_connect(
                                outcome,
                                resolved,
                                &mut engine,
                                &writer,
                                &output,
                                &mut dynamic_managers,
                            );
                        }
                    }
                }
            }
        };

        match cmd {
            ProtocolCommand::Message {
                msg_id,
                content,
                files: _,
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
                            output.emit_error(&format!("bad slash invocation: {reason}"), false);
                            output.emit_stream_end(&msg_id, 0, 0, 0, 0, 0, FinishReason::Error);
                            continue;
                        }
                    }
                }

                let mut stopped = false;
                let mut pending_config: Option<PendingConfig> = None;
                let mut mode_changed = false;

                {
                    let engine_fut = engine.run(&content, &msg_id);
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
                                            );
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
                                        );
                                    }
                                    Err(e) => {
                                        output.emit_error(&format!("{e:#}"), false);
                                        output.emit_stream_end(
                                            &msg_id,
                                            0,
                                            0,
                                            0,
                                            0,
                                            0,
                                            FinishReason::Error,
                                        );
                                    }
                                }
                                break;
                            }
                            Some(sub_cmd) = cmd_rx.recv() => {
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
                                        // NOT end the session. Dropping `engine_fut` (via the inner
                                        // `break` below) cancels the turn; we emit `stream_end`
                                        // (FinishReason::Stop) for this msg_id so the host's turn-loop
                                        // gets its terminator and doesn't hang. `stopped` then makes
                                        // the outer loop `continue` (keep reading commands) instead of
                                        // breaking — the pre-fix `break` stranded the session
                                        // ("new chat required") after any mid-turn Stop. Only EOF and
                                        // `/exit` end a json-stream session, matching the TUI (Esc
                                        // cancels the turn, never closes the session).
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
                                    ProtocolCommand::SetMode { mode } => {
                                        // GHSA-8r7g: a wire peer may not escalate to
                                        // an auto-approving mode (Force or AutoEdit)
                                        // without a local-operator opt-in.
                                        let mode_str = format!("{mode:?}").to_lowercase();
                                        if approval_manager.set_mode_from_wire(mode) {
                                            mode_changed = true;
                                            let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                                                msg_id: String::new(),
                                                message: format!("mode updated: {}", approval_manager.current_mode()),
                                            });
                                        } else {
                                            let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                                                msg_id: String::new(),
                                                message: format!("set_mode: '{mode_str}' refused — an auto-approving mode (auto_edit/force) requires a local-operator opt-in (launch with --force or WAYLAND_ALLOW_WIRE_FORCE=1)"),
                                            });
                                        }
                                    }
                                    ProtocolCommand::Ping => {
                                        let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Pong);
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
                                        let outcome = wcore_agent::approval::ApprovalOutcome {
                                            approved,
                                            modifications,
                                        };
                                        let resolved =
                                            approval_bridge.resolve(&resume_token, outcome).await;
                                        let _ = writer.emit(
                                            &wcore_protocol::events::ProtocolEvent::ApprovalResume {
                                                resume_token: resume_token.clone(),
                                                approved,
                                            },
                                        );
                                        if !resolved {
                                            let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                                                msg_id: String::new(),
                                                message: format!(
                                                    "approval_resume received for unknown token: {resume_token} (stale resume?)"
                                                ),
                                            });
                                        }
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
                if !approval_manager.set_mode_from_wire(mode) {
                    let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                        msg_id: String::new(),
                        message: format!("set_mode: '{mode_str}' refused — an auto-approving mode (auto_edit/force) requires a local-operator opt-in (launch with --force or WAYLAND_ALLOW_WIRE_FORCE=1)"),
                    });
                    eprintln!("[protocol] SetMode refused ({mode_str}, no local opt-in)");
                } else {
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
            ProtocolCommand::AddMcpServer { name, .. } => {
                output.emit_error(
                    &format!("AddMcpServer '{name}': rejected — only allowed before first Message"),
                    false,
                );
            }
            ProtocolCommand::Ping => {
                let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Pong);
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
                let outcome = wcore_agent::approval::ApprovalOutcome {
                    approved,
                    modifications,
                };
                let resolved = approval_bridge.resolve(&resume_token, outcome).await;
                let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::ApprovalResume {
                    resume_token: resume_token.clone(),
                    approved,
                });
                if !resolved {
                    let _ = writer.emit(&wcore_protocol::events::ProtocolEvent::Info {
                        msg_id: String::new(),
                        message: format!(
                            "approval_resume received for unknown token: {resume_token} (stale resume?)"
                        ),
                    });
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use wcore_mcp::manager::McpManager;

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
        let mgr = McpManager::new_for_test_with_tools(vec![
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
        ]);

        let events = mcp_ready_events_for(&mgr);
        assert_eq!(events.len(), 2, "expected one McpReady per server");

        // Helper sorts servers by name, so order is deterministic: fs, search.
        match &events[0] {
            ProtocolEvent::McpReady { name, tools } => {
                assert_eq!(name, "fs");
                let mut sorted = tools.clone();
                sorted.sort();
                assert_eq!(sorted, vec!["read_file".to_string(), "write_file".into()]);
            }
            other => panic!("expected McpReady, got {other:?}"),
        }
        match &events[1] {
            ProtocolEvent::McpReady { name, tools } => {
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
        let events = mcp_ready_events_for(&mgr);
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
        let events = mcp_ready_events_for(&mgr);
        assert_eq!(events.len(), 1, "tool-less servers must still emit");
        match &events[0] {
            ProtocolEvent::McpReady { name, tools } => {
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

    /// wayland#551: deferred-MCP integration must register the manager's
    /// tools into a LIVE engine (post-boot), emit per-server events, and
    /// park the manager alive in `dynamic_managers`. Pins the integration
    /// half of the deferral wiring — removing `register_mcp_tools` from
    /// `integrate_deferred_mcp` fails this.
    #[tokio::test]
    async fn integrate_deferred_mcp_registers_tools_into_live_engine() {
        let (mut engine, _sink) = wcore_agent::bootstrap::AgentBootstrap::build_for_test(
            wcore_config::config::Config::default(),
            vec![],
        );
        let before = engine.tool_names().len();
        let mgr = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "quick",
            false,
            Box::new(NoopTransport) as Box<dyn McpTransport>,
            vec![tool("quick_echo")],
        )]));
        let writer = ProtocolWriter::new();
        let mut dynamic_managers = Vec::new();
        assert!(
            integrate_deferred_mcp(
                &mut engine,
                mgr,
                &HashMap::new(),
                &writer,
                &mut dynamic_managers
            ),
            "integration must succeed on an idle engine"
        );
        assert!(
            engine.tool_names().len() > before
                && engine.tool_names().iter().any(|n| n.contains("quick_echo")),
            "deferred server's tools must be registered; got {:?}",
            engine.tool_names()
        );
        assert_eq!(dynamic_managers.len(), 1, "manager must be kept alive");
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
        assert!(
            !integrate_deferred_mcp(
                &mut engine,
                mgr,
                &HashMap::new(),
                &writer,
                &mut dynamic_managers
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
        assert_eq!(
            initial_session_mode(ApprovalMode::Default, false),
            SessionMode::Default
        );
        assert_eq!(
            initial_session_mode(ApprovalMode::AutoEdit, false),
            SessionMode::AutoEdit,
            "config auto-edit must reach the session (the #241 regression)"
        );
        assert_eq!(
            initial_session_mode(ApprovalMode::Force, false),
            SessionMode::Force,
            "config force must reach the session without --force"
        );

        // --force overrides every config posture to Force.
        for m in [
            ApprovalMode::Default,
            ApprovalMode::AutoEdit,
            ApprovalMode::Force,
        ] {
            assert_eq!(
                initial_session_mode(m, true),
                SessionMode::Force,
                "--force must override config {m:?} to Force"
            );
        }
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
}
