//! `ChannelManagerTransport` — bridges `wcore_tools::send_message::MessageTransport`
//! to `wcore_channels::ChannelManager`.
//!
//! FleetDispatcher-class fix (audit 2026-05-24): `SendMessageTool` was
//! registered at bootstrap with `NullMessageTransport`, so every LLM-
//! initiated `send_message` call returned the Null transport's loud
//! "No message transport configured for platform …" error. The host
//! already lifted `channel_manager` to `Arc<Mutex<ChannelManager>>` for
//! cron's `channel_sink`; this adapter exposes that same manager to the
//! send-message tool so the LLM can drive Telegram/Discord/Slack/etc.
//! through the same channel instances the user configured at
//! `~/.wayland/channels/*.toml`.
//!
//! Mapping convention: `ParsedTarget::platform` (one of the
//! `MessagingPlatform` enum's `as_str()` values: "telegram", "discord",
//! "slack", …) is used directly as the `ChannelManager::send_to` channel
//! name. Operators register their channels under those platform names
//! (the default convention auto-`register` follows). When a user runs
//! multiple bots on the same platform, they MUST register them under
//! distinct names and the LLM must target the named instance — that
//! addressing layer is a separate piece of work.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use wcore_channels::ChannelManager;
use wcore_channels::outgoing::OutgoingMessage;
use wcore_tools::send_message::{MessageTransport, ParsedTarget, SendOutcome};

pub struct ChannelManagerTransport {
    mgr: Arc<Mutex<ChannelManager>>,
}

impl ChannelManagerTransport {
    pub fn new(mgr: Arc<Mutex<ChannelManager>>) -> Self {
        Self { mgr }
    }
}

#[async_trait]
impl MessageTransport for ChannelManagerTransport {
    async fn send(&self, target: &ParsedTarget, message: &str) -> SendOutcome {
        let channel_name = target.platform.as_str();
        let conversation_id = target.chat_id.clone().unwrap_or_default();
        let outgoing = OutgoingMessage {
            conversation_id,
            text: message.to_string(),
            reply_to: target.thread_id.clone(),
            attachments: Vec::new(),
        };
        let guard = self.mgr.lock().await;
        match guard.send_to(channel_name, outgoing).await {
            Ok(receipt) => SendOutcome::Ok {
                message_id: Some(receipt.id),
            },
            Err(e) => SendOutcome::Err {
                message: e.to_string(),
            },
        }
    }
}
