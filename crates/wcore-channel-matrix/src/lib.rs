//! `wcore-channel-matrix` — Matrix CS API channel adapter.
//!
//! **Scope**: Outbound send via `PUT /_matrix/client/v3/rooms/{roomId}/send/m.room.message/{txnId}`.
//! Inbound via `GET /_matrix/client/v3/sync` long-poll on a background task
//! spawned in `start()`; `poll_events` drains the shared inbox the task fills.
//!
//! Avoids `matrix-sdk` to keep build time down (`matrix-sdk` + crypto WASM
//! adds >5 min to clean builds). Raw REST is sufficient for the send use-case.
//!
//! Credentials: access token via wcore-config credentials store. The homeserver
//! URL and user ID are config fields (not secrets).
//!
//! Ported from the desktop app's TypeScript `MatrixPlugin` (Apache-2.0).
//! See F-045 in the wcore audit triage.

pub mod config;
pub mod error;
mod rest;
mod sync;
mod sync_store;

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

use wcore_channels::Channel;
use wcore_channels::error::ChannelError;
use wcore_channels::event::{ChannelEvent, ConnectionState, MessageReceipt};
use wcore_channels::outgoing::OutgoingMessage;
use wcore_config::credentials::CredentialsStore;

pub use config::MatrixConfig;
pub use error::MatrixError;

/// Production Matrix channel adapter.
pub struct MatrixChannel {
    name: String,
    config: MatrixConfig,
    state: ConnectionState,
    access_token: Option<String>,
    http: wcore_egress::EgressClient,
    /// Background `/sync` task pushes into this; `poll_events` drains it.
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    /// Handle to the background `/sync` long-poll task; `None` until started.
    poll_handle: Option<JoinHandle<()>>,
    /// Shutdown signal for the `/sync` task; `None` until started.
    shutdown: Option<watch::Sender<bool>>,
    creds: Arc<dyn CredentialsStore>,
    /// Override for tests.
    api_base: String,
}

impl MatrixChannel {
    pub fn new(
        name: impl Into<String>,
        config: MatrixConfig,
        creds: Arc<dyn CredentialsStore>,
    ) -> Self {
        let api_base = config.homeserver_url.clone();
        Self::with_base(name, config, creds, api_base)
    }

