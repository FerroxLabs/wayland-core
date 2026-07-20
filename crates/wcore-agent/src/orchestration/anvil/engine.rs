//! Anvil climb engine — the loop that turns the A1.5 decision core + substrate
//! into a real forge (spec §6). `run_climb` seeds a candidate, runs the pinned
//! gate, climbs by surgical fail-set-accepted steps, and produces an honest
//! terminal state + receipt payload.
//!
//! The loop is written over two injected seams so it is unit-testable without a
//! live spawner or sandbox (the same discipline as the rest of anvil):
//! - [`Builder`] produces a candidate in its own transaction-owned isolated
//!   checkout (real impl: a forked sub-agent with edit tools; test impl: a fake).
//! - [`EvaluationGateExecutor`] runs the pinned gate against a candidate's
//!   identity and returns the per-check [`GateReport`] (real impl: the sandbox +
//!   a gate-output parser; test impl: a fake).
//!
//! The real seams and the `/forge` wiring live in [`super::forge`]; this file is
//! the engine. It consumes every substrate piece: the gate closure + probe
//! ([`super::gates`]), the cost ledger ([`super::ledger`]), the acceptance +
//! order decision core ([`super::climb`]), the crash-recovery journal
//! ([`super::journal`]).
//!
//! Spec: `docs/design/2026-07-12-anvil-native-gated-forge-design.md` (v2) §6.

use std::path::PathBuf;

use async_trait::async_trait;

use super::TerminalState;
use super::climb::{Acceptance, CandidateId, CheckId, GateReport, evaluate_acceptance};
use super::gates::StabilityPolicy;
use super::journal::{ClimbJournal, JournalEntry, JournalKind};
use super::ledger::{ClimbLedger, LedgerEntry};

/// Opaque, live per-candidate checkout identity.
///
/// This is the climb's ONLY handle onto a candidate's changes. It is
/// deliberately NOT a bare path: production candidates back it with a retained,
/// transaction-owned standalone checkout (the predecessor `MutationAttemptGuard`
/// / `TransactionWorkspace` opened by the production spawner, carrying its own
/// opaque transaction/checkout/base/head/tree identity). Every access
/// re-derives the live checkout root through that retained authority — so a
/// released, drifted, or substituted checkout fails closed BEFORE any gate runs
/// — and dropping the identity terminalizes the owned transaction (RAII loser
/// cleanup). The engine never stores, compares, or reconstructs a candidate from
/// a raw filesystem path.
pub trait CandidateCheckout: Send + Sync + std::fmt::Debug {
    /// Resolve the candidate's live checkout root, re-proving execution
    /// authority for the exact bound checkout. Fails closed (an
    /// [`EngineError`]) on a released, drifted, or substituted transaction.
    /// This is the only path from an opaque candidate identity to a concrete
    /// working directory, and it is re-run on every gate invocation and
    /// stability rerun so the subject can never silently change.
    fn resolve_root(&self) -> Result<PathBuf, EngineError>;
}

/// A candidate build the climb produced, bound to its own retained,
/// transaction-owned isolated checkout.
///
/// Not `Clone`: the owned [`CandidateCheckout`] is a single-owner lifecycle
/// handle, so a candidate's transaction identity can never be duplicated or
/// collapsed with another candidate's. A rejected, superseded, or dropped
/// candidate terminalizes its own transaction when this value is dropped.
#[derive(Debug)]
pub struct BuiltCandidate {
    /// Which attempt produced it.
    pub id: CandidateId,
    /// The opaque, live identity of this candidate's transaction-owned checkout.
    pub checkout: Box<dyn CandidateCheckout>,
    /// What producing it cost (settled into the ledger).
    pub spend: LedgerEntry,
}

/// Feedback handed to the [`Builder`] for a surgical attempt: the checks still
/// failing on the current best, plus the bounded, injection-fenced diagnostic
/// tail (never raw gate output).
#[derive(Debug, Clone)]
pub struct BuildFeedback {
    /// The checks the builder should fix.
    pub failing: Vec<CheckId>,
    /// Bounded, sanitized diagnostics (from [`GateReport::diagnostics`]).
    pub diagnostics: String,
    /// One-shot frontier unblocking guidance from the escalation valve, when a
    /// stall was diagnosed (spec §6.4). `None` on the un-stalled path.
    pub valve_guidance: Option<String>,
}

/// Produces candidate builds. The real implementation forks a sub-agent with
/// edit tools into an isolated worktree; tests use a fake.
#[async_trait]
pub trait Builder: Send + Sync {
    /// Build a candidate for `task`. `feedback` is `None` for the initial probe
    /// and `Some` for a surgical attempt targeting the still-failing checks.
    async fn build(
        &self,
        task: &str,
        feedback: Option<&BuildFeedback>,
    ) -> Result<BuiltCandidate, EngineError>;
}

