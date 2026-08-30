use serde::{Deserialize, Serialize};

/// The historical 200,000-token assumption.
///
/// Retained for ONE job: the ceiling a Flux router tier alias floor is clamped
/// against in [`CompactConfig::known_context_window`], where it means "a guess
/// about a pool may never RAISE the boundary above what we used to assume".
///
/// It is no longer the unknown-model fallback — see
/// [`UNVERIFIED_CONTEXT_WINDOW`] for why 200,000 was the wrong value for that
/// job.
pub const DEFAULT_CONTEXT_WINDOW: usize = 200_000;

/// Context window ASSUMED for compaction boundaries when the operator
/// configured none AND the active model is unknown to
/// [`crate::limits::model_output_ceiling`] (FerroxLabs/wayland#1150).
///
/// # Why this is not 200,000
///
/// The constant this replaces claimed in its own doc comment to be
/// "deliberately conservative" on the stated ground that "under-estimating the
/// window compacts early (annoying but recoverable), over-estimating it 400s
/// the provider and drops context (data loss)" — and then carried 200,000, the
/// TOP of the plausible range rather than the bottom. The reasoning was right
/// and the value contradicted it.
///
/// Measured consequence, from the #1150 report: a 32k local model served over
/// an OpenAI-compatible endpoint got microcompact at 83,500, autocompact at
/// 167,000 and an emergency stop at 197,000 — three boundaries the session can
/// never reach, because the endpoint truncates or 400s first. The reporter sat
/// at 83,208 input tokens with nothing firing. The default burned.
///
/// # Why 32,768
///
/// It is the bottom of the modern served range, and it is the same policy
/// [`crate::limits`] already applies to an unknown model's OUTPUT ceiling —
/// `UNKNOWN_CAP` is 8,192, not the largest ceiling in the table. That module's
/// header states the rule this constant now follows: "erring toward `None`/low
/// is safe (an undersize truncates, which is user-visible but recoverable); a
/// too-high entry would 400". The asymmetry is the whole argument: an
/// over-estimate fails SILENTLY (the endpoint drops the head of the
/// conversation and the model answers from a context it no longer has), while
/// an under-estimate fails LOUDLY (an extra `Autocompact: summarized …` line
/// the user can see, plus the unknown-window notice naming
/// `[compact] context_window`).
///
/// # The cost, stated plainly
///
/// A model that is unlisted because it is NEWER than the catalogue — a fresh
/// frontier release with a 1M window — now compacts at ~22.9k instead of 167k
/// until the release-time `model_output_ceiling` refresh catches up. That is a
/// real regression for that case, and it is the deliberate trade: it is
/// visible, recoverable in one config line, and bounded by a release process
/// that already exists, whereas the silent-truncation case is none of those.
pub const UNVERIFIED_CONTEXT_WINDOW: usize = 32_768;

/// FerroxLabs/wayland#1179 — the largest share of a context window the
/// ABSOLUTE reserve buffers may consume before they are scaled down.
///
/// # The problem this replaces
///
/// `output_reserve` + `autocompact_buffer` is 33,000 tokens by default, tuned
/// when the only window in play was 200,000. That is 16.5% of a 200k window,
/// 100.7% of a 32,768-token one, and 806% of the 4,096-token slot #1172
/// measured a stock Ollama actually serving. Subtracting an absolute figure
/// from a window an order of magnitude smaller does not produce a conservative
/// boundary, it produces a degenerate one: `input_ceiling()` saturates to zero
/// (the #255 pre-flight guard then fires on EVERY turn and aborts the run) and
/// the autocompact threshold saturates to zero, which on that path means
/// ALWAYS FIRE, not "no threshold".
///
/// #1150 patched the second of those with a 0.70-of-window replacement used
/// only when the subtraction had already collapsed. That closed the cliff and
/// left the slope: at 32,768 the threshold became 22,937 while the pre-flight
/// ceiling stayed at 32,768 − 23,000 = 9,768, so the guard shed and aborted
/// at 9,768 and autocompact — sitting 13,169 tokens ABOVE it — could never
/// fire at all. Two boundaries derived from the same window disagreed about
/// which came first.
///
/// # Why 0.55, and why it is not a picked fraction
///
/// Scaling ALL THREE reserves by one factor keeps them ordered by
/// construction, because the ordering only ever depended on their relative
/// sizes: `threshold = w − s(output_reserve + autocompact_buffer)` is below
/// `ceiling = w − s(output_reserve + emergency_buffer)` for every `s > 0`
/// exactly when `autocompact_buffer > emergency_buffer`, which is the
/// invariant the absolute figures already encoded. That is the part #1150's
/// notes correctly warned about: a proportional floor applied to the THRESHOLD
/// ALONE would have raised a pinned 60,000-token window's trigger from 27,000
/// to 42,000, past its own pre-flight shed ceiling of 37,000 — an inversion.
/// Scaling the ceiling with it cannot invert.
///
/// The fraction itself is then FORCED, not chosen. The scale engages below
/// `(output_reserve + autocompact_buffer) / MAX_RESERVE_FRACTION`, which at the
/// default reserves is `33,000 / 0.55 = 60,000`. 0.55 is the LARGEST fraction
/// that leaves that 60,000 window at a scale of exactly 1.0 — i.e. the largest
/// one for which the case #1150 named as the thing not to disturb is
/// byte-for-byte unchanged. Anything larger keeps eating small windows;
/// anything smaller pulls the crossover UP and starts retuning windows nobody
/// has evidence about. Every window at or above 60,000 — which is every model
/// in the [`crate::limits`] catalogue, the smallest being 128,000 — is
/// therefore untouched.
///
/// Measured consequences, at the five points #1179 asks for (default reserves,
/// `BASELINE_TURN_TOKENS` = 3,118):
///
/// | window | ceiling before → after | threshold before → after | verdict |
/// |---|---|---|---|
/// | 4,096 | 0 → 2,527 | 2,867 → 1,844 | still below the baseline turn: unusable |
/// | 8,192 | 0 → 5,053 | 5,734 → 3,688 | workable, 570 tokens of room |
/// | 32,768 | 9,768 → 20,208 | 22,937 → 14,747 | inversion fixed |
/// | 60,000 | 37,000 → 37,000 | 27,000 → 27,000 | unchanged |
/// | 200,000 | 177,000 → 177,000 | 167,000 → 167,000 | unchanged |
pub const MAX_RESERVE_FRACTION: f64 = 0.55;

/// Core's OWN baseline turn, in real prompt tokens — the system prompt plus
/// eight tool schemas, before the user has said anything.
///
/// MEASURED, not modelled: #1172 drove a real `qwen3:8b` through a logging
/// reverse proxy and read the figure off the endpoint's own `usage` block. It
/// is the floor under every boundary in this module, because a threshold below
/// it fires on an empty conversation and a ceiling below it aborts the run
/// before the user has typed anything. It is what makes
/// [`CompactConfig::supports_compaction`] answerable with a number instead of
/// a guess.
pub const BASELINE_TURN_TOKENS: usize = 3_118;

