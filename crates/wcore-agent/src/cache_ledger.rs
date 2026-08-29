//! F23-04 — the cache / compaction **ledger**: the durable, operator-readable
//! record of what the prompt cache and the compactor actually did.
//!
//! ## Why this exists
//!
//! Phase 23's Success Criterion 4 reads:
//!
//! > Cache and compaction expose **quality, invalidation, token-pressure, cost
//! > truth**.
//!
//! Before this module all four were computed and then thrown away:
//!
//! | Clause | Where it was computed | Where an operator could see it |
//! |---|---|---|
//! | quality | [`crate::cache_diagnostics::CacheBreakDetector`] | an `emit_info` line behind `compact.cache_diagnostics = true`, per turn, never aggregated |
//! | invalidation | `CacheBreakCause`, per turn | same line; `wcore_providers::cache_observation::InvalidationCause` had **zero construction sites** |
//! | token-pressure | `CompactState::last_real_input_tokens` vs the autocompact threshold | nowhere |
//! | cost truth | `resolve_turn_cost` → `TurnTrace.cost_usd` | `/cost` (TUI only), total spend only, no cache attribution |
//!
//! A number computed and never surfaced fails the criterion exactly as hard as
//! one never computed. So this module does two things and only two things:
//! **accumulate** those four families across a whole session, and **persist**
//! the accumulation somewhere a separate process can read it without an engine.
//! Rendering is `wcore-cli`'s `cache` subcommand.
//!
//! ## Cost truth is a status, not a number
//!
//! [`TurnSample::cost_source`] records WHERE the USD figure came from, because
//! three different things had been rendering as one number:
//!
//! - a `$0.00` from a model nobody could price;
//! - a `$0.00` from a genuinely free model;
//! - a real-looking figure derived from the provider FAMILY's rate because the
//!   catalog did not list the model — measured live while building this, on
//!   model `test-model`, which `resolve_turn_cost` reports as `priced = true`.
//!
//! [`LedgerSummary::cost_truth`] therefore grades the session
//! [`CostTruth::Priced`], [`CostTruth::Estimated`], [`CostTruth::Partial`] or
//! [`CostTruth::Unpriced`], and `cache verify` exits non-zero on all but the
//! first.
//!
//! The second half of cost truth is the counterfactual: the ledger records both
//! what the session **was** billed and what the same tokens **would have** been
//! billed with no cache at all ([`TurnSample::uncached_equivalent_usd`]). The
//! difference is signed on purpose — a session that writes cache it never reads
//! back costs MORE than an uncached one, and that must be reportable as a
//! negative saving rather than clamped to zero.
//!
//! #1163: that counterfactual is [`Option`]-typed, and the `Option` is the
//! whole point. It needs catalog rates the billed figure does not — a
//! provider-reported spend arrives priced whether or not anything can price the
//! model. MEASURED on `flux-router`/`flux-reasoning`: the catalog had no row,
//! the counterfactual defaulted to `0.0`, and the subtraction reported
//! `saving_usd = -cost` on a session whose warm hit ratio was 98%. Two figures
//! of different trustworthiness were being rendered under one verdict. So the
//! unpriceable counterfactual is `None` — [`LedgerSummary::cache_saving_usd`]
//! returns `None` with it, and [`LedgerSummary::saving_truth`] grades the
//! SAVING separately from [`LedgerSummary::cost_truth`], which still grades only
//! what was billed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use wcore_providers::cache_observation::{
    CacheRetention, InvalidationCause, PromptCacheObservation,
};

use crate::cache_diagnostics::{CacheBreakCause, CacheDiagnostic};

/// On-disk schema version. Bumped when a field's meaning changes; readers
/// refuse a version they do not understand rather than silently mis-reporting.
///
/// `2` (#1163 follow-up): [`TurnSample::uncached_equivalent_usd`] changed from
/// `f64` to `Option<f64>`, and with it the MEANING of a zero. A v1 writer set
/// the field to `0.0` precisely when nothing could price the counterfactual —
/// that zero is the fabricated baseline #1163 was filed about. A v2 writer
/// omits the field in that case and writes `Some(0.0)` only for a genuine
/// priced zero. `#[serde(default)]` decodes a v1 `0.0` as `Some(0.0)`, so
/// reading a v1 file at schema 1 reproduces the ticket verbatim on the fixed
/// build — the same `saving_usd = -cost`, now additionally graded
/// `saving_truth=priced`. The version is what lets the reader tell the two
/// zeros apart; see [`migrate_v1_counterfactual`].
pub const LEDGER_SCHEMA: u32 = 2;

/// Directory name under the Wayland home holding one ledger per session.
pub const LEDGER_DIR: &str = "cache-ledger";

/// Where a round-trip's USD figure came from.
///
/// This is not a boolean, and that is the point. `AgentEngine::resolve_turn_cost`
/// has two price paths and reports `priced = true` for both: an exact
/// `wcore-pricing` catalog row for this provider×model, and — when the catalog
/// misses — the `ProviderCompat` family default. Those are different kinds of
/// fact. A `claude-opus-4-7` row is a published rate; the same rate applied to
/// an unrecognised model id because it was dispatched to Anthropic is a
/// **directional estimate for a model nobody priced**.
///
/// Measured while building this: a session on model `test-model` came back
/// `priced = true` at Anthropic's generic rate. Rendering that identically to a
/// catalog price is how a cost surface earns trust it has not got, so the
/// ledger records the provenance and [`CostTruth`] grades on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    /// #1139 — the PROVIDER reported this round-trip's dollar figure itself
    /// (`usage.cost_usd` on the OpenAI wire). This outranks [`Self::Catalog`]:
    /// a catalog row is our model of what a call costs, this is what the
    /// account was billed for the call that actually happened — including any
    /// routing, surcharge or discount the catalog cannot see.
    ProviderReported,
    /// Exact `wcore-pricing` catalog row for this provider×model.
    Catalog,
    /// `ProviderCompat` family defaults — the catalog did not know this model.
    ProviderDefaults,
    /// No price could be produced at all; `cost_usd` is `0.0` and means nothing.
    Unpriced,
}

impl CostSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProviderReported => "provider_reported",
            Self::Catalog => "catalog",
            Self::ProviderDefaults => "provider_defaults",
            Self::Unpriced => "unpriced",
        }
    }

    /// Did a price of any kind come out?
    pub fn is_priced(&self) -> bool {
        !matches!(self, Self::Unpriced)
    }
}

// ── Per-turn sample ─────────────────────────────────────────────────────────

