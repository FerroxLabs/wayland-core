//! Journal-backed authority for child execution and supervision.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use wcore_types::execution_policy::EffectiveExecutionPolicy;
use wcore_types::message::TokenUsage;
use wcore_types::spawner::{
    ChildDeliveryReconciliation, ChildDeliveryState, ChildDesiredState, ChildId,
    ChildRecoveryState, DurableChildRecord, DurableChildResult, DurableChildStatus,
    DurableChildTransition, ForkOverrides, Spawner, SubAgentConfig, SubAgentResult,
};

use crate::durable_child::{DurableChildStore, DurableChildWrite};
use crate::session_journal::{JournalError, SessionJournal, state_payload_digest};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableAuthorityFailure {
    RecoveryInspect,
    RecoveryPersist,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableSpawnerPoison {
    pub child_id: ChildId,
    pub failure: DurableAuthorityFailure,
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
    #[error("durable spawner authority is poisoned after {failure:?} failed for child {child_id}")]
    AuthorityPoisoned {
        child_id: ChildId,
        failure: DurableAuthorityFailure,
    },
    #[error("durable child session authority is not bound")]
    AuthorityUnbound,
    #[error("durable child session authority generation overflowed")]
    AuthorityGenerationOverflow,
    #[error(
        "durable child launch authority is stale: resolved for session {launch_session} generation {launch_generation}, current authority is {current}"
    )]
    StaleAuthority {
        launch_session: String,
        launch_generation: u64,
        current: String,
    },
    #[error("durable child journal session mismatch: expected {expected}, found {found}")]
    SessionMismatch { expected: String, found: String },
    #[error("durable child journal {0} has no matching canonical baseline")]
    NonCanonicalSession(String),
    #[error("fresh durable child session {0} already contains child history")]
    FreshSessionHasChildHistory(String),
    #[error("durable child executor is unavailable for this session authority")]
    ExecutorUnavailable,
    #[error("durable child session policy authority is not installed")]
    PolicyAuthorityUnbound,
    #[error("durable child session policy authority conflicts with the installed policy")]
    PolicyAuthorityConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableAuthorityToken {
    session_id: String,
    generation: u64,
}

/// Session-pinned supervision authority for every durable child origin.
///
/// A handle never follows a later session bind. Operations against a stale
/// generation fail closed rather than observing or mutating the new session.
#[derive(Clone)]
pub struct DurableChildSupervisor {
    authority: DurableSessionAuthority,
    token: DurableAuthorityToken,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedExecutionEvidence<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    pub effective_policy_digest: &'a str,
}

impl DurableAuthorityToken {
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Default)]
pub struct DurableSessionAuthority {
    state: Arc<Mutex<DurableSessionAuthorityState>>,
}

#[derive(Default)]
struct DurableSessionAuthorityState {
    generation: u64,
    binding: Option<DurableSessionBinding>,
    effective_policy: Option<EffectiveExecutionPolicy>,
}

struct DurableSessionBinding {
    session_id: String,
    spawner: DurableSpawner,
}

impl DurableSessionAuthority {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn install_effective_policy(
        &self,
        policy: EffectiveExecutionPolicy,
    ) -> Result<(), DurableSpawnerError> {
        let mut state = self.state.lock();
        if state
            .effective_policy
            .as_ref()
            .is_some_and(|installed| installed != &policy)
        {
            return Err(DurableSpawnerError::PolicyAuthorityConflict);
        }
        state.effective_policy = Some(policy);
        Ok(())
    }

    pub(crate) fn effective_policy(&self) -> Result<EffectiveExecutionPolicy, DurableSpawnerError> {
        self.state
            .lock()
            .effective_policy
            .clone()
            .ok_or(DurableSpawnerError::PolicyAuthorityUnbound)
    }

    /// Replace the active journal authority atomically.
    ///
    /// The old binding is cleared before the candidate is inspected. A failed
    /// bind therefore leaves every clone fail-closed instead of retaining stale
    /// authority from the previous session.
    pub(crate) fn bind(
        &self,
        journal: SessionJournal,
        expected_session_id: &str,
    ) -> Result<DurableAuthorityToken, DurableSpawnerError> {
        self.bind_with_recovery(journal, expected_session_id, true)
    }

