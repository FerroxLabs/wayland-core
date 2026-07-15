use std::collections::BTreeSet;
use std::path::Path;

use serde_json::json;
use wcore_agent::session_journal::{
    ApprovalDecision, ApprovalOrigin, ApprovalResolution, BudgetAmount, BudgetOwner, BudgetPurpose,
    BudgetUnit, CheckpointOrigin, CheckpointPurpose, ChildNotStartedReason, CompletionOutcome,
    DeliveryCompletion, DeliveryEvidence, DeliveryNotStartedReason, DeliveryOrigin, DeliveryStage,
    DeliveryUnknownReason, JournalEnvelope, JournalError, ProviderAttemptNotStartedReason,
    ProviderAttemptPurpose, ProviderStreamEvent, ReducedSessionState, SessionEvent, SessionJournal,
    SessionSnapshot, ToolNotStartedReason, replay_state, snapshot_path_for, state_payload_digest,
    write_snapshot,
};

const SESSION_ID: &str = "crash-matrix-session";
const FRAME_HEADER_BYTES: usize = 12;
const FRAME_DIGEST_BYTES: usize = 32;

struct Scenario {
    name: &'static str,
    events: Vec<SessionEvent>,
}

fn digest(value: &serde_json::Value) -> String {
    state_payload_digest(value).unwrap()
}

fn message_event(turn_id: &str, index: u64, text: &str) -> SessionEvent {
    let message = json!({
        "role": "assistant",
        "content": [{"type": "text", "text": text}],
    });
    SessionEvent::ConversationMessageCommitted {
        turn_id: turn_id.to_owned(),
        message_index: index,
        message_digest: digest(&message),
        message,
    }
}

fn conversation_state_event(turn_id: &str, messages: Vec<serde_json::Value>) -> SessionEvent {
    let messages_digest = digest(&serde_json::Value::Array(messages.clone()));
    SessionEvent::ConversationStateCommitted {
        turn_id: turn_id.to_owned(),
        messages,
        messages_digest,
    }
}

fn tool_intent(execution_id: &str, provider_call_id: &str, ordinal: u64) -> SessionEvent {
    let requested_input = json!({"command": "printf requested"});
    let effective_input = json!({"command": "printf effective"});
    SessionEvent::ToolIntentRecorded {
        tool_execution_id: execution_id.to_owned(),
        provider_call_id: provider_call_id.to_owned(),
        turn_id: "turn-main".to_owned(),
        ordinal,
        tool: "bash".to_owned(),
        requested_input_digest: digest(&requested_input),
        effective_input_digest: digest(&effective_input),
        requested_input,
        effective_input,
    }
}

