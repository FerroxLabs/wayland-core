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
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use wcore_protocol::events::RecoveryCursor;
use wcore_swarm::fleet::{FleetError, ShardSummary};
use wcore_types::goal::{
    ExhaustionKind, GoalId, GoalStrategy, GoalTerminalState, VerifiedTerminal,
};

use crate::engine::AgentError;
use crate::orchestration::anvil::TerminalState;
use crate::orchestration::anvil::engine::{ClimbOutcome, EngineError};
use crate::orchestration::council::CouncilOutcome;
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

/// What a Fleet dispatch produced.
///
/// Modelled like [`DirectOutcome`] rather than as a bare `Result` because the
/// Goal's fleet driver can fail in a way `FleetError` cannot express — a claim
/// refusal, a ledger fence, an aborted wave. Squeezing that into
/// `FleetError::Timeout` to satisfy a signature would be a fabricated terminal,
/// so it gets its own carrier and lands in `Blocked` with a stated reason.
#[derive(Debug)]
pub enum FleetOutcome<'a> {
    /// Shards came back. Bound at [`ShardSummary`] — see [`StrategyTermination::from_fleet`].
    Dispatched(&'a [ShardSummary]),
    /// The dispatcher itself failed.
    Failed(&'a FleetError),
    /// The Goal's fleet driver failed around dispatch, for a stated reason.
    DriverFailed { detail: String },
}

/// What a Council run produced.
///
/// Exists for the same reason as [`FleetOutcome`] and [`AnvilOutcome`]: the
/// SHIPPED council entry point (`drive_council`) returns
/// `anyhow::Result<CouncilRunResult>`, not `Result<_, CouncilError>`. The typed
/// arm is kept and preferred — [`Self::from_anyhow`] downcasts, so a wrapped
/// `CouncilError` still reaches its exact terminal category (`Unpriced`,
/// `Exhausted{Resource}`, …) rather than being flattened. Only an error that is
/// genuinely not a `CouncilError` falls through to `DriverFailed`.
pub enum CouncilRunOutcome<'a> {
    /// The council ran and produced a result.
    Ran(&'a CouncilRunResult),
    /// The MANUAL council path ran, producing a bare [`CouncilOutcome`].
    ///
    /// A separate variant because that path has no [`AssemblyPlan`] — the
    /// operator's configured roster IS the plan. Constructing an empty
    /// `AssemblyPlan` just to reach [`Self::Ran`] would fabricate a decision the
    /// assembler never made, which is the same anti-pattern as squeezing a
    /// driver failure into an engine error. The terminal mapping is identical to
    /// `Ran(Council { .. })`, because it is the same counting rule over the same
    /// two fields.
    RanManual(&'a CouncilOutcome),
    /// The council failed with its own typed error.
    Failed(&'a CouncilError),
    /// The driver failed around the council, for a stated reason.
    DriverFailed { detail: String },
}

/// Hand-written because [`CouncilRunResult`] is not `Debug`, and deriving it
/// there to satisfy this enum would be a drive-by change to the council's public
/// API. The variant name is all a diagnostic needs here.
impl std::fmt::Debug for CouncilRunOutcome<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ran(_) => f.write_str("CouncilRunOutcome::Ran(..)"),
            Self::RanManual(_) => f.write_str("CouncilRunOutcome::RanManual(..)"),
            Self::Failed(error) => write!(f, "CouncilRunOutcome::Failed({error})"),
            Self::DriverFailed { detail } => {
                write!(f, "CouncilRunOutcome::DriverFailed({detail})")
            }
        }
    }
}

impl<'a> CouncilRunOutcome<'a> {
    /// Classify a shipped-driver error, preferring the typed mapping.
    ///
    /// Written as a constructor rather than left to each call site because a
    /// caller that forgot to downcast would silently lose `Unpriced` — the one
    /// carrier the 22-02 census said the lifted taxonomy had to ADD, on the
    /// grounds that folding it into `Blocked` loses why the run never started.
    #[must_use]
    pub fn from_anyhow(error: &'a anyhow::Error) -> Self {
        error.downcast_ref::<CouncilError>().map_or_else(
            || Self::DriverFailed {
                detail: error.to_string(),
            },
            Self::Failed,
        )
    }
}

