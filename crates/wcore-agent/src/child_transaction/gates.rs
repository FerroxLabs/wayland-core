//! Guard-owned `AcceptedCandidate` and the fail-closed gate acceptance machine.
//!
//! The acceptance machine consumes ONLY module-private
//! [`ObservedGateResult`](super::gate_executor::ObservedGateResult)s produced by
//! the [`GateExecutor`](super::gate_executor::GateExecutor) against ONE live
//! sealed candidate, in the plan's declared order. Missing, reordered,
//! duplicated, failed, or subject-mismatched evidence keeps the candidate
//! non-landing. Only after the observed results are lowered into an authoritative
//! durable receipt — built, conditionally appended, reopened from durable
//! storage, reduced, and matched by the authoritative receipt closure — does an
//! [`AcceptedCandidate`] exist.
//!
//! [`AcceptedCandidate`] is opaque: not serializable, not cloneable, with no
//! public constructor. It OWNS the original still-armed [`MutationAttemptGuard`]
//! (the retained standalone-checkout lifecycle handle moved out of the durable
//! launch — never a reopened path manager) plus the [`CandidateSeal`]. Dropping
//! it terminalizes the non-landing transaction and performs only owned cleanup.
//! This packet makes NO parent-landing / CAS / full-lifecycle claim.

use std::path::PathBuf;

use thiserror::Error;
use wcore_swarm::worktree::{CandidateSeal, TransactionWorkspace};
use wcore_types::child_transaction::{ChildGateOutcome, ChildGatePlan};

use super::gate_executor::{
    GateExecutionSubject, GateStageError, LiveCandidateRoot, ObservedGateResult,
};

/// The retained standalone-checkout lifecycle handle for one mutating child.
///
/// This is the SAME `!Clone` RAII handle the durable launch owns (20-04): it
/// owns the transaction-private checkout on disk and terminalizes it exactly
/// once on drop. 20-12 moves this original armed handle — never a reopened path
/// manager — into the acceptance machine, so the accepted candidate keeps the
/// live checkout for exactly as long as the acceptance exists.
pub struct MutationAttemptGuard {
    workspace: TransactionWorkspace,
}

impl std::fmt::Debug for MutationAttemptGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MutationAttemptGuard")
            .finish_non_exhaustive()
    }
}

impl MutationAttemptGuard {
    /// Take ownership of the durable launch's retained checkout handle.
    pub fn new(workspace: TransactionWorkspace) -> Self {
        Self { workspace }
    }

    /// The retained checkout workspace. Consumers mint seals and derive the
    /// live candidate cwd through this handle; it is never a bare path.
    pub fn workspace(&self) -> &TransactionWorkspace {
        &self.workspace
    }
}

/// Live source of the candidate cwd for a gate spawn, backed by the retained
/// checkout. Each `resolve_root` mints a fresh [`CandidateSeal`] (which
/// re-proves execution authority and the pristine-source manifest), so the cwd
/// can only ever be that of the exact live, clean, sealed candidate. A released
/// or drifted transaction fails closed here before any gate runs.
pub(crate) struct SealedCandidateRoot<'a> {
    guard: &'a MutationAttemptGuard,
}

impl<'a> SealedCandidateRoot<'a> {
    pub(crate) fn new(guard: &'a MutationAttemptGuard) -> Self {
        Self { guard }
    }
}

impl LiveCandidateRoot for SealedCandidateRoot<'_> {
    fn resolve_root(&self) -> Result<PathBuf, GateStageError> {
        // Minting the seal re-proves execution authority AND recomputes the
        // source manifest, so a released, drifted, or substituted checkout is
        // rejected before the cwd is handed to containment. The seal binds the
        // very same retained checkout authority whose display path is the cwd.
        let _seal = self
            .guard
            .workspace()
            .seal_candidate()
            .map_err(|error| GateStageError::Seal(error.to_string()))?;
        Ok(self
            .guard
            .workspace()
            .checkout_authority()
            .display_path()
            .to_path_buf())
    }
}