/// Runs the pinned gate against ONE live candidate identity and returns its
/// per-check report.
///
/// Renamed from the earlier `GateExecutor` to make the trust boundary explicit:
/// this is Anvil's ADVISORY evaluation-and-selection surface. Its reports,
/// stamps, journals, receipts, and `SandboxGate` results can reject or score a
/// candidate, but they are NOT the Phase 20 acceptance input — they cannot
/// construct the module-private observed result, mint `accepted_candidate`, or
/// authorize parent integration. It accepts the exact candidate identity, never
/// a bare path: the subject root is always re-derived through
/// [`CandidateCheckout::resolve_root`], so path substitution, a stale head/tree,
/// a sibling checkout, or an identity/path disagreement fails closed.
#[async_trait]
pub trait EvaluationGateExecutor: Send + Sync {
    /// Run the gate against `candidate`'s live checkout and return its per-check
    /// report. The implementation resolves the subject root through the
    /// candidate identity itself; it is never handed a bare path.
    async fn run(&self, candidate: &dyn CandidateCheckout) -> Result<GateReport, EngineError>;
}

/// Evidence handed to the escalation valve on a detected stall: the same
/// fail-set fingerprint has repeated across consecutive candidates.
#[derive(Debug, Clone)]
pub struct StallReport {
    /// The repeated fail-set fingerprint ([`super::climb::FailSet::fail_hash`]).
    pub fail_hash: u64,
    /// Consecutive candidates that failed with this exact fingerprint.
    pub repeats: u32,
    /// The checks stuck failing.
    pub failing: Vec<CheckId>,
    /// Bounded, sanitized diagnostics from the latest failing report.
    pub diagnostics: String,
}

/// The escalation valve (spec §6.4): ONE frontier diagnostic turn on a detected
/// stall. It reads the stall evidence and writes unblocking guidance back INTO
/// the loop — it never does the work (the moment it does, the loop is a dumb
/// loop at frontier prices). Real impl: a read-only frontier fork; tests fake.
#[async_trait]
pub trait Valve: Send + Sync {
    /// Diagnose `stall` and return unblocking guidance for the next builder
    /// attempt (a corrected assumption, a decomposed step, the file the driver
    /// never opened).
    async fn diagnose(&self, task: &str, stall: &StallReport) -> Result<String, EngineError>;
}

/// A climb aborted before it could produce a terminal state through the normal
/// path — surfaced honestly, never swallowed.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A builder could not produce a candidate (spawn refused, crashed).
    #[error("builder failed: {0}")]
    Builder(String),
    /// The gate could not execute against a candidate (sandbox refused).
    #[error("gate execution failed: {0}")]
    Gate(String),
}

/// Static parameters of a climb.
#[derive(Debug, Clone)]
pub struct ClimbParams {
    /// The task being forged.
    pub task: String,
    /// N-of-M stability required before the reserved `verified` stamp.
    pub stability: StabilityPolicy,
    /// Hard cap on climb iterations (probe counts as 1).
    pub max_iterations: u32,
    /// The pinned gate closure digest (hex), for the journal + receipt.
    pub gate_closure_digest: String,
    /// Consecutive identical fail-hashes that count as a stall (spec §6.4 —
    /// the "same reason" clause is load-bearing: different failures mean the
    /// climb is progressing through a hard patch; identical failures mean it
    /// is walking into the same wall). `0` disables stall detection.
    pub stall_after: u32,
    /// Wall-clock deadline for the WHOLE climb. Checked between steps (a
    /// single in-flight builder/gate await is bounded by its own timeout):
    /// past the deadline the climb stops and reports an honest `timed_out`
    /// receipt instead of being killed receipt-less by an outer dispatch
    /// timeout. `None` = ungoverned.
    pub deadline: Option<std::time::Instant>,
}

/// The result of a climb — everything the receipt needs (spec §8).
///
/// Not `Clone`: it may own the selected winner's live checkout identity
/// ([`Self::winner`]), a single-owner lifecycle handle.
#[derive(Debug)]
pub struct ClimbOutcome {
    /// How the climb ended (spec §6.5).
    pub terminal: TerminalState,
    /// The honesty-vocabulary stamp earned (spec §2) — `verified` ONLY for a
    /// real Tier-1 gate passing with stability.
    pub stamp: String,
    /// Passing / total checks on the final candidate.
    pub checks_passed: u32,
    /// Total checks on the final candidate.
    pub checks_total: u32,
    /// Iterations performed.
    pub iterations: u32,
    /// Escalation-valve fires during the climb (spec §6.4; 0 on the happy
    /// path — the reserve is the point).
    pub valve_fires: u32,
    /// The selected winner's retained, transaction-owned checkout identity, if
    /// any candidate reached a keepable state. This is the ONLY candidate whose
    /// transaction survives the climb — every loser, rejected, and superseded
    /// candidate has already terminalized (RAII on drop). Holding this keeps the
    /// winner's checkout live for the parent-owned gate/landing lifecycle to
    /// consume; dropping it terminalizes the winner too. Identities are never
    /// collapsed or reused across candidates.
    pub winner: Option<Box<dyn CandidateCheckout>>,
    /// Display echo of the winner's checkout root (metadata only). Derived from
    /// [`Self::winner`] for logs/receipts; it is NOT the source of identity and
    /// is never used to reconstruct a candidate.
    pub best_worktree: Option<PathBuf>,
}

/// The reserved `verified` stamp string.
const STAMP_VERIFIED: &str = "verified";
/// Stamp for a gate that went green but could not prove stability (flaky) — not
/// verification (spec §2/§5).
const STAMP_SELF_CHECKED: &str = "self_checked";
/// Stamp when nothing keepable was produced.
const STAMP_NONE: &str = "none";

