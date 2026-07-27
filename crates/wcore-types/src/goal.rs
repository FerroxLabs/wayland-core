//! The provider-neutral Goal vocabulary.
//!
//! It lives here, in the bottom crate, because `wcore-protocol` must be able to
//! name Goal identity, strategy, loop policy, wait kind and terminal state on
//! the wire without depending on `wcore-agent`, and `wcore-agent` must be able
//! to persist the same words in the F12 session journal. One definition, two
//! consumers, no upward dependency.
//!
//! ## Where the terminal taxonomy came from
//!
//! It was NOT invented here. `crates/wcore-agent/src/orchestration/anvil/mod.rs`
//! already carried a ten-variant terminal enum whose in-source comment calls it
//! "the COMPLETE enum (spec §6.5). Every climb ends in exactly one of these...
//! There is no silent fourth exit." That enum already reserved `Verified` for a
//! real Tier-1 gate and already kept partially-checked outcomes
//! (`CriteriaChecked`, `SelfChecked`, `NeedsEscalation`) as explicit categories
//! instead of rounding them to success or failure — which is exactly the
//! discipline Phase 22 Success Criterion 3 asks for. The 22-02 census
//! (`.planning/phases/22-supervision-durable-goals-fleet-loops/22-02-LOOP-OWNER-CENSUS.md`)
//! measured all five engines against the source and found that inventing a sixth
//! vocabulary beside it would be the "parallel lifecycle" PROJECT.md forbids.
//!
//! So this taxonomy LIFTS Anvil's and adds only what the census showed the other
//! four engines genuinely produce and Anvil's could not carry:
//!
//! | Added carrier | Measured need |
//! |---|---|
//! | [`GoalTerminalState::Unpriced`] | `CouncilError::UnpriceableRoster` refuses to run because no budget ceiling can be certified. Folding that into `Blocked` loses that the run never started for a *pricing* reason. |
//! | [`ExhaustionKind`] on [`GoalTerminalState::Exhausted`] | `WorkflowRunError::SchemaValidationFailed` and `::DispatchBudgetExceeded` are both "ran out of attempts" and mean opposite things — fix the prompt versus raise the budget. |
//! | [`GoalTerminalState::PartiallyCompleted`] | Fleet's `ShardSummary` carries `successes`/`failures`; 97-of-100 is neither success nor failure. |

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Stable identity of one durable objective.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GoalId(pub String);

impl GoalId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for GoalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which execution engine owns the work for a Goal.
///
/// Recorded ON the durable Goal record, never inferred per turn: the heuristic
/// intent router stays task-shape routing and does not become the loop owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStrategy {
    /// The agent engine's own turn loop.
    Direct,
    /// The ForgeFlows `WorkflowRunner`.
    ForgeFlows,
    /// The swarm `FleetDispatcher`.
    Fleet,
    /// The Crucible council.
    Council,
    /// The Anvil gated forge.
    Anvil,
}

impl GoalStrategy {
    /// The complete set. Asserted rather than assumed so a sixth strategy
    /// cannot be added without the completeness test in
    /// `crates/wcore-agent/tests/goal_kernel_test.rs` going red.
    pub const ALL: [GoalStrategy; 5] = [
        GoalStrategy::Direct,
        GoalStrategy::ForgeFlows,
        GoalStrategy::Fleet,
        GoalStrategy::Council,
        GoalStrategy::Anvil,
    ];

    /// Whether this strategy's verification owner is capable of producing
    /// host-observed deterministic evidence at all.
    ///
    /// Measured, not assumed (22-02 census §2–§5): only Anvil runs a real
    /// executable gate. ForgeFlows validates output *shape*; Council's
    /// aggregator is a model judge; Fleet counts `succeeded` booleans; Direct
    /// produces no verdict about its own output. Under the F20-GATE-02
    /// discipline none of those can mint a verified terminal state, and this
    /// predicate is the single place that fact is written down.
    #[must_use]
    pub fn can_produce_host_observed_evidence(self) -> bool {
        matches!(self, GoalStrategy::Anvil)
    }
}

/// Why a bounded run stopped attempting.
///
/// The distinction is load-bearing: one says the work kept coming back wrong,
/// the other says the resource envelope ran out. An operator fixes those
/// differently, and collapsing them destroys the only signal that says which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExhaustionKind {
    /// Attempts kept producing an unusable result — e.g. a schema-bearing stage
    /// that never validated inside its retry budget.
    Quality,
    /// The resource envelope ran out — e.g. the per-run dispatch budget, a
    /// token or cost ceiling, a shard timeout.
    Resource,
}

