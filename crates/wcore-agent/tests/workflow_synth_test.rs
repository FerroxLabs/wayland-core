//! B7 — integration tests for `synthesize_workflow`.
//!
//! Each test drives `synthesize_workflow` with a `SequencedProvider`-style mock
//! whose Nth `stream` call returns a configured response. Because every workflow
//! `spawn_one` is exactly one LLM call, the call index equals the synthesis
//! attempt index (attempt 1 = call 0, the first re-prompt = call 1, …), so the
//! mock's response sequence maps directly onto the synthesis attempts.
//!
//! The synthesis loop runs up to `MAX_SYNTH_ATTEMPTS` (3) attempts and retries
//! on BOTH failure modes: a missing/unparseable block (prose / tool call) AND
//! an extracted-but-invalid block.
//!
//! Coverage:
//! 1. Valid RON on attempt 1 → a `WorkflowPlan` that parses and is runnable.
//! 2. PROSE (no block) on attempt 1, valid on attempt 2 → succeeds, and the
//!    re-prompt carried the "did not output a RON block" correction. This is the
//!    bug fix: the old code aborted on attempt 1 with `NoRonBlock`.
//! 3. Prose on attempts 1 AND 2, valid on 3 → succeeds (full 3-attempt budget).
//! 4. Prose on all 3 attempts → `SynthError::NoRonBlock` (no panic).
//! 5. Invalid RON on attempt 1, valid on attempt 2 → succeeds via the parse-
//!    retry path, and the re-prompt carried the parse error.
//! 6. Invalid RON on all 3 attempts → `SynthError::InvalidAfterReprompt`.
//! 7. RON wrapped in prose + markdown fences → extraction still finds the block.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::test_config;
use serde_json::Value;
use tokio::sync::mpsc;
use wcore_agent::orchestration::workflow::runner::WorkflowRunner;
use wcore_agent::spawner::AgentSpawner;
use wcore_agent::workflow_synth::{SynthError, synthesize_workflow};
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
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

/// Returns a configured text per call, in order, recording each request's
/// Debug-formatted form so the re-prompt's content can be inspected.
struct SequencedProvider {
    texts: Vec<String>,
    seen: Arc<Mutex<Vec<String>>>,
    turn: Mutex<usize>,
}

impl SequencedProvider {
    fn new(texts: Vec<&str>, seen: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            texts: texts.into_iter().map(String::from).collect(),
            seen,
            turn: Mutex::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for SequencedProvider {
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
        // Past the configured list, keep emitting the last text (stable tail).
        let text = self
            .texts
            .get(n)
            .or_else(|| self.texts.last())
            .cloned()
            .unwrap_or_default();
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in ok_events(text) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// A minimal valid 2-stage workflow. `review every file` triggers the B3
/// detector so the advisory-context path is exercised too.
const VALID_RON: &str = r#"Workflow(
    meta: (name: "review-changes", description: "review a diff", est_agents: 2),
    phases: [Phase(title: "analyze", steps: [
        Agent((id: "scan", prompt: "scan the diff")),
        Agent((id: "summarize", prompt: "summarize", input: Some("scan"))),
    ])],
)"#;

/// 1. Valid RON on the first turn → a `WorkflowPlan` that parses AND is runnable
///    end-to-end through `WorkflowRunner` (so synthesis output is real, not a
///    plan that merely parses).
#[tokio::test]
async fn valid_ron_first_turn_returns_runnable_plan() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(SequencedProvider::new(vec![VALID_RON], Arc::clone(&seen)));
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = synthesize_workflow("review every file in the repo", &spawner)
        .await
        .expect("valid RON should synthesise a plan");

    assert_eq!(plan.meta.name, "review-changes");
    // Exactly one LLM call: no re-prompt happened.
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "synthesis should not retry on valid RON"
    );

    // The synthesised plan is RUNNABLE: drive it through the runner. The same
    // sequenced provider keeps emitting valid RON as the per-stage output, which
    // is fine — we only assert the workflow executes both stages without error.
    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("synthesised plan should run to completion");
    assert!(result.final_state.get("scan").is_some());
    assert!(result.final_state.get("summarize").is_some());
}

/// 2. THE BUG FIX. The model answers attempt 1 with PROSE (no `Workflow(...)`
///    block at all) — the exact live-run failure that returned `NoRonBlock` and
///    silently fell through. It must now re-prompt and succeed on attempt 2,
///    and the 2nd prompt must carry the "did not output a RON block" correction.
#[tokio::test]
async fn prose_then_valid_recovers_with_missing_block_correction() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    // Attempt 0: pure prose, no RON block (extract_ron → None). Attempt 1: valid.
    let provider = Arc::new(SequencedProvider::new(
        vec![
            "Sure, let me look at the files first before I design the workflow.",
            VALID_RON,
        ],
        Arc::clone(&seen),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = synthesize_workflow("review every file", &spawner)
        .await
        .expect("a prose-only first answer must be re-prompted, not aborted");
    assert_eq!(plan.meta.name, "review-changes");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "expected one re-prompt (2 total calls)");
    // The 2nd prompt carried the missing-block correction (NOT the parse-error one).
    assert!(
        seen[1].contains("You did not output a RON block"),
        "the re-prompt after prose must carry the missing-block correction; got: {}",
        seen[1]
    );
}

