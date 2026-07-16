use serde_json::json;
use wcore_protocol::commands::{
    OperatorResolutionAuthority, OperatorResolutionValidationError, ProtocolCommand,
    RecoveredApprovalDecision, ResolveInterruptedApprovalCommand, ResumeTurnAction,
    ResumeTurnCommand, SessionResyncCommand,
};
use wcore_protocol::contract::generated_artifacts;
use wcore_protocol::events::{
    OperatorResolutionEvidence, OperatorResolutionEvidenceSource, OperatorToolEffectOutcome,
    OperatorToolEffectResolution, ProtocolEvent, RecoveryBudgetSnapshot, RecoveryCursor,
    RecoveryLifecycle, RecoveryReconcileReason, RecoveryReplayItem, RecoveryReplayKind,
    RecoveryTurnSnapshot, RecoveryUnavailableReason,
};

fn cursor(sequence: Option<u64>, digest: &str) -> RecoveryCursor {
    RecoveryCursor {
        journal_sequence: sequence,
        journal_digest: digest.to_owned(),
    }
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn journal_digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn operator_resolution() -> OperatorToolEffectResolution {
    OperatorToolEffectResolution {
        recovery_version: 1,
        session_id: "session-3".to_owned(),
        turn_id: "turn-2".to_owned(),
        cursor: cursor(Some(42), &journal_digest('a')),
        tool_execution_id: "tool-execution-9".to_owned(),
        outcome: OperatorToolEffectOutcome::Succeeded,
        operator_id: "operator-7".to_owned(),
        evidence: OperatorResolutionEvidence {
            source: OperatorResolutionEvidenceSource::ExternalSystemRecord,
            reference_id: "record-11".to_owned(),
            observed_at_unix_ms: 1_721_000_003_000,
            digest: digest('b'),
        },
    }
}

#[test]
fn session_resync_command_is_versioned_correlated_and_genesis_safe() {
    let command: ProtocolCommand = serde_json::from_value(json!({
        "type": "session_resync",
        "recovery_version": 1,
        "request_id": "request-7",
        "session_id": "session-3",
        "after": {
            "journal_digest": journal_digest('0')
        }
    }))
    .unwrap();

    assert_eq!(
        command,
        ProtocolCommand::SessionResync(SessionResyncCommand {
            recovery_version: 1,
            request_id: "request-7".to_owned(),
            session_id: "session-3".to_owned(),
            after: Some(cursor(None, &journal_digest('0'))),
        })
    );
    assert_eq!(
        serde_json::to_value(cursor(None, &journal_digest('0'))).unwrap(),
        json!({"journal_digest": journal_digest('0')})
    );
}

#[test]
fn resume_turn_command_binds_action_to_cursor() {
    let command: ProtocolCommand = serde_json::from_value(json!({
        "type": "resume_turn",
        "recovery_version": 1,
        "request_id": "request-8",
        "session_id": "session-3",
        "turn_id": "turn-2",
        "cursor": {
            "journal_sequence": 42,
            "journal_digest": journal_digest('1')
        },
        "action": "reconcile"
    }))
    .unwrap();

    assert_eq!(
        command,
        ProtocolCommand::ResumeTurn(ResumeTurnCommand {
            recovery_version: 1,
            request_id: "request-8".to_owned(),
            session_id: "session-3".to_owned(),
            turn_id: "turn-2".to_owned(),
            cursor: cursor(Some(42), &journal_digest('1')),
            action: ResumeTurnAction::Reconcile,
        })
    );
}

#[test]
fn resolve_interrupted_approval_is_versioned_request_cursor_and_approval_correlated() {
    let command: ProtocolCommand = serde_json::from_value(json!({
        "type": "resolve_interrupted_approval",
        "recovery_version": 1,
        "request_id": "request-9",
        "session_id": "session-3",
        "turn_id": "turn-2",
        "cursor": {
            "journal_sequence": 42,
            "journal_digest": journal_digest('2')
        },
        "approval_id": "approval-4",
        "decision": "approve",
        "answer": "Proceed"
    }))
    .unwrap();

    assert_eq!(
        command,
        ProtocolCommand::ResolveInterruptedApproval(ResolveInterruptedApprovalCommand {
            recovery_version: 1,
            request_id: "request-9".to_owned(),
            session_id: "session-3".to_owned(),
            turn_id: "turn-2".to_owned(),
            cursor: cursor(Some(42), &journal_digest('2')),
            approval_id: "approval-4".to_owned(),
            decision: RecoveredApprovalDecision::Approve,
            answer: Some("Proceed".to_owned()),
        })
    );
}

#[test]
fn recovery_commands_reject_unversioned_uncorrelated_or_unknown_actions() {
    for invalid in [
        json!({
            "type": "session_resync",
            "request_id": "request-7",
            "session_id": "session-3"
        }),
        json!({
            "type": "session_resync",
            "recovery_version": 1,
            "session_id": "session-3"
        }),
        json!({
            "type": "resume_turn",
            "recovery_version": 1,
            "request_id": "request-8",
            "session_id": "session-3",
            "turn_id": "turn-2",
            "cursor": {
                "journal_sequence": 42,
                "journal_digest": journal_digest('1')
            },
            "action": "claim_effect_succeeded"
        }),
    ] {
        assert!(serde_json::from_value::<ProtocolCommand>(invalid).is_err());
    }
}

#[test]
fn recovery_commands_reject_unknown_top_level_authority_fields() {
    let valid_commands = [
        json!({
            "type": "session_resync",
            "recovery_version": 1,
            "request_id": "request-7",
            "session_id": "session-3"
        }),
        json!({
            "type": "resume_turn",
            "recovery_version": 1,
            "request_id": "request-8",
            "session_id": "session-3",
            "turn_id": "turn-2",
            "cursor": {
                "journal_sequence": 42,
                "journal_digest": journal_digest('1')
            },
            "action": "reconcile"
        }),
        json!({
            "type": "resolve_interrupted_approval",
            "recovery_version": 1,
            "request_id": "request-9",
            "session_id": "session-3",
            "turn_id": "turn-2",
            "cursor": {
                "journal_sequence": 42,
                "journal_digest": journal_digest('2')
            },
            "approval_id": "approval-4",
            "decision": "deny"
        }),
    ];

    for mut command in valid_commands {
        command["future_authority"] = json!({"silently_ignored": true});
        assert!(
            serde_json::from_value::<ProtocolCommand>(command).is_err(),
            "recovery commands must reject unknown authority-bearing fields"
        );
    }
}

#[test]
fn operator_resolution_is_typed_cursor_bound_and_receipted() {
    let resolution = operator_resolution();
    let command: ProtocolCommand = serde_json::from_value(json!({
        "type": "resolve_unknown_tool_effect",
        "recovery_version": resolution.recovery_version,
        "session_id": resolution.session_id,
        "turn_id": resolution.turn_id,
        "cursor": resolution.cursor,
        "tool_execution_id": resolution.tool_execution_id,
        "outcome": "succeeded",
        "operator_id": resolution.operator_id,
        "evidence": resolution.evidence,
    }))
    .unwrap();
    let expected_cursor = cursor(Some(42), &journal_digest('a'));
    command
        .validate_operator_resolution(&OperatorResolutionAuthority {
            session_id: "session-3",
            turn_id: "turn-2",
            cursor: &expected_cursor,
            tool_execution_id: "tool-execution-9",
        })
        .unwrap();

    let ProtocolCommand::ResolveUnknownToolEffect(resolution) = command else {
        panic!("typed operator-resolution command was not preserved");
    };
    let receipt = serde_json::to_value(ProtocolEvent::UnknownToolEffectResolved { resolution })
        .expect("operator-resolution receipt must serialize");
    assert_eq!(receipt["type"], "unknown_tool_effect_resolved");
    assert_eq!(receipt["session_id"], "session-3");
    assert_eq!(receipt["cursor"]["journal_sequence"], 42);
    assert_eq!(receipt["tool_execution_id"], "tool-execution-9");
    assert_eq!(receipt["outcome"], "succeeded");
    assert_eq!(receipt["evidence"]["source"], "external_system_record");
}

#[test]
fn operator_resolution_rejects_unknown_critical_and_malformed_claims() {
    let valid = serde_json::to_value(operator_resolution()).unwrap();
    let mut command = valid.as_object().unwrap().clone();
    command.insert("type".to_owned(), json!("resolve_unknown_tool_effect"));

    let mutations: [fn(&mut serde_json::Map<String, serde_json::Value>); 4] = [
        |value: &mut serde_json::Map<String, serde_json::Value>| {
            value.insert("future_authority_rule".to_owned(), json!(true));
        },
        |value: &mut serde_json::Map<String, serde_json::Value>| {
            value.insert("outcome".to_owned(), json!("partially_succeeded"));
        },
        |value: &mut serde_json::Map<String, serde_json::Value>| {
            value["evidence"]["source"] = json!("future_receipt");
        },
        |value: &mut serde_json::Map<String, serde_json::Value>| {
            value["cursor"]["future_authority"] = json!("unsupported");
        },
    ];
    for mutate in mutations {
        let mut invalid = command.clone();
        mutate(&mut invalid);
        assert!(
            serde_json::from_value::<ProtocolCommand>(invalid.into()).is_err(),
            "unknown authority-bearing fields and enums must fail closed"
        );
    }

    let expected_cursor = cursor(Some(42), &journal_digest('a'));
    let authority = OperatorResolutionAuthority {
        session_id: "session-3",
        turn_id: "turn-2",
        cursor: &expected_cursor,
        tool_execution_id: "tool-execution-9",
    };
    let mut malformed = command.clone();
    malformed.insert("operator_id".to_owned(), json!(""));
    let malformed: ProtocolCommand = serde_json::from_value(malformed.into()).unwrap();
    assert_eq!(
        malformed.validate_operator_resolution(&authority),
        Err(OperatorResolutionValidationError::Malformed {
            field: "operator_id"
        })
    );

    let mut wrong_version = command;
    wrong_version.insert("recovery_version".to_owned(), json!(2));
    let wrong_version: ProtocolCommand = serde_json::from_value(wrong_version.into()).unwrap();
    assert_eq!(
        wrong_version.validate_operator_resolution(&authority),
        Err(OperatorResolutionValidationError::UnsupportedVersion { actual: 2 })
    );
}

#[test]
fn operator_resolution_rejects_noncanonical_cursor_digests() {
    let resolution = operator_resolution();
    let authority_cursor = resolution.cursor.clone();
    let authority = OperatorResolutionAuthority {
        session_id: &resolution.session_id,
        turn_id: &resolution.turn_id,
        cursor: &authority_cursor,
        tool_execution_id: &resolution.tool_execution_id,
    };

    for invalid_digest in [
        digest('a'),
        "A".repeat(64),
        "a".repeat(63),
        format!("{}g", "a".repeat(63)),
    ] {
        let mut invalid = resolution.clone();
        invalid.cursor.journal_digest = invalid_digest;
        assert_eq!(
            ProtocolCommand::ResolveUnknownToolEffect(invalid)
                .validate_operator_resolution(&authority),
            Err(OperatorResolutionValidationError::Malformed {
                field: "cursor.journal_digest"
            })
        );
    }
}

#[test]
fn operator_resolution_rejects_every_stale_authority_dimension() {
    let resolution = operator_resolution();
    let command = ProtocolCommand::ResolveUnknownToolEffect(resolution.clone());

    let stale_cursor = cursor(Some(41), &journal_digest('c'));
    for (authority, field) in [
        (
            OperatorResolutionAuthority {
                session_id: "other-session",
                turn_id: &resolution.turn_id,
                cursor: &resolution.cursor,
                tool_execution_id: &resolution.tool_execution_id,
            },
            "session_id",
        ),
        (
            OperatorResolutionAuthority {
                session_id: &resolution.session_id,
                turn_id: "other-turn",
                cursor: &resolution.cursor,
                tool_execution_id: &resolution.tool_execution_id,
            },
            "turn_id",
        ),
        (
            OperatorResolutionAuthority {
                session_id: &resolution.session_id,
                turn_id: &resolution.turn_id,
                cursor: &resolution.cursor,
                tool_execution_id: "other-tool-execution",
            },
            "tool_execution_id",
        ),
        (
            OperatorResolutionAuthority {
                session_id: &resolution.session_id,
                turn_id: &resolution.turn_id,
                cursor: &stale_cursor,
                tool_execution_id: &resolution.tool_execution_id,
            },
            "cursor",
        ),
    ] {
        assert_eq!(
            command.validate_operator_resolution(&authority),
            Err(OperatorResolutionValidationError::Stale { field })
        );
    }
}

#[test]
fn generated_contract_contains_versioned_operator_resolution_command_and_event() {
    let artifacts = generated_artifacts().unwrap();
    let command: serde_json::Value = serde_json::from_slice(
        artifacts
            .get("commands/resolve_unknown_tool_effect.json")
            .unwrap(),
    )
    .unwrap();
    let event: serde_json::Value = serde_json::from_slice(
        artifacts
            .get("events/unknown_tool_effect_resolved.json")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(command["recovery_version"], 1);
    assert_eq!(event["recovery_version"], 1);
    assert_eq!(command["cursor"], event["cursor"]);
    assert_eq!(command["evidence"], event["evidence"]);

    for schema_path in [
        "schema/host-command.schema.json",
        "schema/core-event.schema.json",
    ] {
        let schema: serde_json::Value =
            serde_json::from_slice(artifacts.get(schema_path).unwrap()).unwrap();
        let branch = schema["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|branch| {
                matches!(
                    branch["properties"]["type"]["const"].as_str(),
                    Some("resolve_unknown_tool_effect" | "unknown_tool_effect_resolved")
                )
            })
            .expect("operator-resolution schema branch must exist");
        assert_eq!(branch["additionalProperties"], false);
        assert_eq!(branch["properties"]["recovery_version"]["const"], 1);
        assert_eq!(
            branch["properties"]["outcome"]["enum"],
            json!(["succeeded", "failed", "not_started"])
        );
        assert_eq!(
            branch["properties"]["evidence"]["additionalProperties"],
            false
        );
        assert_eq!(
            branch["properties"]["cursor"]["additionalProperties"],
            false
        );
        assert_eq!(
            branch["properties"]["cursor"]["properties"]["journal_digest"]["pattern"],
            "^[0-9a-f]{64}$"
        );
        assert_eq!(
            branch["properties"]["evidence"]["properties"]["digest"]["pattern"],
            "^sha256:[0-9a-f]{64}$"
        );
    }
}

#[test]
fn generated_recovery_command_contract_is_closed_raw_digest_and_correlated() {
    let artifacts = generated_artifacts().unwrap();
    let command: serde_json::Value = serde_json::from_slice(
        artifacts
            .get("commands/resolve_interrupted_approval.json")
            .expect("interrupted approval fixture must exist"),
    )
    .unwrap();
    assert_eq!(command["type"], "resolve_interrupted_approval");
    assert_eq!(command["decision"], "approve");
    assert_eq!(command["cursor"]["journal_digest"], journal_digest('6'));
    serde_json::from_value::<ProtocolCommand>(command)
        .expect("canonical interrupted approval must deserialize");

    let command_schema: serde_json::Value = serde_json::from_slice(
        artifacts
            .get("schema/host-command.schema.json")
            .expect("host command schema must exist"),
    )
    .unwrap();
    for (wire_type, cursor_field) in [
        ("session_resync", "after"),
        ("resume_turn", "cursor"),
        ("resolve_interrupted_approval", "cursor"),
    ] {
        let branch = command_schema["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|branch| branch["properties"]["type"]["const"] == wire_type)
            .unwrap_or_else(|| panic!("missing schema branch for {wire_type}"));
        assert_eq!(branch["additionalProperties"], false);
        assert_eq!(branch["properties"]["recovery_version"]["const"], 1);
        assert_eq!(
            branch["properties"][cursor_field]["additionalProperties"],
            false
        );
        assert_eq!(
            branch["properties"][cursor_field]["properties"]["journal_digest"]["pattern"],
            "^[0-9a-f]{64}$"
        );
    }

    let interrupted = command_schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["properties"]["type"]["const"] == "resolve_interrupted_approval")
        .unwrap();
    assert_eq!(
        interrupted["properties"]["decision"]["enum"],
        json!(["approve", "deny"])
    );

    let event_schema: serde_json::Value = serde_json::from_slice(
        artifacts
            .get("schema/core-event.schema.json")
            .expect("core event schema must exist"),
    )
    .unwrap();
    let snapshot = event_schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|branch| branch["properties"]["type"]["const"] == "session_recovery_snapshot")
        .expect("recovery snapshot schema branch must exist");
    assert_eq!(
        snapshot["properties"]["state_digest"]["pattern"],
        "^[0-9a-f]{64}$"
    );

    let manifest: serde_json::Value = serde_json::from_slice(
        artifacts
            .get("manifest.json")
            .expect("contract manifest must exist"),
    )
    .unwrap();
    let manifest_command = manifest["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["type"] == "resolve_interrupted_approval")
        .expect("manifest must inventory interrupted approval");
    assert_eq!(manifest_command["criticality"], "safety");
    assert_eq!(
        manifest_command["correlation"],
        "request_id_cursor_and_approval_id"
    );
}

#[test]
fn recovery_snapshot_serializes_only_sanitized_typed_state() {
    let event = ProtocolEvent::SessionRecoverySnapshot {
        recovery_version: 1,
        request_id: "request-7".to_owned(),
        session_id: "session-3".to_owned(),
        cursor: cursor(Some(42), &journal_digest('c')),
        state_digest: journal_digest('d'),
        lifecycle: RecoveryLifecycle::ReconciliationRequired,
        pending_turn: Some(RecoveryTurnSnapshot {
            turn_id: "turn-2".to_owned(),
            msg_id: Some("message-2".to_owned()),
            lifecycle: RecoveryLifecycle::ReconciliationRequired,
            pending_call_id: Some("call-9".to_owned()),
            reconcile_reason: Some(RecoveryReconcileReason::ToolOutcomeUnknown),
        }),
        budget: RecoveryBudgetSnapshot {
            tokens_used: 12_000,
            token_limit: Some(20_000),
            cost_used_usd: 1.25,
            cost_limit_usd: Some(5.0),
        },
    };

    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({
            "type": "session_recovery_snapshot",
            "recovery_version": 1,
            "request_id": "request-7",
            "session_id": "session-3",
            "cursor": {
                "journal_sequence": 42,
                "journal_digest": journal_digest('c')
            },
            "state_digest": journal_digest('d'),
            "lifecycle": "reconciliation_required",
            "pending_turn": {
                "turn_id": "turn-2",
                "msg_id": "message-2",
                "lifecycle": "reconciliation_required",
                "pending_call_id": "call-9",
                "reconcile_reason": "tool_outcome_unknown"
            },
            "budget": {
                "tokens_used": 12000,
                "token_limit": 20000,
                "cost_used_usd": 1.25,
                "cost_limit_usd": 5.0
            }
        })
    );
}

