//! The production landing orchestrator: compose the SELECTED winner's terminal
//! delegated-mutation chain fail-closed.
//!
//! This is the single production entry point that turns the climb's opaque
//! winner identity into a parent-owned landing. It is a pure composition over
//! the already-audited primitives — it allocates nothing, runs no git itself,
//! and mints no authority the durable journal would not accept. The whole chain
//! is:
//!
//! 1. Consume the winner's retained landing authority
//!    ([`CandidateCheckout::into_landing_authority`]) — the still-armed
//!    [`MutationAttemptGuard`](crate::child_transaction::MutationAttemptGuard)
//!    plus its freshly minted
//!    [`CandidateSeal`](wcore_swarm::worktree::CandidateSeal). A winner that
//!    surrenders `None` (a released, drifted, or substituted checkout) is a HARD
//!    fail-closed refusal ([`WinnerLandingError::NoLandingAuthority`]) — never a
//!    silent skip that reports success while landing nothing.
//! 2. [`open`](ChildTransactionLifecycle::open) the durable transaction (bound +
//!    revalidated against the live journal).
//! 3. [`accept_selected_winner`](ChildTransactionLifecycle::accept_selected_winner)
//!    — drive the winner through the parent-owned 06C gate execution to a durable
//!    [`AcceptedCandidate`](crate::child_transaction::AcceptedCandidate).
//! 4. [`land`](ChildTransactionLifecycle::land) into the Wayland-owned
//!    integration checkout via the 20-07 quarantined-import + `git update-ref`
//!    CAS, advancing `target_ref`.
//!
//! **Surface-for-accept:** the caller passes the Wayland-owned integration
//! checkout — a standalone clone at the exact user tip, on its own branch, never
//! the user's working tree — and a `target_ref` that is the clone's own
//! `refs/heads/<branch>` (the 20-07 primitive requires a fully-qualified
//! `refs/heads/` ref whose current tip is the base and whose symbolic HEAD names
//! it; `refs/wayland/landing/<slug>` is the primitive's INTERNAL quarantine ref,
//! not a caller target). This function advances that branch inside the CLONE
//! through the parent CAS; the user's real repository — a different checkout — is
//! never touched. Whether/when to fast-forward the user's branch onto the clone's
//! landed commit is a later, Desktop-mediated decision.
//!
//! Only the terminal composition lives here. Deriving the request bundle from a
//! live climb (the winner's transaction identity, the gate plan/subject/closures,
//! the integration checkout + target ref) is the caller's job (the Anvil forge
//! wiring); this module keeps that derivation out so the fail-closed chain stays
//! small, single-responsibility, and independently provable.

use std::path::PathBuf;

use wcore_types::child_transaction::ChildGatePlan;
use wcore_types::spawner::ChildId;

use super::engine::CandidateCheckout;
use crate::child_transaction::{
    AuthorizedGateClosure, ChildTransactionLifecycle, GateExecutionSubject,
    MutationAcceptanceError, ParentLandingAuthorization, ParentLandingAuthorizationError,
};
use crate::session_journal::JournalError;

/// Everything the terminal chain needs about the SELECTED winner *besides* its
/// opaque checkout identity, gathered from the live climb by the caller.
///
/// Deliberately owns its inputs (`String`/`PathBuf`/`Vec`): the request is
/// consumed exactly once by [`land_selected_winner`], mirroring the single-use
/// winner it lands. The bundle is the seam the Anvil forge wiring populates —
/// keeping [`land_selected_winner`]'s signature stable while identity threading
/// lands separately.
#[derive(Debug)]
pub struct WinnerLandingRequest {
    /// The winner's durable transaction id (the identity the journal opened).
    pub transaction_id: String,
    /// The winner child's id.
    pub child_id: ChildId,
    /// The base commit the winner was forked from — the exact tip `target_ref`
    /// must still name for the CAS to succeed.
    pub base_revision: String,
    /// The parent-sealed gate plan bound to this transaction (06C).
    pub gate_plan: ChildGatePlan,
    /// The sealed gate-execution subject for the winner (06C).
    pub subject: GateExecutionSubject,
    /// The authorized, SHA-256-sealed gate closures to run before acceptance.
    pub closures: Vec<AuthorizedGateClosure>,
    /// The Wayland-owned integration checkout to land into (a standalone clone at
    /// the user tip — NEVER the user's working tree).
    pub integration_checkout: PathBuf,
    /// The fully-qualified `refs/heads/<branch>` to advance via the parent CAS —
    /// the integration clone's own branch (its symbolic HEAD, tip == the base).
    /// Surface-for-accept: the landed commit lands on this branch INSIDE the
    /// Wayland-owned clone; the user's real repository is untouched.
    pub target_ref: String,
    /// The acceptance timestamp (unix ms), passed in so the composition stays
    /// deterministic and clock-injection-free.
    pub now_unix_ms: u64,
}