/// 3. Prose on attempts 1 AND 2, valid on 3 → succeeds. Proves the budget is a
///    full 3 attempts, not 2.
#[tokio::test]
async fn prose_twice_then_valid_uses_full_budget() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(SequencedProvider::new(
        vec![
            "Let me explore the repo.",
            "I will read a few files and report back.",
            VALID_RON,
        ],
        Arc::clone(&seen),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = synthesize_workflow("review every file", &spawner)
        .await
        .expect("valid RON on the third attempt must succeed");
    assert_eq!(plan.meta.name, "review-changes");
    assert_eq!(
        seen.lock().unwrap().len(),
        3,
        "expected exactly three calls (two re-prompts)"
    );
}

/// 4. Prose on ALL three attempts → a typed `NoRonBlock`, no panic, no plan.
#[tokio::test]
async fn prose_on_all_attempts_returns_no_ron_block() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(SequencedProvider::new(
        vec!["I cannot find any files to review, so here is my reasoning instead."],
        Arc::clone(&seen),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    match synthesize_workflow("review every file", &spawner).await {
        Ok(_) => panic!("persistent prose must abort, got a plan"),
        Err(SynthError::NoRonBlock { attempt, .. }) => assert_eq!(attempt, 3),
        Err(other) => panic!("expected NoRonBlock, got {other:?}"),
    }
    assert_eq!(
        seen.lock().unwrap().len(),
        3,
        "must abort after exactly three attempts"
    );
}

/// 5. Invalid (extractable but unparseable) RON on attempt 1, valid on attempt 2
///    → succeeds via the parse-retry path, and the re-prompt carried the parse
///    error verbatim plus the prior RON.
#[tokio::test]
async fn invalid_then_valid_succeeds_via_parse_retry() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    // Attempt 0: a Workflow block the extractor finds but the PARSER rejects
    // (empty phases → EmptyWorkflow). Attempt 1: valid.
    let provider = Arc::new(SequencedProvider::new(
        vec![r#"Workflow(meta: (name: "x"), phases: [])"#, VALID_RON],
        Arc::clone(&seen),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = synthesize_workflow("review every file", &spawner)
        .await
        .expect("a parse-retry should recover");
    assert_eq!(plan.meta.name, "review-changes");

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "expected one re-prompt (2 total calls)");
    // The re-prompt fed the parse error back to the model.
    assert!(
        seen[1].contains("Your RON did not parse"),
        "re-prompt must carry the parse-error correction; got: {}",
        seen[1]
    );
    // And it echoed the prior invalid RON so the model can see what it emitted.
    assert!(
        seen[1].contains("your previous RON"),
        "re-prompt must include the prior RON; got: {}",
        seen[1]
    );
}

/// 6. Invalid RON on ALL three attempts → a typed `InvalidAfterReprompt`, no
///    panic, no fabricated workflow. Proves the 3-attempt budget aborts cleanly.
#[tokio::test]
async fn invalid_all_attempts_returns_synth_error() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    // Every response contains a Workflow block the extractor finds but the parser
    // rejects (empty phases), so no attempt validates.
    let provider = Arc::new(SequencedProvider::new(
        vec![r#"Workflow(meta: (name: "x"), phases: [])"#],
        Arc::clone(&seen),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    // `WorkflowPlan` does not derive `Debug`, so `expect_err` (which formats the
    // Ok side) can't be used; match the result directly instead.
    match synthesize_workflow("review every file", &spawner).await {
        Ok(_) => panic!("persistent invalid RON must abort, got a plan"),
        Err(SynthError::InvalidAfterReprompt { attempt, .. }) => assert_eq!(attempt, 3),
        Err(other) => panic!("expected InvalidAfterReprompt, got {other:?}"),
    }
    // EXACTLY three LLM calls (the initial + two re-prompts), then it aborts.
    assert_eq!(
        seen.lock().unwrap().len(),
        3,
        "must abort after exactly three attempts"
    );
}

/// 7. RON wrapped in prose + markdown fences → extraction still finds the
///    `Workflow(...)` block on the first attempt (no wasted re-prompt).
#[tokio::test]
async fn ron_wrapped_in_prose_and_fences_is_extracted() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let wrapped =
        format!("Sure! Here is your workflow:\n\n```ron\n{VALID_RON}\n```\n\nLet me know!");
    let provider = Arc::new(SequencedProvider::new(
        vec![wrapped.as_str()],
        Arc::clone(&seen),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = synthesize_workflow("review every file", &spawner)
        .await
        .expect("fenced + prose-wrapped RON should still extract and parse");

    assert_eq!(plan.meta.name, "review-changes");
    // Extracted on the first attempt — no re-prompt needed despite the fences.
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "fence/prose handling must not waste the re-prompt"
    );
}
