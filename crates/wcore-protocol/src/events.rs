use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use wcore_types::message::FinishReason;

/// Serde helper: skip serializing a `bool` field when it is `false`.
///
/// Used on W0 forward-additive `Capabilities` flags so default-off flags
/// don't appear in the serialized JSON — preserving v0.1.21 shape for
/// hosts that haven't learned about them yet. Removing this helper or
/// changing its semantics breaks the W0 invariant; the golden tests in
/// `tests/golden_v0_1_21.rs` will catch any regression.
fn is_false(b: &bool) -> bool {
    !*b
}

/// Events emitted by the agent to the client (Agent -> Client)
///
/// `Clone` is derived (Wave 2) so the in-process TUI bridge can fan an
/// event out across the protocol writer and the channel-backed sink
/// without re-serializing.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ProtocolEvent {
    Ready {
        version: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        capabilities: Capabilities,
    },
    StreamStart {
        msg_id: String,
    },
    TextDelta {
        text: String,
        msg_id: String,
    },
    Thinking {
        text: String,
        msg_id: String,
    },
    ToolRequest {
        msg_id: String,
        call_id: String,
        tool: ToolInfo,
    },
    ToolRunning {
        msg_id: String,
        call_id: String,
        tool_name: String,
    },
    ToolResult {
        msg_id: String,
        call_id: String,
        tool_name: String,
        status: ToolStatus,
        output: String,
        output_type: OutputType,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
    },
    ToolCancelled {
        msg_id: String,
        call_id: String,
        reason: String,
    },
    StreamEnd {
        msg_id: String,
        /// Why the stream ended. Required; engine emits `Error` if it can't
        /// classify the provider's stop signal. Host UIs should render
        /// `Length` as a truncation warning (closes the Gemini Pro
        /// reasoning-token empty-response bug at the protocol contract).
        finish_reason: FinishReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// #279(c): stable per-run correlation handle grouping every event of
        /// one agent run (survives multi-message / --resume). None on synthetic
        /// stream-ends (slash/exit/stop) that aren't a model run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_run_id: Option<String>,
    },
    Error {
        #[serde(skip_serializing_if = "Option::is_none")]
        msg_id: Option<String>,
        error: ErrorInfo,
    },
    Info {
        msg_id: String,
        message: String,
    },
    ConfigChanged {
        capabilities: Capabilities,
    },
    McpReady {
        name: String,
        tools: Vec<String>,
    },
    /// An MCP server failed (or timed out) at connect. The companion to
    /// [`McpReady`]: it carries the preserved failure cause so a host /
    /// the TUI `/doctor` view can tell the user *why* a server's tools never
    /// appeared, instead of the server silently vanishing. Additive — hosts
    /// that don't recognise it drop it per the W0 decoder contract.
    McpFailed {
        name: String,
        reason: String,
    },
    /// W1: F9 structured trace for one turn. Gated by the W0-reserved
    /// `capabilities.structured_traces` flag — the engine only emits this
    /// variant when the corresponding ProtocolSink builder was configured
    /// with `with_structured_traces(true)`. Hosts that don't recognise the
    /// `trace_event` `type` MUST drop it silently per the W0 host decoder
    /// contract; hosts that opt in surface it via their trace UI.
    ///
    /// The trace payload is `serde_json::Value` rather than a typed
    /// `TurnTrace` so this crate stays independent of `wcore-observability`
    /// (which depends on `wcore-config`, which would otherwise create a
    /// downstream protocol-crate dependency).
    TraceEvent {
        msg_id: String,
        trace: Value,
    },
    /// W6 F7: end-of-session cost aggregate. Gated by the W0-reserved
    /// `capabilities.cost_attribution` flag — engine emits this variant
    /// only when `AdvertisedCapabilitiesConfig.cost_attribution = true`
    /// (bootstrap flips this when ProviderCompat has cost rows; single
    /// authority per audit rev-2 finding 5). Hosts that don't recognise the
    /// `session_cost` `type` MUST drop it silently per the W0 host decoder
    /// contract; hosts that opt in surface it via their cost UI.
    ///
    /// Per-turn cost still rides inside `TraceEvent.trace.cost_usd` (gated
    /// by `capabilities.structured_traces`); this variant is the typed
    /// aggregate for hosts that don't want to parse trace JSON.
    SessionCost {
        session_id: String,
        total_cost_usd: f64,
        per_turn: Vec<TurnCost>,
    },
    /// W7: F2 sub-agent event. The inner payload is a serialized
    /// `ProtocolEvent` (kept as `serde_json::Value` here to avoid a
    /// recursive variant — the engine serializes the sub-agent's event
    /// to a Value before wrapping). `parent_call_id` groups events
    /// emitted by sub-agents spawned by a single `SpawnTool` call.
    /// Gated by the W0-reserved `capabilities.sub_agent_traces` flag —
    /// the engine only emits this when the corresponding ProtocolSink
    /// builder was configured with `with_sub_agent_traces(true)`. Hosts
    /// that don't recognise the `sub_agent_event` `type` MUST drop it
    /// silently per the W0 host decoder contract.
    SubAgentEvent {
        parent_call_id: String,
        agent_name: String,
        inner: Value,
    },
    /// ForgeFlows-Live: a workflow (ForgeFlows / Dynamic Workflows) run
    /// started. Emitted once, before the first node dispatches, so hosts
    /// (the TUI Workflows tab and the external `wayland` desktop app) get a
    /// clean lifecycle signal instead of inferring the run from the first
    /// `workflow:<node_id>`-prefixed `SubAgentEvent`. `workflow_id` is a
    /// stable correlation handle for the run; `name` is the author's display
    /// name; `node_count` is the number of agent nodes the run will dispatch.
    /// Rides the existing W0-reserved `capabilities.sub_agent_traces` flag
    /// (the same observability surface as `SubAgentEvent`) — no dedicated
    /// capability is added. Hosts that don't recognise the `workflow_started`
    /// `type` MUST drop it silently per the W0 host decoder contract.
    WorkflowStarted {
        workflow_id: String,
        name: String,
        node_count: usize,
    },
    /// ForgeFlows-Live: a workflow run finished. Emitted once, after the run
    /// completes (success or failure), as the terminal bookend to
    /// `WorkflowStarted`. `succeeded` is `true` only when the run produced
    /// no errored stages. Rides the existing `capabilities.sub_agent_traces`
    /// flag (no dedicated capability). Hosts that don't recognise the
    /// `workflow_finished` `type` MUST drop it silently per the W0 host
    /// decoder contract.
    WorkflowFinished {
        workflow_id: String,
        succeeded: bool,
    },
    /// W7: F4 streaming tool-result chunk. Long-running tools (e.g.
    /// `Bash` on a multi-minute build) emit one of these per chunk of
    /// stdout/stderr while running, ahead of the final `ToolResult`.
    /// Gated by the W0-reserved `capabilities.streaming_tools` flag
    /// (`ProtocolSink::with_streaming_tools(true)`). Hosts that don't
    /// recognise `tool_chunk` MUST drop it silently; the existing
    /// `ToolResult` still arrives at the end carrying the full
    /// buffered output for buffered hosts.
    ToolChunk {
        msg_id: String,
        call_id: String,
        tool_name: String,
        chunk: String,
    },
    /// W7: F8 provider circuit-breaker state transition. Emitted when
    /// `ResilientProvider` transitions between Closed / Open / HalfOpen,
    /// or when a fallback provider is engaged. NOT gated by an opt-in
    /// flag — circuit transitions are always-visible diagnostics, like
    /// `Error`. (Documented under "errors are always allowed" in
    /// `docs/json-stream-protocol.md` Host Decoder Contract section.)
    ///
    /// **Design rationale (rev-2, audit F4):** The W0 capability pattern
    /// is host-advertisement of decoder capability, NOT host-opt-in of
    /// emission. Errors today are always emitted because hosts that
    /// don't know `error` still drop the line silently per the W0
    /// forward-compat baseline. Same logic applies here:
    /// `provider_circuit_event` is a failure-mode diagnostic — opting
    /// in for it would mean a buggy host renders no fallback indication
    /// for an entire incident. The always-on choice is consistent with
    /// W0 (cross-audit approved 2026-05-15).
    ProviderCircuitEvent {
        primary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        fallback: Option<String>,
        /// "closed" | "open" | "half_open"
        state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// W7: S4 approval requested — engine wants the host's permission
    /// before proceeding with `call_id`. `resume_token` echoes back in
    /// the host's `ApprovalResume` command. Gated by the W0-reserved
    /// `capabilities.hitl_suspend` flag.
    ///
    /// **Wave SC SECURITY MAJOR (correlation-id model).** The
    /// `correlation_id` field is the opaque public handle the host UI
    /// uses to match this `ApprovalRequired` against the eventual
    /// `ApprovalResume`. `resume_token` carries the same opaque value
    /// (kept for backwards-compat with existing hosts; new hosts
    /// should prefer `correlation_id`). The actual bridge-side secret
    /// never appears on the wire — `ProtocolSink::redact_tokens`
    /// strips matching strings from streaming tool output as
    /// defense-in-depth against tools that snoop stdout.
    ApprovalRequired {
        call_id: String,
        resume_token: String,
        /// Wave SC opaque handle for UI matching. Same value as
        /// `resume_token` in this revision; future revisions may
        /// diverge the two.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        correlation_id: String,
        reason: String,
        context: String,
    },
    /// W7: S4 session is in Suspended state — emitted alongside
    /// ApprovalRequired so hosts that render a state pill can update
    /// independently of the modal flow.
    Suspend {
        reason: String,
        resume_token: String,
    },
    /// W7: S4 approval resolved — engine echoes the resume decision
    /// back so the host can clear UI state regardless of who emitted
    /// the corresponding command (CLI, UI, plugin).
    ApprovalResume {
        resume_token: String,
        approved: bool,
    },
    /// W8a A.7: ExecutionBudget cap exceeded — singular event per
    /// session, fires once when the first cap trips. Always-emitted +
    /// host-tolerated additive variant per audit F5: hosts that don't
    /// know the `budget_exceeded` type drop the line silently per the
    /// W0 host decoder contract, so no dedicated capability flag is
    /// reserved. `reason` is one of the deterministic
    /// `ExecutionBudgetView::first_exceeded_reason()` strings
    /// (`max_wall_time`, `max_tool_runtime`, `max_processes`,
    /// `max_agent_depth`, `max_tokens_in`, `max_tokens_out`,
    /// `max_cost_usd`); `observed` and `limit` are human-readable
    /// formatted strings (e.g. `"62.0s"` / `"60.0s"`, `"16384"` / `"4096"`).
    BudgetExceeded {
        reason: String,
        observed: String,
        limit: String,
    },
    /// Wave RB RELIABILITY MAJOR: a tool's `execute_with_ctx` panicked.
    /// The orchestration dispatcher caught the panic via
    /// `tokio::task::JoinError::is_panic()` and converted it to a
    /// structured `ToolResult { is_error: true, content: "Tool panicked: ..." }`
    /// so the LLM context sees a normal tool failure and the session
    /// continues. This event is emitted ALONGSIDE the synthetic
    /// `ToolResult` so a host can render the panic as a distinct
    /// diagnostic (vs. a normal `is_error: true` ToolResult).
    ///
    /// Always-on (no capability flag) — same rationale as `Error`,
    /// `BudgetExceeded`, and `ProviderCircuitEvent`: panic-recovery
    /// diagnostics are always-visible per the audit F4 W0 design.
    /// Hosts that don't recognise `tool_panicked` MUST drop the line
    /// silently per the W0 host decoder contract.
    ToolPanicked {
        msg_id: String,
        call_id: String,
        tool_name: String,
        /// Best-effort panic message extracted from the `JoinError`'s
        /// payload (downcast to `&str` / `String`). May be `"<non-string panic payload>"`
        /// if the panic used a non-string payload type.
        panic_message: String,
    },
    /// Wave RB STABILITY MINOR #10: a plugin failed to register with the
    /// host because one of its `Scoped*Registry::new(...)` calls returned
    /// an error other than the expected `AccessDenied` "permission not
    /// requested" sentinel. The plugin still loads — partial registration
    /// is allowed — but the host can render a diagnostic so the user
    /// understands why a tool/hook/etc. they expected is missing.
    ///
    /// Always-on (no capability flag) — same rationale as `Error` and
    /// `ProviderCircuitEvent`: plugin-registration failures are failure
    /// diagnostics. Hosts that don't recognise `plugin_registration_failed`
    /// drop the line silently per the W0 host decoder contract.
    PluginRegistrationFailed {
        plugin_name: String,
        /// Which scoped registry failed (e.g. `"tools"`, `"hooks"`,
        /// `"agents"`, `"skills"`, `"rules"`, `"mcp"`, `"providers"`).
        surface: String,
        /// The `PluginError` rendered via Display.
        error_kind: String,
        message: String,
    },
    /// W8a H.1: opaque event emitted by a registered plugin. Gated by
    /// the W0-reserved `capabilities.plugins` flag — the engine only
    /// emits this variant in sessions that advertise plugins=true (set
    /// in `build_capabilities` once at least one plugin is loaded).
    /// `plugin_name` matches the plugin's manifest name; `event_type`
    /// is plugin-defined free-form (e.g. `"memory_capture"`,
    /// `"index_rebuild_complete"`); `payload` is the plugin-supplied
    /// JSON value. Hosts that don't recognise the variant drop it
    /// silently per the W0 forward-compat baseline.
    PluginEvent {
        plugin_name: String,
        event_type: String,
        payload: Value,
    },
    /// W10B: F12 GEPA evolution event. Emitted at every scored child when the
    /// host has the `gepa_enabled` capability advertised. Older hosts that
    /// don't know this variant drop it silently per the W0 host decoder
    /// contract.
    ///
    /// `evolution_event` rides on its own dedicated capability flag rather
    /// than overloading `structured_traces` — see F6 audit fix in W10B
    /// rev-2: hosts that want W1 turn traces shouldn't be forced to also
    /// accept thousands of W10B events per `evolve` run.
    EvolutionEvent {
        run_id: String,
        generation: u32,
        parent_id: String,
        child_id: String,
        mutation_kind: String,
        score: f64,
        retained: bool,
    },
    /// W8c.1 E.14: browser-suite op event. Emitted by the engine once per
    /// completed browser op (Navigate, Snapshot, Click, ...) so the host
    /// can render a compact tool-call trail. Gated by the W0-reserved
    /// `capabilities.browser_suite` flag — engine advertises the flag
    /// when the wayland-browser plugin is loaded. Hosts that don't
    /// recognise `browser_event` MUST drop it silently per the W0 host
    /// decoder contract.
    BrowserEvent {
        msg_id: String,
        call_id: String,
        /// Op kind as serialized by `BrowserOp` (e.g. `"navigate"`).
        op: String,
        /// Origin / target URL when relevant (`Navigate`, `NewTab`,
        /// `Download`). `None` for ops without a URL (`Snapshot`, `Click`).
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// One-line human-readable summary (e.g. `"loaded"`,
        /// `"clicked @e3 button \"Submit\""`).
        summary: String,
    },
    /// W8c.1 E.14: a browser op was blocked by `BrowserPolicy` before
    /// dispatch — the host renders an explicit block notification so the
    /// user can react. Always emitted alongside the corresponding error
    /// `ToolResult`; the dedicated variant gives hosts a typed surface
    /// for blocked-URL telemetry. Gated by `capabilities.browser_suite`.
    BrowserPolicyDenied {
        msg_id: String,
        url: String,
        reason: String,
    },
    /// W8c.2 F.9: computer-use op event. Emitted by the engine once per
    /// completed CUA op (LeftClick, Type, Screenshot, ...) so the host
    /// can render a compact action trail. Gated by the W0-reserved
    /// `capabilities.computer_use` flag — engine advertises the flag
    /// when the wayland-cua plugin is loaded. Hosts that don't
    /// recognise `cua_event` MUST drop it silently per the W0 host
    /// decoder contract.
    CuaEvent {
        msg_id: String,
        call_id: String,
        /// Op kind as serialized by `CuaOp` (e.g. `"left_click"`).
        op: String,
        /// `[x, y]` screen coords for ops that have them (mouse/key);
        /// `None` for `Screenshot`, `AxTree`, `Wait`, `FrontmostApp`.
        #[serde(skip_serializing_if = "Option::is_none")]
        coords: Option<[i32; 2]>,
        /// One-line human-readable summary (e.g. `"clicked at (100, 200)"`,
        /// `"typed 14 chars"`).
        summary: String,
    },
    /// W8c.2 F.9: a CUA op was blocked by `CuaPolicy` before dispatch.
    /// Mirrors `BrowserPolicyDenied` — surfaces a typed channel so the
    /// host can render policy violations as a distinct notification
    /// kind. Gated by `capabilities.computer_use`.
    CuaPolicyDenied {
        msg_id: String,
        /// The op kind tag that was rejected.
        op: String,
        /// Frontmost-app id at the time of dispatch (best-effort; may
        /// be empty if the backend can't determine it).
        #[serde(default, skip_serializing_if = "String::is_empty")]
        app: String,
        reason: String,
    },
    /// W5 M6 / #279(d) + #280: a context compaction occurred. Gated by
    /// capabilities.non_destructive_compact; hosts that don't recognise
    /// compact_offload MUST drop it per the host decoder contract.
    CompactOffload {
        msg_id: String,
        /// Why compaction fired (e.g. "window_pressure", "manual").
        reason: String,
        /// Tokens reclaimed. 0 when not measurable.
        tokens_freed: u64,
        /// Active-window fill AFTER compaction, same opaque 0..=100 u32 as
        /// Usage.active_window_percent (from ContextWindow::percent()).
        #[serde(skip_serializing_if = "Option::is_none")]
        active_window_percent: Option<u32>,
    },
    Pong,
}

