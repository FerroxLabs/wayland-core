use std::collections::HashMap;

use serde::Deserialize;
use thiserror::Error;

use crate::diagnostics::GetRuntimeDiagnosticsCommand;
use crate::events::{OperatorToolEffectResolution, RecoveryCursor};

pub const OPERATOR_RESOLUTION_RECOVERY_VERSION: u16 = 1;
pub const RECOVERED_APPROVAL_VERSION: u16 = 1;

/// Closed payload for a versioned durable-session resynchronization request.
///
/// This command was introduced with recovery v1, so rejecting unknown fields
/// does not tighten any legacy wire shape. It prevents a host from believing
/// an authority-bearing extension was honored when Core actually ignored it.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SessionResyncCommand {
    pub recovery_version: u16,
    pub request_id: String,
    pub session_id: String,
    #[serde(default)]
    pub after: Option<RecoveryCursor>,
}

/// Closed payload for an operator action on an interrupted durable turn.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResumeTurnCommand {
    pub recovery_version: u16,
    pub request_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub cursor: RecoveryCursor,
    pub action: ResumeTurnAction,
}

/// Closed payload for resolving the exact approval restored after a crash.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResolveInterruptedApprovalCommand {
    pub recovery_version: u16,
    pub request_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub cursor: RecoveryCursor,
    pub approval_id: String,
    pub decision: RecoveredApprovalDecision,
    #[serde(default)]
    pub answer: Option<String>,
}

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
    /// Add explicit operator-authorized headroom to the active session's
    /// provider envelope. This never changes global or per-user limits.
    ContinueWithBudget {
        #[serde(default)]
        additional_tokens: u64,
        #[serde(default)]
        additional_cost_usd: f64,
    },
    /// Request a versioned, idempotently-correlated recovery view of a
    /// durable session. `after = None` asks for the current snapshot;
    /// supplying a cursor additionally asks for typed replay after it.
    SessionResync(SessionResyncCommand),
    /// Apply an explicit recovery action to an interrupted turn. The cursor
    /// binds the decision to the state the operator actually inspected.
    ResumeTurn(ResumeTurnCommand),
    /// Resolve the exact approval gate restored for an interrupted durable
    /// turn. The cursor prevents a stale host card from authorizing a newer
    /// tool call after the session head advances.
    ResolveInterruptedApproval(ResolveInterruptedApprovalCommand),
    /// Resolve an unknown tool effect only after binding the operator's typed
    /// claim to the exact durable state they inspected. Dispatchers must call
    /// [`ProtocolCommand::validate_operator_resolution`] against live
    /// authority before applying this command.
    ResolveUnknownToolEffect(OperatorToolEffectResolution),
    /// Request the process's versioned, redacted effective runtime view.
    GetRuntimeDiagnostics(GetRuntimeDiagnosticsCommand),
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
    /// Request a read-only, process-lifetime developer capability for an
    /// already-running local Desktop session. Core accepts this only when the
    /// local launcher opted in and the workspace uses the trusted-local
    /// sandbox profile. It never widens writes or disables containment.
    GrantWorkspaceCapability {
        executable: String,
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

/// Live authority against which an operator-resolution command is validated.
#[derive(Debug, Clone, Copy)]
pub struct OperatorResolutionAuthority<'a> {
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub cursor: &'a RecoveryCursor,
    pub tool_execution_id: &'a str,
}

