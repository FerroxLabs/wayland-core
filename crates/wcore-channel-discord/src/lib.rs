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
//!   6. On `op=7 RECONNECT` / dropped socket / heartbeat lapse: tear
//!      down and RESUME (op 6) against `resume_gateway_url`, replaying
//!      events buffered during the gap. On `op=9 INVALID_SESSION`:
//!      resume when `d == true`, else clear the session and fall back to
//!      a fresh IDENTIFY after the Discord-required 1–5s wait.

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

    fn task_handle(&self) -> Option<&tokio::task::JoinHandle<()>> {
        self.gateway_handle.as_ref()
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self
            .gateway_handle
            .as_ref()
            .is_some_and(|h| !h.is_finished())
        {
            // Already running — idempotent. A finished handle (the gateway task
            // died) falls through to respawn so supervised reconnect heals the
            // channel instead of treating a dead task as alive.
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

        // Resolve this bot's own user id so the gateway can do precise
        // is_self / mention detection. Without it a mention-gated guild
        // channel can never admit a turn. Best-effort: on failure proceed
        // with None (DMs and explicit-id paths still work; mention gating
        // just stays conservative) rather than failing the whole start.
        let bot_id = match rest::get_current_user_id(&self.http, &self.api_base, &token).await {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channel_discord",
                    error = %e,
                    "could not resolve bot user id via /users/@me; mention/self detection degraded",
                );
                None
            }
        };

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
            bot_id,
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
        // Generate the dedup nonce once and reuse it across the retry loop
        // inside `rest::send_message` (HIGH-7): a retry after a lost success
        // re-sends the same nonce, which Discord dedupes instead of posting
        // a duplicate.
        let nonce = rest::next_nonce();
        let body = rest::CreateMessageBody {
            content: &msg.text,
            message_reference: reference,
            nonce: Some(&nonce),
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

    /// Setup and authentication probe — reference implementation for the
    /// PERSISTENT-CONNECTION half of the Phase 24 channel matrix.
    ///
    /// Answers all three setup questions WITHOUT opening the gateway and
    /// without sending a message: is `credential_handle` resolvable (config
    /// complete), does the token authenticate (`GET /users/@me`), and which bot
    /// identity did it authenticate as.
    ///
    /// # Why the bot id is worth a round trip
    ///
    /// A Discord bot token that is live but belongs to the WRONG application
    /// starts cleanly, IDENTIFYs cleanly, and then answers in the wrong
    /// server. `start()` cannot distinguish that case; `/users/@me` can, and
    /// it is the only part of this probe that costs a network call.
    ///
    /// # The three failure modes are three different operator actions
    ///
    /// A missing credential is an `Incomplete` (edit the credentials store), a
    /// rejected one is `Unauthenticated` (rotate the token), and an
    /// unreachable API is `Unreachable` (no verdict was reached at all — retry
    /// later). Collapsing these into one boolean is what makes an operator
    /// rotate a working token because the network was down.
    async fn probe(&self) -> Result<wcore_channels::ProbeReport, ChannelError> {
        use wcore_channels::ProbeReport;

        if self.config.credential_handle.trim().is_empty() {
            return Ok(ProbeReport::incomplete(
                &self.name,
                "discord",
                vec!["options.credential_handle".to_string()],
            ));
        }

        // NOTE: only the HANDLE is ever named in a finding. The token's value
        // never enters the report — see `wcore_channels::probe` on T-24-03-06.
        let token = match self.creds.get(&self.config.credential_handle) {
            Ok(Some(t)) => t,
            Ok(None) => {
                return Ok(ProbeReport::incomplete(
                    &self.name,
                    "discord",
                    vec![format!(
                        "credential {:?} is not present in the credentials store",
                        self.config.credential_handle
                    )],
                ));
            }
            Err(e) => {
                return Ok(ProbeReport::incomplete(
                    &self.name,
                    "discord",
                    vec![format!("credentials store unreadable: {e}")],
                ));
            }
        };

        match rest::get_current_user_id(&self.http, &self.api_base, &token).await {
            Ok(id) => Ok(ProbeReport::ok(&self.name, "discord", id)),
            // `DiscordError::Auth` is exactly "the platform looked at this
            // token and said no"; everything else is "we never got an answer".
            Err(DiscordError::Auth(reason)) => {
                Ok(ProbeReport::unauthenticated(&self.name, "discord", reason))
            }
            Err(other) => Ok(ProbeReport::unreachable(
                &self.name,
                "discord",
                other.to_string(),
            )),
        }
    }

    /// Discord's per-attachment ceiling for a non-boosted upload.
    fn media_bounds(&self) -> wcore_channels::MediaBounds {
        wcore_channels::MediaBounds {
            max_bytes: 25 * 1024 * 1024,
            max_attachments: 10,
        }
    }

    /// Discord caps a single message at 2000 characters.
    fn max_message_len(&self) -> Option<usize> {
        Some(2000)
    }

    /// `POST /channels/{id}/typing` — shows the bot as typing for ~10s.
    async fn send_typing(&self, conversation_id: &str) -> Result<(), ChannelError> {
        let token = self.bot_token.as_deref().ok_or(ChannelError::NotStarted)?;
        rest::trigger_typing(&self.http, &self.api_base, token, conversation_id)
            .await
            .map_err(ChannelError::from)
    }

    /// `PUT /channels/{id}/messages/{msg}/reactions/{emoji}/@me` — adds the
    /// bot's reaction (the ack signal). Unicode emoji are accepted directly.
    async fn react(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let token = self.bot_token.as_deref().ok_or(ChannelError::NotStarted)?;
        rest::add_reaction(
            &self.http,
            &self.api_base,
            token,
            conversation_id,
            message_id,
            emoji,
        )
        .await
        .map_err(ChannelError::from)
    }

    async fn fetch_media(
        &self,
        attachment: &wcore_channels::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        rest::download_bytes(&self.http, &attachment.url, rest::MEDIA_HOSTS)
            .await
            .map_err(ChannelError::from)
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

    #[test]
    fn max_message_len_is_discord_cap() {
        let creds = InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN);
        let ch = DiscordChannel::new("test", cfg(), creds);
        assert_eq!(ch.max_message_len(), Some(2000));
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

    // -----------------------------------------------------------------
    // 9. Setup and authentication probe (Phase 24, Criterion 3).
    //
    // Every case runs against a LOCAL mockito endpoint. No Discord token
    // and no network reach a vendor: the plan requires this proof to
    // reproduce on three platforms and in review, and a vendor outage
    // must not be able to turn a real defect into a green.
    // -----------------------------------------------------------------

    fn probe_channel(
        creds: Arc<dyn CredentialsStore>,
        api_base: String,
        config: DiscordConfig,
    ) -> DiscordChannel {
        DiscordChannel::with_bases(
            "test",
            config,
            creds,
            api_base,
            "ws://127.0.0.1:1".to_string(),
        )
    }

    #[tokio::test]
    async fn probe_reports_ok_with_the_bot_identity_without_opening_the_gateway() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/v10/users/@me")
            .match_header("authorization", format!("Bot {TEST_TOKEN}").as_str())
            .with_status(200)
            .with_body(r#"{"id":"998877","username":"acme-bot"}"#)
            .create_async()
            .await;

        let ch = probe_channel(
            InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN),
            server.url(),
            cfg(),
        );
        let report = ch.probe().await.expect("probe returns a report");
        mock.assert_async().await;

        assert_eq!(report.outcome, wcore_channels::ProbeOutcome::Ok);
        assert_eq!(
            report.identity.as_deref(),
            Some("998877"),
            "the identity is what distinguishes a live token for the WRONG \
             application from the right one; start() cannot tell them apart"
        );
        assert!(
            ch.task_handle().is_none(),
            "a probe must not open the gateway — it answers setup questions \
             without putting traffic on a production surface"
        );
    }

    #[tokio::test]
    async fn probe_separates_a_rejected_token_from_an_unreachable_api() {
        // These are opposite operator actions. A probe that folded them into
        // one boolean makes an operator rotate a working token because the
        // network was down.
        let mut server = mockito::Server::new_async().await;
        let rejected = server
            .mock("GET", "/api/v10/users/@me")
            .with_status(401)
            .with_body(r#"{"message":"401: Unauthorized","code":0}"#)
            .create_async()
            .await;
        let ch = probe_channel(
            InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN),
            server.url(),
            cfg(),
        );
        let report = ch.probe().await.unwrap();
        rejected.assert_async().await;
        assert_eq!(
            report.outcome,
            wcore_channels::ProbeOutcome::Unauthenticated,
            "HTTP 401 is the platform looking at the token and saying no"
        );
        assert!(
            report.config_complete,
            "the CONFIG was fine; the token was not"
        );

        // Nothing listening at all — no verdict was reached.
        let ch = probe_channel(
            InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN),
            "http://127.0.0.1:1".to_string(),
            cfg(),
        );
        let report = ch.probe().await.unwrap();
        assert_eq!(
            report.outcome,
            wcore_channels::ProbeOutcome::Unreachable,
            "a refused connection is not a credential verdict"
        );
    }

    #[tokio::test]
    async fn probe_reports_incomplete_when_the_credential_is_absent() {
        let creds: Arc<dyn CredentialsStore> = Arc::new(InMemoryCreds::new());
        let ch = probe_channel(creds, "http://unused".to_string(), cfg());
        let report = ch.probe().await.unwrap();
        assert_eq!(report.outcome, wcore_channels::ProbeOutcome::Incomplete);
        assert!(!report.config_complete);
        assert!(
            report.findings[0].contains("discord.test.bot_token"),
            "the finding must name the HANDLE so the operator knows where to \
             look; got {:?}",
            report.findings
        );
    }

    #[tokio::test]
    async fn probe_reports_incomplete_when_no_credential_handle_is_configured() {
        let ch = probe_channel(
            Arc::new(InMemoryCreds::new()),
            "http://unused".to_string(),
            DiscordConfig {
                credential_handle: String::new(),
                ..cfg()
            },
        );
        let report = ch.probe().await.unwrap();
        assert_eq!(report.outcome, wcore_channels::ProbeOutcome::Incomplete);
        assert_eq!(
            report.findings,
            vec!["options.credential_handle".to_string()]
        );
    }

    #[tokio::test]
    async fn probe_output_never_carries_the_bot_token() {
        // T-24-03-06 with a POSITIVE CONTROL: the token is provably in the
        // adapter's credentials store before its absence from the report means
        // anything.
        const CANARY: &str = "MTIz.ABCDEF.F24D-DISCORD-PROBE-CANARY-3b7f";
        let creds = InMemoryCreds::with_token("discord.test.bot_token", CANARY);
        assert_eq!(
            creds.get("discord.test.bot_token").unwrap().as_deref(),
            Some(CANARY),
            "POSITIVE CONTROL: the adapter really can read the canary"
        );

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v10/users/@me")
            .with_status(401)
            .with_body(r#"{"message":"401: Unauthorized"}"#)
            .create_async()
            .await;
        let ch = probe_channel(creds, server.url(), cfg());
        let report = ch.probe().await.unwrap();
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            !json.contains(CANARY),
            "the probe leaked the bot token into its report: {json}"
        );
    }
}
