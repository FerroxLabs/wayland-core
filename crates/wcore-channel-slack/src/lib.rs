//! `wcore-channel-slack` — production Slack adapter implementing the
//! `wcore_channels::Channel` trait.
//!
//! Outbound: Web API `chat.postMessage` with bearer auth, retry +
//! exponential backoff + jitter, `Retry-After` honoured on HTTP 429,
//! permanent-error short-circuit on 4xx + known Slack error codes.
//!
//! Inbound: Slack Events API webhooks. The engine's webhook router
//! invokes `SlackChannel::ingest_event(raw_body, signature, timestamp)`
//! when a POST hits `/channels/slack/<name>/webhook`. The adapter
//! verifies the HMAC-SHA256 signature, checks the timestamp falls
//! within a 5-minute replay window, parses the JSON envelope, and
//! enqueues a `ChannelEvent` for the next `poll_events()`.
//!
//! Secrets (bot token, signing secret) are resolved at `start()` time
//! from the `CredentialsStore`. The TOML config carries credential
//! handles only.

pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod inbound;

/// The single source of this adapter's inbound media bounds.
///
/// [`Channel::media_bounds`] returns this, and [`api::download_file`] caps the
/// streamed body at `MEDIA_BOUNDS.max_bytes`. One constant, both sites, so the
/// advertised number and the enforced number cannot drift apart.
///
/// This adapter previously declared NOTHING, so it advertised the 25 MiB trait
/// default while enforcing a hardcoded 100 MiB — a 4x gap nobody could see,
/// because the declaration had no reader anywhere in the workspace. 100 MiB is
/// the value that has actually governed inbound fetches since 2026-06-12.
pub const MEDIA_BOUNDS: wcore_channels::MediaBounds = wcore_channels::MediaBounds {
    max_bytes: 100 * 1024 * 1024,
    max_attachments: 10,
};

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;
use wcore_channels::{
    Channel, ChannelError, WebhookRequest, WebhookResponse,
    event::{ChannelEvent, ConnectionState, MessageReceipt},
    outgoing::OutgoingMessage,
};
use wcore_config::credentials::CredentialsStore;

pub use config::SlackConfig;
pub use error::SlackError;

/// Production Slack adapter.
///
/// One instance per workspace. Lifecycle:
///   construct (`new`) →
///   `start()` (resolves secrets from CredentialsStore) →
///   loop `poll_events` / `send_message` →
///   `stop()` (drops cached secrets).
pub struct SlackChannel {
    name: String,
    config: SlackConfig,
    state: ConnectionState,
    bot_token: Option<String>,
    signing_secret: Option<String>,
    http: wcore_egress::EgressClient,
    credentials: Arc<dyn CredentialsStore>,
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
}

