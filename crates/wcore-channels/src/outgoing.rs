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
    /// Optional DESTINATION thread/topic within `conversation_id`.
    ///
    /// This is where the message goes, and it is deliberately independent of
    /// [`Self::reply_to`], which is the specific message being quoted. The two
    /// were conflated before: a Telegram forum-topic destination was written
    /// into `reply_to`, so the adapter sent it as `reply_to_message_id` (a
    /// quote of the topic-creation message) and never sent
    /// `message_thread_id`, the field that actually selects the topic.
    /// Substituting one for the other is what issue #253 calls out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Optional reply-target: the specific message being quoted or replied to.
    /// NOT a destination — see [`Self::thread_id`].
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