#[test]
fn recovery_replay_is_ordered_and_content_free() {
    let event = ProtocolEvent::SessionRecoveryReplay {
        recovery_version: 1,
        request_id: "request-7".to_owned(),
        session_id: "session-3".to_owned(),
        from: Some(cursor(Some(40), &journal_digest('4'))),
        through: cursor(Some(42), &journal_digest('6')),
        items: vec![
            RecoveryReplayItem {
                cursor: cursor(Some(41), &journal_digest('5')),
                turn_id: Some("turn-2".to_owned()),
                kind: RecoveryReplayKind::ToolStarted,
            },
            RecoveryReplayItem {
                cursor: cursor(Some(42), &journal_digest('6')),
                turn_id: Some("turn-2".to_owned()),
                kind: RecoveryReplayKind::EffectUncertain,
            },
        ],
    };

    let wire = serde_json::to_value(event).unwrap();
    assert_eq!(wire["type"], "session_recovery_replay");
    assert_eq!(wire["items"][0]["kind"], "tool_started");
    assert_eq!(wire["items"][1]["kind"], "effect_uncertain");
    for forbidden in [
        "content",
        "prompt",
        "arguments",
        "output",
        "path",
        "resume_token",
    ] {
        assert!(!wire.to_string().contains(forbidden));
    }
}

