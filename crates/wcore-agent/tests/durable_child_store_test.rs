use std::collections::BTreeMap;
use std::fs;

use serde_json::json;
use wcore_agent::durable_child::{DurableChildStore, DurableChildWrite};
use wcore_agent::session_journal::{
    CompletionOutcome, ExternalEffectState, SessionEvent, SessionJournal,
};
use wcore_types::spawner::{
    ChildDeliveryReconciliation, ChildDeliveryState, ChildDeliveryTarget, ChildDesiredState,
    ChildId, ChildOrigin, ChildParent, ChildPolicySnapshot, ChildRecoveryState,
    ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildResult, DurableChildStatus,
    DurableChildTransition, MAX_DURABLE_CHILD_ID_BYTES,
};

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn child_record(id: &str, delivery: bool) -> DurableChildRecord {
    DurableChildRecord {
        schema_version: DURABLE_CHILD_SCHEMA_VERSION,
        declaration_id: format!("declare-{id}"),
        child_id: ChildId::new(id).unwrap(),
        parent: ChildParent {
            session_id: "session-1".into(),
            turn_id: None,
            parent_child_id: None,
            workflow_run_id: None,
            graph_node_id: None,
            parent_call_id: None,
        },
        origin: ChildOrigin::Spawn,
        request: ChildRequestEvidence::redacted(digest('a')),
        policy_snapshot: ChildPolicySnapshot {
            contract_version: "effective-execution-policy/v1".into(),
            exact_digest: digest('b'),
            posture: "standard".into(),
            approvals: "ask".into(),
            sandbox: "workspace-write".into(),
            source: "session-effective-policy".into(),
            managed_floor_active: true,
            dangerous_activation_id_digest: None,
        },
        provider: Some("openai".into()),
        model: Some("gpt-test".into()),
        workspace: ChildWorkspace {
            mode: ChildWorkspaceMode::Isolated,
            workspace_id: "workspace-1".into(),
        },
        status: DurableChildStatus::Prepared,
        desired_state: ChildDesiredState::Run,
        recovery: ChildRecoveryState::Clean,
        revision: 0,
        timestamps: ChildTimestamps {
            created_at_unix_ms: 100,
            updated_at_unix_ms: 100,
            queued_at_unix_ms: None,
            started_at_unix_ms: None,
            terminal_at_unix_ms: None,
        },
        result: None,
        delivery_target: delivery.then_some(ChildDeliveryTarget::SessionOutbox),
        delivery_state: if delivery {
            ChildDeliveryState::Pending
        } else {
            ChildDeliveryState::NotRequired
        },
        attempt: 1,
        retry_of: None,
        applied_events: BTreeMap::new(),
    }
}

fn result() -> DurableChildResult {
    DurableChildResult {
        exact_digest: digest('c'),
        turns: 3,
        input_tokens: 100,
        output_tokens: 50,
        artifact_digests: vec![digest('d')],
    }
}

fn assert_declaration_rejected_without_write(
    store: &DurableChildStore,
    journal: &SessionJournal,
    journal_path: &std::path::Path,
    record: DurableChildRecord,
) {
    let len = fs::metadata(journal_path).unwrap().len();
    let seq = journal.state().unwrap().last_seq;
    assert!(store.declare(record).is_err());
    assert_eq!(fs::metadata(journal_path).unwrap().len(), len);
    assert_eq!(journal.state().unwrap().last_seq, seq);
}

