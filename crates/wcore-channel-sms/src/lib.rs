//! `wcore-channel-sms` — production Twilio SMS adapter implementing the
//! `wcore_channels::Channel` trait.
//!
//! Outbound: Twilio REST `POST /2010-04-01/Accounts/<sid>/Messages.json`
//! with HTTP Basic auth (Account SID + Auth Token), retry-with-jitter,
//! `Retry-After` honoured on HTTP 429, permanent-error short-circuit on
//! 4xx.
//!
//! Inbound: Twilio sends `application/x-www-form-urlencoded` POSTs to a
//! configured webhook URL on every incoming SMS. The engine's webhook
//! router invokes `SmsChannel::ingest_webhook(full_url, raw_body, signature)`
//! when a POST hits the channel's URL. The adapter verifies the
//! HMAC-SHA1 signature, parses the form body, and enqueues an
//! `IncomingMessage` for the next `poll_events()`.
//!
//! Secrets (Account SID, Auth Token) are resolved at `start()` time
//! from the `CredentialsStore`. The TOML config carries credential
//! handles only.

pub mod api;
pub mod config;
pub mod error;
pub mod inbound;

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;
use wcore_channels::{
    Channel, ChannelError,
    event::{ChannelEvent, ConnectionState, MessageReceipt},
    outgoing::OutgoingMessage,
};
use wcore_config::credentials::CredentialsStore;

pub use config::SmsConfig;
pub use error::SmsError;

/// Production Twilio SMS adapter.
///
/// One instance per Twilio sub-account / from-number. Lifecycle:
///   construct (`new`) →
///   `start()` (resolves secrets from CredentialsStore) →
///   loop `poll_events` / `send_message` →
///   `stop()` (drops cached secrets).
///
/// SMS has no continuous connection — outbound is one REST call per
/// message, inbound arrives via webhook POSTs to a separately-configured
/// URL. No background task, no shutdown channel.
pub struct SmsChannel {
    name: String,
    config: SmsConfig,
    state: ConnectionState,
    account_sid: Option<String>,
    auth_token: Option<String>,
    http: wcore_egress::EgressClient,
    credentials: Arc<dyn CredentialsStore>,
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
}

