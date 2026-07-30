//! `wcore-channel-whatsapp` — production WhatsApp Cloud API adapter
//! implementing the `wcore_channels::Channel` trait.
//!
//! Outbound: `POST {api_base}/{graph_version}/{phone_number_id}/messages`
//! with bearer auth, retry + exponential backoff + jitter, `Retry-After`
//! honoured on HTTP 429, permanent-error short-circuit on 4xx and Meta
//! auth-class error codes.
//!
//! Inbound: Meta webhook POSTs at `/channels/whatsapp/<name>/webhook`.
//! The adapter verifies the `X-Hub-Signature-256: sha256=<hex>` header
//! against the raw body keyed by the Meta App Secret, parses the JSON
//! envelope, and enqueues one `ChannelEvent::MessageReceived` per text
//! message inside `entry[].changes[].value.messages[]`. Non-text message
//! kinds (image, video, sticker, status, …) surface as
//! `ChannelEvent::PlatformWarning` so the engine sees that traffic
//! arrived without polluting the message stream.
//!
//! Secrets (access token, app secret) are resolved at `start()` time
//! from the `CredentialsStore`. The TOML config carries credential
//! handles only.

pub mod api;
pub mod bridge;
pub mod config;
pub mod error;
pub mod inbound;

/// The single source of this adapter's inbound media bounds.
///
/// [`Channel::media_bounds`] returns this, and [`api::download_media`] caps the
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
use tokio::sync::Mutex;
use wcore_channels::{
    Channel, ChannelError, WebhookRequest, WebhookResponse,
    event::{ChannelEvent, ConnectionState, MessageReceipt},
    outgoing::OutgoingMessage,
};
use wcore_config::credentials::CredentialsStore;

pub use bridge::{WhatsappBackend, WhatsappBridgeChannel, WhatsappBridgeConfig};
pub use config::WhatsappConfig;
pub use error::WhatsappError;

/// Production WhatsApp Cloud API adapter.
///
/// One instance per WhatsApp Business phone number. Lifecycle:
///   construct (`new`) →
///   `start()` (resolves secrets from CredentialsStore) →
///   loop `poll_events` / `send_message` →
///   `stop()` (drops cached secrets).
pub struct WhatsappChannel {
    name: String,
    config: WhatsappConfig,
    state: ConnectionState,
    access_token: Option<String>,
    app_secret: Option<String>,
    http: wcore_egress::EgressClient,
    credentials: Arc<dyn CredentialsStore>,
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
}

