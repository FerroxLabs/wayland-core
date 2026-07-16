//! Canonical durable child store backed by the session journal.

use std::collections::BTreeSet;

use wcore_types::spawner::{
    ChildDeliveryReconciliation, ChildDeliveryState, ChildDesiredState, ChildId,
    ChildRecoveryState, DurableChildRecord, DurableChildResult, DurableChildStatus,
    DurableChildTransition, MAX_DURABLE_CHILD_APPLIED_EVENTS, MAX_DURABLE_CHILD_ARTIFACTS,
    MAX_DURABLE_CHILD_ID_BYTES,
};

use crate::session_journal::{
    JournalEnvelope, JournalError, SessionEvent, SessionJournal, state_payload_digest,
    validate_durable_child_lineage,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransitionDisposition {
    Applied,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableChildWrite {
    Appended(Box<JournalEnvelope>),
    AlreadyCommitted,
}

/// The single mutation/read API for F18 durable child records.
#[derive(Debug, Clone)]
pub struct DurableChildStore {
    journal: SessionJournal,
}

impl DurableChildStore {
    #[must_use]
    pub fn new(journal: SessionJournal) -> Self {
        Self { journal }
    }

    pub fn declare(&self, record: DurableChildRecord) -> Result<DurableChildWrite, JournalError> {
        record
            .validate_declaration()
            .map_err(|error| invalid(error.to_string()))?;
        let declaration_digest =
            state_payload_digest(&serde_json::to_value(&record).map_err(|source| {
                JournalError::Json {
                    context: "encoding durable child declaration",
                    source,
                }
            })?)?;
        let child_id = record.child_id.clone();
        let declaration_id = record.declaration_id.clone();
        let validation_record = record.clone();
        self.journal
            .append_conditionally(SessionEvent::ChildDeclaredV2 { record }, |state, session_id| {
                if validation_record.parent.session_id != session_id {
                    return Err(invalid(format!(
                        "durable child {child_id} parent session does not match journal authority"
                    )));
                }
                if let Some(existing) = state.children.get(child_id.as_str()) {
                    let exact_replay = existing
                        .durable
                        .as_ref()
                        .is_some_and(|durable| durable.declaration_id == declaration_id)
                        && existing.durable_declaration_digest.as_deref()
                            == Some(declaration_digest.as_str());
                    return if exact_replay {
                        Ok(false)
                    } else {
                        Err(invalid(format!(
                            "durable child {child_id} declaration conflicts with committed authority"
                        )))
                    };
                }
                validate_durable_child_lineage(state, &validation_record, session_id)?;
                if let Some((other_child_id, _)) = state.children.iter().find(|(_, child)| {
                    child
                        .durable
                        .as_ref()
                        .is_some_and(|durable| durable.declaration_id == declaration_id)
                }) {
                    return Err(invalid(format!(
                        "durable declaration {declaration_id} is already bound to child {other_child_id}"
                    )));
                }
                Ok(true)
            })
            .map(write_result)
    }

    pub fn transition(
        &self,
        child_id: ChildId,
        event_id: impl Into<String>,
        expected_revision: u64,
        at_unix_ms: u64,
        transition: DurableChildTransition,
    ) -> Result<DurableChildWrite, JournalError> {
        let event_id = event_id.into();
        validate_identifier("event_id", &event_id)?;
        let event_digest = transition_digest(expected_revision, at_unix_ms, &transition)?;
        let lookup_child_id = child_id.clone();
        let lookup_event_id = event_id.clone();
        self.journal
            .append_conditionally(
                SessionEvent::ChildTransitionedV2 {
                    child_id,
                    event_id,
                    expected_revision,
                    at_unix_ms,
                    transition,
                },
                |state, _session_id| {
                    let child = state.children.get(lookup_child_id.as_str()).ok_or_else(|| {
                        invalid(format!("unknown durable child id {lookup_child_id}"))
                    })?;
                    let record = child.durable.as_ref().ok_or_else(|| {
                        invalid(format!(
                            "legacy child {lookup_child_id} cannot accept a V2 transition"
                        ))
                    })?;
                    if let Some(existing_digest) = record.applied_events.get(&lookup_event_id) {
                        return if existing_digest == &event_digest {
                            Ok(false)
                        } else {
                            Err(invalid(format!(
                                "durable child {lookup_child_id} event {lookup_event_id} conflicts with its committed payload"
                            )))
                        };
                    }
                    Ok(true)
                },
            )
            .map(write_result)
    }

    pub fn inspect(&self, child_id: &ChildId) -> Result<Option<DurableChildRecord>, JournalError> {
        Ok(self
            .journal
            .state()?
            .children
            .get(child_id.as_str())
            .and_then(|child| child.durable.clone()))
    }

    pub fn list(&self) -> Result<Vec<DurableChildRecord>, JournalError> {
        Ok(self
            .journal
            .state()?
            .children
            .into_values()
            .filter_map(|child| child.durable)
            .collect())
    }

    /// Persist a content-addressed terminal child payload before the terminal
    /// transition makes its digest visible in the durable record.
    pub(crate) fn store_result_payload(
        &self,
        digest: &str,
        payload: &[u8],
    ) -> Result<(), JournalError> {
        self.journal.store_effect_checkpoint(digest, payload)
    }

    /// Load and integrity-check the terminal child payload addressed by the
    /// digest committed in [`DurableChildResult::exact_digest`].
    pub(crate) fn load_result_payload(&self, digest: &str) -> Result<Vec<u8>, JournalError> {
        self.journal.load_effect_checkpoint(digest)
    }
}

fn write_result(envelope: Option<JournalEnvelope>) -> DurableChildWrite {
    envelope.map_or(DurableChildWrite::AlreadyCommitted, |envelope| {
        DurableChildWrite::Appended(Box::new(envelope))
    })
}

pub(crate) fn apply_transition(
    record: &mut DurableChildRecord,
    event_id: &str,
    expected_revision: u64,
    at_unix_ms: u64,
    transition: &DurableChildTransition,
) -> Result<TransitionDisposition, JournalError> {
    validate_identifier("event_id", event_id)?;
    let event_digest = transition_digest(expected_revision, at_unix_ms, transition)?;
    if let Some(existing_digest) = record.applied_events.get(event_id) {
        return if existing_digest == &event_digest {
            Ok(TransitionDisposition::Duplicate)
        } else {
            Err(invalid(format!(
                "durable child {} event {event_id} conflicts with its committed payload",
                record.child_id
            )))
        };
    }
    if record.applied_events.len() >= MAX_DURABLE_CHILD_APPLIED_EVENTS {
        return Err(invalid(format!(
            "durable child {} applied-event ledger is full",
            record.child_id
        )));
    }
    if expected_revision != record.revision {
        return Err(invalid(format!(
            "durable child {} expected revision {}, found {}",
            record.child_id, expected_revision, record.revision
        )));
    }
    if at_unix_ms < record.timestamps.updated_at_unix_ms {
        return Err(invalid(format!(
            "durable child {} transition timestamp is stale",
            record.child_id
        )));
    }

    let previous = record.clone();
    apply_new_transition(record, at_unix_ms, transition)?;
    record.revision = record
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("durable child revision overflow"))?;
    record.timestamps.updated_at_unix_ms = at_unix_ms;
    record
        .applied_events
        .insert(event_id.to_owned(), event_digest);
    if let Err(error) = validate_evolved_record(record) {
        *record = previous;
        return Err(error);
    }
    Ok(TransitionDisposition::Applied)
}

fn apply_new_transition(
    record: &mut DurableChildRecord,
    at_unix_ms: u64,
    transition: &DurableChildTransition,
) -> Result<(), JournalError> {
    use DurableChildStatus as Status;
    use DurableChildTransition as Transition;

    match transition {
        Transition::Enqueue if record.status == Status::Prepared => {
            record.status = Status::Queued;
            record.timestamps.queued_at_unix_ms = Some(at_unix_ms);
        }
        Transition::Start if record.status == Status::Queued => {
            record.status = Status::Running;
            record
                .timestamps
                .started_at_unix_ms
                .get_or_insert(at_unix_ms);
        }
        Transition::RequestPause
            if record.status == Status::Running
                && record.desired_state == ChildDesiredState::Run =>
        {
            record.desired_state = ChildDesiredState::Pause;
        }
        Transition::Paused
            if record.status == Status::Running
                && record.desired_state == ChildDesiredState::Pause =>
        {
            record.status = Status::Paused;
        }
        Transition::Resume if record.status == Status::Paused => {
            record.status = Status::Queued;
            record.desired_state = ChildDesiredState::Run;
        }
        Transition::RequestCancel
            if !record.status.is_terminal()
                && record.desired_state != ChildDesiredState::Cancel =>
        {
            record.desired_state = ChildDesiredState::Cancel;
        }
        Transition::Succeed { result } if record.status == Status::Running => {
            validate_result(result)?;
            record.status = Status::Succeeded;
            record.result = Some(result.clone());
            record.timestamps.terminal_at_unix_ms = Some(at_unix_ms);
        }
        Transition::Fail { result }
            if matches!(
                record.status,
                Status::Prepared | Status::Queued | Status::Running | Status::Paused
            ) =>
        {
            validate_result(result)?;
            record.status = Status::Failed;
            record.result = Some(result.clone());
            record.timestamps.terminal_at_unix_ms = Some(at_unix_ms);
        }
        Transition::Cancel
            if matches!(
                record.status,
                Status::Prepared | Status::Queued | Status::Running | Status::Paused
            ) && record.desired_state == ChildDesiredState::Cancel =>
        {
            record.status = Status::Cancelled;
            record.timestamps.terminal_at_unix_ms = Some(at_unix_ms);
        }
        Transition::RequireRecovery { reason_digest } if record.status == Status::Running => {
            validate_digest("recovery.reason_digest", reason_digest)?;
            record.status = Status::RecoveryRequired;
            record.recovery = ChildRecoveryState::Required {
                reason_digest: reason_digest.clone(),
            };
        }
        Transition::ResolveRecovery { evidence_digest }
            if record.status == Status::RecoveryRequired
                && record.desired_state != ChildDesiredState::Cancel =>
        {
            validate_digest("recovery.evidence_digest", evidence_digest)?;
            record.status = match record.desired_state {
                ChildDesiredState::Run => Status::Queued,
                ChildDesiredState::Pause => Status::Paused,
                ChildDesiredState::Cancel => {
                    return Err(invalid(
                        "cancel-requested durable child requires cancel-after-recovery",
                    ));
                }
            };
            record.recovery = ChildRecoveryState::Resolved {
                evidence_digest: evidence_digest.clone(),
            };
        }
        Transition::SucceedAfterRecovery { result }
            if record.status == Status::RecoveryRequired =>
        {
            validate_result(result)?;
            record.status = Status::Succeeded;
            record.recovery = ChildRecoveryState::Resolved {
                evidence_digest: result.exact_digest.clone(),
            };
            record.result = Some(result.clone());
            record.timestamps.terminal_at_unix_ms = Some(at_unix_ms);
        }
        Transition::FailAfterRecovery { result } if record.status == Status::RecoveryRequired => {
            validate_result(result)?;
            record.status = Status::Failed;
            record.recovery = ChildRecoveryState::Resolved {
                evidence_digest: result.exact_digest.clone(),
            };
            record.result = Some(result.clone());
            record.timestamps.terminal_at_unix_ms = Some(at_unix_ms);
        }
        Transition::CancelAfterRecovery { evidence_digest }
            if record.status == Status::RecoveryRequired
                && record.desired_state == ChildDesiredState::Cancel =>
        {
            validate_digest("recovery.evidence_digest", evidence_digest)?;
            record.status = Status::Cancelled;
            record.recovery = ChildRecoveryState::Resolved {
                evidence_digest: evidence_digest.clone(),
            };
            record.timestamps.terminal_at_unix_ms = Some(at_unix_ms);
        }
        Transition::DeliveryStarted
            if execution_terminal(record.status)
                && matches!(record.delivery_state, ChildDeliveryState::Pending) =>
        {
            record.delivery_state = ChildDeliveryState::InFlight;
        }
        Transition::DeliveryDelivered { receipt_digest }
            if execution_terminal(record.status)
                && matches!(record.delivery_state, ChildDeliveryState::InFlight) =>
        {
            validate_digest("delivery.receipt_digest", receipt_digest)?;
            record.delivery_state = ChildDeliveryState::Delivered {
                receipt_digest: receipt_digest.clone(),
            };
        }
        Transition::DeliveryFailed { error_digest }
            if execution_terminal(record.status)
                && matches!(record.delivery_state, ChildDeliveryState::InFlight) =>
        {
            validate_digest("delivery.error_digest", error_digest)?;
            record.delivery_state = ChildDeliveryState::Failed {
                error_digest: error_digest.clone(),
            };
        }
        Transition::DeliveryUnknown { evidence_digest }
            if execution_terminal(record.status)
                && matches!(record.delivery_state, ChildDeliveryState::InFlight) =>
        {
            validate_digest("delivery.evidence_digest", evidence_digest)?;
            record.delivery_state = ChildDeliveryState::Unknown {
                evidence_digest: evidence_digest.clone(),
            };
        }
        Transition::RetryFailedDelivery { prior_error_digest }
            if execution_terminal(record.status)
                && matches!(
                    &record.delivery_state,
                    ChildDeliveryState::Failed { error_digest }
                        if error_digest == prior_error_digest
                ) =>
        {
            validate_digest("delivery.prior_error_digest", prior_error_digest)?;
            record.delivery_state = ChildDeliveryState::Pending;
        }
        Transition::ReconcileUnknownDelivery {
            prior_evidence_digest,
            resolution,
        } if execution_terminal(record.status)
            && matches!(
                &record.delivery_state,
                ChildDeliveryState::Unknown { evidence_digest }
                    if evidence_digest == prior_evidence_digest
            ) =>
        {
            validate_digest("delivery.prior_evidence_digest", prior_evidence_digest)?;
            record.delivery_state = match resolution {
                ChildDeliveryReconciliation::Delivered { receipt_digest } => {
                    validate_digest("delivery.receipt_digest", receipt_digest)?;
                    ChildDeliveryState::Delivered {
                        receipt_digest: receipt_digest.clone(),
                    }
                }
                ChildDeliveryReconciliation::Failed { error_digest } => {
                    validate_digest("delivery.error_digest", error_digest)?;
                    ChildDeliveryState::Failed {
                        error_digest: error_digest.clone(),
                    }
                }
                ChildDeliveryReconciliation::NotDelivered { proof_digest } => {
                    validate_digest("delivery.proof_digest", proof_digest)?;
                    ChildDeliveryState::Pending
                }
            };
        }
        Transition::Expire
            if execution_terminal(record.status)
                && matches!(
                    record.delivery_state,
                    ChildDeliveryState::NotRequired | ChildDeliveryState::Delivered { .. }
                ) =>
        {
            record.status = Status::Expired;
        }
        _ => {
            return Err(invalid(format!(
                "durable child {} cannot apply {transition:?} from {:?}/{:?}",
                record.child_id, record.status, record.desired_state
            )));
        }
    }
    Ok(())
}

fn validate_evolved_record(record: &DurableChildRecord) -> Result<(), JournalError> {
    if usize::try_from(record.revision).ok() != Some(record.applied_events.len()) {
        return Err(invalid("durable child revision/event ledger mismatch"));
    }
    if record.timestamps.updated_at_unix_ms < record.timestamps.created_at_unix_ms
        || record
            .timestamps
            .queued_at_unix_ms
            .is_some_and(|time| time < record.timestamps.created_at_unix_ms)
        || record
            .timestamps
            .started_at_unix_ms
            .is_some_and(|time| time < record.timestamps.created_at_unix_ms)
        || record
            .timestamps
            .terminal_at_unix_ms
            .is_some_and(|time| time < record.timestamps.created_at_unix_ms)
    {
        return Err(invalid("durable child timestamps are inconsistent"));
    }
    if record.status.is_terminal() != record.timestamps.terminal_at_unix_ms.is_some() {
        return Err(invalid("durable child terminal timestamp is inconsistent"));
    }
    let result_is_consistent = match record.status {
        DurableChildStatus::Succeeded | DurableChildStatus::Failed => record.result.is_some(),
        DurableChildStatus::Expired => true,
        _ => record.result.is_none(),
    };
    if !result_is_consistent {
        return Err(invalid("durable child result is inconsistent"));
    }
    if (record.status == DurableChildStatus::RecoveryRequired)
        != matches!(record.recovery, ChildRecoveryState::Required { .. })
    {
        return Err(invalid("durable child recovery evidence is inconsistent"));
    }
    match (&record.delivery_target, &record.delivery_state) {
        (None, ChildDeliveryState::NotRequired) => {}
        (Some(_), state) if !matches!(state, ChildDeliveryState::NotRequired) => {}
        _ => return Err(invalid("durable child delivery target/state mismatch")),
    }
    Ok(())
}

fn execution_terminal(status: DurableChildStatus) -> bool {
    matches!(
        status,
        DurableChildStatus::Succeeded | DurableChildStatus::Failed | DurableChildStatus::Cancelled
    )
}

fn validate_result(result: &DurableChildResult) -> Result<(), JournalError> {
    validate_digest("result.exact_digest", &result.exact_digest)?;
    if result.artifact_digests.len() > MAX_DURABLE_CHILD_ARTIFACTS {
        return Err(invalid("durable child result has too many artifacts"));
    }
    let mut unique = BTreeSet::new();
    for digest in &result.artifact_digests {
        validate_digest("result.artifact_digest", digest)?;
        if !unique.insert(digest) {
            return Err(invalid("durable child result repeats an artifact digest"));
        }
    }
    Ok(())
}

fn transition_digest(
    expected_revision: u64,
    at_unix_ms: u64,
    transition: &DurableChildTransition,
) -> Result<String, JournalError> {
    state_payload_digest(&serde_json::json!({
        "expected_revision": expected_revision,
        "at_unix_ms": at_unix_ms,
        "transition": transition,
    }))
}

fn validate_identifier(field: &str, value: &str) -> Result<(), JournalError> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(char::is_control)
        || value.len() > MAX_DURABLE_CHILD_ID_BYTES
    {
        return Err(invalid(format!("invalid durable child {field}")));
    }
    Ok(())
}

fn validate_digest(field: &str, value: &str) -> Result<(), JournalError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid(format!(
            "invalid SHA-256 digest in durable child {field}"
        )));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> JournalError {
    JournalError::InvalidTransition(message.into())
}
