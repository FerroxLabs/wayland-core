//! W8c.1 E.14 — golden snapshots for `BrowserEvent` and
//! `BrowserPolicyDenied`. Both variants are gated by the W0-reserved
//! `capabilities.browser_suite` flag at emission time; hosts that don't
//! recognise the variants drop them silently per the W0 forward-compat
//! baseline.

use serde_json::json;
use wcore_protocol::events::ProtocolEvent;

#[test]
fn golden_browser_event() {
    let event = ProtocolEvent::BrowserEvent {
        msg_id: "m-1".into(),
        call_id: "c-1".into(),
        op: "navigate".into(),
        url: Some("https://example.com".into()),
        summary: "loaded".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(
        got,
        json!({
            "type": "browser_event",
            "msg_id": "m-1",
            "call_id": "c-1",
            "op": "navigate",
            "url": "https://example.com",
            "summary": "loaded",
        })
    );
}

#[test]
fn golden_browser_event_omits_url_when_none() {
    // Ops without a URL (Snapshot, Click, Console, ...) skip the field.
    let event = ProtocolEvent::BrowserEvent {
        msg_id: "m-2".into(),
        call_id: "c-2".into(),
        op: "snapshot".into(),
        url: None,
        summary: "captured 4 refs".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "browser_event");
    assert!(
        got.get("url").is_none(),
        "url field must be omitted when None: {got}"
    );
    assert_eq!(got["summary"], "captured 4 refs");
}

#[test]
fn golden_browser_policy_denied() {
    let event = ProtocolEvent::BrowserPolicyDenied {
        msg_id: "m-1".into(),
        url: "http://169.254.169.254/".into(),
        reason: "metadata endpoint blocked".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(
        got,
        json!({
            "type": "browser_policy_denied",
            "msg_id": "m-1",
            "url": "http://169.254.169.254/",
            "reason": "metadata endpoint blocked",
        })
    );
}