impl WhatsappChannel {
    /// Construct a new adapter. `credentials` is the store the access
    /// token + app secret are pulled from at `start()`.
    pub fn new(
        name: impl Into<String>,
        config: WhatsappConfig,
        credentials: Arc<dyn CredentialsStore>,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            access_token: None,
            app_secret: None,
            http: wcore_egress::EgressClient::new(),
            credentials,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Construct with a caller-supplied `reqwest::Client` (tests use
    /// this to drive a mockito server with a short timeout).
    pub fn with_http_client(
        name: impl Into<String>,
        config: WhatsappConfig,
        credentials: Arc<dyn CredentialsStore>,
        http: wcore_egress::EgressClient,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            access_token: None,
            app_secret: None,
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
    /// Called by the engine's HTTP host when a WhatsApp webhook POST
    /// lands at the channel's configured URL. Verifies the
    /// `X-Hub-Signature-256` header, parses the body, enqueues one
    /// `ChannelEvent::MessageReceived` per text message for the next
    /// `poll_events()`.
    pub async fn ingest_event(
        &self,
        raw_body: &str,
        signature_header: &str,
    ) -> Result<(), WhatsappError> {
        let app_secret = self.app_secret.as_deref().ok_or_else(|| {
            WhatsappError::Auth("app secret not loaded — call start() first".to_string())
        })?;

        inbound::verify_signature(app_secret, raw_body.as_bytes(), signature_header)?;

        let events = inbound::parse_webhook(raw_body)?;
        if events.is_empty() {
            return Ok(());
        }
        let mut inbox = self.inbox.lock().await;
        for ev in events {
            // F9 — bounded, drop-oldest inbox against a flood.
            wcore_channels::push_bounded(&mut inbox, ev);
        }
        Ok(())
    }
}

#[async_trait]
impl Channel for WhatsappChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "whatsapp"
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.state == ConnectionState::Connected {
            return Ok(());
        }
        self.state = ConnectionState::Connecting;

        // Resolve secrets from the credentials store.
        let access_token = self
            .credentials
            .get(&self.config.credential_handle_access_token)
            .map_err(|e| WhatsappError::Credentials(e.to_string()))?
            .ok_or_else(|| {
                WhatsappError::Credentials(format!(
                    "no value for credential handle {:?}",
                    self.config.credential_handle_access_token
                ))
            })?;
        let app_secret = self
            .credentials
            .get(&self.config.credential_handle_app_secret)
            .map_err(|e| WhatsappError::Credentials(e.to_string()))?
            .ok_or_else(|| {
                WhatsappError::Credentials(format!(
                    "no value for credential handle {:?}",
                    self.config.credential_handle_app_secret
                ))
            })?;

        self.access_token = Some(access_token);
        self.app_secret = Some(app_secret);
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
        self.access_token = None;
        self.app_secret = None;
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
        let access_token = self
            .access_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("access token not loaded".to_string()))?;

        let recipient = if msg.conversation_id.is_empty() {
            if self.config.default_recipient.is_empty() {
                return Err(ChannelError::Rejected(
                    "no conversation_id and no default_recipient configured".to_string(),
                ));
            }
            self.config.default_recipient.clone()
        } else {
            msg.conversation_id.clone()
        };

        // When the outbound carries attachments, send each as a media message
        // (link variant) so a non-text reply isn't silently dropped. The first
        // attachment carries `msg.text` as its caption (Cloud API media messages
        // support a caption for image/video/document), so a single text+media
        // reply lands as one message; remaining attachments follow caption-less.
        // The wamid recorded is the last media message's id.
        let wamid = if !msg.attachments.is_empty() {
            let mut last_wamid: Option<String> = None;
            for (idx, url) in msg.attachments.iter().enumerate() {
                let caption = if idx == 0 && !msg.text.is_empty() {
                    Some(msg.text.clone())
                } else {
                    None
                };
                let media_req = api::SendMediaRequest::new_link(recipient.clone(), url, caption)
                    // Only the first message quotes the reply context.
                    .with_reply_context(if idx == 0 { msg.reply_to.clone() } else { None });
                let resp = api::send_media(
                    &self.http,
                    &self.config.api_base_url,
                    &self.config.graph_version,
                    &self.config.phone_number_id,
                    access_token,
                    &media_req,
                    self.config.max_retry_attempts,
                )
                .await
                .map_err(ChannelError::from)?;
                last_wamid = Some(resp.messages[0].id.clone());
            }
            // attachments is non-empty, so the loop ran at least once.
            last_wamid.unwrap_or_default()
        } else {
            // Quote the message being replied to (if this turn is a reply) so the
            // bot threads in-context. `reply_to` carries the inbound wamid via the
            // shared inbound subscriber; None for a fresh message.
            let req = api::SendMessageRequest::new_text(recipient.clone(), msg.text.clone())
                .with_reply_context(msg.reply_to.clone());

            let resp = api::send_message(
                &self.http,
                &self.config.api_base_url,
                &self.config.graph_version,
                &self.config.phone_number_id,
                access_token,
                &req,
                self.config.max_retry_attempts,
            )
            .await
            .map_err(ChannelError::from)?;

            // Per Meta docs the first messages[0].id is the wamid we should
            // record as the platform_id. Earlier api::send_message already
            // validated messages[] is non-empty.
            resp.messages[0].id.clone()
        };

        Ok(MessageReceipt {
            id: wamid,
            conversation_id: recipient,
            ts_secs: chrono::Utc::now().timestamp(),
        })
    }

