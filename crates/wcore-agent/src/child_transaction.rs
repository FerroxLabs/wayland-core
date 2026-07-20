//! Journal-owned authority for delegated-mutation transactions.
//!
//! This module persists evidence only. It does not allocate workspaces, run
//! children or gates, inspect Git, mutate a parent, merge, or roll back.

use std::fmt;

use thiserror::Error;
use wcore_sandbox::SandboxRegistry;
use wcore_swarm::worktree::CandidateSeal;
use wcore_types::child_transaction::{
    CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION, ChildGatePlan, ChildGateReceipt,
    ChildTransactionDisposition, ChildTransactionReceipt, ChildTransactionReducer,
    ChildTransactionReplay, ChildTransactionValidationError,
};
use wcore_types::spawner::{ChildId, ChildWorkspaceMode};

use crate::session_journal::{
    ChildTransactionOpening, ChildTransactionSnapshotBinding, ChildTransactionState,
    CommittedChildTransactionReceipt, JournalEnvelope, JournalError, LandingSubject,
    LandingSuccessor, ReducedSessionState, SessionEvent, SessionJournal,
    child_transaction_opening_token_digest,
};

mod gate_executor;
mod gates;
mod parent;

pub use gate_executor::{
    AuthorizedGateClosure, GateClosureError, GateExecutionSubject, GateStageError,
};
pub use gates::{AcceptanceError, AcceptedCandidate, MutationAttemptGuard};
pub use parent::{
    ParentLandingAuthorization, ParentLandingAuthorizationError, authorize_and_land,
    authorize_and_rollback,
};

use gate_executor::{AuthorizedGateClosureRegistry, GateExecutor, ObservedGateResult};
use gates::{AcceptanceMachine, SealedCandidateRoot};

/// Opaque proof that the live journal writer durably opened one transaction.
///
/// It deliberately has no serde implementation and no public constructor.
/// Snapshot-shaped caller bytes therefore cannot mint execution authority.
#[derive(Clone, PartialEq, Eq)]
pub struct ChildTransactionAuthority {
    session_id: String,
    transaction_id: String,
    opening_seq: u64,
    opening_checksum: String,
    opening_token_digest: String,
    binding_digest: String,
    storage_identity_digest: String,
}

impl fmt::Debug for ChildTransactionAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChildTransactionAuthority")
            .field("transaction_id", &self.transaction_id)
            .field("opening_seq", &self.opening_seq)
            .finish_non_exhaustive()
    }
}

impl ChildTransactionAuthority {
    #[must_use]
    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildTransactionWrite {
    Appended(Box<JournalEnvelope>),
    AlreadyCommitted,
}

/// The single journal-backed mutation/read API for child transactions.
#[derive(Debug, Clone)]
pub struct ChildTransactionStore {
    journal: SessionJournal,
}

impl ChildTransactionStore {
    #[must_use]
    pub fn new(journal: SessionJournal) -> Self {
        Self { journal }
    }

