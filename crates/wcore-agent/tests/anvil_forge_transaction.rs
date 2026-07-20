//! Anvil forge transactional-identity regression (20-05).
//!
//! Proves the load-bearing properties of routing the Anvil forge through the
//! production spawner's run-and-retain seam, at the reachable public engine
//! surface:
//!
//! * Every climb candidate carries its OWN distinct, opaque checkout identity;
//!   identities are never reused or collapsed across candidates.
//! * The advisory [`EvaluationGateExecutor`] resolves each candidate's subject
//!   root THROUGH that candidate's identity — never a bare path handed in — so a
//!   candidate is always gated against its own checkout.
//! * `run_climb` retains ONLY the selected winner; every displaced/rejected
//!   loser terminalizes (RAII) and its checkout is cleaned. Dropping the outcome
//!   terminalizes the winner too — nothing leaks.
//! * The climb performs NO process-global CWD mutation (the regression the old
//!   `std::env::set_current_dir` builder introduced), and the parent workspace
//!   is never mutated.
//!
//! The portable test exercises the full climb over fake identities (plumbing +
//! CWD invariants). The Linux test exercises REAL transaction-owned standalone
//! checkouts allocated by the swarm machinery the production seam uses, proving
//! on-disk winner retention and loser cleanup.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;

use wcore_agent::orchestration::anvil::climb::{CandidateId, CheckOutcome, GateReport, Severity};
use wcore_agent::orchestration::anvil::engine::{
    BuildFeedback, Builder, BuiltCandidate, CandidateCheckout, ClimbParams, EngineError,
    EvaluationGateExecutor, run_climb,
};
use wcore_agent::orchestration::anvil::gates::{BoundedGateOutput, StabilityPolicy};
use wcore_agent::orchestration::anvil::journal::ClimbJournal;
use wcore_agent::orchestration::anvil::ledger::{ClimbLedger, LedgerCap, LedgerEntry};

// ── Shared fixtures ─────────────────────────────────────────────────────────

fn ok(id: &str) -> CheckOutcome {
    CheckOutcome::new(id, true, Severity::Major)
}
fn bad(id: &str) -> CheckOutcome {
    CheckOutcome::new(id, false, Severity::Major)
}
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

/// A builder that hands out a fixed queue of pre-built candidates, each with its
/// own distinct identity. Pre-building lets a test assign the exact identities
/// and observe their lifecycle.
struct QueueBuilder {
    queue: Mutex<std::collections::VecDeque<BuiltCandidate>>,
}
#[async_trait]
impl Builder for QueueBuilder {
    async fn build(
        &self,
        _task: &str,
        _feedback: Option<&BuildFeedback>,
    ) -> Result<BuiltCandidate, EngineError> {
        self.queue
            .lock()
            .pop_front()
            .ok_or_else(|| EngineError::Builder("queue exhausted".into()))
    }
}

/// An advisory gate that RECORDS the root it resolved through each candidate's
/// identity (proving the gate is handed the identity, not a bare path) and
/// returns scripted reports in call order.
struct RecordingGate {
    reports: Mutex<std::collections::VecDeque<GateReport>>,
    resolved_roots: Mutex<Vec<PathBuf>>,
}
#[async_trait]
impl EvaluationGateExecutor for RecordingGate {
    async fn run(&self, candidate: &dyn CandidateCheckout) -> Result<GateReport, EngineError> {
        // Resolve THROUGH the identity — never a bare path — and record it.
        let root = candidate.resolve_root()?;
        self.resolved_roots.lock().push(root);
        self.reports
            .lock()
            .pop_front()
            .ok_or_else(|| EngineError::Gate("no scripted report".into()))
    }
}

fn params() -> ClimbParams {
    ClimbParams {
        task: "t".into(),
        // 1-of-1 stability: a single green run, no reruns.
        stability: StabilityPolicy::new(1, 1),
        max_iterations: 3,
        gate_closure_digest: "deadbeef".into(),
        // No valve in these tests.
        stall_after: u32::MAX,
        deadline: None,
    }
}

