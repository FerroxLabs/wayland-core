//! The adapter surface: the ONE canonical Goal terminal transition, over all
//! five loop owners (F22C, Phase 22 Success Criterion 3).
//!
//! > *"Direct, ForgeFlows, Fleet, Council, and Anvil terminate through one
//! > canonical Goal transition with no nested verification/retry owner."*
//!
//! ## What this module refuses to be
//!
//! It is not a sixth terminal vocabulary. The canonical taxonomy
//! ([`GoalTerminalState`]) was LIFTED from Anvil's own ten-variant enum by the
//! 22-02 census, and this module CONSUMES it without extending it. Nothing here
//! introduces a new terminal category; the mapping table below is the whole of
//! what it adds.
//!
//! It is also not a convention. The phase verdict was explicit that *"a taxonomy
//! everything COULD map onto is not a construction where nothing can terminate
//! any other way"*, so every clause below is either enforced by the type system
//! or refused at the durable boundary — and the clauses that are NEITHER are
//! named as such in `22-C3-SUMMARY.md` rather than quietly counted as done.
//!
//! ## The closed chain
//!
//! ```text
//!   engine's own outcome type
//!         │   (ClimbOutcome | CouncilRunResult | WorkflowRunError | &[ShardSummary] | DirectOutcome)
//!         ▼
//!   exactly one adapter, consuming LoopOwner<S> BY VALUE
//!         │   the only constructors of ↓
//!         ▼
//!   StrategyTermination            ← no other public constructor, not Deserialize, not Clone
//!         │   the only input of ↓
//!         ▼
//!   GoalKernel::finish_loop_owner  ← pub(crate); reducer refuses a plain
//!         │                           GoalTerminated while a claim is live
//!         ▼
//!   SessionEvent::GoalLoopOwnerFinished  ← SessionJournal::append refuses it;
//!                                          only the kernel can mint one
//! ```
//!
//! Four independent links. Breaking the property requires breaking all four.
//!
//! ## Why the token is moved, and why that is the Anvil rule
//!
//! [`LoopOwner`] is not `Clone` and not `Copy`, and each adapter takes it **by
//! value**. A generic retry wrapper around an engine — `for _ in 0..3 { climb()
//! }` — needs the token twice and does not compile. That is Success Criterion
//! 3's *"no nested verification/retry owner"* expressed as a move, not as a
//! comment asking people not to. The compile-fail doctest on
//! [`StrategyTermination::from_anvil`] pins it.
//!
//! Note `cargo nextest` does NOT run doctests. The `compile_fail` proofs need
//! `cargo test --doc -p wcore-agent`, and the executed count must be read back.

use std::marker::PhantomData;

use wcore_protocol::events::RecoveryCursor;
use wcore_swarm::fleet::{FleetError, ShardSummary};
use wcore_types::goal::{
    ExhaustionKind, GoalId, GoalStrategy, GoalTerminalState, VerifiedTerminal,
};

use crate::engine::AgentError;
use crate::orchestration::anvil::TerminalState;
use crate::orchestration::anvil::engine::{ClimbOutcome, EngineError};
use crate::orchestration::council::driver::CouncilRunResult;
use crate::orchestration::council::run::CouncilError;
use crate::orchestration::workflow::runner::{WorkflowRunError, WorkflowRunResult};
use crate::session_journal::JournalError;

use super::kernel::GoalKernel;

mod sealed {
    pub trait Sealed {}
}

/// A compile-time name for one of the five loop owners.
///
/// Sealed: the set is closed at five, and a sixth strategy cannot be given a tag
/// from outside this module. [`GoalStrategy::ALL`] is the runtime half of the
/// same completeness property and [`strategy_tag_name`] is the exhaustive match
/// that fails to compile if a variant is added without a home here.
pub trait StrategyTag: sealed::Sealed {
    /// The durable strategy this tag names.
    const STRATEGY: GoalStrategy;
}

macro_rules! strategy_tags {
    ($($tag:ident => $variant:ident),+ $(,)?) => {
        $(
            /// Compile-time tag for the strategy of the same name.
            #[derive(Debug)]
            pub struct $tag;
            impl sealed::Sealed for $tag {}
            impl StrategyTag for $tag {
                const STRATEGY: GoalStrategy = GoalStrategy::$variant;
            }
        )+
    };
}

strategy_tags! {
    DirectTag => Direct,
    ForgeFlowsTag => ForgeFlows,
    FleetTag => Fleet,
    CouncilTag => Council,
    AnvilTag => Anvil,
}

