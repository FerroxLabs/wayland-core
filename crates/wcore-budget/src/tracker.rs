//! M5.3 — session-keyed / user-keyed budget tracker.
//!
//! `BudgetCap` is built via `BudgetCap::builder()`. The tracker accumulates
//! `(tokens, usd)` per session and emits `BudgetEvent::{Charge, CapWarn,
//! CapBlock}` to the attached event sink (wired via
//! `wcore-observability::ObservabilityBudgetEventBridge` in production —
//! the bridge mirrors the M3.3 memory-trace pattern).
//!
//! Two enforcement axes:
//!
//! 1. **Per-session** caps (`per_session_tokens`, `per_session_usd`) — the
//!    third argument identifies the session. A given session reaching its
//!    cap blocks further charges; a different session id keeps charging.
//! 2. **Per-user daily** cap (`per_user_daily_usd`) — `charge_for_user`
//!    additionally rolls each charge into a per-user daily bucket keyed by
//!    `(user_id, calendar_day_utc)`. Crossing the daily cap blocks further
//!    charges from that user until the next UTC day.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Datelike, Utc};
use thiserror::Error;

/// Caps for the session-keyed / user-keyed tracker. None on every field
/// means "no cap" — the tracker accumulates totals for observability but
/// every charge succeeds.
#[derive(Debug, Clone, Default)]
pub struct BudgetCap {
    pub per_session_tokens: Option<u64>,
    pub per_session_usd: Option<f64>,
    pub per_user_daily_usd: Option<f64>,
}

impl BudgetCap {
    pub fn builder() -> BudgetCapBuilder {
        BudgetCapBuilder::default()
    }
}

/// M5.bootstrap-wiring — translate a `[session_cap]` TOML block into a
/// `BudgetCap`. The TOML schema (`BudgetConfig`) carries seven optional
/// cap fields; only the three this tracker enforces map across:
///
/// - `max_tokens_in + max_tokens_out` (if either present) → `per_session_tokens`
///   (sum-of-direction; the tracker counts charges as a single token total
///   per turn, so wiring "either direction" here is the closest TOML-side
///   semantics without inventing a new schema).
/// - `max_cost_usd` → `per_session_usd`
/// - The wall-time / tool-runtime / processes / agent-depth fields belong
///   to the W8a `ExecutionBudget` tree and have no counterpart here; they
///   are ignored by this conversion (the existing `ExecutionBudget::from(
///   &BudgetConfig)` impl in `wcore-budget::execution` keeps consuming
///   them).
/// - `per_user_daily_usd` has no TOML counterpart today — set it manually
///   via the builder if needed (e.g. multi-tenant deployments).
impl From<&crate::BudgetConfig> for BudgetCap {
    fn from(cfg: &crate::BudgetConfig) -> Self {
        let mut b = BudgetCap::builder();
        let sum_tokens = match (cfg.max_tokens_in, cfg.max_tokens_out) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        if let Some(t) = sum_tokens {
            b = b.per_session_tokens(t);
        }
        if let Some(usd) = cfg.max_cost_usd {
            b = b.per_session_usd(usd);
        }
        b.build()
    }
}

#[derive(Debug, Default, Clone)]
pub struct BudgetCapBuilder {
    cap: BudgetCap,
}

impl BudgetCapBuilder {
    pub fn per_session_tokens(mut self, n: u64) -> Self {
        self.cap.per_session_tokens = Some(n);
        self
    }
    pub fn per_session_usd(mut self, usd: f64) -> Self {
        self.cap.per_session_usd = Some(usd);
        self
    }
    pub fn per_user_daily_usd(mut self, usd: f64) -> Self {
        self.cap.per_user_daily_usd = Some(usd);
        self
    }
    pub fn build(self) -> BudgetCap {
        self.cap
    }
}

/// Errors raised by `BudgetTracker::charge`.
#[derive(Debug, Clone, Error, serde::Serialize)]
pub enum BudgetError {
    /// A configured cap was exceeded by the charge under attempt.
    #[error("budget cap '{kind}' exceeded: limit={limit}, observed={observed}")]
    CapExceeded {
        /// Cap that tripped: `per_session_tokens`, `per_session_usd`,
        /// or `per_user_daily_usd`.
        kind: String,
        /// Configured limit formatted for display (e.g. `"$0.10"`,
        /// `"1000 tokens"`).
        limit: String,
        /// Total post-charge that crossed the limit, formatted for
        /// display.
        observed: String,
    },
}

