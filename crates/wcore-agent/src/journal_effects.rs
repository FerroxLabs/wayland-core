//! Durable journal coordination for externally visible turn effects.
//!
//! Callers must carry an explicit [`TurnEffectScope`] into spawned tasks. A
//! prepared lease is returned only after the intent is durable. Consuming that
//! lease records either the physical start or a typed not-started outcome, so
//! policy denial never needs a fabricated start event.

use serde_json::Value;
use wcore_types::tool::ToolEffectContract;

use crate::session_journal::{
    ApprovalOrigin, ApprovalResolution, BudgetAmount, BudgetOwner, BudgetPurpose,
    ChildNotStartedReason, CompletionOutcome, DeliveryCompletion, DeliveryEvidence,
    DeliveryNotStartedReason, DeliveryOrigin, DeliveryUnknownReason, HOOK_PHASE_LIFECYCLE_VERSION,
    HookManifestSlot, HookPhaseNotStartedReason, HookSlotReceipt, JournalError, SessionEvent,
    SessionJournal, StoredToolInput, ToolHookPhase, ToolNotStartedReason, ToolResolution,
    ToolResolutionSource, ToolUnknownReason, state_payload_digest,
};

/// Session-wide journal authority for effect lifecycles.
#[derive(Debug, Clone)]
pub struct JournalEffectCoordinator {
    journal: SessionJournal,
}

impl JournalEffectCoordinator {
    #[must_use]
    pub fn new(journal: SessionJournal) -> Self {
        Self { journal }
    }

    /// Create an explicit, cloneable scope suitable for `tokio::spawn`.
    #[must_use]
    pub fn for_turn(&self, turn_id: impl Into<String>) -> TurnEffectScope {
        TurnEffectScope {
            coordinator: self.clone(),
            turn_id: turn_id.into(),
        }
    }

    pub fn request_approval(
        &self,
        origin: ApprovalOrigin,
        intent: &Value,
    ) -> Result<PendingApprovalLease, JournalError> {
        self.request_approval_with_id(new_id("approval"), origin, intent)
    }

    /// Persist an approval using the caller's stable correlation identifier.
    ///
    /// Host-backed tool approvals use the provider call id so recovery, the
    /// approval manager, and protocol frames all refer to the same request.
    pub fn request_approval_with_id(
        &self,
        approval_id: impl Into<String>,
        origin: ApprovalOrigin,
        intent: &Value,
    ) -> Result<PendingApprovalLease, JournalError> {
        let approval_id = approval_id.into();
        let intent_digest = state_payload_digest(intent)?;
        self.journal.append(SessionEvent::ApprovalRequested {
            approval_id: approval_id.clone(),
            origin,
            intent_digest,
        })?;
        Ok(PendingApprovalLease {
            journal: self.journal.clone(),
            approval_id,
        })
    }

    pub fn reserve_budget(
        &self,
        owner: BudgetOwner,
        purpose: BudgetPurpose,
        amount: BudgetAmount,
    ) -> Result<BudgetReservationLease, JournalError> {
        let reservation_id = new_id("budget-reservation");
        self.journal.append(SessionEvent::BudgetReserved {
            event_id: new_id("budget-event"),
            reservation_id: reservation_id.clone(),
            owner,
            purpose,
            amount,
        })?;
        Ok(BudgetReservationLease {
            journal: self.journal.clone(),
            reservation_id,
        })
    }

    pub fn prepare_delivery(
        &self,
        origin: DeliveryOrigin,
        destination: impl Into<String>,
        payload: Value,
    ) -> Result<PreparedDeliveryLease, JournalError> {
        let delivery_id = new_id("delivery");
        self.journal.append(SessionEvent::DeliveryPrepared {
            delivery_id: delivery_id.clone(),
            origin,
            destination: destination.into(),
            payload,
        })?;
        Ok(PreparedDeliveryLease {
            journal: self.journal.clone(),
            delivery_id,
        })
    }

    /// Persist a reconciliation decision for an unknown tool effect. This is
    /// also the restart-safe entry point used when no runtime lease survives.
    pub fn resolve_tool(
        &self,
        tool_execution_id: impl Into<String>,
        resolution: ToolResolution,
        source: ToolResolutionSource,
        evidence: Value,
    ) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::ToolExecutionResolved {
            tool_execution_id: tool_execution_id.into(),
            resolution,
            source,
            evidence,
        })?;
        Ok(())
    }
}

/// Explicit turn identity and journal authority. This is ordinary cloneable
/// data, not task-local state, so spawned work cannot silently lose it.
#[derive(Debug, Clone)]
pub struct TurnEffectScope {
    coordinator: JournalEffectCoordinator,
    turn_id: String,
}

impl TurnEffectScope {
    #[must_use]
    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    pub fn store_effect_checkpoint(
        &self,
        digest: &str,
        contents: &[u8],
    ) -> Result<(), JournalError> {
        self.coordinator
            .journal
            .store_effect_checkpoint(digest, contents)
    }

