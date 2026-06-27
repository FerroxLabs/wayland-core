//! A3 spike (2026-05-30): proves the sequential `WorkflowRunner` kernel can
//! dispatch N pipeline stages through `AgentSpawner::spawn_one`, with each
//! stage's output threaded into the next stage's prompt.
//!
//! This is the load-bearing assumption of the Dynamic Workflows architecture
//! (Option C): the spawner path is structurally separate from the per-turn
//! `ExecutionGraph` walker, so it is NOT subject to the first-dispatch-wins
//! guard in `orchestration/node_executor.rs` (lines 202-207) that limits the
//! walker to one real `AgentCall` per turn. Each `spawn_one` builds a fresh
//! `AgentEngine` with its own turn loop.
//!
//! If first-dispatch-wins applied to this path, stages 2 and 3 would return
//! empty/inert results. This test proves all three run and carry distinct
//! outputs, and that stage N's prompt contains stage N-1's output.
//!
//! See .planning/plan/2026-05-30-dynamic-workflows-PLAN.md task A3.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::test_config;
use tokio::sync::mpsc;
use wcore_agent::spawner::{AgentSpawner, SubAgentConfig, SubAgentResult};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// Records every request it sees (Debug-formatted) and returns a distinct,
/// turn-indexed text response so each stage's execution is individually
/// observable.
struct CapturingProvider {
    seen: Arc<Mutex<Vec<String>>>,
    turn: Mutex<usize>,
}

#[async_trait]
impl LlmProvider for CapturingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let n = {
            let mut t = self.turn.lock().unwrap();
            let v = *t;
            *t += 1;
            v
        };
        self.seen.lock().unwrap().push(format!("{request:?}"));

        let events = vec![
            LlmEvent::TextDelta(format!("STAGE{n}-OUT")),
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
        ];

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in events {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// The `WorkflowRunner` kernel under test: run each stage via `spawn_one`,
/// substituting `{prev}` in the stage prompt with the prior stage's text.
/// This is the exact pattern PLAN task A3 promotes into a real module.
async fn run_sequential(spawner: &AgentSpawner, stages: &[(&str, &str)]) -> Vec<SubAgentResult> {
    let mut prev = String::new();
    let mut out = Vec::new();
    for (name, template) in stages {
        let prompt = template.replace("{prev}", &prev);
        let cfg = SubAgentConfig {
            name: (*name).to_string(),
            prompt,
            max_turns: 4,
            max_tokens: 256,
            system_prompt: None,
            provider: None,
            model: None,
            temperature: None,
        };
        let r = spawner.spawn_one(cfg).await;
        prev = r.text.clone();
        out.push(r);
    }
    out
}

#[tokio::test]
async fn workflow_runner_kernel_executes_all_stages_and_threads_data() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CapturingProvider {
        seen: Arc::clone(&seen),
        turn: Mutex::new(0),
    });
    let spawner = AgentSpawner::new(provider, test_config());

    let results = run_sequential(
        &spawner,
        &[
            ("ingest", "ingest the data"),
            ("review", "review using prior output: {prev}"),
            ("verify", "verify using prior output: {prev}"),
        ],
    )
    .await;

    // 1. All three stages executed — escaped first-dispatch-wins. Each ran a
    //    real engine loop and returned a distinct turn-indexed result.
    assert_eq!(results.len(), 3);
    for (i, r) in results.iter().enumerate() {
        assert!(!r.is_error, "stage {i} errored: {}", r.text);
        assert!(r.turns >= 1, "stage {i} did not run a real turn");
    }
    assert_eq!(results[0].text, "STAGE0-OUT");
    assert_eq!(results[1].text, "STAGE1-OUT");
    assert_eq!(results[2].text, "STAGE2-OUT");

    // 2. Data threaded between stages: stage 2's request must contain stage 1's
    //    output, and stage 3's must contain stage 2's.
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "expected exactly one LLM call per stage");
    assert!(
        seen[1].contains("STAGE0-OUT"),
        "stage 2 prompt missing stage 1 output; got: {}",
        seen[1]
    );
    assert!(
        seen[2].contains("STAGE1-OUT"),
        "stage 3 prompt missing stage 2 output; got: {}",
        seen[2]
    );
}
