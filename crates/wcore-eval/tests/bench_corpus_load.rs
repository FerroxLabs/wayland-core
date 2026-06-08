//! M4.1 — load + invariant checks for the 30-case mini-bench corpus.
//!
//! Asserts: exactly 30 cases, the 8/8/8/6 category split documented in
//! `docs/superpowers/plans/milestone-4-learning-loop.md` §M4.1, and
//! globally-unique case ids.

use wcore_eval::bench::{BenchCategory, BenchCorpus};

#[test]
fn bench_corpus_loads_30_cases_4_categories() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus = BenchCorpus::load(root).expect("bench corpus must load");
    assert_eq!(
        corpus.cases.len(),
        30,
        "M4.1 invariant: exactly 30 frozen cases"
    );

    let routing = corpus
        .cases
        .iter()
        .filter(|c| c.frontmatter.category == BenchCategory::ToolRouting)
        .count();
    let arith = corpus
        .cases
        .iter()
        .filter(|c| c.frontmatter.category == BenchCategory::Arithmetic)
        .count();
    let recall = corpus
        .cases
        .iter()
        .filter(|c| c.frontmatter.category == BenchCategory::Recall)
        .count();
    let fileops = corpus
        .cases
        .iter()
        .filter(|c| c.frontmatter.category == BenchCategory::FileOps)
        .count();

    // 8 + 8 + 8 + 6 = 30. Skew tilts toward routing/arith/recall because
    // file-ops cases each need a sandbox temp dir setup; 6 is the upper
    // bound the harness can afford in <30s wall-clock without parallelism.
    assert_eq!(routing, 8, "tool-routing case count drift");
    assert_eq!(arith, 8, "arithmetic case count drift");
    assert_eq!(recall, 8, "recall case count drift");
    assert_eq!(fileops, 6, "file-ops case count drift");
}

#[test]
fn bench_corpus_ids_are_unique() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus = BenchCorpus::load(root).unwrap();
    let mut ids: Vec<_> = corpus
        .cases
        .iter()
        .map(|c| c.frontmatter.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 30, "ids must be unique");
}

#[test]
fn bench_corpus_cases_sorted_alphabetically() {
    // Matches Corpus::load behaviour at corpus.rs:98 — stable order so
    // tests + report-back are reproducible.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus = BenchCorpus::load(root).unwrap();
    let sources: Vec<_> = corpus
        .cases
        .iter()
        .map(|c| c.source.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let mut sorted = sources.clone();
    sorted.sort();
    assert_eq!(sources, sorted, "cases must be returned in sorted order");
}

#[test]
fn bench_corpus_every_case_has_non_empty_prompt_and_rationale() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let corpus = BenchCorpus::load(root).unwrap();
    for c in &corpus.cases {
        assert!(
            !c.frontmatter.prompt.trim().is_empty(),
            "case {} has empty prompt",
            c.frontmatter.id
        );
        assert!(
            !c.frontmatter.rationale.trim().is_empty(),
            "case {} has empty rationale",
            c.frontmatter.id
        );
        assert!(
            c.frontmatter.timeout_secs > 0,
            "case {} has zero timeout",
            c.frontmatter.id
        );
    }
}
