//! Journal-backed authority for child execution and supervision.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use wcore_types::message::TokenUsage;
use wcore_types::spawner::{
    ChildDeliveryReconciliation, ChildDeliveryState, ChildDesiredState, ChildId,
    ChildRecoveryState, DurableChildRecord, DurableChildResult, DurableChildStatus,
    DurableChildTransition, ForkOverrides, Spawner, SubAgentConfig, SubAgentResult,
};

use crate::durable_child::{DurableChildStore, DurableChildWrite};
use crate::session_journal::{JournalError, state_payload_digest};

const DURABLE_SPAWN_REQUEST_SCHEMA: &str = "durable-spawn-request/v1";
const DURABLE_RESULT_PAYLOAD_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableCancelDisposition {
    /// A live execution was signalled and will commit its cancelled state.
    Signalled,
    /// Work had not started, so cancellation was committed immediately.
    CancelledBeforeExecution,
    /// The request is durable, but no live process authority exists after restart.
    AwaitingRecovery,
    /// The child was already terminal; no mutation was needed.
    AlreadyTerminal,
}

#[derive(Debug, Error)]
pub enum DurableSpawnerError {
    #[error(transparent)]
    Journal(#[from] JournalError),
    #[error("unknown durable child {0}")]
    UnknownChild(ChildId),
    #[error("durable child {child_id} cannot start from {status:?}")]
    CannotStart {
        child_id: ChildId,
        status: DurableChildStatus,
    },
    #[error("durable child {0} already has a live execution")]
    AlreadyRunning(ChildId),
    #[error("system clock is before the Unix epoch")]
    Clock(#[source] std::time::SystemTimeError),
    #[error("sub-agent turn count does not fit the durable result schema")]
    TurnCountOverflow,
    #[error("durable child execution evidence mismatch: {0}")]
    EvidenceMismatch(&'static str),
    #[error("durable child result payload is invalid: {0}")]
    InvalidResultPayload(String),
    #[error("durable child {child_id} is already delivered with a different receipt")]
    DeliveryReceiptConflict { child_id: ChildId },
}

/// Journal-backed adapter for durable child execution and supervision.
///
/// The wrapped [`Spawner`] remains unchanged so legacy callers continue to
/// compile. Calling that executor directly is the explicit ephemeral path;
/// durable callers go through this adapter so declaration, cancellation,
/// terminal evidence, and result delivery share one journal authority.
#[derive(Clone)]
pub struct DurableSpawner {
    store: DurableChildStore,
    executor: Arc<dyn Spawner>,
    running: Arc<Mutex<BTreeMap<ChildId, CancellationToken>>>,
    mutations: Arc<Mutex<()>>,
}

impl DurableSpawner {
    pub fn new(
        store: DurableChildStore,
        executor: Arc<dyn Spawner>,
    ) -> Result<Self, DurableSpawnerError> {
        let spawner = Self {
            store,
            executor,
            running: Arc::new(Mutex::new(BTreeMap::new())),
            mutations: Arc::new(Mutex::new(())),
        };
        spawner.reconcile_startup()?;
        Ok(spawner)
    }

    /// Digest the exact executor inputs without persisting their plaintext.
    pub fn request_digest(
        config: &SubAgentConfig,
        overrides: &ForkOverrides,
    ) -> Result<String, DurableSpawnerError> {
        if config.temperature.is_some_and(|value| !value.is_finite()) {
            return Err(DurableSpawnerError::EvidenceMismatch(
                "temperature must be finite",
            ));
        }
        state_payload_digest(&serde_json::json!({
            "schema": DURABLE_SPAWN_REQUEST_SCHEMA,
            "config": {
                "name": config.name,
                "prompt": config.prompt,
                "max_turns": config.max_turns,
                "max_tokens": config.max_tokens,
                "system_prompt": config.system_prompt,
                "provider": config.provider,
                "model": config.model,
                "temperature": config.temperature,
            },
            "overrides": {
                "model": overrides.model,
                "effort": overrides.effort,
                "allowed_tools": overrides.allowed_tools,
            },
        }))
        .map_err(DurableSpawnerError::Journal)
    }

    pub fn inspect(&self, child_id: &ChildId) -> Result<Option<DurableChildRecord>, JournalError> {
        self.store.inspect(child_id)
    }

    pub fn list(&self) -> Result<Vec<DurableChildRecord>, JournalError> {
        self.store.list()
    }

    fn validate_execution_evidence(
        &self,
        record: &DurableChildRecord,
        config: &SubAgentConfig,
        overrides: &ForkOverrides,
        effective_policy_digest: &str,
    ) -> Result<(), DurableSpawnerError> {
        if record.request.exact_digest() != Self::request_digest(config, overrides)? {
            return Err(DurableSpawnerError::EvidenceMismatch("request digest"));
        }
        let provider = config
            .provider
            .as_deref()
            .ok_or(DurableSpawnerError::EvidenceMismatch(
                "provider must be resolved before durable execution",
            ))?;
        if record.provider.as_deref() != Some(provider) {
            return Err(DurableSpawnerError::EvidenceMismatch("provider"));
        }
        let model = overrides
            .model
            .as_deref()
            .or(config.model.as_deref())
            .ok_or(DurableSpawnerError::EvidenceMismatch(
                "model must be resolved before durable execution",
            ))?;
        if record.model.as_deref() != Some(model) {
            return Err(DurableSpawnerError::EvidenceMismatch("model"));
        }
        if record.policy_snapshot.exact_digest != effective_policy_digest {
            return Err(DurableSpawnerError::EvidenceMismatch("policy digest"));
        }
        Ok(())
    }

    fn reconcile_startup(&self) -> Result<(), DurableSpawnerError> {
        let _mutation = self.mutations.lock();
        for current in self.store.list()? {
            if current.status == DurableChildStatus::Running
                && matches!(current.recovery, ChildRecoveryState::Clean)
            {
                let phase = format!("executor-lost-{}", current.revision);
                let reason_digest = executor_lost_digest(&current.child_id, current.revision);
                self.transition_current(
                    &current,
                    &current.declaration_id,
                    &phase,
                    DurableChildTransition::RequireRecovery { reason_digest },
                )?;
                continue;
            }
            if current.status.is_terminal()
                && matches!(current.delivery_state, ChildDeliveryState::InFlight)
            {
                let phase = format!("delivery-interrupted-{}", current.revision);
                let evidence_digest =
                    delivery_interrupted_digest(&current.child_id, current.revision);
                self.transition_current(
                    &current,
                    &current.declaration_id,
                    &phase,
                    DurableChildTransition::DeliveryUnknown { evidence_digest },
                )?;
            }
        }
        Ok(())
    }

    /// Declare and execute one fork through the durable lifecycle.
    pub async fn spawn_fork(
        &self,
        record: DurableChildRecord,
        config: SubAgentConfig,
        overrides: ForkOverrides,
        effective_policy_digest: &str,
    ) -> Result<SubAgentResult, DurableSpawnerError> {
        self.validate_execution_evidence(&record, &config, &overrides, effective_policy_digest)?;
        let child_id = record.child_id.clone();
        let declaration_id = record.declaration_id.clone();
        let cancel = CancellationToken::new();

        {
            let _mutation = self.mutations.lock();
            self.store.declare(record)?;
            let current = self.required_child(&child_id)?;
            match current.status {
                DurableChildStatus::Prepared => {
                    self.transition_current(
                        &current,
                        &declaration_id,
                        "enqueue",
                        DurableChildTransition::Enqueue,
                    )?;
                }
                DurableChildStatus::Queued => {}
                status => {
                    return Err(DurableSpawnerError::CannotStart { child_id, status });
                }
            }
            let current = self.required_child(&child_id)?;
            self.transition_current(
                &current,
                &declaration_id,
                "start",
                DurableChildTransition::Start,
            )?;
            if self
                .running
                .lock()
                .insert(child_id.clone(), cancel.clone())
                .is_some()
            {
                return Err(DurableSpawnerError::AlreadyRunning(child_id));
            }
        }

        let mut running = RunningChildGuard {
            child_id: child_id.clone(),
            declaration_id: declaration_id.clone(),
            store: self.store.clone(),
            running: Arc::clone(&self.running),
            mutations: Arc::clone(&self.mutations),
            armed: true,
        };
        let execution = self.executor.spawn_fork(config, overrides);
        tokio::pin!(execution);

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let _mutation = self.mutations.lock();
                let current = self.required_child(&child_id)?;
                self.transition_current(
                    &current,
                    &declaration_id,
                    "cancelled",
                    DurableChildTransition::Cancel,
                )?;
                running.disarm();
                Ok(SubAgentResult::error(
                    child_id.as_str(),
                    "durable child cancelled before completion",
                ))
            }
            result = &mut execution => {
                let (payload, durable_result) = encode_result_payload(&result)?;
                self.store
                    .store_result_payload(&durable_result.exact_digest, &payload)?;
                let transition = if result.is_error {
                    DurableChildTransition::Fail { result: durable_result }
                } else {
                    DurableChildTransition::Succeed { result: durable_result }
                };
                let _mutation = self.mutations.lock();
                let current = self.required_child(&child_id)?;
                self.transition_current(&current, &declaration_id, "terminal", transition)?;
                running.disarm();
                Ok(result)
            }
        }
    }

    /// Persist a cancellation request and signal the live execution when owned.
    pub fn request_cancel(
        &self,
        child_id: &ChildId,
    ) -> Result<DurableCancelDisposition, DurableSpawnerError> {
        let _mutation = self.mutations.lock();
        let mut current = self.required_child(child_id)?;
        if current.status.is_terminal() {
            return Ok(DurableCancelDisposition::AlreadyTerminal);
        }
        if current.desired_state != ChildDesiredState::Cancel {
            let declaration_id = current.declaration_id.clone();
            self.transition_current(
                &current,
                &declaration_id,
                "cancel-request",
                DurableChildTransition::RequestCancel,
            )?;
            current = self.required_child(child_id)?;
        }

        if let Some(cancel) = self.running.lock().get(child_id).cloned() {
            cancel.cancel();
            return Ok(DurableCancelDisposition::Signalled);
        }

        if matches!(
            current.status,
            DurableChildStatus::Prepared | DurableChildStatus::Queued
        ) {
            let declaration_id = current.declaration_id.clone();
            self.transition_current(
                &current,
                &declaration_id,
                "cancelled",
                DurableChildTransition::Cancel,
            )?;
            return Ok(DurableCancelDisposition::CancelledBeforeExecution);
        }
        Ok(DurableCancelDisposition::AwaitingRecovery)
    }

    /// Claim a terminal result once. A crash after this claim cannot redeliver;
    /// the committed in-flight state must be explicitly reconciled.
    pub fn claim_result(
        &self,
        child_id: &ChildId,
    ) -> Result<Option<SubAgentResult>, DurableSpawnerError> {
        let _mutation = self.mutations.lock();
        let current = self.required_child(child_id)?;
        if !matches!(current.delivery_state, ChildDeliveryState::Pending) {
            return Ok(None);
        }
        let Some(result) = current.result.as_ref() else {
            return Ok(None);
        };
        let payload = self.store.load_result_payload(&result.exact_digest)?;
        let claimed = decode_result_payload(&payload, result)?;
        let phase = format!("delivery-claim-{}", current.revision);
        let write = self.transition_current(
            &current,
            &current.declaration_id,
            &phase,
            DurableChildTransition::DeliveryStarted,
        )?;
        Ok(matches!(write, DurableChildWrite::Appended(_)).then_some(claimed))
    }

    pub fn acknowledge_result(
        &self,
        child_id: &ChildId,
        receipt_digest: String,
    ) -> Result<DurableChildWrite, DurableSpawnerError> {
        let _mutation = self.mutations.lock();
        let current = self.required_child(child_id)?;
        if let ChildDeliveryState::Delivered {
            receipt_digest: committed,
        } = &current.delivery_state
        {
            return if committed == &receipt_digest {
                Ok(DurableChildWrite::AlreadyCommitted)
            } else {
                Err(DurableSpawnerError::DeliveryReceiptConflict {
                    child_id: child_id.clone(),
                })
            };
        }
        self.transition_current(
            &current,
            &current.declaration_id,
            "delivery-ack",
            DurableChildTransition::DeliveryDelivered { receipt_digest },
        )
    }

    pub fn fail_delivery(
        &self,
        child_id: &ChildId,
        error_digest: String,
    ) -> Result<DurableChildWrite, DurableSpawnerError> {
        let _mutation = self.mutations.lock();
        let current = self.required_child(child_id)?;
        let phase = format!("delivery-failed-{}-{error_digest}", current.revision);
        self.transition_current(
            &current,
            &current.declaration_id,
            &phase,
            DurableChildTransition::DeliveryFailed { error_digest },
        )
    }

    pub fn mark_delivery_unknown(
        &self,
        child_id: &ChildId,
        evidence_digest: String,
    ) -> Result<DurableChildWrite, DurableSpawnerError> {
        let _mutation = self.mutations.lock();
        let current = self.required_child(child_id)?;
        let phase = format!("delivery-unknown-{}-{evidence_digest}", current.revision);
        self.transition_current(
            &current,
            &current.declaration_id,
            &phase,
            DurableChildTransition::DeliveryUnknown { evidence_digest },
        )
    }

    pub fn retry_failed_delivery(
        &self,
        child_id: &ChildId,
        prior_error_digest: String,
    ) -> Result<DurableChildWrite, DurableSpawnerError> {
        let _mutation = self.mutations.lock();
        let current = self.required_child(child_id)?;
        let phase = format!("delivery-retry-{}-{prior_error_digest}", current.revision);
        self.transition_current(
            &current,
            &current.declaration_id,
            &phase,
            DurableChildTransition::RetryFailedDelivery { prior_error_digest },
        )
    }

    pub fn reconcile_unknown_delivery(
        &self,
        child_id: &ChildId,
        prior_evidence_digest: String,
        resolution: ChildDeliveryReconciliation,
    ) -> Result<DurableChildWrite, DurableSpawnerError> {
        let _mutation = self.mutations.lock();
        let current = self.required_child(child_id)?;
        let phase = format!(
            "delivery-reconcile-{}-{prior_evidence_digest}",
            current.revision
        );
        self.transition_current(
            &current,
            &current.declaration_id,
            &phase,
            DurableChildTransition::ReconcileUnknownDelivery {
                prior_evidence_digest,
                resolution,
            },
        )
    }

    fn required_child(
        &self,
        child_id: &ChildId,
    ) -> Result<DurableChildRecord, DurableSpawnerError> {
        self.store
            .inspect(child_id)?
            .ok_or_else(|| DurableSpawnerError::UnknownChild(child_id.clone()))
    }

    fn transition_current(
        &self,
        current: &DurableChildRecord,
        declaration_id: &str,
        phase: &str,
        transition: DurableChildTransition,
    ) -> Result<DurableChildWrite, DurableSpawnerError> {
        let now = now_unix_ms()?;
        self.store
            .transition(
                current.child_id.clone(),
                event_id(declaration_id, phase),
                current.revision,
                now.max(current.timestamps.updated_at_unix_ms),
                transition,
            )
            .map_err(Into::into)
    }
}

struct RunningChildGuard {
    child_id: ChildId,
    declaration_id: String,
    store: DurableChildStore,
    running: Arc<Mutex<BTreeMap<ChildId, CancellationToken>>>,
    mutations: Arc<Mutex<()>>,
    armed: bool,
}

impl RunningChildGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RunningChildGuard {
    fn drop(&mut self) {
        self.running.lock().remove(&self.child_id);
        if !self.armed {
            return;
        }
        let _mutation = self.mutations.lock();
        let Ok(Some(current)) = self.store.inspect(&self.child_id) else {
            return;
        };
        if current.status != DurableChildStatus::Running
            || !matches!(current.recovery, ChildRecoveryState::Clean)
        {
            return;
        }
        let transition = DurableChildTransition::RequireRecovery {
            reason_digest: executor_lost_digest(&self.child_id, current.revision),
        };
        let at_unix_ms = now_unix_ms()
            .unwrap_or(current.timestamps.updated_at_unix_ms)
            .max(current.timestamps.updated_at_unix_ms);
        if let Err(error) = self.store.transition(
            self.child_id.clone(),
            event_id(
                &self.declaration_id,
                &format!("executor-lost-{}", current.revision),
            ),
            current.revision,
            at_unix_ms,
            transition,
        ) {
            tracing::error!(child_id = %self.child_id, %error, "failed to persist lost durable child executor");
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableResultPayload {
    schema_version: u16,
    name: String,
    text: String,
    usage: TokenUsage,
    turns: u64,
    is_error: bool,
}

fn encode_result_payload(
    result: &SubAgentResult,
) -> Result<(Vec<u8>, DurableChildResult), DurableSpawnerError> {
    let turns = u64::try_from(result.turns).map_err(|_| DurableSpawnerError::TurnCountOverflow)?;
    let payload = DurableResultPayload {
        schema_version: DURABLE_RESULT_PAYLOAD_SCHEMA_VERSION,
        name: result.name.clone(),
        text: result.text.clone(),
        usage: result.usage.clone(),
        turns,
        is_error: result.is_error,
    };
    let bytes = serde_json::to_vec(&payload).map_err(|error| {
        DurableSpawnerError::InvalidResultPayload(format!("cannot encode payload: {error}"))
    })?;
    let durable = DurableChildResult {
        exact_digest: format!("{:x}", Sha256::digest(&bytes)),
        turns,
        input_tokens: result.usage.input_tokens,
        output_tokens: result.usage.output_tokens,
        artifact_digests: Vec::new(),
    };
    Ok((bytes, durable))
}

fn decode_result_payload(
    bytes: &[u8],
    expected: &DurableChildResult,
) -> Result<SubAgentResult, DurableSpawnerError> {
    let payload: DurableResultPayload = serde_json::from_slice(bytes).map_err(|error| {
        DurableSpawnerError::InvalidResultPayload(format!("cannot decode payload: {error}"))
    })?;
    if payload.schema_version != DURABLE_RESULT_PAYLOAD_SCHEMA_VERSION {
        return Err(DurableSpawnerError::InvalidResultPayload(format!(
            "unsupported payload schema {}",
            payload.schema_version
        )));
    }
    let result = SubAgentResult {
        name: payload.name,
        text: payload.text,
        usage: payload.usage,
        turns: usize::try_from(payload.turns)
            .map_err(|_| DurableSpawnerError::TurnCountOverflow)?,
        is_error: payload.is_error,
    };
    let (_, actual) = encode_result_payload(&result)?;
    if &actual != expected {
        return Err(DurableSpawnerError::InvalidResultPayload(
            "payload metadata does not match the terminal record".into(),
        ));
    }
    Ok(result)
}

fn event_id(declaration_id: &str, phase: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(declaration_id.as_bytes());
    digest.update([0]);
    digest.update(phase.as_bytes());
    format!("{phase}:{:x}", digest.finalize())
}

fn executor_lost_digest(child_id: &ChildId, revision: u64) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("durable-child-executor-lost:{child_id}:{revision}").as_bytes())
    )
}

fn delivery_interrupted_digest(child_id: &ChildId, revision: u64) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            format!("durable-child-delivery-interrupted:{child_id}:{revision}").as_bytes()
        )
    )
}

fn now_unix_ms() -> Result<u64, DurableSpawnerError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(DurableSpawnerError::Clock)?
        .as_millis();
    Ok(u64::try_from(millis).unwrap_or(u64::MAX))
}
