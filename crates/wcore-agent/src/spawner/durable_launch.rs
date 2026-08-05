//! Durable-launch → mutation-attempt bridge.
//!
//! 20-04 resolves a mutating child into a [`ResolvedChildLaunch`] that owns the
//! retained standalone-checkout handle (`TransactionWorkspace`). 20-12 needs
//! that SAME armed handle — moved, never reopened — inside a
//! [`MutationAttemptGuard`] so the accepted candidate keeps the live checkout
//! for exactly as long as the acceptance exists. This module is the only bridge
//! between the two: it moves the workspace out of the launch and derives the
//! gate-time transaction-private writable roots from it.

use std::path::PathBuf;

use wcore_types::spawner::{
    ChildOrigin, ForkOverrides, RequestedChildWorkspace, SubAgentConfig, SubAgentResult,
};

use super::mutation_workspace;
use super::{AgentSpawner, DurableSpawnerError, ResolvedChildLaunch, SpawnExtras};
use crate::child_transaction::MutationAttemptGuard;

impl AgentSpawner {
    /// Run one mutating builder child inside a freshly allocated,
    /// transaction-owned standalone checkout and hand the caller the SAME
    /// still-armed retained lifecycle handle instead of terminalizing it.
    ///
    /// This is the run-and-retain seam Anvil's forge needs (20-05). The normal
    /// durable fork path ([`Self::spawn_fork_with_origin`]) allocates the
    /// isolated checkout, runs the child, and drops → terminalizes the
    /// [`crate::spawner::ResolvedChildLaunch`]-owned checkout handle at
    /// child-scope exit. A gated forge must instead keep the exact same checkout
    /// live AFTER the builder returns so it can gate that candidate and,
    /// winner-only, hand it onward — while losers terminalize by RAII when the
    /// returned guard is dropped.
    ///
    /// Fail-closed contract (preserves every 20-04/20-06 isolation invariant):
    /// - The request MUST classify as [`RequestedChildWorkspace::IsolatedMutation`];
    ///   a shared read-only request is refused before any child work, so a
    ///   writing child can NEVER run outside an isolated checkout.
    /// - The checkout is allocated by the EXISTING production machinery
    ///   ([`Self::prepare_durable_launch`] → `prepare_child_workspace` →
    ///   `WorktreeManager::create_isolated_checkout`). No second checkout is
    ///   cloned, process-global CWD is never touched, and identity is never
    ///   derived from a bare path — the returned [`MutationAttemptGuard`] owns
    ///   the exact retained `TransactionWorkspace` (base/head/tree + retained
    ///   authorities) allocated for this child.
    /// - The child runs through the unchanged durable execute path; only the
    ///   terminal ownership of the checkout handle differs (handed to the caller
    ///   rather than dropped). Each call opens its OWN distinct transaction and
    ///   returns its OWN distinct guard — identity is never reused or collapsed
    ///   across calls.
    pub async fn spawn_builder_into_retained_checkout(
        &self,
        config: SubAgentConfig,
        overrides: ForkOverrides,
        origin: ChildOrigin,
    ) -> Result<(SubAgentResult, MutationAttemptGuard), DurableSpawnerError> {
        // Fail closed before any child work: a builder that classifies as shared
        // read-only has no isolated checkout to retain, and running a mutating
        // child in the parent checkout is exactly what this seam forbids.
        if overrides.requested_workspace() != RequestedChildWorkspace::IsolatedMutation {
            return Err(DurableSpawnerError::EvidenceMismatch(
                "retained-checkout builder spawn requires an isolated mutating workspace",
            ));
        }
        // Allocate the transaction-owned standalone checkout via the production
        // path; the resolved launch owns the retained handle.
        let mut launch = self.prepare_durable_launch(config, overrides).await?;
        // Move the retained checkout out of the launch BEFORE execution so the
        // durable execute path's scope-exit drop no longer terminalizes it; this
        // seam hands ownership to the caller instead. It is held here for the
        // whole child execution (the `TransactionCleanup` liveness), so the
        // checkout the child writes into is never cleaned mid-flight. The field
        // is reachable because this module descends from `spawner`.
        let workspace = launch.take_retained_transaction_workspace().ok_or(
            DurableSpawnerError::EvidenceMismatch(
                "isolated builder launch is missing its retained standalone checkout",
            ),
        )?;
        // Run the builder child through the unchanged durable execute path. The
        // child binds to the checkout via the launch's realized `workspace_root`,
        // not process CWD. The launch's `_transaction_workspace` is now `None`,
        // so its scope-exit drop is a no-op for the checkout.
        let result = self
            .execute_durable_launch(launch, SpawnExtras::default(), origin)
            .await;
        Ok((result, MutationAttemptGuard::new(workspace)))
    }
}

impl ResolvedChildLaunch {
    /// Take the retained standalone-checkout handle out of the launch, leaving
    /// the launch owning `None`.
    ///
    /// Unlike `into_transaction_workspace` (which consumes the whole launch to
    /// build a [`MutationAttemptGuard`] WITHOUT running the child), this takes
    /// the handle by `&mut` so the same launch can then run the builder child
    /// through the durable execute path with its scope-exit drop turned into a
    /// no-op for the checkout. The field is private to the `spawner` module and
    /// reachable here because this module descends from it.
    fn take_retained_transaction_workspace(
        &mut self,
    ) -> Option<wcore_swarm::worktree::TransactionWorkspace> {
        self._transaction_workspace.take()
    }
}

impl AgentSpawner {
    /// Move the durable launch's retained standalone-checkout handle into a
    /// [`MutationAttemptGuard`].
    ///
    /// The handle is the sole armed lifecycle handle from 20-04; it is moved,
    /// never reopened, so exactly one owner terminalizes the checkout. A shared
    /// read-only launch owns no transaction and is refused fail-closed — a
    /// mutation attempt can only ever be built over an isolated checkout.
    pub fn mutation_attempt_guard(
        &self,
        launch: ResolvedChildLaunch,
    ) -> Result<MutationAttemptGuard, DurableSpawnerError> {
        let workspace =
            launch
                .into_transaction_workspace()
                .ok_or(DurableSpawnerError::EvidenceMismatch(
                    "mutation attempt requires an isolated child workspace",
                ))?;
        Ok(MutationAttemptGuard::new(workspace))
    }

    /// The transaction-private writable roots a gate may write to for this
    /// mutation attempt. The candidate checkout is bound read-only by hard
    /// containment, so the retained scratch directory is the only writable
    /// mount a gate receives.
    #[must_use]
    pub fn mutation_writable_roots(&self, guard: &MutationAttemptGuard) -> Vec<PathBuf> {
        mutation_workspace::mutation_writable_roots(guard.workspace())
    }
}
