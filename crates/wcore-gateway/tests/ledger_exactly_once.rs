//! F24-01 Task 2 — delivery is exactly once across a lifecycle event.
//!
//! Written BEFORE `src/ledger.rs` and `src/drain.rs` existed.
//!
//! Every count in this file comes from an INDEPENDENT SINK, never from the
//! ledger's own view of itself. A ledger that grades its own homework
//! proves nothing: the failure mode this criterion guards against is
//! precisely a ledger that believes it settled something the destination
//! never received, or that re-performs something the destination already
//! served.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use wcore_gateway::drain::{DrainController, DrainOutcome};
use wcore_gateway::ledger::{Accept, DeliveryLedger, DeliveryState};

/// An independent destination. It is NOT the ledger and shares no state
/// with it.
///
/// `deliver` appends the id to a log on every delivery it actually
/// performs, and REFUSES an id it has already served — which is what an
/// idempotency key buys at a real endpoint. The two numbers the criterion
/// cares about are therefore both readable from this sink alone: how many
/// deliveries happened (lines) and how many distinct ones (unique ids).
struct Sink {
    log: PathBuf,
    served: HashSet<String>,
}

impl Sink {
    fn new(dir: &Path) -> Self {
        Self {
            log: dir.join("sink.log"),
            served: HashSet::new(),
        }
    }

    /// Returns true when this call actually delivered; false when the sink
    /// recognised the idempotency key and suppressed a duplicate.
    fn deliver(&mut self, id: &str) -> bool {
        if !self.served.insert(id.to_string()) {
            return false;
        }
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .expect("sink log");
        writeln!(f, "{id}").expect("sink append");
        true
    }

    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn unique(&self) -> usize {
        self.lines().into_iter().collect::<HashSet<_>>().len()
    }
}

const SUBMITTED: usize = 200;

fn ids() -> Vec<String> {
    (0..SUBMITTED).map(|i| format!("d-{i:04}")).collect()
}

/// THE CRITERION. Accept 200 deliveries, crash mid-attempt on a subset,
/// restart, and count at the independent sink: delivered equals submitted,
/// unique equals submitted, duplicates zero, losses zero.
#[test]
fn a_crash_mid_attempt_still_delivers_exactly_once_after_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut sink = Sink::new(dir.path());
    let ids = ids();

    // --- first process life -------------------------------------------------
    let mut ledger = DeliveryLedger::open(dir.path()).expect("open ledger");
    for id in &ids {
        assert!(
            matches!(ledger.accept(id).expect("accept"), Accept::Accepted),
            "a first accept of {id} must be Accepted"
        );
    }

    // The first 150 are attempted AND settled. The last 50 are attempted and
    // then the process dies before any of them settles — their outcome is
    // genuinely UNKNOWN, which is the only case a restart may retry.
    for id in ids.iter().take(150) {
        ledger.begin_attempt(id).expect("attempt");
        let delivered = sink.deliver(id);
        ledger.settle(id, delivered).expect("settle");
    }
    for id in ids.iter().skip(150) {
        ledger.begin_attempt(id).expect("attempt");
        // Half of these actually reached the destination before the crash;
        // the ledger cannot know which. This is the exact hazard: naive
        // retry of all of them duplicates half.
        if id.ends_with('0') || id.ends_with('1') || id.ends_with('2') {
            sink.deliver(id);
        }
    }

    // A crash. No drain, no flush, no settle — the process simply stops.
    drop(ledger);

    // --- restart ------------------------------------------------------------
    let mut ledger = DeliveryLedger::open(dir.path()).expect("reopen ledger");

    let pending = ledger.pending();
    assert_eq!(
        pending.len(),
        50,
        "exactly the 50 unknown-outcome deliveries must be pending after restart, \
         not the 150 that provably settled"
    );
    for id in ids.iter().take(150) {
        assert!(
            !pending.contains(id),
            "{id} settled before the crash and must NOT be retried"
        );
    }

    for id in &pending {
        ledger.begin_attempt(id).expect("attempt after restart");
        let delivered = sink.deliver(id);
        ledger.settle(id, delivered).expect("settle after restart");
    }

    // --- the tally, taken from the sink -------------------------------------
    let lines = sink.lines();
    let unique = sink.unique();
    assert_eq!(
        lines.len(),
        SUBMITTED,
        "delivered must equal submitted; duplicates = {}",
        lines.len().saturating_sub(unique)
    );
    assert_eq!(unique, SUBMITTED, "unique must equal submitted; losses > 0");
    assert_eq!(
        lines.len() - unique,
        0,
        "duplicates must be zero at the destination"
    );
    assert!(
        ledger.pending().is_empty(),
        "nothing may remain pending once every delivery settled"
    );
}

