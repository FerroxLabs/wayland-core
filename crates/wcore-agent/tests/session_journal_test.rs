use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde_json::json;
use sha2::{Digest, Sha256};
use wcore_agent::session_journal::{
    ApprovalDecision, ApprovalOrigin, ApprovalResolution, BudgetAmount, BudgetOwner, BudgetPurpose,
    BudgetUnit, CheckpointOrigin, CheckpointPurpose, ChildNotStartedReason, CompletionOutcome,
    DeliveryCompletion, DeliveryEvidence, DeliveryOrigin, DeliveryStage, DeliveryUnknownReason,
    ExternalEffectState, GENESIS_CHECKSUM, JournalEnvelope, JournalError,
    ProviderAttemptNotStartedReason, ProviderAttemptPurpose, ProviderStreamEvent,
    ReducedSessionState, SESSION_JOURNAL_SCHEMA_VERSION, SessionEvent, SessionJournal,
    SessionSnapshot, ToolNotStartedReason, TurnState, load_snapshot, provider_request_digest,
    replay_from_snapshot, replay_state, state_payload_digest, verify_chain, write_snapshot,
};
use wcore_types::llm::LlmRequest;
use wcore_types::message::{ContentBlock, Message, Role};

fn turn_started(turn_id: &str) -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id: turn_id.into(),
        user_message: "hello".into(),
    }
}

fn turn_committed(turn_id: &str) -> SessionEvent {
    SessionEvent::TurnCommitted {
        turn_id: turn_id.into(),
        assistant_message: "done".into(),
    }
}

fn provider_prepared(attempt_id: &str, turn_id: &str) -> SessionEvent {
    SessionEvent::ProviderAttemptPrepared {
        attempt_id: attempt_id.into(),
        turn_id: turn_id.into(),
        purpose: ProviderAttemptPurpose::Conversation,
        provider: "x".into(),
        model: "m".into(),
        request_digest: "r".into(),
    }
}

fn tool_intent(
    tool_execution_id: &str,
    provider_call_id: &str,
    turn_id: &str,
    ordinal: u64,
    tool: &str,
    requested_input: serde_json::Value,
    effective_input: serde_json::Value,
) -> SessionEvent {
    SessionEvent::ToolIntentRecorded {
        tool_execution_id: tool_execution_id.into(),
        provider_call_id: provider_call_id.into(),
        turn_id: turn_id.into(),
        ordinal,
        tool: tool.into(),
        requested_input_digest: state_payload_digest(&requested_input).unwrap(),
        effective_input_digest: state_payload_digest(&effective_input).unwrap(),
        requested_input,
        effective_input,
    }
}

fn text_batch(stream_id: &str, ordinal: u64, text: &str) -> SessionEvent {
    SessionEvent::StreamBatchCommitted {
        stream_id: stream_id.into(),
        ordinal,
        events: vec![ProviderStreamEvent::TextDelta { text: text.into() }],
    }
}

fn imported(session: serde_json::Value) -> SessionEvent {
    SessionEvent::SessionImported {
        source_schema_version: u32::try_from(session["schema_version"].as_u64().unwrap_or(0))
            .unwrap(),
        session_digest: state_payload_digest(&session).unwrap(),
        session,
    }
}

fn message_committed(
    turn_id: &str,
    message_index: u64,
    message: serde_json::Value,
) -> SessionEvent {
    SessionEvent::ConversationMessageCommitted {
        turn_id: turn_id.into(),
        message_index,
        message_digest: state_payload_digest(&message).unwrap(),
        message,
    }
}

fn conversation_state_committed(turn_id: &str, messages: Vec<serde_json::Value>) -> SessionEvent {
    let messages_digest =
        state_payload_digest(&serde_json::Value::Array(messages.clone())).unwrap();
    SessionEvent::ConversationStateCommitted {
        turn_id: turn_id.into(),
        messages,
        messages_digest,
    }
}

fn append_events(path: &Path, events: Vec<SessionEvent>) -> Vec<JournalEnvelope> {
    let journal = SessionJournal::open(path, "s1").unwrap();
    events
        .into_iter()
        .map(|event| journal.append(event).unwrap())
        .collect()
}

fn frame(body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(body.len()).unwrap();
    let mut frame = Vec::new();
    frame.extend_from_slice(b"WJ01");
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(&(!length).to_be_bytes());
    frame.extend_from_slice(body);
    frame.extend_from_slice(&Sha256::digest(body));
    frame
}

#[test]
fn provider_request_digest_is_stable_for_the_exact_request() {
    let request = LlmRequest {
        model: "model-a".into(),
        system: "system".into(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "question".into(),
            }],
        )],
        max_tokens: 1_024,
        conversation_id: Some("conversation-1".into()),
        client_context_tokens: Some(42),
        ..LlmRequest::default()
    };

    let first = provider_request_digest(&request).unwrap();
    let second = provider_request_digest(&request.clone()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.len(), 64);
}

#[test]
fn provider_request_digest_changes_when_wire_relevant_input_changes() {
    let request = LlmRequest {
        model: "model-a".into(),
        system: "system".into(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "question".into(),
            }],
        )],
        max_tokens: 1_024,
        ..LlmRequest::default()
    };
    let original = provider_request_digest(&request).unwrap();

    let mut changed_message = request.clone();
    changed_message.messages[0].content = vec![ContentBlock::Text {
        text: "different question".into(),
    }];
    assert_ne!(original, provider_request_digest(&changed_message).unwrap());

    let mut changed_model = request.clone();
    changed_model.model = "model-b".into();
    assert_ne!(original, provider_request_digest(&changed_model).unwrap());

    let mut changed_limit = request;
    changed_limit.max_tokens += 1;
    assert_ne!(original, provider_request_digest(&changed_limit).unwrap());
}