#[test]
fn durable_child_lifecycle_is_idempotent_and_survives_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let journal_path = temp.path().join("session.journal");
    let child_id = ChildId::new("child-1").unwrap();

    {
        let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
        let store = DurableChildStore::new(journal.clone());
        let declaration = child_record("child-1", true);
        store.declare(declaration.clone()).unwrap();
        store
            .transition(
                child_id.clone(),
                "enqueue-1",
                0,
                101,
                DurableChildTransition::Enqueue,
            )
            .unwrap();
        let transition_retry_len = fs::metadata(&journal_path).unwrap().len();
        let transition_retry_seq = journal.state().unwrap().last_seq;
        let transition_retry = store
            .transition(
                child_id.clone(),
                "enqueue-1",
                0,
                101,
                DurableChildTransition::Enqueue,
            )
            .unwrap();
        assert_eq!(transition_retry, DurableChildWrite::AlreadyCommitted);
        assert_eq!(
            fs::metadata(&journal_path).unwrap().len(),
            transition_retry_len
        );
        assert_eq!(journal.state().unwrap().last_seq, transition_retry_seq);
        assert_eq!(store.inspect(&child_id).unwrap().unwrap().revision, 1);
        let declaration_retry_len = fs::metadata(&journal_path).unwrap().len();
        let declaration_retry_seq = journal.state().unwrap().last_seq;
        assert_eq!(
            store.declare(declaration.clone()).unwrap(),
            DurableChildWrite::AlreadyCommitted
        );
        assert_eq!(
            fs::metadata(&journal_path).unwrap().len(),
            declaration_retry_len
        );
        assert_eq!(journal.state().unwrap().last_seq, declaration_retry_seq);
        assert_eq!(store.inspect(&child_id).unwrap().unwrap().revision, 1);
        let mut conflicting_declaration = declaration;
        conflicting_declaration.workspace.workspace_id = "other-workspace".into();
        let conflict_len = fs::metadata(&journal_path).unwrap().len();
        let conflict_seq = journal.state().unwrap().last_seq;
        assert!(store.declare(conflicting_declaration).is_err());
        assert_eq!(fs::metadata(&journal_path).unwrap().len(), conflict_len);
        assert_eq!(journal.state().unwrap().last_seq, conflict_seq);
        store
            .transition(
                child_id.clone(),
                "start-1",
                1,
                102,
                DurableChildTransition::Start,
            )
            .unwrap();
        store
            .transition(
                child_id.clone(),
                "succeed-1",
                2,
                103,
                DurableChildTransition::Succeed { result: result() },
            )
            .unwrap();
        store
            .transition(
                child_id.clone(),
                "delivery-start-1",
                3,
                104,
                DurableChildTransition::DeliveryStarted,
            )
            .unwrap();
        store
            .transition(
                child_id.clone(),
                "delivery-done-1",
                4,
                105,
                DurableChildTransition::DeliveryDelivered {
                    receipt_digest: digest('e'),
                },
            )
            .unwrap();
        store
            .transition(
                child_id.clone(),
                "expire-1",
                5,
                106,
                DurableChildTransition::Expire,
            )
            .unwrap();
    }

    let reopened = SessionJournal::open(&journal_path, "session-1").unwrap();
    let store = DurableChildStore::new(reopened);
    let child = store.inspect(&child_id).unwrap().unwrap();
    assert_eq!(child.status, DurableChildStatus::Expired);
    assert_eq!(child.revision, 6);
    assert_eq!(child.applied_events.len(), 6);
    assert_eq!(store.list().unwrap(), vec![child]);
}

#[test]
fn stale_conflicting_and_post_terminal_transitions_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let journal_path = temp.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
    let store = DurableChildStore::new(journal.clone());
    let child_id = ChildId::new("child-1").unwrap();
    store.declare(child_record("child-1", false)).unwrap();
    store
        .transition(
            child_id.clone(),
            "enqueue-1",
            0,
            101,
            DurableChildTransition::Enqueue,
        )
        .unwrap();

    let conflict_len = fs::metadata(&journal_path).unwrap().len();
    let conflict_seq = journal.state().unwrap().last_seq;
    assert!(
        store
            .transition(
                child_id.clone(),
                "stale-1",
                0,
                102,
                DurableChildTransition::Start,
            )
            .is_err()
    );
    assert_eq!(fs::metadata(&journal_path).unwrap().len(), conflict_len);
    assert_eq!(journal.state().unwrap().last_seq, conflict_seq);
    assert!(
        store
            .transition(
                child_id.clone(),
                "enqueue-1",
                0,
                102,
                DurableChildTransition::Enqueue,
            )
            .is_err()
    );
    assert_eq!(store.inspect(&child_id).unwrap().unwrap().revision, 1);

    store
        .transition(
            child_id.clone(),
            "start-1",
            1,
            103,
            DurableChildTransition::Start,
        )
        .unwrap();
    store
        .transition(
            child_id.clone(),
            "succeed-1",
            2,
            104,
            DurableChildTransition::Succeed { result: result() },
        )
        .unwrap();
    assert!(
        store
            .transition(
                child_id.clone(),
                "restart-1",
                3,
                105,
                DurableChildTransition::Start,
            )
            .is_err()
    );
    let child = store.inspect(&child_id).unwrap().unwrap();
    assert_eq!(child.status, DurableChildStatus::Succeeded);
    assert_eq!(child.revision, 3);
}

