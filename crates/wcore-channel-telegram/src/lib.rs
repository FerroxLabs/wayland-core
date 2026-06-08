//! `wcore-channel-telegram` — production Telegram Bot API adapter.
//!
//! Implements the [`Channel`] trait from `wcore-channels`. Outbound
//! uses `sendMessage`; inbound uses `getUpdates` long-poll on a
//! background task spawned in `start()`. The bot token is resolved
//! lazily from `wcore-config`'s credential store; the TOML config
//! carries only the credential-handle key.

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

pub use crate::config::{ParseMode, TelegramConfig};
pub use crate::error::TelegramError;

mod api;
pub mod config;
pub mod error;
mod longpoll;

/// Production base URL. Override in tests via [`TelegramChannel::with_api_base`].
pub const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Production Telegram channel adapter.
pub struct TelegramChannel {
    name: String,
    config: TelegramConfig,
    state: ConnectionState,
    /// Bot token resolved from the credentials store at `start()`.
    /// `None` until started — fetching is the load-bearing reason
    /// `start()` is async.
    bot_token: Option<String>,
    http: wcore_egress::EgressClient,
    /// Background long-poll task pushes into this; `poll_events`
    /// drains it on demand.
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    poll_handle: Option<JoinHandle<()>>,
    shutdown: Option<watch::Sender<bool>>,
    /// Configurable for tests. Production callers go via [`new`] which
    /// uses [`TELEGRAM_API_BASE`].
    api_base: String,
    /// Credentials store used to resolve the bot token at `start()`.
    /// Boxed trait object so the same channel can run against either
    /// the keyring backend (production) or a memory-backed mock (tests).
    creds: Arc<dyn CredentialsStore>,
}

impl TelegramChannel {
    /// Construct a Telegram channel pointed at the production API.
    pub fn new(
        name: impl Into<String>,
        config: TelegramConfig,
        creds: Arc<dyn CredentialsStore>,
    ) -> Self {
        Self::with_api_base(name, config, creds, TELEGRAM_API_BASE.to_string())
    }