    /// Durably open a transaction from the exact state held by the live
    /// journal writer. The authority is returned only after the opening frame
    /// has been synced.
    pub fn open(
        &self,
        transaction_id: impl Into<String>,
        child_id: ChildId,
        base_revision: impl Into<String>,
        gate_plan: ChildGatePlan,
    ) -> Result<ChildTransactionAuthority, JournalError> {
        let transaction_id = transaction_id.into();
        let base_revision = base_revision.into();
        gate_plan
            .validate()
            .map_err(|error| invalid(error.to_string()))?;

        let expected_transaction_id = transaction_id.clone();
        let expected_child_id = child_id.clone();
        let expected_base_revision = base_revision.clone();
        let expected_gate_plan = gate_plan.clone();
        self.journal
            .append_from_committed_authority(move |state, snapshot_authority| {
                if let Some(existing) = state.child_transactions.get(&expected_transaction_id) {
                    let opening = &existing.opening;
                    if opening.child_id == expected_child_id
                        && opening.base_revision == expected_base_revision
                        && opening.gate_plan == expected_gate_plan
                    {
                        return Ok(None);
                    }
                    return Err(invalid(format!(
                        "child transaction {} opening conflicts with committed authority",
                        expected_transaction_id
                    )));
                }

                let child = durable_child(state, &expected_child_id, &expected_transaction_id)?;
                if child.workspace.mode != ChildWorkspaceMode::Isolated {
                    return Err(invalid(format!(
                        "child transaction {} requires an isolated child workspace",
                        expected_transaction_id
                    )));
                }
                let opening = ChildTransactionOpening {
                    transaction_id: expected_transaction_id.clone(),
                    child_id: expected_child_id.clone(),
                    child_declaration_id: child.declaration_id.clone(),
                    child_revision: child.revision,
                    workspace_id: child.workspace.workspace_id.clone(),
                    base_revision: expected_base_revision.clone(),
                    request_digest: child.request.exact_digest.clone(),
                    policy_digest: child.policy_snapshot.exact_digest.clone(),
                    gate_plan: expected_gate_plan.clone(),
                    snapshot: ChildTransactionSnapshotBinding {
                        session_id: snapshot_authority.session_id.clone(),
                        storage_identity_digest: snapshot_authority.storage_identity_digest.clone(),
                        binding_schema_version: snapshot_authority.binding_schema_version,
                        durable_authority_generation: snapshot_authority
                            .durable_authority_generation
                            .clone(),
                        snapshot_schema_version: snapshot_authority.snapshot_schema_version,
                        cursor: snapshot_authority.cursor,
                        cursor_checksum: snapshot_authority.cursor_checksum.clone(),
                        state_digest: snapshot_authority.state_digest.clone(),
                        binding_digest: snapshot_authority.binding_digest.clone(),
                    },
                };
                Ok(Some(SessionEvent::ChildTransactionOpened { opening }))
            })?;

        let state = self.journal.state()?;
        let transaction = state
            .child_transactions
            .get(&transaction_id)
            .ok_or_else(|| invalid(format!("child transaction {transaction_id} was not opened")))?;
        let authority = authority_from_state(transaction)?;
        self.validate_storage_identity(&authority)?;
        Ok(authority)
    }

    /// Revalidate the retained opening before either workspace allocation or
    /// child launch. Unrelated journal appends do not change this authority.
    pub fn revalidate(&self, authority: &ChildTransactionAuthority) -> Result<(), JournalError> {
        self.validate_storage_identity(authority)?;
        let state = self.journal.state()?;
        validate_retained_authority(&state, authority, true).map(|_| ())
    }

    /// Commit one canonical receipt under the retained opening authority.
    pub fn commit(
        &self,
        authority: &ChildTransactionAuthority,
        receipt: ChildTransactionReceipt,
    ) -> Result<ChildTransactionWrite, JournalError> {
        self.validate_storage_identity(authority)?;
        let receipt_digest = receipt
            .canonical_digest()
            .map_err(|error| invalid(error.to_string()))?;
        let validation_authority = authority.clone();
        let validation_receipt = receipt.clone();
        let validation_digest = receipt_digest.clone();
        self.journal
            .append_conditionally(
                SessionEvent::ChildTransactionReceiptCommitted {
                    transaction_id: authority.transaction_id.clone(),
                    opening_token_digest: authority.opening_token_digest.clone(),
                    receipt_digest,
                    receipt,
                },
                move |state, session_id| {
                    if session_id != validation_authority.session_id {
                        return Err(JournalError::SessionMismatch {
                            expected: validation_authority.session_id.clone(),
                            found: session_id.to_owned(),
                        });
                    }
                    validate_retained_authority(state, &validation_authority, false)?;
                    project_commit(
                        state,
                        &validation_authority,
                        &validation_digest,
                        &validation_receipt,
                    )
                    .map(|projection| matches!(projection, CommitProjection::Applied(_)))
                },
            )
            .map(write_result)
    }

