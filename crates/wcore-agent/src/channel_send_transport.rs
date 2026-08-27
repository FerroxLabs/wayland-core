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
//!
//! ## Rate limiting (wayland#585)
//!
//! This adapter is the seam the LLM-driven `send_message` tool reaches in the
//! default (engine-owned channel table) configuration, and it is throttled
//! here — NOT one layer lower in
//! [`ChannelManager::send_to`], which the human/operator and cron paths share
//! (pinned by `channel_inbound::tests::interactive_sends_bypass_the_rate_limit`
//! and by `operator_send_to_is_not_throttled_by_the_tool_limiter` below).
//!
//! [`wcore_channels::AutoReplyRateLimiter`] previously gated exactly one seam,
//! the `run_turn` auto-reply in `channel_inbound.rs`, so two agents wired to
//! the same channel could ping-pong forever *through the tool* while the guard
//! built for that failure never saw it. A suppressed send returns
//! [`SendOutcome::Err`], which `SendMessageTool` renders as an `is_error`
//! `ToolResult` the model actually reads — a `warn!` alone reaches nobody with
//! `RUST_LOG` unset, so it can never end a model-driven loop.
//!
//! **Coverage, stated plainly:** this is not the only `MessageTransport`.
//! Under `WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1` (the desktop) `bootstrap`
//! deliberately keeps [`crate::host_send_transport::HostDelegatedTransport`]
//! and never installs this adapter at all — so that transport carries its
//! OWN limiter, in this same shape, since the wayland#585 follow-up. Those
//! two are the only production transports that deliver, and both are now
//! throttled. The check still does NOT live in `SendMessageTool` itself, so
//! a third transport added later would start unthrottled.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use async_trait::async_trait;
use tokio::sync::RwLock;
use wcore_channels::outgoing::OutgoingMessage;
use wcore_channels::{
    AutoReplyRateLimiter, ChannelManager, DEFAULT_AUTO_REPLY_WINDOW, DEFAULT_CONVERSATION_CAP,
    DEFAULT_MAX_AUTO_REPLIES,
};
use wcore_tools::send_message::{
    MessageTransport, ParsedTarget, SendOutcome, THROTTLED_ERROR_PREFIX,
};

pub struct ChannelManagerTransport {
    mgr: Arc<RwLock<ChannelManager>>,
    /// Per-conversation rolling-window guard on TOOL-DRIVEN sends
    /// (wayland#585). Behind a `std::Mutex` because `MessageTransport::send`
    /// takes `&self` and the tool is shared across concurrent turns; the
    /// critical section is a bounded map op and the guard is dropped before
    /// any `.await`, so it never crosses a suspension point.
    rate_limiter: StdMutex<AutoReplyRateLimiter>,
}

impl ChannelManagerTransport {
    pub fn new(mgr: Arc<RwLock<ChannelManager>>) -> Self {
        Self {
            mgr,
            // Constructed from the named constants rather than `default()` so
            // the refusal message below cannot drift from the live budget.
            rate_limiter: StdMutex::new(AutoReplyRateLimiter::new(
                DEFAULT_MAX_AUTO_REPLIES,
                DEFAULT_AUTO_REPLY_WINDOW,
                DEFAULT_CONVERSATION_CAP,
            )),
        }
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
        self.deliver(target, message, None).await
    }

    /// Put the tool execution's durable idempotency key on the wire.
    ///
    /// [`ChannelManager::send_to_keyed`] is the only send that transmits it,
    /// and only an adapter declaring
    /// [`Channel::supports_outbound_idempotency`] actually forwards it to the
    /// platform — so this is a pass-through for every destination that has no
    /// idempotency surface, and the replay-suppressing send for the ones that
    /// do (Matrix, within its single-message cap). Without this override the
    /// trait default falls back to the unkeyed [`MessageTransport::send`] and
    /// the key the journal minted never leaves the process.
    async fn send_keyed(
        &self,
        target: &ParsedTarget,
        message: &str,
        idempotency_key: Option<&str>,
    ) -> SendOutcome {
        self.deliver(target, message, idempotency_key).await
    }
}

