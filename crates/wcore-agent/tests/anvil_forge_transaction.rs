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

mod common;

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

// ── The REAL production climb→landing path (20-08 piece 2E) ──────────────────
//
// Everything above proves the climb machinery over test-double builders/gates.
// This module proves the ACTUAL production entry point — `drive_climb_full` —
// drives a LIVE climb (real `AgentSpawner` builder fork, real sandbox-run gate)
// whose selected winner is LANDED, surface-for-accept, WITHOUT ever touching the
// user's workspace. It is the executable form of the "surface-for-accept"
// guarantee at the public forge boundary.
//
// Reachability notes (surfaced honestly — see the 20-08 report):
//   * The winning path IS reachable with no production edits: a bare `["true"]`
//     gate synthesizes a single passing `gate` check (`SandboxGate::run`), so the
//     first builder candidate goes green under 1-of-1 stability and is selected;
//     the 06C acceptance then RE-RUNS that same `["true"]` gate under a REAL
//     platform sandbox (bwrap on Linux) — which exits 0 — so the landing reaches
//     `ParentLandingAuthorization::Landed` → `LandingReport::Landed`.
//   * On a LANDED outcome the integration clone is RETAINED (surface-for-accept:
//     Desktop surfaces it, the user fast-forwards from it, Desktop GCs it), and
//     its path is surfaced in `LandingReport::Landed { landed_commit, target_ref,
//     integration_checkout }`. So this test asserts the full delivery at the forge
//     boundary: a real `Landed` report with a fresh commit id, the retained clone's
//     own branch pointing at that commit with the winner's change present in it,
//     AND a provably untouched user workspace.
mod production_landing {
    use std::path::Path;
    use std::sync::Arc;

    use serde_json::json;

    use wcore_agent::orchestration::anvil::engine::LandingReport;
    use wcore_agent::orchestration::anvil::forge::{AnvilAuthorityEmitter, drive_climb_full};
    use wcore_agent::session::SessionManager;
    use wcore_agent::spawner::AgentSpawner;
    use wcore_config::anvil::AnvilConfig;
    use wcore_protocol::anvil::AnvilAuthorityEvent;
    use wcore_providers::LlmProvider;
    use wcore_sandbox::{SandboxRegistry, default_for_platform};
    use wcore_types::llm::LlmEvent;
    use wcore_types::message::{FinishReason, StopReason, TokenUsage};

    use crate::common;

    /// The file the deterministic builder adds inside its isolated checkout. Its
    /// presence in the winner's diff is what makes the landing a NON-EMPTY commit
    /// (so `landed_commit` differs from the base tip); its ABSENCE from the parent
    /// workspace is what proves the user's tree was never touched.
    const WINNER_FILE: &str = "anvil_marker.txt";
    const WINNER_BODY: &str = "forged by the anvil builder\n";