/// Opaque proof that one delegated mutation was accepted from parent-observed
/// gate execution plus authoritative durable replay.
///
/// Structural properties (all load-bearing):
/// - **Not serializable** (no `serde`) and **not cloneable** — the owned
///   [`CandidateSeal`] and [`MutationAttemptGuard`] are both `!Clone`, so a
///   serialized receipt or a duplicated handle can neither mint nor retain
///   acceptance.
/// - **No public constructor** — the only mint is the crate-private
///   [`AcceptanceMachine::accept`], reachable only after the durable receipt
///   closure confirms.
/// - **Guard-owned** — it OWNS the original still-armed guard plus the seal.
///   Field drop order releases the seal (its retained checkout-authority /
///   cleanup-liveness clones) BEFORE the guard, so the guard's checkout cleanup
///   never observes an outstanding seal loan.
///
/// Dropping the candidate terminalizes the non-landing transaction (via the
/// guard's checkout cleanup) and performs only owned cleanup. It makes NO
/// parent-landing / CAS / full-lifecycle claim.
pub struct AcceptedCandidate {
    // NOTE: field order is load-bearing. `seal` drops before `guard` so the
    // seal's retained checkout-authority clone and cleanup-liveness handle are
    // released before the guard terminalizes the checkout.
    seal: CandidateSeal,
    guard: MutationAttemptGuard,
    transaction_id: String,
    accepted_receipt_digest: String,
}

impl std::fmt::Debug for AcceptedCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redacted: neither the receipt digest nor the seal internals are shown.
        formatter
            .debug_struct("AcceptedCandidate")
            .field("transaction_id", &self.transaction_id)
            .finish_non_exhaustive()
    }
}

impl AcceptedCandidate {
    /// Crate-private, test-visible mint used by the acceptance machine after the
    /// authoritative receipt closure has confirmed the durable receipt.
    fn mint(
        guard: MutationAttemptGuard,
        seal: CandidateSeal,
        transaction_id: String,
        accepted_receipt_digest: String,
    ) -> Self {
        Self {
            seal,
            guard,
            transaction_id,
            accepted_receipt_digest,
        }
    }

    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    pub fn accepted_receipt_digest(&self) -> &str {
        &self.accepted_receipt_digest
    }

    /// Borrow the still-armed guard. There is no method that releases it without
    /// dropping (and thereby terminalizing) the candidate.
    pub fn guard(&self) -> &MutationAttemptGuard {
        &self.guard
    }

    /// Borrow the owned seal. It is opaque; this exists only so the candidate can
    /// hand its bound seal to a future landing plan (20-07), which is out of this
    /// packet's scope.
    pub fn seal(&self) -> &CandidateSeal {
        &self.seal
    }
}

/// The fail-closed acceptance state machine over one plan's observed results.
pub(crate) struct AcceptanceMachine;

impl AcceptanceMachine {
    /// Validate that `observed` is exactly the plan's gates, in declared order,
    /// all passed, and all bound to the same execution subject. Any deviation —
    /// missing, extra, reordered, duplicated, failed, or subject-mismatched
    /// evidence — keeps the candidate non-landing.
    pub(crate) fn validate_observed(
        plan: &ChildGatePlan,
        subject: &GateExecutionSubject,
        observed: &[ObservedGateResult],
    ) -> Result<(), AcceptanceError> {
        if observed.len() != plan.required_gates.len() {
            return Err(AcceptanceError::EvidenceCount {
                expected: plan.required_gates.len(),
                observed: observed.len(),
            });
        }
        for (result, requirement) in observed.iter().zip(&plan.required_gates) {
            if result.gate_id() != requirement.gate_id {
                return Err(AcceptanceError::Reordered(requirement.gate_id.clone()));
            }
            if result.outcome() != ChildGateOutcome::Passed {
                return Err(AcceptanceError::GateNotPassed(requirement.gate_id.clone()));
            }
            if result.subject() != subject {
                return Err(AcceptanceError::SubjectMismatch(
                    requirement.gate_id.clone(),
                ));
            }
        }
        Ok(())
    }

