//! Journal-coupled ownership for provider and execution budget authority.
//!
//! Runtime budget mutation is useful only when the same authority survives a
//! crash. This coordinator restores under current policy, reconciles transient
//! reservations conservatively, and appends the resulting authority before it
//! reports a mutation as committed.

use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use parking_lot::Mutex;
use thiserror::Error;
use wcore_budget::{
    BudgetCap, BudgetError, BudgetEventSink, BudgetReservation, BudgetTracker, ExecutionBudget,
    ExecutionBudgetSnapshot, ExecutionBudgetView, ProcessCleanupProof,
    RestoredReservationReconciliation,
};

use crate::session_journal::{
    ActiveTurnBudgetAuthority, BUDGET_AUTHORITY_SCHEMA_VERSION, BudgetAuthorityCursor,
    BudgetAuthorityState, BudgetWallClockAuthority, ExternalEffectState, JournalEnvelope,
    ProviderBudgetReservationAuthority, ReducedSessionState, SessionEvent, SessionJournal,
    state_payload_digest,
};

/// Thread-safe owner used by engine surfaces. Every mutation must lock the
/// coordinator and enter [`BudgetAuthorityCoordinator::transaction`].
pub type SharedBudgetAuthorityCoordinator = Arc<Mutex<BudgetAuthorityCoordinator>>;

/// Cloneable policy inputs retained across fresh-session journal creation.
#[derive(Clone)]
pub struct BudgetAuthoritySeed {
    pub provider_caps: BudgetCap,
    /// Whether locally committed per-session extensions survive restart.
    /// Managed policy sets this false so current organization ceilings clamp
    /// all prior interactive headroom.
    pub preserve_committed_session_extensions: bool,
    pub execution_policy: ExecutionBudget,
    pub wall_clock: BudgetWallClockAuthority,
    pub process_cleanup_proof: Option<ProcessCleanupProof>,
}

impl BudgetAuthoritySeed {
    pub fn config(
        &self,
        journal: Option<SessionJournal>,
        budget_session_id: impl Into<String>,
    ) -> BudgetAuthorityConfig {
        BudgetAuthorityConfig {
            journal,
            budget_session_id: budget_session_id.into(),
            provider_caps: self.provider_caps.clone(),
            preserve_committed_session_extensions: self.preserve_committed_session_extensions,
            execution_policy: self.execution_policy.clone(),
            wall_clock: self.wall_clock.clone(),
            process_cleanup_proof: self.process_cleanup_proof.clone(),
        }
    }

    pub fn detached(
        &self,
        budget_session_id: impl Into<String>,
    ) -> Result<SharedBudgetAuthorityCoordinator, BudgetAuthorityError> {
        BudgetAuthorityCoordinator::bind(self.config(None, budget_session_id))
            .map(BudgetAuthorityCoordinator::into_shared)
    }
}

/// Current policy and durable binding inputs for one budget authority.
pub struct BudgetAuthorityConfig {
    /// Optional durable session journal. Without one, transactions remain
    /// fail-closed in memory but do not claim crash durability.
    pub journal: Option<SessionJournal>,
    /// Stable identity. A restored journal must contain this exact value.
    pub budget_session_id: String,
    /// Provider caps effective in the new process. Restore intersects these
    /// with captured caps and durable extensions.
    pub provider_caps: BudgetCap,
    /// Preserve durable local grants on restart. Must be false whenever a
    /// managed policy controls the provider ceiling.
    pub preserve_committed_session_extensions: bool,
    /// Current session-root execution policy.
    pub execution_policy: ExecutionBudget,
    /// New-session wall-clock authority. On restore only the semantic variant
    /// is checked; the persisted absolute deadline remains authoritative.
    pub wall_clock: BudgetWallClockAuthority,
    /// Platform evidence allowing restored process counters to be cleared.
    pub process_cleanup_proof: Option<ProcessCleanupProof>,
}

/// A coordinator cannot resume after a journal commit failure: runtime state
/// may have changed while durable authority did not.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BudgetAuthorityError {
    #[error("budget authority is permanently faulted: {reason}")]
    Faulted { reason: String },
    #[error("invalid budget authority configuration: {0}")]
    InvalidConfiguration(String),
    #[error("invalid durable budget authority: {0}")]
    InvalidAuthority(String),
    #[error("budget snapshot operation failed: {0}")]
    Snapshot(String),
    #[error("budget journal operation failed: {0}")]
    Journal(String),
}

struct ActiveTurnRuntime {
    turn_id: String,
    execution: ExecutionBudgetView,
}

/// Mutable authority exposed only for the duration of a transaction.
pub struct BudgetAuthorityMutation<'a> {
    provider_tracker: &'a mut BudgetTracker,
    execution: &'a ExecutionBudgetView,
}

impl BudgetAuthorityMutation<'_> {
    pub fn provider_tracker(&mut self) -> &mut BudgetTracker {
        self.provider_tracker
    }

    /// Current active-turn budget, or the session root between turns.
    pub fn execution(&self) -> &ExecutionBudgetView {
        self.execution
    }
}

/// Owns the runtime budgets and their latest durable journal epoch.
pub struct BudgetAuthorityCoordinator {
    provider_tracker: BudgetTracker,
    execution_root: ExecutionBudgetView,
    active_turn: Option<ActiveTurnRuntime>,
    budget_session_id: String,
    authority_epoch: u64,
    captured_at_unix_millis: u64,
    wall_clock: BudgetWallClockAuthority,
    journal: Option<SessionJournal>,
    provider_reservations: BTreeMap<String, ProviderBudgetReservationAuthority>,
    restored_reservations: RestoredReservationReconciliation,
    event_sink: Option<Arc<dyn BudgetEventSink>>,
    fault: Option<String>,
}

enum RestoredTurnAction {
    None,
    Begin(String),
    Finish(String),
}

impl BudgetAuthorityCoordinator {
    /// Restore accepted journal authority or create and commit epoch one.
    ///
    /// Existing authority is always recommitted after cap intersection and
    /// restart reconciliation, even when no provider reservations existed.
    pub fn bind(config: BudgetAuthorityConfig) -> Result<Self, BudgetAuthorityError> {
        validate_config(&config)?;
        let now = unix_millis()?;
        let Some(journal) = config.journal.clone() else {
            return Ok(Self {
                provider_tracker: BudgetTracker::new(config.provider_caps),
                execution_root: config.execution_policy.start_root(),
                active_turn: None,
                budget_session_id: config.budget_session_id,
                authority_epoch: 0,
                captured_at_unix_millis: now,
                wall_clock: config.wall_clock,
                journal: None,
                provider_reservations: BTreeMap::new(),
                restored_reservations: empty_reconciliation(),
                event_sink: None,
                fault: None,
            });
        };

        let reduced = journal
            .state()
            .map_err(|error| BudgetAuthorityError::Journal(error.to_string()))?;
        if reduced.imported_baseline.is_none() {
            return Err(BudgetAuthorityError::InvalidAuthority(
                "journal has no canonical imported session baseline".to_owned(),
            ));
        }

        let authority = reduced.budget_authority.clone();
        let restored_turn = restored_turn_action(&reduced, authority.as_ref())?;
        let mut coordinator = if let Some(authority) = authority {
            restore(config, authority, &reduced, now)?
        } else {
            require_pristine_authority_free_session(&reduced)?;
            Self {
                provider_tracker: BudgetTracker::new(config.provider_caps),
                execution_root: config.execution_policy.start_root(),
                active_turn: None,
                budget_session_id: config.budget_session_id,
                authority_epoch: 0,
                captured_at_unix_millis: now,
                wall_clock: config.wall_clock,
                journal: Some(journal),
                provider_reservations: BTreeMap::new(),
                restored_reservations: empty_reconciliation(),
                event_sink: None,
                fault: None,
            }
        };

        match restored_turn {
            RestoredTurnAction::None => {}
            RestoredTurnAction::Begin(turn_id) => {
                coordinator.active_turn = Some(ActiveTurnRuntime {
                    turn_id,
                    execution: coordinator.execution_root.sub_budget(None),
                });
            }
            RestoredTurnAction::Finish(turn_id) => {
                coordinator.finish_active_turn_in_memory(&turn_id)?;
            }
        }

        coordinator.commit_current_authority()?;
        Ok(coordinator)
    }

