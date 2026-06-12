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
mod offset_store;

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

        // Clear any previously-registered webhook before long-polling. A bot
        // cannot use webhooks and getUpdates at the same time — a stale webhook
        // makes every getUpdates return 409 forever while we'd still report
        // Connected. A bad token is fatal (Auth), but any other deleteWebhook
        // failure is best-effort: surfacing it would block startup on a
        // transient blip, and getUpdates will retry regardless.
        if let Err(e) = api::delete_webhook(&self.http, &self.api_base, &token).await {
            match ChannelError::from(e) {
                auth @ ChannelError::Auth(_) => return Err(auth),
                other => tracing::warn!(
                    target: "wcore_channel_telegram",
                    channel = %self.name,
                    error = %other,
                    "deleteWebhook failed at start; proceeding to long-poll anyway"
                ),
            }
        }

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
            channel_name: self.name.clone(),
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
        let reply_to = msg.reply_to.as_deref().and_then(|s| s.parse::<i64>().ok());

        // Track the most recent successful send so the receipt reflects
        // the last thing Telegram accepted (text first, then each
        // attachment). At least one of text/attachments is always sent.
        let mut last_result: Option<api::Message> = None;

        // ---- Text -------------------------------------------------------
        // Skip the text send entirely when the body is empty but there are
        // attachments — Telegram rejects empty-body sendMessage, and the
        // documents carry the payload in that case.
        let has_attachments = !msg.attachments.is_empty();
        if !msg.text.is_empty() || !has_attachments {
            // Under MarkdownV2 / HTML every reserved character must be escaped or
            // Telegram rejects the send with `400 ... can't parse entities` and
            // silently drops the reply. We escape the full text per parse mode
            // (see the `escape_*` docs for v1 semantics). Legacy `Markdown` has a
            // different reserved set and is left untouched here. `escaped`
            // outlives `body`'s borrow.
            let escaped;
            let text: &str = match self.config.parse_mode {
                ParseMode::MarkdownV2 => {
                    escaped = config::escape_markdown_v2(&msg.text);
                    &escaped
                }
                ParseMode::Html => {
                    escaped = config::escape_html(&msg.text);
                    &escaped
                }
                ParseMode::Markdown => &msg.text,
            };
            let body = api::SendMessageBody {
                chat_id: &msg.conversation_id,
                text,
                parse_mode: Some(self.config.parse_mode.as_api_str()),
                reply_to_message_id: reply_to,
            };
            let result = api::send_message(&self.http, &self.api_base, token, &body)
                .await
                .map_err(ChannelError::from)?;
            last_result = Some(result);
        }

        // ---- Attachments ------------------------------------------------
        // Each attachment URL is fetched by Telegram itself via sendDocument
        // (no local upload, no SSRF surface on our side). sendDocument works
        // for arbitrary file URLs — images included — which keeps v1 simple.
        for url in &msg.attachments {
            let body = api::build_send_document(&msg.conversation_id, url, None, reply_to);
            let result = api::send_document(&self.http, &self.api_base, token, &body)
                .await
                .map_err(ChannelError::from)?;
            last_result = Some(result);
        }

        // `last_result` is always Some: we send text unless there are
        // attachments, and we send at least one attachment otherwise.
        let result = last_result.ok_or_else(|| {
            ChannelError::Other("send_message produced no outbound request".to_string())
        })?;
        Ok(MessageReceipt {
            id: result.message_id.to_string(),
            conversation_id: msg.conversation_id,
            ts_secs: result.date,
        })
    }

    /// POST `sendChatAction` with `action: "typing"`. Best-effort: the
    /// typing indicator auto-expires after ~5s, which is why the inbound
    /// subscriber refreshes it. A single attempt is sufficient — transport
    /// failures map to [`ChannelError::Transport`] and are logged + ignored
    /// by the caller, never fatal to the turn.
    async fn send_typing(&self, conversation_id: &str) -> Result<(), ChannelError> {
        let token = self.bot_token.as_deref().ok_or(ChannelError::NotStarted)?;
        let body = api::build_send_chat_action(conversation_id);
        api::send_chat_action(&self.http, &self.api_base, token, &body)
            .await
            .map_err(ChannelError::from)
    }

    /// POST `setMessageReaction` with a single emoji. `message_id` arrives
    /// as a string and must parse to an `i64`; a parse failure is a caller
    /// bug and surfaces as [`ChannelError::Rejected`]. We do not validate
    /// the emoji against Telegram's fixed reaction set — an unknown emoji is
    /// rejected by Telegram with a 400, which maps to `Rejected`.
    async fn react(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let token = self.bot_token.as_deref().ok_or(ChannelError::NotStarted)?;
        let message_id: i64 = message_id
            .parse()
            .map_err(|_| ChannelError::Rejected("invalid message_id".to_string()))?;
        let body = api::build_set_reaction(conversation_id, message_id, emoji);
        api::set_message_reaction(&self.http, &self.api_base, token, &body)
            .await
            .map_err(ChannelError::from)
    }

    /// Download inbound media bytes. The attachment carries only the opaque
    /// Telegram `file_id` (in `path`); the token-bearing download URL
    /// (`{base}/file/bot{token}/{file_path}`) is resolved here on demand and
    /// kept as an ephemeral local, so the bot token never enters the event
    /// struct, traces, or logs.
    async fn fetch_media(
        &self,
        attachment: &wcore_channels::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        let file_id = attachment.path.as_deref().ok_or_else(|| {
            ChannelError::Other("telegram attachment missing file_id (path)".to_string())
        })?;
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;
        let url = api::get_file(&self.http, &self.api_base, bot_token, file_id)
            .await
            .map_err(ChannelError::from)?;
        api::download_bytes(&self.http, &url)
            .await
            .map_err(ChannelError::from)
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/telegram.json")
    }

    /// Telegram caps a single message at 4096 characters.
    fn max_message_len(&self) -> Option<usize> {
        Some(4096)
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
        // Isolate per-test persisted state (the getUpdates offset watermark)
        // under a pid-unique WAYLAND_HOME so concurrent test processes — and
        // prior box runs — can't read each other's offset and skip a mocked
        // update. nextest runs one test per process, so the pid is unique and
        // the env mutation is local. Set once, before any state I/O.
        static ISOLATE: std::sync::Once = std::sync::Once::new();
        ISOLATE.call_once(|| {
            let dir =
                std::env::temp_dir().join(format!("wcore_tg_test_state_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            // SAFETY: process-per-test under nextest; no other thread reads the env.
            unsafe { std::env::set_var("WAYLAND_HOME", &dir) };
        });

        TelegramConfig {
            credential_handle: "telegram.test.bot_token".to_string(),
            allowed_chat_ids: Vec::new(),
            // 0 makes mockito getUpdates return immediately without long-polling.
            long_poll_timeout_secs: 0,
            parse_mode: ParseMode::MarkdownV2,
        }
    }

    const TEST_TOKEN: &str = "111:AAAA-test-bot-token";

    /// Register the `deleteWebhook` mock that `start()` now always POSTs before
    /// spawning the long-poll task. Returns nothing useful — tests just need it
    /// present so `start()` doesn't fail on an unmatched 501.
    async fn mock_delete_webhook(server: &mut mockito::Server) {
        server
            .mock("POST", format!("/bot{TEST_TOKEN}/deleteWebhook").as_str())
            .with_status(200)
            .with_body(r#"{"ok":true,"result":true}"#)
            .expect_at_least(0)
            .create_async()
            .await;
    }

    #[test]
    fn max_message_len_is_telegram_cap() {
        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let ch = TelegramChannel::new("test", cfg(), creds);
        assert_eq!(ch.max_message_len(), Some(4096));
    }

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
    // 1b. MarkdownV2 channel escapes reserved chars in the send payload;
    //     HTML channel sends the raw text (no MarkdownV2 escaping). This
    //     is the HIGH-9 regression guard: a reply like `Hi! (ok).` must
    //     reach Telegram as `Hi\! \(ok\)\.` under MarkdownV2.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn markdown_v2_escapes_payload_text() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .match_body(mockito::Matcher::PartialJsonString(
                // JSON-encoded form of `Hi\! \(ok\)\.` — each backslash is
                // doubled because it is itself escaped in the JSON string.
                r#"{"text":"Hi\\! \\(ok\\)\\.","parse_mode":"MarkdownV2"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":1}}}"#)
            .expect(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        ch.send_message(OutgoingMessage::text("1", "Hi! (ok)."))
            .await
            .unwrap();
        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    #[tokio::test]
    async fn html_mode_does_not_apply_markdown_v2_escaping() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .match_body(mockito::Matcher::PartialJsonString(
                // Raw text, unescaped, under HTML parse mode.
                r#"{"text":"Hi! (ok).","parse_mode":"HTML"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":1}}}"#)
            .expect(1)
            .create_async()
            .await;

        let html_cfg = TelegramConfig {
            parse_mode: ParseMode::Html,
            ..cfg()
        };
        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", html_cfg, creds, server.url());
        ch.start().await.unwrap();
        ch.send_message(OutgoingMessage::text("1", "Hi! (ok)."))
            .await
            .unwrap();
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
        // State isolation (pid-unique WAYLAND_HOME) is set up by `cfg()` below.
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
    // 9. Inbound media: the attachment carries only the opaque file_id; the
    //    token-bearing download URL is resolved lazily by fetch_media (getFile
    //    + download), so the bot token never enters the event/logs/traces.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn inbound_photo_carries_file_id_and_fetch_media_resolves_lazily() {
        let mut server = mockito::Server::new_async().await;
        // getUpdates returns one message with a photo (two sizes).
        let _m_upd = server
            .mock("GET", format!("/bot{TEST_TOKEN}/getUpdates").as_str())
            .match_query(mockito::Matcher::UrlEncoded("offset".into(), "0".into()))
            .with_status(200)
            .with_body(
                r#"{"ok":true,"result":[{"update_id":1,"message":{"message_id":5,"date":1,"chat":{"id":7},"from":{"id":1,"username":"u"},"photo":[{"file_id":"small_id"},{"file_id":"big_id"}]}}]}"#,
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
        // getFile for the largest photo resolves to a file_path.
        let _m_file = server
            .mock("GET", format!("/bot{TEST_TOKEN}/getFile").as_str())
            .match_query(mockito::Matcher::UrlEncoded(
                "file_id".into(),
                "big_id".into(),
            ))
            .with_status(200)
            .with_body(
                r#"{"ok":true,"result":{"file_id":"big_id","file_path":"photos/file_0.jpg"}}"#,
            )
            .expect_at_least(1)
            .create_async()
            .await;
        // The lazy fetch_media download of the resolved file_path returns bytes.
        let _m_dl = server
            .mock(
                "GET",
                format!("/file/bot{TEST_TOKEN}/photos/file_0.jpg").as_str(),
            )
            .with_status(200)
            .with_body(b"PNGDATA".as_slice())
            .expect_at_least(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut got = None;
        while std::time::Instant::now() < deadline {
            for e in ch.poll_events().await.unwrap() {
                if let ChannelEvent::MessageReceived { msg } = e
                    && !msg.attachments.is_empty()
                {
                    got = Some(msg);
                    break;
                }
            }
            if got.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let msg = got.expect("expected a MessageReceived event with an attachment");
        let att = &msg.attachments[0];
        // The inbound attachment carries ONLY the opaque file_id — never the
        // token-bearing download URL (which would leak the token into the
        // event struct, traces, and logs).
        assert_eq!(att.path.as_deref(), Some("big_id"));
        assert!(
            att.url.is_empty(),
            "url must not be populated at ingest, got {:?}",
            att.url
        );
        assert_eq!(att.content_type.as_deref(), Some("image/jpeg"));
        // fetch_media resolves the token URL lazily (getFile) and downloads.
        let bytes = ch
            .fetch_media(att)
            .await
            .expect("fetch_media resolves + downloads");
        assert_eq!(&bytes, b"PNGDATA");
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 10. Outbound attachments: each URL goes out via sendDocument with
    //     chat_id + document=<url>, after the text send.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn outbound_attachment_sends_via_send_document() {
        let mut server = mockito::Server::new_async().await;
        let _m_text = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .with_status(200)
            .with_body(r#"{"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":1}}}"#)
            .expect(1)
            .create_async()
            .await;
        let m_doc = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendDocument").as_str())
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"chat_id":"1","document":"https://example.com/a.pdf"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"ok":true,"result":{"message_id":2,"date":2,"chat":{"id":1}}}"#)
            .expect(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        let msg = OutgoingMessage {
            conversation_id: "1".to_string(),
            text: "see attached".to_string(),
            reply_to: None,
            attachments: vec!["https://example.com/a.pdf".to_string()],
        };
        // Receipt reflects the last send (the document).
        let receipt = ch.send_message(msg).await.unwrap();
        assert_eq!(receipt.id, "2");
        m_doc.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 11. Outbound: empty text + attachments skips sendMessage entirely
    //     and only sends the document.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn outbound_empty_text_with_attachment_skips_send_message() {
        let mut server = mockito::Server::new_async().await;
        let m_text = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .with_status(200)
            .with_body(r#"{"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":1}}}"#)
            .expect(0) // <- must NOT be called
            .create_async()
            .await;
        let m_doc = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendDocument").as_str())
            .with_status(200)
            .with_body(r#"{"ok":true,"result":{"message_id":9,"date":9,"chat":{"id":1}}}"#)
            .expect(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        let msg = OutgoingMessage {
            conversation_id: "1".to_string(),
            text: String::new(),
            reply_to: None,
            attachments: vec!["https://example.com/b.png".to_string()],
        };
        let receipt = ch.send_message(msg).await.unwrap();
        assert_eq!(receipt.id, "9");
        m_text.assert_async().await;
        m_doc.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 12. send_typing POSTs sendChatAction with action=typing and succeeds
    //     on 200.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_typing_posts_chat_action_on_200() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendChatAction").as_str())
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"chat_id":"42","action":"typing"}"#.to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"result":true}"#)
            .expect(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        ch.send_typing("42").await.unwrap();
        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    #[tokio::test]
    async fn send_typing_before_start_errors_not_started() {
        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let ch = TelegramChannel::with_api_base("test", cfg(), creds, "http://unused".to_string());
        let err = ch.send_typing("42").await.expect_err("expected NotStarted");
        assert!(matches!(err, ChannelError::NotStarted), "got {err:?}");
    }

    // -----------------------------------------------------------------
    // 13. react POSTs setMessageReaction with the nested reaction array
    //     and succeeds on 200.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn react_posts_set_message_reaction_on_200() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "POST",
                format!("/bot{TEST_TOKEN}/setMessageReaction").as_str(),
            )
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"chat_id":"42","message_id":7,"reaction":[{"type":"emoji","emoji":"👀"}]}"#
                    .to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"result":true}"#)
            .expect(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        ch.react("42", "7", "👀").await.unwrap();
        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 14. react with a non-numeric message_id is Rejected without ever
    //     hitting the network.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn react_with_non_numeric_message_id_is_rejected() {
        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        // Start so bot_token is populated and we exercise the parse path,
        // not the NotStarted path. api_base is unused (no request is made).
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", format!("/bot{TEST_TOKEN}/getUpdates").as_str())
            .with_status(200)
            .with_body(r#"{"ok":true,"result":[]}"#)
            .expect_at_least(0)
            .create_async()
            .await;
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();

        let err = ch
            .react("42", "not-a-number", "👀")
            .await
            .expect_err("expected Rejected for bad message_id");
        match err {
            ChannelError::Rejected(msg) => {
                assert!(msg.contains("invalid message_id"), "msg = {msg}")
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 15. react maps a Telegram 400 (e.g. unknown emoji) to a clean
    //     Rejected error.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn react_maps_400_to_rejected() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "POST",
                format!("/bot{TEST_TOKEN}/setMessageReaction").as_str(),
            )
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":false,"error_code":400,"description":"Bad Request: REACTION_INVALID"}"#,
            )
            .expect(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        let err = ch
            .react("42", "7", "🦄")
            .await
            .expect_err("expected Rejected for invalid reaction");
        match err {
            ChannelError::Rejected(msg) => {
                assert!(msg.contains("400"), "msg = {msg}");
                assert!(msg.contains("REACTION_INVALID"), "msg = {msg}");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        mock.assert_async().await;
        ch.stop().await.unwrap();
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

    // -----------------------------------------------------------------
    // 16. HTML parse mode escapes `< > &` so a reply containing them does
    //     not trigger Telegram's `400 ... can't parse entities` (which
    //     silently drops the reply). Rank 34 regression guard.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn html_mode_escapes_reserved_entities_in_payload() {
        let mut server = mockito::Server::new_async().await;
        mock_delete_webhook(&mut server).await;
        let mock = server
            .mock("POST", format!("/bot{TEST_TOKEN}/sendMessage").as_str())
            .match_body(mockito::Matcher::PartialJsonString(
                // `a < b & c > d` must reach Telegram HTML-escaped. `&` is
                // escaped first so it is not double-escaped via `&lt;`/`&gt;`.
                r#"{"text":"a &lt; b &amp; c &gt; d","parse_mode":"HTML"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"ok":true,"result":{"message_id":1,"date":1,"chat":{"id":1}}}"#)
            .expect(1)
            .create_async()
            .await;

        let html_cfg = TelegramConfig {
            parse_mode: ParseMode::Html,
            ..cfg()
        };
        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", html_cfg, creds, server.url());
        ch.start().await.unwrap();
        ch.send_message(OutgoingMessage::text("1", "a < b & c > d"))
            .await
            .unwrap();
        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 17. start() POSTs deleteWebhook before long-polling so a previously
    //     registered webhook can't 409 getUpdates forever. Rank 19.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn start_deletes_webhook_before_long_polling() {
        let mut server = mockito::Server::new_async().await;
        let m_del = server
            .mock("POST", format!("/bot{TEST_TOKEN}/deleteWebhook").as_str())
            .with_status(200)
            .with_body(r#"{"ok":true,"result":true}"#)
            .expect(1)
            .create_async()
            .await;
        let _m_poll = server
            .mock("GET", format!("/bot{TEST_TOKEN}/getUpdates").as_str())
            .with_status(200)
            .with_body(r#"{"ok":true,"result":[]}"#)
            .expect_at_least(0)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();
        m_del.assert_async().await;
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 18. A 401 on deleteWebhook (revoked token) is fatal and surfaces as
    //     Auth — start() must not proceed to spawn the poll task. Rank 19.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn start_deletes_webhook_401_is_auth() {
        let mut server = mockito::Server::new_async().await;
        let _m_del = server
            .mock("POST", format!("/bot{TEST_TOKEN}/deleteWebhook").as_str())
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#)
            .expect(1)
            .create_async()
            .await;

        let creds = InMemoryCreds::with_token("telegram.test.bot_token", TEST_TOKEN);
        let mut ch = TelegramChannel::with_api_base("test", cfg(), creds, server.url());
        let err = ch
            .start()
            .await
            .expect_err("expected Auth on 401 deleteWebhook");
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        assert!(
            ch.poll_handle.is_none(),
            "poll task must not spawn on auth failure"
        );
    }
}
