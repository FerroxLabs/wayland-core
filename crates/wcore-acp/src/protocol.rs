//! ACP wire protocol types — JSON-RPC 2.0 envelopes + session/message types.
//!
//! Reference: https://github.com/anthropics/agent-client-protocol
//!
//! All types use `#[serde(deny_unknown_fields)]` to surface protocol drift
//! at parse time, and `#[non_exhaustive]` on public enums to allow SemVer
//! evolution.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// JSON-RPC 2.0 protocol version string.
pub const JSONRPC_VERSION: &str = "2.0";

// ── JSON-RPC envelope ────────────────────────────────────────────────────

/// JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub data: Option<serde_json::Value>,
}

/// Standard JSON-RPC 2.0 error codes plus ACP-specific extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCode {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    InternalError,
    /// ACP: session not found.
    SessionNotFound,
    /// ACP: authentication required or invalid.
    AuthRequired,
    /// ACP: tool execution failed.
    ToolFailed,
    /// ACP: `session/create` named an `agent` selector that is not in the
    /// authorized roster (persona-profiles Phase A). Distinct from
    /// `InvalidParams` so a host can tell "malformed request" from "no such
    /// agent for this principal". The roster returns only AUTHORIZED agents, so
    /// this doubles as the not-authorized signal without leaking existence.
    AgentNotFound,
}

impl ErrorCode {
    pub fn code(self) -> i64 {
        match self {
            Self::ParseError => -32700,
            Self::InvalidRequest => -32600,
            Self::MethodNotFound => -32601,
            Self::InvalidParams => -32602,
            Self::InternalError => -32603,
            Self::SessionNotFound => -32001,
            Self::AuthRequired => -32002,
            Self::ToolFailed => -32003,
            Self::AgentNotFound => -32004,
        }
    }
}

// ── Session lifecycle ────────────────────────────────────────────────────

/// `session/create` request payload.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCreateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Optional persona-agent selector (Phase A, persona-profiles). Names an
    /// [`AgentInfo::id`] from `agents/list` to bind this session to a trusted
    /// AgentPack persona (system_prompt/model/allowed_tools overlay) within the
    /// SAME process identity — it never selects a different profile/credential
    /// boundary. `serde(default)` + `skip_serializing_if` keep an absent
    /// selector byte-identical to the pre-persona wire (compat regression test
    /// `session_create_without_agent_is_byte_identical`); a client MUST only
    /// send it after the server advertises the `agent_selection` capability
    /// (added in PR-2). The field is inert until the roster/selector lands
    /// feature-flagged; an unknown id resolves to [`ErrorCode::AgentNotFound`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// `session/create` response payload.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionCreateResponse {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// `session/list` request payload (empty body — included for symmetry).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionListRequest {}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionGetRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionGetResponse {
    pub session: SessionMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDeleteRequest {
    pub session_id: String,
}

/// Session metadata returned by list/get.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionMetadata {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub created_at: i64,
    pub last_activity: i64,
    pub message_count: u64,
}

// ── Agent roster (persona-profiles Phase A) ──────────────────────────────

/// One entry in the `agents/list` roster — a selectable persona-agent.
///
/// SECURITY (red-team R4, mirrors the codebase's own F-070 anti-fingerprinting
/// in `a2a/default_handler.rs`): this type carries ONLY an opaque `id` and a
/// human `label`, plus an optional operator-authored `description` for display.
/// It MUST NEVER carry the persona's `system_prompt`/SOUL, model, provider, API
/// key, filesystem paths, or any other capability/credential detail — those
/// stay server-side and are bound by id at `session/create`. The roster itself
/// returns only agents the calling principal is AUTHORIZED to see (PR-3'), so a
/// leaked `AgentInfo` reveals nothing an authorized selector could not already
/// use.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentInfo {
    /// Opaque, stable selector id (matches `SessionCreateRequest::agent`).
    pub id: String,
    /// Human-readable display label. Non-secret.
    pub label: String,
    /// Optional operator-authored one-line display description. Non-secret;
    /// never derived from the persona's prompt/SOUL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `agents/list` request payload (empty body — included for symmetry, like
/// [`SessionListRequest`]).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentsListRequest {}

/// `agents/list` response payload: the roster of persona-agents the calling
/// principal is authorized to select.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentsListResponse {
    pub agents: Vec<AgentInfo>,
}

// ── Messages ─────────────────────────────────────────────────────────────

