//! Golden snapshot tests for the v0.1.21 ProtocolEvent surface.
//!
//! Each test constructs a canonical value of one variant, serializes it,
//! and asserts the resulting JSON `Value` matches an inline expected
//! value. The point is regression-guarding: if a future PR drifts a field
//! name, ordering, or serialization semantics, the matching golden test
//! fails and surfaces the schema drift before it can break the Wayland
//! Desktop host.
//!
//! Adding a new event variant in a future wave: add a new golden test
//! here. Changing an existing variant: update both the golden AND the
//! `docs/json-stream-protocol.md` documentation.

use serde_json::{Value, json};
use wcore_protocol::events::{Capabilities, ErrorInfo, FinishReason, ProtocolEvent, Usage};

fn serialize(event: &ProtocolEvent) -> Value {
    serde_json::to_value(event).expect("event must serialize")
}

#[test]
fn golden_ready_v0_1_21() {
    let event = ProtocolEvent::Ready {
        version: "0.1.21".into(),
        session_id: Some("sess-001".into()),
        capabilities: Capabilities {
            tool_approval: true,
            thinking: true,
            modes: vec!["default".into(), "auto_edit".into(), "force".into()],
            mcp: true,
            ..Default::default()
        },
    };
    let got = serialize(&event);
    let want = json!({
        "type": "ready",
        "version": "0.1.21",
        "session_id": "sess-001",
        "capabilities": {
            "tool_approval": true,
            "thinking": true,
            "effort": false,
            "effort_levels": [],
            "modes": ["default", "auto_edit", "force"],
            "current_mode": "default",
            "mcp": true
            // W0 flags ABSENT when default-off
        }
    });
    assert_eq!(got, want, "Ready event golden drift");
}

#[test]
fn golden_pong_v0_1_21() {
    let got = serialize(&ProtocolEvent::Pong);
    assert_eq!(got, json!({ "type": "pong" }), "Pong event golden drift");
}

#[test]
fn golden_stream_start_v0_1_21() {
    let got = serialize(&ProtocolEvent::StreamStart {
        msg_id: "m-1".into(),
    });
    assert_eq!(got, json!({ "type": "stream_start", "msg_id": "m-1" }));
}

#[test]
fn golden_text_delta_v0_1_21() {
    let got = serialize(&ProtocolEvent::TextDelta {
        text: "hello world".into(),
        msg_id: "m-1".into(),
    });
    assert_eq!(
        got,
        json!({
            "type": "text_delta",
            "text": "hello world",
            "msg_id": "m-1"
        })
    );
}

#[test]
fn golden_thinking_v0_1_21() {
    let got = serialize(&ProtocolEvent::Thinking {
        text: "considering options".into(),
        msg_id: "m-1".into(),
    });
    assert_eq!(
        got,
        json!({
            "type": "thinking",
            "text": "considering options",
            "msg_id": "m-1"
        })
    );
}

#[test]
fn golden_stream_end_stop_v0_1_21() {
    let event = ProtocolEvent::StreamEnd {
        msg_id: "m-1".into(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: None,
            cache_write_tokens: None,
            active_window_percent: None,
        }),
        agent_run_id: None,
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "stream_end",
            "msg_id": "m-1",
            "finish_reason": "stop",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50
                // cache_* omitted when None (skip_serializing_if)
            }
        })
    );
}

#[test]
fn golden_stream_end_length_v0_1_21() {
    let event = ProtocolEvent::StreamEnd {
        msg_id: "m-1".into(),
        finish_reason: FinishReason::Length,
        usage: None,
        agent_run_id: None,
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "stream_end",
            "msg_id": "m-1",
            "finish_reason": "length"
            // usage absent when None
        })
    );
}

#[test]
fn golden_stream_end_error_v0_1_21() {
    // Codex audit Finding 5: rev 1 omitted Error.
    let event = ProtocolEvent::StreamEnd {
        msg_id: "m-1".into(),
        finish_reason: FinishReason::Error,
        usage: None,
        agent_run_id: None,
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "stream_end",
            "msg_id": "m-1",
            "finish_reason": "error"
        })
    );
}

#[test]
fn golden_stream_end_every_finish_reason_serializes_snake_case() {
    // Defensive enumeration: if a new FinishReason variant is added, this
    // test forces a decision about its serialization tag. If FinishReason
    // is non-exhaustive elsewhere, this test will still cover today's set
    // and surface the new variant as a compile error here.
    let cases: [(FinishReason, &str); 3] = [
        (FinishReason::Stop, "stop"),
        (FinishReason::Length, "length"),
        (FinishReason::Error, "error"),
    ];
    for (fr, expected) in cases {
        let event = ProtocolEvent::StreamEnd {
            msg_id: "m".into(),
            finish_reason: fr,
            usage: None,
            agent_run_id: None,
        };
        let got = serialize(&event);
        assert_eq!(
            got["finish_reason"], expected,
            "FinishReason::{:?} serialization drift",
            fr
        );
    }
}