    pub fn budget_session_id(&self) -> &str {
        &self.budget_session_id
    }

    pub fn authority_epoch(&self) -> u64 {
        self.authority_epoch
    }

    pub fn wall_clock(&self) -> &BudgetWallClockAuthority {
        &self.wall_clock
    }

    pub fn active_turn_id(&self) -> Option<&str> {
        self.active_turn.as_ref().map(|turn| turn.turn_id.as_str())
    }

    pub fn restored_reservation_reconciliation(&self) -> &RestoredReservationReconciliation {
        &self.restored_reservations
    }

    pub fn fault_reason(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    /// True only when this runtime is backed by a committed journal epoch.
    pub fn is_durably_bound(&self) -> bool {
        self.journal.is_some() && self.authority_epoch > 0
    }

    /// Replace an unused detached bootstrap authority without changing the
    /// shared Arc already captured by engine-adjacent consumers.
    pub fn bind_shared_pristine(
        shared: &SharedBudgetAuthorityCoordinator,
        config: BudgetAuthorityConfig,
    ) -> Result<(), BudgetAuthorityError> {
        let mut current = shared.lock();
        current.ensure_healthy()?;
        if current.journal.is_some()
            || current.authority_epoch != 0
            || current.active_turn.is_some()
            || !provider_tracker_is_pristine(&current.provider_tracker)
        {
            return Err(BudgetAuthorityError::InvalidAuthority(
                "detached budget authority is no longer pristine".to_owned(),
            ));
        }
        let mut replacement = Self::bind(config)?;
        if let Some(sink) = current.event_sink.clone() {
            replacement.install_event_sink(sink)?;
        }
        *current = replacement;
        Ok(())
    }

    /// Bind a detached bootstrap authority, or validate an idempotent bind to
    /// the exact same durable session. Switching this shared owner to a
    /// different session is deliberately unsupported: watcher, spawner, and
    /// child-engine clones all retain this Arc and must move atomically.
    pub fn bind_shared_session(
        shared: &SharedBudgetAuthorityCoordinator,
        config: BudgetAuthorityConfig,
    ) -> Result<(), BudgetAuthorityError> {
        {
            let current = shared.lock();
            current.ensure_healthy()?;
            if current.is_durably_bound() {
                return current.validate_idempotent_binding(&config);
            }
        }
        Self::bind_shared_pristine(shared, config)
    }

    /// Wrap this sole authority owner for shared engine access.
    pub fn into_shared(self) -> SharedBudgetAuthorityCoordinator {
        Arc::new(Mutex::new(self))
    }

    /// Clone the currently effective execution view for read-only consumers.
    /// Mutations must still be routed through [`Self::transaction`].
    pub fn current_execution_view(&self) -> Result<ExecutionBudgetView, BudgetAuthorityError> {
        self.ensure_healthy()?;
        Ok(self.current_execution().clone())
    }

    /// Install the provider-budget observability sink on the owned tracker.
    /// The sink is process-local and does not change durable authority.
    pub fn install_event_sink(
        &mut self,
        sink: Arc<dyn BudgetEventSink>,
    ) -> Result<(), BudgetAuthorityError> {
        self.ensure_healthy()?;
        self.provider_tracker.set_event_sink(Arc::clone(&sink));
        self.event_sink = Some(sink);
        Ok(())
    }

    /// Read runtime authority without exposing a mutable provider tracker.
    pub fn inspect<R>(
        &self,
        inspect: impl FnOnce(&BudgetTracker, &ExecutionBudgetView) -> R,
    ) -> Result<R, BudgetAuthorityError> {
        self.ensure_healthy()?;
        Ok(inspect(&self.provider_tracker, self.current_execution()))
    }

    /// Apply a runtime mutation and append the complete authority before
    /// returning its result. The closure's result may itself be fallible;
    /// authority is committed either way because denied admissions can mutate
    /// blocked-session latches.
    pub fn transaction<R>(
        &mut self,
        mutation: impl FnOnce(&mut BudgetAuthorityMutation<'_>) -> R,
    ) -> Result<R, BudgetAuthorityError> {
        self.ensure_healthy()?;
        let execution = self
            .active_turn
            .as_ref()
            .map(|turn| &turn.execution)
            .unwrap_or(&self.execution_root);
        let result = mutation(&mut BudgetAuthorityMutation {
            provider_tracker: &mut self.provider_tracker,
            execution,
        });
        self.commit_current_authority()?;
        Ok(result)
    }

    /// Atomically reserve one paid provider call and bind that reservation to
    /// the logical dispatch whose physical-attempt receipts will reconcile it
    /// after a restart.
    pub(crate) fn reserve_provider_dispatch(
        &mut self,
        dispatch_id: &str,
        session_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        usd: f64,
    ) -> Result<Result<BudgetReservation, BudgetError>, BudgetAuthorityError> {
        self.ensure_healthy()?;
        if dispatch_id.trim().is_empty() {
            return Err(BudgetAuthorityError::InvalidConfiguration(
                "provider dispatch id must not be empty".to_owned(),
            ));
        }
        if self.provider_reservations.contains_key(dispatch_id) {
            return Err(BudgetAuthorityError::InvalidAuthority(format!(
                "provider dispatch {dispatch_id} already owns a budget reservation"
            )));
        }
        let prior_attempt_ids = self.provider_attempt_ids(dispatch_id)?;
        let result =
            self.provider_tracker
                .reserve_turn(session_id, input_tokens, output_tokens, usd);
        if let Ok(reservation) = &result {
            self.provider_reservations.insert(
                dispatch_id.to_owned(),
                ProviderBudgetReservationAuthority {
                    reservation: *reservation,
                    prior_attempt_ids,
                },
            );
        }
        self.commit_current_authority()?;
        Ok(result)
    }

    /// Replace one dispatch-bound reservation with authoritative provider
    /// usage. The binding and tracker reservation disappear in the same
    /// durable authority epoch.
    pub(crate) fn settle_provider_dispatch(
        &mut self,
        dispatch_id: &str,
        reservation: BudgetReservation,
        input_tokens: u64,
        output_tokens: u64,
        usd: f64,
    ) -> Result<Result<(), BudgetError>, BudgetAuthorityError> {
        self.require_provider_reservation(dispatch_id, reservation)?;
        self.provider_reservations.remove(dispatch_id);
        let result =
            self.provider_tracker
                .settle_turn(reservation, input_tokens, output_tokens, usd);
        self.commit_current_authority()?;
        Ok(result)
    }

    /// Release a dispatch-bound admission only when the caller has proved the
    /// physical send never started.
    pub(crate) fn release_provider_dispatch(
        &mut self,
        dispatch_id: &str,
        reservation: BudgetReservation,
    ) -> Result<(), BudgetAuthorityError> {
        self.require_provider_reservation(dispatch_id, reservation)?;
        self.provider_reservations.remove(dispatch_id);
        if !self.provider_tracker.release(reservation) {
            return Err(BudgetAuthorityError::InvalidAuthority(format!(
                "provider dispatch {dispatch_id} reservation is missing from tracker authority"
            )));
        }
        self.commit_current_authority()?;
        Ok(())
    }

    fn require_provider_reservation(
        &self,
        dispatch_id: &str,
        reservation: BudgetReservation,
    ) -> Result<(), BudgetAuthorityError> {
        let binding = self.provider_reservations.get(dispatch_id).ok_or_else(|| {
            BudgetAuthorityError::InvalidAuthority(format!(
                "provider dispatch {dispatch_id} has no budget reservation"
            ))
        })?;
        if binding.reservation != reservation {
            return Err(BudgetAuthorityError::InvalidAuthority(format!(
                "provider dispatch {dispatch_id} does not own the supplied budget reservation"
            )));
        }
        Ok(())
    }

    fn provider_attempt_ids(&self, dispatch_id: &str) -> Result<Vec<String>, BudgetAuthorityError> {
        let Some(journal) = self.journal.as_ref() else {
            return Ok(Vec::new());
        };
        let reduced = journal
            .state()
            .map_err(|error| BudgetAuthorityError::Journal(error.to_string()))?;
        Ok(reduced
            .provider_attempts
            .iter()
            .filter(|(_, attempt)| attempt.dispatch_id.as_deref() == Some(dispatch_id))
            .map(|(attempt_id, _)| attempt_id.clone())
            .collect())
    }

    /// Start an active-turn child after the journal's `TurnStarted` event has
    /// committed. The returned handle shares counters with coordinator state;
    /// callers must route every mutation through `transaction`.
    pub fn begin_active_turn(
        &mut self,
        turn_id: impl Into<String>,
        override_: Option<ExecutionBudget>,
    ) -> Result<(), BudgetAuthorityError> {
        self.ensure_healthy()?;
        if self.active_turn.is_some() {
            return Err(BudgetAuthorityError::InvalidAuthority(
                "an active-turn budget already exists".to_owned(),
            ));
        }
        let turn_id = turn_id.into();
        if turn_id.trim().is_empty() {
            return Err(BudgetAuthorityError::InvalidConfiguration(
                "active turn id must not be empty".to_owned(),
            ));
        }
        self.active_turn = Some(ActiveTurnRuntime {
            turn_id,
            execution: self.execution_root.sub_budget(override_),
        });
        self.commit_current_authority()?;
        Ok(())
    }

    /// Finish an active turn after its terminal journal event commits. Root
    /// counters are promoted from the child chain before the child is removed.
    pub fn finish_active_turn(&mut self, turn_id: &str) -> Result<(), BudgetAuthorityError> {
        self.ensure_healthy()?;
        self.finish_active_turn_in_memory(turn_id)?;
        self.commit_current_authority()?;
        Ok(())
    }

    fn finish_active_turn_in_memory(&mut self, turn_id: &str) -> Result<(), BudgetAuthorityError> {
        let active = self.active_turn.as_ref().ok_or_else(|| {
            BudgetAuthorityError::InvalidAuthority("no active-turn budget exists".to_owned())
        })?;
        if active.turn_id != turn_id {
            return Err(BudgetAuthorityError::InvalidAuthority(format!(
                "active-turn budget belongs to {}, not {turn_id}",
                active.turn_id
            )));
        }
        let active_snapshot = match active.execution.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Err(self.latch(format!("capturing active-turn authority: {error}")));
            }
        };
        let root_snapshot = match root_snapshot(&active_snapshot) {
            Ok(snapshot) => snapshot,
            Err(error) => return Err(self.latch(format!("promoting active-turn root: {error}"))),
        };
        self.execution_root = match ExecutionBudgetView::from_snapshot(root_snapshot) {
            Ok(root) => root,
            Err(error) => {
                return Err(self.latch(format!("restoring promoted execution root: {error}")));
            }
        };
        self.active_turn = None;
        Ok(())
    }

