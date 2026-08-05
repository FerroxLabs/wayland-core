//! Loop-convergence E2E for the engine-side runaway breaker.
//!
//! A model that repeats the SAME tool call every turn, against a tool that
//! returns the SAME failing result, must be stopped by the breaker WELL BEFORE
//! `max_turns` — proving a no-progress loop converges instead of burning tokens
//! to the turn cap (the "8.5M tokens in 2 hours" class of report).

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use wcore_agent::engine::{AgentEngine, AgentError};
use wcore_agent::output::OutputSink;
use wcore_agent::test_utils::TestSink;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::Tool;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};
use wcore_types::tool::ToolResult;

use common::{MockLlmProvider, MockTool, test_config};

fn assert_midflight_monitor_observed(events: &[serde_json::Value]) {
    assert_midflight_monitor_occurrences(events, 1);
}

fn assert_midflight_monitor_occurrences(events: &[serde_json::Value], count: usize) {
    let stages: Vec<_> = events
        .iter()
        .filter(|event| {
            event["type"].as_str() == Some("capability_activation")
                && event["capability"].as_str() == Some("mid_flight_monitor")
        })
        .filter_map(|event| event["stage"].as_str())
        .collect();
    let expected = ["reached", "outcome_changed", "observed"].repeat(count);
    assert_eq!(
        stages, expected,
        "a monitor-owned outcome must emit complete runtime proof; construction is emitted \
         once by production bootstrap: {events:?}"
    );
}

fn assert_monitor_decision(events: &[serde_json::Value], directive: &str, reason: &str) {
    assert!(
        events.iter().any(|event| {
            event["type"].as_str() == Some("mid_flight_monitor_decision")
                && event["directive"].as_str() == Some(directive)
                && event["reason"].as_str() == Some(reason)
        }),
        "missing monitor decision {directive}/{reason}: {events:?}"
    );
}

/// One turn that asks for the same tool with the same args. The id varies per
/// turn (so per-turn history is well-formed); the breaker keys on
/// name+args+result, not id, so every turn shares one signature.
fn loop_turn(i: usize) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: format!("call-{i}"),
            name: "loop_tool".to_string(),
            input: json!({ "q": "same" }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage {
                input_tokens: 50,
                output_tokens: 10,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

struct RecordingProvider {
    turns: Mutex<Vec<Vec<LlmEvent>>>,
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

struct BlockingProvider {
    entered: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl LlmProvider for BlockingProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
        self.entered.notify_one();
        std::future::pending().await
    }
}

impl RecordingProvider {
    fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            turns: Mutex::new(turns),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn requests(&self) -> Arc<Mutex<Vec<LlmRequest>>> {
        Arc::clone(&self.requests)
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        let events = self.turns.lock().unwrap().remove(0);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

struct SequencedErrorTool {
    name: &'static str,
    errors: Mutex<Vec<String>>,
}

struct VolatileSuccessTool {
    name: &'static str,
    calls: std::sync::atomic::AtomicU32,
}

#[async_trait]
impl Tool for VolatileSuccessTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Returns the same outcome with a volatile counter"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Info
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn execute(&self, _input: serde_json::Value) -> ToolResult {
        let call = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        ToolResult {
            content: format!("unchanged request_id {call}"),
            is_error: false,
        }
    }
}

struct ImprovingTool {
    name: &'static str,
    remaining: Arc<std::sync::atomic::AtomicU32>,
}

#[async_trait]
impl Tool for ImprovingTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Returns a meaningfully improving numeric outcome"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Info
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn execute(&self, _input: serde_json::Value) -> ToolResult {
        let remaining = self
            .remaining
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        ToolResult {
            content: format!("{remaining} tests failed"),
            is_error: false,
        }
    }
}

#[async_trait]
impl Tool for SequencedErrorTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "Returns one root error with volatile details"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }

    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Info
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    async fn execute(&self, _input: serde_json::Value) -> ToolResult {
        ToolResult {
            content: self.errors.lock().unwrap().remove(0),
            is_error: true,
        }
    }
}