/// Drive a gated-forge climb over the injected seams (spec §6.1–6.3, minimal A1
/// shape): probe → gate → surgical fail-set-accepted climb → terminal. Every
/// paid step is journalled before it is trusted, and the ledger caps spend.
///
/// This is the engine the real `/forge` path constructs the seams for; it does
/// NOT emit the receipt (the caller does, at the single top-level exit, spec §8)
/// nor acquire the lease / pin the gate (its caller owns those).
pub async fn run_climb(
    params: &ClimbParams,
    builder: &dyn Builder,
    gate: &dyn EvaluationGateExecutor,
    valve: Option<&dyn Valve>,
    ledger: &ClimbLedger,
    journal: &mut ClimbJournal,
) -> ClimbOutcome {
    let mut iterations: u32 = 0;
    // Valve bookkeeping (spec §6.4): consecutive identical fail-hashes = a
    // stall; the valve buys exactly ONE frontier diagnostic turn per climb.
    let mut last_fail_hash: Option<u64> = None;
    let mut same_reason: u32 = 0;
    let mut valve_fires: u32 = 0;
    let mut guidance: Option<String> = None;

    // ── Probe: the initial candidate. ────────────────────────────────────────
    let probe = match builder.build(&params.task, None).await {
        Ok(c) => c,
        Err(e) => return blocked(format!("probe builder failed: {e}")),
    };
    let mut report = match gate_and_record(gate, &probe, ledger, journal, params).await {
        Ok(r) => r,
        Err(e) => return blocked(format!("probe gate failed: {e}")),
    };
    iterations += 1;
    track_stall(&report, &mut last_fail_hash, &mut same_reason);
    // The most recent gate report regardless of acceptance — REJECTED
    // candidates drive the stall counter, so valve evidence must reflect
    // them, not the last accepted best.
    let mut latest = report.clone();
    let mut best = (probe, report.clone());

    // Keep-best: if the probe is green (and stable), it is the winner. Ownership
    // of `best` moves into the outcome so ONLY the winner's transaction survives;
    // every other candidate has already terminalized by RAII.
    if let Some(kind) = check_keepable(gate, &best, params).await {
        return finish_keepable(best, kind, iterations, valve_fires);
    }

    // ── Surgical climb: fix the failing checks, accept only non-regressions. ──
    while iterations < params.max_iterations {
        if ledger.is_exhausted() {
            break;
        }
        // Wall-clock governor: stop BETWEEN steps and report honestly rather
        // than letting an outer dispatch timeout kill the climb receipt-less.
        if past_deadline(params) {
            return timed_out_from_best(&best.1, iterations, valve_fires);
        }

        // Stall? Buy ONE frontier diagnostic turn, feed the guidance back into
        // the loop, and resume cheap. The valve never inherits the task; a
        // valve error must not kill the climb (the loop just stays cheap-dumb).
        if let Some(v) = valve
            && params.stall_after > 0
            && same_reason >= params.stall_after
            && valve_fires < VALVE_BUDGET
        {
            let stall = StallReport {
                fail_hash: last_fail_hash.unwrap_or_default(),
                repeats: same_reason,
                failing: latest.fail_set().ids().cloned().collect(),
                diagnostics: latest.diagnostics.tail().to_string(),
            };
            valve_fires += 1;
            journal_valve(journal, &stall, ledger);
            if let Ok(g) = v.diagnose(&params.task, &stall).await {
                guidance = Some(g);
            }
            same_reason = 0;
        }

        let mut feedback = feedback_from(&report);
        feedback.valve_guidance = guidance.clone();
        let candidate = match builder.build(&params.task, Some(&feedback)).await {
            Ok(c) => c,
            // A failed surgical attempt is not fatal — keep the best so far.
            Err(_) => break,
        };
        let candidate_report =
            match gate_and_record(gate, &candidate, ledger, journal, params).await {
                Ok(r) => r,
                Err(_) => continue,
            };
        iterations += 1;
        track_stall(&candidate_report, &mut last_fail_hash, &mut same_reason);
        latest = candidate_report.clone();

        // Accept iff the new fail-set is a non-regression on the current best
        // (spec §6.3 — safety-class never traded).
        match evaluate_acceptance(&best.1.fail_set(), &candidate_report.fail_set()) {
            Acceptance::Accept { .. } => {
                journal_step(
                    journal,
                    JournalKind::Promote,
                    &candidate,
                    &candidate_report,
                    ledger,
                );
                // Persist the replacement winner BEFORE cleaning the displaced
                // best: constructing the new tuple moves the accepted candidate's
                // checkout in, and only then is the previous `best` dropped —
                // terminalizing the displaced candidate's transaction. Identities
                // never collapse: the winner keeps its own distinct checkout.
                best = (candidate, candidate_report.clone());
                report = candidate_report;
                if let Some(kind) = check_keepable(gate, &best, params).await {
                    return finish_keepable(best, kind, iterations, valve_fires);
                }
            }
            // A rejected candidate is never stored in `best`, so it is dropped at
            // the end of this iteration — terminalizing its transaction and
            // cleaning its checkout without ever touching the parent.
            Acceptance::Reject(_) => { /* logged via journal Candidate; keep best */ }
        }
    }

    // ── No stable-green candidate: report honestly. ──────────────────────────
    terminal_from_best(&best.1, iterations, valve_fires)
}