/// #1150's whole claim, enforced by the compiler rather than by a runtime
/// assertion that could never fail: the window assumed for a model we cannot
/// verify must be the CONSERVATIVE end of the range. Raising
/// `UNVERIFIED_CONTEXT_WINDOW` back to (or above) the historical 200,000 breaks
/// the build here rather than silently restoring the runaway.
const _: () = assert!(UNVERIFIED_CONTEXT_WINDOW < DEFAULT_CONTEXT_WINDOW);

/// The reserves must stay a MINORITY of the window they are taken out of. At
/// or above 1.0 the scaled reserves would consume the whole window and every
/// boundary would saturate to zero again — the exact cliff this replaces; at or
/// below 0 they would vanish and the pre-flight guard would never fire.
const _: () = assert!(MAX_RESERVE_FRACTION > 0.0 && MAX_RESERVE_FRACTION < 1.0);

/// The scale is only ever a REDUCTION, and it must not reorder the buffers:
/// `autocompact_buffer > emergency_buffer` is what puts the autocompact
/// threshold below the pre-flight ceiling, and one common factor preserves it.
const _: () = assert!(default_autocompact_buffer_const() > default_emergency_buffer_const());

/// Configuration for the multi-level context compaction system.
///
/// All token-related fields are in tokens (not bytes or characters).
/// The defaults are tuned for Claude models with a 200k context window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactConfig {
    /// Operator override of the context window, in tokens.
    ///
    /// `None` — the default — means **not configured**. The compaction
    /// boundaries then divide by the ACTIVE MODEL's real window from
    /// [`crate::limits::model_output_ceiling`], falling back to
    /// [`DEFAULT_CONTEXT_WINDOW`] when the registry does not know the model
    /// (GH#635). `Some(n)` is an explicit operator setting and always wins.
    /// See [`CompactConfig::effective_context_window`].
    ///
    /// **The `Option` IS the distinction.** serde has no "was this key
    /// present?" signal for a field carrying `#[serde(default = "…")]`, so
    /// before GH#635 "took the 200k default" and "was configured to exactly
    /// 200k" were literally the same state — and the boundaries had no way to
    /// prefer the model's real window without silently overriding an operator.
    /// The TOML/JSON key is unchanged (`context_window = 128000` still works);
    /// absence now deserializes to `None` instead of `200_000`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,

    /// Tokens reserved for output generation.
    /// Subtracted from `context_window` to get the effective input budget.
    #[serde(default = "default_output_reserve")]
    pub output_reserve: usize,

    /// Buffer below the effective window that triggers autocompact.
    /// `threshold = context_window - output_reserve - autocompact_buffer`
    #[serde(default = "default_autocompact_buffer")]
    pub autocompact_buffer: usize,

    /// Tokens from context_window limit to trigger emergency block.
    /// `emergency_limit = context_window - emergency_buffer`
    #[serde(default = "default_emergency_buffer")]
    pub emergency_buffer: usize,

    /// Max consecutive autocompact failures before the circuit breaker trips.
    #[serde(default = "default_max_failures")]
    pub max_failures: u32,

    /// Microcompact: keep the N most recent compactable tool results.
    #[serde(default = "default_micro_keep_recent")]
    pub micro_keep_recent: usize,

    /// Microcompact: gap threshold in seconds for time-based trigger.
    /// When the last assistant message is older than this — and real pressure
    /// has reached `micro_pressure_fraction` — microcompact fires.
    #[serde(default = "default_micro_gap_seconds")]
    pub micro_gap_seconds: u64,

    /// Microcompact: share of the autocompact threshold that REAL input
    /// pressure must reach before EITHER microcompact trigger may fire.
    ///
    /// Neither trigger is a pressure signal on its own. The count trigger
    /// ("more than `micro_keep_recent * 2` tool results exist") fires on the
    /// eleventh tool result of a session regardless of how empty the context
    /// window is. Corpus row A-6 died of exactly that: 25 microcompacts
    /// inside 60 turns freed ~2k tokens each while real pressure never
    /// exceeded ~10% of the window, and the agent spent every turn re-reading
    /// the thirteen files whose results had just been erased. It made no edit
    /// at all.
    ///
    /// The TIME trigger ("the last assistant message is older than
    /// `micro_gap_seconds`") reaches the same conversation by a different
    /// route: a resumed session loads yesterday's timestamps, so the first
    /// turn back from lunch wiped the working set at any occupancy. It is not
    /// a staleness signal either — the pass keeps the most RECENT results,
    /// which predate the gap exactly as much as the ones it clears.
    ///
    /// Gating both on the SAME watermark autocompact reads
    /// (`CompactState::last_real_input_tokens`) keeps microcompact as the
    /// cheap early relief valve — it still fires well before the summarizer —
    /// without erasing a working set the model has room to hold.
    ///
    /// `0.0` restores the old unconditional triggers. Clamped to
    /// `0.0..=1.0` at the use site.
    #[serde(default = "default_micro_pressure_fraction")]
    pub micro_pressure_fraction: f64,

    /// Tool names whose results are eligible for microcompact content clearing.
    #[serde(default = "default_compactable_tools")]
    pub compactable_tools: Vec<String>,

    /// Microcompact: byte size above which a tool result is eligible for
    /// clearing REGARDLESS of which tool produced it.
    ///
    /// `compactable_tools` is an allow-list of six built-in names, so an agent
    /// whose loop is delegation, web fetch, RepoMap or MCP calls has zero
    /// compactable results: the count trigger never fires and the clear pass
    /// has nothing it may touch, at any size and any pressure below hard
    /// overflow (issue #559 — a team leader re-billing 600 KB of stale
    /// delegated transcripts on every sub-call). The list cannot be extended
    /// to cover it either: MCP and plugin tool names are not knowable at build
    /// time.
    ///
    /// Size is the right property for the residue. A body this large, already
    /// past the `micro_keep_recent` protected tail and under at least
    /// `micro_pressure_fraction` of real pressure, is stale working data
    /// whatever produced it. Small results from unlisted tools (a todo list,
    /// an answered question) carry state and stay untouched, so this is not a
    /// licence to erase everything.
    ///
    /// This field also sizes the pass's *retained tail*, which is otherwise a
    /// pure count and so is blind to volume: the tail keeps at most
    /// `micro_keep_recent * micro_large_result_bytes` bytes of body (100 KB at
    /// the defaults), always retaining the newest result whatever its size.
    /// Without that bound three 500 KB delegated transcripts were all
    /// protected purely because there were fewer than `micro_keep_recent` of
    /// them, and the count trigger — which needs ELEVEN results at the default
    /// — could not see them either.
    ///
    /// `0` restores the old name-and-count-only behaviour.
    #[serde(default = "default_micro_large_result_bytes")]
    pub micro_large_result_bytes: usize,

    /// Whether the compaction system is enabled.
    /// When false, microcompact and autocompact are skipped
    /// (emergency truncation still applies).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Enable prompt cache diagnostics output to user.
    /// When true, cache hit/miss info is shown via OutputSink.
    /// Default: false.
    #[serde(default)]
    pub cache_diagnostics: bool,

    #[serde(default)]
    pub compaction: wcore_compact::CompactionLevel,

    #[serde(default)]
    pub toon: bool,

    /// Model id used for autocompact summarization.
    ///
    /// Summarization is a cheap-model task; running it on the live premium
    /// model costs ~15-20x more than necessary. When set, the autocompact
    /// LLM request targets this model instead of the live conversation model.
    /// The id is a plain provider-served model string (no provider assumed).
    ///
    /// Default: `None` — use the live model, preserving prior behavior.
    #[serde(default)]
    pub compaction_model: Option<String>,

    // --- #280 smart auto-compaction (default-OFF; soak before enabling) ---
    /// MASTER GATE for #280 smart auto-compaction. When false (the default),
    /// NOTHING in the smart path runs: the proactive pre-gate early-returns and
    /// `run_compaction` behaves byte-for-byte as before this feature landed.
    /// This is the default-OFF guarantee — flip to true only after a soak.
    #[serde(default)]
    pub smart_enabled: bool,

    /// High-water active-window share that ARMS a proactive compact (#280).
    /// Spec band 0.60–0.70; clamped to that band at the use site so an
    /// out-of-band TOML value is corrected rather than firing at 1% or never.
    #[serde(default = "default_smart_trigger_fraction")]
    pub smart_trigger_fraction: f64,

    /// Hysteresis low-water (#280). After a smart fire the trigger DISARMS and
    /// re-arms only once a later turn's fraction drops below this. Forced to
    /// `min(trigger - 0.05)` at the use site so it can never collapse hysteresis.
    #[serde(default = "default_smart_release_fraction")]
    pub smart_release_fraction: f64,

    /// Minimum completed turns between two smart fires (#280). Belt-and-
    /// suspenders for the post-stream watermark refresh lag.
    #[serde(default = "default_smart_cooldown_turns")]
    pub smart_cooldown_turns: u32,

    /// Cannot-shrink terminal latch (#280): if a smart-triggered compact frees
    /// fewer than this many tokens, smart compaction latches OFF for the rest
    /// of the session (guards against "frees ~nothing, fire forever").
    #[serde(default = "default_smart_min_shrink_tokens")]
    pub smart_min_shrink_tokens: u64,

    /// Write the non-destructive handoff Episode to long-term memory on a smart
    /// fire (#280). Default true, but only reachable when `smart_enabled`. Lets
    /// the memory write be soaked/disabled independently for NullMemory hosts.
    #[serde(default = "default_true")]
    pub smart_handoff_to_memory: bool,

    /// Continuous compaction of HISTORICAL assistant tool-call arguments
    /// (parity gap 2): large `tool_calls[].function.arguments` payloads (e.g.
    /// Write file bodies) older than the last N assistant turns are replaced
    /// with a compact stub. TOML table: `[compact.tool_call_args]`.
    #[serde(default)]
    pub tool_call_args: ToolCallArgsConfig,

    /// Ceiling on the TOTAL size of accumulated tool RESULT bodies carried in
    /// history (FerroxLabs/wayland#1150 c4). TOML table:
    /// `[compact.tool_results]`.
    #[serde(default)]
    pub tool_results: ToolResultsConfig,
}