    /// Mint the [`AcceptedCandidate`] AFTER the authoritative receipt closure has
    /// confirmed the durable receipt. Consumes the guard and seal so acceptance
    /// owns them; there is no path from a serialized receipt to this call.
    pub(crate) fn accept(
        guard: MutationAttemptGuard,
        seal: CandidateSeal,
        transaction_id: String,
        accepted_receipt_digest: String,
    ) -> AcceptedCandidate {
        AcceptedCandidate::mint(guard, seal, transaction_id, accepted_receipt_digest)
    }
}

/// A fail-closed acceptance refusal. Every variant keeps the candidate
/// non-landing and durably diagnosable.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AcceptanceError {
    #[error("acceptance requires {expected} observed gate results, saw {observed}")]
    EvidenceCount { expected: usize, observed: usize },
    #[error("observed gate results are out of declared order at '{0}'")]
    Reordered(String),
    #[error("observed gate '{0}' did not pass")]
    GateNotPassed(String),
    #[error("observed gate '{0}' is bound to a different execution subject")]
    SubjectMismatch(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_swarm::worktree::WorktreeManager;

    async fn run_git(repo: &std::path::Path, args: &[&str]) {
        let mut command = wcore_config::shell::shell_command_argv("git", args);
        command.current_dir(repo);
        let output = command.output().await.expect("run git fixture command");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    async fn init_repo(repo: &std::path::Path) {
        run_git(repo, &["init"]).await;
        run_git(repo, &["config", "user.email", "wayland@example.invalid"]).await;
        run_git(repo, &["config", "user.name", "Wayland Test"]).await;
        std::fs::write(repo.join("README.md"), "guard drop fixture\n").unwrap();
        run_git(repo, &["add", "README.md"]).await;
        run_git(repo, &["commit", "-m", "fixture"]).await;
    }

    /// Building a real isolated checkout requires git; the checkout lifecycle is
    /// the swarm's, so this is gated to Linux (the harness platform) where the
    /// isolated-checkout machinery is exercised. The test proves that dropping
    /// the guard the accepted candidate owns terminalizes the transaction and
    /// removes the checkout from disk — i.e. acceptance drop performs the owned
    /// cleanup and nothing is leaked.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn guard_drop_terminalizes_and_cleans() {
        let repo = tempfile::tempdir().unwrap();
        init_repo(repo.path()).await;
        let state = tempfile::tempdir().unwrap();
        let checkouts = state.path().join("checkouts");
        std::fs::create_dir_all(&checkouts).unwrap();

        let manager = WorktreeManager::new_with_workspace_root(repo.path(), &checkouts)
            .expect("worktree manager");
        let pinned_head = manager.pinned_head().await.expect("pinned head");
        let capacity = manager.workspace_capacity(1).await.expect("capacity");
        let workspace = manager
            .create_isolated_checkout(
                "guard-drop-child",
                "wayland-child/guard-drop-child",
                &pinned_head,
                capacity,
            )
            .await
            .expect("isolated checkout");

        // The realized checkout exists on disk and is the retained handle's cwd.
        let checkout_root = workspace.checkout_authority().display_path().to_path_buf();
        assert!(checkout_root.is_dir(), "checkout must exist after creation");

        // Mint the live seal from the retained checkout, then wrap the same armed
        // handle in the guard and hand both to an accepted candidate.
        let seal = workspace.seal_candidate().expect("seal candidate");
        let guard = MutationAttemptGuard::new(workspace);
        let accepted = AcceptanceMachine::accept(
            guard,
            seal,
            "transaction-guard-drop".to_owned(),
            "f".repeat(64),
        );
        assert_eq!(accepted.transaction_id(), "transaction-guard-drop");
        assert_eq!(accepted.accepted_receipt_digest(), &"f".repeat(64));
        // The seal and guard are genuinely owned (borrow to prove they are live).
        let _ = accepted.seal();
        let _ = accepted.guard();
        assert!(
            checkout_root.is_dir(),
            "checkout must still exist while the accepted candidate is held"
        );

        // Dropping the accepted candidate drops the seal, then the guard, whose
        // checkout cleanup terminalizes the transaction and removes the checkout.
        drop(accepted);
        assert!(
            !checkout_root.exists(),
            "checkout must be terminalized and removed after acceptance drop"
        );
    }
}
