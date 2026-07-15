use wcore_protocol::workflow::{
    WorkflowReplayAcceptance, WorkflowReplayError, WorkflowReplayReducer,
    validate_workflow_contract_version,
};

fn start(event_id: &str) -> String {
    start_with(event_id, "wf", 1)
}

fn start_with(event_id: &str, workflow_id: &str, node_count: u64) -> String {
    format!(
        r#"{{"type":"workflow_started","workflow_id":"{workflow_id}","name":"Audit","node_count":{node_count},"run_id":"run-1","event_id":"{event_id}","sequence":0}}"#
    )
}

fn node(event_id: &str, sequence: u64, state: &str) -> String {
    node_with_child(event_id, sequence, state, None)
}

fn node_with_child(
    event_id: &str,
    sequence: u64,
    state: &str,
    child_run_id: Option<&str>,
) -> String {
    let child_run_id = child_run_id
        .map(|id| format!(",\"child_run_id\":\"{id}\""))
        .unwrap_or_default();
    format!(
        r#"{{"type":"workflow_node_event","run_id":"run-1","node_id":"scan","event_id":"{event_id}","sequence":{sequence},"state":"{state}"{child_run_id}}}"#
    )
}

fn child(
    event_id: &str,
    child_sequence: u64,
    parent_call_id: &str,
    agent_name: &str,
    inner_type: &str,
    terminal_state: Option<&str>,
) -> String {
    child_with_parent(
        event_id,
        child_sequence,
        parent_call_id,
        agent_name,
        inner_type,
        terminal_state,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn child_with_parent(
    event_id: &str,
    child_sequence: u64,
    parent_call_id: &str,
    agent_name: &str,
    inner_type: &str,
    terminal_state: Option<&str>,
    parent_child_run_id: Option<&str>,
) -> String {
    let terminal_state = terminal_state
        .map(|state| format!(",\"terminal_state\":\"{state}\""))
        .unwrap_or_default();
    let parent_child_run_id = parent_child_run_id
        .map(|id| format!(",\"parent_child_run_id\":\"{id}\""))
        .unwrap_or_default();
    format!(
        r#"{{"type":"sub_agent_event","parent_call_id":"{parent_call_id}","agent_name":"{agent_name}","inner":{{"type":"{inner_type}"}},"run_id":"run-1","child_run_id":"child-1","child_sequence":{child_sequence},"event_id":"{event_id}"{terminal_state}{parent_child_run_id}}}"#
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
    reducer
        .accept_json(&child(
            "child-0",
            0,
            "workflow:scan",
            "scan",
            "text_delta",
            None,
        ))
        .unwrap();
    assert!(matches!(
        reducer.accept_json(&child(
            "child-2",
            2,
            "workflow:scan",
            "scan",
            "text_delta",
            None,
        )),
        Err(WorkflowReplayError::ChildOutOfOrder {
            expected: 1,
            actual: 2,
            ..
        })
    ));
    assert_eq!(
        reducer.accept_json(&child(
            "child-1",
            1,
            "workflow:scan",
            "other",
            "text_delta",
            None,
        )),
        Err(WorkflowReplayError::ChildCorrelationChanged {
            child_run_id: "child-1".to_string()
        })
    );
}

#[test]
fn child_parent_lineage_is_nonempty_and_immutable() {
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    reducer
        .accept_json(&child_with_parent(
            "child-0",
            0,
            "workflow:scan",
            "scan",
            "text_delta",
            None,
            Some("parent-child-a"),
        ))
        .unwrap();
    assert_eq!(
        reducer.accept_json(&child_with_parent(
            "child-1",
            1,
            "workflow:scan",
            "scan",
            "text_delta",
            None,
            Some("parent-child-b"),
        )),
        Err(WorkflowReplayError::ChildCorrelationChanged {
            child_run_id: "child-1".to_string()
        })
    );

    let mut malformed = WorkflowReplayReducer::new("1.0").unwrap();
    malformed.accept_json(&start("malformed-0")).unwrap();
    assert_eq!(
        malformed.accept_json(&child_with_parent(
            "malformed-child",
            0,
            "workflow:scan",
            "scan",
            "text_delta",
            None,
            Some(""),
        )),
        Err(WorkflowReplayError::Malformed {
            field: "parent_child_run_id"
        })
    );
}

#[test]
fn finish_requires_original_workflow_identity_and_exact_node_inventory() {
    let mut identity = WorkflowReplayReducer::new("1.0").unwrap();
    identity.accept_json(&start("event-0")).unwrap();
    identity
        .accept_json(&node("event-1", 1, "succeeded"))
        .unwrap();
    assert_eq!(
        identity
            .accept_json(&finish("event-2", 2, "succeeded", true).replace("\"wf\"", "\"other\"")),
        Err(WorkflowReplayError::WorkflowIdentityChanged {
            run_id: "run-1".to_string(),
        })
    );

    let mut inventory = WorkflowReplayReducer::new("1.0").unwrap();
    inventory
        .accept_json(&start_with("inventory-0", "wf", 2))
        .unwrap();
    inventory
        .accept_json(&node("inventory-1", 1, "succeeded"))
        .unwrap();
    assert_eq!(
        inventory.accept_json(&finish("inventory-2", 2, "succeeded", true)),
        Err(WorkflowReplayError::NodeCountMismatch {
            run_id: "run-1".to_string(),
            expected: 2,
            actual: 1,
        })
    );
}

#[test]
fn successful_finish_rejects_failed_nodes() {
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    reducer.accept_json(&node("event-1", 1, "failed")).unwrap();
    assert_eq!(
        reducer.accept_json(&finish("event-2", 2, "succeeded", true)),
        Err(WorkflowReplayError::SuccessfulRunHasFailedNodes {
            run_id: "run-1".to_string(),
            node_ids: vec!["scan".to_string()],
        })
    );
}

#[test]
fn successful_finish_allows_blocked_non_live_branches() {
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    reducer.accept_json(&node("event-1", 1, "blocked")).unwrap();
    assert_eq!(
        reducer
            .accept_json(&finish("event-2", 2, "succeeded", true))
            .unwrap(),
        WorkflowReplayAcceptance::Advanced
    );
}

#[test]
fn child_terminal_is_typed_conflict_checked_and_absorbing() {
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    reducer
        .accept_json(&child(
            "child-0",
            0,
            "workflow:scan",
            "scan",
            "text_delta",
            None,
        ))
        .unwrap();
    assert_eq!(
        reducer
            .accept_json(&child(
                "child-1",
                1,
                "workflow:scan",
                "scan",
                "info",
                Some("succeeded"),
            ))
            .unwrap(),
        WorkflowReplayAcceptance::Advanced
    );
    assert_eq!(
        reducer
            .accept_json(&child(
                "child-2",
                2,
                "workflow:scan",
                "scan",
                "text_delta",
                None,
            ))
            .unwrap(),
        WorkflowReplayAcceptance::IgnoredAfterChildTerminal
    );
    assert_eq!(
        reducer.accept_json(&child(
            "child-3",
            3,
            "workflow:scan",
            "scan",
            "error",
            Some("failed"),
        )),
        Err(WorkflowReplayError::ConflictingChildTerminal {
            child_run_id: "child-1".to_string(),
        })
    );

    let mut mismatch = WorkflowReplayReducer::new("1.0").unwrap();
    mismatch.accept_json(&start("mismatch-0")).unwrap();
    assert_eq!(
        mismatch.accept_json(&child(
            "mismatch-1",
            0,
            "workflow:scan",
            "scan",
            "error",
            Some("succeeded"),
        )),
        Err(WorkflowReplayError::ChildTerminalTypeMismatch {
            child_run_id: "child-1".to_string(),
        })
    );
}

#[test]
fn child_parent_and_node_binding_are_strict() {
    let mut invalid_parent = WorkflowReplayReducer::new("1.0").unwrap();
    invalid_parent.accept_json(&start("event-0")).unwrap();
    assert_eq!(
        invalid_parent.accept_json(&child(
            "child-0",
            0,
            "call:scan",
            "scan",
            "text_delta",
            None,
        )),
        Err(WorkflowReplayError::InvalidChildParent {
            child_run_id: "child-1".to_string(),
        })
    );

    let mut changed = WorkflowReplayReducer::new("1.0").unwrap();
    changed.accept_json(&start("changed-0")).unwrap();
    changed
        .accept_json(&node_with_child(
            "changed-1",
            1,
            "running",
            Some("child-other"),
        ))
        .unwrap();
    assert_eq!(
        changed.accept_json(&child(
            "changed-child",
            0,
            "workflow:scan",
            "scan",
            "text_delta",
            None,
        )),
        Err(WorkflowReplayError::NodeChildCorrelationChanged {
            run_id: "run-1".to_string(),
            node_id: "scan".to_string(),
        })
    );
}

#[test]
fn finish_requires_terminal_children_and_explicit_consistent_links() {
    let mut active = WorkflowReplayReducer::new("1.0").unwrap();
    active.accept_json(&start("active-0")).unwrap();
    active
        .accept_json(&node_with_child("active-1", 1, "running", Some("child-1")))
        .unwrap();
    active
        .accept_json(&child(
            "active-child",
            0,
            "workflow:scan",
            "scan",
            "text_delta",
            None,
        ))
        .unwrap();
    active
        .accept_json(&node_with_child(
            "active-2",
            2,
            "succeeded",
            Some("child-1"),
        ))
        .unwrap();
    assert_eq!(
        active.accept_json(&finish("active-3", 3, "succeeded", true)),
        Err(WorkflowReplayError::ChildrenStillActive {
            run_id: "run-1".to_string(),
            child_run_ids: vec!["child-1".to_string()],
        })
    );

    let mut unlinked = WorkflowReplayReducer::new("1.0").unwrap();
    unlinked.accept_json(&start("unlinked-0")).unwrap();
    unlinked
        .accept_json(&child(
            "unlinked-child",
            0,
            "workflow:scan",
            "scan",
            "info",
            Some("succeeded"),
        ))
        .unwrap();
    unlinked
        .accept_json(&node("unlinked-1", 1, "succeeded"))
        .unwrap();
    assert_eq!(
        unlinked.accept_json(&finish("unlinked-2", 2, "succeeded", true)),
        Err(WorkflowReplayError::ChildNotLinked {
            run_id: "run-1".to_string(),
            child_run_id: "child-1".to_string(),
            node_id: "scan".to_string(),
        })
    );

    let mut inconsistent = WorkflowReplayReducer::new("1.0").unwrap();
    inconsistent.accept_json(&start("inconsistent-0")).unwrap();
    inconsistent
        .accept_json(&child(
            "inconsistent-child",
            0,
            "workflow:scan",
            "scan",
            "error",
            Some("failed"),
        ))
        .unwrap();
    inconsistent
        .accept_json(&node_with_child(
            "inconsistent-1",
            1,
            "succeeded",
            Some("child-1"),
        ))
        .unwrap();
    assert_eq!(
        inconsistent.accept_json(&finish("inconsistent-2", 2, "succeeded", true)),
        Err(WorkflowReplayError::SucceededNodeHasFailedChild {
            run_id: "run-1".to_string(),
            child_run_id: "child-1".to_string(),
            node_id: "scan".to_string(),
        })
    );
}

#[test]
fn child_evidence_cannot_arrive_after_its_node_terminal() {
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    reducer
        .accept_json(&node_with_child("event-1", 1, "running", Some("child-1")))
        .unwrap();
    reducer
        .accept_json(&node_with_child("event-2", 2, "succeeded", Some("child-1")))
        .unwrap();
    assert_eq!(
        reducer.accept_json(&child(
            "child-late",
            0,
            "workflow:scan",
            "scan",
            "error",
            Some("failed"),
        )),
        Err(WorkflowReplayError::ChildAfterNodeTerminal {
            run_id: "run-1".to_string(),
            child_run_id: "child-1".to_string(),
            node_id: "scan".to_string(),
        })
    );
}

#[test]
fn fully_correlated_child_run_replays_to_completion() {
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    reducer.accept_json(&start("event-0")).unwrap();
    reducer
        .accept_json(&node_with_child("event-1", 1, "running", Some("child-1")))
        .unwrap();
    reducer
        .accept_json(&child(
            "child-0",
            0,
            "workflow:scan",
            "scan",
            "info",
            Some("succeeded"),
        ))
        .unwrap();
    reducer
        .accept_json(&node_with_child("event-2", 2, "succeeded", Some("child-1")))
        .unwrap();
    assert_eq!(
        reducer
            .accept_json(&finish("event-3", 3, "succeeded", true))
            .unwrap(),
        WorkflowReplayAcceptance::Advanced
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
