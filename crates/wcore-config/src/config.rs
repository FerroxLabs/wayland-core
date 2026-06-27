use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::browser::BrowserConfig;
use crate::compact::CompactConfig;
use crate::compat::ProviderCompat;
use crate::debug::DebugConfig;
use crate::file_cache::FileCacheConfig;
use crate::hooks::HooksConfig;
use crate::plan::PlanConfig;
use wcore_types::llm::ThinkingConfig;

// ---------------------------------------------------------------------------
// Provider-specific sub-configurations (defined here to avoid circular deps)
// ---------------------------------------------------------------------------

/// AWS Bedrock credentials configuration
//
// `Debug` is hand-written (not derived) so the long-lived AWS secrets never
// land in a log/trace via `{:?}` — only their presence is shown.
#[derive(Clone, Deserialize, Serialize, Default)]
pub struct BedrockConfig {
    pub region: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub session_token: Option<String>,
    pub profile: Option<String>,
}

impl std::fmt::Debug for BedrockConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redact = |o: &Option<String>| o.as_ref().map(|_| "<redacted>");
        f.debug_struct("BedrockConfig")
            .field("region", &self.region)
            .field("access_key_id", &redact(&self.access_key_id))
            .field("secret_access_key", &redact(&self.secret_access_key))
            .field("session_token", &redact(&self.session_token))
            .field("profile", &self.profile)
            .finish()
    }
}

/// Google Vertex AI authentication configuration
//
// `Debug` is hand-written so the inline service-account key never leaks via
// `{:?}` — only its presence is shown.
#[derive(Clone, Deserialize, Serialize, Default)]
pub struct VertexConfig {
    pub project_id: Option<String>,
    pub region: Option<String>,
    pub credentials_file: Option<String>,
    pub service_account_json: Option<String>,
}

impl std::fmt::Debug for VertexConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VertexConfig")
            .field("project_id", &self.project_id)
            .field("region", &self.region)
            .field("credentials_file", &self.credentials_file)
            .field(
                "service_account_json",
                &self.service_account_json.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Azure OpenAI authentication mode.
///
/// v0.6.4 Task 3.1: Azure OpenAI accepts either a static `api-key` header
/// (the legacy / default mode that ships with v0.6.3) or an
/// `Authorization: Bearer {aad_token}` header sourced from Entra ID / OAuth.
/// The bearer token is short-lived; the actual token-acquisition path is
/// pluggable via a token-source function provided at provider construction,
/// which keeps the AAD SDK out of the wcore-providers dep tree and lets
/// tests inject a deterministic mock token.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AzureAuthMode {
    /// Static `api-key: {key}` header. The v0.6.3 default; preserved for
    /// existing configs via `#[serde(default)]` on the field that selects it.
    #[default]
    ApiKey,
    /// `Authorization: Bearer {aad_token}` header. The token is acquired
    /// out-of-band by a caller-supplied token source.
    AadBearer,
}

/// Transport type for MCP server connections
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TransportType {
    #[default]
    Stdio,
    Sse,
    StreamableHttp,
}

/// A single MCP server configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpServerConfig {
    pub transport: TransportType,
    /// For stdio transport: the command to run
    pub command: Option<String>,
    /// For stdio transport: arguments to the command
    pub args: Option<Vec<String>>,
    /// Environment variables to set for this server (stdio)
    pub env: Option<HashMap<String, String>>,
    /// For SSE/HTTP transport: the URL
    pub url: Option<String>,
    /// HTTP headers for SSE/HTTP transports
    pub headers: Option<HashMap<String, String>>,
    /// Whether tools from this server should be deferred (name-only stub sent to LLM).
    /// Defaults to true when omitted — MCP tools are deferred by default to reduce
    /// input token usage. Set to `false` to send full schemas eagerly.
    pub deferred: Option<bool>,
    /// Allow this MCP server's URL to resolve to a loopback address
    /// (127.0.0.0/8, ::1, localhost). MCP endpoints are trusted user config,
    /// not model-driven URLs, so the SSRF guard should not block a user's own
    /// local MCP server. Off by default. Other private/LAN/link-local/CGNAT/
    /// cloud-metadata ranges and internal hostnames remain blocked even when
    /// enabled. No effect on stdio transport.
    #[serde(default)]
    pub allow_local: bool,
}

/// Collection of MCP server configurations
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, McpServerConfig>,
    /// W6 F17 — MCP curation policy.
    /// `Off` exposes every connected MCP tool (today's behaviour). `TopK(n)`
    /// trims the per-turn MCP tool list to the n highest-ranked tools via
    /// `wcore_agent::mcp_curator::McpCurator`. Default `TopK(15)`.
    #[serde(default)]
    pub curation: McpCurationPolicy,
}

/// W6 F17 — MCP tool curation policy. Selected at config-load time; consumed
/// per-turn by the engine.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpCurationPolicy {
    Off,
    TopK { k: usize },
}

impl Default for McpCurationPolicy {
    fn default() -> Self {
        Self::TopK { k: 15 }
    }
}

/// Top-level config file structure
/// B2 — egress security policy (`[security]`). On by default: the egress gate
/// blocks exfil-shaped traffic (POST/PUT/PATCH bodies, shared-platform hosts,
/// GET/HEAD with a long/high-entropy path or query) to non-allowlisted external
/// hosts. Local destinations and the auto-derived provider/first-party hosts are
/// always allowed.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SecurityConfig {
    /// Master switch for the egress gate. On by default. Disabling is
    /// **config-file only** (never a bare env var — supply-chain hazard, C8) and
    /// additionally requires the explicit `--i-accept-exfil-risk` CLI flag at
    /// the same invocation before a `false` here is honored.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Operator-curated extra allowlist entries — registrable domains (cover
    /// their subdomains, e.g. `"example.com"`) or exact hosts (for shared-
    /// platform hosts that can't be apex-allowed, e.g. `"myapp.workers.dev"`).
    /// Added on top of the auto-derived provider + first-party defaults.
    #[serde(default)]
    pub egress_allow: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            egress_allow: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ConfigFile {
    #[serde(default)]
    pub default: DefaultConfig,

    /// B2 — `[security]` egress policy block.
    #[serde(default)]
    pub security: SecurityConfig,

    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,

    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,

    #[serde(default)]
    pub tools: ToolsConfig,

    #[serde(default)]
    pub session: SessionConfig,

    /// `[inbound_webhook]` — HTTP host for inbound platform webhooks
    /// (Slack / WhatsApp / Twilio SMS). Off by default.
    #[serde(default)]
    pub inbound_webhook: InboundWebhookConfig,

    #[serde(default)]
    pub compact: CompactConfig,

    #[serde(default)]
    pub plan: PlanConfig,

    #[serde(default)]
    pub file_cache: FileCacheConfig,

    #[serde(default)]
    pub hooks: HooksConfig,

    pub bedrock: Option<BedrockConfig>,
    pub vertex: Option<VertexConfig>,

    #[serde(default)]
    pub mcp: McpConfig,

    #[serde(default)]
    pub debug: DebugConfig,

    #[serde(default)]
    pub observability: ObservabilityConfig,

    /// W7 F8-3: provider resilience chain (`ResilientProvider` wrap).
    /// Off by default — see [`ProviderChainConfig`].
    #[serde(default)]
    pub provider_chain: ProviderChainConfig,

    /// W8a A.5: ExecutionBudget caps (wall-time/tool-runtime/processes/
    /// agent-depth/tokens/cost). All fields default to `None` = no cap.
    /// Wired through bootstrap into `ExecutionBudgetView` in A.6.
    #[serde(default)]
    pub budget: crate::budget::BudgetConfig,

    /// Wave SD: credential storage selection (`plaintext` default,
    /// `keyring` opt-in). Closes SECURITY MAJOR #16.
    #[serde(default)]
    pub storage: StorageConfig,

    /// M3.1: wcore-memory v2 smart-layer wiring. `enabled = false` by
    /// default (bootstrap uses `Arc::new(NullMemory)`); flipping
    /// `enabled = true` swaps in a real `Memory::open` backend and starts
    /// the decay scheduler. See [`MemoryConfig`].
    ///
    /// `Option` so `merge_config_files` can tell an ABSENT `[memory]` table
    /// (`None` ⇒ inherit the other layer) from an EXPLICIT one that happens to
    /// match `MemoryConfig::default()` (`Some` ⇒ override). Comparing a resolved
    /// `MemoryConfig` to its default conflates the two and silently drops a
    /// project that explicitly opts in with `enabled = true` over a global
    /// `enabled = false`. A present-but-partial table still deserializes to
    /// `Some` with per-field serde defaults applied.
    #[serde(default)]
    pub memory: Option<MemoryConfig>,

    /// FleetDispatcher-class fix (audit 2026-05-24 §3): the `[browser]`
    /// block carries the operator-facing `BrowserPolicyConfig` consumed
    /// by `AgentBootstrap` to mutate each `BrowserToolSpec.policy` before
    /// the host registrar reifies plugin-supplied specs. Without this
    /// block being present in the on-disk config the runtime falls back
    /// to the deny-all default (matches `BrowserPolicyConfig::default()`).
    #[serde(default)]
    pub browser: BrowserConfig,

    /// M5.bootstrap-wiring: opt-in `[session_cap]` block — per-session /
    /// per-user tracker caps wired into `wcore_budget::BudgetTracker`
    /// during bootstrap. Distinct from the `[budget]` block above (which
    /// drives the W8a `ExecutionBudget` tree). Missing block ⇒ `None` ⇒
    /// bootstrap skips tracker installation, preserving pre-M5.3 behaviour.
    #[serde(default)]
    pub session_cap: Option<crate::budget::BudgetConfig>,

    /// Crucible (Mixture-of-Providers) — opt-in `[crucible]` block defining the
    /// cross-provider council roster + bounds. OFF by default (`enabled =
    /// false`); validated into a runnable roster at bootstrap. Lives on
    /// `ConfigFile` (the on-disk shape) rather than the resolved `Config` —
    /// bootstrap reads it alongside the `[providers]` map (which is also
    /// `ConfigFile`-only) to build the council.
    #[serde(default)]
    pub crucible: crate::crucible::CrucibleConfig,
}

/// Wave SD — top-level `[storage]` block in `config.toml`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StorageConfig {
    #[serde(default)]
    pub credentials: crate::credentials::CredentialsStorageConfig,
}

/// M3.1 — top-level `[memory]` block in `config.toml`.
///
/// Controls the wcore-memory v2 smart layer (5-partition × 3-tier cognitive
/// memory). Defaults are conservative and opt-in: `enabled = false` means
/// bootstrap wires `Arc::new(NullMemory)` and the dream-cycle / decay
/// scheduler never run. Flipping `enabled = true` swaps in a real
/// `Memory::open` backend and starts the background scheduler.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryConfig {
    /// If `true`, bootstrap constructs a real `wcore_memory::Memory` and
    /// spawns the decay scheduler. If `false`, bootstrap uses
    /// `Arc::new(NullMemory)` and all memory ops are no-ops. Default: true
    /// (matches `MemoryConfig::default` — F-091). The serde default is
    /// `default_true` so a present `[memory]` table that omits `enabled` keeps
    /// memory ON rather than silently disabling it (the two defaults must agree).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Minimum seconds between session-end dream-cycle firings. Prevents
    /// short interactive sessions from churning the consolidation pipeline.
    /// Default: 1800 (30 minutes).
    #[serde(default = "default_dream_throttle_secs")]
    pub dream_cycle_throttle_secs: u64,

    /// How often the background decay scheduler ticks `consolidate.decay()`
    /// (M3.2). Default: 3600 (1 hour).
    #[serde(default = "default_decay_interval_secs")]
    pub decay_interval_secs: u64,

    /// M4.5: embedding backend selection. Default `Hashed` keeps offline
    /// dev + tests cheap; flipping to `OpenAi` / `Voyage` / `LocalBge`
    /// activates the M4.6 / M4.7 / M4.7b backends when those land.
    #[serde(default)]
    pub embedder: EmbedderConfig,
}

/// M4.5 — embedding backend selection. Defaults to the deterministic
/// hashed-token bag so a fresh `wcore.toml` doesn't pay an API-key cost
/// just to bring memory online.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EmbedderConfig {
    #[serde(default)]
    pub backend: EmbedderBackend,

    /// Environment variable name from which to read the API key
    /// (e.g. "OPENAI_API_KEY", "VOYAGE_API_KEY"). Unused when backend is
    /// `Hashed` or `LocalBge`.
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Model override (e.g. "text-embedding-3-small", "voyage-2",
    /// "bge-small-en-v1.5"). Falls back to a per-backend default when None.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbedderBackend {
    /// Deterministic 384-dim hashed-token bag (no API key, no model load).
    #[default]
    Hashed,
    /// OpenAI embeddings API. Activated by M4.6.
    OpenAi,
    /// Voyage AI embeddings API. Activated by M4.7.
    Voyage,
    /// Local bge-small via candle/ggml. Activated by M4.7b under the
    /// `local-embedder` feature flag.
    LocalBge,
}

fn default_dream_throttle_secs() -> u64 {
    1800
}

fn default_decay_interval_secs() -> u64 {
    3600
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            // F-091 (CRIT, D4 decision): default ON. A fresh install gets a
            // real MemoryManager so GEPA, SkillRouter seeds, SkillDrafter, and
            // user-model write-back all work out of the box. Opt out via
            // `memory.enabled = false` in wcore.toml, or via the
            // `--no-memory` CLI flag (wired in wcore-cli's `main`, which sets
            // `config.memory.enabled = false` before `Config` is handed to
            // `AgentBootstrap`).
            enabled: true,
            dream_cycle_throttle_secs: default_dream_throttle_secs(),
            decay_interval_secs: default_decay_interval_secs(),
            embedder: EmbedderConfig::default(),
        }
    }
}

/// W7 F8-3: provider resilience chain config — wraps the primary provider
/// in a `ResilientProvider` with a `CircuitBreaker`. Forward-additive:
/// `enabled = false` by default, in which case bootstrap uses the primary
/// provider directly (W7-base behaviour, no wrap). Defaults shipped here
/// match the `CircuitConfig::default()` shape on the provider side so a
/// minimal `[provider_chain] enabled = true` block in `wcore.toml` is
/// sufficient to opt in.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderChainConfig {
    /// Wrap the primary provider in `ResilientProvider`. Default `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Number of failures within `window` before the breaker opens.
    /// Default `3` — matches `wcore_providers::CircuitConfig::default`.
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    /// Cooldown before an Open breaker probes via HalfOpen, in seconds.
    /// Default `30` — matches the W7 spec ("recovery timeout").
    #[serde(default = "default_recovery_timeout_secs")]
    pub recovery_timeout_secs: u64,
    /// Ordered fallback model identifiers tried (in sequence) when the
    /// primary provider's circuit opens or it returns a retryable error.
    /// Each entry is a model string in the same form as `[default] model`
    /// (a literal id or a `<provider>:<role>` short-form, e.g.
    /// `anthropic:sonnet`). Empty by default → no fallback chain, only the
    /// circuit breaker is active.
    ///
    /// Only fallbacks that resolve to the **same provider** as the primary
    /// (a cheaper / alternate model on the same endpoint) are wired today:
    /// they reuse the primary's resolved credentials and base URL. Entries
    /// that name a different provider are skipped at bootstrap with a warning
    /// — cross-provider failover needs its own credential resolution and is
    /// reserved for a follow-up.
    #[serde(default)]
    pub fallback_models: Vec<String>,
}

impl Default for ProviderChainConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            failure_threshold: default_failure_threshold(),
            recovery_timeout_secs: default_recovery_timeout_secs(),
            fallback_models: Vec::new(),
        }
    }
}

fn default_failure_threshold() -> u32 {
    3
}
fn default_recovery_timeout_secs() -> u64 {
    30
}

/// Engine observability toggles. Most are off by default (opt-in via
/// `wcore.toml`); `skills_lifecycle` defaults ON so the learn-and-evolve
/// loop (auto-skill drafting + curator + router seeding) runs out of the
/// box — see the manual `Default` impl below.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ObservabilityConfig {
    /// W1: emit `trace_event` over the JSON stream protocol and advertise
    /// `capabilities.structured_traces = true` on the Ready event. Hosts
    /// that haven't learned about the new variant must remain off; flip
    /// this only when the host (e.g. Wayland Desktop) is ready to consume.
    #[serde(default)]
    pub structured_traces: bool,
    /// W9: enable autonomous skill creation (F10), curator (F11), and
    /// P5 user-model inference (PUM). Default off until the curated set
    /// is operator-reviewed. When true the engine bootstrap will register
    /// the F11 `Curator` hook on `on_session_end` and (once the engine
    /// is wired to memory in a follow-up wave) drive per-turn F10
    /// detect/stage/emit and end-of-session PUM inference.
    ///
    /// Defaults to `true` (smart default): the learn-and-evolve loop is the
    /// product's headline capability and must run out of the box. A user can
    /// still opt out with `[observability] skills_lifecycle = false`. Both the
    /// serde default (TOML-omitted) and the struct `Default` impl yield true,
    /// so a no-config first-run session also gets the loop.
    #[serde(default = "default_true")]
    pub skills_lifecycle: bool,
    /// F-092 (W7-N): emit `evolution_event` during real sessions and apply
    /// the Paraphrase mutator to successful trajectories. Default off —
    /// the live evolve path is opt-in only (CLI: `--online-evolution`,
    /// config: `[observability] online_evolution = true`). When true the
    /// engine emits one `ProtocolEvent::EvolutionEvent` per session at
    /// session-end when the session had at least one successful tool call,
    /// and persists a Paraphrase variant to `$WAYLAND_HOME/evolved/`.
    #[serde(default)]
    pub online_evolution: bool,
    /// Dynamic Workflows B3 — opt-in `WorkflowCandidate` detection signal.
    /// When `true`, the engine computes a cheap keyword/pattern heuristic
    /// on each turn's user input (alongside the existing intent-telemetry
    /// classify) to flag turns that *look like* a fan-out / multi-step
    /// audit / migration / "be comprehensive" workflow. The signal is
    /// telemetry-only — it NEVER influences routing, template selection,
    /// or tool dispatch (the confirm gate lands in B6). Default `false`:
    /// when off, the heuristic is not even computed, so a default-config
    /// session behaves byte-for-byte as before.
    #[serde(default)]
    pub workflow_detection_enabled: bool,
    /// Dynamic Workflows B6 — opt-in LIVE workflow confirm gate. Distinct
    /// from `workflow_detection_enabled` (the B3/B4 shadow-only signal):
    /// when `true` AND a turn's input looks like a workflow candidate AND
    /// both an approval manager and a protocol writer are wired, the engine
    /// synthesises a `WorkflowPlan`, emits a `Workflow` tool-request +
    /// approval-required, and — only on explicit user approval — runs the
    /// workflow as the turn's output. Default `false`: when off the live
    /// gate never fires and the turn behaves exactly as before. Note this
    /// gate authorises *running* the workflow only; the workflow's inner
    /// sub-agent tools still gate through the normal approval path.
    #[serde(default)]
    pub workflow_live_mode: bool,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            structured_traces: false,
            // Learn-and-evolve loop ON by default (smart default). Mirrors the
            // `#[serde(default = "default_true")]` on the field so struct-default
            // construction (e.g. `ConfigFile::default()` on a no-config first run)
            // and TOML-omitted load agree.
            skills_lifecycle: true,
            online_evolution: false,
            workflow_detection_enabled: false,
            workflow_live_mode: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DefaultConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    pub model: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default)]
    pub max_turns: Option<usize>,
    /// The default tool-approval posture for an interactive session
    /// (`default` / `auto-edit` / `force`). Consumed at TUI boot to set the
    /// approval manager's initial mode; `--force` still overrides to `force`.
    #[serde(default)]
    pub approval_mode: ApprovalMode,
    pub system_prompt: Option<String>,
    /// The display name the user chose during onboarding ("what should I
    /// call you?"). Optional — absent on configs written before this
    /// field existed and on the Ollama/Skip paths that never reached the
    /// name prompt. Purely cosmetic; the engine never gates on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// D004 — read-only / offline posture. When `true` the session must
    /// refuse every outbound provider API call (the "Skip — browse code,
    /// no API calls" onboarding path). Defaults to `false`.
    ///
    /// NOTE: this field is the persisted source of truth for the posture,
    /// but the refusal gate that honours it at turn-submit time lives in
    /// the engine/provider layer (`wcore-agent` bootstrap), which reads
    /// this flag and short-circuits before any provider request. Until
    /// that gate is wired, onboarding must NOT promise "no API calls" as if
    /// it were already enforced.
    #[serde(default)]
    pub read_only: bool,
}

impl Default for DefaultConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            model: None,
            max_tokens: default_max_tokens(),
            max_turns: None,
            approval_mode: ApprovalMode::default(),
            system_prompt: None,
            user: None,
            read_only: false,
        }
    }
}

/// The session's default tool-approval posture, persisted as
/// `[default] approval_mode`. Mirrors `wcore_protocol::commands::SessionMode`
/// (Default / AutoEdit / Force) but is defined here so `wcore-config` stays
/// decoupled from the protocol crate; the TUI/engine map between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    /// Ask before writing or running anything.
    #[default]
    Default,
    /// Apply edits automatically; still ask before running commands.
    AutoEdit,
    /// Never ask — apply and run everything.
    Force,
}

impl ApprovalMode {
    /// The lowercase wire string shared by the config + the TUI `ConfigView`.
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalMode::Default => "default",
            ApprovalMode::AutoEdit => "auto-edit",
            ApprovalMode::Force => "force",
        }
    }

    /// Parse the wire string; an unknown/empty value falls back to `Default`.
    pub fn from_wire(s: &str) -> ApprovalMode {
        match s {
            "auto-edit" => ApprovalMode::AutoEdit,
            "force" => ApprovalMode::Force,
            _ => ApprovalMode::Default,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProviderConfig {
    /// Underlying built-in provider type for a custom provider alias.
    pub provider: Option<String>,
    /// Optional default model for this provider entry.
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    /// Enable prompt caching (Anthropic only, default: true)
    pub prompt_caching: Option<bool>,
    /// Provider compatibility overrides
    pub compat: Option<ProviderCompat>,
}

/// A named profile bundles provider + model + overrides
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProfileConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: Option<u32>,
    pub max_turns: Option<usize>,
    /// Inherit settings from another profile
    pub extends: Option<String>,
    /// MCP server names to enable for this profile (references [mcp.servers.*])
    pub mcp_servers: Option<Vec<String>>,
    /// Provider compatibility overrides
    pub compat: Option<ProviderCompat>,
}

/// Per-skill deny/allow rule lists loaded from `[tools.skills]` in config.toml.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SkillsPermissionConfig {
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub allow: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolsConfig {
    #[serde(default)]
    pub auto_approve: bool,
    #[serde(default = "default_allow_list")]
    pub allow_list: Vec<String>,
    /// Skill-level deny/allow rules. Merged by concatenation across global + project configs.
    #[serde(default)]
    pub skills: SkillsPermissionConfig,
    /// W6 F15 — verification loop. When true, registers VerifyWriteHook on
    /// the HookEngine; the hook re-reads files after successful Write tool
    /// calls and injects a verification-failed message back into the next
    /// turn on mismatch. Off by default — cheap but not free, and best
    /// suited for long autonomous sessions.
    ///
    /// Field name kept as `verify_edits` (not `verify_writes`) because the
    /// W6.1 follow-up extends this hook to also cover Edit (audit rev-2
    /// finding 7); renaming once is cheaper than renaming twice.
    #[serde(default = "default_true")]
    pub verify_edits: bool,
    /// Windows-only: select the interpreter the Bash tool runs commands
    /// through. `"powershell"` (Windows PowerShell 5.1) or `"pwsh"`
    /// (PowerShell 7+); unset / any other value keeps the default `cmd`.
    /// No-op on Unix. The `WAYLAND_BASH_SHELL` env var overrides this at
    /// runtime. The host (desktop app) writes this key from its shell toggle.
    #[serde(default)]
    pub windows_shell: Option<String>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            auto_approve: false,
            allow_list: default_allow_list(),
            skills: SkillsPermissionConfig::default(),
            verify_edits: true,
            windows_shell: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_session_dir")]
    pub directory: String,
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            directory: default_session_dir(),
            max_sessions: default_max_sessions(),
        }
    }
}

/// Inbound webhook host (`[inbound_webhook]`).
///
/// When `enabled`, the agent stands up an HTTP listener that routes
/// `POST`/`GET /webhooks/<channel>` requests to the matching channel's
/// signature-verifying [`Channel::ingest_webhook`] path. Off by default —
/// no listener is bound unless the operator opts in.
///
/// `public_base_url` must be set to the exact public URL (scheme + host)
/// the platform calls when the host sits behind a reverse proxy: Twilio
/// signs the full request URL, so a mismatch fails signature verification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct InboundWebhookConfig {
    /// Whether to bind the inbound webhook listener. Default `false`.
    pub enabled: bool,
    /// Socket address to bind. Default `"127.0.0.1:8787"` (loopback only;
    /// front it with a TLS-terminating proxy for public exposure).
    pub bind: String,
    /// Public base URL the platform calls (scheme + host, no trailing
    /// path). Required for Twilio signature verification behind a proxy;
    /// `None` reconstructs the URL from the request `Host` header.
    pub public_base_url: Option<String>,
}

impl Default for InboundWebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: "127.0.0.1:8787".to_string(),
            public_base_url: None,
        }
    }
}

// --- Default value functions ---

fn default_provider() -> String {
    "anthropic".to_string()
}
fn default_max_tokens() -> u32 {
    // A generous request ceiling. This is SAFE despite being large because the
    // engine clamps it per-model before sending (`size_output_cap`): a known
    // model is clamped to its real output ceiling (so e.g. gpt-4o never 400s on
    // a 16384 cap), and an unknown/router model is clamped to a conservative
    // floor with the truncation auto-continue loop as the net. 8192 (the prior
    // default) truncated routine build turns; 64000 lets frontier models emit
    // large code/docs in a single round once clamped to what they actually
    // allow. Treated as a CAP, never sent raw.
    64000
}
fn default_allow_list() -> Vec<String> {
    // Read-only info-gathering tools — no destructive action, safe to
    // auto-approve. Anything that writes, executes, or sends a message
    // is NOT in this list and still gates on the approval flow. New
    // installs get this default; existing users keep whatever they
    // have (the legacy three-tool list still passes the
    // `is-default` check via `default_allow_list_legacy_set`).
    vec![
        "Read".into(),
        "Grep".into(),
        "Glob".into(),
        "web".into(),
        "WebFetch".into(),
        "vision_analyze".into(),
        "transcribe_audio".into(),
        "ToolSearch".into(),
        "Skill".into(),
        "wayland_status".into(),
        "wayland_telemetry_query".into(),
    ]
}
fn default_true() -> bool {
    true
}
fn default_session_dir() -> String {
    // F-035 + F-010: per-user, consistent regardless of cwd.
    // Resolution flows through wayland_config_dir() so WAYLAND_HOME is
    // honoured.  W3-H's TODO(F-010) resolved: the canonical helper is now
    // wayland_config_dir() in this file.
    wayland_config_dir()
        .join("sessions")
        .to_string_lossy()
        .into_owned()
}
fn default_max_sessions() -> usize {
    20
}

