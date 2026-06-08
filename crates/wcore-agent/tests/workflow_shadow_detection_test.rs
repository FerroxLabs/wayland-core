//! Dynamic Workflows B4 — shadow-mode workflow-detection emission.
//!
//! B3 added a telemetry-only `WorkflowCandidate` signal at the engine's
//! intent-telemetry seam, gated behind
//! `observability.workflow_detection_enabled` (default off). B4 upgrades the
//! debug-only `tracing` line into a structured, aggregatable
//! `WorkflowDetectionRecord` emitted through the engine's existing
//! `OutputSink::emit_trace` channel (the same channel `TurnTrace` flows
//! through). It is shadow mode: it records what the Detected tier WOULD have
//! proposed without ever prompting the user or touching routing.
//!
//! The seam sits on the tool-bearing turn path (it runs after the
//! `tool_calls.is_empty()` early-return), so these tests script a turn that
//! issues one tool call (reaching the seam) followed by a clean text turn that
//! ends the run. The captured protocol event stream is then inspected for the
//! structured trace.
//!
//! Invariants pinned here:
//!   1. flag ON + candidate input  → exactly one `workflow_detection` record,
//!      carrying the candidate's confidence/rationale and a truncated excerpt.
//!   2. flag OFF (default)         → no `workflow_detection` record at all.
//!   3. routing/output is identical whether the flag is on or off (the seam
//!      is a pure side-channel — it cannot perturb the turn).
//!   4. the record's `task_excerpt` never exceeds the documented byte bound.

mod common;

use std::sync::Arc;

use common::{MockLlmProvider, MockTool};
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::test_utils::{TestSink, TestSinkHandle};
use wcore_config::config::{Config, ProviderType};
use wcore_observability::trace::{TASK_EXCERPT_MAX, summarize_workflow_detection};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// A task whose phrasing trips multiple strong workflow signals
/// ("every file", "across all", "in parallel").
const CANDIDATE_INPUT: &str =
    "Audit every file across all crates in parallel and report the findings comprehensively";

fn config_with_detection(enabled: bool) -> Config {
    let mut cfg = Config {
        provider: ProviderType::Anthropic,
        api_key: "sk-test".into(),
        model: "test-model".into(),
        max_tokens: 1024,
        max_turns: Some(4),
        ..Default::default()
    };
    cfg.tools.auto_approve = true;
    cfg.observability.workflow_detection_enabled = enabled;
    cfg
}

/// Turn 1: one tool call (drives the engine past the `tool_calls.is_empty()`
/// early-return so the B3/B4 seam fires). Turn 2: a clean text Done that ends
/// the run.
fn tool_then_text_turns() -> Vec<Vec<LlmEvent>> {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".into(),
            name: "mock_tool".into(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage::default(),
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("done".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage::default(),
        },
    ];
    vec![turn1, turn2]
}

fn build_engine(enabled: bool) -> (AgentEngine, TestSinkHandle) {
    let sink = TestSink::new();
    let handle = sink.handle();
    let output: Arc<dyn OutputSink> = Arc::new(sink);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "result", false)));

    let provider = Arc::new(MockLlmProvider::with_turns(tool_then_text_turns()));
    let engine =
        AgentEngine::new_with_provider(provider, config_with_detection(enabled), registry, output);
    (engine, handle)
}

/// Pull every `workflow_detection` trace payload out of the captured event
/// stream. The protocol wire form is
/// `{"type":"trace_event","msg_id":...,"trace":{...}}`; the shadow record is
/// the inner `trace` object whose `kind == "workflow_detection"`.
fn shadow_records(events: &[serde_json::Value]) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|e| e["type"] == "trace_event")
        .map(|e| e["trace"].clone())
        .filter(|t| t["kind"] == "workflow_detection")
        .collect()
}

