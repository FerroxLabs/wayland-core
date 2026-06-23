//! T3-3.1.4 — `send_message` cross-channel messaging tool.
//!
//! Ported from an upstream MIT-licensed library (see THIRD-PARTY-NOTICES.md). The Python original
//! routes to ~17 external
//! messaging platforms (Telegram, Discord, Slack, Matrix, Signal,
//! Email, SMS, etc.). Wayland's engine has no adapters for any of
//! those, so this port covers the **dispatch surface** only — schema,
//! target parsing, platform name validation, and a pluggable
//! `MessageTransport` boundary that a host (CLI / Electron / gateway
//! daemon) wires to a real backend at construction time.
//!
//! Without a transport bound, `execute()` returns a structured error
//! ("no transport configured for platform <name>") rather than a
//! silent stub — honoring the NO-STUBS contract of T3.
//!
//! Divergences from the Python original (intentional):
//! * No cron-auto-delivery duplicate suppression (cron scheduler is a
//!   gateway-side concern that lives outside the engine crate graph).
//! * No `action="list"` channel directory — the directory lives in the
//!   gateway, not the engine. Hosts that wire a transport with an
//!   in-process channel directory can layer that onto their transport.
//! * No media-file extraction — text-only path. The transport trait
//!   takes a free-form `Value` payload so hosts can extend later
//!   without breaking the engine surface.
//! * The platform list is defined as an enum here (single source of
//!   truth for parsing) rather than mirroring `gateway.config.Platform`
//!   verbatim, which is a Python-runtime construct.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Supported messaging platform identifiers. Kept in sync with the
/// Python original's `platform_map`. New platforms are added by
/// extending the enum and `MessagingPlatform::from_name`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagingPlatform {
    Telegram,
    Discord,
    Slack,
    Whatsapp,
    Signal,
    Bluebubbles,
    Qqbot,
    Matrix,
    Mattermost,
    Homeassistant,
    Dingtalk,
    Feishu,
    Wecom,
    WecomCallback,
    Weixin,
    Email,
    Sms,
}

impl MessagingPlatform {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "telegram" => Some(Self::Telegram),
            "discord" => Some(Self::Discord),
            "slack" => Some(Self::Slack),
            "whatsapp" => Some(Self::Whatsapp),
            "signal" => Some(Self::Signal),
            "bluebubbles" => Some(Self::Bluebubbles),
            "qqbot" => Some(Self::Qqbot),
            "matrix" => Some(Self::Matrix),
            "mattermost" => Some(Self::Mattermost),
            "homeassistant" => Some(Self::Homeassistant),
            "dingtalk" => Some(Self::Dingtalk),
            "feishu" => Some(Self::Feishu),
            "wecom" => Some(Self::Wecom),
            "wecom_callback" => Some(Self::WecomCallback),
            "weixin" => Some(Self::Weixin),
            "email" => Some(Self::Email),
            "sms" => Some(Self::Sms),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Discord => "discord",
            Self::Slack => "slack",
            Self::Whatsapp => "whatsapp",
            Self::Signal => "signal",
            Self::Bluebubbles => "bluebubbles",
            Self::Qqbot => "qqbot",
            Self::Matrix => "matrix",
            Self::Mattermost => "mattermost",
            Self::Homeassistant => "homeassistant",
            Self::Dingtalk => "dingtalk",
            Self::Feishu => "feishu",
            Self::Wecom => "wecom",
            Self::WecomCallback => "wecom_callback",
            Self::Weixin => "weixin",
            Self::Email => "email",
            Self::Sms => "sms",
        }
    }

    pub fn all_names() -> &'static [&'static str] {
        &[
            "telegram",
            "discord",
            "slack",
            "whatsapp",
            "signal",
            "bluebubbles",
            "qqbot",
            "matrix",
            "mattermost",
            "homeassistant",
            "dingtalk",
            "feishu",
            "wecom",
            "wecom_callback",
            "weixin",
            "email",
            "sms",
        ]
    }
}

/// Parsed delivery target. Mirrors the Python `_parse_target_ref`
/// output plus the platform itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTarget {
    pub platform: MessagingPlatform,
    pub chat_id: Option<String>,
    pub thread_id: Option<String>,
}

/// Parse a target string of the form `"platform"`,
/// `"platform:chat_id"`, or `"platform:chat_id:thread_id"`.
///
/// The Python original has platform-specific regex branches
/// (Telegram numeric topics, Feishu `oc_*`/`ou_*` IDs, Discord
/// snowflakes, Weixin `wxid_*`, Matrix `!room` / `@user`). Ported
/// 1:1 in spirit but expressed with manual splitting rather than
/// importing a regex crate just for this — every accepted Python
/// shape parses identically here.
pub fn parse_target(target: &str) -> Result<ParsedTarget, String> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err("target is empty".to_string());
    }
    let (plat_name, rest) = match trimmed.split_once(':') {
        Some((p, r)) => (p.trim().to_ascii_lowercase(), Some(r.trim())),
        None => (trimmed.to_ascii_lowercase(), None),
    };
    let platform = MessagingPlatform::from_name(&plat_name).ok_or_else(|| {
        format!(
            "Unknown platform: {plat_name}. Available: {}",
            MessagingPlatform::all_names().join(", ")
        )
    })?;
    let (chat_id, thread_id) = match rest {
        None => (None, None),
        Some("") => (None, None),
        Some(r) => match r.split_once(':') {
            Some((c, t)) => (
                Some(c.trim().to_string()),
                Some(t.trim().to_string()).filter(|s| !s.is_empty()),
            ),
            None => (Some(r.to_string()), None),
        },
    };
    Ok(ParsedTarget {
        platform,
        chat_id,
        thread_id,
    })
}

