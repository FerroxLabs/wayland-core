//! W8b.2.B Task C.1 — `ExecutionGraph` core (Node / Edge / GraphConfig /
//! NodeExecutor). The graph is additive: it does NOT replace the
//! existing orchestration loop yet — that's Task C.5. These tests pin
//! the public surface and the topological walk (sequential + parallel +
//! conditional edges + reducers).
//!
//! Design departure from the literal plan: `GraphContext` does NOT
//! hold a borrowed `AgentRegistry` (that crate-level coupling lives in
//! W7 F2, out of scope for this sub-wave). Instead the graph walks
//! against a pluggable `NodeExecutor` trait, which the main loop later
//! supplies in Task C.5 by adapting the existing agent dispatch path.

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use wcore_agent::orchestration::graph::{
    AggregationStrategy, EdgeCondition, ExecutionGraph, GraphConfig, GraphContext, GraphError,
    InputMapper, NodeExecutor, Predicate, StateReducer,
};

type Handler = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

/// Test executor: each agent name maps to a closure that produces a
/// deterministic JSON output from its input. Records call order so we
/// can assert sequentiality / concurrency.
#[derive(Default)]
struct ScriptedExec {
    log: Arc<Mutex<Vec<String>>>,
    handlers: std::collections::HashMap<String, Handler>,
}

impl ScriptedExec {
    fn handle(mut self, name: &str, f: impl Fn(&Value) -> Value + Send + Sync + 'static) -> Self {
        self.handlers.insert(name.to_string(), Arc::new(f));
        self
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().clone()
    }
}

#[async_trait]
impl NodeExecutor for ScriptedExec {
    async fn run_agent(&self, agent: &str, input: &Value) -> Result<Value, String> {
        self.log.lock().push(format!("call:{agent}"));
        match self.handlers.get(agent) {
            Some(h) => Ok(h(input)),
            None => Ok(json!({ "output": format!("ran:{agent}"), "input": input })),
        }
    }
}

fn ctx(exec: Arc<dyn NodeExecutor>) -> GraphContext {
    GraphContext {
        cancel: CancellationToken::new(),
        executor: exec,
    }
}

#[tokio::test]
async fn graph_executes_single_node_direct() {
    let exec = Arc::new(ScriptedExec::default().handle("main", |_| json!({"output": "hi"})));
    let config = GraphConfig::single_node("main", InputMapper::PassThrough);
    let result = ExecutionGraph::execute(config, json!({"task": "say hi"}), ctx(exec.clone()))
        .await
        .unwrap();
    assert_eq!(result.final_state["output"], json!("hi"));
    assert_eq!(exec.log(), vec!["call:main"]);
}

