//! `wcore-channel-discord` — production Discord adapter.
//!
//! Implements the [`Channel`] trait from `wcore-channels`. Outbound
//! uses `POST /api/v10/channels/{channel_id}/messages` with `Bot <token>`
//! auth; inbound uses the Discord Gateway WebSocket (v10) on a
//! background task spawned in `start()`. The bot token is resolved
//! lazily from `wcore-config`'s credential store; the TOML config
//! carries only the credential-handle key.
//!
//! Gateway lifecycle:
//!   1. Connect to `wss://gateway.discord.gg/?v=10&encoding=json`.
//!   2. Receive `op=10 HELLO`, take `heartbeat_interval` from it.
//!   3. Send `op=2 IDENTIFY` with intents bitmask (default
//!      GUILD_MESSAGES | MESSAGE_CONTENT).
//!   4. Heartbeat every `heartbeat_interval` ms; treat the connection
//!      as dead if `HEARTBEAT_ACK` doesn't arrive within the configured
//!      grace window.
//!   5. Map every `op=0 t="MESSAGE_CREATE"` to a `ChannelEvent::MessageReceived`
//!      and queue it for `poll_events`.
//!   6. On `op=7 RECONNECT` / `op=9 INVALID_SESSION` / socket error:
//!      tear down + re-IDENTIFY on next session. Full RESUME (with
//!      session_id) is deferred.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

use wcore_channels::Channel;
use wcore_channels::error::ChannelError;
use wcore_channels::event::{ChannelEvent, ConnectionState, MessageReceipt};
use wcore_channels::outgoing::OutgoingMessage;
use wcore_config::credentials::CredentialsStore;

pub use crate::config::{
    DEFAULT_INTENTS, DiscordConfig, INTENT_GUILD_MESSAGES, INTENT_MESSAGE_CONTENT,
};
pub use crate::error::DiscordError;

pub mod config;
pub mod error;
mod gateway;
mod rest;

/// Production REST base URL. Override in tests via [`DiscordChannel::with_bases`].
pub const DISCORD_API_BASE: &str = "https://discord.com";
/// Production Gateway base URL. Override in tests via [`DiscordChannel::with_bases`].
pub const DISCORD_GATEWAY_BASE: &str = "wss://gateway.discord.gg";

/// Production Discord channel adapter.
pub struct DiscordChannel {
    name: String,
    config: DiscordConfig,
    state: ConnectionState,
    /// Bot token resolved from the credentials store at `start()`.
    bot_token: Option<String>,
    http: wcore_egress::EgressClient,
    /// Background gateway task pushes into this; `poll_events` drains it.
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    gateway_handle: Option<JoinHandle<()>>,
    shutdown: Option<watch::Sender<bool>>,
    /// REST base. Configurable for tests.
    api_base: String,
    /// Gateway WebSocket base. Configurable for tests.
    gateway_base: String,
    /// Credentials store used to resolve the bot token at `start()`.
    creds: Arc<dyn CredentialsStore>,
}

impl DiscordChannel {
    /// Construct a Discord channel pointed at the production endpoints.
    pub fn new(
        name: impl Into<String>,
        config: DiscordConfig,
        creds: Arc<dyn CredentialsStore>,
    ) -> Self {
        Self::with_bases(
            name,
            config,
            creds,
            DISCORD_API_BASE.to_string(),
            DISCORD_GATEWAY_BASE.to_string(),
        )
    }