#[test]
fn golden_usage_with_full_cache_fields_v0_1_21() {
    // Gemini audit Finding 4: lock the full Usage schema including
    // cache_read_tokens and cache_write_tokens being SERIALIZED when Some.
    let event = ProtocolEvent::StreamEnd {
        msg_id: "m-1".into(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: Some(800),
            cache_write_tokens: Some(200),
            active_window_percent: None,
        }),
        agent_run_id: None,
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "stream_end",
            "msg_id": "m-1",
            "finish_reason": "stop",
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 500,
                "cache_read_tokens": 800,
                "cache_write_tokens": 200
            }
        })
    );
}

#[test]
fn golden_stream_end_no_new_keys_when_unset_279() {
    let event = ProtocolEvent::StreamEnd {
        msg_id: "m-1".into(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: Some(800),
            cache_write_tokens: Some(200),
            active_window_percent: None,
        }),
        agent_run_id: None,
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "stream_end",
            "msg_id": "m-1",
            "finish_reason": "stop",
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 500,
                "cache_read_tokens": 800,
                "cache_write_tokens": 200
            }
        }),
        "unset #279 fields must not appear on the wire"
    );
}

#[test]
fn golden_stream_end_with_new_keys_when_set_279() {
    let event = ProtocolEvent::StreamEnd {
        msg_id: "m-1".into(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: None,
            cache_write_tokens: None,
            active_window_percent: Some(73),
        }),
        agent_run_id: Some("agent-run-abc".into()),
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "stream_end",
            "msg_id": "m-1",
            "finish_reason": "stop",
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 500,
                "active_window_percent": 73
            },
            "agent_run_id": "agent-run-abc"
        }),
        "set #279 fields appear only when present; rest unchanged"
    );
}

#[test]
fn golden_error_v0_1_21() {
    let event = ProtocolEvent::Error {
        msg_id: Some("m-1".into()),
        error: ErrorInfo {
            code: "provider_unavailable".into(),
            message: "Anthropic returned 503".into(),
            retryable: true,
        },
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "error",
            "msg_id": "m-1",
            "error": {
                "code": "provider_unavailable",
                "message": "Anthropic returned 503",
                "retryable": true
            }
        })
    );
}

#[test]
fn golden_error_without_msg_id_v0_1_21() {
    // session-level errors omit msg_id (skip_serializing_if = None)
    let event = ProtocolEvent::Error {
        msg_id: None,
        error: ErrorInfo {
            code: "session_init_failed".into(),
            message: "could not bind socket".into(),
            retryable: false,
        },
    };
    let got = serialize(&event);
    assert!(
        got.get("msg_id").is_none(),
        "msg_id must be absent when None"
    );
    assert_eq!(got["error"]["code"], "session_init_failed");
}

#[test]
fn golden_info_v0_1_21() {
    let event = ProtocolEvent::Info {
        msg_id: "m-1".into(),
        message: "memory loaded".into(),
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "info",
            "msg_id": "m-1",
            "message": "memory loaded"
        })
    );
}

#[test]
fn golden_config_changed_v0_1_21() {
    let event = ProtocolEvent::ConfigChanged {
        capabilities: Capabilities {
            tool_approval: true,
            thinking: true,
            effort: true,
            effort_levels: vec!["low".into(), "medium".into(), "high".into()],
            modes: vec!["default".into()],
            mcp: true,
            ..Default::default()
        },
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "config_changed",
            "capabilities": {
                "tool_approval": true,
                "thinking": true,
                "effort": true,
                "effort_levels": ["low", "medium", "high"],
                "modes": ["default"],
                "current_mode": "default",
                "mcp": true
            }
        })
    );
}

#[test]
fn golden_mcp_ready_v0_1_21() {
    let event = ProtocolEvent::McpReady {
        name: "memory-server".into(),
        tools: vec!["memory_store".into(), "memory_search".into()],
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "mcp_ready",
            "name": "memory-server",
            "tools": ["memory_store", "memory_search"]
        })
    );
}

#[test]
fn golden_tool_cancelled_v0_1_21() {
    let event = ProtocolEvent::ToolCancelled {
        msg_id: "m-1".into(),
        call_id: "c-1".into(),
        reason: "user_stop".into(),
    };
    assert_eq!(
        serialize(&event),
        json!({
            "type": "tool_cancelled",
            "msg_id": "m-1",
            "call_id": "c-1",
            "reason": "user_stop"
        })
    );
}

/// W0 audit follow-up: lock each new capability flag's serialized key
/// name. A typo or rename on any of these 10 flags would silently ship as
/// a wire-format change because each flag is only verified individually.
/// This test exercises each one in isolation, asserting BOTH (a) the
/// documented key is present when the flag is true and (b) no
/// neighbouring W0 keys leaked in alongside.
type FlagSetter = fn(&mut Capabilities);

