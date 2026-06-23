//! W9 acceptance: F10 + F11 + PUM wired end-to-end against a real
//! in-memory wcore-memory dispatcher.

use serde_json::json;
use std::sync::Arc;
use wcore_memory::api::MemoryApi;
use wcore_memory::partition::UserModelInferencer;
use wcore_memory::v2_types::{AccessToken, ProcedureStatus, Tier};
use wcore_observability::trace::{ToolCallTrace, TurnTrace};
use wcore_skills::curate::Curator;
use wcore_skills::draft::{DraftWriter, PatternDetector};

fn turn(i: usize, tools: &[(&str, serde_json::Value)]) -> TurnTrace {
    TurnTrace {
        turn: i,
        model: "test".into(),
        provider: "test".into(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read: 0,
        cache_write: 0,
        cache_hit_rate: 0.0,
        cost_usd: 0.0,
        tool_calls: tools
            .iter()
            .enumerate()
            .map(|(j, (n, v))| ToolCallTrace::new(format!("c-{i}-{j}"), n.to_string(), v.clone()))
            .collect(),
        hook_actions: vec![],
        source_product: "wcore-agent".into(),
        agent_run_id: String::new(),
    }
}

#[tokio::test]
async fn w9_happy_path_f10_f11_pum() {
    let tmp = tempfile::tempdir().unwrap();
    let dispatcher = wcore_memory::open_for_test(tmp.path()).await.unwrap();
    let mem: Arc<dyn MemoryApi> = Arc::new(dispatcher);

    // 1. Synthetic traces: same 5-tool sequence 3 times.
    let seq = vec![
        ("Grep", json!({"pattern": "fn run"})),
        ("Read", json!({"path": "a.rs"})),
        ("Edit", json!({"path": "a.rs", "old": "x", "new": "y"})),
        ("Bash", json!({"cmd": "cargo test"})),
        ("Bash", json!({"cmd": "cargo clippy"})),
    ];
    let traces: Vec<TurnTrace> = (0..3).map(|i| turn(i, &seq)).collect();

    // 2. F10: detect + stage.
    let cands = PatternDetector::default().detect(&traces);
    assert_eq!(cands.len(), 1);
    let writer = DraftWriter::new(mem.clone());
    let _pid = writer
        .stage(&cands[0], AccessToken::MainAgent)
        .await
        .unwrap();

    // Confirm staged procedure exists and was named with the auto- prefix.
    let staged = mem
        .list_procedures(Tier::Project, AccessToken::System)
        .await
        .unwrap();
    assert!(
        staged
            .iter()
            .any(|p| p.name.starts_with("auto-") && matches!(p.status, ProcedureStatus::Staged))
    );

    // 3. F11: curator runs and emits a (possibly empty) report.
    let report = Curator::new(mem.clone()).run().await.unwrap();
    // Single staged draft → nothing to dedupe.
    assert_eq!(report.dedupes.len(), 0);

    // 4. PUM: infer + persist user-model deltas.
    let inf = UserModelInferencer::new(mem.clone());
    let written = inf.infer_and_persist(&traces).await.unwrap();
    assert!(written >= 4, "PUM writes at least 4 keys");

    // 5. Round-trip the user model.
    let model = mem.user_model(AccessToken::System).await.unwrap();
    let keys: Vec<&str> = model.entries.iter().map(|e| e.key.as_str()).collect();
    assert!(keys.contains(&"preferences.tool_order"));
    assert!(keys.contains(&"tool_habits.recent_top5"));
    assert!(keys.contains(&"language.primary"));
    assert!(keys.contains(&"working_hours.local_tz_window"));
}
