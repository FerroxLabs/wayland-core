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
//! [`TurnSample::cost_priced`] carries `wcore_pricing`'s `PriceStatus.priced`
//! flag. A `$0.00` from an unpriced model and a `$0.00` from a genuinely free
//! model are **different facts**, and a ledger that renders both as `0.00`
//! is worse than no ledger. [`LedgerSummary::cost_truth`] therefore grades the
//! session's cost as [`CostTruth::Priced`], [`CostTruth::Partial`] or
//! [`CostTruth::Unpriced`], and `cache verify` exits non-zero on the latter two.
//!
//! The second half of cost truth is the counterfactual: the ledger records both
//! what the session **was** billed and what the same tokens **would have** been
//! billed with no cache at all ([`TurnSample::uncached_equivalent_usd`]). The
//! difference is signed on purpose — a session that writes cache it never reads
//! back costs MORE than an uncached one, and that must be reportable as a
//! negative saving rather than clamped to zero.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use wcore_providers::cache_observation::{
    CacheRetention, InvalidationCause, PromptCacheObservation,
};

use crate::cache_diagnostics::{CacheBreakCause, CacheDiagnostic};

/// On-disk schema version. Bumped when a field's meaning changes; readers
/// refuse a version they do not understand rather than silently mis-reporting.
pub const LEDGER_SCHEMA: u32 = 1;

/// Directory name under the Wayland home holding one ledger per session.
pub const LEDGER_DIR: &str = "cache-ledger";

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
    /// `false` means the catalog could not price this provider×model. The
    /// USD figure is then a floor, not a fact.
    pub cost_priced: bool,
    /// What the SAME tokens would have cost with no cache: every cached token
    /// re-billed at the ordinary input rate.
    pub uncached_equivalent_usd: f64,

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
    pub fn cache_saving_usd(&self) -> f64 {
        self.uncached_equivalent_usd - self.cost_usd
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
    pub turn: u64,
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

    /// Was a compaction recorded at or after `turn`? Used to attribute a cache
    /// miss to [`InvalidationCause::HistoryRewritten`] rather than to whatever
    /// the prompt/tool-hash comparison guessed. Compaction rewrites the message
    /// history wholesale, so the NEXT round-trip cannot possibly hit — and
    /// attributing that to "system prompt drift" would be a lie about a
    /// self-inflicted miss.
    pub fn compacted_since_turn(&self, turn: u64) -> bool {
        self.compactions.iter().any(|c| c.turn >= turn)
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
        for t in &self.turns {
            s.uncached_input_tokens = s
                .uncached_input_tokens
                .saturating_add(t.uncached_input_tokens);
            s.cache_read_tokens = s.cache_read_tokens.saturating_add(t.cache_read_tokens);
            s.cache_write_tokens = s.cache_write_tokens.saturating_add(t.cache_write_tokens);
            s.output_tokens = s.output_tokens.saturating_add(t.output_tokens);
            s.cost_usd += t.cost_usd;
            s.uncached_equivalent_usd += t.uncached_equivalent_usd;
            if t.cost_priced {
                s.priced_round_trips += 1;
            } else {
                s.unpriced_round_trips += 1;
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
    /// Every round-trip priced from the catalog. The USD figure is a fact.
    Priced,
    /// Some round-trips are unpriced. The USD figure is a **floor**.
    Partial,
    /// No round-trip could be priced (or there are none). The USD figure
    /// carries no information and must not be presented as spend.
    Unpriced,
}

impl CostTruth {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Priced => "priced",
            Self::Partial => "partial",
            Self::Unpriced => "unpriced",
        }
    }

    /// Can an operator trust the USD number as spend?
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
    pub uncached_equivalent_usd: f64,
    pub priced_round_trips: u64,
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
    pub fn cache_saving_usd(&self) -> f64 {
        self.uncached_equivalent_usd - self.cost_usd
    }

    /// Saving as a fraction of the uncached counterfactual. `0.0` when the
    /// counterfactual is zero (nothing to save against).
    pub fn cache_saving_ratio(&self) -> f64 {
        if self.uncached_equivalent_usd == 0.0 {
            return 0.0;
        }
        self.cache_saving_usd() / self.uncached_equivalent_usd
    }

    /// Peak context pressure as a fraction of the autocompact threshold.
    pub fn peak_pressure_ratio(&self) -> f64 {
        if self.autocompact_threshold_tokens == 0 {
            return 0.0;
        }
        self.peak_watermark_tokens as f64 / self.autocompact_threshold_tokens as f64
    }

    /// Grade the trustworthiness of [`Self::cost_usd`].
    pub fn cost_truth(&self) -> CostTruth {
        if self.priced_round_trips == 0 {
            CostTruth::Unpriced
        } else if self.unpriced_round_trips > 0 {
            CostTruth::Partial
        } else {
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

/// Read one ledger, refusing an unknown schema.
pub fn load(path: &Path) -> Result<CacheLedger, LedgerError> {
    let raw = std::fs::read(path).map_err(|source| LedgerError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let ledger: CacheLedger =
        serde_json::from_slice(&raw).map_err(|source| LedgerError::Malformed {
            path: path.to_path_buf(),
            source,
        })?;
    if ledger.schema != LEDGER_SCHEMA {
        return Err(LedgerError::SchemaMismatch {
            path: path.to_path_buf(),
            found: ledger.schema,
            expected: LEDGER_SCHEMA,
        });
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
            cost_priced: true,
            uncached_equivalent_usd: 0.02,
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
        s.uncached_equivalent_usd = 0.04;
        assert!(
            s.cache_saving_usd() < 0.0,
            "a write-heavy turn that never read back must report a NEGATIVE saving, got {}",
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
        assert!((s.cache_saving_usd() - 0.03).abs() < 1e-9);
        assert!((s.cache_saving_ratio() - 0.5).abs() < 1e-9);
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
        unpriced.cost_priced = false;
        unpriced.cost_usd = 0.0;
        l.record_turn(unpriced);
        let s = l.summarize();
        assert_eq!(s.cost_truth(), CostTruth::Partial);
        assert!(!s.cost_truth().is_trustworthy());
        assert_eq!(s.unpriced_round_trips, 1);
    }

    #[test]
    fn cost_truth_unpriced_when_nothing_priced() {
        let mut l = CacheLedger::new("sess-unpriced");
        let mut t = sample(1, 1_000, 0, 0);
        t.cost_priced = false;
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
    fn compaction_since_turn_detects_history_rewrite() {
        let mut l = CacheLedger::new("sess-compact");
        assert!(!l.compacted_since_turn(0));
        l.record_compaction(CompactionEvent {
            turn: 4,
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
        assert!(l.compacted_since_turn(4));
        assert!(!l.compacted_since_turn(5));
    }

    #[test]
    fn summary_counts_failed_compactions_separately() {
        let mut l = CacheLedger::new("sess-fail");
        l.record_compaction(CompactionEvent {
            turn: 1,
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
        let dir = Path::new("/tmp/ledgers");
        let p = ledger_path(dir, "../../etc/passwd");
        assert_eq!(
            p.parent().unwrap(),
            dir,
            "traversal escaped: {}",
            p.display()
        );
        assert!(!p.to_string_lossy().contains(".."), "{}", p.display());
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
