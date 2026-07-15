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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Datelike, Utc};
use thiserror::Error;

/// Caps for the session-keyed / user-keyed tracker. None on every field
/// means "no cap" — the tracker accumulates totals for observability but
/// every charge succeeds.
#[derive(Debug, Clone, Default)]
pub struct BudgetCap {
    pub per_session_tokens: Option<u64>,
    pub per_session_input_tokens: Option<u64>,
    pub per_session_output_tokens: Option<u64>,
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
/// - `max_tokens_in` and `max_tokens_out` retain their independent directions;
///   their saturating sum also forms the legacy aggregate ceiling.
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
        if let Some(t) = cfg.max_tokens_in {
            b = b.per_session_input_tokens(t);
        }
        if let Some(t) = cfg.max_tokens_out {
            b = b.per_session_output_tokens(t);
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
    pub fn per_session_input_tokens(mut self, n: u64) -> Self {
        self.cap.per_session_input_tokens = Some(n);
        self
    }
    pub fn per_session_output_tokens(mut self, n: u64) -> Self {
        self.cap.per_session_output_tokens = Some(n);
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
        /// `per_user_daily_usd`, or `per_user_daily_identity_required`.
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
    input_tokens: u64,
    output_tokens: u64,
    usd: f64,
}

#[derive(Debug, Default, Clone, Copy)]
struct ReservedTotals {
    tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    usd: f64,
}

#[derive(Debug, Default, Clone, Copy)]
struct SessionExtension {
    tokens: u64,
    usd: f64,
}

#[derive(Debug, Clone)]
struct ReservationEntry {
    session_id: String,
    tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    usd: f64,
}

/// Opaque admission reservation returned before a provider call starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub struct BudgetReservation(u64);

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
    reserved_per_session: HashMap<String, ReservedTotals>,
    reservations: HashMap<u64, ReservationEntry>,
    next_reservation_id: u64,
    session_extensions: HashMap<String, SessionExtension>,
    /// Sessions with a provider admission/settlement cap receipt outstanding.
    /// An extension consumes this latch; arbitrary pre-emptive widening is not
    /// a valid Continue operation.
    blocked_sessions: HashSet<String>,
    per_user_daily: HashMap<String, DailyTotals>,
    sink: Option<Arc<dyn BudgetEventSink>>,
}

impl BudgetTracker {
    pub fn new(caps: BudgetCap) -> Self {
        let mut caps = caps;
        for cap in [&mut caps.per_session_usd, &mut caps.per_user_daily_usd] {
            if cap.is_some_and(|usd| !usd.is_finite() || usd < 0.0) {
                *cap = Some(0.0);
            }
        }
        Self {
            caps,
            per_session: HashMap::new(),
            reserved_per_session: HashMap::new(),
            reservations: HashMap::new(),
            next_reservation_id: 1,
            session_extensions: HashMap::new(),
            blocked_sessions: HashSet::new(),
            per_user_daily: HashMap::new(),
            sink: None,
        }
    }

    /// Install an observability sink. Calls emit synchronously on the
    /// charge hot path.
    pub fn set_event_sink(&mut self, sink: Arc<dyn BudgetEventSink>) {
        self.sink = Some(sink);
    }

    /// Add operator-authorized headroom to one session without widening any
    /// other session or the per-user daily ceiling.
    pub fn extend_session(
        &mut self,
        session_id: &str,
        additional_tokens: u64,
        additional_usd: f64,
    ) -> Result<(), BudgetError> {
        if !self.blocked_sessions.contains(session_id) {
            return Err(self.cap_block(
                session_id,
                "no_exhausted_budget",
                "an outstanding budget-exceeded receipt".to_string(),
                "none".to_string(),
            ));
        }
        if !additional_usd.is_finite() || additional_usd < 0.0 {
            return Err(self.cap_block(
                session_id,
                "invalid_usd",
                "finite non-negative USD".to_string(),
                additional_usd.to_string(),
            ));
        }
        if additional_tokens == 0 && additional_usd == 0.0 {
            return Err(self.cap_block(
                session_id,
                "empty_extension",
                "positive tokens or USD".to_string(),
                "zero".to_string(),
            ));
        }
        let current_extension_usd = self
            .session_extensions
            .get(session_id)
            .map(|extension| extension.usd)
            .unwrap_or(0.0);
        let Some(next_extension_usd) = checked_usd_add(current_extension_usd, additional_usd)
        else {
            return Err(self.cap_block(
                session_id,
                "invalid_usd",
                "finite representable USD extension".to_string(),
                format!("{current_extension_usd} + {additional_usd}"),
            ));
        };
        if let Some(base_cap) = self.caps.per_session_usd
            && checked_usd_add(base_cap, next_extension_usd).is_none()
        {
            return Err(self.cap_block(
                session_id,
                "invalid_usd",
                "finite representable effective USD cap".to_string(),
                format!("{base_cap} + {next_extension_usd}"),
            ));
        }
        let extension = self
            .session_extensions
            .entry(session_id.to_string())
            .or_default();
        extension.tokens = extension.tokens.saturating_add(additional_tokens);
        extension.usd = next_extension_usd;
        self.blocked_sessions.remove(session_id);
        Ok(())
    }