impl ChannelManagerTransport {
    async fn deliver(
        &self,
        target: &ParsedTarget,
        message: &str,
        idempotency_key: Option<&str>,
    ) -> SendOutcome {
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

        // wayland#585 — throttle the tool seam per conversation. Keyed on the
        // RESOLVED channel plus the conversation id, joined by an ASCII unit
        // separator so no channel name / conversation id pair can alias
        // another. A poisoned mutex is recovered rather than panicking: the
        // critical section is a bounded map op that cannot itself panic.
        let limit_key = format!("{channel_name}\u{1f}{}", outgoing.conversation_id);
        let allowed = {
            let mut limiter = self
                .rate_limiter
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            limiter.check_and_record(&limit_key, Instant::now())
        };
        if !allowed {
            // Content-free: channel and conversation only, never message text.
            tracing::warn!(
                target: "wcore_agent::channel_send_transport",
                channel = %channel_name,
                conversation = %outgoing.conversation_id,
                "send_message suppressed: per-conversation rate limit hit (ping-pong guard)"
            );
            // `THROTTLED_ERROR_PREFIX` is not decoration: `SendMessageTool`
            // matches on it to tag the tool result as caller-attributed, which
            // is what keeps a throttle from arming the row-B-3
            // human-unreachable freeze (see that constant's doc).
            return SendOutcome::Err {
                message: format!(
                    "{}this conversation has already sent {} messages through \
                     send_message within the last {} seconds, so further sends are \
                     suppressed to stop a runaway agent-to-agent reply loop. Stop sending \
                     to this conversation and report the situation instead of retrying; \
                     the budget refills as older sends age out of the window.",
                    THROTTLED_ERROR_PREFIX,
                    DEFAULT_MAX_AUTO_REPLIES,
                    DEFAULT_AUTO_REPLY_WINDOW.as_secs(),
                ),
            };
        }

        // `send_to` IS `send_to_keyed(.., None)`, so the unkeyed path through
        // here is byte-identical to what it always was; a `Some` key is the
        // only change in behaviour, and only at an adapter that declares it
        // transmits one.
        match guard
            .send_to_keyed(&channel_name, outgoing, idempotency_key)
            .await
        {
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
    use wcore_channels::{DEFAULT_MAX_AUTO_REPLIES, MockChannel};
    use wcore_tools::send_message::MessagingPlatform;

    fn target(platform: MessagingPlatform, chat_id: &str) -> ParsedTarget {
        ParsedTarget {
            platform,
            chat_id: Some(chat_id.to_string()),
            thread_id: None,
        }
    }

    /// A Matrix-shaped adapter: it declares that it transmits an idempotency
    /// key, and records which send arm the manager actually took.
    struct KeyRecordingChannel {
        keys: Arc<StdMutex<Vec<Option<String>>>>,
    }

    #[async_trait]
    impl wcore_channels::Channel for KeyRecordingChannel {
        fn name(&self) -> &str {
            "matrix"
        }

        fn platform(&self) -> &str {
            "matrix"
        }

        async fn start(&mut self) -> Result<(), wcore_channels::ChannelError> {
            Ok(())
        }

        async fn stop(&mut self) -> Result<(), wcore_channels::ChannelError> {
            Ok(())
        }

        async fn poll_events(
            &mut self,
        ) -> Result<Vec<wcore_channels::ChannelEvent>, wcore_channels::ChannelError> {
            Ok(Vec::new())
        }

        async fn send_message(
            &mut self,
            msg: wcore_channels::OutgoingMessage,
        ) -> Result<wcore_channels::MessageReceipt, wcore_channels::ChannelError> {
            self.keys.lock().unwrap().push(None);
            Ok(wcore_channels::MessageReceipt {
                id: "unkeyed".to_string(),
                conversation_id: msg.conversation_id,
                ts_secs: 0,
            })
        }

        async fn send_message_idempotent(
            &mut self,
            msg: wcore_channels::OutgoingMessage,
            key: &str,
        ) -> Result<wcore_channels::MessageReceipt, wcore_channels::ChannelError> {
            self.keys.lock().unwrap().push(Some(key.to_string()));
            Ok(wcore_channels::MessageReceipt {
                id: "keyed".to_string(),
                conversation_id: msg.conversation_id,
                ts_secs: 0,
            })
        }

        fn supports_outbound_idempotency(&self) -> bool {
            true
        }

        fn config_schema(&self) -> &str {
            r#"{"name": "string", "platform": "matrix"}"#
        }
    }

    async fn keyed_channel_transport()
    -> (ChannelManagerTransport, Arc<StdMutex<Vec<Option<String>>>>) {
        let keys = Arc::new(StdMutex::new(Vec::new()));
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(KeyRecordingChannel { keys: keys.clone() }))
            .await;
        mgr.start_all().await.expect("start channels");
        (
            ChannelManagerTransport::new(Arc::new(RwLock::new(mgr))),
            keys,
        )
    }

    /// The last hop. `SendMessageTool` hands the journal's key to the
    /// transport; this is where it either goes on the wire or is dropped.
    ///
    /// `ChannelManager::send_to` is `send_to_keyed(.., None)`, so a transport
    /// that calls `send_to` throws the key away silently — the send still
    /// succeeds and the destination has no way to recognise the replay. That
    /// was the state of this adapter, and it is the measured shape of the
    /// duplicate the F13 contract exists to prevent.
    #[tokio::test]
    async fn a_keyed_send_transmits_the_key_to_the_adapter() {
        let (transport, keys) = keyed_channel_transport().await;

        let outcome = transport
            .send_keyed(
                &target(MessagingPlatform::Matrix, "!room:server.org"),
                "hello",
                Some("idem-key-1"),
            )
            .await;

        match outcome {
            SendOutcome::Ok { message_id } => assert_eq!(message_id.as_deref(), Some("keyed")),
            SendOutcome::Err { message } => panic!("expected Ok, got Err: {message}"),
        }
        assert_eq!(
            keys.lock().unwrap().clone(),
            vec![Some("idem-key-1".to_string())],
            "the adapter must receive the key the journal minted"
        );
    }

    /// The negative arm: an unkeyed send must stay unkeyed. Without it the
    /// test above would pass against an adapter handed any key at all, and a
    /// fabricated key deduplicates real messages rather than replays.
    #[tokio::test]
    async fn an_unkeyed_send_stays_unkeyed() {
        let (transport, keys) = keyed_channel_transport().await;

        transport
            .send(
                &target(MessagingPlatform::Matrix, "!room:server.org"),
                "hello",
            )
            .await;

        assert_eq!(keys.lock().unwrap().clone(), vec![None]);
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

    // ------------------------------------------------------------------
    // wayland#585 — RED ARM. `send_message` bypasses the auto-reply limiter.
    //
    // `AutoReplyRateLimiter` gates exactly one seam: the `run_turn` auto-reply
    // in `channel_inbound.rs`. The LLM-driven `send_message` tool never touches
    // it — it lands here, in `ChannelManagerTransport::send`, which calls
    // `ChannelManager::send_to` with no throttle of any kind. So two agents
    // wired to the same channel ping-pong forever THROUGH THE TOOL, and the
    // guard that exists for precisely that failure never sees it.
    //
    // The check belongs in this transport and NOT in `ChannelManager::send_to`:
    // `send_to` is shared with the human/operator path, pinned by
    // `channel_inbound::tests::interactive_sends_bypass_the_rate_limit`.
    // ------------------------------------------------------------------

    /// A channel whose outbound log is SHARED with the test, so the number of
    /// messages that actually reached the wire can be asserted — not merely the
    /// outcome the transport reported. `MockChannel` owns its `sent` vec by
    /// value once boxed into the manager, which makes "was it suppressed, or
    /// sent and then reported as an error" unaskable.
    struct LoggingChannel {
        name: String,
        platform: String,
        started: bool,
        sent: SentLog,
        next_id: u64,
    }

    type SentLog = Arc<tokio::sync::Mutex<Vec<OutgoingMessage>>>;

    impl LoggingChannel {
        fn new(name: &str, platform: &str) -> (Self, SentLog) {
            let sent: SentLog = Arc::new(tokio::sync::Mutex::new(Vec::new()));
            (
                Self {
                    name: name.to_string(),
                    platform: platform.to_string(),
                    started: false,
                    sent: Arc::clone(&sent),
                    next_id: 0,
                },
                sent,
            )
        }
    }

    #[async_trait]
    impl wcore_channels::Channel for LoggingChannel {
        fn name(&self) -> &str {
            &self.name
        }
        fn platform(&self) -> &str {
            &self.platform
        }
        async fn start(&mut self) -> Result<(), wcore_channels::ChannelError> {
            self.started = true;
            Ok(())
        }
        async fn stop(&mut self) -> Result<(), wcore_channels::ChannelError> {
            self.started = false;
            Ok(())
        }
        async fn poll_events(
            &mut self,
        ) -> Result<Vec<wcore_channels::ChannelEvent>, wcore_channels::ChannelError> {
            Ok(Vec::new())
        }
        async fn send_message(
            &mut self,
            msg: OutgoingMessage,
        ) -> Result<wcore_channels::MessageReceipt, wcore_channels::ChannelError> {
            if !self.started {
                return Err(wcore_channels::ChannelError::NotStarted);
            }
            let id = format!("log-out-{}", self.next_id);
            self.next_id += 1;
            let receipt = wcore_channels::MessageReceipt {
                id,
                conversation_id: msg.conversation_id.clone(),
                ts_secs: 0,
            };
            self.sent.lock().await.push(msg);
            Ok(receipt)
        }
        fn config_schema(&self) -> &str {
            r#"{"name": "string", "platform": "mock"}"#
        }
    }

    async fn logging_transport(platform: &str) -> (ChannelManagerTransport, SentLog) {
        let (ch, sent) = LoggingChannel::new(platform, platform);
        let mut mgr = ChannelManager::new();
        mgr.register(Box::new(ch)).await;
        mgr.start_all().await.expect("start channels");
        (
            ChannelManagerTransport::new(Arc::new(RwLock::new(mgr))),
            sent,
        )
    }

    /// RED ARM 1 — the tool seam has no limiter at all.
    ///
    /// Drive ONE conversation past `DEFAULT_MAX_AUTO_REPLIES` through
    /// `ChannelManagerTransport::send`, the seam the `send_message` tool
    /// reaches. Every send past the cap must be refused and must never reach
    /// the wire. Today all 35 are delivered, which is the unbounded
    /// agent-to-agent loop the limiter was built to stop.
    #[tokio::test]
    async fn tool_driven_sends_are_rate_limited_per_conversation() {
        let (transport, sent) = logging_transport("slack").await;

        let cap = DEFAULT_MAX_AUTO_REPLIES;
        let over = 5usize;
        let mut allowed = 0usize;
        let mut refused = 0usize;
        for i in 0..cap + over {
            match transport
                .send(&target(MessagingPlatform::Slack, "c1"), &format!("m{i}"))
                .await
            {
                SendOutcome::Ok { .. } => allowed += 1,
                SendOutcome::Err { .. } => refused += 1,
            }
        }

        assert_eq!(
            allowed, cap,
            "the tool path must admit exactly the per-conversation budget"
        );
        assert_eq!(
            refused, over,
            "every send past the budget must be refused, not delivered"
        );
        assert_eq!(
            sent.lock().await.len(),
            cap,
            "a refused send must never reach the wire"
        );
    }

    /// RED ARM 2 — the throttle is per conversation, with its own control.
    ///
    /// The first half is red today (the over-budget send succeeds). The second
    /// half is the paired NEGATIVE CONTROL: a different conversation on the
    /// same channel must still send. Without it the first half could pass
    /// vacuously from a globally wedged transport or a mock that simply dies
    /// after N sends.
    #[tokio::test]
    async fn a_second_conversation_is_unaffected_by_the_first_ones_budget() {
        let (transport, sent) = logging_transport("slack").await;

        for i in 0..DEFAULT_MAX_AUTO_REPLIES {
            let outcome = transport
                .send(&target(MessagingPlatform::Slack, "c1"), &format!("m{i}"))
                .await;
            assert!(
                matches!(outcome, SendOutcome::Ok { .. }),
                "send {i} is within budget and must be delivered"
            );
        }
        match transport
            .send(&target(MessagingPlatform::Slack, "c1"), "over")
            .await
        {
            SendOutcome::Err { .. } => {}
            SendOutcome::Ok { .. } => {
                panic!("the send past the per-conversation budget must be refused")
            }
        }

        // NEGATIVE CONTROL: the channel and the transport are both healthy.
        match transport
            .send(
                &target(MessagingPlatform::Slack, "c2"),
                "fresh conversation",
            )
            .await
        {
            SendOutcome::Ok { .. } => {}
            SendOutcome::Err { message } => {
                panic!("a different conversation must not inherit c1's budget; got: {message}")
            }
        }
        assert_eq!(
            sent.lock().await.len(),
            DEFAULT_MAX_AUTO_REPLIES + 1,
            "c1's budget plus c2's one send reached the wire"
        );
    }

    /// RED ARM 3 — the suppression must reach the MODEL.
    ///
    /// A `warn!` reaches nobody with `RUST_LOG` unset, so a log line can never
    /// stop an agent-to-agent loop: the model just calls `send_message` again.
    /// The only surface that ends the loop is an `is_error` `ToolResult` the
    /// model actually reads. Paired positive control: the FIRST call must be a
    /// non-error result, so a blanket `is_error` cannot pass this test.
    #[tokio::test]
    async fn a_throttled_send_message_is_an_error_tool_result() {
        use wcore_tools::Tool;
        use wcore_tools::send_message::SendMessageTool;

        let (transport, _sent) = logging_transport("slack").await;
        let tool = SendMessageTool::new(Arc::new(transport));

        // POSITIVE CONTROL: an in-budget send is NOT an error result.
        let first = tool
            .execute(serde_json::json!({ "target": "slack:c1", "message": "m0" }))
            .await;
        assert!(
            !first.is_error,
            "an in-budget send must be a success result; got: {}",
            first.content
        );

        for i in 1..DEFAULT_MAX_AUTO_REPLIES {
            let _ = tool
                .execute(serde_json::json!({
                    "target": "slack:c1",
                    "message": format!("m{i}")
                }))
                .await;
        }

        let over = tool
            .execute(serde_json::json!({ "target": "slack:c1", "message": "over" }))
            .await;
        assert!(
            over.is_error,
            "a throttled send must reach the model as an is_error ToolResult; got: {}",
            over.content
        );
        assert!(
            over.content.to_ascii_lowercase().contains("rate limit"),
            "the model must be told WHY it was refused, not handed a bare failure; got: {}",
            over.content
        );
    }

    /// CONTROL for the two-seam decision — the human/operator path stays open.
    ///
    /// The limiter goes in this transport, NOT in `ChannelManager::send_to`,
    /// which the operator/human path shares (pinned by
    /// `channel_inbound::tests::interactive_sends_bypass_the_rate_limit`).
    /// After the TOOL budget for a conversation is fully spent, direct
    /// `send_to` for that same conversation must still be delivered. This test
    /// passes today and must keep passing: it is what fails if the check is
    /// put one layer too low.
    #[tokio::test]
    async fn operator_send_to_is_not_throttled_by_the_tool_limiter() {
        let (ch, sent) = LoggingChannel::new("slack", "slack");
        let mut inner = ChannelManager::new();
        inner.register(Box::new(ch)).await;
        inner.start_all().await.expect("start channels");
        let mgr = Arc::new(RwLock::new(inner));
        let transport = ChannelManagerTransport::new(Arc::clone(&mgr));

        for i in 0..DEFAULT_MAX_AUTO_REPLIES + 5 {
            let _ = transport
                .send(&target(MessagingPlatform::Slack, "c1"), &format!("m{i}"))
                .await;
        }
        let before = sent.lock().await.len();

        for i in 0..10 {
            mgr.read()
                .await
                .send_to(
                    "slack",
                    OutgoingMessage::text("c1", format!("operator-{i}")),
                )
                .await
                .expect("operator sends are never rate limited");
        }

        assert_eq!(
            sent.lock().await.len(),
            before + 10,
            "direct operator sends must bypass the tool-path limiter entirely"
        );
    }
}
