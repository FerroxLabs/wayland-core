//! Wave RC (audit MAJOR #9) — non-finite scorer outputs surface as
//! `TerminationReason::ScoreInvalid` within a single iteration, NOT
//! a runaway loop.
//!
//! Mechanism: inject a scorer that produces a NaN `combined`. The
//! evolution loop computes `top_score` over the generation's scored
//! children (NaN propagates through the fold-max), pushes it into the
//! `PlateauDetector`, and the detector rejects the sample. The loop
//! must:
//!
//! - terminate WITH a defined `ScoreInvalid` reason,
//! - record at least one generation run,
//! - NOT spin to `max_generations`.

use std::sync::Arc;
use std::time::Duration;

use wcore_eval::{Candidate, ScoreDimensions, ScoreOutcome, Scorer, Verdict};
use wcore_evolve::evolve::{PlateauDetector, PlateauError, TerminationReason};
use wcore_evolve::generation::BudgetStub;
use wcore_evolve::mutator::{Mutator, Precondition, Reorder, SwapSynonym};
use wcore_evolve::{EvolveParams, NullTraceSink};
use wcore_skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

/// Scorer that always returns a NaN combined score. Stand-in for a
/// real-world divide-by-zero or malformed LLM-scorer response.
struct NanScorer;

impl Scorer for NanScorer {
    fn score(&self, _candidate: &Candidate) -> ScoreOutcome {
        ScoreOutcome {
            dimensions: ScoreDimensions {
                outcome: 0.0,
                cost_penalty: 0.0,
                size_penalty: 0.0,
                combined: f64::NAN,
            },
            predicted: Verdict::Bad,
        }
    }
}

#[tokio::test]
async fn nan_top_score_surfaces_as_score_invalid_within_one_iteration() {
    // Three deterministic mutators (no Paraphrase to keep this offline).
    // max_generations is 10 so the test would HANG / SPIN if the
    // detector failed to surface the NaN.
    let mutators: Vec<Arc<dyn Mutator>> = vec![
        Arc::new(Reorder),
        Arc::new(SwapSynonym),
        Arc::new(Precondition),
    ];

    let graveyard_dir = tempfile::tempdir().expect("tempdir").keep();

    let params = EvolveParams {
        seed_skill: seed_skill(),
        max_generations: 10,
        fan_out: 4,
        plateau_window: 3,
        plateau_min_delta: 0.01,
        budget: Arc::new(BudgetStub::unbounded()),
        graveyard_root: graveyard_dir,
        run_id: "nan-run".into(),
        run_seed: "nan-seed".into(),
        child_timeout: Duration::from_secs(5),
        scorer: Arc::new(NanScorer),
        mutators,
        trace_sink: Arc::new(NullTraceSink),
    };

    let outcome = wcore_evolve::evolve(params).await.expect("evolve ok");

    match outcome.termination {
        TerminationReason::ScoreInvalid {
            generation,
            score_bits,
        } => {
            assert_eq!(generation, 0, "must fail loud on the first generation");
            assert!(
                f64::from_bits(score_bits).is_nan() || !f64::from_bits(score_bits).is_finite(),
                "score_bits must round-trip to a non-finite value"
            );
        }
        other => panic!("expected ScoreInvalid, got {other:?}"),
    }

    // ScoreInvalid surfaces AFTER the offending generation has been
    // processed, so generations_run is exactly 1 (we break immediately).
    assert_eq!(
        outcome.generations_run, 1,
        "must terminate after one generation, not spin"
    );
    assert!(
        outcome.generations_run < 10,
        "must NOT run to max_generations"
    );
}

#[test]
fn plateau_detector_returns_non_finite_error_directly() {
    let mut d = PlateauDetector::new(3, 0.01);
    // Push a few healthy samples first.
    d.push(0.5).expect("finite");
    d.push(0.6).expect("finite");
    let err = d.push(f64::NAN).expect_err("NaN must surface");
    assert!(matches!(err, PlateauError::NonFiniteScore { .. }));
}

#[test]
fn plateau_detector_all_timeout_zero_history_does_not_terminate() {
    // Documents the "every child timed out" edge case: an empty
    // history must NOT trigger plateau (we have no signal). The
    // evolve loop's `ChildTimedOut` arm skips `push` entirely, so the
    // detector simply remains in its "not enough samples yet" state.
    let d = PlateauDetector::new(3, 0.01);
    assert!(
        !d.should_terminate(),
        "empty plateau history must not declare a plateau"
    );
}

fn seed_skill() -> SkillMetadata {
    const PARENT_BODY: &str = include_str!("fixtures/parent_skill.md");
    SkillMetadata {
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
    }
}
