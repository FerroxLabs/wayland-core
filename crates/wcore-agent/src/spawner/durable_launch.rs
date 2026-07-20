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

use super::mutation_workspace;
use super::{AgentSpawner, DurableSpawnerError, ResolvedChildLaunch};
use crate::child_transaction::MutationAttemptGuard;

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