/// The canonical 3-candidate climb: an initial best (probe, fails `{b}`), a
/// REJECTED replacement (regresses to `{a,b} ⊃ {b}`), and an ACCEPTED winner
/// (green). Reports are consumed in call order.
fn three_candidate_reports() -> std::collections::VecDeque<GateReport> {
    vec![
        report(vec![ok("a"), bad("b")]),  // c0 probe → {b} (initial best)
        report(vec![bad("a"), bad("b")]), // c1 surgical → {a,b} ⊃ {b} → REJECT
        report(vec![ok("a"), ok("b")]),   // c2 surgical → green → ACCEPT (winner)
    ]
    .into()
}

// ── Portable identity + CWD invariants (fake checkouts) ─────────────────────

/// A fake candidate identity: a stable in-memory root plus a shared drop counter
/// so a test can observe terminalization order without a live checkout.
#[derive(Debug)]
struct FakeCheckout {
    root: PathBuf,
    dropped: Arc<AtomicUsize>,
}
impl CandidateCheckout for FakeCheckout {
    fn resolve_root(&self) -> Result<PathBuf, EngineError> {
        Ok(self.root.clone())
    }
}
impl Drop for FakeCheckout {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

fn fake_candidate(id: &str, dropped: &Arc<AtomicUsize>) -> (BuiltCandidate, PathBuf) {
    let root = PathBuf::from(format!("/anvil-cand/{id}"));
    let candidate = BuiltCandidate {
        id: CandidateId::new(id),
        checkout: Box::new(FakeCheckout {
            root: root.clone(),
            dropped: Arc::clone(dropped),
        }),
        spend: LedgerEntry::gate_exec(std::time::Duration::from_millis(1)),
    };
    (candidate, root)
}

#[tokio::test]
async fn distinct_identities_no_cwd_change_winner_retained_losers_dropped() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let (c0, r0) = fake_candidate("c0", &dropped);
    let (c1, r1) = fake_candidate("c1", &dropped);
    let (c2, r2) = fake_candidate("c2", &dropped);

    // Every candidate has a DISTINCT identity — never reused/collapsed.
    assert_ne!(r0, r1);
    assert_ne!(r1, r2);
    assert_ne!(r0, r2);

    let builder = QueueBuilder {
        queue: Mutex::new(vec![c0, c1, c2].into()),
    };
    let gate = RecordingGate {
        reports: Mutex::new(three_candidate_reports()),
        resolved_roots: Mutex::new(Vec::new()),
    };
    let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
    let dir = tempfile::tempdir().unwrap();
    let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();

    // Regression guard: the climb must NEVER mutate process-global CWD.
    let cwd_before = std::env::current_dir().unwrap();
    let outcome = run_climb(&params(), &builder, &gate, None, &ledger, &mut journal).await;
    let cwd_after = std::env::current_dir().unwrap();
    assert_eq!(
        cwd_before, cwd_after,
        "the climb must not change the process working directory"
    );

    // The gate resolved each candidate's OWN root through its identity.
    let resolved = gate.resolved_roots.lock().clone();
    assert_eq!(resolved, vec![r0.clone(), r1.clone(), r2.clone()]);

    // Winner-only selection: c2 is retained, its root echoed.
    assert_eq!(outcome.best_worktree.as_ref(), Some(&r2));
    let winner_root = outcome
        .winner
        .as_ref()
        .expect("winner identity retained")
        .resolve_root()
        .unwrap();
    assert_eq!(winner_root, r2);

    // Losers c0 (displaced best) and c1 (rejected) have terminalized; the winner
    // is still held live inside the outcome.
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        2,
        "both losers terminalize while the winner is retained"
    );
    // Consuming the outcome terminalizes the winner too — nothing leaks.
    drop(outcome);
    assert_eq!(dropped.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn no_winner_terminalizes_every_candidate() {
    // The wall never moves: no candidate goes green, so no winner is retained and
    // every candidate terminalizes by the time the climb returns.
    let dropped = Arc::new(AtomicUsize::new(0));
    let (c0, _r0) = fake_candidate("c0", &dropped);
    let (c1, _r1) = fake_candidate("c1", &dropped);
    let (c2, _r2) = fake_candidate("c2", &dropped);
    let builder = QueueBuilder {
        queue: Mutex::new(vec![c0, c1, c2].into()),
    };
    let stuck = || report(vec![ok("a"), bad("b")]);
    let gate = RecordingGate {
        reports: Mutex::new(vec![stuck(), stuck(), stuck()].into()),
        resolved_roots: Mutex::new(Vec::new()),
    };
    let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
    let dir = tempfile::tempdir().unwrap();
    let mut journal = ClimbJournal::open(dir.path().join("j")).unwrap();

    let outcome = run_climb(&params(), &builder, &gate, None, &ledger, &mut journal).await;

    assert!(outcome.winner.is_none(), "no green ⇒ no retained winner");
    assert!(outcome.best_worktree.is_none());
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        3,
        "every candidate terminalizes when no winner is selected"
    );
}

