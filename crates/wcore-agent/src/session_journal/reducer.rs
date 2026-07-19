//! Deterministic session-journal state reduction and payload digests.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::*;
use crate::durable_child::{TransitionDisposition, apply_transition};
use crate::provider_recovery::{
    provider_response_digest, validate_appended_provider_events, validate_finished_provider_events,
};
use wcore_types::spawner::{ChildDeliveryTarget, DurableChildRecord, DurableChildStatus};

impl ReducedSessionState {
    pub fn digest(&self) -> Result<String, JournalError> {
        let bytes = serde_json::to_vec(self).map_err(|source| JournalError::Json {
            context: "encoding reduced state",
            source,
        })?;
        Ok(sha256_hex(&bytes))
    }
}

pub(crate) fn reduce(
    mut state: ReducedSessionState,
    envelope: &JournalEnvelope,
) -> Result<ReducedSessionState, JournalError> {
    let expected_seq = state.last_seq.map_or(0, |seq| seq + 1);
    validate_journal_schema_for_reader(envelope.schema_version)?;
    enforce_typed_event_schema_boundary(envelope)?;
    match state.session_id.as_deref() {
        Some(expected) if expected != envelope.session_id => {
            return Err(JournalError::SessionMismatch {
                expected: expected.to_owned(),
                found: envelope.session_id.clone(),
            });
        }
        None => state.session_id = Some(envelope.session_id.clone()),
        _ => {}
    }
    if envelope.seq != expected_seq {
        return Err(JournalError::SequenceMismatch {
            expected: expected_seq,
            found: envelope.seq,
        });
    }
    if envelope.previous_checksum != state.last_checksum {
        return Err(JournalError::PreviousChecksumMismatch { seq: envelope.seq });
    }
    if envelope.computed_checksum()? != envelope.checksum {
        return Err(JournalError::ChecksumMismatch { seq: envelope.seq });
    }
    apply_event(&mut state, &envelope.event)?;
    state.last_seq = Some(envelope.seq);
    state.last_checksum.clone_from(&envelope.checksum);
    Ok(state)
}

pub fn replay_state(entries: &[JournalEnvelope]) -> Result<ReducedSessionState, JournalError> {
    let mut state = ReducedSessionState::default();
    let mut previous_schema = None;
    for envelope in entries {
        reject_schema_regression(previous_schema, envelope.schema_version)?;
        state = reduce(state, envelope)?;
        previous_schema = Some(envelope.schema_version);
    }
    Ok(state)
}

fn duplicate(kind: &str, id: &str) -> JournalError {
    JournalError::InvalidTransition(format!("duplicate {kind} id {id}"))
}

fn missing(kind: &str, id: &str) -> JournalError {
    JournalError::InvalidTransition(format!("unknown {kind} id {id}"))
}

fn valid_sha256_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_hook_manifest(slots: &[HookManifestSlot]) -> bool {
    slots.iter().enumerate().all(|(index, slot)| {
        slot.ordinal == index as u64
            && !slot.slot_id.is_empty()
            && valid_sha256_digest(&slot.descriptor_digest)
    }) && slots
        .iter()
        .map(|slot| slot.slot_id.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        == slots.len()
}

fn valid_hook_receipts(manifest: &[HookManifestSlot], receipts: &[HookSlotReceipt]) -> bool {
    manifest.len() == receipts.len()
        && manifest.iter().zip(receipts).all(|(slot, receipt)| {
            slot.ordinal == receipt.ordinal
                && slot.slot_id == receipt.slot_id
                && slot.descriptor_digest == receipt.descriptor_digest
        })
}

fn required_mut<'a, T>(
    map: &'a mut BTreeMap<String, T>,
    kind: &str,
    id: &str,
) -> Result<&'a mut T, JournalError> {
    map.get_mut(id).ok_or_else(|| missing(kind, id))
}

fn require_prepared(
    effect: &ExternalEffectState,
    kind: &str,
    id: &str,
) -> Result<(), JournalError> {
    if matches!(effect, ExternalEffectState::Prepared) {
        Ok(())
    } else {
        Err(JournalError::InvalidTransition(format!(
            "{kind} {id} was not prepared"
        )))
    }
}

fn require_unknown(effect: &ExternalEffectState, kind: &str, id: &str) -> Result<(), JournalError> {
    if matches!(effect, ExternalEffectState::Unknown) {
        Ok(())
    } else {
        Err(JournalError::InvalidTransition(format!(
            "{kind} {id} has no unresolved started effect"
        )))
    }
}

fn require_tool_prepared(effect: &ToolEffectState, id: &str) -> Result<(), JournalError> {
    if matches!(effect, ToolEffectState::Prepared) {
        Ok(())
    } else {
        Err(JournalError::InvalidTransition(format!(
            "tool execution {id} was not prepared"
        )))
    }
}

fn require_tool_running(effect: &ToolEffectState, id: &str) -> Result<(), JournalError> {
    if matches!(effect, ToolEffectState::Running) {
        Ok(())
    } else {
        Err(JournalError::InvalidTransition(format!(
            "tool execution {id} is not running"
        )))
    }
}

fn validate_filesystem_start_receipt(tool: &ToolState, id: &str) -> Result<(), JournalError> {
    if !matches!(
        tool.effect_contract.kind,
        wcore_types::tool::ToolEffectKind::FilesystemTransactional
    ) {
        return Ok(());
    }
    if tool.effect_contract.reconciler.as_deref()
        != Some(wcore_tools::effects::FILESYSTEM_EFFECT_RECONCILER)
    {
        return Err(JournalError::InvalidTransition(format!(
            "filesystem-transactional tool execution {id} has an unsupported reconciler"
        )));
    }
    let encoded = tool.effect_receipt.clone().ok_or_else(|| {
        JournalError::InvalidTransition(format!(
            "filesystem-transactional tool execution {id} has no durable effect receipt"
        ))
    })?;
    let receipt = serde_json::from_value::<wcore_tools::effects::FilesystemEffectReceiptV1>(
        encoded,
    )
    .map_err(|error| {
        JournalError::InvalidTransition(format!(
            "filesystem-transactional tool execution {id} has a malformed effect receipt: {error}"
        ))
    })?;
    receipt.validate().map_err(|error| {
        JournalError::InvalidTransition(format!(
            "filesystem-transactional tool execution {id} has an invalid effect receipt: {error}"
        ))
    })
}

#[allow(clippy::too_many_arguments)]
fn validate_tool_retry(
    state: &ReducedSessionState,
    retry_of: Option<&str>,
    idempotency_key: &str,
    provider_call_id: &str,
    turn_id: &str,
    ordinal: u64,
    tool: &str,
    requested_input: &StoredToolInput,
    requested_input_digest: &str,
    effective_input: &StoredToolInput,
    effective_input_digest: &str,
    effect_contract: &wcore_types::tool::ToolEffectContract,
    effect_receipt: &Option<serde_json::Value>,
) -> Result<(), JournalError> {
    let Some(retry_of) = retry_of else {
        if state.tools.values().any(|existing| {
            existing.turn_id == turn_id && existing.provider_call_id == provider_call_id
        }) {
            return Err(JournalError::InvalidTransition(format!(
                "turn {turn_id} already has provider tool call {provider_call_id}"
            )));
        }
        if state
            .tools
            .values()
            .any(|existing| existing.idempotency_key == idempotency_key)
        {
            return Err(JournalError::InvalidTransition(format!(
                "duplicate tool idempotency key {idempotency_key}"
            )));
        }
        return Ok(());
    };

    let prior = state
        .tools
        .get(retry_of)
        .ok_or_else(|| missing("retry source tool execution", retry_of))?;
    if !matches!(prior.effect, ToolEffectState::NotStarted) {
        return Err(JournalError::InvalidTransition(format!(
            "tool execution {retry_of} is not durably not started"
        )));
    }
    if prior.idempotency_key != idempotency_key
        || prior.provider_call_id != provider_call_id
        || prior.turn_id != turn_id
        || prior.ordinal != ordinal
        || prior.tool != tool
        || &prior.requested_input != requested_input
        || prior.requested_input_digest != requested_input_digest
        || &prior.effective_input != effective_input
        || prior.effective_input_digest != effective_input_digest
        || &prior.effect_contract != effect_contract
        || &prior.effect_receipt != effect_receipt
    {
        return Err(JournalError::InvalidTransition(format!(
            "tool retry does not exactly match source execution {retry_of}"
        )));
    }

    let mut ancestors = BTreeSet::new();
    let mut cursor = Some(retry_of);
    while let Some(id) = cursor {
        if !ancestors.insert(id.to_owned()) {
            return Err(JournalError::InvalidTransition(format!(
                "tool retry lineage contains a cycle at {id}"
            )));
        }
        let attempt = state
            .tools
            .get(id)
            .ok_or_else(|| missing("retry ancestor tool execution", id))?;
        cursor = attempt.retry_of.as_deref();
    }

    if state.tools.iter().any(|(id, existing)| {
        existing.idempotency_key == idempotency_key && !ancestors.contains(id)
    }) {
        return Err(JournalError::InvalidTransition(format!(
            "duplicate tool idempotency key {idempotency_key} outside retry lineage"
        )));
    }
    if state.tools.iter().any(|(id, existing)| {
        existing.turn_id == turn_id
            && existing.provider_call_id == provider_call_id
            && !ancestors.contains(id)
    }) {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} already has provider tool call {provider_call_id} outside retry lineage"
        )));
    }
    Ok(())
}

fn require_tool_unknown(effect: &ToolEffectState, id: &str) -> Result<(), JournalError> {
    if matches!(effect, ToolEffectState::Unknown { .. }) {
        Ok(())
    } else {
        Err(JournalError::InvalidTransition(format!(
            "tool execution {id} has no unresolved effect"
        )))
    }
}

fn provider_stream_events(stream: &StreamState) -> Vec<ProviderStreamEvent> {
    stream.batches.iter().flatten().cloned().collect()
}

fn require_correlated_dispatch(
    attempt: &ProviderAttemptState,
    attempt_id: &str,
    dispatch_id: &str,
) -> Result<(), JournalError> {
    match attempt.dispatch_id.as_deref() {
        Some(actual) if actual == dispatch_id => Ok(()),
        Some(actual) => Err(JournalError::InvalidTransition(format!(
            "provider attempt {attempt_id} belongs to dispatch {actual}, not {dispatch_id}"
        ))),
        None => Err(JournalError::InvalidTransition(format!(
            "legacy provider attempt {attempt_id} has no dispatch correlation"
        ))),
    }
}

fn validate_correlated_terminal(
    state: &ReducedSessionState,
    attempt_id: &str,
    outcome: &CompletionOutcome,
    response_digest: Option<&str>,
) -> Result<(), JournalError> {
    let streams = state
        .streams
        .values()
        .filter(|stream| stream.attempt_id == attempt_id)
        .collect::<Vec<_>>();
    if matches!(outcome, CompletionOutcome::Succeeded) && streams.len() != 1 {
        return Err(JournalError::InvalidTransition(format!(
            "successful recovery-correlated provider attempt {attempt_id} must have exactly one stream"
        )));
    }
    if streams.len() > 1 {
        return Err(JournalError::InvalidTransition(format!(
            "recovery-correlated provider attempt {attempt_id} has multiple streams"
        )));
    }
    let events = streams
        .first()
        .map_or_else(Vec::new, |stream| provider_stream_events(stream));
    if matches!(outcome, CompletionOutcome::Succeeded) {
        let stream = streams[0];
        if !stream.finished {
            return Err(JournalError::InvalidTransition(format!(
                "successful recovery-correlated provider attempt {attempt_id} has an unfinished stream"
            )));
        }
        validate_finished_provider_events(&events)?;
    }
    match (events.is_empty(), response_digest) {
        (true, None) => Ok(()),
        (false, Some(recorded)) => {
            let computed = provider_response_digest(&events)?;
            if computed == recorded {
                Ok(())
            } else {
                Err(JournalError::InvalidTransition(format!(
                    "provider attempt {attempt_id} response digest does not match its durable stream"
                )))
            }
        }
        (true, Some(_)) => Err(JournalError::InvalidTransition(format!(
            "provider attempt {attempt_id} has a response digest without durable events"
        ))),
        (false, None) => Err(JournalError::InvalidTransition(format!(
            "provider attempt {attempt_id} has durable events without a response digest"
        ))),
    }
}

fn require_active_turn(state: &ReducedSessionState, turn_id: &str) -> Result<(), JournalError> {
    let turn = state
        .turns
        .get(turn_id)
        .ok_or_else(|| missing("turn", turn_id))?;
    if turn.completion.is_some() {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} is terminal"
        )));
    }
    Ok(())
}

fn require_approval_origin_prepared(
    state: &ReducedSessionState,
    origin: &ApprovalOrigin,
) -> Result<(), JournalError> {
    match origin {
        ApprovalOrigin::Turn { turn_id } => require_active_turn(state, turn_id),
        ApprovalOrigin::ProviderAttempt { attempt_id } => {
            let attempt = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?;
            require_active_turn(state, &attempt.turn_id)?;
            require_prepared(&attempt.effect, "provider attempt", attempt_id)
        }
        ApprovalOrigin::ToolExecution { tool_execution_id } => {
            let tool = state
                .tools
                .get(tool_execution_id)
                .ok_or_else(|| missing("tool execution", tool_execution_id))?;
            require_active_turn(state, &tool.turn_id)?;
            require_tool_prepared(&tool.effect, tool_execution_id)
        }
        ApprovalOrigin::Child { child_id } => {
            let child = state
                .children
                .get(child_id)
                .ok_or_else(|| missing("child", child_id))?;
            require_active_turn(state, &child.turn_id)?;
            require_prepared(&child.effect, "child", child_id)
        }
        ApprovalOrigin::Delivery { delivery_id } => {
            let delivery = state
                .deliveries
                .get(delivery_id)
                .ok_or_else(|| missing("delivery", delivery_id))?;
            require_delivery_origin_active(state, &delivery.origin)?;
            require_prepared(&delivery.effect, "delivery", delivery_id)
        }
    }
}

fn require_budget_owner_exists(
    state: &ReducedSessionState,
    owner: &BudgetOwner,
) -> Result<(), JournalError> {
    match owner {
        BudgetOwner::Session => Ok(()),
        BudgetOwner::Turn { turn_id } => require_active_turn(state, turn_id),
        BudgetOwner::ProviderAttempt { attempt_id } => state
            .provider_attempts
            .contains_key(attempt_id)
            .then_some(())
            .ok_or_else(|| missing("provider attempt", attempt_id)),
        BudgetOwner::ToolExecution { tool_execution_id } => state
            .tools
            .contains_key(tool_execution_id)
            .then_some(())
            .ok_or_else(|| missing("tool execution", tool_execution_id)),
        BudgetOwner::Child { child_id } => state
            .children
            .contains_key(child_id)
            .then_some(())
            .ok_or_else(|| missing("child", child_id)),
    }
}

