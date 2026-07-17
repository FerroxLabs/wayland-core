
use std::collections::BTreeMap;

use crate::spawner::{
    ChildDeliveryState, ChildDesiredState, ChildOrigin, ChildParent, ChildPolicySnapshot,
    ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace,
    DURABLE_CHILD_SCHEMA_VERSION,
};

use super::*;

fn digest(byte: char) -> String {
    std::iter::repeat_n(byte, 64).collect()
}

fn revision(byte: char) -> String {
    std::iter::repeat_n(byte, 40).collect()
}

fn gate_plan() -> ChildGatePlan {
    ChildGatePlan {
        required_gates: vec![ChildGateRequirement {
            gate_id: "cargo-test".to_owned(),
            gate_closure_digest: digest('6'),
        }],
    }
}

fn subject() -> ChildGateSubject {
    ChildGateSubject {
        base_revision: revision('1'),
        candidate_revision: revision('2'),
        diff_digest: digest('3'),
        request_digest: digest('4'),
        policy_digest: digest('5'),
        gate_plan_digest: gate_plan().canonical_digest().unwrap(),
        gate_closure_digest: digest('6'),
    }
}

fn passed_gate() -> ChildGateReceipt {
    ChildGateReceipt {
        gate_id: "cargo-test".to_owned(),
        subject: subject(),
        evidence_digest: digest('7'),
        outcome: ChildGateOutcome::Passed,
        exit_code: Some(0),
    }
}

fn receipt() -> ChildTransactionReceipt {
    ChildTransactionReceipt {
        schema_version: CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION,
        transaction_id: "transaction-1".to_owned(),
        receipt_id: "transaction-1-receipt-0".to_owned(),
        receipt_revision: 0,
        previous_receipt_digest: None,
        child_id: ChildId::new("child-1").unwrap(),
        child_declaration_id: "declaration-1".to_owned(),
        child_revision: 4,
        workspace_id: "isolated-child-1".to_owned(),
        base_revision: revision('1'),
        candidate_revision: Some(revision('2')),
        request_digest: digest('4'),
        policy_digest: digest('5'),
        gate_plan_digest: gate_plan().canonical_digest().unwrap(),
        diff_digest: Some(digest('3')),
        gates: vec![passed_gate()],
        disposition: ChildTransactionDisposition::MergeReady {
            expected_parent_revision: revision('1'),
        },
        created_at_unix_ms: 100,
        updated_at_unix_ms: 200,
    }
}

fn durable_child() -> DurableChildRecord {
    DurableChildRecord {
        schema_version: DURABLE_CHILD_SCHEMA_VERSION,
        declaration_id: "declaration-1".to_owned(),
        child_id: ChildId::new("child-1").unwrap(),
        parent: ChildParent {
            session_id: "session-1".to_owned(),
            turn_id: None,
            parent_child_id: None,
            workflow_run_id: None,
            graph_node_id: None,
            parent_call_id: None,
        },
        origin: ChildOrigin::Delegate,
        request: ChildRequestEvidence::redacted(digest('4')),
        policy_snapshot: ChildPolicySnapshot {
            contract_version: "execution-policy/v1".to_owned(),
            exact_digest: digest('5'),
            posture: "smart".to_owned(),
            approvals: "on_request".to_owned(),
            sandbox: "required".to_owned(),
            source: "local".to_owned(),
            managed_floor_active: false,
            dangerous_activation_id_digest: None,
        },
        provider: Some("test".to_owned()),
        model: Some("test-model".to_owned()),
        workspace: ChildWorkspace {
            mode: ChildWorkspaceMode::Isolated,
            workspace_id: "isolated-child-1".to_owned(),
        },
        status: DurableChildStatus::Succeeded,
        desired_state: ChildDesiredState::Run,
        recovery: ChildRecoveryState::Clean,
        revision: 4,
        timestamps: ChildTimestamps {
            created_at_unix_ms: 10,
            updated_at_unix_ms: 20,
            queued_at_unix_ms: Some(11),
            started_at_unix_ms: Some(12),
            terminal_at_unix_ms: Some(20),
        },
        result: None,
        delivery_target: None,
        delivery_state: ChildDeliveryState::NotRequired,
        attempt: 1,
        retry_of: None,
        applied_events: BTreeMap::new(),
    }
}