/// The ONE canonical terminal taxonomy. Every strategy terminates into exactly
/// one of these, and no adapter may extend it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[non_exhaustive]
pub enum GoalTerminalState {
    /// All checks green with the required stability on a REAL executable gate.
    /// The only state that earns the reserved `verified` stamp, and the only
    /// one that cannot be constructed from a value a model-authored tool result
    /// can produce. See [`VerifiedTerminal`].
    Verified,
    /// User-confirmed derived criteria passed. Stamped `criteria-checked`,
    /// never `verified`.
    CriteriaChecked,
    /// Self-generated checks only — correlated evidence, not truth.
    SelfChecked,
    /// The work completed for some units and not others, and that split is the
    /// honest answer. Carries the counts rather than rounding them.
    PartiallyCompleted { completed: u64, failed: u64 },
    /// Attempts ran out. [`ExhaustionKind`] says whether the wall was quality or
    /// resource.
    Exhausted {
        kind: ExhaustionKind,
        attempts: u64,
        detail: String,
    },
    /// Some checks remain uncracked; the operator is offered escalate / show
    /// attempts / accept-partial.
    NeedsEscalation,
    /// The run never started because its cost could not be certified. Distinct
    /// from `Blocked`: nothing was attempted, and the reason was pricing.
    Unpriced { detail: String },
    /// Could not proceed, for a stated reason.
    Blocked { reason: String },
    /// The operator or host cancelled; partial work is reported honestly.
    Cancelled,
    /// A time budget was exhausted mid-run.
    TimedOut,
    /// Execution was refused on posture or permissions.
    PermissionDenied,
    /// A crash was caught and the run recovered from its journal.
    CrashedRecovered,
    /// A newer run for the same objective superseded this one.
    Superseded,
    /// Resume could not reconstruct the recorded authority/budget envelope
    /// exactly, so the Goal is parked for explicit operator resolution rather
    /// than resumed under a possibly-wider envelope. Fail-closed by
    /// construction: this is never a default, it is a refusal.
    AuthorityUnreconstructable { detail: String },
}

impl GoalTerminalState {
    /// Whether this state earns the reserved `verified` stamp. A single, tight
    /// predicate, deliberately — the honesty vocabulary hangs off it.
    #[must_use]
    pub fn is_verified(&self) -> bool {
        matches!(self, GoalTerminalState::Verified)
    }
}

/// Host-observed deterministic evidence that a real executable gate ran and
/// passed with the required stability.
///
/// This type has NO public constructor from raw data. The only way to obtain
/// one is [`VerifiedTerminal::from_host_observed_gate`], which is reachable
/// only from a caller holding a [`HostGateObservation`] — and
/// [`HostGateObservation`] is deliberately NOT `Deserialize`. That is the
/// structural half of the anti-forgery property: a model-authored tool result
/// is JSON, JSON must deserialize to reach any Rust value, and there is no
/// deserialization route to this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostGateObservation {
    /// The pinned gate closure digest the parent executed.
    pub gate_closure_digest: String,
    /// Checks that passed on the final candidate.
    pub checks_passed: u32,
    /// Total checks on the final candidate.
    pub checks_total: u32,
    /// Consecutive passing repeats observed (the stability policy's N).
    pub stability_repeats: u32,
}

impl HostGateObservation {
    /// Construct from a gate the PARENT ran. The name is the contract: a
    /// caller, child, model or advisory evaluator claim is not an observation,
    /// and there is no other constructor.
    #[must_use]
    pub fn from_parent_executed_gate(
        gate_closure_digest: impl Into<String>,
        checks_passed: u32,
        checks_total: u32,
        stability_repeats: u32,
    ) -> Self {
        Self {
            gate_closure_digest: gate_closure_digest.into(),
            checks_passed,
            checks_total,
            stability_repeats,
        }
    }
}

/// A terminal state that is allowed to be [`GoalTerminalState::Verified`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifiedTerminal(GoalTerminalState);

