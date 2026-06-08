//! v0.8.1 U12 — A2A protocol value types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aHandshake {
    pub agent_id: String,
    /// "wayland-core" | "forge" | "hermes" | "openclaw" | "other"
    pub agent_kind: String,
    /// semver
    pub version: String,
    pub capabilities: A2aCapabilities,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct A2aCapabilities {
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub max_concurrent_tasks: u32,
    #[serde(default)]
    pub streaming_supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aMessage {
    pub from: String,
    pub to: String,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<A2aAttachment>,
    #[serde(default)]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aAttachment {
    pub mime_type: String,
    /// base64
    pub data: String,
    pub name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum A2aError {
    #[error("not implemented yet (v0.8.2 follow-up): {0}")]
    NotImplemented(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("handler error: {0}")]
    HandlerError(String),
}