/// Observability event emitted by `BudgetTracker` on every charge attempt.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BudgetEvent {
    /// Successful charge — emitted on every accepted charge.
    Charge {
        session_id: String,
        tokens: u64,
        usd: f64,
    },
    /// Charge accepted but the running total is ≥80% of the strictest
    /// configured cap on this session.
    CapWarn { session_id: String, pct_used: f32 },
    /// Charge rejected because it would exceed a cap.
    CapBlock {
        session_id: String,
        reason: BudgetError,
    },
}

/// Sink for `BudgetEvent`. Implementations forward to whichever telemetry
/// channel the host wires up (`ObservabilityBudgetEventBridge` in
/// production). Sink calls happen synchronously on the charge hot path —
/// implementations MUST NOT block.
pub trait BudgetEventSink: Send + Sync {
    fn emit(&self, event: &BudgetEvent);
}

#[derive(Debug, Default, Clone, Copy)]
struct SessionTotals {
    tokens: u64,
    usd: f64,
}

#[derive(Debug, Clone, Copy)]
struct DailyTotals {
    /// Year-month-day in UTC (chrono `NaiveDate::num_days_from_ce` is
    /// stable across timezone boundary changes).
    day_ordinal: i32,
    usd: f64,
}

pub struct BudgetTracker {
    caps: BudgetCap,
    per_session: HashMap<String, SessionTotals>,
    per_user_daily: HashMap<String, DailyTotals>,
    sink: Option<Arc<dyn BudgetEventSink>>,
}

impl BudgetTracker {
    pub fn new(caps: BudgetCap) -> Self {
        Self {
            caps,
            per_session: HashMap::new(),
            per_user_daily: HashMap::new(),
            sink: None,
        }
    }

    /// Install an observability sink. Calls emit synchronously on the
    /// charge hot path.
    pub fn set_event_sink(&mut self, sink: Arc<dyn BudgetEventSink>) {
        self.sink = Some(sink);
    }

    /// Record `(tokens, usd)` against `session_id`. Returns `Err` if the
    /// charge would exceed a per-session cap; in that case the running
    /// totals are NOT incremented (the rejected charge does not "stick").
    pub fn charge(&mut self, session_id: &str, tokens: u64, usd: f64) -> Result<(), BudgetError> {
        let entry = self.per_session.entry(session_id.to_string()).or_default();
        let next_tokens = entry.tokens.saturating_add(tokens);
        let next_usd = entry.usd + usd;

        if let Some(cap) = self.caps.per_session_tokens
            && next_tokens > cap
        {
            let err = BudgetError::CapExceeded {
                kind: "per_session_tokens".to_string(),
                limit: format!("{cap} tokens"),
                observed: format!("{next_tokens} tokens"),
            };
            self.emit(BudgetEvent::CapBlock {
                session_id: session_id.to_string(),
                reason: err.clone(),
            });
            return Err(err);
        }
        if let Some(cap) = self.caps.per_session_usd
            && next_usd > cap
        {
            let err = BudgetError::CapExceeded {
                kind: "per_session_usd".to_string(),
                limit: format!("${cap:.4}"),
                observed: format!("${next_usd:.4}"),
            };
            self.emit(BudgetEvent::CapBlock {
                session_id: session_id.to_string(),
                reason: err.clone(),
            });
            return Err(err);
        }

        // Charge accepted — commit.
        entry.tokens = next_tokens;
        entry.usd = next_usd;

        self.emit(BudgetEvent::Charge {
            session_id: session_id.to_string(),
            tokens,
            usd,
        });

        if let Some(pct) = self.pct_used_strictest(session_id)
            && pct >= 0.80
        {
            self.emit(BudgetEvent::CapWarn {
                session_id: session_id.to_string(),
                pct_used: pct,
            });
        }
        Ok(())
    }

