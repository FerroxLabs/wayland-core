//! W10B GEPA multi-generation loop.
//!
//! Iterates generations of mutated children scored through `wcore_eval::Scorer`,
//! emits one `EvolutionEventTrace` per scored child via the injected
//! `TraceSink`, archives losers to the graveyard, and terminates on:
//!   - generation ceiling
//!   - plateau (rolling-window improvement below `min_delta`)
//!   - budget exhaustion

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use wcore_eval::{ScoreOutcome, Scorer};
use wcore_observability::trace::EvolutionEventTrace;
use wcore_skills::types::SkillMetadata;

use crate::error::EvolveError;
use crate::generation::{Budget, Generation, GenerationParams, ScoredCandidate, TerminationCause};
use crate::mutator::{MutationKind, Mutator};

pub mod graveyard;
pub mod plateau;

pub use graveyard::{GraveyardError, LoserEntry};
pub use plateau::{PlateauDetector, PlateauError};

/// Sink for evolution events. The CLI injects a `GatedTraceSink` that drops
/// events when `capabilities.gepa_enabled` is off; production callers compose
/// over their existing `ProtocolSink`.
pub trait TraceSink: Send + Sync {
    fn emit_evolution_event(&self, event: &EvolutionEventTrace);
}

/// No-op sink — useful for offline runs / tests that don't care about events.
pub struct NullTraceSink;

impl TraceSink for NullTraceSink {
    fn emit_evolution_event(&self, _event: &EvolutionEventTrace) {}
}

/// Why the loop terminated. Surfaced in `EvolveOutcome` so callers can decide
/// whether to retry, promote, or report.
#[derive(Debug, Clone)]
pub enum TerminationReason {
    GenerationCeiling,
    Plateau {
        window: usize,
        min_delta: f64,
    },
    BudgetExhausted,
    NoImprovementFound,
    /// Wave RC (audit MAJOR #9) — a generation produced a non-finite
    /// top score (NaN / ±inf). The plateau detector refuses such
    /// samples (NaN comparisons are always false; the loop would
    /// otherwise spin to `max_generations`). The loop fails loud
    /// rather than masking the corrupt scorer output.
    ScoreInvalid {
        /// Zero-based index of the generation whose top score was
        /// rejected.
        generation: u32,
        /// IEEE 754 bit pattern of the offending score (retained for
        /// diagnostics; NaN payloads survive round-tripping).
        score_bits: u64,
    },
}

/// All inputs the loop needs. `seed_skill` is the W9 F10 draft (or active
/// catalog skill) being evolved.
///
/// Budget + Scorer are `Arc<dyn ...>` (not `Box`) so the loop can share them
/// across per-generation `Generation::new` calls without re-instantiating.
pub struct EvolveParams {
    pub seed_skill: SkillMetadata,
    pub max_generations: u32,
    pub fan_out: u32,
    /// Default `3` per High 1 audit fix; window MUST be >= number of mutator
    /// strategies in rotation.
    pub plateau_window: usize,
    pub plateau_min_delta: f64,
    pub budget: Arc<dyn Budget + Send + Sync>,
    pub graveyard_root: PathBuf,
    pub run_id: String,
    pub run_seed: String,
    pub child_timeout: Duration,
    /// Scorer to evaluate every candidate. Typically
    /// `Arc::new(DefaultScorer::default())` (W10A LOCKED constants) but
    /// injected so tests can stub.
    pub scorer: Arc<dyn Scorer + Send + Sync>,
    /// Mutators in fixed round-robin order. CLI default:
    /// `[Paraphrase, Reorder, SwapSynonym, Precondition]`.
    pub mutators: Vec<Arc<dyn Mutator>>,
    /// Emits an `evolution_event` per scored child. The sink itself does NOT
    /// know about `capabilities.gepa_enabled`; gating is the host boundary's
    /// responsibility (Task 4 CLI injects a `GatedTraceSink`).
    pub trace_sink: Arc<dyn TraceSink>,
}