    #[doc(hidden)]
    pub fn with_base(
        name: impl Into<String>,
        config: MatrixConfig,
        creds: Arc<dyn CredentialsStore>,
        api_base: String,
    ) -> Self {
        let http = wcore_egress::EgressClient::builder()
            .user_agent(concat!("wayland-core/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();

        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            access_token: None,
            http,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            poll_handle: None,
            shutdown: None,
            creds,
            api_base,
        }
    }

    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// The one send path, keyed or not.
    ///
    /// Both trait methods route through here so there is exactly one place
    /// where a transaction id reaches the wire. Two send paths would let the
    /// keyed one drift away from the unkeyed one and quietly stop transmitting
    /// the key while `supports_outbound_idempotency` still claimed it did.
    async fn put_message(
        &mut self,
        msg: OutgoingMessage,
        delivery_key: Option<&str>,
    ) -> Result<MessageReceipt, ChannelError> {
        let token = self
            .access_token
            .as_deref()
            .ok_or(ChannelError::NotStarted)?;

        let event_id = rest::send_text_message(
            &self.http,
            &self.api_base,
            token,
            &msg.conversation_id,
            &msg.text,
            delivery_key,
        )
        .await
        .map_err(|e| ChannelError::Transport(e.to_string()))?;

        Ok(MessageReceipt {
            id: event_id,
            conversation_id: msg.conversation_id.clone(),
            ts_secs: chrono::Utc::now().timestamp(),
        })
    }
}

#[async_trait]
impl Channel for MatrixChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "matrix"
    }

    fn task_handle(&self) -> Option<&tokio::task::JoinHandle<()>> {
        self.poll_handle.as_ref()
    }

    /// Conservative per-message body cap. A Matrix event must serialize under
    /// the spec's 65536-byte hard limit (including all envelope fields), so a
    /// homeserver rejects an over-long `body`. Declaring the cap makes the
    /// channel manager chunk long replies instead of sending one rejected event.
    fn max_message_len(&self) -> Option<usize> {
        Some(32_768)
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.poll_handle.as_ref().is_some_and(|h| !h.is_finished()) {
            // Already running — idempotent. A finished handle (the /sync task
            // died) falls through to respawn so supervised reconnect heals the
            // channel instead of treating a dead task as alive.
            return Ok(());
        }
        self.state = ConnectionState::Connecting;

        let token = self
            .creds
            .get(&self.config.credential_handle_access_token)
            .map_err(|e| ChannelError::Auth(format!("credentials lookup: {e}")))?
            .ok_or_else(|| {
                ChannelError::Auth(format!(
                    "Matrix access token not found at {:?}",
                    self.config.credential_handle_access_token
                ))
            })?;

        self.access_token = Some(token.clone());

        // Emit a Connected state-change so subscribers know the channel
        // went live (the manager will tag and broadcast it).
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connected,
            });

        // Spawn the /sync long-poll task.
        let (tx, rx) = watch::channel(false);
        let args = sync::SyncArgs {
            http: self.http.clone(),
            api_base: self.api_base.clone(),
            access_token: token,
            user_id: self.config.user_id.clone(),
            inbox: Arc::clone(&self.inbox),
            shutdown: rx,
            // F24-C3-H6 — the `/sync` cursor survives this process, keyed by
            // the account it belongs to, so a restart resumes instead of
            // discarding the downtime window as an initial sync.
            state_path: sync_store::state_path(&self.api_base, &self.config.user_id, &self.name),
        };
        let handle = tokio::spawn(sync::sync_loop(args));
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
            // Give the loop a brief moment to observe the shutdown signal and
            // drop out; if it lingers past the grace window (e.g. parked in a
            // long /sync read), abort it. `timeout(dur, handle)` would only
            // DROP the handle on elapse — which DETACHES, not aborts, the task,
            // leaking it — so race the join against a sleep and abort
            // explicitly via the AbortHandle on the timeout arm.
            let abort = handle.abort_handle();
            tokio::select! {
                _ = handle => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                    abort.abort();
                    tracing::warn!(
                        target: "wcore_channel_matrix",
                        channel = %self.name,
                        "/sync task did not exit within shutdown grace; aborted"
                    );
                }
            }
        }
        self.access_token = None;
        self.state = ConnectionState::Disconnected;
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Disconnected,
            });
        Ok(())
    }

    /// Drains the shared inbox the background `/sync` task fills.
    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        Ok(self.inbox.lock().await.drain(..).collect())
    }

    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        self.put_message(msg, None).await
    }

    /// Matrix's transaction id IS an idempotency key, so the delivery key is
    /// carried on the wire rather than ignored: the id is derived from it and
    /// is therefore identical when the same logical delivery is replayed after
    /// a restart. The homeserver returns the original `event_id` and posts
    /// nothing.
    async fn send_message_idempotent(
        &mut self,
        msg: OutgoingMessage,
        key: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        self.put_message(msg, Some(key)).await
    }

    /// This adapter DOES transmit the key — as the `{txnId}` path segment of
    /// the send PUT (see [`rest::send_text_message`]) — so the delivery spine
    /// may retry an outcome-unknown delivery through it.
    ///
    /// This returned `false` until the transaction id stopped coming from a
    /// counter that reset to 1 on every process start. That default was HONEST
    /// then: the adapter was putting an id on the wire that could not survive
    /// the restart it was supposed to cover. Flipping it without fixing the id
    /// would have converted a visible duplicate into an invisible one.
    ///
    /// `matrix_declares_idempotency_only_because_the_txn_id_is_derived_from_the_key`
    /// binds this claim to the wire.
    fn supports_outbound_idempotency(&self) -> bool {
        true
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/matrix.json")
    }

    /// `PUT /rooms/{room}/typing/{userId}` — the bot's own `user_id` (a
    /// config field) is the path subject. 30s server-side timeout; the
    /// subscriber re-sends on a shorter cadence while a turn runs.
    async fn send_typing(&self, conversation_id: &str) -> Result<(), ChannelError> {
        let token = self
            .access_token
            .as_deref()
            .ok_or(ChannelError::NotStarted)?;
        rest::send_typing(
            &self.http,
            &self.api_base,
            token,
            conversation_id,
            &self.config.user_id,
            30_000,
        )
        .await
        .map_err(|e| ChannelError::Transport(e.to_string()))
    }

    /// `m.reaction` annotation relating to the inbound event — the ack
    /// signal. `message_id` is the Matrix `event_id`.
    async fn react(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let token = self
            .access_token
            .as_deref()
            .ok_or(ChannelError::NotStarted)?;
        rest::send_reaction(
            &self.http,
            &self.api_base,
            token,
            conversation_id,
            message_id,
            emoji,
        )
        .await
        .map_err(|e| ChannelError::Transport(e.to_string()))
    }

    /// Download unencrypted inbound media by its `mxc://` URI via the
    /// authenticated media endpoint. `attachment.url` carries the `mxc://`
    /// URI mapped by the `/sync` parser.
    async fn fetch_media(
        &self,
        attachment: &wcore_channels::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        let token = self
            .access_token
            .as_deref()
            .ok_or(ChannelError::NotStarted)?;
        rest::download_media(&self.http, &self.api_base, token, &attachment.url)
            .await
            .map_err(|e| ChannelError::Transport(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use wcore_config::credentials::{CredentialsError, CredentialsStore as CredsTrait};

    struct MemCreds {
        inner: StdMutex<std::collections::HashMap<String, String>>,
    }
    impl MemCreds {
        fn with_token(handle: &str, token: &str) -> Arc<dyn CredsTrait> {
            let s = Self {
                inner: StdMutex::new(std::collections::HashMap::new()),
            };
            s.inner
                .lock()
                .unwrap()
                .insert(handle.to_string(), token.to_string());
            Arc::new(s)
        }
        fn empty() -> Arc<dyn CredsTrait> {
            Arc::new(Self {
                inner: StdMutex::new(std::collections::HashMap::new()),
            })
        }
    }
    impl CredsTrait for MemCreds {
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

    fn cfg() -> MatrixConfig {
        MatrixConfig {
            homeserver_url: "https://matrix.example.org".to_string(),
            credential_handle_access_token: "matrix.test.token".to_string(),
            user_id: "@bot:matrix.example.org".to_string(),
        }
    }

    const TEST_TOKEN: &str = "syt_test_token_abc123";
    const TEST_ROOM: &str = "!room123:matrix.example.org";

    // 1. Config round-trip through ChannelConfig.options.
    #[test]
    fn config_round_trip_via_channel_config_options() {
        let raw = r#"
name = "acme-matrix"
platform = "matrix"

[options]
homeserver_url = "https://matrix.example.org"
credential_handle_access_token = "matrix.acme.token"
user_id = "@bot:matrix.example.org"
"#;
        let outer: wcore_channels::ChannelConfig = toml::from_str(raw).unwrap();
        let cfg: MatrixConfig = outer.options.try_into().unwrap();
        assert_eq!(cfg.homeserver_url, "https://matrix.example.org");
        assert_eq!(cfg.credential_handle_access_token, "matrix.acme.token");
        assert_eq!(cfg.user_id, "@bot:matrix.example.org");
    }

    // 2. platform() returns "matrix".
    #[test]
    fn platform_tag_is_matrix() {
        let ch = MatrixChannel::new("test", cfg(), MemCreds::empty());
        assert_eq!(ch.platform(), "matrix");
    }

    // 3. send_message before start surfaces NotStarted.
    #[tokio::test]
    async fn send_before_start_errors_not_started() {
        let mut ch = MatrixChannel::new("test", cfg(), MemCreds::empty());
        let err = ch
            .send_message(OutgoingMessage::text(TEST_ROOM, "hello"))
            .await
            .expect_err("should be NotStarted");
        assert!(matches!(err, ChannelError::NotStarted));
    }

    // 4. start() with missing credential surfaces Auth.
    #[tokio::test]
    async fn start_with_missing_token_errors_auth() {
        let mut ch = MatrixChannel::new("test", cfg(), MemCreds::empty());
        let err = ch.start().await.expect_err("expected Auth");
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
    }

    // 5. send_message hits PUT /_matrix/client/v3/rooms/{roomId}/send/m.room.message/{txn}.
    #[tokio::test]
    async fn send_message_succeeds_on_200() {
        let mut server = mockito::Server::new_async().await;
        // An UNKEYED send carries a process-unique `wl-u{ms:x}-{n:x}` id. It
        // used to be a bare counter starting at 1; that is the defect, because
        // a fresh process re-walked ids the homeserver still held and its new
        // messages were dropped as replays.
        let mock = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(
                    r"/_matrix/client/v3/rooms/[^/]+/send/m\.room\.message/wl-u[0-9a-f]+-[0-9a-f]+"
                        .to_string(),
                ),
            )
            .match_header("authorization", format!("Bearer {TEST_TOKEN}").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"event_id":"$abc123"}"#)
            .create_async()
            .await;

        let creds = MemCreds::with_token("matrix.test.token", TEST_TOKEN);
        let mut ch = MatrixChannel::with_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text(
                "!room123:matrix.example.org",
                "hello Matrix",
            ))
            .await
            .unwrap();

        assert_eq!(receipt.id, "$abc123");
        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    /// The capability declaration and the wire must agree.
    ///
    /// `supports_outbound_idempotency()` returning `true` is what permits the
    /// gateway's delivery spine to retry an outcome-unknown delivery through
    /// this adapter. If the transaction id stopped being derived from the
    /// delivery key while that claim stood, every such retry would become a
    /// second message in the room. The mock matches the EXACT path segment, so
    /// reverting the derivation reddens here rather than only in a live run.
    #[tokio::test]
    async fn matrix_declares_idempotency_only_because_the_txn_id_is_derived_from_the_key() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "PUT",
                "/_matrix/client/v3/rooms/%21room123%3Amatrix.example.org/send/\
                 m.room.message/cron:job-a:1785121776528",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"event_id":"$abc123"}"#)
            .expect(1)
            .create_async()
            .await;

        let creds = MemCreds::with_token("matrix.test.token", TEST_TOKEN);
        let mut ch = MatrixChannel::with_base("test", cfg(), creds, server.url());
        assert!(
            ch.supports_outbound_idempotency(),
            "matrix claims it can deduplicate a replay"
        );
        ch.start().await.unwrap();

        ch.send_message_idempotent(
            OutgoingMessage::text("!room123:matrix.example.org", "hello"),
            "cron:job-a:1785121776528",
        )
        .await
        .unwrap();

        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    /// The property that makes the token worth anything: the SAME logical
    /// delivery produces the SAME transaction id from a DIFFERENT process.
    ///
    /// A process-local counter cannot do this, and that is the whole defect.
    /// The test is a pure function check because it is asserting a property of
    /// the derivation, not of one process's state — no counter, no clock, no
    /// prior call can influence it.
    #[test]
    fn the_txn_id_is_stable_for_one_delivery_and_distinct_across_deliveries() {
        use crate::rest::txn_id_for_key;
        let a = txn_id_for_key("cron:job-a:1785121776528");
        let again = txn_id_for_key("cron:job-a:1785121776528");
        assert_eq!(a, again, "the same delivery must map to the same txn id");

        // A different occurrence of the SAME job must NOT collapse — that
        // would make the homeserver drop the second as a replay, which is a
        // message loss rather than a duplicate.
        let next_occurrence = txn_id_for_key("cron:job-a:1785121776529");
        assert_ne!(a, next_occurrence);
        let other_job = txn_id_for_key("cron:job-b:1785121776528");
        assert_ne!(a, other_job);

        // A key needing escaping is hashed, and still stable + distinct.
        let odd = txn_id_for_key("cron:job a/b?:1");
        let odd_again = txn_id_for_key("cron:job a/b?:1");
        assert_eq!(odd, odd_again);
        assert!(
            odd.starts_with("wl-") && !odd.contains(['/', '?', ' ']),
            "an unsafe key must be hashed into a path-safe id, got {odd}"
        );
        assert_ne!(odd, txn_id_for_key("cron:job a/b?:2"));
    }
}