    /// Append the current complete authority. Detached coordinators return
    /// `None` and make no durability claim.
    pub fn commit_current_authority(
        &mut self,
    ) -> Result<Option<JournalEnvelope>, BudgetAuthorityError> {
        self.ensure_healthy()?;
        let Some(journal) = self.journal.clone() else {
            return Ok(None);
        };
        let result = self.build_and_append(&journal);
        match result {
            Ok(envelope) => Ok(Some(envelope)),
            Err(error) => Err(self.latch(error)),
        }
    }

    fn build_and_append(&mut self, journal: &SessionJournal) -> Result<JournalEnvelope, String> {
        let reduced = journal.state().map_err(|error| error.to_string())?;
        let durable_epoch = reduced
            .budget_authority
            .as_ref()
            .map(|authority| authority.authority_epoch)
            .unwrap_or(0);
        if durable_epoch != self.authority_epoch {
            return Err(format!(
                "durable authority epoch {durable_epoch} does not match runtime epoch {}",
                self.authority_epoch
            ));
        }
        let next_epoch = self
            .authority_epoch
            .checked_add(1)
            .ok_or_else(|| "budget authority epoch is exhausted".to_owned())?;
        let now = unix_millis().map_err(|error| error.to_string())?;
        let captured_at = now.max(self.captured_at_unix_millis);
        let provider_tracker = self
            .provider_tracker
            .snapshot()
            .map_err(|error| format!("capturing provider authority: {error}"))?;
        let (execution_root, active_turn) = self.execution_snapshots()?;
        let conversation = serde_json::Value::Array(reduced.conversation);
        let conversation_digest =
            state_payload_digest(&conversation).map_err(|error| error.to_string())?;
        let authority = BudgetAuthorityState {
            schema_version: BUDGET_AUTHORITY_SCHEMA_VERSION,
            authority_epoch: next_epoch,
            prior_cursor: BudgetAuthorityCursor {
                journal_sequence: reduced.last_seq,
                journal_checksum: reduced.last_checksum,
            },
            budget_session_id: self.budget_session_id.clone(),
            provider_tracker,
            provider_reservations: self.provider_reservations.clone(),
            execution_root,
            active_turn,
            captured_at_unix_millis: captured_at,
            wall_clock: self.wall_clock.clone(),
            conversation_digest,
        };
        let envelope = journal
            .append(SessionEvent::BudgetAuthorityCommitted { authority })
            .map_err(|error| error.to_string())?;
        self.authority_epoch = next_epoch;
        self.captured_at_unix_millis = captured_at;
        Ok(envelope)
    }

    fn execution_snapshots(
        &self,
    ) -> Result<(ExecutionBudgetSnapshot, Option<ActiveTurnBudgetAuthority>), String> {
        if let Some(active) = &self.active_turn {
            let execution = active
                .execution
                .snapshot()
                .map_err(|error| format!("capturing active-turn execution authority: {error}"))?;
            let execution_root = root_snapshot(&execution)
                .map_err(|error| format!("capturing active-turn root authority: {error}"))?;
            return Ok((
                execution_root,
                Some(ActiveTurnBudgetAuthority {
                    turn_id: active.turn_id.clone(),
                    execution,
                }),
            ));
        }
        self.execution_root
            .snapshot()
            .map(|root| (root, None))
            .map_err(|error| format!("capturing execution root authority: {error}"))
    }

    fn current_execution(&self) -> &ExecutionBudgetView {
        self.active_turn
            .as_ref()
            .map(|turn| &turn.execution)
            .unwrap_or(&self.execution_root)
    }