#[tokio::test]
async fn flag_on_emits_one_shadow_record_for_candidate_turn() {
    let (mut engine, handle) = build_engine(true);

    engine
        .run(CANDIDATE_INPUT, "msg-1")
        .await
        .expect("run should succeed");

    let events = handle.snapshot();
    let records = shadow_records(&events);

    assert_eq!(
        records.len(),
        1,
        "flag on + candidate input must emit exactly one shadow record; events: {events:?}"
    );

    let rec = &records[0];
    assert_eq!(rec["kind"], "workflow_detection");
    assert!(
        rec["confidence"].as_f64().unwrap() > 0.0,
        "confidence carried from the candidate"
    );
    assert!(
        rec["rationale"]
            .as_str()
            .unwrap()
            .contains("workflow signals"),
        "rationale carried from the candidate"
    );
    assert!(!rec["ts"].as_str().unwrap().is_empty(), "ts stamped");
    // The excerpt is a short prefix, never the full prompt.
    let excerpt = rec["task_excerpt"].as_str().unwrap();
    assert!(
        excerpt.len() <= TASK_EXCERPT_MAX,
        "excerpt {} bytes exceeds bound {TASK_EXCERPT_MAX}",
        excerpt.len()
    );
    assert!(
        CANDIDATE_INPUT.starts_with(excerpt),
        "excerpt must be a prefix of the task"
    );

    // The operator-review helper should count this one record.
    let summary = summarize_workflow_detection(&records);
    assert_eq!(summary.count, 1);
    assert!(summary.mean_confidence > 0.0);
}

#[tokio::test]
async fn flag_off_emits_no_shadow_record() {
    let (mut engine, handle) = build_engine(false);

    engine
        .run(CANDIDATE_INPUT, "msg-1")
        .await
        .expect("run should succeed");

    let events = handle.snapshot();
    let records = shadow_records(&events);
    assert!(
        records.is_empty(),
        "default-off config must emit zero shadow records; got {records:?}"
    );
}

#[tokio::test]
async fn flag_on_non_candidate_turn_emits_nothing() {
    // An ordinary single-task turn must NOT trip the heuristic even with the
    // flag on — shadow mode only records genuine candidates.
    let (mut engine, handle) = build_engine(true);

    engine
        .run("fix the typo in README line 12", "msg-1")
        .await
        .expect("run should succeed");

    let events = handle.snapshot();
    assert!(
        shadow_records(&events).is_empty(),
        "ordinary task must not produce a shadow record; events: {events:?}"
    );
}

#[tokio::test]
async fn shadow_emission_does_not_change_turn_output() {
    // The seam is a pure side-channel: the result the engine produces for the
    // same scripted turns must be identical with the flag on vs off. This pins
    // the B3/B4 invariant that detection cannot perturb routing or the turn.
    let (mut on_engine, _h1) = build_engine(true);
    let on = on_engine
        .run(CANDIDATE_INPUT, "msg-1")
        .await
        .expect("on run");

    let (mut off_engine, _h2) = build_engine(false);
    let off = off_engine
        .run(CANDIDATE_INPUT, "msg-1")
        .await
        .expect("off run");

    assert_eq!(
        on.text, off.text,
        "shadow detection must not alter the run's output text"
    );
    assert_eq!(
        on.turns, off.turns,
        "shadow detection must not alter the turn count"
    );
}

#[tokio::test]
async fn shadow_record_excerpt_is_truncated_for_long_prompt() {
    // A very long candidate prompt must have its excerpt clipped to the bound
    // — we never log the whole prompt in the shadow record.
    let long_prompt = "audit every file ".repeat(50); // >> TASK_EXCERPT_MAX bytes
    let (mut engine, handle) = build_engine(true);

    engine
        .run(&long_prompt, "msg-1")
        .await
        .expect("run should succeed");

    let events = handle.snapshot();
    let records = shadow_records(&events);
    assert_eq!(records.len(), 1, "candidate must fire");
    let excerpt = records[0]["task_excerpt"].as_str().unwrap();
    assert_eq!(
        excerpt.len(),
        TASK_EXCERPT_MAX,
        "long prompt must clip to the byte bound exactly"
    );
}
