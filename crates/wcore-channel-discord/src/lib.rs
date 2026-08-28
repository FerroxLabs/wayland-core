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

/// The single source of this adapter's inbound media bounds.
///
/// [`Channel::media_bounds`] returns this, and [`rest::download_bytes`] caps
/// the streamed body at `MEDIA_BOUNDS.max_bytes`. One constant, both sites, so
/// the advertised number and the enforced number cannot drift apart.
///
/// They previously had: this adapter advertised 25 MiB while `download_bytes`
/// buffered up to 100 MiB from a hardcoded constant, because nothing in the
/// workspace ever read the declaration. 100 MiB is the value that has actually
/// governed inbound fetches since 2026-06-12 and is retained deliberately —
/// Discord's own per-attachment ceiling is 25 MiB only for a NON-BOOSTED
/// upload, and boosted servers and Nitro senders legitimately exceed it, so
/// declaring 25 here would degrade media this adapter has always accepted.
/// `max_bytes` is an intake policy, not a restatement of a platform tier.
pub const MEDIA_BOUNDS: wcore_channels::MediaBounds = wcore_channels::MediaBounds {
    max_bytes: 100 * 1024 * 1024,
    max_attachments: 10,
};

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
    /// Construct a Discord channel from config.
    ///
    /// Both base URLs come from the config, which defaults them to the
    /// production endpoints ([`DISCORD_API_BASE`] / [`DISCORD_GATEWAY_BASE`]).
    /// This is the constructor `wcore-channels-registry` uses, so it is the
    /// only path by which a shipped binary can be pointed at a local fixture
    /// (F24-C3-DISCORD).
    pub fn new(
        name: impl Into<String>,
        config: DiscordConfig,
        creds: Arc<dyn CredentialsStore>,
    ) -> Self {
        let api_base = config.api_base_url.clone();
        let gateway_base = config.gateway_url.clone();
        Self::with_bases(name, config, creds, api_base, gateway_base)
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

    /// The one send path, keyed or not.
    ///
    /// Both trait methods route through here so exactly one place decides what
    /// nonce reaches the wire. Two send paths would let the keyed one drift and
    /// quietly stop transmitting the derived nonce while
    /// `supports_outbound_idempotency` still claimed it did.
    ///
    /// The nonce is generated ONCE and reused across the retry loop inside
    /// `rest::send_message` (HIGH-7): a retry after a lost success re-sends the
    /// same nonce, which Discord dedupes instead of posting a duplicate. With a
    /// delivery key that reuse now extends across process restarts too, not
    /// just across one process's retries.
    async fn post_message(
        &mut self,
        msg: OutgoingMessage,
        delivery_key: Option<&str>,
    ) -> Result<MessageReceipt, ChannelError> {
        let token = self.bot_token.as_deref().ok_or(ChannelError::NotStarted)?;
        let reference = msg
            .reply_to
            .as_deref()
            .map(|m| rest::MessageReference { message_id: m });
        let nonce = match delivery_key {
            Some(k) => rest::nonce_for_key(k),
            None => rest::next_nonce(),
        };
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
        self.post_message(msg, None).await
    }

    /// Discord's `nonce` IS an idempotency token, so the delivery key is
    /// carried on the wire rather than ignored: the nonce is derived from it
    /// and is therefore identical when the same logical delivery is replayed
    /// after a restart.
    async fn send_message_idempotent(
        &mut self,
        msg: OutgoingMessage,
        key: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        self.post_message(msg, Some(key)).await
    }

    /// `false` — **measured against real Discord on 2026-07-30, not inferred.**
    ///
    /// This adapter does transmit the delivery key, as the `nonce` field of the
    /// create-message body (see [`Self::post_message`]), and that part still
    /// works: the nonce is derived from the key by [`rest::nonce_for_key`] and
    /// is therefore identical across a restart. **Discord simply does not
    /// deduplicate on it.**
    ///
    /// This method returned `true` from the day the derived nonce landed until
    /// somebody drove a real replay at the real platform. What that run found:
    ///
    /// - An identical nonce, same channel, same author, replayed at 0 s, 5 s,
    ///   30 s and 90 s produced **two distinct message ids every time** — not
    ///   even at zero delay. There is no dedup window to be inside of; the
    ///   backlog item that asked how long it was (`BL-24C1-DISCORD-WINDOW`) has
    ///   the answer "it does not exist".
    /// - The nonce is **accepted**, not rejected: `POST` returns 200 and Discord
    ///   echoes the value back in the create response, so this is the platform's
    ///   behaviour and not a malformed token.
    /// - End to end through the gateway: one `once:` cron job, killed mid-send
    ///   so its outcome was genuinely unknown, restarted (the gateway itself
    ///   reported `carried=1 … unknown-outcome 1`) — **two messages arrived.**
    ///
    /// `nonce` is a client-side reconciliation echo, not a server-side
    /// idempotency key.
    ///
    /// Declaring `true` here is worse than useless: it makes
    /// `LedgeredHandler::dispatch_fire` take the *re-attempt* arm instead of the
    /// *abandon* arm, so the spine deliberately re-sends a possibly-delivered
    /// message on the strength of a suppression that never happens. That is the
    /// exact "a false `true` converts a visible duplicate into an invisible one"
    /// argument `docs/delivery-semantics.md` §6 makes — it just happened to be
    /// this adapter violating it. With `false`, an outcome-unknown Discord
    /// delivery is abandoned and made nameable by `wayland-core gateway
    /// abandoned`, exactly like the other at-most-once adapters.
    fn supports_outbound_idempotency(&self) -> bool {
        false
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

    /// This adapter's inbound intake policy — see [`MEDIA_BOUNDS`], which is
    /// the same constant [`rest::download_bytes`] caps the streamed body at.
    fn media_bounds(&self) -> wcore_channels::MediaBounds {
        MEDIA_BOUNDS
    }

    /// Discord caps a single message at 2000 characters. Documented at
    /// <https://docs.discord.com/developers/resources/message> (Create Message)
    /// — "content?* — string — Message contents (up to 2000 characters)"; the
    /// 25 MiB figure on the same page is the whole request, not this field.
    /// The 4,000 figure seen elsewhere is a Nitro client affordance with no
    /// bot-facing documentation. Boundary-probed live 2026-08-27 (wayland#934):
    /// 2,000 accepted, 2,001 refused `400 50035 Invalid Form Body`.
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

    /// Discord implements all four: `PATCH`/`DELETE` on the message resource,
    /// `PUT …/reactions/{emoji}/@me`, and `POST /channels/{id}/typing`.
    fn native_actions(&self) -> wcore_channels::NativeActions {
        use wcore_channels::ActionSupport::Implemented;
        wcore_channels::NativeActions::none()
            .edit(Implemented)
            .delete(Implemented)
            .react(Implemented)
            .typing(Implemented)
    }

    /// `PATCH /channels/{id}/messages/{msg}` — see [`rest::edit_message`].
    async fn edit_message(
        &self,
        conversation_id: &str,
        message_id: &str,
        new_text: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        let token = self.bot_token.as_deref().ok_or(ChannelError::NotStarted)?;
        let msg = rest::edit_message(
            &self.http,
            &self.api_base,
            token,
            conversation_id,
            message_id,
            new_text,
        )
        .await
        .map_err(ChannelError::from)?;
        Ok(MessageReceipt {
            id: msg.id,
            conversation_id: msg
                .channel_id
                .unwrap_or_else(|| conversation_id.to_string()),
            ts_secs: 0,
        })
    }

    /// `DELETE /channels/{id}/messages/{msg}` — see [`rest::delete_message`].
    async fn delete_message(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<(), ChannelError> {
        let token = self.bot_token.as_deref().ok_or(ChannelError::NotStarted)?;
        rest::delete_message(
            &self.http,
            &self.api_base,
            token,
            conversation_id,
            message_id,
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
            api_base_url: DISCORD_API_BASE.to_string(),
            gateway_url: DISCORD_GATEWAY_BASE.to_string(),
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

    /// The derived nonce still reaches the wire — and the capability
    /// declaration must nonetheless stay `false`.
    ///
    /// **This test used to assert the opposite, and the assertion was wrong.**
    /// It was a mockito test, so the only thing it could ever check was that we
    /// *send* a stable token; it inferred from that that Discord would *honour*
    /// it. Driven at real Discord on 2026-07-30, that inference was false: an
    /// identical nonce replayed at 0/5/30/90 s produced two messages every time,
    /// and a real kill-and-restart of the gateway put a duplicate in a real
    /// channel. See `supports_outbound_idempotency`.
    ///
    /// Both halves are asserted here deliberately, because they are independent
    /// and both matter:
    ///
    /// - the keyed path must keep deriving the nonce from the delivery key
    ///   (a revert to `next_nonce()` would still be a regression — the token is
    ///   useful to clients even though Discord will not dedupe on it);
    /// - the capability must stay `false`, because `true` makes the delivery
    ///   spine re-send an outcome-unknown delivery into a platform that does not
    ///   suppress the replay.
    ///
    /// A future lane that "fixes" this back to `true` on the strength of the
    /// nonce being on the wire is repeating the original error; the wire is not
    /// the question, the platform's behaviour is.
    #[tokio::test]
    async fn discord_sends_the_derived_nonce_but_must_not_claim_idempotency() {
        let mut server = mockito::Server::new_async().await;
        let expected = rest::nonce_for_key("cron:job-a:1785121776528");
        let mock = server
            .mock(
                "POST",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages").as_str(),
            )
            .match_body(mockito::Matcher::PartialJsonString(format!(
                r#"{{"nonce":"{expected}"}}"#
            )))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"42","channel_id":"424242"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = start_channel_with_rest_only(&server).await;
        assert!(
            !ch.supports_outbound_idempotency(),
            "Discord must NOT claim outbound idempotency: measured live on \
             2026-07-30, an identical nonce replayed after a real gateway \
             restart produced a SECOND message at real Discord. Claiming true \
             here makes the delivery spine re-send outcome-unknown deliveries \
             into a platform that does not suppress them."
        );
        ch.send_message_idempotent(
            OutgoingMessage::text(TEST_CHANNEL, "hello"),
            "cron:job-a:1785121776528",
        )
        .await
        .unwrap();

        mock.assert_async().await;
        ch.stop().await.unwrap();
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
    // F24-C3-DISCORD — `new()` must honour the config seam.
    //
    // `with_bases` already existed, but it is doc(hidden) and only ever
    // called in-process by unit tests. `wcore-channels-registry` builds the
    // SHIPPED adapter via `new()`, so if `new()` ignores the config the
    // seam is invisible to every out-of-process harness — which is the
    // state Phase 24 was actually in.
    // -----------------------------------------------------------------

    #[test]
    fn new_honours_the_config_bases_so_the_shipped_path_is_redirectable() {
        let creds = InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN);
        let ch = DiscordChannel::new(
            "test",
            DiscordConfig {
                api_base_url: "http://127.0.0.1:18211".to_string(),
                gateway_url: "ws://127.0.0.1:18212".to_string(),
                ..cfg()
            },
            creds,
        );
        assert_eq!(
            ch.api_base, "http://127.0.0.1:18211",
            "new() must take the REST base from config, not the constant"
        );
        assert_eq!(
            ch.gateway_base, "ws://127.0.0.1:18212",
            "new() must take the gateway base from config; without this the \
             binary's INBOUND stays on production while outbound is redirected"
        );
    }

    #[test]
    fn control_new_with_a_default_config_still_points_at_production() {
        // The paired control. Parse from TOML that names neither key, so this
        // exercises the real operator path rather than a struct literal.
        let parsed: DiscordConfig =
            toml::from_str(r#"credential_handle = "discord.test.bot_token""#).unwrap();
        let creds = InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN);
        let ch = DiscordChannel::new("test", parsed, creds);
        assert_eq!(ch.api_base, DISCORD_API_BASE);
        assert_eq!(ch.gateway_base, DISCORD_GATEWAY_BASE);
        assert_eq!(ch.api_base, "https://discord.com");
        assert_eq!(ch.gateway_base, "wss://gateway.discord.gg");
    }

    /// The cap is load-bearing: it is the boundary the chunker splits on.
    ///
    /// # Why this is not `assert_eq!(max_message_len(), Some(2000))`
    ///
    /// wayland#934. Until 2026-08-28 this test compared the function's return
    /// value against the literal that function returns a few lines above. That
    /// restates the code. Measured the same day: with `chunk_message` mutated to
    /// emit pieces 1000 chars OVER the cap — HIGH-6, the reject-and-drop bug the
    /// cap exists to prevent — six of the seven adapter cap tests still passed.
    ///
    /// So this drives the boundary instead, through the same
    /// `ChannelManager::chunks_for` decision `send_to_keyed` itself reads:
    /// a body AT the cap is one message (which is what lets the idempotency key
    /// ride, `docs/delivery-semantics.md` §4.1); one char OVER splits; no piece
    /// of the split may exceed the cap; and the split is lossless.
    ///
    /// The NUMBER is bound elsewhere, deliberately. No unit test in this crate
    /// can check a number against a platform. What binds it is
    /// `wcore-channels-registry/tests/delivery_semantics_declaration.rs`, which
    /// reads this method through the PRODUCTION factory and compares it against
    /// the `discord.cap` row of `docs/delivery-semantics.md` — a row that
    /// now also carries `discord.cap_source`, the vendor documentation the
    /// number is derived from, and `discord.cap_measured`, which states
    /// whether it has ever been checked at the real platform.
    #[test]
    fn a_body_over_the_cap_splits_into_pieces_the_platform_will_accept() {
        let creds = InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN);
        let ch = DiscordChannel::new("test", cfg(), creds);
        let cap = ch.max_message_len().expect(
            "discord must declare a finite cap; None disables chunking and reinstates HIGH-6",
        );

        // AT the cap: one message. One char earlier and the conditional
        // guarantee of §4.1 would stop short of where the document says it does.
        let at_cap = "x".repeat(cap);
        assert_eq!(
            wcore_channels::manager::ChannelManager::chunks_for(Some(cap), &at_cap).len(),
            1,
            "a body of exactly {cap} chars must still go as ONE message"
        );

        // ONE CHAR OVER: splits, and — the assertion the old test could not make
        // — every piece is itself within the cap. A piece over the platform
        // limit is rejected and dropped in turn, which is the whole of HIGH-6.
        let over = format!("{at_cap}y");
        let chunks = wcore_channels::manager::ChannelManager::chunks_for(Some(cap), &over);
        assert_eq!(
            chunks.len(),
            2,
            "an unbroken run of {} chars at cap {cap} must split into exactly 2 pieces",
            over.chars().count()
        );
        let widest = chunks.iter().map(|c| c.chars().count()).max().unwrap_or(0);
        assert!(
            widest <= cap,
            "a chunk of {widest} chars exceeds the {cap}-char cap — the platform rejects it and \
             the body is dropped, which is the HIGH-6 bug the cap exists to prevent"
        );
        assert_eq!(
            chunks.concat(),
            over,
            "the split must be lossless: the destination gets the whole body or the chunker ate it"
        );
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

    // ---- native actions: edit / delete (Phase 24 C3) ----------------------

    /// The edit is a `PATCH` on the MESSAGE resource, not a `POST` to the
    /// collection. The mock matches the method and the exact path, so an edit
    /// that accidentally posted a new message would redden here — which is the
    /// worst possible edit bug and the easiest one to write.
    #[tokio::test]
    async fn edit_patches_the_message_resource_with_the_new_content() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "PATCH",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages/9001").as_str(),
            )
            .match_header("authorization", format!("Bot {TEST_TOKEN}").as_str())
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "content": "edited body"
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"9001","channel_id":"424242"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = start_channel_with_rest_only(&server).await;
        let receipt = ch
            .edit_message(TEST_CHANNEL, "9001", "edited body")
            .await
            .expect("edit succeeds");
        assert_eq!(receipt.id, "9001");
        assert_eq!(receipt.conversation_id, TEST_CHANNEL);

        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    /// The delete is a `DELETE` on the message resource and Discord answers
    /// `204 No Content` — no body to parse.
    #[tokio::test]
    async fn delete_hits_the_message_resource_and_accepts_204() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "DELETE",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages/9001").as_str(),
            )
            .match_header("authorization", format!("Bot {TEST_TOKEN}").as_str())
            .with_status(204)
            .expect(1)
            .create_async()
            .await;

        let mut ch = start_channel_with_rest_only(&server).await;
        ch.delete_message(TEST_CHANNEL, "9001")
            .await
            .expect("delete succeeds");

        mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    /// **The failing direction.** `404 Unknown Message` must surface as an
    /// error, not as a success. A delete that reports `Ok` for a message that
    /// is still there is the single worst outcome this operation has.
    #[tokio::test]
    async fn a_404_on_delete_is_an_error_not_a_silent_success() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock(
                "DELETE",
                format!("/api/v10/channels/{TEST_CHANNEL}/messages/nope").as_str(),
            )
            .with_status(404)
            .with_header("content-type", "application/json")
            .with_body(r#"{"code":10008,"message":"Unknown Message"}"#)
            .create_async()
            .await;

        let mut ch = start_channel_with_rest_only(&server).await;
        let err = ch.delete_message(TEST_CHANNEL, "nope").await.unwrap_err();
        assert!(
            !matches!(err, ChannelError::Unsupported { .. }),
            "got Unsupported — the delete override is missing: {err:?}"
        );
        assert!(
            err.to_string().contains("404"),
            "the platform status must reach the operator, got {err}"
        );

        ch.stop().await.unwrap();
    }

    /// Declaration ↔ behaviour, both directions, for this adapter.
    #[tokio::test]
    async fn native_action_declaration_matches_behaviour() {
        use wcore_channels::ActionSupport;
        let creds = InMemoryCreds::with_token("discord.test.bot_token", TEST_TOKEN);
        let ch = DiscordChannel::with_bases(
            "test",
            cfg(),
            creds,
            "https://unused.example".to_string(),
            "ws://127.0.0.1:1".to_string(),
        );
        let a = ch.native_actions();
        assert_eq!(a.edit, ActionSupport::Implemented);
        assert_eq!(a.delete, ActionSupport::Implemented);
        assert_eq!(a.react, ActionSupport::Implemented);
        assert_eq!(a.typing, ActionSupport::Implemented);

        // Unstarted → NotStarted, which proves an override ran. The trait
        // default would have answered Unsupported instead.
        let e = ch.edit_message("C", "1", "x").await.unwrap_err();
        assert!(matches!(e, ChannelError::NotStarted), "got {e:?}");
        let d = ch.delete_message("C", "1").await.unwrap_err();
        assert!(matches!(d, ChannelError::NotStarted), "got {d:?}");
    }
}