    fn validate_idempotent_binding(
        &self,
        config: &BudgetAuthorityConfig,
    ) -> Result<(), BudgetAuthorityError> {
        if config.budget_session_id != self.budget_session_id {
            return Err(BudgetAuthorityError::InvalidAuthority(format!(
                "cross-session budget rebind is unsupported: current={}, requested={}",
                self.budget_session_id, config.budget_session_id
            )));
        }
        let journal = config.journal.as_ref().ok_or_else(|| {
            BudgetAuthorityError::InvalidAuthority(
                "a durable budget authority cannot rebind to a detached session".to_owned(),
            )
        })?;
        let reduced = journal
            .state()
            .map_err(|error| BudgetAuthorityError::Journal(error.to_string()))?;
        let durable = reduced.budget_authority.as_ref().ok_or_else(|| {
            BudgetAuthorityError::InvalidAuthority(
                "same-session rebind journal has no budget authority".to_owned(),
            )
        })?;
        let provider_tracker = self
            .provider_tracker
            .snapshot()
            .map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))?;
        let (execution_root, active_turn) = self
            .execution_snapshots()
            .map_err(BudgetAuthorityError::Snapshot)?;
        let execution_matches =
            execution_snapshot_matches(&durable.execution_root, &execution_root)?;
        let active_turn_matches = match (&durable.active_turn, &active_turn) {
            (None, None) => true,
            (Some(durable), Some(live)) if durable.turn_id == live.turn_id => {
                execution_snapshot_matches(&durable.execution, &live.execution)?
            }
            _ => false,
        };
        if durable.authority_epoch != self.authority_epoch
            || durable.budget_session_id != self.budget_session_id
            || durable.provider_tracker != provider_tracker
            || !execution_matches
            || !active_turn_matches
            || durable.wall_clock != self.wall_clock
            || durable.captured_at_unix_millis != self.captured_at_unix_millis
        {
            return Err(BudgetAuthorityError::InvalidAuthority(
                "same-session rebind does not match the live durable authority".to_owned(),
            ));
        }
        Ok(())
    }

    fn ensure_healthy(&self) -> Result<(), BudgetAuthorityError> {
        match &self.fault {
            Some(reason) => Err(BudgetAuthorityError::Faulted {
                reason: reason.clone(),
            }),
            None => Ok(()),
        }
    }

    fn latch(&mut self, reason: impl Into<String>) -> BudgetAuthorityError {
        let reason = reason.into();
        if self.fault.is_none() {
            self.fault = Some(reason.clone());
        }
        BudgetAuthorityError::Journal(reason)
    }
}

/// Monotonic elapsed time advances between two snapshots of the same live
/// authority. Strip only that clock-derived field; every cap and counter must
/// still match the durable event exactly.
fn execution_snapshot_matches(
    durable: &ExecutionBudgetSnapshot,
    live: &ExecutionBudgetSnapshot,
) -> Result<bool, BudgetAuthorityError> {
    let mut durable = serde_json::to_value(durable)
        .map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))?;
    let mut live = serde_json::to_value(live)
        .map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))?;
    strip_snapshot_elapsed(&mut durable)?;
    strip_snapshot_elapsed(&mut live)?;
    Ok(durable == live)
}

fn strip_snapshot_elapsed(snapshot: &mut serde_json::Value) -> Result<(), BudgetAuthorityError> {
    let states = snapshot
        .get_mut("states")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| {
            BudgetAuthorityError::Snapshot(
                "execution snapshot JSON is missing its states array".to_owned(),
            )
        })?;
    for state in states {
        let state = state.as_object_mut().ok_or_else(|| {
            BudgetAuthorityError::Snapshot(
                "execution snapshot state is not a JSON object".to_owned(),
            )
        })?;
        if state.remove("elapsed").is_none() {
            return Err(BudgetAuthorityError::Snapshot(
                "execution snapshot state is missing elapsed authority".to_owned(),
            ));
        }
    }
    Ok(())
}

fn require_pristine_authority_free_session(
    reduced: &ReducedSessionState,
) -> Result<(), BudgetAuthorityError> {
    let baseline = reduced.imported_baseline.as_ref().ok_or_else(|| {
        BudgetAuthorityError::InvalidAuthority(
            "journal has no canonical imported session baseline".to_owned(),
        )
    })?;
    let imported_messages_are_empty = baseline
        .session
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .is_some_and(Vec::is_empty);
    let imported_usage_is_zero = baseline.session.get("total_usage").is_none_or(|usage| {
        usage.as_object().is_some_and(|fields| {
            fields
                .values()
                .all(|value| value.as_u64().is_some_and(|count| count == 0))
        })
    });
    let pristine = reduced.last_seq == Some(0)
        && baseline.imported_message_count == 0
        && reduced.conversation.is_empty()
        && imported_messages_are_empty
        && imported_usage_is_zero;
    if pristine {
        return Ok(());
    }
    Err(BudgetAuthorityError::InvalidAuthority(
        "authority-free bind is allowed only for a pristine imported session".to_owned(),
    ))
}

fn provider_tracker_is_pristine(tracker: &BudgetTracker) -> bool {
    tracker.is_pristine()
}

fn restored_turn_action(
    reduced: &ReducedSessionState,
    authority: Option<&BudgetAuthorityState>,
) -> Result<RestoredTurnAction, BudgetAuthorityError> {
    let Some(authority) = authority else {
        return Ok(RestoredTurnAction::None);
    };
    let open_turn = reduced
        .turns
        .iter()
        .find(|(_, turn)| turn.completion.is_none())
        .map(|(turn_id, _)| turn_id.as_str());
    let Some(active) = authority.active_turn.as_ref() else {
        if let Some(turn_id) = open_turn {
            return Ok(RestoredTurnAction::Begin(turn_id.to_owned()));
        }
        return Ok(RestoredTurnAction::None);
    };
    let turn = reduced.turns.get(&active.turn_id).ok_or_else(|| {
        BudgetAuthorityError::InvalidAuthority(format!(
            "budget authority references missing turn {}",
            active.turn_id
        ))
    })?;
    if let Some(open_turn_id) = open_turn
        && open_turn_id != active.turn_id
    {
        return Err(BudgetAuthorityError::InvalidAuthority(format!(
            "budget authority references turn {}, but durable active turn is {open_turn_id}",
            active.turn_id
        )));
    }
    Ok(match turn.completion {
        Some(_) => RestoredTurnAction::Finish(active.turn_id.clone()),
        None => RestoredTurnAction::None,
    })
}

fn reconcile_dispatch_bound_reservations(
    provider_tracker: &mut BudgetTracker,
    bindings: &BTreeMap<String, ProviderBudgetReservationAuthority>,
    reduced: &ReducedSessionState,
) -> Result<RestoredReservationReconciliation, BudgetAuthorityError> {
    let mut reconciled = empty_reconciliation();
    let mut seen_reservations = HashSet::new();

    for (dispatch_id, binding) in bindings {
        if !seen_reservations.insert(binding.reservation) {
            return Err(BudgetAuthorityError::InvalidAuthority(
                "one provider reservation is bound to multiple dispatches".to_owned(),
            ));
        }
        if !provider_tracker.has_reservation(binding.reservation) {
            return Err(BudgetAuthorityError::InvalidAuthority(format!(
                "provider dispatch {dispatch_id} references a missing budget reservation"
            )));
        }

        let prior_attempt_ids = binding
            .prior_attempt_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        for attempt_id in &binding.prior_attempt_ids {
            let attempt = reduced.provider_attempts.get(attempt_id).ok_or_else(|| {
                BudgetAuthorityError::InvalidAuthority(format!(
                    "provider dispatch {dispatch_id} references missing prior attempt {attempt_id}"
                ))
            })?;
            if attempt.dispatch_id.as_deref() != Some(dispatch_id.as_str()) {
                return Err(BudgetAuthorityError::InvalidAuthority(format!(
                    "provider attempt {attempt_id} belongs to another dispatch"
                )));
            }
        }

        let physical_send_started =
            reduced
                .provider_attempts
                .iter()
                .any(|(attempt_id, attempt)| {
                    attempt.dispatch_id.as_deref() == Some(dispatch_id.as_str())
                        && !prior_attempt_ids.contains(attempt_id.as_str())
                        && matches!(
                            &attempt.effect,
                            ExternalEffectState::Unknown | ExternalEffectState::Completed { .. }
                        )
                });
        if physical_send_started {
            reconciled.reservations_settled = reconciled.reservations_settled.saturating_add(1);
            let (input_tokens, output_tokens, cost_usd) = provider_tracker
                .reservation_admitted_maximum(binding.reservation)
                .ok_or_else(|| {
                    BudgetAuthorityError::InvalidAuthority(format!(
                        "provider dispatch {dispatch_id} reservation disappeared before reconciliation"
                    ))
                })?;
            reconciled.input_tokens_charged =
                reconciled.input_tokens_charged.saturating_add(input_tokens);
            reconciled.output_tokens_charged = reconciled
                .output_tokens_charged
                .saturating_add(output_tokens);
            reconciled.cost_usd_charged += cost_usd;
            if let Err(error) =
                provider_tracker.settle_reservation_conservatively(binding.reservation)
            {
                reconciled.cap_errors.push(error);
            }
        } else if !provider_tracker.release(binding.reservation) {
            return Err(BudgetAuthorityError::InvalidAuthority(format!(
                "provider dispatch {dispatch_id} reservation disappeared during reconciliation"
            )));
        }
    }

    Ok(reconciled)
}