fn execution_snapshot_state_count(
    snapshot: &wcore_budget::ExecutionBudgetSnapshot,
    field: &'static str,
) -> Result<usize, JournalError> {
    let value = serde_json::to_value(snapshot).map_err(|source| JournalError::Json {
        context: "encoding budget authority execution snapshot",
        source,
    })?;
    value
        .get("states")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .ok_or_else(|| {
            JournalError::InvalidTransition(format!(
                "budget authority {field} has no execution state array"
            ))
        })
}

fn validate_budget_authority(
    state: &ReducedSessionState,
    authority: &BudgetAuthorityState,
) -> Result<(), JournalError> {
    if state.imported_baseline.is_none() {
        return Err(JournalError::InvalidTransition(
            "budget authority requires a canonical imported session baseline".to_owned(),
        ));
    }
    if authority.schema_version != LEGACY_BUDGET_AUTHORITY_SCHEMA_VERSION
        && authority.schema_version != BUDGET_AUTHORITY_SCHEMA_VERSION
    {
        return Err(JournalError::InvalidTransition(format!(
            "unsupported budget authority schema {}; expected {} or {}",
            authority.schema_version,
            LEGACY_BUDGET_AUTHORITY_SCHEMA_VERSION,
            BUDGET_AUTHORITY_SCHEMA_VERSION
        )));
    }
    if authority.schema_version == LEGACY_BUDGET_AUTHORITY_SCHEMA_VERSION
        && !authority.provider_reservations.is_empty()
    {
        return Err(JournalError::InvalidTransition(
            "legacy budget authority cannot contain provider reservation bindings".to_owned(),
        ));
    }
    if authority.authority_epoch == 0 {
        return Err(JournalError::InvalidTransition(
            "budget authority epoch must be non-zero".to_owned(),
        ));
    }
    if authority.budget_session_id.trim().is_empty() {
        return Err(JournalError::InvalidTransition(
            "budget authority session id must not be empty".to_owned(),
        ));
    }
    if authority.captured_at_unix_millis == 0 {
        return Err(JournalError::InvalidTransition(
            "budget authority capture time must be non-zero".to_owned(),
        ));
    }
    if authority.prior_cursor.journal_sequence != state.last_seq
        || authority.prior_cursor.journal_checksum != state.last_checksum
    {
        return Err(JournalError::InvalidTransition(
            "budget authority prior cursor does not match the current journal head".to_owned(),
        ));
    }

    let conversation = serde_json::Value::Array(state.conversation.clone());
    if state_payload_digest(&conversation)? != authority.conversation_digest {
        return Err(JournalError::InvalidTransition(
            "budget authority conversation digest does not match canonical context".to_owned(),
        ));
    }

    let provider_tracker = wcore_budget::BudgetTracker::from_snapshot(
        authority.provider_tracker.clone(),
    )
    .map_err(|error| {
        JournalError::InvalidTransition(format!(
            "budget authority provider snapshot is invalid: {error}"
        ))
    })?;
    let mut bound_reservations = HashSet::new();
    for (dispatch_id, binding) in &authority.provider_reservations {
        if dispatch_id.trim().is_empty() {
            return Err(JournalError::InvalidTransition(
                "budget authority provider dispatch id must not be empty".to_owned(),
            ));
        }
        if !bound_reservations.insert(binding.reservation) {
            return Err(JournalError::InvalidTransition(
                "budget authority binds one provider reservation to multiple dispatches".to_owned(),
            ));
        }
        if !provider_tracker.has_reservation(binding.reservation) {
            return Err(JournalError::InvalidTransition(format!(
                "budget authority dispatch {dispatch_id} references a missing provider reservation"
            )));
        }
        let mut prior_attempt_ids = HashSet::new();
        for attempt_id in &binding.prior_attempt_ids {
            if !prior_attempt_ids.insert(attempt_id) {
                return Err(JournalError::InvalidTransition(format!(
                    "budget authority dispatch {dispatch_id} repeats prior attempt {attempt_id}"
                )));
            }
            let attempt = state.provider_attempts.get(attempt_id).ok_or_else(|| {
                JournalError::InvalidTransition(format!(
                    "budget authority dispatch {dispatch_id} references missing prior attempt {attempt_id}"
                ))
            })?;
            if attempt.dispatch_id.as_deref() != Some(dispatch_id.as_str()) {
                return Err(JournalError::InvalidTransition(format!(
                    "budget authority prior attempt {attempt_id} belongs to another dispatch"
                )));
            }
        }
    }
    wcore_budget::ExecutionBudgetView::from_snapshot(authority.execution_root.clone()).map_err(
        |error| {
            JournalError::InvalidTransition(format!(
                "budget authority execution root snapshot is invalid: {error}"
            ))
        },
    )?;
    if execution_snapshot_state_count(&authority.execution_root, "execution root")? != 1 {
        return Err(JournalError::InvalidTransition(
            "budget authority execution root must contain exactly one root state".to_owned(),
        ));
    }

    if let Some(active_turn) = &authority.active_turn {
        require_active_turn(state, &active_turn.turn_id)?;
        wcore_budget::ExecutionBudgetView::from_snapshot(active_turn.execution.clone()).map_err(
            |error| {
                JournalError::InvalidTransition(format!(
                    "budget authority active-turn snapshot is invalid: {error}"
                ))
            },
        )?;
        if execution_snapshot_state_count(&active_turn.execution, "active turn")? < 2 {
            return Err(JournalError::InvalidTransition(
                "budget authority active-turn snapshot must retain its session-root ancestor"
                    .to_owned(),
            ));
        }
    }

    match state.budget_authority.as_ref() {
        None => {
            if authority.authority_epoch != 1 {
                return Err(JournalError::InvalidTransition(format!(
                    "first budget authority epoch must be 1, found {}",
                    authority.authority_epoch
                )));
            }
        }
        Some(previous) => {
            if authority.schema_version < previous.schema_version {
                return Err(JournalError::InvalidTransition(format!(
                    "budget authority schema regressed from {} to {}",
                    previous.schema_version, authority.schema_version
                )));
            }
            let expected_epoch = previous.authority_epoch.checked_add(1).ok_or_else(|| {
                JournalError::InvalidTransition("budget authority epoch is exhausted".to_owned())
            })?;
            if authority.authority_epoch != expected_epoch {
                return Err(JournalError::InvalidTransition(format!(
                    "budget authority epoch regression or gap: expected {expected_epoch}, found {}",
                    authority.authority_epoch
                )));
            }
            if authority.budget_session_id != previous.budget_session_id {
                return Err(JournalError::InvalidTransition(
                    "budget authority session identity changed".to_owned(),
                ));
            }
            if authority.captured_at_unix_millis < previous.captured_at_unix_millis {
                return Err(JournalError::InvalidTransition(
                    "budget authority capture time regressed".to_owned(),
                ));
            }
            match (&previous.wall_clock, &authority.wall_clock) {
                (
                    BudgetWallClockAuthority::ActiveRuntime,
                    BudgetWallClockAuthority::ActiveRuntime,
                ) => {}
                (
                    BudgetWallClockAuthority::AbsoluteDeadline {
                        deadline_unix_millis: previous_deadline,
                    },
                    BudgetWallClockAuthority::AbsoluteDeadline {
                        deadline_unix_millis: next_deadline,
                    },
                ) if next_deadline <= previous_deadline => {}
                (
                    BudgetWallClockAuthority::AbsoluteDeadline { .. },
                    BudgetWallClockAuthority::AbsoluteDeadline { .. },
                ) => {
                    return Err(JournalError::InvalidTransition(
                        "budget authority absolute deadline was widened".to_owned(),
                    ));
                }
                _ => {
                    return Err(JournalError::InvalidTransition(
                        "budget authority wall-clock semantics changed".to_owned(),
                    ));
                }
            }
            if let Some(previous_turn) = &previous.active_turn
                && state
                    .turns
                    .get(&previous_turn.turn_id)
                    .is_some_and(|turn| turn.completion.is_none())
                && authority.active_turn.is_none()
            {
                return Err(JournalError::InvalidTransition(format!(
                    "budget authority dropped active-turn state for {}",
                    previous_turn.turn_id
                )));
            }
        }
    }
    Ok(())
}

fn require_delivery_origin_active(
    state: &ReducedSessionState,
    origin: &DeliveryOrigin,
) -> Result<(), JournalError> {
    match origin {
        DeliveryOrigin::Turn { turn_id } => require_active_turn(state, turn_id),
        DeliveryOrigin::InboundReply { .. } | DeliveryOrigin::Cron { .. } => Ok(()),
    }
}

fn approval_origin_belongs_to_turn(
    state: &ReducedSessionState,
    origin: &ApprovalOrigin,
    turn_id: &str,
) -> bool {
    match origin {
        ApprovalOrigin::Turn {
            turn_id: origin_turn,
        } => origin_turn == turn_id,
        ApprovalOrigin::ProviderAttempt { attempt_id } => state
            .provider_attempts
            .get(attempt_id)
            .is_some_and(|attempt| attempt.turn_id == turn_id),
        ApprovalOrigin::ToolExecution { tool_execution_id } => state
            .tools
            .get(tool_execution_id)
            .is_some_and(|tool| tool.turn_id == turn_id),
        ApprovalOrigin::Child { child_id } => state
            .children
            .get(child_id)
            .is_some_and(|child| child.turn_id == turn_id),
        ApprovalOrigin::Delivery { delivery_id } => {
            state.deliveries.get(delivery_id).is_some_and(|delivery| {
                matches!(
                    &delivery.origin,
                    DeliveryOrigin::Turn {
                        turn_id: origin_turn
                    } if origin_turn == turn_id
                )
            })
        }
    }
}

fn budget_owner_belongs_to_turn(
    state: &ReducedSessionState,
    owner: &BudgetOwner,
    turn_id: &str,
) -> bool {
    match owner {
        BudgetOwner::Session => false,
        BudgetOwner::Turn {
            turn_id: owner_turn,
        } => owner_turn == turn_id,
        BudgetOwner::ProviderAttempt { attempt_id } => state
            .provider_attempts
            .get(attempt_id)
            .is_some_and(|attempt| attempt.turn_id == turn_id),
        BudgetOwner::ToolExecution { tool_execution_id } => state
            .tools
            .get(tool_execution_id)
            .is_some_and(|tool| tool.turn_id == turn_id),
        BudgetOwner::Child { child_id } => state
            .children
            .get(child_id)
            .is_some_and(|child| child.durable.is_none() && child.turn_id == turn_id),
    }
}

pub(crate) fn require_turn_descendants_terminal(
    state: &ReducedSessionState,
    turn_id: &str,
) -> Result<(), JournalError> {
    let pending_approval = state.approvals.iter().find(|(_, approval)| {
        approval.resolution.is_none()
            && approval_origin_belongs_to_turn(state, &approval.origin, turn_id)
    });
    if let Some((approval_id, _)) = pending_approval {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} has pending approval {approval_id}"
        )));
    }
    if let Some((attempt_id, _)) = state.provider_attempts.iter().find(|(_, attempt)| {
        attempt.turn_id == turn_id
            && matches!(
                attempt.effect,
                ExternalEffectState::Prepared | ExternalEffectState::Unknown
            )
    }) {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} has nonterminal provider attempt {attempt_id}"
        )));
    }
    if let Some((tool_execution_id, _)) = state.tools.iter().find(|(_, tool)| {
        tool.turn_id == turn_id
            && matches!(
                tool.effect,
                ToolEffectState::Prepared
                    | ToolEffectState::Running
                    | ToolEffectState::Unknown { .. }
            )
    }) {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} has nonterminal tool execution {tool_execution_id}"
        )));
    }
    if let Some((hook_phase_id, _)) = state.hook_phases.iter().find(|(_, phase)| {
        phase.turn_id == turn_id
            && !matches!(
                phase.state,
                HookPhaseState::Consumed { .. }
                    | HookPhaseState::NotStarted { .. }
                    | HookPhaseState::NotApplicable
                    | HookPhaseState::AbandonedUnknown
            )
    }) {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} has nonterminal hook phase {hook_phase_id}"
        )));
    }
    if let Some((child_id, _)) = state.children.iter().find(|(_, child)| {
        child.turn_id == turn_id
            && child.durable.is_none()
            && matches!(
                child.effect,
                ExternalEffectState::Prepared | ExternalEffectState::Unknown
            )
    }) {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} has nonterminal child {child_id}"
        )));
    }
    if let Some((delivery_id, _)) = state.deliveries.iter().find(|(_, delivery)| {
        matches!(
            &delivery.origin,
            DeliveryOrigin::Turn {
                turn_id: origin_turn
            } if origin_turn == turn_id
        ) && matches!(
            delivery.effect,
            ExternalEffectState::Prepared | ExternalEffectState::Unknown
        )
    }) {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} has nonterminal delivery {delivery_id}"
        )));
    }
    if let Some((reservation_id, _)) = state.budgets.iter().find(|(_, budget)| {
        budget.used.is_none()
            && !budget.released
            && budget_owner_belongs_to_turn(state, &budget.owner, turn_id)
    }) {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} has open budget reservation {reservation_id}"
        )));
    }
    Ok(())
}

pub fn state_payload_digest(value: &serde_json::Value) -> Result<String, JournalError> {
    fn canonical(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(canonical).collect())
            }
            serde_json::Value::Object(values) => serde_json::Value::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            scalar => scalar.clone(),
        }
    }
    let bytes = serde_json::to_vec(&canonical(value)).map_err(|source| JournalError::Json {
        context: "encoding checkpoint state",
        source,
    })?;
    Ok(sha256_hex(&bytes))
}