#[test]
fn append_is_contiguous_checksummed_and_exclusively_owned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    std::fs::write(&path, []).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let first = SessionJournal::open(&path, "s1").unwrap();
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::AlreadyOwned { .. })
    ));
    let second = first.clone();
    let zero = first.append(turn_started("t0")).unwrap();
    let one = second.append(turn_committed("t0")).unwrap();
    assert_eq!((zero.seq, one.seq), (0, 1));
    assert_eq!(zero.previous_checksum, GENESIS_CHECKSUM);
    assert_eq!(one.previous_checksum, zero.checksum);
    assert_eq!(SessionJournal::replay(&path).unwrap(), vec![zero, one]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(first);
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::AlreadyOwned { .. })
    ));
    drop(second);
    assert!(SessionJournal::open(&path, "s1").is_ok());
}

#[test]
fn lease_holder_process_exits_without_drop() {
    let Ok(path) = std::env::var("WCORE_TEST_JOURNAL_LEASE_PATH") else {
        return;
    };
    let _journal = SessionJournal::open(path, "crash-owner").unwrap();
    std::process::exit(0);
}

#[test]
fn operating_system_releases_writer_lease_after_process_exit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lease_holder_process_exits_without_drop"])
        .env("WCORE_TEST_JOURNAL_LEASE_PATH", &path)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(SessionJournal::open(path, "crash-owner").is_ok());
}

#[test]
fn torn_tail_is_ignored_healed_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    append_events(&path, vec![turn_started("t0")]);
    let torn = frame(br#"{"incomplete":true}"#);
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&torn[..torn.len() - 7])
        .unwrap();
    assert_eq!(SessionJournal::replay(&path).unwrap().len(), 1);
    let journal = SessionJournal::open(&path, "s1").unwrap();
    assert_eq!(journal.append(turn_committed("t0")).unwrap().seq, 1);
    assert_eq!(SessionJournal::replay(&path).unwrap().len(), 2);
}

#[test]
fn complete_corrupt_final_frame_is_a_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    std::fs::write(&path, frame(b"{not json}")).unwrap();
    assert!(matches!(
        SessionJournal::replay(path),
        Err(JournalError::CorruptFrame { frame: 1, .. })
    ));
}

#[test]
fn complete_frame_digest_corruption_is_a_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let mut bytes = frame(br#"{"valid":"json"}"#);
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        SessionJournal::replay(path),
        Err(JournalError::FrameDigestMismatch { frame: 1, .. })
    ));
}

#[test]
fn checksum_sequence_previous_and_schema_tampering_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let entries = append_events(
        &dir.path().join("session.journal"),
        vec![turn_started("t0"), turn_committed("t0")],
    );
    let zero = entries[0].clone();
    let one = entries[1].clone();

    let mut bad_checksum = zero.clone();
    bad_checksum.checksum = "bad".into();
    assert!(matches!(
        verify_chain(&[bad_checksum]),
        Err(JournalError::ChecksumMismatch { .. })
    ));

    let mut gap = one.clone();
    gap.seq = 2;
    assert!(matches!(
        verify_chain(&[zero.clone(), gap]),
        Err(JournalError::SequenceMismatch { .. })
    ));

    let mut wrong_previous = one.clone();
    wrong_previous.previous_checksum = GENESIS_CHECKSUM.into();
    assert!(matches!(
        verify_chain(&[zero.clone(), wrong_previous]),
        Err(JournalError::PreviousChecksumMismatch { .. })
    ));
    assert!(matches!(
        verify_chain(&[one]),
        Err(JournalError::SequenceMismatch { .. })
    ));

    let mut future = zero;
    future.schema_version = SESSION_JOURNAL_SCHEMA_VERSION + 1;
    assert!(matches!(
        verify_chain(&[future]),
        Err(JournalError::UnsupportedSchema { .. })
    ));

    let mut obsolete = entries[0].clone();
    obsolete.schema_version = SESSION_JOURNAL_SCHEMA_VERSION - 1;
    assert!(matches!(
        verify_chain(&[obsolete]),
        Err(JournalError::UnsupportedSchema { .. })
    ));
}

#[test]
fn obsolete_journal_schema_is_rejected_before_decoding_incompatible_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v2.journal");
    let obsolete = serde_json::to_vec(&json!({
        "schema_version": SESSION_JOURNAL_SCHEMA_VERSION - 1,
        "session_id": "s1",
        "seq": 0,
        "previous_checksum": GENESIS_CHECKSUM,
        "event": {
            "type": "stream_delta_committed",
            "stream_id": "stream",
            "ordinal": 0,
            "content": "lossy-v1"
        },
        "checksum": "irrelevant-for-unsupported-schema"
    }))
    .unwrap();
    std::fs::write(&path, frame(&obsolete)).unwrap();
    assert!(matches!(
        SessionJournal::replay(path),
        Err(JournalError::UnsupportedSchema { found: 2, .. })
    ));
}

#[test]
fn unresolved_started_external_effects_reduce_to_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let entries = append_events(
        &dir.path().join("session.journal"),
        vec![
            turn_started("turn"),
            provider_prepared("p", "turn"),
            SessionEvent::ProviderAttemptStarted {
                attempt_id: "p".into(),
            },
            tool_intent(
                "tool-exec",
                "provider-call",
                "turn",
                0,
                "bash",
                json!({"cmd":"true"}),
                json!({"cmd":"true"}),
            ),
            SessionEvent::ToolExecutionStarted {
                tool_execution_id: "tool-exec".into(),
            },
            SessionEvent::ChildPrepared {
                child_id: "c".into(),
                turn_id: "turn".into(),
                request: json!({"task":"x"}),
            },
            SessionEvent::ChildStarted {
                child_id: "c".into(),
            },
            SessionEvent::DeliveryPrepared {
                delivery_id: "d".into(),
                origin: DeliveryOrigin::Turn {
                    turn_id: "turn".into(),
                },
                destination: "host".into(),
                payload: json!({"text":"x"}),
            },
            SessionEvent::DeliveryStarted {
                delivery_id: "d".into(),
            },
        ],
    );
    let state = replay_state(&entries).unwrap();
    for effect in [
        &state.provider_attempts["p"].effect,
        &state.tools["tool-exec"].effect,
        &state.children["c"].effect,
        &state.deliveries["d"].effect,
    ] {
        assert_eq!(effect, &ExternalEffectState::Unknown);
        assert!(effect.requires_reconciliation());
    }
}