fn main_scenario() -> Scenario {
    let imported = json!({
        "id": SESSION_ID,
        "schema_version": 2,
        "messages": [],
    });
    let conversation = vec![json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "committed"}],
    })];
    let checkpoint = json!({"cursor": 7, "safe": true});

    Scenario {
        name: "all_families",
        events: vec![
            SessionEvent::SessionImported {
                source_schema_version: 2,
                session_digest: digest(&imported),
                session: imported,
            },
            SessionEvent::TurnStarted {
                turn_id: "turn-main".to_owned(),
                user_message: "exercise every durable family".to_owned(),
            },
            message_event("turn-main", 0, "first"),
            conversation_state_event("turn-main", conversation),
            SessionEvent::ProviderAttemptPrepared {
                attempt_id: "provider-success".to_owned(),
                turn_id: "turn-main".to_owned(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".to_owned(),
                model: "fixture-model".to_owned(),
                request_digest: "request-success".to_owned(),
            },
            SessionEvent::ProviderAttemptStarted {
                attempt_id: "provider-success".to_owned(),
            },
            SessionEvent::StreamStarted {
                stream_id: "stream-success".to_owned(),
                attempt_id: "provider-success".to_owned(),
            },
            SessionEvent::StreamBatchCommitted {
                stream_id: "stream-success".to_owned(),
                ordinal: 0,
                events: vec![ProviderStreamEvent::TextDelta {
                    text: "durable delta".to_owned(),
                }],
            },
            SessionEvent::StreamFinished {
                stream_id: "stream-success".to_owned(),
            },
            SessionEvent::ProviderAttemptFinished {
                attempt_id: "provider-success".to_owned(),
                outcome: CompletionOutcome::Succeeded,
                response_digest: Some("response-success".to_owned()),
            },
            SessionEvent::ProviderAttemptPrepared {
                attempt_id: "provider-not-started".to_owned(),
                turn_id: "turn-main".to_owned(),
                purpose: ProviderAttemptPurpose::Compaction,
                provider: "fixture".to_owned(),
                model: "fixture-model".to_owned(),
                request_digest: "request-blocked".to_owned(),
            },
            SessionEvent::ProviderAttemptNotStarted {
                attempt_id: "provider-not-started".to_owned(),
                reason: ProviderAttemptNotStartedReason::EgressDenied {
                    policy: "offline".to_owned(),
                },
            },
            tool_intent("tool-success", "provider-call-success", 0),
            SessionEvent::ApprovalRequested {
                approval_id: "approval-tool".to_owned(),
                origin: ApprovalOrigin::ToolExecution {
                    tool_execution_id: "tool-success".to_owned(),
                },
                intent_digest: "tool-intent".to_owned(),
            },
            SessionEvent::ApprovalResolved {
                approval_id: "approval-tool".to_owned(),
                resolution: ApprovalResolution::Decided {
                    decision: ApprovalDecision::AllowOnce,
                },
            },
            SessionEvent::ToolExecutionStarted {
                tool_execution_id: "tool-success".to_owned(),
            },
            SessionEvent::ToolExecutionFinished {
                tool_execution_id: "tool-success".to_owned(),
                outcome: CompletionOutcome::Succeeded,
                result: json!({"exit_code": 0}),
            },
            tool_intent("tool-not-started", "provider-call-blocked", 1),
            SessionEvent::ToolExecutionNotStarted {
                tool_execution_id: "tool-not-started".to_owned(),
                reason: ToolNotStartedReason::PolicyDenied {
                    policy: "managed".to_owned(),
                },
            },
            SessionEvent::BudgetReserved {
                event_id: "budget-reserve-settle".to_owned(),
                reservation_id: "reservation-settle".to_owned(),
                owner: BudgetOwner::Turn {
                    turn_id: "turn-main".to_owned(),
                },
                purpose: BudgetPurpose::Conversation,
                amount: BudgetAmount {
                    value: 100,
                    unit: BudgetUnit::Tokens,
                },
            },
            SessionEvent::BudgetSettled {
                event_id: "budget-settle".to_owned(),
                reservation_id: "reservation-settle".to_owned(),
                amount: BudgetAmount {
                    value: 80,
                    unit: BudgetUnit::Tokens,
                },
            },
            SessionEvent::BudgetReserved {
                event_id: "budget-reserve-release".to_owned(),
                reservation_id: "reservation-release".to_owned(),
                owner: BudgetOwner::Session,
                purpose: BudgetPurpose::Delivery,
                amount: BudgetAmount {
                    value: 1,
                    unit: BudgetUnit::Requests,
                },
            },
            SessionEvent::BudgetReleased {
                event_id: "budget-release".to_owned(),
                reservation_id: "reservation-release".to_owned(),
            },
            SessionEvent::CheckpointCommitted {
                checkpoint_id: "checkpoint-main".to_owned(),
                purpose: CheckpointPurpose::Recovery,
                origin: CheckpointOrigin::Turn {
                    turn_id: "turn-main".to_owned(),
                },
                state_digest: digest(&checkpoint),
                state: checkpoint,
            },
            SessionEvent::ChildPrepared {
                child_id: "child-success".to_owned(),
                turn_id: "turn-main".to_owned(),
                request: json!({"task": "verify"}),
            },
            SessionEvent::ChildStarted {
                child_id: "child-success".to_owned(),
            },
            SessionEvent::ChildFinished {
                child_id: "child-success".to_owned(),
                outcome: CompletionOutcome::Succeeded,
                result: json!({"report": "complete"}),
            },
            SessionEvent::ChildPrepared {
                child_id: "child-not-started".to_owned(),
                turn_id: "turn-main".to_owned(),
                request: json!({"task": "blocked"}),
            },
            SessionEvent::ChildNotStarted {
                child_id: "child-not-started".to_owned(),
                reason: ChildNotStartedReason::PolicyDenied {
                    policy: "no-children".to_owned(),
                },
            },
            SessionEvent::DeliveryPrepared {
                delivery_id: "delivery-confirmed".to_owned(),
                origin: DeliveryOrigin::Turn {
                    turn_id: "turn-main".to_owned(),
                },
                destination: "host".to_owned(),
                payload: json!({"text": "done"}),
            },
            SessionEvent::DeliveryStarted {
                delivery_id: "delivery-confirmed".to_owned(),
            },
            SessionEvent::DeliveryFinished {
                delivery_id: "delivery-confirmed".to_owned(),
                completion: DeliveryCompletion::Confirmed {
                    outcome: CompletionOutcome::Succeeded,
                    receipt: json!({"message_id": "message-1"}),
                },
            },
            SessionEvent::DeliveryPrepared {
                delivery_id: "delivery-unknown".to_owned(),
                origin: DeliveryOrigin::Cron {
                    schedule_id: "schedule-1".to_owned(),
                    fire_id: "fire-1".to_owned(),
                },
                destination: "channel".to_owned(),
                payload: json!({"text": "scheduled"}),
            },
            SessionEvent::DeliveryStarted {
                delivery_id: "delivery-unknown".to_owned(),
            },
            SessionEvent::DeliveryFinished {
                delivery_id: "delivery-unknown".to_owned(),
                completion: DeliveryCompletion::Unknown {
                    reason: DeliveryUnknownReason::AcknowledgementMissing,
                    evidence: DeliveryEvidence {
                        last_observed_stage: DeliveryStage::PayloadSent,
                        detail: Some("connection closed".to_owned()),
                    },
                },
            },
            SessionEvent::DeliveryPrepared {
                delivery_id: "delivery-not-started".to_owned(),
                origin: DeliveryOrigin::InboundReply {
                    inbound_reply_id: "reply-1".to_owned(),
                },
                destination: "channel".to_owned(),
                payload: json!({"text": "blocked"}),
            },
            SessionEvent::DeliveryNotStarted {
                delivery_id: "delivery-not-started".to_owned(),
                reason: DeliveryNotStartedReason::PolicyDenied {
                    policy: "offline".to_owned(),
                },
            },
            SessionEvent::TurnCommitted {
                turn_id: "turn-main".to_owned(),
                assistant_message: "committed".to_owned(),
            },
        ],
    }
}