// ── Real transaction-owned checkouts (Linux harness) ────────────────────────

#[cfg(target_os = "linux")]
mod real_checkouts {
    use super::*;
    use wcore_agent::child_transaction::MutationAttemptGuard;
    use wcore_swarm::worktree::WorktreeManager;

    /// The production-shaped candidate identity: a retained, transaction-owned
    /// standalone checkout wrapped in a [`MutationAttemptGuard`], resolving its
    /// root through a fresh candidate seal on every access (re-proving execution
    /// authority) — the exact contract the forge's `RetainedCheckout` uses.
    #[derive(Debug)]
    struct GuardCheckout {
        guard: MutationAttemptGuard,
    }
    impl CandidateCheckout for GuardCheckout {
        fn resolve_root(&self) -> Result<PathBuf, EngineError> {
            self.guard
                .workspace()
                .seal_candidate()
                .map_err(|error| EngineError::Gate(format!("candidate seal refused: {error}")))?;
            Ok(self
                .guard
                .workspace()
                .checkout_authority()
                .display_path()
                .to_path_buf())
        }
    }

    fn run_git(repo: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_repo(repo: &std::path::Path) {
        run_git(repo, &["init"]);
        run_git(repo, &["config", "user.email", "wayland@example.invalid"]);
        run_git(repo, &["config", "user.name", "Wayland Test"]);
        std::fs::write(repo.join("README.md"), "candidate fixture\n").unwrap();
        run_git(repo, &["add", "README.md"]);
        run_git(repo, &["commit", "-m", "fixture"]);
    }

    async fn real_candidate(
        manager: &WorktreeManager,
        pinned_head: &str,
        child: &str,
        dropped: &Arc<AtomicUsize>,
    ) -> (BuiltCandidate, PathBuf) {
        let capacity = manager.workspace_capacity(1).await.expect("capacity");
        let workspace = manager
            .create_isolated_checkout(child, &format!("anvil-cand/{child}"), pinned_head, capacity)
            .await
            .expect("isolated checkout");
        let root = workspace.checkout_authority().display_path().to_path_buf();
        let guard = MutationAttemptGuard::new(workspace);
        // A dedicated drop-counter identity: wrap the guard so the test observes
        // terminalization order as the engine drops losers.
        struct Counted {
            inner: GuardCheckout,
            dropped: Arc<AtomicUsize>,
        }
        impl std::fmt::Debug for Counted {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("Counted").finish_non_exhaustive()
            }
        }
        impl CandidateCheckout for Counted {
            fn resolve_root(&self) -> Result<PathBuf, EngineError> {
                self.inner.resolve_root()
            }
        }
        impl Drop for Counted {
            fn drop(&mut self) {
                self.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
        let candidate = BuiltCandidate {
            id: CandidateId::new(child),
            checkout: Box::new(Counted {
                inner: GuardCheckout { guard },
                dropped: Arc::clone(dropped),
            }),
            spend: LedgerEntry::gate_exec(std::time::Duration::from_millis(1)),
        };
        (candidate, root)
    }

    #[tokio::test]
    async fn real_checkouts_isolated_winner_retained_losers_cleaned() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let state = tempfile::tempdir().unwrap();
        let checkouts = state.path().join("checkouts");
        std::fs::create_dir_all(&checkouts).unwrap();
        let manager =
            WorktreeManager::new_with_workspace_root(repo.path(), &checkouts).expect("manager");
        let pinned_head = manager.pinned_head().await.expect("pinned head");

        let dropped = Arc::new(AtomicUsize::new(0));
        let (c0, r0) = real_candidate(&manager, &pinned_head, "c0", &dropped).await;
        let (c1, r1) = real_candidate(&manager, &pinned_head, "c1", &dropped).await;
        let (c2, r2) = real_candidate(&manager, &pinned_head, "c2", &dropped).await;

        // Distinct, real, on-disk checkouts.
        assert_ne!(r0, r1);
        assert_ne!(r1, r2);
        assert_ne!(r0, r2);
        assert!(r0.is_dir() && r1.is_dir() && r2.is_dir());

        let builder = QueueBuilder {
            queue: Mutex::new(vec![c0, c1, c2].into()),
        };
        let gate = RecordingGate {
            reports: Mutex::new(three_candidate_reports()),
            resolved_roots: Mutex::new(Vec::new()),
        };
        let ledger = ClimbLedger::new("t", LedgerCap::unlimited());
        let jdir = tempfile::tempdir().unwrap();
        let mut journal = ClimbJournal::open(jdir.path().join("j")).unwrap();

        let cwd_before = std::env::current_dir().unwrap();
        let outcome = run_climb(&params(), &builder, &gate, None, &ledger, &mut journal).await;
        let cwd_after = std::env::current_dir().unwrap();
        assert_eq!(cwd_before, cwd_after, "climb must not change process CWD");

        // No sibling-checkout substitution: each candidate was gated against its
        // OWN real checkout, resolved through its own identity (seal), in order.
        assert_eq!(
            gate.resolved_roots.lock().clone(),
            vec![r0.clone(), r1.clone(), r2.clone()]
        );

        // Winner c2 retained on disk; losers c0/c1 terminalized and removed.
        assert_eq!(outcome.best_worktree.as_ref(), Some(&r2));
        assert!(r2.is_dir(), "winner checkout must survive the climb");
        assert!(
            !r0.exists(),
            "displaced best must be terminalized and removed"
        );
        assert!(
            !r1.exists(),
            "rejected candidate must be terminalized and removed"
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 2);

        // The parent repository working tree was never mutated (checkouts live
        // outside the repo; nothing leaked back in).
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(repo.path())
            .output()
            .expect("git status");
        assert!(
            String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "the parent workspace must be untouched by the climb"
        );

        // Consuming the outcome terminalizes the winner too — no leak.
        drop(outcome);
        assert_eq!(dropped.load(Ordering::SeqCst), 3);
        assert!(
            !r2.exists(),
            "winner checkout is cleaned once the outcome is consumed"
        );
    }

    #[tokio::test]
    async fn seal_backed_identity_resolves_its_own_live_checkout() {
        // The production-shaped identity resolves the SAME checkout root on every
        // access, re-minting a fresh candidate seal each time (re-proving
        // execution authority) — the exact per-candidate subject the gate uses.
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path());
        let state = tempfile::tempdir().unwrap();
        let checkouts = state.path().join("checkouts");
        std::fs::create_dir_all(&checkouts).unwrap();
        let manager =
            WorktreeManager::new_with_workspace_root(repo.path(), &checkouts).expect("manager");
        let pinned_head = manager.pinned_head().await.expect("pinned head");
        let capacity = manager.workspace_capacity(1).await.expect("capacity");
        let workspace = manager
            .create_isolated_checkout("solo", "anvil-cand/solo", &pinned_head, capacity)
            .await
            .expect("isolated checkout");
        let root = workspace.checkout_authority().display_path().to_path_buf();
        let identity = GuardCheckout {
            guard: MutationAttemptGuard::new(workspace),
        };
        // Stable across repeated resolution (each mints a fresh seal), and bound
        // to the live on-disk checkout.
        assert_eq!(identity.resolve_root().unwrap(), root);
        assert_eq!(identity.resolve_root().unwrap(), root);
        assert!(root.is_dir());
        // Dropping the identity terminalizes the transaction and removes it.
        drop(identity);
        assert!(!root.exists(), "dropping the identity cleans its checkout");
    }
}
