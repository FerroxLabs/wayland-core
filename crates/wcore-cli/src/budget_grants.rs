use std::collections::HashMap;

use wcore_protocol::commands::ContinueWithBudgetCommand;
#[cfg(test)]
use wcore_protocol::events::BudgetGrantOutcome;
use wcore_protocol::events::{BudgetGrantRefusalReason, BudgetGrantResult};

/// Maximum correlated grant results retained for one JSON-stream session.
///
/// Entries are never evicted: forgetting a request ID could apply a delayed
/// replay twice. Once full, the ledger refuses new IDs for the rest of the
/// session instead.
pub const MAX_BUDGET_GRANT_LEDGER_ENTRIES: usize = 1_024;

/// A result plus its authoritative cached JSON representation.
///
/// Identical replay returns the exact stored bytes. The runtime emits the
/// corresponding typed result through the protocol writer.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetGrantEmission {
    result: BudgetGrantResult,
    result_bytes: Vec<u8>,
}

impl BudgetGrantEmission {
    pub fn result(&self) -> &BudgetGrantResult {
        &self.result
    }

    pub fn into_result(self) -> BudgetGrantResult {
        self.result
    }

    pub fn result_bytes(&self) -> &[u8] {
        &self.result_bytes
    }
}

/// Session-local at-most-once ledger for host-authorized budget grants.
///
/// An identical request ID replays its cached result bytes without invoking
/// the durable mutation again; conflicting reuse fails closed without
/// replacing the authoritative entry.
#[derive(Default)]
pub struct BudgetGrantLedger {
    entries: HashMap<String, BudgetGrantEntry>,
}

struct BudgetGrantEntry {
    command: ContinueWithBudgetCommand,
    emission: BudgetGrantEmission,
}

impl BudgetGrantLedger {
    pub fn complete<F>(
        &mut self,
        command: ContinueWithBudgetCommand,
        apply: F,
    ) -> BudgetGrantEmission
    where
        F: FnOnce(&ContinueWithBudgetCommand) -> Result<(), BudgetGrantRefusalReason>,
    {
        if let Some(emission) = self.replay_or_conflict(&command) {
            return emission;
        }
        if self.entries.len() >= MAX_BUDGET_GRANT_LEDGER_ENTRIES {
            return emission_for(
                &command,
                Err(BudgetGrantRefusalReason::LedgerCapacityExceeded),
            );
        }

        let emission = terminal_emission(&command, apply(&command));
        self.insert(command, emission.clone());
        emission
    }

    fn insert(&mut self, command: ContinueWithBudgetCommand, emission: BudgetGrantEmission) {
        self.entries.insert(
            command.request_id.clone(),
            BudgetGrantEntry { command, emission },
        );
    }

    fn replay_or_conflict(
        &self,
        command: &ContinueWithBudgetCommand,
    ) -> Option<BudgetGrantEmission> {
        let entry = self.entries.get(&command.request_id)?;
        if entry.command == *command {
            Some(entry.emission.clone())
        } else {
            Some(emission_for(
                command,
                Err(BudgetGrantRefusalReason::RequestIdConflict),
            ))
        }
    }
}

fn terminal_emission(
    command: &ContinueWithBudgetCommand,
    result: Result<(), BudgetGrantRefusalReason>,
) -> BudgetGrantEmission {
    emission_for(command, result)
}

