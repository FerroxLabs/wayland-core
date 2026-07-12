//! Anvil cost ledger — one per-task-lineage budget with ATOMIC
//! reservation-before-dispatch (A1.4 slice, spec §7).
//!
//! Every provider call and gate execution reserves capacity from the ledger
//! ATOMICALLY before it dispatches, so parallel builders can never race a
//! check-then-launch past the budget. Actuals reconcile after
//! ([`ClimbLedger::settle`]); an exhausted reservation refuses, and the caller
//! cancels the undispatched work. The ledger covers BOTH axes the climb
//! spends — provider **tokens/cost** and gate **exec wallclock** (test suites
//! are a real cost, spec §7) — and reports reserved-vs-settled totals plus the
//! `priced` flag the receipt needs (spec §2 honesty vocabulary: unpriced spend
//! renders "unpriced", never `$0`).
//!
//! ## Shared envelope, not a shared type
//! Spec §1 requires Anvil and Crucible to share ONE spend envelope per task so
//! the two can never stack spend on a goal. That is a VALUE contract — identical
//! units (`u64` microcents + a `priced: bool`, exactly what `CouncilSpend` and
//! `ProtocolEvent::AnvilReceipt` already speak) — not type inheritance. The
//! ledger therefore composes those units rather than extending council's
//! `CouncilSpend` (whose accumulation is bound to council's proposal model).
//!
//! ## Scope
//! A1.4 is the ledger PRIMITIVE + its accounting. The climb loop that actually
//! wraps each dispatch in reserve→settle, carries residual budget into
//! escalation, journals per-iteration spend, and emits the receipt lands with
//! the climb slice (A1.5).

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use wcore_types::crucible::MICROCENTS_PER_USD;

/// Which budget axis a reservation tripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Provider cost, in microcents.
    Cost,
    /// Gate/provider exec wallclock.
    Wallclock,
}

impl std::fmt::Display for Axis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Axis::Cost => write!(f, "cost (microcents)"),
            Axis::Wallclock => write!(f, "wallclock (ms)"),
        }
    }
}

/// Reserving would exceed the per-task budget on `axis`. The caller cancels the
/// undispatched work rather than overspending. Values are microcents for
/// [`Axis::Cost`] and milliseconds for [`Axis::Wallclock`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "climb budget exhausted on {axis}: committed {committed} + requested {requested} exceeds cap {cap}"
)]
pub struct Exhausted {
    pub axis: Axis,
    pub committed: u64,
    pub requested: u64,
    pub cap: u64,
}

/// What kind of dispatch a settled entry accounts for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerEntryKind {
    /// A model/provider call (has tokens + a possibly-priced cost).
    ProviderCall,
    /// A gate execution (its cost is exec wallclock; tokens are zero).
    GateExec,
}

/// One settled dispatch — the anvil analog of council's `ProviderSpend`,
/// extended with the exec-wallclock axis and a gate-execution kind.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerEntry {
    pub kind: LedgerEntryKind,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Cost in microcents (0 when unpriced — see `priced`).
    pub cost_microcents: u64,
    /// Whether `cost_microcents` is a real, metered catalog price. A single
    /// unpriced entry makes the whole climb's settled spend unpriced.
    pub priced: bool,
    /// Exec wallclock this dispatch consumed.
    pub wallclock: Duration,
}

impl LedgerEntry {
    /// A settled provider call.
    #[must_use]
    pub fn provider_call(
        provider: impl Into<String>,
        model: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
        cost_microcents: u64,
        priced: bool,
        wallclock: Duration,
    ) -> Self {
        Self {
            kind: LedgerEntryKind::ProviderCall,
            provider: Some(provider.into()),
            model,
            input_tokens,
            output_tokens,
            cost_microcents,
            priced,
            wallclock,
        }
    }