#[tokio::test]
async fn repeated_identical_successful_tool_call_converges_via_loopguard() {
    // Identical-SUCCESS no-progress loop: the model calls the same tool with the
    // same args, getting the same NON-error result every turn. This is
    // LoopGuard's domain — it keys on the full (tool, args, is_error, content)
    // signature, so an identical successful call accumulates and trips at the
    // threshold. (#475: the failing-loop case is now owned by FailureGuard —
    // see `failing_tool_loop_converges_via_failure_cap_with_maxturns` below.)
    // max_turns raised to 30 so the breaker (default 10), not the turn cap, is
    // what stops it.
    let turns: Vec<Vec<LlmEvent>> = (0..30).map(loop_turn).collect();
    let provider = Arc::new(MockLlmProvider::with_turns(turns));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new(
        "loop_tool",
        "same result every time",
        false, // identical SUCCESSFUL outcome — FailureGuard ignores successes
    )));

    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut config = test_config();
    config.max_turns = Some(30);

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("do the thing", "")
        .await
        .expect("run completes (terminated cleanly, not Err)");

    // LoopGuard (threshold 10) must stop the loop well before max_turns(30).
    assert!(
        result.turns < 30,
        "LoopGuard must converge the identical-success loop before max_turns; turns = {}",
        result.turns
    );

    // …and surface the no-progress-loop error (LoopGuard's message).
    let events = handle.snapshot();
    let saw_loop_error = events
        .iter()
        .any(|e| e["type"].as_str() == Some("error") && e.to_string().contains("no-progress loop"));
    assert!(
        saw_loop_error,
        "expected a visible no-progress-loop error event; got {events:?}"
    );
}

#[tokio::test]
async fn repeated_root_error_injects_a_changed_strategy_directive() {
    let mut turns: Vec<Vec<LlmEvent>> = (0..3)
        .map(|i| {
            vec![
                LlmEvent::ToolUse {
                    id: format!("unstable-{i}"),
                    name: "unstable_tool".to_string(),
                    input: json!({ "attempt": i }),
                    extra: None,
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                    finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
                    usage: TokenUsage {
                        input_tokens: 20,
                        output_tokens: 5,
                        ..Default::default()
                    },
                },
            ]
        })
        .collect();
    turns.push(vec![
        LlmEvent::TextDelta("changed approach".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage {
                input_tokens: 20,
                output_tokens: 5,
                ..Default::default()
            },
        },
    ]);
    let provider = Arc::new(RecordingProvider::new(turns));
    let requests = provider.requests();

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(SequencedErrorTool {
        name: "unstable_tool",
        errors: Mutex::new(vec![
            "permission denied at /tmp/a/work.dat line 1".to_string(),
            "permission denied at /tmp/b/work.dat line 2".to_string(),
            "permission denied at /tmp/c/work.dat line 3".to_string(),
        ]),
    }));

    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut engine = AgentEngine::new_with_provider(provider, test_config(), registry, output);
    let result = engine
        .run("recover from the tool error", "")
        .await
        .expect("the monitor replans instead of terminating the run");
    assert_eq!(result.text, "changed approach");

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    let final_request = serde_json::to_string(&requests[3].messages).unwrap();
    assert!(
        final_request.contains("Mid-flight monitor directive"),
        "the turn after the third normalized root error must carry changed-strategy guidance: {final_request}"
    );
    let events = handle.snapshot();
    assert_monitor_decision(&events, "replan", "repeated_error");
    assert_midflight_monitor_observed(&events);
}

