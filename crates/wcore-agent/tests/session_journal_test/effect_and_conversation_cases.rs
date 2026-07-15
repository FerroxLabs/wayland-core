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