    /// A settled gate execution — cost is its wallclock; no tokens, and it is
    /// `priced` (wallclock is always a real, metered cost).
    #[must_use]
    pub fn gate_exec(wallclock: Duration) -> Self {
        Self {
            kind: LedgerEntryKind::GateExec,
            provider: None,
            model: None,
            input_tokens: 0,
            output_tokens: 0,
            cost_microcents: 0,
            priced: true,
            wallclock,
        }
    }
}

/// The per-task budget ceiling. Either axis may be uncapped (`None`); §7's
/// "conservative configured upper bound" is expressed by supplying a cap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LedgerCap {
    pub max_microcents: Option<u64>,
    pub max_wallclock: Option<Duration>,
}

impl LedgerCap {
    /// No ceiling on either axis (reservations never exhaust).
    #[must_use]
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// A cost-only ceiling.
    #[must_use]
    pub fn cost(max_microcents: u64) -> Self {
        Self {
            max_microcents: Some(max_microcents),
            max_wallclock: None,
        }
    }
}

/// The settled-spend rollup for the receipt (spec §8) and the future
/// `ClimbResult.spend`. `cost_microcents` + `priced` match
/// `ProtocolEvent::AnvilReceipt` byte-for-byte.
#[derive(Debug, Clone, PartialEq)]
pub struct SettledSpend {
    pub cost_microcents: u64,
    /// False if ANY settled entry was unpriced (spec §2: render "unpriced").
    pub priced: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub wallclock: Duration,
}

impl SettledSpend {
    /// Settled cost in USD (display only — accounting stays integer microcents).
    #[must_use]
    pub fn cost_usd(&self) -> f64 {
        self.cost_microcents as f64 / MICROCENTS_PER_USD
    }
}

/// A granted reservation. Holding it keeps `est` committed against the budget;
/// [`ClimbLedger::settle`] reconciles it with actuals (releasing the estimate
/// and charging the real cost), and DROPPING it without settling releases the
/// estimate (the dispatch was cancelled). Either way the estimate never leaks.
#[derive(Debug)]
#[must_use = "a reservation must be settled or dropped; leaking it holds budget"]
pub struct Reservation {
    inner: Arc<Mutex<Inner>>,
    est_microcents: u64,
    est_wallclock: Duration,
    /// True while this reservation still holds its estimate outstanding.
    live: bool,
}

impl Reservation {
    /// The reserved cost estimate (microcents).
    #[must_use]
    pub fn est_microcents(&self) -> u64 {
        self.est_microcents
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        // Released without settling ⇒ the dispatch was cancelled; return the
        // estimate to the budget. `settle` clears `live` first, so it never
        // double-releases (and never re-locks — parking_lot is not reentrant).
        if self.live {
            let mut inner = self.inner.lock();
            inner.outstanding_microcents = inner
                .outstanding_microcents
                .saturating_sub(self.est_microcents);
            inner.outstanding_wallclock = inner
                .outstanding_wallclock
                .saturating_sub(self.est_wallclock);
        }
    }
}

#[derive(Debug)]
struct Inner {
    task_id: String,
    cap: LedgerCap,
    /// Sum of granted-but-not-yet-settled reservation estimates.
    outstanding_microcents: u64,
    outstanding_wallclock: Duration,
    /// Sum of settled actuals.
    settled_microcents: u64,
    settled_wallclock: Duration,
    total_input_tokens: u64,
    total_output_tokens: u64,
    /// True until an unpriced entry settles.
    all_priced: bool,
    entries: Vec<LedgerEntry>,
}

/// A single per-task-lineage cost ledger. Cheap to [`Clone`] (shares one locked
/// core) so every parallel builder reserves against the SAME budget — the lock
/// makes reservation atomic, so no two builders can both pass a check-then-launch
/// past the ceiling.
#[derive(Clone)]
pub struct ClimbLedger {
    inner: Arc<Mutex<Inner>>,
}

