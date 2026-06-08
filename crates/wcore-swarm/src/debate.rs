//! Multi-round debate over a sequence of [`DebateRound`]s.
//!
//! [`Debate::evaluate`] walks the rounds in order and returns the first
//! round whose [`Consensus::majority`] outcome is `Agreed`. If no round
//! converges, returns [`DebateOutcome::Diverged`] with the final
//! round's outcome and the total round count.
//!
//! The data carried in each `DebateRound` is opaque to this crate: the
//! orchestrator is responsible for replaying round-N-1 outputs into
//! round-N's worker briefs (that's a `wcore-agent` concern, not a
//! `wcore-swarm` one). All we do here is consolidate the results.

use serde::{Deserialize, Serialize};

use crate::SwarmResult;
use crate::consensus::{Consensus, ConsensusOutcome};
use crate::scorer::Scorer;

/// One round of a multi-round debate. `round` is a 1-indexed sequence
/// number reported back in [`DebateOutcome::Converged`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateRound {
    pub round: u32,
    pub results: Vec<SwarmResult>,
}

/// Outcome of [`Debate::evaluate`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DebateOutcome {
    /// The earliest round whose consensus was `Agreed`. `value` is the
    /// winning bucket; `converged_at_round` is the round number from
    /// the corresponding [`DebateRound`].
    Converged {
        value: String,
        converged_at_round: u32,
    },
    /// No round reached `Agreed`. `final_round_outcome` is the
    /// `Consensus::majority` outcome of the LAST round (or an empty
    /// `Disputed` if `rounds == 0`); `rounds` is the total round count.
    Diverged {
        final_round_outcome: ConsensusOutcome,
        rounds: u32,
    },
}

/// Stateless namespace for debate consolidation.
pub struct Debate;

impl Debate {
    /// Walk `rounds` in order; the FIRST round whose consensus is
    /// `Agreed` short-circuits with [`DebateOutcome::Converged`]. If no
    /// round agrees, returns [`DebateOutcome::Diverged`] carrying the
    /// final round's outcome and total round count.
    ///
    /// On an empty `rounds` slice, returns `Diverged` with an empty
    /// `Disputed` final outcome and `rounds = 0`.
    pub fn evaluate<S: Scorer>(rounds: &[DebateRound], scorer: &S) -> DebateOutcome {
        let mut final_outcome = ConsensusOutcome::Disputed {
            top_k: vec![],
            total: 0,
        };
        for r in rounds {
            let outcome = Consensus::majority(&r.results, scorer);
            if let ConsensusOutcome::Agreed { ref value, .. } = outcome {
                return DebateOutcome::Converged {
                    value: value.clone(),
                    converged_at_round: r.round,
                };
            }
            final_outcome = outcome;
        }
        DebateOutcome::Diverged {
            final_round_outcome: final_outcome,
            rounds: rounds.len() as u32,
        }
    }
}
