//! M4.1 — property: `BenchScorer::score` is deterministic and pure.
//!
//! Mirrors `tests/scoring_determinism.rs` for the legacy `DefaultScorer`.
//! Scoring is driven by an inline `BenchRunner` that returns canned
//! outputs keyed off `case.frontmatter.id`. No LLM, no file I/O on the
//! hot path; given identical inputs the combined score is bit-equal
//! across invocations.

use std::sync::Arc;

use wcore_eval::bench::{BenchCategory, BenchCorpus, BenchScorer, CannedBenchRunner};
use wcore_eval::corpus::Verdict;
use wcore_eval::scorer::Scorer;
use wcore_skills::types::{ExecutionContext, LoadedFrom, SkillMetadata, SkillSource};

fn baseline_skill() -> SkillMetadata {
    SkillMetadata {
        name: "bench-fixture".into(),
        display_name: None,
        description: "A bench fixture skill used by determinism tests.".into(),
        has_user_specified_description: true,
        allowed_tools: vec![],
        argument_hint: None,
        argument_names: vec![],
        when_to_use: Some("Used only by BenchScorer determinism tests.".into()),
        version: None,
        model: Some("claude-sonnet-4-7".into()),
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
        content: "Bench fixture body. $ARGUMENTS".into(),
        content_length: 32,
        skill_root: None,
    }
}

fn load_corpus() -> BenchCorpus {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    BenchCorpus::load(root).expect("bench corpus must load")
}

#[test]
fn bench_scorer_is_deterministic_across_two_invocations() {
    let corpus = load_corpus();
    let scorer = BenchScorer::new(corpus, Arc::new(CannedBenchRunner::new()));
    let candidate = wcore_eval::Candidate {
        skill: baseline_skill(),
        trace: None,
        source_filename: "bench-fixture".into(),
    };
    let a = scorer.score(&candidate);
    let b = scorer.score(&candidate);
    assert_eq!(
        a.dimensions.combined.to_bits(),
        b.dimensions.combined.to_bits(),
        "f64 bit-equality required for determinism"
    );
    assert_eq!(
        a.dimensions.outcome.to_bits(),
        b.dimensions.outcome.to_bits()
    );
    assert_eq!(a.predicted, b.predicted);
}

#[test]
fn bench_scorer_all_passing_predicts_good() {
    let corpus = load_corpus();
    let scorer = BenchScorer::new(corpus, Arc::new(CannedBenchRunner::new()));
    let candidate = wcore_eval::Candidate {
        skill: baseline_skill(),
        trace: None,
        source_filename: "bench-fixture".into(),
    };
    let out = scorer.score(&candidate);
    assert!(
        (out.dimensions.combined - 1.0).abs() < 1e-9,
        "all 30 cases should pass with the canned runner; got combined={}",
        out.dimensions.combined
    );
    assert_eq!(out.predicted, Verdict::Good);
}

#[test]
fn bench_scorer_majority_failing_predicts_bad() {
    let corpus = load_corpus();
    // Force every case to return a sentinel that no strategy accepts.
    let mut runner = CannedBenchRunner::new();
    for c in &corpus.cases {
        runner = runner.with_override(&c.frontmatter.id, "##UNMATCHABLE_SENTINEL##");
    }
    let scorer = BenchScorer::new(corpus, Arc::new(runner));
    let candidate = wcore_eval::Candidate {
        skill: baseline_skill(),
        trace: None,
        source_filename: "bench-fixture".into(),
    };
    let out = scorer.score(&candidate);
    assert!(
        out.dimensions.combined < 0.65,
        "below-cutoff score expected; got {}",
        out.dimensions.combined
    );
    assert_eq!(out.predicted, Verdict::Bad);
}

#[test]
fn bench_scorer_combined_is_pass_ratio() {
    let corpus = load_corpus();
    // Force exactly the first arithmetic case to fail; everything else
    // should pass. With 30 cases and 1 failure, combined = 29/30.
    let first_arith_id = corpus
        .cases
        .iter()
        .find(|c| c.frontmatter.category == BenchCategory::Arithmetic)
        .map(|c| c.frontmatter.id.clone())
        .expect("must have at least one arithmetic case");
    let runner = CannedBenchRunner::new().with_override(&first_arith_id, "##UNMATCHABLE##");
    let scorer = BenchScorer::new(corpus, Arc::new(runner));
    let candidate = wcore_eval::Candidate {
        skill: baseline_skill(),
        trace: None,
        source_filename: "bench-fixture".into(),
    };
    let out = scorer.score(&candidate);
    let expected = 29.0_f64 / 30.0_f64;
    assert!(
        (out.dimensions.combined - expected).abs() < 1e-12,
        "expected combined={expected}, got {}",
        out.dimensions.combined
    );
    // 29/30 ≈ 0.9667, well above the 0.65 cutoff.
    assert_eq!(out.predicted, Verdict::Good);
}