#[tokio::test]
async fn varied_bash_failures_remain_bounded_without_a_turn_cap() {
    let turns = (0..20)
        .map(|attempt| {
            vec![
                LlmEvent::ToolUse {
                    id: format!("bash-{attempt}"),
                    name: "Bash".to_string(),
                    input: json!({ "command": format!("build target-{attempt}") }),
                    extra: None,
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                    finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
                    usage: TokenUsage::default(),
                },
            ]
        })
        .collect();
    let provider = Arc::new(RecordingProvider::new(turns));
    let requests = provider.requests();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(SequencedErrorTool {
        name: "Bash",
        errors: Mutex::new(
            ["a", "b", "c", "d", "e", "f"]
                .into_iter()
                .enumerate()
                .map(|(attempt, directory)| {
                    format!(
                        "compiler missing at /tmp/{directory}/work.dat line {}",
                        attempt + 1
                    )
                })
                .collect(),
        ),
    }));
    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut config = test_config();
    config.max_turns = None;
    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);

    let result = engine
        .run("keep changing the build command", "")
        .await
        .expect("the monitor must stop repeated Bash root failures cleanly");

    assert_eq!(result.finish_reason, FinishReason::MaxTurns);
    let request_count = requests.lock().unwrap().len();
    assert!(
        request_count < 20,
        "the repeated-error monitor must bound Bash even after its tool circuit opens; requests={request_count}"
    );
    let events = handle.snapshot();
    assert_monitor_decision(&events, "replan", "repeated_error");
    assert_monitor_decision(&events, "stop", "repeated_error");
    assert_midflight_monitor_occurrences(&events, 3); // root failure replan, circuit-open replan, circuit-open stop
    assert!(
        events
            .iter()
            .all(|event| !event.to_string().contains("times in a row")),
        "Bash remains outside FailureGuard; the monitor must own this stop: {events:?}"
    );
}

#[tokio::test]
async fn normalized_alternating_route_replans_once_then_stops_if_ignored() {
    let turns = (0..12)
        .map(|index| named_turn(index, if index % 2 == 0 { "route_a" } else { "route_b" }))
        .collect();
    let provider = Arc::new(RecordingProvider::new(turns));
    let requests = provider.requests();

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(VolatileSuccessTool {
        name: "route_a",
        calls: std::sync::atomic::AtomicU32::new(0),
    }));
    registry.register(Box::new(VolatileSuccessTool {
        name: "route_b",
        calls: std::sync::atomic::AtomicU32::new(0),
    }));

    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut config = test_config();
    config.max_turns = Some(30);
    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);

    let result = engine
        .run("repeat the alternating route", "")
        .await
        .expect("the monitor stops an ignored route loop cleanly");

    assert_eq!(result.finish_reason, FinishReason::MaxTurns);
    assert_eq!(
        requests.lock().unwrap().len(),
        12,
        "three A/B cycles trigger one replan; three more trigger a bounded stop"
    );
    let events = handle.snapshot();
    assert_monitor_decision(&events, "replan", "repeated_tool_route");
    assert_monitor_decision(&events, "stop", "repeated_tool_route");
    assert_midflight_monitor_occurrences(&events, 2);
}

#[tokio::test]
async fn one_step_volatile_route_replans_once_then_stops_if_ignored() {
    let turns = (0..6)
        .map(|index| named_turn(index, "route_single"))
        .collect();
    let provider = Arc::new(RecordingProvider::new(turns));
    let requests = provider.requests();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(VolatileSuccessTool {
        name: "route_single",
        calls: std::sync::atomic::AtomicU32::new(1000),
    }));
    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut config = test_config();
    config.max_turns = Some(30);
    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);

    let result = engine
        .run("repeat one volatile route", "")
        .await
        .expect("the monitor stops an ignored one-step route cleanly");
    assert_eq!(result.finish_reason, FinishReason::MaxTurns);
    assert_eq!(requests.lock().unwrap().len(), 6);
    let events = handle.snapshot();
    assert_monitor_decision(&events, "replan", "repeated_tool_route");
    assert_monitor_decision(&events, "stop", "repeated_tool_route");
    assert_midflight_monitor_occurrences(&events, 2);
}