fn restore(
    config: BudgetAuthorityConfig,
    authority: BudgetAuthorityState,
    reduced: &ReducedSessionState,
    now: u64,
) -> Result<BudgetAuthorityCoordinator, BudgetAuthorityError> {
    if authority.budget_session_id != config.budget_session_id {
        return Err(BudgetAuthorityError::InvalidAuthority(format!(
            "budget session identity mismatch: durable={}, requested={}",
            authority.budget_session_id, config.budget_session_id
        )));
    }
    if !same_wall_clock_semantics(&authority.wall_clock, &config.wall_clock) {
        return Err(BudgetAuthorityError::InvalidAuthority(
            "wall-clock semantics differ from durable authority".to_owned(),
        ));
    }
    let elapsed_adjustment = match &authority.wall_clock {
        BudgetWallClockAuthority::ActiveRuntime => Duration::ZERO,
        BudgetWallClockAuthority::AbsoluteDeadline { .. } => {
            Duration::from_millis(now.saturating_sub(authority.captured_at_unix_millis))
        }
    };
    let mut provider_tracker = if config.preserve_committed_session_extensions {
        BudgetTracker::from_snapshot_with_current_caps_preserving_extensions(
            authority.provider_tracker,
            config.provider_caps,
        )
    } else {
        BudgetTracker::from_snapshot_with_current_caps(
            authority.provider_tracker,
            config.provider_caps,
        )
    }
    .map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))?;
    let mut restored_reservations = reconcile_dispatch_bound_reservations(
        &mut provider_tracker,
        &authority.provider_reservations,
        reduced,
    )?;
    let unbound = provider_tracker.reconcile_restored_reservations_conservatively();
    restored_reservations.reservations_settled = restored_reservations
        .reservations_settled
        .saturating_add(unbound.reservations_settled);
    restored_reservations.input_tokens_charged = restored_reservations
        .input_tokens_charged
        .saturating_add(unbound.input_tokens_charged);
    restored_reservations.output_tokens_charged = restored_reservations
        .output_tokens_charged
        .saturating_add(unbound.output_tokens_charged);
    restored_reservations.cost_usd_charged += unbound.cost_usd_charged;
    restored_reservations.cap_errors.extend(unbound.cap_errors);

    let execution_root_snapshot = authority.execution_root;
    let active_turn = authority
        .active_turn
        .map(|active| {
            let execution = replace_root_snapshot(active.execution, &execution_root_snapshot)?;
            let execution = ExecutionBudgetView::from_snapshot_for_restart(
                execution,
                config.execution_policy.clone(),
                elapsed_adjustment,
                config.process_cleanup_proof.as_ref(),
            )
            .map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))?;
            Ok::<_, BudgetAuthorityError>(ActiveTurnRuntime {
                turn_id: active.turn_id,
                execution,
            })
        })
        .transpose()?;
    let execution_root = ExecutionBudgetView::from_snapshot_for_restart(
        execution_root_snapshot,
        config.execution_policy,
        elapsed_adjustment,
        config.process_cleanup_proof.as_ref(),
    )
    .map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))?;

    let execution = active_turn
        .as_ref()
        .map(|turn| &turn.execution)
        .unwrap_or(&execution_root);
    execution.record_tokens(
        restored_reservations.input_tokens_charged,
        restored_reservations.output_tokens_charged,
    );
    execution.record_cost(restored_reservations.cost_usd_charged);

    Ok(BudgetAuthorityCoordinator {
        provider_tracker,
        execution_root,
        active_turn,
        budget_session_id: config.budget_session_id,
        authority_epoch: authority.authority_epoch,
        captured_at_unix_millis: authority.captured_at_unix_millis.max(now),
        wall_clock: authority.wall_clock,
        journal: config.journal,
        provider_reservations: BTreeMap::new(),
        restored_reservations,
        event_sink: None,
        fault: None,
    })
}

fn root_snapshot(
    snapshot: &ExecutionBudgetSnapshot,
) -> Result<ExecutionBudgetSnapshot, BudgetAuthorityError> {
    let mut value = serde_json::to_value(snapshot)
        .map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))?;
    let states = value
        .get_mut("states")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| {
            BudgetAuthorityError::InvalidAuthority(
                "execution snapshot has no state array".to_owned(),
            )
        })?;
    let root = states.first().cloned().ok_or_else(|| {
        BudgetAuthorityError::InvalidAuthority("execution snapshot has no root state".to_owned())
    })?;
    *states = vec![root];
    serde_json::from_value(value).map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))
}

fn replace_root_snapshot(
    active: ExecutionBudgetSnapshot,
    root: &ExecutionBudgetSnapshot,
) -> Result<ExecutionBudgetSnapshot, BudgetAuthorityError> {
    let root_value = serde_json::to_value(root)
        .map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))?;
    let root_state = root_value
        .get("states")
        .and_then(serde_json::Value::as_array)
        .and_then(|states| states.first())
        .cloned()
        .ok_or_else(|| {
            BudgetAuthorityError::InvalidAuthority(
                "execution root snapshot has no root state".to_owned(),
            )
        })?;
    let mut active_value = serde_json::to_value(&active)
        .map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))?;
    let active_states = active_value
        .get_mut("states")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| {
            BudgetAuthorityError::InvalidAuthority(
                "active-turn snapshot has no state array".to_owned(),
            )
        })?;
    let active_root = active_states.first_mut().ok_or_else(|| {
        BudgetAuthorityError::InvalidAuthority("active-turn snapshot has no root state".to_owned())
    })?;
    *active_root = root_state;
    serde_json::from_value(active_value)
        .map_err(|error| BudgetAuthorityError::Snapshot(error.to_string()))
}

fn validate_config(config: &BudgetAuthorityConfig) -> Result<(), BudgetAuthorityError> {
    if config.budget_session_id.trim().is_empty() {
        return Err(BudgetAuthorityError::InvalidConfiguration(
            "budget session id must not be empty".to_owned(),
        ));
    }
    if matches!(
        config.wall_clock,
        BudgetWallClockAuthority::AbsoluteDeadline {
            deadline_unix_millis: 0
        }
    ) {
        return Err(BudgetAuthorityError::InvalidConfiguration(
            "absolute deadline must not be zero".to_owned(),
        ));
    }
    Ok(())
}