    /// Persist aggregate hook-phase authority before any hook implementation
    /// can observe or affect the tool round. The identifier is deterministic,
    /// so recovery can rediscover the same phase without minting a second
    /// execution authority.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_hook_phase(
        &self,
        provider_call_id: impl Into<String>,
        ordinal: u64,
        phase: ToolHookPhase,
        tool_execution_id: Option<String>,
        input_digest: impl Into<String>,
        hook_authority_digest: impl Into<String>,
        hook_manifest_digest: impl Into<String>,
        hook_slots: Vec<HookManifestSlot>,
    ) -> Result<PreparedHookPhaseLease, JournalError> {
        let provider_call_id = provider_call_id.into();
        let input_digest = input_digest.into();
        let hook_authority_digest = hook_authority_digest.into();
        let hook_manifest_digest = hook_manifest_digest.into();
        let session_id = self.coordinator.journal.session_id()?;
        let hook_phase_id = hook_phase_id(
            &session_id,
            &self.turn_id,
            &provider_call_id,
            ordinal,
            phase,
            tool_execution_id.as_deref(),
            &input_digest,
            &hook_authority_digest,
            &hook_manifest_digest,
        )?;
        self.coordinator
            .journal
            .append(SessionEvent::HookPhasePrepared {
                hook_phase_id: hook_phase_id.clone(),
                lifecycle_version: HOOK_PHASE_LIFECYCLE_VERSION,
                turn_id: self.turn_id.clone(),
                provider_call_id,
                ordinal,
                phase,
                tool_execution_id,
                input_digest,
                hook_authority_digest,
                hook_manifest_digest,
                hook_slots,
            })?;
        Ok(PreparedHookPhaseLease {
            journal: self.coordinator.journal.clone(),
            hook_phase_id,
        })
    }

    pub fn prepare_tool(
        &self,
        provider_call_id: impl Into<String>,
        ordinal: u64,
        tool: impl Into<String>,
        requested_input: Value,
        effective_input: Value,
    ) -> Result<PreparedToolLease, JournalError> {
        self.prepare_tool_with_contract(
            provider_call_id,
            ordinal,
            tool,
            requested_input,
            effective_input,
            ToolEffectContract::default(),
        )
    }

    pub fn prepare_tool_with_contract(
        &self,
        provider_call_id: impl Into<String>,
        ordinal: u64,
        tool: impl Into<String>,
        requested_input: Value,
        effective_input: Value,
        effect_contract: ToolEffectContract,
    ) -> Result<PreparedToolLease, JournalError> {
        self.prepare_tool_recorded(
            provider_call_id,
            ordinal,
            tool,
            requested_input,
            effective_input,
            effect_contract,
            None,
            None,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare_tool_with_effect_receipt(
        &self,
        provider_call_id: impl Into<String>,
        ordinal: u64,
        tool: impl Into<String>,
        requested_input: Value,
        effective_input: Value,
        effect_contract: ToolEffectContract,
        effect_receipt: Value,
    ) -> Result<PreparedToolLease, JournalError> {
        self.prepare_tool_recorded(
            provider_call_id,
            ordinal,
            tool,
            requested_input,
            effective_input,
            effect_contract,
            Some(effect_receipt),
            None,
            None,
            None,
        )
    }

    /// Prepare an invocation whose exact inputs are recoverable only through
    /// caller-provided secured envelopes. The key still derives from the exact
    /// plaintext digests, never from the encrypted representation.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_tool_with_secured_inputs(
        &self,
        provider_call_id: impl Into<String>,
        ordinal: u64,
        tool: impl Into<String>,
        requested_input: Value,
        effective_input: Value,
        effect_contract: ToolEffectContract,
        secured_requested_input: Value,
        secured_effective_input: Value,
    ) -> Result<PreparedToolLease, JournalError> {
        self.prepare_tool_recorded(
            provider_call_id,
            ordinal,
            tool,
            requested_input,
            effective_input,
            effect_contract,
            None,
            Some(secured_requested_input),
            Some(secured_effective_input),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare_tool_after_hook(
        &self,
        provider_call_id: impl Into<String>,
        ordinal: u64,
        tool: impl Into<String>,
        requested_input: Value,
        effective_input: Value,
        effect_contract: ToolEffectContract,
        pre_hook_phase_id: impl Into<String>,
    ) -> Result<PreparedToolLease, JournalError> {
        self.prepare_tool_recorded(
            provider_call_id,
            ordinal,
            tool,
            requested_input,
            effective_input,
            effect_contract,
            None,
            None,
            None,
            Some(pre_hook_phase_id.into()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare_tool_with_effect_receipt_after_hook(
        &self,
        provider_call_id: impl Into<String>,
        ordinal: u64,
        tool: impl Into<String>,
        requested_input: Value,
        effective_input: Value,
        effect_contract: ToolEffectContract,
        effect_receipt: Value,
        pre_hook_phase_id: impl Into<String>,
    ) -> Result<PreparedToolLease, JournalError> {
        self.prepare_tool_recorded(
            provider_call_id,
            ordinal,
            tool,
            requested_input,
            effective_input,
            effect_contract,
            Some(effect_receipt),
            None,
            None,
            Some(pre_hook_phase_id.into()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_tool_recorded(
        &self,
        provider_call_id: impl Into<String>,
        ordinal: u64,
        tool: impl Into<String>,
        requested_input: Value,
        effective_input: Value,
        effect_contract: ToolEffectContract,
        effect_receipt: Option<Value>,
        secured_requested_input: Option<Value>,
        secured_effective_input: Option<Value>,
        pre_hook_phase_id: Option<String>,
    ) -> Result<PreparedToolLease, JournalError> {
        let tool_execution_id = new_id("tool-execution");
        let provider_call_id = provider_call_id.into();
        let tool = tool.into();
        let requested_input_digest = state_payload_digest(&requested_input)?;
        let effective_input_digest = state_payload_digest(&effective_input)?;
        let requested_input = secured_requested_input.map_or_else(
            || StoredToolInput::redacted(requested_input_digest.clone()),
            |envelope| StoredToolInput::Secured {
                exact_digest: requested_input_digest.clone(),
                envelope,
            },
        );
        let effective_input = secured_effective_input.map_or_else(
            || StoredToolInput::redacted(effective_input_digest.clone()),
            |envelope| StoredToolInput::Secured {
                exact_digest: effective_input_digest.clone(),
                envelope,
            },
        );
        let session_id = self.coordinator.journal.session_id()?;
        let idempotency_key = tool_idempotency_key(
            &session_id,
            &self.turn_id,
            &provider_call_id,
            ordinal,
            &tool,
            &effective_input_digest,
        )?;
        self.coordinator
            .journal
            .append(SessionEvent::ToolIntentRecordedV2 {
                tool_execution_id: tool_execution_id.clone(),
                idempotency_key: idempotency_key.clone(),
                retry_of: None,
                provider_call_id,
                turn_id: self.turn_id.clone(),
                ordinal,
                tool,
                requested_input,
                requested_input_digest,
                effective_input,
                effective_input_digest,
                effect_contract,
                effect_receipt,
                pre_hook_phase_id,
            })?;
        Ok(PreparedToolLease {
            journal: self.coordinator.journal.clone(),
            tool_execution_id,
            idempotency_key,
        })
    }

    /// Create a new physical-attempt authority after a prior attempt was
    /// durably proven not to have started. The original attempt remains
    /// immutable, and the retry reuses its exact stable idempotency key,
    /// inputs, tool identity, effect contract, and reconciliation receipt.
    pub fn retry_not_started_tool(
        &self,
        prior_tool_execution_id: &str,
    ) -> Result<PreparedToolLease, JournalError> {
        let state = self.coordinator.journal.state()?;
        let prior = state.tools.get(prior_tool_execution_id).ok_or_else(|| {
            JournalError::InvalidTransition(format!(
                "unknown tool execution id {prior_tool_execution_id}"
            ))
        })?;
        if prior.turn_id != self.turn_id {
            return Err(JournalError::InvalidTransition(format!(
                "tool execution {prior_tool_execution_id} belongs to turn {}, not {}",
                prior.turn_id, self.turn_id
            )));
        }

        let tool_execution_id = new_id("tool-execution");
        let idempotency_key = prior.idempotency_key.clone();
        self.coordinator
            .journal
            .append(SessionEvent::ToolIntentRecordedV2 {
                tool_execution_id: tool_execution_id.clone(),
                idempotency_key: idempotency_key.clone(),
                retry_of: Some(prior_tool_execution_id.to_owned()),
                provider_call_id: prior.provider_call_id.clone(),
                turn_id: prior.turn_id.clone(),
                ordinal: prior.ordinal,
                tool: prior.tool.clone(),
                requested_input: prior.requested_input.clone(),
                requested_input_digest: prior.requested_input_digest.clone(),
                effective_input: prior.effective_input.clone(),
                effective_input_digest: prior.effective_input_digest.clone(),
                effect_contract: prior.effect_contract.clone(),
                effect_receipt: prior.effect_receipt.clone(),
                pre_hook_phase_id: prior.pre_hook_phase_id.clone(),
            })?;
        Ok(PreparedToolLease {
            journal: self.coordinator.journal.clone(),
            tool_execution_id,
            idempotency_key,
        })
    }

    pub fn request_approval(&self, intent: &Value) -> Result<PendingApprovalLease, JournalError> {
        self.coordinator.request_approval(
            ApprovalOrigin::Turn {
                turn_id: self.turn_id.clone(),
            },
            intent,
        )
    }

    pub fn request_approval_with_id(
        &self,
        approval_id: impl Into<String>,
        intent: &Value,
    ) -> Result<PendingApprovalLease, JournalError> {
        self.coordinator.request_approval_with_id(
            approval_id,
            ApprovalOrigin::Turn {
                turn_id: self.turn_id.clone(),
            },
            intent,
        )
    }

    pub fn reserve_budget(
        &self,
        purpose: BudgetPurpose,
        amount: BudgetAmount,
    ) -> Result<BudgetReservationLease, JournalError> {
        self.coordinator.reserve_budget(
            BudgetOwner::Turn {
                turn_id: self.turn_id.clone(),
            },
            purpose,
            amount,
        )
    }

    pub fn prepare_child(
        &self,
        child_id: impl Into<String>,
        request: Value,
    ) -> Result<PreparedChildLease, JournalError> {
        let child_id = child_id.into();
        self.coordinator
            .journal
            .append(SessionEvent::ChildPrepared {
                child_id: child_id.clone(),
                turn_id: self.turn_id.clone(),
                request,
            })?;
        Ok(PreparedChildLease {
            journal: self.coordinator.journal.clone(),
            child_id,
        })
    }

    pub fn prepare_delivery(
        &self,
        destination: impl Into<String>,
        payload: Value,
    ) -> Result<PreparedDeliveryLease, JournalError> {
        self.coordinator.prepare_delivery(
            DeliveryOrigin::Turn {
                turn_id: self.turn_id.clone(),
            },
            destination,
            payload,
        )
    }
}

#[derive(Debug)]
#[must_use = "a prepared hook phase must be started or closed as not applicable"]
pub struct PreparedHookPhaseLease {
    journal: SessionJournal,
    hook_phase_id: String,
}

impl PreparedHookPhaseLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.hook_phase_id
    }

    pub fn start(
        self,
        result_digest: Option<String>,
    ) -> Result<StartedHookPhaseLease, JournalError> {
        self.journal.append(SessionEvent::HookPhaseStarted {
            hook_phase_id: self.hook_phase_id.clone(),
            result_digest: result_digest.clone(),
        })?;
        Ok(StartedHookPhaseLease {
            journal: self.journal,
            hook_phase_id: self.hook_phase_id,
            result_digest,
        })
    }

    pub fn not_applicable(self) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::HookPhaseNotApplicable {
            hook_phase_id: self.hook_phase_id,
        })?;
        Ok(())
    }

    pub fn not_started(self, reason: HookPhaseNotStartedReason) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::HookPhaseNotStarted {
            hook_phase_id: self.hook_phase_id,
            reason,
        })?;
        Ok(())
    }
}

#[derive(Debug)]
#[must_use = "a started hook phase must be finished or abandoned as unknown"]
pub struct StartedHookPhaseLease {
    journal: SessionJournal,
    hook_phase_id: String,
    result_digest: Option<String>,
}

impl StartedHookPhaseLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.hook_phase_id
    }

    pub fn finish(
        self,
        effective_input_digest: Option<String>,
        outcome_digest: impl Into<String>,
        slot_receipts_digest: impl Into<String>,
        slot_receipts: Vec<HookSlotReceipt>,
    ) -> Result<crate::session_journal::HookPhaseConsumption, JournalError> {
        let hook_phase_id = self.hook_phase_id;
        let outcome_digest = outcome_digest.into();
        self.journal.append(SessionEvent::HookPhaseFinished {
            hook_phase_id: hook_phase_id.clone(),
            result_digest: self.result_digest,
            effective_input_digest,
            outcome_digest: outcome_digest.clone(),
            slot_receipts_digest: slot_receipts_digest.into(),
            slot_receipts,
        })?;
        Ok(crate::session_journal::HookPhaseConsumption {
            hook_phase_id,
            outcome_digest,
        })
    }

    pub fn abandon_unknown(self) -> Result<(), JournalError> {
        self.journal
            .append(SessionEvent::HookPhaseAbandonedUnknown {
                hook_phase_id: self.hook_phase_id,
            })?;
        Ok(())
    }
}