/// The valve buys at most this many frontier turns per climb (spec §6.4). If
/// one diagnostic turn didn't unblock the wall, the plan is wrong — that goes
/// back to the caller as `needs_escalation`, not to more valve spend.
const VALVE_BUDGET: u32 = 1;

/// Update the consecutive same-fail-hash counter from a report. Green reports
/// and CHANGED fail-hashes reset the streak (progress through a hard patch is
/// not a stall — only the same wall, repeatedly, is).
fn track_stall(report: &GateReport, last: &mut Option<u64>, same_reason: &mut u32) {
    if report.all_green() {
        *last = None;
        *same_reason = 0;
        return;
    }
    let hash = report.fail_set().fail_hash();
    if *last == Some(hash) {
        *same_reason += 1;
    } else {
        *last = Some(hash);
        *same_reason = 1;
    }
}

/// Journal a valve fire (best-effort, same contract as [`journal_step`]).
fn journal_valve(journal: &mut ClimbJournal, stall: &StallReport, ledger: &ClimbLedger) {
    let fail_ids = stall
        .failing
        .iter()
        .map(|c| c.as_str().to_string())
        .collect();
    let entry = JournalEntry::new(
        JournalKind::Valve,
        format!("valve-{:016x}", stall.fail_hash),
        ledger.settled_microcents(),
    )
    .with_result(0, fail_ids);
    let _ = journal.append(entry);
}

/// Run the gate on `candidate`, settle its build cost + a gate-exec entry into
/// the ledger, and journal the candidate step. Returns the report.
async fn gate_and_record(
    gate: &dyn EvaluationGateExecutor,
    candidate: &BuiltCandidate,
    ledger: &ClimbLedger,
    journal: &mut ClimbJournal,
    params: &ClimbParams,
) -> Result<GateReport, EngineError> {
    // Charge the builder's spend (reserve+settle keeps the cap honest even though
    // the actual is already known — the reservation is the race-free gate §7).
    if let Ok(res) = ledger.reserve(candidate.spend.cost_microcents, candidate.spend.wallclock) {
        ledger.settle(res, candidate.spend.clone());
    }
    // The gate is handed the candidate's opaque identity, not a bare path: it
    // re-derives (and re-proves) the live checkout root through the identity, so
    // a substituted/stale/sibling checkout fails closed here.
    let report = gate.run(candidate.checkout.as_ref()).await?;
    // Journal the gated candidate (with the pinned gate digest) before it is
    // acted on — crash recovery replays from here (spec §6.5).
    let fail_ids = report
        .fail_set()
        .ids()
        .map(|c| c.as_str().to_string())
        .collect();
    let entry = JournalEntry::new(
        JournalKind::Candidate,
        candidate.id.as_str(),
        ledger.settled_microcents(),
    )
    .with_gate_digest(params.gate_closure_digest.as_str())
    .with_candidate(candidate.id.as_str())
    .with_result(report.score(), fail_ids);
    let _ = journal.append(entry);
    Ok(report)
}

/// The keepable terminal a green candidate earned, if any.
enum KeepableKind {
    /// Green AND stable across the required reruns — the reserved `verified`.
    Verified,
    /// Green once but not stably (flaky) — honest `self_checked`, quarantined.
    SelfChecked,
}

/// If `best` is green, decide whether it is `verified` (green + stable) or
/// `self_checked` (green but flaky); `None` means not green, keep climbing.
/// Borrows `best` (its identity is re-resolved for every stability rerun), so
/// ownership of the winner is only moved into the outcome by [`finish_keepable`].
async fn check_keepable(
    gate: &dyn EvaluationGateExecutor,
    best: &(BuiltCandidate, GateReport),
    params: &ClimbParams,
) -> Option<KeepableKind> {
    if !best.1.all_green() {
        return None;
    }
    if stability_holds(gate, best.0.checkout.as_ref(), params.stability).await {
        Some(KeepableKind::Verified)
    } else {
        Some(KeepableKind::SelfChecked)
    }
}

/// Build the keepable outcome, MOVING the winning candidate's checkout identity
/// into [`ClimbOutcome::winner`]. This is the only path that retains a
/// candidate's transaction past the climb; every other candidate has already
/// terminalized. The winner is handed onward to the parent-owned gate/landing
/// lifecycle — the advisory stamp here does not itself land anything.
fn finish_keepable(
    best: (BuiltCandidate, GateReport),
    kind: KeepableKind,
    iterations: u32,
    valve_fires: u32,
) -> ClimbOutcome {
    let (candidate, report) = best;
    let (terminal, stamp) = match kind {
        KeepableKind::Verified => (TerminalState::Verified, STAMP_VERIFIED),
        // Green once but not stably — honest self-checked, quarantined (spec §2).
        KeepableKind::SelfChecked => (TerminalState::NeedsEscalation, STAMP_SELF_CHECKED),
    };
    // Display echo only; identity lives in `winner`. A resolve failure here does
    // not un-select the winner — the retained identity is still returned.
    let best_worktree = candidate.checkout.resolve_root().ok();
    ClimbOutcome {
        terminal,
        stamp: stamp.to_string(),
        checks_passed: report.score(),
        checks_total: u32::try_from(report.total()).unwrap_or(u32::MAX),
        iterations,
        valve_fires,
        winner: Some(candidate.checkout),
        best_worktree,
    }
}

