use rstest::rstest;
use wcore_protocol::commands::{ApprovalScope, ProtocolCommand, SessionMode};

#[rstest]
#[case(
    r#"{"type":"message","msg_id":"m1","content":"Hello"}"#,
    ProtocolCommand::Message {
        msg_id: "m1".to_string(),
        content: "Hello".to_string(),
        files: vec![],
    }
)]
#[case(
    r#"{"type":"message","msg_id":"m2","content":"Read this","files":["/tmp/a.rs"]}"#,
    ProtocolCommand::Message {
        msg_id: "m2".to_string(),
        content: "Read this".to_string(),
        files: vec!["/tmp/a.rs".to_string()],
    }
)]
#[case(r#"{"type":"stop"}"#, ProtocolCommand::Stop)]
#[case(
    r#"{"type":"init_history","text":"history"}"#,
    ProtocolCommand::InitHistory {
        text: "history".to_string(),
    }
)]
#[case(
    r#"{"type":"set_mode","mode":"default"}"#,
    ProtocolCommand::SetMode {
        mode: SessionMode::Default,
    }
)]
#[case(
    r#"{"type":"set_mode","mode":"auto_edit"}"#,
    ProtocolCommand::SetMode {
        mode: SessionMode::AutoEdit,
    }
)]
#[case(
    r#"{"type":"set_mode","mode":"force"}"#,
    ProtocolCommand::SetMode {
        mode: SessionMode::Force,
    }
)]
fn deserializes_protocol_commands(#[case] json: &str, #[case] expected: ProtocolCommand) {
    let cmd: ProtocolCommand = serde_json::from_str(json).expect("command should deserialize");
    assert_eq!(cmd, expected);
}

#[rstest]
#[case(r#"{"type":"tool_approve","call_id":"c1"}"#, ApprovalScope::Once)]
#[case(
    r#"{"type":"tool_approve","call_id":"c1","scope":"always"}"#,
    ApprovalScope::Always
)]
fn deserializes_tool_approve_scope(#[case] json: &str, #[case] expected_scope: ApprovalScope) {
    let cmd: ProtocolCommand = serde_json::from_str(json).expect("tool approve should deserialize");

    match cmd {
        ProtocolCommand::ToolApprove {
            call_id,
            scope,
            answer,
        } => {
            assert_eq!(call_id, "c1");
            assert_eq!(scope, expected_scope);
            // v0.9.3 W0.1 — `answer` defaults to None when the wire omits it.
            assert_eq!(answer, None);
        }
        other => panic!("expected ToolApprove, got {other:?}"),
    }
}

#[rstest]
#[case(r#"{"type":"tool_deny","call_id":"c1"}"#, "")]
#[case(
    r#"{"type":"tool_deny","call_id":"c1","reason":"not allowed"}"#,
    "not allowed"
)]
fn deserializes_tool_deny_reason(#[case] json: &str, #[case] expected_reason: &str) {
    let cmd: ProtocolCommand = serde_json::from_str(json).expect("tool deny should deserialize");

    match cmd {
        ProtocolCommand::ToolDeny { call_id, reason } => {
            assert_eq!(call_id, "c1");
            assert_eq!(reason, expected_reason);
        }
        other => panic!("expected ToolDeny, got {other:?}"),
    }
}
