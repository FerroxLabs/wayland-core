use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use wcore_agent::child_transaction::{ChildTransactionStore, ChildTransactionWrite};
use wcore_agent::durable_child::DurableChildStore;
use wcore_agent::session_journal::{SessionEvent, SessionJournal};
use wcore_types::child_transaction::{
    CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION, ChildGatePlan, ChildGateRequirement,
    ChildTransactionDisposition, ChildTransactionReceipt,
};
use wcore_types::spawner::{
    ChildDeliveryState, ChildDesiredState, ChildId, ChildOrigin, ChildParent, ChildPolicySnapshot,
    ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus, DurableChildTransition,
};

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn revision(character: char) -> String {
    std::iter::repeat_n(character, 40).collect()
}

fn gate_plan() -> ChildGatePlan {
    ChildGatePlan {
        required_gates: vec![ChildGateRequirement {
            gate_id: "cargo-test".into(),
            gate_closure_digest: digest('c'),
        }],
    }
}

fn child_record() -> DurableChildRecord {
    DurableChildRecord {
        schema_version: DURABLE_CHILD_SCHEMA_VERSION,
        declaration_id: "declare-child-1".into(),
        child_id: ChildId::new("child-1").unwrap(),
        parent: ChildParent {
            session_id: "session-1".into(),
            turn_id: None,
            parent_child_id: None,
            workflow_run_id: None,
            graph_node_id: None,
            parent_call_id: None,
        },
        origin: ChildOrigin::Delegate,
        request: ChildRequestEvidence::redacted(digest('a')),
        policy_snapshot: ChildPolicySnapshot {
            contract_version: "effective-execution-policy/v1".into(),
            exact_digest: digest('b'),
            posture: "smart".into(),
            approvals: "on_request".into(),
            sandbox: "required".into(),
            source: "session-effective-policy".into(),
            managed_floor_active: true,
            dangerous_activation_id_digest: None,
        },
        provider: Some("test".into()),
        model: Some("test-model".into()),
        workspace: ChildWorkspace {
            mode: ChildWorkspaceMode::Isolated,
            workspace_id: "workspace-child-1".into(),
        },
        status: DurableChildStatus::Prepared,
        desired_state: ChildDesiredState::Run,
        recovery: ChildRecoveryState::Clean,
        revision: 0,
        timestamps: ChildTimestamps {
            created_at_unix_ms: 10,
            updated_at_unix_ms: 10,
            queued_at_unix_ms: None,
            started_at_unix_ms: None,
            terminal_at_unix_ms: None,
        },
        result: None,
        delivery_target: None,
        delivery_state: ChildDeliveryState::NotRequired,
        attempt: 1,
        retry_of: None,
        applied_events: BTreeMap::new(),
    }
}

fn genesis_receipt() -> ChildTransactionReceipt {
    let plan = gate_plan();
    ChildTransactionReceipt {
        schema_version: CHILD_TRANSACTION_RECEIPT_SCHEMA_VERSION,
        transaction_id: "transaction-1".into(),
        receipt_id: "transaction-1-receipt-0".into(),
        receipt_revision: 0,
        previous_receipt_digest: None,
        child_id: ChildId::new("child-1").unwrap(),
        child_declaration_id: "declare-child-1".into(),
        child_revision: 0,
        workspace_id: "workspace-child-1".into(),
        base_revision: revision('1'),
        candidate_revision: None,
        request_digest: digest('a'),
        policy_digest: digest('b'),
        gate_plan_digest: plan.canonical_digest().unwrap(),
        diff_digest: None,
        gates: Vec::new(),
        disposition: ChildTransactionDisposition::Active,
        created_at_unix_ms: 100,
        updated_at_unix_ms: 100,
    }
}

fn successor(genesis: &ChildTransactionReceipt, genesis_digest: &str) -> ChildTransactionReceipt {
    let mut receipt = genesis.clone();
    receipt.receipt_id = "transaction-1-receipt-1".into();
    receipt.receipt_revision = 1;
    receipt.previous_receipt_digest = Some(genesis_digest.into());
    receipt.child_revision = 1;
    receipt.updated_at_unix_ms = 200;
    receipt.disposition = ChildTransactionDisposition::Retained {
        reason_digest: digest('d'),
    };
    receipt
}

fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    SessionJournal,
    DurableChildStore,
    ChildTransactionStore,
) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.journal");
    let journal = SessionJournal::open(&path, "session-1").unwrap();
    let children = DurableChildStore::new(journal.clone());
    children.declare(child_record()).unwrap();
    let transactions = ChildTransactionStore::new(journal.clone());
    (temp, path, journal, children, transactions)
}

#[test]
fn authoritative_opening_is_durable_revalidated_and_stable_across_unrelated_appends() {
    let (_temp, path, journal, _children, transactions) = fixture();
    let authority = transactions
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            gate_plan(),
        )
        .unwrap();
    assert_eq!(authority.transaction_id(), "transaction-1");
    transactions.revalidate(&authority).unwrap();
    let opened_len = std::fs::metadata(&path).unwrap().len();

    journal
        .append(SessionEvent::TurnStarted {
            turn_id: "unrelated-turn".into(),
            user_message: "unrelated".into(),
        })
        .unwrap();
    transactions.revalidate(&authority).unwrap();
    let retried = transactions
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            gate_plan(),
        )
        .unwrap();
    assert_eq!(authority, retried);
    assert!(std::fs::metadata(&path).unwrap().len() > opened_len);
}

#[test]
fn receipt_chain_persists_reopens_and_exact_retry_is_idempotent() {
    let (temp, path, journal, children, transactions) = fixture();
    let authority = transactions
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            gate_plan(),
        )
        .unwrap();
    let genesis = genesis_receipt();
    let genesis_digest = genesis.canonical_digest().unwrap();
    assert!(matches!(
        transactions.commit(&authority, genesis.clone()).unwrap(),
        ChildTransactionWrite::Appended(_)
    ));
    assert_eq!(
        transactions.commit(&authority, genesis.clone()).unwrap(),
        ChildTransactionWrite::AlreadyCommitted
    );

    children
        .transition(
            ChildId::new("child-1").unwrap(),
            "enqueue-1",
            0,
            101,
            DurableChildTransition::Enqueue,
        )
        .unwrap();
    let next = successor(&genesis, &genesis_digest);
    transactions.commit(&authority, next).unwrap();
    let projected = transactions.inspect("transaction-1").unwrap().unwrap();
    assert_eq!(projected.receipts.len(), 2);
    assert_eq!(projected.receipts[0].child_snapshot.revision, 0);
    assert_eq!(projected.receipts[1].child_snapshot.revision, 1);

    drop(transactions);
    drop(children);
    drop(journal);
    let reopened = SessionJournal::open(&path, "session-1").unwrap();
    let projected = ChildTransactionStore::new(reopened)
        .inspect("transaction-1")
        .unwrap()
        .unwrap();
    assert_eq!(projected.receipts.len(), 2);
    drop(temp);
}

#[test]
fn caller_append_conflicting_opening_and_conflicting_receipt_fail_without_effects() {
    let (_temp, path, journal, _children, transactions) = fixture();
    let authority = transactions
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            gate_plan(),
        )
        .unwrap();
    let committed_len = std::fs::metadata(&path).unwrap().len();
    let projected = transactions.inspect("transaction-1").unwrap().unwrap();
    assert!(
        journal
            .append(SessionEvent::ChildTransactionOpened {
                opening: projected.opening.clone(),
            })
            .is_err()
    );
    assert!(
        transactions
            .open(
                "transaction-1",
                ChildId::new("child-1").unwrap(),
                revision('2'),
                gate_plan(),
            )
            .is_err()
    );

    let receipt = genesis_receipt();
    transactions.commit(&authority, receipt.clone()).unwrap();
    let after_receipt = std::fs::metadata(&path).unwrap().len();
    let mut conflict = receipt;
    conflict.receipt_id = "transaction-1-conflict".into();
    conflict.updated_at_unix_ms = 101;
    assert!(transactions.commit(&authority, conflict).is_err());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), after_receipt);
    assert!(after_receipt > committed_len);
}