    /// Append one landing-lifecycle authority event under the retained opening
    /// authority. The event is bound to the durable opening token; the reducer
    /// validates every transition against the exact prior lifecycle state, and
    /// the public [`SessionJournal::append`] denylist rejects these events, so
    /// only this authorized path can mint landing / recovery / rollback
    /// authority. Retries are idempotent: an event that reduces to no change is
    /// still fail-closed against reorder, gaps, and post-terminal transitions.
    fn append_landing_event(
        &self,
        authority: &ChildTransactionAuthority,
        event: SessionEvent,
    ) -> Result<ChildTransactionWrite, JournalError> {
        self.validate_storage_identity(authority)?;
        let validation_authority = authority.clone();
        self.journal
            .append_conditionally(event, move |state, session_id| {
                if session_id != validation_authority.session_id {
                    return Err(JournalError::SessionMismatch {
                        expected: validation_authority.session_id.clone(),
                        found: session_id.to_owned(),
                    });
                }
                validate_retained_authority(state, &validation_authority, false)?;
                Ok(true)
            })
            .map(write_result)
    }

    pub(crate) fn append_landing_prepared(
        &self,
        authority: &ChildTransactionAuthority,
        subject: LandingSubject,
    ) -> Result<ChildTransactionWrite, JournalError> {
        self.append_landing_event(
            authority,
            SessionEvent::ChildTransactionLandingPrepared {
                transaction_id: authority.transaction_id.clone(),
                opening_token_digest: authority.opening_token_digest.clone(),
                subject,
            },
        )
    }

    pub(crate) fn append_landing_ref_advanced(
        &self,
        authority: &ChildTransactionAuthority,
        successor: LandingSuccessor,
    ) -> Result<ChildTransactionWrite, JournalError> {
        self.append_landing_event(
            authority,
            SessionEvent::ChildTransactionLandingRefAdvanced {
                transaction_id: authority.transaction_id.clone(),
                opening_token_digest: authority.opening_token_digest.clone(),
                successor,
            },
        )
    }

    pub(crate) fn append_landing_projected(
        &self,
        authority: &ChildTransactionAuthority,
        successor: LandingSuccessor,
    ) -> Result<ChildTransactionWrite, JournalError> {
        self.append_landing_event(
            authority,
            SessionEvent::ChildTransactionLandingProjected {
                transaction_id: authority.transaction_id.clone(),
                opening_token_digest: authority.opening_token_digest.clone(),
                successor,
            },
        )
    }

    pub(crate) fn append_landed(
        &self,
        authority: &ChildTransactionAuthority,
        successor: LandingSuccessor,
    ) -> Result<ChildTransactionWrite, JournalError> {
        self.append_landing_event(
            authority,
            SessionEvent::ChildTransactionLanded {
                transaction_id: authority.transaction_id.clone(),
                opening_token_digest: authority.opening_token_digest.clone(),
                successor,
            },
        )
    }

    pub(crate) fn append_landing_conflict(
        &self,
        authority: &ChildTransactionAuthority,
        detail: String,
    ) -> Result<ChildTransactionWrite, JournalError> {
        self.append_landing_event(
            authority,
            SessionEvent::ChildTransactionLandingConflict {
                transaction_id: authority.transaction_id.clone(),
                opening_token_digest: authority.opening_token_digest.clone(),
                detail,
            },
        )
    }

    pub(crate) fn append_landing_recovery_required(
        &self,
        authority: &ChildTransactionAuthority,
        detail: String,
    ) -> Result<ChildTransactionWrite, JournalError> {
        self.append_landing_event(
            authority,
            SessionEvent::ChildTransactionLandingRecoveryRequired {
                transaction_id: authority.transaction_id.clone(),
                opening_token_digest: authority.opening_token_digest.clone(),
                detail,
            },
        )
    }

    pub(crate) fn append_rollback_prepared(
        &self,
        authority: &ChildTransactionAuthority,
        successor: LandingSuccessor,
    ) -> Result<ChildTransactionWrite, JournalError> {
        self.append_landing_event(
            authority,
            SessionEvent::ChildTransactionRollbackPrepared {
                transaction_id: authority.transaction_id.clone(),
                opening_token_digest: authority.opening_token_digest.clone(),
                successor,
            },
        )
    }

    pub(crate) fn append_rolled_back(
        &self,
        authority: &ChildTransactionAuthority,
        successor: LandingSuccessor,
    ) -> Result<ChildTransactionWrite, JournalError> {
        self.append_landing_event(
            authority,
            SessionEvent::ChildTransactionRolledBack {
                transaction_id: authority.transaction_id.clone(),
                opening_token_digest: authority.opening_token_digest.clone(),
                successor,
            },
        )
    }