/// One LLM round-trip's worth of cache, pressure and cost facts.
///
/// Token fields are **disjoint**: `uncached_input_tokens` is the provider's
/// billed non-cached input, and cache reads / writes are counted separately —
/// matching `wcore_pricing::PricingCatalog::estimate_cost_with_cache_*`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnSample {
    /// 0-based agent-loop turn index.
    pub turn: u64,
    /// 1-based round-trip index within the session (a turn can be several).
    pub round_trip: u64,
    /// RFC3339 UTC timestamp.
    pub ts: String,
    pub provider: String,
    /// The model ACTUALLY dispatched (post tier-swap), so cost is never
    /// mis-attributed to the premium model when a cheap one served the turn.
    pub model: String,
    pub retention: CacheRetention,

    // -- quality ------------------------------------------------------------
    pub uncached_input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,

    // -- invalidation -------------------------------------------------------
    /// `None` on a healthy (cache-serving) round-trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidation_cause: Option<InvalidationCause>,

    // -- cost truth ---------------------------------------------------------
    /// What this round-trip was billed, cache rates applied.
    pub cost_usd: f64,
    /// Where [`Self::cost_usd`] came from. Not a bool: see [`CostSource`].
    pub cost_source: CostSource,
    /// What the SAME tokens would have cost with no cache: every cached token
    /// re-billed at the ordinary input rate.
    ///
    /// `None` when nothing could PRICE that counterfactual for this
    /// provider×model — no catalog row and no user-supplied rate. A
    /// provider-family preset is deliberately not accepted here: it is a
    /// conservative ceiling, and subtracting a real billed figure from a
    /// ceiling is not a measurement. Absent on the wire (#1163).
    ///
    /// A `0.0` in a **v1** file does NOT mean `Some(0.0)`: v1 wrote `0.0`
    /// exactly when nothing could price the counterfactual, which is the
    /// fabricated baseline this field was made optional to remove. `load`
    /// therefore migrates a v1 zero back to `None` — see
    /// [`migrate_v1_counterfactual`] — instead of letting `#[serde(default)]`
    /// launder it into a priced zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncached_equivalent_usd: Option<f64>,

    // -- token pressure -----------------------------------------------------
    /// Real-pressure watermark (`CompactState::last_real_input_tokens`) — the
    /// number the AUTO-compaction trigger reads.
    pub watermark_tokens: u64,
    /// Conservative watermark (`CompactState::last_input_tokens`) — the number
    /// the EMERGENCY hard-stop reads.
    pub conservative_watermark_tokens: u64,
    /// `context_window - output_reserve - autocompact_buffer`.
    pub autocompact_threshold_tokens: u64,
    /// `context_window - emergency_buffer`.
    pub emergency_limit_tokens: u64,
}

impl TurnSample {
    /// Total input the provider processed across all three input categories.
    pub fn total_input_tokens(&self) -> u64 {
        self.uncached_input_tokens
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    /// Cache quality for this round-trip: `cache_read / total_input`.
    /// `0.0` when the provider reported no input at all.
    pub fn hit_ratio(&self) -> f64 {
        let total = self.total_input_tokens();
        if total == 0 {
            return 0.0;
        }
        self.cache_read_tokens as f64 / total as f64
    }

    /// Did the prompt cache serve anything on this round-trip?
    pub fn is_hit(&self) -> bool {
        self.cache_read_tokens > 0
    }

    /// Signed cache saving. **Negative** when the cache-write premium exceeded
    /// what the reads saved back — a real and under-reported outcome for short
    /// sessions, and the reason this is not clamped at zero.
    ///
    /// `None` when the counterfactual could not be priced. There is nothing to
    /// subtract from, and subtracting against a fabricated zero yields exactly
    /// `-cost` — a confident claim that the cache cost everything and saved
    /// nothing (#1163).
    pub fn cache_saving_usd(&self) -> Option<f64> {
        self.uncached_equivalent_usd
            .map(|uncached| uncached - self.cost_usd)
    }

    /// How full the context is, as a fraction of the autocompact threshold.
    /// `>= 1.0` means the next turn is eligible for auto-compaction.
    pub fn pressure_ratio(&self) -> f64 {
        if self.autocompact_threshold_tokens == 0 {
            return 0.0;
        }
        self.watermark_tokens as f64 / self.autocompact_threshold_tokens as f64
    }

    /// Project this sample onto the wire-shaped
    /// [`PromptCacheObservation`].
    ///
    /// This is the type's **first production construction site**. Before F23-04
    /// `PromptCacheObservation` and `InvalidationCause` were `pub use`d from
    /// `wcore-providers` — advertised on that crate's public API — and never
    /// built anywhere in the workspace. Re-using the published vocabulary here,
    /// rather than inventing a third one, is deliberate.
    pub fn as_observation(&self) -> PromptCacheObservation {
        match self.invalidation_cause {
            Some(cause) if !self.is_hit() => PromptCacheObservation::miss(
                self.retention,
                self.provider.clone(),
                self.model.clone(),
                cause,
            ),
            _ => PromptCacheObservation::hit(
                self.retention,
                self.provider.clone(),
                self.model.clone(),
                self.cache_read_tokens,
                self.cache_write_tokens,
            ),
        }
    }
}

/// Translate the engine-side [`CacheBreakCause`] into the published
/// [`InvalidationCause`] vocabulary.
///
/// The two enums existed side by side with no bridge between them: the engine
/// only ever produced `CacheBreakCause`, and `InvalidationCause` — the richer,
/// publicly exported one the criterion's "invalidation" clause names — was
/// never constructed. `FirstRequest` maps to `NoMarker` because no cache marker
/// can have been emitted before the first request of a session.
pub fn invalidation_cause_of(cause: &CacheBreakCause) -> InvalidationCause {
    match cause {
        CacheBreakCause::SystemPromptChanged => InvalidationCause::SystemPromptDrift,
        CacheBreakCause::ToolsChanged => InvalidationCause::ToolDefinitionsChanged,
        CacheBreakCause::ModelChanged => InvalidationCause::ModelChanged,
        // The index is dropped here on purpose: `InvalidationCause` is the
        // published, aggregate vocabulary. The index survives on the
        // engine-side `CacheBreakCause`, which is what the `cache_health_warn`
        // log line renders.
        CacheBreakCause::MessagesChanged { .. } => InvalidationCause::HistoryRewritten,
        CacheBreakCause::TtlExpiry => InvalidationCause::Expired,
        CacheBreakCause::FirstRequest => InvalidationCause::NoMarker,
    }
}

/// Reduce a [`CacheDiagnostic`] to the invalidation cause to record, if any.
///
/// `Healthy` records nothing — there is no invalidation to attribute.
pub fn cause_of_diagnostic(diag: &CacheDiagnostic) -> Option<InvalidationCause> {
    match diag {
        CacheDiagnostic::Healthy { .. } => None,
        CacheDiagnostic::PartialMiss { cause, .. } | CacheDiagnostic::FullMiss { cause } => {
            Some(invalidation_cause_of(cause))
        }
    }
}

// ── Compaction events ───────────────────────────────────────────────────────

/// Which compactor ran, and how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionKind {
    /// `micro::microcompact` — clears tool-result bodies, no LLM call.
    Micro,
    /// `auto::autocompact` — LLM summarization, succeeded.
    Auto,
    /// `auto::autocompact` — attempted and failed (circuit breaker, provider
    /// error, prompt-too-long). Recorded because a compactor that keeps
    /// failing is precisely the token-pressure fact an operator needs.
    AutoFailed,
}

impl CompactionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Micro => "micro",
            Self::Auto => "auto",
            Self::AutoFailed => "auto_failed",
        }
    }
}

/// What made the compactor run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    /// The real-pressure watermark crossed the autocompact threshold.
    Watermark,
    /// A pre-gate forced compaction ahead of the watermark.
    SmartForce,
    /// The microcompact heuristic fired.
    MicroHeuristic,
}

impl CompactionTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Watermark => "watermark",
            Self::SmartForce => "smart_force",
            Self::MicroHeuristic => "micro_heuristic",
        }
    }
}

