//! Host-observable projection of a durable Goal (F22-C1).
//!
//! ## Why this module exists at all
//!
//! Durable Goals were real in the engine and invisible everywhere a user or a
//! host could meet them: at `8bcb052b` there were ZERO `goal` matches under
//! `crates/wcore-protocol/src/` and zero under `crates/wcore-cli/src/tui/`. The
//! CLI (`goal open|task|run|status|effects`) was one surface of three, and
//! Wayland Desktop — the primary control plane per `AGENTS.md` — could not
//! observe a Goal in any form. This module is the protocol half of closing that.
//!
//! ## What is deliberately NOT re-spelled here
//!
//! `wcore-protocol` already depends on `wcore-types`, and `wcore_types::goal`
//! already owns the canonical Goal taxonomy — [`GoalStrategy`],
//! [`GoalTerminalState`], [`LoopPolicy`], [`WaitKind`], [`TaskId`]. Those types
//! are used here directly rather than mirrored. A second Goal vocabulary on the
//! wire is a vocabulary that can disagree with the chain, which is the exact
//! failure mode the CLI's `goal status` avoids by printing the reduced state
//! itself (`goal_cmd.rs:577-580`).
//!
//! Only the three shapes with no `wcore-types` home are mirrored — the
//! lifecycle, the authority record and the task ledger — because they live in
//! `wcore-agent::session_journal::model`, and `wcore-protocol` cannot depend on
//! `wcore-agent` (the dependency runs the other way). The conversion, and a
//! field-coverage test that fails when `GoalState` grows a field this projection
//! does not carry, live in `wcore-agent::goal::wire`.
//!
//! ## This module opens no authority route
//!
//! `wcore_agent::goal::record` is explicit that a durable record must be
//! deserializable to be replayable, and that the *effective envelope* must not
//! be: the only function that turns a record back into a
//! `GoalAuthoritySnapshot` is `GoalAuthorityRecord::reconstruct`, it lives in
//! `wcore-agent`, and it takes `GoalAuthorityRecord` — not
//! [`GoalAuthorityWire`]. A host that deserializes [`GoalAuthorityWire`] holds a
//! description of an envelope, and there is no function anywhere that accepts it
//! as one. The digests are carried so a host can *recognise* an envelope, never
//! so it can assert one.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use wcore_types::goal::{GoalStrategy, GoalTerminalState, LoopPolicy, WaitKind};

use crate::events::RecoveryCursor;

/// Wire version for the durable-Goal projection.
///
/// Separate from `CONTRACT_MINOR` for the same reason `recovery_version` is: a
/// host needs to reason about this projection's shape without having to decode
/// the whole contract descriptor first.
pub const GOAL_PROTOCOL_VERSION: u16 = 1;

/// Where a Goal is in its durable lifecycle.
///
/// Mirrors `wcore_agent::session_journal::GoalLifecycle` one variant for one
/// variant, including the `tag = "state"` discriminator, so the two serialize
/// identically. `Opened` stays distinct from `Running`: a Goal authorized but
/// not yet consuming an iteration of its loop bound is not the same thing as one
/// that has, and collapsing them makes the bound off by one on resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GoalLifecycleWire {
    /// Authorized, no iteration consumed yet.
    Opened,
    /// Executing an iteration.
    Running,
    /// Not executing, blocked on something named.
    Waiting { wait: WaitKind },
    /// Finished, in exactly one canonical terminal category.
    Terminated { terminal: GoalTerminalState },
}

impl GoalLifecycleWire {
    /// Whether this Goal has finished.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminated { .. })
    }
}

/// The authority envelope a Goal was authorized under, as a host sees it.
///
/// Descriptive only — see the module note. `snapshot_digest` is the digest the
/// kernel committed over the other fields, so a host can tell two envelopes
/// apart without being able to construct either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalAuthorityWire {
    /// Effective limits after intersection with the parent envelope. This is
    /// the intersection, never the request.
    pub effective_limits: BTreeMap<String, u64>,
    /// The strategy that was actually authorized.
    pub strategy: GoalStrategy,
    /// The loop policy that was actually authorized.
    pub loop_policy: LoopPolicy,
    /// Digest of the parent envelope this was resolved against.
    pub parent_envelope_digest: String,
    /// Digest the kernel committed over the four fields above.
    pub snapshot_digest: String,
}

/// The single strategy currently executing a Goal, as a host sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalLoopOwnerWire {
    /// Which of the five engines owns the loop.
    pub strategy: GoalStrategy,
    /// Monotonic claim counter for this Goal.
    pub epoch: u32,
    /// When this claim stops being evidence that an owner is alive.
    pub lease_expires_unix_ms: u64,
}