/// The completeness assertion, as an exhaustive match.
///
/// A sixth [`GoalStrategy`] variant makes this fail to compile with a
/// non-exhaustive-match error, which is the loud failure 22-02 Task 3 asked for
/// instead of a silent fall into a `_` default. There is deliberately no
/// wildcard arm; do not add one.
#[must_use]
pub fn strategy_tag_name(strategy: GoalStrategy) -> &'static str {
    match strategy {
        GoalStrategy::Direct => "DirectTag",
        GoalStrategy::ForgeFlows => "ForgeFlowsTag",
        GoalStrategy::Fleet => "FleetTag",
        GoalStrategy::Council => "CouncilTag",
        GoalStrategy::Anvil => "AnvilTag",
    }
}

/// Proof that the bearer is the ONE loop owner of a Goal, for one claim.
///
/// Minted only by [`GoalLoop`], only after the durable Goal record has been read
/// and found to authorize `S`, and only when no other claim is live. It is not
/// `Clone` and not `Copy`, and every adapter consumes it by value, so exactly
/// one canonical termination can be produced per claim — not zero, not two.
#[derive(Debug)]
pub struct LoopOwner<S: StrategyTag> {
    goal_id: GoalId,
    epoch: u32,
    marker: PhantomData<S>,
}

impl<S: StrategyTag> LoopOwner<S> {
    /// The Goal this claim owns.
    #[must_use]
    pub fn goal_id(&self) -> &GoalId {
        &self.goal_id
    }

    /// The claim epoch. A termination naming a stale epoch is refused durably.
    #[must_use]
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// The strategy this claim is for, from the tag rather than from data.
    #[must_use]
    pub fn strategy(&self) -> GoalStrategy {
        S::STRATEGY
    }

    /// Consume the claim, producing the canonical termination. Private: the five
    /// adapters are the only callers, which is what makes them the only
    /// constructors of [`StrategyTermination`].
    fn terminate(self, terminal: GoalTerminalState) -> StrategyTermination {
        StrategyTermination {
            goal_id: self.goal_id,
            epoch: self.epoch,
            strategy: S::STRATEGY,
            terminal,
        }
    }
}

/// The ONE value a Goal can terminate through.
///
/// Deliberately opaque, non-`Clone`, and with no `Deserialize`: a
/// model-authored tool result is JSON, JSON must deserialize to reach a Rust
/// value, and there is no deserialization route to this type. Its only
/// constructors are the five adapters below; its only consumer is
/// [`GoalLoop::finish`].
#[must_use = "a StrategyTermination that is not handed to the canonical transition means the Goal never terminated"]
#[derive(Debug)]
pub struct StrategyTermination {
    goal_id: GoalId,
    epoch: u32,
    strategy: GoalStrategy,
    terminal: GoalTerminalState,
}

impl StrategyTermination {
    /// Which engine produced this termination.
    #[must_use]
    pub fn strategy(&self) -> GoalStrategy {
        self.strategy
    }

    /// The canonical terminal category. Read-only: observing it does not
    /// terminate anything, and there is no route from here back to a
    /// constructor.
    #[must_use]
    pub fn terminal(&self) -> &GoalTerminalState {
        &self.terminal
    }

    /// The claim epoch this termination belongs to.
    #[must_use]
    pub fn epoch(&self) -> u32 {
        self.epoch
    }
}

/// What a Direct run produced.
///
/// Direct is the only one of the five with no named terminal type of its own —
/// the 22-02 census measured it as *"the engine has no named terminal enum:
/// 'done' is the absence of a further turn"*. These four shapes are exactly the
/// ones the census enumerated, named so an adapter can take them as input. This
/// is an adapter INPUT, not a sixth terminal vocabulary: it maps into
/// [`GoalTerminalState`] and is never persisted.
#[derive(Debug)]
pub enum DirectOutcome<'a> {
    /// The turn loop ran to completion. Note what this does NOT say: Direct has
    /// no verification owner at all, so a completed Direct run is unchecked.
    Completed,
    /// `--max-turns` stopped the loop.
    TurnLimitReached { turns: u64 },
    /// The operator or host cancelled.
    Cancelled,
    /// The engine surfaced an error.
    Failed(&'a AgentError),
}