#[test]
fn legacy_children_replay_without_becoming_v2_authority() {
    let temp = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(temp.path().join("session.journal"), "session-1").unwrap();
    journal
        .append(SessionEvent::TurnStarted {
            turn_id: "turn-1".into(),
            user_message: "hello".into(),
        })
        .unwrap();
    journal
        .append(SessionEvent::ChildPrepared {
            child_id: "legacy-child".into(),
            turn_id: "turn-1".into(),
            request: json!({"legacy": true}),
        })
        .unwrap();
    journal
        .append(SessionEvent::ChildStarted {
            child_id: "legacy-child".into(),
        })
        .unwrap();
    journal
        .append(SessionEvent::ChildFinished {
            child_id: "legacy-child".into(),
            outcome: CompletionOutcome::Succeeded,
            result: json!({"ok": true}),
        })
        .unwrap();

    let child = journal.state().unwrap().children["legacy-child"].clone();
    assert!(child.durable.is_none());
    assert!(matches!(
        child.effect,
        ExternalEffectState::Completed {
            outcome: CompletionOutcome::Succeeded
        }
    ));
}

#[test]
fn durable_schema_rejects_unbounded_ids_and_never_persists_prompt_plaintext() {
    let oversized = "x".repeat(MAX_DURABLE_CHILD_ID_BYTES + 1);
    assert!(ChildId::new(&oversized).is_err());
    assert!(serde_json::from_value::<ChildId>(json!(oversized)).is_err());
    assert!(ChildId::new(" child").is_err());
    assert!(serde_json::from_value::<ChildId>(json!("child\n")).is_err());

    const PROMPT_CANARY: &str = "TOP-SECRET-PROMPT-CANARY";
    let temp = tempfile::tempdir().unwrap();
    let journal_path = temp.path().join("session.journal");
    {
        let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
        DurableChildStore::new(journal)
            .declare(child_record("child-1", false))
            .unwrap();
    }
    let bytes = fs::read(journal_path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(PROMPT_CANARY));
    let serialized = serde_json::to_string(&child_record("child-2", false)).unwrap();
    assert!(!serialized.contains(PROMPT_CANARY));
    assert!(!serialized.contains("ciphertext"));
}

#[test]
fn declaration_rejects_missing_parent_and_invalid_retry_sequence() {
    let temp = tempfile::tempdir().unwrap();
    let journal_path = temp.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
    let store = DurableChildStore::new(journal.clone());

    let mut foreign_session = child_record("foreign-child", false);
    foreign_session.parent.session_id = "session-2".into();
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, foreign_session);

    let mut missing_parent = child_record("child-2", false);
    missing_parent.parent.parent_child_id = Some(ChildId::new("missing").unwrap());
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, missing_parent);

    let mut missing_retry = child_record("missing-retry", false);
    missing_retry.attempt = 2;
    missing_retry.retry_of = Some(ChildId::new("missing").unwrap());
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, missing_retry);

    store.declare(child_record("child-1", false)).unwrap();
    let mut invalid_retry = child_record("child-2", false);
    invalid_retry.attempt = 2;
    invalid_retry.retry_of = Some(ChildId::new("child-1").unwrap());
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, invalid_retry);

    let mut reused_declaration = child_record("child-3", false);
    reused_declaration.declaration_id = "declare-child-1".into();
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, reused_declaration);
}