#[tokio::test]
async fn improving_numeric_outcomes_do_not_trip_route_monitor() {
    let turns = (0..10)
        .map(|index| named_turn(index, if index % 2 == 0 { "test_a" } else { "test_b" }))
        .collect();
    let provider = Arc::new(RecordingProvider::new(turns));
    let remaining = Arc::new(std::sync::atomic::AtomicU32::new(10));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ImprovingTool {
        name: "test_a",
        remaining: Arc::clone(&remaining),
    }));
    registry.register(Box::new(ImprovingTool {
        name: "test_b",
        remaining,
    }));
    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut config = test_config();
    config.max_turns = Some(10);
    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);

    let result = engine
        .run("iterate until the tests pass", "")
        .await
        .expect("meaningful numeric progress must reach the normal turn cap");
    assert_eq!(result.turns, 10);
    assert_eq!(result.finish_reason, FinishReason::MaxTurns);
    assert!(
        handle
            .snapshot()
            .iter()
            .all(|event| event["type"].as_str() != Some("mid_flight_monitor_decision")),
        "meaningful numeric progress must not be classified as a route loop"
    );
}

#[tokio::test]
async fn host_cancel_interrupts_a_provider_that_never_opens_a_stream() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let provider = Arc::new(BlockingProvider {
        entered: Arc::clone(&entered),
    });
    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut engine =
        AgentEngine::new_with_provider(provider, test_config(), ToolRegistry::new(), output);
    let cancel = engine.cancel_token();

    let run = tokio::spawn(async move { engine.run("wait forever", "").await });
    tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
        .await
        .expect("provider call must begin");
    cancel.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), run)
        .await
        .expect("host cancellation must bound provider wait")
        .expect("run task must join");
    assert!(matches!(result, Err(AgentError::UserAborted)));

    let monitor_stages: Vec<_> = handle
        .snapshot()
        .into_iter()
        .filter(|event| {
            event["type"].as_str() == Some("capability_activation")
                && event["capability"].as_str() == Some("mid_flight_monitor")
        })
        .filter_map(|event| event["stage"].as_str().map(str::to_string))
        .collect();
    assert!(
        monitor_stages.is_empty(),
        "cancellation alone must not claim a monitor occurrence; the one-time \
         construction chain belongs to production bootstrap"
    );
}

#[tokio::test]
async fn failing_tool_loop_converges_via_failure_cap_with_maxturns() {
    // #475: the model retries a tool that keeps FAILING (here identically, but
    // FailureGuard is content-agnostic so varied-args validation-error loops
    // converge the same way). FailureGuard supersedes LoopGuard here — it is
    // immune to the tool-registry circuit breaker changing the error text after
    // ~3 failures (which resets LoopGuard's content-keyed streak). It converges
    // the loop before max_turns and, per #457, exits with finish_reason=max_turns
    // so the host can offer "Continue" rather than a model-failure UX.
    let turns: Vec<Vec<LlmEvent>> = (0..30).map(loop_turn).collect();
    let provider = Arc::new(MockLlmProvider::with_turns(turns));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new(
        "loop_tool",
        "network is unreachable in this sandbox",
        true, // failing outcome every call
    )));

    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut config = test_config();
    config.max_turns = Some(30);

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("install the deps", "")
        .await
        .expect("run completes (terminated cleanly, not Err)");

    assert!(
        result.turns < 30,
        "the failure-cap must converge the failing loop before max_turns; turns = {}",
        result.turns
    );

    // #457 wiring: a retry-cap stop is Continue-able, not a hard failure.
    assert_eq!(
        result.finish_reason,
        FinishReason::MaxTurns,
        "the failure-cap exit must surface finish_reason=max_turns (Continue-able)"
    );

    // …and surface the failure-cap guidance (FailureGuard's message).
    let events = handle.snapshot();
    let saw_failure_cap = events.iter().any(|e| {
        e["type"].as_str() == Some("error") && e.to_string().contains("failed") && {
            e.to_string().contains("times in a row")
        }
    });
    assert!(
        saw_failure_cap,
        "expected a visible failure-cap error event; got {events:?}"
    );
}