/// `message/send` request payload. Server emits a stream of [`MessageEvent`]s.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageSendRequest {
    pub session_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
}

/// One frame in the message stream.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MessageEvent {
    Thinking {
        text: String,
    },
    TextDelta {
        text: String,
    },
    ToolCall {
        call: ToolCall,
    },
    /// D012 (P0 security) — a mutating tool call that requires approval before
    /// it executes. This is the ACP/REST analogue of the TUI/json-stream
    /// `ToolRequest` + `ApprovalRequired` vocabulary: without it the protocol
    /// could only emit a bare `ToolCall`, indistinguishable from an
    /// already-approved call, so the safety control silently depended on which
    /// front-end drove the engine.
    ///
    /// Contract: when the session's approval posture gates a tool, the engine
    /// MUST emit exactly one `ApprovalRequired { call, .. }` for that call
    /// BEFORE the corresponding `ToolResult` (i.e. before the tool runs). A
    /// host that does not respond leaves the tool gated (it does not execute);
    /// the engine times the pending approval out rather than running ungated.
    /// Under an explicit allow-all / Force posture this frame is NOT emitted —
    /// the operator opted into auto-approval and the bare `ToolCall` rides
    /// straight to `ToolResult`.
    ApprovalRequired {
        /// The gated tool call. Carries the same `id` as the matching
        /// `ToolCall` / `ToolResult` so a host can correlate its decision.
        call: ToolCall,
        /// Human-readable explanation of why approval is required (e.g. the
        /// tool category). No em-dashes; surfaced verbatim to hosts.
        reason: String,
        /// GHSA-8r7g M2 (wayland#568) — the server-generated SECRET
        /// `resume_token` (`apr-{uuid}`) the host MUST present on the matching
        /// `POST .../resolve` to answer a BRIDGE-backed gate (Crucible council
        /// / egress consent). Empty for a manager-gated tool (ordinary
        /// approve/deny), which has no secret and resolves by the call `id`.
        /// `skip_serializing_if` keeps the frame clean when there is no secret.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        resume_token: String,
    },
    ToolResult {
        result: ToolResult,
    },
    Done {
        stop_reason: String,
    },
    Error {
        error: JsonRpcError,
    },
}

// ── Tools ────────────────────────────────────────────────────────────────

/// Advertised tool definition.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's input. Free-form object.
    #[schema(value_type = Object)]
    pub input_schema: serde_json::Value,
}

/// Tool call request from the model.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Tool input arguments. Free-form object.
    #[schema(value_type = Object)]
    pub input: serde_json::Value,
}

