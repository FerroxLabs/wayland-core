//! Projecting the reduced Goal state onto the host protocol (F22-C1).
//!
//! ## Why this is a conversion and not the reduced state itself
//!
//! `goal status` prints the reduced `GoalState` verbatim, on the stated
//! principle that "a surface that renders its own shape is a surface that can
//! disagree with the chain" (`goal_cmd.rs:577-580`). The protocol surface cannot
//! do the same thing, for a structural reason rather than a preference:
//! `GoalState` is defined in `wcore-agent`, `wcore-agent` depends on
//! `wcore-protocol`, and the dependency cannot run both ways. So the wire needs
//! its own types and this module is the single conversion between them.
//!
//! That reintroduces exactly the disagreement risk the CLI avoided, so it is
//! answered twice rather than argued away:
//!
//! 1. **`state_digest`** is taken over the canonical JSON of the FULL reduced
//!    `GoalState` — including the attempt history the wire summarises. A host can
//!    always tell which chain state its view came from, and can tell two
//!    different chain states apart even when the summarised fields match.
//! 2. **[`tests::the_projection_carries_every_field_of_the_reduced_goal_state`]**
//!    enumerates `GoalState`'s serialized keys at runtime and fails when one is
//!    neither projected nor listed as deliberately withheld. Add a field to
//!    `GoalState` and forget this module, and that test goes red rather than the
//!    wire quietly rotting.

use wcore_protocol::events::{ProtocolEvent, RecoveryCursor};
use wcore_protocol::goal::{
    GOAL_PROTOCOL_VERSION, GoalAuthorityWire, GoalLifecycleWire, GoalLoopOwnerWire, GoalProjection,
    GoalTaskWire, GoalTaskWireStatus, GoalTransitionKind,
};

use crate::session_journal::{
    GoalLifecycle, GoalState, GoalTaskAttemptStatus, GoalTaskState, JournalEnvelope, JournalError,
    SessionEvent, replay_state, state_payload_digest,
};

use super::record::GoalAuthorityRecord;

/// Fields of `GoalState` that are deliberately not carried on the v1 wire.
///
/// Empty today. It exists so that withholding a field is an explicit, reviewable
/// act rather than an omission the coverage test cannot distinguish from a bug,
/// and it lives HERE rather than inside the test module so anyone editing
/// [`goal_projection`] meets it in the same screen.
///
/// `cfg(test)` because the coverage test is its only consumer — the declaration
/// is the record, the test is the enforcement.
#[cfg(test)]
const DELIBERATELY_NOT_ON_THE_WIRE: &[&str] = &[];

/// Digest over the canonical JSON of the complete reduced Goal state.
///
/// Uses the journal's own canonicalizer rather than a second one, for the reason
/// `record.rs:82-86` gives: two canonicalizations over one journal are
/// guaranteed to drift from each other.
pub fn goal_state_digest(state: &GoalState) -> Result<String, JournalError> {
    state_payload_digest(&serde_json::to_value(state).map_err(|error| {
        JournalError::InvalidTransition(format!("goal state is not serializable: {error}"))
    })?)
}

/// Project one durable task onto its wire summary.
///
/// The status is DERIVED from the attempt history and the Goal's dependency
/// graph, in the order the ledger itself settles them: a durable completion
/// outranks a live claim, and an unresolved outcome outranks everything except a
/// completion, because `requires_resolution` is what stops a silent retry.
fn task_wire(goal: &GoalState, task: &GoalTaskState) -> GoalTaskWire {
    let status = if let Some(completion) = &task.completion {
        if completion.delivered {
            GoalTaskWireStatus::Completed
        } else {
            GoalTaskWireStatus::CompletedUndelivered
        }
    } else if task.requires_resolution() {
        GoalTaskWireStatus::NeedsResolution
    } else if task.live_attempt().is_some() {
        GoalTaskWireStatus::Running
    } else if matches!(
        task.attempts.last().map(|attempt| &attempt.status),
        Some(GoalTaskAttemptStatus::Revoked { .. })
    ) {
        GoalTaskWireStatus::Revoked
    } else if goal.dependencies_met(task) {
        GoalTaskWireStatus::Claimable
    } else {
        GoalTaskWireStatus::Blocked
    };

    GoalTaskWire {
        task_id: task.task_id.clone(),
        depends_on: task.depends_on.clone(),
        idempotency_key: task.idempotency_key.clone(),
        status,
        epoch: task.epoch(),
        attempts: u32::try_from(task.attempts.len()).unwrap_or(u32::MAX),
        outcome: task
            .completion
            .as_ref()
            .map(|completion| completion.outcome.clone()),
        dependency_releases: task.dependency_releases,
        last_transition_seq: task.last_transition_seq,
    }
}

