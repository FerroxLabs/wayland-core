use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde_json::Value;
use wcore_protocol::anvil::{
    AnvilApplyOutcome, AnvilReceiptError, AnvilReceiptReducer, AnvilReceiptStatus,
};
use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::contract::{
    CONTRACT_ROOT, ContractCapabilityStatus, HostContractObserver, HostObservation,
    HostObservationError, producer_contract_descriptor,
};
use wcore_protocol::execution_policy::validate_execution_policy_contract_version;
use wcore_protocol::workflow::{
    WorkflowReplayAcceptance, WorkflowReplayError, WorkflowReplayReducer,
};

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACT_ROOT)
}

fn observer() -> HostContractObserver {
    HostContractObserver::new(producer_contract_descriptor())
}

fn negotiate(observer: &mut HostContractObserver) {
    let ready = fs::read(root().join("events/ready.json")).unwrap();
    assert!(matches!(
        observer.observe_json_line(&ready),
        Ok(HostObservation::Negotiated(_))
    ));
}

fn json_lines(relative: &str) -> Vec<Value> {
    fs::read_to_string(root().join(relative))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn raw_lines(relative: &str) -> Vec<String> {
    fs::read_to_string(root().join(relative))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn malformed_and_unknown_commands_never_deserialize_for_dispatch() {
    for name in [
        "invalid-json.jsonl",
        "missing-type.jsonl",
        "non-object.jsonl",
        "non-string-type.jsonl",
        "unknown-type.jsonl",
        "wrong-required-field.jsonl",
    ] {
        let bytes = fs::read(root().join("adversarial/commands").join(name)).unwrap();
        assert!(
            serde_json::from_slice::<ProtocolCommand>(&bytes).is_err(),
            "{name} unexpectedly produced a dispatchable command"
        );
    }
}

#[test]
fn negotiated_host_drops_serialized_unknown_noncritical_event() {
    let mut observer = observer();
    negotiate(&mut observer);
    let line = fs::read(root().join("adversarial/events/unknown-noncritical.jsonl")).unwrap();
    assert_eq!(
        observer.observe_json_line(&line),
        Ok(HostObservation::DroppedUnknownNonCritical {
            event_type: "future_observation".into()
        })
    );
}

#[test]
fn negotiated_host_rejects_unknown_critical_and_unclassified_events() {
    for (name, expected) in [
        (
            "unknown-critical.jsonl",
            HostObservationError::UnknownCriticalEvent {
                event_type: "future_authority".into(),
            },
        ),
        (
            "unknown-criticality.jsonl",
            HostObservationError::UnknownCriticality {
                event_type: "future_unclassified".into(),
            },
        ),
    ] {
        let mut observer = observer();
        negotiate(&mut observer);
        let line = fs::read(root().join("adversarial/events").join(name)).unwrap();
        assert_eq!(observer.observe_json_line(&line), Err(expected), "{name}");
    }
}

#[test]
fn ready_replay_fails_closed_on_major_schema_and_fixture_mismatch() {
    for (name, expected) in [
        (
            "version-mismatch.jsonl",
            HostObservationError::UnsupportedContractMajor { actual: 2 },
        ),
        (
            "schema-mismatch.jsonl",
            HostObservationError::SchemaDigestMismatch,
        ),
        (
            "fixture-mismatch.jsonl",
            HostObservationError::FixtureDigestMismatch,
        ),
    ] {
        let line = fs::read(root().join("adversarial/events").join(name)).unwrap();
        assert_eq!(observer().observe_json_line(&line), Err(expected), "{name}");
    }
}

#[test]
fn remaining_deferrals_exclude_live_negotiation_guarantees() {
    let deferred = fs::read_to_string(root().join("DEFERRED.md")).unwrap();
    assert!(!deferred.contains("unknown_critical_fail_closed"));
    assert!(!deferred.contains("version_mismatch_handshake"));
    assert!(deferred.contains("ordinary_turn_tool_replay_reducer"));
    assert!(deferred.contains("anvil_desktop_replay_reducer"));
    assert!(deferred.contains("anvil_persistent_mutation_watcher"));
    assert!(!deferred.contains("workflow_node_child_lifecycle"));
    assert!(!deferred.contains("anvil_origin_replay_mutation_staleness"));
}

#[test]
fn canonical_ready_advertises_the_embedded_generated_contract() {
    let ready: Value =
        serde_json::from_slice(&fs::read(root().join("events/ready.json")).unwrap()).unwrap();
    let expected = producer_contract_descriptor();
    assert_eq!(ready["contract"], serde_json::to_value(&expected).unwrap());
    assert_eq!(
        expected.capabilities.get("contract_negotiation"),
        Some(&ContractCapabilityStatus::Available)
    );
}

#[test]
fn negotiated_observer_accepts_authoritative_anvil_invalidation() {
    let mut observer = observer();
    negotiate(&mut observer);
    let line = fs::read(root().join("events/anvil_receipt_invalidated.json")).unwrap();
    let value: Value = serde_json::from_slice(&line).unwrap();
    assert_eq!(
        observer.observe_json_line(&line),
        Ok(HostObservation::Event(value))
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyApply {
    Advanced,
    Duplicate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyError {
    Version,
    NonCritical,
    Conflict,
    OutOfOrder,
}

#[derive(Default)]
struct SerializedPolicyReducer {
    current: Option<Value>,
}

impl SerializedPolicyReducer {
    fn apply(&mut self, event: &Value) -> Result<PolicyApply, PolicyError> {
        let snapshot = if event["type"] == "ready" {
            &event["execution_policy"]
        } else {
            event
        };
        let version = snapshot["contract_version"]
            .as_str()
            .ok_or(PolicyError::Version)?;
        validate_execution_policy_contract_version(version).map_err(|_| PolicyError::Version)?;
        if snapshot["critical"] != true {
            return Err(PolicyError::NonCritical);
        }
        let revision = snapshot["revision"]
            .as_u64()
            .ok_or(PolicyError::OutOfOrder)?;
        let Some(current) = &self.current else {
            if revision != 0 {
                return Err(PolicyError::OutOfOrder);
            }
            self.current = Some(snapshot.clone());
            return Ok(PolicyApply::Advanced);
        };
        let current_revision = current["revision"].as_u64().unwrap();
        if revision == current_revision {
            return if snapshot == current {
                Ok(PolicyApply::Duplicate)
            } else {
                Err(PolicyError::Conflict)
            };
        }
        if revision != current_revision.checked_add(1).unwrap() {
            return Err(PolicyError::OutOfOrder);
        }
        self.current = Some(snapshot.clone());
        Ok(PolicyApply::Advanced)
    }
}

#[test]
fn serialized_policy_reducer_accepts_valid_revisions_and_duplicate_replay() {
    let mut reducer = SerializedPolicyReducer::default();
    let valid = json_lines("adversarial/policy/valid-revisions.jsonl");
    assert_eq!(reducer.apply(&valid[0]), Ok(PolicyApply::Advanced));
    assert_eq!(reducer.apply(&valid[1]), Ok(PolicyApply::Advanced));

    let mut duplicate_reducer = SerializedPolicyReducer::default();
    let duplicate = json_lines("adversarial/policy/duplicate-identical.jsonl");
    assert_eq!(
        duplicate_reducer.apply(&duplicate[0]),
        Ok(PolicyApply::Advanced)
    );
    assert_eq!(
        duplicate_reducer.apply(&duplicate[1]),
        Ok(PolicyApply::Duplicate)
    );
}

#[test]
fn serialized_policy_reducer_fails_closed_on_conflict_gap_version_and_criticality() {
    for (path, expected) in [
        (
            "adversarial/policy/duplicate-conflict.jsonl",
            PolicyError::Conflict,
        ),
        (
            "adversarial/policy/revision-gap.jsonl",
            PolicyError::OutOfOrder,
        ),
        (
            "adversarial/policy/version-mismatch.jsonl",
            PolicyError::Version,
        ),
        (
            "adversarial/policy/noncritical.jsonl",
            PolicyError::NonCritical,
        ),
    ] {
        let events = json_lines(path);
        let mut reducer = SerializedPolicyReducer::default();
        assert_eq!(
            reducer.apply(&events[0]),
            Ok(PolicyApply::Advanced),
            "{path}"
        );
        assert_eq!(reducer.apply(&events[1]), Err(expected), "{path}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowApply {
    Applied,
    Duplicate,
    IgnoredAfterTerminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowError {
    Conflict,
    Sequence,
    ConflictingTerminal,
}

#[derive(Default)]
struct SerializedWorkflowReducer {
    event_bytes: HashMap<String, Vec<u8>>,
    next_sequence: HashMap<String, u64>,
    next_child_sequence: HashMap<String, u64>,
    terminal_runs: HashSet<String>,
    terminal_nodes: HashMap<(String, String), String>,
    terminal_children: HashSet<String>,
}

impl SerializedWorkflowReducer {
    fn apply(&mut self, event: &Value) -> Result<WorkflowApply, WorkflowError> {
        let event_type = event["type"].as_str().unwrap_or("");
        if event_type == "sub_agent_event" && event.get("child_run_id").is_some() {
            return self.apply_child(event);
        }
        if !matches!(
            event_type,
            "workflow_started" | "workflow_node_event" | "workflow_finished"
        ) {
            return Ok(WorkflowApply::IgnoredAfterTerminal);
        }
        let event_id = event["event_id"].as_str().ok_or(WorkflowError::Sequence)?;
        let canonical = serde_json::to_vec(event).unwrap();
        if let Some(previous) = self.event_bytes.get(event_id) {
            return if previous == &canonical {
                Ok(WorkflowApply::Duplicate)
            } else {
                Err(WorkflowError::Conflict)
            };
        }
        let run_id = event["run_id"].as_str().ok_or(WorkflowError::Sequence)?;
        if self.terminal_runs.contains(run_id) {
            return if event_type == "workflow_finished" {
                Err(WorkflowError::ConflictingTerminal)
            } else {
                Ok(WorkflowApply::IgnoredAfterTerminal)
            };
        }
        let observed = event["sequence"].as_u64().ok_or(WorkflowError::Sequence)?;
        let expected = self.next_sequence.get(run_id).copied().unwrap_or(0);
        if observed != expected {
            return Err(WorkflowError::Sequence);
        }

        if event_type == "workflow_node_event" {
            let node_id = event["node_id"].as_str().ok_or(WorkflowError::Sequence)?;
            let state = event["state"].as_str().ok_or(WorkflowError::Sequence)?;
            let key = (run_id.to_owned(), node_id.to_owned());
            if let Some(terminal) = self.terminal_nodes.get(&key) {
                return if is_node_terminal(state) && state != terminal {
                    Err(WorkflowError::ConflictingTerminal)
                } else {
                    Ok(WorkflowApply::IgnoredAfterTerminal)
                };
            }
            if is_node_terminal(state) {
                self.terminal_nodes.insert(key, state.to_owned());
            }
        } else if event_type == "workflow_finished" {
            let terminal = event["terminal_state"]
                .as_str()
                .ok_or(WorkflowError::Sequence)?;
            let succeeded = event["succeeded"]
                .as_bool()
                .ok_or(WorkflowError::Sequence)?;
            if succeeded != (terminal == "succeeded") {
                return Err(WorkflowError::ConflictingTerminal);
            }
            self.terminal_runs.insert(run_id.to_owned());
        }

        self.event_bytes.insert(event_id.to_owned(), canonical);
        self.next_sequence.insert(run_id.to_owned(), expected + 1);
        Ok(WorkflowApply::Applied)
    }

    fn apply_child(&mut self, event: &Value) -> Result<WorkflowApply, WorkflowError> {
        let event_id = event["event_id"].as_str().ok_or(WorkflowError::Sequence)?;
        let canonical = serde_json::to_vec(event).unwrap();
        if let Some(previous) = self.event_bytes.get(event_id) {
            return if previous == &canonical {
                Ok(WorkflowApply::Duplicate)
            } else {
                Err(WorkflowError::Conflict)
            };
        }
        let run_id = event["run_id"].as_str().ok_or(WorkflowError::Sequence)?;
        if self.terminal_runs.contains(run_id) {
            return Ok(WorkflowApply::IgnoredAfterTerminal);
        }
        let child_id = event["child_run_id"]
            .as_str()
            .ok_or(WorkflowError::Sequence)?;
        if self.terminal_children.contains(child_id) {
            return Ok(WorkflowApply::IgnoredAfterTerminal);
        }
        let observed = event["child_sequence"]
            .as_u64()
            .ok_or(WorkflowError::Sequence)?;
        let expected = self.next_child_sequence.get(child_id).copied().unwrap_or(0);
        if observed != expected {
            return Err(WorkflowError::Sequence);
        }
        if matches!(
            event["inner"]["type"].as_str(),
            Some("stream_end" | "error")
        ) {
            self.terminal_children.insert(child_id.to_owned());
        }
        self.event_bytes.insert(event_id.to_owned(), canonical);
        self.next_child_sequence
            .insert(child_id.to_owned(), expected + 1);
        Ok(WorkflowApply::Applied)
    }
}

fn is_node_terminal(state: &str) -> bool {
    matches!(
        state,
        "succeeded" | "failed" | "cancelled" | "timed_out" | "blocked"
    )
}

#[test]
fn serialized_workflow_reducer_replays_correlated_node_and_child_lifecycle() {
    let events = json_lines("adversarial/workflow/valid-lifecycle.jsonl");
    let mut reducer = SerializedWorkflowReducer::default();
    for event in &events {
        assert_eq!(reducer.apply(event), Ok(WorkflowApply::Applied));
    }

    let duplicate = json_lines("adversarial/workflow/duplicate-identical.jsonl");
    let mut duplicate_reducer = SerializedWorkflowReducer::default();
    assert_eq!(
        duplicate_reducer.apply(&duplicate[0]),
        Ok(WorkflowApply::Applied)
    );
    assert_eq!(
        duplicate_reducer.apply(&duplicate[1]),
        Ok(WorkflowApply::Duplicate)
    );
}

#[test]
fn serialized_workflow_reducer_detects_conflict_gaps_and_absorbs_terminals() {
    for path in [
        "adversarial/workflow/duplicate-conflict.jsonl",
        "adversarial/workflow/sequence-gap.jsonl",
        "adversarial/workflow/conflicting-node-terminal.jsonl",
        "adversarial/workflow/child-sequence-gap.jsonl",
        "adversarial/workflow/child-duplicate-conflict.jsonl",
    ] {
        let events = json_lines(path);
        let mut reducer = SerializedWorkflowReducer::default();
        let result = events
            .iter()
            .try_for_each(|event| reducer.apply(event).map(|_| ()));
        assert!(result.is_err(), "{path} did not fail closed");
    }

    let events = json_lines("adversarial/workflow/after-terminal.jsonl");
    let mut reducer = SerializedWorkflowReducer::default();
    assert_eq!(reducer.apply(&events[0]), Ok(WorkflowApply::Applied));
    assert_eq!(reducer.apply(&events[1]), Ok(WorkflowApply::Applied));
    assert_eq!(
        reducer.apply(&events[2]),
        Ok(WorkflowApply::IgnoredAfterTerminal)
    );
}

#[test]
fn production_workflow_reducer_replays_the_checked_corpus() {
    let valid = raw_lines("adversarial/workflow/valid-lifecycle.jsonl");
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    for line in valid {
        assert_eq!(
            reducer.accept_json(&line),
            Ok(WorkflowReplayAcceptance::Advanced)
        );
    }

    let duplicate = raw_lines("adversarial/workflow/duplicate-identical.jsonl");
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    assert_eq!(
        reducer.accept_json(&duplicate[0]),
        Ok(WorkflowReplayAcceptance::Advanced)
    );
    assert_eq!(
        reducer.accept_json(&duplicate[1]),
        Ok(WorkflowReplayAcceptance::Duplicate)
    );

    for path in [
        "adversarial/workflow/duplicate-conflict.jsonl",
        "adversarial/workflow/sequence-gap.jsonl",
        "adversarial/workflow/conflicting-node-terminal.jsonl",
        "adversarial/workflow/child-sequence-gap.jsonl",
        "adversarial/workflow/child-duplicate-conflict.jsonl",
    ] {
        let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
        let error = raw_lines(path)
            .iter()
            .find_map(|line| reducer.accept_json(line).err())
            .unwrap_or_else(|| panic!("{path} did not fail closed"));
        assert!(
            matches!(
                error,
                WorkflowReplayError::ConflictingDuplicate { .. }
                    | WorkflowReplayError::OutOfOrder { .. }
                    | WorkflowReplayError::ConflictingNodeTerminal { .. }
                    | WorkflowReplayError::ChildOutOfOrder { .. }
            ),
            "{path}: unexpected {error:?}"
        );
    }

    let after_terminal = raw_lines("adversarial/workflow/after-terminal.jsonl");
    let mut reducer = WorkflowReplayReducer::new("1.0").unwrap();
    assert_eq!(
        reducer.accept_json(&after_terminal[0]),
        Ok(WorkflowReplayAcceptance::Advanced)
    );
    assert_eq!(
        reducer.accept_json(&after_terminal[1]),
        Ok(WorkflowReplayAcceptance::Advanced)
    );
    assert_eq!(
        reducer.accept_json(&after_terminal[2]),
        Ok(WorkflowReplayAcceptance::IgnoredAfterRunTerminal)
    );
}

#[test]
fn serialized_anvil_reducer_replays_invalidation_and_never_resurrects_trust() {
    let valid = raw_lines("adversarial/anvil/valid-invalidation.jsonl");
    let mut reducer = AnvilReceiptReducer::default();
    assert_eq!(
        reducer.apply_json_line(&valid[0]),
        Ok(AnvilApplyOutcome::Applied)
    );
    assert_eq!(
        reducer.apply_json_line(&valid[1]),
        Ok(AnvilApplyOutcome::Applied)
    );
    assert_eq!(
        reducer.status("receipt-desktop-001"),
        Some(AnvilReceiptStatus::Invalidated)
    );

    let stale = raw_lines("adversarial/anvil/stale-replay.jsonl");
    let mut stale_reducer = AnvilReceiptReducer::default();
    assert_eq!(
        stale_reducer.apply_json_line(&stale[0]),
        Ok(AnvilApplyOutcome::Applied)
    );
    assert_eq!(
        stale_reducer.apply_json_line(&stale[1]),
        Ok(AnvilApplyOutcome::Applied)
    );
    assert_eq!(
        stale_reducer.apply_json_line(&stale[2]),
        Ok(AnvilApplyOutcome::Duplicate)
    );
    assert_eq!(
        stale_reducer.status("receipt-desktop-001"),
        Some(AnvilReceiptStatus::Invalidated)
    );
}

#[test]
fn serialized_anvil_reducer_proves_duplicate_inert_and_fail_closed_vectors() {
    let duplicate = raw_lines("adversarial/anvil/duplicate-identical.jsonl");
    let mut reducer = AnvilReceiptReducer::default();
    assert_eq!(
        reducer.apply_json_line(&duplicate[0]),
        Ok(AnvilApplyOutcome::Applied)
    );
    assert_eq!(
        reducer.apply_json_line(&duplicate[1]),
        Ok(AnvilApplyOutcome::Duplicate)
    );

    let nested = raw_lines("adversarial/anvil/nested-receipt-inert.jsonl");
    assert_eq!(
        AnvilReceiptReducer::default().apply_json_line(&nested[0]),
        Ok(AnvilApplyOutcome::Inert)
    );

    for (path, expected) in [
        ("adversarial/anvil/duplicate-conflict.jsonl", "conflict"),
        ("adversarial/anvil/sequence-gap.jsonl", "gap"),
        ("adversarial/anvil/version-mismatch.jsonl", "version"),
        (
            "adversarial/anvil/unknown-critical-extension.jsonl",
            "extension",
        ),
        ("adversarial/anvil/out-of-order.jsonl", "order"),
        ("adversarial/anvil/altered-body.jsonl", "body"),
        (
            "adversarial/anvil/altered-invalidation-body.jsonl",
            "invalidation_body",
        ),
    ] {
        let lines = raw_lines(path);
        let mut reducer = AnvilReceiptReducer::default();
        let error = lines
            .iter()
            .find_map(|line| reducer.apply_json_line(line).err())
            .unwrap_or_else(|| panic!("{path} did not fail closed"));
        assert!(
            matches!(
                (&error, expected),
                (AnvilReceiptError::EventConflict(_), "conflict")
                    | (AnvilReceiptError::SequenceGap { .. }, "gap")
                    | (AnvilReceiptError::VersionMismatch(_), "version")
                    | (AnvilReceiptError::UnknownCriticalExtension(_), "extension")
                    | (AnvilReceiptError::OutOfOrder { .. }, "order")
                    | (AnvilReceiptError::ReceiptBodyDigestMismatch, "body")
                    | (
                        AnvilReceiptError::InvalidationBodyDigestMismatch,
                        "invalidation_body"
                    )
            ),
            "{path}: unexpected {error:?}"
        );
    }

    let legacy =
        fs::read_to_string(root().join("compat/events/anvil_receipt.legacy.json")).unwrap();
    assert!(matches!(
        AnvilReceiptReducer::default().apply_json_line(&legacy),
        Err(AnvilReceiptError::Malformed(_))
    ));
}