fn terminal_scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "turn_failed",
            events: vec![
                SessionEvent::TurnStarted {
                    turn_id: "turn-failed".to_owned(),
                    user_message: "fail".to_owned(),
                },
                SessionEvent::TurnFailed {
                    turn_id: "turn-failed".to_owned(),
                    error: "fixture failure".to_owned(),
                },
            ],
        },
        Scenario {
            name: "turn_cancelled",
            events: vec![
                SessionEvent::TurnStarted {
                    turn_id: "turn-cancelled".to_owned(),
                    user_message: "cancel".to_owned(),
                },
                SessionEvent::TurnCancelled {
                    turn_id: "turn-cancelled".to_owned(),
                },
            ],
        },
    ]
}

fn event_kind(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::SessionImported { .. } => "session_imported",
        SessionEvent::ConversationMessageCommitted { .. } => "conversation_message_committed",
        SessionEvent::ConversationStateCommitted { .. } => "conversation_state_committed",
        SessionEvent::TurnStarted { .. } => "turn_started",
        SessionEvent::TurnCommitted { .. } => "turn_committed",
        SessionEvent::TurnFailed { .. } => "turn_failed",
        SessionEvent::TurnCancelled { .. } => "turn_cancelled",
        SessionEvent::StreamStarted { .. } => "stream_started",
        SessionEvent::StreamBatchCommitted { .. } => "stream_batch_committed",
        SessionEvent::StreamFinished { .. } => "stream_finished",
        SessionEvent::ProviderAttemptPrepared { .. } => "provider_attempt_prepared",
        SessionEvent::ProviderAttemptStarted { .. } => "provider_attempt_started",
        SessionEvent::ProviderAttemptFinished { .. } => "provider_attempt_finished",
        SessionEvent::ProviderAttemptNotStarted { .. } => "provider_attempt_not_started",
        SessionEvent::ToolIntentRecorded { .. } => "tool_intent_recorded",
        SessionEvent::ToolExecutionStarted { .. } => "tool_execution_started",
        SessionEvent::ToolExecutionFinished { .. } => "tool_execution_finished",
        SessionEvent::ToolExecutionNotStarted { .. } => "tool_execution_not_started",
        SessionEvent::ApprovalRequested { .. } => "approval_requested",
        SessionEvent::ApprovalResolved { .. } => "approval_resolved",
        SessionEvent::BudgetReserved { .. } => "budget_reserved",
        SessionEvent::BudgetSettled { .. } => "budget_settled",
        SessionEvent::BudgetReleased { .. } => "budget_released",
        SessionEvent::CheckpointCommitted { .. } => "checkpoint_committed",
        SessionEvent::ChildPrepared { .. } => "child_prepared",
        SessionEvent::ChildStarted { .. } => "child_started",
        SessionEvent::ChildFinished { .. } => "child_finished",
        SessionEvent::ChildNotStarted { .. } => "child_not_started",
        SessionEvent::DeliveryPrepared { .. } => "delivery_prepared",
        SessionEvent::DeliveryStarted { .. } => "delivery_started",
        SessionEvent::DeliveryNotStarted { .. } => "delivery_not_started",
        SessionEvent::DeliveryFinished { .. } => "delivery_finished",
    }
}