    pub fn inspect(
        &self,
        transaction_id: &str,
    ) -> Result<Option<ChildTransactionState>, JournalError> {
        Ok(self
            .journal
            .state()?
            .child_transactions
            .get(transaction_id)
            .cloned())
    }

    pub fn list(&self) -> Result<Vec<ChildTransactionState>, JournalError> {
        Ok(self
            .journal
            .state()?
            .child_transactions
            .into_values()
            .collect())
    }

    fn validate_storage_identity(
        &self,
        authority: &ChildTransactionAuthority,
    ) -> Result<(), JournalError> {
        if self.journal.storage_identity_digest()? != authority.storage_identity_digest {
            return Err(invalid(format!(
                "child transaction {} session storage was rebound",
                authority.transaction_id
            )));
        }
        Ok(())
    }
}

pub(crate) enum CommitProjection {
    Applied(Box<ChildTransactionState>),
    Duplicate,
}

pub(crate) fn project_commit(
    state: &ReducedSessionState,
    authority: &ChildTransactionAuthority,
    receipt_digest: &str,
    receipt: &ChildTransactionReceipt,
) -> Result<CommitProjection, JournalError> {
    let transaction = state
        .child_transactions
        .get(&authority.transaction_id)
        .ok_or_else(|| missing_authority(&authority.transaction_id))?;
    if receipt.transaction_id != authority.transaction_id
        || receipt.child_id != transaction.opening.child_id
        || receipt.child_declaration_id != transaction.opening.child_declaration_id
        || receipt.workspace_id != transaction.opening.workspace_id
        || receipt.base_revision != transaction.opening.base_revision
        || receipt.request_digest != transaction.opening.request_digest
        || receipt.policy_digest != transaction.opening.policy_digest
        || receipt.gate_plan_digest
            != transaction
                .opening
                .gate_plan
                .canonical_digest()
                .map_err(|error| invalid(error.to_string()))?
    {
        return Err(invalid(format!(
            "child transaction {} receipt conflicts with its durable opening",
            authority.transaction_id
        )));
    }
    if let Some(committed) = transaction
        .receipts
        .iter()
        .find(|committed| committed.receipt_digest == receipt_digest)
    {
        return if committed.receipt == *receipt
            && committed.opening_token_digest == authority.opening_token_digest
        {
            Ok(CommitProjection::Duplicate)
        } else {
            Err(invalid(format!(
                "child transaction {} receipt digest conflicts with committed bytes",
                authority.transaction_id
            )))
        };
    }

    let child = durable_child(state, &receipt.child_id, &authority.transaction_id)?;
    let mut reducer = ChildTransactionReducer::default();
    for committed in &transaction.receipts {
        let replay = reducer
            .apply(
                &committed.receipt_digest,
                committed.receipt.clone(),
                &committed.child_snapshot,
                &transaction.opening.gate_plan,
            )
            .map_err(transaction_error)?;
        if replay != ChildTransactionReplay::Applied {
            return Err(invalid(format!(
                "child transaction {} contains duplicate historical evidence",
                authority.transaction_id
            )));
        }
    }
    let replay = reducer
        .apply(
            receipt_digest,
            receipt.clone(),
            child,
            &transaction.opening.gate_plan,
        )
        .map_err(transaction_error)?;
    if replay != ChildTransactionReplay::Applied {
        return Err(invalid(format!(
            "child transaction {} produced an unexpected duplicate projection",
            authority.transaction_id
        )));
    }

    let mut projection = transaction.clone();
    projection.receipts.push(CommittedChildTransactionReceipt {
        opening_token_digest: authority.opening_token_digest.clone(),
        receipt_digest: receipt_digest.to_owned(),
        receipt: receipt.clone(),
        child_snapshot: child.clone(),
    });
    Ok(CommitProjection::Applied(Box::new(projection)))
}

fn validate_retained_authority<'a>(
    state: &'a ReducedSessionState,
    authority: &ChildTransactionAuthority,
    require_opening_revision: bool,
) -> Result<&'a ChildTransactionState, JournalError> {
    if state.session_id.as_deref() != Some(authority.session_id.as_str()) {
        return Err(JournalError::SessionMismatch {
            expected: authority.session_id.clone(),
            found: state.session_id.clone().unwrap_or_default(),
        });
    }
    let transaction = state
        .child_transactions
        .get(&authority.transaction_id)
        .ok_or_else(|| missing_authority(&authority.transaction_id))?;
    let recomputed = child_transaction_opening_token_digest(
        &transaction.opening,
        transaction.opening_seq,
        &transaction.opening_checksum,
    )?;
    if transaction.opening_seq != authority.opening_seq
        || transaction.opening_checksum != authority.opening_checksum
        || transaction.opening_token_digest != authority.opening_token_digest
        || transaction.opening.snapshot.binding_digest != authority.binding_digest
        || transaction.opening.snapshot.storage_identity_digest != authority.storage_identity_digest
        || recomputed != authority.opening_token_digest
    {
        return Err(invalid(format!(
            "child transaction {} retained opening authority changed",
            authority.transaction_id
        )));
    }
    let child = durable_child(
        state,
        &transaction.opening.child_id,
        &authority.transaction_id,
    )?;
    if child.declaration_id != transaction.opening.child_declaration_id
        || child.workspace.workspace_id != transaction.opening.workspace_id
        || child.workspace.mode != ChildWorkspaceMode::Isolated
        || child.request.exact_digest != transaction.opening.request_digest
        || child.policy_snapshot.exact_digest != transaction.opening.policy_digest
        || (require_opening_revision && child.revision != transaction.opening.child_revision)
    {
        return Err(invalid(format!(
            "child transaction {} child authority changed after opening",
            authority.transaction_id
        )));
    }
    Ok(transaction)
}

