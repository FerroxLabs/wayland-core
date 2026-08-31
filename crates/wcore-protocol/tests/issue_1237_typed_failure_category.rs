//! FerroxLabs/wayland#1237 (decomposed from wayland#388 c7) — the typed
//! failure category on `ErrorInfo`.
//!
//! c1 (round-trip through the frame), c3 (additive on the wire) and the
//! ALPHABET half of c4 live here, in the crate that owns the wire. The half of
//! c2 and c4 that needs an `AgentError` is in
//! `wcore-agent/tests/issue_1237_failure_category_test.rs`.

use wcore_protocol::events::{ErrorInfo, FailureCategory, ProtocolEvent};

/// Every variant, so nothing below can quietly test a subset.
const EVERY_CATEGORY: [FailureCategory; 4] = [
    FailureCategory::ContextLimit,
    FailureCategory::ToolRuntime,
    FailureCategory::LocalWayland,
    FailureCategory::Unknown,
];

fn error_frame(category: FailureCategory) -> serde_json::Value {
    serde_json::to_value(ProtocolEvent::Error {
        msg_id: Some("turn-1".to_string()),
        error: ErrorInfo {
            code: "engine_error".to_string(),
            message: "the context reached 190000 tokens against a limit of 180000".to_string(),
            retryable: false,
            category,
        },
    })
    .expect("an error frame serialises")
}

/// c1 — the category is a typed enum on the frame, and every variant survives
/// a round trip through the wire representation rather than through a Rust
/// clone.
#[test]
fn every_failure_category_round_trips_through_the_error_frame() {
    let mut seen = Vec::new();
    for category in EVERY_CATEGORY {
        let frame = error_frame(category);
        assert_eq!(frame["type"], "error", "control: this is the error frame");
        let wire = frame["error"]["category"]
            .as_str()
            .expect("the category is a string on the wire")
            .to_string();
        let decoded: ErrorInfo =
            serde_json::from_value(frame["error"].clone()).expect("the error object decodes back");
        assert_eq!(
            decoded.category, category,
            "{wire} must decode back to the variant that wrote it"
        );
        assert_eq!(decoded.code, "engine_error");
        assert!(!decoded.retryable);
        seen.push(wire);
    }
    seen.sort();
    assert_eq!(
        seen,
        vec!["context_limit", "local_wayland", "tool_runtime", "unknown"],
        "the wire alphabet is exactly these four; a fifth is a contract change"
    );
}

/// c4, the half that is a property of the TYPE rather than of any one run.
///
/// #388 names five categories. Two of them — provider rate limit and router
/// failure — arrive as the same non-2xx from the same host and cannot be told
/// apart from outside the router (wayland#1184). The refusal to guess is not a
/// convention here, it is the alphabet: there is no value core could emit that
/// names either of them, however wrong its classification became.
#[test]
fn the_category_alphabet_cannot_name_the_router_versus_provider_split() {
    for category in EVERY_CATEGORY {
        let wire = serde_json::to_string(&category).expect("a category serialises");
        assert!(
            !wire.contains("rate_limit") && !wire.contains("router_failure"),
            "{wire} names a distinction core cannot make (#1184)"
        );
    }
    // Known-positive control: the assertion above can fail. These two strings
    // are exactly what #388 asks for and what this repo must not claim.
    let control = serde_json::to_string("rate_limit").unwrap();
    assert!(control.contains("rate_limit"));
}

/// c3 — additive on the wire, both directions.
///
/// A payload written by a Core that predates the field still decodes, as
/// `unknown`; and the three keys a pinned host already reads are byte-identical
/// to what it read before, so a host that ignores the new key renders exactly
/// as it did.
#[test]
fn a_pre_change_error_payload_still_decodes_and_the_old_keys_do_not_move() {
    let pre_change = r#"{"code":"engine_error","message":"kaboom","retryable":true}"#;
    let decoded: ErrorInfo =
        serde_json::from_str(pre_change).expect("a pre-change payload must still decode");
    assert_eq!(decoded.code, "engine_error");
    assert_eq!(decoded.message, "kaboom");
    assert!(decoded.retryable);
    assert_eq!(
        decoded.category,
        FailureCategory::Unknown,
        "a frame that never named a category must read as unknown, not as a guess"
    );

    // Known-positive control: the decoder is not simply accepting anything.
    assert!(
        serde_json::from_str::<ErrorInfo>(r#"{"message":"kaboom","retryable":true}"#).is_err(),
        "control: a payload missing a REQUIRED key must still be refused"
    );

    // The other direction: what a pinned host reads is unchanged.
    for category in EVERY_CATEGORY {
        let frame = error_frame(category);
        let object = frame["error"].as_object().expect("the error is an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["category", "code", "message", "retryable"],
            "the field is ADDED; nothing a host already read may be renamed or removed"
        );
        assert_eq!(object["code"], "engine_error");
        assert_eq!(object["retryable"], false);
    }
}