fn authority_wire(record: &GoalAuthorityRecord) -> GoalAuthorityWire {
    GoalAuthorityWire {
        effective_limits: record.effective_limits.clone(),
        strategy: record.strategy,
        loop_policy: record.loop_policy.clone(),
        parent_envelope_digest: record.parent_envelope_digest.clone(),
        snapshot_digest: record.snapshot_digest.clone(),
    }
}

fn lifecycle_wire(lifecycle: &GoalLifecycle) -> GoalLifecycleWire {
    match lifecycle {
        GoalLifecycle::Opened => GoalLifecycleWire::Opened,
        GoalLifecycle::Running => GoalLifecycleWire::Running,
        GoalLifecycle::Waiting { wait } => GoalLifecycleWire::Waiting { wait: wait.clone() },
        GoalLifecycle::Terminated { terminal } => GoalLifecycleWire::Terminated {
            terminal: terminal.clone(),
        },
    }
}

/// Project the reduced Goal state onto the host wire.
///
/// Task order is the ledger's own `BTreeMap` order — deterministic by task id —
/// so two hosts replaying the same chain render the same sequence.
#[must_use]
pub fn goal_projection(state: &GoalState) -> GoalProjection {
    GoalProjection {
        goal_id: state.goal_id.clone(),
        objective: state.objective.clone(),
        authority: authority_wire(&state.authority),
        lifecycle: lifecycle_wire(&state.lifecycle),
        iterations_started: state.iterations_started,
        iteration_ceiling: state.authority.iteration_ceiling(),
        resume_count: state.resume_count,
        opened_at_unix_ms: state.opened_at_unix_ms,
        cursor: state.cursor(),
        tasks: state
            .tasks
            .values()
            .map(|task| task_wire(state, task))
            .collect(),
        loop_owner: state.loop_owner.as_ref().map(|owner| GoalLoopOwnerWire {
            strategy: owner.strategy,
            epoch: owner.epoch,
            lease_expires_unix_ms: owner.lease_expires_unix_ms,
        }),
        loop_owner_epochs: state.loop_owner_epochs,
    }
}

/// The complete host-observable snapshot event for one Goal.
pub fn goal_snapshot_event(
    session_id: &str,
    state: &GoalState,
) -> Result<ProtocolEvent, JournalError> {
    Ok(ProtocolEvent::GoalSnapshot {
        goal_version: GOAL_PROTOCOL_VERSION,
        session_id: session_id.to_owned(),
        goal_id: state.goal_id.clone(),
        cursor: state.cursor(),
        state_digest: goal_state_digest(state)?,
        goal: goal_projection(state),
    })
}

/// One durable transition milestone for one Goal.
///
/// `cursor` is the Goal's cursor AFTER the transition, which is what the kernel
/// returns from every write path, so a host correlating a transition with the
/// snapshot that follows it sees the same position on both.
#[must_use]
pub fn goal_transition_event(
    session_id: &str,
    goal_id: &str,
    cursor: RecoveryCursor,
    transition: GoalTransitionKind,
    lifecycle: &GoalLifecycle,
) -> ProtocolEvent {
    ProtocolEvent::GoalTransition {
        goal_version: GOAL_PROTOCOL_VERSION,
        session_id: session_id.to_owned(),
        goal_id: goal_id.to_owned(),
        cursor,
        transition,
        lifecycle: lifecycle_wire(lifecycle),
    }
}