impl SlackChannel {
    /// Construct a new adapter. `credentials` is the store the bot token
    /// + signing secret are pulled from at `start()`.
    pub fn new(
        name: impl Into<String>,
        config: SlackConfig,
        credentials: Arc<dyn CredentialsStore>,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            bot_token: None,
            signing_secret: None,
            http: wcore_egress::EgressClient::new(),
            credentials,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Construct with a caller-supplied `reqwest::Client` (tests use
    /// this to drive a mockito server with a short timeout).
    pub fn with_http_client(
        name: impl Into<String>,
        config: SlackConfig,
        credentials: Arc<dyn CredentialsStore>,
        http: wcore_egress::EgressClient,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            bot_token: None,
            signing_secret: None,
            http,
            credentials,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Read-only accessor for the cached connection state. Useful for
    /// UI surfaces that poll without going through `poll_events`.
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// Webhook-router entrypoint.
    ///
    /// Called by the engine's HTTP host when a Slack Events API POST
    /// lands at the channel's configured webhook URL. Verifies signature
    /// + timestamp, parses the body, and either enqueues a `ChannelEvent`
    ///   for the next `poll_events()` or surfaces the challenge string
    ///   from `url_verification` (`Ok(Some(challenge))`).
    pub async fn ingest_event(
        &self,
        raw_body: &str,
        signature: &str,
        timestamp: &str,
    ) -> Result<Option<String>, SlackError> {
        let signing_secret = self.signing_secret.as_deref().ok_or_else(|| {
            SlackError::Auth("signing secret not loaded — call start() first".to_string())
        })?;

        auth::verify_timestamp(timestamp, Utc::now().timestamp())?;
        auth::verify_signature(signing_secret, timestamp, raw_body, signature)?;

        match inbound::parse_webhook(raw_body)? {
            inbound::Parsed::Challenge(c) => Ok(Some(c)),
            inbound::Parsed::Event(ev) => {
                // F9 — bounded, drop-oldest inbox against a flood.
                let mut guard = self.inbox.lock().await;
                wcore_channels::push_bounded(&mut guard, ev);
                Ok(None)
            }
            inbound::Parsed::Ignored => Ok(None),
        }
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "slack"
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.state == ConnectionState::Connected {
            return Ok(());
        }
        self.state = ConnectionState::Connecting;

        // Resolve secrets from the credentials store.
        let bot_token = self
            .credentials
            .get(&self.config.credential_handle_bot_token)
            .map_err(|e| SlackError::Credentials(e.to_string()))?
            .ok_or_else(|| {
                SlackError::Credentials(format!(
                    "no value for credential handle {:?}",
                    self.config.credential_handle_bot_token
                ))
            })?;
        let signing_secret = self
            .credentials
            .get(&self.config.credential_handle_signing_secret)
            .map_err(|e| SlackError::Credentials(e.to_string()))?
            .ok_or_else(|| {
                SlackError::Credentials(format!(
                    "no value for credential handle {:?}",
                    self.config.credential_handle_signing_secret
                ))
            })?;

        self.bot_token = Some(bot_token);
        self.signing_secret = Some(signing_secret);
        self.state = ConnectionState::Connected;

        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connected,
            });
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        if self.state == ConnectionState::Disconnected {
            return Ok(());
        }
        self.bot_token = None;
        self.signing_secret = None;
        self.state = ConnectionState::Disconnected;
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Disconnected,
            });
        Ok(())
    }

    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        // Drain regardless of state — pending events queued before
        // stop() should still surface to the consumer.
        let mut inbox = self.inbox.lock().await;
        if inbox.is_empty() && self.state != ConnectionState::Connected {
            return Err(ChannelError::NotStarted);
        }
        Ok(inbox.drain(..).collect())
    }

    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        self.post(msg, None).await
    }

    async fn send_message_idempotent(
        &mut self,
        msg: OutgoingMessage,
        key: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        self.post(msg, Some(key)).await
    }

    /// This adapter DOES transmit the key (see [`Self::post`]), so the delivery
    /// spine is allowed to retry an outcome-unknown delivery through it.
    ///
    /// Returning `true` here is a claim the wire has to back: if the header
    /// stopped being sent, the spine would keep retrying and every retry would
    /// duplicate. `slack_declares_idempotency_only_because_it_sends_the_header`
    /// is the test that binds the two together.
    fn supports_outbound_idempotency(&self) -> bool {
        true
    }

    fn config_schema(&self) -> &str {
        include_str!("../schemas/slack.json")
    }

    /// Slack caps a single message around 40k characters; 39k is conservative.
    fn max_message_len(&self) -> Option<usize> {
        Some(39_000)
    }

    /// `reactions.add` — the ack signal. `message_id` is the Slack message
    /// `ts`. Slack takes an emoji *shortcode*, not a unicode glyph, so the
    /// ack emoji is mapped; an unmapped emoji is rejected (skipped by the
    /// caller). Note: Slack has no bot-usable typing API, so `send_typing`
    /// deliberately keeps the trait's no-op default.
    async fn react(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;
        let name = api::slack_emoji_name(emoji).ok_or_else(|| {
            ChannelError::Rejected(format!("no slack shortcode for emoji {emoji}"))
        })?;
        let req = api::AddReactionRequest {
            channel: conversation_id.to_string(),
            timestamp: message_id.to_string(),
            name: name.to_string(),
        };
        api::add_reaction(&self.http, &self.config.api_base_url, bot_token, &req)
            .await
            .map_err(ChannelError::from)
    }

    /// Slack: `chat.update` and `chat.delete` are real, `reactions.add` is real,
    /// and there is **no bot-usable typing API** — the `users.setPresence`
    /// surface is a user-token affordance, not a per-conversation typing
    /// indicator a bot token can drive. So typing is a permanent absence, not a
    /// backlog item, and it is recorded as one.
    fn native_actions(&self) -> wcore_channels::NativeActions {
        use wcore_channels::ActionSupport::*;
        wcore_channels::NativeActions::none()
            .edit(Implemented)
            .delete(Implemented)
            .react(Implemented)
            .typing(PlatformHasNoApi)
            .note("typing: Slack exposes no bot-token typing indicator")
    }

    /// `chat.update` — see [`api::update_message`]. `message_id` is the Slack
    /// message `ts`, exactly as it arrives in a send receipt.
    async fn edit_message(
        &self,
        conversation_id: &str,
        message_id: &str,
        new_text: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;
        let req = api::UpdateMessageRequest {
            channel: conversation_id.to_string(),
            ts: message_id.to_string(),
            text: new_text.to_string(),
        };
        let resp = api::update_message(&self.http, &self.config.api_base_url, bot_token, &req)
            .await
            .map_err(ChannelError::from)?;
        let ts = resp.ts.unwrap_or_else(|| message_id.to_string());
        let secs: i64 = ts
            .split('.')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Ok(MessageReceipt {
            id: ts,
            conversation_id: resp.channel.unwrap_or_else(|| conversation_id.to_string()),
            ts_secs: secs,
        })
    }

    /// `chat.delete` — see [`api::delete_message`].
    async fn delete_message(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<(), ChannelError> {
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;
        let req = api::DeleteMessageRequest {
            channel: conversation_id.to_string(),
            ts: message_id.to_string(),
        };
        api::delete_message(&self.http, &self.config.api_base_url, bot_token, &req)
            .await
            .map(|_| ())
            .map_err(ChannelError::from)
    }

    async fn fetch_media(
        &self,
        attachment: &wcore_channels::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;
        api::download_file(&self.http, &attachment.url, bot_token, api::MEDIA_HOSTS)
            .await
            .map_err(ChannelError::from)
    }

    /// This adapter's inbound intake policy — see [`MEDIA_BOUNDS`], which is
    /// the same constant [`api::download_file`] caps the streamed body at.
    fn media_bounds(&self) -> wcore_channels::MediaBounds {
        MEDIA_BOUNDS
    }

    /// Verify a Slack Events API POST and enqueue any resulting event.
    ///
    /// Pulls the `X-Slack-Signature` + `X-Slack-Request-Timestamp` headers
    /// the platform sends, then delegates to [`Self::ingest_event`] (which
    /// runs the signing-secret HMAC + timestamp window). A
    /// `url_verification` challenge surfaces as a `200` echoing the
    /// challenge string; everything else is an empty `200`.
    async fn ingest_webhook(&self, req: &WebhookRequest) -> Result<WebhookResponse, ChannelError> {
        let (sig, ts) = match (
            req.header("x-slack-signature"),
            req.header("x-slack-request-timestamp"),
        ) {
            (Some(sig), Some(ts)) => (sig, ts),
            _ => {
                return Err(ChannelError::Auth("missing slack signature headers".into()));
            }
        };
        match self.ingest_event(&req.body, sig, ts).await {
            Ok(Some(challenge)) => Ok(WebhookResponse::challenge(challenge)),
            Ok(None) => Ok(WebhookResponse::ok()),
            Err(e) => Err(ChannelError::Rejected(e.to_string())),
        }
    }
}

