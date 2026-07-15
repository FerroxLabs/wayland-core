use wcore_protocol::workflow::{
    WorkflowReplayAcceptance, WorkflowReplayError, WorkflowReplayReducer,
    validate_workflow_contract_version,
};

fn start(event_id: &str) -> String {
    format!(
        r#"{{"type":"workflow_started","workflow_id":"wf","name":"Audit","node_count":1,"run_id":"run-1","event_id":"{event_id}","sequence":0}}"#
    )
}

fn node(event_id: &str, sequence: u64, state: &str) -> String {
    format!(
        r#"{{"type":"workflow_node_event","run_id":"run-1","node_id":"scan","event_id":"{event_id}","sequence":{sequence},"state":"{state}"}}"#
    )
}

fn finish(event_id: &str, sequence: u64, terminal: &str, succeeded: bool) -> String {
    format!(
        r#"{{"type":"workflow_finished","workflow_id":"wf","succeeded":{succeeded},"run_id":"run-1","event_id":"{event_id}","sequence":{sequence},"terminal_state":"{terminal}"}}"#
    )
}

#[test]
fn serialized_replay_accepts_canonical_duplicates_and_complete_run() {
    let mut reducer = WorkflowReplayReducer::new("1.7").unwrap();
    assert_eq!(
        reducer.accept_json(&start("event-0")).unwrap(),
        WorkflowReplayAcceptance::Advanced
    );
    // Field order and whitespace are not semantic: canonical bytes match.
    assert_eq!(
        reducer
            .accept_json(
                r#"{ "sequence":0, "event_id":"event-0", "run_id":"run-1", "node_count":1, "name":"Audit", "workflow_id":"wf", "type":"workflow_started" }"#,
            )
            .unwrap(),
        WorkflowReplayAcceptance::Duplicate
    );
    assert_eq!(
        reducer.accept_json(&node("event-1", 1, "queued")).unwrap(),
        WorkflowReplayAcceptance::Advanced
    );
    assert_eq!(
        reducer
            .accept_json(&node("event-2", 2, "succeeded"))
            .unwrap(),
        WorkflowReplayAcceptance::Advanced
    );
    assert_eq!(
        reducer
            .accept_json(&finish("event-3", 3, "succeeded", true))
            .unwrap(),
        WorkflowReplayAcceptance::Advanced
    );
}

#[test]
fn conflicting_event_identity_and_sequence_gaps_fail_closed() {
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    let conflict = start("event-0").replace("Audit", "Changed");
    assert_eq!(
        reducer.accept_json(&conflict),
        Err(WorkflowReplayError::ConflictingDuplicate {
            event_id: "event-0".to_string()
        })
    );

    assert_eq!(
        reducer.accept_json(&node("event-gap", 2, "running")),
        Err(WorkflowReplayError::OutOfOrder {
            run_id: "run-1".to_string(),
            expected: 1,
            actual: 2,
        })
    );
    // A rejected gap does not consume the expected sequence.
    assert_eq!(
        reducer.accept_json(&node("event-1", 1, "running")).unwrap(),
        WorkflowReplayAcceptance::Advanced
    );
    assert!(matches!(
        reducer.accept_json(&node("event-regression", 1, "succeeded")),
        Err(WorkflowReplayError::OutOfOrder {
            expected: 2,
            actual: 1,
            ..
        })
    ));
}

#[test]
fn node_and_run_terminals_are_absorbing() {
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    reducer
        .accept_json(&node("event-1", 1, "succeeded"))
        .unwrap();
    assert_eq!(
        reducer.accept_json(&node("event-2", 2, "running")).unwrap(),
        WorkflowReplayAcceptance::IgnoredAfterNodeTerminal
    );
    assert_eq!(
        reducer
            .accept_json(&finish("event-3", 3, "succeeded", true))
            .unwrap(),
        WorkflowReplayAcceptance::Advanced
    );
    assert_eq!(
        reducer.accept_json(&node("event-4", 4, "running")).unwrap(),
        WorkflowReplayAcceptance::IgnoredAfterRunTerminal
    );
    assert_eq!(
        reducer.accept_json(&finish("event-5", 4, "failed", false)),
        Err(WorkflowReplayError::ConflictingRunTerminal {
            run_id: "run-1".to_string()
        })
    );
}

#[test]
fn conflicting_node_terminal_and_active_finish_fail_closed() {
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    reducer.accept_json(&node("event-1", 1, "failed")).unwrap();
    assert_eq!(
        reducer.accept_json(&node("event-2", 2, "blocked")),
        Err(WorkflowReplayError::ConflictingNodeTerminal {
            run_id: "run-1".to_string(),
            node_id: "scan".to_string(),
        })
    );

    let mut active = WorkflowReplayReducer::new("1.0").unwrap();
    active.accept_json(&start("active-0")).unwrap();
    active.accept_json(&node("active-1", 1, "running")).unwrap();
    assert_eq!(
        active.accept_json(&finish("active-2", 2, "failed", false)),
        Err(WorkflowReplayError::NodesStillActive {
            run_id: "run-1".to_string(),
            node_ids: vec!["scan".to_string()],
        })
    );
}

#[test]
fn child_sequences_and_identity_are_strict() {
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    let child = |event_id: &str, sequence: u64, agent_name: &str| {
        format!(
            r#"{{"type":"sub_agent_event","parent_call_id":"workflow:scan","agent_name":"{agent_name}","inner":{{"type":"text_delta"}},"run_id":"run-1","child_run_id":"child-1","child_sequence":{sequence},"event_id":"{event_id}"}}"#
        )
    };
    reducer.accept_json(&child("child-0", 0, "scan")).unwrap();
    assert!(matches!(
        reducer.accept_json(&child("child-2", 2, "scan")),
        Err(WorkflowReplayError::ChildOutOfOrder {
            expected: 1,
            actual: 2,
            ..
        })
    ));
    assert_eq!(
        reducer.accept_json(&child("child-1", 1, "other")),
        Err(WorkflowReplayError::ChildCorrelationChanged {
            child_run_id: "child-1".to_string()
        })
    );
}

#[test]
fn unsupported_version_and_unreachable_states_are_rejected() {
    assert!(matches!(
        validate_workflow_contract_version("2.0"),
        Err(WorkflowReplayError::UnsupportedContractVersion { .. })
    ));
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    assert_eq!(
        reducer.accept_json(&node("event-1", 1, "suspended")),
        Err(WorkflowReplayError::Malformed { field: "state" })
    );
    assert_eq!(
        reducer
            .accept_json(r#"{"type":"text_delta","text":"hi","msg_id":"m"}"#)
            .unwrap(),
        WorkflowReplayAcceptance::Unrelated
    );
}