/// Re-run the gate `stability.of - 1` more times on the SAME candidate identity;
/// the stamp requires `stability.required` of `stability.of` identical-code
/// passes (spec §5). The subject is the candidate's own checkout, re-resolved on
/// every rerun — never a bare path.
async fn stability_holds(
    gate: &dyn EvaluationGateExecutor,
    candidate: &dyn CandidateCheckout,
    stability: StabilityPolicy,
) -> bool {
    let mut passes = 1; // the run that already went green
    for _ in 1..stability.of {
        match gate.run(candidate).await {
            Ok(r) if r.all_green() => passes += 1,
            // A single non-green (or errored) rerun means the check flipped on
            // identical code — flaky, so verification is not earned.
            _ => return false,
        }
    }
    stability.met(passes)
}

/// Build surgical feedback from the current report (valve guidance is folded
/// in by the loop when a stall was diagnosed).
fn feedback_from(report: &GateReport) -> BuildFeedback {
    BuildFeedback {
        failing: report.fail_set().ids().cloned().collect(),
        diagnostics: report.diagnostics.tail().to_string(),
        valve_guidance: None,
    }
}

/// Journal a candidate/promote step (best-effort; a journal I/O error must not
/// crash the climb, but it IS surfaced by degrading crash-recovery — logged).
fn journal_step(
    journal: &mut ClimbJournal,
    kind: JournalKind,
    candidate: &BuiltCandidate,
    report: &GateReport,
    ledger: &ClimbLedger,
) {
    let fail_ids = report
        .fail_set()
        .ids()
        .map(|c| c.as_str().to_string())
        .collect();
    let entry = JournalEntry::new(kind, candidate.id.as_str(), ledger.settled_microcents())
        .with_candidate(candidate.id.as_str())
        .with_result(report.score(), fail_ids);
    let _ = journal.append(entry);
}

/// Whether the climb's wall-clock deadline has passed.
fn past_deadline(params: &ClimbParams) -> bool {
    params
        .deadline
        .is_some_and(|d| std::time::Instant::now() >= d)
}

/// Honest terminal when the wall-clock governor stops the climb (the best
/// candidate so far is reported, never promoted to a stamp it didn't earn).
fn timed_out_from_best(report: &GateReport, iterations: u32, valve_fires: u32) -> ClimbOutcome {
    let stamp = if report.all_green() {
        STAMP_SELF_CHECKED
    } else {
        STAMP_NONE
    };
    ClimbOutcome {
        terminal: TerminalState::TimedOut,
        stamp: stamp.to_string(),
        checks_passed: report.score(),
        checks_total: u32::try_from(report.total()).unwrap_or(u32::MAX),
        iterations,
        valve_fires,
        // Nothing keepable: the caller's `best` still owns the last candidate and
        // terminalizes it on scope exit — no winner is retained.
        winner: None,
        best_worktree: None,
    }
}

/// Terminal state when no stable-green candidate was reached.
fn terminal_from_best(report: &GateReport, iterations: u32, valve_fires: u32) -> ClimbOutcome {
    let (terminal, stamp) = if report.all_green() {
        (TerminalState::NeedsEscalation, STAMP_SELF_CHECKED)
    } else {
        (TerminalState::NeedsEscalation, STAMP_NONE)
    };
    ClimbOutcome {
        terminal,
        stamp: stamp.to_string(),
        checks_passed: report.score(),
        checks_total: u32::try_from(report.total()).unwrap_or(u32::MAX),
        iterations,
        valve_fires,
        // No keepable candidate: the last `best` terminalizes on scope exit.
        winner: None,
        best_worktree: None,
    }
}

