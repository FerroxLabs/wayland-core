//! The durable Goal kernel: the sole writer of Goal transitions.
//!
//! ## What "sole writer" means here, structurally
//!
//! Every transition is an append into the EXISTING F12 session-journal chain,
//! and every read goes back through the EXISTING replay path. There is no second
//! store, no second reducer, no second cursor and no sidecar file. The kernel
//! holds no authoritative state of its own: `goal()` reduces the chain, and if a
//! field cannot be reconstructed by replay it does not exist.
//!
//! `SessionJournal::append` REFUSES every Goal variant, so a caller holding a
//! journal handle cannot mint a transition beside this type. That is the
//! structural half; this type is the only thing that can produce one.
//!
//! ## Why appends go through `append_built_from_head`
//!
//! Each transition's validity is bound to the state it was derived from — an
//! iteration number must be the successor of the committed count, and a
//! terminal transition must be the first. Reading state through `state()` and
//! appending afterwards leaves a window in which another writer advances the
//! head, and the reducer then rejects the append for a collision the caller had
//! no way to observe. `append_built_from_head` captures and appends inside one
//! critical section instead. This is the same primitive Phase 21 repaired for
//! the parallel-sibling budget TOCTOU (`1eb9b5ca`); 22-03's claim-model record
//! names it as the seam any ledger should re-enter rather than mint a second
//! one, and this kernel re-enters it rather than building a substitute.

use wcore_protocol::events::RecoveryCursor;
use wcore_types::goal::{
    GoalAuthoritySnapshot, GoalId, GoalStrategy, GoalTerminalState, VerifiedTerminal, WaitKind,
};

use crate::session_journal::{
    GoalLifecycle, GoalState, JournalError, ReducedSessionState, SessionEvent, SessionJournal,
};

use super::record::GoalAuthorityRecord;

/// What a fresh process found when it picked up an existing Goal.
#[derive(Debug, Clone)]
pub enum GoalRecovery {
    /// The Goal was mid-flight and its envelope was reconstructed exactly.
    Resumed {
        snapshot: GoalAuthoritySnapshot,
        iterations_started: u32,
        resume_count: u32,
    },
    /// The Goal had already terminated. Not an error, and not resumable.
    AlreadyTerminal { terminal: GoalTerminalState },
    /// The envelope could not be reconstructed, so the Goal was parked durably
    /// for explicit operator resolution rather than resumed under a
    /// possibly-wider envelope.
    Blocked { terminal: GoalTerminalState },
}

/// The sole writer of durable Goal transitions.
#[derive(Clone)]
pub struct GoalKernel {
    journal: SessionJournal,
}

impl GoalKernel {
    #[must_use]
    pub fn new(journal: SessionJournal) -> Self {
        Self { journal }
    }

    /// Authorize a new durable Goal.
    ///
    /// The effective envelope is SNAPSHOTTED onto the record rather than
    /// re-derived on resume. Phase 21 owns the anti-amplification property over
    /// the live seams; this records what the envelope WAS at the transition so a
    /// resume restores the same one instead of inventing a second intersection
    /// primitive.
    pub fn open_goal(
        &self,
        goal_id: &GoalId,
        objective: &str,
        snapshot: &GoalAuthoritySnapshot,
        opened_at_unix_ms: u64,
    ) -> Result<RecoveryCursor, JournalError> {
        let authority = GoalAuthorityRecord::from_snapshot(snapshot);
        let goal_id_owned = goal_id.as_str().to_owned();
        let objective = objective.to_owned();
        self.append(move |_state| {
            Ok(SessionEvent::GoalOpened {
                goal_id: goal_id_owned.clone(),
                objective: objective.clone(),
                authority: authority.clone(),
                opened_at_unix_ms,
            })
        })?;
        self.require_cursor(goal_id)
    }

    /// Consume one iteration of the Goal's authorized loop bound.
    ///
    /// The iteration number is derived from the committed head inside the
    /// writer lock, so two callers racing cannot both consume iteration N.
    pub fn start_iteration(&self, goal_id: &GoalId) -> Result<RecoveryCursor, JournalError> {
        let id = goal_id.as_str().to_owned();
        self.append(move |state| {
            let goal = require_goal(state, &id)?;
            Ok(SessionEvent::GoalIterationStarted {
                goal_id: id.clone(),
                iteration: goal.iterations_started.saturating_add(1),
            })
        })?;
        self.require_cursor(goal_id)
    }