/// Schema version for the protected, replayable provider-request snapshot.
pub const PREPARED_PROVIDER_REQUEST_SNAPSHOT_VERSION: u32 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedProviderRequestSnapshot {
    version: u32,
    request: PreparedProviderRequestV1,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedProviderRequestV1 {
    model: String,
    system: String,
    messages: Vec<PreparedMessageV1>,
    tools: Vec<PreparedToolV1>,
    max_tokens: u32,
    thinking: Option<PreparedThinkingV1>,
    reasoning_effort: Option<String>,
    cache_tier: Option<PreparedCacheTierV1>,
    routing_hint: Option<String>,
    stop_sequences: Vec<String>,
    web_search: bool,
    conversation_id: Option<String>,
    client_context_tokens: Option<u64>,
    temperature: Option<f32>,
    omit_max_tokens: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedMessageV1 {
    role: wcore_types::message::Role,
    content: Vec<PreparedContentBlockV1>,
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
    cache_breakpoint: Option<wcore_types::message::MessageCacheHint>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum PreparedContentBlockV1 {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: wcore_types::message::ToolUseId,
        name: String,
        input: serde_json::Value,
        extra: Option<serde_json::Value>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: wcore_types::message::ToolUseId,
        content: String,
        is_error: bool,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "image")]
    Image { mime: String, data: String },
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedToolV1 {
    name: String,
    description: String,
    input_schema: serde_json::Value,
    deferred: bool,
    server: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum PreparedThinkingV1 {
    Enabled { budget_tokens: u32 },
    Disabled,
}

#[derive(serde::Serialize, serde::Deserialize)]
enum PreparedCacheTierV1 {
    #[serde(rename = "5m")]
    Ephemeral5m,
    #[serde(rename = "1h")]
    Ephemeral1h,
    #[serde(rename = "none")]
    None,
}

impl From<&wcore_types::message::ContentBlock> for PreparedContentBlockV1 {
    fn from(value: &wcore_types::message::ContentBlock) -> Self {
        use wcore_types::message::ContentBlock;

        match value {
            ContentBlock::Text { text } => Self::Text { text: text.clone() },
            ContentBlock::ToolUse {
                id,
                name,
                input,
                extra,
            } => Self::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                extra: extra.clone(),
            },
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Self::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: content.clone(),
                is_error: *is_error,
            },
            ContentBlock::Thinking { thinking } => Self::Thinking {
                thinking: thinking.clone(),
            },
            ContentBlock::Image { mime, data } => Self::Image {
                mime: mime.clone(),
                data: data.clone(),
            },
        }
    }
}

impl From<PreparedContentBlockV1> for wcore_types::message::ContentBlock {
    fn from(value: PreparedContentBlockV1) -> Self {
        match value {
            PreparedContentBlockV1::Text { text } => Self::Text { text },
            PreparedContentBlockV1::ToolUse {
                id,
                name,
                input,
                extra,
            } => Self::ToolUse {
                id,
                name,
                input,
                extra,
            },
            PreparedContentBlockV1::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Self::ToolResult {
                tool_use_id,
                content,
                is_error,
            },
            PreparedContentBlockV1::Thinking { thinking } => Self::Thinking { thinking },
            PreparedContentBlockV1::Image { mime, data } => Self::Image { mime, data },
        }
    }
}

impl From<&wcore_types::llm::LlmRequest> for PreparedProviderRequestV1 {
    fn from(request: &wcore_types::llm::LlmRequest) -> Self {
        Self {
            model: request.model.clone(),
            system: request.system.clone(),
            messages: request
                .messages
                .iter()
                .map(|message| PreparedMessageV1 {
                    role: message.role,
                    content: message.content.iter().map(Into::into).collect(),
                    timestamp: message.timestamp,
                    cache_breakpoint: message.cache_breakpoint,
                })
                .collect(),
            tools: request
                .tools
                .iter()
                .map(|tool| PreparedToolV1 {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                    deferred: tool.deferred,
                    server: tool.server.clone(),
                })
                .collect(),
            max_tokens: request.max_tokens,
            thinking: request.thinking.as_ref().map(|thinking| match thinking {
                wcore_types::llm::ThinkingConfig::Enabled { budget_tokens } => {
                    PreparedThinkingV1::Enabled {
                        budget_tokens: *budget_tokens,
                    }
                }
                wcore_types::llm::ThinkingConfig::Disabled => PreparedThinkingV1::Disabled,
            }),
            reasoning_effort: request.reasoning_effort.clone(),
            cache_tier: request.cache_tier.map(|tier| match tier {
                wcore_types::cache_tier::CacheTier::Ephemeral5m => PreparedCacheTierV1::Ephemeral5m,
                wcore_types::cache_tier::CacheTier::Ephemeral1h => PreparedCacheTierV1::Ephemeral1h,
                wcore_types::cache_tier::CacheTier::None => PreparedCacheTierV1::None,
            }),
            routing_hint: request.routing_hint.as_ref().map(|hint| hint.0.clone()),
            stop_sequences: request.stop_sequences.clone(),
            web_search: request.web_search,
            conversation_id: request.conversation_id.clone(),
            client_context_tokens: request.client_context_tokens,
            temperature: request.temperature,
            omit_max_tokens: request.omit_max_tokens,
        }
    }
}

impl From<PreparedProviderRequestV1> for wcore_types::llm::LlmRequest {
    fn from(request: PreparedProviderRequestV1) -> Self {
        Self {
            model: request.model,
            system: request.system,
            messages: request
                .messages
                .into_iter()
                .map(|message| wcore_types::message::Message {
                    role: message.role,
                    content: message.content.into_iter().map(Into::into).collect(),
                    timestamp: message.timestamp,
                    cache_breakpoint: message.cache_breakpoint,
                })
                .collect(),
            tools: request
                .tools
                .into_iter()
                .map(|tool| wcore_types::tool::ToolDef {
                    name: tool.name,
                    description: tool.description,
                    input_schema: tool.input_schema,
                    deferred: tool.deferred,
                    server: tool.server,
                })
                .collect(),
            max_tokens: request.max_tokens,
            thinking: request.thinking.map(|thinking| match thinking {
                PreparedThinkingV1::Enabled { budget_tokens } => {
                    wcore_types::llm::ThinkingConfig::Enabled { budget_tokens }
                }
                PreparedThinkingV1::Disabled => wcore_types::llm::ThinkingConfig::Disabled,
            }),
            reasoning_effort: request.reasoning_effort,
            cache_tier: request.cache_tier.map(|tier| match tier {
                PreparedCacheTierV1::Ephemeral5m => wcore_types::cache_tier::CacheTier::Ephemeral5m,
                PreparedCacheTierV1::Ephemeral1h => wcore_types::cache_tier::CacheTier::Ephemeral1h,
                PreparedCacheTierV1::None => wcore_types::cache_tier::CacheTier::None,
            }),
            routing_hint: request.routing_hint.map(wcore_types::llm::RoutingHint::new),
            stop_sequences: request.stop_sequences,
            web_search: request.web_search,
            conversation_id: request.conversation_id,
            client_context_tokens: request.client_context_tokens,
            temperature: request.temperature,
            omit_max_tokens: request.omit_max_tokens,
        }
    }
}

/// Encode the exact prepared provider request into its canonical protected form.
pub fn prepared_provider_request_snapshot(
    request: &wcore_types::llm::LlmRequest,
) -> Result<serde_json::Value, JournalError> {
    if request.temperature.is_some_and(|value| !value.is_finite()) {
        return Err(JournalError::InvalidTransition(
            "prepared provider request temperature must be finite".to_owned(),
        ));
    }
    serde_json::to_value(PreparedProviderRequestSnapshot {
        version: PREPARED_PROVIDER_REQUEST_SNAPSHOT_VERSION,
        request: request.into(),
    })
    .map_err(|source| JournalError::Json {
        context: "encoding prepared provider request snapshot",
        source,
    })
}

/// Decode a protected provider request, rejecting drift and malformed fields.
pub fn decode_prepared_provider_request_snapshot(
    snapshot_value: &serde_json::Value,
) -> Result<wcore_types::llm::LlmRequest, JournalError> {
    let snapshot = serde_json::from_value::<PreparedProviderRequestSnapshot>(
        snapshot_value.clone(),
    )
    .map_err(|source| JournalError::Json {
        context: "decoding prepared provider request snapshot",
        source,
    })?;
    if snapshot.version != PREPARED_PROVIDER_REQUEST_SNAPSHOT_VERSION {
        return Err(JournalError::InvalidTransition(format!(
            "unsupported prepared provider request snapshot version {}; supported version is {}",
            snapshot.version, PREPARED_PROVIDER_REQUEST_SNAPSHOT_VERSION
        )));
    }
    let request = snapshot.request.into();
    if prepared_provider_request_snapshot(&request)? != *snapshot_value {
        return Err(JournalError::InvalidTransition(
            "prepared provider request snapshot is not canonical".to_owned(),
        ));
    }
    Ok(request)
}

pub fn provider_request_digest(
    request: &wcore_types::llm::LlmRequest,
) -> Result<String, JournalError> {
    state_payload_digest(&prepared_provider_request_snapshot(request)?)
}

#[allow(clippy::too_many_arguments)]
fn commit_recovery_checkpoint(
    state: &mut ReducedSessionState,
    turn_id: &str,
    messages: &[serde_json::Value],
    messages_digest: &str,
    checkpoint_id: &str,
    checkpoint_state_digest: &str,
    checkpoint: &serde_json::Value,
    consumed_hook_phases: &[HookPhaseConsumption],
) -> Result<(), JournalError> {
    let turn = state
        .turns
        .get(turn_id)
        .ok_or_else(|| missing("turn", turn_id))?;
    if turn.completion.is_some() {
        return Err(JournalError::InvalidTransition(format!(
            "turn {turn_id} is terminal"
        )));
    }
    if messages.iter().any(|message| !message.is_object()) {
        return Err(JournalError::InvalidTransition(
            "every conversation recovery message must be an object".to_owned(),
        ));
    }
    let payload = serde_json::Value::Array(messages.to_vec());
    if state_payload_digest(&payload)? != messages_digest {
        return Err(JournalError::InvalidTransition(
            "conversation recovery message digest mismatch".to_owned(),
        ));
    }
    if state_payload_digest(checkpoint)? != checkpoint_state_digest {
        return Err(JournalError::InvalidTransition(format!(
            "checkpoint {checkpoint_id} state digest mismatch"
        )));
    }
    if state.checkpoints.contains_key(checkpoint_id) {
        return Err(duplicate("checkpoint", checkpoint_id));
    }

    let mut seen = BTreeSet::new();
    for consumption in consumed_hook_phases {
        if !seen.insert(consumption.hook_phase_id.as_str()) {
            return Err(JournalError::InvalidTransition(format!(
                "checkpoint {checkpoint_id} repeats hook phase {}",
                consumption.hook_phase_id
            )));
        }
        let phase = state
            .hook_phases
            .get(&consumption.hook_phase_id)
            .ok_or_else(|| missing("hook phase", &consumption.hook_phase_id))?;
        if phase.turn_id != turn_id {
            return Err(JournalError::InvalidTransition(format!(
                "hook phase {} belongs to turn {}, not {turn_id}",
                consumption.hook_phase_id, phase.turn_id
            )));
        }
        match &phase.state {
            HookPhaseState::Finished { outcome_digest, .. }
                if outcome_digest == &consumption.outcome_digest => {}
            HookPhaseState::Finished { .. } => {
                return Err(JournalError::InvalidTransition(format!(
                    "hook phase {} outcome digest mismatch",
                    consumption.hook_phase_id
                )));
            }
            _ => {
                return Err(JournalError::InvalidTransition(format!(
                    "hook phase {} is not finished and consumable",
                    consumption.hook_phase_id
                )));
            }
        }
    }
    let eligible = state
        .hook_phases
        .iter()
        .filter_map(|(phase_id, phase)| {
            (phase.turn_id == turn_id && matches!(phase.state, HookPhaseState::Finished { .. }))
                .then_some(phase_id.as_str())
        })
        .collect::<BTreeSet<_>>();
    if seen != eligible {
        return Err(JournalError::InvalidTransition(format!(
            "checkpoint {checkpoint_id} must consume every finished hook phase for turn {turn_id}"
        )));
    }
    if state.hook_phases.values().any(|phase| {
        phase.turn_id == turn_id
            && matches!(
                phase.state,
                HookPhaseState::Prepared
                    | HookPhaseState::Started { .. }
                    | HookPhaseState::AbandonedUnknown
            )
    }) {
        return Err(JournalError::InvalidTransition(format!(
            "checkpoint {checkpoint_id} cannot cross an unresolved hook phase for turn {turn_id}"
        )));
    }

    state.conversation = messages.to_vec();
    state.checkpoints.insert(
        checkpoint_id.to_owned(),
        CheckpointState {
            purpose: CheckpointPurpose::Recovery,
            origin: CheckpointOrigin::Turn {
                turn_id: turn_id.to_owned(),
            },
            state_digest: checkpoint_state_digest.to_owned(),
            state: checkpoint.clone(),
        },
    );
    for consumption in consumed_hook_phases {
        let phase = state
            .hook_phases
            .get_mut(&consumption.hook_phase_id)
            .ok_or_else(|| missing("hook phase", &consumption.hook_phase_id))?;
        phase.state = HookPhaseState::Consumed {
            outcome_digest: consumption.outcome_digest.clone(),
            checkpoint_id: checkpoint_id.to_owned(),
        };
    }
    Ok(())
}

pub(crate) fn validate_durable_child_lineage(
    state: &ReducedSessionState,
    record: &DurableChildRecord,
    journal_session_id: &str,
) -> Result<(), JournalError> {
    if state
        .session_id
        .as_deref()
        .is_some_and(|state_session_id| state_session_id != journal_session_id)
        || record.parent.session_id != journal_session_id
    {
        return Err(JournalError::InvalidTransition(format!(
            "durable child {} parent session does not match journal authority",
            record.child_id
        )));
    }
    if let Some(turn_id) = &record.parent.turn_id {
        require_active_turn(state, turn_id)?;
    }

    let mut parent_ids = BTreeSet::from([record.child_id.to_string()]);
    let mut next_parent = record.parent.parent_child_id.as_ref();
    while let Some(parent_id) = next_parent {
        if !parent_ids.insert(parent_id.to_string()) {
            return Err(JournalError::InvalidTransition(format!(
                "durable child {} has a cyclic parent lineage",
                record.child_id
            )));
        }
        let parent = state
            .children
            .get(parent_id.as_str())
            .and_then(|child| child.durable.as_ref())
            .ok_or_else(|| missing("durable parent child", parent_id.as_str()))?;
        if parent.parent.session_id != record.parent.session_id {
            return Err(JournalError::InvalidTransition(format!(
                "durable child {} crosses session lineage",
                record.child_id
            )));
        }
        if parent.status.is_terminal() {
            return Err(JournalError::InvalidTransition(format!(
                "durable child {} declares terminal parent {parent_id}",
                record.child_id
            )));
        }
        next_parent = parent.parent.parent_child_id.as_ref();
    }

    if let Some(retry_of) = &record.retry_of {
        let previous = state
            .children
            .get(retry_of.as_str())
            .and_then(|child| child.durable.as_ref())
            .ok_or_else(|| missing("durable retry child", retry_of.as_str()))?;
        if !matches!(
            previous.status,
            DurableChildStatus::Failed | DurableChildStatus::Cancelled
        ) {
            return Err(JournalError::InvalidTransition(format!(
                "durable child {} retries ineligible child {retry_of}",
                record.child_id
            )));
        }
        // Provider/model may change on a retry (for example provider failover),
        // but task identity, authority, workspace, and delivery binding may not.
        if previous.parent != record.parent
            || previous.origin != record.origin
            || previous.request != record.request
            || previous.policy_snapshot != record.policy_snapshot
            || previous.workspace != record.workspace
            || previous.delivery_target != record.delivery_target
            || previous
                .attempt
                .checked_add(1)
                .is_none_or(|attempt| attempt != record.attempt)
        {
            return Err(JournalError::InvalidTransition(format!(
                "durable child {} has an invalid retry sequence",
                record.child_id
            )));
        }
        let mut retry_ids = BTreeSet::from([record.child_id.to_string()]);
        let mut next_retry = Some(retry_of);
        while let Some(retry_id) = next_retry {
            if !retry_ids.insert(retry_id.to_string()) {
                return Err(JournalError::InvalidTransition(format!(
                    "durable child {} has a cyclic retry lineage",
                    record.child_id
                )));
            }
            let retry = state
                .children
                .get(retry_id.as_str())
                .and_then(|child| child.durable.as_ref())
                .ok_or_else(|| missing("durable retry child", retry_id.as_str()))?;
            next_retry = retry.retry_of.as_ref();
        }
    }

    match &record.delivery_target {
        Some(ChildDeliveryTarget::ParentTurn) if record.parent.turn_id.is_none() => {
            return Err(JournalError::InvalidTransition(format!(
                "durable child {} has no parent turn delivery target",
                record.child_id
            )));
        }
        Some(ChildDeliveryTarget::ParentChild { child_id })
            if state
                .children
                .get(child_id.as_str())
                .is_none_or(|child| child.durable.is_none()) =>
        {
            return Err(missing("durable delivery child", child_id.as_str()));
        }
        _ => {}
    }

    Ok(())
}

fn project_durable_child_compatibility(child: &mut ChildState) -> Result<(), JournalError> {
    let record = child.durable.as_ref().ok_or_else(|| {
        JournalError::InvalidTransition("durable child projection lost its record".to_owned())
    })?;
    child.result = record
        .result
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|source| JournalError::Json {
            context: "encoding durable child result evidence",
            source,
        })?;
    child.not_started_reason = None;
    child.effect = match record.status {
        DurableChildStatus::Prepared | DurableChildStatus::Queued | DurableChildStatus::Paused => {
            ExternalEffectState::Prepared
        }
        DurableChildStatus::Running | DurableChildStatus::RecoveryRequired => {
            ExternalEffectState::Unknown
        }
        DurableChildStatus::Succeeded => ExternalEffectState::Completed {
            outcome: CompletionOutcome::Succeeded,
        },
        DurableChildStatus::Failed => ExternalEffectState::Completed {
            outcome: CompletionOutcome::Failed {
                error: "durable child failed; inspect result digest".to_owned(),
            },
        },
        DurableChildStatus::Cancelled | DurableChildStatus::Expired => {
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Cancelled,
            }
        }
    };
    Ok(())
}

