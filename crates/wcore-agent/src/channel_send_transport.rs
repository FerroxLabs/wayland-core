//! `ChannelManagerTransport` — bridges `wcore_tools::send_message::MessageTransport`
//! to `wcore_channels::ChannelManager`.
//!
//! FleetDispatcher-class fix (audit 2026-05-24): `SendMessageTool` was
//! registered at bootstrap with `NullMessageTransport`, so every LLM-
//! initiated `send_message` call returned the Null transport's loud
//! "No message transport configured for platform …" error. The host
//! already lifted `channel_manager` to `Arc<RwLock<ChannelManager>>` for
//! cron's `channel_sink`; this adapter exposes that same manager to the
//! send-message tool so the LLM can drive Telegram/Discord/Slack/etc.
//! through the same channel instances the user configured at
//! `~/.wayland/channels/*.toml`.
//!
//! Mapping convention: `ParsedTarget::platform` (one of the
//! `MessagingPlatform` enum's `as_str()` values: "telegram", "discord",
//! "slack", …) is resolved to a registered `ChannelManager` channel name
//! by platform FAMILY (see [`resolve_channel_name`]). Default-named
//! channels register under the platform token itself ("telegram"), so the
//! exact-match arm preserves the original behavior. Instance-named channels
//! register under a `platform-suffix` key — e.g. the IMAP email connector
//! registers as "email-imap" while its platform is "email" (issue #116) —
//! so the family arm maps the "email" token onto the registered
//! "email-imap"/"email-agentmail" instance. Without this, `send_to("email")`
//! missed and every IMAP email user's `send_message` failed.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use wcore_channels::ChannelManager;
use wcore_channels::outgoing::OutgoingMessage;
use wcore_tools::send_message::{MessageTransport, ParsedTarget, SendOutcome};

pub struct ChannelManagerTransport {
    mgr: Arc<RwLock<ChannelManager>>,
}

impl ChannelManagerTransport {
    pub fn new(mgr: Arc<RwLock<ChannelManager>>) -> Self {
        Self { mgr }
    }
}

/// Resolve a `send_message` platform token to a registered channel name.
///
/// Channels register under their instance name, which is not always the
/// platform token: the default convention uses the token itself ("telegram"),
/// but instance-named connectors use a `platform-suffix` key (the IMAP email
/// connector registers as "email-imap" with platform "email" — issue #116).
///
/// Resolution order:
/// 1. Exact match on the platform token (preserves default-named channels).
/// 2. Family match: the first registered channel whose name is the platform
///    token followed by the `-` instance separator (covers "email-imap"/
///    "email-agentmail" for "email"). The separator is required so a bare
///    prefix can't cross distinct platforms — e.g. "wecom" must NOT resolve
///    to a "wecom_callback" channel ('_', not '-'), and "email" must not
///    match an unrelated "emailfoo".
/// 3. No match: return the token unchanged so `send_to` yields its existing
///    "unknown channel" error.
fn resolve_channel_name(names: &[String], platform_token: &str) -> String {
    if names.iter().any(|n| n == platform_token) {
        return platform_token.to_string();
    }
    if let Some(name) = names.iter().find(|n| {
        n.strip_prefix(platform_token)
            .is_some_and(|r| r.starts_with('-'))
    }) {
        return name.clone();
    }
    platform_token.to_string()
}