impl StrategyTermination {
    /// Adapt a Direct run.
    ///
    /// **A completed Direct run maps to [`GoalTerminalState::NeedsEscalation`],
    /// not to a success category, and that is deliberate.** The census measured
    /// Direct's verification owner as *"None. Direct produces no verdict about
    /// its own output."* `SelfChecked` is documented as "self-generated checks
    /// only", which would claim checks ran; none did. Under-claiming evidence is
    /// always preferred to over-claiming it.
    ///
    /// The lifted taxonomy has no "completed but unchecked" category. That is a
    /// real gap, recorded in `22-C3-SUMMARY.md` rather than papered over by
    /// stretching an existing variant's meaning.
    pub fn from_direct(owner: LoopOwner<DirectTag>, outcome: DirectOutcome<'_>) -> Self {
        let terminal = match outcome {
            DirectOutcome::Completed => GoalTerminalState::NeedsEscalation,
            DirectOutcome::TurnLimitReached { turns } => GoalTerminalState::Exhausted {
                kind: ExhaustionKind::Resource,
                attempts: turns,
                detail: "turn limit reached".to_owned(),
            },
            DirectOutcome::Cancelled | DirectOutcome::Failed(AgentError::UserAborted) => {
                GoalTerminalState::Cancelled
            }
            DirectOutcome::Failed(AgentError::ContextTooLong { input_tokens, .. }) => {
                GoalTerminalState::Exhausted {
                    kind: ExhaustionKind::Resource,
                    attempts: *input_tokens,
                    detail: "context window exhausted".to_owned(),
                }
            }
            // Carried as a category, never swallowed into a clean terminal.
            DirectOutcome::Failed(error) => GoalTerminalState::Blocked {
                reason: error.to_string(),
            },
        };
        owner.terminate(terminal)
    }

    /// Adapt a ForgeFlows `WorkflowRunner` run.
    ///
    /// The census's load-bearing distinction is preserved here and nowhere else:
    /// `SchemaValidationFailed` is [`ExhaustionKind::Quality`] (the model kept
    /// producing the wrong shape — fix the prompt) and `DispatchBudgetExceeded`
    /// is [`ExhaustionKind::Resource`] (the DoS backstop fired — raise the
    /// budget). Collapsing both into "failed" destroys the only signal telling
    /// an operator which.
    ///
    /// The three error variants carrying `partial: Box<WorkflowRunResult>` have
    /// their partials READ, not discarded — `StageFailed` reports the completed
    /// stage count rather than rounding the run to a failure.
    pub fn from_forgeflows(
        owner: LoopOwner<ForgeFlowsTag>,
        result: Result<&WorkflowRunResult, &WorkflowRunError>,
    ) -> Self {
        let terminal = match result {
            Ok(run) => partial_from_stages(run),
            Err(WorkflowRunError::SchemaValidationFailed {
                stage,
                attempts,
                message,
                ..
            }) => GoalTerminalState::Exhausted {
                kind: ExhaustionKind::Quality,
                attempts: *attempts as u64,
                detail: format!("stage `{stage}` never validated: {message}"),
            },
            Err(WorkflowRunError::DispatchBudgetExceeded {
                limit, attempted, ..
            }) => GoalTerminalState::Exhausted {
                kind: ExhaustionKind::Resource,
                attempts: *attempted as u64,
                detail: format!("dispatch budget {limit} exceeded"),
            },
            Err(WorkflowRunError::StageFailed { partial, .. }) => partial_from_stages(partial),
            // Graph faults: the walk never got going, so nothing was attempted.
            Err(other) => GoalTerminalState::Blocked {
                reason: other.to_string(),
            },
        };
        owner.terminate(terminal)
    }

    /// Adapt a Fleet dispatch.
    ///
    /// **Bound at [`ShardSummary`], never at the caller-chosen `T`.** This is the
    /// 22-02 census §3 finding verbatim: *"because `T` is caller-chosen, Fleet
    /// cannot be mapped onto a canonical transition by its return type. The
    /// adapter must bind at the `ShardSummary` level, before the caller's
    /// reducer collapses it. Any design that adapts `T` is adapting whatever the
    /// caller felt like returning."* The signature makes that structural — there
    /// is no way to hand this function a `T`.
    ///
    /// A successful dispatch is reported as
    /// [`GoalTerminalState::PartiallyCompleted`] carrying the real counts, even
    /// when `failed == 0`. 97-of-100 is neither success nor failure, and Fleet's
    /// "verification owner" is a count of `succeeded` booleans — nothing checked
    /// whether the work was right — so no evidence-bearing category is honest.
    pub fn from_fleet(
        owner: LoopOwner<FleetTag>,
        result: Result<&[ShardSummary], &FleetError>,
    ) -> Self {
        let terminal = match result {
            Ok(shards) => GoalTerminalState::PartiallyCompleted {
                completed: shards.iter().map(|s| s.successes as u64).sum(),
                failed: shards.iter().map(|s| s.failures as u64).sum(),
            },
            Err(FleetError::Timeout(_)) => GoalTerminalState::TimedOut,
            Err(other) => GoalTerminalState::Blocked {
                reason: other.to_string(),
            },
        };
        owner.terminate(terminal)
    }