// --- Resolved runtime config ---

// `Debug` is hand-written (below) so the live `api_key` never lands in a log or
// trace via `{:?}`. Every other field delegates to its own Debug (Bedrock/Vertex
// sub-configs redact their own secrets).
#[derive(Clone)]
pub struct Config {
    pub provider_label: String,
    pub provider: ProviderType,
    pub api_key: String,
    pub base_url: String,
    /// B2 — egress security policy (allowlist + on/off). See [`SecurityConfig`].
    pub security: SecurityConfig,
    pub model: String,
    pub max_tokens: u32,
    /// Crucible #3: optional sampling temperature for this session's requests.
    /// `None` (the default) leaves the provider on its own default and omits the
    /// `temperature` body field. The council threads per-tier temperatures here
    /// via `SubAgentConfig` -> `child_config`; the top-level CLI path leaves it
    /// `None`.
    pub temperature: Option<f32>,
    pub max_turns: Option<usize>,
    /// The resolved default tool-approval posture (from `[default]
    /// approval_mode`). Consumed at TUI boot to seed the approval manager's
    /// initial `SessionMode`; `--force` overrides it.
    pub approval_mode: ApprovalMode,
    pub system_prompt: Option<String>,
    pub thinking: Option<ThinkingConfig>,
    pub prompt_caching: bool,
    pub compat: ProviderCompat,
    pub tools: ToolsConfig,
    /// W4 builtin-tools registration gates (Script on/off, RepoMap on/off).
    /// Separate from `tools` (which holds skill permissions).
    pub builtin_tools: crate::tools::BuiltinToolsConfig,
    /// W4 / W0 capability advertisement surface. The bootstrap path is
    /// authoritative; flipping fields here without the matching tool
    /// registration is a no-op.
    pub advertised_capabilities: crate::tools::AdvertisedCapabilitiesConfig,
    pub session: SessionConfig,
    /// Resolved copy of the on-disk `[inbound_webhook]` block. Bootstrap
    /// consults `enabled` to decide whether to spawn the inbound webhook
    /// host (see `wcore_agent::inbound_webhook`).
    pub inbound_webhook: InboundWebhookConfig,
    pub compact: CompactConfig,
    pub plan: PlanConfig,
    pub file_cache: FileCacheConfig,
    pub hooks: HooksConfig,
    pub bedrock: Option<BedrockConfig>,
    pub vertex: Option<VertexConfig>,
    pub mcp: McpConfig,
    pub debug: DebugConfig,
    pub observability: ObservabilityConfig,
    /// W7 F8-3: bootstrap consults `enabled` to decide whether to wrap the
    /// primary provider in `ResilientProvider`.
    pub provider_chain: ProviderChainConfig,
    /// W8a A.5/A.6: ExecutionBudget caps. Resolved-config copy of the
    /// merged `ConfigFile.budget`; bootstrap converts this into a
    /// `wcore_agent::budget::ExecutionBudgetView` via the `From` impl.
    pub budget: crate::budget::BudgetConfig,
    /// Wave SD: credential storage backend selection. Drives the
    /// `CredentialsStore` returned by `Config::open_credentials_store`.
    pub storage: StorageConfig,
    /// M3.1: wcore-memory v2 smart-layer wiring. Resolved-config copy
    /// of the merged `ConfigFile.memory`. Bootstrap consults `enabled`
    /// to decide between `Arc::new(NullMemory)` and a real `Memory::open`.
    pub memory: MemoryConfig,
    /// FleetDispatcher-class fix (audit 2026-05-24 §3): runtime copy of
    /// the merged `ConfigFile.browser`. `AgentBootstrap` reads
    /// `browser.policy.{default_action, allowed_origins, denied_origins}`
    /// and mutates every `plugin_runner.browser.specs[*].policy` before
    /// the host registrar reifies them into a live `BrowserTool`.
    pub browser: BrowserConfig,
    /// M5.bootstrap-wiring: per-session / per-user enforcement caps that
    /// `AgentBootstrap` translates into a `wcore_budget::BudgetTracker`
    /// installed on the engine. `None` (default) skips tracker
    /// installation entirely, preserving pre-M5.3 behaviour. Distinct
    /// from `budget` above, which is the W8a tree-shaped
    /// `ExecutionBudget` (wall-time / tool-runtime / process / token
    /// rollup). See `wcore-budget::tracker` for the cap fields.
    ///
    /// `Config` itself is the resolved (non-serde) runtime type; the
    /// on-disk surface is `ConfigFile.session_cap` which carries the
    /// `#[serde(default)]` attribute.
    pub session_cap: Option<wcore_budget::BudgetConfig>,

    /// Crucible (Mixture-of-Providers) council config, carried onto the resolved
    /// `Config` so the in-process bootstrap can gate the council's cap-less spend
    /// accumulator on `crucible.daily_cap_usd` / `crucible.max_cost_usd` (the
    /// CLI council path reads it from `ConfigFile` directly). Mirrors the
    /// `ConfigFile.crucible` block; populated from the merged on-disk config in
    /// `Config::resolve` and defaults to OFF (`CrucibleConfig::default()`).
    pub crucible: crate::crucible::CrucibleConfig,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("provider_label", &self.provider_label)
            .field("provider", &self.provider)
            // SECURITY: never print the live api_key — only whether one is set.
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "<none>"
                } else {
                    "<redacted>"
                },
            )
            .field("base_url", &self.base_url)
            .field("security", &self.security)
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("max_turns", &self.max_turns)
            .field("approval_mode", &self.approval_mode)
            .field("system_prompt", &self.system_prompt)
            .field("thinking", &self.thinking)
            .field("prompt_caching", &self.prompt_caching)
            .field("compat", &self.compat)
            .field("tools", &self.tools)
            .field("builtin_tools", &self.builtin_tools)
            .field("advertised_capabilities", &self.advertised_capabilities)
            .field("session", &self.session)
            .field("inbound_webhook", &self.inbound_webhook)
            .field("compact", &self.compact)
            .field("plan", &self.plan)
            .field("file_cache", &self.file_cache)
            .field("hooks", &self.hooks)
            .field("bedrock", &self.bedrock)
            .field("vertex", &self.vertex)
            .field("mcp", &self.mcp)
            .field("debug", &self.debug)
            .field("observability", &self.observability)
            .field("provider_chain", &self.provider_chain)
            .field("budget", &self.budget)
            .field("storage", &self.storage)
            .field("memory", &self.memory)
            .field("browser", &self.browser)
            .field("session_cap", &self.session_cap)
            .finish()
    }
}

impl Default for Config {
    /// Test-oriented defaults. The runtime config-resolution path
    /// (`Config::resolve`) builds this struct explicitly from TOML +
    /// CLI args; `Default` exists so test fixtures can use
    /// `Config { field: value, ..Default::default() }` spread syntax
    /// without restating 25+ subfields whenever Config grows.
    ///
    /// Conservative choices:
    /// - `provider`/`provider_label` → Anthropic, matching `DefaultConfig`.
    /// - `api_key` → empty string (no live calls without explicit override).
    /// - `base_url` → empty (resolver fills this in production).
    /// - `model` → empty string; tests that hit a provider override this.
    /// - `prompt_caching` → `false` (the safest default; Anthropic flips
    ///   it true in `Config::resolve` via provider-specific logic, but
    ///   `Default` cannot replicate that conditional).
    /// - `session.enabled` / `plan.enabled` / `builtin_tools.script` etc.
    ///   inherit each sub-config's own `Default` impl which is already
    ///   tuned to the "safe off / on-as-appropriate" stance documented
    ///   on each.
    fn default() -> Self {
        Self {
            provider_label: "anthropic".to_string(),
            provider: ProviderType::default(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            max_tokens: default_max_tokens(),
            temperature: None,
            max_turns: None,
            approval_mode: ApprovalMode::default(),
            system_prompt: None,
            thinking: None,
            prompt_caching: false,
            compat: crate::compat::ProviderCompat::default(),
            tools: ToolsConfig::default(),
            builtin_tools: crate::tools::BuiltinToolsConfig::default(),
            advertised_capabilities: crate::tools::AdvertisedCapabilitiesConfig::default(),
            session: SessionConfig::default(),
            inbound_webhook: InboundWebhookConfig::default(),
            compact: crate::compact::CompactConfig::default(),
            plan: crate::plan::PlanConfig::default(),
            file_cache: crate::file_cache::FileCacheConfig::default(),
            hooks: crate::hooks::HooksConfig::default(),
            bedrock: None,
            vertex: None,
            mcp: McpConfig::default(),
            debug: crate::debug::DebugConfig::default(),
            observability: ObservabilityConfig::default(),
            provider_chain: ProviderChainConfig::default(),
            budget: wcore_budget::BudgetConfig::default(),
            storage: StorageConfig::default(),
            memory: MemoryConfig::default(),
            browser: BrowserConfig::default(),
            security: SecurityConfig::default(),
            session_cap: None,
            crucible: crate::crucible::CrucibleConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Bedrock,
    Vertex,
    /// Google Gemini via the native Generative Language API.
    /// W11 (closes debt B.4-Gemini). Distinct from `Vertex`, which routes
    /// Anthropic-on-Vertex (and historically Gemini-on-Vertex through the
    /// OpenAI-compat path). Native Gemini uses an API key directly.
    Gemini,
    // --- v0.6.3 Tier-2 OpenAI-compatible providers (D.1 Round 1 cleanup) ---
    // These were shipped as code (provider + factory + tests) in v0.6.3 but
    // were unreachable from any config because `create_provider` matched a
    // closed 5-variant enum. Each is a thin newtype over `OpenAIProvider`.
    /// Azure OpenAI — deployment-routed; `base_url` carries the resource
    /// endpoint (`https://{resource}.openai.azure.com`) and `model` the
    /// deployment name. v0.6.4 Task 3.1 added the [`AzureAuthMode`] enum
    /// and the runtime `AzureAuth { ApiKey, AadBearer }` in `wcore-providers`,
    /// but the config→provider wiring (so a `[azure-openai]` section in
    /// `wayland.toml` can flip to AAD bearer) lands in follow-up Task 3.1b
    /// along with the token-source injection seam.
    AzureOpenAI,
    /// Together AI — OpenAI-compatible inference API.
    Together,
    /// Fireworks AI — OpenAI-compatible inference API.
    Fireworks,
    /// NVIDIA NIM — OpenAI-compatible inference API.
    Nvidia,
    /// Perplexity — OpenAI-compatible API (`sonar` model family).
    Perplexity,
    /// Cerebras — OpenAI-compatible inference API.
    Cerebras,
    /// OpenRouter — meta-router fronting 100+ models behind an
    /// OpenAI-compatible chat-completions surface. Model ids use
    /// `vendor/model` format (e.g. `anthropic/claude-opus-4-7`).
    /// v0.8.1 task U10a.
    OpenRouter,
    /// Flux Router — Sean's own router product. OpenAI-compatible
    /// chat-completions surface; URL is configurable until the
    /// production endpoint is finalized. v0.8.1 task U10a.
    FluxRouter,
    // --- v0.8.1 U10b: 3 more OpenAI-compatible providers ----------------
    /// DeepSeek — OpenAI-compatible chat-completions surface
    /// (`deepseek-chat`, `deepseek-reasoner`).
    Deepseek,
    /// xAI (Grok) — OpenAI-compatible chat-completions surface
    /// (`grok-2`, `grok-2-vision`, `grok-beta`).
    Xai,
    /// Groq — fast LPU inference for open-weight models behind an
    /// OpenAI-compatible surface (`llama-3.1-70b-versatile`,
    /// `mixtral-8x7b-32768`, etc.).
    Groq,
    /// Moonshot (Kimi) — OpenAI-compatible chat-completions surface.
    /// v0.8.1 U10e. Aliases: `"moonshot"`, `"kimi"`.
    Moonshot,
    /// Alibaba Qwen via DashScope's `/compatible-mode/v1` OpenAI shape.
    /// v0.8.1 U10e. Aliases: `"qwen"`, `"alibaba"`, `"dashscope"`.
    Qwen,
    /// Mistral AI — OpenAI-compatible chat-completions surface
    /// (`mistral-large-latest`, `mistral-small-latest`, `codestral-latest`).
    /// v0.8.1 U10 (F-025 fix): wired from orphan module to reachable arm.
    Mistral,
    /// Cohere — OpenAI-compatible chat-completions surface via
    /// `api.cohere.com/compatibility/v1`. Models: `command-r-plus`, etc.
    /// v0.8.1 U10 (F-025 fix): wired from orphan module to reachable arm.
    Cohere,
    /// "Sign in with ChatGPT" — routes inference through the ChatGPT Codex
    /// backend (`chatgpt.com/backend-api/codex`) using OAuth tokens from a
    /// ChatGPT subscription instead of an OpenAI API key. Speaks the OpenAI
    /// Responses wire format. The provider is constructed in `bootstrap`
    /// (not `create_native_provider`) because it needs an OAuth-backed bearer
    /// source that lives in `wcore-agent` (layering). Distinct from `OpenAI`,
    /// which is API-key auth against `api.openai.com`.
    OpenAIChatGpt,
    /// MiniMax via its Anthropic-compatible endpoint
    /// (`https://api.minimax.io/anthropic`). Speaks the native Anthropic wire
    /// protocol — `x-api-key` auth, `/v1/messages`, `/v1/models`, SSE, and
    /// Anthropic error envelopes (verified live 2026-06-18) — so it reuses
    /// `wcore_providers::anthropic::AnthropicProvider` rather than a duplicate
    /// struct, distinguished only by base URL, `provider_type` cost label, and
    /// the offline model-alias fallback key. Default model: `MiniMax-M2`.
    MiniMax,
    /// Sakana AI ("Fugu") — OpenAI-compatible chat-completions endpoint at
    /// `https://api.sakana.ai/v1`. Bearer auth (keys are prefixed `fish_`).
    /// Fugu is a multi-agent orchestration/routing layer; models: `fugu`
    /// (default), `fugu-ultra`, `fugu-ultra-20260615`. Thin newtype over
    /// `OpenAIProvider`.
    Sakana,
}

impl ProviderType {
    /// True for the v0.6.3 Tier-2 providers that are thin OpenAI-compatible
    /// newtypes (everything except the four "native" providers + Gemini).
    /// Used to apply OpenAI compat defaults uniformly.
    pub fn is_openai_compatible(self) -> bool {
        matches!(
            self,
            ProviderType::OpenAI
                | ProviderType::AzureOpenAI
                | ProviderType::Together
                | ProviderType::Fireworks
                | ProviderType::Nvidia
                | ProviderType::Perplexity
                | ProviderType::Cerebras
                | ProviderType::OpenRouter
                | ProviderType::FluxRouter
                | ProviderType::Deepseek
                | ProviderType::Xai
                | ProviderType::Groq
                | ProviderType::Moonshot
                | ProviderType::Qwen
                | ProviderType::Mistral
                | ProviderType::Cohere
                // A7: ChatGPT Codex rides the OpenAI Responses wire format and
                // its compat preset is built on `openai_compat_provider`, so it
                // belongs to the OpenAI-compatible family for plumbing purposes.
                | ProviderType::OpenAIChatGpt
                | ProviderType::Sakana
        )
    }
}

impl Default for ProviderType {
    /// Anthropic matches `default_provider()` in `DefaultConfig`, which is
    /// the existing "no override" choice elsewhere in the config layer.
    /// Tests that care about a specific provider override this explicitly.
    fn default() -> Self {
        ProviderType::Anthropic
    }
}

/// The default model string used when neither the CLI, provider config, nor
/// Canonical predicate (R78): the single source of truth for "does this OpenAI
/// model accept the `reasoning_effort` request field" (`o1*`, `o3*`, `gpt-5*`).
/// It lives here, in the lower crate, because `wcore-providers` depends on
/// `wcore-config` (not the reverse); `openai_compat::accepts_reasoning_effort`
/// now forwards to this instead of duplicating the prefix logic.
pub fn openai_model_accepts_effort(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    // o-series: o1, o3, o1-mini, o3-mini, o4 (future), …
    let is_o_series = {
        let b = m.as_bytes();
        b.len() >= 2 && b[0] == b'o' && b[1].is_ascii_digit()
    };
    is_o_series || m.starts_with("gpt-5")
}

/// default-section config picks one. All four built-in providers now route
/// through `wcore_types::model_aliases`, so an upstream model deprecation is
/// a one-line edit in that module (closes debt B.4 / HC-3-followup).
pub(crate) fn default_model_for(provider: ProviderType) -> &'static str {
    use wcore_types::model_aliases::{
        ANTHROPIC_SONNET, BEDROCK_SONNET, MINIMAX_M2, OPENAI_GPT4O, VERTEX_GEMINI_PRO,
        VERTEX_SONNET,
    };
    match provider {
        ProviderType::Anthropic => ANTHROPIC_SONNET,
        ProviderType::OpenAI => OPENAI_GPT4O,
        ProviderType::Bedrock => BEDROCK_SONNET,
        ProviderType::Vertex => VERTEX_SONNET,
        // Native Gemini uses the same model identifiers as Vertex Gemini
        // (the API surface differs, the model IDs don't).
        ProviderType::Gemini => VERTEX_GEMINI_PRO,
        // v0.6.3 Tier-2 providers host heterogeneous model catalogs (Llama,
        // Qwen, DeepSeek, sonar, …) with no single sensible default — the
        // user MUST set `model` in config. Empty string flows through and
        // surfaces as an API error if left unset, which is the honest
        // behavior (we cannot guess a model that exists on the account).
        ProviderType::AzureOpenAI
        | ProviderType::Together
        | ProviderType::Fireworks
        | ProviderType::Nvidia
        | ProviderType::Perplexity
        | ProviderType::Cerebras
        | ProviderType::OpenRouter
        | ProviderType::FluxRouter => "",
        ProviderType::Deepseek | ProviderType::Xai | ProviderType::Groq => "",
        ProviderType::Moonshot | ProviderType::Qwen => "",
        // F-025: Mistral + Cohere have heterogeneous model catalogs; user sets model.
        ProviderType::Mistral | ProviderType::Cohere => "",
        // Sakana has a clear headline default — `fugu` routes across providers,
        // so `--provider sakana` with no model just works.
        ProviderType::Sakana => "fugu",
        // ChatGPT Codex default: gpt-5.5 (the headline Codex model). See
        // `wcore_types::model_aliases` codex consts for the full catalog.
        ProviderType::OpenAIChatGpt => "gpt-5.5",
        // MiniMax has a single documented headline model, so — unlike the
        // heterogeneous Tier-2 catalogs above — it gets a sensible default.
        ProviderType::MiniMax => MINIMAX_M2,
    }
}

/// D002: resolve a provider SLUG (as written into `[default] provider` by
/// onboarding) to its default model, or `""` when the provider hosts a
/// heterogeneous catalog with no sensible default (the Tier-2 / router /
/// data-driven-catalog providers). Onboarding uses this to stamp a
/// `[default] model` line up front when one exists, so a built-in provider
/// never lands in the no-model dead-end; a slug with no default (e.g. `groq`,
/// `openrouter`, or an unknown catalog id) yields `""` and is recovered
/// in-app via the Workspace `/model` affordance.
pub fn default_model_for_slug(slug: &str) -> &'static str {
    match parse_builtin_provider(slug) {
        Some(provider) => default_model_for(provider),
        None => "",
    }
}

/// Parse a built-in provider slug (or documented alias) into its
/// [`ProviderType`]. Thin public wrapper over the crate-private match used by
/// `resolve` — exposed so callers in higher crates (the `/provider` picker)
/// can route a slug through the same single source of truth. Returns `None`
/// for an unknown name.
pub fn provider_type_from_slug(slug: &str) -> Option<ProviderType> {
    parse_builtin_provider(slug)
}

/// The built-in providers a connection check can meaningfully cover: the four
/// natives plus Gemini and the OAuth ChatGPT backend. These are the
/// [`wcore_types::model_aliases::known_providers`] catalog, expressed as
/// [`ProviderType`]s so [`connected_providers`] never has to round-trip
/// through slug strings. Tier-2 / catalog providers are intentionally absent —
/// the picker and the catalog refresh only consider the known set.
const KNOWN_PROVIDER_TYPES: &[ProviderType] = &[
    ProviderType::Anthropic,
    ProviderType::OpenAI,
    ProviderType::Bedrock,
    ProviderType::Vertex,
    ProviderType::Gemini,
    ProviderType::OpenAIChatGpt,
];

/// Canonical slug for a [`ProviderType`] — the inverse of
/// [`parse_builtin_provider`]'s primary spelling (NOT an alias). This is the
/// key under which a provider's live model list is cached
/// (`model_catalog::save`) and the alias-catalog key
/// (`wcore_types::model_aliases`). Keep in sync with `parse_builtin_provider`.
pub fn provider_type_slug(provider: ProviderType) -> &'static str {
    match provider {
        ProviderType::Anthropic => "anthropic",
        ProviderType::OpenAI => "openai",
        ProviderType::Bedrock => "bedrock",
        ProviderType::Vertex => "vertex",
        ProviderType::Gemini => "gemini",
        ProviderType::AzureOpenAI => "azure-openai",
        ProviderType::Together => "together",
        ProviderType::Fireworks => "fireworks",
        ProviderType::Nvidia => "nvidia",
        ProviderType::Perplexity => "perplexity",
        ProviderType::Cerebras => "cerebras",
        ProviderType::OpenRouter => "openrouter",
        ProviderType::FluxRouter => "flux-router",
        ProviderType::Sakana => "sakana",
        ProviderType::Deepseek => "deepseek",
        ProviderType::Xai => "xai",
        ProviderType::Groq => "groq",
        ProviderType::Moonshot => "moonshot",
        ProviderType::Qwen => "qwen",
        ProviderType::Mistral => "mistral",
        ProviderType::Cohere => "cohere",
        ProviderType::OpenAIChatGpt => "openai-chatgpt",
        ProviderType::MiniMax => "minimax",
    }
}

/// Path to the stored OAuth token for the ChatGPT backend
/// (`~/.wayland/oauth/chatgpt.json`). Mirrors `wcore_agent::oauth::OAuthStorage`
/// (`from_home` → `~/.wayland/oauth/`, `path_for("chatgpt")` →
/// `chatgpt.json`) WITHOUT depending on `wcore-agent` (layering): the check is
/// a cheap path existence test, not a token load. The `chatgpt` provider slug
/// is the OAuth-store key (distinct from the `openai-chatgpt` catalog slug).
///
/// Resolved under [`profile_home`] so it honours `WAYLAND_HOME` exactly like the
/// token *writer* (`OAuthStorage::from_home`) — the two must agree or a
/// sandboxed run would look for the token in the wrong place. Identical to the
/// old `dirs::home_dir()/.wayland/oauth/chatgpt.json` when `WAYLAND_HOME` is
/// unset.
fn chatgpt_oauth_token_path() -> PathBuf {
    profile_home().join("oauth").join("chatgpt.json")
}

/// Whether an xAI (Grok) OAuth credential exists to authenticate out-of-band:
/// the engine's own store (`~/.wayland/oauth/xai.json`) or the Grok CLI's
/// `~/.grok/auth.json` (`$GROK_HOME/auth.json` when set). File-existence only —
/// the actual parse + refresh lives in `wcore_agent::oauth::xai` (config can't
/// depend on agent), mirroring how the ChatGPT presence check is split.
fn xai_oauth_credentials_present() -> bool {
    if profile_home().join("oauth").join("xai.json").exists() {
        return true;
    }
    let grok = std::env::var("GROK_HOME")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .map(|d| PathBuf::from(d).join("auth.json"))
        .or_else(|| dirs::home_dir().map(|h| h.join(".grok").join("auth.json")));
    grok.is_some_and(|p| p.exists())
}

/// Whether `provider`'s credential is present right now, decided synchronously
/// with no network. The single source of truth shared by the `/provider`
/// picker (`wcore-cli`) and the model-catalog refresh service
/// (`wcore-providers`). Mirrors the three credential classes
/// [`resolve_api_key`] distinguishes:
///
/// - **Ambient cloud** (`bedrock`, `vertex`): connected only when a real
///   credential source is present on this host (see
///   [`aws_ambient_credentials_present`] / [`gcp_ambient_credentials_present`])
///   — NOT unconditionally. They carry no API key, but listing them as
///   connected on a box with no AWS/GCP credentials offered the user a provider
///   that would error on the first turn.
/// - **OAuth** (`openai-chatgpt`): connected when the stored login file
///   (`~/.wayland/oauth/chatgpt.json`) exists.
/// - **API key** (everything else): connected when `resolve_api_key`
///   resolves a non-empty key via the config field / credentials store / env
///   chain. A `MissingApiKey` error (or an empty resolved key) is "not
///   connected".
pub fn provider_connected(provider: ProviderType) -> bool {
    match provider {
        // Ambient cloud credentials — connected only when AWS/GCP credentials
        // are actually present (env, shared config/credentials files, container
        // or OIDC role, or ADC), decided with no network call.
        ProviderType::Bedrock => aws_ambient_credentials_present(),
        ProviderType::Vertex => gcp_ambient_credentials_present(),
        // OAuth-backed — the stored login token is the credential.
        ProviderType::OpenAIChatGpt => chatgpt_oauth_token_path().exists(),
        // API-key providers: resolved key must be present and non-empty.
        _ => {
            let storage = crate::credentials::CredentialsStorageConfig::default();
            matches!(
                resolve_api_key(None, None, provider, &storage),
                Ok(key) if !key.trim().is_empty()
            )
        }
    }
}