impl ClimbLedger {
    /// A fresh ledger for `task_id` (the lineage key) with budget `cap`.
    #[must_use]
    pub fn new(task_id: impl Into<String>, cap: LedgerCap) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                task_id: task_id.into(),
                cap,
                outstanding_microcents: 0,
                outstanding_wallclock: Duration::ZERO,
                settled_microcents: 0,
                settled_wallclock: Duration::ZERO,
                total_input_tokens: 0,
                total_output_tokens: 0,
                all_priced: true,
                entries: Vec::new(),
            })),
        }
    }

    /// Atomically reserve capacity BEFORE dispatch. Rejects with [`Exhausted`]
    /// when the reservation would push committed spend (settled + outstanding)
    /// past a capped axis; otherwise the estimate is held outstanding until the
    /// returned [`Reservation`] is settled or dropped. This single locked op is
    /// the race-free gate spec §7 requires.
    pub fn reserve(
        &self,
        est_microcents: u64,
        est_wallclock: Duration,
    ) -> Result<Reservation, Exhausted> {
        let mut inner = self.inner.lock();

        if let Some(cap) = inner.cap.max_microcents {
            let committed = inner
                .settled_microcents
                .saturating_add(inner.outstanding_microcents);
            if committed.saturating_add(est_microcents) > cap {
                return Err(Exhausted {
                    axis: Axis::Cost,
                    committed,
                    requested: est_microcents,
                    cap,
                });
            }
        }
        if let Some(cap) = inner.cap.max_wallclock {
            let committed = inner
                .settled_wallclock
                .saturating_add(inner.outstanding_wallclock);
            if committed.saturating_add(est_wallclock) > cap {
                return Err(Exhausted {
                    axis: Axis::Wallclock,
                    committed: dur_ms(committed),
                    requested: dur_ms(est_wallclock),
                    cap: dur_ms(cap),
                });
            }
        }

        inner.outstanding_microcents = inner.outstanding_microcents.saturating_add(est_microcents);
        inner.outstanding_wallclock = inner.outstanding_wallclock.saturating_add(est_wallclock);
        Ok(Reservation {
            inner: Arc::clone(&self.inner),
            est_microcents,
            est_wallclock,
            live: true,
        })
    }

    /// Reconcile a reservation with the actual spend: release its estimate and
    /// charge the real `actual` (which may be more or less than the estimate).
    pub fn settle(&self, mut reservation: Reservation, actual: LedgerEntry) {
        let mut inner = self.inner.lock();
        inner.outstanding_microcents = inner
            .outstanding_microcents
            .saturating_sub(reservation.est_microcents);
        inner.outstanding_wallclock = inner
            .outstanding_wallclock
            .saturating_sub(reservation.est_wallclock);

        inner.settled_microcents = inner
            .settled_microcents
            .saturating_add(actual.cost_microcents);
        inner.settled_wallclock = inner.settled_wallclock.saturating_add(actual.wallclock);
        inner.total_input_tokens = inner.total_input_tokens.saturating_add(actual.input_tokens);
        inner.total_output_tokens = inner
            .total_output_tokens
            .saturating_add(actual.output_tokens);
        if !actual.priced {
            inner.all_priced = false;
        }
        inner.entries.push(actual);

        // Cleared BEFORE the guard drops so `Reservation::drop` sees it settled
        // and does not re-lock (parking_lot is not reentrant) or double-release.
        reservation.live = false;
    }

    /// The lineage/task id this ledger accounts for.
    #[must_use]
    pub fn task_id(&self) -> String {
        self.inner.lock().task_id.clone()
    }

    /// Total settled cost (microcents).
    #[must_use]
    pub fn settled_microcents(&self) -> u64 {
        self.inner.lock().settled_microcents
    }

    /// Currently outstanding (reserved-but-unsettled) cost estimate (microcents).
    #[must_use]
    pub fn outstanding_microcents(&self) -> u64 {
        self.inner.lock().outstanding_microcents
    }

    /// Remaining cost budget (cap − committed), or `None` when cost is uncapped.
    #[must_use]
    pub fn residual_microcents(&self) -> Option<u64> {
        let inner = self.inner.lock();
        inner.cap.max_microcents.map(|cap| {
            let committed = inner
                .settled_microcents
                .saturating_add(inner.outstanding_microcents);
            cap.saturating_sub(committed)
        })
    }

    /// Whether any capped axis has no residual budget left.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        let inner = self.inner.lock();
        let cost_out = matches!(inner.cap.max_microcents, Some(cap)
            if inner.settled_microcents.saturating_add(inner.outstanding_microcents) >= cap);
        let wall_out = matches!(inner.cap.max_wallclock, Some(cap)
            if inner.settled_wallclock.saturating_add(inner.outstanding_wallclock) >= cap);
        cost_out || wall_out
    }

    /// The settled-spend rollup for the receipt / `ClimbResult.spend`.
    #[must_use]
    pub fn settled(&self) -> SettledSpend {
        let inner = self.inner.lock();
        SettledSpend {
            cost_microcents: inner.settled_microcents,
            priced: inner.all_priced,
            input_tokens: inner.total_input_tokens,
            output_tokens: inner.total_output_tokens,
            wallclock: inner.settled_wallclock,
        }
    }

    /// A snapshot of every settled entry (for the climb journal, A1.5).
    #[must_use]
    pub fn entries(&self) -> Vec<LedgerEntry> {
        self.inner.lock().entries.clone()
    }
}