    fn config_schema(&self) -> &str {
        include_str!("../schemas/whatsapp.json")
    }

    /// WhatsApp caps a single text message body at 4096 characters.
    fn max_message_len(&self) -> Option<usize> {
        Some(4096)
    }

    /// WhatsApp Cloud API: **edit and revoke are inbound-only concepts.**
    ///
    /// Meta documents both, and documents them as WEBHOOK EVENTS — a `type:
    /// "edit"` message notification carrying `edit.original_message_id`, and a
    /// `type: "revoke"` entry in `smb_message_echoes` describing *a business
    /// customer deleting a previously sent message* from the SMB app. Neither
    /// has an outbound counterpart: there is no Graph verb by which a Cloud
    /// API sender alters or withdraws a message it has already sent. So this is
    /// [`PlatformHasNoApi`](wcore_channels::ActionSupport::PlatformHasNoApi),
    /// not a backlog item — no amount of work here closes it.
    ///
    /// `typing` is the opposite case and that is exactly why the two states are
    /// separate. Cloud API **does** have a typing indicator, posted to
    /// `/{phone_number_id}/messages` with `typing_indicator: {type: "text"}` —
    /// but it is keyed to the `message_id` of a RECEIVED message, and
    /// [`Channel::send_typing`] is handed only a `conversation_id`. The
    /// capability is real and unreachable through the current trait signature,
    /// which is a seam finding rather than an absence, so it is recorded as
    /// `NotImplemented` with the reason.
    fn native_actions(&self) -> wcore_channels::NativeActions {
        use wcore_channels::ActionSupport::{Implemented, NotImplemented, PlatformHasNoApi};
        wcore_channels::NativeActions::none()
            .edit(PlatformHasNoApi)
            .delete(PlatformHasNoApi)
            .react(Implemented)
            .typing(NotImplemented)
            .note(
                "edit/delete: Cloud API models edit and revoke as INBOUND webhook events only \
                 (messages/edit, smb_message_echoes revoke) — there is no outbound verb. \
                 typing: the endpoint exists but is keyed to a received message_id, which \
                 Channel::send_typing(conversation_id) cannot supply — trait-signature gap.",
            )
    }

