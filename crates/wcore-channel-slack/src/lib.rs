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
                self.inbox.lock().await.push_back(ev);
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

        let resp = api::post_message(
            &self.http,
            &self.config.api_base_url,
            bot_token,
            &req,
            self.config.max_retry_attempts,
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
}