/// Fail-closed protocol-boundary errors for operator tool-effect resolution.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OperatorResolutionValidationError {
    #[error("command is not an operator tool-effect resolution")]
    WrongCommand,
    #[error("unsupported operator-resolution recovery version: {actual}")]
    UnsupportedVersion { actual: u16 },
    #[error("malformed operator-resolution field: {field}")]
    Malformed { field: &'static str },
    #[error("stale operator-resolution authority: {field}")]
    Stale { field: &'static str },
}

impl ProtocolCommand {
    /// Validate syntax and exact live authority before an operator resolution
    /// reaches a dispatcher. Unknown fields/enums are already rejected by the
    /// closed serde types; this boundary additionally rejects malformed and
    /// stale claims.
    pub fn validate_operator_resolution(
        &self,
        authority: &OperatorResolutionAuthority<'_>,
    ) -> Result<(), OperatorResolutionValidationError> {
        let Self::ResolveUnknownToolEffect(resolution) = self else {
            return Err(OperatorResolutionValidationError::WrongCommand);
        };

        if resolution.recovery_version != OPERATOR_RESOLUTION_RECOVERY_VERSION {
            return Err(OperatorResolutionValidationError::UnsupportedVersion {
                actual: resolution.recovery_version,
            });
        }
        for (field, value) in [
            ("session_id", resolution.session_id.as_str()),
            ("turn_id", resolution.turn_id.as_str()),
            ("tool_execution_id", resolution.tool_execution_id.as_str()),
            ("operator_id", resolution.operator_id.as_str()),
        ] {
            if !valid_identifier(value) {
                return Err(OperatorResolutionValidationError::Malformed { field });
            }
        }
        if !valid_identifier(&resolution.evidence.reference_id) {
            return Err(OperatorResolutionValidationError::Malformed {
                field: "evidence.reference_id",
            });
        }
        if resolution.evidence.observed_at_unix_ms == 0 {
            return Err(OperatorResolutionValidationError::Malformed {
                field: "evidence.observed_at_unix_ms",
            });
        }
        if !valid_recovery_cursor_digest(&resolution.cursor.journal_digest) {
            return Err(OperatorResolutionValidationError::Malformed {
                field: "cursor.journal_digest",
            });
        }
        if !valid_evidence_digest(&resolution.evidence.digest) {
            return Err(OperatorResolutionValidationError::Malformed {
                field: "evidence.digest",
            });
        }

        for (field, matches) in [
            ("session_id", resolution.session_id == authority.session_id),
            ("turn_id", resolution.turn_id == authority.turn_id),
            (
                "tool_execution_id",
                resolution.tool_execution_id == authority.tool_execution_id,
            ),
            ("cursor", &resolution.cursor == authority.cursor),
        ] {
            if !matches {
                return Err(OperatorResolutionValidationError::Stale { field });
            }
        }
        Ok(())
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn valid_recovery_cursor_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_evidence_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
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

/// Host-selected action for an interrupted turn.
///
/// `Reconcile` invokes only Core-registered authoritative reconcilers. It does
/// not carry, and must never be interpreted as, a free-form operator claim
/// that an external effect succeeded or failed.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResumeTurnAction {
    Continue,
    Reconcile,
    Cancel,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveredApprovalDecision {
    Approve,
    Deny,
}

/// Per DECISIONS.md §D1: `Force` is the canonical variant name.
///
/// Foreign-agent vocabulary aliases accepted via serde:
/// - `"yolo"` — Gemini CLI (`--yolo` flag surface)
/// - `"dangerously_skip_permissions"` — Claude Code (snake_case form)
/// - `"dangerously-skip-permissions"` — Claude Code (kebab-case form)
///
/// The canonical `"force"` (produced by `rename_all = "snake_case"`) is
/// always accepted. All aliases deserialise to `SessionMode::Force` so
/// foreign agents can drive wcore without an enum rename on either side.
/// Vocabulary that claims sandbox bypass is rejected: an untrusted wire peer
/// cannot mint the local lease required for Dangerous authority.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    Default,
    AutoEdit,
    #[serde(
        alias = "yolo",
        alias = "dangerously_skip_permissions",
        alias = "dangerously-skip-permissions"
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
    fn continue_with_budget_is_typed_and_defaults_missing_axes_to_zero() {
        let command: ProtocolCommand =
            serde_json::from_str(r#"{"type":"continue_with_budget","additional_tokens":250000}"#)
                .unwrap();
        assert_eq!(
            command,
            ProtocolCommand::ContinueWithBudget {
                additional_tokens: 250_000,
                additional_cost_usd: 0.0,
            }
        );
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
    fn workspace_capability_grant_deserializes_without_authority_claims() {
        let command: ProtocolCommand = serde_json::from_str(
            r#"{"type":"grant_workspace_capability","executable":"/opt/sdk/bin/tool"}"#,
        )
        .unwrap();
        assert_eq!(
            command,
            ProtocolCommand::GrantWorkspaceCapability {
                executable: "/opt/sdk/bin/tool".to_string(),
            }
        );
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

    // F-004: SessionMode::Force accepts foreign approval-bypass vocabulary.
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
    fn set_mode_rejects_untrusted_sandbox_bypass_vocabulary() {
        let json = r#"{"type":"set_mode","mode":"dangerously_skip_sandbox_and_permissions"}"#;
        let error = serde_json::from_str::<ProtocolCommand>(json)
            .expect_err("wire peers cannot mint local Dangerous authority");
        assert!(error.to_string().contains("unknown variant"));
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