/// Config for the accumulated tool-RESULT ceiling (wayland#1150 c4).
///
/// Per-result truncation already exists: every tool declares
/// `Tool::max_result_size()` (50,000 chars by default) and the orchestration
/// layer truncates at it before the result enters history. What did not exist
/// is a bound on the SUM. Twenty results at the per-result cap is a megabyte
/// of history re-sent whole on every turn, and the recency pass that clears
/// old results (`micro_keep_recent`) is gated on context pressure, so nothing
/// touches them until the window is nearly full.
///
/// This pass is therefore UNGATED — it runs on every compaction pipeline pass
/// like the tool-call-argument pass, and it applies to every tool rather than
/// only `compactable_tools`: a ceiling a tool can opt out of is not a ceiling.
///
/// **The guarantee**: carried tool-result bytes never exceed
/// `total_budget_bytes` plus the `keep_recent` newest results. Both terms are
/// constants, so the carried size stops growing with session length — which
/// is the property #1150 is about, not any particular number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultsConfig {
    /// Master gate for the pass. Default ON.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Total budget, in bytes of tool-result body text, for the whole
    /// conversation. Once the sum exceeds this, the OLDEST results are
    /// replaced with a stub until it fits again.
    #[serde(default = "default_tr_total_budget_bytes")]
    pub total_budget_bytes: usize,

    /// Newest tool results that are never bounded, however large — the
    /// model's live working set. Counted over tool results, not turns.
    #[serde(default = "default_tr_keep_recent")]
    pub keep_recent: usize,

    /// Epoch quantization of the stub boundary, exactly as
    /// [`ToolCallArgsConfig::epoch_turns`]: the boundary advances in batches
    /// of this many results, so between ticks the pass changes ZERO bytes and
    /// the provider's contiguous prefix cache holds end-to-end. `1` = advance
    /// as tightly as the budget requires. Floored to 1 at the use site.
    #[serde(default = "default_tr_epoch_results")]
    pub epoch_results: usize,
}

impl Default for ToolResultsConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            total_budget_bytes: default_tr_total_budget_bytes(),
            keep_recent: default_tr_keep_recent(),
            epoch_results: default_tr_epoch_results(),
        }
    }
}

/// Config for continuous tool-call-argument compaction (parity gap 2).
///
/// Unlike the tool-RESULT micro-compaction above (trigger-gated), this pass
/// runs on every compaction pipeline pass: an old Write body stops riding in
/// resent history at the first epoch tick after it leaves the protected tail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallArgsConfig {
    /// Master gate for the pass. Default ON.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Assistant turns whose tool-call arguments stay verbatim, counted from
    /// the end of history — the model may still reference recent args.
    /// Floored to 1 at the use site.
    #[serde(default = "default_tca_keep_recent_turns")]
    pub keep_recent_turns: usize,

    /// Minimum serialized size (bytes) of an argument object before it is
    /// stubbed. Tiny args (Read paths, short Bash commands) are never touched.
    #[serde(default = "default_tca_min_args_bytes")]
    pub min_args_bytes: usize,

    /// Epoch quantization of the stub boundary (cache economics): the
    /// boundary advances only every `epoch_turns` assistant turns, stubbing a
    /// batch at once, instead of flipping one message per turn inside the
    /// provider's cached prefix (which would re-bill the byte-identical
    /// protected tail at full price every turn). Between ticks the boundary
    /// is frozen and the whole prefix stays cache-hittable. `1` = advance
    /// every turn (no quantization). Floored to 1 at the use site.
    #[serde(default = "default_tca_epoch_turns")]
    pub epoch_turns: usize,
}

impl Default for ToolCallArgsConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            keep_recent_turns: default_tca_keep_recent_turns(),
            min_args_bytes: default_tca_min_args_bytes(),
            epoch_turns: default_tca_epoch_turns(),
        }
    }
}