/// Duration → whole milliseconds as `u64` (saturating). Used only for the
/// wallclock figures in [`Exhausted`]; accounting keeps the full `Duration`.
fn dur_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(cost: u64, priced: bool) -> LedgerEntry {
        LedgerEntry::provider_call(
            "anthropic",
            None,
            10,
            20,
            cost,
            priced,
            Duration::from_millis(5),
        )
    }

    #[test]
    fn reserve_within_cost_cap_succeeds_and_exceeding_is_exhausted() {
        let l = ClimbLedger::new("task-1", LedgerCap::cost(1000));
        let r1 = l.reserve(600, Duration::ZERO).expect("600 fits under 1000");
        assert_eq!(l.outstanding_microcents(), 600);
        assert_eq!(l.residual_microcents(), Some(400));

        // A second reservation must see the first's outstanding commitment —
        // this is the race-free property: 600 + 500 > 1000.
        let err = l
            .reserve(500, Duration::ZERO)
            .expect_err("500 more overflows");
        assert_eq!(err.axis, Axis::Cost);
        assert_eq!(err.committed, 600);

        // Exactly hitting the cap is allowed.
        let _r2 = l
            .reserve(400, Duration::ZERO)
            .expect("600 + 400 == cap is allowed");
        assert_eq!(l.residual_microcents(), Some(0));
        assert!(l.is_exhausted());
        drop(r1);
    }

    #[test]
    fn wallclock_cap_is_enforced_independently() {
        let l = ClimbLedger::new(
            "task-w",
            LedgerCap {
                max_microcents: None,
                max_wallclock: Some(Duration::from_secs(10)),
            },
        );
        let _r = l.reserve(0, Duration::from_secs(7)).expect("7s fits");
        let err = l
            .reserve(0, Duration::from_secs(4))
            .expect_err("7s + 4s > 10s");
        assert_eq!(err.axis, Axis::Wallclock);
        assert_eq!(err.cap, 10_000, "cap reported in ms");
    }

    #[test]
    fn settle_releases_estimate_and_charges_actual() {
        let l = ClimbLedger::new("task-s", LedgerCap::cost(1000));
        let r = l.reserve(600, Duration::ZERO).unwrap();
        // Actual came in UNDER the estimate: only the actual is charged, the
        // reservation delta is released back.
        l.settle(r, call(500, true));
        assert_eq!(l.settled_microcents(), 500);
        assert_eq!(l.outstanding_microcents(), 0);
        assert_eq!(l.residual_microcents(), Some(500));
    }

    #[test]
    fn dropping_a_reservation_without_settling_releases_it() {
        let l = ClimbLedger::new("task-d", LedgerCap::cost(1000));
        {
            let _r = l.reserve(700, Duration::ZERO).unwrap();
            assert_eq!(l.residual_microcents(), Some(300));
        } // dropped here without settling ⇒ cancelled dispatch
        assert_eq!(l.outstanding_microcents(), 0);
        assert_eq!(l.residual_microcents(), Some(1000));
    }

    #[test]
    fn unpriced_entry_makes_whole_climb_unpriced() {
        let l = ClimbLedger::new("task-p", LedgerCap::unlimited());
        let r1 = l.reserve(100, Duration::ZERO).unwrap();
        l.settle(r1, call(100, true));
        assert!(l.settled().priced, "all priced so far");
        let r2 = l.reserve(0, Duration::ZERO).unwrap();
        l.settle(r2, call(0, false)); // catalog miss
        let s = l.settled();
        assert!(!s.priced, "one unpriced entry ⇒ climb unpriced (spec §2)");
        assert_eq!(s.cost_microcents, 100);
    }

    #[test]
    fn unlimited_cap_never_exhausts_and_has_no_residual() {
        let l = ClimbLedger::new("task-u", LedgerCap::unlimited());
        let r = l.reserve(u64::MAX / 2, Duration::from_secs(3600)).unwrap();
        assert_eq!(l.residual_microcents(), None);
        assert!(!l.is_exhausted());
        l.settle(r, call(u64::MAX / 2, true));
        assert!(!l.is_exhausted());
    }

    #[test]
    fn settled_rollup_accumulates_tokens_cost_and_wallclock() {
        let l = ClimbLedger::new("task-r", LedgerCap::unlimited());
        let r1 = l.reserve(100, Duration::from_millis(10)).unwrap();
        l.settle(
            r1,
            LedgerEntry::provider_call("a", None, 5, 7, 100, true, Duration::from_millis(10)),
        );
        let r2 = l.reserve(0, Duration::from_millis(20)).unwrap();
        l.settle(r2, LedgerEntry::gate_exec(Duration::from_millis(20)));
        let s = l.settled();
        assert_eq!(s.cost_microcents, 100);
        assert_eq!(s.input_tokens, 5);
        assert_eq!(s.output_tokens, 7);
        assert_eq!(s.wallclock, Duration::from_millis(30));
        assert_eq!(l.entries().len(), 2);
        assert_eq!(l.task_id(), "task-r");
    }

    #[test]
    fn cost_usd_converts_microcents() {
        // 1 USD = 100_000_000 microcents.
        let s = SettledSpend {
            cost_microcents: 100_000_000,
            priced: true,
            input_tokens: 0,
            output_tokens: 0,
            wallclock: Duration::ZERO,
        };
        assert!((s.cost_usd() - 1.0).abs() < 1e-9);
    }

    /// Atomicity under real parallelism: N threads each try to reserve the same
    /// unit from a shared ledger whose cap admits exactly K. EXACTLY K may
    /// succeed — the lock makes reservation race-free, so the ceiling is never
    /// over-committed.
    #[test]
    fn parallel_reservations_never_exceed_the_cap() {
        const UNIT: u64 = 100;
        const K: u64 = 8;
        const THREADS: usize = 32;
        let l = ClimbLedger::new("task-race", LedgerCap::cost(UNIT * K));

        // Collect granted reservations so they stay OUTSTANDING (dropping them
        // would release their estimate and defeat the assertion).
        let granted: Arc<Mutex<Vec<Reservation>>> = Arc::new(Mutex::new(Vec::new()));
        std::thread::scope(|scope| {
            for _ in 0..THREADS {
                let l = l.clone();
                let granted = Arc::clone(&granted);
                scope.spawn(move || {
                    if let Ok(r) = l.reserve(UNIT, Duration::ZERO) {
                        granted.lock().push(r);
                    }
                });
            }
        });

        assert_eq!(
            granted.lock().len() as u64,
            K,
            "exactly the cap's worth of reservations may be granted"
        );
        assert_eq!(l.outstanding_microcents(), UNIT * K);
        assert!(l.is_exhausted());
    }
}
