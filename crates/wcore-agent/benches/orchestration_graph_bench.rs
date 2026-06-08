//! Benchmark: dispatch a 5-node mock DAG through `ExecutionGraph`.
//!
//! Topology (linear chain, no branching):
//!   a -> b -> c -> d -> end
//! The executor is a no-op that returns the input value unchanged.

use std::sync::Arc;

use async_trait::async_trait;
use criterion::{Criterion, criterion_group, criterion_main};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wcore_agent::orchestration::graph::{
    ExecutionGraph, GraphConfig, GraphContext, InputMapper, NodeExecutor,
};

struct NoopExec;

#[async_trait]
impl NodeExecutor for NoopExec {
    async fn run_agent(&self, _agent: &str, input: &Value) -> Result<Value, String> {
        Ok(input.clone())
    }
}

fn build_linear_dag() -> GraphConfig {
    let mut cfg = GraphConfig::empty("a");
    cfg.add_agent("a", InputMapper::PassThrough);
    cfg.add_agent("b", InputMapper::PassThrough);
    cfg.add_agent("c", InputMapper::PassThrough);
    cfg.add_agent("d", InputMapper::PassThrough);
    cfg.add_end("end");
    cfg.add_edge("a", "b", None);
    cfg.add_edge("b", "c", None);
    cfg.add_edge("c", "d", None);
    cfg.add_edge("d", "end", None);
    cfg
}

fn bench_graph_execute(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    c.bench_function("orchestration_graph_5node_linear", |b| {
        b.iter(|| {
            let cfg = build_linear_dag();
            let ctx = GraphContext {
                cancel: CancellationToken::new(),
                executor: Arc::new(NoopExec),
            };
            rt.block_on(ExecutionGraph::execute(cfg, json!({"x": 1}), ctx))
                .unwrap();
        });
    });
}

criterion_group!(benches, bench_graph_execute);
criterion_main!(benches);