#[test]
fn full_replay_equals_snapshot_plus_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let entries = append_events(
        &dir.path().join("session.journal"),
        vec![
            SessionEvent::TurnStarted {
                turn_id: "t".into(),
                user_message: "hello".into(),
            },
            provider_prepared("p", "t"),
            SessionEvent::ProviderAttemptStarted {
                attempt_id: "p".into(),
            },
            SessionEvent::StreamStarted {
                stream_id: "s".into(),
                attempt_id: "p".into(),
            },
            text_batch("s", 0, "done"),
            SessionEvent::StreamFinished {
                stream_id: "s".into(),
            },
            SessionEvent::ProviderAttemptFinished {
                attempt_id: "p".into(),
                outcome: CompletionOutcome::Succeeded,
                response_digest: Some("response".into()),
            },
            SessionEvent::TurnCommitted {
                turn_id: "t".into(),
                assistant_message: "done".into(),
            },
        ],
    );
    let full = replay_state(&entries).unwrap();
    let snapshot = SessionSnapshot::new("s1", replay_state(&entries[..5]).unwrap()).unwrap();
    assert_eq!(
        full,
        replay_from_snapshot(&snapshot, &entries[5..]).unwrap()
    );
}

#[test]
fn provider_stream_requires_started_attempt_and_preserves_ordered_structured_batches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();

    assert!(matches!(
        journal.append(provider_prepared("orphan", "missing-turn")),
        Err(JournalError::InvalidTransition(_))
    ));

    let stream_started = SessionEvent::StreamStarted {
        stream_id: "stream".into(),
        attempt_id: "attempt".into(),
    };
    assert!(matches!(
        journal.append(stream_started.clone()),
        Err(JournalError::InvalidTransition(_))
    ));

    journal
        .append(provider_prepared("attempt", "turn"))
        .unwrap();
    assert!(matches!(
        journal.append(stream_started.clone()),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ProviderAttemptStarted {
            attempt_id: "attempt".into(),
        })
        .unwrap();
    journal.append(stream_started).unwrap();
    assert!(matches!(
        journal.append(SessionEvent::StreamStarted {
            stream_id: "other".into(),
            attempt_id: "attempt".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    assert!(matches!(
        journal.append(SessionEvent::StreamBatchCommitted {
            stream_id: "stream".into(),
            ordinal: 0,
            events: vec![],
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    assert!(matches!(
        journal.append(text_batch("stream", 1, "gap")),
        Err(JournalError::InvalidTransition(_))
    ));

    let batch = vec![
        ProviderStreamEvent::ThinkingDelta {
            text: "reason".into(),
        },
        ProviderStreamEvent::ToolUse {
            id: "call".into(),
            name: "read".into(),
            input: json!({"path":"README.md"}),
            extra: Some(json!({"signature":"opaque"})),
        },
        ProviderStreamEvent::Done {
            stop_reason: json!("tool_use"),
            finish_reason: json!("tool_calls"),
            usage: json!({"input_tokens":10,"output_tokens":2}),
        },
    ];
    journal
        .append(SessionEvent::StreamBatchCommitted {
            stream_id: "stream".into(),
            ordinal: 0,
            events: batch.clone(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ProviderAttemptFinished {
            attempt_id: "attempt".into(),
            outcome: CompletionOutcome::Succeeded,
            response_digest: Some("response".into()),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::StreamFinished {
            stream_id: "stream".into(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(text_batch("stream", 1, "late")),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ProviderAttemptFinished {
            attempt_id: "attempt".into(),
            outcome: CompletionOutcome::Succeeded,
            response_digest: Some("response".into()),
        })
        .unwrap();

    journal
        .append(SessionEvent::ProviderAttemptPrepared {
            attempt_id: "compaction".into(),
            turn_id: "turn".into(),
            purpose: ProviderAttemptPurpose::Compaction,
            provider: "x".into(),
            model: "m".into(),
            request_digest: "compact-request".into(),
        })
        .unwrap();
    let state = journal.state().unwrap();
    assert_eq!(state.streams["stream"].batches, vec![batch.clone()]);
    assert_eq!(state.provider_attempts["attempt"].turn_id, "turn");
    assert_eq!(
        state.provider_attempts["compaction"].purpose,
        ProviderAttemptPurpose::Compaction
    );
    let replayed = replay_state(&SessionJournal::replay(&path).unwrap()).unwrap();
    assert_eq!(replayed.streams["stream"].batches, vec![batch]);
}

#[test]
fn approval_linkage_and_terminal_resolution_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();
    journal
        .append(tool_intent(
            "exec",
            "call",
            "turn",
            0,
            "bash",
            json!({"cmd":"true"}),
            json!({"cmd":"true"}),
        ))
        .unwrap();

    assert!(matches!(
        journal.append(SessionEvent::ApprovalRequested {
            approval_id: "missing".into(),
            origin: ApprovalOrigin::ToolExecution {
                tool_execution_id: "unknown".into(),
            },
            intent_digest: "intent".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ApprovalRequested {
            approval_id: "approval".into(),
            origin: ApprovalOrigin::ToolExecution {
                tool_execution_id: "exec".into(),
            },
            intent_digest: "intent".into(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ApprovalRequested {
            approval_id: "duplicate-origin".into(),
            origin: ApprovalOrigin::ToolExecution {
                tool_execution_id: "exec".into(),
            },
            intent_digest: "intent".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    let resolved = SessionEvent::ApprovalResolved {
        approval_id: "approval".into(),
        resolution: ApprovalResolution::TimedOut,
    };
    journal.append(resolved.clone()).unwrap();
    assert!(matches!(
        journal.append(resolved),
        Err(JournalError::InvalidTransition(_))
    ));
    let state = journal.state().unwrap();
    assert_eq!(
        state.approvals["approval"].origin,
        ApprovalOrigin::ToolExecution {
            tool_execution_id: "exec".into(),
        }
    );
    assert_eq!(
        state.approvals["approval"].resolution,
        Some(ApprovalResolution::TimedOut)
    );

    journal
        .append(tool_intent(
            "exec-cancel",
            "call-cancel",
            "turn",
            1,
            "bash",
            json!({}),
            json!({}),
        ))
        .unwrap();
    journal
        .append(SessionEvent::ApprovalRequested {
            approval_id: "cancelled".into(),
            origin: ApprovalOrigin::ToolExecution {
                tool_execution_id: "exec-cancel".into(),
            },
            intent_digest: "cancel-intent".into(),
        })
        .unwrap();
    journal
        .append(SessionEvent::ApprovalResolved {
            approval_id: "cancelled".into(),
            resolution: ApprovalResolution::Cancelled,
        })
        .unwrap();

    journal
        .append(tool_intent(
            "exec-allow",
            "call-allow",
            "turn",
            2,
            "bash",
            json!({}),
            json!({}),
        ))
        .unwrap();
    journal
        .append(SessionEvent::ApprovalRequested {
            approval_id: "allowed".into(),
            origin: ApprovalOrigin::ToolExecution {
                tool_execution_id: "exec-allow".into(),
            },
            intent_digest: "allow-intent".into(),
        })
        .unwrap();
    journal
        .append(SessionEvent::ApprovalResolved {
            approval_id: "allowed".into(),
            resolution: ApprovalResolution::Decided {
                decision: ApprovalDecision::AllowOnce,
            },
        })
        .unwrap();

    journal
        .append(provider_prepared("attempt", "turn"))
        .unwrap();
    journal
        .append(SessionEvent::ApprovalRequested {
            approval_id: "provider-approval".into(),
            origin: ApprovalOrigin::ProviderAttempt {
                attempt_id: "attempt".into(),
            },
            intent_digest: "provider-intent".into(),
        })
        .unwrap();
}

#[test]
fn children_and_deliveries_distinguish_prepared_from_started_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ChildStarted {
            child_id: "child".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ChildPrepared {
            child_id: "child".into(),
            turn_id: "turn".into(),
            request: json!({"task":"inspect"}),
        })
        .unwrap();
    assert_eq!(
        journal.state().unwrap().children["child"].effect,
        ExternalEffectState::Prepared
    );
    journal
        .append(SessionEvent::ChildStarted {
            child_id: "child".into(),
        })
        .unwrap();
    assert_eq!(
        journal.state().unwrap().children["child"].effect,
        ExternalEffectState::Unknown
    );
    journal
        .append(SessionEvent::ChildFinished {
            child_id: "child".into(),
            outcome: CompletionOutcome::Succeeded,
            result: json!({"answer":"done"}),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ChildStarted {
            child_id: "child".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    assert!(matches!(
        journal.append(SessionEvent::DeliveryStarted {
            delivery_id: "delivery".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::DeliveryPrepared {
            delivery_id: "delivery".into(),
            origin: DeliveryOrigin::Turn {
                turn_id: "turn".into(),
            },
            destination: "host".into(),
            payload: json!({"text":"hello"}),
        })
        .unwrap();
    assert_eq!(
        journal.state().unwrap().deliveries["delivery"].effect,
        ExternalEffectState::Prepared
    );
    journal
        .append(SessionEvent::DeliveryStarted {
            delivery_id: "delivery".into(),
        })
        .unwrap();
    assert_eq!(
        journal.state().unwrap().deliveries["delivery"].effect,
        ExternalEffectState::Unknown
    );
    journal
        .append(SessionEvent::DeliveryFinished {
            delivery_id: "delivery".into(),
            completion: DeliveryCompletion::Confirmed {
                outcome: CompletionOutcome::Succeeded,
                receipt: json!({"accepted":true}),
            },
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::DeliveryStarted {
            delivery_id: "delivery".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    journal
        .append(SessionEvent::DeliveryPrepared {
            delivery_id: "delivery-denied".into(),
            origin: DeliveryOrigin::Turn {
                turn_id: "turn".into(),
            },
            destination: "host".into(),
            payload: json!({"text":"blocked"}),
        })
        .unwrap();
    let denied = DeliveryNotStartedReason::PolicyDenied {
        policy: "managed".into(),
    };
    journal
        .append(SessionEvent::DeliveryNotStarted {
            delivery_id: "delivery-denied".into(),
            reason: denied.clone(),
        })
        .unwrap();
    let denied_state = &journal.state().unwrap().deliveries["delivery-denied"];
    assert_eq!(denied_state.effect, ExternalEffectState::NotStarted);
    assert_eq!(denied_state.not_started_reason, Some(denied));
    assert!(matches!(
        journal.append(SessionEvent::DeliveryStarted {
            delivery_id: "delivery-denied".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
}

#[test]
fn prepared_provider_tool_and_child_can_finish_without_a_fabricated_start() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();

    journal
        .append(provider_prepared("attempt", "turn"))
        .unwrap();
    let provider_reason = ProviderAttemptNotStartedReason::EgressDenied {
        policy: "network-boundary".into(),
    };
    journal
        .append(SessionEvent::ProviderAttemptNotStarted {
            attempt_id: "attempt".into(),
            reason: provider_reason.clone(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ProviderAttemptStarted {
            attempt_id: "attempt".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(provider_prepared("started-attempt", "turn"))
        .unwrap();
    journal
        .append(SessionEvent::ProviderAttemptStarted {
            attempt_id: "started-attempt".into(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ProviderAttemptNotStarted {
            attempt_id: "started-attempt".into(),
            reason: ProviderAttemptNotStartedReason::Cancelled {
                reason: "too late".into(),
            },
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    let requested = json!({"path":"requested"});
    let effective = json!({"path":"effective"});
    journal
        .append(tool_intent(
            "execution",
            "provider-call",
            "turn",
            0,
            "read",
            requested.clone(),
            effective.clone(),
        ))
        .unwrap();
    let tool_reason = ToolNotStartedReason::ApprovalDenied {
        approval_id: "approval".into(),
    };
    journal
        .append(SessionEvent::ToolExecutionNotStarted {
            tool_execution_id: "execution".into(),
            reason: tool_reason.clone(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ToolExecutionStarted {
            tool_execution_id: "execution".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(tool_intent(
            "started-execution",
            "started-provider-call",
            "turn",
            1,
            "read",
            json!({}),
            json!({}),
        ))
        .unwrap();
    journal
        .append(SessionEvent::ToolExecutionStarted {
            tool_execution_id: "started-execution".into(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ToolExecutionNotStarted {
            tool_execution_id: "started-execution".into(),
            reason: ToolNotStartedReason::Cancelled {
                reason: "too late".into(),
            },
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    journal
        .append(SessionEvent::ChildPrepared {
            child_id: "child".into(),
            turn_id: "turn".into(),
            request: json!({"task":"inspect"}),
        })
        .unwrap();
    let child_reason = ChildNotStartedReason::PolicyDenied {
        policy: "spawn-disabled".into(),
    };
    journal
        .append(SessionEvent::ChildNotStarted {
            child_id: "child".into(),
            reason: child_reason.clone(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ChildStarted {
            child_id: "child".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ChildPrepared {
            child_id: "started-child".into(),
            turn_id: "turn".into(),
            request: json!({"task":"started"}),
        })
        .unwrap();
    journal
        .append(SessionEvent::ChildStarted {
            child_id: "started-child".into(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ChildNotStarted {
            child_id: "started-child".into(),
            reason: ChildNotStartedReason::Cancelled {
                reason: "too late".into(),
            },
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    let state = journal.state().unwrap();
    assert_eq!(
        state.provider_attempts["attempt"].effect,
        ExternalEffectState::NotStarted
    );
    assert_eq!(
        state.provider_attempts["attempt"].not_started_reason,
        Some(provider_reason)
    );
    assert_eq!(state.tools["execution"].provider_call_id, "provider-call");
    assert_eq!(state.tools["execution"].turn_id, "turn");
    assert_eq!(state.tools["execution"].ordinal, 0);
    assert_eq!(state.tools["execution"].requested_input, requested);
    assert_eq!(state.tools["execution"].effective_input, effective);
    assert_eq!(
        state.tools["execution"].effect,
        ExternalEffectState::NotStarted
    );
    assert_eq!(
        state.tools["execution"].not_started_reason,
        Some(tool_reason)
    );
    assert_eq!(
        state.children["child"].effect,
        ExternalEffectState::NotStarted
    );
    assert_eq!(
        state.children["child"].not_started_reason,
        Some(child_reason)
    );
}

#[test]
fn tool_execution_identity_digests_and_collisions_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();
    journal
        .append(tool_intent(
            "execution-a",
            "provider-call-a",
            "turn",
            0,
            "bash",
            json!({"cmd":"echo requested"}),
            json!({"cmd":"echo effective"}),
        ))
        .unwrap();

    for event in [
        tool_intent(
            "execution-a",
            "provider-call-b",
            "turn",
            1,
            "bash",
            json!({}),
            json!({}),
        ),
        tool_intent(
            "execution-b",
            "provider-call-a",
            "turn",
            1,
            "bash",
            json!({}),
            json!({}),
        ),
        tool_intent(
            "execution-c",
            "provider-call-c",
            "turn",
            0,
            "bash",
            json!({}),
            json!({}),
        ),
    ] {
        assert!(matches!(
            journal.append(event),
            Err(JournalError::InvalidTransition(_))
        ));
    }

    let bad_input = json!({"cmd":"false"});
    assert!(matches!(
        journal.append(SessionEvent::ToolIntentRecorded {
            tool_execution_id: "execution-d".into(),
            provider_call_id: "provider-call-d".into(),
            turn_id: "turn".into(),
            ordinal: 1,
            tool: "bash".into(),
            requested_input: bad_input.clone(),
            requested_input_digest: "wrong".into(),
            effective_input_digest: state_payload_digest(&bad_input).unwrap(),
            effective_input: bad_input,
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    let bad_effective = json!({"cmd":"effective"});
    assert!(matches!(
        journal.append(SessionEvent::ToolIntentRecorded {
            tool_execution_id: "execution-e".into(),
            provider_call_id: "provider-call-e".into(),
            turn_id: "turn".into(),
            ordinal: 1,
            tool: "bash".into(),
            requested_input_digest: state_payload_digest(&json!({})).unwrap(),
            requested_input: json!({}),
            effective_input: bad_effective,
            effective_input_digest: "wrong".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
}

#[test]
fn budgets_have_typed_amounts_owners_and_globally_stable_event_ids() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    let reserved = BudgetAmount {
        value: 1_000,
        unit: BudgetUnit::Tokens,
    };
    journal
        .append(SessionEvent::BudgetReserved {
            event_id: "budget-event-1".into(),
            reservation_id: "reservation-1".into(),
            owner: BudgetOwner::Session,
            purpose: BudgetPurpose::Conversation,
            amount: reserved,
        })
        .unwrap();

    assert!(matches!(
        journal.append(SessionEvent::BudgetReserved {
            event_id: "budget-event-1".into(),
            reservation_id: "reservation-2".into(),
            owner: BudgetOwner::Session,
            purpose: BudgetPurpose::Compaction,
            amount: reserved,
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    assert!(matches!(
        journal.append(SessionEvent::BudgetReserved {
            event_id: "budget-event-zero".into(),
            reservation_id: "reservation-zero".into(),
            owner: BudgetOwner::Session,
            purpose: BudgetPurpose::Conversation,
            amount: BudgetAmount {
                value: 0,
                unit: BudgetUnit::Tokens,
            },
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    assert!(matches!(
        journal.append(SessionEvent::BudgetSettled {
            event_id: "budget-event-2".into(),
            reservation_id: "reservation-1".into(),
            amount: BudgetAmount {
                value: 1,
                unit: BudgetUnit::Credits,
            },
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::BudgetSettled {
            event_id: "budget-event-2".into(),
            reservation_id: "reservation-1".into(),
            amount: BudgetAmount {
                value: 750,
                unit: BudgetUnit::Tokens,
            },
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::BudgetReleased {
            event_id: "budget-event-3".into(),
            reservation_id: "reservation-1".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    journal.append(turn_started("turn")).unwrap();
    journal
        .append(SessionEvent::BudgetReserved {
            event_id: "budget-event-4".into(),
            reservation_id: "reservation-2".into(),
            owner: BudgetOwner::Turn {
                turn_id: "turn".into(),
            },
            purpose: BudgetPurpose::ToolExecution,
            amount: BudgetAmount {
                value: 5,
                unit: BudgetUnit::ToolCalls,
            },
        })
        .unwrap();
    journal
        .append(SessionEvent::BudgetReleased {
            event_id: "budget-event-5".into(),
            reservation_id: "reservation-2".into(),
        })
        .unwrap();

    let state = journal.state().unwrap();
    assert_eq!(state.budgets["reservation-1"].reserved, reserved);
    assert_eq!(
        state.budgets["reservation-1"].used,
        Some(BudgetAmount {
            value: 750,
            unit: BudgetUnit::Tokens,
        })
    );
    assert_eq!(
        state.budgets["reservation-1"].event_ids,
        vec!["budget-event-1", "budget-event-2"]
    );
    assert_eq!(state.budget_event_ids["budget-event-2"], "reservation-1");
    assert_eq!(
        state.budgets["reservation-2"].owner,
        BudgetOwner::Turn {
            turn_id: "turn".into(),
        }
    );
    assert_eq!(
        state.budgets["reservation-2"].purpose,
        BudgetPurpose::ToolExecution
    );
    assert!(state.budgets["reservation-2"].released);
}

#[test]
fn delivery_origins_and_terminal_unknown_evidence_remain_truthful() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();

    journal
        .append(SessionEvent::DeliveryPrepared {
            delivery_id: "not-started".into(),
            origin: DeliveryOrigin::InboundReply {
                inbound_reply_id: "inbound-not-started".into(),
            },
            destination: "host".into(),
            payload: json!({"text":"not sent"}),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::DeliveryFinished {
            delivery_id: "not-started".into(),
            completion: DeliveryCompletion::Unknown {
                reason: DeliveryUnknownReason::AcknowledgementMissing,
                evidence: DeliveryEvidence {
                    last_observed_stage: DeliveryStage::DispatchAccepted,
                    detail: None,
                },
            },
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    let origins = [
        DeliveryOrigin::Turn {
            turn_id: "turn".into(),
        },
        DeliveryOrigin::InboundReply {
            inbound_reply_id: "inbound-1".into(),
        },
        DeliveryOrigin::Cron {
            schedule_id: "schedule-1".into(),
            fire_id: "fire-1".into(),
        },
    ];
    for (ordinal, origin) in origins.into_iter().enumerate() {
        let delivery_id = format!("delivery-{ordinal}");
        journal
            .append(SessionEvent::DeliveryPrepared {
                delivery_id: delivery_id.clone(),
                origin: origin.clone(),
                destination: "host".into(),
                payload: json!({"text":"hello"}),
            })
            .unwrap();
        journal
            .append(SessionEvent::DeliveryStarted {
                delivery_id: delivery_id.clone(),
            })
            .unwrap();
        assert_eq!(
            journal.state().unwrap().deliveries[&delivery_id].origin,
            origin
        );
    }

    let completion = DeliveryCompletion::Unknown {
        reason: DeliveryUnknownReason::TimedOut { timeout_ms: 30_000 },
        evidence: DeliveryEvidence {
            last_observed_stage: DeliveryStage::AwaitingAcknowledgement,
            detail: Some("host did not acknowledge before deadline".into()),
        },
    };
    journal
        .append(SessionEvent::DeliveryFinished {
            delivery_id: "delivery-1".into(),
            completion: completion.clone(),
        })
        .unwrap();
    let state = journal.state().unwrap();
    let delivery = &state.deliveries["delivery-1"];
    assert_eq!(delivery.completion, Some(completion));
    assert_eq!(delivery.effect, ExternalEffectState::Unknown);
    assert!(delivery.effect.requires_reconciliation());
    assert!(matches!(
        journal.append(SessionEvent::DeliveryFinished {
            delivery_id: "delivery-1".into(),
            completion: DeliveryCompletion::Confirmed {
                outcome: CompletionOutcome::Failed {
                    error: "late guess".into(),
                },
                receipt: json!(null),
            },
        }),
        Err(JournalError::InvalidTransition(_))
    ));
}

#[test]
fn snapshot_is_atomic_owner_only_and_detects_tampering() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.snapshot");
    let snapshot = SessionSnapshot::new("s1", ReducedSessionState::default()).unwrap();
    write_snapshot(&path, &snapshot).unwrap();
    assert_eq!(load_snapshot(&path).unwrap(), snapshot);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let mut tampered = snapshot;
    tampered.state.turns.insert(
        "x".into(),
        TurnState {
            user_message: "changed".into(),
            completion: None,
        },
    );
    std::fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
    assert!(matches!(
        load_snapshot(path),
        Err(JournalError::SnapshotDigestMismatch)
    ));
}

#[test]
fn invalid_transition_is_not_written_or_assigned_a_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0")).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        journal.append(turn_started("t0")),
        Err(JournalError::InvalidTransition(_))
    ));
    assert!(matches!(
        journal.append(SessionEvent::ToolExecutionFinished {
            tool_execution_id: "missing".into(),
            outcome: CompletionOutcome::Succeeded,
            result: json!(null),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        journal
            .append(SessionEvent::TurnCancelled {
                turn_id: "t0".into(),
            })
            .unwrap()
            .seq,
        1
    );
}

#[test]
fn only_one_turn_can_be_active_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0")).unwrap();
    let before = std::fs::read(&path).unwrap();
    let error = journal.append(turn_started("t1")).unwrap_err();
    assert!(
        error.to_string().contains("turn t0 is still active"),
        "active turn must be named deterministically: {error}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);

    journal
        .append(SessionEvent::TurnFailed {
            turn_id: "t0".into(),
            error: "interrupted".into(),
        })
        .unwrap();
    journal.append(turn_started("t1")).unwrap();
    assert!(journal.state().unwrap().turns["t1"].completion.is_none());
}

#[test]
fn terminal_effects_are_singular_and_checkpoint_digest_is_verified() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();
    journal
        .append(tool_intent(
            "tool",
            "provider-call",
            "turn",
            0,
            "bash",
            json!({}),
            json!({}),
        ))
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ToolExecutionFinished {
            tool_execution_id: "tool".into(),
            outcome: CompletionOutcome::Succeeded,
            result: json!(null),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ToolExecutionStarted {
            tool_execution_id: "tool".into(),
        })
        .unwrap();
    let finished = SessionEvent::ToolExecutionFinished {
        tool_execution_id: "tool".into(),
        outcome: CompletionOutcome::Succeeded,
        result: json!({"ok":true}),
    };
    journal.append(finished.clone()).unwrap();
    assert!(matches!(
        journal.append(finished),
        Err(JournalError::InvalidTransition(_))
    ));

    let state = json!({"b": 2, "a": 1});
    assert!(matches!(
        journal.append(SessionEvent::CheckpointCommitted {
            checkpoint_id: "bad".into(),
            purpose: CheckpointPurpose::Recovery,
            origin: CheckpointOrigin::Session,
            state_digest: "wrong".into(),
            state: state.clone(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::CheckpointCommitted {
            checkpoint_id: "good".into(),
            purpose: CheckpointPurpose::Recovery,
            origin: CheckpointOrigin::Turn {
                turn_id: "turn".into(),
            },
            state_digest: state_payload_digest(&state).unwrap(),
            state,
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::CheckpointCommitted {
            checkpoint_id: "good".into(),
            purpose: CheckpointPurpose::Compaction,
            origin: CheckpointOrigin::Session,
            state_digest: state_payload_digest(&json!({"replacement":true})).unwrap(),
            state: json!({"replacement":true}),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    let reduced = journal.state().unwrap();
    let checkpoint = &reduced.checkpoints["good"];
    assert_eq!(checkpoint.purpose, CheckpointPurpose::Recovery);
    assert_eq!(
        checkpoint.origin,
        CheckpointOrigin::Turn {
            turn_id: "turn".into(),
        }
    );
}

#[test]
fn session_import_seeds_exact_structured_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let messages = json!([
        {"role":"user","content":[{"type":"text","text":"hello"}]},
        {"role":"assistant","content":[{"type":"thinking","thinking":"reason"}]}
    ]);
    let session = json!({
        "schema_version": 1,
        "id": "s1",
        "provider": "anthropic",
        "model": "claude",
        "messages": messages,
        "extra": {"preserved": true}
    });
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(imported(session.clone())).unwrap();
    let state = journal.state().unwrap();
    assert_eq!(state.conversation, messages.as_array().unwrap().clone());
    let baseline = state.imported_baseline.unwrap();
    assert_eq!(baseline.source_schema_version, 1);
    assert_eq!(baseline.imported_message_count, 2);
    assert_eq!(baseline.session, session);

    journal.append(turn_started("t0")).unwrap();
    let appended = json!({"role":"assistant","content":[{"type":"text","text":"next"}]});
    journal
        .append(message_committed("t0", 2, appended.clone()))
        .unwrap();
    assert_eq!(journal.state().unwrap().conversation[2], appended);
}

#[test]
fn invalid_or_nonfirst_session_import_never_writes() {
    let dir = tempfile::tempdir().unwrap();

    let wrong_id_path = dir.path().join("wrong-id.journal");
    let wrong_id = SessionJournal::open(&wrong_id_path, "s1").unwrap();
    let event = imported(json!({"schema_version":1,"id":"other","messages":[]}));
    assert!(matches!(
        wrong_id.append(event),
        Err(JournalError::SessionMismatch { .. })
    ));
    assert!(std::fs::read(&wrong_id_path).unwrap().is_empty());
    drop(wrong_id);

    let wrong_digest_path = dir.path().join("wrong-digest.journal");
    let wrong_digest = SessionJournal::open(&wrong_digest_path, "s1").unwrap();
    let session = json!({"schema_version":1,"id":"s1","messages":[]});
    assert!(matches!(
        wrong_digest.append(SessionEvent::SessionImported {
            source_schema_version: 1,
            session,
            session_digest: "wrong".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    assert!(std::fs::read(&wrong_digest_path).unwrap().is_empty());
    drop(wrong_digest);

    let nonfirst_path = dir.path().join("nonfirst.journal");
    let nonfirst = SessionJournal::open(&nonfirst_path, "s1").unwrap();
    nonfirst.append(turn_started("t0")).unwrap();
    let before = std::fs::read(&nonfirst_path).unwrap();
    let baseline = json!({"schema_version":1,"id":"s1","messages":[]});
    assert!(matches!(
        nonfirst.append(imported(baseline.clone())),
        Err(JournalError::InvalidTransition(_))
    ));
    assert_eq!(std::fs::read(&nonfirst_path).unwrap(), before);
    drop(nonfirst);

    let duplicate_path = dir.path().join("duplicate.journal");
    let duplicate = SessionJournal::open(&duplicate_path, "s1").unwrap();
    duplicate.append(imported(baseline.clone())).unwrap();
    let before = std::fs::read(&duplicate_path).unwrap();
    assert!(matches!(
        duplicate.append(imported(baseline)),
        Err(JournalError::InvalidTransition(_))
    ));
    assert_eq!(std::fs::read(&duplicate_path).unwrap(), before);
}

#[test]
fn structured_message_commit_preserves_tool_and_thinking_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal
        .append(imported(
            json!({"schema_version":1,"id":"s1","messages":[]}),
        ))
        .unwrap();
    journal.append(turn_started("t0")).unwrap();
    let message = json!({
        "role": "assistant",
        "content": [
            {"type":"thinking","thinking":"inspect first","signature":"sig"},
            {"type":"tool_use","id":"call-1","name":"Read","input":{"path":"x.rs"}},
            {"type":"tool_result","tool_use_id":"call-1","content":[{"type":"text","text":"bytes"}]}
        ],
        "provider_metadata": {"cache":"hit"}
    });
    journal
        .append(message_committed("t0", 0, message.clone()))
        .unwrap();
    assert_eq!(journal.state().unwrap().conversation, vec![message]);
}

#[test]
fn invalid_message_commit_never_advances_conversation_or_journal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal
        .append(imported(
            json!({"schema_version":1,"id":"s1","messages":[]}),
        ))
        .unwrap();
    journal.append(turn_started("t0")).unwrap();
    let message = json!({"role":"assistant","content":[]});
    let before = std::fs::read(&path).unwrap();

    for invalid in [
        message_committed("t0", 1, message.clone()),
        SessionEvent::ConversationMessageCommitted {
            turn_id: "t0".into(),
            message_index: 0,
            message: message.clone(),
            message_digest: "wrong".into(),
        },
        message_committed("unknown", 0, message.clone()),
    ] {
        assert!(matches!(
            journal.append(invalid),
            Err(JournalError::InvalidTransition(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(journal.state().unwrap().conversation.is_empty());
    }

    journal
        .append(SessionEvent::TurnCancelled {
            turn_id: "t0".into(),
        })
        .unwrap();
    let terminal = std::fs::read(&path).unwrap();
    assert!(matches!(
        journal.append(message_committed("t0", 0, message)),
        Err(JournalError::InvalidTransition(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), terminal);
    assert!(journal.state().unwrap().conversation.is_empty());
}

#[test]
fn conversation_state_commit_replaces_exactly_for_compaction_and_rebase() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal
        .append(imported(json!({
            "schema_version": 1,
            "id": "s1",
            "messages": [
                {"role":"user","content":[{"type":"text","text":"old question"}]},
                {"role":"assistant","content":[{"type":"thinking","thinking":"old reasoning"}]},
                {"role":"assistant","content":[{"type":"text","text":"old answer"}]}
            ]
        })))
        .unwrap();
    journal.append(turn_started("t0")).unwrap();

    let compacted = vec![
        json!({"role":"system","content":[{"type":"text","text":"summary"}]}),
        json!({"role":"user","content":[{"type":"text","text":"current question"}]}),
    ];
    journal
        .append(conversation_state_committed("t0", compacted.clone()))
        .unwrap();
    assert_eq!(journal.state().unwrap().conversation, compacted);

    let rebased = vec![json!({
        "role":"system",
        "content":[{"type":"text","text":"mutated summary"}],
        "metadata":{"rebase":true}
    })];
    journal
        .append(conversation_state_committed("t0", rebased.clone()))
        .unwrap();
    assert_eq!(journal.state().unwrap().conversation, rebased);
}

#[test]
fn invalid_conversation_state_commit_never_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal
        .append(imported(
            json!({"schema_version":1,"id":"s1","messages":[]}),
        ))
        .unwrap();
    journal.append(turn_started("t0")).unwrap();
    let valid_messages = vec![json!({"role":"assistant","content":[]})];
    let digest = state_payload_digest(&serde_json::Value::Array(valid_messages.clone())).unwrap();
    let before = std::fs::read(&path).unwrap();

    for invalid in [
        conversation_state_committed("unknown", valid_messages.clone()),
        conversation_state_committed("t0", vec![json!("not an object")]),
        SessionEvent::ConversationStateCommitted {
            turn_id: "t0".into(),
            messages: vec![
                json!({"role":"assistant","content":[{"type":"text","text":"mutated"}]}),
            ],
            messages_digest: digest,
        },
    ] {
        assert!(matches!(
            journal.append(invalid),
            Err(JournalError::InvalidTransition(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(journal.state().unwrap().conversation.is_empty());
    }

    journal
        .append(SessionEvent::TurnFailed {
            turn_id: "t0".into(),
            error: "stopped".into(),
        })
        .unwrap();
    let terminal = std::fs::read(&path).unwrap();
    assert!(matches!(
        journal.append(conversation_state_committed("t0", valid_messages)),
        Err(JournalError::InvalidTransition(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), terminal);
    assert!(journal.state().unwrap().conversation.is_empty());
}