#[tokio::test]
async fn varied_content_failing_loop_converges_via_failure_cap_only() {
    // Isolation proof (audit follow-up): a tool that FAILS with DIFFERENT
    // content every call. LoopGuard keys on (tool, args, content), so its
    // signature changes every turn and it NEVER accumulates — the ONLY breaker
    // that can converge this loop is FailureGuard (content-agnostic). Proves
    // FailureGuard owns the failing loop DETERMINISTICALLY, independent of the
    // circuit breaker or LoopGuard, and exits Continue-able (#457).
    let turns: Vec<Vec<LlmEvent>> = (0..30).map(loop_turn).collect();
    let provider = Arc::new(MockLlmProvider::with_turns(turns));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FailingChangingTool::default()));

    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut config = test_config();
    config.max_turns = Some(30);

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("keep trying", "").await.expect("run completes");

    assert!(
        result.turns < 30,
        "FailureGuard must converge the varied-content failing loop; turns = {}",
        result.turns
    );
    assert_eq!(
        result.finish_reason,
        FinishReason::MaxTurns,
        "the failure-cap exit must be Continue-able (max_turns), not a hard error"
    );
    let events = handle.snapshot();
    assert!(
        events.iter().any(
            |e| e["type"].as_str() == Some("error") && e.to_string().contains("times in a row")
        ),
        "expected the failure-cap message; got {events:?}"
    );
    // LoopGuard must NOT have fired (content varied every call).
    assert!(
        !events
            .iter()
            .any(|e| e.to_string().contains("no-progress loop")),
        "LoopGuard must not fire when content varies; got {events:?}"
    );
}

/// One turn that calls a NAMED tool (so a run can alternate between distinct
/// tool names turn-to-turn). Mirrors `loop_turn` but parameterizes the tool.
fn named_turn(i: usize, tool: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: format!("call-{i}"),
            name: tool.to_string(),
            input: json!({ "q": "same" }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage {
                input_tokens: 50,
                output_tokens: 10,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

#[tokio::test]
async fn interleaved_failing_tools_converge_via_global_failure_cap() {
    // #160: a failing loop that ALTERNATES tools — tool_a fails, tool_b fails,
    // tool_a fails, tool_b fails… Neither breaker caught this before the fix:
    //   * LoopGuard keys on the full signature, and the tool name alternates
    //     every turn, so its consecutive-signature streak resets each turn.
    //   * The old FailureGuard keyed the streak on the tool NAME and reset the
    //     count to 1 whenever the failing tool changed — so it never passed 1.
    // With the global (tool-agnostic) failure count, consecutive guarded-tool
    // errors accumulate across identities and converge the loop before the turn
    // cap, exiting Continue-able (finish_reason=max_turns, #457).
    let turns: Vec<Vec<LlmEvent>> = (0..30)
        .map(|i| named_turn(i, if i % 2 == 0 { "tool_a" } else { "tool_b" }))
        .collect();
    let provider = Arc::new(MockLlmProvider::with_turns(turns));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new(
        "tool_a",
        "auth failed: missing scope",
        true,
    )));
    registry.register(Box::new(MockTool::new("tool_b", "not found: bad id", true)));

    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut config = test_config();
    config.max_turns = Some(30);

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("keep flailing between tools", "")
        .await
        .expect("run completes (terminated cleanly, not Err)");

    assert!(
        result.turns < 30,
        "the global failure-cap must converge the interleaved failing loop \
         before max_turns; turns = {}",
        result.turns
    );
    assert_eq!(
        result.finish_reason,
        FinishReason::MaxTurns,
        "the failure-cap exit must be Continue-able (max_turns), not a hard error"
    );
    let events = handle.snapshot();
    assert!(
        events.iter().any(
            |e| e["type"].as_str() == Some("error") && e.to_string().contains("times in a row")
        ),
        "expected the failure-cap message; got {events:?}"
    );
    // LoopGuard must NOT have fired — the tool name alternates every turn, so no
    // signature repeats consecutively.
    assert!(
        !events
            .iter()
            .any(|e| e.to_string().contains("no-progress loop")),
        "LoopGuard must not fire when the tool alternates every turn; got {events:?}"
    );
}

/// #475 no-regression: a tool that fails FEWER than the threshold (default 10)
/// consecutive times must NOT trip the failure-cap — each error result still
/// reaches the model and the run proceeds normally. Here 6 failing calls under
/// max_turns=6 stop at the TURN CAP, not the failure-cap, proving a recoverable
/// isError flow is not aborted mid-stream.
#[tokio::test]
async fn sub_threshold_failures_do_not_trip_failure_cap() {
    let turns: Vec<Vec<LlmEvent>> = (0..6).map(loop_turn).collect();
    let provider = Arc::new(MockLlmProvider::with_turns(turns));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new(
        "loop_tool",
        "transient error, retry",
        true, // failing, but only 6 times — below the default cap of 10
    )));

    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;
    let mut config = test_config();
    config.max_turns = Some(6);

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("try it", "").await.expect("run completes");

    assert_eq!(
        result.turns, 6,
        "run must reach the turn cap, not be cut early"
    );
    let events = handle.snapshot();
    // The stop is the max_turns cap, NOT the failure-cap.
    assert!(
        events.iter().any(|e| e["type"].as_str() == Some("info")
            && e.to_string().contains("reached the configured max_turns")),
        "expected the max_turns stop, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| e.to_string().contains("times in a row")),
        "the failure-cap must NOT fire below its threshold; got {events:?}"
    );
}