impl CompactConfig {
    /// The active model's context window in tokens, or `None` when it is
    /// genuinely unknown (FerroxLabs/wayland#1150).
    ///
    /// Same precedence as [`Self::effective_context_window`] — operator
    /// setting, then registry, then Flux tier floor — but it stops there
    /// instead of substituting [`DEFAULT_CONTEXT_WINDOW`]. Every caller that
    /// must not act on a guess uses this one: the
    /// [`crate::context_window::ContextWindow`] kernel (via
    /// [`Self::kernel_config_window`]), the skills prompt budget, and the
    /// status-bar gauge.
    pub fn known_context_window(&self, provider: &str, model: &str) -> Option<usize> {
        if let Some(configured) = self.context_window {
            return Some(configured);
        }
        if let Some((_out_ceiling, window)) = crate::limits::model_output_ceiling(provider, model) {
            return Some(window as usize);
        }
        if let Some(floor) = crate::limits::flux_tier_context_window(model) {
            // Router alias: conservative floor only, never a raise.
            return Some((floor as usize).min(DEFAULT_CONTEXT_WINDOW));
        }
        None
    }

    /// The `config_window` argument for
    /// [`crate::context_window::ContextWindow::resolve`]: the operator's
    /// EXPLICIT `[compact] context_window`, or `0` meaning "there is no
    /// fallback — do not fabricate one".
    ///
    /// This exists because the predecessor (`fallback_context_window`, deleted
    /// with #1150) folded [`DEFAULT_CONTEXT_WINDOW`] into that argument, so
    /// the kernel's documented "unknown model ⇒ `None`, no fabricated
    /// denominator" arm was unreachable from production: `resolve` returned
    /// `Some(200_000)` for every unlisted model. A user on a 32k local model
    /// got a `% full` gauge divided by 200,000 and a pre-flight shed ceiling of
    /// 177,000 tokens — a guard that could never fire before the provider 400d.
    /// Returning `0` here is what lets `resolve` say "I do not know", which is
    /// the truth and which every downstream consumer already handles by failing
    /// open.
    pub fn kernel_config_window(&self) -> u64 {
        self.context_window.unwrap_or(0) as u64
    }

    /// **THE single definition of the divisor every compaction boundary uses**
    /// (GH#635).
    ///
    /// Before this existed, `autocompact_threshold` and `emergency_limit` both
    /// divided by the raw `context_window` field, which nothing ever synced
    /// from the model's real window: a 1.05M-window model still autocompacted
    /// at ~177k and emergency-stopped at ~197k — a 5x premature compaction,
    /// while `size_output_cap` and the #255 pre-flight guard were meanwhile
    /// using the model's REAL window. This function is what brings the
    /// boundaries into line with those two.
    ///
    /// Precedence, in order:
    ///
    /// 1. **An explicit operator setting wins.** `Some(n)` is a deliberate
    ///    instruction (commonly "cap me below what this model allows"), and
    ///    silently replacing it with a registry number would be a lie.
    /// 2. **A registry-KNOWN model's real window.** This is the only source
    ///    allowed to RAISE the boundary, because it is verified data rather
    ///    than a guess (see the header of [`crate::limits`]).
    /// 3. **A Flux router tier alias's conservative pool-minimum floor.** A
    ///    tier alias is *not* a known model, so it may only ever LOWER the
    ///    boundary — `.min(DEFAULT_CONTEXT_WINDOW)` makes that structural
    ///    rather than a property of today's 128k value. Matching the #255
    ///    kernel here is the point: the pre-flight guard already divides by
    ///    this floor, and leaving autocompact on a larger window is the
    ///    CORE-4 wedge where compaction never fires.
    /// 4. **[`UNVERIFIED_CONTEXT_WINDOW`].** An unknown, unlisted or otherwise
    ///    unroutable model is assumed to be at the BOTTOM of the served range,
    ///    not the top (#1150). 200,000 here is what let a 32k model grow to
    ///    83,208 input tokens with every boundary out of reach.
    ///
    /// Step 4 is a declared FALLBACK, not knowledge: this returns `usize` and
    /// promises no `None`, because its two consumers — the static autocompact
    /// threshold and the emergency hard stop — are plain integers on the
    /// cache-ledger/protocol surface. A caller that must not act on a guess
    /// uses [`Self::known_context_window`] instead, and the session tells the
    /// operator when this step is reached so they can set the real number.
    ///
    /// `provider` / `model` must be the POST-swap effective pair — the same
    /// values fed to `size_output_cap` and
    /// [`crate::context_window::ContextWindow::resolve`].
    pub fn effective_context_window(&self, provider: &str, model: &str) -> usize {
        self.known_context_window(provider, model)
            .unwrap_or(UNVERIFIED_CONTEXT_WINDOW)
    }

    /// #1179 — the reserve buffers this config applies AT `window`.
    ///
    /// Identity whenever `output_reserve + autocompact_buffer` already fits
    /// inside [`MAX_RESERVE_FRACTION`] of the window, which is every window at
    /// or above 60,000 with the default reserves. Below that all three are
    /// scaled by ONE common factor, so their ordering — and therefore the
    /// ordering of every boundary derived from them — is preserved.
    pub fn scaled_reserves(&self, window: usize) -> ScaledReserves {
        let nominal = self.output_reserve.saturating_add(self.autocompact_buffer);
        let budget = window as f64 * MAX_RESERVE_FRACTION;
        if nominal == 0 || nominal as f64 <= budget {
            return ScaledReserves {
                output_reserve: self.output_reserve,
                autocompact_buffer: self.autocompact_buffer,
                emergency_buffer: self.emergency_buffer,
            };
        }
        let scale = budget / nominal as f64;
        let apply = |v: usize| (v as f64 * scale) as usize;
        ScaledReserves {
            output_reserve: apply(self.output_reserve),
            autocompact_buffer: apply(self.autocompact_buffer),
            emergency_buffer: apply(self.emergency_buffer),
        }
    }

    /// The autocompact trigger for `window`:
    /// `window − scaled output_reserve − scaled autocompact_buffer`.
    ///
    /// THE definition. `wcore_agent::compact::auto::autocompact_threshold`
    /// resolves the window and delegates here, so a reporter and an enforcer
    /// cannot end up on different arithmetic.
    ///
    /// Cannot saturate to zero for any positive window: the scaled reserves are
    /// at most [`MAX_RESERVE_FRACTION`] of it, so this is at least
    /// `(1 − MAX_RESERVE_FRACTION) × window`. That is why #1150's
    /// `MIN_AUTOCOMPACT_WINDOW_FRACTION` replacement — which existed only for
    /// the saturated case — is gone rather than kept as an unreachable branch.
    pub fn autocompact_threshold_for_window(&self, window: usize) -> usize {
        let r = self.scaled_reserves(window);
        window
            .saturating_sub(r.output_reserve)
            .saturating_sub(r.autocompact_buffer)
    }