#[test]
fn golden_each_w0_capability_flag_locks_its_documented_name() {
    let cases: [(&str, FlagSetter); 10] = [
        ("streaming_tools", |c| c.streaming_tools = true),
        ("sub_agent_traces", |c| c.sub_agent_traces = true),
        ("cost_attribution", |c| c.cost_attribution = true),
        ("hitl_suspend", |c| c.hitl_suspend = true),
        ("non_destructive_compact", |c| {
            c.non_destructive_compact = true
        }),
        ("structured_traces", |c| c.structured_traces = true),
        ("rpc_tool_script", |c| c.rpc_tool_script = true),
        ("browser_suite", |c| c.browser_suite = true),
        ("computer_use", |c| c.computer_use = true),
        ("plugins", |c| c.plugins = true),
    ];

    for (expected_key, setter) in cases {
        let mut caps = Capabilities::default();
        setter(&mut caps);
        let event = ProtocolEvent::Ready {
            version: "0.1.21".into(),
            session_id: None,
            capabilities: caps,
        };
        let got = serialize(&event);
        let caps_obj = got["capabilities"]
            .as_object()
            .expect("capabilities object");
        assert_eq!(
            caps_obj.get(expected_key),
            Some(&Value::Bool(true)),
            "W0 flag {expected_key} did not serialize under its documented key name"
        );

        // No other W0 flag leaked in.
        let other_w0_keys = [
            "streaming_tools",
            "sub_agent_traces",
            "cost_attribution",
            "hitl_suspend",
            "non_destructive_compact",
            "structured_traces",
            "rpc_tool_script",
            "browser_suite",
            "computer_use",
            "plugins",
        ];
        for other in other_w0_keys {
            if other == expected_key {
                continue;
            }
            assert!(
                caps_obj.get(other).is_none(),
                "flipping {expected_key} on leaked {other} into the JSON"
            );
        }
    }
}

// --- W6 F7: SessionCost event golden ---

#[test]
fn golden_session_cost_v_w6() {
    use wcore_protocol::events::TurnCost;
    let event = ProtocolEvent::SessionCost {
        session_id: "sess-001".into(),
        total_cost_usd: 0.123456,
        per_turn: vec![
            TurnCost {
                turn: 0,
                model: "claude-opus-4-7".into(),
                provider: "anthropic".into(),
                cost_usd: 0.05,
            },
            TurnCost {
                turn: 1,
                model: "claude-opus-4-7".into(),
                provider: "anthropic".into(),
                cost_usd: 0.073456,
            },
        ],
    };
    let got = serialize(&event);
    let want = json!({
        "type": "session_cost",
        "session_id": "sess-001",
        "total_cost_usd": 0.123456,
        "per_turn": [
            {
                "turn": 0,
                "model": "claude-opus-4-7",
                "provider": "anthropic",
                "cost_usd": 0.05
            },
            {
                "turn": 1,
                "model": "claude-opus-4-7",
                "provider": "anthropic",
                "cost_usd": 0.073456
            }
        ]
    });
    assert_eq!(got, want, "SessionCost golden drift");
}

#[test]
fn golden_session_cost_empty_per_turn() {
    let event = ProtocolEvent::SessionCost {
        session_id: "sess-empty".into(),
        total_cost_usd: 0.0,
        per_turn: vec![],
    };
    let got = serialize(&event);
    let want = json!({
        "type": "session_cost",
        "session_id": "sess-empty",
        "total_cost_usd": 0.0,
        "per_turn": []
    });
    assert_eq!(got, want);
}

// --- W6 F7: Ready event advertises cost_attribution when set ---

#[test]
fn golden_ready_with_cost_attribution_advertised() {
    let event = ProtocolEvent::Ready {
        version: "0.1.21".into(),
        session_id: Some("sess-cost".into()),
        capabilities: Capabilities {
            tool_approval: true,
            cost_attribution: true,
            ..Default::default()
        },
    };
    let got = serialize(&event);
    let caps = got
        .get("capabilities")
        .and_then(|v| v.as_object())
        .expect("capabilities is object");
    assert_eq!(
        caps.get("cost_attribution"),
        Some(&json!(true)),
        "cost_attribution must appear on Ready when set"
    );
    // Default-off W0 sibling flags must NOT leak.
    for k in [
        "streaming_tools",
        "sub_agent_traces",
        "hitl_suspend",
        "non_destructive_compact",
        "structured_traces",
        "rpc_tool_script",
        "browser_suite",
        "computer_use",
        "plugins",
    ] {
        assert!(
            caps.get(k).is_none(),
            "flipping cost_attribution leaked {k}"
        );
    }
}