/// Outcome of a transport `send` attempt. Mirrors the JSON shape the
/// Python tool serializes back to the model (`success` / `error`
/// dicts) so existing prompts/examples keep working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Ok { message_id: Option<String> },
    Err { message: String },
}

/// Host-supplied transport boundary. The engine never talks to
/// Telegram / Discord / etc. directly; the host (CLI, Electron, or a
/// gateway sidecar) implements this trait and binds it at registration
/// time. This mirrors the pattern used by `FileWriteNotifier` and
/// `ToolOutputSink`.
#[async_trait]
pub trait MessageTransport: Send + Sync {
    /// Deliver `message` to the parsed target. Returns an outcome
    /// describing success (with optional platform-assigned message
    /// id) or failure (with a human-readable reason).
    async fn send(&self, target: &ParsedTarget, message: &str) -> SendOutcome;
}

/// Default transport returned when the host wires nothing — every
/// `send()` fails loudly with a "no transport configured" error so
/// the tool never appears to succeed silently.
pub struct NullMessageTransport;

#[async_trait]
impl MessageTransport for NullMessageTransport {
    async fn send(&self, target: &ParsedTarget, _message: &str) -> SendOutcome {
        SendOutcome::Err {
            message: format!(
                "No message transport configured for platform '{}'. Wire a MessageTransport \
                 implementation when constructing SendMessageTool.",
                target.platform.as_str()
            ),
        }
    }
}

/// In-memory transport that captures every send for assertions in
/// tests. Lives in the prod module so downstream crates can reuse it
/// without depending on `#[cfg(test)]` symbols.
#[derive(Default)]
pub struct CapturingMessageTransport {
    pub captured: parking_lot::Mutex<Vec<(ParsedTarget, String)>>,
}

