//! Host Decoder Contract Test
//!
//! Models the decoder that the Wayland Desktop Electron host MUST
//! implement to remain compatible with future wcore versions. The
//! contract distinguishes THREE outcomes — known event, unknown event
//! type (forward-compat, silent), and malformed JSON (corruption,
//! observable). The production host code at
//! `app/src/process/agent/wcore/index.ts` is the actual consumer; this
//! test asserts the contract is implementable and that today's events
//! plus tomorrow's hypothetical additions satisfy it.
//!
//! This file is NOT a substitute for verifying the production host's
//! conformance. See `docs/json-stream-protocol.md` "Host Decoder
//! Contract" section for the authoritative host-side spec.

use serde_json::{Value, json};
use wcore_protocol::events::{Capabilities, ProtocolEvent};

/// What the host decoder returns for one input line.
///
/// - `Known(value)`: a v0.1.21 event the host knows how to render.
/// - `UnknownType(s)`: a `type` value the host doesn't recognize. Per
///   the contract, the host MUST drop these silently — this is the
///   forward-compatibility path for new wcore variants.
/// - `Malformed(reason)`: the input wasn't decodable as JSON, or had no
///   `type` field, or `type` wasn't a string. Per the contract, the
///   host SHOULD log or count these (rate-limited in production); they
///   indicate protocol corruption / framing bugs, not normal evolution.
#[derive(Debug, PartialEq)]
enum DecodeOutcome {
    Known(Value),
    UnknownType(String),
    Malformed(&'static str),
}

const V0_1_21_KNOWN_TYPES: &[&str] = &[
    "ready",
    "stream_start",
    "text_delta",
    "thinking",
    "tool_request",
    "tool_running",
    "tool_result",
    "tool_cancelled",
    "stream_end",
    "error",
    "info",
    "config_changed",
    "mcp_ready",
    "pong",
];

/// Reference implementation of the host decoder contract.
fn host_decode(line: &str) -> DecodeOutcome {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return DecodeOutcome::Malformed("not valid JSON"),
    };
    let type_val = match v.get("type") {
        Some(t) => t,
        None => return DecodeOutcome::Malformed("missing 'type' field"),
    };
    let type_str = match type_val.as_str() {
        Some(s) => s,
        None => return DecodeOutcome::Malformed("'type' is not a string"),
    };
    if V0_1_21_KNOWN_TYPES.contains(&type_str) {
        DecodeOutcome::Known(v)
    } else {
        DecodeOutcome::UnknownType(type_str.to_string())
    }
}

#[test]
fn host_decodes_known_v0_1_21_variant() {
    let serialized = serde_json::to_string(&ProtocolEvent::Pong).unwrap();
    match host_decode(&serialized) {
        DecodeOutcome::Known(v) => assert_eq!(v["type"], "pong"),
        other => panic!("expected Known(pong), got {other:?}"),
    }
}

#[test]
fn host_decodes_ready_with_default_capabilities() {
    let event = ProtocolEvent::Ready {
        version: "0.1.21".into(),
        session_id: None,
        capabilities: Capabilities::default(),
    };
    let serialized = serde_json::to_string(&event).unwrap();
    match host_decode(&serialized) {
        DecodeOutcome::Known(v) => {
            assert_eq!(v["type"], "ready");
            // W0 flags must be ABSENT in default-off serialization
            assert!(v["capabilities"].get("browser_suite").is_none());
            assert!(v["capabilities"].get("plugins").is_none());
        }
        other => panic!("expected Known(ready), got {other:?}"),
    }
}

#[test]
fn host_drops_unknown_event_type_silently() {
    // Simulates a future wcore emitting a new variant (e.g. tool_chunk from W7).
    let future_event = json!({
        "type": "tool_chunk",
        "msg_id": "m-1",
        "call_id": "c-1",
        "chunk": "partial stdout..."
    });
    match host_decode(&future_event.to_string()) {
        DecodeOutcome::UnknownType(t) => assert_eq!(t, "tool_chunk"),
        other => panic!("expected UnknownType, got {other:?}"),
    }
}

