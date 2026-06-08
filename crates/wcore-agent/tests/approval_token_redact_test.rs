//! Wave SC SECURITY MAJOR fix — `ApprovalBridge` correlation ids are
//! redacted from streaming tool output before emission.
//!
//! Closes the audit finding: a tool that snoops stdout (e.g. Bash
//! invoking `tee` against captured protocol output) MUST NOT be able
//! to lift an in-flight approval correlation id and self-resolve. The
//! defense-in-depth pass strips active tokens from any emit_*
//! stream that carries tool-derived text.

use std::sync::Arc;

use wcore_agent::approval::{ApprovalBridge, ApprovalRequest};
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_protocol::writer::ProtocolWriter;

#[tokio::test]
async fn redact_strips_active_correlation_id_from_tool_output() {
    let bridge = ApprovalBridge::new();
    let writer = Arc::new(ProtocolWriter::new());
    let sink = ProtocolSink::new(writer).with_streaming_tools(true);
    sink.share_token_redactor_with(&bridge.redactor());

    // Spawn a pending approval — the bridge refreshes the redactor's
    // active set as part of the request.
    let (correlation_id, _rx) = bridge
        .request(ApprovalRequest {
            call_id: "c-1".into(),
            reason: "test".into(),
            context: "ctx".into(),
        })
        .await;
    // Direct redactor verification — the sink's redactor MUST observe
    // the correlation id.
    let active = sink.token_redactor().snapshot();
    assert!(
        active.contains(&correlation_id),
        "redactor must observe the in-flight correlation id"
    );

    // Redact through the sink's accessor.
    let chunk = format!("here is a leaked token: {correlation_id} — oops!");
    let redacted = sink.token_redactor().redact(&chunk);
    assert!(
        !redacted.contains(&correlation_id),
        "redact must scrub the raw correlation id"
    );
    assert!(
        redacted.contains("[REDACTED]"),
        "redact must replace with [REDACTED] marker"
    );
}

#[tokio::test]
async fn bridge_resolve_only_succeeds_with_correct_correlation_id() {
    // Sanity that fabricating a token doesn't resolve a pending
    // approval — the only way to resolve is to know the exact
    // correlation id from the bridge's pending map.
    let bridge = ApprovalBridge::new();
    let (correlation_id, _rx) = bridge
        .request(ApprovalRequest {
            call_id: "c-1".into(),
            reason: "test".into(),
            context: "ctx".into(),
        })
        .await;
    // Fabricated token — different from the real one.
    let bad = "apr-00000000-0000-0000-0000-000000000000".to_string();
    assert_ne!(bad, correlation_id);
    let resolved = bridge
        .resolve(
            &bad,
            wcore_agent::approval::ApprovalOutcome {
                approved: true,
                modifications: None,
            },
        )
        .await;
    assert!(!resolved, "resolve with wrong token must return false");
    // Real one works.
    let resolved = bridge
        .resolve(
            &correlation_id,
            wcore_agent::approval::ApprovalOutcome {
                approved: true,
                modifications: None,
            },
        )
        .await;
    assert!(resolved, "resolve with correct token must succeed");
}

#[tokio::test]
async fn redactor_no_op_when_no_approvals_in_flight() {
    let bridge = ApprovalBridge::new();
    let writer = Arc::new(ProtocolWriter::new());
    let sink = ProtocolSink::new(writer).with_streaming_tools(true);
    sink.share_token_redactor_with(&bridge.redactor());

    // No pending approvals → active set is empty → redact returns
    // input unchanged. Important for production-fast-path
    // correctness (we don't want to scribble on tool output when
    // there's no approval in flight).
    let text = "no approvals, no redaction";
    let redacted = sink.token_redactor().redact(text);
    assert_eq!(redacted, text);
}

#[tokio::test]
async fn redactor_observes_multiple_in_flight_ids() {
    let bridge = ApprovalBridge::new();
    let writer = Arc::new(ProtocolWriter::new());
    let sink = ProtocolSink::new(writer).with_streaming_tools(true);
    sink.share_token_redactor_with(&bridge.redactor());

    let (cid_a, _ra) = bridge
        .request(ApprovalRequest {
            call_id: "a".into(),
            reason: "".into(),
            context: "".into(),
        })
        .await;
    let (cid_b, _rb) = bridge
        .request(ApprovalRequest {
            call_id: "b".into(),
            reason: "".into(),
            context: "".into(),
        })
        .await;

    let composite = format!("first {cid_a} then {cid_b} — both leaked");
    let redacted = sink.token_redactor().redact(&composite);
    assert!(!redacted.contains(&cid_a), "cid_a must be redacted");
    assert!(!redacted.contains(&cid_b), "cid_b must be redacted");
}

#[tokio::test]
async fn redactor_releases_id_after_resolve() {
    // After resolve, the id is no longer "active" and the redactor
    // stops scrubbing it (the threat surface is the in-flight window
    // only).
    let bridge = ApprovalBridge::new();
    let writer = Arc::new(ProtocolWriter::new());
    let sink = ProtocolSink::new(writer);
    sink.share_token_redactor_with(&bridge.redactor());

    let (correlation_id, _rx) = bridge
        .request(ApprovalRequest {
            call_id: "c-1".into(),
            reason: "".into(),
            context: "".into(),
        })
        .await;
    assert!(sink.token_redactor().snapshot().contains(&correlation_id));

    bridge
        .resolve(
            &correlation_id,
            wcore_agent::approval::ApprovalOutcome {
                approved: true,
                modifications: None,
            },
        )
        .await;
    assert!(
        !sink.token_redactor().snapshot().contains(&correlation_id),
        "resolved id must drop out of the active set"
    );
}