    /// The #255 pre-flight input ceiling for `window`:
    /// `window − scaled output_reserve − scaled emergency_buffer`.
    pub fn input_ceiling_for_window(&self, window: usize) -> usize {
        let r = self.scaled_reserves(window);
        window
            .saturating_sub(r.output_reserve)
            .saturating_sub(r.emergency_buffer)
    }

    /// The emergency hard stop for `window`: `window − scaled emergency_buffer`.
    pub fn emergency_limit_for_window(&self, window: usize) -> usize {
        window.saturating_sub(self.scaled_reserves(window).emergency_buffer)
    }

    /// FerroxLabs/wayland#1200 — the accumulated tool-result ceiling AT
    /// `window`, or today's flat constants when no window is known.
    ///
    /// [`ToolResultsConfig::total_budget_bytes`] and
    /// [`ToolResultsConfig::keep_recent`] are ABSOLUTE figures, and at their
    /// shipped defaults their worst case is 120,000 + 4 x
    /// [`MAX_TOOL_RESULT_BYTES`] = 320,000 bytes, about 80,000 tokens - roughly
    /// 2.4x the entire 32,768-token window this release assumes for an unlisted
    /// model. The pass's guarantee ("carried bytes stop growing with the
    /// session") was true; the guarantee a 32k user needs ("carried bytes fit
    /// the window") was neither claimed nor delivered.
    ///
    /// Two terms, so both are bounded here. Bounding only the budget leaves the
    /// protected tail dominating at a small window, which is why #1150 c4 left
    /// this open rather than half-closing it:
    ///
    /// 1. **The budget** is capped at half the bytes the pre-flight guard will
    ///    actually admit ([`Self::input_ceiling_for_window`] x
    ///    [`CHARS_PER_TOKEN`]). The ceiling, not the raw window, because
    ///    carried results are INPUT and the ceiling is the boundary that admits
    ///    input; a bound the guard aborts past is not a bound.
    /// 2. **The protected tail** keeps its COUNT and gains a BYTE cap, the
    ///    other half. Capping the count instead would drop the tail to one
    ///    result on any window under ~100k even when the results are small,
    ///    and a stubbed working set is how #1172's re-read loop starts. Capping
    ///    the bytes leaves a normal session's four results all protected and
    ///    bites only on the pathological one this ticket measured.
    ///
    /// The newest result is protected unconditionally at the use site, so the
    /// worst case carries one result at the ingestion cap however small the
    /// window is - see [`Self::worst_case_carried_tool_result_bytes`] and the
    /// named gap recorded with it.
    ///
    /// Identity above a ~103,000-token window, so no large-window sizing moves.
    pub fn tool_result_bounds(&self, window: Option<usize>) -> ToolResultBounds {
        let tr = &self.tool_results;
        let Some(window) = window else {
            return ToolResultBounds {
                total_budget_bytes: tr.total_budget_bytes,
                protected_tail_bytes: None,
                keep_recent: tr.keep_recent,
            };
        };
        let admissible = self
            .input_ceiling_for_window(window)
            .saturating_mul(CHARS_PER_TOKEN);
        let half = admissible / 2;
        ToolResultBounds {
            total_budget_bytes: tr.total_budget_bytes.min(half),
            protected_tail_bytes: Some(half),
            keep_recent: tr.keep_recent,
        }
    }

    /// #1200 c2 — the WORST-CASE bytes a bounded conversation can still carry
    /// in tool results, stated as the arithmetic the ticket stated it in:
    /// `total_budget_bytes + keep_recent x max_result_size`.
    ///
    /// The tail term is `min(cap, keep_recent x MAX_TOOL_RESULT_BYTES)` raised
    /// to at least [`MAX_TOOL_RESULT_BYTES`], because
    /// `bound_accumulated_tool_results` protects the newest result
    /// unconditionally. NAMED GAP: below a window whose admissible input is
    /// under [`MAX_TOOL_RESULT_BYTES`] (about 25,000 tokens of ceiling), that
    /// one result is the binding term and it is NOT window-derived - the
    /// per-result ingestion cap lives in `wcore_tools::Tool::max_result_size`
    /// and is a fixed 50,000 chars. Nothing here can close that.
    pub fn worst_case_carried_tool_result_bytes(&self, window: Option<usize>) -> usize {
        let b = self.tool_result_bounds(window);
        let tail = match b.protected_tail_bytes {
            Some(cap) => cap
                .min(b.keep_recent.saturating_mul(MAX_TOOL_RESULT_BYTES))
                .max(MAX_TOOL_RESULT_BYTES),
            None => b.keep_recent.saturating_mul(MAX_TOOL_RESULT_BYTES),
        };
        b.total_budget_bytes.saturating_add(tail)
    }

    /// #1179 — is `window` big enough for compaction to be worth pointing at?
    ///
    /// Both boundaries must clear core's own [`BASELINE_TURN_TOKENS`]. A
    /// threshold below it summarizes an empty conversation at the top of every
    /// turn; a ceiling below it aborts the run before the user has typed
    /// anything. Neither is a fix, and both are what a naively-applied learned
    /// window would have produced.
    ///
    /// At the 4,096-token slot #1172 measured it answers `false` — core's
    /// baseline turn alone is 76% of that window and no division of it leaves
    /// room to work. #1172 c3: `false` here does NOT mean the window is
    /// ignored. The learned window is still applied
    /// (`AgentEngine::narrow_to_served_window` has no escape hatch), and
    /// `AgentEngine::unworkable_window_refusal` stops the run out loud naming
    /// [`Self::minimum_workable_window`], rather than sizing the session
    /// against a window the endpoint was observed not to serve.
    pub fn supports_compaction(&self, window: usize) -> bool {
        self.autocompact_threshold_for_window(window) > BASELINE_TURN_TOKENS
            && self.input_ceiling_for_window(window) > BASELINE_TURN_TOKENS
    }

    /// #1179 — THE autocompact DECISION at `window`, refusal included.
    ///
    /// [`Self::autocompact_threshold_for_window`] stays a plain number because
    /// the cache ledger and the context gauge report it. This is the decision,
    /// and the decision carries [`Self::supports_compaction`] with it.
    ///
    /// # Why the gate lives here and not only where a learned window enters
    ///
    /// The refusal is a property of the WINDOW, not of the route the window
    /// arrived by. #1179 first shipped it inside
    /// `AgentEngine::narrow_to_served_window`, the single place #1172's LEARNED
    /// figure is admitted, which left the CONFIGURED path — the one #1150's own
    /// notice tells operators to use — running unguarded. A `[compact]
    /// context_window` of 6,000 (a local Ollama `num_ctx` of 6,144 lands in the
    /// same band) yields a threshold of 2,700 against a 3,118-token baseline
    /// turn: the trigger is already true before the user has typed anything,
    /// the summarizer cannot reclaim a system prompt or a tool schema, and the
    /// next turn asks again. That is an LLM call at the top of every turn,
    /// forever.
    ///
    /// Refusing is not compaction failing to fire. Below the crossover there is
    /// no trigger value that both clears the baseline turn and stays under the
    /// pre-flight ceiling, so firing is a loop and refusing is the only honest
    /// answer — the same answer the learned path already gives at 4,096. The
    /// emergency hard stop (`emergency_limit`) is untouched and still bounds the
    /// run, and the operator is told at bootstrap rather than left to infer it.
    pub fn should_autocompact_at(&self, window: usize, tokens: usize) -> bool {
        self.enabled
            && self.supports_compaction(window)
            && tokens >= self.autocompact_threshold_for_window(window)
    }