    /// Park the Goal on something named.
    pub fn begin_wait(
        &self,
        goal_id: &GoalId,
        wait: WaitKind,
    ) -> Result<RecoveryCursor, JournalError> {
        let id = goal_id.as_str().to_owned();
        self.append(move |_state| {
            Ok(SessionEvent::GoalWaitBegun {
                goal_id: id.clone(),
                wait: wait.clone(),
            })
        })?;
        self.require_cursor(goal_id)
    }

    /// Resolve the wait and return to running.
    pub fn resume_from_wait(&self, goal_id: &GoalId) -> Result<RecoveryCursor, JournalError> {
        let id = goal_id.as_str().to_owned();
        self.append(move |_state| {
            Ok(SessionEvent::GoalWaitResolved {
                goal_id: id.clone(),
            })
        })?;
        self.require_cursor(goal_id)
    }

    /// Terminate the Goal in a canonical category.
    ///
    /// This path CANNOT produce a verified terminal state. `Verified` is
    /// reachable only through [`Self::terminate_verified`], which requires a
    /// [`VerifiedTerminal`] — a type with no deserialization route. Refusing it
    /// here as well as in the reducer means neither a caller mistake nor a
    /// hand-built journal record can mint the reserved stamp.
    pub fn terminate(
        &self,
        goal_id: &GoalId,
        terminal: GoalTerminalState,
    ) -> Result<RecoveryCursor, JournalError> {
        if terminal.is_verified() {
            return Err(JournalError::InvalidTransition(
                "a verified terminal state requires host-observed gate evidence".to_owned(),
            ));
        }
        self.append_terminal(goal_id, terminal)
    }

    /// Terminate the Goal as verified, on host-observed deterministic evidence.
    pub fn terminate_verified(
        &self,
        goal_id: &GoalId,
        verified: VerifiedTerminal,
    ) -> Result<RecoveryCursor, JournalError> {
        self.append_terminal(goal_id, verified.into_terminal())
    }

    /// Claim the Goal's ONE loop owner for `strategy` (F22C).
    ///
    /// `pub(crate)`, not public: a caller must go through [`GoalLoop`], which is
    /// the only thing that mints the [`LoopOwner`] token the adapters consume.
    /// A claim taken without that token would be a claim nobody could ever
    /// finish, which is a durable deadlock, not a feature.
    ///
    /// The strategy is NOT a parameter the caller chooses freely — the reducer
    /// refuses a claim naming anything other than the strategy on the durable
    /// Goal record.
    ///
    /// [`GoalLoop`]: super::strategy::GoalLoop
    /// [`LoopOwner`]: super::strategy::LoopOwner
    pub(crate) fn claim_loop_owner(
        &self,
        goal_id: &GoalId,
        strategy: GoalStrategy,
        now_unix_ms: u64,
        lease_ms: u64,
    ) -> Result<u32, JournalError> {
        let id = goal_id.as_str().to_owned();
        let lease_expires_unix_ms = now_unix_ms.saturating_add(lease_ms.max(1));
        self.append(move |state| {
            let goal = require_goal(state, &id)?;
            Ok(SessionEvent::GoalLoopOwnerClaimed {
                goal_id: id.clone(),
                strategy,
                // Derived from the committed head INSIDE the writer lock, the
                // same reason `start_iteration` does: two callers racing must
                // not both believe they hold epoch N.
                epoch: goal.loop_owner_epochs.saturating_add(1),
                now_unix_ms,
                lease_expires_unix_ms,
            })
        })?;
        let epoch = self
            .goal(goal_id)?
            .and_then(|goal| goal.loop_owner.map(|owner| owner.epoch))
            .ok_or_else(|| {
                JournalError::InvalidTransition(format!("goal {goal_id} holds no loop owner claim"))
            })?;
        Ok(epoch)
    }

    /// THE canonical Goal terminal transition (F22C, Success Criterion 3).
    ///
    /// This is the only function in the codebase that can terminate a Goal
    /// under a live loop owner, and the only value it accepts is a
    /// [`StrategyTermination`] — which has no public constructor other than the
    /// five engine adapters. The chain is therefore closed at both ends:
    ///
    /// * upward, `SessionJournal::append` refuses every `Goal*` variant, so only
    ///   this kernel can mint the record;
    /// * downward, the reducer refuses a plain `GoalTerminated` while a claim is
    ///   live, so a caller cannot route around this function;
    /// * sideways, `StrategyTermination` cannot be built except by adapting one
    ///   of the five engines' real outcomes.
    ///
    /// [`StrategyTermination`]: super::strategy::StrategyTermination
    pub(crate) fn finish_loop_owner(
        &self,
        goal_id: &GoalId,
        epoch: u32,
        terminal: GoalTerminalState,
    ) -> Result<RecoveryCursor, JournalError> {
        let id = goal_id.as_str().to_owned();
        self.append(move |_state| {
            Ok(SessionEvent::GoalLoopOwnerFinished {
                goal_id: id.clone(),
                epoch,
                terminal: terminal.clone(),
            })
        })?;
        self.require_cursor(goal_id)
    }

