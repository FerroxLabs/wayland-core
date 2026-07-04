//! Loop-convergence E2E for the engine-side runaway breaker.
//!
//! A model that repeats the SAME tool call every turn, against a tool that
//! returns the SAME failing result, must be stopped by the breaker WELL BEFORE
//! `max_turns` — proving a no-progress loop converges instead of burning tokens
//! to the turn cap (the "8.5M tokens in 2 hours" class of report).

mod common;

use std::sync::Arc;

use serde_json::json;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::test_utils::TestSink;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{MockLlmProvider, MockTool, test_config};

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