    /// Adapt a Council (Crucible) run.
    ///
    /// [`CouncilError::UnpriceableRoster`] maps to
    /// [`GoalTerminalState::Unpriced`], which is the single carrier the census
    /// said the lifted taxonomy had to add: *"folding that into `Blocked` loses
    /// the fact that the run never started for a pricing reason."*
    ///
    /// `skipped` survives. A council answer fused from 2 of 5 proposers because
    /// 3 were keyless is not the same artifact as a unanimous one, and the
    /// difference is invisible in `final_text`.
    ///
    /// A council can never reach [`GoalTerminalState::Verified`]: its
    /// verification owner is an LLM aggregator, and
    /// [`GoalStrategy::can_produce_host_observed_evidence`] is false for it.
    /// This adapter has no route to `Verified` at all — see the `compile_fail`
    /// proof on [`Self::from_anvil`].
    pub fn from_council(
        owner: LoopOwner<CouncilTag>,
        result: Result<&CouncilRunResult, &CouncilError>,
    ) -> Self {
        let terminal = match result {
            Ok(CouncilRunResult::Council { outcome, .. }) => {
                GoalTerminalState::PartiallyCompleted {
                    completed: outcome.chosen_from.len() as u64,
                    failed: outcome.skipped.len() as u64,
                }
            }
            // One model's answer, no roster to count and no verification owner.
            Ok(CouncilRunResult::Direct { .. }) => GoalTerminalState::NeedsEscalation,
            Ok(CouncilRunResult::Cancelled) => GoalTerminalState::Cancelled,
            Err(CouncilError::UnpriceableRoster) => GoalTerminalState::Unpriced {
                detail: CouncilError::UnpriceableRoster.to_string(),
            },
            Err(error @ CouncilError::OverBudget { .. }) => GoalTerminalState::Exhausted {
                kind: ExhaustionKind::Resource,
                attempts: 0,
                detail: error.to_string(),
            },
            Err(error @ CouncilError::DailyBudgetExhausted { .. }) => {
                GoalTerminalState::Exhausted {
                    kind: ExhaustionKind::Resource,
                    attempts: 0,
                    detail: error.to_string(),
                }
            }
            // Usable proposals ran short: a quality wall, not a resource one.
            Err(error @ CouncilError::InsufficientProposals { got, .. }) => {
                GoalTerminalState::Exhausted {
                    kind: ExhaustionKind::Quality,
                    attempts: *got as u64,
                    detail: error.to_string(),
                }
            }
            Err(error @ CouncilError::NoResolver) => GoalTerminalState::Blocked {
                reason: error.to_string(),
            },
        };
        owner.terminate(terminal)
    }

