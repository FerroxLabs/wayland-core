//! Durable reservations for one comparison programme, not a daily resettable cap.
use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use wcore_budget::{BudgetCap, BudgetReservation, BudgetTracker, BudgetTrackerSnapshot};

pub const LEG_NANO_USD: u64 = 250_000_000;
pub const PROGRAMME_NANO_USD: u64 = 20_000_000_000;
pub const PRIOR_NANO_USD: u64 = 837_500;
pub const OUTPUT_CAP: u64 = 1024;
const PROGRAMME: &str = "w16-programme";

/// Receipts contain counters and digests, never credentials or request/response bodies.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallReceipt {
    pub leg: String,
    pub request_sha256: String,
    pub input_bound: u64,
    pub reserved_nano_usd: u64,
    pub charged_nano_usd: u64,
    pub outcome: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_basis: String,
    leg_reservation: BudgetReservation,
    programme_reservation: BudgetReservation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetReport {
    pub fixture_only: bool,
    pub count_requests: u64,
    pub prior_nano_usd: u64,
    pub halted: bool,
    pub calls: Vec<CallReceipt>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    schema: u32,
    legs: BudgetTrackerSnapshot,
    programme: BudgetTrackerSnapshot,
    report: BudgetReport,
}

/// One custody path across all legs. File locking also excludes concurrent processes.
pub struct Ledger {
    path: PathBuf,
    serial: Mutex<()>,
    failed: AtomicBool,
}

fn tracker(cap: f64) -> BudgetTracker {
    BudgetTracker::new(BudgetCap::builder().per_session_usd(cap).build())
}
fn dollars(nano: u64) -> f64 {
    nano as f64 / 1_000_000_000.0
}
/// Standard Astra pricing, reserving every input token at the cache-write rate.
/// Actual token usage can be reconciled without pretending unknown cache classes are free.
pub(super) fn price(input: u64, output: u64) -> Result<u64> {
    let (input_rate, output_rate) = if input > 272_000 {
        (25_000u64, 75_000u64)
    } else {
        (12_500u64, 50_000u64)
    };
    input
        .checked_mul(input_rate)
        .and_then(|n| {
            output
                .checked_mul(output_rate)
                .and_then(|o| n.checked_add(o))
        })
        .ok_or_else(|| anyhow!("usage arithmetic overflow"))
}

impl Ledger {
    pub fn open(path: PathBuf) -> Result<Self> {
        let ledger = Self {
            path,
            serial: Mutex::new(()),
            failed: AtomicBool::new(false),
        };
        ledger.transact(|_| Ok(()))?;
        Ok(ledger)
    }