/// Whether AWS credentials the Bedrock provider's default SDK chain would use
/// are present on this host — checked synchronously with no network (never
/// touches IMDS). Mirrors the sources listed in `bedrock.rs`'s
/// "No AWS credentials found" error: explicit access keys, a named profile, an
/// ECS/EKS container or web-identity role, or the shared `~/.aws` files.
fn aws_ambient_credentials_present() -> bool {
    let present = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    // Explicit static keys (both halves required), a named profile, or an
    // ECS/EKS/OIDC role handed to the process via env.
    if (present("AWS_ACCESS_KEY_ID") && present("AWS_SECRET_ACCESS_KEY"))
        || present("AWS_PROFILE")
        || present("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
        || present("AWS_CONTAINER_CREDENTIALS_FULL_URI")
        || present("AWS_WEB_IDENTITY_TOKEN_FILE")
    {
        return true;
    }
    // Shared credentials/config files (honour the standard overrides, else the
    // default `~/.aws/{credentials,config}` locations).
    let home = dirs::home_dir();
    let creds_file = std::env::var_os("AWS_SHARED_CREDENTIALS_FILE")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".aws").join("credentials")));
    let config_file = std::env::var_os("AWS_CONFIG_FILE")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".aws").join("config")));
    creds_file.is_some_and(|p| p.exists()) || config_file.is_some_and(|p| p.exists())
}

/// Whether GCP credentials the Vertex provider would use are present on this
/// host — checked synchronously with no network. Mirrors `vertex.rs`'s
/// resolution order: a `GOOGLE_APPLICATION_CREDENTIALS` service-account file, or
/// gcloud Application Default Credentials at
/// `~/.config/gcloud/application_default_credentials.json`.
fn gcp_ambient_credentials_present() -> bool {
    if std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS").is_some_and(|v| !v.is_empty()) {
        return true;
    }
    dirs::home_dir()
        .map(|h| h.join(".config/gcloud/application_default_credentials.json"))
        .is_some_and(|p| p.exists())
}

/// The built-in providers (from [`KNOWN_PROVIDER_TYPES`]) that have a usable
/// credential right now — see [`provider_connected`]. Used by the model-catalog
/// refresh service to decide which providers to fetch live model lists for, and
/// by the `/provider` picker to separate ready providers from ones that would
/// error on the first turn.
pub fn connected_providers() -> Vec<ProviderType> {
    KNOWN_PROVIDER_TYPES
        .iter()
        .copied()
        .filter(|p| provider_connected(*p))
        .collect()
}

/// Default base URL for `provider` when neither CLI, config, nor a catalog
/// entry supplies one. Extracted from `Config::resolve` so the model-catalog
/// refresh service (`wcore-providers`) can stamp the same URL onto a
/// per-provider discovery `Config` without duplicating the mapping. An empty
/// string means "let the provider supply its own default" (Tier-2 newtypes) or
/// "URL is derived from region/project, not base_url" (Bedrock/Vertex).
pub fn default_base_url_for(provider: ProviderType) -> String {
    match provider {
        ProviderType::Anthropic => "https://api.anthropic.com".into(),
        ProviderType::OpenAI => "https://api.openai.com".into(),
        // Bedrock/Vertex URLs are constructed from region/project, not base_url
        ProviderType::Bedrock | ProviderType::Vertex => String::new(),
        // Mirrors `wcore_providers::gemini::DEFAULT_GEMINI_BASE_URL`.
        // We can't import that here (would create a circular dep:
        // wcore-providers already depends on wcore-config). The
        // provider crate falls back to this same literal when
        // `base_url` is empty, so a future drift here is benign
        // until someone overrides this value mid-stack.
        ProviderType::Gemini => "https://generativelanguage.googleapis.com".into(),
        // v0.6.3 Tier-2 providers: the provider newtype falls back to
        // its own `*_DEFAULT_BASE_URL` const when `base_url` is empty,
        // so leave it empty here and let the provider supply the
        // default. Azure OpenAI is the exception — it has no static
        // default (the resource subdomain is account-specific) and
        // REQUIRES `base_url` to be set; an empty value surfaces as a
        // loud connect error rather than a wrong-host request.
        ProviderType::AzureOpenAI
        | ProviderType::Together
        | ProviderType::Fireworks
        | ProviderType::Nvidia
        | ProviderType::Perplexity
        | ProviderType::Cerebras
        | ProviderType::OpenRouter
        | ProviderType::FluxRouter
        // Sakana's newtype falls back to SAKANA_DEFAULT_BASE_URL when empty.
        | ProviderType::Sakana => String::new(),
        ProviderType::Deepseek | ProviderType::Xai | ProviderType::Groq => String::new(),
        ProviderType::Moonshot | ProviderType::Qwen => String::new(),
        // F-025: Mistral + Cohere fall back to their own default base URLs.
        ProviderType::Mistral | ProviderType::Cohere => String::new(),
        // ChatGPT Codex backend — NOT api.openai.com. The provider
        // appends `/responses` to this base. Mirrors
        // `wcore_providers::openai_chatgpt::CODEX_BASE_URL`.
        ProviderType::OpenAIChatGpt => "https://chatgpt.com/backend-api/codex".into(),
        // MiniMax's Anthropic-compatible endpoint. The reused AnthropicProvider
        // appends `/v1/messages` (and `/v1/models`) to this base.
        ProviderType::MiniMax => "https://api.minimax.io/anthropic".into(),
    }
}

/// The `ProviderCompat` preset for a native (non-catalog) `provider`. Extracted
/// from `Config::resolve` so the model-catalog refresh service can build a
/// per-provider discovery `Config` with the correct wire shape and cost
/// attribution without duplicating the mapping. Catalog (`--provider <id>`)
/// entries do NOT go through here — they use `ProviderCompat::from_catalog_entry`
/// at the call site.
pub fn compat_defaults_for(provider: ProviderType) -> ProviderCompat {
    match provider {
        ProviderType::Anthropic => ProviderCompat::anthropic_defaults(),
        ProviderType::Bedrock => ProviderCompat::bedrock_defaults(),
        ProviderType::Vertex => ProviderCompat::vertex_defaults(),
        ProviderType::Gemini => ProviderCompat::gemini_defaults(),
        ProviderType::OpenAI => ProviderCompat::openai_defaults(),
        ProviderType::AzureOpenAI => ProviderCompat::azure_openai_defaults(),
        ProviderType::Together => ProviderCompat::together_defaults(),
        ProviderType::Fireworks => ProviderCompat::fireworks_defaults(),
        ProviderType::Nvidia => ProviderCompat::nvidia_defaults(),
        ProviderType::Perplexity => ProviderCompat::perplexity_defaults(),
        ProviderType::Cerebras => ProviderCompat::cerebras_defaults(),
        ProviderType::OpenRouter => ProviderCompat::openrouter_defaults(),
        ProviderType::FluxRouter => ProviderCompat::flux_router_defaults(),
        ProviderType::Sakana => ProviderCompat::sakana_defaults(),
        ProviderType::Deepseek => ProviderCompat::deepseek_defaults(),
        ProviderType::Xai => ProviderCompat::xai_defaults(),
        ProviderType::Groq => ProviderCompat::groq_defaults(),
        ProviderType::Moonshot => ProviderCompat::moonshot_defaults(),
        ProviderType::Qwen => ProviderCompat::qwen_defaults(),
        // F-025: Mistral + Cohere wired to reachable compat defaults.
        ProviderType::Mistral => ProviderCompat::mistral_defaults(),
        ProviderType::Cohere => ProviderCompat::cohere_defaults(),
        // ChatGPT Codex: OpenAI Responses wire format, effort levels,
        // provider id "openai-chatgpt" for cost attribution.
        ProviderType::OpenAIChatGpt => ProviderCompat::chatgpt_defaults(),
        ProviderType::MiniMax => ProviderCompat::minimax_defaults(),
    }
}

impl Config {
    /// Derive a single-purpose `Config` for live model discovery of `provider`,
    /// reusing `self` for everything but the provider-identifying fields.
    ///
    /// Overrides exactly four fields so `create_native_provider` constructs the
    /// right client: `provider`, the resolved `api_key` (config/store/env
    /// chain — empty for ambient cloud), the default `base_url`, and the compat
    /// preset (wire shape + cost attribution). Every other field (debug,
    /// prompt_caching, bedrock/vertex sub-configs, …) is inherited from `self`
    /// so the discovery client matches the base environment.
    ///
    /// `provider_label` is set to the canonical slug so the constructed
    /// provider's cost attribution and any label-keyed logging read correctly.
    /// The model is left as `self.model` — `list_models` does not consult it.
    pub fn for_provider_discovery(&self, provider: ProviderType) -> Self {
        let storage = crate::credentials::CredentialsStorageConfig::default();
        let api_key = resolve_api_key(None, None, provider, &storage).unwrap_or_default();
        Self {
            provider,
            provider_label: provider_type_slug(provider).to_string(),
            api_key,
            base_url: default_base_url_for(provider),
            compat: compat_defaults_for(provider),
            ..self.clone()
        }
    }

    /// Like [`for_provider_discovery`](Self::for_provider_discovery), but binds
    /// an explicitly-supplied `api_key` instead of resolving one from storage or
    /// the environment. This is the seam for the `/config` paste-to-detect flow:
    /// it lets the engine probe a *just-pasted* key against a candidate provider
    /// (via `list_models`) before the key is ever written to disk. The provider
    /// identity, base URL, and compat preset are stamped from `provider`; the
    /// model is irrelevant to `list_models` and is left as `self.model`.
    pub fn for_key_validation(&self, provider: ProviderType, api_key: &str) -> Self {
        Self {
            provider,
            provider_label: provider_type_slug(provider).to_string(),
            api_key: api_key.to_string(),
            base_url: default_base_url_for(provider),
            compat: compat_defaults_for(provider),
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedProviderConfig {
    requested_name: String,
    provider_type: ProviderType,
    effective_config: ProviderConfig,
    /// Set when `requested_name` matched a bundled data-driven catalog entry
    /// (rather than a built-in `ProviderType` or a user alias). The catalog
    /// path resolves to `ProviderType::OpenAI` for wire construction but
    /// carries the entry so the resolver can stamp the catalog `base_url`,
    /// the catalog-derived `compat` (id + api_path), and read the key from
    /// the entry's `env_var`.
    catalog_entry: Option<crate::catalog::CatalogEntry>,
}

/// CLI arguments needed for config resolution
#[derive(Default)]
pub struct CliArgs {
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub max_turns: Option<usize>,
    pub system_prompt: Option<String>,
    pub profile: Option<String>,
    pub auto_approve: bool,
    pub project_dir: Option<PathBuf>,
}

impl Config {
    /// Load and merge config from all sources
    pub fn resolve(cli: &CliArgs) -> anyhow::Result<Self> {
        // 1. Load global config. D011: a corrupt (parse-failing) file that
        //    EXISTS must propagate a typed error here rather than silently
        //    downgrade to defaults — silent defaulting wipes the user's whole
        //    config and reads as a fresh install. A genuinely-absent file
        //    still yields defaults (handled inside try_load_config_file).
        let global = try_load_config_file(&global_config_path())?;

        // 2. Load project config (from project_dir if specified, else CWD).
        //    Same dataloss-safe contract as the global file.
        let project_path = cli
            .project_dir
            .as_ref()
            .map(|d| d.join(".wayland-core.toml"))
            .unwrap_or_else(project_config_path);
        let project = try_load_config_file(&project_path)?;

        // 3. Merge: global <- project
        let mut merged = merge_config_files(global, project);

        // 4. If --profile specified, overlay profile settings
        if let Some(profile_name) = &cli.profile {
            merged = apply_profile(merged, profile_name)?;
        }

        // 5. Apply CLI overrides and resolve final config
        let provider_str = cli.provider.as_deref().unwrap_or(&merged.default.provider);

        let resolved_provider = resolve_provider_alias(&merged.providers, provider_str)?;
        let provider_label = resolved_provider.requested_name.clone();
        let provider = resolved_provider.provider_type;
        let provider_config = resolved_provider.effective_config;
        // Set only when `--provider <id>` matched a bundled data-driven catalog
        // entry (resolves to ProviderType::OpenAI). Used below to stamp the
        // catalog base_url, the catalog-derived compat, and the env-var key.
        let catalog_entry = resolved_provider.catalog_entry;

        let base_url = cli
            .base_url
            .clone()
            .or_else(|| provider_config.base_url.clone())
            .or_else(|| catalog_entry.as_ref().map(|e| e.base_url.clone()))
            .unwrap_or_else(|| default_base_url_for(provider));

        let raw_model = cli
            .model
            .clone()
            .or(provider_config.model.clone())
            .or(merged.default.model.clone())
            .unwrap_or_else(|| {
                // Catalog providers resolve to ProviderType::OpenAI but host
                // heterogeneous model catalogs — there is no sensible default
                // (OPENAI_GPT4O would not exist on e.g. Novita). Mirror the
                // Tier-2 contract: empty string, forcing the user to set
                // `--model`; an unset model surfaces as an honest API error.
                if catalog_entry.is_some() {
                    String::new()
                } else {
                    default_model_for(provider).to_string()
                }
            });
        // Expand `<provider>:<role>` short-forms (e.g. `bedrock:sonnet` →
        // full Bedrock literal). Literals without a known prefix flow
        // through unchanged — see `wcore_types::model_aliases::expand_short_form`
        // for the exact rule set. Closes debt B.4 (HC-3-followup).
        let model = wcore_types::model_aliases::expand_short_form(&raw_model)
            .map(str::to_string)
            .unwrap_or(raw_model);

        let max_tokens = cli.max_tokens.unwrap_or(merged.default.max_tokens);
        let max_turns = cli.max_turns.or(merged.default.max_turns);
        let approval_mode = merged.default.approval_mode;

        let system_prompt = cli
            .system_prompt
            .clone()
            .or(merged.default.system_prompt.clone());

        // 6. Resolve API key: CLI > config file > store > env var.
        //    Wave SD: the credentials store (plaintext-with-0o600 or
        //    keyring) is consulted between the inline config field and
        //    the env-var fallback, closing SECURITY MAJOR #16's
        //    "plaintext in config.toml only" pathway.
        // A catalog provider resolves to ProviderType::OpenAI, which is unknown
        // to `resolve_api_key` -- it only tries OPENAI_API_KEY / API_KEY. A user
        // who set the provider's OWN documented env var (e.g. NOVITA_API_KEY)
        // must have it honored as a fallback HERE, in BOTH cases: when the
        // standard chain errors (no OPENAI_API_KEY -> MissingApiKey) and when it
        // resolves to an empty key. Resolve it once up front so it covers both
        // paths -- previously the Err case short-circuited on the `?` BEFORE
        // this fallback ran, so a valid catalog credential in the entry's env
        // var produced a spurious "No API key found".
        let catalog_env_key = (cli.api_key.is_none() && provider_config.api_key.is_none())
            .then(|| {
                catalog_entry
                    .as_ref()
                    .and_then(|e| std::env::var(&e.env_var).ok())
            })
            .flatten();
        let mut api_key = match resolve_api_key(
            cli.api_key.as_deref(),
            provider_config.api_key.as_deref(),
            provider,
            &merged.storage.credentials,
        ) {
            Ok(key) => key,
            // The standard chain found nothing; honor the catalog entry's own
            // env var before surfacing MissingApiKey.
            Err(e) => match catalog_env_key.clone() {
                Some(key) => key,
                None => return Err(e),
            },
        };
        // The chain resolved to an empty key but a catalog env var is
        // also present -- the explicit catalog credential wins.
        if api_key.is_empty()
            && let Some(key) = catalog_env_key
        {
            api_key = key;
        }

        // 7. Apply auto_approve from CLI
        let mut tools = merged.tools;
        if cli.auto_approve {
            tools.auto_approve = true;
        }

        // Resolve prompt_caching: default true for Anthropic
        let prompt_caching = provider_config
            .prompt_caching
            .unwrap_or(matches!(provider, ProviderType::Anthropic));

        // Resolve compat: provider-type defaults + user overrides.
        //
        // D.2 (v0.6.3) — the 6 Tier-2 providers share the OpenAI *wire*
        // shape but each gets its own preset so `provider_type` carries the
        // real provider id. Reusing `openai_defaults()` verbatim mislabelled
        // their cost attribution as `"openai"` and charged them GPT-class
        // rates ($8/$32 per Mtok) for cheap open-weight models. Each
        // dedicated preset stamps the real id and clears the inline cost
        // rows so pricing resolves via the `wcore-pricing` catalog.
        // A catalog provider resolves to ProviderType::OpenAI but must NOT use
        // `openai_defaults()` — that mislabels cost attribution as "openai" and
        // charges GPT-class rates. Derive the compat from the catalog entry so
        // `provider_type` carries the real id, the cost rows are the $0
        // sentinel (catalog-resolved pricing), and `api_path` lands the request
        // on the right endpoint. Native `--provider openai` (no catalog entry)
        // keeps `openai_defaults()` unchanged.
        let compat_defaults = if let Some(entry) = catalog_entry.as_ref() {
            ProviderCompat::from_catalog_entry(&entry.id, entry.api_path.as_deref())
        } else {
            compat_defaults_for(provider)
        };

        let user_compat = provider_config.compat.clone().unwrap_or_default();

        let mut compat = ProviderCompat::merge(compat_defaults, user_compat.clone());

        // F-088: for OpenAI, gate the effort capability advertisement on
        // whether the requested model actually accepts `reasoning_effort`.
        // The per-request gate (openai_compat::accepts_reasoning_effort) already
        // blocks the field from the API body for non-reasoning models; this
        // fix brings the `ready` event's `effort` flag into alignment so the
        // host UI doesn't show a reasoning-effort slider for gpt-4o and family.
        // Only applies when the user hasn't explicitly overridden the compat
        // (user_compat.supports_effort = None → we may adjust; Some(_) → honour
        // their explicit setting).
        if provider == ProviderType::OpenAI
            && user_compat.supports_effort.is_none()
            && compat.supports_effort.unwrap_or(false)
        {
            // `model` is resolved below; grab the effective model string now.
            let effective_model = cli
                .model
                .as_deref()
                .unwrap_or_else(|| provider_config.model.as_deref().unwrap_or(""));
            if !effective_model.is_empty() && !openai_model_accepts_effort(effective_model) {
                compat.supports_effort = Some(false);
                compat.effort_levels = Some(vec![]);
            }
        }

        Ok(Config {
            provider_label,
            provider,
            api_key,
            base_url,
            model,
            max_tokens,
            // Crucible #3: the top-level session leaves temperature unset; the
            // council sets per-tier temperatures via SubAgentConfig downstream.
            temperature: None,
            max_turns,
            approval_mode,
            system_prompt,
            thinking: None,
            prompt_caching,
            compat,
            tools,
            builtin_tools: crate::tools::BuiltinToolsConfig::default(),
            advertised_capabilities: crate::tools::AdvertisedCapabilitiesConfig::default(),
            session: merged.session,
            inbound_webhook: merged.inbound_webhook,
            compact: merged.compact,
            plan: merged.plan,
            file_cache: merged.file_cache,
            hooks: merged.hooks,
            bedrock: merged.bedrock,
            vertex: merged.vertex,
            mcp: merged.mcp,
            debug: merged.debug,
            observability: merged.observability,
            provider_chain: merged.provider_chain,
            budget: merged.budget,
            storage: merged.storage,
            // Absent `[memory]` resolves to the (memory-ON) default.
            memory: merged.memory.unwrap_or_default(),
            browser: merged.browser,
            security: merged.security,
            session_cap: merged.session_cap,
            crucible: merged.crucible,
        })
    }

    /// Wave SD — open the configured credentials store. The plaintext
    /// backend lands beside the main config file (so the existing
    /// `secure_config_file` step covers it); the keyring backend
    /// uses the configured service name (default `"wayland-core"`).
    ///
    /// Returns Err on transient backend errors (e.g. keyring locked).
    pub fn open_credentials_store(
        &self,
    ) -> Result<Box<dyn crate::credentials::CredentialsStore>, crate::credentials::CredentialsError>
    {
        crate::credentials::open_store(&self.storage.credentials, &credentials_storage_path())
    }
}

/// Wave SD — path used by the plaintext credentials backend. Lives next
/// to `config.toml` so the same parent dir / perms hardening applies.
pub fn credentials_storage_path() -> PathBuf {
    app_config_dir()
        .unwrap_or_else(|| PathBuf::from("wayland-core"))
        .join("credentials.toml")
}

fn parse_builtin_provider(s: &str) -> Option<ProviderType> {
    match s {
        "anthropic" => Some(ProviderType::Anthropic),
        "openai" => Some(ProviderType::OpenAI),
        "bedrock" => Some(ProviderType::Bedrock),
        "vertex" => Some(ProviderType::Vertex),
        // F-027: "google" is a natural alias users try with GOOGLE_API_KEY.
        // Route to the native Gemini provider which uses an API key directly.
        "gemini" | "google" => Some(ProviderType::Gemini),
        // v0.6.3 Tier-2 OpenAI-compatible providers (D.1 Round 1 cleanup).
        "azure-openai" | "azure" => Some(ProviderType::AzureOpenAI),
        "together" => Some(ProviderType::Together),
        "fireworks" => Some(ProviderType::Fireworks),
        "nvidia" => Some(ProviderType::Nvidia),
        "perplexity" => Some(ProviderType::Perplexity),
        "cerebras" => Some(ProviderType::Cerebras),
        // v0.8.1 U10a: router-class OpenAI-compatible endpoints.
        "openrouter" => Some(ProviderType::OpenRouter),
        "flux-router" | "flux" => Some(ProviderType::FluxRouter),
        // Sakana AI ("Fugu") — OpenAI-compatible. "fugu" is the natural
        // model-brand alias users reach for.
        "sakana" | "fugu" => Some(ProviderType::Sakana),
        // v0.8.1 U10b: native OpenAI-compatible providers.
        "deepseek" => Some(ProviderType::Deepseek),
        "xai" | "grok" => Some(ProviderType::Xai),
        "groq" => Some(ProviderType::Groq),
        // v0.8.1 U10e: Moonshot (Kimi) + Alibaba Qwen (DashScope).
        // Aliases mirror how the upstream APIs are spelled in the wild:
        // "kimi" is the model brand for Moonshot; "alibaba"/"dashscope"
        // are documented synonyms for Qwen.
        "moonshot" | "kimi" => Some(ProviderType::Moonshot),
        "qwen" | "alibaba" | "dashscope" => Some(ProviderType::Qwen),
        // F-025: Mistral + Cohere wired from orphan modules to reachable arms.
        // LiteLLM/LmStudio/Vllm deleted per DECISIONS.md §D3 — revivable as
        // plugins if local-runtime support is needed again.
        "mistral" => Some(ProviderType::Mistral),
        "cohere" => Some(ProviderType::Cohere),
        // "Sign in with ChatGPT" — OAuth-backed Codex backend. "chatgpt" is the
        // natural short alias; "openai-chatgpt" is the canonical id.
        "openai-chatgpt" | "chatgpt" => Some(ProviderType::OpenAIChatGpt),
        // MiniMax via its Anthropic-compatible endpoint. "minimaxi" mirrors the
        // domain spelling some of MiniMax's own docs/SDKs use.
        "minimax" | "minimaxi" => Some(ProviderType::MiniMax),
        _ => None,
    }
}

/// Canonical human-readable list of all built-in provider names.
///
/// F-027: used in the "Unknown provider" error message so users see the full
/// current list (22 names) rather than the stale 4-name string that was
/// hardcoded at the call site. Keep in sync with `parse_builtin_provider`.
pub const BUILTIN_PROVIDER_NAMES: &str = "anthropic, openai, bedrock, vertex, gemini (alias: google), \
     azure-openai (alias: azure), together, fireworks, nvidia, perplexity, \
     cerebras, openrouter, flux-router (alias: flux), deepseek, xai (alias: grok), \
     groq, moonshot (alias: kimi), qwen (aliases: alibaba, dashscope), \
     mistral, cohere, openai-chatgpt (alias: chatgpt), sakana (alias: fugu)";

fn merge_provider_configs(base: ProviderConfig, overlay: ProviderConfig) -> ProviderConfig {
    ProviderConfig {
        provider: overlay.provider.or(base.provider),
        model: overlay.model.or(base.model),
        api_key: overlay.api_key.or(base.api_key),
        base_url: overlay.base_url.or(base.base_url),
        prompt_caching: overlay.prompt_caching.or(base.prompt_caching),
        compat: match (base.compat, overlay.compat) {
            (Some(base), Some(overlay)) => Some(ProviderCompat::merge(base, overlay)),
            (Some(base), None) => Some(base),
            (None, Some(overlay)) => Some(overlay),
            (None, None) => None,
        },
    }
}

fn resolve_provider_alias(
    providers: &HashMap<String, ProviderConfig>,
    requested: &str,
) -> anyhow::Result<ResolvedProviderConfig> {
    if let Some(provider_type) = parse_builtin_provider(requested) {
        return Ok(ResolvedProviderConfig {
            requested_name: requested.to_string(),
            provider_type,
            effective_config: providers.get(requested).cloned().unwrap_or_default(),
            catalog_entry: None,
        });
    }

    // Data-driven catalog fallthrough: a `--provider <id>` that is neither a
    // built-in nor a user alias may still match a bundled OpenAI-compatible
    // catalog entry. Native arms always win (checked first, above), so a
    // native-collision id never reaches here. The catalog entry resolves to
    // the OpenAI wire path; the caller stamps base_url/compat/key from it.
    if !providers.contains_key(requested)
        && let Some(catalog) = crate::catalog::ProviderCatalog::bundled()
        && let Some(entry) = catalog.get(requested)
    {
        return Ok(ResolvedProviderConfig {
            requested_name: requested.to_string(),
            // Guarded by `!providers.contains_key(requested)`, so there is no
            // user-config overlay for a bare catalog id; base_url/compat/key
            // are stamped from the entry by the resolver.
            provider_type: ProviderType::OpenAI,
            effective_config: ProviderConfig::default(),
            catalog_entry: Some(entry.clone()),
        });
    }

    // F-027: error message now lists all 20+ built-in providers instead of the
    // stale 4-name string. Also note that google → gemini is already handled
    // by parse_builtin_provider above, so users will never reach this error
    // for "google".
    let alias_config = providers.get(requested).cloned().ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown provider: '{}'. Expected a built-in provider ({}) \
             or a custom alias defined in [providers.{}].",
            requested,
            BUILTIN_PROVIDER_NAMES,
            requested
        )
    })?;

    let underlying = alias_config.provider.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "Provider alias '{}' requires a 'provider' field in [providers.{}] \
             that maps to a built-in type ({}).",
            requested,
            requested,
            BUILTIN_PROVIDER_NAMES
        )
    })?;

    let provider_type = parse_builtin_provider(&underlying).ok_or_else(|| {
        anyhow::anyhow!(
            "Provider alias '{}' maps to '{}', which is not a built-in provider. \
             Use one of: {}.",
            requested,
            underlying,
            BUILTIN_PROVIDER_NAMES
        )
    })?;

    Ok(ResolvedProviderConfig {
        requested_name: requested.to_string(),
        provider_type,
        effective_config: merge_provider_configs(
            providers.get(&underlying).cloned().unwrap_or_default(),
            alias_config,
        ),
        catalog_entry: None,
    })
}