/// What a task in the durable ledger is doing right now.
///
/// A CLOSED, derived summary of `GoalTaskState`'s attempt history. The set is
/// exhaustive over that history and the derivation is pinned by
/// `wcore-agent::goal::wire`'s tests.
///
/// `NeedsResolution` is deliberately NOT a kind of failure, for the same reason
/// `GoalTaskAttemptStatus::Unknown` is not: a failed attempt says the effect did
/// not happen, an unresolved one says the ledger cannot tell, and those settle
/// differently. Collapsing them on the wire is how a host builds a silent retry
/// button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalTaskWireStatus {
    /// Declared, never claimed, and its dependencies are unmet.
    Blocked,
    /// A worker could claim it right now.
    Claimable,
    /// Owned by a live claim.
    Running,
    /// The last claim was revoked; the worker that held it may still be alive.
    Revoked,
    /// Carries a durable completion the parent has not yet observed. This is
    /// the outbox a restarted parent drains — distinct from `Completed`,
    /// because a completion that exists and a completion that was delivered are
    /// two different facts.
    CompletedUndelivered,
    /// Carries a durable, delivered completion.
    Completed,
    /// The attempt's outcome could not be established. Requires an operator or
    /// reconciler; never a silent retry.
    NeedsResolution,
}

/// One task in a Goal's durable ledger, summarised for a control plane.
///
/// The full attempt history (`attempts`, `handoffs`, the completion's
/// `effect_digest`) is NOT on the wire in v1 — it is available from
/// `wayland-core goal status`. `GoalProjection::state_digest` is what keeps this
/// narrowing honest: it is taken over the canonical JSON of the FULL reduced
/// `GoalState`, so a host can always tell which chain state its summary
/// corresponds to and can detect that two summaries came from different states
/// even when the summarised fields happen to match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalTaskWire {
    pub task_id: String,
    /// Task ids that must carry a DURABLE completion before this one is
    /// claimable.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub depends_on: BTreeSet<String>,
    /// The key the task's effect is deduplicated by, stable across attempts.
    pub idempotency_key: String,
    pub status: GoalTaskWireStatus,
    /// The committed claim epoch. Zero means never claimed.
    pub epoch: u64,
    /// How many claims this task has ever granted.
    pub attempts: u32,
    /// The terminal outcome of the durable completion, if there is one. Uses
    /// the ONE canonical terminal taxonomy, never a second vocabulary for
    /// tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<GoalTerminalState>,
    /// How many times this task transitioned from blocked to claimable. The
    /// exactly-once-unblock property is a count, not an assertion.
    pub dependency_releases: u64,
    pub last_transition_seq: u64,
}

/// The complete host-observable projection of one durable Goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalProjection {
    pub goal_id: String,
    pub objective: String,
    pub authority: GoalAuthorityWire,
    pub lifecycle: GoalLifecycleWire,
    /// Iterations consumed against the recorded loop bound.
    pub iterations_started: u32,
    /// The ceiling the recorded loop policy authorizes. `None` is `Manual`,
    /// which has no numeric ceiling because each advance is itself an explicit
    /// operator action — it is NOT "unbounded", and a host must not render it
    /// as such.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration_ceiling: Option<u32>,
    /// How many times this Goal has been resumed after a crash.
    pub resume_count: u32,
    pub opened_at_unix_ms: u64,
    /// The cursor a reconnecting host resumes this Goal from. The protocol
    /// crate's EXISTING cursor shape, not a second definition.
    pub cursor: RecoveryCursor,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<GoalTaskWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_owner: Option<GoalLoopOwnerWire>,
    /// How many loop-owner claims this Goal has ever granted.
    pub loop_owner_epochs: u32,
}