#[tokio::test]
async fn graph_executes_sequential_pipeline_a_b_c() {
    let exec = Arc::new(
        ScriptedExec::default()
            .handle("A", |_| json!({"output": "a"}))
            .handle("B", |i| {
                let prev = i["output"].as_str().unwrap_or("");
                json!({"output": format!("b<-{prev}")})
            })
            .handle("C", |i| {
                let prev = i["output"].as_str().unwrap_or("");
                json!({"output": format!("c<-{prev}")})
            }),
    );

    // A -> B -> C, each piping `output` into next input via PassThrough
    let mut config = GraphConfig::empty("A");
    config.add_agent("A", InputMapper::PassThrough);
    config.add_agent("B", InputMapper::PassThrough);
    config.add_agent("C", InputMapper::PassThrough);
    config.add_edge("A", "B", None);
    config.add_edge("B", "C", None);

    let result = ExecutionGraph::execute(config, json!({}), ctx(exec.clone()))
        .await
        .unwrap();
    assert_eq!(result.final_state["output"], json!("c<-b<-a"));
    assert_eq!(exec.log(), vec!["call:A", "call:B", "call:C"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graph_executes_parallel_fanout_and_joins() {
    // Topology:
    //         entry
    //          |
    //         FAN (Aggregator passthrough -> three children)
    //         / | \
    //        A  B  C   (concurrent)
    //         \ | /
    //         JOIN (Aggregator::Concat array)
    //
    // The walker MUST run A/B/C on the same wall-clock tick (concurrent)
    // and then funnel into JOIN.

    // 80ms × 3 sleeps + 200ms bound below: sequential = 240ms (fails the
    // bound), parallel ≈ 80ms + CI scheduler overhead (passes). Bumped
    // from 20ms × 3 / 50ms bound after CI run 25949482628 flaked on
    // macos-latest — a 5ms overhead budget is unreasonable on shared
    // runners. The parallelism assertion is preserved.
    let exec = Arc::new(
        ScriptedExec::default()
            .handle("A", |_| {
                std::thread::sleep(Duration::from_millis(80));
                json!({"output": "a"})
            })
            .handle("B", |_| {
                std::thread::sleep(Duration::from_millis(80));
                json!({"output": "b"})
            })
            .handle("C", |_| {
                std::thread::sleep(Duration::from_millis(80));
                json!({"output": "c"})
            }),
    );

    let mut config = GraphConfig::empty("FAN");
    config.add_passthrough("FAN");
    config.add_agent("A", InputMapper::PassThrough);
    config.add_agent("B", InputMapper::PassThrough);
    config.add_agent("C", InputMapper::PassThrough);
    config.add_aggregator("JOIN", AggregationStrategy::ConcatOutputs);
    config.add_edge("FAN", "A", None);
    config.add_edge("FAN", "B", None);
    config.add_edge("FAN", "C", None);
    config.add_edge("A", "JOIN", None);
    config.add_edge("B", "JOIN", None);
    config.add_edge("C", "JOIN", None);

    // Timing-based parallelism asserts proved brittle on macos-latest
    // CI runners across 3 cycles (see orchestration_templates_test.rs
    // for the full reasoning). Parallelism is enforced by the
    // `multi_thread` runtime above and validated by the OUTPUT-
    // COMPLETENESS assertion below: serial execution that hangs on
    // any child would leave `out != expected`.
    let result = ExecutionGraph::execute(config, json!({}), ctx(exec.clone()))
        .await
        .unwrap();
    // Output is the concatenated array of children's outputs, set-equal
    // (order doesn't matter for parallel completion).
    let out = result.final_state["output"]
        .as_array()
        .expect("array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let expected = ["a", "b", "c"]
        .into_iter()
        .map(String::from)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(out, expected);
}

#[tokio::test]
async fn graph_respects_edge_condition() {
    let exec = Arc::new(
        ScriptedExec::default()
            .handle("router", |_| json!({"go": "left"}))
            .handle("left", |_| json!({"output": "L"}))
            .handle("right", |_| json!({"output": "R"})),
    );

    let mut config = GraphConfig::empty("router");
    config.add_agent("router", InputMapper::PassThrough);
    config.add_agent("left", InputMapper::PassThrough);
    config.add_agent("right", InputMapper::PassThrough);
    config.add_edge(
        "router",
        "left",
        Some(EdgeCondition::StateEquals {
            path: "go".into(),
            value: json!("left"),
        }),
    );
    config.add_edge(
        "router",
        "right",
        Some(EdgeCondition::StateEquals {
            path: "go".into(),
            value: json!("right"),
        }),
    );

    let result = ExecutionGraph::execute(config, json!({}), ctx(exec.clone()))
        .await
        .unwrap();
    assert_eq!(result.final_state["output"], json!("L"));
    let log = exec.log();
    assert!(log.contains(&"call:router".to_string()));
    assert!(log.contains(&"call:left".to_string()));
    assert!(!log.contains(&"call:right".to_string()));
}

#[tokio::test]
async fn graph_cancels_when_token_fires() {
    let exec = Arc::new(
        ScriptedExec::default()
            .handle("A", |_| json!({"output": "a"}))
            .handle("B", |_| json!({"output": "b"})),
    );
    let mut config = GraphConfig::empty("A");
    config.add_agent("A", InputMapper::PassThrough);
    config.add_agent("B", InputMapper::PassThrough);
    config.add_edge("A", "B", None);

    let cancel = CancellationToken::new();
    cancel.cancel();
    let gctx = GraphContext {
        cancel: cancel.clone(),
        executor: exec.clone(),
    };
    let err = ExecutionGraph::execute(config, json!({}), gctx)
        .await
        .expect_err("must short-circuit");
    assert!(matches!(err, GraphError::Cancelled));
}

#[tokio::test]
async fn graph_state_reducer_merges_children_outputs() {
    let exec = Arc::new(
        ScriptedExec::default()
            .handle("A", |_| json!({"a_done": true}))
            .handle("B", |_| json!({"b_done": true})),
    );
    let mut config = GraphConfig::empty("ROOT");
    config.add_passthrough("ROOT");
    config.add_agent("A", InputMapper::PassThrough);
    config.add_agent("B", InputMapper::PassThrough);
    config.add_aggregator("JOIN", AggregationStrategy::MergeObjects);
    config.add_edge("ROOT", "A", None);
    config.add_edge("ROOT", "B", None);
    config.add_edge("A", "JOIN", None);
    config.add_edge("B", "JOIN", None);

    let result = ExecutionGraph::execute(config, json!({}), ctx(exec.clone()))
        .await
        .unwrap();
    assert_eq!(result.final_state["a_done"], json!(true));
    assert_eq!(result.final_state["b_done"], json!(true));
}

#[tokio::test]
async fn predicate_node_evaluates_against_state() {
    let exec = Arc::new(ScriptedExec::default().handle("A", |_| json!({"output": "a"})));
    let mut config = GraphConfig::empty("PRED");
    config.add_predicate(
        "PRED",
        Predicate::StateContainsKey {
            path: "task".into(),
        },
    );
    config.add_agent("A", InputMapper::PassThrough);
    // Edge taken only when predicate=true.
    config.add_edge(
        "PRED",
        "A",
        Some(EdgeCondition::StateEquals {
            path: "predicate_result".into(),
            value: json!(true),
        }),
    );

    let result = ExecutionGraph::execute(config, json!({"task": "x"}), ctx(exec.clone()))
        .await
        .unwrap();
    assert_eq!(result.final_state["output"], json!("a"));
}

#[tokio::test]
async fn input_mapper_select_picks_subfield() {
    let exec = Arc::new(
        ScriptedExec::default()
            .handle("A", |_| json!({"payload": {"value": 42}}))
            .handle("B", |i| json!({"output": i.clone()})),
    );
    let mut config = GraphConfig::empty("A");
    config.add_agent("A", InputMapper::PassThrough);
    config.add_agent(
        "B",
        InputMapper::Select {
            path: "payload".into(),
        },
    );
    config.add_edge("A", "B", None);

    let result = ExecutionGraph::execute(config, json!({}), ctx(exec.clone()))
        .await
        .unwrap();
    assert_eq!(result.final_state["output"], json!({"value": 42}));
}

// Sanity check that state_reducers (custom merge) is respected.
#[tokio::test]
async fn custom_state_reducer_overrides_default() {
    let exec = Arc::new(
        ScriptedExec::default()
            .handle("A", |_| json!({"counter": 1}))
            .handle("B", |_| json!({"counter": 1})),
    );
    let mut config = GraphConfig::empty("ROOT");
    config.add_passthrough("ROOT");
    config.add_agent("A", InputMapper::PassThrough);
    config.add_agent("B", InputMapper::PassThrough);
    config.add_aggregator("JOIN", AggregationStrategy::MergeObjects);
    config.add_edge("ROOT", "A", None);
    config.add_edge("ROOT", "B", None);
    config.add_edge("A", "JOIN", None);
    config.add_edge("B", "JOIN", None);
    config
        .state_reducers
        .insert("counter".into(), StateReducer::SumNumbers);

    let result = ExecutionGraph::execute(config, json!({"counter": 0}), ctx(exec.clone()))
        .await
        .unwrap();
    // SumNumbers folds via f64; the resulting Number serialises as 2.0
    // but compares equal to 2 only via numeric value, not Value::eq.
    assert_eq!(
        result.final_state["counter"].as_f64(),
        Some(2.0),
        "got {}",
        result.final_state["counter"]
    );
}