    fn session_token_cap(&self, session_id: &str) -> Option<u64> {
        self.caps.per_session_tokens.map(|cap| {
            cap.saturating_add(
                self.session_extensions
                    .get(session_id)
                    .map(|extension| extension.tokens)
                    .unwrap_or(0),
            )
        })
    }

    fn session_usd_cap(&self, session_id: &str) -> Option<f64> {
        self.caps.per_session_usd.map(|cap| {
            cap + self
                .session_extensions
                .get(session_id)
                .map(|extension| extension.usd)
                .unwrap_or(0.0)
        })
    }

    fn session_input_token_cap(&self, session_id: &str) -> Option<u64> {
        self.caps.per_session_input_tokens.map(|cap| {
            cap.saturating_add(
                self.session_extensions
                    .get(session_id)
                    .map(|extension| extension.tokens)
                    .unwrap_or(0),
            )
        })
    }

    fn session_output_token_cap(&self, session_id: &str) -> Option<u64> {
        self.caps.per_session_output_tokens.map(|cap| {
            cap.saturating_add(
                self.session_extensions
                    .get(session_id)
                    .map(|extension| extension.tokens)
                    .unwrap_or(0),
            )
        })
    }

    /// Whether this session is governed by a monetary ceiling, including an
    /// operator-authorized extension. Callers use this to reject an unpriceable
    /// provider call before it can masquerade as a zero-dollar reservation.
    pub fn has_session_usd_cap(&self, session_id: &str) -> bool {
        self.session_usd_cap(session_id).is_some()
    }

    /// Reserve worst-case tokens and USD before starting a paid call. Both
    /// committed usage and other in-flight reservations participate in the
    /// admission decision, so concurrent calls cannot each claim the same
    /// remaining budget.
    ///
    /// This session-only API fails closed when a per-user daily cap is active:
    /// without a user identity it cannot safely debit that cap. Callers that
    /// need daily enforcement must use a user-aware charging path.
    pub fn reserve(
        &mut self,
        session_id: &str,
        tokens: u64,
        usd: f64,
    ) -> Result<BudgetReservation, BudgetError> {
        self.reserve_turn(session_id, tokens, 0, usd)
    }

