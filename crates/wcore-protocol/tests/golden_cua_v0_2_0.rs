//! W8c.2 F.9 — golden snapshots for `CuaEvent` and `CuaPolicyDenied`.
//!
//! Both variants are gated by the W0-reserved `capabilities.computer_use`
//! flag at emission time; hosts that don't recognise the variants drop
//! them silently per the W0 forward-compat baseline.

use serde_json::json;
use wcore_protocol::events::ProtocolEvent;

#[test]
fn golden_cua_event() {
    let event = ProtocolEvent::CuaEvent {
        msg_id: "m-1".into(),
        call_id: "c-1".into(),
        op: "left_click".into(),
        coords: Some([100, 200]),
        summary: "clicked at (100,200)".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(
        got,
        json!({
            "type": "cua_event",
            "msg_id": "m-1",
            "call_id": "c-1",
            "op": "left_click",
            "coords": [100, 200],
            "summary": "clicked at (100,200)",
        })
    );
}

#[test]
fn golden_cua_event_omits_coords_when_none() {
    let event = ProtocolEvent::CuaEvent {
        msg_id: "m-2".into(),
        call_id: "c-2".into(),
        op: "screenshot".into(),
        coords: None,
        summary: "captured full screen".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "cua_event");
    assert!(
        got.get("coords").is_none(),
        "coords field must be omitted when None: {got}"
    );
    assert_eq!(got["summary"], "captured full screen");
}

#[test]
fn golden_cua_policy_denied() {
    let event = ProtocolEvent::CuaPolicyDenied {
        msg_id: "m-3".into(),
        op: "key".into(),
        app: "Finder".into(),
        reason: "key combo \"cmd+q+system\" is forbidden by policy".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(
        got,
        json!({
            "type": "cua_policy_denied",
            "msg_id": "m-3",
            "op": "key",
            "app": "Finder",
            "reason": "key combo \"cmd+q+system\" is forbidden by policy",
        })
    );
}

#[test]
fn golden_cua_policy_denied_omits_empty_app() {
    let event = ProtocolEvent::CuaPolicyDenied {
        msg_id: "m-4".into(),
        op: "left_click".into(),
        app: String::new(),
        reason: "app forbidden by policy".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert!(
        got.get("app").is_none(),
        "empty app field must be omitted: {got}"
    );
}

#[test]
fn cua_event_serializes_with_expected_type_tag() {
    let event = ProtocolEvent::CuaEvent {
        msg_id: "m-5".into(),
        call_id: "c-5".into(),
        op: "type".into(),
        coords: None,
        summary: "typed 7 chars".into(),
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["type"], "cua_event");
    assert_eq!(v["op"], "type");
    assert!(v.get("coords").is_none(), "coords omitted when None");
}
