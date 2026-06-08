//! B1 — integration tests for `WorkflowTool`.
//!
//! Models the mock provider on `workflow_runner_test.rs` (a turn-indexed
//! capturing provider) and the registration assertion on the bootstrap
//! tool-list snapshot (`bootstrap_registers_all_expected_tools`).
//!
//! Coverage:
//! 1. The tool registers under the name `Workflow` (bootstrap snapshot).
//! 2. An inline-RON 2-stage workflow executes end-to-end through the tool and
//!    returns a non-error result whose content reflects both stages.
//! 3. Malformed RON returns an error result carrying the parse error
//!    (`is_error == true`), not a panic.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::test_config;
use serde_json::json;
use tokio::sync::mpsc;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::null_sink::NullSink;
use wcore_agent::spawner::AgentSpawner;
use wcore_agent::workflow_tool::WorkflowTool;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::Tool;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// Returns a distinct, turn-indexed text per call so each stage is
/// individually observable — the same instrument the runner tests use.
struct CapturingProvider {
    turn: Mutex<usize>,
}

impl CapturingProvider {
    fn new() -> Self {
        Self {
            turn: Mutex::new(0),
        }
    }
}

fn ok_events(text: String) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

#[async_trait]
impl LlmProvider for CapturingProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let n = {
            let mut t = self.turn.lock().unwrap();
            let v = *t;
            *t += 1;
            v
        };
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in ok_events(format!("STAGE{n}-OUT")) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

fn minimal_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(5),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

/// 1. The tool registers under `Workflow` in the live bootstrap registry —
///    asserted via the same `tool_names()` snapshot the other built-ins use.
#[tokio::test]
async fn workflow_tool_is_registered_in_bootstrap() {
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(
        minimal_config(),
        workdir.path().to_str().unwrap(),
        Arc::new(NullSink),
    )
    .build()
    .await
    .expect("bootstrap should build");

    let names = result.engine.tool_names();
    assert!(
        names.iter().any(|n| n == "Workflow"),
        "WorkflowTool should be registered; got: {names:?}"
    );
}

/// 2. An inline-RON 2-stage workflow executes end-to-end through the tool and
///    returns a non-error result. Both stages appear in the rendered output.
#[tokio::test]
async fn inline_two_stage_workflow_runs_end_to_end() {
    let provider = Arc::new(CapturingProvider::new());
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()));
    let tool = WorkflowTool::new(spawner);

    let src = r#"
Workflow(
    meta: (name: "two-stage", description: "ingest then review", est_agents: 2),
    phases: [Phase(title: "run", steps: [
        Pipeline(id: "pipe", stages: [
            (id: "ingest", prompt: "ingest the data"),
            (id: "review", prompt: "review prior", input: Some("ingest")),
        ]),
    ])],
)
"#;

    let out = tool.execute(json!({ "workflow": src })).await;

    assert!(
        !out.is_error,
        "two-stage workflow should succeed; got: {}",
        out.content
    );
    // Both stages produced output threaded into the final state / summary.
    assert!(
        out.content.contains("ingest"),
        "result should mention the ingest stage; got: {}",
        out.content
    );
    assert!(
        out.content.contains("review"),
        "result should mention the review stage; got: {}",
        out.content
    );
    assert!(
        out.content.contains("STAGE0-OUT") && out.content.contains("STAGE1-OUT"),
        "final state should carry both stage outputs; got: {}",
        out.content
    );
}

/// 3. Malformed RON returns an error result carrying the parse error — never a
///    panic. `is_error` must be true.
#[tokio::test]
async fn malformed_ron_returns_parse_error_not_panic() {
    let provider = Arc::new(CapturingProvider::new());
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()));
    let tool = WorkflowTool::new(spawner);

    // Not valid RON for a `Workflow` (truncated / garbage).
    let out = tool
        .execute(json!({ "workflow": "Workflow(this is not valid ron" }))
        .await;

    assert!(
        out.is_error,
        "malformed RON must return an error result; got ok: {}",
        out.content
    );
    assert!(
        out.content.contains("parse error"),
        "error result should carry the parse error; got: {}",
        out.content
    );
}

/// A missing `workflow` parameter is a typed error, not a panic.
#[tokio::test]
async fn missing_workflow_param_returns_error() {
    let provider = Arc::new(CapturingProvider::new());
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()));
    let tool = WorkflowTool::new(spawner);

    let out = tool.execute(json!({})).await;
    assert!(out.is_error, "missing 'workflow' must be an error");
}

/// A provider that counts every `stream` call, so a test can assert exactly how
/// many sub-agents (pipeline items) were dispatched.
struct CountingProvider {
    calls: Arc<Mutex<usize>>,
}

#[async_trait]
impl LlmProvider for CountingProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        {
            let mut c = self.calls.lock().unwrap();
            *c += 1;
        }
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in ok_events("ITEM-OUT".to_string()) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// A single-stage `over:` pipeline whose collection comes from the caller's
/// initial state key `changed_files`.
const OVER_PIPELINE: &str = r#"
Workflow(
    meta: (name: "over-from-inputs", est_agents: 1),
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", over: Some("changed_files"), stages: [
            (id: "s1", prompt: "process the file"),
        ]),
    ])],
)
"#;

/// FIX C: an `inputs` object supplied to `WorkflowTool` becomes the runner's
/// initial state, so an `over:`-pipeline streams over the caller-provided array.
/// Here 3 items → 3 sub-agent dispatches and a 3-element result array.
#[tokio::test]
async fn inputs_object_feeds_over_pipeline() {
    let calls = Arc::new(Mutex::new(0usize));
    let provider = Arc::new(CountingProvider {
        calls: Arc::clone(&calls),
    });
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()));
    let tool = WorkflowTool::new(spawner);

    let out = tool
        .execute(json!({
            "workflow": OVER_PIPELINE,
            "inputs": { "changed_files": ["a.rs", "b.rs", "c.rs"] },
        }))
        .await;

    assert!(
        !out.is_error,
        "over-pipeline with inputs should succeed; got: {}",
        out.content
    );
    // One sub-agent per item — the pipeline streamed over all 3 caller inputs.
    assert_eq!(
        *calls.lock().unwrap(),
        3,
        "pipeline should dispatch one sub-agent per input item; got content: {}",
        out.content
    );
}

/// FIX C control: WITHOUT `inputs`, the `over:` collection resolves to empty, so
/// the pipeline dispatches zero sub-agents — the pre-fix behaviour is preserved.
#[tokio::test]
async fn over_pipeline_without_inputs_dispatches_nothing() {
    let calls = Arc::new(Mutex::new(0usize));
    let provider = Arc::new(CountingProvider {
        calls: Arc::clone(&calls),
    });
    let spawner = Arc::new(AgentSpawner::new(provider, test_config()));
    let tool = WorkflowTool::new(spawner);

    let out = tool.execute(json!({ "workflow": OVER_PIPELINE })).await;

    assert!(
        !out.is_error,
        "empty over-pipeline should still succeed; got: {}",
        out.content
    );
    assert_eq!(
        *calls.lock().unwrap(),
        0,
        "no inputs → the over-collection is empty → zero sub-agents dispatched"
    );
}
