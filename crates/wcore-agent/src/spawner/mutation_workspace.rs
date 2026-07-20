//! Mutation-attempt workspace helpers.
//!
//! Pure derivations over a retained isolated checkout
//! ([`TransactionWorkspace`]) that the durable-launch bridge uses to describe a
//! mutation attempt's containment. Keeping them here isolates the
//! checkout-shape knowledge (which directory is the read-only candidate, which
//! is the transaction-private writable scratch) from the launch bridge.

use std::path::PathBuf;

use wcore_swarm::worktree::TransactionWorkspace;

/// The transaction-private writable roots for a mutating child's acceptance
/// gates: exactly the retained scratch directory.
///
/// The candidate checkout is bound READ-ONLY by hard containment (the source
/// must not be mutated by a gate), so the scratch directory — created and owned
/// by the transaction — is the sole writable mount. It is derived through the
/// retained scratch authority (never an ambient path) so it names the exact
/// live directory the transaction owns.
pub(crate) fn mutation_writable_roots(workspace: &TransactionWorkspace) -> Vec<PathBuf> {
    vec![workspace.scratch_authority().display_path().to_path_buf()]
}
