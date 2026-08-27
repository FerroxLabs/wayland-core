//! #1140 wiring: a failed sub-agent must reach the PARENT with its own
//! diagnostic and with the spend it actually incurred.
//!
//! Both halves are graded through `SpawnTool::execute` — the parent-visible
//! surface — because that is where the loss happened:
//!
//!  * the child's real error text goes to the CHILD's `OutputSink` (`NullSink`,
//!    or a `ChannelSink` feeding `--json-stream`), so only a JSON-stream host
//!    ever saw it; the parent LLM got a generic termination line, or — for a
//!    reasoning-only turn — a blank success, and
//!  * the `Err` arm of the launch built `SubAgentResult { usage:
//!    TokenUsage::default(), turns: 0, .. }`, throwing away everything the
//!    child had accumulated before it failed.
//!
//! Asserting on `subagent_ok_result` or on the `spawn_tool` formatter directly
//! would pass with either fix reverted, so nothing here does.

mod common;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use wcore_agent::spawn_tool::SpawnTool;
use wcore_agent::spawner::AgentSpawner;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::Tool;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{bound_test_spawner_arc, test_config};

/// Non-trivial per-round-trip usage, so "the child burned tokens" is a
/// measurement rather than an accident of a default.
const IN_TOKENS: u64 = 4_321;
const OUT_TOKENS: u64 = 765;

fn usage() -> TokenUsage {
    TokenUsage {
        input_tokens: IN_TOKENS,
        output_tokens: OUT_TOKENS,
        ..Default::default()
    }
}

/// One scripted outcome per `stream()` call.
struct ScriptedProvider {
    script: Mutex<VecDeque<Result<Vec<LlmEvent>, ProviderError>>>,
    calls: AtomicUsize,
}

impl ScriptedProvider {
    fn new(script: Vec<Result<Vec<LlmEvent>, ProviderError>>) -> Self {
        Self {
            script: Mutex::new(VecDeque::from(script)),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = self.script.lock().unwrap().pop_front();
        let events = match next {
            Some(Ok(events)) => events,
            // A script that runs out must not hang the run.
            Some(Err(e)) => return Err(e),
            None => {
                return Err(ProviderError::Api {
                    status: 400,
                    message: "script exhausted".to_string(),
                });
            }
        };
        let (tx, rx) = mpsc::channel(64);
        for event in events {
            let _ = tx.send(event).await;
        }
        Ok(rx)
    }
}

/// A turn that thinks and says nothing — the #1109b shape. The engine emits
/// "The model produced only reasoning…" for it, into the child's own sink.
fn reasoning_only_turn() -> Vec<LlmEvent> {
    vec![
        LlmEvent::ThinkingDelta("I should probably answer this.".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ]
}

/// A turn that calls a tool, so the agent loop continues to a second
/// round-trip (the tool need not exist — a failed call still keeps the loop
/// going, which is all this needs).
fn tool_turn() -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: "call-1140".to_string(),
            name: "no_such_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: usage(),
        },
    ]
}

/// Spawn one child driven by `script` and return the PARENT's tool result text.
async fn spawn_child(script: Vec<Result<Vec<LlmEvent>, ProviderError>>) -> (String, bool) {
    let provider = std::sync::Arc::new(ScriptedProvider::new(script));
    let (spawner, _root) = bound_test_spawner_arc(AgentSpawner::new(provider, test_config()));
    let tool = SpawnTool::new(spawner);
    let result = tool
        .execute(json!({
            "tasks": [{ "name": "worker", "prompt": "do the thing" }]
        }))
        .await;
    (result.content, result.is_error)
}

/// Every token count the parent renders, as `(turns, input, output)`.
fn reported_counters(content: &str) -> (u64, u64, u64) {
    // `[turns: N | tokens: A in / B out]`
    let tail = content
        .rsplit_once("[turns: ")
        .unwrap_or_else(|| panic!("no counters line in the parent's tool result:\n{content}"))
        .1;
    let turns: u64 = tail
        .split(' ')
        .next()
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| panic!("unparseable turn count in:\n{content}"));
    let after = tail.split_once("tokens: ").expect("tokens segment").1;
    let mut nums = after.split_whitespace();
    let input: u64 = nums.next().unwrap().parse().unwrap();
    nums.next(); // "in"
    nums.next(); // "/"
    let output: u64 = nums.next().unwrap().parse().unwrap();
    (turns, input, output)
}

// ────────────────────────────────────────────────────────────────────────────

/// HALF ONE — the swallowed diagnostic. A reasoning-only child returns `Ok`
/// with empty text, so the parent used to receive a blank body under an `[OK]`
/// heading: the strongest possible statement that nothing was wrong.
#[tokio::test]
async fn a_reasoning_only_child_hands_the_parent_the_engines_real_diagnostic() {
    let (content, is_error) = spawn_child(vec![Ok(reasoning_only_turn())]).await;

    assert!(
        content.contains("produced only reasoning"),
        "the parent must receive the CHILD's own diagnostic, not a generic \
         line. Got:\n{content}"
    );
    assert!(
        is_error,
        "a child that produced no answer at all has not succeeded:\n{content}"
    );

    let (turns, input, output) = reported_counters(&content);
    assert_eq!(turns, 1, "the child took a real turn:\n{content}");
    assert_eq!((input, output), (IN_TOKENS, OUT_TOKENS));
}

/// HALF TWO — the discarded spend. The child completes one billed round-trip,
/// then its provider fails. The `Err` arm reported `turns: 0` and zero tokens
/// over work that had already been paid for.
#[tokio::test]
async fn a_child_that_errors_after_a_billed_round_trip_still_reports_that_spend() {
    let (content, is_error) = spawn_child(vec![
        Ok(tool_turn()),
        Err(ProviderError::Api {
            status: 400,
            message: "invalid x-api-key".to_string(),
        }),
    ])
    .await;

    assert!(is_error, "the child failed:\n{content}");

    let (turns, input, output) = reported_counters(&content);
    assert_eq!(
        turns, 1,
        "one round-trip completed before the failure, and the parent must be \
         told so:\n{content}"
    );
    assert_eq!(
        (input, output),
        (IN_TOKENS, OUT_TOKENS),
        "the tokens the child burned before failing are spend, and reporting \
         them as zero is how a failed Spawn costs nothing:\n{content}"
    );
}

/// THE CONTROL. A child that answers normally must keep reporting exactly what
/// it always did — no synthesized diagnostic, no error flag. Without this, both
/// assertions above could be satisfied by a change that simply marks every
/// child failed.
#[tokio::test]
async fn the_control_a_healthy_child_is_untouched() {
    let (content, is_error) = spawn_child(vec![Ok(vec![
        LlmEvent::TextDelta("the answer".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ])])
    .await;

    assert!(!is_error, "a healthy child is not an error:\n{content}");
    assert!(content.contains("the answer"), "{content}");
    assert!(
        !content.contains("produced only reasoning")
            && !content.contains("terminated without completing"),
        "no failure text may be synthesized for a healthy child:\n{content}"
    );
}