pub(crate) fn authority_from_state(
    transaction: &ChildTransactionState,
) -> Result<ChildTransactionAuthority, JournalError> {
    let recomputed = child_transaction_opening_token_digest(
        &transaction.opening,
        transaction.opening_seq,
        &transaction.opening_checksum,
    )?;
    if recomputed != transaction.opening_token_digest {
        return Err(invalid(format!(
            "child transaction {} has invalid opening authority",
            transaction.opening.transaction_id
        )));
    }
    Ok(ChildTransactionAuthority {
        session_id: transaction.opening.snapshot.session_id.clone(),
        transaction_id: transaction.opening.transaction_id.clone(),
        opening_seq: transaction.opening_seq,
        opening_checksum: transaction.opening_checksum.clone(),
        opening_token_digest: transaction.opening_token_digest.clone(),
        binding_digest: transaction.opening.snapshot.binding_digest.clone(),
        storage_identity_digest: transaction.opening.snapshot.storage_identity_digest.clone(),
    })
}

fn durable_child<'a>(
    state: &'a ReducedSessionState,
    child_id: &ChildId,
    transaction_id: &str,
) -> Result<&'a wcore_types::spawner::DurableChildRecord, JournalError> {
    state
        .children
        .get(child_id.as_str())
        .and_then(|child| child.durable.as_ref())
        .ok_or_else(|| {
            invalid(format!(
                "child transaction {transaction_id} references unknown durable child {child_id}"
            ))
        })
}

fn write_result(envelope: Option<JournalEnvelope>) -> ChildTransactionWrite {
    envelope.map_or(ChildTransactionWrite::AlreadyCommitted, |envelope| {
        ChildTransactionWrite::Appended(Box::new(envelope))
    })
}

fn missing_authority(transaction_id: &str) -> JournalError {
    invalid(format!(
        "child transaction {transaction_id} has no retained opening authority"
    ))
}

fn transaction_error(error: impl fmt::Display) -> JournalError {
    invalid(error.to_string())
}

fn invalid(message: impl Into<String>) -> JournalError {
    JournalError::InvalidTransition(message.into())
}

