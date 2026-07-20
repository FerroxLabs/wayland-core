//! Durable-transaction authorization for parent landing and rollback.
//!
//! This module is the upper half of the delegated-mutation landing: it binds a
//! durably gate-accepted transaction (the 06C [`AcceptedCandidate`]) to the
//! pure, parent-owned compare-and-swap primitive in `wcore-swarm`
//! ([`WorktreeManager::land_candidate`]). It:
//!
//! * refuses to land unless the transaction's latest durable receipt is exactly
//!   the acceptance receipt the [`AcceptedCandidate`] carries (the accepted
//!   prerequisite);
//! * conditionally appends `LandingPrepared` — bound to the exact expected
//!   parent/candidate/preimage identity the primitive computed under its lock —
//!   *before* invoking the CAS primitive;
//! * appends each returned ref-advanced / projection outcome and `Landed` only
//!   after coherent projection is verified, and maps every other typed outcome
//!   to an identity-matched conditional append;
//! * authorizes rollback only from a durably `Landed` state and records
//!   recovery when the primitive refuses (foreign drift).
//!
//! It supplies the durable preimage to the lower layer and consumes its typed
//! outcomes; it never resolves conflicts and never mints authority the reducer
//! would not accept — the append denylist plus the reducer's transition matrix
//! keep the lower-layer primitive, the child, and the model from minting a
//! landing, recovery, or rollback.

use std::path::Path;

use thiserror::Error;
use wcore_swarm::worktree::{
    LandingOutcome, ParentPreimage, ParentSuccessor, RollbackHandle, WorktreeManager,
};

use crate::session_journal::{
    JournalError, LandingSubject, LandingSuccessor, state_payload_digest,
};

use super::{AcceptedCandidate, ChildTransactionAuthority, ChildTransactionStore};

/// The outcome of an authorized landing or rollback, mirrored from the durable
/// journal state the primitive's typed outcome produced.
#[derive(Debug)]
pub enum ParentLandingAuthorization {
    /// The candidate landed coherently; the successor and a rollback handle are
    /// returned. The durable journal records `Landed`.
    Landed {
        successor: LandingSuccessor,
        rollback: Box<RollbackHandle>,
    },
    /// The landing was exactly reversed; the journal records `RolledBack`.
    RolledBack { successor: LandingSuccessor },
    /// The parent conflicts with the candidate; no mutation. The journal records
    /// `Conflict`.
    Conflict { detail: String },
    /// The ref advanced but projection/verification is incomplete; the journal
    /// records the reached lifecycle state for the recovery matrix.
    Incomplete { detail: String },
    /// An inconsistency requires explicit resolution; the journal records
    /// `RecoveryRequired`.
    RecoveryRequired { detail: String },
}

/// A fail-closed refusal from the landing authorization layer.
#[derive(Debug, Error)]
pub enum ParentLandingAuthorizationError {
    #[error("child transaction is not durably open")]
    MissingTransaction,
    #[error("accepted candidate does not bind this transaction")]
    CandidateMismatch,
    #[error("child transaction has no durable acceptance receipt for the candidate")]
    NotAccepted,
    #[error("parent primitive returned no preimage to journal")]
    NoPreimage,
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("parent landing primitive failed: {0}")]
    Primitive(String),
}

type AuthResult = Result<ParentLandingAuthorization, ParentLandingAuthorizationError>;

fn to_successor(successor: &ParentSuccessor) -> LandingSuccessor {
    LandingSuccessor {
        landed_commit: successor.landed_commit.clone(),
        landed_tree: successor.landed_tree.clone(),
        quarantine_ref: successor.quarantine_ref.clone(),
    }
}

/// Build the durable landing subject from the parent preimage the primitive
/// captured plus the accepted receipt digest that authorized the integration.
fn landing_subject(
    preimage: &ParentPreimage,
    base_commit: &str,
    accepted_receipt_digest: &str,
) -> Result<LandingSubject, JournalError> {
    let preimage_digest = state_payload_digest(&serde_json::json!({
        "domain": "wayland-core:parent-landing-preimage:v1",
        "common_git_dir": preimage.common_git_dir.display().to_string(),
        "checkout_root": preimage.checkout_root.display().to_string(),
        "target_ref": preimage.target_ref,
        "symbolic_head": preimage.symbolic_head,
        "expected_commit": preimage.expected_commit,
        "expected_tree": preimage.expected_tree,
        "index_tree": preimage.index_tree,
        "worktree_digest": preimage.worktree_digest,
        "lock_identity": preimage.lock_identity,
    }))?;
    Ok(LandingSubject {
        accepted_receipt_digest: accepted_receipt_digest.to_owned(),
        target_ref: preimage.target_ref.clone(),
        base_commit: base_commit.to_owned(),
        expected_commit: preimage.expected_commit.clone(),
        expected_tree: preimage.expected_tree.clone(),
        symbolic_head: preimage.symbolic_head.clone(),
        index_tree: preimage.index_tree.clone(),
        worktree_digest: preimage.worktree_digest.clone(),
        lock_identity: preimage.lock_identity.clone(),
        preimage_digest,
    })
}