#[test]
fn recovery_refusal_and_lifecycle_use_fail_closed_reason_enums() {
    let unavailable = serde_json::to_value(ProtocolEvent::SessionRecoveryUnavailable {
        recovery_version: 1,
        request_id: "request-9".to_owned(),
        session_id: "session-3".to_owned(),
        reason: RecoveryUnavailableReason::CursorDigestMismatch,
    })
    .unwrap();
    assert_eq!(unavailable["reason"], "cursor_digest_mismatch");

    let lifecycle = serde_json::to_value(ProtocolEvent::TurnRecoveryLifecycle {
        recovery_version: 1,
        session_id: "session-3".to_owned(),
        turn_id: "turn-2".to_owned(),
        cursor: cursor(Some(42), &journal_digest('c')),
        lifecycle: RecoveryLifecycle::ReconciliationRequired,
        reconcile_reason: Some(RecoveryReconcileReason::UnknownCriticalState),
    })
    .unwrap();
    assert_eq!(lifecycle["type"], "turn_recovery_lifecycle");
    assert_eq!(lifecycle["reconcile_reason"], "unknown_critical_state");

    assert!(
        serde_json::from_str::<RecoveryUnavailableReason>("\"future_reason\"").is_err(),
        "unknown recovery refusal reasons must not silently default"
    );
    assert!(
        serde_json::from_str::<RecoveryReconcileReason>("\"future_reason\"").is_err(),
        "unknown reconciliation reasons must not silently default"
    );
}
