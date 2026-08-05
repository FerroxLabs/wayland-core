//! Engine-level test: one TurnTrace emitted per turn, populated from
//! turn_usage and tool calls. Uses MockLlmProvider + a capture sink to
//! observe the emissions and assert their shape.

mod common;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{MockLlmProvider, MockTool, test_config};

/// Decorator OutputSink that records every emit_trace JSON payload into a
/// shared Vec while delegating everything else to a quiet TerminalSink.
struct CaptureSink {
    inner: Arc<TerminalSink>,
    captured: Arc<Mutex<Vec<Value>>>,
}

impl CaptureSink {
    fn new() -> (Self, Arc<Mutex<Vec<Value>>>) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = Self {
            inner: Arc::new(TerminalSink::new(true)),
            captured: captured.clone(),
        };
        (sink, captured)
    }
}

impl OutputSink for CaptureSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        self.inner.emit_text_delta(text, msg_id);
    }
    fn emit_thinking(&self, text: &str, msg_id: &str) {
        self.inner.emit_thinking(text, msg_id);
    }
    fn emit_tool_call(&self, name: &str, input: &str) {
        self.inner.emit_tool_call(name, input);
    }
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        self.inner.emit_tool_result(name, is_error, content);
    }
    fn emit_stream_start(&self, msg_id: &str) {
        self.inner.emit_stream_start(msg_id);
    }
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input: u64,
        output: u64,
        cache_creation: u64,
        cache_read: u64,
        finish: FinishReason,
    ) {
        self.inner.emit_stream_end(
            msg_id,
            turns,
            input,
            output,
            cache_creation,
            cache_read,
            finish,
        );
    }
    fn emit_error(&self, msg: &str, retryable: bool) {
        self.inner.emit_error(msg, retryable);
    }
    fn emit_info(&self, msg: &str) {
        self.inner.emit_info(msg);
    }
    fn emit_trace(&self, _msg_id: &str, trace_json: &Value) {
        if let Ok(mut g) = self.captured.lock() {
            g.push(trace_json.clone());
        }
    }
}

#[tokio::test]
async fn agent_emits_one_turn_trace_per_turn() {
    // Turn 1: LLM calls Read tool with cache_read = 0 (cold cache).
    // Turn 2: LLM returns final text. cache_read > 0 (warm cache from
    // turn 1's prefix), proving the cache_hit_rate field populates.
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "tu_01".into(),
            name: "mock_tool".into(),
            input: json!({ "path": "/etc/hosts" }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage {
                input_tokens: 1_000,
                output_tokens: 30,
                cache_creation_tokens: 500,
                cache_read_tokens: 0,
            },
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("done".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage {
                input_tokens: 300,
                output_tokens: 5,
                cache_creation_tokens: 0,
                cache_read_tokens: 900, // 75% of 1,200 total input
            },
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let config = test_config(); // Anthropic preset → cache_message_breakpoints = Some(true)
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new(
        "mock_tool",
        "127.0.0.1 localhost",
        false,
    )));

    let (sink, captured) = CaptureSink::new();
    let output: Arc<dyn OutputSink> = Arc::new(sink);

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("read hosts", "m-1").await.expect("engine ok");

    assert_eq!(result.turns, 2);

    let traces = captured.lock().unwrap().clone();
    assert_eq!(traces.len(), 2, "one TurnTrace per turn; got: {traces:#?}");

    // Turn 0: cold cache.
    let t0 = &traces[0];
    assert_eq!(t0["turn"], 0);
    assert_eq!(t0["input_tokens"], 1_000);
    assert_eq!(t0["cache_read"], 0);
    assert_eq!(t0["cache_write"], 500);
    assert_eq!(t0["cache_hit_rate"], 0.0);
    let calls = t0["tool_calls"].as_array().expect("tool_calls array");
    assert_eq!(calls.len(), 1, "turn 0 must capture one tool call");
    assert_eq!(calls[0]["tool_name"], "mock_tool");
    assert_eq!(calls[0]["call_id"], "tu_01");
    assert_eq!(calls[0]["source_product"], "wayland-core");

    // Turn 1: warm cache. cache_hit_rate = 900/(300 + 900) = 0.75.
    let t1 = &traces[1];
    assert_eq!(t1["turn"], 1);
    assert_eq!(t1["cache_read"], 900);
    let hit = t1["cache_hit_rate"].as_f64().expect("cache_hit_rate f64");
    assert!(
        (hit - 0.75).abs() < 1e-9,
        "cache_hit_rate must be cache_read/total_input_tokens; got {hit}"
    );
    assert_eq!(
        t1["tool_calls"].as_array().unwrap().len(),
        0,
        "turn 1 has no tool calls (final text turn)"
    );

    // Source-product tag (S5) on every emitted trace.
    for (i, t) in traces.iter().enumerate() {
        assert_eq!(
            t["source_product"], "wayland-core",
            "trace[{i}] missing source_product tag"
        );
    }
}