    /// Adapt an Anvil climb — the ONLY adapter that can reach
    /// [`GoalTerminalState::Verified`].
    ///
    /// The verified path consumes [`ClimbOutcome::gate_observation`], which the
    /// climb engine measures from a real executable gate and a real stability
    /// rerun count. This adapter constructs no observation of its own: there is
    /// no parameter through which a caller could supply one, so an adapter's
    /// paraphrase of the evidence cannot become a verified stamp. A climb whose
    /// observation does not clear the bar is reported as `NeedsEscalation` — a
    /// refusal, never a downgraded-but-still-verified value.
    ///
    /// The other nine Anvil terminal states map 1:1, because the canonical
    /// taxonomy was lifted from them.
    ///
    /// # A generic retry wrapper around a climb does not compile
    ///
    /// ```compile_fail
    /// use wcore_agent::goal::strategy::{AnvilTag, LoopOwner, StrategyTermination};
    /// use wcore_agent::orchestration::anvil::engine::{ClimbOutcome, EngineError};
    ///
    /// fn retry_wrapper(owner: LoopOwner<AnvilTag>, outcomes: &[ClimbOutcome]) {
    ///     // `owner` is moved by the first adapter call, so a second iteration
    ///     // is a use-after-move. This is Success Criterion 3's "no nested
    ///     // verification/retry owner", enforced by the borrow checker.
    ///     for outcome in outcomes {
    ///         let _ = StrategyTermination::from_anvil(owner, Ok(outcome), 1);
    ///     }
    /// }
    /// ```
    ///
    /// # A non-Anvil claim cannot be handed to the Anvil adapter
    ///
    /// ```compile_fail
    /// use wcore_agent::goal::strategy::{CouncilTag, LoopOwner, StrategyTermination};
    /// use wcore_agent::orchestration::anvil::engine::ClimbOutcome;
    ///
    /// fn wrong_tag(owner: LoopOwner<CouncilTag>, outcome: &ClimbOutcome) {
    ///     // Council's verification owner is a model judge. The type system,
    ///     // not a reviewer, is what stops it borrowing Anvil's evidence path.
    ///     let _ = StrategyTermination::from_anvil(owner, Ok(outcome), 1);
    /// }
    /// ```
    pub fn from_anvil(
        owner: LoopOwner<AnvilTag>,
        result: Result<&ClimbOutcome, &EngineError>,
        required_stability: u32,
    ) -> Self {
        let outcome = match result {
            Ok(outcome) => outcome,
            // Aborted before it could produce a terminal state through the
            // normal path — surfaced, never swallowed.
            Err(error) => {
                return owner.terminate(GoalTerminalState::Blocked {
                    reason: error.to_string(),
                });
            }
        };
        let terminal = match &outcome.terminal {
            TerminalState::Verified => {
                // The ONLY route to a verified Goal. `from_host_observed_gate`
                // re-checks the strategy, the digest, the check totals and the
                // stability bar, and returns None rather than a weaker verified.
                outcome
                    .gate_observation
                    .as_ref()
                    .and_then(|observation| {
                        VerifiedTerminal::from_host_observed_gate(
                            GoalStrategy::Anvil,
                            observation,
                            required_stability,
                        )
                    })
                    .map_or(GoalTerminalState::NeedsEscalation, |verified| {
                        verified.into_terminal()
                    })
            }
            TerminalState::CriteriaChecked => GoalTerminalState::CriteriaChecked,
            TerminalState::SelfChecked => GoalTerminalState::SelfChecked,
            TerminalState::NeedsEscalation => GoalTerminalState::NeedsEscalation,
            TerminalState::Blocked(reason) => GoalTerminalState::Blocked {
                reason: reason.clone(),
            },
            TerminalState::Cancelled => GoalTerminalState::Cancelled,
            TerminalState::TimedOut => GoalTerminalState::TimedOut,
            TerminalState::PermissionDenied => GoalTerminalState::PermissionDenied,
            TerminalState::CrashedRecovered => GoalTerminalState::CrashedRecovered,
            TerminalState::Superseded => GoalTerminalState::Superseded,
        };
        owner.terminate(terminal)
    }
}

/// Completed/failed stages, from the runner's own per-stage error flags.
fn partial_from_stages(run: &WorkflowRunResult) -> GoalTerminalState {
    let failed = run.stage_results.iter().filter(|s| s.is_error).count() as u64;
    GoalTerminalState::PartiallyCompleted {
        completed: run.stage_results.len() as u64 - failed,
        failed,
    }
}

/// Why a loop-owner claim could not be taken or finished.
#[derive(Debug, thiserror::Error)]
pub enum GoalLoopError {
    /// The Goal's durable record authorizes a different strategy.
    ///
    /// Strategy selection is read from the record, never inferred by the
    /// heuristic intent router at dispatch time.
    #[error("goal {goal_id} authorizes strategy {authorized:?}, not {requested:?}")]
    StrategyMismatch {
        goal_id: GoalId,
        authorized: GoalStrategy,
        requested: GoalStrategy,
    },
    /// No such Goal on the chain.
    #[error("goal {0} does not exist")]
    UnknownGoal(GoalId),
    /// A claim, or the termination, was refused at the durable boundary. A
    /// nested claim arrives here, and the Goal is left resumable.
    #[error(transparent)]
    Journal(#[from] JournalError),
}

/// Drives one strategy over one Goal, and is the only thing that mints a
/// [`LoopOwner`].
///
/// Every `run_*` follows the same three steps: read the strategy off the durable
/// record and refuse a mismatch, claim the one loop owner (refused durably if
/// another is live), then run the engine and terminate with whatever it hands
/// back. There is no fourth step and no early exit that skips the third — the
/// closure's return type is [`StrategyTermination`], so a body that does not
/// produce one does not typecheck.
#[derive(Clone)]
pub struct GoalLoop {
    kernel: GoalKernel,
}

impl GoalLoop {
    #[must_use]
    pub fn new(kernel: GoalKernel) -> Self {
        Self { kernel }
    }