#[test]
fn host_drops_compact_offload_when_capability_unknown_279() {
    let line = json!({
        "type": "compact_offload",
        "msg_id": "m-1",
        "reason": "window_pressure",
        "tokens_freed": 4096,
        "active_window_percent": 41
    })
    .to_string();
    match host_decode(&line) {
        DecodeOutcome::UnknownType(t) => assert_eq!(t, "compact_offload"),
        other => panic!("expected UnknownType, got {other:?}"),
    }
}

#[test]
fn host_tolerates_unknown_fields_on_stream_end_279() {
    let line = json!({
        "type": "stream_end",
        "msg_id": "m-1",
        "finish_reason": "stop",
        "usage": {"input_tokens": 10, "output_tokens": 5, "active_window_percent": 88},
        "agent_run_id": "agent-run-xyz"
    })
    .to_string();
    match host_decode(&line) {
        DecodeOutcome::Known(v) => {
            assert_eq!(v["type"], "stream_end");
            assert_eq!(v["finish_reason"], "stop");
            assert_eq!(v["usage"]["input_tokens"], 10);
        }
        other => panic!("expected Known(stream_end), got {other:?}"),
    }
}

#[test]
fn capability_aware_host_parses_compact_offload_279() {
    fn decode_with_caps(line: &str, known_extra: &[&str]) -> DecodeOutcome {
        let v: Value = serde_json::from_str(line).unwrap();
        let t = v["type"].as_str().unwrap();
        if V0_1_21_KNOWN_TYPES.contains(&t) || known_extra.contains(&t) {
            DecodeOutcome::Known(v)
        } else {
            DecodeOutcome::UnknownType(t.to_string())
        }
    }
    let line = json!({
        "type": "compact_offload",
        "msg_id": "m-1",
        "reason": "window_pressure",
        "tokens_freed": 4096,
        "active_window_percent": 41
    })
    .to_string();
    match decode_with_caps(&line, &["compact_offload"]) {
        DecodeOutcome::Known(v) => {
            assert_eq!(v["reason"], "window_pressure");
            assert_eq!(v["tokens_freed"], 4096);
            assert_eq!(v["active_window_percent"], 41);
        }
        other => panic!("capability-aware host must parse compact_offload, got {other:?}"),
    }
}

/// W8a A.7 — `BudgetExceeded` ships as a host-tolerated additive variant
/// per audit F5: no dedicated capability flag, always-emitted, dropped
/// silently by hosts that don't know the `budget_exceeded` `type`
/// string. Confirms a v0.1.21 host classifies it as `UnknownType` —
/// the W0 forward-compat baseline.
#[test]
fn host_drops_budget_exceeded_silently_per_w8a_a7() {
    let event = ProtocolEvent::BudgetExceeded {
        reason: "max_wall_time".into(),
        observed: "62.0s".into(),
        limit: "60.0s".into(),
    };
    let serialized = serde_json::to_string(&event).unwrap();
    match host_decode(&serialized) {
        DecodeOutcome::UnknownType(t) => assert_eq!(t, "budget_exceeded"),
        other => panic!("expected UnknownType(budget_exceeded), got {other:?}"),
    }
}

/// W8a H.1 — `plugin_event` is gated by the W0 `capabilities.plugins`
/// flag at emission time, but as a wire-shape concern it MUST be
/// dropped silently by a v0.1.21 host that has not opted in. Confirms
/// the variant classifies as `UnknownType` to a v0.1.21-only decoder.
#[test]
fn host_drops_plugin_event_silently_per_w8a_h1() {
    let event = ProtocolEvent::PluginEvent {
        plugin_name: "wayland-ijfw".into(),
        event_type: "memory_capture".into(),
        payload: json!({ "key": "abc" }),
    };
    let serialized = serde_json::to_string(&event).unwrap();
    match host_decode(&serialized) {
        DecodeOutcome::UnknownType(t) => assert_eq!(t, "plugin_event"),
        other => panic!("expected UnknownType(plugin_event), got {other:?}"),
    }
}