fn emission_for(
    command: &ContinueWithBudgetCommand,
    outcome: Result<(), BudgetGrantRefusalReason>,
) -> BudgetGrantEmission {
    let result = match outcome {
        Ok(()) => BudgetGrantResult::granted(
            command.request_id.clone(),
            command.additional_tokens,
            command.additional_cost_usd,
        ),
        Err(reason) => BudgetGrantResult::refused(
            command.request_id.clone(),
            command.additional_tokens,
            command.additional_cost_usd,
            reason,
        ),
    };
    // These values contain only validated strings, integers, finite floats,
    // and closed enums, so JSON serialization cannot fail.
    let result_bytes = serde_json::to_vec(&result)
        .expect("validated budget grant result must always serialize as JSON");
    BudgetGrantEmission {
        result,
        result_bytes,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn grant(request_id: &str, tokens: u64) -> ContinueWithBudgetCommand {
        ContinueWithBudgetCommand {
            request_id: request_id.into(),
            additional_tokens: tokens,
            additional_cost_usd: 0.0,
        }
    }

    #[test]
    fn identical_completed_request_replays_exact_bytes_without_double_grant() {
        let mut ledger = BudgetGrantLedger::default();
        let calls = Cell::new(0);
        let command = grant("budget-001", 10);

        let first = ledger.complete(command.clone(), |_| {
            calls.set(calls.get() + 1);
            Ok(())
        });
        let replay = ledger.complete(command, |_| {
            calls.set(calls.get() + 1);
            Ok(())
        });

        assert_eq!(calls.get(), 1);
        assert_eq!(first.result_bytes(), replay.result_bytes());
        assert_eq!(first, replay);
        assert_eq!(first.result().outcome, BudgetGrantOutcome::Granted);
    }

    #[test]
    fn conflicting_request_id_reuse_is_refused_without_mutation() {
        let mut ledger = BudgetGrantLedger::default();
        let calls = Cell::new(0);
        ledger.complete(grant("budget-001", 10), |_| {
            calls.set(calls.get() + 1);
            Ok(())
        });

        let conflict = ledger.complete(grant("budget-001", 20), |_| {
            calls.set(calls.get() + 1);
            Ok(())
        });

        assert_eq!(calls.get(), 1);
        assert_eq!(conflict.result().outcome, BudgetGrantOutcome::Refused);
        assert_eq!(
            conflict.result().refusal_reason,
            Some(BudgetGrantRefusalReason::RequestIdConflict)
        );
    }

    #[test]
    fn active_turn_refusal_replays_and_fresh_id_grants_once() {
        let mut ledger = BudgetGrantLedger::default();
        let calls = Cell::new(0);
        let command = grant("budget-001", 10);
        let refused = ledger.complete(command.clone(), |_| {
            Err(BudgetGrantRefusalReason::TurnInProgress)
        });
        let replay = ledger.complete(command, |_| {
            calls.set(calls.get() + 1);
            Ok(())
        });

        assert_eq!(
            refused.result().refusal_reason,
            Some(BudgetGrantRefusalReason::TurnInProgress)
        );
        assert_eq!(calls.get(), 0);
        assert_eq!(refused.result_bytes(), replay.result_bytes());

        let fresh_command = grant("budget-002", 10);
        let granted = ledger.complete(fresh_command.clone(), |_| {
            calls.set(calls.get() + 1);
            Ok(())
        });
        let granted_replay = ledger.complete(fresh_command, |_| {
            calls.set(calls.get() + 1);
            Ok(())
        });

        assert_eq!(calls.get(), 1);
        assert_eq!(granted.result().outcome, BudgetGrantOutcome::Granted);
        assert_eq!(granted.result_bytes(), granted_replay.result_bytes());
    }

    #[test]
    fn ledger_overload_refuses_without_applying() {
        let mut ledger = BudgetGrantLedger::default();
        for index in 0..MAX_BUDGET_GRANT_LEDGER_ENTRIES {
            ledger.complete(grant(&format!("budget-{index}"), 1), |_| Ok(()));
        }
        let calls = Cell::new(0);

        let refused = ledger.complete(grant("budget-overload", 1), |_| {
            calls.set(calls.get() + 1);
            Ok(())
        });

        assert_eq!(calls.get(), 0);
        assert_eq!(
            refused.result().refusal_reason,
            Some(BudgetGrantRefusalReason::LedgerCapacityExceeded)
        );
    }
}