#[derive(Debug)]
#[must_use = "a prepared tool must be started or terminalized as not started"]
pub struct PreparedToolLease {
    journal: SessionJournal,
    tool_execution_id: String,
    idempotency_key: String,
}

impl PreparedToolLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.tool_execution_id
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    #[must_use]
    pub fn approval_origin(&self) -> ApprovalOrigin {
        ApprovalOrigin::ToolExecution {
            tool_execution_id: self.tool_execution_id.clone(),
        }
    }

    #[must_use]
    pub fn budget_owner(&self) -> BudgetOwner {
        BudgetOwner::ToolExecution {
            tool_execution_id: self.tool_execution_id.clone(),
        }
    }

    pub fn start(self) -> Result<StartedToolLease, JournalError> {
        self.journal.append(SessionEvent::ToolExecutionStarted {
            tool_execution_id: self.tool_execution_id.clone(),
        })?;
        Ok(StartedToolLease {
            journal: self.journal,
            tool_execution_id: self.tool_execution_id,
            idempotency_key: self.idempotency_key,
        })
    }

    pub fn not_started(self, reason: ToolNotStartedReason) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::ToolExecutionNotStarted {
            tool_execution_id: self.tool_execution_id,
            reason,
        })?;
        Ok(())
    }
}

#[derive(Debug)]
#[must_use = "a started tool must be finished or explicitly left unknown"]
pub struct StartedToolLease {
    journal: SessionJournal,
    tool_execution_id: String,
    idempotency_key: String,
}

impl StartedToolLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.tool_execution_id
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    pub fn succeed(self, result: Value) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::ToolExecutionFinished {
            tool_execution_id: self.tool_execution_id,
            outcome: CompletionOutcome::Succeeded,
            result,
        })?;
        Ok(())
    }

    /// Record an authoritative terminal failure. Timeouts, cancellation, and
    /// lost transports must use [`Self::unknown`] instead because they do not
    /// prove that an external effect failed.
    pub fn fail(self, error: impl Into<String>, result: Value) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::ToolExecutionFinished {
            tool_execution_id: self.tool_execution_id,
            outcome: CompletionOutcome::Failed {
                error: error.into(),
            },
            result,
        })?;
        Ok(())
    }

    pub fn unknown(
        self,
        reason: ToolUnknownReason,
        evidence: Value,
    ) -> Result<UnknownToolEffect, JournalError> {
        self.journal.append(SessionEvent::ToolExecutionUnknown {
            tool_execution_id: self.tool_execution_id.clone(),
            reason,
            evidence,
        })?;
        Ok(UnknownToolEffect {
            coordinator: JournalEffectCoordinator {
                journal: self.journal,
            },
            tool_execution_id: self.tool_execution_id,
        })
    }
}