impl SlackChannel {
    async fn post(
        &mut self,
        msg: OutgoingMessage,
        idempotency_key: Option<&str>,
    ) -> Result<MessageReceipt, ChannelError> {
        if self.state != ConnectionState::Connected {
            return Err(ChannelError::NotStarted);
        }
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;

        let conversation_id = if msg.conversation_id.is_empty() {
            if self.config.default_channel_id.is_empty() {
                return Err(ChannelError::Rejected(
                    "no conversation_id and no default_channel_id configured".to_string(),
                ));
            }
            self.config.default_channel_id.clone()
        } else {
            msg.conversation_id.clone()
        };

        let req = api::PostMessageRequest {
            channel: conversation_id.clone(),
            text: msg.text.clone(),
            thread_ts: msg.reply_to.clone(),
        };

        let resp = api::post_message_keyed(
            &self.http,
            &self.config.api_base_url,
            bot_token,
            &req,
            self.config.max_retry_attempts,
            idempotency_key,
        )
        .await
        .map_err(ChannelError::from)?;

        let ts = resp
            .ts
            .ok_or_else(|| ChannelError::Rejected("slack response missing ts".to_string()))?;
        let secs: i64 = ts
            .split('.')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        Ok(MessageReceipt {
            id: ts,
            conversation_id: resp.channel.unwrap_or(conversation_id),
            ts_secs: secs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use wcore_config::credentials::CredentialsError;

    /// In-memory CredentialsStore for tests.
    pub(crate) struct MapStore {
        inner: StdMutex<std::collections::HashMap<String, String>>,
    }

    impl MapStore {
        pub fn new(entries: &[(&str, &str)]) -> Arc<Self> {
            let mut m = std::collections::HashMap::new();
            for (k, v) in entries {
                m.insert((*k).to_string(), (*v).to_string());
            }
            Arc::new(Self {
                inner: StdMutex::new(m),
            })
        }
    }

    impl CredentialsStore for MapStore {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            Ok(self.inner.lock().unwrap().get(key).cloned())
        }
        fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
            self.inner
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), CredentialsError> {
            self.inner.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn cfg_for(server_url: &str) -> SlackConfig {
        SlackConfig::new_for_test(server_url)
    }

    fn store_for_test() -> Arc<MapStore> {
        MapStore::new(&[
            ("slack.test.bot_token", "xoxb-test-token"),
            ("slack.test.signing_secret", "shhh"),
        ])
    }

    #[tokio::test]
    async fn send_message_hits_chat_postmessage_with_bearer() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .match_header("authorization", "Bearer xoxb-test-token")
            .match_header(
                "content-type",
                mockito::Matcher::Regex("application/json.*".to_string()),
            )
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "channel": "C1",
                "text": "hello"
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"1234.567","channel":"C1"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("C1", "hello"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "1234.567");
        assert_eq!(receipt.conversation_id, "C1");
        assert_eq!(receipt.ts_secs, 1234);

        mock.assert_async().await;
    }

    /// The capability declaration and the wire must agree.
    ///
    /// `supports_outbound_idempotency()` returning `true` is what permits the
    /// gateway's delivery spine to retry a delivery whose outcome is unknown.
    /// If that claim were true while the header was not actually sent, every
    /// such retry would become a second message at the destination — the exact
    /// duplicate measured on 2026-07-27 against an independent sink. This test
    /// binds the two: the mock matches on the header, so dropping it from
    /// `post_message_keyed` reddens here rather than only in a live run.
    #[tokio::test]
    async fn slack_declares_idempotency_only_because_it_sends_the_header() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .match_header("idempotency-key", "cron:job-a:1785121776528")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"1.0","channel":"C1"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        assert!(
            ch.supports_outbound_idempotency(),
            "slack claims it can deduplicate a replay"
        );
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        ch.send_message_idempotent(
            OutgoingMessage::text("C1", "hello"),
            "cron:job-a:1785121776528",
        )
        .await
        .unwrap();

        mock.assert_async().await;
    }

    /// An unkeyed send must NOT carry the header, or every ordinary message
    /// would present a key the destination could collapse against.
    #[tokio::test]
    async fn an_unkeyed_send_carries_no_idempotency_header() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .match_header("idempotency-key", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"1.0","channel":"C1"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();
        ch.send_message(OutgoingMessage::text("C1", "hello"))
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_retries_on_5xx() {
        let mut server = mockito::Server::new_async().await;
        let fail = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(503)
            .with_body("upstream")
            .expect(1)
            .create_async()
            .await;
        let succeed = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"42.0","channel":"C1"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("C1", "hi"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "42.0");

        fail.assert_async().await;
        succeed.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_honours_retry_after_on_429() {
        let mut server = mockito::Server::new_async().await;
        let throttled = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(429)
            .with_header("Retry-After", "0")
            .with_body("rate-limited")
            .expect(1)
            .create_async()
            .await;
        let succeed = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"99.0","channel":"C1"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("C1", "hi"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "99.0");

        throttled.assert_async().await;
        succeed.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_4xx_is_permanent() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(401)
            .with_body("invalid_auth")
            .expect(1)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let err = ch
            .send_message(OutgoingMessage::text("C1", "hi"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_ok_false_invalid_auth_surfaces_as_auth_error() {
        // Slack 200-with-ok:false surface for permanent auth failure.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"invalid_auth"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let err = ch
            .send_message(OutgoingMessage::text("C1", "hi"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn ingest_event_valid_signature_enqueues_message() {
        let cfg = cfg_for("https://unused.example");
        let store = store_for_test();
        let mut ch = SlackChannel::new("test", cfg, store);
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let body = r#"{"type":"event_callback","event":{"type":"message","channel":"C1","user":"U1","text":"hi","ts":"1700000000.000100"}}"#;
        let ts = Utc::now().timestamp().to_string();
        let sig = auth::expected_signature("shhh", &ts, body);

        let out = ch.ingest_event(body, &sig, &ts).await.unwrap();
        assert!(out.is_none(), "no challenge expected for event_callback");

        let evs = ch.poll_events().await.unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.text, "hi");
                assert_eq!(msg.author, "U1");
                assert_eq!(msg.conversation_id, "C1");
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ingest_event_invalid_signature_errors() {
        let mut ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        ch.start().await.unwrap();

        let body = r#"{"type":"event_callback","event":{"type":"message","channel":"C1","user":"U1","text":"hi","ts":"1700000000.000100"}}"#;
        let ts = Utc::now().timestamp().to_string();
        // Wrong signature.
        let err = ch.ingest_event(body, "v0=deadbeef", &ts).await.unwrap_err();
        assert!(matches!(err, SlackError::SignatureMismatch));
    }

    #[tokio::test]
    async fn ingest_event_stale_timestamp_errors() {
        let mut ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        ch.start().await.unwrap();

        let body = r#"{"type":"event_callback","event":{"type":"message","channel":"C1","user":"U1","text":"hi","ts":"1700000000.000100"}}"#;
        // 1 hour ago — outside the 5-minute replay window.
        let stale_ts = (Utc::now().timestamp() - 3600).to_string();
        let sig = auth::expected_signature("shhh", &stale_ts, body);

        let err = ch.ingest_event(body, &sig, &stale_ts).await.unwrap_err();
        assert!(matches!(err, SlackError::StaleTimestamp(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn ingest_event_url_verification_surfaces_challenge() {
        let mut ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        ch.start().await.unwrap();

        let body = r#"{"type":"url_verification","challenge":"hello-world","token":"x"}"#;
        let ts = Utc::now().timestamp().to_string();
        let sig = auth::expected_signature("shhh", &ts, body);

        let out = ch.ingest_event(body, &sig, &ts).await.unwrap();
        assert_eq!(out.as_deref(), Some("hello-world"));
    }

    #[tokio::test]
    async fn config_schema_is_valid_json() {
        let ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        let parsed: serde_json::Value =
            serde_json::from_str(ch.config_schema()).expect("schema parses");
        assert_eq!(parsed["title"].as_str(), Some("SlackChannelConfig"));
    }

    #[tokio::test]
    async fn max_message_len_is_slack_cap() {
        let ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        assert_eq!(ch.max_message_len(), Some(39_000));
    }

    #[tokio::test]
    async fn start_missing_bot_token_errors() {
        let store = MapStore::new(&[("slack.test.signing_secret", "shhh")]);
        let mut ch = SlackChannel::new("test", cfg_for("https://unused.example"), store);
        let err = ch.start().await.unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        assert_eq!(ch.state(), ConnectionState::Connecting);
    }

    // ---- native actions: edit / delete (Phase 24 C3) ----------------------

    /// The edit reaches `chat.update` with the bearer, the `ts` and the new
    /// text — asserted on the WIRE, not on the return value.
    ///
    /// `mock.assert_async()` is the load-bearing line: without it the test
    /// would pass against an adapter that never issued a request at all, which
    /// is precisely the shape a defaulted `Unsupported` would produce if the
    /// override were removed and the error swallowed.
    #[tokio::test]
    async fn edit_hits_chat_update_with_bearer_ts_and_text() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.update")
            .match_header("authorization", "Bearer xoxb-test-token")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "channel": "C1",
                "ts": "1234.567",
                "text": "edited body"
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"1234.567","channel":"C1"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();

        let receipt = ch
            .edit_message("C1", "1234.567", "edited body")
            .await
            .expect("edit succeeds");
        assert_eq!(receipt.id, "1234.567");
        assert_eq!(receipt.conversation_id, "C1");
        assert_eq!(receipt.ts_secs, 1234);

        mock.assert_async().await;
    }

    /// The delete reaches `chat.delete` with `channel` + `ts` and nothing else.
    #[tokio::test]
    async fn delete_hits_chat_delete_with_channel_and_ts() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.delete")
            .match_header("authorization", "Bearer xoxb-test-token")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "channel": "C1",
                "ts": "1234.567"
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"1234.567","channel":"C1"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();

        ch.delete_message("C1", "1234.567")
            .await
            .expect("delete succeeds");

        mock.assert_async().await;
    }

    /// **The failing direction.** A platform `ok:false` must surface as an
    /// error, never as a silent success — a caller that believes a message was
    /// deleted when it still exists is the worst outcome this operation has.
    ///
    /// This is the control for the two cases above: they prove the gate can
    /// pass, this proves it can fail.
    #[tokio::test]
    async fn a_platform_refusal_is_an_error_not_a_silent_success() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/api/chat.delete")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"message_not_found"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();

        let err = ch.delete_message("C1", "9999.000").await.unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("message_not_found"),
            "the platform's own code must reach the operator, got {rendered}"
        );
        // …and specifically NOT `Unsupported`, which would mean the override
        // vanished and the trait default answered instead.
        assert!(
            !matches!(err, ChannelError::Unsupported { .. }),
            "got Unsupported — the edit/delete override is missing"
        );
    }

    /// Auth failure on a mutate is `Auth`, distinctly from a platform refusal,
    /// so an operator is told to fix the token rather than to fix the message.
    #[tokio::test]
    async fn an_invalid_token_on_edit_is_auth_not_rejected() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/api/chat.update")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"invalid_auth"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();

        let err = ch.edit_message("C1", "1.0", "x").await.unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
    }

    /// The declaration and the behaviour must agree — the same binding
    /// `slack_declares_idempotency_only_because_it_sends_the_header` makes for
    /// outbound idempotency, applied to native actions.
    #[tokio::test]
    async fn native_action_declaration_matches_behaviour() {
        use wcore_channels::ActionSupport;
        let ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        let a = ch.native_actions();
        assert_eq!(a.edit, ActionSupport::Implemented);
        assert_eq!(a.delete, ActionSupport::Implemented);
        assert_eq!(a.react, ActionSupport::Implemented);
        // Slack genuinely has no bot typing API. This must be the PERMANENT
        // state, not the backlog one — recording it as `NotImplemented` would
        // put a task on a list that can never be completed.
        assert_eq!(a.typing, ActionSupport::PlatformHasNoApi);
        assert!(!a.note.is_empty(), "a non-implemented op must say why");

        // Not started → the ops answer Auth (token not loaded), which proves
        // the override exists. A missing override would answer Unsupported.
        let e = ch.edit_message("C1", "1.0", "x").await.unwrap_err();
        assert!(
            !matches!(e, ChannelError::Unsupported { .. }),
            "declared Implemented but the trait default answered: {e:?}"
        );
        let d = ch.delete_message("C1", "1.0").await.unwrap_err();
        assert!(
            !matches!(d, ChannelError::Unsupported { .. }),
            "declared Implemented but the trait default answered: {d:?}"
        );
    }
}
