//! F-005 engine-side contract test: ApprovalRequired → ApprovalResume round-trip.
//!
//! Documents and validates the protocol contract that the Wayland app (Cluster L)
//! must implement to unblock HITL-gated tools:
//!
//!   1. Engine emits `ProtocolEvent::ApprovalRequired { call_id, resume_token, .. }`
//!   2. Host sends `ProtocolCommand::ApprovalResume { resume_token, approved, .. }`
//!   3. Engine accepts the command and routes it to the approval bridge.
//!
//! This test operates at the serde/wire level to verify the JSON shapes are
//! mutually compatible — the host can reconstruct a valid `ApprovalResume`
//! command from the fields delivered in the `ApprovalRequired` event.
//!
//! **F-005 gap:** The app's `WCoreCommand` union in
//! `app/src/process/agent/wcore/protocol.ts` is missing the `approval_resume`
//! arm. Until Cluster L adds it, HITL-gated tools started from the app hang
//! indefinitely. The engine contract tested here is correct; the fix belongs
//! entirely in app-side TypeScript.

use serde_json::{Value, json};
use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::events::ProtocolEvent;

/// Engine emits `ApprovalRequired` — verify the JSON wire shape.
#[test]
fn approval_required_event_serializes_correctly() {
    let event = ProtocolEvent::ApprovalRequired {
        call_id: "call-abc".to_string(),
        resume_token: "tok-xyz".to_string(),
        correlation_id: "tok-xyz".to_string(),
        reason: "Bash wants to delete files".to_string(),
        context: "rm -rf /tmp/test".to_string(),
        plan: None,
    };

    let json_str = serde_json::to_string(&event).expect("event must serialize");
    let parsed: Value = serde_json::from_str(&json_str).expect("must be valid JSON");

    assert_eq!(parsed["type"], "approval_required");
    assert_eq!(parsed["call_id"], "call-abc");
    assert_eq!(parsed["resume_token"], "tok-xyz");
    assert_eq!(parsed["reason"], "Bash wants to delete files");
    assert_eq!(parsed["context"], "rm -rf /tmp/test");
}

/// Host sends `ApprovalResume` using the token from the event — verify round-trip.
#[test]
fn approval_resume_command_accepted_with_token_from_event() {
    // Simulate the host echoing back the resume_token it received.
    let resume_token = "tok-xyz";
    let host_json = json!({
        "type": "approval_resume",
        "resume_token": resume_token,
        "approved": true
    })
    .to_string();

    let cmd: ProtocolCommand =
        serde_json::from_str(&host_json).expect("ApprovalResume must deserialize");

    match cmd {
        ProtocolCommand::ApprovalResume {
            resume_token: tok,
            approved,
            modifications,
        } => {
            assert_eq!(tok, "tok-xyz", "resume_token must round-trip intact");
            assert!(approved, "approved flag must round-trip");
            assert!(
                modifications.is_none(),
                "modifications absent when not sent"
            );
        }
        other => panic!("expected ApprovalResume, got {other:?}"),
    }
}

/// Host sends `ApprovalResume` with `approved: false` and modifications — verify shape.
#[test]
fn approval_resume_command_denied_with_modifications() {
    let host_json = json!({
        "type": "approval_resume",
        "resume_token": "tok-deny",
        "approved": false,
        "modifications": {"substitute_command": "echo safe"}
    })
    .to_string();

    let cmd: ProtocolCommand = serde_json::from_str(&host_json)
        .expect("ApprovalResume with modifications must deserialize");

    match cmd {
        ProtocolCommand::ApprovalResume {
            approved,
            modifications,
            ..
        } => {
            assert!(!approved);
            let mods = modifications.expect("modifications must be present");
            assert_eq!(mods["substitute_command"], "echo safe");
        }
        other => panic!("expected ApprovalResume, got {other:?}"),
    }
}

/// Validate the token emitted in the event equals the token the host must echo.
/// This is the key contract: host reads `resume_token` from the event JSON and
/// echoes it verbatim in the `approval_resume` command.
#[test]
fn resume_token_round_trip_from_event_to_command() {
    // Engine side: serialise ApprovalRequired event.
    let emitted_token = "unique-bridge-token-42";
    let event = ProtocolEvent::ApprovalRequired {
        call_id: "c1".to_string(),
        resume_token: emitted_token.to_string(),
        correlation_id: emitted_token.to_string(),
        reason: "needs approval".to_string(),
        context: "tool context".to_string(),
        plan: None,
    };
    let event_json: Value = serde_json::to_value(&event).unwrap();

    // Host side: extract resume_token from the event JSON and build resume command.
    let token_from_event = event_json["resume_token"]
        .as_str()
        .expect("resume_token must be a string in event JSON");

    let host_cmd_json = json!({
        "type": "approval_resume",
        "resume_token": token_from_event,
        "approved": true,
    })
    .to_string();

    let cmd: ProtocolCommand = serde_json::from_str(&host_cmd_json).unwrap();
    match cmd {
        ProtocolCommand::ApprovalResume { resume_token, .. } => {
            assert_eq!(
                resume_token, emitted_token,
                "token must survive the event→command round-trip intact"
            );
        }
        other => panic!("expected ApprovalResume, got {other:?}"),
    }
}