impl EvolveParams {
    /// Build a small fixture for the end-to-end acceptance test. Uses the
    /// four deterministic mutators (Paraphrase falls back to a fixture
    /// provider so no real LLM is touched).
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn fixture_degraded_then_recovers() -> Self {
        use crate::generation::BudgetStub;
        use crate::mutator::{Paraphrase, ParaphraseProvider, Precondition, Reorder, SwapSynonym};
        use wcore_eval::DefaultScorer;
        use wcore_skills::types::{ExecutionContext, LoadedFrom, SkillSource};

        const PARENT_BODY: &str = include_str!("../../tests/fixtures/parent_skill.md");
        const PARAPHRASE_FIXTURE: &str =
            include_str!("../../tests/fixtures/paraphrase/run-0-child-0.txt");

        struct FixtureProvider {
            response: &'static str,
        }
        impl ParaphraseProvider for FixtureProvider {
            fn paraphrase_blocking(
                &self,
                _body: &str,
                _seed_token: &str,
            ) -> Result<String, String> {
                Ok(self.response.to_string())
            }
        }

        let seed_skill = SkillMetadata {
            name: "refactor-imports".into(),
            display_name: None,
            description: "Reorder Rust import groups".into(),
            has_user_specified_description: true,
            allowed_tools: vec![],
            argument_hint: None,
            argument_names: vec![],
            when_to_use: Some("After editing imports".into()),
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
            content_length: PARENT_BODY.len(),
            content: PARENT_BODY.to_string(),
            skill_root: None,
        };

        let provider: Arc<dyn ParaphraseProvider> = Arc::new(FixtureProvider {
            response: PARAPHRASE_FIXTURE,
        });
        let mutators: Vec<Arc<dyn Mutator>> = vec![
            Arc::new(Paraphrase {
                provider,
                temperature: 0.0,
            }),
            Arc::new(Reorder),
            Arc::new(SwapSynonym),
            Arc::new(Precondition),
        ];

        let graveyard_dir = std::env::temp_dir().join(format!(
            "wcore-evolve-fixture-graveyard-{}",
            std::process::id()
        ));

        Self {
            seed_skill,
            max_generations: 4,
            fan_out: 4,
            plateau_window: 3,
            plateau_min_delta: 0.01,
            budget: Arc::new(BudgetStub::unbounded()),
            graveyard_root: graveyard_dir,
            run_id: "fixture-run".into(),
            run_seed: "fixture-seed".into(),
            child_timeout: Duration::from_secs(5),
            scorer: Arc::new(DefaultScorer::default()),
            mutators,
            trace_sink: Arc::new(NullTraceSink),
        }
    }
}

/// Loop result. `best_candidate` is `None` only when no child improved over
/// the parent's score across every generation.
pub struct EvolveOutcome {
    pub parent_score: ScoreOutcome,
    pub best_candidate: Option<ScoredCandidate>,
    pub generations_run: u32,
    pub termination: TerminationReason,
    pub all_scored: Vec<ScoredCandidate>,
}

fn body_excerpt(body: &str) -> String {
    const MAX_LEN: usize = 512;
    if body.len() <= MAX_LEN {
        return body.to_string();
    }
    let mut end = MAX_LEN;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    body.get(..end).unwrap_or("").to_string()
}

fn score_parent(parent_skill: &SkillMetadata, scorer: &(dyn Scorer + Send + Sync)) -> ScoreOutcome {
    let candidate = wcore_eval::Candidate {
        skill: parent_skill.clone(),
        trace: None,
        source_filename: format!("{}.md", parent_skill.name),
    };
    scorer.score(&candidate)
}

fn mutation_kind_str(k: MutationKind) -> &'static str {
    match k {
        MutationKind::Paraphrase => "Paraphrase",
        MutationKind::Reorder => "Reorder",
        MutationKind::SwapSynonym => "SwapSynonym",
        MutationKind::Precondition => "Precondition",
    }
}