    /// Bind a session created for this run without paying recovery replay.
    ///
    /// The journal itself proves freshness. A caller cannot misclassify an
    /// existing child-bearing journal as fresh to bypass reconciliation.
    pub(crate) fn bind_fresh(
        &self,
        journal: SessionJournal,
        expected_session_id: &str,
    ) -> Result<DurableAuthorityToken, DurableSpawnerError> {
        self.bind_with_recovery(journal, expected_session_id, false)
    }

    fn bind_with_recovery(
        &self,
        journal: SessionJournal,
        expected_session_id: &str,
        reconcile_existing: bool,
    ) -> Result<DurableAuthorityToken, DurableSpawnerError> {
        let mut state = self.state.lock();
        state.binding = None;
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or(DurableSpawnerError::AuthorityGenerationOverflow)?;
        let found = journal.session_id()?;
        if found != expected_session_id {
            return Err(DurableSpawnerError::SessionMismatch {
                expected: expected_session_id.to_owned(),
                found,
            });
        }
        let journal_state = journal.state()?;
        if journal_state.session_id.as_deref() != Some(found.as_str())
            || journal_state.imported_baseline.is_none()
        {
            return Err(DurableSpawnerError::NonCanonicalSession(found));
        }
        if !reconcile_existing
            && (!journal_state.children.is_empty() || !journal_state.deliveries.is_empty())
        {
            return Err(DurableSpawnerError::FreshSessionHasChildHistory(found));
        }
        let token = DurableAuthorityToken {
            session_id: found.clone(),
            generation: state.generation,
        };
        let store = DurableChildStore::new(journal);
        let spawner = if reconcile_existing {
            DurableSpawner::for_session(store)?
        } else {
            DurableSpawner::for_fresh_session(store)
        };
        state.binding = Some(DurableSessionBinding {
            session_id: found,
            spawner,
        });
        Ok(token)
    }

    pub(crate) fn token(&self) -> Result<DurableAuthorityToken, DurableSpawnerError> {
        let state = self.state.lock();
        let binding = state
            .binding
            .as_ref()
            .ok_or(DurableSpawnerError::AuthorityUnbound)?;
        Ok(DurableAuthorityToken {
            session_id: binding.session_id.clone(),
            generation: state.generation,
        })
    }

    pub(crate) fn supervisor(&self) -> Result<DurableChildSupervisor, DurableSpawnerError> {
        Ok(DurableChildSupervisor {
            authority: self.clone(),
            token: self.token()?,
        })
    }

    fn with_runtime<T>(
        &self,
        token: &DurableAuthorityToken,
        use_runtime: impl FnOnce(&DurableSpawner) -> Result<T, DurableSpawnerError>,
    ) -> Result<T, DurableSpawnerError> {
        let state = self.state.lock();
        let binding =
            state
                .binding
                .as_ref()
                .ok_or_else(|| DurableSpawnerError::StaleAuthority {
                    launch_session: token.session_id.clone(),
                    launch_generation: token.generation,
                    current: format!("unbound generation {}", state.generation),
                })?;
        if state.generation != token.generation || binding.session_id != token.session_id {
            return Err(DurableSpawnerError::StaleAuthority {
                launch_session: token.session_id.clone(),
                launch_generation: token.generation,
                current: format!(
                    "session {} generation {}",
                    binding.session_id, state.generation
                ),
            });
        }
        use_runtime(&binding.spawner)
    }

    pub(crate) fn with_store<T>(
        &self,
        token: &DurableAuthorityToken,
        use_store: impl FnOnce(&DurableChildStore) -> Result<T, DurableSpawnerError>,
    ) -> Result<T, DurableSpawnerError> {
        let state = self.state.lock();
        let binding =
            state
                .binding
                .as_ref()
                .ok_or_else(|| DurableSpawnerError::StaleAuthority {
                    launch_session: token.session_id.clone(),
                    launch_generation: token.generation,
                    current: format!("unbound generation {}", state.generation),
                })?;
        if state.generation != token.generation || binding.session_id != token.session_id {
            return Err(DurableSpawnerError::StaleAuthority {
                launch_session: token.session_id.clone(),
                launch_generation: token.generation,
                current: format!(
                    "session {} generation {}",
                    binding.session_id, state.generation
                ),
            });
        }
        use_store(&binding.spawner.store)
    }