#[test]
fn recovery_preserves_pause_and_cancel_intent_and_terminal_race_truth() {
    let temp = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(temp.path().join("session.journal"), "session-1").unwrap();
    let store = DurableChildStore::new(journal);

    let paused_id = ChildId::new("paused-child").unwrap();
    store
        .declare(child_record(paused_id.as_str(), false))
        .unwrap();
    for (event_id, revision, time, transition) in [
        ("p-enqueue", 0, 101, DurableChildTransition::Enqueue),
        ("p-start", 1, 102, DurableChildTransition::Start),
        (
            "p-request-pause",
            2,
            103,
            DurableChildTransition::RequestPause,
        ),
        (
            "p-recovery",
            3,
            104,
            DurableChildTransition::RequireRecovery {
                reason_digest: digest('1'),
            },
        ),
        (
            "p-resolve",
            4,
            105,
            DurableChildTransition::ResolveRecovery {
                evidence_digest: digest('2'),
            },
        ),
    ] {
        store
            .transition(paused_id.clone(), event_id, revision, time, transition)
            .unwrap();
    }
    let paused = store.inspect(&paused_id).unwrap().unwrap();
    assert_eq!(paused.status, DurableChildStatus::Paused);
    assert_eq!(paused.desired_state, ChildDesiredState::Pause);

    let cancelled_id = ChildId::new("cancelled-child").unwrap();
    store
        .declare(child_record(cancelled_id.as_str(), false))
        .unwrap();
    for (event_id, revision, time, transition) in [
        ("c-enqueue", 0, 101, DurableChildTransition::Enqueue),
        ("c-start", 1, 102, DurableChildTransition::Start),
        (
            "c-request-cancel",
            2,
            103,
            DurableChildTransition::RequestCancel,
        ),
        (
            "c-recovery",
            3,
            104,
            DurableChildTransition::RequireRecovery {
                reason_digest: digest('3'),
            },
        ),
        (
            "c-cancel-recovery",
            4,
            105,
            DurableChildTransition::CancelAfterRecovery {
                evidence_digest: digest('4'),
            },
        ),
    ] {
        store
            .transition(cancelled_id.clone(), event_id, revision, time, transition)
            .unwrap();
    }
    assert_eq!(
        store.inspect(&cancelled_id).unwrap().unwrap().status,
        DurableChildStatus::Cancelled
    );

    let race_id = ChildId::new("race-child").unwrap();
    store
        .declare(child_record(race_id.as_str(), false))
        .unwrap();
    for (event_id, revision, time, transition) in [
        ("r-enqueue", 0, 101, DurableChildTransition::Enqueue),
        ("r-start", 1, 102, DurableChildTransition::Start),
        (
            "r-request-cancel",
            2,
            103,
            DurableChildTransition::RequestCancel,
        ),
        (
            "r-observed-success",
            3,
            104,
            DurableChildTransition::Succeed { result: result() },
        ),
    ] {
        store
            .transition(race_id.clone(), event_id, revision, time, transition)
            .unwrap();
    }
    let raced = store.inspect(&race_id).unwrap().unwrap();
    assert_eq!(raced.status, DurableChildStatus::Succeeded);
    assert_eq!(raced.desired_state, ChildDesiredState::Cancel);

    let recovered_success_id = ChildId::new("recovered-success-child").unwrap();
    store
        .declare(child_record(recovered_success_id.as_str(), false))
        .unwrap();
    for (event_id, revision, time, transition) in [
        ("s-enqueue", 0, 101, DurableChildTransition::Enqueue),
        ("s-start", 1, 102, DurableChildTransition::Start),
        (
            "s-recovery",
            2,
            103,
            DurableChildTransition::RequireRecovery {
                reason_digest: digest('b'),
            },
        ),
        (
            "s-observed-success",
            3,
            104,
            DurableChildTransition::SucceedAfterRecovery { result: result() },
        ),
    ] {
        store
            .transition(
                recovered_success_id.clone(),
                event_id,
                revision,
                time,
                transition,
            )
            .unwrap();
    }
    assert_eq!(
        store
            .inspect(&recovered_success_id)
            .unwrap()
            .unwrap()
            .status,
        DurableChildStatus::Succeeded
    );
}

