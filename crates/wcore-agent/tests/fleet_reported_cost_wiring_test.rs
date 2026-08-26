//! #1139, second half: a provider-reported cost must survive from the child's
//! round-trips to the parent that dispatched it.
//!
//! `provider_reported_cost_wiring_test` grades the FRONT of the chain — SSE
//! bytes to `LedgerSummary`. It stops at the ledger, and deliberately: it never
//! looks at `AgentResult.usage`. This file grades the BACK, and the two do not
//! overlap, so neither can keep the other green:
//!
//!   1. `AgentEngine` folds each round-trip's `reported_cost_usd` into the
//!      session total. It accumulated only the four token counters, so
//!      `AgentResult.usage.reported_cost_usd` was `None` on every path — and
//!      therefore so was every `SubAgentResult`, on every spawn topology.
//!   2. The FLEET topology then round-trips `SubAgentResult` through a
//!      hand-rolled JSON codec (`sub_agent_result_to_payload` /
//!      `payload_to_sub_agent_result`) that named the four token fields and
//!      nothing else — dropping the cost again at both ends.
//!
//! Driven through `AgentSpawner::spawn_via_fleet`, the production call site of
//! that codec pair, so breaking either end shows up here.

mod common;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::spawner::AgentSpawner;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};
use wcore_types::spawner::SubAgentConfig;

use common::{bound_test_spawner, test_config};

/// Two round-trip prices that sum to a third distinct value, so a test that
/// silently kept only one of them still fails.
const FIRST_USD: f64 = 0.012_500;
const SECOND_USD: f64 = 0.030_250;
const TOTAL_USD: f64 = 0.042_750;

fn usage(reported: Option<f64>) -> TokenUsage {
    TokenUsage {
        input_tokens: 1_000,
        output_tokens: 200,
        reported_cost_usd: reported,
        ..Default::default()
    }
}

struct ScriptedProvider {
    script: Mutex<VecDeque<Vec<LlmEvent>>>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let events = self.script.lock().unwrap().pop_front().unwrap_or_else(|| {
            // Tail: a priced end-turn, so a child that outruns its script does
            // not silently poison the aggregate and fake this test green.
            end_turn("tail", Some(0.0))
        });
        let (tx, rx) = mpsc::channel(64);
        for e in events {
            let _ = tx.send(e).await;
        }
        Ok(rx)
    }
}

fn end_turn(text: &str, reported: Option<f64>) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(reported),
        },
    ]
}

fn tool_turn(reported: Option<f64>) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: "call-1139".to_string(),
            name: "no_such_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: usage(reported),
        },
    ]
}

/// Dispatch one child through the REAL fleet path and hand back what the parent
/// receives on the other side of the JSON codec.
async fn fleet_child(script: Vec<Vec<LlmEvent>>) -> TokenUsage {
    let provider = Arc::new(ScriptedProvider {
        script: Mutex::new(VecDeque::from(script)),
    });
    let (spawner, _root) = bound_test_spawner(AgentSpawner::new(provider, test_config()));
    let mut results = spawner
        .spawn_via_fleet(
            vec![SubAgentConfig {
                name: "worker".to_string(),
                prompt: "do the thing".to_string(),
                max_turns: 5,
                max_tokens: 1024,
                system_prompt: None,
                provider: None,
                model: None,
                temperature: None,
            }],
            "fleet-1139",
        )
        .await;
    assert_eq!(results.len(), 1, "one child dispatched, one result back");
    results.remove(0).usage
}

// ────────────────────────────────────────────────────────────────────────────

/// The chain, end to end: two priced round-trips, summed by the engine and
/// carried home across the fleet codec.
#[tokio::test]
async fn a_fleet_dispatched_childs_reported_cost_reaches_the_parent() {
    let usage = fleet_child(vec![
        tool_turn(Some(FIRST_USD)),
        end_turn("done", Some(SECOND_USD)),
    ])
    .await;

    let got = usage
        .reported_cost_usd
        .expect("the child was priced on every round-trip, so the parent must get a figure");
    assert!(
        (got - TOTAL_USD).abs() < 1e-9,
        "the parent must receive the SUM of the child's round-trips \
         ({FIRST_USD} + {SECOND_USD} = {TOTAL_USD}); got {got}"
    );
    // The token counters are the known-positive: they always survived this
    // codec, so if they were missing the harness itself would be broken and
    // the cost assertion above would mean nothing.
    assert_eq!(usage.input_tokens, 2_000, "two round-trips of input");
    assert_eq!(usage.output_tokens, 400);
}

/// THE HONESTY CONTROL. One round-trip priced, one silent. The total must come
/// back `None` — unknown — and NOT the partial sum, which would be a floor
/// rendered in a field that reads as a total.
#[tokio::test]
async fn one_unpriced_round_trip_makes_the_whole_child_unpriced() {
    let usage = fleet_child(vec![tool_turn(Some(FIRST_USD)), end_turn("done", None)]).await;

    assert_eq!(
        usage.reported_cost_usd, None,
        "a session with an unpriced round-trip has no total — reporting the \
         priced subset would be a floor wearing a total's clothes"
    );
    assert_eq!(
        usage.input_tokens, 2_000,
        "the token counters are unaffected: only the COST is unknown"
    );
}

/// THE NEGATIVE CONTROL. Nothing reported anywhere. Must be `None`, never
/// `Some(0.0)` — the distinction the whole ticket is about. Without this, a
/// change that hard-coded `Some(0.0)` would satisfy neither of the above but
/// would still look like "a cost reached the parent".
#[tokio::test]
async fn a_child_no_one_priced_reports_unknown_not_free() {
    let usage = fleet_child(vec![end_turn("done", None)]).await;

    assert_eq!(
        usage.reported_cost_usd, None,
        "no provider figure anywhere means UNKNOWN; `Some(0.0)` would claim \
         the call was free"
    );
    assert_eq!(usage.input_tokens, 1_000, "the child did run");
}