/// One compaction attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionEvent {
    /// Round-trips already completed when this compaction ran. Compaction
    /// happens at the TOP of a turn, before the next request goes out, so this
    /// is "after round-trip N" — it deliberately is not an agent-loop turn
    /// index, which does not advance one-per-request.
    pub after_round_trip: u64,
    pub ts: String,
    pub kind: CompactionKind,
    pub trigger: CompactionTrigger,
    /// Watermark that made this fire.
    pub watermark_tokens: u64,
    /// Threshold it crossed.
    pub threshold_tokens: u64,
    /// Input-token count before compaction (`CompactResult::pre_compact_tokens`
    /// for auto; the microcompact estimate for micro).
    pub pre_tokens: u64,
    /// Estimated tokens reclaimed.
    pub tokens_freed: u64,
    /// Messages collapsed (auto) or tool results cleared (micro).
    pub items_collapsed: u64,
    /// Populated only for [`CompactionKind::AutoFailed`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── The ledger ──────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("ledger io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("ledger at {path} is malformed: {source}")]
    Malformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("ledger at {path} has schema {found}, this build understands {expected}")]
    SchemaMismatch {
        path: PathBuf,
        found: u32,
        expected: u32,
    },
    #[error("no cache ledger found in {0}")]
    Empty(PathBuf),
}

/// The whole session's cache + compaction record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheLedger {
    pub schema: u32,
    /// The engine's `conversation_id` — the same id the Flux sticky-routing
    /// header and `cache_health_warn` telemetry carry.
    pub session_id: String,
    pub started_at: String,
    pub updated_at: String,
    /// `true` once the session ended cleanly. A ledger left `false` was
    /// interrupted, and its totals are a lower bound.
    pub session_complete: bool,
    pub turns: Vec<TurnSample>,
    pub compactions: Vec<CompactionEvent>,
}

impl CacheLedger {
    pub fn new(session_id: impl Into<String>) -> Self {
        let now = now_rfc3339();
        Self {
            schema: LEDGER_SCHEMA,
            session_id: session_id.into(),
            started_at: now.clone(),
            updated_at: now,
            session_complete: false,
            turns: Vec::new(),
            compactions: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty() && self.compactions.is_empty()
    }

    /// Next round-trip index (1-based).
    pub fn next_round_trip(&self) -> u64 {
        self.turns.len() as u64 + 1
    }

    /// Did a compaction run after the last round-trip recorded? Used to
    /// attribute a cache miss to [`InvalidationCause::HistoryRewritten`] rather
    /// than to whatever the prompt/tool-hash comparison guessed. Compaction
    /// rewrites the message history wholesale, so the NEXT round-trip cannot
    /// possibly hit — and attributing that to "system prompt drift" would be a
    /// lie about a self-inflicted miss.
    pub fn compacted_since_last_round_trip(&self) -> bool {
        let completed = self.turns.len() as u64;
        self.compactions
            .iter()
            .any(|c| c.after_round_trip >= completed)
    }

    pub fn record_turn(&mut self, sample: TurnSample) {
        self.updated_at = sample.ts.clone();
        self.turns.push(sample);
    }

    pub fn record_compaction(&mut self, event: CompactionEvent) {
        self.updated_at = event.ts.clone();
        self.compactions.push(event);
    }

    pub fn mark_complete(&mut self) {
        self.session_complete = true;
        self.updated_at = now_rfc3339();
    }

    /// Aggregate the four criterion clauses across the session.
    pub fn summarize(&self) -> LedgerSummary {
        let mut s = LedgerSummary {
            session_id: self.session_id.clone(),
            started_at: self.started_at.clone(),
            updated_at: self.updated_at.clone(),
            session_complete: self.session_complete,
            ..LedgerSummary::default()
        };

        s.round_trips = self.turns.len() as u64;
        // `None` the moment any round-trip's counterfactual is unknown; stays
        // `Some` only while every one of them priced.
        let mut counterfactual = Some(0.0f64);
        for t in &self.turns {
            s.uncached_input_tokens = s
                .uncached_input_tokens
                .saturating_add(t.uncached_input_tokens);
            s.cache_read_tokens = s.cache_read_tokens.saturating_add(t.cache_read_tokens);
            s.cache_write_tokens = s.cache_write_tokens.saturating_add(t.cache_write_tokens);
            s.output_tokens = s.output_tokens.saturating_add(t.output_tokens);
            s.cost_usd += t.cost_usd;
            // A partial sum of counterfactuals is a FLOOR, and a floor
            // subtracted from a complete billed total reports a saving smaller
            // than the truth with full confidence. One unknown row therefore
            // makes the session total unknown — the same rule
            // `AgentEngine::fold_reported_cost` applies one level down.
            match t.uncached_equivalent_usd {
                Some(usd) => {
                    if let Some(total) = counterfactual.as_mut() {
                        *total += usd;
                    }
                }
                None => {
                    counterfactual = None;
                    s.counterfactual_unpriced_round_trips += 1;
                }
            }
            match t.cost_source {
                CostSource::ProviderReported => s.provider_reported_round_trips += 1,
                CostSource::Catalog => s.catalog_priced_round_trips += 1,
                CostSource::ProviderDefaults => s.estimated_round_trips += 1,
                CostSource::Unpriced => s.unpriced_round_trips += 1,
            }
            if t.is_hit() {
                s.hit_round_trips += 1;
            } else {
                s.miss_round_trips += 1;
            }
            if let Some(cause) = t.invalidation_cause {
                *s.invalidation_causes
                    .entry(cause.as_str().to_string())
                    .or_insert(0) += 1;
            }
            if t.watermark_tokens > s.peak_watermark_tokens {
                s.peak_watermark_tokens = t.watermark_tokens;
                s.autocompact_threshold_tokens = t.autocompact_threshold_tokens;
                s.emergency_limit_tokens = t.emergency_limit_tokens;
            }
            // Warm quality: skip the first two round-trips, which cannot hit
            // (nothing has been written yet). Reporting a cold session's 0%
            // as a cache-health failure is the false alarm this excludes.
            if t.round_trip > WARM_AFTER_ROUND_TRIPS {
                s.warm_round_trips += 1;
                s.warm_cache_read_tokens =
                    s.warm_cache_read_tokens.saturating_add(t.cache_read_tokens);
                s.warm_total_input_tokens = s
                    .warm_total_input_tokens
                    .saturating_add(t.total_input_tokens());
            }
        }

        s.compactions = self.compactions.len() as u64;
        for c in &self.compactions {
            match c.kind {
                CompactionKind::Micro => s.micro_compactions += 1,
                CompactionKind::Auto => s.auto_compactions += 1,
                CompactionKind::AutoFailed => s.failed_compactions += 1,
            }
            s.tokens_reclaimed = s.tokens_reclaimed.saturating_add(c.tokens_freed);
        }

        // An empty ledger has no counterfactual either — a `Some(0.0)` there
        // would render as "the cache saved exactly nothing", which is a claim.
        s.uncached_equivalent_usd = if self.turns.is_empty() {
            None
        } else {
            counterfactual
        };

        s
    }
}

/// A session counts as warm strictly AFTER this many round-trips — the same
/// constant the engine's `cache_health_warn` detector uses.
pub const WARM_AFTER_ROUND_TRIPS: u64 =
    crate::cache_diagnostics::CACHE_HEALTH_WARM_AFTER_ROUND_TRIPS;

/// How much of the session's reported cost is a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostTruth {
    /// Every round-trip priced from an exact catalog row. The USD figure is a
    /// fact.
    Priced,
    /// Every round-trip has a price, but at least one came from
    /// [`CostSource::ProviderDefaults`] — the provider family's rate applied to
    /// a model the catalog does not list. Directionally right; not a fact.
    Estimated,
    /// Some round-trips have no price at all. The USD figure is a **floor**.
    Partial,
    /// No round-trip could be priced (or there are none). The USD figure
    /// carries no information and must not be presented as spend.
    Unpriced,
}