fn apply_event(state: &mut ReducedSessionState, event: &SessionEvent) -> Result<(), JournalError> {
    match event {
        SessionEvent::SessionImported {
            source_schema_version,
            session,
            session_digest,
        } => {
            let pristine = state.last_seq.is_none()
                && state.imported_baseline.is_none()
                && state.conversation.is_empty()
                && state.turns.is_empty()
                && state.streams.is_empty()
                && state.provider_attempts.is_empty()
                && state.tools.is_empty()
                && state.hook_phases.is_empty()
                && state.approvals.is_empty()
                && state.budgets.is_empty()
                && state.budget_event_ids.is_empty()
                && state.budget_authority.is_none()
                && state.checkpoints.is_empty()
                && state.children.is_empty()
                && state.deliveries.is_empty();
            if !pristine {
                return Err(JournalError::InvalidTransition(
                    "session import must be the first event".to_owned(),
                ));
            }
            let object = session.as_object().ok_or_else(|| {
                JournalError::InvalidTransition("imported session must be an object".to_owned())
            })?;
            let imported_id = object
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    JournalError::InvalidTransition(
                        "imported session id must be a string".to_owned(),
                    )
                })?;
            let expected_id = state.session_id.as_deref().unwrap_or_default();
            if imported_id != expected_id {
                return Err(JournalError::SessionMismatch {
                    expected: expected_id.to_owned(),
                    found: imported_id.to_owned(),
                });
            }
            match object
                .get("schema_version")
                .and_then(serde_json::Value::as_u64)
            {
                Some(version) if version == u64::from(*source_schema_version) => {}
                None if *source_schema_version == 0 => {}
                _ => {
                    return Err(JournalError::InvalidTransition(
                        "imported session schema version mismatch".to_owned(),
                    ));
                }
            }
            let messages = object
                .get("messages")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    JournalError::InvalidTransition(
                        "imported session messages must be an array".to_owned(),
                    )
                })?;
            if messages.iter().any(|message| !message.is_object()) {
                return Err(JournalError::InvalidTransition(
                    "every imported session message must be an object".to_owned(),
                ));
            }
            if state_payload_digest(session)? != *session_digest {
                return Err(JournalError::InvalidTransition(
                    "imported session digest mismatch".to_owned(),
                ));
            }
            state.conversation.clone_from(messages);
            state.imported_baseline = Some(ImportedSessionBaseline {
                source_schema_version: *source_schema_version,
                session_digest: session_digest.clone(),
                imported_message_count: messages.len() as u64,
                session: session.clone(),
            });
        }
        SessionEvent::ConversationMessageCommitted {
            turn_id,
            message_index,
            message,
            message_digest,
        } => {
            let turn = state
                .turns
                .get(turn_id)
                .ok_or_else(|| missing("turn", turn_id))?;
            if turn.completion.is_some() {
                return Err(JournalError::InvalidTransition(format!(
                    "turn {turn_id} is terminal"
                )));
            }
            let index = usize::try_from(*message_index).map_err(|_| {
                JournalError::InvalidTransition("conversation message index overflow".to_owned())
            })?;
            if index != state.conversation.len() {
                return Err(JournalError::InvalidTransition(format!(
                    "conversation expected index {}, found {message_index}",
                    state.conversation.len()
                )));
            }
            if !message.is_object() {
                return Err(JournalError::InvalidTransition(
                    "conversation message must be an object".to_owned(),
                ));
            }
            if state_payload_digest(message)? != *message_digest {
                return Err(JournalError::InvalidTransition(
                    "conversation message digest mismatch".to_owned(),
                ));
            }
            state.conversation.push(message.clone());
        }
        SessionEvent::ConversationStateCommitted {
            turn_id,
            messages,
            messages_digest,
        } => {
            let turn = state
                .turns
                .get(turn_id)
                .ok_or_else(|| missing("turn", turn_id))?;
            if turn.completion.is_some() {
                return Err(JournalError::InvalidTransition(format!(
                    "turn {turn_id} is terminal"
                )));
            }
            if messages.iter().any(|message| !message.is_object()) {
                return Err(JournalError::InvalidTransition(
                    "every conversation state message must be an object".to_owned(),
                ));
            }
            let payload = serde_json::Value::Array(messages.clone());
            if state_payload_digest(&payload)? != *messages_digest {
                return Err(JournalError::InvalidTransition(
                    "conversation state digest mismatch".to_owned(),
                ));
            }
            state.conversation.clone_from(messages);
        }
        SessionEvent::ConversationRecoveryCheckpointCommitted {
            turn_id,
            messages,
            messages_digest,
            checkpoint_id,
            checkpoint_state_digest,
            checkpoint,
        } => {
            commit_recovery_checkpoint(
                state,
                turn_id,
                messages,
                messages_digest,
                checkpoint_id,
                checkpoint_state_digest,
                checkpoint,
                &[],
            )?;
        }
        SessionEvent::ConversationRecoveryCheckpointCommittedV2 {
            turn_id,
            messages,
            messages_digest,
            checkpoint_id,
            checkpoint_state_digest,
            checkpoint,
            consumed_hook_phases,
        } => {
            commit_recovery_checkpoint(
                state,
                turn_id,
                messages,
                messages_digest,
                checkpoint_id,
                checkpoint_state_digest,
                checkpoint,
                consumed_hook_phases,
            )?;
        }
        SessionEvent::TurnStarted {
            turn_id,
            user_message,
        } => {
            if state.turns.contains_key(turn_id) {
                return Err(duplicate("turn", turn_id));
            }
            if let Some((active_turn_id, _)) = state
                .turns
                .iter()
                .find(|(_, turn)| turn.completion.is_none())
            {
                return Err(JournalError::InvalidTransition(format!(
                    "turn {active_turn_id} is still active"
                )));
            }
            state.turns.insert(
                turn_id.clone(),
                TurnState {
                    user_message: user_message.clone(),
                    completion: None,
                },
            );
        }
        SessionEvent::TurnCommitted {
            turn_id,
            assistant_message,
        } => {
            require_turn_descendants_terminal(state, turn_id)?;
            let turn = required_mut(&mut state.turns, "turn", turn_id)?;
            if turn.completion.is_some() {
                return Err(duplicate("turn completion", turn_id));
            }
            turn.completion = Some(TurnCompletion::Committed {
                assistant_message: assistant_message.clone(),
            });
        }
        SessionEvent::TurnFailed { turn_id, error } => {
            require_turn_descendants_terminal(state, turn_id)?;
            let turn = required_mut(&mut state.turns, "turn", turn_id)?;
            if turn.completion.is_some() {
                return Err(duplicate("turn completion", turn_id));
            }
            turn.completion = Some(TurnCompletion::Failed {
                error: error.clone(),
            });
        }
        SessionEvent::TurnCancelled { turn_id } => {
            require_turn_descendants_terminal(state, turn_id)?;
            let turn = required_mut(&mut state.turns, "turn", turn_id)?;
            if turn.completion.is_some() {
                return Err(duplicate("turn completion", turn_id));
            }
            turn.completion = Some(TurnCompletion::Cancelled);
        }
        SessionEvent::StreamStarted {
            stream_id,
            attempt_id,
        } => {
            if state.streams.contains_key(stream_id) {
                return Err(duplicate("stream", stream_id));
            }
            let attempt = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?;
            require_unknown(&attempt.effect, "provider attempt", attempt_id)?;
            if state
                .streams
                .values()
                .any(|stream| stream.attempt_id == *attempt_id)
            {
                return Err(JournalError::InvalidTransition(format!(
                    "provider attempt {attempt_id} already has a stream"
                )));
            }
            state.streams.insert(
                stream_id.clone(),
                StreamState {
                    attempt_id: attempt_id.clone(),
                    next_ordinal: 0,
                    batches: Vec::new(),
                    finished: false,
                },
            );
        }
        SessionEvent::StreamBatchCommitted {
            stream_id,
            ordinal,
            events,
        } => {
            let attempt_id = state
                .streams
                .get(stream_id)
                .ok_or_else(|| missing("stream", stream_id))?
                .attempt_id
                .clone();
            let attempt = state
                .provider_attempts
                .get(&attempt_id)
                .ok_or_else(|| missing("provider attempt", &attempt_id))?;
            require_unknown(&attempt.effect, "provider attempt", &attempt_id)?;
            let correlated = attempt.dispatch_id.is_some();
            let stream = required_mut(&mut state.streams, "stream", stream_id)?;
            if stream.finished || *ordinal != stream.next_ordinal {
                return Err(JournalError::InvalidTransition(format!(
                    "stream {stream_id} expected batch {}, found {ordinal}",
                    stream.next_ordinal
                )));
            }
            if events.is_empty() {
                return Err(JournalError::InvalidTransition(format!(
                    "stream {stream_id} batch {ordinal} is empty"
                )));
            }
            if correlated {
                let existing = provider_stream_events(stream);
                validate_appended_provider_events(&existing, events)?;
            }
            stream.batches.push(events.clone());
            stream.next_ordinal += 1;
        }
        SessionEvent::StreamFinished { stream_id } => {
            let attempt_id = state
                .streams
                .get(stream_id)
                .ok_or_else(|| missing("stream", stream_id))?
                .attempt_id
                .clone();
            let attempt = state
                .provider_attempts
                .get(&attempt_id)
                .ok_or_else(|| missing("provider attempt", &attempt_id))?;
            require_unknown(&attempt.effect, "provider attempt", &attempt_id)?;
            let correlated = attempt.dispatch_id.is_some();
            let stream = required_mut(&mut state.streams, "stream", stream_id)?;
            if stream.finished {
                return Err(duplicate("stream completion", stream_id));
            }
            if correlated {
                validate_finished_provider_events(&provider_stream_events(stream))?;
            }
            stream.finished = true;
        }
        SessionEvent::ProviderAttemptPrepared {
            attempt_id,
            turn_id,
            purpose,
            provider,
            model,
            request_digest,
        } => {
            require_active_turn(state, turn_id)?;
            if state.provider_attempts.contains_key(attempt_id) {
                return Err(duplicate("provider attempt", attempt_id));
            }
            state.provider_attempts.insert(
                attempt_id.clone(),
                ProviderAttemptState {
                    dispatch_id: None,
                    turn_id: turn_id.clone(),
                    purpose: *purpose,
                    provider: provider.clone(),
                    model: model.clone(),
                    request_digest: request_digest.clone(),
                    response_digest: None,
                    not_started_reason: None,
                    effect: ExternalEffectState::Prepared,
                },
            );
        }
        SessionEvent::ProviderAttemptPreparedV2 {
            attempt_id,
            dispatch_id,
            turn_id,
            purpose,
            provider,
            model,
            request_digest,
        } => {
            require_active_turn(state, turn_id)?;
            if dispatch_id.trim().is_empty() {
                return Err(JournalError::InvalidTransition(
                    "provider dispatch id must not be empty".to_owned(),
                ));
            }
            if state.provider_attempts.contains_key(attempt_id) {
                return Err(duplicate("provider attempt", attempt_id));
            }
            state.provider_attempts.insert(
                attempt_id.clone(),
                ProviderAttemptState {
                    dispatch_id: Some(dispatch_id.clone()),
                    turn_id: turn_id.clone(),
                    purpose: *purpose,
                    provider: provider.clone(),
                    model: model.clone(),
                    request_digest: request_digest.clone(),
                    response_digest: None,
                    not_started_reason: None,
                    effect: ExternalEffectState::Prepared,
                },
            );
        }
        SessionEvent::ProviderAttemptStarted { attempt_id } => {
            let turn_id = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let attempt =
                required_mut(&mut state.provider_attempts, "provider attempt", attempt_id)?;
            require_prepared(&attempt.effect, "provider attempt", attempt_id)?;
            attempt.effect = ExternalEffectState::Unknown;
        }
        SessionEvent::ProviderAttemptFinished {
            attempt_id,
            outcome,
            response_digest,
        } => {
            let attempt = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?;
            if attempt.dispatch_id.is_some() {
                return Err(JournalError::InvalidTransition(format!(
                    "recovery-correlated provider attempt {attempt_id} requires a V2 terminal receipt"
                )));
            }
            if matches!(outcome, CompletionOutcome::Succeeded) {
                let stream = state
                    .streams
                    .values()
                    .find(|stream| stream.attempt_id == *attempt_id)
                    .ok_or_else(|| {
                        JournalError::InvalidTransition(format!(
                            "successful provider attempt {attempt_id} has no stream"
                        ))
                    })?;
                if !stream.finished {
                    return Err(JournalError::InvalidTransition(format!(
                        "successful provider attempt {attempt_id} has an unfinished stream"
                    )));
                }
            }
            let attempt =
                required_mut(&mut state.provider_attempts, "provider attempt", attempt_id)?;
            require_unknown(&attempt.effect, "provider attempt", attempt_id)?;
            attempt.response_digest.clone_from(response_digest);
            attempt.effect = ExternalEffectState::Completed {
                outcome: outcome.clone(),
            };
        }
        SessionEvent::ProviderAttemptFinishedV2 {
            attempt_id,
            dispatch_id,
            outcome,
            response_digest,
        } => {
            let attempt = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?;
            require_correlated_dispatch(attempt, attempt_id, dispatch_id)?;
            require_unknown(&attempt.effect, "provider attempt", attempt_id)?;
            validate_correlated_terminal(state, attempt_id, outcome, response_digest.as_deref())?;
            let attempt =
                required_mut(&mut state.provider_attempts, "provider attempt", attempt_id)?;
            attempt.response_digest.clone_from(response_digest);
            attempt.effect = ExternalEffectState::Completed {
                outcome: outcome.clone(),
            };
        }
        SessionEvent::ProviderAttemptNotStarted { attempt_id, reason } => {
            let attempt = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?;
            if attempt.dispatch_id.is_some() {
                return Err(JournalError::InvalidTransition(format!(
                    "recovery-correlated provider attempt {attempt_id} requires a V2 not-started receipt"
                )));
            }
            let turn_id = attempt.turn_id.clone();
            require_active_turn(state, &turn_id)?;
            let attempt =
                required_mut(&mut state.provider_attempts, "provider attempt", attempt_id)?;
            require_prepared(&attempt.effect, "provider attempt", attempt_id)?;
            attempt.not_started_reason = Some(reason.clone());
            attempt.effect = ExternalEffectState::NotStarted;
        }
        SessionEvent::ProviderAttemptNotStartedV2 {
            attempt_id,
            dispatch_id,
            reason,
        } => {
            let attempt = state
                .provider_attempts
                .get(attempt_id)
                .ok_or_else(|| missing("provider attempt", attempt_id))?;
            require_correlated_dispatch(attempt, attempt_id, dispatch_id)?;
            let turn_id = attempt.turn_id.clone();
            require_active_turn(state, &turn_id)?;
            let attempt =
                required_mut(&mut state.provider_attempts, "provider attempt", attempt_id)?;
            require_prepared(&attempt.effect, "provider attempt", attempt_id)?;
            attempt.not_started_reason = Some(reason.clone());
            attempt.effect = ExternalEffectState::NotStarted;
        }
        SessionEvent::ToolIntentRecorded {
            tool_execution_id,
            provider_call_id,
            turn_id,
            ordinal,
            tool,
            requested_input,
            requested_input_digest,
            effective_input,
            effective_input_digest,
        } => {
            require_active_turn(state, turn_id)?;
            if state.tools.contains_key(tool_execution_id) {
                return Err(duplicate("tool execution", tool_execution_id));
            }
            state.tools.insert(
                tool_execution_id.clone(),
                ToolState {
                    idempotency_key: format!("legacy:{tool_execution_id}"),
                    retry_of: None,
                    provider_call_id: provider_call_id.clone(),
                    turn_id: turn_id.clone(),
                    ordinal: *ordinal,
                    tool: tool.clone(),
                    requested_input: StoredToolInput::Secured {
                        exact_digest: requested_input_digest.clone(),
                        envelope: requested_input.clone(),
                    },
                    requested_input_digest: requested_input_digest.clone(),
                    effective_input: StoredToolInput::Secured {
                        exact_digest: effective_input_digest.clone(),
                        envelope: effective_input.clone(),
                    },
                    effective_input_digest: effective_input_digest.clone(),
                    effect_contract: wcore_types::tool::ToolEffectContract::default(),
                    effect_receipt: None,
                    pre_hook_phase_id: None,
                    result: None,
                    not_started_reason: None,
                    resolution_source: None,
                    resolution_evidence: None,
                    effect: ToolEffectState::Prepared,
                },
            );
        }
        SessionEvent::ToolIntentRecordedV2 {
            tool_execution_id,
            idempotency_key,
            retry_of,
            provider_call_id,
            turn_id,
            ordinal,
            tool,
            requested_input,
            requested_input_digest,
            effective_input,
            effective_input_digest,
            effect_contract,
            effect_receipt,
            pre_hook_phase_id,
        } => {
            require_active_turn(state, turn_id)?;
            if state.tools.contains_key(tool_execution_id) {
                return Err(duplicate("tool execution", tool_execution_id));
            }
            if requested_input.exact_digest() != requested_input_digest {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} requested input record digest mismatch"
                )));
            }
            if effective_input.exact_digest() != effective_input_digest {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} effective input record digest mismatch"
                )));
            }
            if idempotency_key.is_empty() {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} has an empty idempotency key"
                )));
            }
            if effect_receipt.is_some() && effect_contract.reconciler.is_none() {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} has an effect receipt without a reconciler"
                )));
            }
            if let Some(phase_id) = pre_hook_phase_id {
                let phase = state
                    .hook_phases
                    .get(phase_id)
                    .ok_or_else(|| missing("hook phase", phase_id))?;
                let consumed_retry_binding = matches!(phase.state, HookPhaseState::Consumed { .. })
                    && retry_of.as_ref().is_some_and(|prior_id| {
                        state.tools.get(prior_id).is_some_and(|prior| {
                            prior.pre_hook_phase_id.as_deref() == Some(phase_id.as_str())
                                && prior.effective_input_digest == *effective_input_digest
                        })
                    });
                let effective_digest = match &phase.state {
                    HookPhaseState::Finished {
                        effective_input_digest: Some(digest),
                        ..
                    } => Some(digest),
                    HookPhaseState::Consumed { .. } if consumed_retry_binding => None,
                    _ => {
                        return Err(JournalError::InvalidTransition(format!(
                            "tool execution {tool_execution_id} pre-hook phase is not finished"
                        )));
                    }
                };
                if phase.phase != ToolHookPhase::PreToolUse
                    || phase.turn_id != *turn_id
                    || phase.provider_call_id != *provider_call_id
                    || phase.ordinal != *ordinal
                    || effective_digest.is_some_and(|digest| digest != effective_input_digest)
                {
                    return Err(JournalError::InvalidTransition(format!(
                        "tool execution {tool_execution_id} pre-hook binding mismatch"
                    )));
                }
            } else if state.hook_phases.values().any(|phase| {
                phase.turn_id == *turn_id
                    && phase.provider_call_id == *provider_call_id
                    && phase.ordinal == *ordinal
                    && phase.phase == ToolHookPhase::PreToolUse
            }) {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} omitted its pre-hook binding"
                )));
            }
            validate_tool_retry(
                state,
                retry_of.as_deref(),
                idempotency_key,
                provider_call_id,
                turn_id,
                *ordinal,
                tool,
                requested_input,
                requested_input_digest,
                effective_input,
                effective_input_digest,
                effect_contract,
                effect_receipt,
            )?;
            state.tools.insert(
                tool_execution_id.clone(),
                ToolState {
                    idempotency_key: idempotency_key.clone(),
                    retry_of: retry_of.clone(),
                    provider_call_id: provider_call_id.clone(),
                    turn_id: turn_id.clone(),
                    ordinal: *ordinal,
                    tool: tool.clone(),
                    requested_input: requested_input.clone(),
                    requested_input_digest: requested_input_digest.clone(),
                    effective_input: effective_input.clone(),
                    effective_input_digest: effective_input_digest.clone(),
                    effect_contract: effect_contract.clone(),
                    effect_receipt: effect_receipt.clone(),
                    pre_hook_phase_id: pre_hook_phase_id.clone(),
                    result: None,
                    not_started_reason: None,
                    resolution_source: None,
                    resolution_evidence: None,
                    effect: ToolEffectState::Prepared,
                },
            );
        }
        SessionEvent::ToolExecutionStarted { tool_execution_id } => {
            let tool_snapshot = state
                .tools
                .get(tool_execution_id)
                .ok_or_else(|| missing("tool execution", tool_execution_id))?;
            let turn_id = tool_snapshot.turn_id.clone();
            require_active_turn(state, &turn_id)?;
            if state.hook_phases.values().any(|phase| {
                phase.turn_id == tool_snapshot.turn_id
                    && phase.provider_call_id == tool_snapshot.provider_call_id
                    && phase.ordinal == tool_snapshot.ordinal
                    && phase.phase == ToolHookPhase::PreToolUse
                    && matches!(
                        phase.state,
                        HookPhaseState::Prepared | HookPhaseState::Started { .. }
                    )
            }) {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} cannot start while its pre-hook phase is active"
                )));
            }
            if tool_snapshot.pre_hook_phase_id.is_some()
                && !state.hook_phases.values().any(|phase| {
                    phase.tool_execution_id.as_deref() == Some(tool_execution_id)
                        && phase.phase == ToolHookPhase::PostToolUse
                        && matches!(phase.state, HookPhaseState::Prepared)
                })
            {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} lacks a prepared post-hook phase"
                )));
            }
            let tool = required_mut(&mut state.tools, "tool execution", tool_execution_id)?;
            require_tool_prepared(&tool.effect, tool_execution_id)?;
            validate_filesystem_start_receipt(tool, tool_execution_id)?;
            tool.effect = ToolEffectState::Running;
        }
        SessionEvent::ToolExecutionFinished {
            tool_execution_id,
            outcome,
            result,
        } => {
            let tool = required_mut(&mut state.tools, "tool execution", tool_execution_id)?;
            require_tool_running(&tool.effect, tool_execution_id)?;
            tool.result = Some(result.clone());
            tool.effect = match outcome {
                CompletionOutcome::Succeeded => ToolEffectState::Succeeded,
                CompletionOutcome::Failed { error } => ToolEffectState::Failed {
                    error: error.clone(),
                },
                CompletionOutcome::Cancelled => {
                    return Err(JournalError::InvalidTransition(format!(
                        "tool execution {tool_execution_id} cancellation must be recorded as unknown"
                    )));
                }
            };
        }
        SessionEvent::ToolExecutionNotStarted {
            tool_execution_id,
            reason,
        } => {
            let turn_id = state
                .tools
                .get(tool_execution_id)
                .ok_or_else(|| missing("tool execution", tool_execution_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let tool = required_mut(&mut state.tools, "tool execution", tool_execution_id)?;
            require_tool_prepared(&tool.effect, tool_execution_id)?;
            tool.not_started_reason = Some(reason.clone());
            tool.effect = ToolEffectState::NotStarted;
        }
        SessionEvent::ToolExecutionUnknown {
            tool_execution_id,
            reason,
            evidence,
        } => {
            let tool = required_mut(&mut state.tools, "tool execution", tool_execution_id)?;
            require_tool_running(&tool.effect, tool_execution_id)?;
            tool.effect = ToolEffectState::Unknown {
                reason: reason.clone(),
                evidence: evidence.clone(),
            };
        }
        SessionEvent::ToolExecutionResolved {
            tool_execution_id,
            resolution,
            source,
            evidence,
        } => {
            let tool = required_mut(&mut state.tools, "tool execution", tool_execution_id)?;
            require_tool_unknown(&tool.effect, tool_execution_id)?;
            tool.resolution_source = Some(source.clone());
            tool.resolution_evidence = Some(evidence.clone());
            match resolution {
                ToolResolution::Succeeded { result } => {
                    tool.result = Some(result.clone());
                    tool.effect = ToolEffectState::Succeeded;
                }
                ToolResolution::Failed { error, result } => {
                    tool.result.clone_from(result);
                    tool.effect = ToolEffectState::Failed {
                        error: error.clone(),
                    };
                }
                ToolResolution::NotStarted { reason } => {
                    tool.not_started_reason = Some(reason.clone());
                    tool.effect = ToolEffectState::NotStarted;
                }
            }
        }
        SessionEvent::HookPhasePrepared {
            hook_phase_id,
            lifecycle_version,
            turn_id,
            provider_call_id,
            ordinal,
            phase,
            tool_execution_id,
            input_digest,
            hook_authority_digest,
            hook_manifest_digest,
            hook_slots,
        } => {
            require_active_turn(state, turn_id)?;
            if state.hook_phases.contains_key(hook_phase_id) {
                return Err(duplicate("hook phase", hook_phase_id));
            }
            if *lifecycle_version != HOOK_PHASE_LIFECYCLE_VERSION
                || provider_call_id.is_empty()
                || hook_slots.is_empty()
                || !valid_sha256_digest(input_digest)
                || !valid_sha256_digest(hook_authority_digest)
                || !valid_sha256_digest(hook_manifest_digest)
                || !valid_hook_manifest(hook_slots)
                || state_payload_digest(&serde_json::to_value(hook_slots).map_err(|source| {
                    JournalError::Json {
                        context: "encoding hook manifest",
                        source,
                    }
                })?)?
                    != *hook_manifest_digest
            {
                return Err(JournalError::InvalidTransition(format!(
                    "hook phase {hook_phase_id} has invalid version or authority binding"
                )));
            }
            if state.hook_phases.values().any(|existing| {
                existing.turn_id == *turn_id
                    && existing.provider_call_id == *provider_call_id
                    && existing.ordinal == *ordinal
                    && existing.phase == *phase
                    && (*phase == ToolHookPhase::PreToolUse
                        || existing.tool_execution_id == *tool_execution_id)
            }) {
                return Err(JournalError::InvalidTransition(format!(
                    "duplicate hook phase authority for {turn_id}/{provider_call_id}/{ordinal}/{phase:?}"
                )));
            }
            match (phase, tool_execution_id.as_deref()) {
                (ToolHookPhase::PreToolUse, None) => {}
                (ToolHookPhase::PostToolUse, Some(tool_execution_id)) => {
                    let tool = state
                        .tools
                        .get(tool_execution_id)
                        .ok_or_else(|| missing("tool execution", tool_execution_id))?;
                    if tool.turn_id != *turn_id
                        || tool.provider_call_id != *provider_call_id
                        || tool.ordinal != *ordinal
                        || tool.effective_input_digest != *input_digest
                        || !matches!(tool.effect, ToolEffectState::Prepared)
                    {
                        return Err(JournalError::InvalidTransition(format!(
                            "post hook phase {hook_phase_id} is not bound to the prepared tool"
                        )));
                    }
                }
                _ => {
                    return Err(JournalError::InvalidTransition(format!(
                        "hook phase {hook_phase_id} has an invalid tool binding"
                    )));
                }
            }
            state.hook_phases.insert(
                hook_phase_id.clone(),
                HookPhaseExecutionState {
                    lifecycle_version: *lifecycle_version,
                    turn_id: turn_id.clone(),
                    provider_call_id: provider_call_id.clone(),
                    ordinal: *ordinal,
                    phase: *phase,
                    tool_execution_id: tool_execution_id.clone(),
                    input_digest: input_digest.clone(),
                    hook_authority_digest: hook_authority_digest.clone(),
                    hook_manifest_digest: hook_manifest_digest.clone(),
                    hook_slots: hook_slots.clone(),
                    state: HookPhaseState::Prepared,
                },
            );
        }
        SessionEvent::HookPhaseStarted {
            hook_phase_id,
            result_digest,
        } => {
            let phase = state
                .hook_phases
                .get(hook_phase_id)
                .ok_or_else(|| missing("hook phase", hook_phase_id))?;
            if !matches!(phase.state, HookPhaseState::Prepared) {
                return Err(JournalError::InvalidTransition(format!(
                    "hook phase {hook_phase_id} is not prepared"
                )));
            }
            match phase.phase {
                ToolHookPhase::PreToolUse if result_digest.is_none() => {}
                ToolHookPhase::PostToolUse => {
                    let digest = result_digest
                        .as_deref()
                        .filter(|digest| valid_sha256_digest(digest))
                        .ok_or_else(|| {
                            JournalError::InvalidTransition(format!(
                                "post hook phase {hook_phase_id} lacks a valid result digest"
                            ))
                        })?;
                    let tool_id = phase.tool_execution_id.as_deref().ok_or_else(|| {
                        JournalError::InvalidTransition(format!(
                            "post hook phase {hook_phase_id} lacks a tool binding"
                        ))
                    })?;
                    let tool = state
                        .tools
                        .get(tool_id)
                        .ok_or_else(|| missing("tool execution", tool_id))?;
                    let result = tool.result.as_ref().ok_or_else(|| {
                        JournalError::InvalidTransition(format!(
                            "post hook phase {hook_phase_id} tool result is not durable"
                        ))
                    })?;
                    if !matches!(
                        tool.effect,
                        ToolEffectState::Succeeded | ToolEffectState::Failed { .. }
                    ) || state_payload_digest(result)? != digest
                    {
                        return Err(JournalError::InvalidTransition(format!(
                            "post hook phase {hook_phase_id} result binding mismatch"
                        )));
                    }
                }
                _ => {
                    return Err(JournalError::InvalidTransition(format!(
                        "pre hook phase {hook_phase_id} must not bind a result"
                    )));
                }
            }
            state.hook_phases.get_mut(hook_phase_id).unwrap().state = HookPhaseState::Started {
                result_digest: result_digest.clone(),
            };
        }
        SessionEvent::HookPhaseFinished {
            hook_phase_id,
            result_digest,
            effective_input_digest,
            outcome_digest,
            slot_receipts_digest,
            slot_receipts,
        } => {
            let phase = state
                .hook_phases
                .get(hook_phase_id)
                .ok_or_else(|| missing("hook phase", hook_phase_id))?;
            let HookPhaseState::Started {
                result_digest: started_result_digest,
            } = &phase.state
            else {
                return Err(JournalError::InvalidTransition(format!(
                    "hook phase {hook_phase_id} is not started"
                )));
            };
            let input_binding_valid = match phase.phase {
                ToolHookPhase::PreToolUse => effective_input_digest
                    .as_deref()
                    .is_some_and(valid_sha256_digest),
                ToolHookPhase::PostToolUse => effective_input_digest.is_none(),
            };
            if started_result_digest != result_digest
                || !input_binding_valid
                || !valid_sha256_digest(outcome_digest)
                || !valid_sha256_digest(slot_receipts_digest)
                || !valid_hook_receipts(&phase.hook_slots, slot_receipts)
                || state_payload_digest(&serde_json::to_value(slot_receipts).map_err(
                    |source| JournalError::Json {
                        context: "encoding hook slot receipts",
                        source,
                    },
                )?)? != *slot_receipts_digest
            {
                return Err(JournalError::InvalidTransition(format!(
                    "hook phase {hook_phase_id} has an invalid finished receipt"
                )));
            }
            state.hook_phases.get_mut(hook_phase_id).unwrap().state = HookPhaseState::Finished {
                result_digest: result_digest.clone(),
                effective_input_digest: effective_input_digest.clone(),
                outcome_digest: outcome_digest.clone(),
                slot_receipts_digest: slot_receipts_digest.clone(),
                slot_receipts: slot_receipts.clone(),
            };
        }
        SessionEvent::HookPhaseNotStarted {
            hook_phase_id,
            reason,
        } => {
            let phase = required_mut(&mut state.hook_phases, "hook phase", hook_phase_id)?;
            if !matches!(phase.state, HookPhaseState::Prepared) {
                return Err(JournalError::InvalidTransition(format!(
                    "hook phase {hook_phase_id} is not prepared"
                )));
            }
            phase.state = HookPhaseState::NotStarted {
                reason: reason.clone(),
            };
        }
        SessionEvent::HookPhaseNotApplicable { hook_phase_id } => {
            let phase = state
                .hook_phases
                .get(hook_phase_id)
                .ok_or_else(|| missing("hook phase", hook_phase_id))?;
            if phase.phase != ToolHookPhase::PostToolUse
                || !matches!(phase.state, HookPhaseState::Prepared)
                || phase
                    .tool_execution_id
                    .as_ref()
                    .and_then(|tool_id| state.tools.get(tool_id))
                    .is_none_or(|tool| !matches!(tool.effect, ToolEffectState::NotStarted))
            {
                return Err(JournalError::InvalidTransition(format!(
                    "hook phase {hook_phase_id} is not a non-started post phase"
                )));
            }
            state.hook_phases.get_mut(hook_phase_id).unwrap().state = HookPhaseState::NotApplicable;
        }
        SessionEvent::HookPhaseAbandonedUnknown { hook_phase_id } => {
            let phase = required_mut(&mut state.hook_phases, "hook phase", hook_phase_id)?;
            if !matches!(phase.state, HookPhaseState::Started { .. }) {
                return Err(JournalError::InvalidTransition(format!(
                    "hook phase {hook_phase_id} is not started and unknown"
                )));
            }
            phase.state = HookPhaseState::AbandonedUnknown;
        }
        SessionEvent::ApprovalRequested {
            approval_id,
            origin,
            intent_digest,
        } => {
            require_approval_origin_prepared(state, origin)?;
            if state.approvals.contains_key(approval_id) {
                return Err(duplicate("approval", approval_id));
            }
            if state
                .approvals
                .values()
                .any(|approval| approval.origin == *origin && approval.resolution.is_none())
            {
                return Err(JournalError::InvalidTransition(format!(
                    "approval origin {origin:?} already has a pending approval"
                )));
            }
            state.approvals.insert(
                approval_id.clone(),
                ApprovalState {
                    origin: origin.clone(),
                    intent_digest: intent_digest.clone(),
                    resolution: None,
                },
            );
        }
        SessionEvent::ApprovalResolved {
            approval_id,
            resolution,
        } => {
            let approval = state
                .approvals
                .get(approval_id)
                .ok_or_else(|| missing("approval", approval_id))?;
            if approval.resolution.is_some() {
                return Err(duplicate("approval resolution", approval_id));
            }
            let origin = approval.origin.clone();
            require_approval_origin_prepared(state, &origin)?;
            let approval = required_mut(&mut state.approvals, "approval", approval_id)?;
            approval.resolution = Some(resolution.clone());
        }
        SessionEvent::BudgetReserved {
            event_id,
            reservation_id,
            owner,
            purpose,
            amount,
        } => {
            if state.budget_event_ids.contains_key(event_id) {
                return Err(duplicate("budget event", event_id));
            }
            if state.budgets.contains_key(reservation_id) {
                return Err(duplicate("budget reservation", reservation_id));
            }
            require_budget_owner_exists(state, owner)?;
            if amount.value == 0 {
                return Err(JournalError::InvalidTransition(format!(
                    "budget reservation {reservation_id} amount must be nonzero"
                )));
            }
            state.budgets.insert(
                reservation_id.clone(),
                BudgetState {
                    owner: owner.clone(),
                    purpose: *purpose,
                    reserved: *amount,
                    used: None,
                    released: false,
                    event_ids: vec![event_id.clone()],
                },
            );
            state
                .budget_event_ids
                .insert(event_id.clone(), reservation_id.clone());
        }
        SessionEvent::BudgetSettled {
            event_id,
            reservation_id,
            amount,
        } => {
            if state.budget_event_ids.contains_key(event_id) {
                return Err(duplicate("budget event", event_id));
            }
            let budget = required_mut(&mut state.budgets, "budget reservation", reservation_id)?;
            if budget.used.is_some()
                || budget.released
                || amount.unit != budget.reserved.unit
                || amount.value > budget.reserved.value
            {
                return Err(duplicate("budget settlement", reservation_id));
            }
            budget.used = Some(*amount);
            budget.event_ids.push(event_id.clone());
            state
                .budget_event_ids
                .insert(event_id.clone(), reservation_id.clone());
        }
        SessionEvent::BudgetReleased {
            event_id,
            reservation_id,
        } => {
            if state.budget_event_ids.contains_key(event_id) {
                return Err(duplicate("budget event", event_id));
            }
            let budget = required_mut(&mut state.budgets, "budget reservation", reservation_id)?;
            if budget.used.is_some() || budget.released {
                return Err(duplicate("budget release", reservation_id));
            }
            budget.released = true;
            budget.event_ids.push(event_id.clone());
            state
                .budget_event_ids
                .insert(event_id.clone(), reservation_id.clone());
        }
        SessionEvent::BudgetAuthorityCommitted { authority } => {
            validate_budget_authority(state, authority)?;
            state.budget_authority = Some(authority.clone());
        }
        SessionEvent::CheckpointCommitted {
            checkpoint_id,
            purpose,
            origin,
            state_digest,
            state: checkpoint,
        } => {
            if let CheckpointOrigin::Turn { turn_id } = origin
                && !state.turns.contains_key(turn_id)
            {
                return Err(missing("turn", turn_id));
            }
            if state_payload_digest(checkpoint)? != *state_digest {
                return Err(JournalError::InvalidTransition(format!(
                    "checkpoint {checkpoint_id} state digest mismatch"
                )));
            }
            if state
                .checkpoints
                .insert(
                    checkpoint_id.clone(),
                    CheckpointState {
                        purpose: *purpose,
                        origin: origin.clone(),
                        state_digest: state_digest.clone(),
                        state: checkpoint.clone(),
                    },
                )
                .is_some()
            {
                return Err(duplicate("checkpoint", checkpoint_id));
            }
        }
        SessionEvent::ChildPrepared {
            child_id,
            turn_id,
            request,
        } => {
            require_active_turn(state, turn_id)?;
            if state.children.contains_key(child_id) {
                return Err(duplicate("child", child_id));
            }
            state.children.insert(
                child_id.clone(),
                ChildState {
                    turn_id: turn_id.clone(),
                    request: request.clone(),
                    result: None,
                    not_started_reason: None,
                    effect: ExternalEffectState::Prepared,
                    durable: None,
                    durable_declaration_digest: None,
                },
            );
        }
        SessionEvent::ChildStarted { child_id } => {
            let turn_id = state
                .children
                .get(child_id)
                .ok_or_else(|| missing("child", child_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let child = required_mut(&mut state.children, "child", child_id)?;
            if child.durable.is_some() {
                return Err(JournalError::InvalidTransition(format!(
                    "durable child {child_id} requires V2 transitions"
                )));
            }
            require_prepared(&child.effect, "child", child_id)?;
            child.effect = ExternalEffectState::Unknown;
        }
        SessionEvent::ChildFinished {
            child_id,
            outcome,
            result,
        } => {
            let child = required_mut(&mut state.children, "child", child_id)?;
            if child.durable.is_some() {
                return Err(JournalError::InvalidTransition(format!(
                    "durable child {child_id} requires V2 transitions"
                )));
            }
            require_unknown(&child.effect, "child", child_id)?;
            child.result = Some(result.clone());
            child.effect = ExternalEffectState::Completed {
                outcome: outcome.clone(),
            };
        }
        SessionEvent::ChildNotStarted { child_id, reason } => {
            let turn_id = state
                .children
                .get(child_id)
                .ok_or_else(|| missing("child", child_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let child = required_mut(&mut state.children, "child", child_id)?;
            if child.durable.is_some() {
                return Err(JournalError::InvalidTransition(format!(
                    "durable child {child_id} requires V2 transitions"
                )));
            }
            require_prepared(&child.effect, "child", child_id)?;
            child.not_started_reason = Some(reason.clone());
            child.effect = ExternalEffectState::NotStarted;
        }
        SessionEvent::ChildDeclaredV2 { record } => {
            record
                .validate_declaration()
                .map_err(|error| JournalError::InvalidTransition(error.to_string()))?;
            let child_id = record.child_id.to_string();
            let declaration_value =
                serde_json::to_value(record).map_err(|source| JournalError::Json {
                    context: "encoding durable child declaration",
                    source,
                })?;
            let declaration_digest = state_payload_digest(&declaration_value)?;
            if let Some(existing) = state.children.get(&child_id) {
                if existing
                    .durable
                    .as_ref()
                    .is_none_or(|durable| durable.declaration_id != record.declaration_id)
                    || existing.durable_declaration_digest.as_deref()
                        != Some(declaration_digest.as_str())
                {
                    return Err(JournalError::InvalidTransition(format!(
                        "durable child {child_id} declaration conflicts with committed authority"
                    )));
                }
            } else {
                if let Some((other_child_id, _)) = state.children.iter().find(|(_, child)| {
                    child
                        .durable
                        .as_ref()
                        .is_some_and(|durable| durable.declaration_id == record.declaration_id)
                }) {
                    return Err(JournalError::InvalidTransition(format!(
                        "durable declaration {} is already bound to child {other_child_id}",
                        record.declaration_id
                    )));
                }
                let journal_session_id = state.session_id.clone().ok_or_else(|| {
                    JournalError::InvalidTransition(
                        "durable child declaration has no journal session authority".to_owned(),
                    )
                })?;
                validate_durable_child_lineage(state, record, &journal_session_id)?;
                let request =
                    serde_json::to_value(&record.request).map_err(|source| JournalError::Json {
                        context: "encoding durable child request evidence",
                        source,
                    })?;
                state.children.insert(
                    child_id,
                    ChildState {
                        turn_id: record.parent.turn_id.clone().unwrap_or_default(),
                        request,
                        result: None,
                        not_started_reason: None,
                        effect: ExternalEffectState::Prepared,
                        durable: Some(record.clone()),
                        durable_declaration_digest: Some(declaration_digest),
                    },
                );
            }
        }
        SessionEvent::ChildTransitionedV2 {
            child_id,
            event_id,
            expected_revision,
            at_unix_ms,
            transition,
        } => {
            let child = required_mut(&mut state.children, "durable child", child_id.as_str())?;
            let record = child.durable.as_mut().ok_or_else(|| {
                JournalError::InvalidTransition(format!(
                    "legacy child {child_id} cannot accept a V2 transition"
                ))
            })?;
            if apply_transition(
                record,
                event_id,
                *expected_revision,
                *at_unix_ms,
                transition,
            )? == TransitionDisposition::Applied
            {
                project_durable_child_compatibility(child)?;
            }
        }
        SessionEvent::DeliveryPrepared {
            delivery_id,
            origin,
            destination,
            payload,
        } => {
            require_delivery_origin_active(state, origin)?;
            if state.deliveries.contains_key(delivery_id) {
                return Err(duplicate("delivery", delivery_id));
            }
            state.deliveries.insert(
                delivery_id.clone(),
                DeliveryState {
                    origin: origin.clone(),
                    destination: destination.clone(),
                    payload: payload.clone(),
                    completion: None,
                    not_started_reason: None,
                    effect: ExternalEffectState::Prepared,
                },
            );
        }
        SessionEvent::DeliveryStarted { delivery_id } => {
            let origin = state
                .deliveries
                .get(delivery_id)
                .ok_or_else(|| missing("delivery", delivery_id))?
                .origin
                .clone();
            require_delivery_origin_active(state, &origin)?;
            let delivery = required_mut(&mut state.deliveries, "delivery", delivery_id)?;
            require_prepared(&delivery.effect, "delivery", delivery_id)?;
            delivery.effect = ExternalEffectState::Unknown;
        }
        SessionEvent::DeliveryNotStarted {
            delivery_id,
            reason,
        } => {
            let origin = state
                .deliveries
                .get(delivery_id)
                .ok_or_else(|| missing("delivery", delivery_id))?
                .origin
                .clone();
            require_delivery_origin_active(state, &origin)?;
            let delivery = required_mut(&mut state.deliveries, "delivery", delivery_id)?;
            require_prepared(&delivery.effect, "delivery", delivery_id)?;
            delivery.not_started_reason = Some(reason.clone());
            delivery.effect = ExternalEffectState::NotStarted;
        }
        SessionEvent::DeliveryFinished {
            delivery_id,
            completion,
        } => {
            let delivery = required_mut(&mut state.deliveries, "delivery", delivery_id)?;
            require_unknown(&delivery.effect, "delivery", delivery_id)?;
            if delivery.completion.is_some() {
                return Err(duplicate("delivery completion", delivery_id));
            }
            delivery.completion = Some(completion.clone());
            if let DeliveryCompletion::Confirmed { outcome, .. } = completion {
                delivery.effect = ExternalEffectState::Completed {
                    outcome: outcome.clone(),
                };
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tool_effect_transition_properties {
    use super::*;

    #[derive(Clone, Copy, Debug)]
    enum StateKind {
        Prepared,
        Running,
        Succeeded,
        Failed,
        NotStarted,
        Unknown,
    }

    #[derive(Clone, Copy, Debug)]
    enum EventKind {
        Start,
        FinishSucceeded,
        FinishFailed,
        FinishCancelled,
        NotStarted,
        Unknown,
        ResolveSucceeded,
        ResolveFailed,
        ResolveNotStarted,
    }

    fn effect(state: StateKind) -> ToolEffectState {
        match state {
            StateKind::Prepared => ToolEffectState::Prepared,
            StateKind::Running => ToolEffectState::Running,
            StateKind::Succeeded => ToolEffectState::Succeeded,
            StateKind::Failed => ToolEffectState::Failed {
                error: "terminal failure".into(),
            },
            StateKind::NotStarted => ToolEffectState::NotStarted,
            StateKind::Unknown => ToolEffectState::Unknown {
                reason: ToolUnknownReason::Interrupted,
                evidence: serde_json::json!({"fixture": true}),
            },
        }
    }

    fn state_with_tool(state: StateKind) -> ReducedSessionState {
        let mut reduced = ReducedSessionState::default();
        reduced.turns.insert(
            "turn".into(),
            TurnState {
                user_message: "test".into(),
                completion: None,
            },
        );
        reduced.tools.insert(
            "tool".into(),
            ToolState {
                idempotency_key: "stable-key".into(),
                retry_of: None,
                provider_call_id: "provider-call".into(),
                turn_id: "turn".into(),
                ordinal: 0,
                tool: "Opaque".into(),
                requested_input: StoredToolInput::redacted("requested"),
                requested_input_digest: "requested".into(),
                effective_input: StoredToolInput::redacted("effective"),
                effective_input_digest: "effective".into(),
                effect_contract: wcore_types::tool::ToolEffectContract::default(),
                effect_receipt: None,
                pre_hook_phase_id: None,
                result: None,
                not_started_reason: None,
                resolution_source: None,
                resolution_evidence: None,
                effect: effect(state),
            },
        );
        reduced
    }

    fn event(event: EventKind) -> SessionEvent {
        match event {
            EventKind::Start => SessionEvent::ToolExecutionStarted {
                tool_execution_id: "tool".into(),
            },
            EventKind::FinishSucceeded => SessionEvent::ToolExecutionFinished {
                tool_execution_id: "tool".into(),
                outcome: CompletionOutcome::Succeeded,
                result: serde_json::json!({"ok": true}),
            },
            EventKind::FinishFailed => SessionEvent::ToolExecutionFinished {
                tool_execution_id: "tool".into(),
                outcome: CompletionOutcome::Failed {
                    error: "failed".into(),
                },
                result: serde_json::json!({"ok": false}),
            },
            EventKind::FinishCancelled => SessionEvent::ToolExecutionFinished {
                tool_execution_id: "tool".into(),
                outcome: CompletionOutcome::Cancelled,
                result: serde_json::json!({"cancelled": true}),
            },
            EventKind::NotStarted => SessionEvent::ToolExecutionNotStarted {
                tool_execution_id: "tool".into(),
                reason: ToolNotStartedReason::Cancelled {
                    reason: "before dispatch".into(),
                },
            },
            EventKind::Unknown => SessionEvent::ToolExecutionUnknown {
                tool_execution_id: "tool".into(),
                reason: ToolUnknownReason::Interrupted,
                evidence: serde_json::json!({"cut": "running"}),
            },
            EventKind::ResolveSucceeded => SessionEvent::ToolExecutionResolved {
                tool_execution_id: "tool".into(),
                resolution: ToolResolution::Succeeded {
                    result: serde_json::json!({"reconciled": true}),
                },
                source: ToolResolutionSource::Operator {
                    operator_id: "operator".into(),
                },
                evidence: serde_json::json!({"ticket": "T-1"}),
            },
            EventKind::ResolveFailed => SessionEvent::ToolExecutionResolved {
                tool_execution_id: "tool".into(),
                resolution: ToolResolution::Failed {
                    error: "authoritative failure".into(),
                    result: None,
                },
                source: ToolResolutionSource::Operator {
                    operator_id: "operator".into(),
                },
                evidence: serde_json::json!({"ticket": "T-1"}),
            },
            EventKind::ResolveNotStarted => SessionEvent::ToolExecutionResolved {
                tool_execution_id: "tool".into(),
                resolution: ToolResolution::NotStarted {
                    reason: ToolNotStartedReason::Cancelled {
                        reason: "not dispatched".into(),
                    },
                },
                source: ToolResolutionSource::Operator {
                    operator_id: "operator".into(),
                },
                evidence: serde_json::json!({"ticket": "T-1"}),
            },
        }
    }

    fn allowed(state: StateKind, event: EventKind) -> bool {
        matches!(
            (state, event),
            (
                StateKind::Prepared,
                EventKind::Start | EventKind::NotStarted
            ) | (
                StateKind::Running,
                EventKind::FinishSucceeded | EventKind::FinishFailed | EventKind::Unknown
            ) | (
                StateKind::Unknown,
                EventKind::ResolveSucceeded
                    | EventKind::ResolveFailed
                    | EventKind::ResolveNotStarted
            )
        )
    }

    #[test]
    fn every_tool_effect_state_event_pair_obeys_the_locked_transition_property() {
        let states = [
            StateKind::Prepared,
            StateKind::Running,
            StateKind::Succeeded,
            StateKind::Failed,
            StateKind::NotStarted,
            StateKind::Unknown,
        ];
        let events = [
            EventKind::Start,
            EventKind::FinishSucceeded,
            EventKind::FinishFailed,
            EventKind::FinishCancelled,
            EventKind::NotStarted,
            EventKind::Unknown,
            EventKind::ResolveSucceeded,
            EventKind::ResolveFailed,
            EventKind::ResolveNotStarted,
        ];

        for state in states {
            for event_kind in events {
                let mut reduced = state_with_tool(state);
                let result = apply_event(&mut reduced, &event(event_kind));
                assert_eq!(
                    result.is_ok(),
                    allowed(state, event_kind),
                    "unexpected transition disposition for {state:?} + {event_kind:?}: {result:?}"
                );
            }
        }
    }
}

#[cfg(test)]
mod hook_phase_transition_tests {
    use super::*;

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn active_turn_state() -> ReducedSessionState {
        let mut state = ReducedSessionState::default();
        state.turns.insert(
            "turn".into(),
            TurnState {
                user_message: "test".into(),
                completion: None,
            },
        );
        state
    }

    fn prepare_pre() -> SessionEvent {
        let slots = manifest();
        SessionEvent::HookPhasePrepared {
            hook_phase_id: "hook-pre".into(),
            lifecycle_version: HOOK_PHASE_LIFECYCLE_VERSION,
            turn_id: "turn".into(),
            provider_call_id: "call".into(),
            ordinal: 0,
            phase: ToolHookPhase::PreToolUse,
            tool_execution_id: None,
            input_digest: DIGEST.into(),
            hook_authority_digest: DIGEST.into(),
            hook_manifest_digest: state_payload_digest(&serde_json::to_value(&slots).unwrap())
                .unwrap(),
            hook_slots: slots,
        }
    }

    fn manifest() -> Vec<HookManifestSlot> {
        (0..2)
            .map(|ordinal| HookManifestSlot {
                ordinal,
                slot_id: format!("slot-{ordinal}"),
                source: HookSlotSource::Rust,
                descriptor_digest: DIGEST.into(),
            })
            .collect()
    }

    fn receipts() -> Vec<HookSlotReceipt> {
        manifest()
            .into_iter()
            .map(|slot| HookSlotReceipt {
                ordinal: slot.ordinal,
                slot_id: slot.slot_id,
                descriptor_digest: slot.descriptor_digest,
                status: HookSlotTerminalStatus::Completed,
            })
            .collect()
    }

    fn finish_pre(state: &mut ReducedSessionState) {
        apply_event(state, &prepare_pre()).unwrap();
        apply_event(
            state,
            &SessionEvent::HookPhaseStarted {
                hook_phase_id: "hook-pre".into(),
                result_digest: None,
            },
        )
        .unwrap();
        let receipts = receipts();
        apply_event(
            state,
            &SessionEvent::HookPhaseFinished {
                hook_phase_id: "hook-pre".into(),
                result_digest: None,
                effective_input_digest: Some(DIGEST.into()),
                outcome_digest: DIGEST.into(),
                slot_receipts_digest: state_payload_digest(
                    &serde_json::to_value(&receipts).unwrap(),
                )
                .unwrap(),
                slot_receipts: receipts,
            },
        )
        .unwrap();
    }

    fn consume_pre(state: &mut ReducedSessionState) {
        let messages = vec![serde_json::json!({"role": "user", "content": "test"})];
        let messages_digest =
            state_payload_digest(&serde_json::Value::Array(messages.clone())).unwrap();
        let checkpoint = serde_json::json!({"version": 1});
        let checkpoint_state_digest = state_payload_digest(&checkpoint).unwrap();
        apply_event(
            state,
            &SessionEvent::ConversationRecoveryCheckpointCommittedV2 {
                turn_id: "turn".into(),
                messages,
                messages_digest,
                checkpoint_id: "checkpoint".into(),
                checkpoint_state_digest,
                checkpoint,
                consumed_hook_phases: vec![HookPhaseConsumption {
                    hook_phase_id: "hook-pre".into(),
                    outcome_digest: DIGEST.into(),
                }],
            },
        )
        .unwrap();
    }

    fn tool_intent(
        tool_execution_id: &str,
        retry_of: Option<&str>,
        pre_hook_phase_id: Option<&str>,
    ) -> SessionEvent {
        SessionEvent::ToolIntentRecordedV2 {
            tool_execution_id: tool_execution_id.into(),
            idempotency_key: "stable-tool-key".into(),
            retry_of: retry_of.map(str::to_owned),
            provider_call_id: "call".into(),
            turn_id: "turn".into(),
            ordinal: 0,
            tool: "Opaque".into(),
            requested_input: StoredToolInput::redacted(DIGEST),
            requested_input_digest: DIGEST.into(),
            effective_input: StoredToolInput::redacted(DIGEST),
            effective_input_digest: DIGEST.into(),
            effect_contract: wcore_types::tool::ToolEffectContract::default(),
            effect_receipt: None,
            pre_hook_phase_id: pre_hook_phase_id.map(str::to_owned),
        }
    }

    fn prepare_post(hook_phase_id: &str, tool_execution_id: &str) -> SessionEvent {
        let slots = manifest();
        SessionEvent::HookPhasePrepared {
            hook_phase_id: hook_phase_id.into(),
            lifecycle_version: HOOK_PHASE_LIFECYCLE_VERSION,
            turn_id: "turn".into(),
            provider_call_id: "call".into(),
            ordinal: 0,
            phase: ToolHookPhase::PostToolUse,
            tool_execution_id: Some(tool_execution_id.into()),
            input_digest: DIGEST.into(),
            hook_authority_digest: DIGEST.into(),
            hook_manifest_digest: state_payload_digest(&serde_json::to_value(&slots).unwrap())
                .unwrap(),
            hook_slots: slots,
        }
    }

    #[test]
    fn finished_hook_outcome_is_consumed_atomically_with_checkpoint() {
        let mut state = active_turn_state();
        apply_event(&mut state, &prepare_pre()).unwrap();
        let receipts = receipts();
        apply_event(
            &mut state,
            &SessionEvent::HookPhaseStarted {
                hook_phase_id: "hook-pre".into(),
                result_digest: None,
            },
        )
        .unwrap();
        apply_event(
            &mut state,
            &SessionEvent::HookPhaseFinished {
                hook_phase_id: "hook-pre".into(),
                result_digest: None,
                effective_input_digest: Some(DIGEST.into()),
                outcome_digest: DIGEST.into(),
                slot_receipts_digest: state_payload_digest(
                    &serde_json::to_value(&receipts).unwrap(),
                )
                .unwrap(),
                slot_receipts: receipts,
            },
        )
        .unwrap();

        let messages = vec![serde_json::json!({"role": "user", "content": "test"})];
        let messages_digest =
            state_payload_digest(&serde_json::Value::Array(messages.clone())).unwrap();
        let checkpoint = serde_json::json!({"version": 1});
        let checkpoint_state_digest = state_payload_digest(&checkpoint).unwrap();
        apply_event(
            &mut state,
            &SessionEvent::ConversationRecoveryCheckpointCommittedV2 {
                turn_id: "turn".into(),
                messages: messages.clone(),
                messages_digest,
                checkpoint_id: "checkpoint".into(),
                checkpoint_state_digest,
                checkpoint,
                consumed_hook_phases: vec![HookPhaseConsumption {
                    hook_phase_id: "hook-pre".into(),
                    outcome_digest: DIGEST.into(),
                }],
            },
        )
        .unwrap();

        assert_eq!(state.conversation, messages);
        assert!(matches!(
            state.hook_phases["hook-pre"].state,
            HookPhaseState::Consumed {
                ref outcome_digest,
                ref checkpoint_id,
            } if outcome_digest == DIGEST && checkpoint_id == "checkpoint"
        ));
    }

    #[test]
    fn hook_finished_receipt_must_cover_the_prepared_manifest() {
        let mut state = active_turn_state();
        apply_event(&mut state, &prepare_pre()).unwrap();
        apply_event(
            &mut state,
            &SessionEvent::HookPhaseStarted {
                hook_phase_id: "hook-pre".into(),
                result_digest: None,
            },
        )
        .unwrap();

        assert!(matches!(
            apply_event(
                &mut state,
                &SessionEvent::HookPhaseFinished {
                    hook_phase_id: "hook-pre".into(),
                    result_digest: None,
                    effective_input_digest: Some(DIGEST.into()),
                    outcome_digest: DIGEST.into(),
                    slot_receipts_digest: DIGEST.into(),
                    slot_receipts: receipts()[..1].to_vec(),
                },
            ),
            Err(JournalError::InvalidTransition(_))
        ));
        assert!(matches!(
            state.hook_phases["hook-pre"].state,
            HookPhaseState::Started { .. }
        ));
    }

    #[test]
    fn started_hook_can_only_be_abandoned_as_unknown() {
        let mut state = active_turn_state();
        apply_event(&mut state, &prepare_pre()).unwrap();
        apply_event(
            &mut state,
            &SessionEvent::HookPhaseStarted {
                hook_phase_id: "hook-pre".into(),
                result_digest: None,
            },
        )
        .unwrap();
        apply_event(
            &mut state,
            &SessionEvent::HookPhaseAbandonedUnknown {
                hook_phase_id: "hook-pre".into(),
            },
        )
        .unwrap();
        assert!(matches!(
            state.hook_phases["hook-pre"].state,
            HookPhaseState::AbandonedUnknown
        ));
    }

    #[test]
    fn consumed_pre_hook_can_bind_only_its_exact_retry_lineage() {
        let mut state = active_turn_state();
        finish_pre(&mut state);
        apply_event(
            &mut state,
            &tool_intent("tool-original", None, Some("hook-pre")),
        )
        .unwrap();
        apply_event(
            &mut state,
            &SessionEvent::ToolExecutionNotStarted {
                tool_execution_id: "tool-original".into(),
                reason: ToolNotStartedReason::Cancelled {
                    reason: "crash before physical dispatch".into(),
                },
            },
        )
        .unwrap();
        consume_pre(&mut state);

        let mut wrong_digest =
            tool_intent("tool-wrong-digest", Some("tool-original"), Some("hook-pre"));
        let SessionEvent::ToolIntentRecordedV2 {
            effective_input,
            effective_input_digest,
            ..
        } = &mut wrong_digest
        else {
            unreachable!();
        };
        *effective_input_digest =
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into();
        *effective_input = StoredToolInput::redacted(effective_input_digest.clone());
        assert!(matches!(
            apply_event(&mut state.clone(), &wrong_digest),
            Err(JournalError::InvalidTransition(_))
        ));

        apply_event(
            &mut state,
            &tool_intent("tool-retry", Some("tool-original"), Some("hook-pre")),
        )
        .unwrap();
        assert_eq!(
            state.tools["tool-retry"].pre_hook_phase_id.as_deref(),
            Some("hook-pre")
        );
    }

    #[test]
    fn post_hook_authority_is_unique_per_retry_attempt() {
        let mut state = active_turn_state();
        apply_event(&mut state, &tool_intent("tool-original", None, None)).unwrap();
        apply_event(&mut state, &prepare_post("post-original", "tool-original")).unwrap();
        apply_event(
            &mut state,
            &SessionEvent::ToolExecutionNotStarted {
                tool_execution_id: "tool-original".into(),
                reason: ToolNotStartedReason::Cancelled {
                    reason: "crash before physical dispatch".into(),
                },
            },
        )
        .unwrap();
        apply_event(
            &mut state,
            &SessionEvent::HookPhaseNotApplicable {
                hook_phase_id: "post-original".into(),
            },
        )
        .unwrap();

        apply_event(
            &mut state,
            &tool_intent("tool-retry", Some("tool-original"), None),
        )
        .unwrap();
        apply_event(&mut state, &prepare_post("post-retry", "tool-retry")).unwrap();

        assert!(matches!(
            apply_event(
                &mut state,
                &prepare_post("post-retry-duplicate", "tool-retry")
            ),
            Err(JournalError::InvalidTransition(_))
        ));
        assert!(state.hook_phases.contains_key("post-original"));
        assert!(state.hook_phases.contains_key("post-retry"));
    }
}

#[cfg(test)]
mod approval_transition_tests {
    use super::*;

    fn active_turn_state() -> ReducedSessionState {
        let mut state = ReducedSessionState::default();
        state.turns.insert(
            "turn".into(),
            TurnState {
                user_message: "test".into(),
                completion: None,
            },
        );
        state
    }

    fn request(id: &str) -> SessionEvent {
        SessionEvent::ApprovalRequested {
            approval_id: id.into(),
            origin: ApprovalOrigin::Turn {
                turn_id: "turn".into(),
            },
            intent_digest: format!("digest:{id}"),
        }
    }

    #[test]
    fn approval_origin_is_reusable_only_after_prior_request_is_terminal() {
        let mut state = active_turn_state();
        apply_event(&mut state, &request("approval-1")).unwrap();

        let concurrent = apply_event(&mut state, &request("approval-2"));
        assert!(matches!(
            concurrent,
            Err(JournalError::InvalidTransition(_))
        ));

        apply_event(
            &mut state,
            &SessionEvent::ApprovalResolved {
                approval_id: "approval-1".into(),
                resolution: ApprovalResolution::Decided {
                    decision: ApprovalDecision::AllowOnce,
                },
            },
        )
        .unwrap();
        apply_event(&mut state, &request("approval-2")).unwrap();

        assert!(state.approvals["approval-1"].resolution.is_some());
        assert!(state.approvals["approval-2"].resolution.is_none());
    }

    #[test]
    fn resolved_approval_id_cannot_be_reused() {
        let mut state = active_turn_state();
        apply_event(&mut state, &request("approval-1")).unwrap();
        apply_event(
            &mut state,
            &SessionEvent::ApprovalResolved {
                approval_id: "approval-1".into(),
                resolution: ApprovalResolution::Cancelled,
            },
        )
        .unwrap();

        assert!(matches!(
            apply_event(&mut state, &request("approval-1")),
            Err(JournalError::InvalidTransition(_))
        ));
    }
}

#[cfg(test)]
mod terminal_turn_transition_tests {
    use super::*;

    fn active_turn_state() -> ReducedSessionState {
        let mut state = ReducedSessionState::default();
        state.turns.insert(
            "turn".into(),
            TurnState {
                user_message: "test".into(),
                completion: None,
            },
        );
        state
    }

    fn assert_terminal_rejected(event: SessionEvent, expected: &str) {
        let mut state = active_turn_state();
        apply_event(&mut state, &event).unwrap();
        let error = apply_event(
            &mut state,
            &SessionEvent::TurnCancelled {
                turn_id: "turn".into(),
            },
        )
        .unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "unexpected terminal rejection: {error}"
        );
    }

    #[test]
    fn terminal_turn_rejects_every_nonterminal_descendant_class() {
        assert_terminal_rejected(
            SessionEvent::ApprovalRequested {
                approval_id: "approval".into(),
                origin: ApprovalOrigin::Turn {
                    turn_id: "turn".into(),
                },
                intent_digest: "intent".into(),
            },
            "pending approval",
        );
        assert_terminal_rejected(
            SessionEvent::ProviderAttemptPrepared {
                attempt_id: "provider".into(),
                turn_id: "turn".into(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".into(),
                model: "fixture-model".into(),
                request_digest: "request".into(),
            },
            "nonterminal provider attempt",
        );
        assert_terminal_rejected(
            SessionEvent::ToolIntentRecorded {
                tool_execution_id: "tool".into(),
                provider_call_id: "provider-call".into(),
                turn_id: "turn".into(),
                ordinal: 0,
                tool: "Write".into(),
                requested_input: serde_json::json!({}),
                requested_input_digest: "requested".into(),
                effective_input: serde_json::json!({}),
                effective_input_digest: "effective".into(),
            },
            "nonterminal tool execution",
        );
        assert_terminal_rejected(
            SessionEvent::ChildPrepared {
                child_id: "child".into(),
                turn_id: "turn".into(),
                request: serde_json::json!({}),
            },
            "nonterminal child",
        );
        assert_terminal_rejected(
            SessionEvent::DeliveryPrepared {
                delivery_id: "delivery".into(),
                origin: DeliveryOrigin::Turn {
                    turn_id: "turn".into(),
                },
                destination: "fixture".into(),
                payload: serde_json::json!({}),
            },
            "nonterminal delivery",
        );
        assert_terminal_rejected(
            SessionEvent::BudgetReserved {
                event_id: "budget-event".into(),
                reservation_id: "budget".into(),
                owner: BudgetOwner::Turn {
                    turn_id: "turn".into(),
                },
                purpose: BudgetPurpose::Conversation,
                amount: BudgetAmount {
                    value: 1,
                    unit: BudgetUnit::Tokens,
                },
            },
            "open budget reservation",
        );
    }
}

#[cfg(test)]
mod provider_recovery_invariant_tests {
    use super::*;

    fn active_state() -> ReducedSessionState {
        let mut state = ReducedSessionState::default();
        state.turns.insert(
            "turn".into(),
            TurnState {
                user_message: "test".into(),
                completion: None,
            },
        );
        state
    }

    fn done_event() -> ProviderStreamEvent {
        ProviderStreamEvent::Done {
            stop_reason: serde_json::json!("end_turn"),
            finish_reason: serde_json::json!("stop"),
            usage: serde_json::json!({
                "input_tokens": 1,
                "output_tokens": 2,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 0
            }),
        }
    }

    fn start_correlated(state: &mut ReducedSessionState) {
        apply_event(
            state,
            &SessionEvent::ProviderAttemptPreparedV2 {
                attempt_id: "attempt".into(),
                dispatch_id: "dispatch".into(),
                turn_id: "turn".into(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".into(),
                model: "fixture-model".into(),
                request_digest: "request".into(),
            },
        )
        .unwrap();
        apply_event(
            state,
            &SessionEvent::ProviderAttemptStarted {
                attempt_id: "attempt".into(),
            },
        )
        .unwrap();
        apply_event(
            state,
            &SessionEvent::StreamStarted {
                stream_id: "stream".into(),
                attempt_id: "attempt".into(),
            },
        )
        .unwrap();
    }

    #[test]
    fn correlated_success_requires_one_final_done_and_exact_digest() {
        let mut state = active_state();
        start_correlated(&mut state);
        let events = vec![
            ProviderStreamEvent::TextDelta { text: "ok".into() },
            done_event(),
        ];
        apply_event(
            &mut state,
            &SessionEvent::StreamBatchCommitted {
                stream_id: "stream".into(),
                ordinal: 0,
                events: events.clone(),
            },
        )
        .unwrap();
        apply_event(
            &mut state,
            &SessionEvent::StreamFinished {
                stream_id: "stream".into(),
            },
        )
        .unwrap();
        let wrong = apply_event(
            &mut state.clone(),
            &SessionEvent::ProviderAttemptFinishedV2 {
                attempt_id: "attempt".into(),
                dispatch_id: "dispatch".into(),
                outcome: CompletionOutcome::Succeeded,
                response_digest: Some("wrong".into()),
            },
        )
        .unwrap_err();
        assert!(wrong.to_string().contains("digest does not match"));

        apply_event(
            &mut state,
            &SessionEvent::ProviderAttemptFinishedV2 {
                attempt_id: "attempt".into(),
                dispatch_id: "dispatch".into(),
                outcome: CompletionOutcome::Succeeded,
                response_digest: Some(provider_response_digest(&events).unwrap()),
            },
        )
        .unwrap();
    }

    #[test]
    fn correlated_stream_rejects_nonfinal_or_missing_done() {
        let mut nonfinal = active_state();
        start_correlated(&mut nonfinal);
        let error = apply_event(
            &mut nonfinal,
            &SessionEvent::StreamBatchCommitted {
                stream_id: "stream".into(),
                ordinal: 0,
                events: vec![
                    done_event(),
                    ProviderStreamEvent::TextDelta {
                        text: "after terminal".into(),
                    },
                ],
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("non-final terminal"));

        let mut missing = active_state();
        start_correlated(&mut missing);
        apply_event(
            &mut missing,
            &SessionEvent::StreamBatchCommitted {
                stream_id: "stream".into(),
                ordinal: 0,
                events: vec![ProviderStreamEvent::TextDelta {
                    text: "partial".into(),
                }],
            },
        )
        .unwrap();
        let error = apply_event(
            &mut missing,
            &SessionEvent::StreamFinished {
                stream_id: "stream".into(),
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("exactly one final Done"));
    }

    #[test]
    fn legacy_attempt_replays_but_cannot_be_recovered() {
        let mut state = active_state();
        apply_event(
            &mut state,
            &SessionEvent::ProviderAttemptPrepared {
                attempt_id: "attempt".into(),
                turn_id: "turn".into(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".into(),
                model: "fixture-model".into(),
                request_digest: "request".into(),
            },
        )
        .unwrap();
        apply_event(
            &mut state,
            &SessionEvent::ProviderAttemptStarted {
                attempt_id: "attempt".into(),
            },
        )
        .unwrap();
        apply_event(
            &mut state,
            &SessionEvent::StreamStarted {
                stream_id: "stream".into(),
                attempt_id: "attempt".into(),
            },
        )
        .unwrap();
        apply_event(
            &mut state,
            &SessionEvent::StreamBatchCommitted {
                stream_id: "stream".into(),
                ordinal: 0,
                events: vec![ProviderStreamEvent::Error {
                    message: "legacy malformed stream".into(),
                }],
            },
        )
        .unwrap();
        apply_event(
            &mut state,
            &SessionEvent::StreamFinished {
                stream_id: "stream".into(),
            },
        )
        .unwrap();
        apply_event(
            &mut state,
            &SessionEvent::ProviderAttemptFinished {
                attempt_id: "attempt".into(),
                outcome: CompletionOutcome::Succeeded,
                response_digest: Some("legacy".into()),
            },
        )
        .unwrap();

        assert!(matches!(
            crate::provider_recovery::recover_provider_round(&state, "dispatch", "attempt"),
            Err(crate::provider_recovery::ProviderRecoveryError::LegacyAttempt(_))
        ));
    }
}