#[test]
fn gate_subject_is_bound_to_exact_candidate_and_authority() {
    receipt().validate().unwrap();
    let mut stale = receipt();
    stale.gates[0].subject.candidate_revision = revision('8');
    assert_eq!(
        stale.validate(),
        Err(ChildTransactionValidationError::GateSubjectMismatch)
    );
}

#[test]
fn merge_dispositions_require_executable_passing_evidence() {
    let mut missing = receipt();
    missing.gates.clear();
    assert_eq!(
        missing.validate(),
        Err(ChildTransactionValidationError::InvalidDisposition)
    );
    let mut failed = receipt();
    failed.gates[0].outcome = ChildGateOutcome::Failed;
    failed.gates[0].exit_code = Some(1);
    assert_eq!(
        failed.validate(),
        Err(ChildTransactionValidationError::InvalidDisposition)
    );
}

#[test]
fn child_binding_requires_isolation_identity_policy_request_and_revision() {
    receipt()
        .validate_for_child(&durable_child(), &gate_plan())
        .unwrap();
    let mut shared = durable_child();
    shared.workspace.mode = ChildWorkspaceMode::SharedReadOnly;
    assert_eq!(
        receipt().validate_for_child(&shared, &gate_plan()),
        Err(ChildTransactionValidationError::ChildBindingMismatch)
    );
    let mut unrelated = durable_child();
    unrelated.request = ChildRequestEvidence::redacted(digest('9'));
    assert_eq!(
        receipt().validate_for_child(&unrelated, &gate_plan()),
        Err(ChildTransactionValidationError::ChildBindingMismatch)
    );
}

#[test]
fn reducer_rejects_sibling_receipts_and_terminal_supersession() {
    let mut reducer = ChildTransactionReducer::default();
    let genesis = receipt();
    let genesis_digest = genesis.canonical_digest().unwrap();
    assert_eq!(
        reducer.apply(
            &genesis_digest,
            genesis.clone(),
            &durable_child(),
            &gate_plan(),
        ),
        Ok(ChildTransactionReplay::Applied)
    );
    assert_eq!(
        reducer.apply(
            &genesis_digest,
            genesis.clone(),
            &durable_child(),
            &gate_plan(),
        ),
        Ok(ChildTransactionReplay::Duplicate)
    );

    let mut merged = genesis.clone();
    merged.receipt_id = "transaction-1-receipt-1".to_owned();
    merged.receipt_revision = 1;
    merged.previous_receipt_digest = Some(genesis_digest.clone());
    merged.updated_at_unix_ms = 300;
    merged.disposition = ChildTransactionDisposition::Merged {
        expected_parent_revision: revision('1'),
        parent_revision: revision('8'),
        parent_tree_digest: digest('b'),
        ancestry_evidence_digest: digest('c'),
    };
    let merged_digest = merged.canonical_digest().unwrap();
    assert_eq!(
        reducer.apply(&merged_digest, merged, &durable_child(), &gate_plan(),),
        Ok(ChildTransactionReplay::Applied)
    );

    let mut sibling = genesis;
    sibling.receipt_id = "transaction-1-conflict-1".to_owned();
    sibling.receipt_revision = 1;
    sibling.previous_receipt_digest = Some(genesis_digest);
    sibling.updated_at_unix_ms = 300;
    sibling.disposition = ChildTransactionDisposition::Conflict {
        expected_parent_revision: revision('1'),
        observed_parent_revision: revision('9'),
        evidence_digest: digest('e'),
    };
    let sibling_digest = sibling.canonical_digest().unwrap();
    assert_eq!(
        reducer.apply(&sibling_digest, sibling, &durable_child(), &gate_plan(),),
        Err(ChildTransactionValidationError::InvalidSequence)
    );
}

#[test]
fn reducer_rejects_revision_overflow() {
    let mut latest = receipt();
    latest.receipt_revision = u64::MAX;
    let latest_digest = latest.canonical_digest().unwrap();
    let mut reducer = ChildTransactionReducer {
        latest: Some((latest_digest.clone(), latest.clone())),
    };

    let mut overflow = latest;
    overflow.receipt_id = "transaction-1-overflow".to_owned();
    overflow.previous_receipt_digest = Some(latest_digest);
    overflow.updated_at_unix_ms = 300;
    overflow.disposition = ChildTransactionDisposition::Retained {
        reason_digest: digest('c'),
    };
    let overflow_digest = overflow.canonical_digest().unwrap();
    assert_eq!(
        reducer.apply(&overflow_digest, overflow, &durable_child(), &gate_plan(),),
        Err(ChildTransactionValidationError::InvalidSequence)
    );
}