    /// Record `(tokens, usd)` against `session_id` AND against the
    /// per-user daily UTC bucket for `user_id`. If either the per-session
    /// or the per-user-daily cap is exceeded, the charge is rejected and
    /// neither bucket is incremented.
    pub fn charge_for_user(
        &mut self,
        session_id: &str,
        user_id: &str,
        tokens: u64,
        usd: f64,
    ) -> Result<(), BudgetError> {
        self.charge_for_user_at(session_id, user_id, tokens, usd, Utc::now())
    }

    /// Test/observability-friendly form of `charge_for_user` that pins the
    /// wall clock. Production callers should use `charge_for_user`.
    pub fn charge_for_user_at(
        &mut self,
        session_id: &str,
        user_id: &str,
        tokens: u64,
        usd: f64,
        now: DateTime<Utc>,
    ) -> Result<(), BudgetError> {
        let today_ord = now.date_naive().num_days_from_ce();

        // Compute the prospective per-user-daily total *before* mutating
        // either bucket so a rejected charge leaves both at the prior
        // totals.
        let prior_daily = self.per_user_daily.get(user_id).copied();
        let next_daily_usd = match prior_daily {
            Some(d) if d.day_ordinal == today_ord => d.usd + usd,
            _ => usd, // new day → reset bucket
        };

        if let Some(cap) = self.caps.per_user_daily_usd
            && next_daily_usd > cap
        {
            let err = BudgetError::CapExceeded {
                kind: "per_user_daily_usd".to_string(),
                limit: format!("${cap:.4}"),
                observed: format!("${next_daily_usd:.4}"),
            };
            self.emit(BudgetEvent::CapBlock {
                session_id: session_id.to_string(),
                reason: err.clone(),
            });
            return Err(err);
        }

        // Per-session check happens through `charge` so a rejection
        // there also doesn't mutate the daily bucket.
        self.charge(session_id, tokens, usd)?;

        // Commit daily bucket.
        self.per_user_daily.insert(
            user_id.to_string(),
            DailyTotals {
                day_ordinal: today_ord,
                usd: next_daily_usd,
            },
        );
        Ok(())
    }

    /// Snapshot of `(tokens, usd)` charged so far to `session_id`.
    pub fn session_totals(&self, session_id: &str) -> (u64, f64) {
        self.per_session
            .get(session_id)
            .map(|s| (s.tokens, s.usd))
            .unwrap_or((0, 0.0))
    }

    /// Today-UTC USD charged so far for `user_id` (returns `0.0` if no
    /// charges today or `user_id` unseen).
    pub fn user_daily_usd(&self, user_id: &str) -> f64 {
        let today = Utc::now().date_naive().num_days_from_ce();
        self.per_user_daily
            .get(user_id)
            .filter(|d| d.day_ordinal == today)
            .map(|d| d.usd)
            .unwrap_or(0.0)
    }

    fn emit(&self, event: BudgetEvent) {
        if let Some(sink) = self.sink.as_ref() {
            sink.emit(&event);
        }
    }