impl CostTruth {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Priced => "priced",
            Self::Estimated => "estimated",
            Self::Partial => "partial",
            Self::Unpriced => "unpriced",
        }
    }

    /// Can an operator trust the USD number as spend?
    ///
    /// Only [`Self::Priced`]. An estimate at the family rate for an unlisted
    /// model is useful for direction and wrong for accounting, and the whole
    /// reason this type exists is that the two had been rendering identically.
    pub fn is_trustworthy(&self) -> bool {
        matches!(self, Self::Priced)
    }
}

/// Session-level aggregate: one struct carrying all four criterion clauses.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LedgerSummary {
    pub session_id: String,
    pub started_at: String,
    pub updated_at: String,
    pub session_complete: bool,

    pub round_trips: u64,

    // quality
    pub uncached_input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub hit_round_trips: u64,
    pub miss_round_trips: u64,
    pub warm_round_trips: u64,
    pub warm_cache_read_tokens: u64,
    pub warm_total_input_tokens: u64,

    // invalidation
    pub invalidation_causes: BTreeMap<String, u64>,

    // token pressure
    pub peak_watermark_tokens: u64,
    pub autocompact_threshold_tokens: u64,
    pub emergency_limit_tokens: u64,
    pub compactions: u64,
    pub micro_compactions: u64,
    pub auto_compactions: u64,
    pub failed_compactions: u64,
    pub tokens_reclaimed: u64,

    // cost truth
    pub cost_usd: f64,
    /// The session's uncached counterfactual, or `None` when ANY round-trip's
    /// could not be priced. See [`TurnSample::uncached_equivalent_usd`].
    #[serde(default)]
    pub uncached_equivalent_usd: Option<f64>,
    /// Round-trips whose counterfactual could not be priced. Reported so an
    /// operator can see WHY the saving is unknown rather than just that it is.
    #[serde(default)]
    pub counterfactual_unpriced_round_trips: u64,
    /// Round-trips whose figure the provider reported itself — spend, not a
    /// price model. See [`CostSource::ProviderReported`].
    #[serde(default)]
    pub provider_reported_round_trips: u64,
    /// Round-trips priced from an exact catalog row.
    pub catalog_priced_round_trips: u64,
    /// Round-trips priced from provider-family defaults — an estimate for a
    /// model the catalog does not list.
    pub estimated_round_trips: u64,
    /// Round-trips with no price at all.
    pub unpriced_round_trips: u64,
}