#[test]
fn reducer_recomputes_content_addressed_receipt_digest() {
    let mut reducer = ChildTransactionReducer::default();
    assert_eq!(
        reducer.apply(&digest('a'), receipt(), &durable_child(), &gate_plan(),),
        Err(ChildTransactionValidationError::ReceiptDigestMismatch)
    );
}

#[test]
fn merge_ready_requires_the_exact_authorized_gate_plan() {
    let mut incomplete_plan = gate_plan();
    incomplete_plan.required_gates.push(ChildGateRequirement {
        gate_id: "cargo-clippy".to_owned(),
        gate_closure_digest: digest('8'),
    });
    let mut candidate = receipt();
    candidate.gate_plan_digest = incomplete_plan.canonical_digest().unwrap();
    candidate.gates[0].subject.gate_plan_digest = candidate.gate_plan_digest.clone();
    assert_eq!(
        candidate.validate_for_child(&durable_child(), &incomplete_plan),
        Err(ChildTransactionValidationError::GatePlanMismatch)
    );

    let mut invented = receipt();
    invented.gates[0].gate_id = "invented-pass".to_owned();
    assert_eq!(
        invented.validate_for_child(&durable_child(), &gate_plan()),
        Err(ChildTransactionValidationError::GatePlanMismatch)
    );
}

#[test]
fn merged_receipt_cannot_change_authorized_parent_revision() {
    let mut reducer = ChildTransactionReducer::default();
    let genesis = receipt();
    let genesis_digest = genesis.canonical_digest().unwrap();
    reducer
        .apply(
            &genesis_digest,
            genesis.clone(),
            &durable_child(),
            &gate_plan(),
        )
        .unwrap();

    let mut drifted = genesis;
    drifted.receipt_id = "transaction-1-receipt-1".to_owned();
    drifted.receipt_revision = 1;
    drifted.previous_receipt_digest = Some(genesis_digest);
    drifted.updated_at_unix_ms = 300;
    drifted.disposition = ChildTransactionDisposition::Merged {
        expected_parent_revision: revision('9'),
        parent_revision: revision('8'),
        parent_tree_digest: digest('b'),
        ancestry_evidence_digest: digest('c'),
    };
    let drifted_digest = drifted.canonical_digest().unwrap();
    assert_eq!(
        reducer.apply(&drifted_digest, drifted, &durable_child(), &gate_plan(),),
        Err(ChildTransactionValidationError::InvalidSequence)
    );
}

#[test]
fn merge_and_rollback_require_parent_cas_result_evidence() {
    let mut merged = receipt();
    merged.disposition = ChildTransactionDisposition::Merged {
        expected_parent_revision: revision('1'),
        parent_revision: "moving-main".to_owned(),
        parent_tree_digest: digest('b'),
        ancestry_evidence_digest: digest('c'),
    };
    assert!(matches!(
        merged.validate(),
        Err(ChildTransactionValidationError::InvalidField(
            "merged.parent_revision"
        ))
    ));

    let mut rolled_back = receipt();
    rolled_back.disposition = ChildTransactionDisposition::RolledBack {
        expected_parent_revision: revision('8'),
        parent_revision: revision('9'),
        parent_tree_digest: "not-a-digest".to_owned(),
        evidence_digest: digest('e'),
    };
    assert!(matches!(
        rolled_back.validate(),
        Err(ChildTransactionValidationError::InvalidDigest(
            "rollback.parent_tree_digest"
        ))
    ));
}

#[test]
fn receipt_rejects_unknown_host_path_and_invalid_exit_semantics() {
    let mut value = serde_json::to_value(receipt()).unwrap();
    value.as_object_mut().unwrap().insert(
        "host_path".to_owned(),
        serde_json::json!("/Users/alice/repo"),
    );
    assert!(serde_json::from_value::<ChildTransactionReceipt>(value).is_err());

    let mut failed_without_status = receipt();
    failed_without_status.gates[0].outcome = ChildGateOutcome::Failed;
    failed_without_status.gates[0].exit_code = None;
    assert_eq!(
        failed_without_status.validate(),
        Err(ChildTransactionValidationError::InvalidField(
            "gate.exit_code"
        ))
    );
}