#[test]
fn host_drops_multiple_unknown_types_quietly() {
    let lines = [
        r#"{"type": "browser_navigate", "url": "https://example.com"}"#,
        r#"{"type": "computer_use_screenshot", "data_url": "data:..."}"#,
        r#"{"type": "ijfw_memory_event", "key": "x"}"#,
    ];
    for line in lines {
        match host_decode(line) {
            DecodeOutcome::UnknownType(_) => {}
            other => panic!("line {line:?} expected UnknownType, got {other:?}"),
        }
    }
}

#[test]
fn host_distinguishes_malformed_from_unknown() {
    // Each of these is malformed in a different way. The host must
    // recognize them as protocol corruption — distinct from forward-
    // compat unknown types. In production, malformed should be logged
    // or counted (rate-limited); unknown types must be silent.

    assert_eq!(
        host_decode("not even json {{ "),
        DecodeOutcome::Malformed("not valid JSON")
    );
    assert_eq!(host_decode(""), DecodeOutcome::Malformed("not valid JSON"));
    assert_eq!(
        host_decode(r#"["array", "not", "object"]"#),
        DecodeOutcome::Malformed("missing 'type' field")
    );
    assert_eq!(
        host_decode(r#"{"no_type_here": "field"}"#),
        DecodeOutcome::Malformed("missing 'type' field")
    );
    assert_eq!(
        host_decode(r#"{"type": 42}"#),
        DecodeOutcome::Malformed("'type' is not a string")
    );
    assert_eq!(
        host_decode(r#"{"type": null}"#),
        DecodeOutcome::Malformed("'type' is not a string")
    );
}

#[test]
fn host_decoder_handles_noisy_stream_without_panic() {
    // Gemini audit Finding 5: a real stream contains a mix of valid lines,
    // unknown-type lines, and garbage. The decoder loop must survive all
    // three without panicking and must classify each correctly.
    let lines = [
        (r#"{"type": "pong"}"#, "known"),
        (r#"{"type": "tool_chunk", "x": 1}"#, "unknown"),
        ("garbage line, definitely not json", "malformed"),
        (
            r#"{"type": "info", "msg_id": "m-1", "message": "ok"}"#,
            "known",
        ),
        (r#"{"type": null, "broken": true}"#, "malformed"),
        (r#"{"type": "future_thing"}"#, "unknown"),
    ];
    for (line, expected_kind) in lines {
        let outcome = host_decode(line);
        let actual_kind = match outcome {
            DecodeOutcome::Known(_) => "known",
            DecodeOutcome::UnknownType(_) => "unknown",
            DecodeOutcome::Malformed(_) => "malformed",
        };
        assert_eq!(actual_kind, expected_kind, "line {line:?} classified wrong");
    }
}

#[test]
fn host_tolerates_unknown_fields_on_known_variants() {
    // A future wcore might add a new optional field to an existing
    // variant. The host's Value-based decoder ignores fields it doesn't
    // know — they're present in the Value but invisible to the host's
    // typed render logic.
    let future_pong = json!({
        "type": "pong",
        "future_field_we_dont_know": "some_value",
        "another_future_field": 42
    });
    match host_decode(&future_pong.to_string()) {
        DecodeOutcome::Known(v) => {
            assert_eq!(v["type"], "pong");
            assert_eq!(v["future_field_we_dont_know"], "some_value");
        }
        other => panic!("expected Known with extra fields, got {other:?}"),
    }
}

#[test]
fn host_tolerates_unknown_capabilities_flag_on_ready() {
    // Critical W0 contract: a future wcore that has flipped W0 flags on
    // (e.g. plugins=true) emits the corresponding capability key. A host
    // that doesn't know about that key must ignore it without breaking
    // the Ready event's render.
    let future_ready = json!({
        "type": "ready",
        "version": "0.3.0",
        "capabilities": {
            "tool_approval": true, "thinking": true, "effort": false,
            "effort_levels": [], "modes": ["default"],
            "current_mode": "default", "mcp": true,
            "plugins": true, "browser_suite": true,
            "future_capability_we_dont_know": "value"
        }
    });
    match host_decode(&future_ready.to_string()) {
        DecodeOutcome::Known(v) => {
            assert_eq!(v["type"], "ready");
            assert_eq!(v["capabilities"]["tool_approval"], true);
            // Host sees the new flag but isn't obligated to act on it.
            assert_eq!(v["capabilities"]["plugins"], true);
        }
        other => panic!("expected Known with extra capabilities, got {other:?}"),
    }
}

// --- W6 F7 SessionCost forward-compat ---

#[test]
fn host_drops_session_cost_when_not_in_v0_1_21_known_types() {
    // session_cost is a W6 variant. The v0.1.21 reference decoder MUST
    // treat it as UnknownType — the W0 forward-compat contract. Hosts
    // that have opted into capabilities.cost_attribution upgrade their
    // V0_1_21_KNOWN_TYPES list; this test is the floor, not the ceiling.
    let line = r#"{"type":"session_cost","session_id":"s","total_cost_usd":0.5,"per_turn":[]}"#;
    match host_decode(line) {
        DecodeOutcome::UnknownType(t) => assert_eq!(t, "session_cost"),
        other => panic!("expected UnknownType(session_cost), got {other:?}"),
    }
}

#[test]
fn engine_emits_session_cost_in_v6_known_type_namespace() {
    // Round-trip the typed variant to JSON and confirm the `type` tag
    // matches the W6 contract literal "session_cost" (snake_case).
    use wcore_protocol::events::TurnCost;
    let event = ProtocolEvent::SessionCost {
        session_id: "sess-x".into(),
        total_cost_usd: 0.0,
        per_turn: vec![TurnCost {
            turn: 0,
            model: "m".into(),
            provider: "anthropic".into(),
            cost_usd: 0.0,
        }],
    };
    let serialized = serde_json::to_string(&event).unwrap();
    match host_decode(&serialized) {
        DecodeOutcome::UnknownType(t) => assert_eq!(t, "session_cost"),
        other => panic!("expected UnknownType(session_cost), got {other:?}"),
    }
}

// =====================================================================
// W7 opt-in known-types: parallel decoder for hosts that have flipped
// the `streaming_tools` / `sub_agent_traces` / `hitl_suspend` capability
// acknowledgement. Adding to a separate const + decoder rather than
// mutating `V0_1_21_KNOWN_TYPES` keeps the W0 forward-compat invariant
// independently testable.
// =====================================================================

/// W7 opt-in event types. A host that has flipped `streaming_tools`,
/// `sub_agent_traces`, or `hitl_suspend` capability acknowledgement
/// in its UI adds the relevant strings to its known set. The default
/// decoder (above) treats them as UnknownType — which is the W0
/// forward-compat path. This const + decoder model the opted-in
/// variant.
const W7_OPTED_IN_KNOWN_TYPES: &[&str] = &[
    "sub_agent_event",
    "tool_chunk",
    "approval_required",
    "suspend",
    "approval_resume",
    "provider_circuit_event",
];

fn host_decode_w7(line: &str) -> DecodeOutcome {
    let outcome = host_decode(line);
    if let DecodeOutcome::UnknownType(t) = &outcome
        && W7_OPTED_IN_KNOWN_TYPES.contains(&t.as_str())
        && let Ok(v) = serde_json::from_str::<Value>(line)
    {
        return DecodeOutcome::Known(v);
    }
    outcome
}

#[test]
fn host_drops_w7_event_types_silently_under_v0_1_21_decoder() {
    // The W0 baseline decoder MUST drop W7 variants silently. This is
    // the forward-compat path that lets W7 binaries talk to an
    // unupdated host.
    let lines = [
        r#"{"type": "sub_agent_event", "parent_call_id": "c", "agent_name": "r", "inner": {}}"#,
        r#"{"type": "tool_chunk", "msg_id": "m", "call_id": "c", "tool_name": "Bash", "chunk": "out"}"#,
        r#"{"type": "approval_required", "call_id": "c", "resume_token": "t", "reason": "r", "context": ""}"#,
        r#"{"type": "suspend", "reason": "r", "resume_token": "t"}"#,
        r#"{"type": "approval_resume", "resume_token": "t", "approved": true}"#,
        r#"{"type": "provider_circuit_event", "primary": "anthropic", "state": "open"}"#,
    ];
    for line in lines {
        match host_decode(line) {
            DecodeOutcome::UnknownType(_) => {}
            other => panic!("line {line:?} expected UnknownType, got {other:?}"),
        }
    }
}

#[test]
fn host_decodes_w7_event_types_under_opted_in_decoder() {
    // A host that has opted in to W7 capabilities adds the new type
    // strings to its known set. The W7 decoder treats them as Known.
    let lines = [
        (
            r#"{"type": "sub_agent_event", "parent_call_id": "c", "agent_name": "r", "inner": {}}"#,
            "sub_agent_event",
        ),
        (
            r#"{"type": "tool_chunk", "msg_id": "m", "call_id": "c", "tool_name": "Bash", "chunk": "out"}"#,
            "tool_chunk",
        ),
        (
            r#"{"type": "approval_required", "call_id": "c", "resume_token": "t", "reason": "r", "context": ""}"#,
            "approval_required",
        ),
        (
            r#"{"type": "suspend", "reason": "r", "resume_token": "t"}"#,
            "suspend",
        ),
        (
            r#"{"type": "approval_resume", "resume_token": "t", "approved": true}"#,
            "approval_resume",
        ),
        (
            r#"{"type": "provider_circuit_event", "primary": "anthropic", "state": "open"}"#,
            "provider_circuit_event",
        ),
    ];
    for (line, expected_type) in lines {
        match host_decode_w7(line) {
            DecodeOutcome::Known(v) => assert_eq!(v["type"], expected_type),
            other => panic!("line {line:?} expected Known({expected_type}), got {other:?}"),
        }
    }
}

#[test]
fn host_decoder_still_distinguishes_malformed_with_w7_known_types() {
    // The opted-in decoder still classifies garbage as Malformed —
    // adding to the known set must not change the Malformed path.
    assert_eq!(
        host_decode_w7("garbage"),
        DecodeOutcome::Malformed("not valid JSON")
    );
    assert_eq!(
        host_decode_w7(r#"{"no_type": 1}"#),
        DecodeOutcome::Malformed("missing 'type' field")
    );
}

// =====================================================================
// W9 / W9.1 opt-in known-type: `trace_event` is the W1 envelope carrying
// per-turn TurnTrace JSON; W9 introduced a second payload shape that
// rides inside it — `trace.kind = "skill_drafted"` (emitted by the per-
// turn skill-drafting flow added in W9.1 T3). Hosts that have flipped
// `capabilities.structured_traces` add `trace_event` to their known set;
// hosts that haven't drop it silently per the W0 forward-compat baseline.
//
// Tests in this block lock in BOTH directions:
//   - baseline decoder (no opt-in) drops `trace_event` silently
//   - structured-traces-opted-in decoder treats `trace_event` as Known
//     for the W1 turn-summary payload AND the W9.1 `skill_drafted`
//     payload
// =====================================================================

const STRUCTURED_TRACES_OPTED_IN_KNOWN_TYPES: &[&str] = &["trace_event"];

fn host_decode_structured_traces(line: &str) -> DecodeOutcome {
    let outcome = host_decode(line);
    if let DecodeOutcome::UnknownType(t) = &outcome
        && STRUCTURED_TRACES_OPTED_IN_KNOWN_TYPES.contains(&t.as_str())
        && let Ok(v) = serde_json::from_str::<Value>(line)
    {
        return DecodeOutcome::Known(v);
    }
    outcome
}

#[test]
fn host_drops_trace_event_silently_under_v0_1_21_decoder() {
    // The W0 baseline decoder MUST drop `trace_event` silently — both
    // the W1 turn-summary shape and the W9.1 `skill_drafted` payload
    // shape ride inside the same `trace_event` envelope.
    let lines = [
        r#"{"type": "trace_event", "msg_id": "m-1", "trace": {"turn": 0, "model": "x", "provider": "anthropic", "input_tokens": 1, "output_tokens": 1, "cache_read": 0, "cache_write": 0, "cache_hit_rate": 0.0, "cost_usd": 0.0, "tool_calls": [], "hook_actions": [], "source_product": "wayland-core"}}"#,
        r#"{"type": "trace_event", "msg_id": "m-2", "trace": {"kind": "skill_drafted", "name": "auto-grep-read-edit-bash-bash", "description": "x", "tool_sequence": ["Grep","Read","Edit","Bash","Bash"], "repeat_count": 3}}"#,
    ];
    for line in lines {
        match host_decode(line) {
            DecodeOutcome::UnknownType(t) => assert_eq!(t, "trace_event"),
            other => panic!("line {line:?} expected UnknownType(trace_event), got {other:?}"),
        }
    }
}

#[test]
fn host_decodes_trace_event_under_structured_traces_opt_in() {
    // Hosts that have flipped `capabilities.structured_traces` add
    // `trace_event` to their known set. The opted-in decoder treats
    // both the W1 turn-summary shape and the W9.1 `skill_drafted`
    // payload as Known.
    let lines = [
        r#"{"type": "trace_event", "msg_id": "m-1", "trace": {"turn": 0, "model": "x", "provider": "anthropic", "input_tokens": 1, "output_tokens": 1, "cache_read": 0, "cache_write": 0, "cache_hit_rate": 0.0, "cost_usd": 0.0, "tool_calls": [], "hook_actions": [], "source_product": "wayland-core"}}"#,
        r#"{"type": "trace_event", "msg_id": "m-2", "trace": {"kind": "skill_drafted", "name": "auto-grep-read-edit-bash-bash", "description": "x", "tool_sequence": ["Grep","Read","Edit","Bash","Bash"], "repeat_count": 3}}"#,
    ];
    for line in lines {
        match host_decode_structured_traces(line) {
            DecodeOutcome::Known(v) => assert_eq!(v["type"], "trace_event"),
            other => panic!("line {line:?} expected Known(trace_event), got {other:?}"),
        }
    }
}

#[test]
fn host_decoder_still_distinguishes_malformed_with_structured_traces_known_types() {
    // The opted-in decoder still classifies garbage as Malformed —
    // adding to the known set must not change the Malformed path.
    assert_eq!(
        host_decode_structured_traces("garbage"),
        DecodeOutcome::Malformed("not valid JSON")
    );
    assert_eq!(
        host_decode_structured_traces(r#"{"no_type": 1}"#),
        DecodeOutcome::Malformed("missing 'type' field")
    );
}

// =====================================================================
// W10B opt-in known-type: `evolution_event` is the GEPA per-child event
// emitted by `wcore-cli evolve`. Hosts that have flipped
// `capabilities.gepa_enabled` add `evolution_event` to their known set;
// hosts that haven't drop it silently per the W0 forward-compat baseline.
//
// Note: `gepa_enabled` is a separate capability flag from
// `structured_traces` — F6 audit fix in W10B rev-2 split the W1 turn-
// trace family from the W10B per-child evolution family so hosts can
// opt in independently. A host wanting only W1 turn traces is NOT
// forced to accept thousands of W10B events per `evolve` run.
// =====================================================================

const GEPA_ENABLED_OPTED_IN_KNOWN_TYPES: &[&str] = &["evolution_event"];

fn host_decode_gepa_enabled(line: &str) -> DecodeOutcome {
    let outcome = host_decode(line);
    if let DecodeOutcome::UnknownType(t) = &outcome
        && GEPA_ENABLED_OPTED_IN_KNOWN_TYPES.contains(&t.as_str())
        && let Ok(v) = serde_json::from_str::<Value>(line)
    {
        return DecodeOutcome::Known(v);
    }
    outcome
}

#[test]
fn host_drops_evolution_event_silently_under_v0_1_21_decoder() {
    // W0 baseline decoder MUST drop `evolution_event` silently — hosts that
    // haven't advertised `gepa_enabled` MUST NOT crash on this variant.
    let line = r#"{"type": "evolution_event", "run_id": "run-001", "generation": 2, "parent_id": "skill-refactor-imports", "child_id": "run-001/2/3", "mutation_kind": "Reorder", "score": 0.83, "retained": true}"#;
    match host_decode(line) {
        DecodeOutcome::UnknownType(t) => assert_eq!(t, "evolution_event"),
        other => panic!("line {line:?} expected UnknownType(evolution_event), got {other:?}"),
    }
}

#[test]
fn host_decodes_evolution_event_under_gepa_enabled_opt_in() {
    // Hosts that have flipped `capabilities.gepa_enabled` add
    // `evolution_event` to their known set. The opted-in decoder treats
    // it as Known.
    let line = r#"{"type": "evolution_event", "run_id": "run-001", "generation": 2, "parent_id": "skill-refactor-imports", "child_id": "run-001/2/3", "mutation_kind": "Reorder", "score": 0.83, "retained": true}"#;
    match host_decode_gepa_enabled(line) {
        DecodeOutcome::Known(v) => assert_eq!(v["type"], "evolution_event"),
        other => panic!("line {line:?} expected Known(evolution_event), got {other:?}"),
    }
}

#[test]
fn host_decoder_still_distinguishes_malformed_with_gepa_enabled_known_types() {
    // The opted-in decoder still classifies garbage as Malformed —
    // adding to the known set must not change the Malformed path.
    assert_eq!(
        host_decode_gepa_enabled("garbage"),
        DecodeOutcome::Malformed("not valid JSON")
    );
    assert_eq!(
        host_decode_gepa_enabled(r#"{"no_type": 1}"#),
        DecodeOutcome::Malformed("missing 'type' field")
    );
}

#[test]
fn structured_traces_and_gepa_enabled_are_independent_opt_ins() {
    // F6 audit fix: a host advertising only `gepa_enabled` must NOT receive
    // `trace_event` as Known; a host advertising only `structured_traces`
    // must NOT receive `evolution_event` as Known. Each event family gets
    // its own opt-in.
    let trace_line = r#"{"type": "trace_event", "msg_id": "m-1", "trace": {"kind": "skill_drafted", "name": "x", "description": "y", "tool_sequence": [], "repeat_count": 0}}"#;
    let evo_line = r#"{"type": "evolution_event", "run_id": "r", "generation": 0, "parent_id": "p", "child_id": "c", "mutation_kind": "Reorder", "score": 0.5, "retained": false}"#;

    // gepa-only: trace_event dropped, evolution_event known
    match host_decode_gepa_enabled(trace_line) {
        DecodeOutcome::UnknownType(t) => assert_eq!(t, "trace_event"),
        other => panic!("gepa-only must drop trace_event, got {other:?}"),
    }
    match host_decode_gepa_enabled(evo_line) {
        DecodeOutcome::Known(v) => assert_eq!(v["type"], "evolution_event"),
        other => panic!("gepa-only must know evolution_event, got {other:?}"),
    }

    // structured-traces-only: evolution_event dropped, trace_event known
    match host_decode_structured_traces(evo_line) {
        DecodeOutcome::UnknownType(t) => assert_eq!(t, "evolution_event"),
        other => panic!("structured-only must drop evolution_event, got {other:?}"),
    }
    match host_decode_structured_traces(trace_line) {
        DecodeOutcome::Known(v) => assert_eq!(v["type"], "trace_event"),
        other => panic!("structured-only must know trace_event, got {other:?}"),
    }
}
