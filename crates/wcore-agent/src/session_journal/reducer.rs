//! Deterministic session-journal state reduction and payload digests.

use std::collections::BTreeMap;

use super::*;

impl ReducedSessionState {
    pub fn digest(&self) -> Result<String, JournalError> {
        let bytes = serde_json::to_vec(self).map_err(|source| JournalError::Json {
            context: "encoding reduced state",
            source,
        })?;
        Ok(sha256_hex(&bytes))
    }
}

pub fn reduce(
    mut state: ReducedSessionState,
    envelope: &JournalEnvelope,
) -> Result<ReducedSessionState, JournalError> {
    let expected_seq = state.last_seq.map_or(0, |seq| seq + 1);
    if envelope.schema_version != SESSION_JOURNAL_SCHEMA_VERSION {
        return Err(JournalError::UnsupportedSchema {
            found: envelope.schema_version,
            supported: SESSION_JOURNAL_SCHEMA_VERSION,
        });
    }
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
    entries
        .iter()
        .try_fold(ReducedSessionState::default(), reduce)
}

fn duplicate(kind: &str, id: &str) -> JournalError {
    JournalError::InvalidTransition(format!("duplicate {kind} id {id}"))
}

fn missing(kind: &str, id: &str) -> JournalError {
    JournalError::InvalidTransition(format!("unknown {kind} id {id}"))
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
            require_prepared(&tool.effect, "tool execution", tool_execution_id)
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

fn require_delivery_origin_active(
    state: &ReducedSessionState,
    origin: &DeliveryOrigin,
) -> Result<(), JournalError> {
    match origin {
        DeliveryOrigin::Turn { turn_id } => require_active_turn(state, turn_id),
        DeliveryOrigin::InboundReply { .. } | DeliveryOrigin::Cron { .. } => Ok(()),
    }
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

pub fn provider_request_digest(
    request: &wcore_types::llm::LlmRequest,
) -> Result<String, JournalError> {
    let thinking = request.thinking.as_ref().map(|thinking| match thinking {
        wcore_types::llm::ThinkingConfig::Enabled { budget_tokens } => serde_json::json!({
            "mode": "enabled",
            "budget_tokens": budget_tokens,
        }),
        wcore_types::llm::ThinkingConfig::Disabled => {
            serde_json::json!({ "mode": "disabled" })
        }
    });
    let tools = request
        .tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "name": &tool.name,
                "description": &tool.description,
                "input_schema": &tool.input_schema,
                "deferred": tool.deferred,
                "server": &tool.server,
            })
        })
        .collect::<Vec<_>>();
    let request_value = serde_json::json!({
        "model": &request.model,
        "system": &request.system,
        "messages": &request.messages,
        "tools": tools,
        "max_tokens": request.max_tokens,
        "thinking": thinking,
        "reasoning_effort": &request.reasoning_effort,
        "cache_tier": request.cache_tier.map(|tier| tier.as_str()),
        "routing_hint": request.routing_hint.as_ref().map(|hint| &hint.0),
        "stop_sequences": &request.stop_sequences,
        "web_search": request.web_search,
        "conversation_id": &request.conversation_id,
        "client_context_tokens": request.client_context_tokens,
        "temperature": request.temperature,
        "omit_max_tokens": request.omit_max_tokens,
    });
    state_payload_digest(&request_value)
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
                && state.approvals.is_empty()
                && state.budgets.is_empty()
                && state.budget_event_ids.is_empty()
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
            let turn = required_mut(&mut state.turns, "turn", turn_id)?;
            if turn.completion.is_some() {
                return Err(duplicate("turn completion", turn_id));
            }
            turn.completion = Some(TurnCompletion::Committed {
                assistant_message: assistant_message.clone(),
            });
        }
        SessionEvent::TurnFailed { turn_id, error } => {
            let turn = required_mut(&mut state.turns, "turn", turn_id)?;
            if turn.completion.is_some() {
                return Err(duplicate("turn completion", turn_id));
            }
            turn.completion = Some(TurnCompletion::Failed {
                error: error.clone(),
            });
        }
        SessionEvent::TurnCancelled { turn_id } => {
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
            let stream = required_mut(&mut state.streams, "stream", stream_id)?;
            if stream.finished {
                return Err(duplicate("stream completion", stream_id));
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
        SessionEvent::ProviderAttemptNotStarted { attempt_id, reason } => {
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
            if state.tools.values().any(|existing| {
                existing.turn_id == *turn_id && existing.provider_call_id == *provider_call_id
            }) {
                return Err(JournalError::InvalidTransition(format!(
                    "turn {turn_id} already has provider tool call {provider_call_id}"
                )));
            }
            if state
                .tools
                .values()
                .any(|existing| existing.turn_id == *turn_id && existing.ordinal == *ordinal)
            {
                return Err(JournalError::InvalidTransition(format!(
                    "turn {turn_id} already has tool execution ordinal {ordinal}"
                )));
            }
            if state_payload_digest(requested_input)? != *requested_input_digest {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} requested input digest mismatch"
                )));
            }
            if state_payload_digest(effective_input)? != *effective_input_digest {
                return Err(JournalError::InvalidTransition(format!(
                    "tool execution {tool_execution_id} effective input digest mismatch"
                )));
            }
            state.tools.insert(
                tool_execution_id.clone(),
                ToolState {
                    provider_call_id: provider_call_id.clone(),
                    turn_id: turn_id.clone(),
                    ordinal: *ordinal,
                    tool: tool.clone(),
                    requested_input: requested_input.clone(),
                    requested_input_digest: requested_input_digest.clone(),
                    effective_input: effective_input.clone(),
                    effective_input_digest: effective_input_digest.clone(),
                    result: None,
                    not_started_reason: None,
                    effect: ExternalEffectState::Prepared,
                },
            );
        }
        SessionEvent::ToolExecutionStarted { tool_execution_id } => {
            let turn_id = state
                .tools
                .get(tool_execution_id)
                .ok_or_else(|| missing("tool execution", tool_execution_id))?
                .turn_id
                .clone();
            require_active_turn(state, &turn_id)?;
            let tool = required_mut(&mut state.tools, "tool execution", tool_execution_id)?;
            require_prepared(&tool.effect, "tool execution", tool_execution_id)?;
            tool.effect = ExternalEffectState::Unknown;
        }
        SessionEvent::ToolExecutionFinished {
            tool_execution_id,
            outcome,
            result,
        } => {
            let tool = required_mut(&mut state.tools, "tool execution", tool_execution_id)?;
            require_unknown(&tool.effect, "tool execution", tool_execution_id)?;
            tool.result = Some(result.clone());
            tool.effect = ExternalEffectState::Completed {
                outcome: outcome.clone(),
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
            require_prepared(&tool.effect, "tool execution", tool_execution_id)?;
            tool.not_started_reason = Some(reason.clone());
            tool.effect = ExternalEffectState::NotStarted;
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
                .any(|approval| approval.origin == *origin)
            {
                return Err(JournalError::InvalidTransition(format!(
                    "approval origin {origin:?} already has an approval"
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
            require_prepared(&child.effect, "child", child_id)?;
            child.effect = ExternalEffectState::Unknown;
        }
        SessionEvent::ChildFinished {
            child_id,
            outcome,
            result,
        } => {
            let child = required_mut(&mut state.children, "child", child_id)?;
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
            require_prepared(&child.effect, "child", child_id)?;
            child.not_started_reason = Some(reason.clone());
            child.effect = ExternalEffectState::NotStarted;
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
