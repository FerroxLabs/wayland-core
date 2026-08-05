use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use tokio::sync::Notify;
use wcore_agent::durable_child::{DurableChildStore, DurableChildWrite};
use wcore_agent::session_journal::SessionJournal;
use wcore_agent::spawner::{
    DurableCancelDisposition, DurableSpawner, ForkOverrides, Spawner, SubAgentConfig,
    SubAgentResult,
};
use wcore_types::message::TokenUsage;
use wcore_types::spawner::{
    ChildDeliveryReconciliation, ChildDeliveryState, ChildDeliveryTarget, ChildDesiredState,
    ChildId, ChildOrigin, ChildParent, ChildPolicySnapshot, ChildRecoveryState,
    ChildRequestEvidence, ChildTimestamps, ChildWorkspace, ChildWorkspaceMode,
    DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildStatus, DurableChildTransition,
};

struct ImmediateSpawner;

#[async_trait]
impl Spawner for ImmediateSpawner {
    async fn spawn_fork(
        &self,
        config: SubAgentConfig,
        _overrides: ForkOverrides,
    ) -> SubAgentResult {
        SubAgentResult {
            name: config.name,
            text: "completed".into(),
            usage: TokenUsage {
                input_tokens: 11,
                output_tokens: 7,
                ..TokenUsage::default()
            },
            turns: 2,
            is_error: false,
        }
    }
}

struct BlockingSpawner {
    started: Arc<Notify>,
}

struct CountingSpawner {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Spawner for CountingSpawner {
    async fn spawn_fork(
        &self,
        config: SubAgentConfig,
        _overrides: ForkOverrides,
    ) -> SubAgentResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        SubAgentResult::error(&config.name, "must not execute")
    }
}

#[async_trait]
impl Spawner for BlockingSpawner {
    async fn spawn_fork(
        &self,
        _config: SubAgentConfig,
        _overrides: ForkOverrides,
    ) -> SubAgentResult {
        self.started.notify_one();
        std::future::pending().await
    }
}

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn record(
    id: &str,
    origin: ChildOrigin,
    config: &SubAgentConfig,
    overrides: &ForkOverrides,
) -> DurableChildRecord {
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
        origin,
        request: ChildRequestEvidence::redacted(
            DurableSpawner::request_digest(config, overrides).unwrap(),
        ),
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
        provider: config.provider.clone(),
        model: overrides.model.clone().or_else(|| config.model.clone()),
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
        delivery_target: Some(ChildDeliveryTarget::SessionOutbox),
        delivery_state: ChildDeliveryState::Pending,
        attempt: 1,
        retry_of: None,
        applied_events: BTreeMap::new(),
    }
}

fn config(name: &str) -> SubAgentConfig {
    SubAgentConfig {
        name: name.into(),
        prompt: "do the work".into(),
        max_turns: 3,
        max_tokens: 128,
        system_prompt: None,
        provider: Some("openai".into()),
        model: Some("gpt-test".into()),
        temperature: None,
    }
}

fn policy_digest() -> String {
    digest('b')
}

#[tokio::test]
async fn adapter_lists_inspects_and_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.journal");
    let child_id = ChildId::new("spawn-child").unwrap();
    {
        let journal = SessionJournal::open(&path, "session-1").unwrap();
        let spawner =
            DurableSpawner::new(DurableChildStore::new(journal), Arc::new(ImmediateSpawner))
                .unwrap();
        let config = config("spawn-child");
        let overrides = ForkOverrides {
            model: Some("gpt-override".into()),
            ..ForkOverrides::default()
        };
        let result = spawner
            .spawn_fork(
                record(child_id.as_str(), ChildOrigin::Spawn, &config, &overrides),
                config,
                overrides,
                &policy_digest(),
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(spawner.list().unwrap().len(), 1);
        let stored = spawner.inspect(&child_id).unwrap().unwrap();
        assert_eq!(stored.status, DurableChildStatus::Succeeded);
        assert_eq!(stored.result.as_ref().unwrap().turns, 2);
        assert_eq!(stored.result.as_ref().unwrap().input_tokens, 11);
        assert_eq!(stored.model.as_deref(), Some("gpt-override"));
    }

    let reopened = SessionJournal::open(&path, "session-1").unwrap();
    let spawner =
        DurableSpawner::new(DurableChildStore::new(reopened), Arc::new(ImmediateSpawner)).unwrap();
    let stored = spawner.inspect(&child_id).unwrap().unwrap();
    assert_eq!(stored.status, DurableChildStatus::Succeeded);
    assert_eq!(spawner.list().unwrap(), vec![stored]);
}