    pub(crate) fn admit_resolved(
        &self,
        token: &DurableAuthorityToken,
        record: DurableChildRecord,
        config: &SubAgentConfig,
        overrides: &ForkOverrides,
        evidence: ResolvedExecutionEvidence<'_>,
    ) -> Result<AdmittedDurableSpawn, DurableSpawnerError> {
        let state = self.state.lock();
        let binding =
            state
                .binding
                .as_ref()
                .ok_or_else(|| DurableSpawnerError::StaleAuthority {
                    launch_session: token.session_id.clone(),
                    launch_generation: token.generation,
                    current: format!("unbound generation {}", state.generation),
                })?;
        if state.generation != token.generation || binding.session_id != token.session_id {
            return Err(DurableSpawnerError::StaleAuthority {
                launch_session: token.session_id.clone(),
                launch_generation: token.generation,
                current: format!(
                    "session {} generation {}",
                    binding.session_id, state.generation
                ),
            });
        }
        binding
            .spawner
            .admit_resolved(record, config, overrides, evidence)
    }
}

impl DurableChildSupervisor {
    #[must_use]
    pub fn session_id(&self) -> &str {
        self.token.session_id()
    }

    pub fn list(&self) -> Result<Vec<DurableChildRecord>, DurableSpawnerError> {
        self.authority
            .with_runtime(&self.token, |runtime| Ok(runtime.list()?))
    }

    pub fn inspect(
        &self,
        child_id: &ChildId,
    ) -> Result<Option<DurableChildRecord>, DurableSpawnerError> {
        self.authority
            .with_runtime(&self.token, |runtime| Ok(runtime.inspect(child_id)?))
    }

    pub fn request_cancel(
        &self,
        child_id: &ChildId,
    ) -> Result<DurableCancelDisposition, DurableSpawnerError> {
        self.authority
            .with_runtime(&self.token, |runtime| runtime.request_cancel(child_id))
    }
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
    executor: Option<Arc<dyn Spawner>>,
    running: Arc<Mutex<BTreeMap<ChildId, CancellationToken>>>,
    mutations: Arc<Mutex<()>>,
    poison: Arc<Mutex<Option<DurableSpawnerPoison>>>,
    drop_recovery: Arc<dyn DropRecoveryAuthority>,
}

pub(crate) struct AdmittedDurableSpawn {
    runtime: DurableSpawner,
    child_id: ChildId,
    name: String,
    declaration_id: String,
    cancel: CancellationToken,
    running: RunningChildGuard,
}

impl DurableSpawner {
    pub fn new(
        store: DurableChildStore,
        executor: Arc<dyn Spawner>,
    ) -> Result<Self, DurableSpawnerError> {
        let drop_recovery: Arc<dyn DropRecoveryAuthority> = Arc::new(store.clone());
        let spawner = Self {
            store,
            executor: Some(executor),
            running: Arc::new(Mutex::new(BTreeMap::new())),
            mutations: Arc::new(Mutex::new(())),
            poison: Arc::new(Mutex::new(None)),
            drop_recovery,
        };
        spawner.reconcile_startup()?;
        Ok(spawner)
    }

    fn for_session(store: DurableChildStore) -> Result<Self, DurableSpawnerError> {
        let spawner = Self::for_fresh_session(store);
        spawner.reconcile_startup()?;
        Ok(spawner)
    }

