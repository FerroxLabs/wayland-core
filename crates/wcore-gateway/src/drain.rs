//! Drain: a state, not a sleep.
//!
//! Phase 24 plan 24-01, Task 2.
//!
//! The order is FIXED and every step is observable:
//!
//! 1. stop admitting new work;
//! 2. publish the in-flight counts;
//! 3. wait within a stated budget, republishing the counts as they fall;
//! 4. flush the ledger to a durable point;
//! 5. exit with a status that names clean or forced.
//!
//! A drain that slept for a fixed interval and then declared success would
//! pass every test that does not count deliveries, and fail the only one
//! that does.
//!
//! The wait is driven by an INJECTED clock closure rather than by sleeping.
//! A scheduling test that sleeps to reach a boundary is flaky by
//! construction; the shipped runtime passes a closure that actually waits,
//! and the suite passes one that does not.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use crate::ledger::{AbandonReason, DeliveryLedger, LedgerError};

/// How a drain ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrainOutcome {
    /// Every in-flight turn finished and every pending delivery settled
    /// inside the budget.
    Clean,
    /// The budget was exceeded. Work was abandoned, and it is named.
    Forced,
}

/// One published observation during a drain. An operator watching
/// `gateway drain` sees this sequence; a host receives the same numbers
/// through the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrainProgress {
    pub turns_in_flight: usize,
    pub deliveries_pending: usize,
    pub elapsed_ms: u64,
}

/// The result of a drain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DrainReport {
    pub outcome: DrainOutcome,
    /// Every published observation, in order. The first is taken before
    /// any waiting, so an operator can see the counts actually fall.
    pub trace: Vec<DrainProgress>,
    /// Deliveries abandoned by a forced exit, BY IDENTITY. Empty on a
    /// clean drain.
    pub abandoned: Vec<String>,
    /// Turns still in flight when a forced exit occurred.
    pub abandoned_turns: usize,
    /// Whether the ledger reached a durable point before the report was
    /// produced. A clean drain that did not flush is a lost delivery.
    pub flushed: bool,
}

/// A live turn. While one exists the gateway counts it as in flight;
/// dropping it releases the count, including on unwind.
#[derive(Debug)]
pub struct TurnGuard {
    turns: Arc<AtomicUsize>,
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        self.turns.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Admission control plus the drain procedure.
#[derive(Debug, Default)]
pub struct DrainController {
    admitting: AtomicBool,
    turns: Arc<AtomicUsize>,
}

impl DrainController {
    pub fn new() -> Self {
        Self {
            admitting: AtomicBool::new(true),
            turns: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Whether new work is still being admitted.
    pub fn is_admitting(&self) -> bool {
        self.admitting.load(Ordering::SeqCst)
    }

    /// Close admission. Idempotent, and the FIRST thing a drain does — a
    /// drain that waits before closing admission can never converge,
    /// because new work keeps arriving behind it.
    pub fn close_admission(&self) {
        self.admitting.store(false, Ordering::SeqCst);
    }

    /// Admit a turn, or refuse because admission is closed.
    pub fn begin_turn(&self) -> Option<TurnGuard> {
        if !self.is_admitting() {
            return None;
        }
        self.turns.fetch_add(1, Ordering::SeqCst);
        Some(TurnGuard {
            turns: Arc::clone(&self.turns),
        })
    }

    /// Turns currently in flight.
    pub fn turns_in_flight(&self) -> usize {
        self.turns.load(Ordering::SeqCst)
    }

    fn observe(&self, ledger: &DeliveryLedger, elapsed_ms: u64) -> DrainProgress {
        DrainProgress {
            turns_in_flight: self.turns_in_flight(),
            deliveries_pending: ledger.pending_count(),
            elapsed_ms,
        }
    }

    /// Run the drain to its terminal point.
    ///
    /// `wait` is the injected clock: it is called between observations,
    /// may perform whatever waiting the caller wants, and returns total
    /// elapsed milliseconds. It receives the ledger so the caller's real
    /// dispatcher can settle work while the drain observes it.
    pub fn drain<F>(
        &self,
        ledger: &mut DeliveryLedger,
        budget_ms: u64,
        mut wait: F,
    ) -> Result<DrainReport, LedgerError>
    where
        F: FnMut(&mut DeliveryLedger) -> u64,
    {
        // Step 1, before anything else.
        self.close_admission();

        // Step 2: publish before waiting, so the trace shows the starting
        // point rather than only the end state.
        let mut trace = vec![self.observe(ledger, 0)];
        let mut elapsed = 0u64;
        let mut outcome = DrainOutcome::Clean;

        loop {
            let last = *trace.last().expect("trace is never empty");
            if last.turns_in_flight == 0 && last.deliveries_pending == 0 {
                break;
            }
            if elapsed >= budget_ms {
                outcome = DrainOutcome::Forced;
                break;
            }
            // Step 3.
            elapsed = wait(ledger);
            trace.push(self.observe(ledger, elapsed));
        }

        let mut abandoned = Vec::new();
        let mut abandoned_turns = 0;
        if outcome == DrainOutcome::Forced {
            abandoned = ledger.pending();
            for id in &abandoned {
                // Recorded durably WITH ITS REASON, so a restart sees an
                // abandonment rather than inferring a loss from an absent
                // record — and so `gateway abandoned` can tell an operator
                // that this one ran out of shutdown budget (safe to re-run)
                // rather than having an unknown fate (must be checked at the
                // destination first).
                ledger.abandon(id, AbandonReason::DrainBudgetExpired)?;
            }
            abandoned_turns = self.turns_in_flight();
        }

        // Step 4: durable BEFORE the report claims anything.
        ledger.flush()?;

        // Step 5.
        Ok(DrainReport {
            outcome,
            trace,
            abandoned,
            abandoned_turns,
            flushed: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_guard_releases_its_count_on_drop() {
        let ctl = DrainController::new();
        {
            let _g = ctl.begin_turn().unwrap();
            assert_eq!(ctl.turns_in_flight(), 1);
        }
        assert_eq!(ctl.turns_in_flight(), 0);
    }

    #[test]
    fn close_admission_is_idempotent() {
        let ctl = DrainController::new();
        ctl.close_admission();
        ctl.close_admission();
        assert!(!ctl.is_admitting());
        assert!(ctl.begin_turn().is_none());
    }
}