#[tokio::test]
async fn cancellation_is_visible_and_stops_the_owned_execution() {
    let temp = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(temp.path().join("session.journal"), "session-1").unwrap();
    let started = Arc::new(Notify::new());
    let spawner = DurableSpawner::new(
        DurableChildStore::new(journal),
        Arc::new(BlockingSpawner {
            started: Arc::clone(&started),
        }),
    )
    .unwrap();
    let child_id = ChildId::new("cancel-child").unwrap();
    let task_spawner = spawner.clone();
    let config = config("cancel-child");
    let overrides = ForkOverrides::default();
    let record = record("cancel-child", ChildOrigin::Delegate, &config, &overrides);
    let task = tokio::spawn(async move {
        task_spawner
            .spawn_fork(record, config, overrides, &policy_digest())
            .await
    });
    started.notified().await;
    assert_eq!(
        spawner.request_cancel(&child_id).unwrap(),
        DurableCancelDisposition::Signalled
    );
    assert!(task.await.unwrap().unwrap().is_error);
    let stored = spawner.inspect(&child_id).unwrap().unwrap();
    assert_eq!(stored.status, DurableChildStatus::Cancelled);
    assert_eq!(stored.desired_state, ChildDesiredState::Cancel);
}

#[tokio::test]
async fn mismatched_execution_evidence_fails_before_write_or_execution() {
    let temp = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(temp.path().join("session.journal"), "session-1").unwrap();
    let journal_observer = journal.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let spawner = DurableSpawner::new(
        DurableChildStore::new(journal),
        Arc::new(CountingSpawner {
            calls: Arc::clone(&calls),
        }),
    )
    .unwrap();
    let config = config("mismatch-child");
    let overrides = ForkOverrides::default();
    let mut request_mismatch = record("request-mismatch", ChildOrigin::Spawn, &config, &overrides);
    request_mismatch.request = ChildRequestEvidence::redacted(digest('9'));
    assert!(
        spawner
            .spawn_fork(
                request_mismatch,
                config.clone(),
                overrides.clone(),
                &policy_digest(),
            )
            .await
            .is_err()
    );

    let mut model_mismatch = record("model-mismatch", ChildOrigin::Spawn, &config, &overrides);
    model_mismatch.model = Some("different-model".into());
    assert!(
        spawner
            .spawn_fork(
                model_mismatch,
                config.clone(),
                overrides.clone(),
                &policy_digest(),
            )
            .await
            .is_err()
    );

    let mut provider_mismatch =
        record("provider-mismatch", ChildOrigin::Spawn, &config, &overrides);
    provider_mismatch.provider = Some("different-provider".into());
    assert!(
        spawner
            .spawn_fork(
                provider_mismatch,
                config.clone(),
                overrides.clone(),
                &policy_digest(),
            )
            .await
            .is_err()
    );

    let mut unresolved_provider = config.clone();
    unresolved_provider.provider = None;
    let unresolved_provider_record = record(
        "unresolved-provider",
        ChildOrigin::Spawn,
        &unresolved_provider,
        &overrides,
    );
    assert!(
        spawner
            .spawn_fork(
                unresolved_provider_record,
                unresolved_provider,
                overrides.clone(),
                &policy_digest(),
            )
            .await
            .is_err()
    );

    let mut unresolved_model = config.clone();
    unresolved_model.model = None;
    let unresolved_model_record = record(
        "unresolved-model",
        ChildOrigin::Spawn,
        &unresolved_model,
        &overrides,
    );
    assert!(
        spawner
            .spawn_fork(
                unresolved_model_record,
                unresolved_model,
                overrides.clone(),
                &policy_digest(),
            )
            .await
            .is_err()
    );

    let policy_mismatch = record("policy-mismatch", ChildOrigin::Spawn, &config, &overrides);
    assert!(
        spawner
            .spawn_fork(policy_mismatch, config, overrides, &digest('8'))
            .await
            .is_err()
    );
    assert!(spawner.list().unwrap().is_empty());
    assert_eq!(journal_observer.state().unwrap().last_seq, None);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn aborted_execution_requires_recovery_and_reopen_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.journal");
    let journal = SessionJournal::open(&path, "session-1").unwrap();
    let started = Arc::new(Notify::new());
    let spawner = DurableSpawner::new(
        DurableChildStore::new(journal),
        Arc::new(BlockingSpawner {
            started: Arc::clone(&started),
        }),
    )
    .unwrap();
    let child_id = ChildId::new("aborted-child").unwrap();
    let config = config(child_id.as_str());
    let overrides = ForkOverrides::default();
    let declaration = record(
        child_id.as_str(),
        ChildOrigin::Delegate,
        &config,
        &overrides,
    );
    let task_spawner = spawner.clone();
    let task = tokio::spawn(async move {
        task_spawner
            .spawn_fork(declaration, config, overrides, &policy_digest())
            .await
    });
    started.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let recovered = spawner.inspect(&child_id).unwrap().unwrap();
    assert_eq!(recovered.status, DurableChildStatus::RecoveryRequired);
    assert!(matches!(
        recovered.recovery,
        ChildRecoveryState::Required { .. }
    ));
    let revision = recovered.revision;
    drop(spawner);

    let reopened = SessionJournal::open(&path, "session-1").unwrap();
    let spawner =
        DurableSpawner::new(DurableChildStore::new(reopened), Arc::new(ImmediateSpawner)).unwrap();
    assert_eq!(
        spawner.inspect(&child_id).unwrap().unwrap().revision,
        revision
    );
}