#[test]
fn delivery_failure_and_unknown_outcomes_require_content_bound_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(temp.path().join("session.journal"), "session-1").unwrap();
    let store = DurableChildStore::new(journal);
    let child_id = ChildId::new("delivery-child").unwrap();
    store
        .declare(child_record(child_id.as_str(), true))
        .unwrap();
    for (event_id, revision, time, transition) in [
        ("enqueue", 0, 101, DurableChildTransition::Enqueue),
        ("start", 1, 102, DurableChildTransition::Start),
        (
            "succeed",
            2,
            103,
            DurableChildTransition::Succeed { result: result() },
        ),
        (
            "delivery-start",
            3,
            104,
            DurableChildTransition::DeliveryStarted,
        ),
        (
            "delivery-failed",
            4,
            105,
            DurableChildTransition::DeliveryFailed {
                error_digest: digest('5'),
            },
        ),
    ] {
        store
            .transition(child_id.clone(), event_id, revision, time, transition)
            .unwrap();
    }
    assert!(
        store
            .transition(
                child_id.clone(),
                "retry-wrong",
                5,
                106,
                DurableChildTransition::RetryFailedDelivery {
                    prior_error_digest: digest('6'),
                },
            )
            .is_err()
    );
    for (event_id, revision, time, transition) in [
        (
            "retry-right",
            5,
            107,
            DurableChildTransition::RetryFailedDelivery {
                prior_error_digest: digest('5'),
            },
        ),
        (
            "delivery-restart",
            6,
            108,
            DurableChildTransition::DeliveryStarted,
        ),
        (
            "delivery-unknown",
            7,
            109,
            DurableChildTransition::DeliveryUnknown {
                evidence_digest: digest('7'),
            },
        ),
    ] {
        store
            .transition(child_id.clone(), event_id, revision, time, transition)
            .unwrap();
    }
    assert!(
        store
            .transition(
                child_id.clone(),
                "reconcile-wrong",
                8,
                110,
                DurableChildTransition::ReconcileUnknownDelivery {
                    prior_evidence_digest: digest('8'),
                    resolution: ChildDeliveryReconciliation::Delivered {
                        receipt_digest: digest('9'),
                    },
                },
            )
            .is_err()
    );
    store
        .transition(
            child_id.clone(),
            "reconcile-not-delivered",
            8,
            111,
            DurableChildTransition::ReconcileUnknownDelivery {
                prior_evidence_digest: digest('7'),
                resolution: ChildDeliveryReconciliation::NotDelivered {
                    proof_digest: digest('a'),
                },
            },
        )
        .unwrap();
    assert_eq!(
        store.inspect(&child_id).unwrap().unwrap().delivery_state,
        ChildDeliveryState::Pending
    );
}

#[test]
fn parent_child_delivery_is_bound_to_the_declared_parent() {
    let temp = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(temp.path().join("session.journal"), "session-1").unwrap();
    let store = DurableChildStore::new(journal);
    store.declare(child_record("parent", false)).unwrap();
    store.declare(child_record("sibling", false)).unwrap();

    let mut mismatched = child_record("child", true);
    mismatched.parent.parent_child_id = Some(ChildId::new("parent").unwrap());
    mismatched.delivery_target = Some(ChildDeliveryTarget::ParentChild {
        child_id: ChildId::new("sibling").unwrap(),
    });
    assert!(store.declare(mismatched).is_err());

    let mut bound = child_record("child", true);
    bound.parent.parent_child_id = Some(ChildId::new("parent").unwrap());
    bound.delivery_target = Some(ChildDeliveryTarget::ParentChild {
        child_id: ChildId::new("parent").unwrap(),
    });
    store.declare(bound).unwrap();
}