/// W6 F7 per-turn cost row carried by [`ProtocolEvent::SessionCost`].
/// `provider` is the structured per-provider id from `ProviderCompat.provider_type()`
/// (e.g. `"anthropic"`, `"bedrock"`, `"openai"`, `"vertex"`, `"ollama"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCost {
    pub turn: usize,
    pub model: String,
    pub provider: String,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    // v0.1.21 baseline (shape unchanged)
    pub tool_approval: bool,
    pub thinking: bool,
    pub effort: bool,
    pub effort_levels: Vec<String>,
    /// The advertised approval modes. These MUST be the canonical wire
    /// spellings — `default` / `auto_edit` / `force` — i.e. the
    /// `#[serde(rename_all = "snake_case")]` forms of
    /// `crate::commands::SessionMode` and exactly what
    /// `ToolApprovalManager::current_mode()` emits. A host parses these back
    /// through `SessionMode`, so advertising a non-canonical spelling here
    /// (e.g. kebab `auto-edit`) re-opens the D033 round-trip downgrade.
    pub modes: Vec<String>,
    /// The active mode, in the same canonical spelling as `modes` above.
    pub current_mode: String,
    pub mcp: bool,

    // W0 — forward-additive opt-in flags. All default-false; `skip_serializing_if`
    // keeps the JSON output byte-identical to v0.1.21 when these are off.
    //
    // Setting a flag to `true` is ENGINE ADVERTISEMENT, not host opt-in: the
    // engine is signalling "I will emit the corresponding new event variants
    // this session." The host's obligation is to tolerate unknown event types
    // and unknown fields per `docs/json-stream-protocol.md` host-decoder
    // contract — emission gating in future waves is governed by
    // `wcore-config`, not by what the host has acknowledged.
    /// W7: F4 streaming tool-result chunks (e.g. `tool_chunk` events).
    #[serde(default, skip_serializing_if = "is_false")]
    pub streaming_tools: bool,

    /// W7: F2 sub-agent events streamed via ChannelSink with `parent_call_id`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sub_agent_traces: bool,

    /// W6: F7 per-turn / per-session cost events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cost_attribution: bool,

    /// W7: S4 Suspend / ApprovalRequired turn-state events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hitl_suspend: bool,

    /// W5: M6 non-destructive compaction events (`compact_offload`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub non_destructive_compact: bool,

    /// W1: F9 structured `ExecutionTrace` / `TurnTrace` events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub structured_traces: bool,

    /// W4: F13 RPC tool script (`Script` tool) trace-expansion events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub rpc_tool_script: bool,

    /// W8: B1 browser tool family events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub browser_suite: bool,

    /// W8: B2 wcore-cua computer-use events.
    #[serde(default, skip_serializing_if = "is_false")]
    pub computer_use: bool,

    /// W2.5/W8: P1 plugin-registered tools/hooks/agents visible to the host.
    #[serde(default, skip_serializing_if = "is_false")]
    pub plugins: bool,

    /// W10B: F12 GEPA `evolution_event` emission. Forward-additive; default
    /// off. Setting this true advertises that the engine will emit
    /// `evolution_event` variants during a `wcore-cli evolve` run. Hosts
    /// that haven't learned about this flag drop the variant silently per
    /// the W0 host decoder contract.
    ///
    /// `structured_traces` (W1) is no longer overloaded with
    /// `evolution_event` — F6 audit fix in W10B rev-2 split the W1 turn-
    /// trace family from the W10B per-child evolution family so hosts can
    /// opt in independently.
    #[serde(default, skip_serializing_if = "is_false")]
    pub gepa_enabled: bool,

    /// F-093 — active user-model backend tag. `"local"` (on-disk JSON) or
    /// `"honcho"` (dialectic user modeling via Honcho server). Empty string
    /// when memory is disabled. Forward-additive: hosts that haven't seen
    /// this field yet ignore it per the W0 decoder contract.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub user_model_backend: String,

    /// F-092 (W7-N): live-session online evolution. Forward-additive;
    /// default off. When true the engine emits `evolution_event` at
    /// session-end for every real session (not just offline `evolve` runs)
    /// and applies the Paraphrase mutator live to successful trajectories.
    /// Opt-in via `--online-evolution` CLI flag or
    /// `[observability] online_evolution = true` in config.
    #[serde(default, skip_serializing_if = "is_false")]
    pub online_evolution: bool,

    /// Rank 85 — explicit memory-enabled signal. `user_model_backend` is an
    /// empty string both when memory is disabled and when an older host
    /// doesn't know the field, so it can't disambiguate the two. This flag
    /// is emitted (`true`) only when long-term memory is on, giving the host
    /// an unambiguous bool to key on instead of inferring from the backend
    /// tag. Forward-additive; omitted when false per the W0 decoder contract
    /// (absent reads as "off or unknown").
    #[serde(default, skip_serializing_if = "is_false")]
    pub memory_enabled: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            tool_approval: false,
            thinking: false,
            effort: false,
            effort_levels: Vec::new(),
            modes: vec!["default".to_string()],
            current_mode: "default".to_string(),
            mcp: false,
            streaming_tools: false,
            sub_agent_traces: false,
            cost_attribution: false,
            hitl_suspend: false,
            non_destructive_compact: false,
            structured_traces: false,
            rpc_tool_script: false,
            browser_suite: false,
            computer_use: false,
            plugins: false,
            gepa_enabled: false,
            user_model_backend: String::new(),
            online_evolution: false,
            memory_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub category: ToolCategory,
    pub args: Value,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    Info,
    Edit,
    Exec,
    Mcp,
}