    /// #1172 — the smallest window this config CAN work in: the least `w` for
    /// which [`Self::supports_compaction`] holds.
    ///
    /// Derived from the reserves rather than hardcoded, so a non-default
    /// `[compact] output_reserve` moves it with them. It exists so the refusal
    /// core raises on an unworkable window can name the number the operator has
    /// to reach — "raise `num_ctx`" without a target leaves them to bisect it
    /// against a symptom that only appears several turns in.
    ///
    /// `supports_compaction` is monotone in `w`, so the bisection below is
    /// exact rather than a heuristic: below the [`MAX_RESERVE_FRACTION`]
    /// crossover both boundaries are fixed fractions of the window, above it
    /// they are the window minus a constant, and the two agree at the crossover
    /// by construction (that is what fixes the fraction at 0.55).
    pub fn minimum_workable_window(&self) -> usize {
        let mut hi = 1usize;
        while !self.supports_compaction(hi) {
            let Some(next) = hi.checked_mul(2) else {
                // Reserves so large no window satisfies them. The caller is
                // reporting a refusal either way; saturating is honest.
                return usize::MAX;
            };
            hi = next;
        }
        let mut lo = hi / 2;
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if self.supports_compaction(mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        hi
    }
}

/// The reserve buffers as they apply at one particular window.
///
/// A distinct type rather than a tuple so a caller cannot silently swap two of
/// three same-typed fields — the failure would be a boundary that is merely
/// wrong rather than one that does not compile.
/// FerroxLabs/wayland#1200 — the accumulated tool-result ceiling in force for
/// one window. See [`CompactConfig::tool_result_bounds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolResultBounds {
    /// Byte ceiling on the SUM of every tool result outside the protected tail.
    pub total_budget_bytes: usize,
    /// Byte ceiling on the protected tail itself, or `None` when no window is
    /// known and the tail is bounded by its COUNT alone (today's behaviour).
    pub protected_tail_bytes: Option<usize>,
    /// Newest tool results eligible for protection, as a count.
    pub keep_recent: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaledReserves {
    /// Tokens held back for the model's output.
    pub output_reserve: usize,
    /// Additional headroom below the input ceiling at which autocompact fires.
    pub autocompact_buffer: usize,
    /// The last-resort headroom below the window itself.
    pub emergency_buffer: usize,
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            context_window: None,
            output_reserve: default_output_reserve(),
            autocompact_buffer: default_autocompact_buffer(),
            emergency_buffer: default_emergency_buffer(),
            max_failures: default_max_failures(),
            micro_keep_recent: default_micro_keep_recent(),
            micro_gap_seconds: default_micro_gap_seconds(),
            micro_pressure_fraction: default_micro_pressure_fraction(),
            compactable_tools: default_compactable_tools(),
            micro_large_result_bytes: default_micro_large_result_bytes(),
            enabled: default_true(),
            cache_diagnostics: false,
            compaction: wcore_compact::CompactionLevel::default(),
            toon: false,
            compaction_model: None,
            smart_enabled: false,
            smart_trigger_fraction: default_smart_trigger_fraction(),
            smart_release_fraction: default_smart_release_fraction(),
            smart_cooldown_turns: default_smart_cooldown_turns(),
            smart_min_shrink_tokens: default_smart_min_shrink_tokens(),
            smart_handoff_to_memory: true,
            tool_call_args: ToolCallArgsConfig::default(),
            tool_results: ToolResultsConfig::default(),
        }
    }
}

// --- Default value functions ---

/// Bytes per token, at the char/4 heuristic every estimator in the workspace
/// uses. Owned HERE because wcore-config sits below every consumer:
/// `wcore_skills::prompt::CHARS_PER_TOKEN` re-exports it rather than restating
/// it, which is what stopped the skills budget drifting onto a different
/// window (FerroxLabs/wayland#1199).
pub const CHARS_PER_TOKEN: usize = 4;

/// Worst-case bytes a SINGLE tool result can carry: the per-result truncation
/// cap applied at ingestion. `wcore_tools::Tool::max_result_size` returns this,
/// so the ceiling arithmetic in [`CompactConfig::tool_result_bounds`] and the
/// cap it is sized against cannot drift apart (FerroxLabs/wayland#1200).
pub const MAX_TOOL_RESULT_BYTES: usize = 50_000;

