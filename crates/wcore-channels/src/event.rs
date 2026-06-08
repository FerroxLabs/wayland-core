//! `ChannelEvent` — uniform event shape across platforms.

use serde::{Deserialize, Serialize};

/// Connection state for a channel. Surfaces through
/// `ChannelEvent::ConnectionStateChanged` so the UI can show online
/// indicators per channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    AuthError,
}

/// One inbound message from a channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IncomingMessage {
    /// Platform-assigned message ID.
    pub id: String,
    /// Channel / room / thread / DM identifier — platform specific.
    pub conversation_id: String,
    /// Author identifier (platform user id or display name —
    /// per-platform).
    pub author: String,
    /// Message text. Always present; rich content travels in
    /// `attachments`.
    pub text: String,
    /// Unix epoch seconds.
    pub ts_secs: i64,
    /// Optional file / media attachments. Each is a URL or platform
    /// reference; channels resolve to bytes on demand.
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// Receipt returned by `Channel::send_message` after the platform
/// accepts the outbound. The `id` is the platform-assigned message
/// id; callers correlate with later inbound echoes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessageReceipt {
    pub id: String,
    pub conversation_id: String,
    pub ts_secs: i64,
}

/// Events surface from a `Channel` via `poll_events()`. Non-exhaustive
/// so new variants don't break consumers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChannelEvent {
    MessageReceived { msg: IncomingMessage },
    ConnectionStateChanged { state: ConnectionState },
    AuthExpired { reason: String },
    PlatformWarning { message: String },
}