impl std::fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Info => write!(f, "info"),
            Self::Edit => write!(f, "edit"),
            Self::Exec => write!(f, "exec"),
            Self::Mcp => write!(f, "mcp"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    Text,
    Diff,
    Image,
}

/// Token usage emitted with `stream_end`.
///
/// **Token accounting note (Task F, BD-audit Concern 2).** `output_tokens`
/// is reported verbatim from the underlying provider response (Anthropic
/// `usage.output_tokens`, OpenAI `usage.completion_tokens`, Bedrock
/// `usage.output_tokens` from the Anthropic-passthrough event stream).
///
/// Across the providers we ship, `output_tokens` reflects the **billable**
/// completion token count, which includes serialized tool-call arguments
/// and thinking tokens (where exposed). It does **not** equal the visible
/// text-delta byte count divided by ~4. App-side heuristics that compare
/// "characters streamed" against `output_tokens` will see large gaps on
/// tool-heavy turns; that is expected, not a bug. Prefer `finish_reason`
/// over content-length comparison for detecting truncation.
///
/// Empirical baseline landed in W12: `docs/tool-token-empirical-2026-05-15.md`.
/// Run `cargo run -p wcore-agent --bin tool_token_bench --features
/// test-utils` to regenerate the scripted-provider numbers; the same
/// doc's §2 runbook covers the live-API path that still needs real
/// provider credentials to fill in.
#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// #279(a): active-window fill 0..=100, sourced by the engine from
    /// wcore_config::context_window::ContextWindow::percent() on the
    /// POST-swap effective model. Opaque u32; wcore-protocol takes NO
    /// wcore-config dep. None (omitted) when the window is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_window_percent: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_ready_event_serialization() {
        let event = ProtocolEvent::Ready {
            version: "0.1.0".to_string(),
            session_id: Some("abc123".to_string()),
            capabilities: Capabilities {
                tool_approval: true,
                thinking: true,
                modes: vec!["default".into(), "auto_edit".into(), "force".into()],
                ..Default::default()
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "ready");
        assert_eq!(json["version"], "0.1.0");
        assert_eq!(json["session_id"], "abc123");
        assert_eq!(json["capabilities"]["tool_approval"], true);

        // session_id omitted when None
        let event_no_sid = ProtocolEvent::Ready {
            version: "0.1.0".to_string(),
            session_id: None,
            capabilities: Capabilities {
                tool_approval: true,
                thinking: true,
                modes: vec!["default".into(), "auto_edit".into(), "force".into()],
                ..Default::default()
            },
        };
        let json2 = serde_json::to_value(&event_no_sid).unwrap();
        assert!(json2.get("session_id").is_none());
    }

    #[test]
    fn test_text_delta_event_serialization() {
        let event = ProtocolEvent::TextDelta {
            text: "hello".to_string(),
            msg_id: "m1".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "text_delta");
        assert_eq!(json["text"], "hello");
        assert_eq!(json["msg_id"], "m1");
    }

    #[test]
    fn test_tool_request_event_serialization() {
        let event = ProtocolEvent::ToolRequest {
            msg_id: "m1".to_string(),
            call_id: "c1".to_string(),
            tool: ToolInfo {
                name: "Bash".to_string(),
                category: ToolCategory::Exec,
                args: json!({"command": "ls"}),
                description: "Execute: ls".to_string(),
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_request");
        assert_eq!(json["tool"]["category"], "exec");
    }

    #[test]
    fn test_tool_result_event_serialization() {
        let event = ProtocolEvent::ToolResult {
            msg_id: "m1".to_string(),
            call_id: "c1".to_string(),
            tool_name: "Read".to_string(),
            status: ToolStatus::Success,
            output: "file content".to_string(),
            output_type: OutputType::Text,
            metadata: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["status"], "success");
        assert!(json.get("metadata").is_none());
    }

    #[test]
    fn test_error_event_serialization() {
        let event = ProtocolEvent::Error {
            msg_id: None,
            error: ErrorInfo {
                code: "rate_limit".to_string(),
                message: "Too many requests".to_string(),
                retryable: true,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "error");
        assert!(json.get("msg_id").is_none());
        assert_eq!(json["error"]["retryable"], true);
    }

    #[test]
    fn test_stream_end_with_usage() {
        let event = ProtocolEvent::StreamEnd {
            msg_id: "m1".to_string(),
            finish_reason: FinishReason::Stop,
            usage: Some(Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: Some(20),
                cache_write_tokens: None,
                active_window_percent: None,
            }),
            agent_run_id: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "stream_end");
        assert_eq!(json["finish_reason"], "stop");
        assert_eq!(json["usage"]["input_tokens"], 100);
        assert!(json["usage"].get("cache_write_tokens").is_none());
    }

    #[test]
    fn test_stream_end_finish_reason_serialization() {
        // Required field: each variant should serialize to its snake_case name.
        for (variant, expected) in [
            (FinishReason::Stop, "stop"),
            (FinishReason::Length, "length"),
            (FinishReason::Error, "error"),
        ] {
            let event = ProtocolEvent::StreamEnd {
                msg_id: "m1".to_string(),
                finish_reason: variant,
                usage: None,
                agent_run_id: None,
            };
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(json["finish_reason"], expected, "variant {variant:?}");
        }
    }

    #[test]
    fn test_stream_end_finish_reason_required_in_output() {
        // Verify the field is always present in JSON, even when usage is None.
        let event = ProtocolEvent::StreamEnd {
            msg_id: "m1".to_string(),
            finish_reason: FinishReason::Length,
            usage: None,
            agent_run_id: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert!(
            json.get("finish_reason").is_some(),
            "finish_reason must be present on every stream_end event"
        );
    }

    #[test]
    fn test_tool_category_display() {
        assert_eq!(ToolCategory::Info.to_string(), "info");
        assert_eq!(ToolCategory::Edit.to_string(), "edit");
        assert_eq!(ToolCategory::Exec.to_string(), "exec");
        assert_eq!(ToolCategory::Mcp.to_string(), "mcp");
    }

    #[test]
    fn test_ready_event_with_expanded_capabilities() {
        let event = ProtocolEvent::Ready {
            version: "0.2.0".to_string(),
            session_id: Some("abc".to_string()),
            capabilities: Capabilities {
                tool_approval: true,
                thinking: true,
                effort: true,
                effort_levels: vec!["low".into(), "medium".into(), "high".into()],
                modes: vec!["default".into(), "auto_edit".into(), "force".into()],
                ..Default::default()
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["capabilities"]["thinking"], true);
        assert_eq!(json["capabilities"]["effort"], true);
        assert_eq!(json["capabilities"]["effort_levels"][0], "low");
        assert_eq!(json["capabilities"]["modes"][2], "force");
    }

    #[test]
    fn test_mcp_ready_event_serialization() {
        let event = ProtocolEvent::McpReady {
            name: "team-tools".to_string(),
            tools: vec!["team_send_message".into(), "team_task_create".into()],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "mcp_ready");
        assert_eq!(json["name"], "team-tools");
        assert_eq!(json["tools"][0], "team_send_message");
        assert_eq!(json["tools"][1], "team_task_create");
        assert_eq!(json["tools"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_pong_event_serialization() {
        let event = ProtocolEvent::Pong;
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "pong");
        assert_eq!(json.as_object().unwrap().len(), 1);
    }

    #[test]
    fn test_config_changed_event_serialization() {
        let event = ProtocolEvent::ConfigChanged {
            capabilities: Capabilities {
                tool_approval: true,
                thinking: false,
                effort: true,
                effort_levels: vec!["low".into(), "medium".into(), "high".into()],
                modes: vec!["default".into(), "auto_edit".into(), "force".into()],
                current_mode: "default".into(),
                mcp: true,
                ..Default::default()
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "config_changed");
        assert_eq!(json["capabilities"]["thinking"], false);
        assert_eq!(json["capabilities"]["effort"], true);
    }

    #[test]
    fn capabilities_default_has_all_w0_flags_false_and_v0_1_21_baseline() {
        let caps = Capabilities::default();

        // v0.1.21 baseline fields
        assert!(!caps.tool_approval);
        assert!(!caps.thinking);
        assert!(!caps.effort);
        assert!(caps.effort_levels.is_empty());
        assert_eq!(caps.modes, vec!["default".to_string()]);
        assert_eq!(caps.current_mode, "default");
        assert!(!caps.mcp);

        // W0 — new opt-in flags, all default false
        assert!(!caps.streaming_tools);
        assert!(!caps.sub_agent_traces);
        assert!(!caps.cost_attribution);
        assert!(!caps.hitl_suspend);
        assert!(!caps.non_destructive_compact);
        assert!(!caps.structured_traces);
        assert!(!caps.rpc_tool_script);
        assert!(!caps.browser_suite);
        assert!(!caps.computer_use);
        assert!(!caps.plugins);
        assert!(!caps.gepa_enabled);
    }

    #[test]
    fn capabilities_default_off_serializes_without_new_flag_keys() {
        // Critical: with all W0 flags off, the JSON output must be byte-identical
        // to the v0.1.21 shape so hosts that don't know about the new flags see
        // no change in the wire format.
        let event = ProtocolEvent::Ready {
            version: "0.1.21".into(),
            session_id: None,
            capabilities: Capabilities::default(),
        };
        let json = serde_json::to_value(&event).unwrap();
        let caps_obj = &json["capabilities"];

        // v0.1.21 keys present
        for k in [
            "tool_approval",
            "thinking",
            "effort",
            "effort_levels",
            "modes",
            "current_mode",
            "mcp",
        ] {
            assert!(caps_obj.get(k).is_some(), "v0.1.21 key {k} missing");
        }

        // W0 flags ABSENT when default-off (skip_serializing_if invariant)
        for k in [
            "streaming_tools",
            "sub_agent_traces",
            "cost_attribution",
            "hitl_suspend",
            "non_destructive_compact",
            "structured_traces",
            "rpc_tool_script",
            "browser_suite",
            "computer_use",
            "plugins",
            "gepa_enabled",
        ] {
            assert!(
                caps_obj.get(k).is_none(),
                "W0 flag {k} leaked into JSON when default-off"
            );
        }
    }

    #[test]
    fn capabilities_round_trips_through_deserialize() {
        // W0 audit Finding 3: Capabilities now derives Deserialize so future
        // host-side parsing or test fixtures can read it back. Each W0 flag
        // is annotated `#[serde(default)]` so default-off serializations
        // (which omit the key entirely via skip_serializing_if) deserialize
        // back to `false` cleanly.
        let original = Capabilities {
            tool_approval: true,
            thinking: true,
            effort: true,
            effort_levels: vec!["low".into(), "high".into()],
            modes: vec!["default".into(), "force".into()],
            current_mode: "force".into(),
            mcp: true,
            browser_suite: true,
            ..Default::default()
        };
        let serialized = serde_json::to_string(&original).unwrap();
        let parsed: Capabilities = serde_json::from_str(&serialized).unwrap();
        // Spot-check round-trip preserves both v0.1.21 and W0 fields.
        assert!(parsed.tool_approval);
        assert_eq!(
            parsed.modes,
            vec!["default".to_string(), "force".to_string()]
        );
        assert!(parsed.browser_suite);
        // Default-off W0 flags round-trip as false.
        assert!(!parsed.plugins);
        assert!(!parsed.computer_use);
    }

    #[test]
    fn capabilities_default_off_serialization_deserializes_with_w0_flags_false() {
        // The skip_serializing_if invariant means a default-off Capabilities
        // serializes WITHOUT any W0 flag keys. Deserializing that JSON back
        // must yield default-off (all W0 flags = false) thanks to serde(default).
        let serialized = serde_json::to_string(&Capabilities::default()).unwrap();
        let parsed: Capabilities = serde_json::from_str(&serialized).unwrap();
        assert!(!parsed.streaming_tools);
        assert!(!parsed.sub_agent_traces);
        assert!(!parsed.cost_attribution);
        assert!(!parsed.hitl_suspend);
        assert!(!parsed.non_destructive_compact);
        assert!(!parsed.structured_traces);
        assert!(!parsed.rpc_tool_script);
        assert!(!parsed.browser_suite);
        assert!(!parsed.computer_use);
        assert!(!parsed.plugins);
        assert!(!parsed.gepa_enabled);
        assert!(!parsed.memory_enabled);
    }

    #[test]
    fn capabilities_memory_enabled_emits_when_on_and_omits_when_off() {
        // Rank 85: memory_enabled gives the host an explicit bool to key on,
        // disambiguating "memory disabled" from "field unknown". On → present
        // and true; off → omitted (skip_serializing_if) but decodes to false.
        let on = Capabilities {
            memory_enabled: true,
            ..Default::default()
        };
        let on_json = serde_json::to_string(&on).unwrap();
        assert!(
            on_json.contains("\"memory_enabled\":true"),
            "expected memory_enabled key when on: {on_json}"
        );
        assert!(
            serde_json::from_str::<Capabilities>(&on_json)
                .unwrap()
                .memory_enabled
        );

        let off_json = serde_json::to_string(&Capabilities::default()).unwrap();
        assert!(
            !off_json.contains("memory_enabled"),
            "memory_enabled must be omitted when off: {off_json}"
        );
        assert!(
            !serde_json::from_str::<Capabilities>(&off_json)
                .unwrap()
                .memory_enabled
        );
    }

    #[test]
    fn capabilities_flag_on_serializes_with_key_present() {
        let caps = Capabilities {
            browser_suite: true,
            ..Default::default()
        };
        let event = ProtocolEvent::Ready {
            version: "0.2.0".into(),
            session_id: None,
            capabilities: caps,
        };
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["capabilities"]["browser_suite"], true);
        assert!(json["capabilities"].get("computer_use").is_none());
    }
}
