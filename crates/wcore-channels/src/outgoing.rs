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
    /// Optional reply-target on platforms that support threading.
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
            reply_to: None,
            attachments: Vec::new(),
        }
    }
}
