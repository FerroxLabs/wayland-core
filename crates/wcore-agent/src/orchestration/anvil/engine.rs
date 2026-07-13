//! Anvil climb engine — the loop that turns the A1.5 decision core + substrate
//! into a real forge (spec §6). `run_climb` seeds a candidate, runs the pinned
//! gate, climbs by surgical fail-set-accepted steps, and produces an honest
//! terminal state + receipt payload.
//!
//! The loop is written over two injected seams so it is unit-testable without a
//! live spawner or sandbox (the same discipline as the rest of anvil):
//! - [`Builder`] produces a candidate in an isolated worktree (real impl: a
//!   forked sub-agent with edit tools; test impl: a fake).
//! - [`GateExecutor`] runs the pinned gate against a candidate's worktree and
//!   returns the per-check [`GateReport`] (real impl: the sandbox + a gate-output
//!   parser; test impl: a fake).
//!
//! The real seams and the `/forge` wiring live in [`super::forge`]; this file is
//! the engine. It consumes every substrate piece: the gate closure + probe
//! ([`super::gates`]), the cost ledger ([`super::ledger`]), the acceptance +
//! order decision core ([`super::climb`]), the crash-recovery journal
//! ([`super::journal`]).
//!
//! Spec: `docs/design/2026-07-12-anvil-native-gated-forge-design.md` (v2) §6.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::TerminalState;
use super::climb::{Acceptance, CandidateId, CheckId, GateReport, evaluate_acceptance};
use super::gates::StabilityPolicy;
use super::journal::{ClimbJournal, JournalEntry, JournalKind};
use super::ledger::{ClimbLedger, LedgerEntry};

/// A candidate build the climb produced, in its own isolated worktree.
#[derive(Debug, Clone)]
pub struct BuiltCandidate {
    /// Which attempt produced it.
    pub id: CandidateId,
    /// The isolated worktree holding this candidate's changes.
    pub worktree: PathBuf,
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

/// Runs the pinned gate against a candidate worktree. The real implementation
/// executes the closure in the sandbox and parses per-check results; tests use a
/// fake.
#[async_trait]
pub trait GateExecutor: Send + Sync {
    /// Run the gate against `worktree` and return its per-check report.
    async fn run(&self, worktree: &Path) -> Result<GateReport, EngineError>;
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
}

/// The result of a climb — everything the receipt needs (spec §8).
#[derive(Debug, Clone)]
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
    /// The winning candidate's worktree, if any reached a keepable state.
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
    gate: &dyn GateExecutor,
    ledger: &ClimbLedger,
    journal: &mut ClimbJournal,
) -> ClimbOutcome {
    let mut iterations: u32 = 0;

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
    let mut best = (probe, report.clone());

    if let Some(done) = check_verified(gate, &best, iterations, params).await {
        return done;
    }

    // ── Surgical climb: fix the failing checks, accept only non-regressions. ──
    while iterations < params.max_iterations {
        if ledger.is_exhausted() {
            break;
        }
        let feedback = feedback_from(&report);
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
                best = (candidate, candidate_report.clone());
                report = candidate_report;
                if let Some(done) = check_verified(gate, &best, iterations, params).await {
                    return done;
                }
            }
            Acceptance::Reject(_) => { /* logged via journal Candidate; keep best */ }
        }
    }

    // ── No stable-green candidate: report honestly. ──────────────────────────
    terminal_from_best(&best.1, iterations)
}

/// Run the gate on `candidate`, settle its build cost + a gate-exec entry into
/// the ledger, and journal the candidate step. Returns the report.
async fn gate_and_record(
    gate: &dyn GateExecutor,
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
    let report = gate.run(&candidate.worktree).await?;
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

/// If `best` is green AND clears the stability bar, produce the `verified`
/// outcome; otherwise `None` (keep climbing). A green-but-flaky gate is NOT
/// verification (spec §5 flake quarantine).
async fn check_verified(
    gate: &dyn GateExecutor,
    best: &(BuiltCandidate, GateReport),
    iterations: u32,
    params: &ClimbParams,
) -> Option<ClimbOutcome> {
    if !best.1.all_green() {
        return None;
    }
    if stability_holds(gate, &best.0.worktree, params.stability).await {
        Some(ClimbOutcome {
            terminal: TerminalState::Verified,
            stamp: STAMP_VERIFIED.to_string(),
            checks_passed: best.1.score(),
            checks_total: u32::try_from(best.1.total()).unwrap_or(u32::MAX),
            iterations,
            best_worktree: Some(best.0.worktree.clone()),
        })
    } else {
        // Green once but not stably — honest self-checked, quarantined (spec §2).
        Some(ClimbOutcome {
            terminal: TerminalState::NeedsEscalation,
            stamp: STAMP_SELF_CHECKED.to_string(),
            checks_passed: best.1.score(),
            checks_total: u32::try_from(best.1.total()).unwrap_or(u32::MAX),
            iterations,
            best_worktree: Some(best.0.worktree.clone()),
        })
    }
}

/// Re-run the gate `stability.of - 1` more times on the SAME worktree; the stamp
/// requires `stability.required` of `stability.of` identical-code passes (spec §5).
async fn stability_holds(
    gate: &dyn GateExecutor,
    worktree: &Path,
    stability: StabilityPolicy,
) -> bool {
    let mut passes = 1; // the run that already went green
    for _ in 1..stability.of {
        match gate.run(worktree).await {
            Ok(r) if r.all_green() => passes += 1,
            // A single non-green (or errored) rerun means the check flipped on
            // identical code — flaky, so verification is not earned.
            _ => return false,
        }
    }
    stability.met(passes)
}

/// Build surgical feedback from the current report.
fn feedback_from(report: &GateReport) -> BuildFeedback {
    BuildFeedback {
        failing: report.fail_set().ids().cloned().collect(),
        diagnostics: report.diagnostics.tail().to_string(),
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

/// Terminal state when no stable-green candidate was reached.
fn terminal_from_best(report: &GateReport, iterations: u32) -> ClimbOutcome {
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

    /// A builder that yields a fixed sequence of worktree ids.
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
            Ok(BuiltCandidate {
                id: CandidateId::new(id.clone()),
                worktree: PathBuf::from(format!("/wt/{id}")),
                spend: LedgerEntry::gate_exec(std::time::Duration::from_millis(1)),
            })
        }
    }

    /// A gate that returns a scripted report per worktree path, keyed by call order.
    struct ScriptGate {
        reports: Mutex<std::collections::VecDeque<GateReport>>,
        // A stable report to repeat for stability reruns of a green candidate.
        stable_green: bool,
    }
    #[async_trait]
    impl GateExecutor for ScriptGate {
        async fn run(&self, _wt: &Path) -> Result<GateReport, EngineError> {
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
        }
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
        run_climb(&params(stab_of), &builder, &gate, &ledger, &mut journal).await
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
        impl GateExecutor for NoGate {
            async fn run(&self, _w: &Path) -> Result<GateReport, EngineError> {
                Err(EngineError::Gate("unreachable".into()))
            }
        }
        let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
        let dir = tempfile::tempdir().unwrap();
        let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();
        let out = run_climb(&params(1), &DeadBuilder, &NoGate, &ledger, &mut journal).await;
        assert!(matches!(out.terminal, TerminalState::Blocked(_)));
    }
}