impl LedgerSummary {
    pub fn total_input_tokens(&self) -> u64 {
        self.uncached_input_tokens
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    /// Session cache quality: `cache_read / total_input`.
    pub fn hit_ratio(&self) -> f64 {
        let total = self.total_input_tokens();
        if total == 0 {
            return 0.0;
        }
        self.cache_read_tokens as f64 / total as f64
    }

    /// Quality excluding the unavoidably-cold opening round-trips. This is the
    /// number worth alarming on; [`Self::hit_ratio`] is the number worth
    /// billing on.
    pub fn warm_hit_ratio(&self) -> f64 {
        if self.warm_total_input_tokens == 0 {
            return 0.0;
        }
        self.warm_cache_read_tokens as f64 / self.warm_total_input_tokens as f64
    }

    /// Signed saving: what the cache actually bought (or cost).
    ///
    /// `None` when the counterfactual is unknown — see
    /// [`Self::uncached_equivalent_usd`].
    pub fn cache_saving_usd(&self) -> Option<f64> {
        self.uncached_equivalent_usd
            .map(|uncached| uncached - self.cost_usd)
    }

    /// Saving as a fraction of the uncached counterfactual. `None` when the
    /// counterfactual is unknown, or when it is zero — there is nothing to take
    /// a fraction of, and `0.0` would read as "the cache changed nothing".
    pub fn cache_saving_ratio(&self) -> Option<f64> {
        match (self.uncached_equivalent_usd, self.cache_saving_usd()) {
            (Some(uncached), Some(saving)) if uncached != 0.0 => Some(saving / uncached),
            _ => None,
        }
    }

    /// Grade the trustworthiness of [`Self::cache_saving_usd`].
    ///
    /// The saving is a DIFFERENCE of two figures, so it is only as good as the
    /// weaker one. [`Self::cost_truth`] grades the billed half and nothing else
    /// — that is its documented contract, and dragging a provider-reported
    /// spend down to `partial` because the CATALOG cannot price a
    /// counterfactual would be a second wrong claim, not a fix for the first.
    ///
    /// So the report carries two verdicts, one per figure. #1163 is what one
    /// verdict over two figures looks like: `cost_truth=priced` printed beside
    /// `saving_usd=-0.061389` on a session with a 98% warm hit ratio.
    pub fn saving_truth(&self) -> CostTruth {
        if self.uncached_equivalent_usd.is_none() {
            CostTruth::Unpriced
        } else {
            self.cost_truth()
        }
    }

    /// Peak context pressure as a fraction of the autocompact threshold.
    pub fn peak_pressure_ratio(&self) -> f64 {
        if self.autocompact_threshold_tokens == 0 {
            return 0.0;
        }
        self.peak_watermark_tokens as f64 / self.autocompact_threshold_tokens as f64
    }

    /// Round-trips that produced a price of any kind.
    pub fn priced_round_trips(&self) -> u64 {
        self.provider_reported_round_trips
            + self.catalog_priced_round_trips
            + self.estimated_round_trips
    }

    /// Grade the trustworthiness of [`Self::cost_usd`].
    ///
    /// Ordered worst-first so a single bad round-trip cannot be averaged away
    /// by good ones — a total is only as trustworthy as its weakest row.
    pub fn cost_truth(&self) -> CostTruth {
        if self.priced_round_trips() == 0 {
            CostTruth::Unpriced
        } else if self.unpriced_round_trips > 0 {
            CostTruth::Partial
        } else if self.estimated_round_trips > 0 {
            CostTruth::Estimated
        } else {
            // Only `ProviderReported` and `Catalog` rows are left, and both are
            // statements about this provider×model rather than a family-rate
            // guess — so the total is a fact.
            CostTruth::Priced
        }
    }
}

// ── Persistence ─────────────────────────────────────────────────────────────

/// Default ledger directory: `<wayland home>/cache-ledger`.
///
/// Routed through [`wcore_config::config::wayland_config_dir`] so `WAYLAND_HOME`
/// hermetically sandboxes it like every other engine-written path.
pub fn default_ledger_dir() -> PathBuf {
    wcore_config::config::wayland_config_dir().join(LEDGER_DIR)
}

/// Path of one session's ledger inside `dir`.
///
/// The session id is sanitized to `[A-Za-z0-9._-]` before it reaches the
/// filesystem: it is a uuid today, but it is restored verbatim from resumed
/// checkpoints and must never be able to escape the directory.
pub fn ledger_path(dir: &Path, session_id: &str) -> PathBuf {
    let safe: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.is_empty() || safe.chars().all(|c| c == '.') {
        "unnamed".to_string()
    } else {
        safe
    };
    dir.join(format!("{safe}.json"))
}

/// Write the ledger atomically (temp file + rename) so a process killed
/// mid-write leaves the previous ledger intact rather than a truncated one.
pub fn save(ledger: &CacheLedger, path: &Path) -> Result<(), LedgerError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| LedgerError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let json = serde_json::to_vec_pretty(ledger).map_err(|source| LedgerError::Malformed {
        path: path.to_path_buf(),
        source,
    })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|source| LedgerError::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| LedgerError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Undo the v1 zero-counterfactual default (#1163).
///
/// v1 stored the counterfactual as a bare `f64` and wrote `0.0` when the model
/// had no catalog row, so the saving rendered as `-cost` against a baseline
/// nobody computed. v2 makes the field optional and omits it in that case, but
/// `#[serde(default)]` cannot tell a v1 `0.0` from a v2 `Some(0.0)` — only the
/// schema version can. Every v1 zero is therefore mapped back to `None`, which
/// is what that writer actually knew.
///
/// A v1 turn whose counterfactual was a genuine priced zero (a round-trip that
/// processed no tokens) is demoted to unknown by this. That is the safe
/// direction: an unknown saving renders as unknown, whereas a false zero
/// renders as a confident negative number, which is the ticket.
fn migrate_v1_counterfactual(ledger: &mut CacheLedger) {
    for turn in &mut ledger.turns {
        if turn.uncached_equivalent_usd == Some(0.0) {
            turn.uncached_equivalent_usd = None;
        }
    }
    ledger.schema = LEDGER_SCHEMA;
}

/// Read one ledger, refusing a schema this build cannot account for.
///
/// Every older schema needs its OWN arm in the match below — a version range
/// would silently apply the v1 migration to a v2 file the day
/// [`LEDGER_SCHEMA`] goes to 3, which is the same class of harm the migration
/// exists to undo. The migration is read-only: the file on disk keeps its own
/// version until something writes it again, so a downgrade still finds the
/// ledger it left behind.
pub fn load(path: &Path) -> Result<CacheLedger, LedgerError> {
    let raw = std::fs::read(path).map_err(|source| LedgerError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut ledger: CacheLedger =
        serde_json::from_slice(&raw).map_err(|source| LedgerError::Malformed {
            path: path.to_path_buf(),
            source,
        })?;
    match ledger.schema {
        LEDGER_SCHEMA => {}
        1 => migrate_v1_counterfactual(&mut ledger),
        found => {
            return Err(LedgerError::SchemaMismatch {
                path: path.to_path_buf(),
                found,
                expected: LEDGER_SCHEMA,
            });
        }
    }
    Ok(ledger)
}

/// Every ledger in `dir`, newest `updated_at` first. A malformed or
/// wrong-schema file is skipped rather than failing the listing — one bad
/// ledger must not hide the rest.
pub fn list(dir: &Path) -> Result<Vec<(PathBuf, CacheLedger)>, LedgerError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(LedgerError::Io {
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    let mut out: Vec<(PathBuf, CacheLedger)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(ledger) = load(&path) {
            out.push((path, ledger));
        }
    }
    out.sort_by(|a, b| b.1.updated_at.cmp(&a.1.updated_at));
    Ok(out)
}

/// The most recently updated ledger in `dir`.
pub fn latest(dir: &Path) -> Result<(PathBuf, CacheLedger), LedgerError> {
    list(dir)?
        .into_iter()
        .next()
        .ok_or_else(|| LedgerError::Empty(dir.to_path_buf()))
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Environment kill-switch. Set to `0`, `false` or `off` to stop the engine
/// writing a ledger at all.
pub const LEDGER_ENV: &str = "WAYLAND_CACHE_LEDGER";

/// Is ledger recording on? **On by default.**
///
/// Deliberately not a TOML opt-in. The `compact.cache_diagnostics` flag it sits
/// beside defaults to `false`, which is exactly why the cache diagnostics that
/// already existed were graded unexposed: a surface nobody turns on is a
/// surface nobody has. A few kilobytes of JSON per session is the whole cost.
pub fn recording_enabled() -> bool {
    match std::env::var(LEDGER_ENV) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

// ── Engine-side recorder ────────────────────────────────────────────────────

/// The engine's handle on the ledger: accumulates, and flushes to disk after
/// every record so a killed session still leaves everything up to its last
/// round-trip on disk.
///
/// Arms itself lazily on first use, so it needs no session id at construction
/// time and adds nothing to the engine's constructors beyond one defaulted
/// field.
#[derive(Debug, Default)]
pub struct CacheLedgerRecorder {
    ledger: Option<CacheLedger>,
    path: Option<PathBuf>,
    /// Directory override — tests point this at a tempdir. `None` means
    /// [`default_ledger_dir`].
    dir_override: Option<PathBuf>,
    /// Retention the engine REQUESTED on the in-flight round-trip, noted where
    /// the request is built and consumed where the response lands. Held here
    /// rather than on the engine so recording the request side costs no extra
    /// engine field. `None` until the first request of a session.
    pending_retention: Option<CacheRetention>,
    /// Last flush failure, kept so a persistently unwritable home is
    /// diagnosable instead of silently producing no ledger.
    last_error: Option<String>,
    /// Flush failures seen. After [`Self::MAX_FLUSH_FAILURES`] the recorder
    /// stops trying: an unwritable home must not cost one failed syscall per
    /// round-trip for the rest of the session.
    flush_failures: u32,
}

impl CacheLedgerRecorder {
    pub const MAX_FLUSH_FAILURES: u32 = 3;

    /// Point the recorder at a specific directory (tests, and any caller that
    /// wants a hermetic ledger location).
    pub fn with_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir_override: Some(dir.into()),
            ..Self::default()
        }
    }

    pub fn ledger(&self) -> Option<&CacheLedger> {
        self.ledger.as_ref()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    fn dir(&self) -> PathBuf {
        self.dir_override.clone().unwrap_or_else(default_ledger_dir)
    }

    /// Arm the recorder for `session_id`, CONTINUING the ledger already on disk
    /// for it when there is one.
    ///
    /// #1161 — the continuation is the point. Keying the ledger by a stable
    /// `conversation_id` stops a resume from writing a SECOND file, but a
    /// recorder that always started from `CacheLedger::new` would then truncate
    /// the first launch's rows on its first flush: same fragmentation, now with
    /// data loss instead of an orphan. So a resumed session picks the record up
    /// where it was left, keeping `started_at`, the round-trip numbering and
    /// every earlier row.
    ///
    /// A file that will not load — malformed, or written by a schema this build
    /// does not understand — starts a fresh ledger. It is already unreadable to
    /// every reader; refusing to record would trade an unreadable past for an
    /// unrecorded present.
    pub fn arm(&mut self, session_id: &str) -> &mut CacheLedger {
        if self.ledger.is_none() {
            let path = ledger_path(&self.dir(), session_id);
            let mut ledger = match load(&path) {
                Ok(existing) if existing.session_id == session_id => existing,
                _ => CacheLedger::new(session_id),
            };
            // The session is live again, so it is no longer complete. A resumed
            // ledger left `session_complete: true` would report its totals as
            // final while the session is still spending.
            ledger.session_complete = false;
            self.ledger = Some(ledger);
            self.path = Some(path);
        }
        self.ledger.as_mut().expect("armed above")
    }

    /// Note the cache retention the engine asked for on the round-trip about
    /// to be sent. Recorded on the request side because the response carries
    /// no retention field — an operator asking "was a 1h cache even requested?"
    /// cannot be answered from the response alone.
    pub fn note_retention(&mut self, retention: CacheRetention) {
        self.pending_retention = Some(retention);
    }

    /// Retention requested for the in-flight round-trip, or
    /// [`CacheRetention::None`] when the engine never asked for one.
    pub fn pending_retention(&self) -> CacheRetention {
        self.pending_retention.unwrap_or(CacheRetention::None)
    }

    /// Next round-trip index, 1-based. `1` before anything is recorded.
    pub fn next_round_trip(&self) -> u64 {
        self.ledger.as_ref().map_or(1, CacheLedger::next_round_trip)
    }

    /// Did a compaction run since the last recorded round-trip?
    pub fn compacted_since_last_round_trip(&self) -> bool {
        self.ledger
            .as_ref()
            .is_some_and(CacheLedger::compacted_since_last_round_trip)
    }

    pub fn record_turn(&mut self, session_id: &str, sample: TurnSample) {
        if !recording_enabled() {
            return;
        }
        self.arm(session_id).record_turn(sample);
        self.flush();
    }

    pub fn record_compaction(&mut self, session_id: &str, event: CompactionEvent) {
        if !recording_enabled() {
            return;
        }
        self.arm(session_id).record_compaction(event);
        self.flush();
    }

    /// Mark the session finished and flush. Safe to call when nothing was ever
    /// recorded — it then does nothing, so a session that made no LLM call
    /// leaves no misleading all-zero ledger behind.
    pub fn finish(&mut self) {
        if !recording_enabled() {
            return;
        }
        if let Some(ledger) = self.ledger.as_mut() {
            if ledger.is_empty() {
                return;
            }
            ledger.mark_complete();
        }
        self.flush();
    }

    /// Finish the current ledger and drop it, so the next record starts a new
    /// one. Called when the engine resets its conversation id.
    pub fn rotate(&mut self) {
        self.finish();
        self.ledger = None;
        self.path = None;
        self.flush_failures = 0;
        self.last_error = None;
    }

    fn flush(&mut self) {
        if self.flush_failures >= Self::MAX_FLUSH_FAILURES {
            return;
        }
        let (Some(ledger), Some(path)) = (self.ledger.as_ref(), self.path.as_ref()) else {
            return;
        };
        if let Err(e) = save(ledger, path) {
            self.flush_failures += 1;
            self.last_error = Some(e.to_string());
            tracing::debug!(
                target: "cache_ledger",
                error = %e,
                failures = self.flush_failures,
                "cache ledger flush failed",
            );
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(round_trip: u64, uncached: u64, read: u64, write: u64) -> TurnSample {
        TurnSample {
            turn: round_trip.saturating_sub(1),
            round_trip,
            ts: format!("2026-07-29T10:0{round_trip}:00.000Z"),
            provider: "anthropic".into(),
            model: "claude-opus-4-7".into(),
            retention: CacheRetention::Ephemeral5m,
            uncached_input_tokens: uncached,
            cache_read_tokens: read,
            cache_write_tokens: write,
            output_tokens: 100,
            invalidation_cause: None,
            cost_usd: 0.01,
            cost_source: CostSource::Catalog,
            uncached_equivalent_usd: Some(0.02),
            watermark_tokens: 10_000 * round_trip,
            conservative_watermark_tokens: 11_000 * round_trip,
            autocompact_threshold_tokens: 100_000,
            emergency_limit_tokens: 190_000,
        }
    }

    #[test]
    fn hit_ratio_is_cache_read_over_total_input() {
        let s = sample(1, 100, 900, 0);
        assert!((s.hit_ratio() - 0.9).abs() < 1e-9);
        assert!(s.is_hit());
    }

    #[test]
    fn hit_ratio_zero_input_does_not_divide_by_zero() {
        let s = sample(1, 0, 0, 0);
        assert_eq!(s.hit_ratio(), 0.0);
        assert!(!s.is_hit());
    }

    #[test]
    fn cache_write_tokens_count_against_quality() {
        // A write-only round-trip paid the write premium and read nothing.
        // Excluding writes from the denominator would flatter the ratio.
        let s = sample(1, 0, 0, 1_000);
        assert_eq!(s.total_input_tokens(), 1_000);
        assert_eq!(s.hit_ratio(), 0.0);
    }

    #[test]
    fn cache_saving_can_be_negative() {
        let mut s = sample(1, 1_000, 0, 5_000);
        s.cost_usd = 0.05;
        s.uncached_equivalent_usd = Some(0.04);
        assert!(
            s.cache_saving_usd().is_some_and(|saving| saving < 0.0),
            "a write-heavy turn that never read back must report a NEGATIVE saving, got {:?}",
            s.cache_saving_usd()
        );
    }

    #[test]
    fn break_cause_maps_onto_published_vocabulary() {
        assert_eq!(
            invalidation_cause_of(&CacheBreakCause::SystemPromptChanged),
            InvalidationCause::SystemPromptDrift
        );
        assert_eq!(
            invalidation_cause_of(&CacheBreakCause::ToolsChanged),
            InvalidationCause::ToolDefinitionsChanged
        );
        assert_eq!(
            invalidation_cause_of(&CacheBreakCause::TtlExpiry),
            InvalidationCause::Expired
        );
        assert_eq!(
            invalidation_cause_of(&CacheBreakCause::FirstRequest),
            InvalidationCause::NoMarker
        );
    }

    #[test]
    fn healthy_diagnostic_records_no_cause() {
        assert_eq!(
            cause_of_diagnostic(&CacheDiagnostic::Healthy { hit_rate: 0.95 }),
            None
        );
        assert_eq!(
            cause_of_diagnostic(&CacheDiagnostic::FullMiss {
                cause: CacheBreakCause::TtlExpiry
            }),
            Some(InvalidationCause::Expired)
        );
    }

    #[test]
    fn observation_construction_round_trips_through_wire_type() {
        let mut s = sample(3, 0, 0, 0);
        s.invalidation_cause = Some(InvalidationCause::HistoryRewritten);
        let obs = s.as_observation();
        assert!(!obs.is_hit());
        assert_eq!(
            obs.invalidation_cause,
            Some(InvalidationCause::HistoryRewritten)
        );
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains("history_rewritten"), "{json}");
    }

    #[test]
    fn summary_aggregates_quality_and_cost() {
        let mut l = CacheLedger::new("sess-1");
        l.record_turn(sample(1, 1_000, 0, 1_000));
        l.record_turn(sample(2, 100, 900, 0));
        l.record_turn(sample(3, 100, 900, 0));
        let s = l.summarize();
        assert_eq!(s.round_trips, 3);
        assert_eq!(s.cache_read_tokens, 1_800);
        assert_eq!(s.cache_write_tokens, 1_000);
        assert_eq!(s.uncached_input_tokens, 1_200);
        assert_eq!(s.total_input_tokens(), 4_000);
        assert_eq!(s.hit_round_trips, 2);
        assert_eq!(s.miss_round_trips, 1);
        // 3 × $0.01 billed against 3 × $0.02 uncached.
        assert!((s.cost_usd - 0.03).abs() < 1e-9);
        assert!((s.cache_saving_usd().expect("all rows priced") - 0.03).abs() < 1e-9);
        assert!((s.cache_saving_ratio().expect("all rows priced") - 0.5).abs() < 1e-9);
        assert_eq!(s.cost_truth(), CostTruth::Priced);
    }

    #[test]
    fn warm_hit_ratio_excludes_cold_opening_round_trips() {
        let mut l = CacheLedger::new("sess-warm");
        // Round-trips 1 and 2 are cold by construction.
        l.record_turn(sample(1, 1_000, 0, 0));
        l.record_turn(sample(2, 1_000, 0, 0));
        // Round-trip 3 is warm and hits perfectly.
        l.record_turn(sample(3, 0, 1_000, 0));
        let s = l.summarize();
        assert_eq!(s.warm_round_trips, 1);
        assert!(
            (s.warm_hit_ratio() - 1.0).abs() < 1e-9,
            "warm ratio should be 1.0, got {}",
            s.warm_hit_ratio()
        );
        assert!(
            s.hit_ratio() < s.warm_hit_ratio(),
            "the cold-inclusive ratio must be the lower of the two"
        );
    }

    #[test]
    fn cost_truth_grades_partial_pricing() {
        let mut l = CacheLedger::new("sess-cost");
        l.record_turn(sample(1, 1_000, 0, 0));
        let mut unpriced = sample(2, 1_000, 0, 0);
        unpriced.cost_source = CostSource::Unpriced;
        unpriced.cost_usd = 0.0;
        l.record_turn(unpriced);
        let s = l.summarize();
        assert_eq!(s.cost_truth(), CostTruth::Partial);
        assert!(!s.cost_truth().is_trustworthy());
        assert_eq!(s.unpriced_round_trips, 1);
        assert_eq!(s.catalog_priced_round_trips, 1);
    }

    #[test]
    fn a_family_default_price_is_estimated_not_priced() {
        // The distinction this whole type exists for. One round-trip whose USD
        // came from the provider's family rate rather than a catalog row makes
        // the session's total an estimate — and `is_trustworthy` must say no.
        let mut l = CacheLedger::new("sess-estimated");
        l.record_turn(sample(1, 1_000, 0, 0));
        let mut est = sample(2, 1_000, 0, 0);
        est.cost_source = CostSource::ProviderDefaults;
        l.record_turn(est);
        let s = l.summarize();
        assert_eq!(s.cost_truth(), CostTruth::Estimated);
        assert!(!s.cost_truth().is_trustworthy());
        assert_eq!(s.estimated_round_trips, 1);
        assert_eq!(s.unpriced_round_trips, 0);
        // And it is NOT collapsed into Priced: the known-negative that proves
        // the grade is actually reading the source.
        assert_ne!(s.cost_truth(), CostTruth::Priced);

        // An unpriced row present alongside an estimated one grades worse, not
        // averaged — a total is only as good as its weakest row.
        let mut worse = sample(3, 1_000, 0, 0);
        worse.cost_source = CostSource::Unpriced;
        l.record_turn(worse);
        assert_eq!(l.summarize().cost_truth(), CostTruth::Partial);
    }

    #[test]
    fn cost_truth_unpriced_when_nothing_priced() {
        let mut l = CacheLedger::new("sess-unpriced");
        let mut t = sample(1, 1_000, 0, 0);
        t.cost_source = CostSource::Unpriced;
        t.cost_usd = 0.0;
        l.record_turn(t);
        assert_eq!(l.summarize().cost_truth(), CostTruth::Unpriced);
    }

    #[test]
    fn empty_ledger_is_unpriced_not_free() {
        // The failure this guards: an empty ledger reporting `$0.00 priced`,
        // which is indistinguishable from a genuinely free session.
        let l = CacheLedger::new("sess-empty");
        assert_eq!(l.summarize().cost_truth(), CostTruth::Unpriced);
    }

    #[test]
    fn compaction_since_last_round_trip_detects_history_rewrite() {
        let mut l = CacheLedger::new("sess-compact");
        // Known-negative: nothing compacted yet, so the flag must be false.
        // Without this the assertion below could pass on a function that
        // returns `true` unconditionally.
        assert!(!l.compacted_since_last_round_trip());

        l.record_turn(sample(1, 1_000, 0, 1_000));
        l.record_turn(sample(2, 100, 900, 0));
        assert!(!l.compacted_since_last_round_trip());

        // Compaction after round-trip 2.
        l.record_compaction(CompactionEvent {
            after_round_trip: 2,
            ts: "2026-07-29T10:04:00.000Z".into(),
            kind: CompactionKind::Auto,
            trigger: CompactionTrigger::Watermark,
            watermark_tokens: 120_000,
            threshold_tokens: 100_000,
            pre_tokens: 120_000,
            tokens_freed: 90_000,
            items_collapsed: 42,
            error: None,
        });
        assert!(
            l.compacted_since_last_round_trip(),
            "a compaction after the last recorded round-trip must be visible"
        );

        // Once round-trip 3 is recorded, the compaction is no longer "since
        // the last round-trip" — round-trip 4 must not inherit the attribution.
        l.record_turn(sample(3, 1_000, 0, 1_000));
        assert!(
            !l.compacted_since_last_round_trip(),
            "the history-rewrite attribution must not persist past the round-trip it explains"
        );
    }

    #[test]
    fn summary_counts_failed_compactions_separately() {
        let mut l = CacheLedger::new("sess-fail");
        l.record_compaction(CompactionEvent {
            after_round_trip: 1,
            ts: "2026-07-29T10:01:00.000Z".into(),
            kind: CompactionKind::AutoFailed,
            trigger: CompactionTrigger::Watermark,
            watermark_tokens: 120_000,
            threshold_tokens: 100_000,
            pre_tokens: 120_000,
            tokens_freed: 0,
            items_collapsed: 0,
            error: Some("circuit breaker tripped".into()),
        });
        let s = l.summarize();
        assert_eq!(s.failed_compactions, 1);
        assert_eq!(s.auto_compactions, 0);
        assert_eq!(s.tokens_reclaimed, 0);
    }

    #[test]
    fn ledger_path_cannot_escape_its_directory() {
        // The invariant is about PATH COMPONENTS, not about the substring
        // "..": `../../etc/passwd` sanitizes to the single filename
        // `.._.._etc_passwd.json`, which contains two dots side by side and
        // traverses nothing. A first draft of this test asserted
        // `!contains("..")` and failed on that correct output — the assertion
        // was wrong, not the sanitizer. Assert components instead.
        let dir = Path::new("/tmp/ledgers");
        for hostile in [
            "../../etc/passwd",
            "..",
            "....",
            "/etc/shadow",
            "a/../../b",
            "",
            "C:\\Windows\\System32",
        ] {
            let p = ledger_path(dir, hostile);
            assert_eq!(
                p.parent().unwrap(),
                dir,
                "traversal escaped for {hostile:?}: {}",
                p.display()
            );
            assert!(
                p.components().count() == dir.components().count() + 1,
                "extra components for {hostile:?}: {}",
                p.display()
            );
            assert!(
                !p.components()
                    .any(|c| matches!(c, std::path::Component::ParentDir)),
                "parent-dir component for {hostile:?}: {}",
                p.display()
            );
        }
        // Known-negative for the instrument itself: an UNSANITIZED join really
        // does escape, so the assertions above are capable of failing.
        let unsanitized = dir.join("../../etc/passwd");
        assert!(
            unsanitized
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
            "the escape check is vacuous — a raw join must show ParentDir"
        );
    }

    #[test]
    fn save_load_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut l = CacheLedger::new("sess-rt");
        l.record_turn(sample(1, 1_000, 0, 500));
        l.mark_complete();
        let path = ledger_path(tmp.path(), &l.session_id);
        save(&l, &path).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn load_refuses_unknown_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("bad.json");
        let mut v = serde_json::to_value(CacheLedger::new("s")).unwrap();
        v["schema"] = serde_json::json!(LEDGER_SCHEMA + 1);
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(matches!(
            load(&path),
            Err(LedgerError::SchemaMismatch { .. })
        ));
    }

    /// A ledger written by v0.13.9 or earlier, in the shape `git show v0.13.9`
    /// confirms: schema 1, `uncached_equivalent_usd` a bare `0.0` because
    /// `flux-reasoning` has no catalog row. Read back at schema 1 this
    /// reproduced #1163 character-for-character on the FIXED build —
    /// `saving_usd=-0.061389`, and now additionally `saving_truth=priced`,
    /// a confidence the pre-fix build never claimed.
    fn legacy_v1_ledger_json(cost_usd: f64) -> serde_json::Value {
        let mut v = serde_json::to_value(CacheLedger::new("aa55aa55-0002")).unwrap();
        v["schema"] = serde_json::json!(1);
        let mut turn = serde_json::to_value(sample(1, 0, 7232, 0)).unwrap();
        turn["cost_usd"] = serde_json::json!(cost_usd);
        turn["cost_source"] = serde_json::json!("provider_reported");
        // v1 wrote the field unconditionally, as a bare f64.
        turn["uncached_equivalent_usd"] = serde_json::json!(0.0);
        v["turns"] = serde_json::json!([turn]);
        v
    }

    #[test]
    fn a_legacy_zero_counterfactual_does_not_read_back_as_a_priced_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("legacy.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&legacy_v1_ledger_json(0.061389)).unwrap(),
        )
        .unwrap();

        let back = load(&path).expect("a v1 ledger must still be readable");
        assert_eq!(
            back.turns[0].uncached_equivalent_usd, None,
            "v1 wrote 0.0 to mean `nothing could price this`; decoding it as \
             Some(0.0) is the fabricated baseline #1163 is about"
        );

        let s = back.summarize();
        assert_eq!(
            s.cache_saving_usd(),
            None,
            "the saving must render as unknown, not as a negative number \
             against a baseline nobody computed"
        );
        assert_eq!(s.counterfactual_unpriced_round_trips, 1);
        assert_ne!(
            s.saving_truth(),
            CostTruth::Priced,
            "a saving computed against an unknown counterfactual must not be \
             graded priced"
        );
    }

    /// The migration must not invent unknowns where the writer knew something.
    /// A v1 row with a real, non-zero counterfactual keeps it, so an operator
    /// upgrading does not lose the savings they could already see.
    #[test]
    fn a_legacy_priced_counterfactual_survives_the_migration() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("legacy-priced.json");
        let mut v = legacy_v1_ledger_json(0.02);
        v["turns"][0]["uncached_equivalent_usd"] = serde_json::json!(0.05);
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();

        let back = load(&path).unwrap();
        assert_eq!(back.turns[0].uncached_equivalent_usd, Some(0.05));
        let s = back.summarize();
        assert!((s.cache_saving_usd().expect("priced") - 0.03).abs() < 1e-9);
        assert_eq!(s.counterfactual_unpriced_round_trips, 0);
    }

    /// The version is what makes the two zeros distinguishable, so a build
    /// that writes the new meaning must not stamp the old version.
    #[test]
    fn a_new_ledger_is_stamped_with_the_schema_that_carries_the_new_meaning() {
        // Read the stamp off a ledger this build actually constructs, rather
        // than off the constant: `assert!(LEDGER_SCHEMA > 1)` is a constant
        // expression and `clippy::assertions_on_constants` (denied in CI)
        // refuses it -- and the interesting claim is about what gets WRITTEN
        // anyway.
        let stamped = CacheLedger::new("s").schema;
        assert_eq!(stamped, LEDGER_SCHEMA);
        assert!(
            stamped > 1,
            "uncached_equivalent_usd changed meaning in place; the module's \
             own rule is that the version is bumped when it does"
        );
    }

    /// #1166 ticket Defect 5 — "off by default". Only the CHAT-VISIBLE half is,
    /// and that is deliberate.
    ///
    /// `compact.cache_diagnostics` defaults to `false` and stays there: it
    /// gates the three `emit_info` lines that print a cache verdict into the
    /// conversation, which #101 filed as alarming users over normal behaviour
    /// (a `TtlExpiry` after a pause is expected). What must NOT sit behind it
    /// are the two DETECTING surfaces — the `cache_health_warn` tracing event
    /// (the CLI's default filter is `info`, so a `warn!` is emitted) and the
    /// ledger record (`recording_enabled` is on unless the env kill-switch is
    /// set). A default install therefore still detects, still attributes and
    /// still persists the verdict; it just does not interrupt the chat.
    ///
    /// A source lint rather than a runtime assertion because the property is
    /// positional: it is about which side of a brace an emission sits on.
    #[test]
    fn a_default_install_still_detects_and_records_a_cache_break() {
        const ENGINE: &str = include_str!("engine.rs");
        const GATE: &str = "if self.compact_config.cache_diagnostics {";

        let gates: Vec<usize> = ENGINE.match_indices(GATE).map(|(i, _)| i).collect();
        assert!(
            !gates.is_empty(),
            "the `{GATE}` gate is not in engine.rs at all -- this lint can no \
             longer say anything about what is behind it"
        );

        let bytes = ENGINE.as_bytes();
        let mut gated: Vec<&str> = Vec::new();
        for start in gates {
            let open = start + GATE.len() - 1;
            let mut depth = 0usize;
            let mut end = open;
            for (i, b) in bytes.iter().enumerate().skip(open) {
                match b {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            gated.push(&ENGINE[open..=end]);
        }

        // Controls on the extraction itself: a runaway brace match would
        // swallow the rest of the file and make every probe below pass for the
        // wrong reason, and an empty body would make them pass vacuously.
        for body in &gated {
            assert!(
                body.len() > 20 && body.len() < 1_000,
                "brace matching produced a {}-byte block; the extraction is \
                 wrong, so its verdict means nothing",
                body.len()
            );
            assert!(
                body.contains("emit_info"),
                "cache_diagnostics gates the chat-visible emit_info lines and \
                 nothing else; found a gated block that emits nothing: {body}"
            );
        }

        for probe in ["cache_health_warn", "recording_enabled"] {
            assert!(
                ENGINE.contains(probe),
                "{probe} is not in engine.rs at all -- this lint is asking \
                 about something that no longer exists"
            );
            assert!(
                gated.iter().all(|body| !body.contains(probe)),
                "{probe} moved inside `{GATE}`. A default install would stop \
                 detecting the break, which is ticket Defect 5 of #1166"
            );
        }
    }

    #[test]
    fn list_skips_malformed_and_orders_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let mut old = CacheLedger::new("old");
        old.updated_at = "2026-07-01T00:00:00.000Z".into();
        let mut new = CacheLedger::new("new");
        new.updated_at = "2026-07-29T00:00:00.000Z".into();
        save(&old, &ledger_path(tmp.path(), "old")).unwrap();
        save(&new, &ledger_path(tmp.path(), "new")).unwrap();
        std::fs::write(tmp.path().join("junk.json"), b"{not json").unwrap();

        let listed = list(tmp.path()).unwrap();
        assert_eq!(
            listed.len(),
            2,
            "the malformed file must be skipped, not fatal"
        );
        assert_eq!(listed[0].1.session_id, "new");
        assert_eq!(latest(tmp.path()).unwrap().1.session_id, "new");
    }

    #[test]
    fn latest_on_empty_dir_is_an_error_not_a_zero_report() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(latest(tmp.path()), Err(LedgerError::Empty(_))));
    }
}