#[test]
fn parent_expiry_is_a_tombstone_and_preserves_descendant_lineage() {
    let temp = tempfile::tempdir().unwrap();
    let journal_path = temp.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
    let store = DurableChildStore::new(journal.clone());
    let parent_id = ChildId::new("parent").unwrap();
    store.declare(child_record("parent", false)).unwrap();
    store
        .transition(
            parent_id.clone(),
            "parent-enqueue",
            0,
            101,
            DurableChildTransition::Enqueue,
        )
        .unwrap();
    store
        .transition(
            parent_id.clone(),
            "parent-start",
            1,
            102,
            DurableChildTransition::Start,
        )
        .unwrap();

    let mut child = child_record("child", false);
    child.parent.parent_child_id = Some(parent_id.clone());
    store.declare(child).unwrap();

    store
        .transition(
            parent_id.clone(),
            "parent-succeed",
            2,
            103,
            DurableChildTransition::Succeed { result: result() },
        )
        .unwrap();
    store
        .transition(
            parent_id.clone(),
            "parent-expire",
            3,
            104,
            DurableChildTransition::Expire,
        )
        .unwrap();

    assert_eq!(
        store.inspect(&parent_id).unwrap().unwrap().status,
        DurableChildStatus::Expired
    );
    let descendant = store
        .inspect(&ChildId::new("child").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(descendant.status, DurableChildStatus::Prepared);
    assert_eq!(descendant.parent.parent_child_id, Some(parent_id));

    let mut late_child = child_record("late-child", false);
    late_child.parent.parent_child_id = Some(ChildId::new("parent").unwrap());
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, late_child);
}

#[test]
fn retry_binds_immutable_authority_but_allows_provider_failover() {
    let temp = tempfile::tempdir().unwrap();
    let journal_path = temp.path().join("session.journal");
    let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
    let store = DurableChildStore::new(journal.clone());
    let source_id = ChildId::new("retry-source").unwrap();
    store
        .declare(child_record(source_id.as_str(), false))
        .unwrap();
    for (event_id, revision, time, transition) in [
        ("source-enqueue", 0, 101, DurableChildTransition::Enqueue),
        ("source-start", 1, 102, DurableChildTransition::Start),
        (
            "source-fail",
            2,
            103,
            DurableChildTransition::Fail { result: result() },
        ),
    ] {
        store
            .transition(source_id.clone(), event_id, revision, time, transition)
            .unwrap();
    }

    let retry_record = |id: &str| {
        let mut record = child_record(id, false);
        record.attempt = 2;
        record.retry_of = Some(source_id.clone());
        record
    };

    let mut changed_origin = retry_record("retry-origin");
    changed_origin.origin = ChildOrigin::Delegate;
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, changed_origin);

    let mut changed_request = retry_record("retry-request");
    changed_request.request = ChildRequestEvidence::redacted(digest('9'));
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, changed_request);

    let mut changed_policy = retry_record("retry-policy");
    changed_policy.policy_snapshot.exact_digest = digest('8');
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, changed_policy);

    let mut changed_workspace = retry_record("retry-workspace");
    changed_workspace.workspace.workspace_id = "other-workspace".into();
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, changed_workspace);

    let mut changed_delivery = retry_record("retry-delivery");
    changed_delivery.delivery_target = Some(ChildDeliveryTarget::SessionOutbox);
    changed_delivery.delivery_state = ChildDeliveryState::Pending;
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, changed_delivery);

    store.declare(child_record("other-parent", false)).unwrap();
    let mut changed_parent = retry_record("retry-parent");
    changed_parent.parent.parent_child_id = Some(ChildId::new("other-parent").unwrap());
    assert_declaration_rejected_without_write(&store, &journal, &journal_path, changed_parent);

    let mut provider_failover = retry_record("retry-valid");
    provider_failover.provider = Some("anthropic".into());
    provider_failover.model = Some("claude-test".into());
    store.declare(provider_failover).unwrap();
}

