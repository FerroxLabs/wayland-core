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

/// Autocompact threshold, as a fraction of the window, for the ONE degenerate
/// case: `window - output_reserve - autocompact_buffer` saturating to zero.
///
/// Those buffers are ABSOLUTE and were tuned for a 200k window — 33,000 tokens
/// of headroom, 16.5% there but 100.7% of a 32,768-token window. Zero is not
/// "no threshold" anywhere in the trigger path, it is "always fire":
/// `should_autocompact` tests `tokens >= threshold`
/// (`wcore_agent::compact::auto`) and `ContextPressure::admits_trigger`
/// short-circuits to `true` on a zero threshold by name
/// (`wcore_agent::compact::micro`). An operator who took the unknown-window
/// notice at its word and set `context_window = 32768` would have got an LLM
/// summarization at the top of every single turn.
///
/// It is a REPLACEMENT for the saturated value, not a floor applied to every
/// window: `wcore_agent::compact::auto::autocompact_threshold` reaches for it
/// only when the subtraction already collapsed. A general floor at this
/// fraction would also have moved windows between ~33,000 and ~110,000, and
/// #1150 is no evidence about that band. Every window above 33,000 tokens —
/// which is every model in the `limits` catalogue, the smallest being
/// 128,000 — keeps exactly the threshold it had.
pub const MIN_AUTOCOMPACT_WINDOW_FRACTION: f64 = 0.70;

/// #1150's whole claim, enforced by the compiler rather than by a runtime
/// assertion that could never fail: the window assumed for a model we cannot
/// verify must be the CONSERVATIVE end of the range. Raising
/// `UNVERIFIED_CONTEXT_WINDOW` back to (or above) the historical 200,000 breaks
/// the build here rather than silently restoring the runaway.
const _: () = assert!(UNVERIFIED_CONTEXT_WINDOW < DEFAULT_CONTEXT_WINDOW);

/// The floor must stay a floor: a fraction at or above 1.0 would put the
/// autocompact threshold at or past the window itself, and a non-positive one
/// would re-open the zero-threshold "always fire" cliff.
const _: () =
    assert!(MIN_AUTOCOMPACT_WINDOW_FRACTION > 0.0 && MIN_AUTOCOMPACT_WINDOW_FRACTION < 1.0);

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
        }
    }
}

// --- Default value functions ---

fn default_output_reserve() -> usize {
    20_000
}
fn default_autocompact_buffer() -> usize {
    13_000
}
fn default_emergency_buffer() -> usize {
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

#[cfg(test)]
mod tests {
    use super::*;

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