/// Control: a tool whose result CHANGES every turn (real progress) must NOT
/// trip the breaker — it runs to the natural max_turns cap instead. Guards
/// against the breaker firing on a legitimate iterate-retest cadence.
#[tokio::test]
async fn changing_results_do_not_trip_the_breaker() {
    // Each turn the model calls the same tool, but the tool's output differs,
    // so the signature changes and the streak never accumulates.
    let turns: Vec<Vec<LlmEvent>> = (0..12).map(loop_turn).collect();
    let provider = Arc::new(MockLlmProvider::with_turns(turns));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ChangingTool::default()));

    let config = test_config(); // max_turns = Some(10)
    let sink = Arc::new(TestSink::new());
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = sink;

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("iterate", "").await.expect("run completes");

    // Reached the turn cap, NOT the breaker (12 turns queued, cap 10).
    assert_eq!(
        result.turns, 10,
        "changing results must run to max_turns, not be cut by the breaker"
    );
    let events = handle.snapshot();
    assert!(
        !events
            .iter()
            .any(|e| e["type"].as_str() == Some("error") && e.to_string().contains("no-progress")),
        "the breaker must not fire when each result differs"
    );
}

/// A tool that returns a different result on each call.
#[derive(Default)]
struct ChangingTool {
    calls: std::sync::Mutex<u32>,
}

#[async_trait::async_trait]
impl wcore_tools::Tool for ChangingTool {
    fn name(&self) -> &str {
        "loop_tool"
    }
    fn description(&self) -> &str {
        "Returns a different result each call"
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }
    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Info
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }
    async fn execute(&self, _input: serde_json::Value) -> wcore_types::tool::ToolResult {
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        wcore_types::tool::ToolResult {
            content: format!("progress step {n}"),
            is_error: false,
        }
    }
}

/// A tool that FAILS with a different result on each call — isolates
/// FailureGuard, since LoopGuard's content-keyed signature never accumulates.
#[derive(Default)]
struct FailingChangingTool {
    calls: std::sync::Mutex<u32>,
}

#[async_trait::async_trait]
impl wcore_tools::Tool for FailingChangingTool {
    fn name(&self) -> &str {
        "loop_tool"
    }
    fn description(&self) -> &str {
        "Fails with a different message each call"
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }
    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Info
    }
    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }
    async fn execute(&self, _input: serde_json::Value) -> wcore_types::tool::ToolResult {
        let mut n = self.calls.lock().unwrap();
        *n += 1;
        wcore_types::tool::ToolResult {
            content: format!("distinct failure {n}"),
            is_error: true,
        }
    }
}