#[derive(Debug)]
#[must_use = "an unknown tool effect must be reconciled or surfaced to an operator"]
pub struct UnknownToolEffect {
    coordinator: JournalEffectCoordinator,
    tool_execution_id: String,
}

impl UnknownToolEffect {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.tool_execution_id
    }

    pub fn resolve(
        self,
        resolution: ToolResolution,
        source: ToolResolutionSource,
        evidence: Value,
    ) -> Result<(), JournalError> {
        self.coordinator
            .resolve_tool(self.tool_execution_id, resolution, source, evidence)
    }
}

#[derive(Debug)]
#[must_use = "an approval request must be resolved"]
pub struct PendingApprovalLease {
    journal: SessionJournal,
    approval_id: String,
}

impl PendingApprovalLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.approval_id
    }

    pub fn resolve(self, resolution: ApprovalResolution) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::ApprovalResolved {
            approval_id: self.approval_id,
            resolution,
        })?;
        Ok(())
    }
}

#[derive(Debug)]
#[must_use = "a budget reservation must be settled or released"]
pub struct BudgetReservationLease {
    journal: SessionJournal,
    reservation_id: String,
}

impl BudgetReservationLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.reservation_id
    }

    pub fn settle(self, amount: BudgetAmount) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::BudgetSettled {
            event_id: new_id("budget-event"),
            reservation_id: self.reservation_id,
            amount,
        })?;
        Ok(())
    }

    pub fn release(self) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::BudgetReleased {
            event_id: new_id("budget-event"),
            reservation_id: self.reservation_id,
        })?;
        Ok(())
    }
}

#[derive(Debug)]
#[must_use = "a prepared child must be started or terminalized as not started"]
pub struct PreparedChildLease {
    journal: SessionJournal,
    child_id: String,
}

impl PreparedChildLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.child_id
    }

    #[must_use]
    pub fn approval_origin(&self) -> ApprovalOrigin {
        ApprovalOrigin::Child {
            child_id: self.child_id.clone(),
        }
    }

    #[must_use]
    pub fn budget_owner(&self) -> BudgetOwner {
        BudgetOwner::Child {
            child_id: self.child_id.clone(),
        }
    }

    pub fn start(self) -> Result<StartedChildLease, JournalError> {
        self.journal.append(SessionEvent::ChildStarted {
            child_id: self.child_id.clone(),
        })?;
        Ok(StartedChildLease {
            journal: self.journal,
            child_id: self.child_id,
        })
    }

    pub fn not_started(self, reason: ChildNotStartedReason) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::ChildNotStarted {
            child_id: self.child_id,
            reason,
        })?;
        Ok(())
    }
}

#[derive(Debug)]
#[must_use = "a started child must be finished or explicitly left unknown"]
pub struct StartedChildLease {
    journal: SessionJournal,
    child_id: String,
}

impl StartedChildLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.child_id
    }

    pub fn finish(self, outcome: CompletionOutcome, result: Value) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::ChildFinished {
            child_id: self.child_id,
            outcome,
            result,
        })?;
        Ok(())
    }

    /// `ChildStarted` is itself the durable transition to `Unknown`.
    #[must_use]
    pub fn leave_unknown(self) -> UnknownEffect {
        UnknownEffect {
            effect_id: self.child_id,
        }
    }
}

#[derive(Debug)]
#[must_use = "a prepared delivery must be started or terminalized as not started"]
pub struct PreparedDeliveryLease {
    journal: SessionJournal,
    delivery_id: String,
}

impl PreparedDeliveryLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.delivery_id
    }

    #[must_use]
    pub fn approval_origin(&self) -> ApprovalOrigin {
        ApprovalOrigin::Delivery {
            delivery_id: self.delivery_id.clone(),
        }
    }

    pub fn start(self) -> Result<StartedDeliveryLease, JournalError> {
        self.journal.append(SessionEvent::DeliveryStarted {
            delivery_id: self.delivery_id.clone(),
        })?;
        Ok(StartedDeliveryLease {
            journal: self.journal,
            delivery_id: self.delivery_id,
        })
    }

    pub fn not_started(self, reason: DeliveryNotStartedReason) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::DeliveryNotStarted {
            delivery_id: self.delivery_id,
            reason,
        })?;
        Ok(())
    }
}

#[derive(Debug)]
#[must_use = "a started delivery must be finished or recorded as unknown"]
pub struct StartedDeliveryLease {
    journal: SessionJournal,
    delivery_id: String,
}

impl StartedDeliveryLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.delivery_id
    }

    pub fn finish(self, completion: DeliveryCompletion) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::DeliveryFinished {
            delivery_id: self.delivery_id,
            completion,
        })?;
        Ok(())
    }

    pub fn unknown(
        self,
        reason: DeliveryUnknownReason,
        evidence: DeliveryEvidence,
    ) -> Result<(), JournalError> {
        self.finish(DeliveryCompletion::Unknown { reason, evidence })
    }
}

/// Receipt returned when a started effect intentionally remains unresolved.
/// Recovery must reconcile it before retrying the operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEffect {
    effect_id: String,
}

impl UnknownEffect {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.effect_id
    }
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4())
}

#[allow(clippy::too_many_arguments)]
fn hook_phase_id(
    session_id: &str,
    turn_id: &str,
    provider_call_id: &str,
    ordinal: u64,
    phase: ToolHookPhase,
    tool_execution_id: Option<&str>,
    input_digest: &str,
    hook_authority_digest: &str,
    hook_manifest_digest: &str,
) -> Result<String, JournalError> {
    let digest = state_payload_digest(&serde_json::json!([
        "wayland-hook-phase-v1",
        session_id,
        turn_id,
        provider_call_id,
        ordinal,
        phase,
        tool_execution_id,
        input_digest,
        hook_authority_digest,
        hook_manifest_digest,
    ]))?;
    Ok(format!("hook-phase-{digest}"))
}