#[test]
fn startup_reconciles_a_persisted_running_child_once() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.journal");
    let child_id = ChildId::new("startup-orphan").unwrap();
    {
        let journal = SessionJournal::open(&path, "session-1").unwrap();
        let store = DurableChildStore::new(journal);
        let config = config(child_id.as_str());
        let overrides = ForkOverrides::default();
        store
            .declare(record(
                child_id.as_str(),
                ChildOrigin::Spawn,
                &config,
                &overrides,
            ))
            .unwrap();
        store
            .transition(
                child_id.clone(),
                "enqueue",
                0,
                101,
                DurableChildTransition::Enqueue,
            )
            .unwrap();
        store
            .transition(
                child_id.clone(),
                "start",
                1,
                102,
                DurableChildTransition::Start,
            )
            .unwrap();
    }

    let journal = SessionJournal::open(&path, "session-1").unwrap();
    let spawner =
        DurableSpawner::new(DurableChildStore::new(journal), Arc::new(ImmediateSpawner)).unwrap();
    let recovered = spawner.inspect(&child_id).unwrap().unwrap();
    assert_eq!(recovered.status, DurableChildStatus::RecoveryRequired);
    let revision = recovered.revision;
    drop(spawner);

    let journal = SessionJournal::open(&path, "session-1").unwrap();
    let spawner =
        DurableSpawner::new(DurableChildStore::new(journal), Arc::new(ImmediateSpawner)).unwrap();
    assert_eq!(
        spawner.inspect(&child_id).unwrap().unwrap().revision,
        revision
    );
}