/// Error raised while resolving a cross-provider council member.
///
/// The council treats these two cases differently: an [`Unknown`](Self::Unknown)
/// provider id is a configuration error the caller should surface, whereas a
/// [`Keyless`](Self::Keyless) provider is a BYO-key member the council simply
/// *skips* (a user who hasn't supplied a key for one council provider should
/// still get a council from the providers they have keyed).
#[derive(Debug, thiserror::Error)]
pub enum CouncilProviderError {
    /// The provider id is neither a built-in provider, a `[providers]` alias,
    /// nor a bundled catalog entry.
    #[error("unknown council provider '{0}'")]
    Unknown(String),
    /// The provider resolved, but no usable api key could be found (inline
    /// config, credentials store, or env var). Skip, don't fail.
    #[error("council provider '{0}' has no usable api key")]
    Keyless(String),
}

/// Resolve a council `spec` (`"provider"` or `"provider:model"`) into a fully
/// keyed runtime [`Config`] for that provider, reusing the exact same alias /
/// catalog / credential / compat resolution as [`Config::resolve`].
///
/// This is the keyed-provider helper the cross-provider council needs: unlike a
/// resolver seeded from a single already-resolved `Config` (which carries only
/// one provider's `api_key`), this consults the on-disk `[providers]` map so it
/// can pull each council member's *own* credentials. Every non-provider runtime
/// setting (max_tokens, max_turns, tools, storage, observability, …) is
/// inherited verbatim from `base` so council members share the session's policy
/// surface and differ only in provider identity, endpoint, model, and key.
///
/// Returns the derived `Config` plus the resolved model (the spec's pinned
/// model if given, else the provider/config default when non-empty; `None` for
/// catalog providers with no default — the API surfaces an honest error).
///
/// Intentional divergences from [`Config::resolve`] (all by design, not bugs):
/// - No CLI override rungs (`--provider`/`--model`/`--api-key`/`--base-url`) —
///   the council never takes CLI args.
/// - No `[default].model` fallback in model resolution. The session default
///   model belongs to the *primary* provider; seeding it onto a different
///   council provider (e.g. an Anthropic-shaped literal onto an OpenAI member)
///   would be wrong. `base` is an already-resolved `Config`, so the on-disk
///   `[default]` block isn't reachable here anyway.
/// - `thinking` is inherited from `base` (whereas `Config::resolve` hard-sets
///   `None`). Identical whenever `base` itself came from `Config::resolve`.
/// - The F-088 OpenAI effort-capability gate uses the fully-resolved model
///   string (more accurate than `Config::resolve`'s pre-expansion check).
pub fn resolve_council_provider(
    providers: &HashMap<String, ProviderConfig>,
    base: &Config,
    spec: &str,
) -> Result<(Config, Option<String>), CouncilProviderError> {
    // Split on the FIRST ':' → (provider_id, model?). A bare "provider" has no
    // model; "provider:model" pins the model.
    let (provider_id, spec_model) = match spec.split_once(':') {
        Some((id, model)) => (id, Some(model.to_string())),
        None => (spec, None),
    };

    // Reuse the full alias + catalog + merge resolution. Any failure here means
    // the id matched nothing resolvable → Unknown (the council surfaces it).
    let resolved = resolve_provider_alias(providers, provider_id)
        .map_err(|_| CouncilProviderError::Unknown(provider_id.to_string()))?;
    let provider = resolved.provider_type;
    let provider_config = resolved.effective_config;
    let catalog_entry = resolved.catalog_entry;

    let base_url = provider_config
        .base_url
        .clone()
        .or_else(|| catalog_entry.as_ref().map(|e| e.base_url.clone()))
        .unwrap_or_else(|| default_base_url_for(provider));

    let raw_model = spec_model
        .clone()
        .or_else(|| provider_config.model.clone())
        .unwrap_or_else(|| {
            // Catalog providers host heterogeneous catalogs with no sensible
            // default — mirror Config::resolve and leave it empty so the user
            // must pin a model (an unset model surfaces as an honest API error).
            if catalog_entry.is_some() {
                String::new()
            } else {
                default_model_for(provider).to_string()
            }
        });
    let model = wcore_types::model_aliases::expand_short_form(&raw_model)
        .map(str::to_string)
        .unwrap_or(raw_model);

    // Credentials: inline config key → store → env var (per provider), plus the
    // catalog entry's own env var as a fallback — exactly Config::resolve's
    // chain, with no CLI key (the council never takes a CLI `--api-key`).
    let catalog_env_key = provider_config
        .api_key
        .is_none()
        .then(|| {
            catalog_entry
                .as_ref()
                .and_then(|e| std::env::var(&e.env_var).ok())
        })
        .flatten();
    // The keyless decision keys off the Ok/Err *distinction*, NOT string
    // emptiness. `resolve_api_key` returns `Ok("")` by design for providers
    // that authenticate out-of-band — Bedrock/Vertex (cloud creds), ChatGPT
    // (OAuth), xAI (when OAuth creds are present). Those are valid council
    // members and MUST be built, not skipped. It returns `Err(MissingApiKey)`
    // only when no credential was found anywhere; that case (with no catalog
    // env var) is the genuine BYO-key-missing member the council skips.
    let api_key = match resolve_api_key(
        None,
        provider_config.api_key.as_deref(),
        provider,
        &base.storage.credentials,
    ) {
        // A real inline / store / env key.
        Ok(key) if !key.is_empty() => key,
        // Out-of-band auth → legitimately empty inline key; build it. (A catalog
        // env var, if somehow set for this id, still wins — mirrors resolve().)
        Ok(empty) => catalog_env_key.clone().unwrap_or(empty),
        // Nothing found anywhere: honor a catalog env var, else this is a
        // keyless BYO member the council skips (not fatal).
        Err(_) => match catalog_env_key.clone() {
            Some(key) => key,
            None => return Err(CouncilProviderError::Keyless(provider_id.to_string())),
        },
    };

    let prompt_caching = provider_config
        .prompt_caching
        .unwrap_or(matches!(provider, ProviderType::Anthropic));

    let compat_defaults = if let Some(entry) = catalog_entry.as_ref() {
        ProviderCompat::from_catalog_entry(&entry.id, entry.api_path.as_deref())
    } else {
        compat_defaults_for(provider)
    };
    let user_compat = provider_config.compat.clone().unwrap_or_default();
    let mut compat = ProviderCompat::merge(compat_defaults, user_compat.clone());

    // F-088: align the advertised effort capability with what the resolved
    // model actually accepts (only when the user hasn't pinned it explicitly).
    if provider == ProviderType::OpenAI
        && user_compat.supports_effort.is_none()
        && compat.supports_effort.unwrap_or(false)
        && !model.is_empty()
        && !openai_model_accepts_effort(&model)
    {
        compat.supports_effort = Some(false);
        compat.effort_levels = Some(vec![]);
    }

    let resolved_model = if model.is_empty() {
        None
    } else {
        Some(model.clone())
    };

    // Inherit every non-provider runtime field from `base`; overwrite only the
    // provider identity, endpoint, model, key, and provider-derived compat.
    let derived = Config {
        provider,
        provider_label: resolved.requested_name.clone(),
        api_key,
        base_url,
        model,
        prompt_caching,
        compat,
        ..base.clone()
    };

    Ok((derived, resolved_model))
}

fn resolve_api_key(
    cli_key: Option<&str>,
    config_key: Option<&str>,
    provider: ProviderType,
    storage: &crate::credentials::CredentialsStorageConfig,
) -> anyhow::Result<String> {
    // CLI arg takes precedence
    if let Some(key) = cli_key {
        return Ok(key.to_string());
    }

    // Config file value
    if let Some(key) = config_key {
        return Ok(key.to_string());
    }

    // Wave SD — credentials store: plaintext-with-0o600 or OS keyring.
    // Keyed by `providers.<provider>.api_key`. Errors are non-fatal here
    // (e.g. keyring locked); we fall through to env/OAuth.
    if let Ok(store) = crate::credentials::open_store(storage, &credentials_storage_path())
        && let Some(key) = lookup_store_api_key(&*store, provider)
    {
        return Ok(key);
    }

    // Env var fallback chain
    if let Ok(key) = std::env::var("API_KEY") {
        return Ok(key);
    }

    match provider {
        ProviderType::Anthropic => {
            if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::OpenAI => {
            if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                return Ok(key);
            }
        }
        // Bedrock uses AWS credentials, Vertex uses GCP credentials
        // They don't need a traditional API key
        ProviderType::Bedrock | ProviderType::Vertex => {
            return Ok(String::new());
        }
        // ChatGPT Codex authenticates via OAuth tokens resolved out-of-band by
        // the bootstrap-built bearer source (same shape as Bedrock/Vertex — no
        // inline API key). Returning an empty key here keeps config resolution
        // from erroring with MissingApiKey when no OPENAI_API_KEY is set.
        ProviderType::OpenAIChatGpt => {
            return Ok(String::new());
        }
        ProviderType::Gemini => {
            // Native Gemini uses an API key (NOT GCP OAuth — that's Vertex).
            // Standard env vars per Google's CLI samples.
            if let Ok(key) = std::env::var("GEMINI_API_KEY") {
                return Ok(key);
            }
            if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
                return Ok(key);
            }
        }
        // v0.6.3 Tier-2 providers each take a static API key via their
        // canonical env var (matches the provider's own docs/SDK conventions).
        ProviderType::AzureOpenAI => {
            if let Ok(key) = std::env::var("AZURE_OPENAI_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Together => {
            if let Ok(key) = std::env::var("TOGETHER_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Fireworks => {
            if let Ok(key) = std::env::var("FIREWORKS_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Nvidia => {
            if let Ok(key) = std::env::var("NVIDIA_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Perplexity => {
            if let Ok(key) = std::env::var("PERPLEXITY_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Cerebras => {
            if let Ok(key) = std::env::var("CEREBRAS_API_KEY") {
                return Ok(key);
            }
        }
        // v0.8.1 U10a — router-class OpenAI-compat providers.
        ProviderType::OpenRouter => {
            if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::FluxRouter => {
            if let Ok(key) = std::env::var("FLUX_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Deepseek => {
            if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Xai => {
            // Grok "Sign in with X" authenticates via OAuth tokens resolved
            // out-of-band by the bootstrap-built bearer source (same shape as
            // ChatGPT). Exempt from the api-key gate when an xAI OAuth
            // credential exists — otherwise a plain `xai` API key still works
            // via XAI_API_KEY below.
            if xai_oauth_credentials_present() {
                return Ok(String::new());
            }
            if let Ok(key) = std::env::var("XAI_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Groq => {
            if let Ok(key) = std::env::var("GROQ_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Moonshot => {
            if let Ok(key) = std::env::var("MOONSHOT_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Qwen => {
            // DashScope is canonical; ALIBABA_API_KEY is a documented alias.
            if let Ok(key) = std::env::var("DASHSCOPE_API_KEY") {
                return Ok(key);
            }
            if let Ok(key) = std::env::var("ALIBABA_API_KEY") {
                return Ok(key);
            }
        }
        // F-025: Mistral + Cohere key resolution.
        ProviderType::Mistral => {
            if let Ok(key) = std::env::var("MISTRAL_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Cohere => {
            if let Ok(key) = std::env::var("COHERE_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::MiniMax => {
            if let Ok(key) = std::env::var("MINIMAX_API_KEY") {
                return Ok(key);
            }
        }
        ProviderType::Sakana => {
            if let Ok(key) = std::env::var("SAKANA_API_KEY") {
                return Ok(key);
            }
        }
    }

    Err(MissingApiKey.into())
}

/// No credential could be resolved for the active provider — CLI flag, config
/// field, credentials store, and every env-var fallback all came up empty.
///
/// Typed (rather than a bare `anyhow!` string) so the CLI entrypoint can tell a
/// *recoverable* "needs setup" condition apart from a hard config error like a
/// TOML [`ConfigLoadError::ParseFailed`]. On an interactive launch the former
/// routes into the Onboarding surface for in-app recovery; the latter must
/// still abort visibly (D011 dataloss guard). The `Display` text is the
/// original user-facing guidance, unchanged, so callers that match on the
/// message keep working.
#[derive(Debug, thiserror::Error)]
#[error(
    "No API key found. Provide via --api-key, config file, or environment variable \
     (API_KEY, ANTHROPIC_API_KEY, or OPENAI_API_KEY)."
)]
pub struct MissingApiKey;

/// The credentials-store key under which `provider`'s API key is stored, or
/// `None` for providers that authenticate out-of-band (Bedrock/Vertex via cloud
/// credentials, ChatGPT Codex via OAuth) and therefore have no store slot.
///
/// This is the single source of truth for the mapping: both the read path
/// ([`lookup_store_api_key`], consumed by [`resolve_api_key`]) and the write
/// path ([`store_provider_api_key`]) go through it, so a key written here is
/// guaranteed to be the key resolution later reads back.
pub fn credentials_store_key(provider: ProviderType) -> Option<String> {
    let key = match provider {
        ProviderType::Anthropic => "providers.anthropic.api_key",
        ProviderType::OpenAI => "providers.openai.api_key",
        ProviderType::Bedrock | ProviderType::Vertex => return None,
        // ChatGPT Codex has no credentials-store API key — auth is OAuth.
        ProviderType::OpenAIChatGpt => return None,
        ProviderType::Gemini => "providers.gemini.api_key",
        // v0.6.3 Tier-2 providers — credentials store path keyed by id.
        ProviderType::AzureOpenAI => "providers.azure-openai.api_key",
        ProviderType::Together => "providers.together.api_key",
        ProviderType::Fireworks => "providers.fireworks.api_key",
        ProviderType::Nvidia => "providers.nvidia.api_key",
        ProviderType::Perplexity => "providers.perplexity.api_key",
        ProviderType::Cerebras => "providers.cerebras.api_key",
        // v0.8.1 U10a — router-class providers.
        ProviderType::OpenRouter => "providers.openrouter.api_key",
        ProviderType::FluxRouter => "providers.flux-router.api_key",
        ProviderType::Deepseek => "providers.deepseek.api_key",
        ProviderType::Xai => "providers.xai.api_key",
        ProviderType::Groq => "providers.groq.api_key",
        ProviderType::Moonshot => "providers.moonshot.api_key",
        ProviderType::Qwen => "providers.qwen.api_key",
        // F-025: Mistral + Cohere key resolution from credentials store.
        ProviderType::Mistral => "providers.mistral.api_key",
        ProviderType::Cohere => "providers.cohere.api_key",
        ProviderType::MiniMax => "providers.minimax.api_key",
        ProviderType::Sakana => "providers.sakana.api_key",
    };
    Some(key.to_string())
}

fn lookup_store_api_key(
    store: &dyn crate::credentials::CredentialsStore,
    provider: ProviderType,
) -> Option<String> {
    let key = credentials_store_key(provider)?;
    store.get(&key).ok().flatten()
}

/// Persist a validated API key for `provider` into the configured credentials
/// store — the same store [`resolve_api_key`] reads from — so a subsequent
/// [`Config::resolve`] (e.g. a live engine rebind) picks it up without a
/// restart and without mutating process environment variables.
///
/// The storage backend (keyring / plaintext-0600 / encrypted-file) is read
/// from the on-disk `[storage.credentials]` block of the *profile-active*
/// config — `load_global_config_file()` and `credentials_storage_path()` both
/// honour `WAYLAND_HOME`, so under an isolated profile this reads that
/// profile's config and writes into that profile's in-home store (the Auto
/// default resolves to the in-home vault, never the shared keyring). Returns
/// an error for providers with no store slot
/// ([`credentials_store_key`] returns `None`) or on a store write failure. The
/// value is never logged.
pub fn store_provider_api_key(provider: ProviderType, api_key: &str) -> anyhow::Result<()> {
    let Some(store_key) = credentials_store_key(provider) else {
        anyhow::bail!(
            "provider {} authenticates out-of-band and has no credentials-store API key",
            provider_type_slug(provider)
        );
    };

    // Resolve the SAME storage backend resolution will later read from: the
    // on-disk `[storage.credentials]` block (defaulted when the file or the
    // block is absent).
    let storage = load_global_config_file()
        .map(|f| f.storage.credentials)
        .unwrap_or_default();

    let store = crate::credentials::open_store(&storage, &credentials_storage_path())?;
    store
        .put(&store_key, api_key)
        .map_err(|e| anyhow::anyhow!("writing {store_key} to credentials store: {e}"))?;
    Ok(())
}

/// Load and parse the global `config.toml` into a [`ConfigFile`], or `None`
/// when the file does not exist. Mirrors the load half of
/// [`patch_global_config`] without mutating or rewriting the file.
fn load_global_config_file() -> Option<ConfigFile> {
    let path = global_config_path();
    let raw = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&raw).ok()
}

// --- App directories ---

/// Canonical config-dir resolver that honours `WAYLAND_HOME`.
///
/// Resolution order (F-010):
///   1. `$WAYLAND_HOME`                     (explicit sandbox / hermetic env)
///   2. `$XDG_DATA_HOME/wayland-core`       (XDG-compliant, Linux-preferred)
///   3. `dirs::config_dir()/wayland-core`   (platform native — macOS/Windows)
///
/// All config, auth, session, and sentinel paths **must** go through this
/// helper so that setting `WAYLAND_HOME` hermetically sandboxes every
/// file the engine touches.  This was the root cause of the F-019 key
/// leak: auditor sub-processes inherited the host environment and picked
/// up the real `~/Library/Application Support/wayland-core/auth.json`.
pub fn wayland_config_dir() -> PathBuf {
    if let Ok(wh) = std::env::var("WAYLAND_HOME") {
        return PathBuf::from(wh);
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("wayland-core");
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("wayland-core"))
        .join("wayland-core")
}

/// Platform-aware app config root.
///
/// - Linux:   `~/.config/wayland-core`  (or `$WAYLAND_HOME` / `$XDG_DATA_HOME`)
/// - macOS:   `~/Library/Application Support/wayland-core` (or override)
/// - Windows: `%APPDATA%\wayland-core`  (or override)
///
/// Delegates to [`wayland_config_dir`] so `WAYLAND_HOME` is always honoured.
pub fn app_config_dir() -> Option<PathBuf> {
    Some(wayland_config_dir())
}

/// The OS-native config root (`dirs::config_dir()`), deliberately NOT
/// `WAYLAND_HOME`-scoped. This is the single sanctioned bypass of
/// [`wayland_config_dir`] for the profiles control plane: `profiles_root()`
/// (see [`crate::profile`]) must resolve OUTSIDE any one profile home — a
/// profile home is a *child* of the profiles root — so it cannot route through
/// the `WAYLAND_HOME`-aware resolver without becoming self-referential. Kept
/// here in `config.rs` (the one file allow-listed by the hermeticity audit for
/// raw `dirs::config_dir()`), so the audit's single-call-site invariant holds.
pub(crate) fn os_native_config_root() -> Option<PathBuf> {
    dirs::config_dir()
}

/// Canonical `~/.wayland` profile home.
///
/// This is the stable dot-directory that plugins and their helper processes
/// (e.g. the IJFW MCP memory server) agree on for profile-scoped state. It is
/// distinct from [`wayland_config_dir`], which resolves the platform-native
/// config dir (`~/Library/Application Support/wayland-core` on macOS,
/// `%APPDATA%\wayland-core` on Windows). Plugin installers write under
/// `~/.wayland`, so the host must expose the same root to launched servers.
///
/// Resolution order:
///   1. `$WAYLAND_HOME`            (explicit sandbox / hermetic override)
///   2. `dirs::home_dir()/.wayland` (default, cross-platform)
///
/// Never hardcodes a leading `/` — `dirs::home_dir()` keeps it correct on
/// Windows. Falls back to a relative `.wayland` only if the home dir cannot
/// be resolved at all (headless CI without `$HOME`).
///
/// This lives in `wcore-config` (the lowest crate the others can depend on) to
/// be the canonical resolver. NOTE: the same `$WAYLAND_HOME`-or-`~/.wayland`
/// pattern is currently re-implemented in several call sites (e.g.
/// `wcore_tools::tirith_security::wayland_home`, `wcore-cron`, `wcore-pricing`,
/// `wcore-cli`, `wcore-agent::bootstrap`). Migrating those onto this function is
/// a follow-up consolidation, deliberately out of scope here to keep the change
/// surgical and avoid colliding with concurrent work on those crates.
pub fn profile_home() -> PathBuf {
    // F12: ignore an override containing an ASCII control char (e.g. NUL or a
    // newline). Such a value can't be passed safely to a child env and almost
    // always indicates a corrupt/hostile environment; fall through to the
    // default rather than propagating it.
    if let Ok(wh) = std::env::var("WAYLAND_HOME")
        && !wh.chars().any(|c| c.is_control())
    {
        return PathBuf::from(wh);
    }
    // F12: make the last-resort fallback absolute where possible to avoid
    // CWD-confusion if the home dir can't be resolved.
    dirs::home_dir()
        .map(|h| h.join(".wayland"))
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|d| d.join(".wayland"))
                .unwrap_or_else(|_| PathBuf::from(".wayland"))
        })
}

// --- Config file loading and merging ---

pub fn global_config_path() -> PathBuf {
    app_config_dir()
        .unwrap_or_else(|| PathBuf::from("wayland-core"))
        .join("config.toml")
}

/// Resolve the project-local config path, accepting both layout forms.
///
/// F-011: the eval-harness scaffold writes `.wayland-core/config.toml`
/// (dir form) while the documented layout is `.wayland-core.toml` (file
/// form).  We try the file form first; if absent, fall back to the dir
/// form.  If BOTH are present we warn and use the file form.
fn project_config_path() -> PathBuf {
    let file_form = PathBuf::from(".wayland-core.toml");
    let dir_form = PathBuf::from(".wayland-core").join("config.toml");
    match (file_form.exists(), dir_form.exists()) {
        (true, true) => {
            eprintln!(
                "Warning: both .wayland-core.toml and .wayland-core/config.toml exist; \
                 using .wayland-core.toml (file form). Remove one to silence this warning."
            );
            file_form
        }
        (true, false) => file_form,
        (false, true) => dir_form,
        (false, false) => file_form, // neither exists; return file form (canonical)
    }
}

/// Load + merge the global and project config files into a [`ConfigFile`]
/// WITHOUT resolving them into a runtime [`Config`].
///
/// `Config::resolve` consumes the merged `ConfigFile` and drops the
/// `ConfigFile`-only blocks (`[providers]`, `[crucible]`) once it has extracted
/// the runtime fields. Consumers that need those blocks — e.g. the Crucible
/// council, which keys per-provider credentials from `[providers]` — load the
/// merged file directly here. `project_dir` defaults to the CWD's
/// `.wayland-core.toml` when `None`.
pub fn load_merged_config_file(project_dir: Option<&Path>) -> anyhow::Result<ConfigFile> {
    let global = try_load_config_file(&global_config_path())?;
    let project_path = project_dir
        .map(|d| d.join(".wayland-core.toml"))
        .unwrap_or_else(project_config_path);
    let project = try_load_config_file(&project_path)?;
    Ok(merge_config_files(global, project))
}

/// Read the configured profiles from the global `config.toml`, for the
/// `/profile` listing. Returns `(name, provider, model)` sorted by name —
/// `provider`/`model` are empty strings when the profile leaves them to
/// inheritance/defaults. Reads `global_config_path()` fresh; empty when the
/// file or its `[profiles]` table is absent. (Project-local profiles overlay
/// at resolve time; the listing reflects the global store the user edits.)
pub fn global_profiles() -> Vec<(String, String, String)> {
    let file = load_config_file(&global_config_path());
    let mut out: Vec<(String, String, String)> = file
        .profiles
        .into_iter()
        .map(|(name, p)| {
            (
                name,
                p.provider.unwrap_or_default(),
                p.model.unwrap_or_default(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// D016: read the `[default] user` display name from the global
/// `config.toml`, fresh from disk. Returns `None` when the file is absent
/// or the name is unset/blank. Used by the TUI engine-rebind seam to fold
/// the onboarded name into the live session's system prompt without a
/// restart. The top-level resolved [`Config`] does not carry this field
/// (it is purely cosmetic), so the rebind path reads it directly here.
pub fn global_user_display_name() -> Option<String> {
    load_config_file(&global_config_path())
        .default
        .user
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
}

/// D011 (P0 dataloss): a config file that EXISTS on disk but fails to parse
/// must surface as a hard, typed error — NOT a silent downgrade to defaults.
/// A silent downgrade behaves like a fresh install and discards every user
/// setting (api keys, providers, profiles, mcp servers), and the error was
/// only ever an `eprintln!` hidden behind the TUI alt-screen.
///
/// `Display` deliberately includes the word "parse" and the file path so the
/// boot path can show a dismissable message that names the file and the parse
/// error verbatim.
#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    /// The file exists and was read, but is not valid TOML. Carries the path
    /// (named in the message) and the underlying `toml` parse error. `path` is
    /// a pre-rendered `String` because `PathBuf` does not implement `Display`
    /// (which thiserror's `{path}` needs).
    #[error("failed to parse {path}: {source}")]
    ParseFailed {
        path: String,
        #[source]
        source: toml::de::Error,
    },
}

/// Fallible config-file loader (the D011 dataloss-safe path).
///
/// Distinguishes the two cases that the old `load_config_file` conflated:
/// - **no file** (a fresh install) → `Ok(ConfigFile::default())`. Defaulting
///   here is correct: there is nothing to lose.
/// - **file exists but fails to parse** → `Err(ConfigLoadError::ParseFailed)`.
///   Returning defaults here would silently wipe the user's whole config, so
///   we refuse and surface a typed error the caller can show + abort on. The
///   on-disk file is never read-modified-written on this path, so the user's
///   settings are preserved untouched.
fn try_load_config_file(path: &Path) -> Result<ConfigFile, ConfigLoadError> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            // Wave SD SECURITY MAJOR #16: warn if config file (which may
            // hold api_key / secret_access_key / client_secret) is
            // world-readable, then auto-tighten to 0o600 so the next
            // process start is clean. Best-effort — failing chmod is
            // non-fatal (the warning is the load-bearing signal).
            crate::credentials::warn_if_world_readable(path);
            let _ = crate::credentials::secure_credential_file(path);
            toml::from_str(&content).map_err(|source| ConfigLoadError::ParseFailed {
                path: path.display().to_string(),
                source,
            })
        }
        // No file (or unreadable) → fresh-install defaults are correct.
        Err(_) => Ok(ConfigFile::default()),
    }
}

/// Infallible config-file loader, used only by the read-only `/profile` and
/// display-name listings. These never round-trip the struct back to disk, so a
/// corrupt file degrading to an empty listing is non-destructive — unlike the
/// resolve path, which is the dataloss vector and uses
/// [`try_load_config_file`]. A parse failure here still warns on stderr.
fn load_config_file(path: &Path) -> ConfigFile {
    match try_load_config_file(path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("Warning: {e}");
            ConfigFile::default()
        }
    }
}

/// Apply an in-place patch to the global `config.toml`, preserving every
/// other key already on disk.
///
/// Loads the on-disk [`ConfigFile`] (or [`ConfigFile::default`] when the file
/// is absent), hands it to `mutate`, then serialises the whole struct back and
/// writes it atomically with `0o600` permissions (the file may hold provider
/// API keys). Because it round-trips the full struct — not a from-scratch
/// render like the onboarding writer — MCP servers, hooks, profiles, providers
/// and every other block survive a partial settings save.
///
/// This is the single-call partial writer the TUI `/config` surface needs (the
/// "`wcore_config` exposes no clean single-call writer for a partial Config`"
/// gap the surface's own docs flag). Returns the path written.
///
/// NOTE: comments and hand-authored formatting are NOT preserved — the TOML
/// serialiser re-emits canonical form. Acceptable for the settings the TUI
/// owns; a future format-preserving pass would need `toml_edit`.
pub fn patch_global_config(mutate: impl FnOnce(&mut ConfigFile)) -> anyhow::Result<PathBuf> {
    let path = global_config_path();
    patch_config_file_at(&path, mutate)?;
    Ok(path)
}

/// The path-injectable core of [`patch_global_config`]. Split out so tests can
/// exercise the load → mutate → serialise → atomic-write round-trip against a
/// temp file with no `WAYLAND_HOME`/global-state race.
fn patch_config_file_at(path: &Path, mutate: impl FnOnce(&mut ConfigFile)) -> anyhow::Result<()> {
    use anyhow::Context;

    let mut file: ConfigFile = if path.exists() {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?
    } else {
        ConfigFile::default()
    };

    mutate(&mut file);

    let toml_str = toml::to_string_pretty(&file).context("serialising config")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config dir {}", parent.display()))?;
    }
    crate::atomic_write(path, toml_str.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    // The file may carry provider keys — keep it owner-only. Best-effort:
    // a chmod failure must not lose the write that already succeeded.
    let _ = crate::credentials::secure_credential_file(path);
    Ok(())
}

/// Resolve the legacy `config.yaml` lookup path, honouring `WAYLAND_HOME`.
///
/// #275 / F-010: previously this resolved against `dirs::home_dir()` only,
/// which meant every test process / sandboxed run / second-user account read
/// the real user's `~/.wayland/config.yaml` even with `WAYLAND_HOME` set —
/// the same hermeticity class as F-019.
///
/// Resolution order:
///   1. `$WAYLAND_HOME/config.yaml` when `WAYLAND_HOME` is set (sandbox /
///      hermetic env). The override owns BOTH the yaml read path and the
///      canonical TOML write path.
///   2. `$HOME/.wayland/config.yaml` otherwise — the Desktop-app default.
fn legacy_yaml_path() -> Option<PathBuf> {
    if std::env::var_os("WAYLAND_HOME").is_some() {
        return Some(wayland_config_dir().join("config.yaml"));
    }
    dirs::home_dir().map(|h| h.join(".wayland").join("config.yaml"))
}

/// One-shot migration from the legacy `config.yaml` (written by the Desktop
/// app, IJFW-style YAML) into the canonical `wayland_config_dir()/config.toml`
/// that the engine reads.
///
/// Runs at bootstrap before `load_config_file` so any fields the engine
/// cares about are present in the TOML on the first start after install.
/// Idempotent: skips when the legacy yaml is absent or the canonical TOML
/// already exists. Never deletes the yaml.
///
/// Both the read path (legacy yaml) and the write path (canonical TOML)
/// route through `wayland_config_dir()` so `WAYLAND_HOME` hermetically
/// sandboxes the entire migration (F-010 / #275).
pub fn migrate_legacy_yaml_if_needed() {
    let legacy_path = match legacy_yaml_path() {
        Some(p) => p,
        None => return, // no home → nothing to migrate
    };
    if !legacy_path.exists() {
        return;
    }

    let canonical_path = global_config_path();

    // Guard on the canonical TOML's EXISTENCE, not on any field within it.
    // The migration is a one-time yaml→toml conversion: once config.toml
    // exists it is the source of truth and must never be re-serialized
    // (doing so destroys user comments and any field outside ConfigFile).
    // Keying on model presence re-fired on every launch when the legacy
    // yaml carried no model (#: destructive re-serialization).
    if canonical_path.exists() {
        return; // already migrated or hand-authored — never touch it again
    }

    // No canonical TOML yet: start the migration from defaults.
    let existing = ConfigFile::default();

    // Parse the legacy yaml. On any error, warn and skip — the migration
    // is best-effort and must never prevent the engine from starting.
    let yaml_src = match std::fs::read_to_string(&legacy_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                "legacy-yaml-migrate: could not read {}: {} — skipping",
                legacy_path.display(),
                e
            );
            return;
        }
    };

    // We only need the few top-level keys the engine understands; all
    // other fields (candid_mode, browser, streaming, skills, …) are
    // Desktop-only and silently ignored here.
    #[derive(serde::Deserialize, Default)]
    struct LegacyYamlModel {
        default: Option<String>,
        provider: Option<String>,
        base_url: Option<String>,
    }
    #[derive(serde::Deserialize, Default)]
    struct LegacyYamlMemory {
        memory_enabled: Option<bool>,
    }
    #[derive(serde::Deserialize, Default)]
    struct LegacyYaml {
        model: Option<LegacyYamlModel>,
        memory: Option<LegacyYamlMemory>,
    }

    let legacy: LegacyYaml = match serde_yaml::from_str(&yaml_src) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "legacy-yaml-migrate: could not parse {}: {} — skipping",
                legacy_path.display(),
                e
            );
            return;
        }
    };

    // Build an updated ConfigFile from defaults overlaid with the fields the
    // yaml provides. (We only reach here when no canonical TOML exists yet.)
    let mut updated = existing;

    if let Some(m) = legacy.model {
        if let Some(model_id) = m.default {
            updated.default.model = Some(model_id);
        }
        if let Some(provider_name) = m.provider {
            // "auto" is the Desktop app's shorthand for "pick based on the
            // model prefix". The engine resolves that via `resolve_provider_alias`
            // — skip it here and let the engine determine the provider at
            // runtime from the model string.
            if provider_name != "auto" {
                updated.default.provider = provider_name.clone();
            }
        }
        if let Some(base_url) = m.base_url {
            // base_url goes on the provider entry that matches the provider
            // string (or "openrouter" if provider is "auto").
            let provider_key = if updated.default.provider == default_provider() {
                // Provider wasn't set from yaml (was "auto" or absent).
                // Infer from the model string if it has a known prefix.
                updated
                    .default
                    .model
                    .as_deref()
                    .and_then(|m| m.split('/').next())
                    .unwrap_or("openrouter")
                    .to_string()
            } else {
                updated.default.provider.clone()
            };
            updated.providers.entry(provider_key).or_default().base_url = Some(base_url);
        }
    }

    if let Some(mem) = legacy.memory
        && let Some(enabled) = mem.memory_enabled
    {
        // Materialize a `[memory]` table on migration: the legacy YAML
        // explicitly carried this setting, so the written config must too.
        updated
            .memory
            .get_or_insert_with(MemoryConfig::default)
            .enabled = enabled;
    }

    // Serialize back to TOML and write atomically.
    let toml_str = match toml::to_string_pretty(&updated) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                "legacy-yaml-migrate: failed to serialise updated config: {e} — skipping"
            );
            return;
        }
    };

    if let Some(parent) = canonical_path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(
            "legacy-yaml-migrate: could not create {}: {e} — skipping",
            parent.display()
        );
        return;
    }

    // Atomic write: write to a sibling .tmp, then rename.
    let tmp_path = canonical_path.with_extension("toml.tmp");
    if let Err(e) = std::fs::write(&tmp_path, toml_str.as_bytes()) {
        tracing::warn!(
            "legacy-yaml-migrate: could not write tmp file {}: {e} — skipping",
            tmp_path.display()
        );
        return;
    }
    if let Err(e) = std::fs::rename(&tmp_path, &canonical_path) {
        tracing::warn!("legacy-yaml-migrate: could not rename tmp → canonical: {e} — skipping");
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }

    tracing::info!(
        "Migrated legacy {} → {} (model={:?}). \
         Review the new config.toml then `rm {}` when satisfied.",
        legacy_path.display(),
        canonical_path.display(),
        updated.default.model.as_deref().unwrap_or(""),
        legacy_path.display(),
    );
}

