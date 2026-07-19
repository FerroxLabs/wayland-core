//! Journal-owned authority for delegated-mutation transactions.
//!
//! This module persists evidence only. It does not allocate workspaces, run
//! children or gates, inspect Git, mutate a parent, merge, or roll back.

use std::fmt;

use wcore_types::child_transaction::{
    ChildGatePlan, ChildTransactionReceipt, ChildTransactionReducer, ChildTransactionReplay,
};
use wcore_types::spawner::{ChildId, ChildWorkspaceMode};

use crate::session_journal::{
    ChildTransactionOpening, ChildTransactionSnapshotBinding, ChildTransactionState,
    CommittedChildTransactionReceipt, JournalEnvelope, JournalError, ReducedSessionState,
    SessionEvent, SessionJournal, child_transaction_opening_token_digest,
};

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
        authority_from_state(transaction)
    }

    /// Revalidate the retained opening before either workspace allocation or
    /// child launch. Unrelated journal appends do not change this authority.
    pub fn revalidate(&self, authority: &ChildTransactionAuthority) -> Result<(), JournalError> {
        let state = self.journal.state()?;
        validate_retained_authority(&state, authority, true).map(|_| ())
    }

    /// Commit one canonical receipt under the retained opening authority.
    pub fn commit(
        &self,
        authority: &ChildTransactionAuthority,
        receipt: ChildTransactionReceipt,
    ) -> Result<ChildTransactionWrite, JournalError> {
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
}

pub(crate) enum CommitProjection {
    Applied(ChildTransactionState),
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
    Ok(CommitProjection::Applied(projection))
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
