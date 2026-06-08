//! W9.1 T3 (T10b): per-turn skill drafting in the engine.
//!
//! Drives the engine through 3 turns of an identical 5-tool sequence
//! via `AgentBootstrap::build_for_test` + `ScriptedProvider`. The
//! `PatternDetector` defaults (min_repeats=3, min_seq_len=5) make this
//! the smallest configuration that triggers `DraftWriter::stage` from
//! the per-turn hook wired in `engine.rs::try_draft_skill_for_turn`.
//!
//! Asserts:
//! 1. With `observability.skills_lifecycle = true`, the engine emits at
//!    least one `TraceEvent` whose `trace.kind == "skill_drafted"` after
//!    the third repeat — exactly the host-visible signal a curator UI
//!    consumes via the `structured_traces` capability opt-in.
//! 2. With the gate OFF (default), the engine emits ZERO `skill_drafted`
//!    payloads even when the same 3-turn pattern is driven. This is the
//!    capability-gate invariant the W9 design contract pins in §5.3.

use serde_json::{Value, json};
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// Build a minimal Config wired for the test fixture. `max_turns = 3`
/// caps the loop after exactly 3 productive turns; without the cap the
/// `ScriptedProvider` would keep replaying the same 5-ToolUse script
/// forever.
fn minimal_config(skills_lifecycle: bool) -> Config {
    let mut cfg = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(3),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    cfg.observability.skills_lifecycle = skills_lifecycle;
    cfg
}

/// Build a one-turn script: 5 identical ToolUse events (different ids so
/// the engine doesn't dedup them) followed by Done{ToolUse}. The
/// `ScriptedProvider` replays this on every `stream()` call, so the
/// engine sees the same 5-tool sequence on each of its 3 turns — exactly
/// what `PatternDetector::default()` is configured to recognise.
///
/// Tools picked: `Grep` and `Glob` exist in the `build_for_test`
/// registry and gracefully return empty results on inputs that match
/// nothing, so the tool executions succeed without filesystem
/// side-effects. Mixing tool names produces a non-trivial signature
/// (not just `[Grep, Grep, Grep, Grep, Grep]`) closer to a realistic
/// detected pattern.
fn five_tool_repeat_script() -> Vec<LlmEvent> {
    let tools = ["Grep", "Glob", "Grep", "Glob", "Grep"];
    let mut events: Vec<LlmEvent> = tools
        .iter()
        .enumerate()
        .map(|(i, name)| LlmEvent::ToolUse {
            id: format!("call-{i}"),
            name: (*name).to_string(),
            input: if *name == "Grep" {
                json!({ "pattern": "no-such-string-xyzzy-w9-1", "path": "." })
            } else {
                json!({ "pattern": "no-such-glob-xyzzy-w9-1-*.none" })
            },
            extra: None,
        })
        .collect();
    events.push(LlmEvent::Done {
        stop_reason: StopReason::ToolUse,
        finish_reason: FinishReason::Stop,
        usage: TokenUsage::default(),
    });
    events
}

/// Count `trace_event` envelopes whose payload carries
/// `kind == "skill_drafted"`. Other `trace_event`s (the per-turn
/// W1 TurnTrace) are ignored.
fn count_skill_drafted_events(events: &[Value]) -> usize {
    events
        .iter()
        .filter(|e| e["type"] == "trace_event" && e["trace"]["kind"] == "skill_drafted")
        .count()
}

#[tokio::test]
async fn engine_emits_skill_drafted_trace_after_three_identical_turns() {
    let (mut engine, _handle) =
        AgentBootstrap::build_for_test(minimal_config(true), five_tool_repeat_script());

    // Drive turns until max_turns trips. `run` loops internally on
    // ToolUse outcomes; max_turns=3 returns MaxTurns after the third
    // turn's TurnTrace + draft hook have fired.
    let _ = engine
        .run_synthetic_turn("trigger pattern detection")
        .await
        .expect("synthetic run should not error");

    let events = engine.captured_protocol_events();
    let drafted = count_skill_drafted_events(&events);
    assert!(
        drafted >= 1,
        "expected at least one skill_drafted TraceEvent after 3 identical turns; \
         got {drafted}. event types seen: {:?}",
        events
            .iter()
            .filter_map(|e| e["type"].as_str())
            .collect::<Vec<_>>()
    );

    // Spot-check the payload shape matches render_skill_drafted_payload's
    // contract (locked by `wcore-skills/tests/skill_drafted_trace_event.rs`).
    let drafted_event = events
        .iter()
        .find(|e| e["type"] == "trace_event" && e["trace"]["kind"] == "skill_drafted")
        .expect("at least one drafted event must exist");
    let payload = &drafted_event["trace"];
    assert_eq!(payload["kind"], "skill_drafted");
    assert!(
        payload["name"]
            .as_str()
            .is_some_and(|s| s.starts_with("auto-")),
        "drafted name should follow auto-<tools> convention; got {:?}",
        payload["name"]
    );
    assert_eq!(
        payload["tool_sequence"],
        json!(["Grep", "Glob", "Grep", "Glob", "Grep"])
    );
    assert!(
        payload["repeat_count"].as_u64().is_some_and(|n| n >= 3),
        "repeat_count should be at least 3 (the min_repeats floor); got {:?}",
        payload["repeat_count"]
    );
}

#[tokio::test]
async fn engine_skips_skill_drafting_when_gate_off() {
    // Same 3-turn pattern, but `skills_lifecycle = false`. No
    // skill_drafted TraceEvent must ever fire — the capability-gate
    // invariant from W9 design contract §5.3.
    let (mut engine, _handle) =
        AgentBootstrap::build_for_test(minimal_config(false), five_tool_repeat_script());

    let _ = engine
        .run_synthetic_turn("trigger pattern detection")
        .await
        .expect("synthetic run should not error");

    let events = engine.captured_protocol_events();
    let drafted = count_skill_drafted_events(&events);
    assert_eq!(
        drafted, 0,
        "skills_lifecycle = false MUST suppress all skill_drafted emissions; \
         got {drafted}"
    );
}

#[tokio::test]
async fn engine_deduplicates_repeated_skill_drafted_emissions() {
    // After the first emission, subsequent turns with the same pattern
    // signature MUST NOT re-emit a `skill_drafted` event. Without this
    // dedup, a 6-turn session would emit 4 duplicate notifications for
    // the same staged draft.
    //
    // Configuration: max_turns=6. Detector matches at turn 2 (3rd repeat
    // present in window). Without dedup, turns 3, 4, 5 would each
    // re-emit. With dedup, only one emission total.
    let mut cfg = minimal_config(true);
    cfg.max_turns = Some(6);
    let (mut engine, _handle) = AgentBootstrap::build_for_test(cfg, five_tool_repeat_script());

    let _ = engine
        .run_synthetic_turn("drive six identical turns")
        .await
        .expect("synthetic run should not error");

    let events = engine.captured_protocol_events();
    let drafted = count_skill_drafted_events(&events);
    assert_eq!(
        drafted, 1,
        "exactly one skill_drafted emission expected across 6 identical turns \
         (signature-dedup invariant); got {drafted}"
    );
}