/// Render the **effective configuration** as a redacted, pretty-printed TOML
/// string: the values the engine resolves from disk, merged in cascade order
/// (global ← project ← `--profile`) with the headline CLI overrides stamped on.
///
/// This is the data layer behind the `/doctor` Effective-config preview. It
/// repeats the file-merge phase of [`Config::resolve`] (steps 1–4) — the same
/// `try_load_config_file` / `merge_config_files` / `apply_profile` path — so the
/// preview never drifts from what `resolve` actually loads.
///
/// **Secrets are redacted.** Every string value whose key name looks like a
/// credential (`api_key`, `token`, `secret`, `password`, `credential`,
/// `private_key`, or anything containing `auth`) is replaced with `***` via a
/// recursive walk of the serialized value tree — robust to new secret-bearing
/// fields (a new key under `[providers.*]` / `[channels.*]` / MCP headers is
/// masked without a code change). Over-redaction is the safe direction.
///
/// Caveats surfaced to the user by the caller's header: live env-resolved API
/// keys never appear here (the file never holds them), and `WAYLAND_HOME`
/// sandboxing is honored through [`global_config_path`].
pub fn effective_config_toml(cli: &CliArgs) -> anyhow::Result<String> {
    use anyhow::Context;

    // Steps 1–4 of `resolve`: load + merge + optional profile overlay.
    let global = try_load_config_file(&global_config_path())
        .context("loading global config for the effective-config preview")?;
    let project_path = cli
        .project_dir
        .as_ref()
        .map(|d| d.join(".wayland-core.toml"))
        .unwrap_or_else(project_config_path);
    let project = try_load_config_file(&project_path)
        .context("loading project config for the effective-config preview")?;
    let mut merged = merge_config_files(global, project);
    if let Some(profile_name) = &cli.profile {
        merged = apply_profile(merged, profile_name)?;
    }

    // Stamp the headline CLI overrides so the preview reflects launch flags
    // (the rest of the CLI surface is provider-resolution detail that does not
    // belong in a config-file preview).
    if let Some(provider) = &cli.provider {
        merged.default.provider = provider.clone();
    }
    if let Some(model) = &cli.model {
        merged.default.model = Some(model.clone());
    }
    if cli.max_turns.is_some() {
        merged.default.max_turns = cli.max_turns;
    }

    let mut value =
        toml::Value::try_from(&merged).context("serializing the merged config for redaction")?;
    redact_secrets_in_place(&mut value);
    toml::to_string_pretty(&value).context("rendering the effective config as TOML")
}

/// True if a TOML key name designates a secret value that must be redacted.
/// Matched case-insensitively as a substring so compound names
/// (`webhook_secret`, `bot_token`, `Authorization`) are covered.
fn is_secret_key(key: &str) -> bool {
    const NEEDLES: [&str; 9] = [
        "api_key",
        "apikey",
        "token",
        "secret",
        "password",
        "passwd",
        "credential",
        "private_key",
        "auth",
    ];
    let lowered = key.to_ascii_lowercase();
    NEEDLES.iter().any(|n| lowered.contains(n))
}

/// Recursively replace every secret-keyed string value in `value` with `***`.
fn redact_secrets_in_place(value: &mut toml::Value) {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table.iter_mut() {
                if is_secret_key(key) {
                    mask_value(child);
                } else {
                    redact_secrets_in_place(child);
                }
            }
        }
        toml::Value::Array(items) => {
            for child in items.iter_mut() {
                redact_secrets_in_place(child);
            }
        }
        _ => {}
    }
}

/// Mask every string reachable from a secret-keyed value (a bare string, an
/// array of strings, or a nested table of strings). Non-string leaves
/// (numbers/bools) under a secret key are left as-is — they are not secrets to
/// leak, and masking them would corrupt the rendered types.
fn mask_value(value: &mut toml::Value) {
    match value {
        toml::Value::String(s) => *s = "***".to_string(),
        toml::Value::Array(items) => items.iter_mut().for_each(mask_value),
        toml::Value::Table(table) => {
            for (_, child) in table.iter_mut() {
                mask_value(child);
            }
        }
        _ => {}
    }
}

/// Merge two config files. Project overrides global.
fn merge_config_files(global: ConfigFile, project: ConfigFile) -> ConfigFile {
    let default = DefaultConfig {
        provider: if project.default.provider != default_provider() {
            project.default.provider
        } else {
            global.default.provider
        },
        model: project.default.model.or(global.default.model),
        max_tokens: if project.default.max_tokens != default_max_tokens() {
            project.default.max_tokens
        } else {
            global.default.max_tokens
        },
        max_turns: project.default.max_turns.or(global.default.max_turns),
        // Project overrides global only when it set a non-default posture.
        approval_mode: if project.default.approval_mode != ApprovalMode::default() {
            project.default.approval_mode
        } else {
            global.default.approval_mode
        },
        system_prompt: project
            .default
            .system_prompt
            .or(global.default.system_prompt),
        user: project.default.user.or(global.default.user),
        // Read-only is a safety posture: either layer asking for it wins, so
        // a project that opts into read-only is never silently re-enabled by
        // a permissive global default.
        read_only: global.default.read_only || project.default.read_only,
    };

    // Merge providers: global as base, project overrides
    let mut providers = global.providers;
    for (k, v) in project.providers {
        let base = providers.remove(&k).unwrap_or_default();
        providers.insert(k, merge_provider_configs(base, v));
    }

    // Merge profiles: global as base, project overrides
    let mut profiles = global.profiles;
    profiles.extend(project.profiles);

    // Tools: project overrides global for scalar fields; skills deny/allow are concatenated
    // (global first, then project) — consistent with the hooks merge strategy.
    let tools = if project.tools.allow_list != default_allow_list() || project.tools.auto_approve {
        ToolsConfig {
            auto_approve: global.tools.auto_approve || project.tools.auto_approve,
            allow_list: project.tools.allow_list,
            skills: SkillsPermissionConfig {
                deny: [global.tools.skills.deny, project.tools.skills.deny].concat(),
                allow: [global.tools.skills.allow, project.tools.skills.allow].concat(),
            },
            // W6 F15 — project overrides global for the verify-edits flag.
            verify_edits: project.tools.verify_edits || global.tools.verify_edits,
            // #182 — project overrides global for the Windows shell selector.
            windows_shell: project.tools.windows_shell.or(global.tools.windows_shell),
        }
    } else {
        ToolsConfig {
            auto_approve: global.tools.auto_approve || project.tools.auto_approve,
            allow_list: global.tools.allow_list,
            skills: SkillsPermissionConfig {
                deny: [global.tools.skills.deny, project.tools.skills.deny].concat(),
                allow: [global.tools.skills.allow, project.tools.skills.allow].concat(),
            },
            verify_edits: project.tools.verify_edits || global.tools.verify_edits,
            windows_shell: project.tools.windows_shell.or(global.tools.windows_shell),
        }
    };

    // Session: project overrides global
    let session = if project.session.directory != default_session_dir() {
        project.session
    } else {
        SessionConfig {
            enabled: global.session.enabled && project.session.enabled,
            directory: if project.session.directory != default_session_dir() {
                project.session.directory
            } else {
                global.session.directory
            },
            max_sessions: if project.session.max_sessions != default_max_sessions() {
                project.session.max_sessions
            } else {
                global.session.max_sessions
            },
        }
    };

    // Hooks: combine hooks from both configs (project hooks appended after global)
    let hooks = HooksConfig {
        pre_tool_use: [global.hooks.pre_tool_use, project.hooks.pre_tool_use].concat(),
        post_tool_use: [global.hooks.post_tool_use, project.hooks.post_tool_use].concat(),
        stop: [global.hooks.stop, project.hooks.stop].concat(),
        // Default ON; an explicit opt-out in either layer wins.
        dispatch_enabled: global.hooks.dispatch_enabled && project.hooks.dispatch_enabled,
    };

    // MCP: merge servers from both configs, project overrides global
    let mut mcp_servers = global.mcp.servers;
    mcp_servers.extend(project.mcp.servers);
    // W6 F17 — curation policy: project overrides global. Both default to
    // TopK { k: 15 } when omitted, so a fresh project file inherits sensibly.
    let mcp = McpConfig {
        servers: mcp_servers,
        curation: project.mcp.curation,
    };

    // Plan: project overrides global if any field differs from default
    let plan = if !project.plan.enabled
        || project.plan.plan_directory != PlanConfig::default().plan_directory
    {
        project.plan
    } else {
        global.plan
    };

    // File cache: project overrides global if any field differs from default.
    let file_cache = if !project.file_cache.enabled
        || project.file_cache.max_entries != FileCacheConfig::default().max_entries
        || project.file_cache.max_size_bytes != FileCacheConfig::default().max_size_bytes
    {
        project.file_cache
    } else {
        global.file_cache
    };

    // Bedrock/Vertex: project overrides global
    let bedrock = project.bedrock.or(global.bedrock);
    let vertex = project.vertex.or(global.vertex);

    // Compact: project overrides global for any non-default field.
    // Since CompactConfig uses serde defaults, a fully-default project config
    // is indistinguishable from "absent". We use project if its context_window
    // differs from the default, otherwise fall back to global.
    let compact = if project.compact.context_window != CompactConfig::default().context_window
        || !project.compact.enabled
    {
        project.compact
    } else {
        global.compact
    };

    let debug = DebugConfig::merge(global.debug, project.debug);

    // Observability is an additive opt-in: project's structured_traces
    // wins when it is `true`; otherwise inherit the global setting. This
    // mirrors the bool-only fields elsewhere — there is no "explicit false"
    // marker because the on-disk default is already false.
    let observability = ObservabilityConfig {
        structured_traces: project.observability.structured_traces
            || global.observability.structured_traces,
        skills_lifecycle: project.observability.skills_lifecycle
            || global.observability.skills_lifecycle,
        online_evolution: project.observability.online_evolution
            || global.observability.online_evolution,
        workflow_detection_enabled: project.observability.workflow_detection_enabled
            || global.observability.workflow_detection_enabled,
        workflow_live_mode: project.observability.workflow_live_mode
            || global.observability.workflow_live_mode,
    };

    // W7 F8-3: project's `enabled = true` wins over global; on `enabled`
    // ties, project's tuning values win (covers the "global on, project
    // tunes thresholds" case without an explicit absent-vs-default marker).
    let provider_chain = if project.provider_chain.enabled || global.provider_chain.enabled {
        if project.provider_chain.enabled {
            project.provider_chain
        } else {
            global.provider_chain
        }
    } else {
        // Neither side opted into chain reporting, but the circuit breaker
        // (and its fallback chain) is wrapped unconditionally in bootstrap.
        // Preserve any `fallback_models` the user set — project over global —
        // so a fallback list works without flipping `enabled`.
        let fallback_models = if project.provider_chain.fallback_models.is_empty() {
            global.provider_chain.fallback_models
        } else {
            project.provider_chain.fallback_models
        };
        ProviderChainConfig {
            fallback_models,
            ..Default::default()
        }
    };

    // W8a A.5: budget merges project-over-global field-by-field. The
    // merge keeps a project-level cap if set, else falls back to the
    // global cap, else None.
    let budget = crate::budget::BudgetConfig {
        max_wall_time_secs: project
            .budget
            .max_wall_time_secs
            .or(global.budget.max_wall_time_secs),
        max_tool_runtime_secs: project
            .budget
            .max_tool_runtime_secs
            .or(global.budget.max_tool_runtime_secs),
        max_processes: project.budget.max_processes.or(global.budget.max_processes),
        max_agent_depth: project
            .budget
            .max_agent_depth
            .or(global.budget.max_agent_depth),
        max_tokens_in: project.budget.max_tokens_in.or(global.budget.max_tokens_in),
        max_tokens_out: project
            .budget
            .max_tokens_out
            .or(global.budget.max_tokens_out),
        max_cost_usd: project.budget.max_cost_usd.or(global.budget.max_cost_usd),
    };

    // Wave SD — storage section: project overrides global if its backend
    // is non-default OR a service name is set.
    let storage = if project.storage.credentials.backend
        != crate::credentials::CredentialsBackend::default()
        || project.storage.credentials.service_name.is_some()
    {
        project.storage
    } else {
        global.storage
    };

    // M3.1 — memory section: a PRESENT project `[memory]` table wins outright;
    // an absent one inherits global. Mirrors the bedrock/vertex `Option`
    // override pattern. Presence (not "differs from default") is the gate, so
    // an explicit project `enabled = true` is honored over a global
    // `enabled = false` even though `true` now equals `MemoryConfig::default`.
    let memory = project.memory.or(global.memory);

    // B2 — security: the egress gate stays ON unless a layer turns it off
    // (most-restrictive `enabled`), and the operator allowlists concatenate
    // (global first, then project), mirroring the hooks/skills merge. A config
    // `enabled = false` still requires the `--i-accept-exfil-risk` CLI flag to
    // be honored (C8), so the merge can't silently disable the boundary.
    let security = SecurityConfig {
        enabled: global.security.enabled && project.security.enabled,
        egress_allow: [global.security.egress_allow, project.security.egress_allow].concat(),
    };

    // M5.bootstrap-wiring — session_cap is an opt-in `Option<BudgetConfig>`:
    // project block (if any) wins over global. Both absent ⇒ `None` ⇒
    // bootstrap skips tracker installation.
    let session_cap = project.session_cap.or(global.session_cap);

    // Inbound webhook host — a present project block (anything differing from
    // the off-by-default) wins outright; otherwise inherit global. Mirrors the
    // presence-over-default strategy used for memory/browser above.
    let inbound_webhook = if project.inbound_webhook != InboundWebhookConfig::default() {
        project.inbound_webhook
    } else {
        global.inbound_webhook
    };

    // FleetDispatcher-class fix (audit 2026-05-24 §3) — browser section:
    // project overrides global if any policy field differs from
    // `BrowserPolicyConfig::default()`. Mirrors the memory/budget strategy
    // above; conservative — preserves the deny-all default when neither
    // block exists.
    let default_browser_policy = crate::browser::BrowserPolicyConfig::default();
    let browser = if project.browser.policy.default_action != default_browser_policy.default_action
        || !project.browser.policy.allowed_origins.is_empty()
        || !project.browser.policy.denied_origins.is_empty()
    {
        project.browser
    } else {
        global.browser
    };

    // Crucible: project overrides global when it set a non-default council
    // (enabled, or a non-empty proposer roster). Mirrors the browser/memory
    // "project overrides when non-default" strategy; preserves the OFF default
    // when neither layer configures a council.
    let crucible = if project.crucible.enabled || !project.crucible.proposers.is_empty() {
        project.crucible
    } else {
        global.crucible
    };

    ConfigFile {
        default,
        providers,
        profiles,
        tools,
        session,
        inbound_webhook,
        compact,
        plan,
        file_cache,
        hooks,
        bedrock,
        vertex,
        mcp,
        debug,
        observability,
        provider_chain,
        budget,
        storage,
        memory,
        browser,
        security,
        session_cap,
        crucible,
    }
}

/// Resolve a profile with inheritance chain (with cycle detection)
fn resolve_profile(
    profiles: &HashMap<String, ProfileConfig>,
    name: &str,
    visited: &mut Vec<String>,
) -> anyhow::Result<ProfileConfig> {
    if visited.contains(&name.to_string()) {
        anyhow::bail!(
            "Circular profile inheritance detected: {} -> {}",
            visited.join(" -> "),
            name
        );
    }
    visited.push(name.to_string());

    let profile = profiles
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Profile '{}' not found in config", name))?
        .clone();

    if let Some(parent_name) = &profile.extends {
        let parent = resolve_profile(profiles, parent_name, visited)?;
        Ok(merge_profiles(parent, profile))
    } else {
        Ok(profile)
    }
}

/// Merge two profiles: overlay takes precedence over base
fn merge_profiles(base: ProfileConfig, overlay: ProfileConfig) -> ProfileConfig {
    ProfileConfig {
        provider: overlay.provider.or(base.provider),
        model: overlay.model.or(base.model),
        api_key: overlay.api_key.or(base.api_key),
        base_url: overlay.base_url.or(base.base_url),
        max_tokens: overlay.max_tokens.or(base.max_tokens),
        max_turns: overlay.max_turns.or(base.max_turns),
        extends: None, // already resolved
        mcp_servers: overlay.mcp_servers.or(base.mcp_servers),
        compat: overlay.compat.or(base.compat),
    }
}

fn apply_profile(mut config: ConfigFile, profile_name: &str) -> anyhow::Result<ConfigFile> {
    let mut visited = Vec::new();
    let profile = resolve_profile(&config.profiles, profile_name, &mut visited)?;

    if let Some(provider) = profile.provider {
        config.default.provider = provider;
    }
    if let Some(model) = profile.model {
        config.default.model = Some(model);
    }
    if let Some(max_tokens) = profile.max_tokens {
        config.default.max_tokens = max_tokens;
    }
    if let Some(max_turns) = profile.max_turns {
        config.default.max_turns = Some(max_turns);
    }

    // Profile can override api_key, base_url, and compat for the active provider
    let provider_name = config.default.provider.clone();
    let entry = config.providers.entry(provider_name).or_default();
    if let Some(api_key) = profile.api_key {
        entry.api_key = Some(api_key);
    }
    if let Some(base_url) = profile.base_url {
        entry.base_url = Some(base_url);
    }
    if let Some(compat) = profile.compat {
        entry.compat = Some(match entry.compat.take() {
            Some(existing) => ProviderCompat::merge(existing, compat),
            None => compat,
        });
    }

    // Filter MCP servers by profile's mcp_servers list
    if let Some(server_names) = profile.mcp_servers {
        config
            .mcp
            .servers
            .retain(|name, _| server_names.contains(name));
    }

    Ok(config)
}

// --- Init config command ---