    fn for_fresh_session(store: DurableChildStore) -> Self {
        let drop_recovery: Arc<dyn DropRecoveryAuthority> = Arc::new(store.clone());
        Self {
            store,
            executor: None,
            running: Arc::new(Mutex::new(BTreeMap::new())),
            mutations: Arc::new(Mutex::new(())),
            poison: Arc::new(Mutex::new(None)),
            drop_recovery,
        }
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
                "requested_workspace": overrides.requested_workspace(),
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

    /// Return the in-process fail-closed authority state.
    ///
    /// A poisoned adapter rejects every spawn, mutation, and delivery action.
    /// Construct a new adapter over the journal to run startup reconciliation
    /// and re-establish authority.
    pub fn authority_poison(&self) -> Option<DurableSpawnerPoison> {
        self.poison.lock().clone()
    }

    fn validate_execution_evidence(
        &self,
        record: &DurableChildRecord,
        config: &SubAgentConfig,
        overrides: &ForkOverrides,
        provider: &str,
        model: &str,
        effective_policy_digest: &str,
    ) -> Result<(), DurableSpawnerError> {
        if record.request.exact_digest() != Self::request_digest(config, overrides)? {
            return Err(DurableSpawnerError::EvidenceMismatch("request digest"));
        }
        if provider.trim().is_empty() {
            return Err(DurableSpawnerError::EvidenceMismatch(
                "provider must be resolved before durable execution",
            ));
        }
        if record.provider.as_deref() != Some(provider) {
            return Err(DurableSpawnerError::EvidenceMismatch("provider"));
        }
        if model.trim().is_empty() {
            return Err(DurableSpawnerError::EvidenceMismatch(
                "model must be resolved before durable execution",
            ));
        }
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
        let executor = self
            .executor
            .as_ref()
            .cloned()
            .ok_or(DurableSpawnerError::ExecutorUnavailable)?;
        let provider = config
            .provider
            .clone()
            .ok_or(DurableSpawnerError::EvidenceMismatch(
                "provider must be resolved before durable execution",
            ))?;
        let model = overrides
            .model
            .clone()
            .or_else(|| config.model.clone())
            .ok_or(DurableSpawnerError::EvidenceMismatch(
                "model must be resolved before durable execution",
            ))?;
        let execution = executor.spawn_fork(config.clone(), overrides.clone());
        self.spawn_resolved(
            record,
            &config,
            &overrides,
            ResolvedExecutionEvidence {
                provider: &provider,
                model: &model,
                effective_policy_digest,
            },
            execution,
        )
        .await
    }

    /// Declare and execute an already-resolved child through this session's
    /// single durable runtime.
    pub(crate) async fn spawn_resolved<F>(
        &self,
        record: DurableChildRecord,
        config: &SubAgentConfig,
        overrides: &ForkOverrides,
        evidence: ResolvedExecutionEvidence<'_>,
        execution: F,
    ) -> Result<SubAgentResult, DurableSpawnerError>
    where
        F: Future<Output = SubAgentResult> + Send,
    {
        self.admit_resolved(record, config, overrides, evidence)?
            .execute(execution)
            .await
    }

    fn admit_resolved(
        &self,
        record: DurableChildRecord,
        config: &SubAgentConfig,
        overrides: &ForkOverrides,
        evidence: ResolvedExecutionEvidence<'_>,
    ) -> Result<AdmittedDurableSpawn, DurableSpawnerError> {
        let child_id = record.child_id.clone();
        let declaration_id = record.declaration_id.clone();
        let cancel = CancellationToken::new();

        {
            let _mutation = self.mutations.lock();
            self.ensure_healthy()?;
            self.validate_execution_evidence(
                &record,
                config,
                overrides,
                evidence.provider,
                evidence.model,
                evidence.effective_policy_digest,
            )?;
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

        Ok(AdmittedDurableSpawn {
            runtime: self.clone(),
            child_id: child_id.clone(),
            name: config.name.clone(),
            declaration_id: declaration_id.clone(),
            cancel,
            running: RunningChildGuard {
                child_id,
                declaration_id,
                recovery: Arc::clone(&self.drop_recovery),
                running: Arc::clone(&self.running),
                mutations: Arc::clone(&self.mutations),
                poison: Arc::clone(&self.poison),
                armed: true,
            },
        })
    }

    /// Persist a cancellation request and signal the live execution when owned.
    pub fn request_cancel(
        &self,
        child_id: &ChildId,
    ) -> Result<DurableCancelDisposition, DurableSpawnerError> {
        let _mutation = self.mutations.lock();
        self.ensure_healthy()?;
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
}

impl AdmittedDurableSpawn {
    pub(crate) async fn execute<F>(
        self,
        execution: F,
    ) -> Result<SubAgentResult, DurableSpawnerError>
    where
        F: Future<Output = SubAgentResult> + Send,
    {
        self.execute_with_parent_cancel(execution, CancellationToken::new())
            .await
    }

    pub(crate) async fn execute_with_parent_cancel<F>(
        mut self,
        execution: F,
        parent_cancel: CancellationToken,
    ) -> Result<SubAgentResult, DurableSpawnerError>
    where
        F: Future<Output = SubAgentResult> + Send,
    {
        tokio::pin!(execution);

        tokio::select! {
            biased;
            () = parent_cancel.cancelled() => self.commit_cancelled(),
            () = self.cancel.cancelled() => {
                self.commit_cancelled()
            }
            result = &mut execution => {
                let (payload, durable_result) = encode_result_payload(&result)?;
                let _mutation = self.runtime.mutations.lock();
                self.runtime.ensure_healthy()?;
                self.runtime.store
                    .store_result_payload(&durable_result.exact_digest, &payload)?;
                let transition = if result.is_error {
                    DurableChildTransition::Fail { result: durable_result }
                } else {
                    DurableChildTransition::Succeed { result: durable_result }
                };
                let current = self.runtime.required_child(&self.child_id)?;
                self.runtime.transition_current(
                    &current,
                    &self.declaration_id,
                    "terminal",
                    transition,
                )?;
                self.running.disarm();
                Ok(result)
            }
        }
    }

    fn commit_cancelled(&mut self) -> Result<SubAgentResult, DurableSpawnerError> {
        let _mutation = self.runtime.mutations.lock();
        self.runtime.ensure_healthy()?;
        let mut current = self.runtime.required_child(&self.child_id)?;
        if current.desired_state != ChildDesiredState::Cancel {
            self.runtime.transition_current(
                &current,
                &self.declaration_id,
                "cancel-request",
                DurableChildTransition::RequestCancel,
            )?;
            current = self.runtime.required_child(&self.child_id)?;
        }
        self.runtime.transition_current(
            &current,
            &self.declaration_id,
            "cancelled",
            DurableChildTransition::Cancel,
        )?;
        self.running.disarm();
        Ok(SubAgentResult::error(
            &self.name,
            "durable child cancelled before completion",
        ))
    }
}

impl DurableSpawner {
    /// Claim a terminal result once. A crash after this claim cannot redeliver;
    /// the committed in-flight state must be explicitly reconciled.
    pub fn claim_result(
        &self,
        child_id: &ChildId,
    ) -> Result<Option<SubAgentResult>, DurableSpawnerError> {
        let _mutation = self.mutations.lock();
        self.ensure_healthy()?;
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
        self.ensure_healthy()?;
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
        self.ensure_healthy()?;
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
        self.ensure_healthy()?;
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
        self.ensure_healthy()?;
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
        self.ensure_healthy()?;
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

    fn ensure_healthy(&self) -> Result<(), DurableSpawnerError> {
        match self.poison.lock().clone() {
            Some(poison) => Err(DurableSpawnerError::AuthorityPoisoned {
                child_id: poison.child_id,
                failure: poison.failure,
            }),
            None => Ok(()),
        }
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

enum DropRecoveryError {
    Inspect(JournalError),
    Persist(JournalError),
}

trait DropRecoveryAuthority: Send + Sync {
    fn require_recovery_if_running(
        &self,
        child_id: &ChildId,
        declaration_id: &str,
    ) -> Result<(), DropRecoveryError>;
}

impl DropRecoveryAuthority for DurableChildStore {
    fn require_recovery_if_running(
        &self,
        child_id: &ChildId,
        declaration_id: &str,
    ) -> Result<(), DropRecoveryError> {
        let current = self
            .inspect(child_id)
            .map_err(DropRecoveryError::Inspect)?
            .ok_or_else(|| {
                DropRecoveryError::Inspect(JournalError::InvalidTransition(format!(
                    "lost durable child {child_id} while reconciling its executor"
                )))
            })?;
        if current.status != DurableChildStatus::Running
            || !matches!(current.recovery, ChildRecoveryState::Clean)
        {
            return Ok(());
        }
        let transition = DurableChildTransition::RequireRecovery {
            reason_digest: executor_lost_digest(child_id, current.revision),
        };
        let at_unix_ms = now_unix_ms()
            .unwrap_or(current.timestamps.updated_at_unix_ms)
            .max(current.timestamps.updated_at_unix_ms);
        self.transition(
            child_id.clone(),
            event_id(
                declaration_id,
                &format!("executor-lost-{}", current.revision),
            ),
            current.revision,
            at_unix_ms,
            transition,
        )
        .map(|_| ())
        .map_err(DropRecoveryError::Persist)
    }
}

struct RunningChildGuard {
    child_id: ChildId,
    declaration_id: String,
    recovery: Arc<dyn DropRecoveryAuthority>,
    running: Arc<Mutex<BTreeMap<ChildId, CancellationToken>>>,
    mutations: Arc<Mutex<()>>,
    poison: Arc<Mutex<Option<DurableSpawnerPoison>>>,
    armed: bool,
}

impl RunningChildGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RunningChildGuard {
    fn drop(&mut self) {
        if !self.armed {
            self.running.lock().remove(&self.child_id);
            return;
        }
        let _mutation = self.mutations.lock();
        self.running.lock().remove(&self.child_id);
        if let Err(error) = self
            .recovery
            .require_recovery_if_running(&self.child_id, &self.declaration_id)
        {
            let (failure, error) = match error {
                DropRecoveryError::Inspect(error) => {
                    (DurableAuthorityFailure::RecoveryInspect, error)
                }
                DropRecoveryError::Persist(error) => {
                    (DurableAuthorityFailure::RecoveryPersist, error)
                }
            };
            let poison = DurableSpawnerPoison {
                child_id: self.child_id.clone(),
                failure,
            };
            self.poison.lock().get_or_insert(poison);
            tracing::error!(child_id = %self.child_id, %error, ?failure, "durable spawner authority poisoned while reconciling a lost executor");
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

pub(crate) fn now_unix_ms() -> Result<u64, DurableSpawnerError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(DurableSpawnerError::Clock)?
        .as_millis();
    Ok(u64::try_from(millis).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use wcore_types::spawner::{
        ChildDeliveryTarget, ChildOrigin, ChildParent, ChildPolicySnapshot, ChildRequestEvidence,
        ChildTimestamps, ChildWorkspace, ChildWorkspaceMode, DURABLE_CHILD_SCHEMA_VERSION,
    };

    use super::*;
    use crate::session_journal::SessionJournal;

    struct UnusedSpawner;

    #[async_trait]
    impl Spawner for UnusedSpawner {
        async fn spawn_fork(
            &self,
            config: SubAgentConfig,
            _overrides: ForkOverrides,
        ) -> SubAgentResult {
            SubAgentResult::error(
                &config.name,
                "executor must not run while authority is poisoned",
            )
        }
    }

    struct FailingRecovery(DurableAuthorityFailure);

    impl DropRecoveryAuthority for FailingRecovery {
        fn require_recovery_if_running(
            &self,
            _child_id: &ChildId,
            _declaration_id: &str,
        ) -> Result<(), DropRecoveryError> {
            let error = JournalError::WriterFaulted;
            match self.0 {
                DurableAuthorityFailure::RecoveryInspect => Err(DropRecoveryError::Inspect(error)),
                DurableAuthorityFailure::RecoveryPersist => Err(DropRecoveryError::Persist(error)),
            }
        }
    }

    fn poison_with_drop_failure(
        failure: DurableAuthorityFailure,
    ) -> (tempfile::TempDir, DurableSpawner, ChildId) {
        let temp = tempfile::tempdir().unwrap();
        let journal_path = temp.path().join("session.journal");
        let journal = SessionJournal::open(&journal_path, "session-1").unwrap();
        let spawner =
            DurableSpawner::new(DurableChildStore::new(journal), Arc::new(UnusedSpawner)).unwrap();
        let child_id = ChildId::new("drop-fault-child").unwrap();
        spawner
            .running
            .lock()
            .insert(child_id.clone(), CancellationToken::new());
        let guard = RunningChildGuard {
            child_id: child_id.clone(),
            declaration_id: "declare-drop-fault-child".into(),
            recovery: Arc::new(FailingRecovery(failure)),
            running: Arc::clone(&spawner.running),
            mutations: Arc::clone(&spawner.mutations),
            poison: Arc::clone(&spawner.poison),
            armed: true,
        };

        drop(guard);
        (temp, spawner, child_id)
    }

    fn assert_poisoned(
        spawner: &DurableSpawner,
        child_id: &ChildId,
        failure: DurableAuthorityFailure,
    ) {
        assert_eq!(
            spawner.authority_poison(),
            Some(DurableSpawnerPoison {
                child_id: child_id.clone(),
                failure,
            })
        );
        assert!(!spawner.running.lock().contains_key(child_id));
        assert!(matches!(
            spawner.request_cancel(child_id),
            Err(DurableSpawnerError::AuthorityPoisoned {
                child_id: poisoned,
                failure: observed,
            }) if poisoned == *child_id && observed == failure
        ));
        assert!(matches!(
            spawner.claim_result(child_id),
            Err(DurableSpawnerError::AuthorityPoisoned {
                child_id: poisoned,
                failure: observed,
            }) if poisoned == *child_id && observed == failure
        ));
    }

    #[test]
    fn guard_inspect_failure_poison_is_shared_and_rejects_authority_actions() {
        let (_temp, spawner, child_id) =
            poison_with_drop_failure(DurableAuthorityFailure::RecoveryInspect);
        assert_poisoned(
            &spawner,
            &child_id,
            DurableAuthorityFailure::RecoveryInspect,
        );
        assert_poisoned(
            &spawner.clone(),
            &child_id,
            DurableAuthorityFailure::RecoveryInspect,
        );
    }

    #[test]
    fn guard_write_failure_poison_clears_only_after_reconstruction() {
        let (temp, spawner, child_id) =
            poison_with_drop_failure(DurableAuthorityFailure::RecoveryPersist);
        assert_poisoned(
            &spawner,
            &child_id,
            DurableAuthorityFailure::RecoveryPersist,
        );

        let journal_path = temp.path().join("session.journal");
        drop(spawner);
        let journal = SessionJournal::open(journal_path, "session-1").unwrap();
        let reconstructed =
            DurableSpawner::new(DurableChildStore::new(journal), Arc::new(UnusedSpawner)).unwrap();
        assert_eq!(reconstructed.authority_poison(), None);
    }

    fn canonical_journal(
        directory: &std::path::Path,
        session_id: &str,
    ) -> (crate::session::SessionManager, SessionJournal) {
        let manager = crate::session::SessionManager::new(directory.to_path_buf(), 10);
        let session = manager
            .create("test", "test-model", "/tmp", Some(session_id))
            .unwrap();
        manager.persist_first_message(&session).unwrap();
        let active = manager.load_for_run(&session.id).unwrap();
        (manager, active.journal)
    }

    fn child_record(session_id: &str, child_id: &str) -> DurableChildRecord {
        DurableChildRecord {
            schema_version: DURABLE_CHILD_SCHEMA_VERSION,
            declaration_id: format!("declare-{child_id}"),
            child_id: ChildId::new(child_id).unwrap(),
            parent: ChildParent {
                session_id: session_id.into(),
                turn_id: None,
                parent_child_id: None,
                workflow_run_id: None,
                graph_node_id: None,
                parent_call_id: None,
            },
            origin: ChildOrigin::Spawn,
            request: ChildRequestEvidence::redacted("a".repeat(64)),
            policy_snapshot: ChildPolicySnapshot {
                contract_version: "effective-execution-policy/v1".into(),
                exact_digest: "b".repeat(64),
                posture: "standard".into(),
                approvals: "ask".into(),
                sandbox: "workspace-write".into(),
                source: "session-effective-policy".into(),
                managed_floor_active: true,
                dangerous_activation_id_digest: None,
            },
            provider: Some("test".into()),
            model: Some("test-model".into()),
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

    #[test]
    fn fresh_binding_accepts_only_a_canonical_childless_journal() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let (_manager, journal) = canonical_journal(dir.path(), "f190020");

        let token = authority.bind_fresh(journal, "f190020").unwrap();

        assert_eq!(token.session_id(), "f190020");
        assert!(
            authority
                .with_store(&token, |store| Ok(store.list()?.is_empty()))
                .unwrap()
        );
    }

    #[test]
    fn fresh_binding_rejects_a_journal_with_durable_child_history() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let clone = authority.clone();
        let (_manager_a, journal_a) = canonical_journal(&dir.path().join("a"), "f190021a");
        let token_a = authority.bind_fresh(journal_a, "f190021a").unwrap();
        let (_manager_b, journal_b) = canonical_journal(&dir.path().join("b"), "f190021b");
        DurableChildStore::new(journal_b.clone())
            .declare(child_record("f190021b", "existing-child"))
            .unwrap();

        assert!(matches!(
            clone.bind_fresh(journal_b, "f190021b"),
            Err(DurableSpawnerError::FreshSessionHasChildHistory(session_id))
                if session_id == "f190021b"
        ));
        assert!(matches!(
            authority.token(),
            Err(DurableSpawnerError::AuthorityUnbound)
        ));
        assert!(matches!(
            clone.with_store(&token_a, |_| Ok(())),
            Err(DurableSpawnerError::StaleAuthority { .. })
        ));
    }

    #[test]
    fn resumed_binding_reconciles_running_and_inflight_terminal_children() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let (_manager, journal) = canonical_journal(dir.path(), "f190022");
        let store = DurableChildStore::new(journal.clone());
        let running_id = ChildId::new("running-child").unwrap();
        store
            .declare(child_record("f190022", running_id.as_str()))
            .unwrap();
        store
            .transition(
                running_id.clone(),
                "enqueue-running",
                0,
                101,
                DurableChildTransition::Enqueue,
            )
            .unwrap();
        store
            .transition(
                running_id.clone(),
                "start-running",
                1,
                102,
                DurableChildTransition::Start,
            )
            .unwrap();

        let delivery_id = ChildId::new("delivery-child").unwrap();
        store
            .declare(child_record("f190022", delivery_id.as_str()))
            .unwrap();
        store
            .transition(
                delivery_id.clone(),
                "enqueue-delivery",
                0,
                101,
                DurableChildTransition::Enqueue,
            )
            .unwrap();
        store
            .transition(
                delivery_id.clone(),
                "start-delivery",
                1,
                102,
                DurableChildTransition::Start,
            )
            .unwrap();
        store
            .transition(
                delivery_id.clone(),
                "succeed-delivery",
                2,
                103,
                DurableChildTransition::Succeed {
                    result: DurableChildResult {
                        exact_digest: "c".repeat(64),
                        turns: 1,
                        input_tokens: 1,
                        output_tokens: 1,
                        artifact_digests: Vec::new(),
                    },
                },
            )
            .unwrap();
        store
            .transition(
                delivery_id.clone(),
                "start-result-delivery",
                3,
                104,
                DurableChildTransition::DeliveryStarted,
            )
            .unwrap();
        drop(store);

        let token = authority.bind(journal, "f190022").unwrap();
        let running = authority
            .with_store(&token, |store| Ok(store.inspect(&running_id)?))
            .unwrap()
            .unwrap();
        let delivery = authority
            .with_store(&token, |store| Ok(store.inspect(&delivery_id)?))
            .unwrap()
            .unwrap();

        assert_eq!(running.status, DurableChildStatus::RecoveryRequired);
        assert!(matches!(
            running.recovery,
            ChildRecoveryState::Required { .. }
        ));
        assert!(matches!(
            delivery.delivery_state,
            ChildDeliveryState::Unknown { .. }
        ));
    }

    #[test]
    fn authority_binding_is_visible_to_clones_and_switch_invalidates_old_token() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let clone = authority.clone();
        assert!(matches!(
            clone.token(),
            Err(DurableSpawnerError::AuthorityUnbound)
        ));

        let (_manager_a, journal_a) = canonical_journal(&dir.path().join("a"), "f19000a");
        let token_a = authority.bind(journal_a, "f19000a").unwrap();
        assert_eq!(clone.token().unwrap(), token_a);

        let (_manager_b, journal_b) = canonical_journal(&dir.path().join("b"), "f19000b");
        let token_b = clone.bind(journal_b, "f19000b").unwrap();
        assert_ne!(token_a.generation(), token_b.generation());
        assert!(matches!(
            authority.with_store(&token_a, |_| Ok(())),
            Err(DurableSpawnerError::StaleAuthority { .. })
        ));
        authority.with_store(&token_b, |_| Ok(())).unwrap();
    }

    #[test]
    fn failed_rebind_leaves_every_clone_unbound() {
        let dir = tempfile::tempdir().unwrap();
        let authority = DurableSessionAuthority::new();
        let clone = authority.clone();
        let (_manager_a, journal_a) = canonical_journal(&dir.path().join("a"), "f19000a");
        let token_a = authority.bind(journal_a, "f19000a").unwrap();

        let (_manager_b, journal_b) = canonical_journal(&dir.path().join("b"), "f19000b");
        assert!(matches!(
            clone.bind(journal_b, "wrong-session"),
            Err(DurableSpawnerError::SessionMismatch { .. })
        ));
        assert!(matches!(
            authority.token(),
            Err(DurableSpawnerError::AuthorityUnbound)
        ));
        assert!(matches!(
            clone.with_store(&token_a, |_| Ok(())),
            Err(DurableSpawnerError::StaleAuthority { .. })
        ));
    }
}