fn default_output_reserve() -> usize {
    20_000
}
fn default_autocompact_buffer() -> usize {
    default_autocompact_buffer_const()
}
fn default_emergency_buffer() -> usize {
    default_emergency_buffer_const()
}
/// `const` mirrors, so the ordering the scaling relies on is checked by the
/// compiler rather than by a runtime assertion that could never fail.
const fn default_autocompact_buffer_const() -> usize {
    13_000
}
const fn default_emergency_buffer_const() -> usize {
    3_000
}
fn default_max_failures() -> u32 {
    3
}
fn default_micro_keep_recent() -> usize {
    5
}
fn default_micro_gap_seconds() -> u64 {
    3600
}
fn default_micro_pressure_fraction() -> f64 {
    0.5
}
fn default_compactable_tools() -> Vec<String> {
    vec![
        "Read".into(),
        "Bash".into(),
        "Grep".into(),
        "Glob".into(),
        "Write".into(),
        "Edit".into(),
    ]
}
/// ~5k tokens at the usual 4 bytes/token estimate.
fn default_micro_large_result_bytes() -> usize {
    20_000
}
fn default_true() -> bool {
    true
}
fn default_smart_trigger_fraction() -> f64 {
    0.65
}
fn default_smart_release_fraction() -> f64 {
    0.50
}
fn default_smart_cooldown_turns() -> u32 {
    2
}
fn default_smart_min_shrink_tokens() -> u64 {
    2_000
}
fn default_tca_keep_recent_turns() -> usize {
    2
}
fn default_tca_min_args_bytes() -> usize {
    768
}
fn default_tca_epoch_turns() -> usize {
    4
}
/// ~30k tokens of accumulated tool-result body at the ~4 bytes/token
/// heuristic. Chosen against the per-result cap it sits above: one result may
/// still be 50,000 bytes, so the ceiling holds at least two full-size results
/// plus the protected tail, and only bites once a session has genuinely
/// accumulated a working set larger than that.
fn default_tr_total_budget_bytes() -> usize {
    120_000
}
fn default_tr_keep_recent() -> usize {
    4
}
fn default_tr_epoch_results() -> usize {
    4
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1172 — the refusal has to name a number, and that number has to be
    /// exact: it is what the operator sets `num_ctx` to.
    #[test]
    fn the_minimum_workable_window_is_the_least_one_that_supports_compaction() {
        let cfg = CompactConfig::default();
        let min = cfg.minimum_workable_window();
        assert_eq!(min, 6_929, "0.45w > BASELINE_TURN_TOKENS at the defaults");
        assert!(cfg.supports_compaction(min));
        assert!(!cfg.supports_compaction(min - 1));
        // The slot #1172 measured, and the band #1179's own notes call out.
        assert!(!cfg.supports_compaction(4_096));
        assert!(!cfg.supports_compaction(6_000));
        assert!(cfg.supports_compaction(8_192));

        // Derived from the reserves, not hardcoded: halving them halves it.
        let lean = CompactConfig {
            output_reserve: 10_000,
            autocompact_buffer: 6_500,
            emergency_buffer: 1_500,
            ..CompactConfig::default()
        };
        let lean_min = lean.minimum_workable_window();
        assert!(lean.supports_compaction(lean_min));
        assert!(!lean.supports_compaction(lean_min - 1));
    }

    #[test]
    fn default_values_match_spec() {
        let cfg = CompactConfig::default();
        // GH#635: the default is "not configured", which is a DIFFERENT state
        // from "configured to 200k". #1150: and "not configured" reaches the
        // kernel as 0 — no fabricated denominator — not as 200k.
        assert_eq!(cfg.context_window, None);
        assert_eq!(cfg.kernel_config_window(), 0);
        assert_eq!(cfg.output_reserve, 20_000);
        assert_eq!(cfg.autocompact_buffer, 13_000);
        assert_eq!(cfg.emergency_buffer, 3_000);
        assert_eq!(cfg.max_failures, 3);
        assert_eq!(cfg.micro_keep_recent, 5);
        assert_eq!(cfg.micro_gap_seconds, 3600);
        assert!(cfg.enabled);
        assert_eq!(
            cfg.compactable_tools,
            vec!["Read", "Bash", "Grep", "Glob", "Write", "Edit"]
        );
    }

    #[test]
    fn toml_full_override() {
        let toml_str = r#"
context_window = 128000
output_reserve = 10000
autocompact_buffer = 8000
emergency_buffer = 2000
max_failures = 5
micro_keep_recent = 3
micro_gap_seconds = 1800
compactable_tools = ["Read", "Bash"]
enabled = false
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.context_window, Some(128_000));
        assert_eq!(cfg.output_reserve, 10_000);
        assert_eq!(cfg.autocompact_buffer, 8_000);
        assert_eq!(cfg.emergency_buffer, 2_000);
        assert_eq!(cfg.max_failures, 5);
        assert_eq!(cfg.micro_keep_recent, 3);
        assert_eq!(cfg.micro_gap_seconds, 1800);
        assert_eq!(cfg.compactable_tools, vec!["Read", "Bash"]);
        assert!(!cfg.enabled);
    }

    #[test]
    fn toml_partial_override_uses_defaults() {
        let toml_str = r#"
context_window = 128000
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.context_window, Some(128_000));
        // Everything else should be default
        assert_eq!(cfg.output_reserve, 20_000);
        assert_eq!(cfg.autocompact_buffer, 13_000);
        assert_eq!(cfg.emergency_buffer, 3_000);
        assert_eq!(cfg.max_failures, 3);
        assert_eq!(cfg.micro_keep_recent, 5);
        assert_eq!(cfg.micro_gap_seconds, 3600);
        assert!(cfg.enabled);
    }

    #[test]
    fn toml_empty_uses_all_defaults() {
        let cfg: CompactConfig = toml::from_str("").unwrap();
        let default = CompactConfig::default();
        assert_eq!(cfg.context_window, default.context_window);
        assert_eq!(cfg.output_reserve, default.output_reserve);
        assert_eq!(cfg.autocompact_buffer, default.autocompact_buffer);
        assert_eq!(cfg.emergency_buffer, default.emergency_buffer);
        assert_eq!(cfg.max_failures, default.max_failures);
        assert_eq!(cfg.micro_keep_recent, default.micro_keep_recent);
        assert_eq!(cfg.micro_gap_seconds, default.micro_gap_seconds);
        assert_eq!(cfg.enabled, default.enabled);
    }

    #[test]
    fn cache_diagnostics_defaults_to_false() {
        let cfg = CompactConfig::default();
        assert!(!cfg.cache_diagnostics);
    }

    #[test]
    fn toml_cache_diagnostics_override() {
        let toml_str = r#"
cache_diagnostics = true
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.cache_diagnostics);
    }

    #[test]
    fn default_compaction_is_safe() {
        let cfg = CompactConfig::default();
        assert_eq!(cfg.compaction, wcore_compact::CompactionLevel::Safe);
    }

    #[test]
    fn default_toon_is_false() {
        let cfg = CompactConfig::default();
        assert!(!cfg.toon);
    }

    #[test]
    fn toml_compaction_level_override() {
        let toml_str = r#"compaction = "full""#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.compaction, wcore_compact::CompactionLevel::Full);
    }

    #[test]
    fn toml_compaction_off() {
        let toml_str = r#"compaction = "off""#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.compaction, wcore_compact::CompactionLevel::Off);
    }

    #[test]
    fn toml_toon_enabled() {
        let toml_str = r#"toon = true"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.toon);
    }

    #[test]
    fn smart_compaction_defaults_off() {
        // #280: the master gate is OFF by default and the band defaults sit in
        // the spec band so the use-site clamp is a no-op for the defaults.
        let cfg = CompactConfig::default();
        assert!(!cfg.smart_enabled);
        assert_eq!(cfg.smart_trigger_fraction, 0.65);
        assert_eq!(cfg.smart_release_fraction, 0.50);
        assert_eq!(cfg.smart_cooldown_turns, 2);
        assert_eq!(cfg.smart_min_shrink_tokens, 2_000);
        assert!(cfg.smart_handoff_to_memory);
    }

    #[test]
    fn toml_empty_keeps_smart_off() {
        // An empty [compact] block must leave smart compaction default-OFF so
        // existing configs are byte-for-byte unaffected.
        let cfg: CompactConfig = toml::from_str("").unwrap();
        assert!(!cfg.smart_enabled);
        assert!(cfg.smart_handoff_to_memory);
    }

    #[test]
    fn toml_smart_partial_override_uses_defaults() {
        // Only the master gate set; every other smart field keeps its default.
        let cfg: CompactConfig = toml::from_str("smart_enabled = true").unwrap();
        assert!(cfg.smart_enabled);
        assert_eq!(cfg.smart_trigger_fraction, 0.65);
        assert_eq!(cfg.smart_cooldown_turns, 2);
        assert_eq!(cfg.smart_min_shrink_tokens, 2_000);
        assert!(cfg.smart_handoff_to_memory);
    }

    #[test]
    fn toml_smart_full_override() {
        let toml_str = r#"
smart_enabled = true
smart_trigger_fraction = 0.68
smart_release_fraction = 0.45
smart_cooldown_turns = 4
smart_min_shrink_tokens = 5000
smart_handoff_to_memory = false
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.smart_enabled);
        assert_eq!(cfg.smart_trigger_fraction, 0.68);
        assert_eq!(cfg.smart_release_fraction, 0.45);
        assert_eq!(cfg.smart_cooldown_turns, 4);
        assert_eq!(cfg.smart_min_shrink_tokens, 5_000);
        assert!(!cfg.smart_handoff_to_memory);
    }

    #[test]
    fn tool_call_args_defaults() {
        // Parity gap 2: default ON, protect the last 2 assistant turns,
        // never stub argument payloads under 768 serialized bytes.
        let cfg = CompactConfig::default();
        assert!(cfg.tool_call_args.enabled);
        assert_eq!(cfg.tool_call_args.keep_recent_turns, 2);
        assert_eq!(cfg.tool_call_args.min_args_bytes, 768);
        assert_eq!(cfg.tool_call_args.epoch_turns, 4);
    }

    #[test]
    fn toml_empty_keeps_tool_call_args_defaults() {
        let cfg: CompactConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.tool_call_args, ToolCallArgsConfig::default());
    }

    #[test]
    fn toml_tool_call_args_override() {
        let toml_str = r#"
[tool_call_args]
enabled = false
keep_recent_turns = 4
min_args_bytes = 2048
epoch_turns = 6
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert!(!cfg.tool_call_args.enabled);
        assert_eq!(cfg.tool_call_args.keep_recent_turns, 4);
        assert_eq!(cfg.tool_call_args.min_args_bytes, 2048);
        assert_eq!(cfg.tool_call_args.epoch_turns, 6);
    }

    #[test]
    fn toml_tool_call_args_partial_override() {
        let cfg: CompactConfig =
            toml::from_str("[tool_call_args]\nkeep_recent_turns = 3\n").unwrap();
        assert!(cfg.tool_call_args.enabled);
        assert_eq!(cfg.tool_call_args.keep_recent_turns, 3);
        assert_eq!(cfg.tool_call_args.min_args_bytes, 768);
        assert_eq!(cfg.tool_call_args.epoch_turns, 4);
    }

    #[test]
    fn json_serialization_roundtrip() {
        let cfg = CompactConfig {
            context_window: Some(100_000),
            output_reserve: 15_000,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: CompactConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.context_window, Some(100_000));
        assert_eq!(back.output_reserve, 15_000);
        assert_eq!(back.autocompact_buffer, cfg.autocompact_buffer);
    }

    // ── GH#635: the effective context window ────────────────────────────

    /// An UNCONFIGURED window on a registry-known large-window model resolves
    /// to that model's real window, not the 200k fallback. This is the whole
    /// defect: `gpt-5.4` serves 1,050,000 tokens.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: delete the
    /// `crate::limits::model_output_ceiling(provider, model)` arm of
    /// `CompactConfig::effective_context_window` (compact.rs) and this returns
    /// 200_000.
    #[test]
    fn effective_window_uses_the_models_real_window_when_unconfigured() {
        let cfg = CompactConfig::default();
        assert_eq!(
            cfg.effective_context_window("openai-chatgpt", "gpt-5.4"),
            1_050_000
        );
        assert_eq!(
            cfg.effective_context_window("anthropic", "claude-opus-4-8"),
            1_000_000
        );
        // A registry-known SMALL model lowers it just as readily.
        assert_eq!(cfg.effective_context_window("openai", "gpt-4o"), 128_000);
    }

    /// An explicit operator setting beats the registry in BOTH directions —
    /// capping a 1.05M model at 300k, and widening an unknown model.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: delete the
    /// `if let Some(configured) = self.context_window { return configured; }`
    /// early return in `CompactConfig::effective_context_window` (compact.rs)
    /// and the first assertion returns 1_050_000.
    #[test]
    fn explicit_operator_window_beats_the_registry() {
        let cfg = CompactConfig {
            context_window: Some(300_000),
            ..CompactConfig::default()
        };
        assert_eq!(
            cfg.effective_context_window("openai-chatgpt", "gpt-5.4"),
            300_000
        );
        assert_eq!(
            cfg.effective_context_window("some-provider", "mystery-model"),
            300_000
        );
        // Including the case that used to be indistinguishable from the
        // serde default: an operator who deliberately pins 200k on a 1M model
        // keeps 200k.
        let pinned = CompactConfig {
            context_window: Some(200_000),
            ..CompactConfig::default()
        };
        assert_eq!(
            pinned.effective_context_window("anthropic", "claude-opus-4-8"),
            200_000
        );
    }

    /// #1150 — an unknown model is sized from the BOTTOM of the served
    /// range, not the top. 200,000 here is what let a 32k model grow to
    /// 83,208 input tokens with every compaction boundary out of reach.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: change the final
    /// `UNVERIFIED_CONTEXT_WINDOW` arm of
    /// `CompactConfig::effective_context_window` (compact.rs) back to
    /// `DEFAULT_CONTEXT_WINDOW`.
    #[test]
    fn unknown_model_falls_back_to_the_unverified_window() {
        let cfg = CompactConfig::default();
        assert_eq!(
            cfg.effective_context_window("some-provider", "mystery-model"),
            UNVERIFIED_CONTEXT_WINDOW
        );
        // claude-3-opus is deliberately absent from the registry (4096 output)
        // — it must NOT inherit a 4.x window.
        assert_eq!(
            cfg.effective_context_window("anthropic", "claude-3-opus"),
            UNVERIFIED_CONTEXT_WINDOW
        );
    }

    /// A router tier alias may only LOWER the boundary — it is a guess about
    /// a pool, not a known model.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: delete the
    /// `crate::limits::flux_tier_context_window(model)` arm of
    /// `CompactConfig::effective_context_window` (compact.rs) and this returns
    /// 200_000 for flux-auto, re-opening the CORE-4 wedge.
    #[test]
    fn router_alias_lowers_but_never_raises() {
        let cfg = CompactConfig::default();
        for alias in ["flux-auto", "flux-fast", "flux-standard", "flux-reasoning"] {
            assert_eq!(
                cfg.effective_context_window("flux-router", alias),
                128_000,
                "{alias} must use the conservative pool-minimum floor"
            );
        }
        // The floor can never exceed the fallback, whatever the table says.
        assert!(cfg.effective_context_window("flux-router", "flux-auto") <= DEFAULT_CONTEXT_WINDOW);
    }

    /// Absence must survive a serde round-trip as absence; if it collapsed
    /// back to `Some(200_000)` every reloaded config would look explicit and
    /// the registry would be permanently shadowed.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: drop
    /// `skip_serializing_if = "Option::is_none"` from the `context_window`
    /// field attribute (compact.rs) — the JSON gains `"context_window":null`.
    #[test]
    fn unconfigured_window_round_trips_as_unconfigured() {
        let cfg = CompactConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(
            !json.contains("context_window"),
            "an unset window must not be serialized at all: {json}"
        );
        let back: CompactConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.context_window, None);
        assert_eq!(back.kernel_config_window(), 0);
    }
}