fn assert_record_invariants(record: &DurableChildRecord) {
    assert_eq!(
        usize::try_from(record.revision).unwrap(),
        record.applied_events.len()
    );
    assert!(record.timestamps.updated_at_unix_ms >= record.timestamps.created_at_unix_ms);
    assert!(
        record
            .timestamps
            .queued_at_unix_ms
            .is_none_or(|time| time >= record.timestamps.created_at_unix_ms)
    );
    assert!(
        record
            .timestamps
            .started_at_unix_ms
            .is_none_or(|time| time >= record.timestamps.created_at_unix_ms)
    );
    assert_eq!(
        record.status.is_terminal(),
        record.timestamps.terminal_at_unix_ms.is_some()
    );
    match record.status {
        DurableChildStatus::Succeeded | DurableChildStatus::Failed => {
            assert!(record.result.is_some());
        }
        DurableChildStatus::Expired => {}
        _ => assert!(record.result.is_none()),
    }
    assert_eq!(
        record.status == DurableChildStatus::RecoveryRequired,
        matches!(record.recovery, ChildRecoveryState::Required { .. })
    );
    assert!(matches!(
        (&record.delivery_target, &record.delivery_state),
        (None, ChildDeliveryState::NotRequired)
            | (Some(_), ChildDeliveryState::Pending)
            | (Some(_), ChildDeliveryState::InFlight)
            | (Some(_), ChildDeliveryState::Delivered { .. })
            | (Some(_), ChildDeliveryState::Failed { .. })
            | (Some(_), ChildDeliveryState::Unknown { .. })
    ));
}

fn transition_catalog() -> Vec<DurableChildTransition> {
    vec![
        DurableChildTransition::Enqueue,
        DurableChildTransition::Start,
        DurableChildTransition::RequestPause,
        DurableChildTransition::Paused,
        DurableChildTransition::Resume,
        DurableChildTransition::RequestCancel,
        DurableChildTransition::Succeed { result: result() },
        DurableChildTransition::Fail { result: result() },
        DurableChildTransition::Cancel,
        DurableChildTransition::RequireRecovery {
            reason_digest: digest('1'),
        },
        DurableChildTransition::ResolveRecovery {
            evidence_digest: digest('2'),
        },
        DurableChildTransition::SucceedAfterRecovery { result: result() },
        DurableChildTransition::FailAfterRecovery { result: result() },
        DurableChildTransition::CancelAfterRecovery {
            evidence_digest: digest('4'),
        },
        DurableChildTransition::DeliveryStarted,
        DurableChildTransition::DeliveryDelivered {
            receipt_digest: digest('e'),
        },
        DurableChildTransition::DeliveryFailed {
            error_digest: digest('5'),
        },
        DurableChildTransition::DeliveryUnknown {
            evidence_digest: digest('7'),
        },
        DurableChildTransition::RetryFailedDelivery {
            prior_error_digest: digest('5'),
        },
        DurableChildTransition::ReconcileUnknownDelivery {
            prior_evidence_digest: digest('7'),
            resolution: ChildDeliveryReconciliation::NotDelivered {
                proof_digest: digest('a'),
            },
        },
        DurableChildTransition::ReconcileUnknownDelivery {
            prior_evidence_digest: digest('7'),
            resolution: ChildDeliveryReconciliation::Delivered {
                receipt_digest: digest('e'),
            },
        },
        DurableChildTransition::ReconcileUnknownDelivery {
            prior_evidence_digest: digest('7'),
            resolution: ChildDeliveryReconciliation::Failed {
                error_digest: digest('5'),
            },
        },
        DurableChildTransition::Expire,
    ]
}

