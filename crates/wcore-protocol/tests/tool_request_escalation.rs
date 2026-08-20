//! Wire shape of `tool_request.tool.escalation` (#1099).
//!
//! Written from the host contract the Desktop lane asked for, not from the
//! producer: the field must be absent (not `null`) when there is no boundary,
//! and must serialize exactly the agreed object when there is one.

use serde_json::json;
use wcore_protocol::commands::PathGrantAccess;
use wcore_protocol::events::{ProtocolEvent, ToolCategory, ToolEscalation, ToolInfo};

fn request(escalation: Option<ToolEscalation>) -> serde_json::Value {
    serde_json::to_value(ProtocolEvent::ToolRequest {
        msg_id: "msg-1".into(),
        call_id: "call-1".into(),
        tool: ToolInfo {
            name: "Read".into(),
            category: ToolCategory::Info,
            args: json!({"file_path": "/Users/me/Documents/notes/q3.md"}),
            description: "Read /Users/me/Documents/notes/q3.md".into(),
            escalation,
        },
    })
    .unwrap()
}

#[test]
fn tool_request_without_a_boundary_has_no_escalation_key() {
    let value = request(None);
    let tool = value["tool"].as_object().unwrap();

    assert!(
        !tool.contains_key("escalation"),
        "an absent boundary must leave the frame byte-identical to what older \
         Core emitted; `\"escalation\": null` is a new member and a host that \
         validates strictly would see a changed shape. Got {tool:?}"
    );
    // The pre-existing members are untouched.
    assert_eq!(tool["name"], "Read");
    assert_eq!(tool["category"], "info");
    assert_eq!(tool["description"], "Read /Users/me/Documents/notes/q3.md");
}

#[test]
fn the_card_serializes_the_desktop_shape() {
    let value = request(Some(ToolEscalation::PathBoundary {
        target: "/Users/me/Documents/notes/q3.md".into(),
        access: PathGrantAccess::Read,
        suggested_root: "/Users/me/Documents/notes".into(),
    }));

    assert_eq!(
        value["tool"]["escalation"],
        json!({
            "kind": "path_boundary",
            "target": "/Users/me/Documents/notes/q3.md",
            "access": "read",
            "suggested_root": "/Users/me/Documents/notes"
        }),
        "this is the object the host renders the 'always allow this folder' \
         button from; a rename on either side breaks the button"
    );
}

#[test]
fn the_suggested_root_is_what_always_path_accepts() {
    // The card and the answer must speak one vocabulary: `suggested_root` goes
    // straight back as `always_path.root`, and `access` uses the same enum as
    // the `grant_path` command.
    let value = request(Some(ToolEscalation::PathBoundary {
        target: "/srv/data/report.csv".into(),
        access: PathGrantAccess::Read,
        suggested_root: "/srv/data".into(),
    }));
    let root = value["tool"]["escalation"]["suggested_root"]
        .as_str()
        .unwrap()
        .to_string();

    let approve: wcore_protocol::commands::ProtocolCommand = serde_json::from_value(json!({
        "type": "tool_approve",
        "call_id": "call-1",
        "scope": { "always_path": { "root": root, "write": false } }
    }))
    .expect("the root Core suggested must round-trip into the scope that grants it");

    match approve {
        wcore_protocol::commands::ProtocolCommand::ToolApprove { scope, .. } => assert_eq!(
            scope,
            wcore_protocol::commands::ApprovalScope::AlwaysPath {
                root: "/srv/data".into(),
                write: false,
            }
        ),
        other => panic!("unexpected command: {other:?}"),
    }
}