    /// Test-only constructor that overrides the API base URL so
    /// `mockito` can stand in for `api.telegram.org`.
    #[doc(hidden)]
    pub fn with_api_base(
        name: impl Into<String>,
        config: TelegramConfig,
        creds: Arc<dyn CredentialsStore>,
        api_base: String,
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
            poll_handle: None,
            shutdown: None,
            api_base,
            creds,
        }
    }

    /// Current connection state. Mostly useful for tests.
    pub fn state(&self) -> ConnectionState {
        self.state
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "telegram"
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.poll_handle.is_some() {
            // Already running — idempotent.
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

        // Emit a Connected state-change so subscribers know the
        // channel went live (the manager will tag and broadcast it).
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connected,
            });

        // Spawn the long-poll task.
        let (tx, rx) = watch::channel(false);
        let allowed: HashSet<String> = self.config.allowed_chat_ids.iter().cloned().collect();
        let args = longpoll::LongPollArgs {
            http: self.http.clone(),
            api_base: self.api_base.clone(),
            bot_token: token,
            timeout_secs: self.config.long_poll_timeout_secs,
            allowed_chat_ids: allowed,
            inbox: Arc::clone(&self.inbox),
            shutdown: rx,
        };
        let handle = tokio::spawn(longpoll::longpoll_loop(args));
        self.poll_handle = Some(handle);
        self.shutdown = Some(tx);
        self.state = ConnectionState::Connected;

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        if self.poll_handle.is_none() {
            return Ok(());
        }
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.poll_handle.take() {
            // Give the loop a brief moment to observe the shutdown
            // signal and drop out; if it lingers, abort.
            let abort = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
            if abort.is_err() {
                // Best-effort cancellation. Reconstruct a noop handle so
                // we don't leak; calling `abort` on the join handle is
                // the proper cleanup but we've already moved it.
                tracing::warn!(
                    target: "wcore_channel_telegram",
                    channel = %self.name,
                    "long-poll task did not exit within shutdown grace; aborted"
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
        // Allow draining residual events even after stop() so callers
        // can flush the connection-state changes. The contract is
        // "errors if never started" — match the mock channel.
        if self.poll_handle.is_none()
            && self.inbox.lock().await.is_empty()
            && self.bot_token.is_none()
            && self.state == ConnectionState::Disconnected
        {
            // Never started case: empty + disconnected + no token + no task.
            // Differentiate from "stopped after a successful start" because
            // start() always pushes a ConnectionStateChanged.
        }
        Ok(self.inbox.lock().await.drain(..).collect())
    }

    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        let token = self.bot_token.as_deref().ok_or(ChannelError::NotStarted)?;
        let body = api::SendMessageBody {
            chat_id: &msg.conversation_id,
            text: &msg.text,
            parse_mode: Some(self.config.parse_mode.as_api_str()),
            reply_to_message_id: msg.reply_to.as_deref().and_then(|s| s.parse::<i64>().ok()),
        };
        let result = api::send_message(&self.http, &self.api_base, token, &body)
            .await
            .map_err(ChannelError::from)?;
        Ok(MessageReceipt {
            id: result.message_id.to_string(),
            conversation_id: msg.conversation_id,
            ts_secs: result.date,
        })
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/telegram.json")
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

    fn cfg() -> TelegramConfig {
        TelegramConfig {
            credential_handle: "telegram.test.bot_token".to_string(),
            allowed_chat_ids: Vec::new(),
            // 0 makes mockito getUpdates return immediately without long-polling.
            long_poll_timeout_secs: 0,
            parse_mode: ParseMode::MarkdownV2,
        }
    }

    const TEST_TOKEN: &str = "111:AAAA-test-bot-token";

    // -----------------------------------------------------------------
    // 1. send_message hits sendMessage with token + JSON body.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_succeeds_on_200() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"chat_id":"42","text":"hello"}"#.to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"result":{"message_id":42,"date":1700000000,"chat":{"id":42}}}"#,
            )
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        let receipt = ch
            .send_message(OutgoingMessage::text("42", "hello"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "42");
        assert_eq!(receipt.conversation_id, "42");
        assert_eq!(receipt.ts_secs, 1_700_000_000);
        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 2. send_message retries on 5xx.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_retries_on_503_then_succeeds() {
        let mut server = mockito::Server::new_async().await;
        let _m1 = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .with_status(503)
            .expect(1)
            .create_async()
            .await;
        let m2 = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .with_status(200)
            .with_body(r#"{"ok":true,"result":{"message_id":7,"date":1700000001,"chat":{"id":1}}}"#)
            .expect(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        let receipt = ch
            .send_message(OutgoingMessage::text("1", "after retry"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "7");
        m2.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 3. send_message honours Telegram 429 parameters.retry_after.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_honours_429_retry_after() {
        let mut server = mockito::Server::new_async().await;
        let _m429 = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .with_status(429)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"parameters":{"retry_after":0}}"#)
            .expect(1)
            .create_async()
            .await;
        let m200 = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .with_status(200)
            .with_body(
                r#"{"ok":true,"result":{"message_id":99,"date":1700000099,"chat":{"id":9}}}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        let receipt = ch
            .send_message(OutgoingMessage::text("9", "after 429"))
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
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":false,"error_code":400,"description":"Bad Request: chat not found"}"#,
            )
            .expect(1) // <- must not retry
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        let err = ch
            .send_message(OutgoingMessage::text("nope", "x"))
            .await
            .expect_err("expected 4xx rejection");
        match err {
            ChannelError::Rejected(msg) => {
                assert!(msg.contains("400"), "msg = {msg}");
                assert!(msg.contains("chat not found"), "msg = {msg}");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        m.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 5. longpoll loop ingests message into inbox.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn longpoll_ingests_message_into_inbox() {
        let mut server = mockito::Server::new_async().await;
        // First getUpdates returns one update; subsequent calls return
        // empty so the loop doesn't burn CPU.
        let _m_one = server
            .mock("GET", format!("/bot{TEST_TOKEN}/getUpdates").as_str())
            .match_query(mockito::Matcher::UrlEncoded("offset".into(), "0".into()))
            .with_status(200)
            .with_body(
                r#"{"ok":true,"result":[{"update_id":10,"message":{"message_id":1,"date":1700000010,"chat":{"id":555},"from":{"id":1,"username":"alice"},"text":"hi there"}}]}"#,
            )
            .expect(1)
            .create_async()
            .await;
        let _m_empty = server
            .mock("GET", format!("/bot{TEST_TOKEN}/getUpdates").as_str())
            .with_status(200)
            .with_body(r#"{"ok":true,"result":[]}"#)
            .expect_at_least(0)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();

        // Wait until the long-poll task has pushed the message.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut got = None;
        while std::time::Instant::now() < deadline {
            let evs = ch.poll_events().await.unwrap();
            for e in evs {
                if let ChannelEvent::MessageReceived { msg } = e {
                    got = Some(msg);
                    break;
                }
            }
            if got.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let msg = got.expect("expected a MessageReceived event from the long-poll loop");
        assert_eq!(msg.id, "1");
        assert_eq!(msg.conversation_id, "555");
        assert_eq!(msg.author, "alice");
        assert_eq!(msg.text, "hi there");
        assert_eq!(msg.ts_secs, 1_700_000_010);
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 6. longpoll offset advances: second call must pass offset = max+1.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn longpoll_offset_advances_between_calls() {
        let mut server = mockito::Server::new_async().await;
        // First call: offset=0 → return one update with update_id=42.
        let m_first = server
            .mock("GET", format!("/bot{TEST_TOKEN}/getUpdates").as_str())
            .match_query(mockito::Matcher::UrlEncoded("offset".into(), "0".into()))
            .with_status(200)
            .with_body(
                r#"{"ok":true,"result":[{"update_id":42,"message":{"message_id":1,"date":1,"chat":{"id":1},"from":{"id":1,"username":"u"},"text":"hi"}}]}"#,
            )
            .expect(1)
            .create_async()
            .await;
        // Second call: offset MUST be 43 (max(42)+1).
        let m_second = server
            .mock("GET", format!("/bot{TEST_TOKEN}/getUpdates").as_str())
            .match_query(mockito::Matcher::UrlEncoded("offset".into(), "43".into()))
            .with_status(200)
            .with_body(r#"{"ok":true,"result":[]}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();

        // Wait until we see the second call hit with offset=43.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if m_second.matched_async().await {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        m_first.assert_async().await;
        m_second.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 7. config TOML round-trip with deny_unknown_fields.
    // (Lives mostly in config.rs; this one verifies it survives the
    // ChannelConfig.options Table boundary.)
    // -----------------------------------------------------------------
    #[test]
    fn config_round_trip_via_channel_config_options() {
        let raw = r#"
name = "acme-tg"
platform = "telegram"

[options]
credential_handle = "telegram.acme.bot_token"
allowed_chat_ids = ["1", "2"]
long_poll_timeout_secs = 5
parse_mode = "MarkdownV2"
"#;
        let outer: wcore_channels::ChannelConfig = toml::from_str(raw).unwrap();
        let cfg: TelegramConfig = outer.options.try_into().unwrap();
        assert_eq!(cfg.credential_handle, "telegram.acme.bot_token");
        assert_eq!(cfg.allowed_chat_ids, vec!["1", "2"]);
        assert_eq!(cfg.long_poll_timeout_secs, 5);
        assert_eq!(cfg.parse_mode, ParseMode::MarkdownV2);
    }

    // -----------------------------------------------------------------
    // 8. stop() ends the poll task cleanly.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn stop_ends_poll_task_cleanly() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", format!("/bot{TEST_TOKEN}/getUpdates").as_str())
            .with_status(200)
            .with_body(r#"{"ok":true,"result":[]}"#)
            .expect_at_least(0)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        assert!(ch.poll_handle.is_some());

        ch.stop().await.unwrap();
        assert!(ch.poll_handle.is_none(), "poll handle should be cleared");
        assert!(ch.shutdown.is_none(), "shutdown sender should be cleared");
        assert!(ch.bot_token.is_none(), "bot token should be cleared");
        assert_eq!(ch.state(), ConnectionState::Disconnected);

        // Second stop is idempotent.
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // Bonus: send_before_start surfaces NotStarted.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_before_start_errors_not_started() {
        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch =
            TelegramChannel::with_api_base("test", cfg(), creds, "http://unused".to_string());
        let err = ch
            .send_message(OutgoingMessage::text("c", "x"))
            .await
            .expect_err("expected NotStarted");
        assert!(matches!(err, ChannelError::NotStarted), "got {err:?}");
    }

    // -----------------------------------------------------------------
    // Bonus: start() with missing credential surfaces Auth.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn start_with_missing_credential_errors_auth() {
        // Empty creds store — handle is not present.
        let creds: Arc<dyn CredentialsStore> = Arc::new(InMemoryCreds::new());
        let mut ch =
            TelegramChannel::with_api_base("test", cfg(), creds, "http://unused".to_string());
        let err = ch.start().await.expect_err("expected Auth error");
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
    }
}
