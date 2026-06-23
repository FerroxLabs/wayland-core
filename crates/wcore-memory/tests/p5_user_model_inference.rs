use serde_json::{Value, json};
use std::sync::Arc;
use wcore_memory::partition::UserModelInferencer;
use wcore_memory::v2_types::AccessToken;
use wcore_observability::trace::{ToolCallTrace, TurnTrace};

#[tokio::test]
async fn inferencer_compiles_and_can_be_constructed() {
    let tmp = tempfile::tempdir().unwrap();
    let mem = wcore_memory::open_for_test(tmp.path()).await.unwrap();
    let inf = UserModelInferencer::new(Arc::new(mem));
    let traces: Vec<TurnTrace> = vec![];
    let deltas = inf.infer(&traces);
    assert!(deltas.is_empty(), "no traces in => no deltas out");
}

fn turn_with_calls(turn: usize, tools: Vec<&str>) -> TurnTrace {
    TurnTrace {
        turn,
        model: "test".into(),
        provider: "test".into(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read: 0,
        cache_write: 0,
        cache_hit_rate: 0.0,
        cost_usd: 0.0,
        tool_calls: tools
            .into_iter()
            .enumerate()
            .map(|(i, n)| ToolCallTrace::new(format!("c-{turn}-{i}"), n.into(), json!({})))
            .collect(),
        hook_actions: vec![],
        source_product: "wcore-agent".into(),
        agent_run_id: String::new(),
    }
}

#[tokio::test]
async fn infer_emits_preferences_tool_order_top5() {
    let traces = vec![
        turn_with_calls(
            0,
            vec!["Read", "Read", "Edit", "Grep", "Bash", "Bash", "Bash"],
        ),
        turn_with_calls(1, vec!["Read", "Glob", "Edit"]),
    ];
    let tmp = tempfile::tempdir().unwrap();
    let mem = wcore_memory::open_for_test(tmp.path()).await.unwrap();
    let inf = UserModelInferencer::new(Arc::new(mem));
    let deltas = inf.infer(&traces);
    let kv: std::collections::HashMap<String, Value> = deltas.into_iter().collect();
    let order = kv.get("preferences.tool_order").expect("key present");
    let arr = order.as_array().unwrap();
    // Top by frequency: Bash(3), Read(3), Edit(2), Grep(1), Glob(1).
    // Ties broken by first-seen, so Read before Bash when counts tie.
    assert!(arr.len() <= 5);
    assert!(arr.contains(&json!("Read")));
    assert!(arr.contains(&json!("Bash")));
    assert!(arr.contains(&json!("Edit")));
}

#[tokio::test]
async fn infer_emits_tool_habits_recent_top5_weighted_by_recency() {
    // Last turn's tools weighted more heavily — even infrequent tools
    // there should appear in recent_top5.
    let traces = vec![
        turn_with_calls(0, vec!["Read", "Read", "Read"]),
        turn_with_calls(1, vec!["Read", "Read", "Read"]),
        turn_with_calls(2, vec!["Spawn", "Glob"]),
    ];
    let tmp = tempfile::tempdir().unwrap();
    let mem = wcore_memory::open_for_test(tmp.path()).await.unwrap();
    let inf = UserModelInferencer::new(Arc::new(mem));
    let deltas = inf.infer(&traces);
    let kv: std::collections::HashMap<String, Value> = deltas.into_iter().collect();
    let recent = kv.get("tool_habits.recent_top5").expect("key present");
    let arr = recent.as_array().unwrap();
    assert!(
        arr.contains(&json!("Spawn")),
        "Spawn must reach recent_top5 by recency weight"
    );
}

#[tokio::test]
async fn infer_emits_language_primary_default_en() {
    let traces = vec![turn_with_calls(0, vec!["Read"])];
    let tmp = tempfile::tempdir().unwrap();
    let mem = wcore_memory::open_for_test(tmp.path()).await.unwrap();
    let inf = UserModelInferencer::new(Arc::new(mem));
    let deltas = inf.infer(&traces);
    let kv: std::collections::HashMap<String, Value> = deltas.into_iter().collect();
    assert_eq!(kv.get("language.primary"), Some(&json!("en")));
}

#[tokio::test]
async fn infer_and_persist_writes_through_system_token() {
    let traces = vec![turn_with_calls(0, vec!["Read", "Edit", "Bash"])];
    let tmp = tempfile::tempdir().unwrap();
    let mem = wcore_memory::open_for_test(tmp.path()).await.unwrap();
    let mem_arc: Arc<dyn wcore_memory::api::MemoryApi> = Arc::new(mem);
    let inf = UserModelInferencer::new(mem_arc.clone());
    let n = inf.infer_and_persist(&traces).await.expect("must persist");
    assert!(n > 0);

    // Round-trip: user_model() returns the keys we wrote.
    let model = mem_arc.user_model(AccessToken::System).await.unwrap();
    let keys: Vec<&str> = model.entries.iter().map(|e| e.key.as_str()).collect();
    assert!(keys.contains(&"preferences.tool_order"));
    assert!(keys.contains(&"language.primary"));
}