/// Render one protocol event as a single JSON-stream line.
///
/// The host protocol is JSON Lines; this is the same one-value-one-LF framing
/// `wcore_protocol::writer` uses, reproduced here only so a CLI verb can emit
/// without standing up a full protocol session.
pub fn event_line(event: &ProtocolEvent) -> Result<String, JournalError> {
    serde_json::to_string(event).map_err(|error| {
        JournalError::InvalidTransition(format!("goal event is not serializable: {error}"))
    })
}

/// Which wire transition, if any, one journal envelope represents for `goal_id`.
///
/// Returns `None` for every event that is not a Goal-level transition for this
/// Goal — including `GoalTaskDeclared` and `GoalTaskTransitioned`, which move
/// the task ledger rather than the Goal's own lifecycle and are observed
/// through the next snapshot's task summaries instead.
fn transition_kind_for(event: &SessionEvent, goal_id: &str) -> Option<GoalTransitionKind> {
    let (id, kind) = match event {
        SessionEvent::GoalOpened { goal_id, .. } => (goal_id, GoalTransitionKind::Opened),
        SessionEvent::GoalIterationStarted { goal_id, .. } => {
            (goal_id, GoalTransitionKind::IterationStarted)
        }
        SessionEvent::GoalWaitBegun { goal_id, .. } => (goal_id, GoalTransitionKind::WaitBegun),
        SessionEvent::GoalWaitResolved { goal_id } => (goal_id, GoalTransitionKind::WaitResolved),
        SessionEvent::GoalRunResumed { goal_id, .. } => (goal_id, GoalTransitionKind::RunResumed),
        SessionEvent::GoalLoopOwnerClaimed { goal_id, .. } => {
            (goal_id, GoalTransitionKind::LoopOwnerClaimed)
        }
        SessionEvent::GoalLoopOwnerFinished { goal_id, .. } => {
            (goal_id, GoalTransitionKind::LoopOwnerFinished)
        }
        SessionEvent::GoalTerminated { goal_id, .. } => (goal_id, GoalTransitionKind::Terminated),
        _ => return None,
    };
    (id == goal_id).then_some(kind)
}