/// A delivery that PROVABLY completed is never re-performed. The ledger
/// distinguishes "attempted, outcome unknown" from "attempted and settled",
/// and only the first is a retry candidate.
#[test]
fn a_provably_settled_delivery_is_not_re_performed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut sink = Sink::new(dir.path());

    let mut ledger = DeliveryLedger::open(dir.path()).expect("open");
    ledger.accept("only").expect("accept");
    ledger.begin_attempt("only").expect("attempt");
    assert!(sink.deliver("only"));
    ledger.settle("only", true).expect("settle");
    drop(ledger);

    let ledger = DeliveryLedger::open(dir.path()).expect("reopen");
    assert!(
        ledger.pending().is_empty(),
        "a settled delivery must not be a retry candidate"
    );
    assert_eq!(ledger.state("only"), Some(DeliveryState::Settled));
    assert_eq!(sink.lines().len(), 1, "the sink saw exactly one delivery");
}

/// Re-accepting an id the ledger already knows is reported as a DUPLICATE
/// rather than silently creating a second delivery. This is the outbound
/// idempotency key: 24-03 routes channel sends through it and must not
/// build a second store.
#[test]
fn re_accepting_a_known_id_is_reported_as_a_duplicate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut ledger = DeliveryLedger::open(dir.path()).expect("open");
    assert!(matches!(ledger.accept("x").unwrap(), Accept::Accepted));
    assert!(matches!(ledger.accept("x").unwrap(), Accept::Duplicate));
    assert_eq!(ledger.pending().len(), 1, "a duplicate creates no new work");
}

/// The ledger is BOUNDED. Sustained delivery does not grow the journal
/// without limit: compaction drops settled records past the retention and
/// keeps every unsettled one, because dropping an unsettled record is a
/// lost delivery.
#[test]
fn the_ledger_is_bounded_under_sustained_delivery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut ledger = DeliveryLedger::open(dir.path()).expect("open");

    for i in 0..5_000 {
        let id = format!("s-{i}");
        ledger.accept(&id).unwrap();
        ledger.begin_attempt(&id).unwrap();
        ledger.settle(&id, true).unwrap();
    }
    // Two deliveries deliberately left unsettled — they must survive every
    // compaction no matter how old they are.
    ledger.accept("survivor-a").unwrap();
    ledger.accept("survivor-b").unwrap();
    ledger.begin_attempt("survivor-b").unwrap();

    let before = std::fs::metadata(DeliveryLedger::journal_path(dir.path()))
        .expect("journal")
        .len();
    ledger.compact(64).expect("compact");
    let after = std::fs::metadata(DeliveryLedger::journal_path(dir.path()))
        .expect("journal")
        .len();

    assert!(
        after < before / 4,
        "compaction must actually shrink the journal: {before} -> {after}"
    );

    // And the bound holds across a reopen, which is the only thing that
    // matters — an in-memory-only bound is not a bound.
    let reopened = DeliveryLedger::open(dir.path()).expect("reopen after compact");
    let pending = reopened.pending();
    assert!(
        pending.contains(&"survivor-a".to_string()) && pending.contains(&"survivor-b".to_string()),
        "compaction must never drop an unsettled delivery, got {pending:?}"
    );
    assert_eq!(pending.len(), 2);
}