    /// Direction-aware provider admission. Cache-read and cache-creation
    /// tokens belong in `input_tokens`; callers must pass disjoint counters.
    pub fn reserve_turn(
        &mut self,
        session_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        usd: f64,
    ) -> Result<BudgetReservation, BudgetError> {
        if self.blocked_sessions.contains(session_id) {
            return Err(self.cap_block(
                session_id,
                "budget_extension_required",
                "an operator-authorized session extension".to_string(),
                "session remains blocked after budget exhaustion".to_string(),
            ));
        }
        if let Some(cap) = self.caps.per_user_daily_usd {
            return Err(self.cap_block(
                session_id,
                "per_user_daily_identity_required",
                format!("${cap:.4} per user per day"),
                "user identity unavailable for reservation".to_string(),
            ));
        }
        let committed = self
            .per_session
            .get(session_id)
            .copied()
            .unwrap_or_default();
        let reserved = self
            .reserved_per_session
            .get(session_id)
            .copied()
            .unwrap_or_default();
        let tokens = input_tokens.saturating_add(output_tokens);
        let next_tokens = committed
            .tokens
            .saturating_add(reserved.tokens)
            .saturating_add(tokens);
        let next_usd = committed.usd + reserved.usd + usd;
        let next_input_tokens = committed
            .input_tokens
            .saturating_add(reserved.input_tokens)
            .saturating_add(input_tokens);
        let next_output_tokens = committed
            .output_tokens
            .saturating_add(reserved.output_tokens)
            .saturating_add(output_tokens);

        if let Some(cap) = self.session_token_cap(session_id)
            && next_tokens > cap
        {
            self.blocked_sessions.insert(session_id.to_string());
            return Err(self.cap_block(
                session_id,
                "per_session_tokens",
                format!("{cap} tokens"),
                format!("{next_tokens} tokens"),
            ));
        }
        if let Some(cap) = self.session_input_token_cap(session_id)
            && next_input_tokens > cap
        {
            self.blocked_sessions.insert(session_id.to_string());
            return Err(self.cap_block(
                session_id,
                "per_session_input_tokens",
                format!("{cap} input tokens"),
                format!("{next_input_tokens} input tokens"),
            ));
        }
        if let Some(cap) = self.session_output_token_cap(session_id)
            && next_output_tokens > cap
        {
            self.blocked_sessions.insert(session_id.to_string());
            return Err(self.cap_block(
                session_id,
                "per_session_output_tokens",
                format!("{cap} output tokens"),
                format!("{next_output_tokens} output tokens"),
            ));
        }
        if !usd.is_finite() || usd < 0.0 {
            return Err(self.cap_block(
                session_id,
                "invalid_usd",
                "finite non-negative USD".to_string(),
                usd.to_string(),
            ));
        }
        if let Some(cap) = self.session_usd_cap(session_id)
            && next_usd > cap
        {
            self.blocked_sessions.insert(session_id.to_string());
            return Err(self.cap_block(
                session_id,
                "per_session_usd",
                format!("${cap:.4}"),
                format!("${next_usd:.4}"),
            ));
        }

        let id = self.next_reservation_id;
        self.next_reservation_id = self.next_reservation_id.saturating_add(1);
        let totals = self
            .reserved_per_session
            .entry(session_id.to_string())
            .or_default();
        totals.tokens = totals.tokens.saturating_add(tokens);
        totals.input_tokens = totals.input_tokens.saturating_add(input_tokens);
        totals.output_tokens = totals.output_tokens.saturating_add(output_tokens);
        totals.usd += usd;
        self.reservations.insert(
            id,
            ReservationEntry {
                session_id: session_id.to_string(),
                tokens,
                input_tokens,
                output_tokens,
                usd,
            },
        );
        Ok(BudgetReservation(id))
    }

    /// Reconcile an admitted call with authoritative provider usage. Actual
    /// usage is always recorded, even when a provider exceeds its reservation;
    /// the returned error then prevents any subsequent admission.
    pub fn settle(
        &mut self,
        reservation: BudgetReservation,
        actual_tokens: u64,
        actual_usd: f64,
    ) -> Result<(), BudgetError> {
        self.settle_turn(reservation, actual_tokens, 0, actual_usd)
    }