/// A `Blocked` outcome for a stated reason (spec §6.5).
fn blocked(reason: String) -> ClimbOutcome {
    ClimbOutcome {
        terminal: TerminalState::Blocked(reason),
        stamp: STAMP_NONE.to_string(),
        checks_passed: 0,
        checks_total: 0,
        iterations: 0,
        valve_fires: 0,
        winner: None,
        best_worktree: None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::climb::{CheckOutcome, Severity};
    use super::super::gates::BoundedGateOutput;
    use super::super::ledger::LedgerCap;
    use super::*;
    use std::sync::Mutex;

    fn report(checks: Vec<CheckOutcome>) -> GateReport {
        let exit = if checks.iter().all(|c| c.passed) && !checks.is_empty() {
            0
        } else {
            1
        };
        GateReport {
            checks,
            exit_code: exit,
            diagnostics: BoundedGateOutput::from_bytes(b"diag"),
        }
    }

    fn ok(id: &str) -> CheckOutcome {
        CheckOutcome::new(id, true, Severity::Major)
    }
    fn bad(id: &str) -> CheckOutcome {
        CheckOutcome::new(id, false, Severity::Major)
    }

    /// A fake candidate identity for the pure-engine tests: a stable in-memory
    /// root that resolves without a live checkout. Production uses the retained
    /// `MutationAttemptGuard`; the engine only ever sees the opaque trait, so a
    /// fake proves identity plumbing (each candidate carries its OWN handle)
    /// without git/spawner. It records drops so tests can assert loser cleanup.
    #[derive(Debug)]
    struct FakeCheckout {
        root: PathBuf,
        dropped: Option<Arc<std::sync::atomic::AtomicUsize>>,
    }
    impl FakeCheckout {
        fn new(id: &str) -> Self {
            Self {
                root: PathBuf::from(format!("/wt/{id}")),
                dropped: None,
            }
        }
        fn tracked(id: &str, dropped: Arc<std::sync::atomic::AtomicUsize>) -> Self {
            Self {
                root: PathBuf::from(format!("/wt/{id}")),
                dropped: Some(dropped),
            }
        }
    }
    impl CandidateCheckout for FakeCheckout {
        fn resolve_root(&self) -> Result<PathBuf, EngineError> {
            Ok(self.root.clone())
        }
    }
    impl Drop for FakeCheckout {
        fn drop(&mut self) {
            if let Some(counter) = &self.dropped {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    use std::sync::Arc;

    fn candidate(id: &str) -> BuiltCandidate {
        BuiltCandidate {
            id: CandidateId::new(id.to_string()),
            checkout: Box::new(FakeCheckout::new(id)),
            spend: LedgerEntry::gate_exec(std::time::Duration::from_millis(1)),
        }
    }

    /// A builder that yields a fixed sequence of candidate ids, each with its own
    /// distinct opaque checkout identity.
    struct SeqBuilder {
        next: Mutex<u32>,
    }
    #[async_trait]
    impl Builder for SeqBuilder {
        async fn build(
            &self,
            _task: &str,
            _fb: Option<&BuildFeedback>,
        ) -> Result<BuiltCandidate, EngineError> {
            let mut n = self.next.lock().unwrap();
            let id = format!("c{n}");
            *n += 1;
            Ok(candidate(&id))
        }
    }

    /// A gate that returns a scripted report per candidate, keyed by call order.
    struct ScriptGate {
        reports: Mutex<std::collections::VecDeque<GateReport>>,
        // A stable report to repeat for stability reruns of a green candidate.
        stable_green: bool,
    }
    #[async_trait]
    impl EvaluationGateExecutor for ScriptGate {
        async fn run(&self, candidate: &dyn CandidateCheckout) -> Result<GateReport, EngineError> {
            // Prove the gate is handed a live identity, not a bare path: resolving
            // the subject root must succeed before scoring.
            candidate.resolve_root()?;
            let mut q = self.reports.lock().unwrap();
            if q.len() == 1 && self.stable_green {
                // repeat the last (green) report for stability reruns
                return Ok(q.front().unwrap().clone());
            }
            q.pop_front()
                .ok_or_else(|| EngineError::Gate("no more scripted reports".into()))
        }
    }

    fn params(stability_of: u32) -> ClimbParams {
        ClimbParams {
            task: "t".into(),
            stability: StabilityPolicy::new(stability_of, stability_of),
            max_iterations: 5,
            gate_closure_digest: "deadbeef".into(),
            stall_after: 2,
            deadline: None,
        }
    }

    #[tokio::test]
    async fn past_deadline_yields_honest_timed_out_receipt() {
        // Probe runs (red), then the governor trips before the first surgical
        // attempt: the outcome is `timed_out` with the probe's honest counts —
        // never a receipt-less kill, never an unearned stamp.
        let builder = SeqBuilder {
            next: Mutex::new(0),
        };
        let gate = ScriptGate {
            reports: Mutex::new(vec![report(vec![ok("a"), bad("b")])].into()),
            stable_green: false,
        };
        let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();
        let mut p = params(1);
        p.deadline = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
        let out = run_climb(&p, &builder, &gate, None, &ledger, &mut journal).await;
        assert_eq!(out.terminal, TerminalState::TimedOut);
        assert_eq!(out.stamp, "none");
        assert_eq!((out.checks_passed, out.checks_total), (1, 2));
        assert_eq!(out.iterations, 1);
    }

    async fn run(reports: Vec<GateReport>, stable_green: bool, stab_of: u32) -> ClimbOutcome {
        let builder = SeqBuilder {
            next: Mutex::new(0),
        };
        let gate = ScriptGate {
            reports: Mutex::new(reports.into()),
            stable_green,
        };
        let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();
        run_climb(
            &params(stab_of),
            &builder,
            &gate,
            None,
            &ledger,
            &mut journal,
        )
        .await
    }

    #[tokio::test]
    async fn probe_green_and_stable_is_verified() {
        // Probe green; stability 1-of-1 (no reruns needed).
        let out = run(vec![report(vec![ok("a"), ok("b")])], true, 1).await;
        assert_eq!(out.terminal, TerminalState::Verified);
        assert_eq!(out.stamp, "verified");
        assert_eq!((out.checks_passed, out.checks_total), (2, 2));
        assert_eq!(out.iterations, 1);
        assert!(out.best_worktree.is_some());
    }

    #[tokio::test]
    async fn green_but_flaky_is_not_verified() {
        // Probe green, but a stability rerun (3-of-3) flips to red → self_checked.
        let mut q = vec![report(vec![ok("a")])]; // probe green
        q.push(report(vec![bad("a")])); // rerun flips → flaky
        let out = run(q, false, 3).await;
        assert_ne!(out.terminal, TerminalState::Verified);
        assert_eq!(out.stamp, "self_checked");
    }

    #[tokio::test]
    async fn surgical_step_that_fixes_a_check_is_promoted_to_verified() {
        // Probe fails b; surgical attempt fixes it → green → verified (1-of-1).
        let out = run(
            vec![
                report(vec![ok("a"), bad("b")]), // probe
                report(vec![ok("a"), ok("b")]),  // surgical fix
            ],
            true,
            1,
        )
        .await;
        assert_eq!(out.terminal, TerminalState::Verified);
        assert_eq!(out.iterations, 2);
    }

    #[tokio::test]
    async fn regressing_candidate_is_rejected_best_retained() {
        // Probe fails b (Major); surgical introduces a NEW Major fail → rejected;
        // no green reached → needs_escalation, not verified.
        let out = run(
            vec![
                report(vec![ok("a"), bad("b")]),  // probe: {b}
                report(vec![bad("a"), bad("b")]), // surgical: {a,b} ⊃ {b} → reject
                report(vec![ok("a"), bad("b")]),  // next attempt: back to {b} (no progress)
                report(vec![ok("a"), bad("b")]),
                report(vec![ok("a"), bad("b")]),
            ],
            false,
            1,
        )
        .await;
        assert_ne!(out.terminal, TerminalState::Verified);
    }

    #[tokio::test]
    async fn probe_builder_failure_is_blocked() {
        struct DeadBuilder;
        #[async_trait]
        impl Builder for DeadBuilder {
            async fn build(
                &self,
                _t: &str,
                _f: Option<&BuildFeedback>,
            ) -> Result<BuiltCandidate, EngineError> {
                Err(EngineError::Builder("spawn refused".into()))
            }
        }
        struct NoGate;
        #[async_trait]
        impl EvaluationGateExecutor for NoGate {
            async fn run(&self, _c: &dyn CandidateCheckout) -> Result<GateReport, EngineError> {
                Err(EngineError::Gate("unreachable".into()))
            }
        }
        let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();
        let out = run_climb(
            &params(1),
            &DeadBuilder,
            &NoGate,
            None,
            &ledger,
            &mut journal,
        )
        .await;
        assert!(matches!(out.terminal, TerminalState::Blocked(_)));
    }

    /// A builder that records the feedback it was handed per attempt.
    struct RecordingBuilder {
        next: Mutex<u32>,
        seen_guidance: Mutex<Vec<Option<String>>>,
    }
    #[async_trait]
    impl Builder for RecordingBuilder {
        async fn build(
            &self,
            _task: &str,
            fb: Option<&BuildFeedback>,
        ) -> Result<BuiltCandidate, EngineError> {
            self.seen_guidance
                .lock()
                .unwrap()
                .push(fb.and_then(|f| f.valve_guidance.clone()));
            let mut n = self.next.lock().unwrap();
            let id = format!("c{n}");
            *n += 1;
            Ok(candidate(&id))
        }
    }

    /// A valve that returns fixed guidance and counts its fires.
    struct CountingValve {
        fires: Mutex<u32>,
    }
    #[async_trait]
    impl Valve for CountingValve {
        async fn diagnose(&self, _task: &str, stall: &StallReport) -> Result<String, EngineError> {
            *self.fires.lock().unwrap() += 1;
            assert!(stall.repeats >= 2, "valve fired before the stall rule");
            assert!(!stall.failing.is_empty());
            Ok("open src/lib.rs — the driver never reads it".to_string())
        }
    }

    #[tokio::test]
    async fn valve_fires_once_on_stall_and_guidance_reaches_the_builder() {
        // Same fail-set {b} three times = a stall after the 2nd repeat; the
        // valve fires ONCE, its guidance rides the next builder feedback, and
        // the climb then goes green.
        let builder = RecordingBuilder {
            next: Mutex::new(0),
            seen_guidance: Mutex::new(Vec::new()),
        };
        let gate = ScriptGate {
            reports: Mutex::new(
                vec![
                    report(vec![ok("a"), bad("b")]), // probe: {b}
                    report(vec![ok("a"), bad("b")]), // attempt: {b} again → stall
                    report(vec![ok("a"), ok("b")]),  // post-valve attempt: green
                ]
                .into(),
            ),
            stable_green: true,
        };
        let valve = CountingValve {
            fires: Mutex::new(0),
        };
        let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();
        let out = run_climb(
            &params(1),
            &builder,
            &gate,
            Some(&valve),
            &ledger,
            &mut journal,
        )
        .await;

        assert_eq!(out.terminal, TerminalState::Verified);
        assert_eq!(out.valve_fires, 1);
        assert_eq!(*valve.fires.lock().unwrap(), 1);
        let seen = builder.seen_guidance.lock().unwrap();
        // probe: no guidance; attempt 2: no guidance yet (stall detected after
        // its report); attempt 3: the valve guidance.
        assert_eq!(seen[0], None);
        assert_eq!(seen[1], None);
        assert!(seen[2].as_deref().unwrap_or("").contains("src/lib.rs"));
    }

    #[tokio::test]
    async fn valve_budget_is_one_then_honest_escalation() {
        // The wall never moves: valve fires once, budget exhausted, the climb
        // ends needs_escalation — never a second frontier turn.
        let builder = RecordingBuilder {
            next: Mutex::new(0),
            seen_guidance: Mutex::new(Vec::new()),
        };
        let stuck = || report(vec![ok("a"), bad("b")]);
        let gate = ScriptGate {
            reports: Mutex::new(vec![stuck(), stuck(), stuck(), stuck(), stuck()].into()),
            stable_green: false,
        };
        let valve = CountingValve {
            fires: Mutex::new(0),
        };
        let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();
        let out = run_climb(
            &params(1),
            &builder,
            &gate,
            Some(&valve),
            &ledger,
            &mut journal,
        )
        .await;

        assert_eq!(out.terminal, TerminalState::NeedsEscalation);
        assert_eq!(out.valve_fires, 1);
        assert_eq!(*valve.fires.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn changed_fail_hash_resets_the_stall_counter() {
        // {b} → {c} → {b}: three NOs but never the same wall twice in a row —
        // no stall, the valve never fires (the "same reason" clause).
        let builder = RecordingBuilder {
            next: Mutex::new(0),
            seen_guidance: Mutex::new(Vec::new()),
        };
        let gate = ScriptGate {
            reports: Mutex::new(
                vec![
                    report(vec![ok("a"), bad("b")]),
                    report(vec![bad("c"), ok("b"), ok("a")]),
                    report(vec![ok("a"), bad("b")]),
                    report(vec![bad("c"), ok("b"), ok("a")]),
                    report(vec![ok("a"), bad("b")]),
                ]
                .into(),
            ),
            stable_green: false,
        };
        let valve = CountingValve {
            fires: Mutex::new(0),
        };
        let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();
        let out = run_climb(
            &params(1),
            &builder,
            &gate,
            Some(&valve),
            &ledger,
            &mut journal,
        )
        .await;

        assert_eq!(out.valve_fires, 0);
        assert_eq!(*valve.fires.lock().unwrap(), 0);
        assert_eq!(out.terminal, TerminalState::NeedsEscalation);
    }

    /// A builder that tags every candidate's checkout identity with a shared drop
    /// counter, so a test can prove which candidates were terminalized (dropped)
    /// and which one is retained as the winner.
    struct TrackingBuilder {
        next: Mutex<u32>,
        dropped: Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait]
    impl Builder for TrackingBuilder {
        async fn build(
            &self,
            _task: &str,
            _fb: Option<&BuildFeedback>,
        ) -> Result<BuiltCandidate, EngineError> {
            let mut n = self.next.lock().unwrap();
            let id = format!("c{n}");
            *n += 1;
            Ok(BuiltCandidate {
                id: CandidateId::new(id.clone()),
                checkout: Box::new(FakeCheckout::tracked(&id, Arc::clone(&self.dropped))),
                spend: LedgerEntry::gate_exec(std::time::Duration::from_millis(1)),
            })
        }
    }

    #[tokio::test]
    async fn winner_retained_losers_terminalized_and_only_winner_survives() {
        use std::sync::atomic::Ordering;
        // Probe fails {b} (loser, superseded by the fix); a surgical attempt goes
        // green and is the winner. The displaced probe must terminalize (drop)
        // while the winner is retained in the outcome — and dropping the outcome
        // finally terminalizes the winner too, so nothing leaks.
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let builder = TrackingBuilder {
            next: Mutex::new(0),
            dropped: Arc::clone(&dropped),
        };
        let gate = ScriptGate {
            reports: Mutex::new(
                vec![
                    report(vec![ok("a"), bad("b")]), // probe c0: loser
                    report(vec![ok("a"), ok("b")]),  // surgical c1: winner (green)
                ]
                .into(),
            ),
            stable_green: true,
        };
        let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();
        let out = run_climb(&params(1), &builder, &gate, None, &ledger, &mut journal).await;

        assert_eq!(out.terminal, TerminalState::Verified);
        // The winner identity is retained; its display echo resolves.
        assert!(out.winner.is_some(), "winner identity must be retained");
        assert!(out.best_worktree.is_some());
        // Exactly the displaced probe (c0) has terminalized so far; the winner
        // (c1) is still live inside `out.winner`.
        assert_eq!(
            dropped.load(Ordering::SeqCst),
            1,
            "only the displaced loser is terminalized while the winner is held"
        );
        // Consuming the outcome (the parent lifecycle taking the winner) drops
        // the winner and terminalizes its transaction — nothing is leaked.
        drop(out);
        assert_eq!(
            dropped.load(Ordering::SeqCst),
            2,
            "dropping the outcome terminalizes the retained winner too"
        );
    }

    #[tokio::test]
    async fn no_winner_terminalizes_every_candidate() {
        use std::sync::atomic::Ordering;
        // The wall never moves: no candidate ever goes green, so there is no
        // winner and EVERY candidate (probe + surgical attempts) terminalizes by
        // the time the climb returns.
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let builder = TrackingBuilder {
            next: Mutex::new(0),
            dropped: Arc::clone(&dropped),
        };
        let stuck = || report(vec![ok("a"), bad("b")]);
        let gate = ScriptGate {
            reports: Mutex::new(vec![stuck(), stuck(), stuck(), stuck(), stuck()].into()),
            stable_green: false,
        };
        let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();
        let out = run_climb(&params(1), &builder, &gate, None, &ledger, &mut journal).await;

        assert!(out.winner.is_none(), "a no-green climb keeps no winner");
        assert!(out.best_worktree.is_none());
        let built = *builder.next.lock().unwrap();
        assert_eq!(
            dropped.load(Ordering::SeqCst) as u32,
            built,
            "every built candidate is terminalized when no winner is selected"
        );
    }
}