fn tool_idempotency_key(
    session_id: &str,
    turn_id: &str,
    provider_call_id: &str,
    ordinal: u64,
    tool: &str,
    effective_input_digest: &str,
) -> Result<String, JournalError> {
    state_payload_digest(&serde_json::json!([
        "wayland-tool-effect-v1",
        session_id,
        turn_id,
        provider_call_id,
        ordinal,
        tool,
        effective_input_digest,
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_journal::{
        ApprovalDecision, BudgetUnit, DeliveryStage, ExternalEffectState, StoredToolInput,
        ToolEffectState, ToolState,
    };
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use wcore_types::tool::{ToolEffectContract, ToolEffectKind};

    struct Fixture {
        _dir: tempfile::TempDir,
        journal: SessionJournal,
        scope: TurnEffectScope,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".into(),
                user_message: "test".into(),
            })
            .unwrap();
        let scope = JournalEffectCoordinator::new(journal.clone()).for_turn("turn-1");
        Fixture {
            _dir: dir,
            journal,
            scope,
        }
    }

    fn retry_event(prior_id: &str, new_id: &str, prior: &ToolState) -> SessionEvent {
        SessionEvent::ToolIntentRecordedV2 {
            tool_execution_id: new_id.to_owned(),
            idempotency_key: prior.idempotency_key.clone(),
            retry_of: Some(prior_id.to_owned()),
            provider_call_id: prior.provider_call_id.clone(),
            turn_id: prior.turn_id.clone(),
            ordinal: prior.ordinal,
            tool: prior.tool.clone(),
            requested_input: prior.requested_input.clone(),
            requested_input_digest: prior.requested_input_digest.clone(),
            effective_input: prior.effective_input.clone(),
            effective_input_digest: prior.effective_input_digest.clone(),
            effect_contract: prior.effect_contract.clone(),
            effect_receipt: prior.effect_receipt.clone(),
            pre_hook_phase_id: prior.pre_hook_phase_id.clone(),
        }
    }

    #[test]
    fn tool_execution_identity_and_transitions_are_durable() {
        let fixture = fixture();
        let prepared = fixture
            .scope
            .prepare_tool(
                "provider-call-1",
                0,
                "Read",
                json!({"path": "requested"}),
                json!({"path": "effective"}),
            )
            .unwrap();
        let execution_id = prepared.id().to_owned();
        let idempotency_key = prepared.idempotency_key().to_owned();
        assert_ne!(execution_id, "provider-call-1");
        assert_eq!(idempotency_key.len(), 64);
        assert!(matches!(
            &fixture.journal.state().unwrap().tools[&execution_id].effect,
            ToolEffectState::Prepared
        ));

        let started = prepared.start().unwrap();
        assert_eq!(started.idempotency_key(), idempotency_key);
        assert!(matches!(
            &fixture.journal.state().unwrap().tools[&execution_id].effect,
            ToolEffectState::Running
        ));
        started.succeed(json!({"ok": true})).unwrap();
        assert!(matches!(
            &fixture.journal.state().unwrap().tools[&execution_id].effect,
            ToolEffectState::Succeeded
        ));
    }

    #[test]
    fn not_started_retry_is_a_new_linked_attempt_with_the_same_authority() {
        let fixture = fixture();
        let first = fixture
            .scope
            .prepare_tool_with_contract(
                "provider-call-retry",
                0,
                "Write",
                json!({"path": "requested"}),
                json!({"path": "effective"}),
                ToolEffectContract {
                    kind: ToolEffectKind::RepeatSafe,
                    reconciler: None,
                },
            )
            .unwrap();
        let first_id = first.id().to_owned();
        let stable_key = first.idempotency_key().to_owned();
        first
            .start()
            .unwrap()
            .unknown(
                ToolUnknownReason::Interrupted,
                json!({"cut": "after_start"}),
            )
            .unwrap()
            .resolve(
                ToolResolution::NotStarted {
                    reason: ToolNotStartedReason::Cancelled {
                        reason: "reconciled preimage".into(),
                    },
                },
                ToolResolutionSource::Reconciler {
                    reconciler: "test.reconciler.v1".into(),
                },
                json!({"current": "preimage"}),
            )
            .unwrap();

        let second = fixture.scope.retry_not_started_tool(&first_id).unwrap();
        let second_id = second.id().to_owned();
        assert_ne!(second_id, first_id);
        assert_eq!(second.idempotency_key(), stable_key);

        let state = fixture.journal.state().unwrap();
        let first_state = &state.tools[&first_id];
        let second_state = &state.tools[&second_id];
        assert!(matches!(first_state.effect, ToolEffectState::NotStarted));
        assert!(matches!(second_state.effect, ToolEffectState::Prepared));
        assert_eq!(second_state.retry_of.as_deref(), Some(first_id.as_str()));
        assert_eq!(second_state.idempotency_key, first_state.idempotency_key);
        assert_eq!(second_state.requested_input, first_state.requested_input);
        assert_eq!(second_state.effective_input, first_state.effective_input);
        assert_eq!(second_state.tool, first_state.tool);
        assert_eq!(second_state.effect_contract, first_state.effect_contract);

        assert!(fixture.scope.retry_not_started_tool(&first_id).is_err());
        second
            .not_started(ToolNotStartedReason::Cancelled {
                reason: "second attempt did not start".into(),
            })
            .unwrap();
        let third = fixture.scope.retry_not_started_tool(&second_id).unwrap();
        assert_eq!(third.idempotency_key(), stable_key);
    }

    #[test]
    fn retry_reducer_rejects_identity_input_tool_and_contract_drift() {
        fn prior_not_started() -> Fixture {
            let fixture = fixture();
            fixture
                .scope
                .prepare_tool_with_contract(
                    "provider-call-retry-mismatch",
                    0,
                    "Write",
                    json!({"path": "requested"}),
                    json!({"path": "effective"}),
                    ToolEffectContract {
                        kind: ToolEffectKind::RepeatSafe,
                        reconciler: None,
                    },
                )
                .unwrap()
                .not_started(ToolNotStartedReason::Cancelled {
                    reason: "not applied".into(),
                })
                .unwrap();
            fixture
        }

        type RetryMutation = fn(&mut SessionEvent);
        let mutations: [RetryMutation; 4] = [
            |event| {
                if let SessionEvent::ToolIntentRecordedV2 {
                    idempotency_key, ..
                } = event
                {
                    *idempotency_key = "different-key".into();
                }
            },
            |event| {
                if let SessionEvent::ToolIntentRecordedV2 {
                    effective_input,
                    effective_input_digest,
                    ..
                } = event
                {
                    let digest = state_payload_digest(&json!({"path": "different"})).unwrap();
                    *effective_input = StoredToolInput::redacted(digest.clone());
                    *effective_input_digest = digest;
                }
            },
            |event| {
                if let SessionEvent::ToolIntentRecordedV2 { tool, .. } = event {
                    *tool = "Edit".into();
                }
            },
            |event| {
                if let SessionEvent::ToolIntentRecordedV2 {
                    effect_contract, ..
                } = event
                {
                    *effect_contract = ToolEffectContract::default();
                }
            },
        ];

        for (index, mutate) in mutations.into_iter().enumerate() {
            let fixture = prior_not_started();
            let state = fixture.journal.state().unwrap();
            let (prior_id, prior) = state.tools.iter().next().unwrap();
            let mut event = retry_event(prior_id, &format!("retry-{index}"), prior);
            mutate(&mut event);
            assert!(matches!(
                fixture.journal.append(event),
                Err(JournalError::InvalidTransition(_))
            ));
        }
    }

    #[test]
    fn retry_is_forbidden_from_every_state_except_not_started() {
        let prepared = fixture();
        let lease = prepared
            .scope
            .prepare_tool("provider-prepared", 0, "Read", json!({}), json!({}))
            .unwrap();
        let prepared_id = lease.id().to_owned();
        assert!(prepared.scope.retry_not_started_tool(&prepared_id).is_err());

        let running = fixture();
        let lease = running
            .scope
            .prepare_tool("provider-running", 0, "Bash", json!({}), json!({}))
            .unwrap();
        let running_id = lease.id().to_owned();
        let _started = lease.start().unwrap();
        assert!(running.scope.retry_not_started_tool(&running_id).is_err());

        let unknown = fixture();
        let lease = unknown
            .scope
            .prepare_tool("provider-unknown", 0, "Bash", json!({}), json!({}))
            .unwrap();
        let unknown_id = lease.id().to_owned();
        let _unknown_effect = lease
            .start()
            .unwrap()
            .unknown(ToolUnknownReason::TransportLost, json!({}))
            .unwrap();
        assert!(unknown.scope.retry_not_started_tool(&unknown_id).is_err());

        let succeeded = fixture();
        let lease = succeeded
            .scope
            .prepare_tool("provider-succeeded", 0, "Read", json!({}), json!({}))
            .unwrap();
        let succeeded_id = lease.id().to_owned();
        lease.start().unwrap().succeed(json!({"ok": true})).unwrap();
        assert!(
            succeeded
                .scope
                .retry_not_started_tool(&succeeded_id)
                .is_err()
        );

        let failed = fixture();
        let lease = failed
            .scope
            .prepare_tool("provider-failed", 0, "Read", json!({}), json!({}))
            .unwrap();
        let failed_id = lease.id().to_owned();
        lease
            .start()
            .unwrap()
            .fail("authoritative", json!({"ok": false}))
            .unwrap();
        assert!(failed.scope.retry_not_started_tool(&failed_id).is_err());
    }

    #[test]
    fn tool_ordinal_uniqueness_is_scoped_to_provider_call_identity() {
        let fixture = fixture();
        let first = fixture
            .scope
            .prepare_tool("provider-round-1-call", 0, "Read", json!({}), json!({}))
            .unwrap();
        let second = fixture
            .scope
            .prepare_tool("provider-round-2-call", 0, "Read", json!({}), json!({}))
            .unwrap();
        assert_ne!(first.idempotency_key(), second.idempotency_key());
        assert_eq!(fixture.journal.state().unwrap().tools.len(), 2);
    }

    #[test]
    fn idempotency_key_domain_has_a_stable_golden_order() {
        assert_eq!(
            tool_idempotency_key(
                "golden-session",
                "golden-turn",
                "golden-provider-call",
                7,
                "Bash",
                "golden-effective-digest",
            )
            .unwrap(),
            "8bd364f6a17070cc764509c3a96481c47b923665a91ea93ada8828e7afebaf23"
        );
    }

    #[test]
    fn authoritative_tool_failure_is_distinct_from_unknown() {
        let fixture = fixture();
        let lease = fixture
            .scope
            .prepare_tool("provider-call-failed", 5, "Read", json!({}), json!({}))
            .unwrap();
        let id = lease.id().to_owned();
        lease
            .start()
            .unwrap()
            .fail("authoritative failure", json!({"receipt": "failed"}))
            .unwrap();
        assert!(matches!(
            &fixture.journal.state().unwrap().tools[&id].effect,
            ToolEffectState::Failed { error } if error == "authoritative failure"
        ));
    }

    #[test]
    fn deterministic_key_uses_exact_digest_while_inputs_are_redacted() {
        fn prepare_in(dir: &std::path::Path) -> (String, crate::session_journal::ToolState) {
            let journal =
                SessionJournal::open(dir.join("session.journal"), "same-session").unwrap();
            journal
                .append(SessionEvent::TurnStarted {
                    turn_id: "same-turn".into(),
                    user_message: "test".into(),
                })
                .unwrap();
            let scope = JournalEffectCoordinator::new(journal.clone()).for_turn("same-turn");
            let lease = scope
                .prepare_tool(
                    "provider-call",
                    7,
                    "Bash",
                    json!({"token": "requested-secret"}),
                    json!({"token": "effective-secret"}),
                )
                .unwrap();
            let id = lease.id().to_owned();
            (
                lease.idempotency_key().to_owned(),
                journal.state().unwrap().tools[&id].clone(),
            )
        }

        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let (first_key, first_state) = prepare_in(first.path());
        let (second_key, second_state) = prepare_in(second.path());

        assert_eq!(first_key, second_key);
        assert_eq!(
            first_state.effective_input_digest,
            second_state.effective_input_digest
        );
        assert!(matches!(
            first_state.requested_input,
            StoredToolInput::Redacted { .. }
        ));
        assert!(matches!(
            first_state.effective_input,
            StoredToolInput::Redacted { .. }
        ));
        assert_eq!(
            first_key,
            tool_idempotency_key(
                "same-session",
                "same-turn",
                "provider-call",
                7,
                "Bash",
                &first_state.effective_input_digest,
            )
            .unwrap()
        );
        let encoded = serde_json::to_string(&first_state).unwrap();
        assert!(!encoded.contains("requested-secret"));
        assert!(!encoded.contains("effective-secret"));
        let journal_bytes = std::fs::read(first.path().join("session.journal")).unwrap();
        let journal_text = String::from_utf8_lossy(&journal_bytes);
        assert!(!journal_text.contains("requested-secret"));
        assert!(!journal_text.contains("effective-secret"));
    }

    #[test]
    fn secured_input_envelope_and_contract_are_durable_without_changing_key_digest() {
        let fixture = fixture();
        let contract = ToolEffectContract {
            kind: ToolEffectKind::ProviderIdempotent,
            reconciler: Some("fixture-by-key-v1".into()),
        };
        let exact_effective = json!({"token": "secret"});
        let expected_digest = state_payload_digest(&exact_effective).unwrap();
        let lease = fixture
            .scope
            .prepare_tool_with_secured_inputs(
                "provider-call-secured",
                3,
                "Remote",
                json!({"requested": "secret"}),
                exact_effective,
                contract.clone(),
                json!({"ciphertext": "requested-envelope"}),
                json!({"ciphertext": "effective-envelope"}),
            )
            .unwrap();
        let state = fixture.journal.state().unwrap();
        let tool = &state.tools[lease.id()];
        assert_eq!(tool.effect_contract, contract);
        assert_eq!(tool.effective_input_digest, expected_digest);
        assert!(matches!(
            &tool.effective_input,
            StoredToolInput::Secured { exact_digest, envelope }
                if exact_digest == &expected_digest
                    && envelope == &json!({"ciphertext": "effective-envelope"})
        ));
    }

    #[test]
    fn unknown_tool_requires_explicit_durable_operator_resolution() {
        let fixture = fixture();
        let unknown = fixture
            .scope
            .prepare_tool("provider-call-unknown", 4, "Bash", json!({}), json!({}))
            .unwrap()
            .start()
            .unwrap()
            .unknown(
                ToolUnknownReason::Interrupted,
                json!({"boundary": "post_spawn"}),
            )
            .unwrap();
        let id = unknown.id().to_owned();
        assert!(matches!(
            &fixture.journal.state().unwrap().tools[&id].effect,
            ToolEffectState::Unknown {
                reason: ToolUnknownReason::Interrupted,
                ..
            }
        ));

        unknown
            .resolve(
                ToolResolution::Succeeded {
                    result: json!({"receipt": "operator-confirmed"}),
                },
                ToolResolutionSource::Operator {
                    operator_id: "operator-1".into(),
                },
                json!({"ticket": "INC-1"}),
            )
            .unwrap();
        let state = fixture.journal.state().unwrap();
        let tool = &state.tools[&id];
        assert!(matches!(tool.effect, ToolEffectState::Succeeded));
        assert!(matches!(
            tool.resolution_source,
            Some(ToolResolutionSource::Operator { ref operator_id }) if operator_id == "operator-1"
        ));
        assert_eq!(tool.resolution_evidence, Some(json!({"ticket": "INC-1"})));
        assert!(matches!(
            JournalEffectCoordinator::new(fixture.journal.clone()).resolve_tool(
                id,
                ToolResolution::Failed {
                    error: "duplicate resolution".into(),
                    result: None,
                },
                ToolResolutionSource::Operator {
                    operator_id: "operator-2".into(),
                },
                json!({}),
            ),
            Err(JournalError::InvalidTransition(_))
        ));
    }

    #[test]
    fn filesystem_receipt_is_durable_and_unknown_can_reconcile_not_started() {
        let fixture = fixture();
        let contract = ToolEffectContract {
            kind: ToolEffectKind::FilesystemTransactional,
            reconciler: Some(wcore_tools::effects::FILESYSTEM_EFFECT_RECONCILER.into()),
        };
        let intended = b"new";
        let receipt = json!({
            "version": 1,
            "reconciler": wcore_tools::effects::FILESYSTEM_EFFECT_RECONCILER,
            "path": "/workspace/file.txt",
            "preparation_object": {
                "authority": "in-memory:test",
                "path": "/workspace/file.txt",
                "parent": "in-memory-parent:/workspace"
            },
            "precondition": { "state": "absent" },
            "intended": {
                "sha256": format!("{:x}", Sha256::digest(intended)),
                "len": intended.len()
            },
        });
        let unknown = fixture
            .scope
            .prepare_tool_with_effect_receipt(
                "provider-call-fs",
                8,
                "Write",
                json!({"file_path":"/workspace/file.txt","content":"new"}),
                json!({"file_path":"/workspace/file.txt","content":"new"}),
                contract.clone(),
                receipt.clone(),
            )
            .unwrap()
            .start()
            .unwrap()
            .unknown(
                ToolUnknownReason::Interrupted,
                json!({"boundary":"before_cas"}),
            )
            .unwrap();
        let id = unknown.id().to_owned();
        let prepared = &fixture.journal.state().unwrap().tools[&id];
        assert_eq!(prepared.effect_contract, contract);
        assert_eq!(prepared.effect_receipt, Some(receipt));

        unknown
            .resolve(
                ToolResolution::NotStarted {
                    reason: ToolNotStartedReason::Cancelled {
                        reason: "preimage remained unchanged".into(),
                    },
                },
                ToolResolutionSource::Reconciler {
                    reconciler: wcore_tools::effects::FILESYSTEM_EFFECT_RECONCILER.into(),
                },
                json!({"observed":"preimage"}),
            )
            .unwrap();
        let resolved = &fixture.journal.state().unwrap().tools[&id];
        assert!(matches!(resolved.effect, ToolEffectState::NotStarted));
        assert!(matches!(
            resolved.not_started_reason,
            Some(ToolNotStartedReason::Cancelled { .. })
        ));
    }

    #[test]
    fn malformed_filesystem_receipt_cannot_cross_the_durable_start_boundary() {
        let fixture = fixture();
        let prepared = fixture
            .scope
            .prepare_tool_with_effect_receipt(
                "provider-call-invalid-fs",
                0,
                "Write",
                json!({"file_path":"/workspace/file.txt","content":"new"}),
                json!({"file_path":"/workspace/file.txt","content":"new"}),
                ToolEffectContract {
                    kind: ToolEffectKind::FilesystemTransactional,
                    reconciler: Some(wcore_tools::effects::FILESYSTEM_EFFECT_RECONCILER.into()),
                },
                json!({
                    "version": 1,
                    "reconciler": wcore_tools::effects::FILESYSTEM_EFFECT_RECONCILER,
                    "path": "/workspace/file.txt"
                }),
            )
            .unwrap();
        let id = prepared.id().to_owned();
        assert!(matches!(
            prepared.start(),
            Err(JournalError::InvalidTransition(message))
                if message.contains("malformed effect receipt")
        ));
        assert!(matches!(
            fixture.journal.state().unwrap().tools[&id].effect,
            ToolEffectState::Prepared
        ));
    }

    #[test]
    fn cancellation_and_result_persistence_failure_cannot_false_terminalize() {
        let fixture = fixture();
        let prepared = fixture
            .scope
            .prepare_tool("provider-call-persist", 6, "Remote", json!({}), json!({}))
            .unwrap();
        let id = prepared.id().to_owned();
        assert!(matches!(
            fixture.journal.append(SessionEvent::ToolExecutionUnknown {
                tool_execution_id: id.clone(),
                reason: ToolUnknownReason::ResultPersistenceFailed {
                    error: "not running".into(),
                },
                evidence: json!({}),
            }),
            Err(JournalError::InvalidTransition(_))
        ));

        let running = prepared.start().unwrap();
        assert!(matches!(
            fixture.journal.append(SessionEvent::ToolExecutionFinished {
                tool_execution_id: id.clone(),
                outcome: CompletionOutcome::Cancelled,
                result: json!({}),
            }),
            Err(JournalError::InvalidTransition(_))
        ));
        let unknown = running
            .unknown(
                ToolUnknownReason::ResultPersistenceFailed {
                    error: "disk full".into(),
                },
                json!({"result_digest": "uncommitted"}),
            )
            .unwrap();
        assert_eq!(unknown.id(), id);
        assert!(matches!(
            &fixture.journal.state().unwrap().tools[&id].effect,
            ToolEffectState::Unknown {
                reason: ToolUnknownReason::ResultPersistenceFailed { error },
                ..
            } if error == "disk full"
        ));
    }

    #[test]
    fn opaque_error_is_durable_as_ambiguous_unknown() {
        let fixture = fixture();
        let reason = ToolUnknownReason::AmbiguousFailure {
            error: "remote returned an unproven error".into(),
        };
        assert_eq!(
            serde_json::to_value(&reason).unwrap()["kind"],
            "ambiguous_failure"
        );
        let unknown = fixture
            .scope
            .prepare_tool("provider-call-opaque", 8, "Plugin", json!({}), json!({}))
            .unwrap()
            .start()
            .unwrap()
            .unknown(reason.clone(), json!({"adapter": "opaque"}))
            .unwrap();
        assert!(matches!(
            &fixture.journal.state().unwrap().tools[unknown.id()].effect,
            ToolEffectState::Unknown {
                reason: ToolUnknownReason::AmbiguousFailure { error },
                evidence,
            } if error == "remote returned an unproven error"
                && evidence == &json!({"adapter": "opaque"})
        ));
    }

    #[test]
    fn typed_effect_denials_do_not_fabricate_start() {
        let fixture = fixture();
        let tool = fixture
            .scope
            .prepare_tool("provider-call-1", 0, "Bash", json!({}), json!({}))
            .unwrap();
        let tool_id = tool.id().to_owned();
        tool.not_started(ToolNotStartedReason::PolicyDenied {
            policy: "workspace policy".into(),
        })
        .unwrap();

        let child = fixture
            .scope
            .prepare_child("child-1", json!({"task": "inspect"}))
            .unwrap();
        child
            .not_started(ChildNotStartedReason::ApprovalDenied {
                approval_id: "approval-1".into(),
            })
            .unwrap();

        let delivery = fixture
            .scope
            .prepare_delivery("channel:user", json!({"text": "blocked"}))
            .unwrap();
        let delivery_id = delivery.id().to_owned();
        delivery
            .not_started(DeliveryNotStartedReason::PolicyDenied {
                policy: "offline".into(),
            })
            .unwrap();

        let state = fixture.journal.state().unwrap();
        assert!(matches!(
            &state.tools[&tool_id].effect,
            ToolEffectState::NotStarted
        ));
        assert!(matches!(
            &state.children["child-1"].effect,
            ExternalEffectState::NotStarted
        ));
        assert!(matches!(
            &state.deliveries[&delivery_id].effect,
            ExternalEffectState::NotStarted
        ));
    }

    #[test]
    fn dispatcher_not_started_reasons_serialize_and_reduce_without_running() {
        let fixture = fixture();
        let cases = [
            (
                ToolNotStartedReason::HookDenied {
                    reason: "pre-hook denied".into(),
                },
                "hook_denied",
            ),
            (
                ToolNotStartedReason::BudgetDenied {
                    reason: "tool cap reached".into(),
                },
                "budget_denied",
            ),
            (ToolNotStartedReason::CircuitOpen, "circuit_open"),
            (ToolNotStartedReason::UnknownTool, "unknown_tool"),
        ];

        for (ordinal, (reason, expected_kind)) in cases.into_iter().enumerate() {
            let encoded = serde_json::to_value(&reason).unwrap();
            assert_eq!(encoded["kind"], expected_kind);
            let lease = fixture
                .scope
                .prepare_tool(
                    format!("provider-call-not-started-{ordinal}"),
                    ordinal as u64 + 20,
                    "Fixture",
                    json!({}),
                    json!({}),
                )
                .unwrap();
            let id = lease.id().to_owned();
            lease.not_started(reason.clone()).unwrap();
            let state = fixture.journal.state().unwrap();
            let tool = &state.tools[&id];
            assert_eq!(tool.not_started_reason, Some(reason));
            assert!(matches!(tool.effect, ToolEffectState::NotStarted));
        }
    }

    #[test]
    fn explicit_scope_carries_approval_budget_child_and_delivery_authority() {
        fn assert_send_sync_clone<T: Send + Sync + Clone>() {}
        assert_send_sync_clone::<TurnEffectScope>();

        let fixture = fixture();
        let approval = fixture
            .scope
            .request_approval(&json!({"tool": "Read"}))
            .unwrap();
        approval
            .resolve(ApprovalResolution::Decided {
                decision: ApprovalDecision::AllowOnce,
            })
            .unwrap();

        fixture
            .scope
            .reserve_budget(
                BudgetPurpose::ToolExecution,
                BudgetAmount {
                    value: 10,
                    unit: BudgetUnit::ToolCalls,
                },
            )
            .unwrap()
            .settle(BudgetAmount {
                value: 1,
                unit: BudgetUnit::ToolCalls,
            })
            .unwrap();

        let child = fixture
            .scope
            .clone()
            .prepare_child("child-1", json!({"task": "inspect"}))
            .unwrap()
            .start()
            .unwrap()
            .leave_unknown();
        assert_eq!(child.id(), "child-1");

        fixture
            .scope
            .prepare_delivery("channel:user", json!({"text": "hello"}))
            .unwrap()
            .start()
            .unwrap()
            .unknown(
                DeliveryUnknownReason::AcknowledgementMissing,
                DeliveryEvidence {
                    last_observed_stage: DeliveryStage::PayloadSent,
                    detail: Some("transport returned without acknowledgement".into()),
                },
            )
            .unwrap();

        let state = fixture.journal.state().unwrap();
        assert_eq!(state.approvals.len(), 1);
        assert_eq!(state.budgets.len(), 1);
        assert!(matches!(
            &state.children["child-1"].effect,
            ExternalEffectState::Unknown
        ));
        assert!(state.deliveries.values().any(|delivery| matches!(
            &delivery.completion,
            Some(DeliveryCompletion::Unknown { .. })
        )));
    }

    #[test]
    fn missing_turn_fails_before_returning_an_effect_lease() {
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        let scope = JournalEffectCoordinator::new(journal.clone()).for_turn("missing-turn");

        assert!(
            scope
                .prepare_tool("provider-call", 0, "Read", json!({}), json!({}))
                .is_err()
        );
        assert!(journal.state().unwrap().tools.is_empty());
    }

    #[test]
    fn hook_phase_lease_requires_explicit_started_and_finished_transitions() {
        let fixture = fixture();
        let digest = "a".repeat(64);
        let slots = vec![HookManifestSlot {
            ordinal: 0,
            slot_id: "slot-0".into(),
            source: crate::session_journal::HookSlotSource::Rust,
            descriptor_digest: digest.clone(),
        }];
        let manifest_digest = state_payload_digest(&serde_json::to_value(&slots).unwrap()).unwrap();
        let prepared = fixture
            .scope
            .prepare_hook_phase(
                "provider-call-hook",
                0,
                ToolHookPhase::PreToolUse,
                None,
                digest.clone(),
                digest.clone(),
                manifest_digest,
                slots.clone(),
            )
            .unwrap();
        let phase_id = prepared.id().to_owned();
        prepared
            .start(None)
            .unwrap()
            .finish(
                Some(digest.clone()),
                digest.clone(),
                state_payload_digest(
                    &serde_json::to_value(vec![HookSlotReceipt {
                        ordinal: 0,
                        slot_id: "slot-0".into(),
                        descriptor_digest: digest,
                        status: crate::session_journal::HookSlotTerminalStatus::Completed,
                    }])
                    .unwrap(),
                )
                .unwrap(),
                vec![HookSlotReceipt {
                    ordinal: 0,
                    slot_id: "slot-0".into(),
                    descriptor_digest: "a".repeat(64),
                    status: crate::session_journal::HookSlotTerminalStatus::Completed,
                }],
            )
            .unwrap();

        assert!(matches!(
            fixture.journal.state().unwrap().hook_phases[&phase_id].state,
            crate::session_journal::HookPhaseState::Finished { .. }
        ));
    }
}
