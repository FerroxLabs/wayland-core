//! Acceptance test: the loop is not non-functional. We don't assert the loop
//! always beats the parent (W10A scoring is the trust boundary; if the parent
//! is already optimal, no child can improve it), but we DO assert the loop
//! runs to a defined termination reason and never produces a NaN baseline.
//!
//! The "best is at least as good as the parent" invariant holds because
//! `best_candidate` is only set when a child strictly beats the parent's
//! `combined` score — otherwise it stays `None`. Either way the loop
//! terminates cleanly.

use wcore_evolve::evolve::TerminationReason;

#[tokio::test]
async fn fixture_evolve_terminates_cleanly_with_defined_reason() {
    // The fixture builder is `#[cfg(test)]`-gated and accessible via
    // `wcore_evolve::evolve::EvolveParams::fixture_degraded_then_recovers()`
    // INSIDE the crate. To call it from an integration test we use a public
    // constructor — fall back to building the params inline below.
    let params = build_fixture_params();
    let outcome = wcore_evolve::evolve(params).await.expect("evolve ok");
    let baseline = outcome.parent_score.dimensions.combined;
    let best = outcome
        .best_candidate
        .as_ref()
        .map(|c| c.score.dimensions.combined)
        .unwrap_or(baseline);

    assert!(
        best >= baseline - 1e-6,
        "best fell below parent baseline: parent={baseline:.3} best={best:.3}"
    );
    assert!(
        matches!(
            outcome.termination,
            TerminationReason::GenerationCeiling
                | TerminationReason::Plateau { .. }
                | TerminationReason::NoImprovementFound
        ),
        "unexpected termination: {:?}",
        outcome.termination
    );
    assert!(outcome.generations_run > 0, "no generations ran");
}

// Fixture builder duplicated here because `#[cfg(test)]` items inside the
// crate are not visible from integration tests. Same shape as the in-crate
// helper.
fn build_fixture_params() -> wcore_evolve::EvolveParams {
    use std::sync::Arc;
    use std::time::Duration;
    use wcore_eval::DefaultScorer;
    use wcore_evolve::generation::BudgetStub;
    use wcore_evolve::mutator::{
        Mutator, Paraphrase, ParaphraseProvider, Precondition, Reorder, SwapSynonym,
    };
    use wcore_evolve::{EvolveParams, NullTraceSink};
    use wcore_skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

    const PARENT_BODY: &str = include_str!("fixtures/parent_skill.md");
    const PARAPHRASE_FIXTURE: &str = include_str!("fixtures/paraphrase/run-0-child-0.txt");

    struct FixtureProvider {
        response: &'static str,
    }
    impl ParaphraseProvider for FixtureProvider {
        fn paraphrase_blocking(&self, _body: &str, _seed_token: &str) -> Result<String, String> {
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

    let graveyard_dir = tempfile::tempdir().expect("tempdir").keep();

    EvolveParams {
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
