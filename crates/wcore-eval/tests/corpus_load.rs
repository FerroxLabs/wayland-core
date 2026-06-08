//! Structural validation of the reference corpus. Does NOT score anything.

use wcore_eval::{Corpus, ExpectedOutcome};

const CRATE_ROOT: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn corpus_loads_exactly_60_cases() {
    let c = Corpus::load(CRATE_ROOT.as_ref()).expect("load");
    assert_eq!(
        c.len(),
        60,
        "W10A corpus must have exactly 60 cases (30 good + 30 bad)"
    );
}

#[test]
fn corpus_is_balanced_30_good_30_bad() {
    let c = Corpus::load(CRATE_ROOT.as_ref()).expect("load");
    let good = c
        .cases
        .iter()
        .filter(|c| c.frontmatter.expected_outcome == ExpectedOutcome::Good)
        .count();
    let bad = c
        .cases
        .iter()
        .filter(|c| c.frontmatter.expected_outcome == ExpectedOutcome::Bad)
        .count();
    assert_eq!(good, 30, "must be exactly 30 good cases");
    assert_eq!(bad, 30, "must be exactly 30 bad cases");
}

#[test]
fn every_case_references_existing_skill_body() {
    let root = std::path::PathBuf::from(CRATE_ROOT);
    let c = Corpus::load(&root).expect("load");
    for case in &c.cases {
        let p = root
            .join("data/skills")
            .join(format!("{}.md", case.frontmatter.skill_body));
        assert!(
            p.exists(),
            "missing skill body {} for case {}",
            p.display(),
            case.frontmatter.id
        );
    }
}

#[test]
fn trace_paired_cases_reference_existing_trace_fixtures() {
    let root = std::path::PathBuf::from(CRATE_ROOT);
    let c = Corpus::load(&root).expect("load");
    for case in c
        .cases
        .iter()
        .filter(|c| c.frontmatter.trace_fixture.is_some())
    {
        let fixture = case.frontmatter.trace_fixture.as_deref().unwrap();
        let p = root.join("data/traces").join(fixture);
        assert!(
            p.exists(),
            "missing trace fixture {} for case {}",
            p.display(),
            case.frontmatter.id
        );
    }
}

#[test]
fn case_ids_are_unique() {
    let c = Corpus::load(CRATE_ROOT.as_ref()).expect("load");
    let mut ids: Vec<&str> = c.cases.iter().map(|c| c.frontmatter.id.as_str()).collect();
    ids.sort();
    let len = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), len, "duplicate case ids detected");
}
