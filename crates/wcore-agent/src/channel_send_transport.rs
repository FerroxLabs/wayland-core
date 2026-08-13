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
/// A channel's INSTANCE NAME and its PLATFORM are independent. `send_to` keys
/// on the name, but `send_message` targets carry the platform token, so the
/// two have to be bridged. The bridge used to be pure string guessing over the
/// name list — exact match on the token, else a `token-` prefix — which meant
/// an operator who named their email channel anything other than "email" or
/// "email-*" had no reachable outbound path at all. A channel configured as
/// `name = "mail"` / `platform = "email"` (the shape the product's own
/// `channel list` prints) resolved to the literal "mail"-less token "email"
/// and `send_to` answered "unknown channel: email", so the model was told the
/// email backend was not configured while it was configured and running.
///
/// `platform_members` fixes that: it is what the adapters themselves report
/// their platform to be (`ChannelManager::names_for_platform`), which is
/// authoritative in a way that a name never was.
///
/// Resolution order:
/// 1. A channel that reports this platform AND is named for it — the default
///    convention ("telegram"/"telegram"), preserved exactly.
/// 2. Any channel that reports this platform, first alphabetically. This is
///    what reaches `name = "mail"` / `platform = "email"`.
/// 3. Legacy name-only fallbacks, for adapters that report a platform string
///    other than the token: exact name match, then the `token-` family prefix
///    ("email-imap"/"email-agentmail" for "email" — issue #116). The separator
///    is required so a bare prefix can't cross distinct platforms: "wecom"
///    must NOT resolve to a "wecom_callback" channel ('_', not '-'), and
///    "email" must not match an unrelated "emailfoo".
/// 4. No match: return the token unchanged so `send_to` yields its existing
///    "unknown channel" error. Never a silent success.
fn resolve_channel_name(
    names: &[String],
    platform_members: &[String],
    platform_token: &str,
) -> String {
    if platform_members.iter().any(|n| n == platform_token) {
        return platform_token.to_string();
    }
    if let Some(name) = platform_members.first() {
        return name.clone();
    }
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
        // A channel's instance name is chosen by the operator and need not
        // resemble the platform token at all ("mail" for platform "email").
        // Ask the adapters what platform they are before falling back to any
        // name-shaped guess (issue #116 and the B-3 corpus row).
        let members = guard.names_for_platform(platform_token).await;
        let channel_name = resolve_channel_name(&guard.list_names(), &members, platform_token);
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

    /// The legacy name-only arms, exercised with NO platform metadata so the
    /// resolver has nothing but names to go on — the situation an adapter that
    /// reports a platform string other than the token leaves it in.
    #[test]
    fn resolve_prefers_exact_name_then_family_prefix() {
        let none: Vec<String> = Vec::new();

        // Exact platform-token match wins (default-named channels).
        let names = vec!["email".to_string(), "email-imap".to_string()];
        assert_eq!(resolve_channel_name(&names, &none, "email"), "email");

        // No exact token, but a family member exists: resolve to it.
        let names = vec!["telegram".to_string(), "email-imap".to_string()];
        assert_eq!(resolve_channel_name(&names, &none, "email"), "email-imap");

        // Nothing in the family: return the token unchanged so send_to errors.
        let names = vec!["telegram".to_string()];
        assert_eq!(resolve_channel_name(&names, &none, "email"), "email");

        // Separator guard: the family arm requires the platform token followed
        // by '-'. "wecom" and "wecom_callback" are DISTINCT platforms (the
        // separator is '_'), so a "wecom_callback" channel must NOT satisfy a
        // "wecom" target — that would re-introduce the cross-family misroute
        // this fix exists to prevent. Token returned unchanged → unknown channel.
        let names = vec!["wecom_callback".to_string()];
        assert_eq!(resolve_channel_name(&names, &none, "wecom"), "wecom");

        // An unrelated name that merely shares the prefix without the separator
        // ("emailfoo") must not match either.
        let names = vec!["emailfoo".to_string()];
        assert_eq!(resolve_channel_name(&names, &none, "email"), "email");
    }

    /// Corpus row B-3. An operator-named channel — `name = "mail"`,
    /// `platform = "email"`, exactly what the fixture writes and what the
    /// product's own `channel list` prints back — must be reachable from a
    /// `send_message` target of `email:...`.
    ///
    /// Before the platform-metadata arm existed this resolved to the bare
    /// token "email", `send_to` answered "unknown channel: email", and the
    /// agent reported to the user that the email backend was not configured
    /// while a healthy, started email channel sat one lookup away. The
    /// approval request for a dangerous change was never sent as a result.
    #[test]
    fn resolve_reaches_an_operator_named_channel_by_platform() {
        let names = vec!["mail".to_string()];
        let members = vec!["mail".to_string()];
        assert_eq!(resolve_channel_name(&names, &members, "email"), "mail");
    }

    /// Platform metadata must not override a channel that is BOTH named for
    /// the platform and reports it — the default convention stays exact.
    #[test]
    fn resolve_prefers_the_platform_named_member() {
        let names = vec!["email".to_string(), "mail".to_string()];
        let members = vec!["email".to_string(), "mail".to_string()];
        assert_eq!(resolve_channel_name(&names, &members, "email"), "email");
    }

    /// Metadata is authoritative over a name collision: a channel merely
    /// NAMED "email" that is not an email channel must not win over the one
    /// that reports the platform.
    #[test]
    fn resolve_ignores_a_name_collision_from_another_platform() {
        // "email" here is a Telegram channel an operator named badly; "mail"
        // is the real email adapter.
        let names = vec!["email".to_string(), "mail".to_string()];
        let members = vec!["mail".to_string()];
        assert_eq!(resolve_channel_name(&names, &members, "email"), "mail");
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

    /// Corpus row B-3, end to end through the real `ChannelManager`.
    ///
    /// The fixture writes `name = "mail"` / `platform = "email"` and the
    /// product's own `channel list` prints it back as `mail  email  enabled`.
    /// A `send_message` target of `email:oncall@fixture.local` must reach it.
    ///
    /// Pre-fix, `resolve_channel_name` saw only the name list `["mail"]`,
    /// matched neither "email" nor "email-*", handed the bare token to
    /// `send_to`, and got `unknown channel: email` — which the agent reported
    /// to the user as "the email backend isn't configured" while a started,
    /// healthy email channel sat one lookup away. That is why no approval
    /// request was ever delivered on either platform.
    #[tokio::test]
    async fn send_reaches_an_operator_named_channel_by_platform() {
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(MockChannel::new("mail").with_platform("email")))
            .await;
        mgr.start_all().await.expect("start channels");
        let transport = ChannelManagerTransport::new(Arc::new(RwLock::new(mgr)));

        let outcome = transport
            .send(
                &target(MessagingPlatform::Email, "oncall@fixture.local"),
                "approval needed: moneykit 2.0.0",
            )
            .await;

        match outcome {
            SendOutcome::Ok { message_id } => assert!(message_id.is_some()),
            SendOutcome::Err { message } => {
                panic!("an operator-named email channel must be reachable; got: {message}")
            }
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