/// Tool execution result, paired to a [`ToolCall`].
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolResult {
    pub call_id: String,
    /// Tool output payload. Free-form (string or object).
    #[schema(value_type = Object)]
    pub output: serde_json::Value,
    #[serde(default)]
    pub is_error: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonrpc_request_roundtrip() {
        let req = JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: serde_json::json!(42),
            method: "session/create".to_string(),
            params: Some(serde_json::json!({"model": "claude-opus-4-7"})),
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: JsonRpcRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.method, "session/create");
        assert_eq!(back.jsonrpc, "2.0");
    }

    #[test]
    fn jsonrpc_response_with_error() {
        let resp = JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: serde_json::json!(7),
            result: None,
            error: Some(JsonRpcError {
                code: ErrorCode::SessionNotFound.code(),
                message: "no such session".to_string(),
                data: None,
            }),
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: JsonRpcResponse = serde_json::from_str(&s).unwrap();
        assert!(back.result.is_none());
        assert_eq!(back.error.unwrap().code, -32001);
    }

    #[test]
    fn deny_unknown_fields_on_request() {
        let bad = r#"{"jsonrpc":"2.0","id":1,"method":"x","mystery":"bad"}"#;
        let r: Result<JsonRpcRequest, _> = serde_json::from_str(bad);
        assert!(r.is_err(), "deny_unknown_fields should reject");
    }

    #[test]
    fn message_event_text_delta_serializes() {
        let ev = MessageEvent::TextDelta { text: "hi".into() };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"kind\":\"text_delta\""));
        assert!(s.contains("\"text\":\"hi\""));
    }

    #[test]
    fn tool_call_roundtrip() {
        let call = ToolCall {
            id: "tc-1".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "ls"}),
        };
        let s = serde_json::to_string(&call).unwrap();
        let back: ToolCall = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, "bash");
    }

    #[test]
    fn session_metadata_roundtrip() {
        let meta = SessionMetadata {
            session_id: "s1".into(),
            model: Some("claude-sonnet-4-6".into()),
            created_at: 1700000000,
            last_activity: 1700001000,
            message_count: 12,
        };
        let s = serde_json::to_string(&meta).unwrap();
        let back: SessionMetadata = serde_json::from_str(&s).unwrap();
        assert_eq!(back.message_count, 12);
    }

    #[test]
    fn error_code_distinct_values() {
        use std::collections::HashSet;
        let codes: HashSet<i64> = [
            ErrorCode::ParseError,
            ErrorCode::InvalidRequest,
            ErrorCode::MethodNotFound,
            ErrorCode::InvalidParams,
            ErrorCode::InternalError,
            ErrorCode::SessionNotFound,
            ErrorCode::AuthRequired,
            ErrorCode::ToolFailed,
            ErrorCode::AgentNotFound,
        ]
        .iter()
        .map(|c| c.code())
        .collect();
        assert_eq!(codes.len(), 9);
    }

    #[test]
    fn agent_not_found_code_value() {
        assert_eq!(ErrorCode::AgentNotFound.code(), -32004);
    }

    /// Compat regression (red-team R2): a `session/create` that does NOT select
    /// a persona-agent must serialize BYTE-IDENTICALLY to the pre-persona wire —
    /// no `agent` key present — so old hosts/servers are unaffected.
    #[test]
    fn session_create_without_agent_is_byte_identical() {
        let req = SessionCreateRequest {
            model: Some("claude-opus-4-8".into()),
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert_eq!(s, r#"{"model":"claude-opus-4-8"}"#);
        assert!(!s.contains("agent"), "absent selector must not appear on wire");
        // And a legacy payload (no `agent` field) still deserializes.
        let legacy: SessionCreateRequest =
            serde_json::from_str(r#"{"model":"claude-opus-4-8"}"#).unwrap();
        assert!(legacy.agent.is_none());
    }

    #[test]
    fn session_create_with_agent_roundtrips() {
        let req = SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: Some("researcher".into()),
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains(r#""agent":"researcher""#));
        let back: SessionCreateRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.agent.as_deref(), Some("researcher"));
    }

    /// Red-team R4: `AgentInfo` must expose ONLY id/label/description on the
    /// wire — never prompt/SOUL/model/provider/key/paths. Guards against a
    /// future field addition silently leaking a persona's capabilities.
    #[test]
    fn agent_info_exposes_no_secret_fields() {
        let info = AgentInfo {
            id: "researcher".into(),
            label: "Researcher".into(),
            description: Some("Deep web research persona".into()),
        };
        let v: serde_json::Value = serde_json::to_value(&info).unwrap();
        let keys: std::collections::BTreeSet<&str> =
            v.as_object().unwrap().keys().map(String::as_str).collect();
        let allowed: std::collections::BTreeSet<&str> =
            ["id", "label", "description"].into_iter().collect();
        assert!(
            keys.is_subset(&allowed),
            "AgentInfo leaked non-allowlisted field(s): {:?}",
            &keys - &allowed
        );
        for forbidden in ["system_prompt", "soul", "model", "provider", "api_key", "path"] {
            assert!(!keys.contains(forbidden), "AgentInfo must not carry {forbidden}");
        }
    }

    #[test]
    fn agent_info_omits_absent_description() {
        let info = AgentInfo {
            id: "a1".into(),
            label: "A1".into(),
            description: None,
        };
        let s = serde_json::to_string(&info).unwrap();
        assert_eq!(s, r#"{"id":"a1","label":"A1"}"#);
    }

    #[test]
    fn agents_list_response_roundtrip() {
        let resp = AgentsListResponse {
            agents: vec![
                AgentInfo { id: "a".into(), label: "A".into(), description: None },
                AgentInfo {
                    id: "b".into(),
                    label: "B".into(),
                    description: Some("second".into()),
                },
            ],
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: AgentsListResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.agents.len(), 2);
        assert_eq!(back.agents[1].description.as_deref(), Some("second"));
    }

    #[test]
    fn agents_list_request_rejects_unknown_fields() {
        let bad = r#"{"mystery":"x"}"#;
        let r: Result<AgentsListRequest, _> = serde_json::from_str(bad);
        assert!(r.is_err(), "deny_unknown_fields should reject");
    }
}