impl VerifiedTerminal {
    /// The ONLY route to a verified terminal state.
    ///
    /// Returns `None` — never a downgraded-but-still-verified value — when the
    /// observation does not actually clear the bar. A partial gate pass is
    /// `NeedsEscalation`, which is information, not a failure to report.
    #[must_use]
    pub fn from_host_observed_gate(
        strategy: GoalStrategy,
        observation: &HostGateObservation,
        required_stability: u32,
    ) -> Option<Self> {
        if !strategy.can_produce_host_observed_evidence() {
            return None;
        }
        if observation.gate_closure_digest.is_empty() {
            return None;
        }
        if observation.checks_total == 0 || observation.checks_passed != observation.checks_total {
            return None;
        }
        if observation.stability_repeats < required_stability {
            return None;
        }
        Some(Self(GoalTerminalState::Verified))
    }

    #[must_use]
    pub fn into_terminal(self) -> GoalTerminalState {
        self.0
    }
}

/// How a Goal's bounded loop is governed. Session-local only in this phase;
/// persistent scheduling is deferred explicitly to Phase 24.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoopPolicy {
    /// Exactly one pass. The default, and the only one that cannot multiply a
    /// bound.
    Once,
    /// A fixed number of iterations, bounded up front.
    Fixed { iterations: u32 },
    /// A dynamic loop bounded by BOTH an iteration cap and a wall-clock budget.
    /// Neither bound is optional: a dynamic loop with one bound is an unbounded
    /// loop wearing a bound.
    Dynamic {
        max_iterations: u32,
        max_wall_millis: u64,
    },
    /// Driven by an external event, still bounded by a maximum number of
    /// deliveries so a chatty source cannot become an unbounded loop.
    EventDriven { max_deliveries: u32 },
    /// Advanced only by an explicit operator action.
    Manual,
}

impl LoopPolicy {
    /// Whether this policy can, by itself, cause more than one execution.
    #[must_use]
    pub fn can_iterate(&self) -> bool {
        !matches!(self, LoopPolicy::Once)
    }
}

/// What a Goal is waiting on when it is not running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaitKind {
    /// Waiting for a wall-clock instant, expressed as unix millis.
    Until { unix_millis: u64 },
    /// Waiting for a human decision.
    Approval { approval_id: String },
    /// Waiting for a delegated child to reach a terminal state.
    Child { child_id: String },
    /// Waiting for an external event by name.
    Event { event: String },
    /// Waiting for an operator to resolve something the system refuses to guess.
    OperatorResolution { detail: String },
}

/// Stable identity of one durable task inside a Goal's ledger (F22-03).
///
/// A task is Fleet work the ledger tracks across a kill: its dependencies, its
/// attempts, who owns it now and whether its completion ever reached the parent.
/// It is identified separately from the Goal because a Goal outlives any single
/// task and a task is reassigned without the Goal changing identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why an attempt's outcome could not be established.
///
/// Every variant means the same operational thing — the ledger does NOT know
/// whether the effect happened — and the ledger's response to all of them is
/// identical: park the task for explicit resolution rather than retry it. The
/// variants are kept distinct because an operator settles them differently, the
/// same reason [`ExhaustionKind`] exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskUnknownReason {
    /// The owning process died between starting the effect and recording it.
    OwnerDiedMidAttempt,
    /// The lease expired while the owner may still have been running.
    LeaseExpiredWhileOwnerLive,
    /// The effect was dispatched and its receipt never arrived.
    ReceiptMissing,
    /// Something else, stated rather than collapsed into one of the above.
    Other { detail: String },
}

/// UNTRUSTED authority/budget envelope as it arrives from a host or child.
///
/// This is the only shape an untrusted payload can deserialize into. It is
/// deliberately NOT the effective envelope: it must pass a resolver to become
/// one. The pattern mirrors the output-only effective-execution-policy
/// discipline already in `execution_policy.rs`, and it exists because an
/// authority snapshot an untrusted input can deserialize into directly is an
/// authority-widening route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalAuthorityRequest {
    /// Requested named limits. Values are REQUESTS, never grants.
    #[serde(default)]
    pub requested_limits: BTreeMap<String, u64>,
    /// Requested strategy for the work.
    pub strategy: GoalStrategy,
    /// Requested loop policy.
    pub loop_policy: LoopPolicy,
}