/// Drain is a STATE, not a sleep, and its steps are ordered: admission
/// closes FIRST, the in-flight counts are published, the budget is
/// honoured, the ledger is flushed to a durable point, and only then may
/// the process exit.
#[test]
fn drain_closes_admission_first_then_reports_falling_counts_and_exits_clean() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut ledger = DeliveryLedger::open(dir.path()).expect("open");
    let ctl = DrainController::new();

    // Two turns in flight and three deliveries pending when drain starts.
    let mut t1 = ctl.begin_turn();
    let mut t2 = ctl.begin_turn();
    assert!(
        t1.is_some() && t2.is_some(),
        "an admitting gateway admits turns"
    );
    for id in ["p1", "p2", "p3"] {
        ledger.accept(id).unwrap();
    }

    assert!(ctl.is_admitting());
    ctl.close_admission();
    assert!(
        !ctl.is_admitting(),
        "admission must close before anything else"
    );
    assert!(
        ctl.begin_turn().is_none(),
        "a drained gateway must admit no new work"
    );

    // Finish the in-flight work and settle the pending deliveries as the
    // drain polls. `tick` is the injected clock: the drain never sleeps, so
    // the test is deterministic rather than timing-dependent.
    let mut step = 0u64;
    let report = ctl
        .drain(&mut ledger, 10_000, |ledger| {
            step += 1;
            match step {
                1 => drop(t1.take().expect("t1")),
                2 => drop(t2.take().expect("t2")),
                _ => {
                    for id in ["p1", "p2", "p3"] {
                        if ledger.state(id) != Some(DeliveryState::Settled) {
                            let _ = ledger.begin_attempt(id);
                            let _ = ledger.settle(id, true);
                        }
                    }
                }
            }
            step * 10 // elapsed ms, far inside the budget
        })
        .expect("drain");

    assert_eq!(report.outcome, DrainOutcome::Clean);
    assert!(
        report.abandoned.is_empty(),
        "a clean drain abandons nothing, got {:?}",
        report.abandoned
    );

    // The published trace must actually FALL — a drain that reports the
    // same numbers throughout has told an operator nothing.
    let first = report.trace.first().expect("a drain publishes its counts");
    let last = report.trace.last().expect("a drain publishes its counts");
    assert_eq!(first.turns_in_flight, 2);
    assert_eq!(first.deliveries_pending, 3);
    assert_eq!(last.turns_in_flight, 0);
    assert_eq!(last.deliveries_pending, 0);
}

/// A drain that exceeds its budget exits FORCED, names the work it
/// abandoned by identity, and is distinguishable from a clean drain. An
/// unbounded wait is the worse failure; the bounded forced exit is the
/// accepted one and it is made visible (threat T-24-01-06).
#[test]
fn a_drain_over_budget_exits_forced_and_names_what_it_abandoned() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut ledger = DeliveryLedger::open(dir.path()).expect("open");
    let ctl = DrainController::new();

    let _stuck = ctl
        .begin_turn()
        .expect("an admitting gateway admits a turn");
    ledger.accept("never-settles").unwrap();
    ledger.begin_attempt("never-settles").unwrap();

    ctl.close_admission();
    let mut elapsed = 0u64;
    let report = ctl
        .drain(&mut ledger, 500, |_| {
            elapsed += 200;
            elapsed
        })
        .expect("drain returns a report even when it is forced");

    assert_eq!(
        report.outcome,
        DrainOutcome::Forced,
        "exceeding the budget must be distinguishable from a clean drain"
    );
    assert!(
        report.abandoned.contains(&"never-settles".to_string()),
        "a forced drain must name the deliveries it abandoned, got {:?}",
        report.abandoned
    );
    assert_eq!(report.abandoned_turns, 1);

    // The abandonment is DURABLE: a restart must see it recorded rather
    // than inferring a lost delivery from an absent record.
    drop(ledger);
    let reopened = DeliveryLedger::open(dir.path()).expect("reopen");
    assert_eq!(
        reopened.state("never-settles"),
        Some(DeliveryState::Abandoned),
        "the abandonment must survive the process that recorded it"
    );
}

/// Drain flushes the ledger to a durable point BEFORE it reports clean. A
/// clean drain whose journal was still buffered is the lost-delivery case
/// this criterion exists to exclude.
#[test]
fn a_clean_drain_leaves_the_journal_durable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut ledger = DeliveryLedger::open(dir.path()).expect("open");
    let ctl = DrainController::new();
    ledger.accept("flushed").unwrap();
    ledger.begin_attempt("flushed").unwrap();
    ledger.settle("flushed", true).unwrap();

    ctl.close_admission();
    let report = ctl.drain(&mut ledger, 1_000, |_| 1).expect("drain");
    assert_eq!(report.outcome, DrainOutcome::Clean);
    assert!(
        report.flushed,
        "a clean drain must have flushed the ledger to a durable point"
    );

    // Prove it by reading the journal from a second reader, without the
    // writer's cooperation.
    let raw = std::fs::read_to_string(DeliveryLedger::journal_path(dir.path())).expect("journal");
    assert!(
        raw.contains("flushed"),
        "the settled record must be on disk, not only in memory"
    );
}