/// Spec §4.2 acceptance: a five-turn session against a stub provider that
/// reports realistic cache_read tokens (simulating Anthropic's per-turn
/// caching behaviour) MUST produce TurnTraces with cache_hit_rate > 0.5
/// from turn 2 onwards. If this regresses, downstream waves consume
/// silently-broken cache data.
#[tokio::test]
async fn cache_hit_rate_exceeds_threshold_from_turn_two() {
    // Each turn after the first reports cache_read = 80% of total input,
    // mimicking the steady-state once the system + tools + tail markers
    // are in place. Turn 0 is cold (cache_read = 0). The threshold check
    // is per-turn; the assertion uses the per-turn TurnTrace, not a
    // session aggregate (ExecutionTrace lands in W6).
    fn done_with(uncached_input: u64, cache_read: u64) -> LlmEvent {
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage {
                input_tokens: uncached_input,
                output_tokens: 20,
                cache_creation_tokens: 0,
                cache_read_tokens: cache_read,
            },
        }
    }
    fn text_turn(text: &str, uncached_input: u64, cache_read: u64) -> Vec<LlmEvent> {
        vec![
            LlmEvent::TextDelta(text.into()),
            done_with(uncached_input, cache_read),
        ]
    }

    let turns = vec![
        text_turn("t0", 2_000, 0),   // cold
        text_turn("t1", 420, 1_680), // 80% of 2,100 total
        text_turn("t2", 440, 1_760), // 80% of 2,200 total
        text_turn("t3", 460, 1_840), // 80% of 2,300 total
        text_turn("t4", 480, 1_920), // 80% of 2,400 total
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(turns));

    let (sink, captured) = CaptureSink::new();
    let output: Arc<dyn OutputSink> = Arc::new(sink);

    // MockLlmProvider's "Done" without ToolUse ends the turn; with no
    // follow-up turns the agent loop returns after one provider call. To
    // exercise five turns we drive the engine via five independent run()
    // calls (the MockLlmProvider state-machine pops one Vec<LlmEvent> per
    // stream() invocation, and the engine calls stream() once per turn).
    let config = test_config();
    let registry = ToolRegistry::new();
    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    for i in 0..5 {
        engine
            .run(&format!("u{i}"), &format!("m-{i}"))
            .await
            .expect("engine ok");
    }

    let traces = captured.lock().unwrap().clone();
    assert_eq!(
        traces.len(),
        5,
        "five traces expected; got {}",
        traces.len()
    );

    // Turn 0: cold. cache_hit_rate must be 0.0.
    assert_eq!(traces[0]["cache_hit_rate"], 0.0, "turn 0 must be cold");

    // Turns 1..=4: cache_hit_rate must exceed the §4.2 threshold (0.5).
    for (i, t) in traces.iter().enumerate().skip(1) {
        let hit = t["cache_hit_rate"]
            .as_f64()
            .unwrap_or_else(|| panic!("trace[{i}] cache_hit_rate not f64"));
        assert!(
            hit > 0.5,
            "spec §4.2 acceptance: turn {i} cache_hit_rate = {hit}, expected > 0.5. \
             If this fails, the cache markers are not reaching the provider's response \
             parser — investigate mark_cache_boundaries / anthropic_shared::build_messages \
             / TokenUsage propagation BEFORE merging W1."
        );
    }
}