    fn transact<T>(&self, change: impl FnOnce(&mut Stored) -> Result<T>) -> Result<T> {
        ensure!(
            !self.failed.load(Ordering::Acquire),
            "budget persistence failed; admission is disabled"
        );
        let _serial = self
            .serial
            .lock()
            .map_err(|_| anyhow!("budget mutex poisoned"))?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| anyhow!("ledger needs a parent"))?;
        std::fs::create_dir_all(parent)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path.with_extension("lock"))?;
        let mut lock = fd_lock::RwLock::new(file);
        let _guard = lock.try_write()?;
        let mut stored = match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice::<Stored>(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut programme = tracker(20.0);
                let prior = programme.reserve(PROGRAMME, 0, dollars(PRIOR_NANO_USD))?;
                programme.settle(prior, 0, dollars(PRIOR_NANO_USD))?;
                Stored {
                    schema: 1,
                    legs: tracker(0.25).snapshot()?,
                    programme: programme.snapshot()?,
                    report: BudgetReport {
                        fixture_only: true,
                        count_requests: 0,
                        prior_nano_usd: PRIOR_NANO_USD,
                        halted: false,
                        calls: vec![],
                    },
                }
            }
            Err(error) => return Err(error.into()),
        };
        ensure!(
            stored.schema == 1
                && stored.report.fixture_only
                && stored.report.prior_nano_usd == PRIOR_NANO_USD,
            "comparison budget authority mismatch"
        );
        let result = change(&mut stored);
        // Persist even a failed settlement: overrun and halt are not rolled back.
        if let Err(error) =
            wcore_config::atomic_io::atomic_write(&self.path, &serde_json::to_vec(&stored)?)
        {
            self.failed.store(true, Ordering::Release);
            return Err(error.into());
        }
        // The existing writer fsyncs contents; this adds rename durability on Unix.
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        result
    }

    pub(super) fn record_count(&self) -> Result<()> {
        self.transact(|stored| {
            ensure!(!stored.report.halted, "programme halted");
            stored.report.count_requests = stored
                .report
                .count_requests
                .checked_add(1)
                .ok_or_else(|| anyhow!("count attempt counter overflow"))?;
            Ok(())
        })
    }

    pub(super) fn disable(&self) {
        self.failed.store(true, Ordering::Release);
    }

    pub fn report(&self) -> Result<BudgetReport> {
        self.transact(|stored| Ok(stored.report.clone()))
    }

    pub(super) fn reserve(&self, leg: &str, input: u64, digest: String) -> Result<usize> {
        ensure!(!leg.is_empty(), "leg identity required");
        let reserved = price(input, OUTPUT_CAP)?;
        self.transact(|stored| {
            ensure!(
                !stored.report.halted,
                "programme halted after usage overrun"
            );
            let exposure = |call: &CallReceipt| {
                if call.outcome == "possibly_sent" {
                    call.reserved_nano_usd
                } else {
                    call.charged_nano_usd
                }
            };
            let total = stored
                .report
                .calls
                .iter()
                .try_fold(PRIOR_NANO_USD, |n, c| {
                    n.checked_add(exposure(c))
                        .ok_or_else(|| anyhow!("budget total overflow"))
                })?;
            let per_leg = stored
                .report
                .calls
                .iter()
                .filter(|c| c.leg == leg)
                .map(exposure)
                .sum::<u64>();
            ensure!(
                reserved <= LEG_NANO_USD.saturating_sub(per_leg),
                "leg budget exhausted"
            );
            ensure!(
                reserved <= PROGRAMME_NANO_USD.saturating_sub(total),
                "programme budget exhausted"
            );
            let mut legs = BudgetTracker::from_snapshot(stored.legs.clone())?;
            let mut programme = BudgetTracker::from_snapshot(stored.programme.clone())?;
            let lr = legs.reserve_turn(leg, input, OUTPUT_CAP, dollars(reserved))?;
            let pr = programme.reserve_turn(PROGRAMME, input, OUTPUT_CAP, dollars(reserved))?;
            let id = stored.report.calls.len();
            stored.report.calls.push(CallReceipt {
                leg: leg.into(),
                request_sha256: digest,
                input_bound: input,
                reserved_nano_usd: reserved,
                charged_nano_usd: 0,
                outcome: "possibly_sent".into(),
                input_tokens: None,
                output_tokens: None,
                cost_basis: "reserved_worst_case".into(),
                leg_reservation: lr,
                programme_reservation: pr,
            });
            stored.legs = legs.snapshot()?;
            stored.programme = programme.snapshot()?;
            Ok(id)
        })
    }

    pub(super) fn settle(&self, id: usize, usage: Option<(u64, u64)>, no_send: bool) -> Result<()> {
        self.transact(|stored| {
            let call = stored
                .report
                .calls
                .get_mut(id)
                .ok_or_else(|| anyhow!("unknown claim"))?;
            if call.outcome != "possibly_sent" {
                return Ok(());
            }
            let mut legs = BudgetTracker::from_snapshot(stored.legs.clone())?;
            let mut programme = BudgetTracker::from_snapshot(stored.programme.clone())?;
            if no_send {
                legs.release(call.leg_reservation);
                programme.release(call.programme_reservation);
                call.outcome = "proven_no_send".into();
                call.cost_basis = "no_generation_post".into();
            } else if let Some((input, output)) = usage {
                stored.report.halted |= input > call.input_bound || output > OUTPUT_CAP;
                let charge = price(input, output)?;
                let overrun = input > call.input_bound
                    || output > OUTPUT_CAP
                    || charge > call.reserved_nano_usd;
                let a = legs.settle_turn(call.leg_reservation, input, output, dollars(charge));
                let b = programme.settle_turn(
                    call.programme_reservation,
                    input,
                    output,
                    dollars(charge),
                );
                stored.report.halted |= overrun || a.is_err() || b.is_err();
                call.charged_nano_usd = charge;
                call.input_tokens = Some(input);
                call.output_tokens = Some(output);
                call.outcome = if overrun {
                    "usage_overrun"
                } else {
                    "usage_reconciled"
                }
                .into();
                call.cost_basis = "actual_token_counts_at_conservative_cache_write_rate".into();
            } else {
                legs.settle_reservation_conservatively(call.leg_reservation)?;
                programme.settle_reservation_conservatively(call.programme_reservation)?;
                call.charged_nano_usd = call.reserved_nano_usd;
                call.outcome = "unknown_consumed".into();
                call.cost_basis = "missing_authoritative_usage".into();
            }
            stored.legs = legs.snapshot()?;
            stored.programme = programme.snapshot()?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programme_history_and_uncertain_claims_survive_reopen_without_daily_reset() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("budget.json");
        let ledger = Ledger::open(path.clone()).unwrap();
        // Exactly25c per distinct leg, including the fixed output allowance.
        assert_eq!(price(15_904, OUTPUT_CAP).unwrap(), LEG_NANO_USD);
        for index in 0..79 {
            let claim = ledger
                .reserve(&format!("leg-{index}"), 15_904, "digest".into())
                .unwrap();
            ledger
                .settle(claim, Some((15_904, OUTPUT_CAP)), false)
                .unwrap();
        }
        drop(ledger);
        let ledger = Ledger::open(path).unwrap();
        assert!(
            ledger.reserve("leg-80", 15_904, "digest".into()).is_err(),
            "prior canary must prevent an80thfull25cleg"
        );
        assert_eq!(ledger.report().unwrap().calls.len(), 79);
        assert_eq!(ledger.report().unwrap().prior_nano_usd, PRIOR_NANO_USD);
    }

    #[test]
    fn unfinished_reservation_is_not_refunded_and_proved_no_send_is_released() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("budget.json");
        let ledger = Ledger::open(path.clone()).unwrap();
        let claim = ledger.reserve("leg", 15_904, "digest".into()).unwrap();
        drop(ledger);
        let ledger = Ledger::open(path).unwrap();
        assert!(ledger.reserve("leg", 1, "digest".into()).is_err());
        ledger.settle(claim, None, true).unwrap();
        let second = ledger.reserve("leg", 15_904, "digest".into()).unwrap();
        ledger.settle(second, None, false).unwrap();
        ledger.settle(second, Some((0, 0)), false).unwrap();
        let report = ledger.report().unwrap();
        assert_eq!(report.calls[1].charged_nano_usd, LEG_NANO_USD);
        assert_eq!(report.calls[1].outcome, "unknown_consumed");
        assert!(ledger.reserve("leg", 0, "digest".into()).is_err());
    }
}