pub fn init_config() -> anyhow::Result<()> {
    let path = global_config_path();
    if path.exists() {
        eprintln!("Config already exists: {}", path.display());
        // Wave SD: even on a no-op init, ensure perms are tight.
        let _ = crate::credentials::secure_credential_file(&path);
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, DEFAULT_CONFIG_TEMPLATE)?;
    // Wave SD SECURITY MAJOR #16: enforce 0o600 on first write so the
    // file is never world-readable between create() and the next save.
    crate::credentials::secure_credential_file(&path)?;
    eprintln!("Config created: {}", path.display());
    Ok(())
}

const DEFAULT_CONFIG_TEMPLATE: &str = r#"# wayland-core configuration

# Default provider settings
[default]
provider = "anthropic"            # built-in provider or custom alias from [providers.<name>]
# model = "claude-sonnet-4-6"      # default; see default_model_for() in this crate
max_tokens = 64000                 # a CAP; the engine clamps it per-model before sending
# max_turns = 30                  # optional: omit for unlimited turns
# system_prompt = "..."          # optional custom system prompt

# Provider-specific API settings
[providers.anthropic]
# api_key = "sk-ant-xxx"         # can also use env: API_KEY or ANTHROPIC_API_KEY
# base_url = "https://api.anthropic.com"

[providers.openai]
# api_key = "sk-xxx"             # can also use env: OPENAI_API_KEY
# base_url = "https://api.openai.com"

# Custom provider alias (maps to a built-in provider type)
# [providers.my-service]
# provider = "openai"
# model = "custom-model-v1"
# api_key = "sk-xxx"
# base_url = "https://my-service.example.com/api/openai"

# Provider compatibility overrides (usually not needed — defaults work)
# [providers.openai.compat]
# max_tokens_field = "max_completion_tokens"  # for OpenAI official models
# merge_assistant_messages = true
# clean_orphan_tool_calls = true
# dedup_tool_results = true
# strip_patterns = ["__OPENROUTER_REASONING_DETAILS__"]

# AWS Bedrock configuration (uses AWS SigV4 auth, no API key needed)
# [bedrock]
# region = "us-east-1"
# access_key_id = "AKIA..."
# secret_access_key = "..."
# session_token = "..."
# profile = "my-profile"        # or use AWS profile

# Google Vertex AI configuration (uses GCP OAuth2 auth, no API key needed)
# [vertex]
# project_id = "my-gcp-project"
# region = "us-central1"
# credentials_file = "/path/to/service-account.json"  # or use ADC

# Named profiles for quick switching (--profile <name>)
# [profiles.deepseek]
# provider = "openai"
# model = "deepseek-chat"
# api_key = "sk-xxx"
# base_url = "https://api.deepseek.com"

# [profiles.ollama]
# provider = "openai"
# model = "qwen2.5:32b"
# api_key = "ollama"
# base_url = "http://localhost:11434"

# [profiles.my-service]
# provider = "my-service"

# [profiles.bedrock-claude]
# provider = "bedrock"
# model = "anthropic.claude-sonnet-4-6-20251015-v1:0"
# # or: model = "bedrock:sonnet" (short-form, see wcore_types::model_aliases)

# [profiles.vertex-claude]
# provider = "vertex"
# model = "claude-sonnet-4-6@20251015"
# # or: model = "vertex:sonnet" (short-form, see wcore_types::model_aliases)

# Tool confirmation settings
[tools]
auto_approve = false             # --auto-approve overrides
# Tools that skip confirmation even when auto_approve = false
allow_list = ["Read", "Grep", "Glob"]

# Context compaction settings
# [compact]
# context_window = 200000        # context window size in tokens
# output_reserve = 20000         # tokens reserved for output
# autocompact_buffer = 13000     # buffer below effective window for autocompact trigger
# emergency_buffer = 3000        # tokens from limit for emergency block
# max_failures = 3               # consecutive failures before circuit-breaker trips
# micro_keep_recent = 5          # keep N most recent tool results
# micro_gap_seconds = 3600       # gap threshold for time-based microcompact
# compactable_tools = ["Read", "Bash", "Grep", "Glob", "Write", "Edit"]
# enabled = true

# File state cache (dedup repeated reads, staleness detection)
# [file_cache]
# max_entries = 100            # max cached file entries
# max_size_bytes = 26214400    # 25 MB total cache size
# enabled = true

# Session settings
[session]
enabled = true
directory = ".wayland-core/sessions"  # relative to project root
max_sessions = 20                # auto-cleanup oldest

# Hook system: run shell commands at tool lifecycle events
# [[hooks.post_tool_use]]
# name = "rustfmt"
# tool_match = ["Write", "Edit"]
# file_match = ["*.rs"]
# command = "rustfmt ${TOOL_INPUT_FILE_PATH}"

# [[hooks.post_tool_use]]
# name = "prettier"
# tool_match = ["Write", "Edit"]
# file_match = ["*.ts", "*.tsx"]
# command = "npx prettier --write ${TOOL_INPUT_FILE_PATH}"

# [[hooks.stop]]
# name = "final-lint"
# command = "cargo clippy --quiet 2>&1 | tail -5"

# MCP (Model Context Protocol) servers
# [mcp.servers.filesystem]
# transport = "stdio"
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-filesystem", "/Users/me/project"]

# [mcp.servers.github]
# transport = "stdio"
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-github"]
# env = { GITHUB_TOKEN = "ghp_xxx" }

# [mcp.servers.remote]
# transport = "sse"
# url = "http://localhost:3001/sse"

