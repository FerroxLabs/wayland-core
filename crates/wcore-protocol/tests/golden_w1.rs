//! W1 addition: trace_event golden. Locks the new variant added in W1
//! (`ProtocolEvent::TraceEvent { msg_id, trace }`). The v0.1.21 golden
//! in `golden_v0_1_21.rs` stays untouched; this file evolves alongside
//! W1+ protocol additions.

use serde_json::json;
use wcore_protocol::events::ProtocolEvent;

#[test]
fn golden_trace_event_w1() {
    let event = ProtocolEvent::TraceEvent {
        msg_id: "m-1".into(),
        trace: json!({
            "turn": 0,
            "model": "claude-3-5-haiku",
            "provider": "anthropic",
            "input_tokens": 1000,
            "output_tokens": 50,
            "cache_read": 800,
            "cache_write": 0,
            "cache_hit_rate": 0.8,
            "cost_usd": 0.0,
            "tool_calls": [],
            "hook_actions": [],
            "source_product": "wayland-core"
        }),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "trace_event");
    assert_eq!(got["msg_id"], "m-1");
    assert_eq!(got["trace"]["turn"], 0);
    assert_eq!(got["trace"]["cache_hit_rate"], 0.8);
}