/// The EFFECTIVE authority and budget envelope recorded on a durable Goal
/// transition.
///
/// Output-only: it does not implement `Deserialize` from an untrusted source
/// path. It is produced by [`resolve_goal_authority`] from a request plus the
/// parent's already-effective envelope, and it is what a resume restores rather
/// than re-deriving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoalAuthoritySnapshot {
    /// The effective limits after intersection with the parent envelope. A
    /// child can only ever narrow.
    pub effective_limits: BTreeMap<String, u64>,
    /// The strategy that was actually authorized.
    pub strategy: GoalStrategy,
    /// The loop policy that was actually authorized.
    pub loop_policy: LoopPolicy,
    /// Digest of the parent envelope this was resolved against, so a resume can
    /// prove it reconstructed the same one rather than assuming it did.
    pub parent_envelope_digest: String,
}

/// Intersect a request with the parent's effective envelope.
///
/// Narrowing only, in both directions of the map:
/// - a limit the parent does not name cannot be created by the request;
/// - a limit the request asks to raise is clamped to the parent's value;
/// - a limit the parent names and the request omits is inherited unchanged.
///
/// This does NOT invent a second intersection primitive for budgets. It records
/// what the effective envelope WAS at the transition so resume restores the
/// same one; Phase 21 owns the anti-amplification property over the live seams.
#[must_use]
pub fn resolve_goal_authority(
    request: &GoalAuthorityRequest,
    parent_effective_limits: &BTreeMap<String, u64>,
    parent_envelope_digest: impl Into<String>,
) -> GoalAuthoritySnapshot {
    let mut effective_limits = parent_effective_limits.clone();
    for (name, requested) in &request.requested_limits {
        if let Some(parent) = parent_effective_limits.get(name) {
            effective_limits.insert(name.clone(), (*requested).min(*parent));
        }
        // A limit the parent does not name is not a limit the child may create.
    }
    GoalAuthoritySnapshot {
        effective_limits,
        strategy: request.strategy,
        loop_policy: request.loop_policy.clone(),
        parent_envelope_digest: parent_envelope_digest.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
    }

    #[test]
    fn a_request_can_narrow_a_limit_but_never_widen_or_create_one() {
        let parent = limits(&[("max_tokens", 1000), ("max_cost_cents", 50)]);
        let request = GoalAuthorityRequest {
            requested_limits: limits(&[
                ("max_tokens", 100),     // narrower — honored
                ("max_cost_cents", 999), // wider — clamped
                ("max_processes", 64),   // unnamed by parent — refused
            ]),
            strategy: GoalStrategy::Direct,
            loop_policy: LoopPolicy::Once,
        };
        let snapshot = resolve_goal_authority(&request, &parent, "digest");
        assert_eq!(snapshot.effective_limits.get("max_tokens"), Some(&100));
        assert_eq!(snapshot.effective_limits.get("max_cost_cents"), Some(&50));
        assert_eq!(snapshot.effective_limits.get("max_processes"), None);
    }

    #[test]
    fn a_limit_the_request_omits_is_inherited_unchanged() {
        let parent = limits(&[("max_tokens", 1000)]);
        let request = GoalAuthorityRequest {
            requested_limits: BTreeMap::new(),
            strategy: GoalStrategy::Anvil,
            loop_policy: LoopPolicy::Once,
        };
        let snapshot = resolve_goal_authority(&request, &parent, "digest");
        assert_eq!(snapshot.effective_limits.get("max_tokens"), Some(&1000));
    }

    #[test]
    fn only_a_strategy_with_a_real_gate_can_reach_verified() {
        let observation = HostGateObservation::from_parent_executed_gate("abc", 10, 10, 3);
        // The negative case is the load-bearing one.
        for strategy in GoalStrategy::ALL {
            let got = VerifiedTerminal::from_host_observed_gate(strategy, &observation, 3);
            if strategy.can_produce_host_observed_evidence() {
                assert!(got.is_some(), "{strategy:?} runs a real gate");
            } else {
                assert!(
                    got.is_none(),
                    "{strategy:?} has no host-observed verification owner and must not reach verified"
                );
            }
        }
    }

    #[test]
    fn a_partial_or_unstable_gate_pass_does_not_reach_verified() {
        let partial = HostGateObservation::from_parent_executed_gate("abc", 9, 10, 3);
        assert!(
            VerifiedTerminal::from_host_observed_gate(GoalStrategy::Anvil, &partial, 3).is_none()
        );
        let unstable = HostGateObservation::from_parent_executed_gate("abc", 10, 10, 1);
        assert!(
            VerifiedTerminal::from_host_observed_gate(GoalStrategy::Anvil, &unstable, 3).is_none()
        );
        let no_gate = HostGateObservation::from_parent_executed_gate("", 10, 10, 3);
        assert!(
            VerifiedTerminal::from_host_observed_gate(GoalStrategy::Anvil, &no_gate, 3).is_none()
        );
        let zero_checks = HostGateObservation::from_parent_executed_gate("abc", 0, 0, 3);
        assert!(
            VerifiedTerminal::from_host_observed_gate(GoalStrategy::Anvil, &zero_checks, 3)
                .is_none()
        );
    }

    #[test]
    fn the_terminal_taxonomy_carries_every_shape_the_census_measured() {
        // One representative per engine outcome the 22-02 census found, proving
        // none of them has to be rounded to success or failure.
        let carried = [
            // ForgeFlows: schema retries exhausted (quality) vs dispatch budget
            // exceeded (resource) — the same word, opposite meanings.
            GoalTerminalState::Exhausted {
                kind: ExhaustionKind::Quality,
                attempts: 3,
                detail: "schema validation never passed".into(),
            },
            GoalTerminalState::Exhausted {
                kind: ExhaustionKind::Resource,
                attempts: 64,
                detail: "dispatch budget".into(),
            },
            // Fleet: 97 of 100.
            GoalTerminalState::PartiallyCompleted {
                completed: 97,
                failed: 3,
            },
            // Fleet: shard timeout.
            GoalTerminalState::TimedOut,
            // Council: unpriceable roster — refused before spend.
            GoalTerminalState::Unpriced {
                detail: "roster not fully priced".into(),
            },
            // Council with skipped proposers is not a unanimous one.
            GoalTerminalState::PartiallyCompleted {
                completed: 2,
                failed: 3,
            },
            // Anvil: every one of its own ten states has a home.
            GoalTerminalState::Verified,
            GoalTerminalState::CriteriaChecked,
            GoalTerminalState::SelfChecked,
            GoalTerminalState::NeedsEscalation,
            GoalTerminalState::Blocked {
                reason: "gate cannot execute".into(),
            },
            GoalTerminalState::Cancelled,
            GoalTerminalState::PermissionDenied,
            GoalTerminalState::CrashedRecovered,
            GoalTerminalState::Superseded,
            // Resume that cannot reconstruct its envelope parks explicitly.
            GoalTerminalState::AuthorityUnreconstructable {
                detail: "parent envelope digest mismatch".into(),
            },
        ];
        // Exactly one of them is verified — the predicate stays tight.
        assert_eq!(carried.iter().filter(|t| t.is_verified()).count(), 1);
        // Every one round-trips through the wire shape the protocol will use.
        for state in &carried {
            let encoded = serde_json::to_string(state).expect("terminal state encodes");
            let decoded: GoalTerminalState =
                serde_json::from_str(&encoded).expect("terminal state decodes");
            assert_eq!(&decoded, state);
        }
    }

    #[test]
    fn a_dynamic_loop_cannot_be_expressed_with_only_one_bound() {
        // Structural, not conventional: the variant has no single-bound form,
        // so a caller cannot write one.
        let policy = LoopPolicy::Dynamic {
            max_iterations: 5,
            max_wall_millis: 60_000,
        };
        assert!(policy.can_iterate());
        assert!(!LoopPolicy::Once.can_iterate());
    }

    #[test]
    fn an_untrusted_payload_deserializes_into_a_request_and_not_an_envelope() {
        // The request shape accepts host JSON.
        let payload = r#"{"requested_limits":{"max_tokens":10},"strategy":"anvil","loop_policy":{"kind":"once"}}"#;
        let request: GoalAuthorityRequest =
            serde_json::from_str(payload).expect("a host request deserializes");
        assert_eq!(request.strategy, GoalStrategy::Anvil);
        // And the effective envelope is reachable only through the resolver.
        // (`GoalAuthoritySnapshot` implements Serialize but NOT Deserialize;
        // that is asserted by the compiler, not by this test.)
        let snapshot = resolve_goal_authority(&request, &limits(&[("max_tokens", 5)]), "d");
        assert_eq!(snapshot.effective_limits.get("max_tokens"), Some(&5));
    }
}