    /// Send a reaction message — the ack signal. `conversation_id` is the
    /// recipient `wa_id`, `message_id` the inbound `wamid`. Unicode emoji
    /// are sent directly. Note: WhatsApp's typing indicator is tied to a
    /// per-message read receipt (it needs the message id, which the typing
    /// keepalive does not carry), so `send_typing` keeps the trait no-op.
    async fn react(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let access_token = self
            .access_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("access token not loaded".to_string()))?;
        let req = api::SendReactionRequest::new(
            conversation_id.to_string(),
            message_id.to_string(),
            emoji.to_string(),
        );
        api::send_reaction(
            &self.http,
            &self.config.api_base_url,
            &self.config.graph_version,
            &self.config.phone_number_id,
            access_token,
            &req,
        )
        .await
        .map_err(ChannelError::from)
    }

    /// Download inbound WhatsApp media. `attachment.url` carries the Meta
    /// media id (not a URL); `api::download_media` resolves it to a
    /// short-lived URL then fetches the bytes, both hops bearer-authenticated.
    async fn fetch_media(
        &self,
        attachment: &wcore_channels::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        let access_token = self
            .access_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("access token not loaded".to_string()))?;
        api::download_media(
            &self.http,
            &self.config.api_base_url,
            &self.config.graph_version,
            access_token,
            &attachment.url,
            api::MEDIA_DOWNLOAD_HOSTS,
        )
        .await
        .map_err(ChannelError::from)
    }

    /// This adapter's inbound intake policy — see [`MEDIA_BOUNDS`], which is
    /// the same constant [`api::download_media`] caps the streamed body at.
    fn media_bounds(&self) -> wcore_channels::MediaBounds {
        MEDIA_BOUNDS
    }

    /// Handle a Meta WhatsApp Cloud API webhook request.
    ///
    /// Meta drives two distinct flows over the same URL:
    ///   * **GET** — the one-time subscription handshake. Meta calls with
    ///     `hub.mode=subscribe`, `hub.verify_token=<operator token>`, and
    ///     `hub.challenge=<nonce>`. When the mode is `subscribe` and the
    ///     token matches the connector's configured `verify_token`, the
    ///     challenge is echoed back verbatim; otherwise it is rejected.
    ///   * **POST** — runtime delivery. The `X-Hub-Signature-256` header
    ///     is verified against the app secret (in [`Self::ingest_event`]).
    async fn ingest_webhook(&self, req: &WebhookRequest) -> Result<WebhookResponse, ChannelError> {
        if req.method == "GET" {
            let configured = self.config.verify_token.as_deref();
            let mode = req.query_get("hub.mode");
            let token = req.query_get("hub.verify_token");
            let challenge = req.query_get("hub.challenge");
            match (mode, token, challenge, configured) {
                (Some("subscribe"), Some(token), Some(challenge), Some(configured))
                    if token == configured =>
                {
                    Ok(WebhookResponse::challenge(challenge))
                }
                _ => Err(ChannelError::Auth(
                    "whatsapp webhook verification failed".into(),
                )),
            }
        } else {
            let sig = req
                .header("x-hub-signature-256")
                .ok_or_else(|| ChannelError::Auth("missing whatsapp signature header".into()))?;
            match self.ingest_event(&req.body, sig).await {
                Ok(()) => Ok(WebhookResponse::ok()),
                Err(e) => Err(ChannelError::Rejected(e.to_string())),
            }
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

    fn cfg_for(server_url: &str) -> WhatsappConfig {
        WhatsappConfig::new_for_test(server_url)
    }

    fn store_for_test() -> Arc<MapStore> {
        MapStore::new(&[
            ("whatsapp.test.access_token", "EAAtest-token"),
            ("whatsapp.test.app_secret", "shhh"),
        ])
    }

    #[tokio::test]
    async fn send_message_hits_endpoint_with_bearer_and_json_body() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v18.0/10000000000/messages")
            .match_header("authorization", "Bearer EAAtest-token")
            .match_header(
                "content-type",
                mockito::Matcher::Regex("application/json.*".to_string()),
            )
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "messaging_product": "whatsapp",
                "to": "+15555550100",
                "type": "text",
                "text": {"body": "hello"}
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"messaging_product":"whatsapp","contacts":[{"input":"+15555550100","wa_id":"15555550100"}],"messages":[{"id":"wamid.HBgLMTUwMDA="}]}"#,
            )
            .create_async()
            .await;

        let mut ch = WhatsappChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("+15555550100", "hello"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "wamid.HBgLMTUwMDA=");
        assert_eq!(receipt.conversation_id, "+15555550100");

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_with_attachment_sends_media_body() {
        // An outbound carrying an attachment must POST a media message (link
        // variant) with the text as caption — not silently drop it.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v18.0/10000000000/messages")
            .match_header("authorization", "Bearer EAAtest-token")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "messaging_product": "whatsapp",
                "to": "+15555550100",
                "type": "image",
                "image": {
                    "link": "https://cdn.example/pic.jpg",
                    "caption": "see attached"
                }
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"messaging_product":"whatsapp","messages":[{"id":"wamid.MEDIA"}]}"#)
            .create_async()
            .await;

        let mut ch = WhatsappChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let msg = OutgoingMessage {
            conversation_id: "+15555550100".to_string(),
            text: "see attached".to_string(),
            reply_to: None,
            attachments: vec!["https://cdn.example/pic.jpg".to_string()],
        };
        let receipt = ch.send_message(msg).await.unwrap();
        assert_eq!(receipt.id, "wamid.MEDIA");

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_retries_on_5xx() {
        let mut server = mockito::Server::new_async().await;
        let fail = server
            .mock("POST", "/v18.0/10000000000/messages")
            .with_status(503)
            .with_body("upstream")
            .expect(1)
            .create_async()
            .await;
        let succeed = server
            .mock("POST", "/v18.0/10000000000/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"messaging_product":"whatsapp","messages":[{"id":"wamid.OK"}]}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = WhatsappChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("+15555550100", "hi"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "wamid.OK");

        fail.assert_async().await;
        succeed.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_honours_retry_after_on_429() {
        let mut server = mockito::Server::new_async().await;
        let throttled = server
            .mock("POST", "/v18.0/10000000000/messages")
            .with_status(429)
            .with_header("Retry-After", "0")
            .with_body("rate-limited")
            .expect(1)
            .create_async()
            .await;
        let succeed = server
            .mock("POST", "/v18.0/10000000000/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"messaging_product":"whatsapp","messages":[{"id":"wamid.LATER"}]}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = WhatsappChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("+15555550100", "hi"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "wamid.LATER");

        throttled.assert_async().await;
        succeed.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_4xx_other_than_429_is_permanent() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v18.0/10000000000/messages")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"error":{"message":"Invalid parameter","code":100,"type":"OAuthException"}}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let mut ch = WhatsappChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let err = ch
            .send_message(OutgoingMessage::text("+15555550100", "hi"))
            .await
            .unwrap_err();
        // 400 with an `error.code=100` (non auth-class) surfaces as Rejected.
        assert!(matches!(err, ChannelError::Rejected(_)), "got {err:?}");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_401_is_auth_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v18.0/10000000000/messages")
            .with_status(401)
            .with_body(r#"{"error":{"message":"Invalid OAuth token","code":190}}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = WhatsappChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let err = ch
            .send_message(OutgoingMessage::text("+15555550100", "hi"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn ingest_event_valid_signature_enqueues_message() {
        let mut ch =
            WhatsappChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let body = r#"{"entry":[{"changes":[{"value":{"messages":[{"from":"15555550100","id":"wamid.X","timestamp":"1700000000","text":{"body":"hi"},"type":"text"}]}}]}]}"#;
        let sig = inbound::expected_signature("shhh", body.as_bytes());

        ch.ingest_event(body, &sig).await.unwrap();

        let evs = ch.poll_events().await.unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.text, "hi");
                assert_eq!(msg.author, "15555550100");
                assert_eq!(msg.conversation_id, "15555550100");
                assert_eq!(msg.id, "wamid.X");
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ingest_event_invalid_signature_errors() {
        let mut ch =
            WhatsappChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        ch.start().await.unwrap();

        let body = r#"{"entry":[]}"#;
        let err = ch.ingest_event(body, "sha256=deadbeef").await.unwrap_err();
        assert!(matches!(err, WhatsappError::SignatureMismatch));
    }

    #[tokio::test]
    async fn config_schema_is_valid_json() {
        let ch = WhatsappChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        let parsed: serde_json::Value =
            serde_json::from_str(ch.config_schema()).expect("schema parses");
        assert_eq!(parsed["title"].as_str(), Some("WhatsappChannelConfig"));
    }

    #[tokio::test]
    async fn max_message_len_is_whatsapp_cap() {
        let ch = WhatsappChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        assert_eq!(ch.max_message_len(), Some(4096));
    }

    #[tokio::test]
    async fn start_missing_access_token_errors() {
        let store = MapStore::new(&[("whatsapp.test.app_secret", "shhh")]);
        let mut ch = WhatsappChannel::new("test", cfg_for("https://unused.example"), store);
        let err = ch.start().await.unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        assert_eq!(ch.state(), ConnectionState::Connecting);
    }
}