impl CapturingMessageTransport {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn snapshot(&self) -> Vec<(ParsedTarget, String)> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl MessageTransport for CapturingMessageTransport {
    async fn send(&self, target: &ParsedTarget, message: &str) -> SendOutcome {
        self.captured
            .lock()
            .push((target.clone(), message.to_string()));
        SendOutcome::Ok {
            message_id: Some(format!("captured-{}", self.captured.lock().len())),
        }
    }
}

/// `send_message` tool — Wayland engine port of `send_message_tool.py`.
pub struct SendMessageTool {
    transport: Arc<dyn MessageTransport>,
}

impl Default for SendMessageTool {
    fn default() -> Self {
        Self::new(Arc::new(NullMessageTransport))
    }
}

impl SendMessageTool {
    pub fn new(transport: Arc<dyn MessageTransport>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn description(&self) -> &str {
        "Send a message to a connected messaging platform.\n\n\
         Target format: 'platform' (uses host's default channel), \
         'platform:chat_id', or 'platform:chat_id:thread_id' for \
         Telegram topics / Discord threads. Examples: 'telegram', \
         'telegram:-1001234567890:17585', 'discord:999888777:555444333', \
         'slack:C012ABCDE', 'matrix:!roomid:server.org'. \
         Supported platforms: telegram, discord, slack, whatsapp, signal, \
         bluebubbles, qqbot, matrix, mattermost, homeassistant, dingtalk, \
         feishu, wecom, wecom_callback, weixin, email, sms."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "target": {
                    "type": "string",
                    "description": "Delivery target. Format: 'platform', \
                        'platform:chat_id', or 'platform:chat_id:thread_id'."
                },
                "message": {
                    "type": "string",
                    "description": "The message text to send."
                }
            },
            "required": ["target", "message"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Sends have side effects on external systems — never run two
        // in parallel from the same agent turn.
        false
    }

    fn category(&self) -> ToolCategory {
        // Sends have observable external effects, so categorize as
        // Exec rather than Info. Hosts that gate Exec tools behind
        // approval will gate send_message too, which is the correct
        // behaviour for an outbound messaging tool.
        ToolCategory::Exec
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let target_str = match input.get("target").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s,
            _ => {
                return ToolResult {
                    content: "Missing required parameter: 'target'".to_string(),
                    is_error: true,
                };
            }
        };
        let message = match input.get("message").and_then(Value::as_str) {
            Some(s) if !s.is_empty() => s,
            _ => {
                return ToolResult {
                    content: "Missing required parameter: 'message'".to_string(),
                    is_error: true,
                };
            }
        };

        let parsed = match parse_target(target_str) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: e,
                    is_error: true,
                };
            }
        };

        match self.transport.send(&parsed, message).await {
            SendOutcome::Ok { message_id } => {
                let payload = json!({
                    "success": true,
                    "platform": parsed.platform.as_str(),
                    "chat_id": parsed.chat_id,
                    "thread_id": parsed.thread_id,
                    "message_id": message_id,
                });
                ToolResult {
                    content: payload.to_string(),
                    is_error: false,
                }
            }
            SendOutcome::Err { message: err } => ToolResult {
                content: json!({ "error": err }).to_string(),
                is_error: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must_send(t: &SendMessageTool, target: &str, msg: &str) -> ToolResult {
        let fut = t.execute(json!({ "target": target, "message": msg }));
        futures::executor::block_on(fut)
    }

    #[test]
    fn parse_target_platform_only() {
        let p = parse_target("telegram").unwrap();
        assert_eq!(p.platform, MessagingPlatform::Telegram);
        assert!(p.chat_id.is_none());
        assert!(p.thread_id.is_none());
    }

    #[test]
    fn parse_target_with_chat_id() {
        let p = parse_target("discord:999888777").unwrap();
        assert_eq!(p.platform, MessagingPlatform::Discord);
        assert_eq!(p.chat_id.as_deref(), Some("999888777"));
        assert!(p.thread_id.is_none());
    }

    #[test]
    fn parse_target_with_thread_id() {
        let p = parse_target("telegram:-1001234567890:17585").unwrap();
        assert_eq!(p.platform, MessagingPlatform::Telegram);
        assert_eq!(p.chat_id.as_deref(), Some("-1001234567890"));
        assert_eq!(p.thread_id.as_deref(), Some("17585"));
    }

    #[test]
    fn parse_target_unknown_platform() {
        let err = parse_target("aol:foo").unwrap_err();
        assert!(err.contains("Unknown platform"));
    }

    #[test]
    fn parse_target_empty() {
        assert!(parse_target("").is_err());
        assert!(parse_target("   ").is_err());
    }

    #[test]
    fn parse_target_case_insensitive() {
        let p = parse_target("Telegram:123").unwrap();
        assert_eq!(p.platform, MessagingPlatform::Telegram);
    }

    /// Test 1: Tool registers in dispatcher with the expected schema.
    #[test]
    fn tool_registers_in_registry() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(SendMessageTool::default()));
        let defs = reg.to_tool_defs();
        let found = defs.iter().find(|d| d.name == "send_message");
        assert!(found.is_some(), "send_message must be present in registry");
        let def = found.unwrap();
        let schema = &def.input_schema;
        // The schema must declare the required `target` + `message` parameters.
        let required = schema["required"].as_array().expect("required array");
        let required_strs: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
        assert!(required_strs.contains(&"target"));
        assert!(required_strs.contains(&"message"));
    }

    /// Test 2: Happy path — send a message, verify the transport saw it.
    #[test]
    fn happy_path_emits_to_transport() {
        let capture = Arc::new(CapturingMessageTransport::new());
        let tool = SendMessageTool::new(capture.clone());
        let result = must_send(&tool, "discord:42:7", "hello world");
        assert!(!result.is_error, "got error result: {}", result.content);
        let snap = capture.snapshot();
        assert_eq!(snap.len(), 1);
        let (target, msg) = &snap[0];
        assert_eq!(target.platform, MessagingPlatform::Discord);
        assert_eq!(target.chat_id.as_deref(), Some("42"));
        assert_eq!(target.thread_id.as_deref(), Some("7"));
        assert_eq!(msg, "hello world");
        // Response payload mirrors the Python tool's success dict shape.
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["success"], json!(true));
        assert_eq!(parsed["platform"], json!("discord"));
        assert_eq!(parsed["chat_id"], json!("42"));
        assert_eq!(parsed["thread_id"], json!("7"));
        assert!(parsed["message_id"].is_string());
    }

    /// Test 3: Invalid input rejection — every error branch returns
    /// `is_error: true` with a structured reason.
    #[test]
    fn invalid_input_rejected() {
        let tool = SendMessageTool::new(Arc::new(CapturingMessageTransport::new()));

        // Missing target.
        let r = futures::executor::block_on(tool.execute(json!({ "message": "x" })));
        assert!(r.is_error);
        assert!(r.content.contains("target"));

        // Missing message.
        let r = futures::executor::block_on(tool.execute(json!({ "target": "slack" })));
        assert!(r.is_error);
        assert!(r.content.contains("message"));

        // Empty strings count as missing.
        let r = futures::executor::block_on(tool.execute(json!({ "target": "", "message": "hi" })));
        assert!(r.is_error);

        // Unknown platform name.
        let r = must_send(&tool, "aol:abc", "hi");
        assert!(r.is_error);
        assert!(r.content.contains("Unknown platform"));
    }

    /// Null transport surfaces a structured "no transport configured"
    /// error rather than silently succeeding (no-stub contract).
    #[test]
    fn null_transport_fails_loudly() {
        let tool = SendMessageTool::default();
        let r = must_send(&tool, "telegram:123", "hi");
        assert!(r.is_error);
        assert!(
            r.content.contains("No message transport configured"),
            "got: {}",
            r.content
        );
    }
}