#[test]
fn bounded_model_exploration_preserves_state_machine_and_storage_invariants() {
    let enqueue = DurableChildTransition::Enqueue;
    let start = DurableChildTransition::Start;
    let request_pause = DurableChildTransition::RequestPause;
    let request_cancel = DurableChildTransition::RequestCancel;
    let succeed = DurableChildTransition::Succeed { result: result() };
    let delivery_start = DurableChildTransition::DeliveryStarted;
    let prefixes = vec![
        vec![],
        vec![enqueue.clone()],
        vec![enqueue.clone(), start.clone()],
        vec![enqueue.clone(), start.clone(), request_pause.clone()],
        vec![
            enqueue.clone(),
            start.clone(),
            request_pause.clone(),
            DurableChildTransition::Paused,
        ],
        vec![
            enqueue.clone(),
            start.clone(),
            request_pause.clone(),
            DurableChildTransition::RequireRecovery {
                reason_digest: digest('1'),
            },
        ],
        vec![enqueue.clone(), start.clone(), request_cancel.clone()],
        vec![
            enqueue.clone(),
            start.clone(),
            request_cancel.clone(),
            DurableChildTransition::RequireRecovery {
                reason_digest: digest('1'),
            },
        ],
        vec![
            enqueue.clone(),
            start.clone(),
            DurableChildTransition::RequireRecovery {
                reason_digest: digest('1'),
            },
        ],
        vec![enqueue.clone(), start.clone(), succeed.clone()],
        vec![
            enqueue.clone(),
            start.clone(),
            succeed.clone(),
            delivery_start.clone(),
        ],
        vec![
            enqueue.clone(),
            start.clone(),
            succeed.clone(),
            delivery_start.clone(),
            DurableChildTransition::DeliveryFailed {
                error_digest: digest('5'),
            },
        ],
        vec![
            enqueue.clone(),
            start.clone(),
            succeed.clone(),
            delivery_start.clone(),
            DurableChildTransition::DeliveryUnknown {
                evidence_digest: digest('7'),
            },
        ],
        vec![
            enqueue.clone(),
            start.clone(),
            succeed.clone(),
            delivery_start,
            DurableChildTransition::DeliveryDelivered {
                receipt_digest: digest('e'),
            },
        ],
        vec![
            enqueue.clone(),
            start.clone(),
            succeed,
            DurableChildTransition::DeliveryStarted,
            DurableChildTransition::DeliveryDelivered {
                receipt_digest: digest('e'),
            },
            DurableChildTransition::Expire,
        ],
        vec![
            enqueue,
            start,
            request_cancel,
            DurableChildTransition::Cancel,
        ],
    ];

    for (prefix_index, prefix) in prefixes.into_iter().enumerate() {
        for (candidate_index, candidate) in transition_catalog().into_iter().enumerate() {
            let temp = tempfile::tempdir().unwrap();
            let journal_path = temp.path().join("session.journal");
            let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
            let store = DurableChildStore::new(journal.clone());
            let child_id = ChildId::new("model-child").unwrap();
            store
                .declare(child_record(child_id.as_str(), true))
                .unwrap();
            for (step, transition) in prefix.iter().cloned().enumerate() {
                let revision = store.inspect(&child_id).unwrap().unwrap().revision;
                store
                    .transition(
                        child_id.clone(),
                        format!("prefix-{step}"),
                        revision,
                        101 + u64::try_from(step).unwrap(),
                        transition,
                    )
                    .unwrap_or_else(|error| {
                        panic!(
                            "invalid model prefix {prefix_index} step {step}: {prefix:?}: {error}"
                        )
                    });
            }

            let before = store.inspect(&child_id).unwrap().unwrap();
            assert_record_invariants(&before);
            let before_seq = journal.state().unwrap().last_seq;
            let before_len = fs::metadata(&journal_path).unwrap().len();
            let at_unix_ms = 200 + u64::try_from(candidate_index).unwrap();
            let result = store.transition(
                child_id.clone(),
                "candidate",
                before.revision,
                at_unix_ms,
                candidate.clone(),
            );
            match result {
                Ok(DurableChildWrite::Appended(_)) => {
                    let after = store.inspect(&child_id).unwrap().unwrap();
                    assert_record_invariants(&after);
                    assert_eq!(after.revision, before.revision + 1);
                    let replay_seq = journal.state().unwrap().last_seq;
                    let replay_len = fs::metadata(&journal_path).unwrap().len();
                    assert_eq!(
                        store
                            .transition(
                                child_id.clone(),
                                "candidate",
                                before.revision,
                                at_unix_ms,
                                candidate,
                            )
                            .unwrap(),
                        DurableChildWrite::AlreadyCommitted
                    );
                    assert_eq!(journal.state().unwrap().last_seq, replay_seq);
                    assert_eq!(fs::metadata(&journal_path).unwrap().len(), replay_len);
                }
                Ok(DurableChildWrite::AlreadyCommitted) => {
                    panic!("fresh model event unexpectedly reported already committed");
                }
                Err(_) => {
                    assert_eq!(store.inspect(&child_id).unwrap().unwrap(), before);
                    assert_eq!(journal.state().unwrap().last_seq, before_seq);
                    assert_eq!(fs::metadata(&journal_path).unwrap().len(), before_len);
                }
            }
        }
    }
}
