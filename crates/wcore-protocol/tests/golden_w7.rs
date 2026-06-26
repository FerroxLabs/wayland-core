//! W7 additions: golden snapshots for the new variants added by the
//! W7 wave. The v0.1.21 golden (`golden_v0_1_21.rs`) and the W1
//! golden (`golden_w1.rs`) stay untouched. This file evolves with W7+.

use serde_json::json;
use wcore_protocol::events::ProtocolEvent;

#[test]
fn golden_sub_agent_event_w7() {
    let inner = json!({ "type": "text_delta", "text": "step 1", "msg_id": "m-sub-1" });
    let event = ProtocolEvent::SubAgentEvent {
        parent_call_id: "c-spawn-1".into(),
        agent_name: "code-reviewer".into(),
        inner: inner.clone(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "sub_agent_event");
    assert_eq!(got["parent_call_id"], "c-spawn-1");
    assert_eq!(got["agent_name"], "code-reviewer");
    assert_eq!(got["inner"], inner);
}

#[test]
fn golden_workflow_started() {
    let event = ProtocolEvent::WorkflowStarted {
        workflow_id: "audit-run".into(),
        name: "Audit".into(),
        node_count: 3,
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "workflow_started");
    assert_eq!(got["workflow_id"], "audit-run");
    assert_eq!(got["name"], "Audit");
    assert_eq!(got["node_count"], 3);
}

#[test]
fn golden_workflow_finished() {
    let event = ProtocolEvent::WorkflowFinished {
        workflow_id: "audit-run".into(),
        succeeded: true,
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "workflow_finished");
    assert_eq!(got["workflow_id"], "audit-run");
    assert_eq!(got["succeeded"], true);

    // Failure variant carries succeeded:false.
    let failed = ProtocolEvent::WorkflowFinished {
        workflow_id: "audit-run".into(),
        succeeded: false,
    };
    let got = serde_json::to_value(&failed).unwrap();
    assert_eq!(got["succeeded"], false);
}

#[test]
fn golden_tool_chunk_w7() {
    let event = ProtocolEvent::ToolChunk {
        msg_id: "m-1".into(),
        call_id: "c-1".into(),
        tool_name: "Bash".into(),
        chunk: "partial stdout line\n".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "tool_chunk");
    assert_eq!(got["msg_id"], "m-1");
    assert_eq!(got["call_id"], "c-1");
    assert_eq!(got["tool_name"], "Bash");
    assert_eq!(got["chunk"], "partial stdout line\n");
}

#[test]
fn golden_provider_circuit_event_open_w7() {
    let event = ProtocolEvent::ProviderCircuitEvent {
        primary: "anthropic.claude-opus-4-7".into(),
        fallback: Some("anthropic.claude-sonnet-4-6".into()),
        state: "open".into(),
        error: Some("3 connection failures in 30s window".into()),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "provider_circuit_event");
    assert_eq!(got["primary"], "anthropic.claude-opus-4-7");
    assert_eq!(got["fallback"], "anthropic.claude-sonnet-4-6");
    assert_eq!(got["state"], "open");
    assert_eq!(got["error"], "3 connection failures in 30s window");
}

#[test]
fn golden_provider_circuit_event_closed_w7() {
    // closed = healthy; emitted on Half-Open → Closed transition. No
    // fallback in use, no error.
    let event = ProtocolEvent::ProviderCircuitEvent {
        primary: "anthropic.claude-opus-4-7".into(),
        fallback: None,
        state: "closed".into(),
        error: None,
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["state"], "closed");
    assert!(got.get("fallback").is_none()); // skip_serializing_if = None
    assert!(got.get("error").is_none());
}

#[test]
fn golden_approval_required_w7() {
    let event = ProtocolEvent::ApprovalRequired {
        call_id: "c-1".into(),
        resume_token: "tok-deadbeef".into(),
        correlation_id: String::new(),
        reason: "destructive shell command".into(),
        context: "rm -rf node_modules".into(),
        plan: None,
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "approval_required");
    assert_eq!(got["call_id"], "c-1");
    assert_eq!(got["resume_token"], "tok-deadbeef");
    assert_eq!(got["reason"], "destructive shell command");
    assert_eq!(got["context"], "rm -rf node_modules");
}

#[test]
fn golden_suspend_w7() {
    let event = ProtocolEvent::Suspend {
        reason: "awaiting_approval".into(),
        resume_token: "tok-deadbeef".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "suspend");
    assert_eq!(got["reason"], "awaiting_approval");
    assert_eq!(got["resume_token"], "tok-deadbeef");
}

#[test]
fn golden_approval_resume_approved_w7() {
    let event = ProtocolEvent::ApprovalResume {
        resume_token: "tok-deadbeef".into(),
        approved: true,
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "approval_resume");
    assert_eq!(got["resume_token"], "tok-deadbeef");
    assert_eq!(got["approved"], true);
}

#[test]
fn golden_approval_resume_rejected_w7() {
    let event = ProtocolEvent::ApprovalResume {
        resume_token: "tok-deadbeef".into(),
        approved: false,
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["approved"], false);
}
