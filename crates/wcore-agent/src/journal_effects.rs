//! Durable journal coordination for externally visible turn effects.
//!
//! Callers must carry an explicit [`TurnEffectScope`] into spawned tasks. A
//! prepared lease is returned only after the intent is durable. Consuming that
//! lease records either the physical start or a typed not-started outcome, so
//! policy denial never needs a fabricated start event.

use serde_json::Value;

use crate::session_journal::{
    ApprovalOrigin, ApprovalResolution, BudgetAmount, BudgetOwner, BudgetPurpose,
    ChildNotStartedReason, CompletionOutcome, DeliveryCompletion, DeliveryEvidence,
    DeliveryNotStartedReason, DeliveryOrigin, DeliveryUnknownReason, JournalError, SessionEvent,
    SessionJournal, ToolNotStartedReason, state_payload_digest,
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
        let approval_id = new_id("approval");
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

    pub fn prepare_tool(
        &self,
        provider_call_id: impl Into<String>,
        ordinal: u64,
        tool: impl Into<String>,
        requested_input: Value,
        effective_input: Value,
    ) -> Result<PreparedToolLease, JournalError> {
        let tool_execution_id = new_id("tool-execution");
        let requested_input_digest = state_payload_digest(&requested_input)?;
        let effective_input_digest = state_payload_digest(&effective_input)?;
        self.coordinator
            .journal
            .append(SessionEvent::ToolIntentRecorded {
                tool_execution_id: tool_execution_id.clone(),
                provider_call_id: provider_call_id.into(),
                turn_id: self.turn_id.clone(),
                ordinal,
                tool: tool.into(),
                requested_input,
                requested_input_digest,
                effective_input,
                effective_input_digest,
            })?;
        Ok(PreparedToolLease {
            journal: self.coordinator.journal.clone(),
            tool_execution_id,
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
#[must_use = "a prepared tool must be started or terminalized as not started"]
pub struct PreparedToolLease {
    journal: SessionJournal,
    tool_execution_id: String,
}

impl PreparedToolLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.tool_execution_id
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
}

impl StartedToolLease {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.tool_execution_id
    }

    pub fn finish(self, outcome: CompletionOutcome, result: Value) -> Result<(), JournalError> {
        self.journal.append(SessionEvent::ToolExecutionFinished {
            tool_execution_id: self.tool_execution_id,
            outcome,
            result,
        })?;
        Ok(())
    }

    /// Consume the runtime lease while preserving the durable `Unknown` state
    /// established by `ToolExecutionStarted`. No second event is required.
    #[must_use]
    pub fn leave_unknown(self) -> UnknownEffect {
        UnknownEffect {
            effect_id: self.tool_execution_id,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_journal::{
        ApprovalDecision, BudgetUnit, DeliveryStage, ExternalEffectState,
    };
    use serde_json::json;

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
        assert_ne!(execution_id, "provider-call-1");
        assert!(matches!(
            &fixture.journal.state().unwrap().tools[&execution_id].effect,
            ExternalEffectState::Prepared
        ));

        let started = prepared.start().unwrap();
        assert!(matches!(
            &fixture.journal.state().unwrap().tools[&execution_id].effect,
            ExternalEffectState::Unknown
        ));
        started
            .finish(CompletionOutcome::Succeeded, json!({"ok": true}))
            .unwrap();
        assert!(matches!(
            &fixture.journal.state().unwrap().tools[&execution_id].effect,
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Succeeded
            }
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
            ExternalEffectState::NotStarted
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
}