# [mcp.servers.api]
# transport = "streamable-http"
# url = "https://tools.example.com/mcp"
# headers = { Authorization = "Bearer xxx" }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_types::model_aliases::OPENAI_GPT4O;

    // -------------------------------------------------------------------------
    // parse_builtin_provider tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_provider_type_from_str_anthropic() {
        let result = parse_builtin_provider("anthropic");
        assert_eq!(result, Some(ProviderType::Anthropic));
    }

    #[test]
    fn default_model_for_slug_resolves_builtin_and_empties_catalog() {
        // D002: a built-in provider slug resolves to a non-empty default model.
        assert!(
            !default_model_for_slug("anthropic").is_empty(),
            "anthropic must have a stamped default model"
        );
        assert!(!default_model_for_slug("openai").is_empty());
        // Catalog / Tier-2 providers (heterogeneous catalogs) have no default —
        // they resolve to "" so onboarding writes no guessed model line and the
        // in-app `/model` recovery covers them.
        assert_eq!(default_model_for_slug("groq"), "");
        assert_eq!(default_model_for_slug("openrouter"), "");
        assert_eq!(default_model_for_slug("deepseek"), "");
        // An unknown / data-driven catalog id (e.g. `novita-ai`) is not a
        // built-in slug — also "" (recovered in-app).
        assert_eq!(default_model_for_slug("novita-ai"), "");
    }

    // -------------------------------------------------------------------------
    // D004 — `[default] read_only` posture round-trip.
    // -------------------------------------------------------------------------

    #[test]
    fn read_only_defaults_to_false_when_absent() {
        // A config with no `read_only` key must deserialize to the
        // permissive default, not silently flip a session offline.
        let cfg: ConfigFile =
            toml::from_str("[default]\nprovider = \"anthropic\"\n").expect("parse minimal config");
        assert!(
            !cfg.default.read_only,
            "an absent read_only key must default to false"
        );
    }

    #[test]
    fn read_only_round_trips_through_toml() {
        // The persisted posture must survive a serialize -> parse cycle so
        // the Skip path's choice reaches the engine gate that honours it.
        let mut cfg = ConfigFile::default();
        cfg.default.read_only = true;
        let rendered = toml::to_string(&cfg).expect("serialize config");
        let reparsed: ConfigFile = toml::from_str(&rendered).expect("reparse config");
        assert!(
            reparsed.default.read_only,
            "read_only = true must round-trip through TOML; rendered:\n{rendered}"
        );
        assert!(
            rendered.contains("read_only = true"),
            "the rendered config must carry the read_only flag; got:\n{rendered}"
        );
    }

    #[test]
    fn test_provider_type_from_str_openai() {
        let result = parse_builtin_provider("openai");
        assert_eq!(result, Some(ProviderType::OpenAI));
    }

    #[test]
    fn test_provider_type_from_str_bedrock() {
        let result = parse_builtin_provider("bedrock");
        assert_eq!(result, Some(ProviderType::Bedrock));
    }

    #[test]
    fn test_provider_type_from_str_vertex() {
        let result = parse_builtin_provider("vertex");
        assert_eq!(result, Some(ProviderType::Vertex));
    }

    #[test]
    fn test_provider_type_from_str_invalid() {
        let result = parse_builtin_provider("invalid");
        assert_eq!(result, None);
    }

    #[test]
    fn parse_builtin_provider_recognizes_v063_tier2_providers() {
        // v0.6.3 D.1 Round 1 cleanup: the 6 new OpenAI-compatible providers
        // must be selectable by their lowercase id.
        assert_eq!(
            parse_builtin_provider("azure-openai"),
            Some(ProviderType::AzureOpenAI)
        );
        assert_eq!(
            parse_builtin_provider("together"),
            Some(ProviderType::Together)
        );
        assert_eq!(
            parse_builtin_provider("fireworks"),
            Some(ProviderType::Fireworks)
        );
        assert_eq!(parse_builtin_provider("nvidia"), Some(ProviderType::Nvidia));
        assert_eq!(
            parse_builtin_provider("perplexity"),
            Some(ProviderType::Perplexity)
        );
        assert_eq!(
            parse_builtin_provider("cerebras"),
            Some(ProviderType::Cerebras)
        );
    }

    #[test]
    fn parses_chatgpt_provider_aliases() {
        // Both the canonical id and the short alias resolve to the same type.
        assert_eq!(
            parse_builtin_provider("openai-chatgpt"),
            Some(ProviderType::OpenAIChatGpt)
        );
        assert_eq!(
            parse_builtin_provider("chatgpt"),
            Some(ProviderType::OpenAIChatGpt)
        );
        // The Codex backend default model is gpt-5.5.
        assert_eq!(default_model_for(ProviderType::OpenAIChatGpt), "gpt-5.5");
        // It rides OpenAI-compat plumbing (A7).
        assert!(ProviderType::OpenAIChatGpt.is_openai_compatible());
    }

    #[test]
    fn minimax_provider_maps_to_anthropic_wire_endpoint() {
        // Canonical id and the `minimaxi` domain-spelling alias both resolve.
        assert_eq!(
            parse_builtin_provider("minimax"),
            Some(ProviderType::MiniMax)
        );
        assert_eq!(
            parse_builtin_provider("minimaxi"),
            Some(ProviderType::MiniMax)
        );
        // Slug round-trips (read==write key for the credentials/catalog paths).
        assert_eq!(provider_type_slug(ProviderType::MiniMax), "minimax");
        // Base URL is MiniMax's Anthropic-compatible endpoint (verified live);
        // the reused AnthropicProvider appends `/v1/messages` to it.
        assert_eq!(
            default_base_url_for(ProviderType::MiniMax),
            "https://api.minimax.io/anthropic"
        );
        // Unlike the heterogeneous Tier-2 catalogs, MiniMax has a headline
        // default model so onboarding never lands in the no-model dead-end.
        assert_eq!(default_model_for(ProviderType::MiniMax), "MiniMax-M2");
        // It authenticates with a plain API key in the credentials store...
        assert_eq!(
            credentials_store_key(ProviderType::MiniMax).as_deref(),
            Some("providers.minimax.api_key")
        );
        // ...and is Anthropic-wire, NOT OpenAI-compatible (cost/plumbing path).
        assert!(!ProviderType::MiniMax.is_openai_compatible());
        assert_eq!(
            compat_defaults_for(ProviderType::MiniMax).provider_type(),
            "minimax"
        );
    }

    #[test]
    fn v063_tier2_providers_are_openai_compatible() {
        for p in [
            ProviderType::AzureOpenAI,
            ProviderType::Together,
            ProviderType::Fireworks,
            ProviderType::Nvidia,
            ProviderType::Perplexity,
            ProviderType::Cerebras,
        ] {
            assert!(p.is_openai_compatible(), "{p:?} must be OpenAI-compatible");
        }
        // Native providers are not OpenAI-compatible.
        assert!(!ProviderType::Anthropic.is_openai_compatible());
        assert!(!ProviderType::Gemini.is_openai_compatible());
    }

    #[test]
    fn test_provider_alias_resolves_to_builtin_provider() {
        let mut providers = HashMap::new();
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                model: Some("custom-model-v1".to_string()),
                api_key: Some("alias-key".to_string()),
                base_url: Some("https://my-service.example.com/v1".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        assert_eq!(resolved.requested_name, "my-service");
        assert_eq!(resolved.provider_type, ProviderType::OpenAI);
        assert_eq!(
            resolved.effective_config.model.as_deref(),
            Some("custom-model-v1")
        );
        assert_eq!(
            resolved.effective_config.api_key.as_deref(),
            Some("alias-key")
        );
        assert_eq!(
            resolved.effective_config.base_url.as_deref(),
            Some("https://my-service.example.com/v1")
        );
    }

    #[test]
    fn catalog_provider_resolves_through_openai_path() {
        // A bundled catalog id that is NOT a built-in and NOT a user alias
        // resolves to the OpenAI wire path, carrying the catalog entry.
        let providers = HashMap::new();
        let resolved =
            resolve_provider_alias(&providers, "novita-ai").expect("catalog id resolves");
        assert_eq!(resolved.requested_name, "novita-ai");
        assert_eq!(resolved.provider_type, ProviderType::OpenAI);
        let entry = resolved
            .catalog_entry
            .expect("catalog entry carried through");
        assert_eq!(entry.id, "novita-ai");
        assert_eq!(entry.base_url, "https://api.novita.ai/openai");
    }

    #[test]
    fn unknown_provider_id_errors_cleanly() {
        let providers = HashMap::new();
        let err = resolve_provider_alias(&providers, "definitely-not-a-provider")
            .expect_err("unknown id must error");
        assert!(
            err.to_string().contains("Unknown provider"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn native_collision_id_prefers_native_arm_not_catalog() {
        // `deepseek` is both a native ProviderType arm AND a catalog entry.
        // The built-in match runs first, so resolution must yield the native
        // Deepseek arm with NO catalog entry attached.
        let providers = HashMap::new();
        let resolved = resolve_provider_alias(&providers, "deepseek").expect("deepseek resolves");
        assert_eq!(resolved.provider_type, ProviderType::Deepseek);
        assert!(
            resolved.catalog_entry.is_none(),
            "native arm must win over the catalog for collision ids"
        );
    }

    #[test]
    fn test_provider_alias_overlays_builtin_provider_defaults() {
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                api_key: Some("builtin-key".to_string()),
                model: Some(OPENAI_GPT4O.to_string()),
                ..Default::default()
            },
        );
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                base_url: Some("https://my-service.example.com/v1".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        assert_eq!(resolved.provider_type, ProviderType::OpenAI);
        assert_eq!(
            resolved.effective_config.api_key.as_deref(),
            Some("builtin-key")
        );
        assert_eq!(
            resolved.effective_config.model.as_deref(),
            Some(OPENAI_GPT4O)
        );
        assert_eq!(
            resolved.effective_config.base_url.as_deref(),
            Some("https://my-service.example.com/v1")
        );
    }

    #[test]
    fn test_provider_alias_requires_underlying_provider_type() {
        let mut providers = HashMap::new();
        providers.insert("my-service".to_string(), ProviderConfig::default());

        let result = resolve_provider_alias(&providers, "my-service");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("my-service"));
        assert!(msg.contains("provider"));
        assert!(msg.contains("built-in type"));
    }

    // ---- resolve_council_provider (keyed cross-provider council) ------------

    #[test]
    fn council_resolves_each_provider_to_its_own_key() {
        // The core cross-provider guarantee: two council members keyed to two
        // different providers each get THEIR OWN credentials from the
        // `[providers]` map — not one shared base key (the bug this fixes).
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                api_key: Some("sk-openai-aaa".to_string()),
                ..Default::default()
            },
        );
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                api_key: Some("sk-ant-bbb".to_string()),
                ..Default::default()
            },
        );
        let base = Config::default();

        let (oa, _) = resolve_council_provider(&providers, &base, "openai").expect("openai");
        let (an, _) = resolve_council_provider(&providers, &base, "anthropic").expect("anthropic");

        assert_eq!(oa.provider, ProviderType::OpenAI);
        assert_eq!(oa.api_key, "sk-openai-aaa");
        assert_eq!(an.provider, ProviderType::Anthropic);
        assert_eq!(an.api_key, "sk-ant-bbb");
        // Distinct keys — the single-base-key behavior would make these equal.
        assert_ne!(oa.api_key, an.api_key);
    }

    #[test]
    fn council_pins_model_from_spec() {
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                api_key: Some("sk-openai".to_string()),
                ..Default::default()
            },
        );
        let base = Config::default();
        let (cfg, model) =
            resolve_council_provider(&providers, &base, "openai:gpt-5.5").expect("resolve");
        assert_eq!(cfg.model, "gpt-5.5");
        assert_eq!(model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn council_resolves_out_of_band_provider() {
        // Vertex/Bedrock/ChatGPT authenticate out-of-band (GCP/AWS creds, OAuth)
        // and resolve to an empty inline key BY DESIGN. They are valid council
        // members and must NOT be skipped as keyless — that would drop exactly
        // the enterprise providers a cross-provider council wants.
        let providers = HashMap::new();
        let base = Config::default();
        let (cfg, _model) = resolve_council_provider(&providers, &base, "vertex")
            .expect("vertex (out-of-band auth) must resolve, not be skipped");
        assert_eq!(cfg.provider, ProviderType::Vertex);
    }

    #[test]
    fn council_skips_genuinely_keyless_provider() {
        // A provider that REQUIRES an inline key but has none (no inline config,
        // no env var) is the real keyless case → skip. `cohere` needs
        // COHERE_API_KEY; with an empty providers map and that env var unset,
        // resolve_api_key returns Err → Keyless.
        let providers = HashMap::new();
        let base = Config::default();
        let err = resolve_council_provider(&providers, &base, "cohere")
            .expect_err("cohere with no key must be keyless");
        assert!(
            matches!(err, CouncilProviderError::Keyless(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn council_errors_unknown_provider() {
        let providers = HashMap::new();
        let base = Config::default();
        let err = resolve_council_provider(&providers, &base, "definitely-not-a-provider")
            .expect_err("unknown id");
        assert!(
            matches!(err, CouncilProviderError::Unknown(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn council_inherits_non_provider_fields_from_base() {
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                api_key: Some("sk-openai".to_string()),
                ..Default::default()
            },
        );
        let base = Config {
            max_tokens: 4242,
            ..Default::default()
        };
        let (cfg, _) = resolve_council_provider(&providers, &base, "openai").expect("resolve");
        assert_eq!(
            cfg.max_tokens, 4242,
            "non-provider field must inherit from base"
        );
    }

    #[test]
    fn bedrock_debug_redacts_secrets() {
        let cfg = BedrockConfig {
            region: Some("us-east-1".to_string()),
            access_key_id: Some("AKIAEXAMPLE".to_string()),
            secret_access_key: Some("super-secret-value".to_string()),
            session_token: Some("token-value".to_string()),
            profile: Some("default".to_string()),
        };
        let dbg = format!("{cfg:?}");
        // Non-secret metadata stays visible.
        assert!(dbg.contains("us-east-1"));
        assert!(dbg.contains("default"));
        // Secrets are masked, never printed verbatim.
        assert!(dbg.contains("<redacted>"));
        assert!(!dbg.contains("AKIAEXAMPLE"));
        assert!(!dbg.contains("super-secret-value"));
        assert!(!dbg.contains("token-value"));
    }

    #[test]
    fn vertex_debug_redacts_inline_key() {
        let cfg = VertexConfig {
            project_id: Some("my-proj".to_string()),
            region: Some("us-central1".to_string()),
            credentials_file: Some("/path/to/key.json".to_string()),
            service_account_json: Some("{\"private_key\":\"LEAK\"}".to_string()),
        };
        let dbg = format!("{cfg:?}");
        assert!(dbg.contains("my-proj"));
        assert!(dbg.contains("<redacted>"));
        assert!(!dbg.contains("LEAK"));
    }

    #[test]
    fn config_debug_redacts_api_key() {
        let cfg = Config {
            api_key: "sk-super-secret-LEAK".to_string(),
            model: "gpt-5.5".to_string(),
            ..Default::default()
        };
        let dbg = format!("{cfg:?}");
        // The live key never appears; only the masked sentinel does.
        assert!(
            !dbg.contains("sk-super-secret-LEAK"),
            "api_key must not leak via Debug"
        );
        assert!(dbg.contains("<redacted>"));
        // Non-secret fields stay visible (Debug still useful).
        assert!(dbg.contains("gpt-5.5"));
    }

    #[test]
    fn config_debug_shows_none_for_empty_api_key() {
        let cfg = Config::default(); // empty api_key
        let dbg = format!("{cfg:?}");
        assert!(dbg.contains("api_key: \"<none>\""));
    }

    #[test]
    fn crucible_block_merges_project_over_global() {
        let global = ConfigFile {
            crucible: crate::crucible::CrucibleConfig {
                enabled: true,
                proposers: vec!["openai".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let project = ConfigFile {
            crucible: crate::crucible::CrucibleConfig {
                enabled: true,
                proposers: vec!["anthropic".to_string(), "gemini".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge_config_files(global, project);
        // Project set a non-default council → it wins.
        assert_eq!(merged.crucible.proposers, vec!["anthropic", "gemini"]);
    }

    #[test]
    fn crucible_defaults_off_when_absent() {
        let merged = merge_config_files(ConfigFile::default(), ConfigFile::default());
        assert!(!merged.crucible.enabled);
        assert!(merged.crucible.proposers.is_empty());
    }

    // -------------------------------------------------------------------------
    // merge_config_files tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_merge_config_cli_overrides_file() {
        // Project config sets a non-default provider; it should win over global.
        let global = ConfigFile {
            default: DefaultConfig {
                provider: "anthropic".to_string(),
                model: Some("global-model".to_string()),
                max_tokens: 4096,
                max_turns: Some(10),
                system_prompt: Some("global prompt".to_string()),
                approval_mode: ApprovalMode::default(),
                user: None,
                read_only: false,
            },
            ..Default::default()
        };
        let project = ConfigFile {
            default: DefaultConfig {
                provider: "openai".to_string(), // non-default -> overrides global
                model: Some("project-model".to_string()),
                max_tokens: 2048,   // non-default -> overrides global
                max_turns: Some(5), // non-default -> overrides global
                system_prompt: Some("project prompt".to_string()),
                approval_mode: ApprovalMode::default(),
                user: None,
                read_only: false,
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);

        assert_eq!(merged.default.provider, "openai");
        assert_eq!(merged.default.model, Some("project-model".to_string()));
        assert_eq!(merged.default.max_tokens, 2048);
        assert_eq!(merged.default.max_turns, Some(5));
        assert_eq!(
            merged.default.system_prompt,
            Some("project prompt".to_string())
        );
    }

    #[test]
    fn test_merge_config_file_provides_defaults() {
        // Project config is default; global values should be preserved.
        let global = ConfigFile {
            default: DefaultConfig {
                provider: "openai".to_string(),
                model: Some("global-model".to_string()),
                max_tokens: 1024,
                max_turns: Some(5),
                system_prompt: Some("global prompt".to_string()),
                approval_mode: ApprovalMode::default(),
                user: None,
                read_only: false,
            },
            ..Default::default()
        };
        // Project stays at built-in defaults (provider = "anthropic", max_tokens = 64000, max_turns = None)
        let project = ConfigFile::default();

        let merged = merge_config_files(global, project);

        // provider: project default "anthropic" == default_provider() -> use global "openai"
        assert_eq!(merged.default.provider, "openai");
        assert_eq!(merged.default.model, Some("global-model".to_string()));
        assert_eq!(merged.default.max_tokens, 1024);
        assert_eq!(merged.default.max_turns, Some(5));
        assert_eq!(
            merged.default.system_prompt,
            Some("global prompt".to_string())
        );
    }

    #[test]
    fn test_merge_config_empty_file() {
        // Two default ConfigFiles merged should yield defaults.
        let merged = merge_config_files(ConfigFile::default(), ConfigFile::default());

        assert_eq!(merged.default.provider, default_provider());
        assert_eq!(merged.default.max_tokens, default_max_tokens());
        assert_eq!(merged.default.max_turns, None);
        assert!(merged.default.model.is_none());
        assert!(merged.providers.is_empty());
        assert!(merged.profiles.is_empty());
    }

    /// F2 regression: an explicit project `[memory] enabled = true` must win
    /// over a global `enabled = false`, even though `true` equals
    /// `MemoryConfig::default()`. The old "differs from default" gate dropped
    /// the project opt-in; the Option-presence gate honors it.
    #[test]
    fn test_merge_project_memory_enabled_overrides_global_disabled() {
        let global: ConfigFile = toml::from_str("[memory]\nenabled = false\n").unwrap();
        let project: ConfigFile = toml::from_str("[memory]\nenabled = true\n").unwrap();

        let merged = merge_config_files(global, project);

        assert!(
            merged
                .memory
                .expect("project [memory] table is present")
                .enabled,
            "explicit project enabled=true must win over global enabled=false",
        );
    }

    /// F2 preserved case: an explicit project `enabled = false` still overrides
    /// a global `enabled = true` (here global is the memory-ON default).
    #[test]
    fn test_merge_project_memory_disabled_overrides_global_enabled() {
        let global: ConfigFile = toml::from_str("[memory]\nenabled = true\n").unwrap();
        let project: ConfigFile = toml::from_str("[memory]\nenabled = false\n").unwrap();

        let merged = merge_config_files(global, project);

        assert!(
            !merged
                .memory
                .expect("project [memory] table is present")
                .enabled,
            "explicit project enabled=false must override global enabled=true",
        );
    }

    /// F2 preserved case: a project with NO `[memory]` table inherits the
    /// global block verbatim (presence, not value, is the gate).
    #[test]
    fn test_merge_absent_project_memory_inherits_global() {
        let global: ConfigFile =
            toml::from_str("[memory]\nenabled = false\ndecay_interval_secs = 99\n").unwrap();
        let project: ConfigFile = toml::from_str("[default]\nprovider = \"anthropic\"\n").unwrap();
        assert!(
            project.memory.is_none(),
            "no [memory] table ⇒ None ⇒ inherit global"
        );

        let merged = merge_config_files(global, project);
        let mem = merged.memory.expect("global [memory] inherited");
        assert!(!mem.enabled);
        assert_eq!(mem.decay_interval_secs, 99);
    }

    // -------------------------------------------------------------------------
    // resolve_profile tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_profile_inheritance() {
        // Profile "child" extends "parent"; child fields win, missing ones fall back to parent.
        // Note: "claude-3"/"claude-4" below are opaque placeholders — this test exercises
        // the override mechanism, not specific model behaviour. See wcore_types::model_aliases
        // for canonical real-model identifiers used in tests that care about the value.
        let mut profiles = HashMap::new();
        profiles.insert(
            "parent".to_string(),
            ProfileConfig {
                provider: Some("anthropic".to_string()),
                model: Some("claude-3".to_string()),
                max_tokens: Some(4096),
                ..Default::default()
            },
        );
        profiles.insert(
            "child".to_string(),
            ProfileConfig {
                model: Some("claude-4".to_string()), // overrides parent
                extends: Some("parent".to_string()),
                ..Default::default()
            },
        );

        let mut visited = Vec::new();
        let result = resolve_profile(&profiles, "child", &mut visited).unwrap();

        // Child's model wins
        assert_eq!(result.model, Some("claude-4".to_string()));
        // Parent's provider is inherited
        assert_eq!(result.provider, Some("anthropic".to_string()));
        // Parent's max_tokens is inherited
        assert_eq!(result.max_tokens, Some(4096));
        // extends is cleared after resolution
        assert!(result.extends.is_none());
    }

    #[test]
    fn test_profile_cycle_detection() {
        // A extends B, B extends A -> should fail with cycle error.
        let mut profiles = HashMap::new();
        profiles.insert(
            "a".to_string(),
            ProfileConfig {
                extends: Some("b".to_string()),
                ..Default::default()
            },
        );
        profiles.insert(
            "b".to_string(),
            ProfileConfig {
                extends: Some("a".to_string()),
                ..Default::default()
            },
        );

        let mut visited = Vec::new();
        let result = resolve_profile(&profiles, "a", &mut visited);

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Circular profile inheritance"));
    }

    #[test]
    fn test_profile_not_found() {
        let profiles: HashMap<String, ProfileConfig> = HashMap::new();
        let mut visited = Vec::new();
        let result = resolve_profile(&profiles, "nonexistent", &mut visited);

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nonexistent"));
    }

    // -------------------------------------------------------------------------
    // resolve_api_key tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_api_key_from_cli_arg() {
        // CLI key takes highest priority regardless of other sources.
        let storage = crate::credentials::CredentialsStorageConfig::default();
        let result = resolve_api_key(
            Some("cli-key"),
            Some("config-key"),
            ProviderType::Anthropic,
            &storage,
        )
        .unwrap();
        assert_eq!(result, "cli-key");
    }

    #[test]
    fn test_api_key_from_config() {
        // When CLI key is absent, config file key should be used.
        let storage = crate::credentials::CredentialsStorageConfig::default();
        let result =
            resolve_api_key(None, Some("config-key"), ProviderType::Anthropic, &storage).unwrap();
        assert_eq!(result, "config-key");
    }

    #[test]
    fn test_api_key_missing_returns_error() {
        // Remove all env vars that could supply a key so the function must fail.
        // Note: single-threaded tests share the process environment; clearing here
        // is safe for unit test purposes.
        // SAFETY: single-threaded test context; no other threads read these vars.
        unsafe {
            std::env::remove_var("API_KEY");
            std::env::remove_var("ANTHROPIC_API_KEY");
        }

        // Anthropic ships with API-key auth only: with no CLI key, no config key,
        // no store entry, and no env var, resolution must fail deterministically.
        let storage = crate::credentials::CredentialsStorageConfig::default();
        let result = resolve_api_key(None, None, ProviderType::Anthropic, &storage);

        let e = result.expect_err("no credential anywhere must surface an error");
        assert!(e.to_string().contains("No API key found"));
    }

    #[test]
    fn test_api_key_bedrock_returns_empty_without_key() {
        // Bedrock uses AWS credentials, so an empty key is the expected success value.
        let storage = crate::credentials::CredentialsStorageConfig::default();
        let result = resolve_api_key(None, None, ProviderType::Bedrock, &storage).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_api_key_vertex_returns_empty_without_key() {
        // Vertex uses GCP credentials, so an empty key is the expected success value.
        let storage = crate::credentials::CredentialsStorageConfig::default();
        let result = resolve_api_key(None, None, ProviderType::Vertex, &storage).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn credentials_store_key_maps_bearer_providers_and_excludes_oob() {
        // Out-of-band auth (cloud creds / OAuth) has no store slot.
        assert_eq!(credentials_store_key(ProviderType::Bedrock), None);
        assert_eq!(credentials_store_key(ProviderType::Vertex), None);
        assert_eq!(credentials_store_key(ProviderType::OpenAIChatGpt), None);
        // Bearer-key providers map to `providers.<slug>.api_key`, including the
        // hyphenated slugs that are easy to get wrong by hand.
        assert_eq!(
            credentials_store_key(ProviderType::Anthropic).as_deref(),
            Some("providers.anthropic.api_key")
        );
        assert_eq!(
            credentials_store_key(ProviderType::AzureOpenAI).as_deref(),
            Some("providers.azure-openai.api_key")
        );
        assert_eq!(
            credentials_store_key(ProviderType::FluxRouter).as_deref(),
            Some("providers.flux-router.api_key")
        );
    }

    #[test]
    fn stored_key_is_read_back_by_resolution() {
        // The contract paste-to-detect depends on: a key written under
        // `credentials_store_key` is the exact key resolution reads back, so a
        // saved credential resolves live on the next rebind. Exercised through
        // the real read path (`lookup_store_api_key`) against a plaintext store,
        // with no process-env mutation.
        use crate::credentials::CredentialsStore;
        let dir = tempfile::tempdir().unwrap();
        let store =
            crate::credentials::PlaintextCredentialsStore::new(dir.path().join("creds.toml"));
        let write_key = credentials_store_key(ProviderType::Deepseek).unwrap();
        store.put(&write_key, "sk-deepseek-secret").unwrap();

        let read = lookup_store_api_key(&store, ProviderType::Deepseek);
        assert_eq!(read.as_deref(), Some("sk-deepseek-secret"));

        // A provider with no slot resolves to nothing from the store.
        assert_eq!(lookup_store_api_key(&store, ProviderType::Bedrock), None);
    }

    // -------------------------------------------------------------------------
    // P5-14: SkillsPermissionConfig TOML deserialization
    // -------------------------------------------------------------------------

    #[test]
    fn test_merge_config_global_auto_approve_preserved_with_project_allow_list() {
        let global = ConfigFile {
            tools: ToolsConfig {
                auto_approve: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let project = ConfigFile {
            tools: ToolsConfig {
                allow_list: vec!["Bash".into()], // non-default, triggers if branch
                ..Default::default()
            },
            ..Default::default()
        };
        let merged = merge_config_files(global, project);
        assert!(
            merged.tools.auto_approve,
            "global auto_approve=true should be preserved"
        );
    }

    #[test]
    fn p5_14_skills_deny_allow_deserialized() {
        let toml_str = r#"
[tools]
auto_approve = false
allow_list = ["Read"]

[tools.skills]
deny = ["dangerous-skill", "admin:*"]
allow = ["commit", "review-pr", "db:*"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.tools.skills.deny,
            vec!["dangerous-skill".to_string(), "admin:*".to_string()]
        );
        assert_eq!(
            config.tools.skills.allow,
            vec![
                "commit".to_string(),
                "review-pr".to_string(),
                "db:*".to_string()
            ]
        );
    }

    #[test]
    fn p5_14_skills_defaults_to_empty() {
        // When [tools.skills] is absent, deny and allow default to empty vecs.
        let config: ConfigFile = toml::from_str("").unwrap();
        assert!(config.tools.skills.deny.is_empty());
        assert!(config.tools.skills.allow.is_empty());
    }

    #[test]
    fn tools_windows_shell_deserializes_and_defaults_none() {
        // #182: the desktop writes `[tools] windows_shell = "powershell"`.
        let config: ConfigFile =
            toml::from_str("[tools]\nwindows_shell = \"powershell\"\n").unwrap();
        assert_eq!(config.tools.windows_shell.as_deref(), Some("powershell"));
        // Absent → None (default `cmd` shell on Windows).
        let bare: ConfigFile = toml::from_str("").unwrap();
        assert_eq!(bare.tools.windows_shell, None);
    }

    #[test]
    fn p5_14_merge_skills_concat() {
        // global and project skills lists are concatenated.
        let global = ConfigFile {
            tools: ToolsConfig {
                skills: SkillsPermissionConfig {
                    deny: vec!["global-deny".to_string()],
                    allow: vec!["global-allow".to_string()],
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let project = ConfigFile {
            tools: ToolsConfig {
                skills: SkillsPermissionConfig {
                    deny: vec!["project-deny".to_string()],
                    allow: vec!["project-allow".to_string()],
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);
        assert_eq!(
            merged.tools.skills.deny,
            vec!["global-deny".to_string(), "project-deny".to_string()]
        );
        assert_eq!(
            merged.tools.skills.allow,
            vec!["global-allow".to_string(), "project-allow".to_string()]
        );
    }

    // -------------------------------------------------------------------------
    // ConfigFile TOML deserialization tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_file_deserialize_minimal() {
        // An empty TOML string should deserialize to all defaults without error.
        let config: ConfigFile = toml::from_str("").unwrap();

        assert_eq!(config.default.provider, "anthropic");
        assert_eq!(config.default.max_tokens, 64000);
        assert_eq!(config.default.max_turns, None);
        assert!(config.default.model.is_none());
        assert!(config.providers.is_empty());
        assert!(config.profiles.is_empty());
    }

    #[test]
    fn test_config_file_deserialize_with_providers() {
        let toml_str = r#"
[default]
provider = "openai"
model = "gpt-4o"
max_tokens = 4096

[providers.openai]
api_key = "sk-test-key"
base_url = "https://api.openai.com"

[providers.anthropic]
api_key = "sk-ant-test"
prompt_caching = false
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();

        assert_eq!(config.default.provider, "openai");
        assert_eq!(config.default.model, Some("gpt-4o".to_string()));
        assert_eq!(config.default.max_tokens, 4096);

        let openai = config.providers.get("openai").unwrap();
        assert_eq!(openai.api_key.as_deref(), Some("sk-test-key"));
        assert_eq!(openai.base_url.as_deref(), Some("https://api.openai.com"));

        let anthropic = config.providers.get("anthropic").unwrap();
        assert_eq!(anthropic.api_key.as_deref(), Some("sk-ant-test"));
        assert_eq!(anthropic.prompt_caching, Some(false));
    }

    #[test]
    fn test_config_file_deserialize_custom_provider_alias() {
        let toml_str = r#"
[default]
provider = "my-service"

[providers.my-service]
provider = "openai"
model = "custom-model-v1"
api_key = "alias-key"
base_url = "https://my-service.example.com/api/openai"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();

        assert_eq!(config.default.provider, "my-service");
        let alias = config.providers.get("my-service").unwrap();
        assert_eq!(alias.provider.as_deref(), Some("openai"));
        assert_eq!(alias.model.as_deref(), Some("custom-model-v1"));
        assert_eq!(alias.api_key.as_deref(), Some("alias-key"));
        assert_eq!(
            alias.base_url.as_deref(),
            Some("https://my-service.example.com/api/openai")
        );
    }

    // -------------------------------------------------------------------------
    // merge_provider_configs tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_merge_provider_configs_overlay_overrides_base() {
        let base = ProviderConfig {
            api_key: Some("base-key".to_string()),
            base_url: Some("https://base.example.com".to_string()),
            model: Some("base-model".to_string()),
            ..Default::default()
        };
        let overlay = ProviderConfig {
            api_key: Some("overlay-key".to_string()),
            model: Some("overlay-model".to_string()),
            ..Default::default()
        };

        let merged = merge_provider_configs(base, overlay);
        assert_eq!(merged.api_key.as_deref(), Some("overlay-key"));
        assert_eq!(merged.model.as_deref(), Some("overlay-model"));
        // base_url not in overlay -> preserved from base
        assert_eq!(merged.base_url.as_deref(), Some("https://base.example.com"));
    }

    #[test]
    fn test_merge_provider_configs_overlay_none_preserves_base() {
        let base = ProviderConfig {
            api_key: Some("base-key".to_string()),
            base_url: Some("https://base.example.com".to_string()),
            model: Some("base-model".to_string()),
            prompt_caching: Some(true),
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let overlay = ProviderConfig::default();

        let merged = merge_provider_configs(base, overlay);
        assert_eq!(merged.api_key.as_deref(), Some("base-key"));
        assert_eq!(merged.base_url.as_deref(), Some("https://base.example.com"));
        assert_eq!(merged.model.as_deref(), Some("base-model"));
        assert_eq!(merged.prompt_caching, Some(true));
        assert_eq!(merged.provider.as_deref(), Some("openai"));
    }

    #[test]
    fn test_merge_provider_configs_compat_merges_both() {
        let base = ProviderConfig {
            compat: Some(ProviderCompat {
                merge_assistant_messages: Some(true),
                clean_orphan_tool_calls: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let overlay = ProviderConfig {
            compat: Some(ProviderCompat {
                merge_assistant_messages: Some(false), // override base
                dedup_tool_results: Some(true),        // new field
                ..Default::default()
            }),
            ..Default::default()
        };

        let merged = merge_provider_configs(base, overlay);
        let compat = merged.compat.unwrap();
        // overlay wins
        assert_eq!(compat.merge_assistant_messages, Some(false));
        // base preserved
        assert_eq!(compat.clean_orphan_tool_calls, Some(true));
        // overlay adds new
        assert_eq!(compat.dedup_tool_results, Some(true));
    }

    #[test]
    fn test_merge_provider_configs_both_empty() {
        let merged = merge_provider_configs(ProviderConfig::default(), ProviderConfig::default());
        assert!(merged.api_key.is_none());
        assert!(merged.base_url.is_none());
        assert!(merged.model.is_none());
        assert!(merged.provider.is_none());
        assert!(merged.prompt_caching.is_none());
        assert!(merged.compat.is_none());
    }

    // -------------------------------------------------------------------------
    // resolve_provider_alias: builtin name path tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_resolve_builtin_provider_with_config() {
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                api_key: Some("openai-key".to_string()),
                base_url: Some("https://custom-openai.example.com".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "openai").unwrap();
        assert_eq!(resolved.requested_name, "openai");
        assert_eq!(resolved.provider_type, ProviderType::OpenAI);
        assert_eq!(
            resolved.effective_config.api_key.as_deref(),
            Some("openai-key")
        );
        assert_eq!(
            resolved.effective_config.base_url.as_deref(),
            Some("https://custom-openai.example.com")
        );
    }

    #[test]
    fn test_resolve_builtin_provider_without_config_entry() {
        let providers = HashMap::new();

        let resolved = resolve_provider_alias(&providers, "anthropic").unwrap();
        assert_eq!(resolved.requested_name, "anthropic");
        assert_eq!(resolved.provider_type, ProviderType::Anthropic);
        // No config entry -> all fields default to None
        assert!(resolved.effective_config.api_key.is_none());
        assert!(resolved.effective_config.base_url.is_none());
        assert!(resolved.effective_config.model.is_none());
    }

    // -------------------------------------------------------------------------
    // resolve_provider_alias: error path tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_resolve_alias_maps_to_invalid_builtin_type() {
        let mut providers = HashMap::new();
        providers.insert(
            "my-db".to_string(),
            ProviderConfig {
                provider: Some("mysql".to_string()),
                ..Default::default()
            },
        );

        let result = resolve_provider_alias(&providers, "my-db");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("my-db"));
        assert!(msg.contains("mysql"));
        assert!(msg.contains("not a built-in provider"));
    }

    #[test]
    fn test_resolve_alias_not_found_in_providers() {
        let providers = HashMap::new();

        let result = resolve_provider_alias(&providers, "nonexistent");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nonexistent"));
        assert!(msg.contains("built-in provider"));
        assert!(msg.contains("[providers.nonexistent]"));
    }

    // -------------------------------------------------------------------------
    // provider_label (requested_name) tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_provider_label_is_alias_name_not_underlying_type() {
        let mut providers = HashMap::new();
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                api_key: Some("key".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        // provider_label should be the alias name, not "openai"
        assert_eq!(resolved.requested_name, "my-service");
        assert_eq!(resolved.provider_type, ProviderType::OpenAI);
    }

    #[test]
    fn test_provider_label_is_builtin_name_for_builtin() {
        let providers = HashMap::new();

        for (name, expected_type) in [
            ("anthropic", ProviderType::Anthropic),
            ("openai", ProviderType::OpenAI),
            ("bedrock", ProviderType::Bedrock),
            ("vertex", ProviderType::Vertex),
        ] {
            let resolved = resolve_provider_alias(&providers, name).unwrap();
            assert_eq!(resolved.requested_name, name);
            assert_eq!(resolved.provider_type, expected_type);
        }
    }

    // -------------------------------------------------------------------------
    // model priority: alias model in resolution chain
    // -------------------------------------------------------------------------

    #[test]
    fn test_alias_model_available_in_effective_config() {
        // Verifies that alias.model is carried through effective_config,
        // which feeds into the priority chain: CLI > alias.model > default.model > hardcoded
        let mut providers = HashMap::new();
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                model: Some("alias-model-v1".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        assert_eq!(
            resolved.effective_config.model.as_deref(),
            Some("alias-model-v1")
        );
    }

    #[test]
    fn test_alias_model_inherits_from_underlying_provider() {
        // When alias has no model but underlying provider does,
        // the alias should inherit it via merge_provider_configs
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                model: Some(OPENAI_GPT4O.to_string()),
                ..Default::default()
            },
        );
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                base_url: Some("https://my-service.example.com".to_string()),
                // no model -> should inherit from openai
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        assert_eq!(
            resolved.effective_config.model.as_deref(),
            Some(OPENAI_GPT4O)
        );
    }

    #[test]
    fn test_alias_model_overrides_underlying_provider_model() {
        // When both alias and underlying provider define model,
        // alias model should win
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                model: Some("gpt-4o".to_string()),
                ..Default::default()
            },
        );
        providers.insert(
            "my-service".to_string(),
            ProviderConfig {
                provider: Some("openai".to_string()),
                model: Some("custom-model-v2".to_string()),
                ..Default::default()
            },
        );

        let resolved = resolve_provider_alias(&providers, "my-service").unwrap();
        assert_eq!(
            resolved.effective_config.model.as_deref(),
            Some("custom-model-v2")
        );
    }

    // -------------------------------------------------------------------------
    // Phase 5.5: FileCacheConfig in ConfigFile / merge
    // -------------------------------------------------------------------------

    #[test]
    fn tc_5_5_04_file_cache_toml_deserialization() {
        let toml_str = r#"
[file_cache]
max_entries = 50
max_size_bytes = 10485760
enabled = false
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.file_cache.max_entries, 50);
        assert_eq!(config.file_cache.max_size_bytes, 10_485_760);
        assert!(!config.file_cache.enabled);
    }

    #[test]
    fn tc_5_5_02_file_cache_defaults_when_absent() {
        let config: ConfigFile = toml::from_str("").unwrap();
        assert_eq!(config.file_cache.max_entries, 100);
        assert_eq!(config.file_cache.max_size_bytes, 25 * 1024 * 1024);
        assert!(config.file_cache.enabled);
    }

    #[test]
    fn tc_5_5_01_file_cache_custom_capacity_propagates() {
        let toml_str = r#"
[file_cache]
max_entries = 50
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.file_cache.max_entries, 50);
        // Other fields keep defaults.
        assert_eq!(config.file_cache.max_size_bytes, 25 * 1024 * 1024);
        assert!(config.file_cache.enabled);
    }

    #[test]
    fn tc_5_5_03_file_cache_disabled_propagates() {
        let toml_str = r#"
[file_cache]
enabled = false
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert!(!config.file_cache.enabled);
    }

    #[test]
    fn merge_file_cache_project_overrides_global() {
        let global = ConfigFile {
            file_cache: FileCacheConfig {
                max_entries: 200,
                max_size_bytes: 50 * 1024 * 1024,
                enabled: true,
            },
            ..Default::default()
        };
        let project = ConfigFile {
            file_cache: FileCacheConfig {
                max_entries: 50,
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);
        assert_eq!(
            merged.file_cache.max_entries, 50,
            "project non-default max_entries should override global"
        );
    }

    #[test]
    fn merge_file_cache_global_preserved_when_project_default() {
        let global = ConfigFile {
            file_cache: FileCacheConfig {
                max_entries: 200,
                max_size_bytes: 50 * 1024 * 1024,
                enabled: true,
            },
            ..Default::default()
        };
        let project = ConfigFile::default();

        let merged = merge_config_files(global, project);
        assert_eq!(
            merged.file_cache.max_entries, 200,
            "global should be preserved when project is all-default"
        );
        assert_eq!(merged.file_cache.max_size_bytes, 50 * 1024 * 1024);
    }

    #[test]
    fn merge_file_cache_project_max_size_bytes_overrides_global() {
        // R-5.5-01: project changes only max_size_bytes (enabled=true, max_entries=default).
        let global = ConfigFile {
            file_cache: FileCacheConfig {
                max_entries: 100,
                max_size_bytes: 50 * 1024 * 1024,
                enabled: true,
            },
            ..Default::default()
        };
        let project = ConfigFile {
            file_cache: FileCacheConfig {
                max_entries: 100,                 // default
                max_size_bytes: 10 * 1024 * 1024, // non-default
                enabled: true,                    // default
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);
        assert_eq!(
            merged.file_cache.max_size_bytes,
            10 * 1024 * 1024,
            "project max_size_bytes should override global"
        );
    }

    #[test]
    fn merge_file_cache_disabled_overrides_global() {
        let global = ConfigFile {
            file_cache: FileCacheConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let project = ConfigFile {
            file_cache: FileCacheConfig {
                enabled: false,
                ..Default::default()
            },
            ..Default::default()
        };

        let merged = merge_config_files(global, project);
        assert!(
            !merged.file_cache.enabled,
            "project enabled=false should override global"
        );
    }

    #[test]
    fn test_resolve_with_project_dir_loads_project_config() {
        let tmp = tempfile::tempdir().unwrap();
        let project_toml = tmp.path().join(".wayland-core.toml");
        std::fs::write(
            &project_toml,
            r#"
[default]
max_tokens = 1234
"#,
        )
        .unwrap();

        let cli_args = CliArgs {
            provider: Some("anthropic".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            max_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };

        let config = Config::resolve(&cli_args).unwrap();
        assert_eq!(config.max_tokens, 1234);
    }

    #[test]
    fn patch_config_file_preserves_unrelated_keys() {
        // The keystone property: a partial save must NOT clobber blocks the
        // surface doesn't edit (MCP servers, hooks, providers, profiles).
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[default]
provider = "anthropic"
model = "claude-sonnet-4-6"
max_turns = 10

[providers.anthropic]
api_key = "sk-ant-keepme"

[memory]
enabled = false
"#,
        )
        .unwrap();

        // Patch only memory.enabled — everything else must survive. The
        // `[memory]` table is present in the source, so `memory` is `Some`.
        patch_config_file_at(&path, |f| {
            f.memory.get_or_insert_with(MemoryConfig::default).enabled = true
        })
        .unwrap();

        let reloaded: ConfigFile =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            reloaded.memory.expect("memory table present").enabled,
            "the patched field must persist"
        );
        assert_eq!(
            reloaded.default.model.as_deref(),
            Some("claude-sonnet-4-6"),
            "an unrelated [default] key must survive the patch"
        );
        assert_eq!(
            reloaded.default.max_turns,
            Some(10),
            "max_turns must survive the patch"
        );
        assert_eq!(
            reloaded
                .providers
                .get("anthropic")
                .and_then(|p| p.api_key.as_deref()),
            Some("sk-ant-keepme"),
            "the provider api key block must NOT be clobbered by a partial save"
        );
    }

    #[test]
    fn patch_config_file_creates_a_fresh_file_when_absent() {
        // No file yet → start from ConfigFile::default(), apply, write.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("config.toml");
        assert!(!path.exists());

        patch_config_file_at(&path, |f| f.default.max_turns = Some(42)).unwrap();

        assert!(
            path.exists(),
            "the writer must create the file + parent dir"
        );
        let reloaded: ConfigFile =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reloaded.default.max_turns, Some(42));
    }

    #[test]
    fn approval_mode_parses_from_toml_and_resolves_onto_config() {
        // The full path: `[default] approval_mode` in TOML → resolved
        // Config.approval_mode (what the TUI boot consumer reads).
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join(".wayland-core.toml");
        std::fs::write(&project, "[default]\napproval_mode = \"auto-edit\"\n").unwrap();
        let cli = CliArgs {
            provider: Some("anthropic".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            max_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: Some(tmp.path().to_path_buf()),
        };
        let config = Config::resolve(&cli).unwrap();
        assert_eq!(config.approval_mode, ApprovalMode::AutoEdit);
    }

    #[test]
    fn approval_mode_wire_strings_round_trip() {
        for m in [
            ApprovalMode::Default,
            ApprovalMode::AutoEdit,
            ApprovalMode::Force,
        ] {
            assert_eq!(ApprovalMode::from_wire(m.as_str()), m);
        }
        assert_eq!(ApprovalMode::Force.as_str(), "force");
        assert_eq!(ApprovalMode::from_wire("garbage"), ApprovalMode::Default);
    }

    #[test]
    fn test_resolve_without_project_dir_uses_cwd() {
        let cli_args = CliArgs {
            provider: Some("anthropic".into()),
            api_key: Some("test-key".into()),
            base_url: None,
            model: None,
            max_tokens: None,
            max_turns: None,
            system_prompt: None,
            profile: None,
            auto_approve: false,
            project_dir: None,
        };

        let config = Config::resolve(&cli_args);
        assert!(config.is_ok());
    }

    // -------------------------------------------------------------------------
    // W1 Task 10: observability.structured_traces opt-in
    // -------------------------------------------------------------------------

    #[test]
    fn observability_structured_traces_defaults_false() {
        let cfg: ConfigFile = toml::from_str("").unwrap();
        assert!(!cfg.observability.structured_traces);
    }

    #[test]
    fn observability_structured_traces_round_trips_through_toml() {
        let toml_src = r#"
[observability]
structured_traces = true
        "#;
        let cfg: ConfigFile = toml::from_str(toml_src).unwrap();
        assert!(cfg.observability.structured_traces);
    }

    // -------------------------------------------------------------------------
    // W9 Task 10a: observability.skills_lifecycle opt-in
    // -------------------------------------------------------------------------

    #[test]
    fn observability_skills_lifecycle_defaults_true() {
        // Smart default (2026-06-04): the learn-and-evolve loop ships ON so it
        // runs out of the box. Both the serde (TOML-omitted) and struct paths
        // must agree, since a no-config first run uses `ConfigFile::default()`.
        let from_toml: ConfigFile = toml::from_str("").unwrap();
        assert!(
            from_toml.observability.skills_lifecycle,
            "skills_lifecycle must default ON (serde/TOML-omitted path)"
        );
        assert!(
            ConfigFile::default().observability.skills_lifecycle,
            "skills_lifecycle must default ON (struct Default path — no-config first run)"
        );
    }

    #[test]
    fn observability_skills_lifecycle_explicit_opt_out_respected() {
        let cfg: ConfigFile = toml::from_str(
            r#"
[observability]
skills_lifecycle = false
        "#,
        )
        .unwrap();
        assert!(
            !cfg.observability.skills_lifecycle,
            "explicit opt-out must be honored"
        );
    }

    #[test]
    fn observability_skills_lifecycle_round_trips_through_toml() {
        let toml_src = r#"
[observability]
skills_lifecycle = true
        "#;
        let cfg: ConfigFile = toml::from_str(toml_src).unwrap();
        assert!(cfg.observability.skills_lifecycle);
        // Independent from structured_traces — flipping one must not flip
        // the other.
        assert!(!cfg.observability.structured_traces);
    }

    #[test]
    fn observability_skills_lifecycle_merges_global_and_project() {
        // Project-on, global-off → on. (Mirrors structured_traces merge.)
        let global: ConfigFile = toml::from_str("").unwrap();
        let project: ConfigFile = toml::from_str(
            r#"
[observability]
skills_lifecycle = true
        "#,
        )
        .unwrap();
        let merged = merge_config_files(global, project);
        assert!(merged.observability.skills_lifecycle);
    }

    // -------------------------------------------------------------------------
    // F-010: wayland_config_dir() canonical helper tests
    // -------------------------------------------------------------------------

    #[test]
    fn wayland_config_dir_uses_wayland_home_when_set() {
        // Serial isolation is not required here because we restore the env var
        // within the test; the variable name is unique to this assertion.
        let key = "WAYLAND_HOME";
        let prev = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, "/tmp/test-wayland-home");
        }
        let dir = wayland_config_dir();
        match prev {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
        assert_eq!(dir, std::path::PathBuf::from("/tmp/test-wayland-home"));
    }

    #[test]
    fn wayland_config_dir_uses_xdg_data_home_when_no_wayland_home() {
        let wh_key = "WAYLAND_HOME";
        let xdg_key = "XDG_DATA_HOME";
        let prev_wh = std::env::var_os(wh_key);
        let prev_xdg = std::env::var_os(xdg_key);
        unsafe {
            std::env::remove_var(wh_key);
            std::env::set_var(xdg_key, "/tmp/test-xdg");
        }
        let dir = wayland_config_dir();
        match prev_wh {
            Some(v) => unsafe { std::env::set_var(wh_key, v) },
            None => unsafe { std::env::remove_var(wh_key) },
        }
        match prev_xdg {
            Some(v) => unsafe { std::env::set_var(xdg_key, v) },
            None => unsafe { std::env::remove_var(xdg_key) },
        }
        assert_eq!(dir, std::path::PathBuf::from("/tmp/test-xdg/wayland-core"));
    }

    #[test]
    fn wayland_config_dir_falls_back_to_dirs_config_dir() {
        // When neither env var is set, result ends with "wayland-core".
        let wh_key = "WAYLAND_HOME";
        let xdg_key = "XDG_DATA_HOME";
        let prev_wh = std::env::var_os(wh_key);
        let prev_xdg = std::env::var_os(xdg_key);
        unsafe {
            std::env::remove_var(wh_key);
            std::env::remove_var(xdg_key);
        }
        let dir = wayland_config_dir();
        match prev_wh {
            Some(v) => unsafe { std::env::set_var(wh_key, v) },
            None => unsafe { std::env::remove_var(wh_key) },
        }
        match prev_xdg {
            Some(v) => unsafe { std::env::set_var(xdg_key, v) },
            None => unsafe { std::env::remove_var(xdg_key) },
        }
        assert!(
            dir.ends_with("wayland-core"),
            "expected path ending in wayland-core, got {}",
            dir.display()
        );
    }

    // -------------------------------------------------------------------------
    // profile_home() — canonical ~/.wayland resolution (B1)
    // -------------------------------------------------------------------------

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn profile_home_uses_wayland_home_override() {
        let key = "WAYLAND_HOME";
        let prev = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, "/tmp/test-profile-home");
        }
        let home = profile_home();
        match prev {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
        assert_eq!(home, std::path::PathBuf::from("/tmp/test-profile-home"));
    }

    // F12: an override containing a control char (e.g. NUL) is ignored — we
    // fall through to the default instead of propagating a poisoned value.
    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn profile_home_ignores_control_char_override() {
        let key = "WAYLAND_HOME";
        let prev = std::env::var_os(key);
        // A tab/newline is a control char `set_var` still accepts (unlike NUL),
        // so it exercises the guard without panicking the test harness.
        unsafe {
            std::env::set_var(key, "/tmp/evil\tinjected");
        }
        let home = profile_home();
        match prev {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
        assert!(
            !home.to_string_lossy().contains('\t'),
            "control-char override must not be propagated, got {}",
            home.display()
        );
        assert!(
            home.ends_with(".wayland"),
            "must fall through to the default, got {}",
            home.display()
        );
    }

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn profile_home_defaults_to_home_dot_wayland() {
        let key = "WAYLAND_HOME";
        let prev = std::env::var_os(key);
        unsafe {
            std::env::remove_var(key);
        }
        let home = profile_home();
        match prev {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
        // Default ends in ".wayland" and is anchored at the user's home dir,
        // never a hardcoded absolute root.
        assert!(
            home.ends_with(".wayland"),
            "expected path ending in .wayland, got {}",
            home.display()
        );
        if let Some(h) = dirs::home_dir() {
            assert_eq!(home, h.join(".wayland"));
        }
    }

    // -------------------------------------------------------------------------
    // #275 / F-010: yaml→toml migration must honour WAYLAND_HOME
    //
    // Pre-fix bug: `migrate_legacy_yaml_if_needed` resolved the legacy yaml
    // path against `dirs::home_dir()`, so every sandboxed/test process under
    // `WAYLAND_HOME` was reading the real user's `~/.wayland/config.yaml`.
    // That broke hermeticity and polluted test runs.
    // -------------------------------------------------------------------------

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn migrate_legacy_yaml_reads_from_wayland_home_when_set() {
        let wh_key = "WAYLAND_HOME";
        let xdg_key = "XDG_DATA_HOME";
        let prev_wh = std::env::var_os(wh_key);
        let prev_xdg = std::env::var_os(xdg_key);

        // Sandbox: `WAYLAND_HOME` points at an isolated tempdir that doubles
        // as the legacy-yaml lookup root and the canonical TOML root.
        let sandbox = tempfile::tempdir().expect("tempdir sandbox");
        let sandbox_path = sandbox.path().to_path_buf();

        unsafe {
            std::env::set_var(wh_key, &sandbox_path);
            // Remove XDG so wayland_config_dir() resolves purely via WAYLAND_HOME.
            std::env::remove_var(xdg_key);
        }

        // Seed a sentinel yaml INSIDE the sandbox.  The migration must read
        // THIS file (not Sean's real ~/.wayland/config.yaml on the host).
        let sandbox_yaml = sandbox_path.join("config.yaml");
        std::fs::write(
            &sandbox_yaml,
            "model:\n  default: sentinel-from-sandbox\n  provider: openai\n",
        )
        .expect("seed sandbox yaml");

        // Run the migration.  Canonical TOML must be created INSIDE the
        // sandbox with the sentinel model, proving the migration honoured
        // WAYLAND_HOME on BOTH the read path (yaml lookup) and the write
        // path (canonical TOML).
        migrate_legacy_yaml_if_needed();

        let canonical_toml = sandbox_path.join("config.toml");
        let toml_contents = std::fs::read_to_string(&canonical_toml).unwrap_or_default();

        // Restore env BEFORE assertions so a failure doesn't leak state into
        // sibling tests.
        match prev_wh {
            Some(v) => unsafe { std::env::set_var(wh_key, v) },
            None => unsafe { std::env::remove_var(wh_key) },
        }
        match prev_xdg {
            Some(v) => unsafe { std::env::set_var(xdg_key, v) },
            None => unsafe { std::env::remove_var(xdg_key) },
        }

        assert!(
            canonical_toml.exists(),
            "migration did not create canonical TOML at {} — \
             likely read yaml from real $HOME instead of WAYLAND_HOME",
            canonical_toml.display()
        );
        assert!(
            toml_contents.contains("sentinel-from-sandbox"),
            "canonical TOML missing sandbox sentinel model; \
             contents:\n{toml_contents}\n\
             (this means the migration read yaml from somewhere other than \
             WAYLAND_HOME — hermeticity bug)"
        );
    }

    // -------------------------------------------------------------------------
    // S9: effective-config preview (`effective_config_toml`) + secret redaction.
    // -------------------------------------------------------------------------

    #[test]
    fn redact_masks_secret_named_keys_at_any_depth() {
        // The redaction walk must mask credential-shaped keys wherever they
        // appear — top-level, nested tables, and inside header tables — while
        // leaving non-secret values (and non-string secret leaves) intact.
        let mut value: toml::Value = toml::from_str(
            r#"
            [default]
            provider = "anthropic"

            [providers.anthropic]
            api_key = "sk-ant-SECRET"
            base_url = "https://api.anthropic.com"

            [channels.telegram]
            bot_token = "12345:SECRET"
            chat_id = 99

            [mcp.servers.notion.headers]
            Authorization = "Bearer SECRET"
            "#,
        )
        .expect("parse fixture");

        redact_secrets_in_place(&mut value);
        let out = toml::to_string_pretty(&value).expect("serialize");

        assert!(!out.contains("SECRET"), "a secret leaked:\n{out}");
        assert!(
            out.contains("api_key = \"***\""),
            "api_key not masked:\n{out}"
        );
        assert!(
            out.contains("bot_token = \"***\""),
            "token not masked:\n{out}"
        );
        assert!(
            out.contains("Authorization = \"***\""),
            "auth header not masked:\n{out}"
        );
        // Non-secret values survive.
        assert!(
            out.contains("provider = \"anthropic\""),
            "provider lost:\n{out}"
        );
        assert!(out.contains("api.anthropic.com"), "base_url lost:\n{out}");
        assert!(out.contains("chat_id = 99"), "non-secret int lost:\n{out}");
    }

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn effective_config_toml_merges_and_redacts_from_disk() {
        let wh_key = "WAYLAND_HOME";
        let prev_wh = std::env::var_os(wh_key);
        let sandbox = tempfile::tempdir().expect("tempdir sandbox");
        // SAFETY: serialized by the `wayland_home_env` serial group.
        unsafe { std::env::set_var(wh_key, sandbox.path()) };

        std::fs::write(
            sandbox.path().join("config.toml"),
            "[default]\nprovider = \"anthropic\"\n\n\
             [providers.anthropic]\napi_key = \"sk-ant-LIVE-SECRET\"\n",
        )
        .expect("seed config.toml");

        let rendered = effective_config_toml(&CliArgs::default());

        // Restore env BEFORE asserting so a failure can't leak state.
        match prev_wh {
            Some(v) => unsafe { std::env::set_var(wh_key, v) },
            None => unsafe { std::env::remove_var(wh_key) },
        }

        let out = rendered.expect("effective config should render");
        assert!(
            out.contains("provider = \"anthropic\""),
            "merged provider missing:\n{out}"
        );
        assert!(
            !out.contains("sk-ant-LIVE-SECRET") && out.contains("***"),
            "the api key must be redacted in the preview:\n{out}"
        );
    }

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn effective_config_toml_stamps_cli_overrides() {
        let wh_key = "WAYLAND_HOME";
        let prev_wh = std::env::var_os(wh_key);
        // Empty sandbox (no config.toml) so the merge starts from defaults and
        // never reads the host's real config.
        let sandbox = tempfile::tempdir().expect("tempdir sandbox");
        // SAFETY: serialized by the `wayland_home_env` serial group.
        unsafe { std::env::set_var(wh_key, sandbox.path()) };

        let cli = CliArgs {
            provider: Some("openai".to_string()),
            model: Some("gpt-sentinel".to_string()),
            ..CliArgs::default()
        };
        let rendered = effective_config_toml(&cli);

        match prev_wh {
            Some(v) => unsafe { std::env::set_var(wh_key, v) },
            None => unsafe { std::env::remove_var(wh_key) },
        }

        let out = rendered.expect("effective config should render");
        assert!(
            out.contains("provider = \"openai\""),
            "CLI provider override not stamped:\n{out}"
        );
        assert!(
            out.contains("gpt-sentinel"),
            "CLI model override not stamped:\n{out}"
        );
    }

    // -------------------------------------------------------------------------
    // D011 (P0 dataloss): a config file that EXISTS but fails to parse must
    // surface a hard, typed error naming the file — NOT silently downgrade to
    // defaults (which behaves like a fresh install and wipes every user
    // setting). A genuinely-absent file still yields defaults (fresh install
    // is the correct behavior there).
    // -------------------------------------------------------------------------

    #[test]
    fn corrupt_config_file_surfaces_typed_parse_error_not_silent_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        // Stray trailing comma / dangling bracket — invalid TOML.
        std::fs::write(&path, "[default\nprovider = \"anthropic\",,\nmodel = \n")
            .expect("write corrupt config");

        let err = try_load_config_file(&path)
            .expect_err("a corrupt existing config must NOT silently downgrade to defaults");

        // The error must name the offending file so the user can find + fix it.
        let msg = err.to_string();
        assert!(
            msg.contains("config.toml") && msg.contains("parse"),
            "the parse error must name the file and say it failed to parse; got: {msg}"
        );
    }

    #[test]
    fn absent_config_file_yields_defaults_not_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.toml");
        assert!(!path.exists());

        // A genuinely-absent file is a fresh install: defaults are correct,
        // never an error.
        let file = try_load_config_file(&path).expect("absent file must yield defaults, not error");
        assert_eq!(file.default.provider, default_provider());
        assert!(file.providers.is_empty());
    }

    #[test]
    fn valid_config_file_round_trips_through_try_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[default]\nprovider = \"openai\"\n").expect("write config");

        let file = try_load_config_file(&path).expect("valid config must load");
        assert_eq!(file.default.provider, "openai");
    }

    // -------------------------------------------------------------------------
    // Migration re-fire (P0 dataloss): the guard keys on the canonical TOML's
    // EXISTENCE, not on whether a `[default]` model is set. A legacy yaml with
    // no model previously left config.toml without a model, so the migration
    // re-serialized config.toml on EVERY launch — destroying user comments and
    // any field outside ConfigFile. Once config.toml exists, migration must be
    // a no-op and leave the file byte-identical.
    // -------------------------------------------------------------------------

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn migrate_legacy_yaml_skips_when_canonical_toml_exists() {
        let wh_key = "WAYLAND_HOME";
        let xdg_key = "XDG_DATA_HOME";
        let prev_wh = std::env::var_os(wh_key);
        let prev_xdg = std::env::var_os(xdg_key);

        let sandbox = tempfile::tempdir().expect("tempdir sandbox");
        let sandbox_path = sandbox.path().to_path_buf();

        unsafe {
            std::env::set_var(wh_key, &sandbox_path);
            std::env::remove_var(xdg_key);
        }

        // A legacy yaml with NO model — the case that defeated the old
        // model-presence guard.
        std::fs::write(
            sandbox_path.join("config.yaml"),
            "memory:\n  memory_enabled: true\n",
        )
        .expect("seed sandbox yaml");

        // A pre-existing canonical TOML carrying a user comment and a field
        // (## MARKER) that ConfigFile would drop on re-serialization.
        let canonical_toml = sandbox_path.join("config.toml");
        let original = "## MARKER: hand-authored, must survive migration\n\
                        [default]\nprovider = \"openai\"\n";
        std::fs::write(&canonical_toml, original).expect("seed canonical toml");

        migrate_legacy_yaml_if_needed();

        let after = std::fs::read_to_string(&canonical_toml).unwrap_or_default();

        // Restore env BEFORE assertions so a failure doesn't leak state.
        match prev_wh {
            Some(v) => unsafe { std::env::set_var(wh_key, v) },
            None => unsafe { std::env::remove_var(wh_key) },
        }
        match prev_xdg {
            Some(v) => unsafe { std::env::set_var(xdg_key, v) },
            None => unsafe { std::env::remove_var(xdg_key) },
        }

        assert_eq!(
            after, original,
            "migration re-serialized an existing config.toml — the comment and \
             byte-for-byte content must be preserved when the canonical TOML \
             already exists"
        );
    }

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn migrate_legacy_yaml_writes_toml_on_first_run() {
        let wh_key = "WAYLAND_HOME";
        let xdg_key = "XDG_DATA_HOME";
        let prev_wh = std::env::var_os(wh_key);
        let prev_xdg = std::env::var_os(xdg_key);

        let sandbox = tempfile::tempdir().expect("tempdir sandbox");
        let sandbox_path = sandbox.path().to_path_buf();

        unsafe {
            std::env::set_var(wh_key, &sandbox_path);
            std::env::remove_var(xdg_key);
        }

        // Legacy yaml present, no canonical TOML yet: a genuine first migration.
        std::fs::write(
            sandbox_path.join("config.yaml"),
            "model:\n  default: first-run-model\n  provider: openai\n",
        )
        .expect("seed sandbox yaml");

        let canonical_toml = sandbox_path.join("config.toml");
        assert!(!canonical_toml.exists(), "precondition: no toml yet");

        migrate_legacy_yaml_if_needed();

        let toml_contents = std::fs::read_to_string(&canonical_toml).unwrap_or_default();

        match prev_wh {
            Some(v) => unsafe { std::env::set_var(wh_key, v) },
            None => unsafe { std::env::remove_var(wh_key) },
        }
        match prev_xdg {
            Some(v) => unsafe { std::env::set_var(xdg_key, v) },
            None => unsafe { std::env::remove_var(xdg_key) },
        }

        assert!(
            canonical_toml.exists(),
            "first migration must create the canonical TOML"
        );
        assert!(
            toml_contents.contains("first-run-model"),
            "first migration must carry the legacy model into the TOML; got:\n{toml_contents}"
        );
    }

    // -------------------------------------------------------------------------
    // connected_providers() / provider_connected() — credential detection
    // -------------------------------------------------------------------------

    /// Env vars that influence a provider's connection verdict. Cleared for the
    /// duration of each connected-providers test so the host environment can't
    /// leak a real key (or `API_KEY`, which `resolve_api_key` checks first).
    const CRED_ENV_KEYS: &[&str] = &[
        "HOME",
        "WAYLAND_HOME",
        "API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        // Ambient cloud credential sources read by the Bedrock/Vertex probes,
        // so the guard is hermetic for them too (sandboxed HOME clears the
        // `~/.aws/*` and ADC file fallbacks).
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_PROFILE",
        "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
        "AWS_CONTAINER_CREDENTIALS_FULL_URI",
        "AWS_WEB_IDENTITY_TOKEN_FILE",
        "AWS_SHARED_CREDENTIALS_FILE",
        "AWS_CONFIG_FILE",
        "GOOGLE_APPLICATION_CREDENTIALS",
    ];

    /// Hermetic credential environment: points `HOME` (the ChatGPT OAuth-file
    /// root) and `WAYLAND_HOME` (the credentials-store root) at fresh tempdirs
    /// and clears every credential env var, restoring all of them on drop.
    /// Tests using it must be `#[serial]`.
    struct CredEnvGuard {
        _home: tempfile::TempDir,
        _wh: tempfile::TempDir,
        prior: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl CredEnvGuard {
        fn new() -> Self {
            let home = tempfile::TempDir::new().unwrap();
            let wh = tempfile::TempDir::new().unwrap();
            let prior = CRED_ENV_KEYS
                .iter()
                .map(|k| (*k, std::env::var_os(k)))
                .collect();
            // SAFETY: callers are #[serial]; no concurrent env access.
            unsafe {
                for k in CRED_ENV_KEYS {
                    std::env::remove_var(k);
                }
                std::env::set_var("HOME", home.path());
                std::env::set_var("WAYLAND_HOME", wh.path());
            }
            Self {
                _home: home,
                _wh: wh,
                prior,
            }
        }

        /// Create the ChatGPT OAuth token file under the guarded `HOME`, exactly
        /// where `wcore_agent::oauth::OAuthStorage::from_home` would
        /// (`~/.wayland/oauth/chatgpt.json`).
        fn write_chatgpt_token(&self) {
            // Write where `chatgpt_oauth_token_path` reads — under the guarded
            // `WAYLAND_HOME` (via `profile_home`), so detection is hermetic on
            // every platform (Windows' `dirs::home_dir()` ignores `HOME`).
            let dir = crate::config::profile_home().join("oauth");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("chatgpt.json"), "{\"access_token\":\"t\"}").unwrap();
        }
    }

    impl Drop for CredEnvGuard {
        fn drop(&mut self) {
            // SAFETY: serialized; restore each prior value (or clear it).
            unsafe {
                for (k, v) in &self.prior {
                    match v {
                        Some(val) => std::env::set_var(k, val),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn connected_providers_detects_key_ambient_and_oauth_excludes_keyless() {
        let guard = CredEnvGuard::new();
        // Keyed provider: Anthropic via its env var.
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test") };
        // Ambient cloud: provide real credential sources via env (no home
        // dependency, so this is hermetic on Windows too where dirs::home_dir()
        // ignores HOME) — AWS static keys for Bedrock, an ADC path for Vertex.
        unsafe {
            std::env::set_var("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE");
            std::env::set_var("AWS_SECRET_ACCESS_KEY", "secret");
            std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", "/tmp/sa.json");
        }
        // OAuth provider: present token file = connected.
        guard.write_chatgpt_token();

        let connected = connected_providers();

        // Keyed provider detected.
        assert!(
            connected.contains(&ProviderType::Anthropic),
            "Anthropic with ANTHROPIC_API_KEY set must be connected: {connected:?}"
        );
        // Ambient cloud is connected when a credential source is present.
        assert!(
            connected.contains(&ProviderType::Bedrock),
            "Bedrock with AWS credentials must be connected: {connected:?}"
        );
        assert!(
            connected.contains(&ProviderType::Vertex),
            "Vertex with GOOGLE_APPLICATION_CREDENTIALS must be connected: {connected:?}"
        );
        // OAuth provider with a stored token file is connected.
        assert!(
            connected.contains(&ProviderType::OpenAIChatGpt),
            "ChatGPT with a stored token file must be connected: {connected:?}"
        );
        // Keyless providers are excluded.
        assert!(
            !connected.contains(&ProviderType::OpenAI),
            "OpenAI without OPENAI_API_KEY must NOT be connected: {connected:?}"
        );
        assert!(
            !connected.contains(&ProviderType::Gemini),
            "Gemini without GEMINI/GOOGLE_API_KEY must NOT be connected: {connected:?}"
        );
    }

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn provider_connected_oauth_false_without_token_file() {
        let _guard = CredEnvGuard::new();
        // No token file written → ChatGPT is not connected. (Ambient-cloud
        // connection is covered hermetically by
        // `ambient_cloud_connection_reflects_real_credentials`, which overrides
        // the AWS shared-file paths rather than relying on the home dir — the
        // only way to make it deterministic on Windows.)
        assert!(
            !provider_connected(ProviderType::OpenAIChatGpt),
            "ChatGPT without a stored token file must be unconnected"
        );
    }

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn for_provider_discovery_overrides_identifying_fields() {
        let _guard = CredEnvGuard::new();
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-openai-test") };
        let base = Config {
            provider: ProviderType::Anthropic,
            prompt_caching: true,
            ..Config::default()
        };
        let cfg = base.for_provider_discovery(ProviderType::OpenAI);
        assert_eq!(cfg.provider, ProviderType::OpenAI);
        assert_eq!(cfg.provider_label, "openai");
        assert_eq!(cfg.api_key, "sk-openai-test");
        assert_eq!(cfg.base_url, "https://api.openai.com");
        assert_eq!(cfg.compat.provider_type(), "openai");
        // Non-identifying fields are inherited from the base.
        assert!(
            cfg.prompt_caching,
            "for_provider_discovery must inherit base fields like prompt_caching"
        );
    }

    #[test]
    #[serial_test::serial(wayland_home_env)]
    fn ambient_cloud_connection_reflects_real_credentials() {
        // Snapshot every var these probes read so the test restores them.
        let keys = [
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_PROFILE",
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            "AWS_WEB_IDENTITY_TOKEN_FILE",
            "AWS_SHARED_CREDENTIALS_FILE",
            "AWS_CONFIG_FILE",
            "GOOGLE_APPLICATION_CREDENTIALS",
        ];
        let saved: Vec<(&str, Option<std::ffi::OsString>)> =
            keys.iter().map(|k| (*k, std::env::var_os(k))).collect();

        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");

        // SAFETY: serialized via the shared `wayland_home_env` group, so no
        // other env-reading test runs concurrently.
        unsafe {
            for k in keys {
                std::env::remove_var(k);
            }
            // Point the AWS shared-file lookups at nonexistent paths so the
            // `~/.aws/*` fallback is bypassed deterministically on every OS.
            std::env::set_var("AWS_SHARED_CREDENTIALS_FILE", &missing);
            std::env::set_var("AWS_CONFIG_FILE", &missing);
        }

        // No env keys + nonexistent shared files ⇒ Bedrock not connected.
        assert!(
            !provider_connected(ProviderType::Bedrock),
            "Bedrock must NOT be connected without any AWS credential source"
        );

        // Explicit static keys ⇒ connected.
        unsafe {
            std::env::set_var("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE");
            std::env::set_var("AWS_SECRET_ACCESS_KEY", "secret");
        }
        assert!(
            provider_connected(ProviderType::Bedrock),
            "explicit AWS keys must mark Bedrock connected"
        );

        // A GOOGLE_APPLICATION_CREDENTIALS path ⇒ Vertex connected.
        unsafe { std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", missing.as_os_str()) };
        assert!(
            provider_connected(ProviderType::Vertex),
            "GOOGLE_APPLICATION_CREDENTIALS must mark Vertex connected"
        );

        // Restore every var.
        // SAFETY: still inside the serial guard.
        unsafe {
            for (k, v) in saved {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}
