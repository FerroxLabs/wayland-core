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

/// Shape of the conversation an inbound message arrived in. Drives the
/// access gate (DM vs group policy) and the session-key composition in
/// the inbound dispatch kernel. `Direct` = 1:1 DM, `Group` = multi-user
/// room / group chat, `Channel` = broadcast / announcement surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ChatType {
    #[default]
    Direct,
    Group,
    Channel,
}

/// Coarse media class for a typed inbound attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    Document,
    #[default]
    Other,
}

/// How the bot came to be addressed in a group context. `None` (on the
/// owning message) means the bot was not addressed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MentionKind {
    /// Explicit platform mention (`@bot`).
    Native,
    /// The message is a reply to one of the bot's messages.
    ReplyToBot,
    /// The message quotes one of the bot's messages.
    QuotedBot,
    /// The bot is a participant in this thread.
    ThreadParticipant,
}

/// One typed inbound attachment. `url` / `path` resolve to bytes lazily;
/// channels fetch on demand (auth-gated, SSRF-guarded — see Phase 5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    /// Platform URL / reference for the media.
    pub url: String,
    /// Local filesystem path once downloaded (None until fetched).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// MIME type if the platform reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Coarse media class.
    #[serde(default)]
    pub kind: MediaKind,
    /// Transcribed text (audio/voice notes) once produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcribed: Option<String>,
}

impl Attachment {
    /// Construct a bare URL attachment with unknown type/kind.
    pub fn url(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Default::default()
        }
    }
}

/// One inbound message from a channel.
///
/// Carries the structured facts the inbound dispatch kernel needs to
/// dedup, gate (access control), and route (session key). Connectors
/// populate every field they can resolve from the platform payload and
/// leave the rest at their defaults.
///
/// Field-trust note: `reply_to_text` and any quoted body are UNTRUSTED
/// remote content and must never be folded into the system prompt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct IncomingMessage {
    /// Platform-assigned message ID. The dedup key (with platform +
    /// account).
    pub id: String,
    /// Chat / room / thread / DM identifier — the conversation this
    /// message belongs to (platform specific). This is the chat id.
    pub conversation_id: String,
    /// Human-facing author label. May be a display name; for stable
    /// identity use `sender_id`.
    pub author: String,
    /// Message text. Always present; rich content travels in
    /// `attachments`.
    pub text: String,
    /// Unix epoch seconds.
    pub ts_secs: i64,
    /// Typed file / media attachments. Channels resolve to bytes on
    /// demand.
    #[serde(default)]
    pub attachments: Vec<Attachment>,

    // ---- Sender identity ----
    /// Stable platform user id of the sender — the access-control /
    /// dedup key. Connectors MUST set this to the immutable id, not a
    /// display name. Defaults to `author` via [`IncomingMessage::new`].
    #[serde(default)]
    pub sender_id: String,
    /// Display name of the sender, if distinct from `sender_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_display: Option<String>,
    /// Platform handle (`@username`) of the sender, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_handle: Option<String>,
    /// Secondary stable id where a platform exposes a union (Signal
    /// UUID, Feishu `union_id`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_alt_id: Option<String>,
    /// Sender is a bot account.
    #[serde(default)]
    pub is_bot: bool,
    /// Message was authored by this bot's own identity (loop guard).
    #[serde(default)]
    pub is_self: bool,

    // ---- Chat context ----
    /// DM / group / channel.
    #[serde(default)]
    pub chat_type: ChatType,
    /// Human-facing chat / room name, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_name: Option<String>,
    /// Enclosing space — guild / workspace / team id, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<String>,
    /// Thread id within the chat, if this message is in a thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Parent chat id when this chat is nested (sub-channel/thread root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_chat_id: Option<String>,

    // ---- Account / platform routing ----
    /// Receiving bot identity for multi-account routing (which bot/token
    /// received this), if the connector tracks more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Platform tag (`"slack"`, `"telegram"`, …). Usually redundant with
    /// the owning channel's platform; carried for normalized handling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,

    // ---- Mention / addressing ----
    /// The bot was addressed by this message (native mention, reply,
    /// quote, or thread participation).
    #[serde(default)]
    pub was_mentioned: bool,
    /// How the bot was addressed, if `was_mentioned`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mention_kind: Option<MentionKind>,

    // ---- Reply / quote (UNTRUSTED remote content) ----
    /// The message id this message replies to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
    /// The body of the replied-to message, if the platform inlines it.
    /// UNTRUSTED — never folded into the system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_text: Option<String>,
}

impl IncomingMessage {
    /// Minimal constructor for the core fields. Enriched fields take
    /// their defaults; `sender_id` is seeded from `author` and SHOULD be
    /// overridden with the stable platform user id. Set the structured
    /// fields (`chat_type`, mention/reply/account, typed attachments)
    /// via struct-update after construction.
    pub fn new(
        id: impl Into<String>,
        conversation_id: impl Into<String>,
        author: impl Into<String>,
        text: impl Into<String>,
        ts_secs: i64,
    ) -> Self {
        let author = author.into();
        Self {
            id: id.into(),
            conversation_id: conversation_id.into(),
            sender_id: author.clone(),
            author,
            text: text.into(),
            ts_secs,
            ..Default::default()
        }
    }
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
///
/// `MessageReceived` carries the full enriched `IncomingMessage` and is
/// by far the dominant variant in real channel traffic; the lifecycle
/// variants are rare. Boxing the message to satisfy `large_enum_variant`
/// would add a heap allocation on the hot path to shrink a fixed buffer
/// by a negligible amount, so the lint is suppressed here deliberately.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
#[non_exhaustive]
pub enum ChannelEvent {
    MessageReceived { msg: IncomingMessage },
    ConnectionStateChanged { state: ConnectionState },
    AuthExpired { reason: String },
    PlatformWarning { message: String },
}
