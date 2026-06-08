//! Resource-bound / DoS-hardening tests for the Dynamic Workflows engine.
//!
//! Covers the four bounds added against attacker-controlled RON:
//!   FIX 1 — global dispatch budget (`DispatchBudgetExceeded`).
//!   FIX 2 — parse-time RON size + nesting guard, and schema-body depth guard.
//!   FIX 3 — pipeline `over:` cardinality cap + bounded polling (order-preserving).
//!   FIX 4 — lowered graph node-count cap.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::test_config;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wcore_agent::orchestration::workflow::error::WorkflowParseError;
use wcore_agent::orchestration::workflow::limits::{
    MAX_NESTING_DEPTH, MAX_OVER_CARDINALITY, MAX_RON_BYTES, MAX_TOTAL_DISPATCHES,
    MAX_WORKFLOW_NODES,
};
use wcore_agent::orchestration::workflow::runner::{
    WorkflowPlan, WorkflowRunError, WorkflowRunner,
};
use wcore_agent::spawner::AgentSpawner;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

fn ok_events(text: String) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage {
                input_tokens: 1,
                output_tokens: 1,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

/// Always-succeeds provider that echoes a fixed body and counts calls, so a
/// test can assert how many dispatches actually reached the LLM layer.
struct CountingProvider {
    calls: Arc<Mutex<usize>>,
    body: String,
}

impl CountingProvider {
    fn new(calls: Arc<Mutex<usize>>, body: &str) -> Self {
        Self {
            calls,
            body: body.to_string(),
        }
    }
}

#[async_trait]
impl LlmProvider for CountingProvider {
    async fn stream(&self, _req: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        *self.calls.lock().unwrap() += 1;
        let body = self.body.clone();
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in ok_events(body) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// Extract the parse error from a `WorkflowPlan::parse` result. `WorkflowPlan`
/// does not derive `Debug`, so `unwrap_err` (which needs `Debug` on the Ok
/// side) cannot be used directly — mirror dsl.rs's `parse_err` helper.
fn parse_err(src: &str) -> WorkflowParseError {
    match WorkflowPlan::parse(src) {
        Ok(_) => panic!("expected a parse error, got Ok"),
        Err(e) => e,
    }
}

// ---------------------------------------------------------------------------
// FIX 2 — parse-time guards
// ---------------------------------------------------------------------------

#[test]
fn fix2_oversized_ron_is_rejected_before_parse() {
    // A document longer than MAX_RON_BYTES is rejected without parsing. We pad
    // with a comment so the bytes are valid-but-huge; the size check fires first.
    let pad = "/* ".to_string() + &"x".repeat(MAX_RON_BYTES) + " */";
    let src = format!("{pad}\nWorkflow(meta: (name: \"x\"), phases: [])");
    match parse_err(&src) {
        WorkflowParseError::TooLarge { size, limit } => {
            assert!(size > limit);
            assert_eq!(limit, MAX_RON_BYTES);
        }
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn fix2_deeply_nested_ron_is_rejected_without_panicking() {
    // Build a document whose paren nesting exceeds the depth cap. Without the
    // pre-parse byte-scan this would recurse RON 0.8 into a stack overflow (an
    // uncatchable abort); with it we get a typed error.
    let deep = format!(
        "Workflow(meta: (name: \"x\"), phases: {})",
        "[".repeat(MAX_NESTING_DEPTH + 10)
    );
    match parse_err(&deep) {
        WorkflowParseError::TooDeep { depth, limit } => {
            assert!(depth > limit);
            assert_eq!(limit, MAX_NESTING_DEPTH);
        }
        other => panic!("expected TooDeep, got {other:?}"),
    }
}

#[test]
fn fix2_deeply_nested_schema_body_errors_typed_no_overflow() {
    // A schema body nested past the depth cap must be rejected as an invalid
    // schema definition rather than overflowing the recursive `from_value`.
    let mut body = String::new();
    for _ in 0..(MAX_NESTING_DEPTH + 5) {
        body.push_str(r#"{ "type": "object", "properties": { "a": "#);
    }
    body.push_str(r#"{ "type": "string" }"#);
    for _ in 0..(MAX_NESTING_DEPTH + 5) {
        body.push_str(" } }");
    }
    // Embed as a schema string inside a workflow. RON-escape the quotes.
    let escaped = body.replace('\\', "\\\\").replace('"', "\\\"");
    let src = format!(
        r#"Workflow(meta: (name: "x"), schemas: {{ "deep": "{escaped}" }}, phases: [Phase(title: "p", steps: [Agent((id: "a", prompt: "p", schema: Some("deep")))])])"#
    );
    match parse_err(&src) {
        // Either the schema compiler's own depth guard or the RON depth guard
        // catches it — both are typed, neither overflows.
        WorkflowParseError::InvalidSchema { name, .. } => assert_eq!(name, "deep"),
        WorkflowParseError::TooDeep { .. } => {}
        other => panic!("expected a typed depth/schema error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// FIX 4 — node-count cap
// ---------------------------------------------------------------------------

#[test]
fn fix4_workflow_lowering_to_too_many_nodes_is_rejected() {
    // Build a single phase with MAX_WORKFLOW_NODES + 1 distinct agent steps.
    let mut steps = String::new();
    for i in 0..(MAX_WORKFLOW_NODES + 1) {
        steps.push_str(&format!("Agent((id: \"a{i}\", prompt: \"p\")),"));
    }
    let src =
        format!(r#"Workflow(meta: (name: "x"), phases: [Phase(title: "p", steps: [{steps}])])"#);
    match parse_err(&src) {
        WorkflowParseError::TooManyNodes { count, limit } => {
            assert!(count > limit);
            assert_eq!(limit, MAX_WORKFLOW_NODES);
        }
        other => panic!("expected TooManyNodes, got {other:?}"),
    }
}

#[test]
fn fix4_small_workflow_under_node_cap_parses() {
    let src = r#"Workflow(meta: (name: "x"), phases: [Phase(title: "p", steps: [Agent((id: "a", prompt: "p"))])])"#;
    assert!(WorkflowPlan::parse(src).is_ok());
}

// ---------------------------------------------------------------------------
// FIX 1 — global dispatch budget
// ---------------------------------------------------------------------------

/// A workflow whose runtime-injected `over:` array, streamed through a pipeline,
/// drives total dispatches past MAX_TOTAL_DISPATCHES → the run aborts with
/// `DispatchBudgetExceeded` and a partial result. The `over:` array stays under
/// the cardinality cap (so FIX 3 does not fire first) but its item count times
/// the stage count exceeds the dispatch budget.
#[tokio::test]
async fn fix1_dispatch_budget_aborts_with_partial_result() {
    let calls = Arc::new(Mutex::new(0usize));
    let provider = Arc::new(CountingProvider::new(Arc::clone(&calls), "ok"));
    let spawner = AgentSpawner::new(provider, test_config());

    // 400 items (< MAX_OVER_CARDINALITY = 500) × 3 stages = 1200 dispatches,
    // which exceeds MAX_TOTAL_DISPATCHES = 1000.
    const _: () = assert!(400 <= MAX_OVER_CARDINALITY);
    const _: () = assert!(400 * 3 > MAX_TOTAL_DISPATCHES);
    let src = r#"
Workflow(
    meta: (name: "budget"),
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", over: Some("files"), stages: [
            (id: "s1", prompt: "one"),
            (id: "s2", prompt: "two"),
            (id: "s3", prompt: "three"),
        ]),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    let files: Vec<Value> = (0..400).map(|i| json!(format!("f{i}"))).collect();
    let initial = json!({ "files": files });

    let runner = WorkflowRunner::new(&spawner);
    let err = runner
        .run(&plan, initial)
        .await
        .expect_err("budget overflow must abort the run");
    match err {
        WorkflowRunError::DispatchBudgetExceeded {
            limit,
            attempted,
            partial,
        } => {
            assert_eq!(limit, MAX_TOTAL_DISPATCHES);
            assert!(
                attempted > limit,
                "attempted {attempted} should exceed {limit}"
            );
            // The partial result is preserved (some stages completed first).
            assert!(!partial.stage_results.is_empty());
        }
        other => panic!("expected DispatchBudgetExceeded, got {other:?}"),
    }
    // The budget caps actual LLM calls at roughly the limit, not the full 1200.
    assert!(
        *calls.lock().unwrap() <= MAX_TOTAL_DISPATCHES + DEFAULT_PIPELINE_HEADROOM,
        "dispatches should be bounded near the budget, got {}",
        *calls.lock().unwrap()
    );
}

/// In-flight pipeline futures are bounded, so even at the budget edge the actual
/// dispatch count cannot run far past the limit. A small slack accounts for
/// futures already in flight when the breach is first observed.
const DEFAULT_PIPELINE_HEADROOM: usize = 64;

#[tokio::test]
async fn fix1_normal_small_workflow_unaffected() {
    let calls = Arc::new(Mutex::new(0usize));
    let provider = Arc::new(CountingProvider::new(Arc::clone(&calls), "ok"));
    let spawner = AgentSpawner::new(provider, test_config());

    let src = r#"
Workflow(
    meta: (name: "small"),
    phases: [Phase(title: "p", steps: [
        Agent((id: "a", prompt: "one")),
        Agent((id: "b", prompt: "two")),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("a tiny workflow must run within budget");
    // Two agent stages dispatched, both succeeded.
    assert_eq!(*calls.lock().unwrap(), 2);
    assert!(result.final_state.get("a").is_some());
    assert!(result.final_state.get("b").is_some());
}

// ---------------------------------------------------------------------------
// FIX 3 — over-cardinality cap + order-preserving bounded polling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fix3_over_cardinality_above_cap_is_rejected() {
    let calls = Arc::new(Mutex::new(0usize));
    let provider = Arc::new(CountingProvider::new(Arc::clone(&calls), "ok"));
    let spawner = AgentSpawner::new(provider, test_config());

    let src = r#"
Workflow(
    meta: (name: "card"),
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", over: Some("files"), stages: [(id: "s1", prompt: "one")]),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    // One more item than the cardinality cap.
    let files: Vec<Value> = (0..(MAX_OVER_CARDINALITY + 1))
        .map(|i| json!(format!("f{i}")))
        .collect();
    let initial = json!({ "files": files });

    let runner = WorkflowRunner::new(&spawner);
    let err = runner
        .run(&plan, initial)
        .await
        .expect_err("over-cap collection must be rejected");
    match err {
        WorkflowRunError::DispatchBudgetExceeded {
            limit, attempted, ..
        } => {
            assert_eq!(limit, MAX_OVER_CARDINALITY);
            assert_eq!(attempted, MAX_OVER_CARDINALITY + 1);
        }
        other => panic!("expected a cardinality rejection, got {other:?}"),
    }
    // Rejected BEFORE building any item future: zero LLM calls.
    assert_eq!(*calls.lock().unwrap(), 0);
}

#[tokio::test]
async fn fix3_moderate_pipeline_runs_and_preserves_order_with_null_holes() {
    // A provider that echoes the item tag for live items, but errors for the
    // item carrying "drop" so that exactly one item becomes a `null` hole.
    struct OrderProvider;
    #[async_trait]
    impl LlmProvider for OrderProvider {
        async fn stream(
            &self,
            req: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let dump = format!("{req:?}");
            if dump.contains("DROPME") {
                return Err(ProviderError::Connection("boom".into()));
            }
            // Echo a stable marker so we can assert order. Find the item tag.
            let tag = ["A0", "A1", "A2", "A3", "A4"]
                .iter()
                .find(|t| dump.contains(*t))
                .copied()
                .unwrap_or("?");
            let body = format!("OUT-{tag}");
            let (tx, rx) = mpsc::channel(64);
            tokio::spawn(async move {
                for ev in ok_events(body) {
                    let _ = tx.send(ev).await;
                }
            });
            Ok(rx)
        }
    }

    let spawner = AgentSpawner::new(Arc::new(OrderProvider), test_config());
    let src = r#"
Workflow(
    meta: (name: "order"),
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", over: Some("files"), stages: [(id: "s1", prompt: "process")]),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    // 5 items; item index 2 carries the DROPME marker so it becomes null.
    let files = json!(["A0", "A1 DROPME", "A2", "A3", "A4"]);
    let initial = json!({ "files": files });

    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, initial)
        .await
        .expect("a moderate pipeline runs to completion");
    let out = result
        .final_state
        .get("pl")
        .and_then(Value::as_array)
        .expect("pipeline writes an array");
    assert_eq!(out.len(), 5, "one entry per input item, in input order");
    // index 1 dropped (it carried DROPME) → null hole; the rest carry output.
    assert!(out[0].is_string());
    assert!(out[1].is_null(), "dropped item must be a null hole");
    assert!(out[2].is_string());
    assert!(out[3].is_string());
    assert!(out[4].is_string());
}