fn same_wall_clock_semantics(
    left: &BudgetWallClockAuthority,
    right: &BudgetWallClockAuthority,
) -> bool {
    matches!(
        (left, right),
        (
            BudgetWallClockAuthority::ActiveRuntime,
            BudgetWallClockAuthority::ActiveRuntime
        ) | (
            BudgetWallClockAuthority::AbsoluteDeadline { .. },
            BudgetWallClockAuthority::AbsoluteDeadline { .. }
        )
    )
}

fn unix_millis() -> Result<u64, BudgetAuthorityError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| BudgetAuthorityError::InvalidConfiguration(error.to_string()))?
        .as_millis();
    u64::try_from(millis).map_err(|_| {
        BudgetAuthorityError::InvalidConfiguration("wall clock exceeds u64 millis".to_owned())
    })
}

fn empty_reconciliation() -> RestoredReservationReconciliation {
    RestoredReservationReconciliation {
        reservations_settled: 0,
        input_tokens_charged: 0,
        output_tokens_charged: 0,
        cost_usd_charged: 0.0,
        cap_errors: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::provider_recovery::provider_response_digest;
    use crate::session_journal::{CompletionOutcome, ProviderAttemptPurpose, ProviderStreamEvent};

    fn baseline(journal: &SessionJournal) {
        let session = json!({
            "id": "session",
            "schema_version": 1,
            "messages": [],
        });
        journal
            .append(SessionEvent::SessionImported {
                source_schema_version: 1,
                session_digest: state_payload_digest(&session).unwrap(),
                session,
            })
            .unwrap();
    }

    fn config(journal: Option<SessionJournal>, token_cap: u64) -> BudgetAuthorityConfig {
        BudgetAuthorityConfig {
            journal,
            budget_session_id: "stable-budget-session".to_owned(),
            provider_caps: BudgetCap::builder()
                .per_session_tokens(token_cap)
                .per_session_usd(10.0)
                .build(),
            preserve_committed_session_extensions: true,
            execution_policy: ExecutionBudget {
                max_tokens_in: Some(token_cap),
                max_wall_time: Some(Duration::from_secs(60)),
                ..ExecutionBudget::default()
            },
            wall_clock: BudgetWallClockAuthority::ActiveRuntime,
            process_cleanup_proof: None,
        }
    }

    fn start_turn(journal: &SessionJournal, coordinator: &mut BudgetAuthorityCoordinator) {
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".to_owned(),
                user_message: "hello".to_owned(),
            })
            .unwrap();
        coordinator.begin_active_turn("turn", None).unwrap();
    }

    fn append_successful_dispatch(journal: &SessionJournal, dispatch_id: &str, attempt_id: &str) {
        let events = vec![ProviderStreamEvent::Done {
            stop_reason: json!("end_turn"),
            finish_reason: json!("stop"),
            usage: json!({
                "input_tokens": 3,
                "output_tokens": 2,
                "cache_creation_tokens": 0,
                "cache_read_tokens": 0
            }),
        }];
        let stream_id = format!("provider-stream:{attempt_id}");
        journal
            .append(SessionEvent::ProviderAttemptPreparedV2 {
                attempt_id: attempt_id.to_owned(),
                dispatch_id: dispatch_id.to_owned(),
                turn_id: "turn".to_owned(),
                purpose: ProviderAttemptPurpose::Conversation,
                provider: "fixture".to_owned(),
                model: "fixture-model".to_owned(),
                request_digest: "request-digest".to_owned(),
            })
            .unwrap();
        journal
            .append(SessionEvent::ProviderAttemptStarted {
                attempt_id: attempt_id.to_owned(),
            })
            .unwrap();
        journal
            .append(SessionEvent::StreamStarted {
                stream_id: stream_id.clone(),
                attempt_id: attempt_id.to_owned(),
            })
            .unwrap();
        journal
            .append(SessionEvent::StreamBatchCommitted {
                stream_id: stream_id.clone(),
                ordinal: 0,
                events: events.clone(),
            })
            .unwrap();
        journal
            .append(SessionEvent::StreamFinished { stream_id })
            .unwrap();
        journal
            .append(SessionEvent::ProviderAttemptFinishedV2 {
                attempt_id: attempt_id.to_owned(),
                dispatch_id: dispatch_id.to_owned(),
                outcome: CompletionOutcome::Succeeded,
                response_digest: Some(provider_response_digest(&events).unwrap()),
            })
            .unwrap();
    }

    #[test]
    fn restart_releases_dispatch_reservation_when_no_send_started() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut first =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        start_turn(&journal, &mut first);
        first
            .reserve_provider_dispatch("dispatch", "stable-budget-session", 60, 20, 1.0)
            .unwrap()
            .unwrap();
        assert_eq!(
            first
                .inspect(|tracker, _| tracker.reserved_totals("stable-budget-session"))
                .unwrap(),
            (80, 1.0)
        );
        drop(first);

        let restored = BudgetAuthorityCoordinator::bind(config(Some(journal), 100)).unwrap();
        assert_eq!(
            restored
                .inspect(|tracker, _| tracker.reserved_totals("stable-budget-session"))
                .unwrap(),
            (0, 0.0)
        );
        assert_eq!(
            restored
                .inspect(|tracker, _| tracker.session_totals("stable-budget-session"))
                .unwrap(),
            (0, 0.0)
        );
        assert_eq!(
            restored
                .restored_reservation_reconciliation()
                .reservations_settled,
            0
        );
        assert_eq!(
            restored
                .current_execution_view()
                .unwrap()
                .observed_for("max_tokens_in"),
            "0"
        );
    }

    #[test]
    fn restart_settles_successful_dispatch_once_before_runtime_settlement() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut first =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        start_turn(&journal, &mut first);
        first
            .reserve_provider_dispatch("dispatch", "stable-budget-session", 60, 20, 1.0)
            .unwrap()
            .unwrap();
        append_successful_dispatch(&journal, "dispatch", "attempt");
        drop(first);

        let restored =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        assert_eq!(
            restored
                .inspect(|tracker, _| tracker.reserved_totals("stable-budget-session"))
                .unwrap(),
            (0, 0.0)
        );
        assert_eq!(
            restored
                .inspect(|tracker, _| tracker.session_totals("stable-budget-session"))
                .unwrap(),
            (80, 1.0)
        );
        assert_eq!(
            restored
                .restored_reservation_reconciliation()
                .reservations_settled,
            1
        );
        assert_eq!(
            restored
                .current_execution_view()
                .unwrap()
                .observed_for("max_tokens_in"),
            "60"
        );
        assert_eq!(
            restored
                .current_execution_view()
                .unwrap()
                .observed_for("max_tokens_out"),
            "20"
        );
        assert_eq!(
            restored
                .current_execution_view()
                .unwrap()
                .observed_for("max_cost_usd"),
            "$1.0000"
        );
        drop(restored);

        let reopened = BudgetAuthorityCoordinator::bind(config(Some(journal), 100)).unwrap();
        assert_eq!(
            reopened
                .inspect(|tracker, _| tracker.session_totals("stable-budget-session"))
                .unwrap(),
            (80, 1.0)
        );
        assert_eq!(
            reopened
                .restored_reservation_reconciliation()
                .reservations_settled,
            0
        );
        assert_eq!(
            reopened
                .current_execution_view()
                .unwrap()
                .observed_for("max_tokens_in"),
            "60"
        );
        assert_eq!(
            reopened
                .current_execution_view()
                .unwrap()
                .observed_for("max_tokens_out"),
            "20"
        );
        assert_eq!(
            reopened
                .current_execution_view()
                .unwrap()
                .observed_for("max_cost_usd"),
            "$1.0000"
        );
    }

    #[test]
    fn fallback_reservation_ignores_prior_attempts_under_same_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut first =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        start_turn(&journal, &mut first);
        let reservation = first
            .reserve_provider_dispatch("dispatch", "stable-budget-session", 10, 10, 1.0)
            .unwrap()
            .unwrap();
        append_successful_dispatch(&journal, "dispatch", "attempt-one");
        first
            .settle_provider_dispatch("dispatch", reservation, 3, 2, 0.1)
            .unwrap()
            .unwrap();
        first
            .reserve_provider_dispatch("dispatch", "stable-budget-session", 30, 20, 1.0)
            .unwrap()
            .unwrap();
        drop(first);

        let restored = BudgetAuthorityCoordinator::bind(config(Some(journal), 100)).unwrap();
        assert_eq!(
            restored
                .inspect(|tracker, _| tracker.reserved_totals("stable-budget-session"))
                .unwrap(),
            (0, 0.0)
        );
        assert_eq!(
            restored
                .inspect(|tracker, _| tracker.session_totals("stable-budget-session"))
                .unwrap(),
            (5, 0.1)
        );
        assert_eq!(
            restored
                .restored_reservation_reconciliation()
                .reservations_settled,
            0
        );
    }

    #[test]
    fn fallback_can_reuse_dispatch_after_proved_no_send_release() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut coordinator = BudgetAuthorityCoordinator::bind(config(Some(journal), 100)).unwrap();
        let primary = coordinator
            .reserve_provider_dispatch("dispatch", "stable-budget-session", 10, 10, 1.0)
            .unwrap()
            .unwrap();

        coordinator
            .release_provider_dispatch("dispatch", primary)
            .unwrap();
        let fallback = coordinator
            .reserve_provider_dispatch("dispatch", "stable-budget-session", 20, 10, 1.0)
            .unwrap()
            .expect("releasing the primary must remove its dispatch binding");

        assert_ne!(primary, fallback);
        assert_eq!(
            coordinator
                .inspect(|tracker, _| tracker.reserved_totals("stable-budget-session"))
                .unwrap(),
            (30, 1.0)
        );
    }

    #[test]
    fn create_and_transaction_commit_complete_authority() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut coordinator =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();

        assert_eq!(coordinator.authority_epoch(), 1);
        coordinator
            .transaction(|authority| {
                authority
                    .provider_tracker()
                    .reserve("stable-budget-session", 60, 1.0)
            })
            .unwrap()
            .unwrap();

        assert_eq!(coordinator.authority_epoch(), 2);
        assert_eq!(
            coordinator
                .inspect(|tracker, _| tracker.reserved_totals("stable-budget-session"))
                .unwrap(),
            (60, 1.0)
        );
        let durable = journal.state().unwrap().budget_authority.unwrap();
        assert_eq!(durable.authority_epoch, 2);
        let restored = BudgetTracker::from_snapshot(durable.provider_tracker).unwrap();
        assert_eq!(restored.reserved_totals("stable-budget-session"), (60, 1.0));
    }

    #[test]
    fn budget_grant_request_survives_durable_reopen_without_double_extension() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut coordinator =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();

        let blocked = coordinator
            .transaction(|authority| {
                authority
                    .provider_tracker()
                    .reserve("stable-budget-session", 101, 0.0)
            })
            .unwrap();
        assert!(blocked.is_err());
        assert_eq!(
            coordinator
                .transaction(|authority| {
                    authority.provider_tracker().extend_session_idempotent(
                        "stable-budget-session",
                        "grant-001",
                        50,
                        0.0,
                    )
                })
                .unwrap()
                .unwrap(),
            wcore_budget::BudgetExtensionOutcome::Applied
        );
        assert_eq!(
            coordinator
                .inspect(|tracker, _| { tracker.effective_session_limits("stable-budget-session") })
                .unwrap()
                .0,
            Some(150)
        );
        drop(coordinator);

        let mut reopened =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        assert_eq!(
            reopened
                .transaction(|authority| {
                    authority.provider_tracker().extend_session_idempotent(
                        "stable-budget-session",
                        "grant-001",
                        50,
                        0.0,
                    )
                })
                .unwrap()
                .unwrap(),
            wcore_budget::BudgetExtensionOutcome::AlreadyApplied
        );
        assert_eq!(
            reopened
                .inspect(|tracker, _| { tracker.effective_session_limits("stable-budget-session") })
                .unwrap()
                .0,
            Some(150)
        );
        assert_eq!(
            reopened
                .transaction(|authority| {
                    authority.provider_tracker().extend_session_idempotent(
                        "stable-budget-session",
                        "grant-001",
                        51,
                        0.0,
                    )
                })
                .unwrap(),
            Err(wcore_budget::BudgetExtensionError::RequestIdConflict)
        );

        let durable = journal.state().unwrap().budget_authority.unwrap();
        let restored = BudgetTracker::from_snapshot(durable.provider_tracker).unwrap();
        assert_eq!(
            restored.effective_session_limits("stable-budget-session").0,
            Some(150)
        );
    }

    #[test]
    fn managed_restore_clamps_prior_interactive_budget_extension() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut coordinator =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        assert!(
            coordinator
                .transaction(|authority| {
                    authority
                        .provider_tracker()
                        .reserve("stable-budget-session", 101, 0.0)
                })
                .unwrap()
                .is_err()
        );
        coordinator
            .transaction(|authority| {
                authority.provider_tracker().extend_session_idempotent(
                    "stable-budget-session",
                    "grant-before-managed",
                    50,
                    0.0,
                )
            })
            .unwrap()
            .unwrap();
        drop(coordinator);

        let mut managed = config(Some(journal), 100);
        managed.preserve_committed_session_extensions = false;
        let restored = BudgetAuthorityCoordinator::bind(managed).unwrap();

        assert_eq!(
            restored
                .inspect(|tracker, _| tracker.effective_session_limits("stable-budget-session"))
                .unwrap()
                .0,
            Some(100)
        );
    }

    #[test]
    fn restore_intersects_caps_and_conservatively_settles_reservations() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut first =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        first
            .transaction(|authority| {
                authority
                    .provider_tracker()
                    .reserve("stable-budget-session", 80, 1.0)
            })
            .unwrap()
            .unwrap();
        drop(first);

        let restored = BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 50)).unwrap();

        assert_eq!(restored.authority_epoch(), 3);
        assert_eq!(
            restored
                .restored_reservation_reconciliation()
                .reservations_settled,
            1
        );
        assert_eq!(
            restored
                .restored_reservation_reconciliation()
                .cap_errors
                .len(),
            1
        );
        assert_eq!(
            restored
                .inspect(|tracker, _| tracker.session_totals("stable-budget-session"))
                .unwrap(),
            (80, 1.0)
        );
        assert_eq!(
            restored
                .current_execution_view()
                .unwrap()
                .observed_for("max_tokens_in"),
            "80"
        );
        assert!(restored.current_execution_view().unwrap().is_exceeded());
        drop(restored);

        let reopened = BudgetAuthorityCoordinator::bind(config(Some(journal), 50)).unwrap();
        assert_eq!(
            reopened
                .restored_reservation_reconciliation()
                .reservations_settled,
            0
        );
        assert_eq!(
            reopened
                .current_execution_view()
                .unwrap()
                .observed_for("max_tokens_in"),
            "80"
        );
        assert!(reopened.current_execution_view().unwrap().is_exceeded());
    }

    #[test]
    fn authority_free_bind_rejects_non_pristine_imported_session() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".to_owned(),
                user_message: "hello".to_owned(),
            })
            .unwrap();

        let error = match BudgetAuthorityCoordinator::bind(config(Some(journal), 100)) {
            Ok(_) => panic!("non-pristine authority-free bind unexpectedly succeeded"),
            Err(error) => error,
        };

        assert!(matches!(error, BudgetAuthorityError::InvalidAuthority(_)));
        assert!(
            error
                .to_string()
                .contains("authority-free bind is allowed only for a pristine imported session")
        );
    }

    #[test]
    fn active_turn_rollups_survive_restore_and_finish() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut coordinator =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".to_owned(),
                user_message: "hello".to_owned(),
            })
            .unwrap();
        coordinator.begin_active_turn("turn", None).unwrap();
        coordinator
            .transaction(|authority| authority.execution().record_tokens(20, 0))
            .unwrap();
        drop(coordinator);

        let mut restored =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        assert_eq!(restored.active_turn_id(), Some("turn"));
        assert_eq!(
            restored
                .inspect(|_, execution| execution.observed_for("max_tokens_in"))
                .unwrap(),
            "20"
        );
        journal
            .append(SessionEvent::TurnCommitted {
                turn_id: "turn".to_owned(),
                assistant_message: "done".to_owned(),
            })
            .unwrap();
        restored.finish_active_turn("turn").unwrap();
        assert_eq!(restored.active_turn_id(), None);
        assert_eq!(
            restored
                .inspect(|_, execution| execution.observed_for("max_tokens_in"))
                .unwrap(),
            "20"
        );
    }

    #[test]
    fn restore_reconciles_active_budget_when_durable_turn_is_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut coordinator =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".to_owned(),
                user_message: "hello".to_owned(),
            })
            .unwrap();
        coordinator.begin_active_turn("turn", None).unwrap();
        coordinator
            .transaction(|authority| authority.execution().record_tokens(20, 0))
            .unwrap();
        journal
            .append(SessionEvent::TurnCommitted {
                turn_id: "turn".to_owned(),
                assistant_message: "done".to_owned(),
            })
            .unwrap();
        let prior_epoch = coordinator.authority_epoch();
        drop(coordinator);

        let restored =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();

        assert_eq!(restored.active_turn_id(), None);
        assert_eq!(restored.authority_epoch(), prior_epoch + 1);
        assert_eq!(
            restored
                .current_execution_view()
                .unwrap()
                .observed_for("max_tokens_in"),
            "20"
        );
        let durable = journal.state().unwrap().budget_authority.unwrap();
        assert!(durable.active_turn.is_none());
    }

    #[test]
    fn shared_detached_seed_rebinds_in_place_to_durable_authority() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let seed = BudgetAuthoritySeed {
            provider_caps: config(None, 100).provider_caps,
            preserve_committed_session_extensions: true,
            execution_policy: config(None, 100).execution_policy,
            wall_clock: BudgetWallClockAuthority::ActiveRuntime,
            process_cleanup_proof: None,
        };
        let shared = seed.detached("bootstrap-session").unwrap();
        let captured = Arc::clone(&shared);

        assert!(!shared.lock().is_durably_bound());
        BudgetAuthorityCoordinator::bind_shared_pristine(
            &shared,
            seed.config(Some(journal.clone()), "stable-budget-session"),
        )
        .unwrap();

        assert!(Arc::ptr_eq(&shared, &captured));
        assert!(captured.lock().is_durably_bound());
        assert_eq!(captured.lock().budget_session_id(), "stable-budget-session");
        assert_eq!(captured.lock().authority_epoch(), 1);
        assert_eq!(
            journal
                .state()
                .unwrap()
                .budget_authority
                .unwrap()
                .budget_session_id,
            "stable-budget-session"
        );
    }

    #[test]
    fn shared_rebind_rejects_an_already_durable_authority() {
        let first_dir = tempfile::tempdir().unwrap();
        let first =
            SessionJournal::open(first_dir.path().join("session.journal"), "session").unwrap();
        baseline(&first);
        let second_dir = tempfile::tempdir().unwrap();
        let second =
            SessionJournal::open(second_dir.path().join("session.journal"), "session").unwrap();
        baseline(&second);
        let seed = BudgetAuthoritySeed {
            provider_caps: config(None, 100).provider_caps,
            preserve_committed_session_extensions: true,
            execution_policy: config(None, 100).execution_policy,
            wall_clock: BudgetWallClockAuthority::ActiveRuntime,
            process_cleanup_proof: None,
        };
        let shared = seed.detached("bootstrap-session").unwrap();
        BudgetAuthorityCoordinator::bind_shared_pristine(
            &shared,
            seed.config(Some(first), "stable-budget-session"),
        )
        .unwrap();

        let error = BudgetAuthorityCoordinator::bind_shared_pristine(
            &shared,
            seed.config(Some(second.clone()), "other-budget-session"),
        )
        .unwrap_err();

        assert!(matches!(error, BudgetAuthorityError::InvalidAuthority(_)));
        assert!(error.to_string().contains("no longer pristine"));
        assert_eq!(shared.lock().budget_session_id(), "stable-budget-session");
        assert!(second.state().unwrap().budget_authority.is_none());
    }

    #[test]
    fn shared_session_bind_is_idempotent_but_rejects_cross_session_switch() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let seed = BudgetAuthoritySeed {
            provider_caps: config(None, 100).provider_caps,
            preserve_committed_session_extensions: true,
            execution_policy: config(None, 100).execution_policy,
            wall_clock: BudgetWallClockAuthority::ActiveRuntime,
            process_cleanup_proof: None,
        };
        let shared = seed.detached("bootstrap-session").unwrap();
        BudgetAuthorityCoordinator::bind_shared_session(
            &shared,
            seed.config(Some(journal.clone()), "stable-budget-session"),
        )
        .unwrap();
        let epoch = shared.lock().authority_epoch();

        BudgetAuthorityCoordinator::bind_shared_session(
            &shared,
            seed.config(Some(journal.clone()), "stable-budget-session"),
        )
        .unwrap();
        assert_eq!(shared.lock().authority_epoch(), epoch);

        let error = BudgetAuthorityCoordinator::bind_shared_session(
            &shared,
            seed.config(Some(journal), "different-budget-session"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cross-session budget rebind"));
        assert_eq!(shared.lock().budget_session_id(), "stable-budget-session");
    }

    #[test]
    fn restore_repairs_turn_started_before_active_budget_commit() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let coordinator =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();
        let prior_epoch = coordinator.authority_epoch();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".to_owned(),
                user_message: "hello".to_owned(),
            })
            .unwrap();
        drop(coordinator);

        let restored =
            BudgetAuthorityCoordinator::bind(config(Some(journal.clone()), 100)).unwrap();

        assert_eq!(restored.active_turn_id(), Some("turn"));
        assert_eq!(restored.authority_epoch(), prior_epoch + 1);
        let durable = journal.state().unwrap().budget_authority.unwrap();
        assert_eq!(
            durable
                .active_turn
                .as_ref()
                .map(|turn| turn.turn_id.as_str()),
            Some("turn")
        );
    }

    #[test]
    fn journal_failure_permanently_latches_runtime_authority() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        baseline(&journal);
        let mut coordinator = BudgetAuthorityCoordinator::bind(config(Some(journal), 100)).unwrap();

        assert!(coordinator.begin_active_turn("unknown-turn", None).is_err());
        assert!(coordinator.fault_reason().is_some());
        assert!(matches!(
            coordinator.transaction(|_| ()),
            Err(BudgetAuthorityError::Faulted { .. })
        ));
        assert!(matches!(
            coordinator.inspect(|_, _| ()),
            Err(BudgetAuthorityError::Faulted { .. })
        ));
    }

    #[test]
    fn detached_coordinator_does_not_claim_a_durable_epoch() {
        let mut coordinator = BudgetAuthorityCoordinator::bind(config(None, 100)).unwrap();
        coordinator
            .transaction(|authority| authority.execution().record_tokens(1, 0))
            .unwrap();
        assert_eq!(coordinator.authority_epoch(), 0);
        assert!(coordinator.commit_current_authority().unwrap().is_none());
    }
}