    /// Highest pct-used across the configured per-session caps for
    /// `session_id`. Returns `None` if no per-session cap is configured.
    fn pct_used_strictest(&self, session_id: &str) -> Option<f32> {
        let entry = self.per_session.get(session_id)?;
        let token_pct = self
            .caps
            .per_session_tokens
            .map(|cap| entry.tokens as f32 / cap as f32);
        let usd_pct = self
            .caps
            .per_session_usd
            .map(|cap| (entry.usd / cap) as f32);
        match (token_pct, usd_pct) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct CollectingSink {
        events: Mutex<Vec<BudgetEvent>>,
    }
    impl BudgetEventSink for CollectingSink {
        fn emit(&self, event: &BudgetEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    #[test]
    fn empty_caps_never_block() {
        let mut t = BudgetTracker::new(BudgetCap::default());
        for _ in 0..10 {
            t.charge("s1", 1_000_000, 100.0).unwrap();
        }
        assert_eq!(t.session_totals("s1").0, 10_000_000);
    }

    #[test]
    fn token_cap_blocks_overrun() {
        let cap = BudgetCap::builder().per_session_tokens(1500).build();
        let mut t = BudgetTracker::new(cap);
        t.charge("s1", 1000, 0.0).unwrap();
        let err = t.charge("s1", 600, 0.0).unwrap_err();
        assert!(
            matches!(err, BudgetError::CapExceeded { ref kind, .. } if kind == "per_session_tokens")
        );
        // Rejected charge must not stick.
        assert_eq!(t.session_totals("s1").0, 1000);
    }

    #[test]
    fn separate_sessions_have_separate_buckets() {
        let cap = BudgetCap::builder().per_session_usd(0.10).build();
        let mut t = BudgetTracker::new(cap);
        t.charge("s1", 0, 0.09).unwrap();
        // s2 starts fresh — must succeed.
        t.charge("s2", 0, 0.09).unwrap();
        // s1 cannot overrun.
        let err = t.charge("s1", 0, 0.05).unwrap_err();
        assert!(matches!(err, BudgetError::CapExceeded { .. }));
    }

    #[test]
    fn charge_emits_event() {
        let sink = Arc::new(CollectingSink::default());
        let mut t = BudgetTracker::new(BudgetCap::default());
        t.set_event_sink(sink.clone());
        t.charge("s1", 100, 0.01).unwrap();
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], BudgetEvent::Charge { tokens: 100, .. }));
    }

    #[test]
    fn cap_warn_emits_above_80pct() {
        let sink = Arc::new(CollectingSink::default());
        let cap = BudgetCap::builder().per_session_usd(0.10).build();
        let mut t = BudgetTracker::new(cap);
        t.set_event_sink(sink.clone());
        // 90% of cap → warn must fire.
        t.charge("s1", 0, 0.09).unwrap();
        let events = sink.events.lock().unwrap();
        let warns: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, BudgetEvent::CapWarn { .. }))
            .collect();
        assert_eq!(warns.len(), 1, "expected one CapWarn, got {events:?}");
    }

    #[test]
    fn cap_block_emits_on_rejection() {
        let sink = Arc::new(CollectingSink::default());
        let cap = BudgetCap::builder().per_session_usd(0.05).build();
        let mut t = BudgetTracker::new(cap);
        t.set_event_sink(sink.clone());
        let _ = t.charge("s1", 0, 0.10);
        let events = sink.events.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, BudgetEvent::CapBlock { .. })),
            "expected a CapBlock event, got {events:?}"
        );
    }

    #[test]
    fn per_user_daily_cap_blocks_after_threshold() {
        let cap = BudgetCap::builder().per_user_daily_usd(0.10).build();
        let mut t = BudgetTracker::new(cap);
        let now = Utc::now();
        t.charge_for_user_at("sA", "alice", 0, 0.05, now).unwrap();
        t.charge_for_user_at("sB", "alice", 0, 0.04, now).unwrap();
        let err = t
            .charge_for_user_at("sC", "alice", 0, 0.02, now)
            .unwrap_err();
        assert!(
            matches!(err, BudgetError::CapExceeded { ref kind, .. } if kind == "per_user_daily_usd")
        );
    }

    #[test]
    fn per_user_daily_cap_resets_next_day() {
        let cap = BudgetCap::builder().per_user_daily_usd(0.10).build();
        let mut t = BudgetTracker::new(cap);
        let today = Utc::now();
        let tomorrow = today + chrono::Duration::days(1);
        t.charge_for_user_at("s1", "alice", 0, 0.09, today).unwrap();
        // Same-day overrun → blocked.
        assert!(t.charge_for_user_at("s1", "alice", 0, 0.05, today).is_err());
        // Next day → fresh bucket.
        t.charge_for_user_at("s1", "alice", 0, 0.09, tomorrow)
            .unwrap();
    }

    #[test]
    fn per_user_block_does_not_touch_session_bucket() {
        let cap = BudgetCap::builder().per_user_daily_usd(0.05).build();
        let mut t = BudgetTracker::new(cap);
        let now = Utc::now();
        let _ = t.charge_for_user_at("s1", "alice", 0, 0.10, now);
        // Per-user cap rejected the charge → session bucket must be 0.
        assert_eq!(t.session_totals("s1").1, 0.0);
    }
}
