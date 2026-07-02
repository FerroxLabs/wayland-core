use std::collections::HashMap;

use serde::Deserialize;

/// Commands sent from the client to the agent (Client -> Agent)
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ProtocolCommand {
    Message {
        msg_id: String,
        content: String,
        #[serde(default)]
        files: Vec<String>,
    },
    Stop,
    ToolApprove {
        call_id: String,
        #[serde(default)]
        scope: ApprovalScope,
        // v0.9.3 — additive answer channel for AskUserQuestion-class tools.
        // Electron host pre-v0.9.3 omits this field; serde-default keeps the
        // older wire shape backwards-compatible. `skip_serializing_if` is a
        // future-proofing no-op today (ProtocolCommand derives only
        // `Deserialize` per commands.rs:3).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<String>,
    },
    ToolDeny {
        call_id: String,
        #[serde(default)]
        reason: String,
    },
    InitHistory {
        text: String,
    },
    SetMode {
        mode: SessionMode,
    },
    SetConfig {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        thinking: Option<String>,
        #[serde(default)]
        thinking_budget: Option<u32>,
        #[serde(default)]
        effort: Option<String>,
        #[serde(default)]
        compaction: Option<String>,
    },
    AddMcpServer {
        name: String,
        transport: String,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        args: Option<Vec<String>>,
        #[serde(default)]
        env: Option<HashMap<String, String>>,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        headers: Option<HashMap<String, String>>,
    },
    /// W7 S4: resume a session that emitted `ApprovalRequired`. The
    /// host echoes the `resume_token` from the corresponding event so
    /// the engine can route the decision to the right pending bridge.
    ///
    /// **F-005 (CRIT app-side gap — TODO Cluster L):** The engine correctly
    /// waits for this command at `wcore-cli/src/main.rs` (ApprovalResume arm
    /// in the command loop), but the Wayland app's `WCoreCommand` union in
    /// `app/src/process/agent/wcore/protocol.ts` is missing this arm. Until
    /// Cluster L adds it, HITL-gated tools started from the app hang
    /// indefinitely because the host can never send the resume frame.
    /// Engine contract is correct; the fix belongs entirely in app-side code.
    ApprovalResume {
        resume_token: String,
        approved: bool,
        #[serde(default)]
        modifications: Option<serde_json::Value>,
    },
    /// #537/#141 host-send-transport hook: the host's reply to a
    /// `host_send_message_request` event, correlated by `call_id`.
    /// `ok = true` resolves the awaiting `send_message` tool call as sent
    /// (with the optional `message_id` receipt); `ok = false` surfaces
    /// `error` as a real tool failure to the model — never a false
    /// success. Routed through the shared `HostSendBridge` by the CLI
    /// command loop (including MID-turn, where the tool is parked — same
    /// pattern as the `ApprovalResume` mid-turn arm from GHSA-8r7g).
    HostSendMessageResult {
        call_id: String,
        ok: bool,
        #[serde(default)]
        message_id: Option<String>,
        #[serde(default)]
        error: Option<String>,
    },
    Ping,
}

#[derive(Debug, Deserialize, Default, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScope {
    #[default]
    Once,
    Always,
    /// Prefix-scoped always-allow (W0). Auto-approves only commands in
    /// the same category whose head matches `prefix` (literal-prefix on
    /// the normalized form). Serializes as
    /// `{"always_prefix":{"prefix":"cargo "}}`. Old clients never emit
    /// it, so the `Once`/`Always` bare-string wire-format is unchanged.
    AlwaysPrefix {
        prefix: String,
    },
}