/// Which durable transition produced a [`crate::events::ProtocolEvent::GoalTransition`].
///
/// One variant per `SessionEvent::Goal*` the kernel can append. This carries the
/// *milestone*, not the payload: the payload is the projection, and a host that
/// wants it asks for a snapshot. Same split `turn_recovery_lifecycle` and
/// `session_recovery_snapshot` already use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalTransitionKind {
    /// The Goal was authorized.
    Opened,
    /// One iteration of the authorized loop bound was consumed.
    IterationStarted,
    /// The Goal parked on something named.
    WaitBegun,
    /// The wait resolved and the Goal returned to running.
    WaitResolved,
    /// A fresh process picked the Goal up after a crash.
    RunResumed,
    /// The Goal's ONE loop owner was claimed.
    LoopOwnerClaimed,
    /// The Goal terminated through the canonical loop-owner transition.
    LoopOwnerFinished,
    /// The Goal terminated outside a loop-owner claim.
    Terminated,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn authority() -> GoalAuthorityWire {
        GoalAuthorityWire {
            effective_limits: BTreeMap::from([("max_tokens".to_owned(), 10_000)]),
            strategy: GoalStrategy::Fleet,
            loop_policy: LoopPolicy::Fixed { iterations: 8 },
            parent_envelope_digest: "wayland-core-goal-fleet/v1".to_owned(),
            snapshot_digest: "sha256:aa".to_owned(),
        }
    }

    #[test]
    fn the_lifecycle_wire_discriminator_matches_the_journal_projection_exactly() {
        // The reducer's GoalLifecycle uses `tag = "state"` with snake_case
        // variants. If this wire type drifted to a different discriminator the
        // two would serialize differently and a host would be reading a shape
        // the chain never produces.
        assert_eq!(
            serde_json::to_value(GoalLifecycleWire::Opened).unwrap(),
            json!({"state": "opened"})
        );
        assert_eq!(
            serde_json::to_value(GoalLifecycleWire::Waiting {
                wait: WaitKind::Event {
                    event: "f23-span-elapsed".to_owned()
                }
            })
            .unwrap(),
            json!({"state": "waiting", "wait": {"kind": "event", "event": "f23-span-elapsed"}})
        );
    }

    #[test]
    fn a_manual_loop_renders_no_ceiling_rather_than_an_unbounded_one() {
        // `Manual` has no numeric ceiling. A projection that emitted `0` or a
        // sentinel would be read as a bound of zero or as "unbounded"; both are
        // wrong, and the taxonomy has no unbounded variant at all.
        let projection = GoalProjection {
            goal_id: "g".to_owned(),
            objective: "o".to_owned(),
            authority: authority(),
            lifecycle: GoalLifecycleWire::Opened,
            iterations_started: 0,
            iteration_ceiling: None,
            resume_count: 0,
            opened_at_unix_ms: 1,
            cursor: RecoveryCursor {
                journal_sequence: Some(1),
                journal_digest: "sha256:bb".to_owned(),
            },
            tasks: Vec::new(),
            loop_owner: None,
            loop_owner_epochs: 0,
        };
        let value = serde_json::to_value(&projection).unwrap();
        assert!(
            value.get("iteration_ceiling").is_none(),
            "an absent ceiling must be absent, not zero: {value}"
        );
        assert!(value.get("tasks").is_none(), "empty ledger must be absent");
    }

    #[test]
    fn the_task_status_set_keeps_undelivered_and_unresolved_distinct() {
        // Three facts that a control plane must not collapse: a completion that
        // exists but was never observed, a completion that was, and an outcome
        // the ledger cannot establish. Collapsing the first two loses the
        // outbox; collapsing the third into a failure builds a silent retry.
        let statuses = [
            GoalTaskWireStatus::CompletedUndelivered,
            GoalTaskWireStatus::Completed,
            GoalTaskWireStatus::NeedsResolution,
        ];
        let rendered = statuses
            .iter()
            .map(|status| serde_json::to_value(status).unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(rendered.len(), 3, "statuses collapsed: {rendered:?}");
        assert_eq!(
            serde_json::to_value(GoalTaskWireStatus::NeedsResolution).unwrap(),
            json!("needs_resolution")
        );
    }

    #[test]
    fn the_authority_wire_round_trips_without_reconstructing_an_envelope() {
        // It deserializes — a host must be able to read it. What it must NOT do
        // is become an effective envelope, and it cannot: no function in any
        // crate accepts a GoalAuthorityWire as authority. This test pins the
        // round trip so the shape is stable, and the type's absence from every
        // authority signature is what pins the rest.
        let wire = authority();
        let round_tripped: GoalAuthorityWire =
            serde_json::from_value(serde_json::to_value(&wire).unwrap()).unwrap();
        assert_eq!(round_tripped, wire);
    }

    #[test]
    fn every_kernel_goal_transition_has_exactly_one_wire_kind() {
        // Eight SessionEvent::Goal* variants the kernel can append:
        // GoalOpened, GoalIterationStarted, GoalWaitBegun, GoalWaitResolved,
        // GoalRunResumed, GoalLoopOwnerClaimed, GoalLoopOwnerFinished,
        // GoalTerminated. A missing kind is a transition a host cannot observe.
        let kinds = [
            GoalTransitionKind::Opened,
            GoalTransitionKind::IterationStarted,
            GoalTransitionKind::WaitBegun,
            GoalTransitionKind::WaitResolved,
            GoalTransitionKind::RunResumed,
            GoalTransitionKind::LoopOwnerClaimed,
            GoalTransitionKind::LoopOwnerFinished,
            GoalTransitionKind::Terminated,
        ];
        let rendered = kinds
            .iter()
            .map(|kind| serde_json::to_value(kind).unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(rendered.len(), 8, "wire kinds collapsed: {rendered:?}");
    }
}
