//! M3.6 — Skills prioritizer session-start tests.
//!
//! Verifies that `SkillPrioritizer::priority_order` reorders a flat list of
//! skill names by procedural-partition Beta-mean score, and that
//! `SkillCatalog::reorder_by` mutates ref order without losing entries.

use std::sync::Arc;

use wcore_memory::api::MemoryApi;
use wcore_memory::memory::Memory;
use wcore_skills::prioritizer::SkillPrioritizer;

#[tokio::test]
async fn recently_successful_skill_surfaces_before_unused() {
    let mem = Memory::open_in_memory().await.unwrap();
    let api: Arc<dyn MemoryApi> = Arc::new(mem);

    // Pre-seed: "good" with 5 successes, "bad" with 5 failures.
    for _ in 0..5 {
        api.record_skill_use("good", true, 1).await.unwrap();
    }
    for _ in 0..5 {
        api.record_skill_use("bad", false, 1).await.unwrap();
    }

    let prio = SkillPrioritizer::new(Arc::clone(&api));
    let order = prio
        .priority_order(
            &["unused".to_string(), "good".to_string(), "bad".to_string()],
            10,
        )
        .await;

    // "good" has Beta-mean ~0.857 → first.
    // "unused" (no procedural row) → middle (kept in input order among untelemetried).
    // "bad" has Beta-mean ~0.143 → last.
    assert_eq!(
        order[0], "good",
        "successful skill must surface first; got {order:?}"
    );
    assert_eq!(
        order.last().unwrap(),
        "bad",
        "failing skill must sink to bottom; got {order:?}"
    );
}

#[tokio::test]
async fn prioritizer_with_no_telemetry_preserves_input_order() {
    let mem = Memory::open_in_memory().await.unwrap();
    let api: Arc<dyn MemoryApi> = Arc::new(mem);
    let prio = SkillPrioritizer::new(api);
    let order = prio
        .priority_order(&["a".to_string(), "b".to_string(), "c".to_string()], 10)
        .await;
    assert_eq!(
        order,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[tokio::test]
async fn prioritizer_respects_min_uses_filter() {
    // M3.6: prioritizer asks top_procedures for min_uses=1. A skill that
    // has been recorded at least once must be eligible; a never-recorded
    // name stays untelemetried.
    let mem = Memory::open_in_memory().await.unwrap();
    let api: Arc<dyn MemoryApi> = Arc::new(mem);
    api.record_skill_use("one_use", true, 1).await.unwrap();

    let prio = SkillPrioritizer::new(api);
    let order = prio
        .priority_order(&["never".to_string(), "one_use".to_string()], 10)
        .await;

    // one_use beat never (one_use has Beta-mean = 2/3 ≈ 0.667 ≥ 0.5; never has no row).
    assert_eq!(order, vec!["one_use".to_string(), "never".to_string()]);
}

#[test]
fn skill_catalog_reorder_by_moves_named_first() {
    use wcore_skills::refs::{SkillCatalog, SkillRef};
    use wcore_skills::types::{LoadedFrom, SkillSource};

    fn mk(name: &str) -> SkillRef {
        SkillRef {
            name: name.into(),
            display_name: None,
            description: String::new(),
            when_to_use: None,
            paths: vec![],
            source: SkillSource::User,
            loaded_from: LoadedFrom::Skills,
            file_path: std::path::PathBuf::from(format!("<virtual:{name}>")),
            content_length_hint: 0,
            user_invocable: true,
            disable_model_invocation: false,
            has_artifacts: false,
            inline_content: None,
        }
    }
    let mut cat = SkillCatalog::from_refs(vec![mk("a"), mk("b"), mk("c")]);
    cat.reorder_by(&["c".into(), "a".into(), "b".into()]);
    let names: Vec<String> = cat.iter_names().collect();
    assert_eq!(
        names,
        vec!["c".to_string(), "a".to_string(), "b".to_string()]
    );
}

#[test]
fn skill_catalog_reorder_by_unknown_names_ignored_unlisted_preserved() {
    use wcore_skills::refs::{SkillCatalog, SkillRef};
    use wcore_skills::types::{LoadedFrom, SkillSource};

    fn mk(name: &str) -> SkillRef {
        SkillRef {
            name: name.into(),
            display_name: None,
            description: String::new(),
            when_to_use: None,
            paths: vec![],
            source: SkillSource::User,
            loaded_from: LoadedFrom::Skills,
            file_path: std::path::PathBuf::from(format!("<virtual:{name}>")),
            content_length_hint: 0,
            user_invocable: true,
            disable_model_invocation: false,
            has_artifacts: false,
            inline_content: None,
        }
    }
    let mut cat = SkillCatalog::from_refs(vec![mk("a"), mk("b"), mk("c"), mk("d")]);
    // priority list mentions "c" first, then a nonexistent "ghost", then "a".
    // Expected: c first, a second, ghost dropped, then b + d (the original
    // order of unlisted entries).
    cat.reorder_by(&["c".into(), "ghost".into(), "a".into()]);
    let names: Vec<String> = cat.iter_names().collect();
    assert_eq!(
        names,
        vec![
            "c".to_string(),
            "a".to_string(),
            "b".to_string(),
            "d".to_string()
        ]
    );
}
