//! Benchmark: dispatch a no-op tool through `ToolRegistry`.
//!
//! Measures the overhead of registry lookup + `Tool::execute` dispatch
//! independent of any real I/O.

use async_trait::async_trait;
use criterion::{Criterion, criterion_group, criterion_main};
use serde_json::{Value, json};
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_tools::dispatcher::ToolDispatcher;
use wcore_tools::registry::ToolRegistry;
use wcore_types::tool::{JsonSchema, ToolResult};

struct NoopTool;

#[async_trait]
impl Tool for NoopTool {
    fn name(&self) -> &str {
        "noop"
    }

    fn description(&self) -> &str {
        "no-op tool for benchmarking"
    }

    fn input_schema(&self) -> JsonSchema {
        json!({"type": "object", "properties": {}})
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, _input: Value) -> ToolResult {
        ToolResult {
            content: String::new(),
            is_error: false,
        }
    }
}

fn bench_registry_dispatch(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(NoopTool));

    c.bench_function("registry_dispatch_noop", |b| {
        b.iter(|| {
            rt.block_on(registry.dispatch("noop", json!({})));
        });
    });
}

criterion_group!(benches, bench_registry_dispatch);
criterion_main!(benches);