/// The complete ordered producer stream for one Goal, replayed from the chain.
///
/// This is the serialized sequence a host replays: every durable Goal-level
/// transition in journal order, each at the cursor it landed at, followed by the
/// current snapshot.
///
/// ## Why the lifecycle is folded through the real reducer
///
/// Each transition reports the lifecycle AFTER it, and deriving that from the
/// transition kind alone would be a guess for `RunResumed` and
/// `LoopOwnerClaimed` — neither of which determines a lifecycle by itself. So
/// this folds the journal prefix through [`replay_state`], the SAME reducer that
/// produces every other view of the chain. A second lifecycle rule beside the
/// reducer is the parallel lifecycle Phase 22 exists to remove, and it would be
/// the third one in this file if it were written here.
///
/// The prefix fold is quadratic in the number of Goal transitions. That is a
/// deliberate trade for correctness on a stream that is bounded by a Goal's loop
/// ceiling, not by session length.
pub fn goal_stream(
    session_id: &str,
    goal_id: &str,
    envelopes: &[JournalEnvelope],
) -> Result<Vec<ProtocolEvent>, JournalError> {
    let mut events = Vec::new();
    for (index, envelope) in envelopes.iter().enumerate() {
        let Some(kind) = transition_kind_for(&envelope.event, goal_id) else {
            continue;
        };
        let state = replay_state(&envelopes[..=index])?;
        let Some(goal) = state.goals.get(goal_id) else {
            return Err(JournalError::InvalidTransition(format!(
                "goal {goal_id} has a transition at seq {} but no reduced state",
                envelope.seq
            )));
        };
        events.push(goal_transition_event(
            session_id,
            goal_id,
            RecoveryCursor {
                journal_sequence: Some(envelope.seq),
                journal_digest: envelope.checksum.clone(),
            },
            kind,
            &goal.lifecycle,
        ));
    }

    let final_state = replay_state(envelopes)?;
    if let Some(goal) = final_state.goals.get(goal_id) {
        events.push(goal_snapshot_event(session_id, goal)?);
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use serde_json::json;

    use super::*;
    use crate::session_journal::{GoalTaskAttempt, GoalTaskCompletion};
    use wcore_types::goal::{
        ExhaustionKind, GoalAuthorityRequest, GoalStrategy, GoalTerminalState, LoopPolicy,
        TaskUnknownReason, WaitKind, resolve_goal_authority,
    };

    /// Build the record through the real kernel path, not by hand, so the
    /// committed digest is the one production would have written.
    fn authority(loop_policy: LoopPolicy) -> GoalAuthorityRecord {
        let limits = BTreeMap::from([("max_tokens".to_owned(), 10_000_u64)]);
        let snapshot = resolve_goal_authority(
            &GoalAuthorityRequest {
                requested_limits: limits.clone(),
                strategy: GoalStrategy::Fleet,
                loop_policy,
            },
            &limits,
            "wayland-core-goal-fleet/v1",
        );
        GoalAuthorityRecord::from_snapshot(&snapshot)
    }

    fn goal(tasks: BTreeMap<String, GoalTaskState>) -> GoalState {
        GoalState {
            goal_id: "goal-001".to_owned(),
            objective: "ship it".to_owned(),
            authority: authority(LoopPolicy::Fixed { iterations: 8 }),
            lifecycle: GoalLifecycle::Running,
            iterations_started: 3,
            resume_count: 1,
            opened_at_unix_ms: 1_721_000_000_000,
            last_transition_seq: 22,
            last_transition_checksum: "sha256:cursor".to_owned(),
            tasks,
            loop_owner: None,
            loop_owner_epochs: 0,
        }
    }

    fn task(
        id: &str,
        depends_on: &[&str],
        attempts: Vec<GoalTaskAttempt>,
        completion: Option<GoalTaskCompletion>,
    ) -> GoalTaskState {
        GoalTaskState {
            task_id: id.to_owned(),
            depends_on: depends_on.iter().map(|d| (*d).to_owned()).collect(),
            idempotency_key: format!("idem-{id}"),
            attempts,
            completion,
            handoffs: Vec::new(),
            dependency_releases: 0,
            last_transition_seq: 18,
            last_transition_checksum: "sha256:task".to_owned(),
        }
    }

    fn attempt(epoch: u64, status: GoalTaskAttemptStatus) -> GoalTaskAttempt {
        GoalTaskAttempt {
            epoch,
            worker_id: "worker-1".to_owned(),
            budget_reservation_id: "res-1".to_owned(),
            lease_expires_unix_ms: 1_721_000_060_000,
            last_liveness_unix_ms: None,
            status,
        }
    }

    /// THE guard this module exists to carry.
    ///
    /// Enumerates the serialized keys of a FULLY populated `GoalState` — every
    /// `skip_serializing_if` field present, so none is silently absent — and
    /// asserts each is either represented on the wire or explicitly withheld.
    /// Without this the mirror rots the first time `GoalState` grows a field.
    #[test]
    fn the_projection_carries_every_field_of_the_reduced_goal_state() {
        let mut state = goal(BTreeMap::from([(
            "task-a".to_owned(),
            task(
                "task-a",
                &[],
                vec![attempt(1, GoalTaskAttemptStatus::Live)],
                None,
            ),
        )]));
        state.loop_owner = Some(crate::session_journal::GoalLoopOwner {
            strategy: GoalStrategy::Fleet,
            epoch: 1,
            lease_expires_unix_ms: 1_721_000_060_000,
        });
        state.loop_owner_epochs = 1;

        let reduced = serde_json::to_value(&state).expect("reduced state serializes");
        let reduced_keys = reduced
            .as_object()
            .expect("reduced state is an object")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        // Every optional field must actually be present, or this test would
        // pass by measuring a smaller struct than the real one.
        for expected in ["tasks", "loop_owner", "loop_owner_epochs"] {
            assert!(
                reduced_keys.contains(expected),
                "fixture is not fully populated: {expected} absent from {reduced_keys:?}"
            );
        }

        let projected =
            serde_json::to_value(goal_projection(&state)).expect("projection serializes");
        let projected_keys = projected
            .as_object()
            .expect("projection is an object")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();

        // `last_transition_seq` and `last_transition_checksum` are carried, but
        // as the protocol's existing `cursor` shape rather than under their own
        // names — two cursor definitions over one journal would drift.
        let carried_as_cursor: BTreeSet<String> =
            ["last_transition_seq", "last_transition_checksum"]
                .into_iter()
                .map(str::to_owned)
                .collect();
        assert!(
            projected_keys.contains("cursor"),
            "cursor must be projected"
        );

        let withheld: BTreeSet<String> = DELIBERATELY_NOT_ON_THE_WIRE
            .iter()
            .map(|field| (*field).to_owned())
            .collect();

        let unrepresented = reduced_keys
            .iter()
            .filter(|key| {
                !projected_keys.contains(*key)
                    && !carried_as_cursor.contains(*key)
                    && !withheld.contains(*key)
            })
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            unrepresented.is_empty(),
            "GoalState fields absent from the wire projection and not declared withheld: \
             {unrepresented:?}. Add them to GoalProjection or to \
             DELIBERATELY_NOT_ON_THE_WIRE with a reason."
        );
    }

    #[test]
    fn a_completion_that_was_never_observed_is_not_reported_as_completed() {
        // The outbox distinction. A restarted parent drains completions that
        // exist but were never delivered; a wire that renders both as
        // `completed` makes that queue invisible.
        let undelivered = GoalTaskCompletion {
            epoch: 1,
            outcome: GoalTerminalState::SelfChecked,
            effect_digest: "sha256:effect".to_owned(),
            delivered: false,
        };
        let delivered = GoalTaskCompletion {
            delivered: true,
            ..undelivered.clone()
        };
        let state = goal(BTreeMap::from([
            (
                "a".to_owned(),
                task(
                    "a",
                    &[],
                    vec![attempt(1, GoalTaskAttemptStatus::Completed)],
                    Some(undelivered),
                ),
            ),
            (
                "b".to_owned(),
                task(
                    "b",
                    &[],
                    vec![attempt(1, GoalTaskAttemptStatus::Completed)],
                    Some(delivered),
                ),
            ),
        ]));
        let projected = goal_projection(&state);
        assert_eq!(
            projected.tasks[0].status,
            GoalTaskWireStatus::CompletedUndelivered
        );
        assert_eq!(projected.tasks[1].status, GoalTaskWireStatus::Completed);
    }

    #[test]
    fn an_unestablished_outcome_is_never_projected_as_a_failure_or_as_claimable() {
        // `GoalTaskAttemptStatus::Unknown` is deliberately not a kind of
        // failure. Projecting it as claimable would let a host build the
        // silent retry the ledger refuses to build.
        let state = goal(BTreeMap::from([(
            "a".to_owned(),
            task(
                "a",
                &[],
                vec![attempt(
                    1,
                    GoalTaskAttemptStatus::Unknown {
                        reason: TaskUnknownReason::OwnerDiedMidAttempt,
                    },
                )],
                None,
            ),
        )]));
        let projected = goal_projection(&state);
        assert_eq!(
            projected.tasks[0].status,
            GoalTaskWireStatus::NeedsResolution
        );
        assert_eq!(projected.tasks[0].outcome, None);
    }

    #[test]
    fn a_task_with_an_unmet_dependency_projects_blocked_not_claimable() {
        let state = goal(BTreeMap::from([
            ("a".to_owned(), task("a", &[], Vec::new(), None)),
            ("b".to_owned(), task("b", &["a"], Vec::new(), None)),
        ]));
        let projected = goal_projection(&state);
        assert_eq!(projected.tasks[0].task_id, "a");
        assert_eq!(projected.tasks[0].status, GoalTaskWireStatus::Claimable);
        assert_eq!(projected.tasks[1].task_id, "b");
        assert_eq!(projected.tasks[1].status, GoalTaskWireStatus::Blocked);
    }

    #[test]
    fn a_manual_loop_projects_no_ceiling_rather_than_a_bound_of_zero() {
        let mut state = goal(BTreeMap::new());
        state.authority = authority(LoopPolicy::Manual);
        assert_eq!(goal_projection(&state).iteration_ceiling, None);

        state.authority = authority(LoopPolicy::Fixed { iterations: 8 });
        assert_eq!(goal_projection(&state).iteration_ceiling, Some(8));
    }

    #[test]
    fn the_state_digest_separates_two_goals_whose_wire_summaries_are_identical() {
        // This is the whole justification for summarising the task ledger. Two
        // chain states that differ ONLY inside the withheld attempt history
        // must still be distinguishable by a host, or the narrowing has lost
        // information a control plane needs.
        let base = task(
            "a",
            &[],
            vec![attempt(1, GoalTaskAttemptStatus::Live)],
            None,
        );
        let mut other = base.clone();
        other.attempts[0].worker_id = "worker-2".to_owned();

        let left = goal(BTreeMap::from([("a".to_owned(), base)]));
        let right = goal(BTreeMap::from([("a".to_owned(), other)]));

        assert_eq!(
            goal_projection(&left).tasks,
            goal_projection(&right).tasks,
            "fixture must differ only inside the withheld history"
        );
        assert_ne!(
            goal_state_digest(&left).unwrap(),
            goal_state_digest(&right).unwrap(),
            "state_digest must separate chain states the summary cannot"
        );
    }

    #[test]
    fn every_lifecycle_including_a_wait_and_a_terminal_projects_onto_the_wire() {
        let cases = [
            GoalLifecycle::Opened,
            GoalLifecycle::Running,
            GoalLifecycle::Waiting {
                wait: WaitKind::Event {
                    event: "f23-span-elapsed".to_owned(),
                },
            },
            GoalLifecycle::Terminated {
                terminal: GoalTerminalState::Exhausted {
                    kind: ExhaustionKind::Quality,
                    attempts: 3,
                    detail: "checks never went green".to_owned(),
                },
            },
        ];
        for lifecycle in cases {
            let mut state = goal(BTreeMap::new());
            state.lifecycle = lifecycle.clone();
            // The reduced and the wire lifecycle must serialize identically —
            // the wire mirrors the discriminator, it does not invent one.
            assert_eq!(
                serde_json::to_value(&lifecycle).unwrap(),
                serde_json::to_value(goal_projection(&state).lifecycle).unwrap(),
                "lifecycle drifted for {lifecycle:?}"
            );
        }
    }

    #[test]
    fn a_snapshot_event_serializes_as_one_json_line_of_the_declared_wire_type() {
        let state = goal(BTreeMap::new());
        let event = goal_snapshot_event("session-001", &state).expect("snapshot builds");
        let line = event_line(&event).expect("event serializes");
        assert!(!line.contains('\n'), "JSON Lines framing broken: {line}");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(value["type"], json!("goal_snapshot"));
        assert_eq!(value["goal_version"], json!(GOAL_PROTOCOL_VERSION));
        assert_eq!(value["goal_id"], json!("goal-001"));
        assert_eq!(value["cursor"]["journal_sequence"], json!(22));
        assert_eq!(
            value["state_digest"],
            json!(goal_state_digest(&state).unwrap())
        );
    }

    #[test]
    fn a_transition_event_reports_the_lifecycle_after_the_transition() {
        let event = goal_transition_event(
            "session-001",
            "goal-001",
            RecoveryCursor {
                journal_sequence: Some(23),
                journal_digest: "sha256:after".to_owned(),
            },
            GoalTransitionKind::LoopOwnerFinished,
            &GoalLifecycle::Terminated {
                terminal: GoalTerminalState::Cancelled,
            },
        );
        let value: serde_json::Value =
            serde_json::from_str(&event_line(&event).unwrap()).expect("valid JSON");
        assert_eq!(value["type"], json!("goal_transition"));
        assert_eq!(value["transition"], json!("loop_owner_finished"));
        assert_eq!(value["lifecycle"]["state"], json!("terminated"));
        assert_eq!(value["lifecycle"]["terminal"]["state"], json!("cancelled"));
        assert_eq!(value["cursor"]["journal_sequence"], json!(23));
    }
}