/// A fail-closed refusal from the winner landing orchestrator.
#[derive(Debug, thiserror::Error)]
pub enum WinnerLandingError {
    /// The selected winner surrendered no landing authority. A real winner
    /// returning `None` from [`CandidateCheckout::into_landing_authority`] means
    /// its retained checkout was released, drifted, or substituted — a hard
    /// refusal, never a silent no-op that reports success.
    #[error(
        "selected winner surrendered no landing authority (released, drifted, or substituted checkout)"
    )]
    NoLandingAuthority,
    /// Opening the durable transaction failed.
    #[error("failed to open child transaction for landing: {0}")]
    Open(#[from] JournalError),
    /// Driving the winner to a durable accepted candidate failed.
    #[error("winner acceptance failed: {0}")]
    Accept(#[from] MutationAcceptanceError),
    /// The parent landing authorization / CAS refused.
    #[error("parent landing authorization failed: {0}")]
    Land(#[from] ParentLandingAuthorizationError),
}

/// Compose the terminal delegated-mutation chain for the single selected winner,
/// fail-closed at every hop.
///
/// Consumes the boxed winner BY VALUE, so only the one candidate moved out of
/// [`ClimbOutcome::winner`](super::engine::ClimbOutcome) can ever reach a
/// landing; every loser was already dropped (RAII-terminalized) before this
/// call. Returns the raw [`ParentLandingAuthorization`] outcome for the caller to
/// map into a user-facing report — mapping is deliberately not this function's
/// job.
pub async fn land_selected_winner(
    winner: Box<dyn CandidateCheckout>,
    lifecycle: &ChildTransactionLifecycle,
    request: WinnerLandingRequest,
) -> Result<ParentLandingAuthorization, WinnerLandingError> {
    // 1. Consume the winner's retained authority. `None` from a real winner is a
    //    hard fail-closed refusal — we never proceed to open/accept/land without
    //    a live guard + seal.
    let (guard, seal) = winner
        .into_landing_authority()
        .ok_or(WinnerLandingError::NoLandingAuthority)?;

    // 2. Open + revalidate the durable transaction.
    let authority = lifecycle.open(
        request.transaction_id,
        request.child_id,
        request.base_revision,
        request.gate_plan,
    )?;

    // 3. Drive the winner through parent-owned gate execution to a durable
    //    AcceptedCandidate. The guard + seal are moved in here and nowhere else.
    //    `Box::pin`: the acceptance future re-runs the pinned gate under hard
    //    containment (sandbox spawn + manifest build) — a very large async state
    //    machine. Heaping it keeps this (and its callers') future off the stack,
    //    which otherwise overflows in debug builds. Behavior-free.
    let accepted = Box::pin(lifecycle.accept_selected_winner(
        &authority,
        &request.subject,
        request.closures,
        guard,
        seal,
        request.now_unix_ms,
    ))
    .await?;

    // 4. Land into the Wayland-owned integration checkout via the parent CAS,
    //    advancing only `target_ref` (surface-for-accept: the user tree is never
    //    touched). Landing is itself fail-closed on the accepted prerequisite.
    //    `Box::pin` for the same future-size reason as the acceptance step.
    let outcome = Box::pin(lifecycle.land(
        &authority,
        &accepted,
        &request.integration_checkout,
        &request.target_ref,
    ))
    .await?;

    Ok(outcome)
}
