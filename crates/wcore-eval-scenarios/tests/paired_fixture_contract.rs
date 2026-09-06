use std::collections::BTreeMap;
use wcore_eval_scenarios::paired::{Family, PairedTask};

fn task() -> PairedTask {
    PairedTask {
        schema: 1,
        family: Family::AnswerWithoutAction,
        case_id: "answer-1".into(),
        seed: "a".repeat(64),
        sentinel: "W16-CONTROL".into(),
        files: BTreeMap::from([("KEEP.txt".into(), "unchanged".into())]),
        prompts: vec!["Reply exactly W16-CONTROL:46 without tools.".into()],
        max_cost_usd: 0.02,
        max_time_secs: 60,
    }
}

#[test]
fn paired_fixture_is_materialized_identically_and_bound_to_prompt() {
    let task = task();
    let root = tempfile::tempdir().unwrap();
    task.prepare_peer(&root.path().join("peer")).unwrap();
    let loaded = PairedTask::load(&root.path().join("peer/task.json")).unwrap();
    assert_eq!(task.digest().unwrap(), loaded.digest().unwrap());
    assert_eq!(
        std::fs::read_to_string(root.path().join("peer/workspace/KEEP.txt")).unwrap(),
        "unchanged"
    );
    assert!(task.prepare_peer(&root.path().join("peer")).is_err());
    let mut changed = loaded;
    changed.prompts[0].push_str(" Different instruction.");
    assert_ne!(task.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn invalid_fixture_authority_budget_and_tiny_long_session_are_refused() {
    for path in [
        "../outside",
        ".git/config",
        ".wayland-core/config.toml",
        "auth.json",
    ] {
        let mut value = task();
        value.files.insert(path.into(), "not allowed".into());
        assert!(value.validate().is_err(), "{path}");
    }
    for budget in [0.0, -0.1, f64::NAN, f64::INFINITY, 0.250_001, 0.30] {
        let mut value = task();
        value.max_cost_usd = budget;
        assert!(value.validate().is_err());
    }
    let mut value = task();
    value.family = Family::LongSessionConstraintRetention;
    assert!(
        value.validate().is_err(),
        "a canary cannot masquerade as a long session"
    );
}

#[tokio::test]
async fn paired_oracle_checks_actual_protected_artifacts_and_exact_answer() {
    let task = task();
    let root = tempfile::tempdir().unwrap();
    task.prepare_peer(&root.path().join("peer")).unwrap();
    let workspace = root.path().join("peer/workspace");
    task.check_artifacts(&workspace, "W16-CONTROL:46")
        .await
        .unwrap();
    assert!(
        task.check_artifacts(&workspace, "W16-CONTROL:47")
            .await
            .is_err()
    );
    std::fs::write(workspace.join("KEEP.txt"), "changed").unwrap();
    assert!(
        task.check_artifacts(&workspace, "W16-CONTROL:46")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn unmetered_paid_bridge_refuses_before_any_candidate_spawn() {
    let provider = wcore_eval_scenarios::providers::ProviderConfig::new(
        wcore_eval_scenarios::providers::ProviderId::OpenAI,
        "gpt-6-astra",
    );
    let root = tempfile::tempdir().unwrap();
    let error = task()
        .run_core(
            &provider,
            &root.path().join("missing-binary"),
            &root.path().join("output"),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("paid calls are blocked"));
    assert!(!root.path().join("output").exists());
}

#[test]
fn paired_memory_budget_is_one_whole_task_ceiling() {
    let mut value = task();
    value.family = Family::RelevantMemoryRecall;
    value.prompts = vec![
        "Store the delivery fact.".into(),
        "Recall the delivery fact.".into(),
    ];
    value.max_cost_usd = 0.25;
    let scenario = value.scenario().unwrap();
    assert_eq!(scenario.max_total_cost_usd, 0.25);
    let root = tempfile::tempdir().unwrap();
    value.prepare_peer(&root.path().join("peer")).unwrap();
    let loaded = PairedTask::load(&root.path().join("peer/task.json")).unwrap();
    assert_eq!(loaded.max_cost_usd, 0.25);
    assert_eq!(loaded.prompts, value.prompts);
    assert_eq!(loaded.family, Family::RelevantMemoryRecall);
    value.max_cost_usd = 0.30;
    assert!(
        value.scenario().is_err(),
        "three native processes do not grant three task budgets"
    );
}