/// Authorize and perform one delegated-mutation landing.
///
/// The candidate's base (the commit it was forked from) is the exact parent tip
/// the target ref must currently name; a drifted parent stops before any
/// mutation. On a coherent landing the journal records
/// `LandingPrepared → RefAdvanced → Projected → Landed`; every other primitive
/// outcome is mapped to its identity-matched durable append.
pub async fn authorize_and_land(
    store: &ChildTransactionStore,
    authority: &ChildTransactionAuthority,
    accepted: &AcceptedCandidate,
    manager: &WorktreeManager,
    integration_checkout: &Path,
    target_ref: &str,
) -> AuthResult {
    let transaction = store
        .inspect(authority.transaction_id())?
        .ok_or(ParentLandingAuthorizationError::MissingTransaction)?;
    if accepted.transaction_id() != authority.transaction_id() {
        return Err(ParentLandingAuthorizationError::CandidateMismatch);
    }
    let accepted_receipt = accepted.accepted_receipt_digest().to_owned();
    if transaction.latest_receipt_digest() != Some(accepted_receipt.as_str()) {
        return Err(ParentLandingAuthorizationError::NotAccepted);
    }

    let workspace = accepted.guard().workspace();
    let seal = accepted.seal();
    let expected_commit = workspace.base_commit.clone();

    // Phase A: bind the parent preimage without mutating (dry preparation).
    let prepared = manager
        .land_candidate(
            workspace,
            seal,
            integration_checkout,
            target_ref,
            &expected_commit,
            false,
        )
        .await
        .map_err(|error| ParentLandingAuthorizationError::Primitive(error.to_string()))?;

    let (preimage, conflict_detail) = match prepared {
        LandingOutcome::Prepared { preimage } => (preimage, None),
        LandingOutcome::Conflict {
            preimage: Some(preimage),
        } => (
            preimage,
            Some("parent conflicts with the candidate base".to_owned()),
        ),
        LandingOutcome::RecoveryRequired {
            preimage: Some(preimage),
            detail,
        } => (preimage, Some(detail)),
        _ => return Err(ParentLandingAuthorizationError::NoPreimage),
    };

    // Conditionally append LandingPrepared BEFORE invoking the CAS primitive.
    let subject = landing_subject(&preimage, &expected_commit, &accepted_receipt)?;
    store.append_landing_prepared(authority, subject)?;

    if let Some(detail) = conflict_detail {
        store.append_landing_conflict(authority, detail.clone())?;
        return Ok(ParentLandingAuthorization::Conflict { detail });
    }

    // Phase C: the exact compare-and-swap.
    let outcome = manager
        .land_candidate(
            workspace,
            seal,
            integration_checkout,
            target_ref,
            &expected_commit,
            true,
        )
        .await
        .map_err(|error| ParentLandingAuthorizationError::Primitive(error.to_string()))?;

    match outcome {
        LandingOutcome::Completed {
            successor,
            rollback,
            ..
        } => {
            let successor = to_successor(&successor);
            store.append_landing_ref_advanced(authority, successor.clone())?;
            store.append_landing_projected(authority, successor.clone())?;
            store.append_landed(authority, successor.clone())?;
            Ok(ParentLandingAuthorization::Landed {
                successor,
                rollback,
            })
        }
        LandingOutcome::RefAdvanced { successor, .. } => {
            let successor = to_successor(&successor);
            store.append_landing_ref_advanced(authority, successor)?;
            Ok(ParentLandingAuthorization::Incomplete {
                detail: "target ref advanced; projection incomplete".to_owned(),
            })
        }
        LandingOutcome::Projected { successor, .. } => {
            let successor = to_successor(&successor);
            store.append_landing_ref_advanced(authority, successor.clone())?;
            store.append_landing_projected(authority, successor)?;
            Ok(ParentLandingAuthorization::Incomplete {
                detail: "successor projected; verification incomplete".to_owned(),
            })
        }
        LandingOutcome::Conflict { .. } => {
            let detail = "parent drifted at compare-and-swap".to_owned();
            store.append_landing_conflict(authority, detail.clone())?;
            Ok(ParentLandingAuthorization::Conflict { detail })
        }
        LandingOutcome::RecoveryRequired { detail, .. } => {
            store.append_landing_recovery_required(authority, detail.clone())?;
            Ok(ParentLandingAuthorization::RecoveryRequired { detail })
        }
        LandingOutcome::Prepared { .. } => {
            let detail = "primitive returned Prepared under a commit request".to_owned();
            store.append_landing_recovery_required(authority, detail.clone())?;
            Ok(ParentLandingAuthorization::RecoveryRequired { detail })
        }
    }
}

/// Authorize and perform an exact reverse compare-and-swap rollback.
///
/// The reducer requires the transaction to be durably `Landed` with the handle's
/// successor before `RollbackPrepared` is accepted; the primitive then reverses
/// only while ref/HEAD/index/worktree still equal the successor, and foreign
/// drift is recorded as `RecoveryRequired` rather than clobbering later work.
pub async fn authorize_and_rollback(
    store: &ChildTransactionStore,
    authority: &ChildTransactionAuthority,
    manager: &WorktreeManager,
    integration_checkout: &Path,
    handle: &RollbackHandle,
) -> AuthResult {
    store
        .inspect(authority.transaction_id())?
        .ok_or(ParentLandingAuthorizationError::MissingTransaction)?;
    let successor = to_successor(&handle.successor);
    store.append_rollback_prepared(authority, successor.clone())?;

    match manager.rollback_landing(integration_checkout, handle).await {
        Ok(()) => {
            store.append_rolled_back(authority, successor.clone())?;
            Ok(ParentLandingAuthorization::RolledBack { successor })
        }
        Err(error) => {
            let detail = error.to_string();
            store.append_landing_recovery_required(authority, detail.clone())?;
            Ok(ParentLandingAuthorization::RecoveryRequired { detail })
        }
    }
}
