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
}

#[async_trait]
impl Channel for MatrixChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "matrix"
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.poll_handle.is_some() {
            // Already running — idempotent.
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
        )
        .await
        .map_err(|e| ChannelError::Transport(e.to_string()))?;

        Ok(MessageReceipt {
            id: event_id,
            conversation_id: msg.conversation_id.clone(),
            ts_secs: chrono::Utc::now().timestamp(),
        })
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/matrix.json")
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
        // The transaction ID is a counter; first call = 1.
        let mock = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(
                    r"/_matrix/client/v3/rooms/[^/]+/send/m\.room\.message/\d+".to_string(),
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
}