/// A fail-closed refusal anywhere in the gate-execution → durable-receipt →
/// acceptance pipeline. Every variant keeps the candidate non-landing and
/// durably diagnosable; none can mint or retain acceptance.
#[derive(Debug, Error)]
pub enum MutationAcceptanceError {
    #[error("child transaction is not durably open")]
    MissingTransaction,
    #[error("execution subject does not bind the orchestrator-owned gate plan")]
    SubjectPlanMismatch,
    #[error("committed acceptance receipt is not durable")]
    ReceiptNotDurable,
    #[error("reopened acceptance receipt does not match the authored bytes")]
    ReceiptMismatch,
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("gate closure authority refused: {0}")]
    Closure(#[from] GateClosureError),
    #[error("gate execution refused: {0}")]
    GateStage(#[from] GateStageError),
    #[error("acceptance refused: {0}")]
    Acceptance(#[from] AcceptanceError),
    #[error("receipt validation failed: {0}")]
    Validation(#[from] ChildTransactionValidationError),
}

/// Build the authoritative acceptance receipt from the durable opening plus the
/// module-private observed gate results. The receipt carries the path-free gate
/// receipts; command text, host paths, captured output, and environment values
/// never enter it. The disposition is `Active` — this packet makes no
/// merge-ready / landing claim.
fn build_acceptance_receipt(
    opening: &ChildTransactionOpening,
    subject: &GateExecutionSubject,
    observed: &[ObservedGateResult],
    now_unix_ms: u64,
) -> ChildTransactionReceipt {
    let gates: Vec<ChildGateReceipt> = observed
        .iter()
        .map(ObservedGateResult::to_gate_receipt)
        .collect();
    ChildTransactionReceipt {
        schema_version: CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION,
        transaction_id: opening.transaction_id.clone(),
        receipt_id: format!("{}-accept-0", opening.transaction_id),
        receipt_revision: 0,
        previous_receipt_digest: None,
        child_id: opening.child_id.clone(),
        child_declaration_id: opening.child_declaration_id.clone(),
        child_revision: opening.child_revision,
        workspace_id: opening.workspace_id.clone(),
        base_revision: opening.base_revision.clone(),
        candidate_revision: Some(subject.candidate_revision.clone()),
        request_digest: opening.request_digest.clone(),
        policy_digest: opening.policy_digest.clone(),
        gate_plan_digest: subject.gate_plan_digest.clone(),
        diff_digest: Some(subject.diff_digest.clone()),
        gates,
        disposition: ChildTransactionDisposition::Active,
        created_at_unix_ms: now_unix_ms,
        updated_at_unix_ms: now_unix_ms,
    }
}

/// The authoritative receipt closure: build the exact canonical bytes,
/// conditionally append them under transaction authority, reopen the durable
/// state, reduce, and match the reopened receipt against the authored bytes and
/// its digest. Any divergence fails closed — acceptance never rests on
/// in-memory bytes that were not durably reduced.
fn commit_and_confirm_acceptance_receipt(
    store: &ChildTransactionStore,
    authority: &ChildTransactionAuthority,
    receipt: ChildTransactionReceipt,
) -> Result<String, MutationAcceptanceError> {
    let expected = receipt.canonical_digest()?;
    // Conditionally append under the retained opening authority (idempotent).
    match store.commit(authority, receipt.clone())? {
        ChildTransactionWrite::Appended(_) | ChildTransactionWrite::AlreadyCommitted => {}
    }
    // Reopen from durable storage and reduce; match the reopened receipt.
    let reduced = store
        .inspect(authority.transaction_id())?
        .ok_or(MutationAcceptanceError::MissingTransaction)?;
    // The durably reduced latest receipt must be exactly the one we authored.
    if reduced.latest_receipt_digest() != Some(expected.as_str()) {
        return Err(MutationAcceptanceError::ReceiptNotDurable);
    }
    let committed = reduced
        .receipts
        .iter()
        .find(|committed| committed.receipt_digest == expected)
        .ok_or(MutationAcceptanceError::ReceiptNotDurable)?;
    if committed.receipt != receipt {
        return Err(MutationAcceptanceError::ReceiptMismatch);
    }
    Ok(expected)
}

/// Drive one delegated mutation from parent-observed gate execution to an
/// [`AcceptedCandidate`], integrating the qualified 06A candidate seal (via the
/// guard's retained checkout) and the 06B hard containment (via the sandbox
/// registry) with the accepted 20-04 durable-transaction authority.
///
/// The candidate cwd resolves exclusively from the live seal; observed results
/// come only from consumed live containment spawns; only exact, in-order,
/// passed, same-subject evidence is accepted; and acceptance exists only after
/// the authoritative receipt closure confirms the durable receipt. The returned
/// [`AcceptedCandidate`] owns the original still-armed guard and the seal.
#[allow(clippy::too_many_arguments)]
pub async fn run_gate_acceptance(
    sandbox: &SandboxRegistry,
    store: &ChildTransactionStore,
    authority: &ChildTransactionAuthority,
    subject: &GateExecutionSubject,
    closures: Vec<AuthorizedGateClosure>,
    guard: MutationAttemptGuard,
    seal: CandidateSeal,
    now_unix_ms: u64,
) -> Result<AcceptedCandidate, MutationAcceptanceError> {
    // Reopen the durable opening; the orchestrator-owned gate plan lives there.
    let state = store
        .inspect(authority.transaction_id())?
        .ok_or(MutationAcceptanceError::MissingTransaction)?;
    let opening = state.opening.clone();
    let plan = opening.gate_plan.clone();

    // The execution subject MUST bind the orchestrator-owned gate plan.
    if subject.gate_plan_digest != plan.canonical_digest()? {
        return Err(MutationAcceptanceError::SubjectPlanMismatch);
    }

    // Authorize the parent-owned closures and pin each into the registry.
    let mut registry = AuthorizedGateClosureRegistry::new();
    for closure in closures {
        registry.authorize(closure)?;
    }

    // Execute every gate against the live sealed candidate, in declared order.
    let candidate = SealedCandidateRoot::new(&guard);
    let executor = GateExecutor::new(&registry, sandbox);
    let observed = executor.execute_plan(&plan, subject, &candidate).await?;

    // Only exact, in-order, passed, same-subject evidence may be accepted.
    AcceptanceMachine::validate_observed(&plan, subject, &observed)?;

    // Build, append, reopen, reduce, and match the authoritative durable receipt.
    let receipt = build_acceptance_receipt(&opening, subject, &observed, now_unix_ms);
    let digest = commit_and_confirm_acceptance_receipt(store, authority, receipt)?;

    Ok(AcceptanceMachine::accept(
        guard,
        seal,
        authority.transaction_id().to_owned(),
        digest,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::durable_child::DurableChildStore;
    use wcore_types::child_transaction::ChildGateRequirement;
    use wcore_types::spawner::{
        ChildDeliveryState, ChildDesiredState, ChildOrigin, ChildParent, ChildPolicySnapshot,
        ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace,
        DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus,
    };

    fn digest(character: char) -> String {
        std::iter::repeat_n(character, 64).collect()
    }

    fn revision(character: char) -> String {
        std::iter::repeat_n(character, 40).collect()
    }

    fn gate_plan() -> ChildGatePlan {
        ChildGatePlan {
            required_gates: vec![ChildGateRequirement {
                gate_id: "cargo-test".into(),
                gate_closure_digest: digest('c'),
            }],
        }
    }

    fn child_record() -> DurableChildRecord {
        DurableChildRecord {
            schema_version: DURABLE_CHILD_SCHEMA_VERSION,
            declaration_id: "declare-child-1".into(),
            child_id: ChildId::new("child-1").unwrap(),
            parent: ChildParent {
                session_id: "session-1".into(),
                turn_id: None,
                parent_child_id: None,
                workflow_run_id: None,
                graph_node_id: None,
                parent_call_id: None,
            },
            origin: ChildOrigin::Delegate,
            request: ChildRequestEvidence::redacted(digest('a')),
            policy_snapshot: ChildPolicySnapshot {
                contract_version: "effective-execution-policy/v1".into(),
                exact_digest: digest('b'),
                posture: "smart".into(),
                approvals: "on_request".into(),
                sandbox: "required".into(),
                source: "session-effective-policy".into(),
                managed_floor_active: true,
                dangerous_activation_id_digest: None,
            },
            provider: Some("test".into()),
            model: Some("test-model".into()),
            workspace: ChildWorkspace {
                mode: ChildWorkspaceMode::Isolated,
                workspace_id: "workspace-child-1".into(),
            },
            status: DurableChildStatus::Prepared,
            desired_state: ChildDesiredState::Run,
            recovery: ChildRecoveryState::Clean,
            revision: 0,
            timestamps: ChildTimestamps {
                created_at_unix_ms: 10,
                updated_at_unix_ms: 10,
                queued_at_unix_ms: None,
                started_at_unix_ms: None,
                terminal_at_unix_ms: None,
            },
            result: None,
            delivery_target: None,
            delivery_state: ChildDeliveryState::NotRequired,
            attempt: 1,
            retry_of: None,
            applied_events: BTreeMap::new(),
        }
    }

    /// A gate-less genesis acceptance receipt for the child at revision 0. The
    /// gate-less `Active` disposition keeps the receipt structurally valid while
    /// this test focuses on the append/reopen/reduce corruption boundary.
    fn genesis_receipt() -> ChildTransactionReceipt {
        ChildTransactionReceipt {
            schema_version: CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION,
            transaction_id: "transaction-1".into(),
            receipt_id: "transaction-1-accept-0".into(),
            receipt_revision: 0,
            previous_receipt_digest: None,
            child_id: ChildId::new("child-1").unwrap(),
            child_declaration_id: "declare-child-1".into(),
            child_revision: 0,
            workspace_id: "workspace-child-1".into(),
            base_revision: revision('1'),
            candidate_revision: None,
            request_digest: digest('a'),
            policy_digest: digest('b'),
            gate_plan_digest: gate_plan().canonical_digest().unwrap(),
            diff_digest: None,
            gates: Vec::new(),
            disposition: ChildTransactionDisposition::Active,
            created_at_unix_ms: 100,
            updated_at_unix_ms: 100,
        }
    }

    fn fixture() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        SessionJournal,
        DurableChildStore,
        ChildTransactionStore,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session.journal");
        let journal = SessionJournal::open(&path, "session-1").unwrap();
        let children = DurableChildStore::new(journal.clone());
        children.declare(child_record()).unwrap();
        let transactions = ChildTransactionStore::new(journal.clone());
        (temp, path, journal, children, transactions)
    }

    /// Proves the authoritative receipt closure: a genuine build → conditional
    /// append → reopen → reduce → match round-trip succeeds and is idempotent,
    /// AND a corrupted durable journal is rejected on reopen/reduce — so
    /// acceptance can never rest on tampered durable evidence.
    #[test]
    fn rejects_append_reopen_reduce_corruption() {
        let (temp, path, journal, children, store) = fixture();
        let authority = store
            .open(
                "transaction-1",
                ChildId::new("child-1").unwrap(),
                revision('1'),
                gate_plan(),
            )
            .unwrap();
        let receipt = genesis_receipt();
        let expected = receipt.canonical_digest().unwrap();

        // The closure builds, appends, reopens, reduces, and matches.
        let digest =
            commit_and_confirm_acceptance_receipt(&store, &authority, receipt.clone()).unwrap();
        assert_eq!(digest, expected);
        // Idempotent: an exact retry re-confirms the same durable receipt.
        let retry =
            commit_and_confirm_acceptance_receipt(&store, &authority, receipt.clone()).unwrap();
        assert_eq!(retry, expected);

        // A clean reopen replays the durable receipt exactly once. Every live
        // journal handle (store, durable-child store, and the writer) must be
        // dropped first so the exclusive writer lease is released.
        drop(store);
        drop(children);
        drop(journal);
        let reopened = SessionJournal::open(&path, "session-1").unwrap();
        let projected = ChildTransactionStore::new(reopened)
            .inspect("transaction-1")
            .unwrap()
            .unwrap();
        assert_eq!(projected.receipts.len(), 1);
        assert_eq!(projected.receipts[0].receipt_digest, expected);

        // Corrupt the durable journal's final (receipt) frame: reopen + reduce
        // must fail closed rather than surface a tampered receipt.
        let original = std::fs::read(&path).unwrap();
        let mut corrupt = original.clone();
        *corrupt.last_mut().unwrap() ^= 0xff;
        let corrupt_path = temp.path().join("corrupt.journal");
        std::fs::write(&corrupt_path, corrupt).unwrap();
        assert!(SessionJournal::recovered_state(&corrupt_path).is_err());
        drop(temp);
    }
}