/// What an Anvil climb produced.
///
/// Modelled like [`FleetOutcome`] rather than as a bare `Result<_, &EngineError>`
/// for exactly the reason stated there: the SHIPPED forge entry point
/// (`drive_climb_full`) fails with `ForgeError` — no gate detected, workspace
/// leased, receipt unbindable — and none of those is an [`EngineError`]. Calling
/// a missing gate `EngineError::Builder` to satisfy a signature would be a
/// fabricated terminal, so the driver's own failures get a carrier and land in
/// `Blocked` with a stated reason.
///
/// Added when Anvil's production verb was attached to the Goal: before that the
/// only caller was a test, which could always produce an `EngineError` because it
/// was calling the engine directly rather than the forge.
#[derive(Debug)]
pub enum AnvilOutcome<'a> {
    /// The climb ran and produced a terminal state.
    Climbed(&'a ClimbOutcome),
    /// The climb engine itself failed.
    EngineFailed(&'a EngineError),
    /// The forge failed around the climb, for a stated reason.
    ForgeFailed { detail: String },
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
    pub fn from_fleet(owner: LoopOwner<FleetTag>, outcome: FleetOutcome<'_>) -> Self {
        let terminal = match outcome {
            FleetOutcome::Dispatched(shards) => GoalTerminalState::PartiallyCompleted {
                completed: shards.iter().map(|s| s.successes as u64).sum(),
                failed: shards.iter().map(|s| s.failures as u64).sum(),
            },
            FleetOutcome::Failed(FleetError::Timeout(_)) => GoalTerminalState::TimedOut,
            FleetOutcome::Failed(other) => GoalTerminalState::Blocked {
                reason: other.to_string(),
            },
            FleetOutcome::DriverFailed { detail } => GoalTerminalState::Blocked { reason: detail },
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
    pub fn from_council(owner: LoopOwner<CouncilTag>, result: CouncilRunOutcome<'_>) -> Self {
        let terminal = match result {
            CouncilRunOutcome::DriverFailed { detail } => {
                return owner.terminate(GoalTerminalState::Blocked { reason: detail });
            }
            // Two arms rather than an or-pattern: the driver path boxes its
            // outcome and the manual path does not. Same counting rule.
            CouncilRunOutcome::Ran(CouncilRunResult::Council { outcome, .. }) => {
                council_counts(outcome)
            }
            CouncilRunOutcome::RanManual(outcome) => council_counts(outcome),
            // One model's answer, no roster to count and no verification owner.
            CouncilRunOutcome::Ran(CouncilRunResult::Direct { .. }) => {
                GoalTerminalState::NeedsEscalation
            }
            CouncilRunOutcome::Ran(CouncilRunResult::Cancelled) => GoalTerminalState::Cancelled,
            CouncilRunOutcome::Failed(CouncilError::UnpriceableRoster) => {
                GoalTerminalState::Unpriced {
                    detail: CouncilError::UnpriceableRoster.to_string(),
                }
            }
            CouncilRunOutcome::Failed(error @ CouncilError::OverBudget { .. }) => {
                GoalTerminalState::Exhausted {
                    kind: ExhaustionKind::Resource,
                    attempts: 0,
                    detail: error.to_string(),
                }
            }
            CouncilRunOutcome::Failed(error @ CouncilError::DailyBudgetExhausted { .. }) => {
                GoalTerminalState::Exhausted {
                    kind: ExhaustionKind::Resource,
                    attempts: 0,
                    detail: error.to_string(),
                }
            }
            // Usable proposals ran short: a quality wall, not a resource one.
            CouncilRunOutcome::Failed(error @ CouncilError::InsufficientProposals { got, .. }) => {
                GoalTerminalState::Exhausted {
                    kind: ExhaustionKind::Quality,
                    attempts: *got as u64,
                    detail: error.to_string(),
                }
            }
            CouncilRunOutcome::Failed(error @ CouncilError::NoResolver) => {
                GoalTerminalState::Blocked {
                    reason: error.to_string(),
                }
            }
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
    /// use wcore_agent::goal::strategy::{AnvilOutcome, AnvilTag, LoopOwner, StrategyTermination};
    /// use wcore_agent::orchestration::anvil::engine::ClimbOutcome;
    ///
    /// fn retry_wrapper(owner: LoopOwner<AnvilTag>, outcomes: &[ClimbOutcome]) {
    ///     // `owner` is moved by the first adapter call, so a second iteration
    ///     // is a use-after-move. This is Success Criterion 3's "no nested
    ///     // verification/retry owner", enforced by the borrow checker.
    ///     for outcome in outcomes {
    ///         let _ = StrategyTermination::from_anvil(
    ///             owner,
    ///             AnvilOutcome::Climbed(outcome),
    ///             1,
    ///         );
    ///     }
    /// }
    /// ```
    ///
    /// # A non-Anvil claim cannot be handed to the Anvil adapter
    ///
    /// ```compile_fail
    /// use wcore_agent::goal::strategy::{AnvilOutcome, CouncilTag, LoopOwner, StrategyTermination};
    /// use wcore_agent::orchestration::anvil::engine::ClimbOutcome;
    ///
    /// fn wrong_tag(owner: LoopOwner<CouncilTag>, outcome: &ClimbOutcome) {
    ///     // Council's verification owner is a model judge. The type system,
    ///     // not a reviewer, is what stops it borrowing Anvil's evidence path.
    ///     let _ = StrategyTermination::from_anvil(
    ///         owner,
    ///         AnvilOutcome::Climbed(outcome),
    ///         1,
    ///     );
    /// }
    /// ```
    pub fn from_anvil(
        owner: LoopOwner<AnvilTag>,
        result: AnvilOutcome<'_>,
        required_stability: u32,
    ) -> Self {
        let outcome = match result {
            AnvilOutcome::Climbed(outcome) => outcome,
            // Aborted before it could produce a terminal state through the
            // normal path — surfaced, never swallowed.
            AnvilOutcome::EngineFailed(error) => {
                return owner.terminate(GoalTerminalState::Blocked {
                    reason: error.to_string(),
                });
            }
            AnvilOutcome::ForgeFailed { detail } => {
                return owner.terminate(GoalTerminalState::Blocked { reason: detail });
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

/// Chosen/skipped proposers, from the council's own provenance.
///
/// `skipped` survives as `failed`: a council answer fused from 2 of 5 proposers
/// because 3 were keyless is not the same artifact as a unanimous one, and the
/// difference is invisible in `final_text`.
fn council_counts(outcome: &CouncilOutcome) -> GoalTerminalState {
    GoalTerminalState::PartiallyCompleted {
        completed: outcome.chosen_from.len() as u64,
        failed: outcome.skipped.len() as u64,
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

/// Wall clock in unix milliseconds.
fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
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
    lease_ms: u64,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

/// Default liveness window for a loop-owner claim.
///
/// Matches the `goal run --lease` default. A claim is evidence that an owner is
/// alive; once it expires a successor may supersede it, which is what stops a
/// `kill -9` from deadlocking the Goal permanently.
pub const DEFAULT_LOOP_OWNER_LEASE_MS: u64 = 60_000;

impl GoalLoop {
    #[must_use]
    pub fn new(kernel: GoalKernel) -> Self {
        Self {
            kernel,
            lease_ms: DEFAULT_LOOP_OWNER_LEASE_MS,
            clock: Arc::new(now_unix_ms),
        }
    }

    /// How long this driver's claims stay evidence that an owner is alive.
    #[must_use]
    pub fn with_lease_ms(mut self, lease_ms: u64) -> Self {
        self.lease_ms = lease_ms.max(1);
        self
    }

    /// Replace the wall clock. Exists so lease expiry is EXERCISABLE rather than
    /// only reasoned about — a lease whose expiry path is only reachable by
    /// sleeping in a test is a lease whose expiry path never gets tested.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        self.clock = clock;
        self
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
        let epoch =
            self.kernel
                .claim_loop_owner(goal_id, S::STRATEGY, (self.clock)(), self.lease_ms)?;
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
    use std::sync::atomic::{AtomicU64, Ordering};

    use wcore_types::goal::{GoalAuthorityRequest, GoalAuthoritySnapshot, LoopPolicy};

    use crate::session_journal::SessionJournal;

    use super::*;

    // ── The loop-owner lease ────────────────────────────────────────────────
    //
    // These live here rather than in `tests/goal_strategy_test.rs` because they
    // exercise `claim_loop_owner` / `finish_loop_owner`, which are `pub(crate)`
    // on purpose: `finish_loop_owner` taking a raw terminal is the one route
    // that would bypass the adapter chain, so it must not be public just to
    // make a test convenient.
    //
    // The lease exists because a live kill found its absence. Before it, a
    // `kill -9` left the claim held by nobody and the Goal could never be
    // claimed or terminated again.

    /// A clock the test moves by hand. Sleeping to reach an expiry is how a
    /// lease's expiry path ends up never being tested at all.
    #[derive(Clone)]
    struct TestClock(Arc<AtomicU64>);

    impl TestClock {
        fn new(start: u64) -> Self {
            Self(Arc::new(AtomicU64::new(start)))
        }
        fn now(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
        fn advance(&self, ms: u64) {
            self.0.fetch_add(ms, Ordering::SeqCst);
        }
        fn as_fn(&self) -> Arc<dyn Fn() -> u64 + Send + Sync> {
            let inner = Arc::clone(&self.0);
            Arc::new(move || inner.load(Ordering::SeqCst))
        }
    }

    fn direct_snapshot() -> GoalAuthoritySnapshot {
        wcore_types::goal::resolve_goal_authority(
            &GoalAuthorityRequest {
                requested_limits: std::collections::BTreeMap::new(),
                strategy: GoalStrategy::Direct,
                loop_policy: LoopPolicy::Once,
            },
            &std::collections::BTreeMap::new(),
            "parent-envelope-digest",
        )
    }

    fn leased_loop(path: &std::path::Path, clock: &TestClock, lease_ms: u64) -> (GoalLoop, GoalId) {
        let driver = GoalLoop::new(GoalKernel::new(
            SessionJournal::open(path, "goal-strategy-lease").expect("journal opens"),
        ))
        .with_lease_ms(lease_ms)
        .with_clock(clock.as_fn());
        let id = GoalId::new("g-lease");
        driver
            .kernel()
            .open_goal(&id, "close criterion 3", &direct_snapshot(), 1_700_000_000)
            .expect("goal opens");
        (driver, id)
    }

    #[tokio::test]
    async fn a_live_claim_refuses_a_second_owner_but_an_expired_one_is_superseded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.journal");
        let clock = TestClock::new(1_000_000);
        let (driver, id) = leased_loop(&path, &clock, 60_000);

        // An owner that died holding its claim: claim, then never finish.
        driver
            .kernel()
            .claim_loop_owner(&id, GoalStrategy::Direct, clock.now(), 60_000)
            .expect("first claim");

        let nested = driver
            .run_direct(&id, |owner| async move {
                StrategyTermination::from_direct(owner, DirectOutcome::Completed)
            })
            .await;
        assert!(nested.is_err(), "a live claim must refuse a second owner");
        assert!(!driver.kernel().goal(&id).unwrap().unwrap().is_terminal());

        // Past the lease, the dead owner's claim is superseded rather than
        // refused forever — otherwise a kill -9 deadlocks the Goal permanently.
        clock.advance(60_001);
        let reclaimed = driver
            .run_direct(&id, |owner| async move {
                StrategyTermination::from_direct(owner, DirectOutcome::Completed)
            })
            .await;
        assert!(
            reclaimed.is_ok(),
            "an expired claim must be reclaimable: {reclaimed:?}"
        );
        assert!(driver.kernel().goal(&id).unwrap().unwrap().is_terminal());
    }

    #[tokio::test]
    async fn a_superseded_owner_cannot_terminate_the_goal_it_no_longer_holds() {
        // Reclaim is safe only because the epoch fences it. A predecessor that
        // comes back to life after its lease expired holds epoch N while the
        // live owner holds N+1, and the finish requires the LIVE epoch.
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.journal");
        let clock = TestClock::new(1_000_000);
        let (driver, id) = leased_loop(&path, &clock, 1_000);

        let stale_epoch = driver
            .kernel()
            .claim_loop_owner(&id, GoalStrategy::Direct, clock.now(), 1_000)
            .expect("first claim");
        clock.advance(1_001);
        let live_epoch = driver
            .kernel()
            .claim_loop_owner(&id, GoalStrategy::Direct, clock.now(), 1_000)
            .expect("successor claims");
        assert_ne!(stale_epoch, live_epoch);

        let stale =
            driver
                .kernel()
                .finish_loop_owner(&id, stale_epoch, GoalTerminalState::CriteriaChecked);
        assert!(
            stale.is_err(),
            "a superseded owner must not terminate the Goal"
        );
        assert!(!driver.kernel().goal(&id).unwrap().unwrap().is_terminal());
    }

    #[tokio::test]
    async fn the_lease_liveness_detector_can_report_both_answers() {
        // Falsification: if `is_live_at` always said "expired", the refusal
        // above would go green for the wrong reason. Assert the detector on both
        // sides of the boundary.
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.journal");
        let clock = TestClock::new(1_000_000);
        let (driver, id) = leased_loop(&path, &clock, 5_000);
        driver
            .kernel()
            .claim_loop_owner(&id, GoalStrategy::Direct, clock.now(), 5_000)
            .expect("claim");

        let owner = driver
            .kernel()
            .goal(&id)
            .unwrap()
            .unwrap()
            .loop_owner
            .expect("claim is live");
        assert!(owner.is_live_at(clock.now()), "live before expiry");
        assert!(!owner.is_live_at(clock.now() + 5_001), "expired after it");
    }

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