fn append_scenario(path: &Path, events: &[SessionEvent]) -> (Vec<JournalEnvelope>, Vec<u8>) {
    let journal = SessionJournal::open(path, SESSION_ID).unwrap();
    for event in events {
        journal.append(event.clone()).unwrap();
    }
    drop(journal);
    (
        SessionJournal::replay(path).unwrap(),
        std::fs::read(path).unwrap(),
    )
}

fn frame_ends(bytes: &[u8]) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        assert_eq!(&bytes[offset..offset + 4], b"WJ01");
        let body_len = u32::from_be_bytes(bytes[offset + 4..offset + 8].try_into().unwrap());
        offset += FRAME_HEADER_BYTES + body_len as usize + FRAME_DIGEST_BYTES;
        assert!(offset <= bytes.len());
        ends.push(offset);
    }
    assert_eq!(offset, bytes.len());
    ends
}

fn partial_frame_cuts(frame: &[u8]) -> Vec<usize> {
    let body_len = u32::from_be_bytes(frame[4..8].try_into().unwrap()) as usize;
    let mut cuts = vec![
        0,
        1,
        FRAME_HEADER_BYTES / 2,
        FRAME_HEADER_BYTES + body_len / 2,
        FRAME_HEADER_BYTES + body_len + FRAME_DIGEST_BYTES / 2,
    ];
    cuts.retain(|cut| *cut < frame.len());
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

fn assert_replay_state(
    path: &Path,
    expected_entries: &[JournalEnvelope],
    expected_state: &ReducedSessionState,
    context: &str,
) {
    assert_eq!(
        SessionJournal::replay(path).unwrap(),
        expected_entries,
        "replay entries diverged at {context}"
    );
    assert_eq!(
        SessionJournal::recovered_state(path).unwrap(),
        *expected_state,
        "reduced state diverged at {context}"
    );
}

#[test]
fn crash_before_during_and_after_every_event_variant_preserves_only_committed_frames() {
    let mut scenarios = vec![main_scenario()];
    scenarios.extend(terminal_scenarios());

    let covered = scenarios
        .iter()
        .flat_map(|scenario| scenario.events.iter().map(event_kind))
        .collect::<BTreeSet<_>>();
    let expected = [
        "session_imported",
        "conversation_message_committed",
        "conversation_state_committed",
        "turn_started",
        "turn_committed",
        "turn_failed",
        "turn_cancelled",
        "stream_started",
        "stream_batch_committed",
        "stream_finished",
        "provider_attempt_prepared",
        "provider_attempt_started",
        "provider_attempt_finished",
        "provider_attempt_not_started",
        "tool_intent_recorded",
        "tool_execution_started",
        "tool_execution_finished",
        "tool_execution_not_started",
        "approval_requested",
        "approval_resolved",
        "budget_reserved",
        "budget_settled",
        "budget_released",
        "checkpoint_committed",
        "child_prepared",
        "child_started",
        "child_finished",
        "child_not_started",
        "delivery_prepared",
        "delivery_started",
        "delivery_not_started",
        "delivery_finished",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(
        covered, expected,
        "the crash matrix must cover every variant"
    );

    let dir = tempfile::tempdir().unwrap();
    for scenario in scenarios {
        let source = dir.path().join(format!("{}.source.journal", scenario.name));
        let (entries, bytes) = append_scenario(&source, &scenario.events);
        let ends = frame_ends(&bytes);
        assert_eq!(entries.len(), scenario.events.len());
        assert_eq!(ends.len(), scenario.events.len());

        for (index, event) in scenario.events.iter().enumerate() {
            let kind = event_kind(event);
            let start = index.checked_sub(1).map_or(0, |previous| ends[previous]);
            let end = ends[index];
            let before_state = replay_state(&entries[..index]).unwrap();
            let after_state = replay_state(&entries[..=index]).unwrap();

            for (cut_index, relative_cut) in partial_frame_cuts(&bytes[start..end])
                .into_iter()
                .enumerate()
            {
                let path = dir.path().join(format!(
                    "{}-{index}-{kind}-cut-{cut_index}.journal",
                    scenario.name
                ));
                std::fs::write(&path, &bytes[..start + relative_cut]).unwrap();
                let context = format!("{} {kind} cut {relative_cut}", scenario.name);
                assert_replay_state(&path, &entries[..index], &before_state, &context);

                // Opening must heal the incomplete tail before the same event
                // is durably retried. The resulting bytes must be identical to
                // the original committed prefix, not merely reduce similarly.
                let journal = SessionJournal::open(&path, SESSION_ID).unwrap();
                assert_eq!(journal.state().unwrap(), before_state, "{context}");
                assert_eq!(journal.append(event.clone()).unwrap(), entries[index]);
                drop(journal);
                assert_eq!(std::fs::read(&path).unwrap(), bytes[..end], "{context}");
                assert_replay_state(&path, &entries[..=index], &after_state, &context);
            }

            let after_path = dir
                .path()
                .join(format!("{}-{index}-{kind}-after.journal", scenario.name));
            std::fs::write(&after_path, &bytes[..end]).unwrap();
            assert_replay_state(
                &after_path,
                &entries[..=index],
                &after_state,
                &format!("{} {kind} exact after", scenario.name),
            );
        }
    }
}

#[test]
fn complete_corrupt_tail_at_every_event_variant_fails_closed() {
    let mut scenarios = vec![main_scenario()];
    scenarios.extend(terminal_scenarios());
    let dir = tempfile::tempdir().unwrap();
    for scenario in scenarios {
        let source = dir.path().join(format!("{}.source.journal", scenario.name));
        let (entries, bytes) = append_scenario(&source, &scenario.events);
        let ends = frame_ends(&bytes);

        for (index, event) in scenario.events.iter().enumerate() {
            let end = ends[index];
            let mut corrupt = bytes[..end].to_vec();
            corrupt[end - 1] ^= 0x80;
            let path = dir.path().join(format!(
                "{}-{index}-{}-corrupt.journal",
                scenario.name,
                event_kind(event)
            ));
            std::fs::write(&path, corrupt).unwrap();
            assert!(matches!(
                SessionJournal::replay(&path),
                Err(JournalError::FrameDigestMismatch { frame, .. }) if frame == index + 1
            ));
            assert!(matches!(
                SessionJournal::recovered_state(&path),
                Err(JournalError::FrameDigestMismatch { frame, .. }) if frame == index + 1
            ));
            assert!(matches!(
                SessionJournal::open(&path, SESSION_ID),
                Err(JournalError::FrameDigestMismatch { frame, .. }) if frame == index + 1
            ));

            // The valid prefix itself remains independently replayable;
            // recovery rejects only because a complete claimed record is
            // corrupt.
            let start = index.checked_sub(1).map_or(0, |previous| ends[previous]);
            let prefix = dir
                .path()
                .join(format!("{}-{index}-valid-prefix.journal", scenario.name));
            std::fs::write(&prefix, &bytes[..start]).unwrap();
            assert_eq!(SessionJournal::replay(prefix).unwrap(), entries[..index]);
        }
    }
}

#[test]
fn snapshot_and_log_crash_phase_matrix_has_one_replay_result() {
    let scenario = main_scenario();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let (entries, full_log) = append_scenario(&path, &scenario.events);
    let expected = replay_state(&entries).unwrap();
    let snapshot_path = snapshot_path_for(&path);

    // Phase 1: crash before snapshot publication leaves the full log alone.
    assert_replay_state(&path, &entries, &expected, "before snapshot publication");

    // Phase 2: every possible published snapshot prefix may overlap the full
    // log. Recovery validates the overlap and replays exactly the suffix.
    for prefix_len in 0..=entries.len() {
        let snapshot =
            SessionSnapshot::new(SESSION_ID, replay_state(&entries[..prefix_len]).unwrap())
                .unwrap();
        write_snapshot(&snapshot_path, &snapshot).unwrap();
        assert_eq!(
            SessionJournal::recovered_state(&path).unwrap(),
            expected,
            "snapshot/full-log overlap failed at prefix {prefix_len}"
        );
    }

    // Phase 3: crash after atomic rotation leaves the final snapshot plus its
    // retained checksum-linked anchor. Both the pre-rotation and post-rotation
    // disk images must select the same state.
    let journal = SessionJournal::open(&path, SESSION_ID).unwrap();
    journal.compact().unwrap();
    let anchor_log = std::fs::read(&path).unwrap();
    assert!(anchor_log.len() < full_log.len());
    drop(journal);
    assert_eq!(SessionJournal::recovered_state(&path).unwrap(), expected);
    std::fs::write(&path, &full_log).unwrap();
    assert_eq!(SessionJournal::recovered_state(&path).unwrap(), expected);
    std::fs::write(&path, &anchor_log).unwrap();
    assert_eq!(SessionJournal::recovered_state(&path).unwrap(), expected);

    // Phase 4: a post-compaction append torn anywhere before its digest is not
    // committed. Reopening heals it, and retrying creates the exact same frame.
    let next = SessionEvent::TurnStarted {
        turn_id: "turn-after-compaction".to_owned(),
        user_message: "continue".to_owned(),
    };
    let journal = SessionJournal::open(&path, SESSION_ID).unwrap();
    journal.append(next.clone()).unwrap();
    let anchor_and_next = std::fs::read(&path).unwrap();
    let next_expected = journal.state().unwrap();
    drop(journal);
    let next_frame = &anchor_and_next[anchor_log.len()..];
    for (cut_index, relative_cut) in partial_frame_cuts(next_frame).into_iter().enumerate() {
        std::fs::write(&path, &anchor_and_next[..anchor_log.len() + relative_cut]).unwrap();
        assert_eq!(
            SessionJournal::recovered_state(&path).unwrap(),
            expected,
            "post-compaction torn suffix cut {cut_index}"
        );
        let journal = SessionJournal::open(&path, SESSION_ID).unwrap();
        assert_eq!(journal.state().unwrap(), expected);
        journal.append(next.clone()).unwrap();
        drop(journal);
        assert_eq!(std::fs::read(&path).unwrap(), anchor_and_next);
        assert_eq!(
            SessionJournal::recovered_state(&path).unwrap(),
            next_expected
        );
    }

    // Phase 5: rotation without the published snapshot has no replay authority
    // for the discarded prefix and must never be treated as a fresh session.
    std::fs::write(&path, &anchor_log).unwrap();
    std::fs::remove_file(&snapshot_path).unwrap();
    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::CompactedJournalMissingSnapshot { .. })
    ));
}

#[test]
fn corrupt_snapshot_never_falls_back_to_a_plausible_log() {
    let scenario = main_scenario();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let (_, _) = append_scenario(&path, &scenario.events);
    let snapshot_path = snapshot_path_for(&path);
    std::fs::write(&snapshot_path, b"{truncated snapshot").unwrap();

    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::Json { .. })
    ));
    assert!(matches!(
        SessionJournal::open(&path, SESSION_ID),
        Err(JournalError::Json { .. })
    ));
}
