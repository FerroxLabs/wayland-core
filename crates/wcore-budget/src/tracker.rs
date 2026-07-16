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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Datelike, Utc};
use thiserror::Error;

/// Caps for the session-keyed / user-keyed tracker. None on every field
/// means "no cap" — the tracker accumulates totals for observability but
/// every charge succeeds.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
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

/// Structured failures raised while extending one blocked session's budget.
///
/// This is intentionally separate from [`BudgetError`]: interactive budget
/// grants need stable machine-readable reasons and must never classify a
/// display-oriented `CapExceeded.kind` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetExtensionError {
    /// The session has no outstanding budget-exceeded state to reopen.
    #[error("the session has no exhausted budget to extend")]
    NoExhaustedBudget,
    /// The USD extension is negative, non-finite, or not representable.
    #[error("the USD extension is invalid")]
    InvalidUsd,
    /// Neither token nor USD headroom was supplied.
    #[error("the budget extension is empty")]
    EmptyExtension,
    /// The caller supplied an unusable idempotency key.
    #[error("the budget extension request id is invalid")]
    InvalidRequestId,
    /// A prior committed grant used this request id with different values.
    #[error("the budget extension request id conflicts with a committed grant")]
    RequestIdConflict,
    /// Retaining another grant receipt would exceed the durable bound.
    #[error("the durable budget grant ledger is full")]
    GrantLedgerCapacityExceeded,
}

/// Result of an idempotent session-budget extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetExtensionOutcome {
    /// This call committed new headroom.
    Applied,
    /// The same request was committed previously; no mutation occurred.
    AlreadyApplied,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct BudgetReservation(u64);

#[derive(Debug, Clone, Copy)]
struct DailyTotals {
    /// Year-month-day in UTC (chrono `NaiveDate::num_days_from_ce` is
    /// stable across timezone boundary changes).
    day_ordinal: i32,
    usd: f64,
}

const BUDGET_TRACKER_SNAPSHOT_VERSION: u32 = 1;
const MAX_BUDGET_EXTENSION_REQUEST_ID_BYTES: usize = 128;
const MAX_DURABLE_BUDGET_GRANTS_PER_SESSION: usize = 1_024;