    /// Reconcile a direction-aware provider admission with authoritative,
    /// disjoint input/output usage.
    pub fn settle_turn(
        &mut self,
        reservation: BudgetReservation,
        mut actual_input_tokens: u64,
        mut actual_output_tokens: u64,
        mut actual_usd: f64,
    ) -> Result<(), BudgetError> {
        let Some(entry) = self.take_reservation(reservation) else {
            return Ok(());
        };
        let invalid_usd =
            (!actual_usd.is_finite() || actual_usd < 0.0).then(|| actual_usd.to_string());
        if invalid_usd.is_some() {
            // The authoritative settlement is unusable, but the provider call
            // was admitted and may have been billed. Consume the conservative
            // reservation rather than leaking an in-flight allowance forever.
            actual_input_tokens = entry.input_tokens;
            actual_output_tokens = entry.output_tokens;
            actual_usd = entry.usd;
        }
        let totals = self
            .per_session
            .entry(entry.session_id.clone())
            .or_default();
        let actual_tokens = actual_input_tokens.saturating_add(actual_output_tokens);
        totals.tokens = totals.tokens.saturating_add(actual_tokens);
        totals.input_tokens = totals.input_tokens.saturating_add(actual_input_tokens);
        totals.output_tokens = totals.output_tokens.saturating_add(actual_output_tokens);
        totals.usd += actual_usd;
        let next_tokens = totals.tokens;
        let next_input_tokens = totals.input_tokens;
        let next_output_tokens = totals.output_tokens;
        let next_usd = totals.usd;

        self.emit(BudgetEvent::Charge {
            session_id: entry.session_id.clone(),
            tokens: actual_tokens,
            usd: actual_usd,
        });

        if let Some(observed) = invalid_usd {
            self.blocked_sessions.insert(entry.session_id.clone());
            return Err(self.cap_block(
                &entry.session_id,
                "invalid_usd",
                "finite non-negative USD".to_string(),
                observed,
            ));
        }

        if let Some(cap) = self.session_token_cap(&entry.session_id)
            && next_tokens > cap
        {
            self.blocked_sessions.insert(entry.session_id.clone());
            return Err(self.cap_block(
                &entry.session_id,
                "per_session_tokens",
                format!("{cap} tokens"),
                format!("{next_tokens} tokens"),
            ));
        }
        if let Some(cap) = self.session_input_token_cap(&entry.session_id)
            && next_input_tokens > cap
        {
            self.blocked_sessions.insert(entry.session_id.clone());
            return Err(self.cap_block(
                &entry.session_id,
                "per_session_input_tokens",
                format!("{cap} input tokens"),
                format!("{next_input_tokens} input tokens"),
            ));
        }
        if let Some(cap) = self.session_output_token_cap(&entry.session_id)
            && next_output_tokens > cap
        {
            self.blocked_sessions.insert(entry.session_id.clone());
            return Err(self.cap_block(
                &entry.session_id,
                "per_session_output_tokens",
                format!("{cap} output tokens"),
                format!("{next_output_tokens} output tokens"),
            ));
        }
        if let Some(cap) = self.session_usd_cap(&entry.session_id)
            && next_usd > cap
        {
            self.blocked_sessions.insert(entry.session_id.clone());
            return Err(self.cap_block(
                &entry.session_id,
                "per_session_usd",
                format!("${cap:.4}"),
                format!("${next_usd:.4}"),
            ));
        }
        if let Some(pct) = self.pct_used_strictest(&entry.session_id)
            && pct >= 0.80
        {
            self.emit(BudgetEvent::CapWarn {
                session_id: entry.session_id,
                pct_used: pct,
            });
        }
        Ok(())
    }

    /// Release an admission that never reached the provider.
    pub fn release(&mut self, reservation: BudgetReservation) -> bool {
        self.take_reservation(reservation).is_some()
    }

    fn take_reservation(&mut self, reservation: BudgetReservation) -> Option<ReservationEntry> {
        let entry = self.reservations.remove(&reservation.0)?;
        if let Some(totals) = self.reserved_per_session.get_mut(&entry.session_id) {
            totals.tokens = totals.tokens.saturating_sub(entry.tokens);
            totals.input_tokens = totals.input_tokens.saturating_sub(entry.input_tokens);
            totals.output_tokens = totals.output_tokens.saturating_sub(entry.output_tokens);
            totals.usd = (totals.usd - entry.usd).max(0.0);
            if totals.tokens == 0
                && totals.input_tokens == 0
                && totals.output_tokens == 0
                && totals.usd == 0.0
            {
                self.reserved_per_session.remove(&entry.session_id);
            }
        }
        Some(entry)
    }

    fn cap_block(
        &self,
        session_id: &str,
        kind: &str,
        limit: String,
        observed: String,
    ) -> BudgetError {
        let err = BudgetError::CapExceeded {
            kind: kind.to_string(),
            limit,
            observed,
        };
        self.emit(BudgetEvent::CapBlock {
            session_id: session_id.to_string(),
            reason: err.clone(),
        });
        err
    }