#[test]
fn malformed_authority_and_pending_snapshot_fail_before_external_effects() {
    let (_temp, path, journal, _children, transactions) = fixture();
    journal.publish_snapshot().unwrap();
    let authority_path = path.with_file_name("session.journal.authority");
    let mut head: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&authority_path).unwrap()).unwrap();
    head["pending"] = head["accepted"].clone();
    std::fs::write(&authority_path, serde_json::to_vec(&head).unwrap()).unwrap();

    let effects = AtomicUsize::new(0);
    let result = transactions.open(
        "transaction-1",
        ChildId::new("child-1").unwrap(),
        revision('1'),
        gate_plan(),
    );
    if result.is_ok() {
        effects.fetch_add(1, Ordering::SeqCst);
    }
    assert!(result.is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[cfg(unix)]
#[test]
fn snapshot_authority_symlink_and_multiple_link_substitution_fail_closed() {
    use std::os::unix::fs::symlink;

    for use_symlink in [true, false] {
        let (_temp, path, journal, _children, transactions) = fixture();
        journal.publish_snapshot().unwrap();
        let authority_path = path.with_file_name("session.journal.authority");
        let displaced = path.with_file_name("displaced.authority");
        std::fs::rename(&authority_path, &displaced).unwrap();
        if use_symlink {
            symlink(&displaced, &authority_path).unwrap();
        } else {
            std::fs::hard_link(&displaced, &authority_path).unwrap();
        }
        assert!(
            transactions
                .open(
                    "transaction-1",
                    ChildId::new("child-1").unwrap(),
                    revision('1'),
                    gate_plan(),
                )
                .is_err()
        );
    }
}

fn split_frames(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let length = u32::from_be_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let frame_len = 12 + length + 32;
        frames.push(bytes[offset..offset + frame_len].to_vec());
        offset += frame_len;
    }
    frames
}

#[test]
fn reordered_truncated_and_corrupt_journals_fail_closed() {
    let (temp, path, journal, _children, transactions) = fixture();
    transactions
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            gate_plan(),
        )
        .unwrap();
    drop(transactions);
    drop(journal);

    let original = std::fs::read(&path).unwrap();
    let mut reordered = split_frames(&original);
    reordered.swap(0, 1);
    let reordered_path = temp.path().join("reordered.journal");
    std::fs::write(&reordered_path, reordered.concat()).unwrap();
    assert!(SessionJournal::recovered_state(&reordered_path).is_err());

    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 0xff;
    let corrupt_path = temp.path().join("corrupt.journal");
    std::fs::write(&corrupt_path, corrupt).unwrap();
    assert!(SessionJournal::recovered_state(&corrupt_path).is_err());

    let truncated_path = temp.path().join("truncated.journal");
    std::fs::write(&truncated_path, &original[..original.len() - 8]).unwrap();
    let truncated = SessionJournal::recovered_state(&truncated_path).unwrap();
    assert!(truncated.child_transactions.is_empty());
    assert!(truncated.children.contains_key("child-1"));
}

#[test]
fn copied_journal_cannot_rebind_an_opening_authority_to_another_store() {
    let (temp, path, journal, _children, transactions) = fixture();
    let authority = transactions
        .open(
            "transaction-1",
            ChildId::new("child-1").unwrap(),
            revision('1'),
            gate_plan(),
        )
        .unwrap();
    drop(transactions);
    drop(journal);

    let rebound_path = temp.path().join("rebound.journal");
    std::fs::copy(&path, &rebound_path).unwrap();
    let rebound = SessionJournal::open(&rebound_path, "session-1").unwrap();
    assert!(
        ChildTransactionStore::new(rebound)
            .revalidate(&authority)
            .is_err()
    );
}

#[test]
fn legacy_journal_replays_with_empty_transaction_projection() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("legacy.journal");
    let journal = SessionJournal::open(&path, "legacy-session").unwrap();
    journal
        .append(SessionEvent::TurnStarted {
            turn_id: "turn-1".into(),
            user_message: "hello".into(),
        })
        .unwrap();
    drop(journal);

    let state = SessionJournal::recovered_state(&path).unwrap();
    assert!(state.child_transactions.is_empty());
    assert!(state.turns.contains_key("turn-1"));
}
