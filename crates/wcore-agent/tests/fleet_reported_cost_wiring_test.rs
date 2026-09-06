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

/// W15: actual provider HTTP plus production in-process fleet sharding and
/// codec. No distributed worker or restart claim is made by this witness.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fleet_http_children_preserve_identity_cost_and_cancellation() {
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    use std::time::Duration;
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;
    use wcore_agent::agents::bus::{AgentBus, AgentMessage};
    use wcore_budget::{BudgetCap, BudgetTracker};
    use wcore_config::compat::ProviderCompat;
    use wcore_config::config::ProviderType;
    use wcore_config::debug::DebugConfig;
    use wcore_providers::openai::OpenAIProvider;
    use wcore_types::spawner::{ChildOrigin, DurableChildStatus};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const COUNT: usize = 11; // Smallest input that crosses the ten-child shard boundary.
    let markers: Vec<_> = (0..COUNT)
        .map(|i| format!("W15_FLEET_{i:02}_END"))
        .collect();
    let cost = |index: usize| (index + 1) as f64 * 0.000_013_7;
    for cancel_in_flight in [false, true] {
        let server = MockServer::start().await;
        let observed = Arc::new(Notify::new());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let response_requests = requests.clone();
        let response_observed = observed.clone();
        let response_markers = markers.clone();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(move |request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                let messages = body["messages"].to_string();
                let index = response_markers.iter().position(|marker| messages.contains(marker))
                    .expect("HTTP request carries its own fleet marker");
                response_requests.lock().unwrap().push(index);
                let marker = &response_markers[index];
                let id = format!("chatcmpl-w15-fleet-{index}");
                let text = json!({"id":id,"object":"chat.completion.chunk","created":0,
                    "model":"gpt-4o","choices":[{"index":0,"delta":{"content":marker},"finish_reason":null}]});
                let stop = json!({"id":id,"object":"chat.completion.chunk","created":0,
                    "model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
                let usage = json!({"id":id,"object":"chat.completion.chunk","created":0,
                    "model":"gpt-4o","choices":[],"usage":{"prompt_tokens":120,
                    "completion_tokens":34,"cost_usd":cost(index)}});
                response_observed.notify_one();
                let response = ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!("data: {text}\n\ndata: {stop}\n\ndata: {usage}\n\ndata: [DONE]\n\n"));
                if cancel_in_flight { response.set_delay(Duration::from_millis(250)) } else { response }
            }).mount(&server).await;
        let compat = ProviderCompat::openai_defaults();
        let provider = Arc::new(OpenAIProvider::new(
            "w15-fake-fleet-key",
            &server.uri(),
            compat.clone(),
            DebugConfig::default(),
        ));
        let mut config = test_config();
        config.provider = ProviderType::OpenAI;
        config.provider_label = "openai".into();
        config.base_url = server.uri();
        config.model = "gpt-4o".into();
        config.compat = compat;
        let budget = Arc::new(parking_lot::Mutex::new(BudgetTracker::new(
            BudgetCap::builder().per_session_usd(1.0).build(),
        )));
        let cancel = CancellationToken::new();
        let bus = Arc::new(AgentBus::new(256));
        let mut events = bus.subscribe();
        let (spawner, journal, _root) = common::bind_test_spawner(
            AgentSpawner::new(provider, config)
                .with_cancel(cancel.clone())
                .with_bus(bus),
        );
        let session = spawner.durable_session_id().unwrap();
        let spawner = spawner.with_provider_budget(budget.clone(), session.clone());
        let tasks = markers
            .iter()
            .map(|marker| SubAgentConfig {
                name: marker.clone(),
                prompt: marker.clone(),
                max_turns: 1,
                max_tokens: 1024,
                system_prompt: None,
                provider: None,
                model: None,
                temperature: None,
            })
            .collect();
        let results = tokio::time::timeout(Duration::from_secs(15), async {
            let (results, ()) = tokio::join!(spawner.spawn_via_fleet(tasks, "w15-http"), async {
                if cancel_in_flight {
                    observed.notified().await;
                    cancel.cancel();
                }
            });
            results
        })
        .await
        .expect("fleet must terminate after success or cancellation");
        assert_eq!(
            results.len(),
            COUNT,
            "fleet reducer retains every task result"
        );
        let names: BTreeSet<_> = results.iter().map(|result| result.name.clone()).collect();
        assert_eq!(names, markers.iter().cloned().collect());
        let state = journal.state().unwrap();
        let children: Vec<_> = state
            .children
            .values()
            .map(|child| child.durable.as_ref().expect("production durable child"))
            .collect();
        let ids: BTreeSet<_> = children
            .iter()
            .map(|child| child.child_id.as_str())
            .collect();
        assert_eq!(ids.len(), children.len());
        for child in &children {
            assert_eq!(child.parent.session_id, session);
            assert_eq!(child.origin, ChildOrigin::Fleet);
            assert!(child.status.is_terminal());
        }
        assert_eq!(budget.lock().reserved_totals(&session), (0, 0.0));
        if cancel_in_flight {
            assert!(cancel.is_cancelled());
            assert!(results.iter().any(|result| result.is_error));
            assert!(
                !requests.lock().unwrap().is_empty(),
                "cancellation follows physical HTTP dispatch"
            );
            let completed_count = requests.lock().unwrap().len();
            tokio::time::sleep(Duration::from_millis(350)).await;
            assert_eq!(
                requests.lock().unwrap().len(),
                completed_count,
                "no physical request after the fleet has terminated"
            );
            let (tokens, charged) = budget.lock().session_totals(&session);
            assert!(
                tokens > 0 && charged > 0.0,
                "cancelled possibly-dispatched requests retain their charge"
            );
        } else {
            let seen = requests.lock().unwrap();
            assert_eq!(seen.len(), COUNT);
            assert_eq!(
                seen.iter().copied().collect::<BTreeSet<_>>(),
                (0..COUNT).collect()
            );
            assert_eq!(children.len(), COUNT);
            assert!(
                children
                    .iter()
                    .all(|child| child.status == DurableChildStatus::Succeeded)
            );
            let mut returned_cost = 0.0;
            for result in &results {
                let index = markers
                    .iter()
                    .position(|marker| *marker == result.name)
                    .unwrap();
                assert!(!result.is_error);
                assert_eq!(
                    result.text, markers[index],
                    "no cross-child result substitution"
                );
                assert_eq!(
                    (result.usage.input_tokens, result.usage.output_tokens),
                    (120, 34)
                );
                let billed = result
                    .usage
                    .reported_cost_usd
                    .expect("wire price survives fleet codec");
                assert!((billed - cost(index)).abs() < 1e-9);
                returned_cost += billed;
            }
            let (tokens, charged) = budget.lock().session_totals(&session);
            assert_eq!(tokens, COUNT as u64 * (120 + 34));
            assert!((charged - (0..COUNT).map(cost).sum::<f64>()).abs() < 1e-9);
            assert!((charged - returned_cost).abs() < 1e-9);
            let mut shard_tags = Vec::new();
            while let Ok(event) = events.try_recv() {
                if let AgentMessage::Spawned {
                    parent_call_id: Some(tag),
                    ..
                } = event
                    && tag.starts_with("fleet:")
                {
                    shard_tags.push(tag);
                }
            }
            assert_eq!(
                shard_tags.len(),
                COUNT,
                "all children traverse fleet dispatch"
            );
            assert!(shard_tags.iter().any(|tag| tag.contains("shard-0")));
            assert!(shard_tags.iter().any(|tag| tag.contains("shard-1")));
        }
    }
}