    /// Record `(tokens, usd)` against `session_id`. Returns `Err` if the
    /// charge would exceed a per-session cap; in that case the running
    /// totals are NOT incremented (the rejected charge does not "stick").
    pub fn charge(&mut self, session_id: &str, tokens: u64, usd: f64) -> Result<(), BudgetError> {
        let reserved = self
            .reserved_per_session
            .get(session_id)
            .copied()
            .unwrap_or_default();
        if !usd.is_finite() || usd < 0.0 {
            return Err(self.cap_block(
                session_id,
                "invalid_usd",
                "finite non-negative USD".to_string(),
                usd.to_string(),
            ));
        }
        let current = self
            .per_session
            .get(session_id)
            .copied()
            .unwrap_or_default();
        let committed_next_tokens = current.tokens.saturating_add(tokens);
        let committed_next_usd = current.usd + usd;
        let next_tokens = committed_next_tokens.saturating_add(reserved.tokens);
        let next_usd = committed_next_usd + reserved.usd;

        if let Some(cap) = self.session_token_cap(session_id)
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
        if let Some(cap) = self.session_usd_cap(session_id)
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
        self.per_session.insert(
            session_id.to_string(),
            SessionTotals {
                tokens: committed_next_tokens,
                input_tokens: current.input_tokens,
                output_tokens: current.output_tokens,
                usd: committed_next_usd,
            },
        );

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

    /// Snapshot of in-flight reservations for `session_id`.
    pub fn reserved_totals(&self, session_id: &str) -> (u64, f64) {
        self.reserved_per_session
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
            .session_token_cap(session_id)
            .map(|cap| entry.tokens as f32 / cap as f32);
        let input_token_pct = self
            .session_input_token_cap(session_id)
            .map(|cap| entry.input_tokens as f32 / cap as f32);
        let output_token_pct = self
            .session_output_token_cap(session_id)
            .map(|cap| entry.output_tokens as f32 / cap as f32);
        let usd_pct = self
            .session_usd_cap(session_id)
            .map(|cap| (entry.usd / cap) as f32);
        [token_pct, input_token_pct, output_token_pct, usd_pct]
            .into_iter()
            .flatten()
            .reduce(f32::max)
    }
}

fn checked_usd_add(left: f64, right: f64) -> Option<f64> {
    if !left.is_finite() || !right.is_finite() || left < 0.0 || right < 0.0 {
        return None;
    }
    if left == 0.0 {
        return Some(right);
    }
    if right == 0.0 {
        return Some(left);
    }
    let sum = left + right;
    (sum.is_finite() && sum > left && sum > right).then_some(sum)
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
    fn reservation_blocks_a_second_call_before_spend_occurs() {
        let cap = BudgetCap::builder().per_session_tokens(1_000).build();
        let mut tracker = BudgetTracker::new(cap);
        let first = tracker.reserve("s1", 800, 0.0).unwrap();

        assert!(tracker.reserve("s1", 300, 0.0).is_err());
        assert_eq!(tracker.session_totals("s1"), (0, 0.0));
        assert_eq!(tracker.reserved_totals("s1"), (800, 0.0));

        tracker.release(first);
        assert_eq!(tracker.reserved_totals("s1"), (0, 0.0));
    }

    #[test]
    fn settlement_refunds_unused_reservation_and_charges_actual_usage() {
        let cap = BudgetCap::builder()
            .per_session_tokens(1_000)
            .per_session_usd(1.0)
            .build();
        let mut tracker = BudgetTracker::new(cap);
        let reservation = tracker.reserve("s1", 900, 0.90).unwrap();

        tracker.settle(reservation, 200, 0.20).unwrap();

        assert_eq!(tracker.session_totals("s1"), (200, 0.20));
        assert_eq!(tracker.reserved_totals("s1"), (0, 0.0));
        assert!(tracker.reserve("s1", 800, 0.80).is_ok());
    }

    #[test]
    fn directional_reservations_enforce_input_and_output_independently() {
        let cap = BudgetCap::builder()
            .per_session_tokens(1_001)
            .per_session_input_tokens(1_000)
            .per_session_output_tokens(1)
            .build();
        let mut tracker = BudgetTracker::new(cap);

        let output_err = tracker.reserve_turn("s1", 1, 2, 0.0).unwrap_err();
        assert!(matches!(
            output_err,
            BudgetError::CapExceeded { ref kind, .. }
                if kind == "per_session_output_tokens"
        ));
        let input_err = tracker.reserve_turn("s2", 1_001, 0, 0.0).unwrap_err();
        assert!(matches!(
            input_err,
            BudgetError::CapExceeded { ref kind, .. }
                if kind == "per_session_input_tokens"
        ));
        assert!(tracker.reserve_turn("s3", 1_000, 1, 0.0).is_ok());
    }

    #[test]
    fn budget_config_conversion_preserves_directional_caps() {
        let config = crate::BudgetConfig {
            max_tokens_in: Some(1_000),
            max_tokens_out: Some(7),
            ..Default::default()
        };
        let cap = BudgetCap::from(&config);
        assert_eq!(cap.per_session_input_tokens, Some(1_000));
        assert_eq!(cap.per_session_output_tokens, Some(7));
        assert_eq!(cap.per_session_tokens, Some(1_007));
    }

    #[test]
    fn directional_settlement_records_overshoot_before_blocking() {
        let cap = BudgetCap::builder().per_session_output_tokens(10).build();
        let mut tracker = BudgetTracker::new(cap);
        let reservation = tracker.reserve_turn("s1", 5, 5, 0.0).unwrap();

        let err = tracker.settle_turn(reservation, 5, 11, 0.0).unwrap_err();
        assert!(matches!(
            err,
            BudgetError::CapExceeded { ref kind, .. }
                if kind == "per_session_output_tokens"
        ));
        assert!(tracker.reserve_turn("s1", 0, 1, 0.0).is_err());
    }

    #[test]
    fn invalid_direct_caps_fail_closed() {
        for usd in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let mut tracker = BudgetTracker::new(BudgetCap::builder().per_session_usd(usd).build());
            assert!(tracker.reserve("s1", 0, 0.01).is_err(), "accepted {usd}");
        }
    }

