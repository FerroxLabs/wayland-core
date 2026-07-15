//! JSON-Lines envelope types for subprocess plugin communication.
//!
//! v0.6.5 Task 3.2 — full request/response envelope.
//!
//! ## Wire format
//!
//! One JSON object per `\n`-terminated line on stdin (host → plugin) and
//! stdout (plugin → host). Each request carries an `id: u64`; the plugin's
//! response echoes the same `id`. This is a JSON-Lines framing chosen over
//! length-prefix or full JSON-RPC because:
//!
//! - line-oriented framing is trivially diffable and tcpdump-friendly,
//! - the subprocess SDK has no need for batching or notifications,
//! - reusing `wcore-mcp::protocol::JsonRpcRequest` would force JSON-RPC
//!   error/result split semantics that the subprocess envelope doesn't
//!   benefit from — keeping the envelope small and purpose-built.
//!
//! Each line decodes into [`SubprocessRequest`] (host-emitted) or
//! [`SubprocessResponse`] (plugin-emitted). Unknown verbs are typed errors
//! at parse time, not silent ignores.
//!
//! See `.blackboard/v0.6.5-PLUGIN-SDK-PLAN.md` §3.2.

use serde::{Deserialize, Serialize};

/// Runtime capability advertised during `Init` by subprocess plugins that
/// understand [`SubprocessVerb::CallToolV2`]. Hosts must not send the
/// versioned verb unless this exact capability was negotiated.
pub const CAPABILITY_CALL_TOOL_V2: &str = "wayland.subprocess.call_tool_v2";

/// Host-to-plugin verb. The engine sends one of these per request line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "verb", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SubprocessVerb {
    /// Initial handshake — plugin replies with manifest + capability list.
    Init,
    /// Engine asks plugin to list its registered tools.
    ListTools,
    /// Engine calls a tool by name.
    CallTool {
        name: String,
        input: serde_json::Value,
    },
    /// Versioned F13 call carrying a stable durable-effect identity. The
    /// legacy `CallTool` verb remains unchanged for older hosts and plugins.
    CallToolV2 {
        name: String,
        input: serde_json::Value,
        effect: SubprocessToolEffectIdentity,
    },
    /// Engine signals shutdown.
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SubprocessToolEffectIdentity {
    pub version: u32,
    pub tool_execution_id: String,
    pub idempotency_key: String,
}

/// Host → plugin envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubprocessRequest {
    /// Monotonic request id, echoed by the matching [`SubprocessResponse`].
    pub id: u64,
    #[serde(flatten)]
    pub verb: SubprocessVerb,
}

impl SubprocessRequest {
    pub fn new(id: u64, verb: SubprocessVerb) -> Self {
        Self { id, verb }
    }
}

/// Plugin-side description of a tool, returned via [`SubprocessResponse::ToolsList`].
///
/// Kept deliberately small: name + description + input schema. Output schema
/// and richer metadata can ride later additions without breaking the wire
/// format because new fields are `#[serde(default)]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDescriptor {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// JSON-schema for the tool's input. Plugins MAY return
    /// `serde_json::Value::Null` if they have no input.
    #[serde(default = "default_input_schema")]
    pub input_schema: serde_json::Value,
}

fn default_input_schema() -> serde_json::Value {
    serde_json::Value::Null
}