    /// The kernel underneath, for callers that also need to open or inspect a
    /// Goal. Terminating through it is closed off: while a claim is live the
    /// reducer refuses a plain `GoalTerminated`.
    #[must_use]
    pub fn kernel(&self) -> &GoalKernel {
        &self.kernel
    }

    /// Claim the one loop owner for `S`, checking the durable record first.
    fn claim<S: StrategyTag>(&self, goal_id: &GoalId) -> Result<LoopOwner<S>, GoalLoopError> {
        let goal = self
            .kernel
            .goal(goal_id)?
            .ok_or_else(|| GoalLoopError::UnknownGoal(goal_id.clone()))?;
        // Test 8: strategy comes from the durable Goal record.
        if goal.authority.strategy != S::STRATEGY {
            return Err(GoalLoopError::StrategyMismatch {
                goal_id: goal_id.clone(),
                authorized: goal.authority.strategy,
                requested: S::STRATEGY,
            });
        }
        let epoch = self.kernel.claim_loop_owner(goal_id, S::STRATEGY)?;
        Ok(LoopOwner {
            goal_id: goal_id.clone(),
            epoch,
            marker: PhantomData,
        })
    }

    /// Write the canonical transition. The only consumer of a
    /// [`StrategyTermination`].
    fn finish(&self, termination: StrategyTermination) -> Result<RecoveryCursor, GoalLoopError> {
        Ok(self.kernel.finish_loop_owner(
            &termination.goal_id,
            termination.epoch,
            termination.terminal,
        )?)
    }
}

/// Generates the five `run_*` entry points.
///
/// One per strategy rather than one generic function, because the tag has to be
/// pinned at the call site for the adapter type-check to bite: `run_anvil` hands
/// out a `LoopOwner<AnvilTag>` and nothing else, so the closure it drives can
/// only call `from_anvil`.
macro_rules! run_entry_points {
    ($($method:ident => $tag:ident),+ $(,)?) => {
        impl GoalLoop {
            $(
                /// Claim the one loop owner, run the engine exactly once, and
                /// terminate through the canonical transition.
                ///
                /// The engine closure MUST return a [`StrategyTermination`], so
                /// there is no path through it that reaches a terminal state any
                /// other way, and no path that terminates zero times.
                ///
                /// A nested claim is refused before the engine runs, leaving the
                /// Goal non-terminal and resumable.
                pub async fn $method<F, Fut>(
                    &self,
                    goal_id: &GoalId,
                    engine: F,
                ) -> Result<RecoveryCursor, GoalLoopError>
                where
                    F: FnOnce(LoopOwner<$tag>) -> Fut,
                    Fut: std::future::Future<Output = StrategyTermination>,
                {
                    let owner = self.claim::<$tag>(goal_id)?;
                    let termination = engine(owner).await;
                    self.finish(termination)
                }
            )+
        }
    };
}

run_entry_points! {
    run_direct => DirectTag,
    run_forgeflows => ForgeFlowsTag,
    run_fleet => FleetTag,
    run_council => CouncilTag,
    run_anvil => AnvilTag,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_strategy_has_exactly_one_compile_time_tag() {
        // Completeness, asserted rather than assumed. The exhaustive match in
        // `strategy_tag_name` is the compile-time half; this is the runtime half
        // that also proves the names are distinct, so a copy-paste that pointed
        // two strategies at one tag would go red.
        let names: Vec<&'static str> = GoalStrategy::ALL
            .into_iter()
            .map(strategy_tag_name)
            .collect();
        assert_eq!(
            names.len(),
            5,
            "the census measured exactly five loop owners"
        );
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "two strategies share a tag");
    }

    #[test]
    fn each_tag_reports_the_strategy_it_names() {
        assert_eq!(DirectTag::STRATEGY, GoalStrategy::Direct);
        assert_eq!(ForgeFlowsTag::STRATEGY, GoalStrategy::ForgeFlows);
        assert_eq!(FleetTag::STRATEGY, GoalStrategy::Fleet);
        assert_eq!(CouncilTag::STRATEGY, GoalStrategy::Council);
        assert_eq!(AnvilTag::STRATEGY, GoalStrategy::Anvil);
    }
}