#[async_trait]
impl MessageTransport for ChannelManagerTransport {
    async fn send(&self, target: &ParsedTarget, message: &str) -> SendOutcome {
        let platform_token = target.platform.as_str();
        let conversation_id = target.chat_id.clone().unwrap_or_default();
        let outgoing = OutgoingMessage {
            conversation_id,
            text: message.to_string(),
            reply_to: target.thread_id.clone(),
            attachments: Vec::new(),
        };
        let guard = self.mgr.read().await;
        // Channels register under their instance name (e.g. "email-imap"),
        // which may differ from the platform token ("email"). Resolve by
        // platform family before dispatching (issue #116).
        let channel_name = resolve_channel_name(&guard.list_names(), platform_token);
        match guard.send_to(&channel_name, outgoing).await {
            Ok(receipt) => SendOutcome::Ok {
                message_id: Some(receipt.id),
            },
            Err(e) => SendOutcome::Err {
                message: e.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_channels::MockChannel;
    use wcore_tools::send_message::MessagingPlatform;

    fn target(platform: MessagingPlatform, chat_id: &str) -> ParsedTarget {
        ParsedTarget {
            platform,
            chat_id: Some(chat_id.to_string()),
            thread_id: None,
        }
    }

    #[test]
    fn resolve_prefers_exact_name_then_family_prefix() {
        // Exact platform-token match wins (default-named channels).
        let names = vec!["email".to_string(), "email-imap".to_string()];
        assert_eq!(resolve_channel_name(&names, "email"), "email");

        // No exact token, but a family member exists: resolve to it.
        let names = vec!["telegram".to_string(), "email-imap".to_string()];
        assert_eq!(resolve_channel_name(&names, "email"), "email-imap");

        // Nothing in the family: return the token unchanged so send_to errors.
        let names = vec!["telegram".to_string()];
        assert_eq!(resolve_channel_name(&names, "email"), "email");

        // Separator guard: the family arm requires the platform token followed
        // by '-'. "wecom" and "wecom_callback" are DISTINCT platforms (the
        // separator is '_'), so a "wecom_callback" channel must NOT satisfy a
        // "wecom" target — that would re-introduce the cross-family misroute
        // this fix exists to prevent. Token returned unchanged → unknown channel.
        let names = vec!["wecom_callback".to_string()];
        assert_eq!(resolve_channel_name(&names, "wecom"), "wecom");

        // An unrelated name that merely shares the prefix without the separator
        // ("emailfoo") must not match either.
        let names = vec!["emailfoo".to_string()];
        assert_eq!(resolve_channel_name(&names, "email"), "email");
    }

    /// Issue #116: an email channel registered under its instance name
    /// ("email-imap") must be reachable when send_message targets the "email"
    /// platform token.
    #[tokio::test]
    async fn send_reaches_named_email_channel_via_platform_family() {
        let mut mgr = ChannelManager::new();
        // Registered under the instance name, NOT the bare platform token —
        // exactly what the desktop ChannelManager does for IMAP email.
        mgr.register(Box::new(MockChannel::new("email-imap"))).await;
        mgr.start_all().await.expect("start channels");
        let transport = ChannelManagerTransport::new(Arc::new(RwLock::new(mgr)));

        let outcome = transport
            .send(&target(MessagingPlatform::Email, "inbox@example.com"), "hi")
            .await;

        match outcome {
            SendOutcome::Ok { message_id } => assert!(message_id.is_some()),
            SendOutcome::Err { message } => panic!("expected Ok, got Err: {message}"),
        }
    }

    /// A genuinely absent platform still surfaces the unknown-channel error.
    #[tokio::test]
    async fn send_to_absent_platform_still_errors() {
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(MockChannel::new("telegram"))).await;
        let transport = ChannelManagerTransport::new(Arc::new(RwLock::new(mgr)));

        let outcome = transport
            .send(&target(MessagingPlatform::Email, "inbox@example.com"), "hi")
            .await;

        match outcome {
            SendOutcome::Err { message } => assert!(
                message.contains("unknown channel"),
                "expected unknown-channel error, got: {message}"
            ),
            SendOutcome::Ok { .. } => panic!("expected Err for an absent platform"),
        }
    }

    /// Cross-family guard (end-to-end): "wecom" and "wecom_callback" are
    /// distinct platforms. Targeting "wecom" with only a "wecom_callback"
    /// channel registered must NOT misroute to it — it must surface the
    /// unknown-channel error, the exact bug class this fix prevents.
    #[tokio::test]
    async fn send_to_wecom_does_not_misroute_to_wecom_callback() {
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(MockChannel::new("wecom_callback")))
            .await;
        mgr.start_all().await.expect("start channels");
        let transport = ChannelManagerTransport::new(Arc::new(RwLock::new(mgr)));

        let outcome = transport
            .send(&target(MessagingPlatform::Wecom, "room1"), "hi")
            .await;

        match outcome {
            SendOutcome::Err { message } => assert!(
                message.contains("unknown channel"),
                "expected unknown-channel error, got: {message}"
            ),
            SendOutcome::Ok { .. } => {
                panic!("wecom must NOT resolve to a wecom_callback channel")
            }
        }
    }
}