/// Plugin → host envelope. Each variant corresponds 1:1 to a
/// [`SubprocessVerb`] request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum SubprocessResponseBody {
    /// Reply to [`SubprocessVerb::Init`].
    InitResult {
        /// Plugin-reported manifest version (free-form string; the host
        /// already has the parsed manifest from disk).
        manifest_version: String,
        /// Capability tags the plugin claims at runtime. The host's
        /// `PluginAccessGate` remains the authority for access. Reserved
        /// `wayland.subprocess.*` tags additionally negotiate optional wire
        /// extensions such as [`CAPABILITY_CALL_TOOL_V2`].
        capabilities: Vec<String>,
    },
    /// Reply to [`SubprocessVerb::ListTools`].
    ToolsList { tools: Vec<ToolDescriptor> },
    /// Reply to [`SubprocessVerb::CallTool`].
    CallToolResult {
        /// Plain-text rendering of the tool output (always present).
        stdout: String,
        /// Optional structured JSON output (`None` for text-only tools).
        #[serde(default)]
        structured: Option<serde_json::Value>,
        /// True when the tool itself reported a domain-level failure
        /// (e.g. invalid input) — distinct from a transport error.
        #[serde(default)]
        is_error: bool,
    },
    /// Reply to [`SubprocessVerb::Shutdown`]. Plugin should close stdin
    /// after sending this and exit cleanly.
    Ack,
    /// Out-of-band error from the plugin. Carries a stable code + message.
    /// The engine maps this onto [`crate::error::SubprocessPluginError::ProtocolError`].
    Error {
        code: String,
        message: String,
        #[serde(default)]
        data: Option<serde_json::Value>,
    },
}

/// Plugin → host envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubprocessResponse {
    /// Echoes the [`SubprocessRequest::id`] this is a reply to.
    pub id: u64,
    #[serde(flatten)]
    pub body: SubprocessResponseBody,
}

impl SubprocessResponse {
    pub fn new(id: u64, body: SubprocessResponseBody) -> Self {
        Self { id, body }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_roundtrips_for_each_verb() {
        let cases = [
            SubprocessRequest::new(1, SubprocessVerb::Init),
            SubprocessRequest::new(2, SubprocessVerb::ListTools),
            SubprocessRequest::new(
                3,
                SubprocessVerb::CallTool {
                    name: "echo".to_string(),
                    input: json!({"msg": "hi"}),
                },
            ),
            SubprocessRequest::new(
                4,
                SubprocessVerb::CallToolV2 {
                    name: "charge".to_string(),
                    input: json!({"amount": 42}),
                    effect: SubprocessToolEffectIdentity {
                        version: 1,
                        tool_execution_id: "execution-42".to_string(),
                        idempotency_key: "stable-key-42".to_string(),
                    },
                },
            ),
            SubprocessRequest::new(5, SubprocessVerb::Shutdown),
        ];
        for req in cases {
            let line = serde_json::to_string(&req).unwrap();
            assert!(line.contains("\"id\""));
            assert!(line.contains("\"verb\""));
            let back: SubprocessRequest = serde_json::from_str(&line).unwrap();
            assert_eq!(back, req);
        }
    }

    #[test]
    fn response_roundtrips_for_each_body() {
        let cases = [
            SubprocessResponse::new(
                1,
                SubprocessResponseBody::InitResult {
                    manifest_version: "0.1.0".into(),
                    capabilities: vec!["tools".into()],
                },
            ),
            SubprocessResponse::new(
                2,
                SubprocessResponseBody::ToolsList {
                    tools: vec![ToolDescriptor {
                        name: "echo".into(),
                        description: Some("Echoes input".into()),
                        input_schema: json!({"type": "object"}),
                    }],
                },
            ),
            SubprocessResponse::new(
                3,
                SubprocessResponseBody::CallToolResult {
                    stdout: "ok".into(),
                    structured: Some(json!({"ok": true})),
                    is_error: false,
                },
            ),
            SubprocessResponse::new(4, SubprocessResponseBody::Ack),
            SubprocessResponse::new(
                5,
                SubprocessResponseBody::Error {
                    code: "TOOL_NOT_FOUND".into(),
                    message: "no such tool".into(),
                    data: None,
                },
            ),
        ];
        for resp in cases {
            let line = serde_json::to_string(&resp).unwrap();
            assert!(line.contains("\"id\""));
            assert!(line.contains("\"kind\""));
            let back: SubprocessResponse = serde_json::from_str(&line).unwrap();
            assert_eq!(back, resp);
        }
    }
}
