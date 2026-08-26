//! `OutgoingMessage` — uniform outbound shape across platforms.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutgoingMessage {
    /// Channel / room / thread / DM identifier. Required.
    pub conversation_id: String,
    /// Message text. Required even when attachments are set; many
    /// platforms reject empty-body messages.
    pub text: String,
    /// Destination topic / thread within `conversation_id`, on platforms
    /// that model one (Telegram forum topics, Slack `thread_ts`).
    ///
    /// This is the DESTINATION, and it is deliberately separate from
    /// [`Self::reply_to`], which names a specific message being quoted. The
    /// two carry different id spaces on the same platform: a Telegram topic
    /// id is not a message id, so collapsing them makes every in-topic reply
    /// quote a message that does not exist (core#253 §5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Optional quoted-message target on platforms that support replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Optional attachments as URL / platform references. Channels
    /// upload bytes on demand.
    #[serde(default)]
    pub attachments: Vec<String>,
}

impl OutgoingMessage {
    /// Convenience constructor for text-only outbound.
    pub fn text(conversation_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            conversation_id: conversation_id.into(),
            text: text.into(),
            thread_id: None,
            reply_to: None,
            attachments: Vec::new(),
        }
    }
}