/// Per DECISIONS.md §D1: `Force` is the canonical variant name.
///
/// Foreign-agent vocabulary aliases accepted via serde:
/// - `"yolo"` — Gemini CLI (`--yolo` flag surface)
/// - `"dangerously_skip_permissions"` — Claude Code (snake_case form)
/// - `"dangerously-skip-permissions"` — Claude Code (kebab-case form)
/// - `"dangerously_skip_sandbox_and_permissions"` — Codex
///
/// The canonical `"force"` (produced by `rename_all = "snake_case"`) is
/// always accepted. All aliases deserialise to `SessionMode::Force` so
/// foreign agents can drive wcore without an enum rename on either side.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Default,
    AutoEdit,
    #[serde(
        alias = "yolo",
        alias = "dangerously_skip_permissions",
        alias = "dangerously-skip-permissions",
        alias = "dangerously_skip_sandbox_and_permissions"
    )]
    Force,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_config_debug_format() {
        let cmd = ProtocolCommand::SetConfig {
            model: Some("test-model".into()),
            thinking: None,
            thinking_budget: None,
            effort: None,
            compaction: None,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetConfig"));
        assert!(dbg.contains("test-model"));
    }

    #[test]
    fn set_config_equality() {
        let a = ProtocolCommand::SetConfig {
            model: Some("m".into()),
            thinking: None,
            thinking_budget: None,
            effort: None,
            compaction: None,
        };
        let b = ProtocolCommand::SetConfig {
            model: Some("m".into()),
            thinking: None,
            thinking_budget: None,
            effort: None,
            compaction: None,
        };
        assert_eq!(a, b);

        let c = ProtocolCommand::SetConfig {
            model: None,
            thinking: None,
            thinking_budget: None,
            effort: None,
            compaction: None,
        };
        assert_ne!(a, c);
    }

    #[test]
    fn set_config_with_all_fields_equality() {
        let a = ProtocolCommand::SetConfig {
            model: Some("m".into()),
            thinking: Some("enabled".into()),
            thinking_budget: Some(8000),
            effort: Some("high".into()),
            compaction: None,
        };
        let b = ProtocolCommand::SetConfig {
            model: Some("m".into()),
            thinking: Some("enabled".into()),
            thinking_budget: Some(8000),
            effort: Some("high".into()),
            compaction: None,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn set_config_all_none_fields() {
        let cmd = ProtocolCommand::SetConfig {
            model: None,
            thinking: None,
            thinking_budget: None,
            effort: None,
            compaction: None,
        };
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("SetConfig"));
    }

    #[test]
    fn set_config_with_compaction() {
        let json = r#"{"type":"set_config","compaction":"full"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::SetConfig { compaction, .. } => {
                assert_eq!(compaction.unwrap(), "full");
            }
            _ => panic!("expected SetConfig"),
        }
    }

    #[test]
    fn set_config_compaction_none_by_default() {
        let json = r#"{"type":"set_config","model":"test"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::SetConfig { compaction, .. } => {
                assert!(compaction.is_none());
            }
            _ => panic!("expected SetConfig"),
        }
    }

    #[test]
    fn add_mcp_server_stdio_deserialize() {
        let json = r#"{
            "type": "add_mcp_server",
            "name": "team-tools",
            "transport": "stdio",
            "command": "node",
            "args": ["bridge.js", "--port", "9000"],
            "env": {"TOKEN": "abc123"}
        }"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::AddMcpServer {
                name,
                transport,
                command,
                args,
                env,
                url,
                headers,
            } => {
                assert_eq!(name, "team-tools");
                assert_eq!(transport, "stdio");
                assert_eq!(command.unwrap(), "node");
                assert_eq!(args.unwrap(), vec!["bridge.js", "--port", "9000"]);
                assert_eq!(env.unwrap().get("TOKEN").unwrap(), "abc123");
                assert!(url.is_none());
                assert!(headers.is_none());
            }
            _ => panic!("expected AddMcpServer"),
        }
    }

    #[test]
    fn ping_deserialize() {
        let json = r#"{"type":"ping"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        assert_eq!(cmd, ProtocolCommand::Ping);
    }

    #[test]
    fn add_mcp_server_sse_deserialize() {
        let json = r#"{
            "type": "add_mcp_server",
            "name": "remote-tools",
            "transport": "sse",
            "url": "http://localhost:8080/sse",
            "headers": {"Authorization": "Bearer tok"}
        }"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::AddMcpServer {
                name,
                transport,
                command,
                url,
                headers,
                ..
            } => {
                assert_eq!(name, "remote-tools");
                assert_eq!(transport, "sse");
                assert!(command.is_none());
                assert_eq!(url.unwrap(), "http://localhost:8080/sse");
                assert_eq!(headers.unwrap().get("Authorization").unwrap(), "Bearer tok");
            }
            _ => panic!("expected AddMcpServer"),
        }
    }

    #[test]
    fn approval_resume_deserialize() {
        let json = r#"{"type":"approval_resume","resume_token":"t","approved":true}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::ApprovalResume {
                resume_token,
                approved,
                modifications,
            } => {
                assert_eq!(resume_token, "t");
                assert!(approved);
                assert!(modifications.is_none());
            }
            _ => panic!("expected ApprovalResume"),
        }
    }

    // F-004: SessionMode::Force must accept all foreign-agent vocabulary aliases.
    // Gemini sends "yolo", Claude Code sends "dangerously_skip_permissions" (and
    // the kebab variant), Codex sends "dangerously_skip_sandbox_and_permissions".
    // All must deserialise to SessionMode::Force without error.
    #[test]
    fn set_mode_force_canonical() {
        let json = r#"{"type":"set_mode","mode":"force"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        assert_eq!(
            cmd,
            ProtocolCommand::SetMode {
                mode: SessionMode::Force
            }
        );
    }

    #[test]
    fn set_mode_force_alias_yolo() {
        let json = r#"{"type":"set_mode","mode":"yolo"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        assert_eq!(
            cmd,
            ProtocolCommand::SetMode {
                mode: SessionMode::Force
            }
        );
    }

    #[test]
    fn set_mode_force_alias_dangerously_skip_permissions() {
        let json = r#"{"type":"set_mode","mode":"dangerously_skip_permissions"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        assert_eq!(
            cmd,
            ProtocolCommand::SetMode {
                mode: SessionMode::Force
            }
        );
    }

    #[test]
    fn set_mode_force_alias_dangerously_skip_permissions_kebab() {
        let json = r#"{"type":"set_mode","mode":"dangerously-skip-permissions"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        assert_eq!(
            cmd,
            ProtocolCommand::SetMode {
                mode: SessionMode::Force
            }
        );
    }

    #[test]
    fn set_mode_force_alias_dangerously_skip_sandbox_and_permissions() {
        let json = r#"{"type":"set_mode","mode":"dangerously_skip_sandbox_and_permissions"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        assert_eq!(
            cmd,
            ProtocolCommand::SetMode {
                mode: SessionMode::Force
            }
        );
    }

    // W0: ApprovalScope gains the prefix-carrying variant `AlwaysPrefix`.
    // `Once`/`Always` must keep their v0.9.1 snake_case bare-string wire form
    // so the Electron app host (which never emits `AlwaysPrefix`) is unaffected.
    #[test]
    fn approval_scope_wire_format_is_backward_compatible() {
        let once: ApprovalScope = serde_json::from_str("\"once\"").unwrap();
        assert_eq!(once, ApprovalScope::Once);
        let always: ApprovalScope = serde_json::from_str("\"always\"").unwrap();
        assert_eq!(always, ApprovalScope::Always);
        // The new variant deserializes from an externally-tagged object.
        let pfx: ApprovalScope =
            serde_json::from_str("{\"always_prefix\":{\"prefix\":\"cargo \"}}").unwrap();
        assert_eq!(
            pfx,
            ApprovalScope::AlwaysPrefix {
                prefix: "cargo ".to_string()
            }
        );
    }

    // v0.9.3 W0.1 — ToolApprove gains an additive `answer` field for
    // AskUserQuestion-class tools (carries the user's choice back through
    // the approval channel). Pre-v0.9.3 hosts omit the field; serde-default
    // must keep that wire shape working. ProtocolCommand derives only
    // `Deserialize` (commands.rs:3), so the test uses `from_str` only.
    #[test]
    fn tool_approve_old_shape_backwards_compatible() {
        let old: ProtocolCommand =
            serde_json::from_str(r#"{"type":"tool_approve","call_id":"abc123","scope":"once"}"#)
                .unwrap();
        match old {
            ProtocolCommand::ToolApprove {
                call_id,
                scope,
                answer,
            } => {
                assert_eq!(call_id, "abc123");
                assert_eq!(scope, ApprovalScope::Once);
                assert_eq!(answer, None);
            }
            _ => panic!("Expected ToolApprove"),
        }
    }

    #[test]
    fn tool_approve_new_shape_deserializes() {
        let new: ProtocolCommand = serde_json::from_str(
            r#"{"type":"tool_approve","call_id":"abc123","scope":"once","answer":"Choice C"}"#,
        )
        .unwrap();
        match new {
            ProtocolCommand::ToolApprove {
                call_id,
                scope,
                answer,
            } => {
                assert_eq!(call_id, "abc123");
                assert_eq!(scope, ApprovalScope::Once);
                assert_eq!(answer, Some("Choice C".to_string()));
            }
            _ => panic!("Expected ToolApprove"),
        }
    }

    /// #537/#141: success reply as the desktop emits it —
    /// `{"type":"host_send_message_result","call_id":...,"ok":true,
    /// "message_id":...}` (error omitted).
    #[test]
    fn host_send_message_result_ok_deserialize() {
        let json = r#"{"type":"host_send_message_result","call_id":"hsm-1","ok":true,"message_id":"msg-123"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::HostSendMessageResult {
                call_id,
                ok,
                message_id,
                error,
            } => {
                assert_eq!(call_id, "hsm-1");
                assert!(ok);
                assert_eq!(message_id.as_deref(), Some("msg-123"));
                assert!(error.is_none());
            }
            _ => panic!("expected HostSendMessageResult"),
        }
    }

    /// #537/#141: failure reply — `ok:false` with `error`, no `message_id`.
    /// Both optionals are serde-default so the minimal shape
    /// (`call_id` + `ok` only) also parses.
    #[test]
    fn host_send_message_result_err_and_minimal_deserialize() {
        let json = r#"{"type":"host_send_message_result","call_id":"hsm-2","ok":false,"error":"SMTP 550: mailbox unavailable"}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::HostSendMessageResult {
                call_id,
                ok,
                message_id,
                error,
            } => {
                assert_eq!(call_id, "hsm-2");
                assert!(!ok);
                assert!(message_id.is_none());
                assert_eq!(error.as_deref(), Some("SMTP 550: mailbox unavailable"));
            }
            _ => panic!("expected HostSendMessageResult"),
        }

        let minimal = r#"{"type":"host_send_message_result","call_id":"hsm-3","ok":true}"#;
        let cmd: ProtocolCommand = serde_json::from_str(minimal).unwrap();
        match cmd {
            ProtocolCommand::HostSendMessageResult {
                message_id, error, ..
            } => {
                assert!(message_id.is_none());
                assert!(error.is_none());
            }
            _ => panic!("expected HostSendMessageResult"),
        }
    }

    #[test]
    fn approval_resume_deserialize_with_modifications() {
        let json = r#"{"type":"approval_resume","resume_token":"t2","approved":false,"modifications":{"note":"edited"}}"#;
        let cmd: ProtocolCommand = serde_json::from_str(json).unwrap();
        match cmd {
            ProtocolCommand::ApprovalResume {
                approved,
                modifications,
                ..
            } => {
                assert!(!approved);
                assert!(modifications.is_some());
            }
            _ => panic!("expected ApprovalResume"),
        }
    }
}