#[tokio::test]
async fn terminal_payload_is_recovered_and_unknown_delivery_requires_reconciliation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.journal");
    let child_id = ChildId::new("delivery-child").unwrap();
    {
        let journal = SessionJournal::open(&path, "session-1").unwrap();
        let spawner =
            DurableSpawner::new(DurableChildStore::new(journal), Arc::new(ImmediateSpawner))
                .unwrap();
        let config = config("delivery-child");
        let overrides = ForkOverrides::default();
        spawner
            .spawn_fork(
                record(
                    child_id.as_str(),
                    ChildOrigin::Workflow,
                    &config,
                    &overrides,
                ),
                config,
                overrides,
                &policy_digest(),
            )
            .await
            .unwrap();
        let first = spawner.claim_result(&child_id).unwrap();
        let first = first.unwrap();
        assert_eq!(first.name, "delivery-child");
        assert_eq!(first.text, "completed");
        assert_eq!(first.usage.input_tokens, 11);
        assert!(spawner.claim_result(&child_id).unwrap().is_none());
    }

    let reopened = SessionJournal::open(&path, "session-1").unwrap();
    let spawner =
        DurableSpawner::new(DurableChildStore::new(reopened), Arc::new(ImmediateSpawner)).unwrap();
    assert!(spawner.claim_result(&child_id).unwrap().is_none());
    let evidence_digest = match spawner.inspect(&child_id).unwrap().unwrap().delivery_state {
        ChildDeliveryState::Unknown { evidence_digest } => evidence_digest,
        state => panic!("expected unknown delivery after restart, got {state:?}"),
    };
    spawner
        .reconcile_unknown_delivery(
            &child_id,
            evidence_digest,
            ChildDeliveryReconciliation::NotDelivered {
                proof_digest: digest('7'),
            },
        )
        .unwrap();
    let reclaimed = spawner.claim_result(&child_id).unwrap().unwrap();
    assert_eq!(reclaimed.text, "completed");
    drop(spawner);

    let reopened = SessionJournal::open(&path, "session-1").unwrap();
    let spawner =
        DurableSpawner::new(DurableChildStore::new(reopened), Arc::new(ImmediateSpawner)).unwrap();
    let evidence_digest = match spawner.inspect(&child_id).unwrap().unwrap().delivery_state {
        ChildDeliveryState::Unknown { evidence_digest } => evidence_digest,
        state => panic!("expected second unknown delivery after restart, got {state:?}"),
    };
    spawner
        .reconcile_unknown_delivery(
            &child_id,
            evidence_digest,
            ChildDeliveryReconciliation::NotDelivered {
                proof_digest: digest('4'),
            },
        )
        .unwrap();
    assert_eq!(
        spawner.claim_result(&child_id).unwrap().unwrap().text,
        "completed"
    );
    let receipt = digest('6');
    spawner
        .acknowledge_result(&child_id, receipt.clone())
        .unwrap();
    drop(spawner);

    let reopened = SessionJournal::open(&path, "session-1").unwrap();
    let spawner =
        DurableSpawner::new(DurableChildStore::new(reopened), Arc::new(ImmediateSpawner)).unwrap();
    assert_eq!(
        spawner.acknowledge_result(&child_id, receipt).unwrap(),
        DurableChildWrite::AlreadyCommitted
    );
    assert!(spawner.acknowledge_result(&child_id, digest('5')).is_err());
}

#[tokio::test]
async fn corrupt_or_missing_result_payload_fails_closed_without_claiming_delivery() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.journal");
    let journal = SessionJournal::open(&path, "session-1").unwrap();
    let spawner =
        DurableSpawner::new(DurableChildStore::new(journal), Arc::new(ImmediateSpawner)).unwrap();
    let child_id = ChildId::new("missing-payload").unwrap();
    let config = config(child_id.as_str());
    let overrides = ForkOverrides::default();
    spawner
        .spawn_fork(
            record(
                child_id.as_str(),
                ChildOrigin::Workflow,
                &config,
                &overrides,
            ),
            config,
            overrides,
            &policy_digest(),
        )
        .await
        .unwrap();
    let result_digest = spawner
        .inspect(&child_id)
        .unwrap()
        .unwrap()
        .result
        .unwrap()
        .exact_digest;
    let payload = path
        .parent()
        .unwrap()
        .join(".session.journal.effects")
        .join(result_digest);
    std::fs::write(&payload, b"corrupt").unwrap();
    assert!(spawner.claim_result(&child_id).is_err());
    std::fs::remove_file(payload).unwrap();
    assert!(spawner.claim_result(&child_id).is_err());
    assert_eq!(
        spawner.inspect(&child_id).unwrap().unwrap().delivery_state,
        ChildDeliveryState::Pending
    );
}

#[tokio::test]
async fn legacy_spawner_trait_remains_source_compatible() {
    let legacy: Arc<dyn Spawner> = Arc::new(ImmediateSpawner);
    let result = legacy
        .spawn_fork(config("legacy"), ForkOverrides::default())
        .await;
    assert_eq!(result.text, "completed");
}