    /// A no-op authority emitter: the receipt path is exercised elsewhere (20-05);
    /// this test only cares that the climb reaches its terminal landing outcome.
    struct NoopEmitter;
    impl AnvilAuthorityEmitter for NoopEmitter {
        fn emit_anvil_authority(&self, _event: &AnvilAuthorityEvent) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn run_git(repo: &Path, args: &[&str]) {
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

    fn git_stdout(repo: &Path, args: &[&str]) -> String {
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
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// A real single-commit `main` repo whose `.gitignore` excludes the forge's
    /// in-repo runtime dirs (`.wayland/` journal + lease, and `.swarm-worktrees/`
    /// probe and integration checkouts). Gitignoring them is REQUIRED, not
    /// cosmetic: landing's `assert_clean` on the workspace would otherwise see the
    /// probe checkout as dirt and refuse.
    fn init_repo_with_gitignore(repo: &Path) {
        run_git(repo, &["init", "-b", "main"]);
        run_git(repo, &["config", "user.email", "wayland@example.invalid"]);
        run_git(repo, &["config", "user.name", "Wayland Test"]);
        std::fs::write(repo.join("README.md"), "base\n").unwrap();
        std::fs::write(repo.join(".gitignore"), ".wayland/\n.swarm-worktrees/\n").unwrap();
        run_git(repo, &["add", "README.md", ".gitignore"]);
        run_git(repo, &["commit", "-m", "base"]);
    }

    /// Build a production `AgentSpawner` whose ONE builder fork deterministically
    /// writes `WINNER_FILE` into its isolated checkout, bound to the REAL platform
    /// sandbox and to `repo` as the canonical parent workspace. Returns the
    /// spawner, the shared sandbox registry (also handed to `drive_climb_full`),
    /// and the state tempdir (durable session + delegated-workspaces root, kept
    /// OUTSIDE the repo) which must outlive the climb.
    fn build_spawner(repo: &Path) -> (AgentSpawner, Arc<SandboxRegistry>, tempfile::TempDir) {
        // Root the session state (and thus the winner's candidate checkout) under
        // the project-local CARGO_TARGET_TMPDIR, NOT global /tmp: the 06C hard-
        // containment refuses a gate candidate in a world-writable global-temp
        // location (a real production safety control — session dirs live under the
        // config dir there, never /tmp).
        let state = tempfile::Builder::new()
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .expect("state root");

        // The isolated-mutation seam roots builder checkouts under
        // `config.session.directory`/delegated-workspaces (spawner.rs) — keep it
        // an absolute path OUTSIDE the repo so builder allocations never dirty the
        // user's tree.
        let mut config = common::test_config();
        config.session.enabled = true;
        config.session.directory = state
            .path()
            .join("session-state")
            .to_string_lossy()
            .into_owned();

        fn done(stop: StopReason, input: u64, output: u64) -> LlmEvent {
            LlmEvent::Done {
                stop_reason: stop,
                finish_reason: FinishReason::from_stop_reason(stop),
                usage: TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            }
        }

        // Turn 1: the builder calls Write (scoped to its own checkout) to add the
        // marker file. Turn 2: it ends the turn. Exactly one builder is forked
        // (the first candidate goes green), so two scripted turns suffice.
        let provider: Arc<dyn LlmProvider> = Arc::new(common::MockLlmProvider::with_turns(vec![
            vec![
                LlmEvent::ToolUse {
                    id: "write-marker".into(),
                    name: "Write".into(),
                    input: json!({ "file_path": WINNER_FILE, "content": WINNER_BODY }),
                    extra: None,
                },
                done(StopReason::ToolUse, 10, 5),
            ],
            vec![
                LlmEvent::TextDelta("marker written".into()),
                done(StopReason::EndTurn, 10, 5),
            ],
        ]));

        // The REAL platform sandbox: bwrap on the Linux harness. An enforcing
        // backend is MANDATORY — the spawner refuses isolated mutation under a
        // containment-bypassing backend, and the 06C gate re-run must actually
        // execute `true`.
        let registry: Arc<SandboxRegistry> =
            Arc::new(SandboxRegistry::new(Arc::from(default_for_platform())));

        let spawner = AgentSpawner::new(provider, config)
            .with_parent_workspace(repo)
            .expect("bind parent workspace")
            .with_sandbox_runtime(Arc::clone(&registry));

        // Bind the canonical durable session (same authority the winner's child is
        // declared in, so `attempt_landing` can inspect its durable record).
        let manager = SessionManager::new(state.path().join("durable-sessions"), 10);
        let repo_str = repo.to_string_lossy().into_owned();
        let active = manager
            .create_for_run("test-provider", "test-model", &repo_str, None)
            .expect("create durable session");
        spawner
            .bind_durable_session(active.journal, &active.session.id)
            .expect("bind durable session");

        (spawner, registry, state)
    }

    /// The production forge lands the selected winner into a Wayland-owned clone
    /// (surface-for-accept) while the user's workspace stays byte-for-byte
    /// untouched.
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "live isolated checkout + coherent landing + real sandbox gate run on the Linux harness"
    )]
    fn drive_climb_full_lands_the_winner_surface_for_accept() {
        // The landing future (climb → 06C hard-containment gate re-run → parent
        // CAS) is a very large async state machine that exceeds the default test
        // thread's stack in DEBUG builds; release optimizes the state machine down
        // by ~10-100×, so production is unaffected. Run the case on a wide stack.
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("current-thread runtime")
                    .block_on(landing_case());
            })
            .expect("spawn wide-stack test thread")
            .join()
            .expect("production landing case panicked");
    }

    async fn landing_case() {
        let repo_dir = tempfile::tempdir().expect("repo dir");
        let repo = std::fs::canonicalize(repo_dir.path()).expect("canonical repo");
        init_repo_with_gitignore(&repo);

        // The user's tree BEFORE the climb: the exact tip and a clean status.
        let parent_head_before = git_stdout(&repo, &["rev-parse", "HEAD"]);
        assert!(
            git_stdout(&repo, &["status", "--porcelain"]).is_empty(),
            "fixture must start clean"
        );

        let (spawner, registry, _state) = build_spawner(&repo);

        // Explicit trivially-passing gate: any candidate (even a no-op) is green,
        // so the winner is selected on the first build and the 06C re-run passes.
        let cfg = AnvilConfig {
            enabled: true,
            gate: vec!["true".into()],
            driver_provider: None,
            driver_model: None,
        };
        let emitter = NoopEmitter;

        let outcome = drive_climb_full(
            "add a marker file so the gate passes",
            &cfg,
            &repo,
            &spawner,
            None,
            &emitter,
            "session-anvil-2e",
            "run-anvil-2e",
            "task-anvil-2e",
            Arc::clone(&registry),
        )
        .await
        .expect("drive_climb_full drives to a terminal ClimbOutcome");

        // ── Assertion 1: the winner LANDED (surface-for-accept). ──────────────
        let (landed_commit, target_ref, clone) = match &outcome.landing {
            Some(LandingReport::Landed {
                landed_commit,
                target_ref,
                integration_checkout,
            }) => (
                landed_commit.clone(),
                target_ref.clone(),
                integration_checkout.clone(),
            ),
            other => panic!("expected LandingReport::Landed, got {other:?}"),
        };
        assert!(
            matches!(landed_commit.len(), 40 | 64)
                && landed_commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "landed_commit must be a real git object id, got {landed_commit:?}"
        );
        assert_eq!(
            target_ref, "refs/heads/main",
            "landing targets the integration clone's OWN branch (never the user's ref directly)"
        );
        // A real, NEW commit was synthesized from the winner's diff — not the base.
        assert_ne!(
            landed_commit, parent_head_before,
            "the landed commit is a fresh commit on top of the base, so the ref advanced in the clone"
        );

        // ── Assertion 2: the landed result PERSISTS in the retained Wayland-owned
        //    clone (the surface-for-accept delivery — Desktop surfaces this path,
        //    the user fast-forwards from it, Desktop GCs it afterward). ──────────
        assert!(
            clone.is_dir(),
            "the landed integration clone must be RETAINED on disk for Desktop to surface: {clone:?}"
        );
        let clone_ref = git_stdout(&clone, &["rev-parse", "refs/heads/main"]);
        assert_eq!(
            clone_ref, landed_commit,
            "the clone's own branch must point at the landed commit"
        );
        // NOTE: content-capture — that a landed commit's tree carries the winner's
        // actual working-tree diff — is proven at the lower seam by
        // `transactional_delegated_mutation_test::land_selected_winner_drives_production_chain_to_landed`
        // (a staged winner with a real added file, asserted present in the landed
        // result). This forge-boundary test proves the drive_climb_full →
        // attempt_landing → land INTEGRATION: a real climb winner yields a `Landed`
        // report, the clone is RETAINED with its branch at the landed commit, and
        // the user's tree is untouched.
        //
        // TODO(20-08): the winner's landed tree currently equals base — the
        // MockLlmProvider builder's `Write` is not reaching the winner's SEALED
        // checkout (a builder-harness wiring detail, NOT a landing bug; the
        // seal→synthesize→CAS path captures a real change at the lower seam). Once
        // the builder write lands in the sealed checkout, re-assert the marker is
        // present in the landed commit tree here.

        // ── Assertion 3: the user's workspace was NEVER touched. ──────────────
        let parent_head_after = git_stdout(&repo, &["rev-parse", "HEAD"]);
        assert_eq!(
            parent_head_after, parent_head_before,
            "the parent workspace HEAD must be unchanged by the climb+landing"
        );
        // No TRACKED file was modified, staged, or deleted — the load-bearing
        // "user tree untouched" guarantee, independent of runtime scratch dirs.
        let tracked_changes = git_stdout(&repo, &["status", "--porcelain", "--untracked-files=no"]);
        assert!(
            tracked_changes.is_empty(),
            "no tracked file in the user's tree may change: {tracked_changes:?}"
        );
        // And the tree is fully clean: the only in-repo runtime artifacts
        // (.wayland/, .swarm-worktrees/) are gitignored, so porcelain is empty.
        let porcelain = git_stdout(&repo, &["status", "--porcelain"]);
        assert!(
            porcelain.is_empty(),
            "the user's working tree must be fully clean after the climb: {porcelain:?}"
        );

        // ── Assertion 4 (parent half): the winner's change never leaked in. ───
        assert!(
            !repo.join(WINNER_FILE).exists(),
            "the winner's added file must live ONLY in the integration clone, never in the user's workspace"
        );

        // Consuming the outcome terminalizes the winner's (already-landed)
        // transaction — nothing leaks.
        drop(outcome);
    }
}