/// Serializable, immutable copy of tracker enforcement authority.
///
/// Derived reservation totals are intentionally omitted and rebuilt from the
/// reservation ledger during restore so serialized input cannot make the two
/// sources disagree.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetTrackerSnapshot {
    schema_version: u32,
    caps: BudgetCap,
    per_session: BTreeMap<String, SessionTotalsSnapshot>,
    #[serde(with = "reservation_snapshot_ledger")]
    reservations: BTreeMap<u64, ReservationSnapshot>,
    next_reservation_id: u64,
    session_extensions: BTreeMap<String, SessionExtensionSnapshot>,
    /// Added compatibly to version 1. Missing fields in older snapshots
    /// migrate to an empty ledger during deserialization.
    #[serde(default)]
    applied_budget_grants: BTreeMap<String, BTreeMap<String, BudgetGrantBindingSnapshot>>,
    blocked_sessions: BTreeSet<String>,
    per_user_daily: BTreeMap<String, DailyTotalsSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionTotalsSnapshot {
    tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    usd: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReservationSnapshot {
    session_id: String,
    input_tokens: u64,
    output_tokens: u64,
    usd: f64,
}

mod reservation_snapshot_ledger {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

    use super::ReservationSnapshot;

    #[derive(Serialize)]
    #[serde(deny_unknown_fields)]
    struct ReservationEntryRef<'a> {
        reservation_id: u64,
        reservation: &'a ReservationSnapshot,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReservationEntry {
        reservation_id: u64,
        reservation: ReservationSnapshot,
    }

    pub(super) fn serialize<S>(
        reservations: &BTreeMap<u64, ReservationSnapshot>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        reservations
            .iter()
            .map(|(reservation_id, reservation)| ReservationEntryRef {
                reservation_id: *reservation_id,
                reservation,
            })
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<BTreeMap<u64, ReservationSnapshot>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries = Vec::<ReservationEntry>::deserialize(deserializer)?;
        let mut reservations = BTreeMap::new();
        for entry in entries {
            let reservation_id = entry.reservation_id;
            if reservations
                .insert(reservation_id, entry.reservation)
                .is_some()
            {
                return Err(D::Error::custom(format!(
                    "duplicate reservation id {reservation_id}"
                )));
            }
        }
        Ok(reservations)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionExtensionSnapshot {
    tokens: u64,
    usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetGrantBindingSnapshot {
    additional_tokens: u64,
    additional_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DailyTotalsSnapshot {
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
    applied_budget_grants: HashMap<String, HashMap<String, BudgetGrantBindingSnapshot>>,
    /// Sessions with a provider admission/settlement cap receipt outstanding.
    /// An extension consumes this latch; arbitrary pre-emptive widening is not
    /// a valid Continue operation.
    blocked_sessions: HashSet<String>,
    per_user_daily: HashMap<String, DailyTotals>,
    sink: Option<Arc<dyn BudgetEventSink>>,
    restore_applied: bool,
    /// Reservation ids recovered from durable state and therefore requiring
    /// explicit restart reconciliation before fresh provider admission.
    restored_reservations: HashSet<u64>,
}

/// Outcome of conservatively charging every provider reservation recovered
/// from durable state.
#[derive(Debug, Clone)]
pub struct RestoredReservationReconciliation {
    /// Number of recovered reservations consumed by this reconciliation.
    pub reservations_settled: usize,
    /// Conservative input-token authority consumed while reconciling.
    pub input_tokens_charged: u64,
    /// Conservative output-token authority consumed while reconciling.
    pub output_tokens_charged: u64,
    /// Conservative cost authority consumed while reconciling.
    pub cost_usd_charged: f64,
    /// Cap receipts raised after the conservative charges were committed.
    /// All reservations are settled even when one or more caps are exceeded.
    pub cap_errors: Vec<BudgetError>,
}

impl BudgetTracker {
    pub fn new(caps: BudgetCap) -> Self {
        let caps = normalize_caps(caps);
        Self {
            caps,
            per_session: HashMap::new(),
            reserved_per_session: HashMap::new(),
            reservations: HashMap::new(),
            next_reservation_id: 1,
            session_extensions: HashMap::new(),
            applied_budget_grants: HashMap::new(),
            blocked_sessions: HashSet::new(),
            per_user_daily: HashMap::new(),
            sink: None,
            restore_applied: false,
            restored_reservations: HashSet::new(),
        }
    }

    /// Capture caps, extensions, committed usage, user-daily usage, blocked
    /// sessions, and every in-flight provider reservation.
    pub fn snapshot(&self) -> Result<BudgetTrackerSnapshot, crate::BudgetSnapshotError> {
        let snapshot = BudgetTrackerSnapshot {
            schema_version: BUDGET_TRACKER_SNAPSHOT_VERSION,
            caps: self.caps.clone(),
            per_session: self
                .per_session
                .iter()
                .map(|(session_id, totals)| {
                    (
                        session_id.clone(),
                        SessionTotalsSnapshot {
                            tokens: totals.tokens,
                            input_tokens: totals.input_tokens,
                            output_tokens: totals.output_tokens,
                            usd: totals.usd,
                        },
                    )
                })
                .collect(),
            reservations: self
                .reservations
                .iter()
                .map(|(id, entry)| {
                    (
                        *id,
                        ReservationSnapshot {
                            session_id: entry.session_id.clone(),
                            input_tokens: entry.input_tokens,
                            output_tokens: entry.output_tokens,
                            usd: entry.usd,
                        },
                    )
                })
                .collect(),
            next_reservation_id: self.next_reservation_id,
            session_extensions: self
                .session_extensions
                .iter()
                .map(|(session_id, extension)| {
                    (
                        session_id.clone(),
                        SessionExtensionSnapshot {
                            tokens: extension.tokens,
                            usd: extension.usd,
                        },
                    )
                })
                .collect(),
            applied_budget_grants: self
                .applied_budget_grants
                .iter()
                .map(|(session_id, grants)| {
                    (
                        session_id.clone(),
                        grants
                            .iter()
                            .map(|(request_id, grant)| (request_id.clone(), *grant))
                            .collect(),
                    )
                })
                .collect(),
            blocked_sessions: self.blocked_sessions.iter().cloned().collect(),
            per_user_daily: self
                .per_user_daily
                .iter()
                .map(|(user_id, totals)| {
                    (
                        user_id.clone(),
                        DailyTotalsSnapshot {
                            day_ordinal: totals.day_ordinal,
                            usd: totals.usd,
                        },
                    )
                })
                .collect(),
        };
        validate_tracker_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    /// Whether this tracker still contains only its configured caps and
    /// process-local wiring, with no durable enforcement state applied.
    pub fn is_pristine(&self) -> bool {
        !self.restore_applied && self.durable_state_is_empty()
    }

    /// Build a tracker directly from serialized enforcement authority.
    pub fn from_snapshot(
        snapshot: BudgetTrackerSnapshot,
    ) -> Result<Self, crate::BudgetSnapshotError> {
        build_tracker_from_snapshot(snapshot)
    }

    /// Restore durable usage under the intersection of the captured caps and
    /// the caps configured for the new process.
    ///
    /// `None` is treated as unbounded, so adding a current cap tightens a
    /// previously unbounded snapshot. Durable per-session extensions are
    /// clamped as necessary so their effective caps cannot exceed either the
    /// captured authority or the current policy.
    pub fn from_snapshot_with_current_caps(
        snapshot: BudgetTrackerSnapshot,
        current_caps: BudgetCap,
    ) -> Result<Self, crate::BudgetSnapshotError> {
        build_tracker_from_snapshot(constrain_tracker_snapshot(snapshot, current_caps)?)
    }

    /// Restore under current base caps while retaining committed per-session
    /// extensions. This is for an unmanaged durable session whose explicit
    /// operator grants must survive process restart. Managed callers must use
    /// [`Self::from_snapshot_with_current_caps`] so their current ceiling
    /// clamps all prior interactive headroom.
    pub fn from_snapshot_with_current_caps_preserving_extensions(
        mut snapshot: BudgetTrackerSnapshot,
        current_caps: BudgetCap,
    ) -> Result<Self, crate::BudgetSnapshotError> {
        validate_tracker_snapshot(&snapshot)?;
        snapshot.caps = intersect_caps(&snapshot.caps, &normalize_caps(current_caps));
        validate_tracker_snapshot(&snapshot)?;
        build_tracker_from_snapshot(snapshot)
    }

    /// Atomically apply serialized authority to a pristine tracker.
    ///
    /// The event sink is runtime wiring rather than durable state, so an
    /// already-installed sink is retained. Reapplying a snapshot, including
    /// an empty one, is rejected to prevent replay from double-restoring.
    pub fn restore_snapshot(
        &mut self,
        snapshot: BudgetTrackerSnapshot,
    ) -> Result<(), crate::BudgetSnapshotError> {
        if self.restore_applied || !self.durable_state_is_empty() {
            return Err(crate::BudgetSnapshotError::RestoreTargetNotPristine);
        }
        let mut restored = build_tracker_from_snapshot(snapshot)?;
        restored.sink = self.sink.take();
        *self = restored;
        Ok(())
    }

    /// Apply durable usage to a pristine tracker while intersecting captured
    /// authority with this tracker's current caps.
    pub fn restore_snapshot_with_current_caps(
        &mut self,
        snapshot: BudgetTrackerSnapshot,
    ) -> Result<(), crate::BudgetSnapshotError> {
        if self.restore_applied || !self.durable_state_is_empty() {
            return Err(crate::BudgetSnapshotError::RestoreTargetNotPristine);
        }
        let current_caps = self.caps.clone();
        let mut restored = Self::from_snapshot_with_current_caps(snapshot, current_caps)?;
        restored.sink = self.sink.take();
        *self = restored;
        Ok(())
    }

    /// Conservatively settle every provider reservation recovered after a
    /// restart at its admitted maximum.
    ///
    /// The operation is exhaustive: a cap error blocks the affected session
    /// but does not leave later restored reservations unsettled. Repeating the
    /// call is safe and reports zero additional settlements.
    pub fn reconcile_restored_reservations_conservatively(
        &mut self,
    ) -> RestoredReservationReconciliation {
        let mut ids: Vec<_> = std::mem::take(&mut self.restored_reservations)
            .into_iter()
            .collect();
        ids.sort_unstable();
        let mut cap_errors = Vec::new();
        let mut reservations_settled = 0;
        let mut input_tokens_charged = 0u64;
        let mut output_tokens_charged = 0u64;
        let mut cost_usd_charged = 0.0;

        for id in ids {
            let Some(entry) = self.reservations.get(&id).cloned() else {
                continue;
            };
            reservations_settled += 1;
            input_tokens_charged = input_tokens_charged.saturating_add(entry.input_tokens);
            output_tokens_charged = output_tokens_charged.saturating_add(entry.output_tokens);
            let next_cost = cost_usd_charged + entry.usd;
            cost_usd_charged = if next_cost.is_finite() {
                next_cost
            } else {
                f64::INFINITY
            };
            if let Err(error) = self.settle_turn(
                BudgetReservation(id),
                entry.input_tokens,
                entry.output_tokens,
                entry.usd,
            ) {
                cap_errors.push(error);
            }
        }

        RestoredReservationReconciliation {
            reservations_settled,
            input_tokens_charged,
            output_tokens_charged,
            cost_usd_charged,
            cap_errors,
        }
    }

    /// Whether this tracker still owns an in-flight provider reservation.
    ///
    /// Durable coordinators use this to validate that their external-effect
    /// correlation ledger names authority that actually exists in the
    /// tracker snapshot.
    pub fn has_reservation(&self, reservation: BudgetReservation) -> bool {
        self.reservations.contains_key(&reservation.0)
    }

    /// Return the admitted maximum owned by one in-flight provider
    /// reservation. Restart coordinators use this before consuming the
    /// reservation so provider and execution ledgers receive the same charge.
    pub fn reservation_admitted_maximum(
        &self,
        reservation: BudgetReservation,
    ) -> Option<(u64, u64, f64)> {
        self.reservations
            .get(&reservation.0)
            .map(|entry| (entry.input_tokens, entry.output_tokens, entry.usd))
    }

    /// Consume one in-flight provider reservation at its admitted maximum.
    ///
    /// This is the targeted counterpart to restart-wide reconciliation. It
    /// lets a durable coordinator settle only the reservation whose physical
    /// dispatch is known to have started, while releasing proved no-send
    /// reservations independently.
    pub fn settle_reservation_conservatively(
        &mut self,
        reservation: BudgetReservation,
    ) -> Result<bool, BudgetError> {
        let Some(entry) = self.reservations.get(&reservation.0).cloned() else {
            return Ok(false);
        };
        self.settle_turn(
            reservation,
            entry.input_tokens,
            entry.output_tokens,
            entry.usd,
        )?;
        Ok(true)
    }

    fn durable_state_is_empty(&self) -> bool {
        self.per_session.is_empty()
            && self.reserved_per_session.is_empty()
            && self.reservations.is_empty()
            && self.next_reservation_id == 1
            && self.session_extensions.is_empty()
            && self.applied_budget_grants.is_empty()
            && self.blocked_sessions.is_empty()
            && self.per_user_daily.is_empty()
            && self.restored_reservations.is_empty()
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
    ) -> Result<(), BudgetExtensionError> {
        if !self.blocked_sessions.contains(session_id) {
            return Err(BudgetExtensionError::NoExhaustedBudget);
        }
        if !additional_usd.is_finite() || additional_usd < 0.0 {
            return Err(BudgetExtensionError::InvalidUsd);
        }
        if additional_tokens == 0 && additional_usd == 0.0 {
            return Err(BudgetExtensionError::EmptyExtension);
        }
        let current_extension_usd = self
            .session_extensions
            .get(session_id)
            .map(|extension| extension.usd)
            .unwrap_or(0.0);
        let Some(next_extension_usd) = checked_usd_add(current_extension_usd, additional_usd)
        else {
            return Err(BudgetExtensionError::InvalidUsd);
        };
        if let Some(base_cap) = self.caps.per_session_usd
            && checked_usd_add(base_cap, next_extension_usd).is_none()
        {
            return Err(BudgetExtensionError::InvalidUsd);
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

    /// Apply operator-authorized headroom at most once for a stable request.
    ///
    /// The request binding is captured in the same tracker snapshot as the
    /// extension. A durable authority transaction therefore commits both or
    /// neither across a crash. Receipts are never evicted: once the per-session
    /// bound is reached, new request ids fail closed.
    pub fn extend_session_idempotent(
        &mut self,
        session_id: &str,
        request_id: &str,
        additional_tokens: u64,
        additional_usd: f64,
    ) -> Result<BudgetExtensionOutcome, BudgetExtensionError> {
        if request_id.trim().is_empty() || request_id.len() > MAX_BUDGET_EXTENSION_REQUEST_ID_BYTES
        {
            return Err(BudgetExtensionError::InvalidRequestId);
        }

        let requested = BudgetGrantBindingSnapshot {
            additional_tokens,
            additional_usd,
        };
        if let Some(existing) = self
            .applied_budget_grants
            .get(session_id)
            .and_then(|grants| grants.get(request_id))
        {
            return if *existing == requested {
                Ok(BudgetExtensionOutcome::AlreadyApplied)
            } else {
                Err(BudgetExtensionError::RequestIdConflict)
            };
        }
        if self
            .applied_budget_grants
            .get(session_id)
            .is_some_and(|grants| grants.len() >= MAX_DURABLE_BUDGET_GRANTS_PER_SESSION)
        {
            return Err(BudgetExtensionError::GrantLedgerCapacityExceeded);
        }

        self.extend_session(session_id, additional_tokens, additional_usd)?;
        self.applied_budget_grants
            .entry(session_id.to_owned())
            .or_default()
            .insert(request_id.to_owned(), requested);
        Ok(BudgetExtensionOutcome::Applied)
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

    /// Effective aggregate session limits, including durable operator grants.
    #[must_use]
    pub fn effective_session_limits(&self, session_id: &str) -> (Option<u64>, Option<f64>) {
        (
            self.session_token_cap(session_id),
            self.session_usd_cap(session_id),
        )
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
        self.restored_reservations.remove(&reservation.0);
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

fn validate_tracker_snapshot(
    snapshot: &BudgetTrackerSnapshot,
) -> Result<(), crate::BudgetSnapshotError> {
    if snapshot.schema_version != BUDGET_TRACKER_SNAPSHOT_VERSION {
        return Err(crate::BudgetSnapshotError::UnsupportedVersion {
            found: snapshot.schema_version,
            expected: BUDGET_TRACKER_SNAPSHOT_VERSION,
        });
    }
    validate_optional_usd("caps.per_session_usd", snapshot.caps.per_session_usd)?;
    validate_optional_usd("caps.per_user_daily_usd", snapshot.caps.per_user_daily_usd)?;

    for (session_id, totals) in &snapshot.per_session {
        validate_usd(&format!("per_session[{session_id:?}].usd"), totals.usd)?;
    }
    for (session_id, extension) in &snapshot.session_extensions {
        validate_usd(
            &format!("session_extensions[{session_id:?}].usd"),
            extension.usd,
        )?;
        if let Some(base) = snapshot.caps.per_session_usd
            && checked_usd_add(base, extension.usd).is_none()
        {
            return Err(invalid_snapshot(format!(
                "session_extensions[{session_id:?}].usd makes the effective cap unrepresentable"
            )));
        }
    }
    for (session_id, grants) in &snapshot.applied_budget_grants {
        if grants.len() > MAX_DURABLE_BUDGET_GRANTS_PER_SESSION {
            return Err(invalid_snapshot(format!(
                "applied_budget_grants[{session_id:?}] exceeds the durable receipt bound"
            )));
        }
        for (request_id, grant) in grants {
            if request_id.trim().is_empty()
                || request_id.len() > MAX_BUDGET_EXTENSION_REQUEST_ID_BYTES
            {
                return Err(invalid_snapshot(format!(
                    "applied_budget_grants[{session_id:?}] contains an invalid request id"
                )));
            }
            validate_usd(
                &format!("applied_budget_grants[{session_id:?}][{request_id:?}].additional_usd"),
                grant.additional_usd,
            )?;
            if grant.additional_tokens == 0 && grant.additional_usd == 0.0 {
                return Err(invalid_snapshot(format!(
                    "applied_budget_grants[{session_id:?}][{request_id:?}] is empty"
                )));
            }
        }
    }
    for (user_id, totals) in &snapshot.per_user_daily {
        validate_usd(&format!("per_user_daily[{user_id:?}].usd"), totals.usd)?;
    }

    let mut max_reservation_id = 0;
    let mut reserved_usd = BTreeMap::<&str, f64>::new();
    for (id, reservation) in &snapshot.reservations {
        if *id == 0 {
            return Err(invalid_snapshot("reservation id must be non-zero"));
        }
        max_reservation_id = max_reservation_id.max(*id);
        validate_usd(&format!("reservations[{id}].usd"), reservation.usd)?;
        let total = reserved_usd
            .entry(reservation.session_id.as_str())
            .or_default();
        *total = checked_usd_add(*total, reservation.usd).ok_or_else(|| {
            invalid_snapshot(format!(
                "reservations for session {:?} overflow USD authority",
                reservation.session_id
            ))
        })?;
    }
    if snapshot.next_reservation_id == 0 || max_reservation_id >= snapshot.next_reservation_id {
        return Err(invalid_snapshot(
            "next_reservation_id must be non-zero and greater than every reservation id",
        ));
    }
    Ok(())
}

fn build_tracker_from_snapshot(
    snapshot: BudgetTrackerSnapshot,
) -> Result<BudgetTracker, crate::BudgetSnapshotError> {
    validate_tracker_snapshot(&snapshot)?;

    let per_session = snapshot
        .per_session
        .into_iter()
        .map(|(session_id, totals)| {
            (
                session_id,
                SessionTotals {
                    tokens: totals.tokens,
                    input_tokens: totals.input_tokens,
                    output_tokens: totals.output_tokens,
                    usd: totals.usd,
                },
            )
        })
        .collect();
    let mut reserved_per_session = HashMap::<String, ReservedTotals>::new();
    let mut reservations = HashMap::new();
    for (id, reservation) in snapshot.reservations {
        let tokens = reservation
            .input_tokens
            .saturating_add(reservation.output_tokens);
        let totals = reserved_per_session
            .entry(reservation.session_id.clone())
            .or_default();
        totals.tokens = totals.tokens.saturating_add(tokens);
        totals.input_tokens = totals.input_tokens.saturating_add(reservation.input_tokens);
        totals.output_tokens = totals
            .output_tokens
            .saturating_add(reservation.output_tokens);
        totals.usd = checked_usd_add(totals.usd, reservation.usd).ok_or_else(|| {
            invalid_snapshot(format!(
                "reservations for session {:?} overflow USD authority",
                reservation.session_id
            ))
        })?;
        reservations.insert(
            id,
            ReservationEntry {
                session_id: reservation.session_id,
                tokens,
                input_tokens: reservation.input_tokens,
                output_tokens: reservation.output_tokens,
                usd: reservation.usd,
            },
        );
    }

    let restored_reservations = reservations.keys().copied().collect();
    Ok(BudgetTracker {
        caps: snapshot.caps,
        per_session,
        reserved_per_session,
        reservations,
        next_reservation_id: snapshot.next_reservation_id,
        session_extensions: snapshot
            .session_extensions
            .into_iter()
            .map(|(session_id, extension)| {
                (
                    session_id,
                    SessionExtension {
                        tokens: extension.tokens,
                        usd: extension.usd,
                    },
                )
            })
            .collect(),
        applied_budget_grants: snapshot
            .applied_budget_grants
            .into_iter()
            .map(|(session_id, grants)| (session_id, grants.into_iter().collect()))
            .collect(),
        blocked_sessions: snapshot.blocked_sessions.into_iter().collect(),
        per_user_daily: snapshot
            .per_user_daily
            .into_iter()
            .map(|(user_id, totals)| {
                (
                    user_id,
                    DailyTotals {
                        day_ordinal: totals.day_ordinal,
                        usd: totals.usd,
                    },
                )
            })
            .collect(),
        sink: None,
        restore_applied: true,
        restored_reservations,
    })
}

fn constrain_tracker_snapshot(
    mut snapshot: BudgetTrackerSnapshot,
    current_caps: BudgetCap,
) -> Result<BudgetTrackerSnapshot, crate::BudgetSnapshotError> {
    validate_tracker_snapshot(&snapshot)?;
    let current_caps = normalize_caps(current_caps);
    let captured_caps = snapshot.caps.clone();

    for extension in snapshot.session_extensions.values_mut() {
        extension.tokens = clamp_token_extension(&captured_caps, &current_caps, extension.tokens);
        extension.usd = clamp_usd_extension(
            captured_caps.per_session_usd,
            current_caps.per_session_usd,
            extension.usd,
        );
    }
    snapshot
        .session_extensions
        .retain(|_, extension| extension.tokens != 0 || extension.usd != 0.0);
    snapshot.caps = intersect_caps(&captured_caps, &current_caps);
    validate_tracker_snapshot(&snapshot)?;
    Ok(snapshot)
}

fn normalize_caps(mut caps: BudgetCap) -> BudgetCap {
    for cap in [&mut caps.per_session_usd, &mut caps.per_user_daily_usd] {
        if cap.is_some_and(|usd| !usd.is_finite() || usd < 0.0) {
            *cap = Some(0.0);
        }
    }
    caps
}

fn intersect_caps(captured: &BudgetCap, current: &BudgetCap) -> BudgetCap {
    BudgetCap {
        per_session_tokens: intersect_optional(
            captured.per_session_tokens,
            current.per_session_tokens,
        ),
        per_session_input_tokens: intersect_optional(
            captured.per_session_input_tokens,
            current.per_session_input_tokens,
        ),
        per_session_output_tokens: intersect_optional(
            captured.per_session_output_tokens,
            current.per_session_output_tokens,
        ),
        per_session_usd: intersect_optional_f64(captured.per_session_usd, current.per_session_usd),
        per_user_daily_usd: intersect_optional_f64(
            captured.per_user_daily_usd,
            current.per_user_daily_usd,
        ),
    }
}

fn intersect_optional<T: Ord + Copy>(captured: Option<T>, current: Option<T>) -> Option<T> {
    match (captured, current) {
        (Some(captured), Some(current)) => Some(captured.min(current)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn intersect_optional_f64(captured: Option<f64>, current: Option<f64>) -> Option<f64> {
    match (captured, current) {
        (Some(captured), Some(current)) => Some(captured.min(current)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn clamp_token_extension(captured: &BudgetCap, current: &BudgetCap, extension: u64) -> u64 {
    [
        extension_headroom_u64(
            captured.per_session_tokens,
            current.per_session_tokens,
            extension,
        ),
        extension_headroom_u64(
            captured.per_session_input_tokens,
            current.per_session_input_tokens,
            extension,
        ),
        extension_headroom_u64(
            captured.per_session_output_tokens,
            current.per_session_output_tokens,
            extension,
        ),
    ]
    .into_iter()
    .flatten()
    .fold(extension, u64::min)
}

fn extension_headroom_u64(
    captured_base: Option<u64>,
    current_cap: Option<u64>,
    extension: u64,
) -> Option<u64> {
    let captured_effective = captured_base.map(|base| base.saturating_add(extension));
    let target = intersect_optional(captured_effective, current_cap);
    let base = intersect_optional(captured_base, current_cap);
    match (target, base) {
        (Some(target), Some(base)) => Some(target.saturating_sub(base)),
        _ => None,
    }
}

fn clamp_usd_extension(
    captured_base: Option<f64>,
    current_cap: Option<f64>,
    extension: f64,
) -> f64 {
    let captured_effective = captured_base.and_then(|base| checked_usd_add(base, extension));
    let target = intersect_optional_f64(captured_effective, current_cap);
    let base = intersect_optional_f64(captured_base, current_cap);
    match (target, base) {
        (Some(target), Some(base)) => (target - base).max(0.0).min(extension),
        _ => extension,
    }
}

fn validate_optional_usd(
    field: &str,
    value: Option<f64>,
) -> Result<(), crate::BudgetSnapshotError> {
    if let Some(value) = value {
        validate_usd(field, value)?;
    }
    Ok(())
}

fn validate_usd(field: &str, value: f64) -> Result<(), crate::BudgetSnapshotError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(invalid_snapshot(format!(
            "{field} must be finite and non-negative"
        )))
    }
}

fn invalid_snapshot(reason: impl Into<String>) -> crate::BudgetSnapshotError {
    crate::BudgetSnapshotError::Invalid {
        reason: reason.into(),
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

        assert_eq!(
            tracker.effective_session_limits("s1"),
            (Some(150), Some(1.5))
        );

        assert!(tracker.reserve("s1", 50, 0.50).is_ok());
        assert!(tracker.reserve("s2", 101, 0.0).is_err());
    }

    #[test]
    fn extension_rejects_effective_usd_cap_overflow() {
        let mut tracker = BudgetTracker::new(BudgetCap::builder().per_session_usd(1.0).build());
        assert!(tracker.reserve("s1", 0, 2.0).is_err());

        let err = tracker.extend_session("s1", 0, f64::MAX).unwrap_err();
        assert_eq!(err, BudgetExtensionError::InvalidUsd);
    }

    #[test]
    fn extension_failures_are_structured_not_display_string_kinds() {
        let mut tracker = BudgetTracker::new(BudgetCap::builder().per_session_tokens(1).build());
        assert_eq!(
            tracker.extend_session("s1", 1, 0.0),
            Err(BudgetExtensionError::NoExhaustedBudget)
        );

        assert!(tracker.reserve("s1", 2, 0.0).is_err());
        assert_eq!(
            tracker.extend_session("s1", 0, 0.0),
            Err(BudgetExtensionError::EmptyExtension)
        );
        assert_eq!(
            tracker.extend_session("s1", 1, f64::NAN),
            Err(BudgetExtensionError::InvalidUsd)
        );
    }

    #[test]
    fn idempotent_extension_replays_after_snapshot_restore_without_mutation() {
        let mut tracker = BudgetTracker::new(BudgetCap::builder().per_session_tokens(100).build());
        assert!(tracker.reserve("s1", 101, 0.0).is_err());
        assert_eq!(
            tracker
                .extend_session_idempotent("s1", "grant-001", 50, 0.0)
                .unwrap(),
            BudgetExtensionOutcome::Applied
        );
        assert_eq!(tracker.effective_session_limits("s1").0, Some(150));

        let snapshot = tracker.snapshot().unwrap();
        let mut reopened = BudgetTracker::from_snapshot(snapshot).unwrap();
        assert_eq!(
            reopened
                .extend_session_idempotent("s1", "grant-001", 50, 0.0)
                .unwrap(),
            BudgetExtensionOutcome::AlreadyApplied
        );
        assert_eq!(reopened.effective_session_limits("s1").0, Some(150));
        assert_eq!(
            reopened.extend_session_idempotent("s1", "grant-001", 51, 0.0),
            Err(BudgetExtensionError::RequestIdConflict)
        );
    }

    #[test]
    fn durable_extension_ledger_is_bounded_without_evicting_receipts() {
        let mut tracker = BudgetTracker::new(BudgetCap::builder().per_session_tokens(1).build());
        for index in 0..MAX_DURABLE_BUDGET_GRANTS_PER_SESSION {
            let cap = tracker.effective_session_limits("s1").0.unwrap();
            assert!(tracker.reserve("s1", cap.saturating_add(1), 0.0).is_err());
            assert_eq!(
                tracker
                    .extend_session_idempotent("s1", &format!("grant-{index}"), 1, 0.0)
                    .unwrap(),
                BudgetExtensionOutcome::Applied
            );
        }

        let cap = tracker.effective_session_limits("s1").0.unwrap();
        assert!(tracker.reserve("s1", cap.saturating_add(1), 0.0).is_err());
        assert_eq!(
            tracker.extend_session_idempotent("s1", "grant-overflow", 1, 0.0),
            Err(BudgetExtensionError::GrantLedgerCapacityExceeded)
        );
        assert_eq!(
            tracker
                .extend_session_idempotent("s1", "grant-0", 1, 0.0)
                .unwrap(),
            BudgetExtensionOutcome::AlreadyApplied
        );
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

    #[test]
    fn tracker_snapshot_json_roundtrip_preserves_enforcement_authority() {
        let caps = BudgetCap::builder()
            .per_session_tokens(100)
            .per_session_usd(1.0)
            .build();
        let mut tracker = BudgetTracker::new(caps);
        tracker.charge("committed", 60, 0.60).unwrap();
        tracker.reserve("committed", 20, 0.20).unwrap();
        assert!(tracker.reserve("blocked", 101, 0.0).is_err());
        tracker.charge("extended", 100, 1.0).unwrap();
        assert!(tracker.reserve("extended", 1, 0.01).is_err());
        tracker.extend_session("extended", 50, 0.50).unwrap();

        let snapshot = tracker.snapshot().unwrap();
        let json = serde_json::to_vec(&snapshot).unwrap();
        let wire: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert!(wire["reservations"].is_array());
        let decoded: BudgetTrackerSnapshot = serde_json::from_slice(&json).unwrap();
        let mut restored = BudgetTracker::from_snapshot(decoded.clone()).unwrap();

        assert_eq!(restored.snapshot().unwrap(), decoded);
        assert_eq!(restored.session_totals("committed"), (60, 0.60));
        assert_eq!(restored.reserved_totals("committed"), (20, 0.20));
        assert!(restored.reserve("blocked", 1, 0.0).is_err());
        assert!(restored.reserve("committed", 21, 0.21).is_err());
        assert!(restored.reserve("extended", 50, 0.50).is_ok());
    }

    #[test]
    fn version_one_snapshot_without_grant_ledger_migrates_to_empty() {
        let snapshot = BudgetTracker::new(BudgetCap::default()).snapshot().unwrap();
        let mut wire = serde_json::to_value(snapshot).unwrap();
        wire.as_object_mut()
            .unwrap()
            .remove("applied_budget_grants");

        let migrated: BudgetTrackerSnapshot = serde_json::from_value(wire).unwrap();
        assert!(migrated.applied_budget_grants.is_empty());
        let restored = BudgetTracker::from_snapshot(migrated).unwrap();
        assert!(
            restored
                .snapshot()
                .unwrap()
                .applied_budget_grants
                .is_empty()
        );
    }

    #[test]
    fn tracker_snapshot_rejects_duplicate_reservation_ids_on_the_wire() {
        let mut tracker = BudgetTracker::new(BudgetCap::default());
        tracker.reserve("session", 1, 0.25).unwrap();
        let mut wire = serde_json::to_value(tracker.snapshot().unwrap()).unwrap();
        let duplicate = wire["reservations"][0].clone();
        wire["reservations"].as_array_mut().unwrap().push(duplicate);

        assert!(serde_json::from_value::<BudgetTrackerSnapshot>(wire).is_err());
    }

    #[test]
    fn tracker_pristine_state_is_typed_and_independent_of_snapshot_wire_shape() {
        let mut tracker = BudgetTracker::new(BudgetCap::default());
        assert!(tracker.is_pristine());

        tracker.reserve("session", 1, 0.25).unwrap();
        assert!(!tracker.is_pristine());
    }

    #[test]
    fn tracker_refuses_duplicate_snapshot_restore() {
        let snapshot = BudgetTracker::new(BudgetCap::default()).snapshot().unwrap();
        let mut target = BudgetTracker::new(BudgetCap::default());

        target.restore_snapshot(snapshot.clone()).unwrap();
        assert_eq!(
            target.restore_snapshot(snapshot).unwrap_err(),
            crate::BudgetSnapshotError::RestoreTargetNotPristine
        );
    }

    #[test]
    fn tracker_snapshot_rejects_nonfinite_and_negative_usd() {
        let mut cap_snapshot = BudgetTracker::new(BudgetCap::default()).snapshot().unwrap();
        cap_snapshot.caps.per_session_usd = Some(f64::NAN);
        assert!(matches!(
            BudgetTracker::from_snapshot(cap_snapshot),
            Err(crate::BudgetSnapshotError::Invalid { .. })
        ));

        let mut tracker = BudgetTracker::new(BudgetCap::default());
        tracker.charge("session", 1, 0.25).unwrap();
        let mut committed_snapshot = tracker.snapshot().unwrap();
        committed_snapshot
            .per_session
            .get_mut("session")
            .unwrap()
            .usd = f64::INFINITY;
        assert!(matches!(
            BudgetTracker::from_snapshot(committed_snapshot),
            Err(crate::BudgetSnapshotError::Invalid { .. })
        ));

        let mut tracker = BudgetTracker::new(BudgetCap::default());
        tracker.reserve("session", 1, 0.25).unwrap();
        let mut reservation_snapshot = tracker.snapshot().unwrap();
        reservation_snapshot
            .reservations
            .values_mut()
            .next()
            .unwrap()
            .usd = -0.01;
        assert!(matches!(
            BudgetTracker::from_snapshot(reservation_snapshot),
            Err(crate::BudgetSnapshotError::Invalid { .. })
        ));
    }

    #[test]
    fn restored_inflight_reservation_remains_conservative_until_reconciled() {
        let mut tracker = BudgetTracker::new(
            BudgetCap::builder()
                .per_session_tokens(100)
                .per_session_usd(1.0)
                .build(),
        );
        let reservation = tracker.reserve("session", 80, 0.80).unwrap();
        let encoded_reservation = serde_json::to_vec(&reservation).unwrap();
        let snapshot = tracker.snapshot().unwrap();

        let mut restored = BudgetTracker::from_snapshot(snapshot).unwrap();
        assert!(restored.reserve("session", 21, 0.21).is_err());
        let reservation: BudgetReservation = serde_json::from_slice(&encoded_reservation).unwrap();
        restored.settle(reservation, 20, 0.20).unwrap();
        assert_eq!(restored.reserved_totals("session"), (0, 0.0));
        assert_eq!(restored.session_totals("session"), (20, 0.20));
    }

    #[test]
    fn current_caps_intersect_every_axis_and_clamp_durable_extensions() {
        let captured_caps = BudgetCap {
            per_session_tokens: Some(100),
            per_session_input_tokens: Some(80),
            per_session_output_tokens: None,
            per_session_usd: Some(1.0),
            per_user_daily_usd: None,
        };
        let mut tracker = BudgetTracker::new(captured_caps);
        tracker.charge("used", 60, 0.60).unwrap();
        assert!(tracker.reserve("blocked", 101, 0.0).is_err());
        tracker.charge("extended", 100, 1.0).unwrap();
        assert!(tracker.reserve("extended", 1, 0.01).is_err());
        tracker.extend_session("extended", 50, 0.50).unwrap();

        let current_caps = BudgetCap {
            per_session_tokens: Some(120),
            per_session_input_tokens: Some(70),
            per_session_output_tokens: Some(30),
            per_session_usd: Some(1.20),
            per_user_daily_usd: Some(0.40),
        };
        let mut restored = BudgetTracker::from_snapshot_with_current_caps(
            tracker.snapshot().unwrap(),
            current_caps,
        )
        .unwrap();
        let restored_snapshot = restored.snapshot().unwrap();

        assert_eq!(restored_snapshot.caps.per_session_tokens, Some(100));
        assert_eq!(restored_snapshot.caps.per_session_input_tokens, Some(70));
        assert_eq!(restored_snapshot.caps.per_session_output_tokens, Some(30));
        assert_eq!(restored_snapshot.caps.per_session_usd, Some(1.0));
        assert_eq!(restored_snapshot.caps.per_user_daily_usd, Some(0.40));
        let extension = restored_snapshot
            .session_extensions
            .get("extended")
            .unwrap();
        assert_eq!(extension.tokens, 0);
        assert!((extension.usd - 0.20).abs() < f64::EPSILON * 4.0);
        assert_eq!(restored.session_totals("used"), (60, 0.60));
        assert!(restored.reserve("blocked", 1, 0.0).is_err());
    }

    #[test]
    fn unmanaged_restore_intersects_base_caps_but_preserves_committed_extension() {
        let mut tracker = BudgetTracker::new(BudgetCap::builder().per_session_tokens(100).build());
        assert!(tracker.reserve("session", 101, 0.0).is_err());
        tracker
            .extend_session_idempotent("session", "grant-001", 50, 0.0)
            .unwrap();

        let mut restored = BudgetTracker::from_snapshot_with_current_caps_preserving_extensions(
            tracker.snapshot().unwrap(),
            BudgetCap::builder().per_session_tokens(100).build(),
        )
        .unwrap();

        assert_eq!(restored.effective_session_limits("session").0, Some(150));
        assert_eq!(
            restored
                .extend_session_idempotent("session", "grant-001", 50, 0.0)
                .unwrap(),
            BudgetExtensionOutcome::AlreadyApplied
        );
    }

    #[test]
    fn restart_reconciliation_settles_every_restored_reservation_at_maximum() {
        let mut tracker = BudgetTracker::new(
            BudgetCap::builder()
                .per_session_tokens(100)
                .per_session_usd(1.0)
                .build(),
        );
        tracker.reserve("over-current", 80, 0.80).unwrap();
        tracker.reserve("within-current", 20, 0.20).unwrap();
        let mut restored = BudgetTracker::from_snapshot_with_current_caps(
            tracker.snapshot().unwrap(),
            BudgetCap::builder()
                .per_session_tokens(50)
                .per_session_usd(0.50)
                .build(),
        )
        .unwrap();

        let report = restored.reconcile_restored_reservations_conservatively();

        assert_eq!(report.reservations_settled, 2);
        assert_eq!(report.input_tokens_charged, 100);
        assert_eq!(report.output_tokens_charged, 0);
        assert!((report.cost_usd_charged - 1.0).abs() < f64::EPSILON);
        assert_eq!(report.cap_errors.len(), 1);
        assert_eq!(restored.reserved_totals("over-current"), (0, 0.0));
        assert_eq!(restored.reserved_totals("within-current"), (0, 0.0));
        assert_eq!(restored.session_totals("over-current"), (80, 0.80));
        assert_eq!(restored.session_totals("within-current"), (20, 0.20));
        assert!(restored.reserve("over-current", 1, 0.0).is_err());
        let repeated = restored.reconcile_restored_reservations_conservatively();
        assert_eq!(repeated.reservations_settled, 0);
        assert_eq!(repeated.input_tokens_charged, 0);
        assert_eq!(repeated.output_tokens_charged, 0);
        assert_eq!(repeated.cost_usd_charged, 0.0);
        assert!(repeated.cap_errors.is_empty());
    }
}
