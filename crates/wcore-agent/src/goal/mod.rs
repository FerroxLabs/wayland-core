//! The durable Goal kernel (F22-02).
//!
//! The Goal VOCABULARY — identity, strategy, the one canonical terminal
//! taxonomy, loop policy, wait kind, and the authority resolver — lives in
//! `wcore_types::goal`, in the bottom crate, because `wcore-protocol` must name
//! those words on the wire without depending on this crate. It is NOT restated
//! here: a second terminal taxonomy beside the canonical one is exactly the
//! parallel lifecycle Phase 22 exists to remove.
//!
//! What lives here is the part that needs the journal: the durable record shape
//! and the state machine that writes it.
//!
//! | Module | Owns |
//! |---|---|
//! | [`record`] | the replayable form of the authority envelope, and its reconstruct-or-refuse rule |
//! | [`kernel`] | the sole writer of Goal transitions over the existing F12 chain |
//! | [`ledger`] | the durable Fleet task ledger above the existing executors, and its claim fence |
//!
//! The reduced projection (`GoalState`, `GoalLifecycle`, `GoalTaskState`) lives
//! beside the other reduced state in `session_journal::model`, for the same
//! reason `ChildTransactionState` does: it is part of what the existing reducer
//! folds, not a second store.

mod kernel;
mod ledger;
mod record;

pub use kernel::{GoalKernel, GoalRecovery};
pub use ledger::{ClaimOutcome, GoalLedger, TaskAuthority};
pub use record::{AuthorityUnreconstructable, GoalAuthorityRecord};

// Re-exported so callers work in one vocabulary: the durable projection and the
// kernel that writes it are named from the same module.
pub use crate::session_journal::{
    GoalLifecycle, GoalState, GoalTaskAttempt, GoalTaskAttemptStatus, GoalTaskCompletion,
    GoalTaskState,
};
