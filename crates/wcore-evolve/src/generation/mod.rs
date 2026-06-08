//! W10B Generation runner.
//!
//! Given a parent skill body and a `MutationSeed`, produce N candidates,
//! score each via W10A's `wcore_eval::Scorer`, and obey a `Budget` so the
//! loop can terminate mid-generation. Per-child `tokio::time::timeout`
//! wraps mutator + score so a hung Paraphrase provider cannot leak past
//! the loop-level budget (F5 audit fix).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use tokio::time::timeout;
use wcore_eval::{Candidate, ScoreOutcome, Scorer};
use wcore_observability::trace::TurnTrace;
use wcore_skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

use crate::error::EvolveError;
use crate::mutator::{Mutation, MutationSeed, Mutator};

/// Per-generation runtime parameters.
pub struct GenerationParams {
    pub fan_out: u32,
    pub budget: Box<dyn Budget + Send + Sync>,
    pub run_id: String,
    pub generation: u32,
    pub parent_hash: String,
    pub parent_body: String,
    /// Per-child wall-clock cap for mutator + score. F5 audit fix: ensures a
    /// hung Paraphrase provider can't blow past the loop-level budget.
    pub child_timeout: Duration,
}

/// Budget trait the Generation runner queries between children. The host loop
/// (Task 3 `evolve()`) typically wraps the wcore-agent `ExecutionBudget`; the
/// `BudgetStub` defined below covers integration-test cases.
pub trait Budget: Send + Sync {
    /// Returns `true` if the loop should stop. Cheap to call between children.
    fn is_exhausted(&self) -> bool;
    /// Called each time a unit of work completes.
    fn tick(&self);
}

/// In-test budget helper. Public so integration tests under `tests/` can
/// construct it. Production code injects an adapter over `ExecutionBudget`
/// from `wcore-agent`.
pub struct BudgetStub {
    /// `None` means unbounded. `Some(n)` means terminate after `n` ticks.
    max_steps: Option<u32>,
    ticks: AtomicU32,
}

impl BudgetStub {
    pub fn unbounded() -> Self {
        Self {
            max_steps: None,
            ticks: AtomicU32::new(0),
        }
    }

    pub fn with_max_steps(max: u32) -> Self {
        Self {
            max_steps: Some(max),
            ticks: AtomicU32::new(0),
        }
    }
}

impl Budget for BudgetStub {
    fn is_exhausted(&self) -> bool {
        match self.max_steps {
            None => false,
            Some(max) => self.ticks.load(Ordering::Relaxed) >= max,
        }
    }

    fn tick(&self) {
        self.ticks.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminationCause {
    Completed,
    BudgetExhausted,
    ChildTimedOut,
}

pub struct ScoredCandidate {
    pub mutation: Mutation,
    pub score: ScoreOutcome,
    pub child_index: u32,
}

pub struct GenerationResult {
    pub scored: Vec<ScoredCandidate>,
    pub terminated_by: TerminationCause,
    /// Per-child timeouts, by `child_index`. Surfaced so the loop can log them.
    pub timed_out_children: Vec<u32>,
}

pub struct Generation {
    mutator: Arc<dyn Mutator>,
    scorer: Arc<dyn Scorer + Send + Sync>,
}

impl Generation {
    pub fn new(mutator: Arc<dyn Mutator>, scorer: Box<dyn Scorer + Send + Sync>) -> Self {
        Self {
            mutator,
            scorer: Arc::from(scorer),
        }
    }

    /// Build a `wcore_eval::Candidate` from a mutated skill body. The
    /// Mutation only carries the body string; the runner stamps a
    /// SkillMetadata around it for Scorer consumption.
    fn candidate_from_mutation(
        mutation: &Mutation,
        parent_skill: &SkillMetadata,
        trace: Option<TurnTrace>,
        child_index: u32,
        generation: u32,
    ) -> Candidate {
        let mut skill = parent_skill.clone();
        skill.content_length = mutation.body.len();
        skill.content = mutation.body.clone();
        Candidate {
            skill,
            trace,
            source_filename: format!("gen-{generation}-child-{child_index}.md"),
        }
    }

    /// Minimal stub SkillMetadata for Task 2 tests. Task 3 plumbs the real
    /// parent skill via EvolveParams.seed_skill.
    fn stub_parent_skill(name: &str, body: &str) -> SkillMetadata {
        SkillMetadata {
            name: name.to_string(),
            display_name: None,
            description: String::new(),
            has_user_specified_description: false,
            allowed_tools: vec![],
            argument_hint: None,
            argument_names: vec![],
            when_to_use: None,
            version: None,
            model: None,
            disable_model_invocation: false,
            user_invocable: true,
            execution_context: ExecutionContext::Inline,
            agent: None,
            effort: None,
            shell: None,
            paths: vec![],
            artifacts: vec![],
            hooks_raw: None,
            source: SkillSource::Bundled,
            loaded_from: LoadedFrom::Bundled,
            content_length: body.len(),
            content: body.to_string(),
            skill_root: None,
        }
    }

    pub async fn run(&self, p: GenerationParams) -> Result<GenerationResult, EvolveError> {
        let mut scored: Vec<ScoredCandidate> = Vec::with_capacity(p.fan_out as usize);
        let mut timed_out_children: Vec<u32> = Vec::new();
        let parent_skill = Self::stub_parent_skill("parent", &p.parent_body);

        for child_index in 0..p.fan_out {
            if p.budget.is_exhausted() {
                return Ok(GenerationResult {
                    scored,
                    terminated_by: TerminationCause::BudgetExhausted,
                    timed_out_children,
                });
            }

            let seed = MutationSeed::new(p.parent_hash.clone(), p.generation, child_index);

            // F5 audit fix: per-child timeout wraps mutator + score so a slow
            // Paraphrase LLM call cannot hang the generation.
            let mutator = Arc::clone(&self.mutator);
            let scorer = Arc::clone(&self.scorer);
            let parent_body = p.parent_body.clone();
            let parent_skill_ref = parent_skill.clone();
            let generation_index = p.generation;

            let child_work = async move {
                // Mutator may block (Paraphrase provider does sync IO); wrap
                // in spawn_blocking so the timeout future can preempt it.
                let mutation =
                    tokio::task::spawn_blocking(move || mutator.mutate(&parent_body, seed))
                        .await
                        .map_err(|e| EvolveError::Io(std::io::Error::other(e.to_string())))??;

                let candidate = Self::candidate_from_mutation(
                    &mutation,
                    &parent_skill_ref,
                    None,
                    child_index,
                    generation_index,
                );
                let score = scorer.score(&candidate);
                Ok::<ScoredCandidate, EvolveError>(ScoredCandidate {
                    mutation,
                    score,
                    child_index,
                })
            };

            match timeout(p.child_timeout, child_work).await {
                Ok(Ok(sc)) => scored.push(sc),
                Ok(Err(EvolveError::Mutation(_))) => {
                    // Mutation failure (NoStepsSection etc.) is non-fatal;
                    // skip this child and continue the generation.
                }
                Ok(Err(other)) => return Err(other),
                Err(_elapsed) => {
                    timed_out_children.push(child_index);
                    // Continue; one slow child should not abort the generation.
                }
            }
            p.budget.tick();
        }

        let terminated_by =
            if !timed_out_children.is_empty() && timed_out_children.len() as u32 == p.fan_out {
                TerminationCause::ChildTimedOut
            } else {
                TerminationCause::Completed
            };
        Ok(GenerationResult {
            scored,
            terminated_by,
            timed_out_children,
        })
    }
}