    /// Test-only constructor that overrides both base URLs so `mockito`
    /// can stand in for `discord.com` and a local WS server (or just
    /// "unused") can stand in for the gateway.
    #[doc(hidden)]
    pub fn with_bases(
        name: impl Into<String>,
        config: DiscordConfig,
        creds: Arc<dyn CredentialsStore>,
        api_base: String,
        gateway_base: String,
    ) -> Self {
        let http = wcore_egress::EgressClient::builder()
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .user_agent(concat!("wayland-core/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| wcore_egress::EgressClient::new());

        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            bot_token: None,
            http,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            gateway_handle: None,
            shutdown: None,
            api_base,
            gateway_base,
            creds,
        }
    }

    /// Current connection state. Mostly useful for tests.
    pub fn state(&self) -> ConnectionState {
        self.state
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "discord"
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.gateway_handle.is_some() {
            return Ok(());
        }

        self.state = ConnectionState::Connecting;

        // Resolve the bot token from the credentials store.
        let token = self
            .creds
            .get(&self.config.credential_handle)
            .map_err(|e| ChannelError::Auth(format!("credentials lookup: {e}")))?
            .ok_or_else(|| {
                ChannelError::Auth(format!(
                    "bot token not found at credential_handle {:?}",
                    self.config.credential_handle
                ))
            })?;
        self.bot_token = Some(token.clone());

        // Spawn the gateway task. The gateway driver pushes its own
        // ConnectionStateChanged(Connected) once IDENTIFY completes.
        let (tx, rx) = watch::channel(false);
        let allowed: HashSet<String> = self.config.allowed_channel_ids.iter().cloned().collect();
        let args = gateway::GatewayArgs {
            gateway_url: self.gateway_base.clone(),
            bot_token: token,
            intents: self.config.intents,
            heartbeat_grace_ms: self.config.heartbeat_grace_ms,
            allowed_channel_ids: allowed,
            inbox: Arc::clone(&self.inbox),
            shutdown: rx,
        };
        let handle = tokio::spawn(gateway::gateway_loop(args));
        self.gateway_handle = Some(handle);
        self.shutdown = Some(tx);
        // Mark Connecting on the local state — gateway emits Connected
        // once IDENTIFY lands.
        self.state = ConnectionState::Connecting;

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        if self.gateway_handle.is_none() {
            return Ok(());
        }
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.gateway_handle.take() {
            let abort = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
            if abort.is_err() {
                tracing::warn!(
                    target: "wcore_channel_discord",
                    channel = %self.name,
                    "gateway task did not exit within shutdown grace; aborted"
                );
            }
        }
        self.bot_token = None;
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
        Ok(self.inbox.lock().await.drain(..).collect())
    }

    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        let token = self.bot_token.as_deref().ok_or(ChannelError::NotStarted)?;
        let reference = msg
            .reply_to
            .as_deref()
            .map(|m| rest::MessageReference { message_id: m });
        let body = rest::CreateMessageBody {
            content: &msg.text,
            message_reference: reference,
        };
        let result = rest::send_message(
            &self.http,
            &self.api_base,
            token,
            &msg.conversation_id,
            &body,
        )
        .await
        .map_err(ChannelError::from)?;
        let ts_secs = result
            .timestamp
            .as_deref()
            .map(rest::parse_iso8601_to_epoch)
            .unwrap_or(0);
        Ok(MessageReceipt {
            id: result.id,
            conversation_id: result
                .channel_id
                .unwrap_or_else(|| msg.conversation_id.clone()),
            ts_secs,
        })
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/discord.json")
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use wcore_config::credentials::CredentialsError;

    // ----- in-memory creds stub for tests -----
    struct InMemoryCreds {
        inner: StdMutex<std::collections::HashMap<String, String>>,
    }
    impl InMemoryCreds {
        fn new() -> Self {
            Self {
                inner: StdMutex::new(std::collections::HashMap::new()),
            }
        }
        fn with_token(handle: &str, token: &str) -> Arc<dyn CredentialsStore> {
            let s = Self::new();
            s.inner
                .lock()
                .unwrap()
                .insert(handle.to_string(), token.to_string());
            Arc::new(s)
        }
    }
    impl CredentialsStore for InMemoryCreds {
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

    fn cfg() -> DiscordConfig {
        DiscordConfig {
            credential_handle: "discord.test.bot_token".to_string(),
            allowed_channel_ids: Vec::new(),
            intents: DEFAULT_INTENTS,
            heartbeat_grace_ms: 5_000,
        }
    }

    const TEST_TOKEN: &str = "MTIz.ABCDEF.test-bot-token";
    const TEST_CHANNEL: &str = "424242";

    /// Build a started channel using mockito for REST and a dummy
    /// gateway URL. The gateway task will fail to connect (no server
    /// listening) and back off in a loop — we don't care; the REST
    /// path is what each send_message test exercises. `stop()` cleans
    /// it up.
    async fn start_channel_with_rest_only(server: &mockito::Server) -> DiscordChannel {
        let creds = InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN);
        let mut ch = DiscordChannel::with_bases(
            "test",
            cfg(),
            creds,
            server.url(),
            // Use an invalid scheme so the gateway task fails fast on
            // every reconnect attempt — backoff keeps it quiet.
            "ws://127.0.0.1:1".to_string(),
        );
        ch.start().await.unwrap();
        ch
    }

    // -----------------------------------------------------------------
    // 1. send_message hits POST /api/v10/channels/<id>/messages with
    //    Bot <token> auth and JSON body.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_succeeds_on_200() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "POST",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages").as_str(),
            )
            .match_header("authorization", format!("Bot {TEST_TOKEN}").as_str())
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"content":"hello"}"#.to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"id":"42","channel_id":"424242","timestamp":"2024-01-02T03:04:05+00:00"}"#,
            )
            .create_async()
            .await;

        let mut ch = start_channel_with_rest_only(&server).await;
        let receipt = ch
            .send_message(OutgoingMessage::text(TEST_CHANNEL, "hello"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "42");
        assert_eq!(receipt.conversation_id, "424242");
        assert_eq!(receipt.ts_secs, 1_704_164_645);
        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 2. send_message retries on 5xx, returns success after retry.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_retries_on_503_then_succeeds() {
        let mut server = mockito::Server::new_async().await;
        let _m1 = server
            .mock(
                "POST",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages").as_str(),
            )
            .with_status(503)
            .expect(1)
            .create_async()
            .await;
        let m2 = server
            .mock(
                "POST",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages").as_str(),
            )
            .with_status(200)
            .with_body(r#"{"id":"7","channel_id":"424242"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = start_channel_with_rest_only(&server).await;
        let receipt = ch
            .send_message(OutgoingMessage::text(TEST_CHANNEL, "after retry"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "7");
        m2.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 3. send_message honours Retry-After header on 429.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_honours_429_retry_after() {
        let mut server = mockito::Server::new_async().await;
        let _m429 = server
            .mock(
                "POST",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages").as_str(),
            )
            .with_status(429)
            .with_header("retry-after", "0")
            .with_header("content-type", "application/json")
            .with_body(r#"{"message":"You are being rate limited","retry_after":0,"global":false}"#)
            .expect(1)
            .create_async()
            .await;
        let m200 = server
            .mock(
                "POST",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages").as_str(),
            )
            .with_status(200)
            .with_body(r#"{"id":"99","channel_id":"424242"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = start_channel_with_rest_only(&server).await;
        let receipt = ch
            .send_message(OutgoingMessage::text(TEST_CHANNEL, "after 429"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "99");
        m200.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 4. send_message bubbles 4xx as permanent.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_4xx_is_permanent() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock(
                "POST",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages").as_str(),
            )
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"code":50001,"message":"Missing Access"}"#)
            .expect(1) // <- must not retry
            .create_async()
            .await;

        let mut ch = start_channel_with_rest_only(&server).await;
        let err = ch
            .send_message(OutgoingMessage::text(TEST_CHANNEL, "x"))
            .await
            .expect_err("expected 4xx rejection");
        match err {
            ChannelError::Rejected(msg) => {
                assert!(msg.contains("400"), "msg = {msg}");
                assert!(msg.contains("Missing Access"), "msg = {msg}");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        m.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 5. config TOML round-trip via ChannelConfig.options.
    // -----------------------------------------------------------------
    #[test]
    fn config_round_trip_via_channel_config_options() {
        let raw = r#"
name = "acme-discord"
platform = "discord"

[options]
credential_handle = "discord.acme.bot_token"
allowed_channel_ids = ["111", "222"]
intents = 513
heartbeat_grace_ms = 8000
"#;
        let outer: wcore_channels::ChannelConfig = toml::from_str(raw).unwrap();
        let cfg: DiscordConfig = outer.options.try_into().unwrap();
        assert_eq!(cfg.credential_handle, "discord.acme.bot_token");
        assert_eq!(cfg.allowed_channel_ids, vec!["111", "222"]);
        assert_eq!(cfg.intents, 513);
        assert_eq!(cfg.heartbeat_grace_ms, 8_000);
    }

    // -----------------------------------------------------------------
    // 6. stop() ends the gateway task cleanly (no leaked tasks).
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn stop_ends_gateway_task_cleanly() {
        let creds = InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN);
        let mut ch = DiscordChannel::with_bases(
            "test",
            cfg(),
            creds,
            "http://unused".to_string(),
            // Reach for a port nothing's bound to so connect fails fast.
            "ws://127.0.0.1:1".to_string(),
        );
        ch.start().await.unwrap();
        assert!(ch.gateway_handle.is_some());

        ch.stop().await.unwrap();
        assert!(
            ch.gateway_handle.is_none(),
            "gateway handle should be cleared"
        );
        assert!(ch.shutdown.is_none(), "shutdown sender should be cleared");
        assert!(ch.bot_token.is_none(), "bot token should be cleared");
        assert_eq!(ch.state(), ConnectionState::Disconnected);

        // Second stop is idempotent.
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 7. send before start surfaces NotStarted.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_before_start_errors_not_started() {
        let creds = InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN);
        let mut ch = DiscordChannel::with_bases(
            "test",
            cfg(),
            creds,
            "http://unused".to_string(),
            "ws://127.0.0.1:1".to_string(),
        );
        let err = ch
            .send_message(OutgoingMessage::text("c", "x"))
            .await
            .expect_err("expected NotStarted");
        assert!(matches!(err, ChannelError::NotStarted), "got {err:?}");
    }

    // -----------------------------------------------------------------
    // 8. start() with missing credential surfaces Auth.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn start_with_missing_credential_errors_auth() {
        let creds: Arc<dyn CredentialsStore> = Arc::new(InMemoryCreds::new());
        let mut ch = DiscordChannel::with_bases(
            "test",
            cfg(),
            creds,
            "http://unused".to_string(),
            "ws://127.0.0.1:1".to_string(),
        );
        let err = ch.start().await.expect_err("expected Auth error");
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
    }
}