    fn append_terminal(
        &self,
        goal_id: &GoalId,
        terminal: GoalTerminalState,
    ) -> Result<RecoveryCursor, JournalError> {
        let id = goal_id.as_str().to_owned();
        self.append(move |_state| {
            Ok(SessionEvent::GoalTerminated {
                goal_id: id.clone(),
                terminal: terminal.clone(),
            })
        })?;
        self.require_cursor(goal_id)
    }

    /// Pick up a Goal in a fresh process after a crash.
    ///
    /// `parent_envelope_digest` is the envelope THIS process can produce. If it
    /// does not match the one the Goal was authorized against, the Goal is
    /// parked durably as `AuthorityUnreconstructable` rather than resumed: a
    /// resume that re-derives its envelope resumes under whatever the parent
    /// happens to be now, which is exactly the widening this kernel must not
    /// reopen. The refusal is a durable transition, not an in-memory verdict, so
    /// the next process sees the park too.
    pub fn recover_with_parent_envelope(
        &self,
        goal_id: &GoalId,
        parent_envelope_digest: &str,
    ) -> Result<GoalRecovery, JournalError> {
        let goal = self
            .goal(goal_id)?
            .ok_or_else(|| JournalError::InvalidTransition(format!("unknown goal {goal_id}")))?;

        if let GoalLifecycle::Terminated { terminal } = goal.lifecycle {
            return Ok(GoalRecovery::AlreadyTerminal { terminal });
        }

        match goal
            .authority
            .reconstruct_against_parent(parent_envelope_digest)
        {
            Ok(snapshot) => {
                let id = goal_id.as_str().to_owned();
                self.append(move |state| {
                    let goal = require_goal(state, &id)?;
                    Ok(SessionEvent::GoalRunResumed {
                        goal_id: id.clone(),
                        resume_count: goal.resume_count.saturating_add(1),
                    })
                })?;
                let resumed = self
                    .goal(goal_id)?
                    .ok_or_else(|| JournalError::InvalidTransition("goal vanished".to_owned()))?;
                Ok(GoalRecovery::Resumed {
                    snapshot,
                    iterations_started: resumed.iterations_started,
                    resume_count: resumed.resume_count,
                })
            }
            Err(error) => {
                let terminal = GoalTerminalState::AuthorityUnreconstructable {
                    detail: error.detail.clone(),
                };
                self.append_terminal(goal_id, terminal.clone())?;
                Ok(GoalRecovery::Blocked { terminal })
            }
        }
    }

    /// The reduced Goal, replayed from the chain.
    pub fn goal(&self, goal_id: &GoalId) -> Result<Option<GoalState>, JournalError> {
        Ok(self.journal.state()?.goals.get(goal_id.as_str()).cloned())
    }

    /// The recovery cursor a reconnecting host resumes this Goal from.
    pub fn cursor(&self, goal_id: &GoalId) -> Result<Option<RecoveryCursor>, JournalError> {
        Ok(self.goal(goal_id)?.map(|goal| goal.cursor()))
    }

    fn require_cursor(&self, goal_id: &GoalId) -> Result<RecoveryCursor, JournalError> {
        self.cursor(goal_id)?.ok_or_else(|| {
            JournalError::InvalidTransition(format!("goal {goal_id} has no committed cursor"))
        })
    }

    fn append<F>(&self, build: F) -> Result<(), JournalError>
    where
        F: FnOnce(&ReducedSessionState) -> Result<SessionEvent, JournalError>,
    {
        self.journal.append_built_from_head(build).map(|_| ())
    }
}

fn require_goal<'a>(
    state: &'a ReducedSessionState,
    goal_id: &str,
) -> Result<&'a GoalState, JournalError> {
    state
        .goals
        .get(goal_id)
        .ok_or_else(|| JournalError::InvalidTransition(format!("unknown goal {goal_id}")))
}