    #[test]
    fn invalid_settlement_consumes_conservative_reservation() {
        let mut tracker = BudgetTracker::new(
            BudgetCap::builder()
                .per_session_tokens(100)
                .per_session_usd(1.0)
                .build(),
        );
        let reservation = tracker.reserve_turn("s1", 60, 20, 0.75).unwrap();

        let error = tracker
            .settle_turn(reservation, 1, 1, f64::NAN)
            .unwrap_err();

        assert!(matches!(
            error,
            BudgetError::CapExceeded { ref kind, .. } if kind == "invalid_usd"
        ));
        assert_eq!(tracker.session_totals("s1"), (80, 0.75));
        assert_eq!(tracker.reserved_totals("s1"), (0, 0.0));
        assert!(tracker.reserve("s1", 1, 0.0).is_err());
    }

    #[test]
    fn reports_whether_a_session_has_a_usd_ceiling() {
        let uncapped = BudgetTracker::new(BudgetCap::default());
        assert!(!uncapped.has_session_usd_cap("s1"));

        let capped = BudgetTracker::new(BudgetCap::builder().per_session_usd(1.0).build());
        assert!(capped.has_session_usd_cap("s1"));
    }

    #[test]
    fn settlement_records_provider_overshoot_and_blocks_future_calls() {
        let cap = BudgetCap::builder().per_session_tokens(100).build();
        let mut tracker = BudgetTracker::new(cap);
        let reservation = tracker.reserve("s1", 90, 0.0).unwrap();

        let err = tracker.settle(reservation, 110, 0.0).unwrap_err();

        assert!(
            matches!(err, BudgetError::CapExceeded { ref kind, .. } if kind == "per_session_tokens")
        );
        assert_eq!(tracker.session_totals("s1").0, 110);
        assert!(tracker.reserve("s1", 1, 0.0).is_err());
    }

    #[test]
    fn additional_budget_is_session_scoped_and_reopens_admission() {
        let cap = BudgetCap::builder()
            .per_session_tokens(100)
            .per_session_usd(1.0)
            .build();
        let mut tracker = BudgetTracker::new(cap);
        tracker.charge("s1", 100, 1.0).unwrap();
        assert!(tracker.reserve("s1", 1, 0.01).is_err());

        tracker.extend_session("s1", 50, 0.50).unwrap();

        assert!(tracker.reserve("s1", 50, 0.50).is_ok());
        assert!(tracker.reserve("s2", 101, 0.0).is_err());
    }

    #[test]
    fn extension_rejects_effective_usd_cap_overflow() {
        let mut tracker = BudgetTracker::new(BudgetCap::builder().per_session_usd(1.0).build());
        assert!(tracker.reserve("s1", 0, 2.0).is_err());

        let err = tracker.extend_session("s1", 0, f64::MAX).unwrap_err();
        assert!(matches!(
            err,
            BudgetError::CapExceeded { ref kind, .. } if kind == "invalid_usd"
        ));
    }

    #[test]
    fn plain_reservation_fails_closed_when_daily_user_cap_requires_identity() {
        let cap = BudgetCap::builder().per_user_daily_usd(0.10).build();
        let mut tracker = BudgetTracker::new(cap);

        let err = tracker.reserve("s1", 100, 0.05).unwrap_err();

        assert!(
            matches!(
                err,
                BudgetError::CapExceeded { ref kind, .. }
                    if kind == "per_user_daily_identity_required"
            ),
            "session-only admission must not bypass a configured daily user cap"
        );
        assert_eq!(tracker.reserved_totals("s1"), (0, 0.0));
        assert_eq!(tracker.session_totals("s1"), (0, 0.0));
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