impl SmsChannel {
    /// Construct a new adapter. `credentials` is the store the Account
    /// SID + Auth Token are pulled from at `start()`.
    pub fn new(
        name: impl Into<String>,
        config: SmsConfig,
        credentials: Arc<dyn CredentialsStore>,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            account_sid: None,
            auth_token: None,
            http: wcore_egress::EgressClient::new(),
            credentials,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Construct with a caller-supplied `reqwest::Client` (tests use
    /// this to drive a mockito server with a short timeout).
    pub fn with_http_client(
        name: impl Into<String>,
        config: SmsConfig,
        credentials: Arc<dyn CredentialsStore>,
        http: wcore_egress::EgressClient,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            account_sid: None,
            auth_token: None,
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
    /// Called by the engine's HTTP host when a Twilio webhook POST
    /// lands at the channel's configured URL. `full_url` is the URL
    /// the request hit, including scheme + host + path + query (Twilio
    /// signs the whole thing). `raw_body` is the literal request body
    /// — `application/x-www-form-urlencoded` per Twilio's spec. The
    /// adapter verifies the signature, parses the body, and enqueues
    /// an `IncomingMessage` for the next `poll_events()`.
    ///
    /// Returns `Err(SmsError::SignatureMismatch)` if the signature is
    /// invalid; the engine should respond HTTP 403 in that case.
    pub async fn ingest_webhook(
        &self,
        full_url: &str,
        raw_body: &str,
        signature: &str,
    ) -> Result<(), SmsError> {
        let auth_token = self.auth_token.as_deref().ok_or_else(|| {
            SmsError::Auth("auth token not loaded — call start() first".to_string())
        })?;

        let pairs = inbound::verify_signature(auth_token, full_url, raw_body, signature)?;
        let msg = inbound::pairs_to_incoming(&pairs)?;
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::MessageReceived { msg });
        Ok(())
    }
}

#[async_trait]
impl Channel for SmsChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "sms"
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.state == ConnectionState::Connected {
            return Ok(());
        }
        self.state = ConnectionState::Connecting;

        let account_sid = self
            .credentials
            .get(&self.config.credential_handle_account_sid)
            .map_err(|e| SmsError::Credentials(e.to_string()))?
            .ok_or_else(|| {
                SmsError::Credentials(format!(
                    "no value for credential handle {:?}",
                    self.config.credential_handle_account_sid
                ))
            })?;
        let auth_token = self
            .credentials
            .get(&self.config.credential_handle_auth_token)
            .map_err(|e| SmsError::Credentials(e.to_string()))?
            .ok_or_else(|| {
                SmsError::Credentials(format!(
                    "no value for credential handle {:?}",
                    self.config.credential_handle_auth_token
                ))
            })?;

        self.account_sid = Some(account_sid);
        self.auth_token = Some(auth_token);
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
        self.account_sid = None;
        self.auth_token = None;
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
        let account_sid = self
            .account_sid
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("account sid not loaded".to_string()))?;
        let auth_token = self
            .auth_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("auth token not loaded".to_string()))?;

        if msg.conversation_id.is_empty() {
            return Err(ChannelError::Rejected(
                "OutgoingMessage.conversation_id is empty (twilio requires To)".to_string(),
            ));
        }

        let resp = api::send_message(
            &self.http,
            &self.config.api_base_url,
            account_sid,
            auth_token,
            &self.config.from_number,
            &msg.conversation_id,
            &msg.text,
            self.config.max_retry_attempts,
        )
        .await
        .map_err(ChannelError::from)?;

        Ok(MessageReceipt {
            id: resp.sid,
            conversation_id: msg.conversation_id,
            ts_secs: Utc::now().timestamp(),
        })
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/sms.json")
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

    const TEST_SID: &str = "ACtest1234567890";
    const TEST_TOKEN: &str = "test-auth-token";

    fn cfg_for(server_url: &str) -> SmsConfig {
        SmsConfig::new_for_test(server_url)
    }

    fn store_for_test() -> Arc<MapStore> {
        MapStore::new(&[
            ("sms.test.account_sid", TEST_SID),
            ("sms.test.auth_token", TEST_TOKEN),
        ])
    }

    // -----------------------------------------------------------------
    // 1. send_message hits Messages.json with basic-auth + form body.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_hits_messages_json_with_basic_auth_and_form() {
        let mut server = mockito::Server::new_async().await;

        // base64(ACtest1234567890:test-auth-token) — precomputed in the
        // test so the assertion is explicit about what Basic auth Twilio
        // expects.
        use base64::Engine;
        let expected_basic =
            base64::engine::general_purpose::STANDARD.encode(format!("{TEST_SID}:{TEST_TOKEN}"));
        let expected_auth_header = format!("Basic {expected_basic}");

        let mock = server
            .mock(
                "POST",
                format!("/2010-04-01/Accounts/{TEST_SID}/Messages.json").as_str(),
            )
            .match_header("authorization", expected_auth_header.as_str())
            .match_header(
                "content-type",
                mockito::Matcher::Regex("application/x-www-form-urlencoded.*".to_string()),
            )
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("From=%2B15550000000".to_string()),
                mockito::Matcher::Regex("To=%2B15551112222".to_string()),
                mockito::Matcher::Regex("Body=hello".to_string()),
            ]))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(r#"{"sid":"SM-fake","status":"queued"}"#)
            .create_async()
            .await;

        let mut ch = SmsChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("+15551112222", "hello"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "SM-fake");
        assert_eq!(receipt.conversation_id, "+15551112222");
        mock.assert_async().await;
    }

    // -----------------------------------------------------------------
    // 2. send_message retries on 5xx.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_retries_on_5xx() {
        let mut server = mockito::Server::new_async().await;
        let fail = server
            .mock(
                "POST",
                format!("/2010-04-01/Accounts/{TEST_SID}/Messages.json").as_str(),
            )
            .with_status(503)
            .with_body("upstream")
            .expect(1)
            .create_async()
            .await;
        let succeed = server
            .mock(
                "POST",
                format!("/2010-04-01/Accounts/{TEST_SID}/Messages.json").as_str(),
            )
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(r#"{"sid":"SM42","status":"queued"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = SmsChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("+1", "hi"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "SM42");

        fail.assert_async().await;
        succeed.assert_async().await;
    }

    // -----------------------------------------------------------------
    // 3. send_message honours Retry-After on 429.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_honours_retry_after_on_429() {
        let mut server = mockito::Server::new_async().await;
        let throttled = server
            .mock(
                "POST",
                format!("/2010-04-01/Accounts/{TEST_SID}/Messages.json").as_str(),
            )
            .with_status(429)
            .with_header("Retry-After", "0")
            .with_body("rate-limited")
            .expect(1)
            .create_async()
            .await;
        let succeed = server
            .mock(
                "POST",
                format!("/2010-04-01/Accounts/{TEST_SID}/Messages.json").as_str(),
            )
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(r#"{"sid":"SM99","status":"queued"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = SmsChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("+1", "hi"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "SM99");

        throttled.assert_async().await;
        succeed.assert_async().await;
    }

    // -----------------------------------------------------------------
    // 4. send_message bubbles 4xx as permanent.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_4xx_is_permanent() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "POST",
                format!("/2010-04-01/Accounts/{TEST_SID}/Messages.json").as_str(),
            )
            .with_status(401)
            .with_body("authentication failed")
            .expect(1) // <- must not retry
            .create_async()
            .await;

        let mut ch = SmsChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let err = ch
            .send_message(OutgoingMessage::text("+1", "hi"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        mock.assert_async().await;
    }

    // -----------------------------------------------------------------
    // 5. ingest_webhook with valid Twilio signature parses message.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn ingest_webhook_valid_signature_enqueues_message() {
        let cfg = cfg_for("https://unused.example");
        let mut ch = SmsChannel::new("test", cfg, store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let url = "https://hooks.example/sms";
        let body =
            "MessageSid=SM123&From=%2B15551234567&To=%2B15559876543&Body=hi+there&NumMedia=0";
        let pairs = inbound::parse_form(body);
        let sig = inbound::expected_signature(TEST_TOKEN, url, &pairs);

        ch.ingest_webhook(url, body, &sig).await.unwrap();

        let evs = ch.poll_events().await.unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.id, "SM123");
                assert_eq!(msg.author, "+15551234567");
                assert_eq!(msg.conversation_id, "+15559876543");
                assert_eq!(msg.text, "hi there");
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // 6. ingest_webhook with invalid signature returns SignatureMismatch.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn ingest_webhook_invalid_signature_errors() {
        let cfg = cfg_for("https://unused.example");
        let mut ch = SmsChannel::new("test", cfg, store_for_test());
        ch.start().await.unwrap();

        let url = "https://hooks.example/sms";
        let body = "MessageSid=SM1&From=%2B1&To=%2B2&Body=hi&NumMedia=0";
        // Wrong signature (well-formed base64, 28 chars, but not a valid HMAC).
        let bogus = "AAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        let err = ch.ingest_webhook(url, body, bogus).await.unwrap_err();
        assert!(matches!(err, SmsError::SignatureMismatch));
    }

    // -----------------------------------------------------------------
    // 7. config TOML round-trip — covered in config.rs; this test
    //    additionally proves the schema include_str! resolves.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn config_schema_is_valid_json() {
        let ch = SmsChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        let parsed: serde_json::Value =
            serde_json::from_str(ch.config_schema()).expect("schema parses");
        assert_eq!(parsed["title"].as_str(), Some("SmsChannelConfig"));
    }

    // -----------------------------------------------------------------
    // Bonus: start() with missing credential surfaces Auth.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn start_missing_account_sid_errors() {
        let store = MapStore::new(&[("sms.test.auth_token", TEST_TOKEN)]);
        let mut ch = SmsChannel::new("test", cfg_for("https://unused.example"), store);
        let err = ch.start().await.unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
    }

    // -----------------------------------------------------------------
    // Bonus: send_before_start surfaces NotStarted.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_before_start_errors_not_started() {
        let mut ch = SmsChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        let err = ch
            .send_message(OutgoingMessage::text("+1", "x"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::NotStarted), "got {err:?}");
    }

    // -----------------------------------------------------------------
    // Bonus: ingest_webhook before start surfaces Auth.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn ingest_webhook_before_start_errors() {
        let ch = SmsChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        let err = ch
            .ingest_webhook(
                "https://x",
                "MessageSid=SM1&From=%2B1&To=%2B2&Body=hi",
                "sig",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, SmsError::Auth(_)));
    }
}
