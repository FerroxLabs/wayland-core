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
mod token;

/// The single source of this adapter's inbound media bounds.
///
/// [`Channel::media_bounds`] returns this, and `rest::MAX_MEDIA_BYTES` — the
/// cap `rest::download_media` streams the body under — is derived from it. One
/// constant, both sites, so the advertised number and the enforced number
/// cannot drift apart.
///
/// This adapter previously declared NOTHING, so it advertised the 25 MiB trait
/// default while enforcing a hardcoded 100 MiB, because the declaration had no
/// reader anywhere in the workspace. 100 MiB is the value that has actually
/// governed inbound fetches since 2026-06-18.
pub const MEDIA_BOUNDS: wcore_channels::MediaBounds = wcore_channels::MediaBounds {
    max_bytes: 100 * 1024 * 1024,
    max_attachments: 10,
};

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

use token::{Renewal, TokenSource, TokenSourceParams};

/// Production Matrix channel adapter.
pub struct MatrixChannel {
    name: String,
    config: MatrixConfig,
    state: ConnectionState,
    /// The live access token and the only path that renews it, shared with the
    /// `/sync` task. A plain `String` here was the whole of #936: the token was
    /// read once in `start()` and could never be replaced, so an expiring
    /// credential (the OIDC / Matrix Authentication Service default) took the
    /// channel down permanently. `None` until started.
    tokens: Option<Arc<TokenSource>>,
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
            tokens: None,
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
        &self,
        msg: OutgoingMessage,
        delivery_key: Option<&str>,
    ) -> Result<MessageReceipt, ChannelError> {
        let room: &str = &msg.conversation_id;
        let text: &str = &msg.text;
        let event_id = self
            .with_access_token(|token| async move {
                rest::send_text_message(
                    &self.http,
                    &self.api_base,
                    &token,
                    room,
                    text,
                    delivery_key,
                )
                .await
            })
            .await?;

        Ok(MessageReceipt {
            id: event_id,
            conversation_id: msg.conversation_id.clone(),
            ts_secs: chrono::Utc::now().timestamp(),
        })
    }

    /// Run one authenticated call, renewing the credential once if the
    /// homeserver rejects it.
    ///
    /// **Every** outbound call goes through here, so there is exactly one
    /// place that decides what a 401 means on the send side — and it is the
    /// same decision the `/sync` loop takes, because both delegate to
    /// [`TokenSource`]. A send path with its own opinion is how "the channel
    /// reports healthy and every message fails" gets built.
    ///
    /// Retrying is safe: a call the homeserver answered 401 was never applied,
    /// so the second attempt cannot duplicate a delivery.
    async fn with_access_token<T, F, Fut>(&self, op: F) -> Result<T, ChannelError>
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = Result<T, MatrixError>>,
    {
        let tokens = self.tokens.as_ref().ok_or(ChannelError::NotStarted)?;
        let presented = tokens.access();
        let rejection = match op(presented.clone()).await {
            Ok(value) => return Ok(value),
            Err(e) => e,
        };
        // The errcode, not the bare status: a 403 `M_FORBIDDEN` is the bot's
        // power level and must not be reported as a dead credential.
        if !token::is_credential_rejection(&rejection) {
            return Err(ChannelError::Transport(rejection.to_string()));
        }
        // Secret-free from here on: the homeserver's error body is an echo of
        // a request we authenticated, so only the errcode label travels.
        let label = token::auth_rejection_label(&rejection);
        match tokens.renew_after_rejection(&presented, &rejection).await {
            Renewal::Renewed => op(tokens.access())
                .await
                .map_err(|e| ChannelError::Transport(e.to_string())),
            Renewal::Deferred(why) => Err(ChannelError::Transport(format!(
                "{label}; could not renew it yet: {why}"
            ))),
            // `TokenSource` has already published `AuthExpired`, so health
            // reads `Unauthenticated` even though a SEND — not the `/sync`
            // loop — is what discovered the dead credential.
            Renewal::Fatal => Err(ChannelError::Auth(label)),
        }
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

        // One shared token for the send path and the `/sync` task: a renewal
        // driven by either is immediately visible to the other, so the
        // outbound half never keeps authenticating with a token the loop has
        // already replaced.
        let tokens = Arc::new(TokenSource::new(TokenSourceParams {
            creds: Arc::clone(&self.creds),
            access_handle: self.config.credential_handle_access_token.clone(),
            refresh_handle: self.config.credential_handle_refresh_token.clone(),
            access_token: token,
            http: self.http.clone(),
            api_base: self.api_base.clone(),
            inbox: Arc::clone(&self.inbox),
        }));
        self.tokens = Some(Arc::clone(&tokens));

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
            tokens,
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
        self.tokens = None;
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
        self.with_access_token(|token| async move {
            rest::send_typing(
                &self.http,
                &self.api_base,
                &token,
                conversation_id,
                &self.config.user_id,
                30_000,
            )
            .await
        })
        .await
    }

    /// `m.reaction` annotation relating to the inbound event — the ack
    /// signal. `message_id` is the Matrix `event_id`.
    async fn react(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        self.with_access_token(|token| async move {
            rest::send_reaction(
                &self.http,
                &self.api_base,
                &token,
                conversation_id,
                message_id,
                emoji,
            )
            .await
        })
        .await
    }

    /// Download unencrypted inbound media by its `mxc://` URI via the
    /// authenticated media endpoint. `attachment.url` carries the `mxc://`
    /// URI mapped by the `/sync` parser.
    async fn fetch_media(
        &self,
        attachment: &wcore_channels::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        let url: &str = &attachment.url;
        self.with_access_token(|token| async move {
            rest::download_media(&self.http, &self.api_base, &token, url).await
        })
        .await
    }

    /// This adapter's inbound intake policy — see [`MEDIA_BOUNDS`], from which
    /// the media download cap is derived.
    fn media_bounds(&self) -> wcore_channels::MediaBounds {
        MEDIA_BOUNDS
    }

    /// Matrix implements all four, though `edit` and `delete` are shaped
    /// differently from every other platform here: an edit is a NEW event
    /// carrying an `m.replace` relation, and a delete is a redaction that
    /// strips content while leaving a tombstone in the timeline. The note says
    /// so, because an operator who expects the message to vanish entirely will
    /// otherwise read a correct redaction as a failure.
    fn native_actions(&self) -> wcore_channels::NativeActions {
        use wcore_channels::ActionSupport::Implemented;
        wcore_channels::NativeActions::none()
            .edit(Implemented)
            .delete(Implemented)
            .react(Implemented)
            .typing(Implemented)
            .note(
                "edit: sent as an m.replace relation (a new event); \
                 delete: a redaction — content is stripped, the event stub remains",
            )
    }

    /// `m.replace` relation — see [`rest::edit_message`]. `message_id` is the
    /// Matrix `event_id` of the message being replaced.
    ///
    /// The returned receipt carries the **replacement** event's id. The caller
    /// keeps the original id for any further edit, because Matrix relates every
    /// edit to the original rather than chaining them.
    async fn edit_message(
        &self,
        conversation_id: &str,
        message_id: &str,
        new_text: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        let new_event_id = self
            .with_access_token(|token| async move {
                rest::edit_message(
                    &self.http,
                    &self.api_base,
                    &token,
                    conversation_id,
                    message_id,
                    new_text,
                )
                .await
            })
            .await?;
        Ok(MessageReceipt {
            id: new_event_id,
            conversation_id: conversation_id.to_string(),
            ts_secs: 0,
        })
    }

    /// Redaction — see [`rest::redact_event`].
    async fn delete_message(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<(), ChannelError> {
        self.with_access_token(|token| async move {
            rest::redact_event(
                &self.http,
                &self.api_base,
                &token,
                conversation_id,
                message_id,
            )
            .await
            .map(|_| ())
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // One in-memory credentials store for the whole crate's tests; it lives
    // beside the token source because that is what reads and rotates it.
    use crate::token::tests::MemCreds;

    fn cfg() -> MatrixConfig {
        MatrixConfig {
            homeserver_url: "https://matrix.example.org".to_string(),
            credential_handle_access_token: "matrix.test.token".to_string(),
            credential_handle_refresh_token: None,
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

    /// The cap the exactly-once guarantee is conditional on.
    ///
    /// # Why this is not the usual constant-against-itself test
    ///
    /// Six other adapters assert `max_message_len()` against the literal their
    /// own function returns, which cannot fail for any reason a reader cares
    /// about. Matrix had **no test at all** until 2026-07-31 — and Matrix's is
    /// the one that carries weight, because it is the single adapter still
    /// claiming exactly-once and `ChannelManager::send_to_keyed` transmits the
    /// idempotency key ONLY while the body fits inside this number. Above it the
    /// body is chunked and sent unkeyed, so the guarantee degrades to
    /// at-least-once (`docs/delivery-semantics.md` §4.1).
    ///
    /// So this asserts the two things that actually matter about the value —
    /// that it is finite, and that it is the number the customer-facing document
    /// states — rather than restating the literal. The document side of the same
    /// binding is
    /// `wcore-channels-registry/tests/delivery_semantics_declaration.rs`, which
    /// reads this method through the production factory and compares it against
    /// the `matrix.cap` row.
    #[tokio::test]
    async fn max_message_len_is_the_cap_the_guarantee_is_conditional_on() {
        let creds = MemCreds::with_token("matrix.test.token", TEST_TOKEN);
        let ch = MatrixChannel::with_base("test", cfg(), creds, "http://unused.invalid".into());

        let cap = ch.max_message_len().expect(
            "Matrix must declare a finite cap: an adapter with no cap would make the \
                     conditional guarantee in docs/delivery-semantics.md §4.1 describe nothing",
        );
        assert_eq!(cap, 32_768);

        // The pair that makes the condition real: one char under the cap is a
        // single message (key rides, exactly-once); one char over is chunked
        // (no key, at-least-once). Driving the same chunker the manager drives
        // rather than asserting the arithmetic.
        let under = "x".repeat(cap);
        let over = "x".repeat(cap + 1);
        assert_eq!(
            wcore_channels::chunk::chunk_message(&under, cap).len(),
            1,
            "a body exactly at the cap must still be one message, or the guarantee stops one \
             char earlier than the document says"
        );
        assert!(
            wcore_channels::chunk::chunk_message(&over, cap).len() > 1,
            "a body one char over the cap must split — that split is what drops the \
             idempotency key"
        );
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

    // ---- native actions: edit / delete (Phase 24 C3) ----------------------

    /// The edit must carry `m.relates_to.rel_type == "m.replace"` and the
    /// authoritative text in `m.new_content`.
    ///
    /// This is not decoration. Matrix has no update verb — a "replacement" that
    /// omits the relation is just a second message in the room, i.e. a silent
    /// duplicate. The mock matches the relation and the new content, so
    /// dropping either reddens here rather than in a live room.
    #[tokio::test]
    async fn edit_sends_an_m_replace_relation_carrying_the_new_content() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(
                    r"/_matrix/client/v3/rooms/[^/]+/send/m\.room\.message/wl-u[0-9a-f]+-[0-9a-f]+"
                        .to_string(),
                ),
            )
            .match_header("authorization", format!("Bearer {TEST_TOKEN}").as_str())
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "msgtype": "m.text",
                "body": "* edited body",
                "m.new_content": { "msgtype": "m.text", "body": "edited body" },
                "m.relates_to": { "rel_type": "m.replace", "event_id": "$orig123" }
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"event_id":"$replacement456"}"#)
            .expect(1)
            .create_async()
            .await;

        let creds = MemCreds::with_token("matrix.test.token", TEST_TOKEN);
        let mut ch = MatrixChannel::with_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();

        let receipt = ch
            .edit_message(TEST_ROOM, "$orig123", "edited body")
            .await
            .expect("edit succeeds");
        // The receipt names the REPLACEMENT, not the original. Returning the
        // original would tell the caller nothing happened.
        assert_eq!(receipt.id, "$replacement456");

        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    /// The delete is a redaction on the redact route, with the event id
    /// percent-encoded into the path (`$` and `:` are legal but the room id is
    /// not, and both segments go through the same encoder).
    #[tokio::test]
    async fn delete_puts_to_the_redact_route_for_the_target_event() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(
                    r"/_matrix/client/v3/rooms/[^/]+/redact/[^/]+/wl-u[0-9a-f]+-[0-9a-f]+"
                        .to_string(),
                ),
            )
            .match_header("authorization", format!("Bearer {TEST_TOKEN}").as_str())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"event_id":"$redaction789"}"#)
            .expect(1)
            .create_async()
            .await;

        let creds = MemCreds::with_token("matrix.test.token", TEST_TOKEN);
        let mut ch = MatrixChannel::with_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();

        ch.delete_message(TEST_ROOM, "$orig123")
            .await
            .expect("redaction succeeds");

        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    /// **The failing direction.** A homeserver that refuses the redaction
    /// (`M_FORBIDDEN` — the bot lacks the power level) must produce an error.
    #[tokio::test]
    async fn a_forbidden_redaction_is_an_error_not_a_silent_success() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(r"/_matrix/client/v3/rooms/[^/]+/redact/.*".to_string()),
            )
            .with_status(403)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errcode":"M_FORBIDDEN","error":"You don't have permission to redact this event"}"#)
            .create_async()
            .await;

        let creds = MemCreds::with_token("matrix.test.token", TEST_TOKEN);
        let mut ch = MatrixChannel::with_base("test", cfg(), creds, server.url());
        ch.start().await.unwrap();

        let err = ch.delete_message(TEST_ROOM, "$orig123").await.unwrap_err();
        assert!(
            !matches!(err, ChannelError::Unsupported { .. }),
            "got Unsupported — the delete override is missing: {err:?}"
        );
        assert!(
            err.to_string().contains("M_FORBIDDEN"),
            "the homeserver's own errcode must reach the operator, got {err}"
        );
        ch.stop().await.unwrap();
    }

    /// A PERMISSION error must not be reported as a dead credential.
    ///
    /// `with_access_token` funnels every outbound call through one 401/403
    /// decision, which is what stops the send path having its own opinion about
    /// expiry. But Matrix uses those two statuses for two different things:
    /// `M_UNKNOWN_TOKEN` says *who you are* is no longer accepted, while
    /// `M_FORBIDDEN` says *what you asked for* is not allowed — an
    /// under-privileged bot asked to redact someone else's event, with a
    /// perfectly live token.
    ///
    /// Collapsing them publishes `AuthExpired`, which the channel manager
    /// projects onto `HealthState::Unauthenticated` and which `TokenSource`
    /// latches so it can never be walked back. One refused redaction would
    /// therefore mark the channel permanently unauthenticated while every
    /// subsequent send kept succeeding — health lying, in the direction the
    /// operator acts on, because `channel reload` cannot fix a power level.
    ///
    /// **The negative half is paired with a known-positive on purpose.** An
    /// "no `AuthExpired` was published" assertion passes just as happily if the
    /// event can never be published at all, so the same shape is run against a
    /// genuine `M_UNKNOWN_TOKEN` revocation, which must still fail closed.
    #[tokio::test]
    async fn a_permission_error_is_not_a_dead_credential() {
        async fn auth_expired_after_a_refused_redaction(status: usize, body: &str) -> Vec<String> {
            let mut server = mockito::Server::new_async().await;
            let _m = server
                .mock(
                    "PUT",
                    mockito::Matcher::Regex(
                        r"/_matrix/client/v3/rooms/[^/]+/redact/.*".to_string(),
                    ),
                )
                .with_status(status)
                .with_header("content-type", "application/json")
                .with_body(body)
                .create_async()
                .await;

            let creds = MemCreds::with_token("matrix.test.token", TEST_TOKEN);
            let mut ch = MatrixChannel::with_base("test", cfg(), creds, server.url());
            ch.start().await.unwrap();
            let err = ch.delete_message(TEST_ROOM, "$orig123").await.unwrap_err();
            assert!(
                err.to_string().contains("M_"),
                "the homeserver's errcode must reach the operator, got {err}"
            );
            let events = ch.poll_events().await.unwrap();
            ch.stop().await.unwrap();
            events
                .into_iter()
                .filter_map(|e| match e {
                    ChannelEvent::AuthExpired { reason } => Some(reason),
                    _ => None,
                })
                .collect()
        }

        // Known-positive: a real revocation on the same route, same shape.
        let revoked = auth_expired_after_a_refused_redaction(
            401,
            r#"{"errcode":"M_UNKNOWN_TOKEN","error":"Token is not active"}"#,
        )
        .await;
        assert_eq!(
            revoked.len(),
            1,
            "a revoked token must still fail closed on the send path, or the \
             assertion below proves nothing: {revoked:?}"
        );

        // The case under test: a live token, an operation the bot may not do.
        let forbidden = auth_expired_after_a_refused_redaction(
            403,
            r#"{"errcode":"M_FORBIDDEN","error":"You don't have permission to redact this event"}"#,
        )
        .await;
        assert!(
            forbidden.is_empty(),
            "a permission error marked the credential dead; health would read \
             Unauthenticated for a live token and `channel reload` cannot fix a \
             power level: {forbidden:?}"
        );
    }

    /// Declaration ↔ behaviour, both directions.
    #[tokio::test]
    async fn native_action_declaration_matches_behaviour() {
        use wcore_channels::ActionSupport;
        let ch = MatrixChannel::new("test", cfg(), MemCreds::empty());
        let a = ch.native_actions();
        assert_eq!(a.edit, ActionSupport::Implemented);
        assert_eq!(a.delete, ActionSupport::Implemented);
        assert_eq!(a.react, ActionSupport::Implemented);
        assert_eq!(a.typing, ActionSupport::Implemented);
        assert!(
            a.note.contains("m.replace") && a.note.contains("redaction"),
            "the note must name the two semantics that differ from every other adapter: {}",
            a.note
        );

        let e = ch.edit_message(TEST_ROOM, "$x", "y").await.unwrap_err();
        assert!(matches!(e, ChannelError::NotStarted), "got {e:?}");
        let d = ch.delete_message(TEST_ROOM, "$x").await.unwrap_err();
        assert!(matches!(d, ChannelError::NotStarted), "got {d:?}");
    }
}