/// Thin Scorer wrapper so we can hand `Generation::new` a `Box<dyn Scorer>`
/// that delegates to the shared `Arc<dyn Scorer>` without consuming it.
struct SharedScorer(Arc<dyn Scorer + Send + Sync>);
impl Scorer for SharedScorer {
    fn score(&self, candidate: &wcore_eval::Candidate) -> ScoreOutcome {
        self.0.score(candidate)
    }
}

/// Thin Budget wrapper so per-generation `GenerationParams::budget` can
/// share the outer `Arc<dyn Budget>` without consuming it.
struct SharedBudget(Arc<dyn Budget + Send + Sync>);
impl Budget for SharedBudget {
    fn is_exhausted(&self) -> bool {
        self.0.is_exhausted()
    }
    fn tick(&self) {
        self.0.tick();
    }
}

/// W10B main entry: evolve the seed skill across `max_generations` until a
/// terminating condition fires. Returns the best scored child found.
pub async fn evolve(params: EvolveParams) -> Result<EvolveOutcome, EvolveError> {
    let parent_score = score_parent(&params.seed_skill, params.scorer.as_ref());
    let parent_hash = format!("{}@{}", params.seed_skill.name, params.run_seed);
    let parent_id = params.seed_skill.name.clone();
    let parent_body = params.seed_skill.content.clone();

    let mut plateau_detector =
        PlateauDetector::new(params.plateau_window, params.plateau_min_delta);
    let mut all_scored: Vec<ScoredCandidate> = Vec::new();
    let mut best_candidate: Option<ScoredCandidate> = None;
    let mut generations_run: u32 = 0;
    let mut termination = TerminationReason::GenerationCeiling;

    if params.mutators.is_empty() {
        return Ok(EvolveOutcome {
            parent_score,
            best_candidate: None,
            generations_run: 0,
            termination: TerminationReason::NoImprovementFound,
            all_scored,
        });
    }

    for generation_index in 0..params.max_generations {
        if params.budget.is_exhausted() {
            termination = TerminationReason::BudgetExhausted;
            break;
        }

        // Round-robin mutator pick.
        let mutator_idx = (generation_index as usize) % params.mutators.len();
        let mutator = match params.mutators.get(mutator_idx) {
            Some(m) => Arc::clone(m),
            None => break,
        };

        let gen_params = GenerationParams {
            fan_out: params.fan_out,
            budget: Box::new(SharedBudget(Arc::clone(&params.budget))),
            run_id: params.run_id.clone(),
            generation: generation_index,
            parent_hash: parent_hash.clone(),
            parent_body: parent_body.clone(),
            child_timeout: params.child_timeout,
        };

        let scorer_for_gen: Box<dyn Scorer + Send + Sync> =
            Box::new(SharedScorer(Arc::clone(&params.scorer)));
        let runner = Generation::new(mutator, scorer_for_gen);
        let result = runner.run(gen_params).await?;

        generations_run = generation_index + 1;

        // Determine top score for plateau bookkeeping BEFORE iterating
        // (we move out of `result.scored` below).
        let top_score = result
            .scored
            .iter()
            .map(|sc| sc.score.dimensions.combined)
            .fold(f64::NEG_INFINITY, f64::max);

        let baseline_for_retention = best_candidate
            .as_ref()
            .map(|c| c.score.dimensions.combined)
            .unwrap_or(parent_score.dimensions.combined);

        for sc in result.scored {
            let child_combined = sc.score.dimensions.combined;
            let retained = child_combined > baseline_for_retention
                && child_combined > parent_score.dimensions.combined;
            let mutation_kind = mutation_kind_str(sc.mutation.kind).to_string();
            let child_id = format!("{}/{}/{}", params.run_id, generation_index, sc.child_index);
            let event = EvolutionEventTrace::new(
                params.run_id.clone(),
                generation_index,
                parent_id.clone(),
                child_id,
                mutation_kind.clone(),
                child_combined,
                retained,
            );
            params.trace_sink.emit_evolution_event(&event);

            if retained {
                // Replace the previous best; archive the displaced winner so
                // every non-current child has a graveyard entry.
                if let Some(displaced) = best_candidate.take() {
                    let displaced_kind = mutation_kind_str(displaced.mutation.kind).to_string();
                    let entry = LoserEntry {
                        run_id: params.run_id.clone(),
                        generation: generation_index,
                        child_index: displaced.child_index,
                        parent_id: parent_id.clone(),
                        mutation_kind: displaced_kind,
                        score: displaced.score.dimensions.combined,
                        body_excerpt: body_excerpt(&displaced.mutation.body),
                    };
                    graveyard::write(&params.graveyard_root, &entry)?;
                    all_scored.push(displaced);
                }
                best_candidate = Some(sc);
            } else {
                let entry = LoserEntry {
                    run_id: params.run_id.clone(),
                    generation: generation_index,
                    child_index: sc.child_index,
                    parent_id: parent_id.clone(),
                    mutation_kind,
                    score: child_combined,
                    body_excerpt: body_excerpt(&sc.mutation.body),
                };
                graveyard::write(&params.graveyard_root, &entry)?;
                all_scored.push(sc);
            }
        }

        match result.terminated_by {
            TerminationCause::BudgetExhausted => {
                termination = TerminationReason::BudgetExhausted;
                break;
            }
            TerminationCause::ChildTimedOut => {
                // Every child timed out this generation. Skip plateau update
                // (no meaningful top score) and try the next mutator. This
                // intentionally does NOT trip the plateau detector — an
                // absent sample is not "no improvement", it is "no signal",
                // and forcing the detector to swallow a stand-in value
                // would either short-circuit the loop (Plateau) or admit
                // a synthetic floor that distorts later windows.
                continue;
            }
            TerminationCause::Completed => {}
        }

        // Wave RC (audit MAJOR #9): the scorer may yield a non-finite
        // top score (divide-by-zero, malformed LLM scorer output,
        // empty `result.scored` collapsing the fold seed). NEG_INFINITY
        // and NaN would silently jam the plateau detector. Fail loud:
        // record the offending bit pattern + generation and break out
        // with `TerminationReason::ScoreInvalid` so the host sees a
        // structured failure rather than a hang.
        match plateau_detector.push(top_score) {
            Ok(()) => {}
            Err(plateau::PlateauError::NonFiniteScore { bits }) => {
                termination = TerminationReason::ScoreInvalid {
                    generation: generation_index,
                    score_bits: bits,
                };
                break;
            }
        }
        if plateau_detector.should_terminate() {
            termination = TerminationReason::Plateau {
                window: params.plateau_window,
                min_delta: params.plateau_min_delta,
            };
            break;
        }
    }

    if best_candidate.is_none() && all_scored.is_empty() && generations_run == 0 {
        return Err(EvolveError::BudgetExhaustedEmpty);
    }

    // Demote `GenerationCeiling` to `NoImprovementFound` if no child was
    // ever retained. Wave RC: do NOT overwrite a more specific
    // termination (Plateau, BudgetExhausted, ScoreInvalid) that the
    // loop body already chose — those reasons carry strictly more
    // information than "no improvement", and overwriting `ScoreInvalid`
    // in particular would re-introduce the audit MAJOR #9 silent-failure
    // mode at the outcome boundary.
    if best_candidate.is_none() && matches!(termination, TerminationReason::GenerationCeiling) {
        termination = TerminationReason::NoImprovementFound;
    }

    Ok(EvolveOutcome {
        parent_score,
        best_candidate,
        generations_run,
        termination,
        all_scored,
    })
}
