//! `wcore-channel-slack` — production Slack adapter implementing the
//! `wcore_channels::Channel` trait.
//!
//! Outbound: Web API `chat.postMessage` with bearer auth, retry +
//! exponential backoff + jitter, `Retry-After` honoured on HTTP 429,
//! permanent-error short-circuit on 4xx + known Slack error codes.
//!
//! Inbound: Slack Events API webhooks. The engine's webhook router
//! invokes `SlackChannel::ingest_event(raw_body, signature, timestamp)`
//! when a POST hits `/channels/slack/<name>/webhook`. The adapter
//! verifies the HMAC-SHA256 signature, checks the timestamp falls
//! within a 5-minute replay window, parses the JSON envelope, and
//! enqueues a `ChannelEvent` for the next `poll_events()`.
//!
//! Secrets (bot token, signing secret) are resolved at `start()` time
//! from the `CredentialsStore`. The TOML config carries credential
//! handles only.

pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod inbound;

/// The single source of this adapter's inbound media bounds.
///
/// [`Channel::media_bounds`] returns this, and [`api::download_file`] caps the
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
use std::time::Duration;

/// How long `start()` will wait for Slack to answer the credential question
/// before giving up and connecting WITHOUT a verdict.
///
/// Deliberately far below the egress client's 300s read timeout: this call sits
/// on the gateway's startup path, so its worst case is startup latency for
/// every channel behind it.
const AUTH_TEST_BUDGET: Duration = Duration::from_secs(10);

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;
use wcore_channels::{
    Channel, ChannelError, WebhookRequest, WebhookResponse,
    event::{ChannelEvent, ConnectionState, MessageReceipt},
    outgoing::OutgoingMessage,
};
use wcore_config::credentials::CredentialsStore;

pub use config::SlackConfig;
pub use error::SlackError;

/// Production Slack adapter.
///
/// One instance per workspace. Lifecycle:
///   construct (`new`) →
///   `start()` (resolves secrets from CredentialsStore) →
///   loop `poll_events` / `send_message` →
///   `stop()` (drops cached secrets).
pub struct SlackChannel {
    name: String,
    config: SlackConfig,
    state: ConnectionState,
    bot_token: Option<String>,
    signing_secret: Option<String>,
    http: wcore_egress::EgressClient,
    credentials: Arc<dyn CredentialsStore>,
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
}

impl SlackChannel {
    /// Construct a new adapter. `credentials` is the store the bot token
    /// + signing secret are pulled from at `start()`.
    pub fn new(
        name: impl Into<String>,
        config: SlackConfig,
        credentials: Arc<dyn CredentialsStore>,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            bot_token: None,
            signing_secret: None,
            http: wcore_egress::EgressClient::new(),
            credentials,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Construct with a caller-supplied `reqwest::Client` (tests use
    /// this to drive a mockito server with a short timeout).
    pub fn with_http_client(
        name: impl Into<String>,
        config: SlackConfig,
        credentials: Arc<dyn CredentialsStore>,
        http: wcore_egress::EgressClient,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            bot_token: None,
            signing_secret: None,
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
    /// Called by the engine's HTTP host when a Slack Events API POST
    /// lands at the channel's configured webhook URL. Verifies signature
    /// + timestamp, parses the body, and either enqueues a `ChannelEvent`
    ///   for the next `poll_events()` or surfaces the challenge string
    ///   from `url_verification` (`Ok(Some(challenge))`).
    pub async fn ingest_event(
        &self,
        raw_body: &str,
        signature: &str,
        timestamp: &str,
    ) -> Result<Option<String>, SlackError> {
        let signing_secret = self.signing_secret.as_deref().ok_or_else(|| {
            SlackError::Auth("signing secret not loaded — call start() first".to_string())
        })?;

        auth::verify_timestamp(timestamp, Utc::now().timestamp())?;
        auth::verify_signature(signing_secret, timestamp, raw_body, signature)?;

        match inbound::parse_webhook(raw_body)? {
            inbound::Parsed::Challenge(c) => Ok(Some(c)),
            inbound::Parsed::Event(ev) => {
                // F9 — bounded, drop-oldest inbox against a flood.
                let mut guard = self.inbox.lock().await;
                wcore_channels::push_bounded(&mut guard, ev);
                Ok(None)
            }
            inbound::Parsed::Ignored => Ok(None),
        }
    }

    /// Route a credential refusal from an outbound call onto the health
    /// surface, and return it to the caller unchanged.
    ///
    /// # Why every outbound call site must go through this
    ///
    /// Slack inbound is webhooks, so on a running gateway an outbound refusal
    /// is the ONLY moment a revoked or deactivated token becomes observable.
    /// `HealthState::Unauthenticated` has exactly one Slack producer after
    /// `start()`: a `ChannelEvent::AuthExpired` drained by the manager's poll
    /// loop. A call site that maps `SlackError::Auth` straight to
    /// `ChannelError::Auth` therefore hands the refusal to one caller and
    /// tells the health surface nothing — the channel keeps reading `Healthy`.
    ///
    /// Only `send_message` published. `react`, `edit_message`, `delete_message`
    /// and `fetch_media` did not, and `react` is the first outbound call the
    /// engine makes on an inbound message (the ack emoji), so the most likely
    /// discovery point for a mid-run revocation was also a silent one.
    ///
    /// Bounded push: with a dead token, one refusal arrives per inbound
    /// message until the manager drains and stops the loop, and drop-oldest
    /// keeps the newest `AuthExpired` — the one that matters.
    async fn publish_auth_expired(&self, surface: &str, code: &str) -> ChannelError {
        tracing::error!(
            target: "wcore_channel_slack",
            channel = %self.name,
            surface = %surface,
            code = %code,
            "slack refused the bot token; publishing AuthExpired"
        );
        let mut guard = self.inbox.lock().await;
        wcore_channels::push_bounded(
            &mut guard,
            ChannelEvent::AuthExpired {
                reason: format!("slack refused the bot token on {surface}: {code}"),
            },
        );
        ChannelError::Auth(code.to_string())
    }

    /// Map an outbound [`SlackError`] to a [`ChannelError`], publishing
    /// `AuthExpired` first when the platform refused the credential.
    async fn outbound_error(&self, surface: &str, e: SlackError) -> ChannelError {
        match e {
            SlackError::Auth(code) => self.publish_auth_expired(surface, &code).await,
            other => ChannelError::from(other),
        }
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "slack"
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.state == ConnectionState::Connected {
            return Ok(());
        }
        self.state = ConnectionState::Connecting;

        // Resolve secrets from the credentials store.
        let bot_token = self
            .credentials
            .get(&self.config.credential_handle_bot_token)
            .map_err(|e| SlackError::Credentials(e.to_string()))?
            .ok_or_else(|| {
                SlackError::Credentials(format!(
                    "no value for credential handle {:?}",
                    self.config.credential_handle_bot_token
                ))
            })?;
        let signing_secret = self
            .credentials
            .get(&self.config.credential_handle_signing_secret)
            .map_err(|e| SlackError::Credentials(e.to_string()))?
            .ok_or_else(|| {
                SlackError::Credentials(format!(
                    "no value for credential handle {:?}",
                    self.config.credential_handle_signing_secret
                ))
            })?;

        self.bot_token = Some(bot_token.clone());
        self.signing_secret = Some(signing_secret);

        // Ask Slack whether the token is live before declaring the channel up.
        //
        // Slack inbound is webhooks, so nothing else in this adapter ever puts
        // the credential in front of the platform. Without this call, `start()`
        // proved only that a STRING was present in the credential store, and a
        // revoked token reported `Healthy` indefinitely (UAT-C2).
        // Bounded on purpose. This runs inside the gateway's channel-startup
        // path, and the shared egress client's read timeout is 300s — long
        // enough that one unresponsive slack.com would stall the whole boot.
        // A timeout is explicitly NOT a credential verdict; it takes the same
        // "could not be completed" branch as any other unreachability.
        let probe = tokio::time::timeout(
            AUTH_TEST_BUDGET,
            api::auth_test(&self.http, &self.config.api_base_url, &bot_token),
        )
        .await;
        let probe = match probe {
            Ok(r) => r,
            Err(_elapsed) => Err(SlackError::Http(format!(
                "auth.test exceeded its {}s startup budget",
                AUTH_TEST_BUDGET.as_secs()
            ))),
        };

        match probe {
            Ok(identity) => {
                tracing::info!(
                    target: "wcore_channel_slack",
                    channel = %self.name,
                    identity = %identity,
                    "slack auth.test accepted the bot token"
                );
            }
            Err(SlackError::Auth(code)) => {
                // Queue the terminal event and return Ok. Returning Err instead
                // would land in the manager's `start() failed` arm, which
                // records `Disconnected` — the state that means "wait". A
                // rejected credential means "rotate the token", and the only
                // route to `HealthState::Unauthenticated` is an adapter-published
                // event drained by the poll loop.
                tracing::error!(
                    target: "wcore_channel_slack",
                    channel = %self.name,
                    code = %code,
                    "slack rejected the bot token at auth.test"
                );
                self.state = ConnectionState::AuthError;
                self.inbox
                    .lock()
                    .await
                    .push_back(ChannelEvent::AuthExpired {
                        reason: format!("slack auth.test rejected the bot token: {code}"),
                    });
                return Ok(());
            }
            Err(e) => {
                // Unreachable or uninterpretable. NOT a credential verdict —
                // proceed as before rather than accusing a live token.
                tracing::warn!(
                    target: "wcore_channel_slack",
                    channel = %self.name,
                    error = %e,
                    "slack auth.test could not be completed; continuing without a \
                     credential verdict"
                );
            }
        }

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
        self.bot_token = None;
        self.signing_secret = None;
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
        self.post(msg, None).await
    }

    async fn send_message_idempotent(
        &mut self,
        msg: OutgoingMessage,
        key: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        self.post(msg, Some(key)).await
    }

    /// **`false` — Slack ignores the key, measured against the real API.**
    ///
    /// This adapter transmits an `Idempotency-Key` header on a keyed send (see
    /// [`Self::post`] and [`api::IDEMPOTENCY_HEADER`]) and continues to do so,
    /// because a Slack-compatible destination configured through
    /// `api_base_url` may honour it. But **`slack.com` does not**, and this
    /// method is not a statement about our wire — it is the bit
    /// `LedgeredHandler::dispatch_fire` reads to decide whether an `Attempted`,
    /// outcome-unknown delivery may be **re-sent** on restart. Answering `true`
    /// at a destination that cannot recognise the replay makes every such
    /// restart a duplicate, and an invisible one, because our own ledger
    /// records a single delivery.
    ///
    /// # The measurement
    ///
    /// 2026-07-30, live against `slack.com`, private channel `C0BLR1UKKU6`,
    /// through this adapter as the production registry factory builds it. Two
    /// `send_message_idempotent` calls with the **same** key and the same body:
    ///
    /// ```text
    /// first  ts=1785385438.299299
    /// replay ts=1785385438.564099   <- a different message, not the first one
    /// arrivals read back from conversations.history: 2
    /// ```
    ///
    /// Confirmed three ways in the same run: two distinct `ts` values returned,
    /// two records present in `conversations.history`, and `chat.delete`
    /// succeeding on both (a delete that succeeds proves the message existed).
    /// A raw-`curl` probe outside the adapter reproduced it identically.
    ///
    /// This previously returned `true`. The evidence behind that was
    /// `slack_declares_idempotency_only_because_it_sends_the_header`, a
    /// `mockito` test — which proves the header leaves us and can prove nothing
    /// about what Slack does with it. `docs/delivery-semantics.md` carried the
    /// same gap in words: its Slack row cited *"the key was present on both
    /// attempts"* as evidence for *"one message"*, which is a different claim.
    ///
    /// [`crates/wcore-channels-registry/tests/live_slack_actions.rs`] now binds
    /// this bit to the platform in both directions: it asserts the arrival
    /// count implied by whatever this method returns, so re-asserting the
    /// guarantee reddens it, and so would Slack starting to honour the key.
    fn supports_outbound_idempotency(&self) -> bool {
        false
    }

    fn config_schema(&self) -> &str {
        include_str!("../schemas/slack.json")
    }

    /// Slack's real single-message limit, measured against the live API
    /// (wayland#934, 2026-08-27): 4,040 characters is the largest body that
    /// arrives as ONE message; at 4,041 Slack splits it into 4,000-char
    /// messages. 4,000 is the split size itself, so a full-length chunk we
    /// send can never be re-split on arrival.
    fn max_message_len(&self) -> Option<usize> {
        Some(4_000)
    }

    /// `reactions.add` — the ack signal. `message_id` is the Slack message
    /// `ts`. Slack takes an emoji *shortcode*, not a unicode glyph, so the
    /// ack emoji is mapped; an unmapped emoji is rejected (skipped by the
    /// caller). Note: Slack has no bot-usable typing API, so `send_typing`
    /// deliberately keeps the trait's no-op default.
    async fn react(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;
        let name = api::slack_emoji_name(emoji).ok_or_else(|| {
            ChannelError::Rejected(format!("no slack shortcode for emoji {emoji}"))
        })?;
        let req = api::AddReactionRequest {
            channel: conversation_id.to_string(),
            timestamp: message_id.to_string(),
            name: name.to_string(),
        };
        match api::add_reaction(&self.http, &self.config.api_base_url, bot_token, &req).await {
            Ok(()) => Ok(()),
            Err(e) => Err(self.outbound_error("reactions.add", e).await),
        }
    }

    /// Slack: `chat.update` and `chat.delete` are real, `reactions.add` is real,
    /// and there is **no bot-usable typing API** — the `users.setPresence`
    /// surface is a user-token affordance, not a per-conversation typing
    /// indicator a bot token can drive. So typing is a permanent absence, not a
    /// backlog item, and it is recorded as one.
    fn native_actions(&self) -> wcore_channels::NativeActions {
        use wcore_channels::ActionSupport::*;
        wcore_channels::NativeActions::none()
            .edit(Implemented)
            .delete(Implemented)
            .react(Implemented)
            .typing(PlatformHasNoApi)
            .note("typing: Slack exposes no bot-token typing indicator")
    }

    /// `chat.update` — see [`api::update_message`]. `message_id` is the Slack
    /// message `ts`, exactly as it arrives in a send receipt.
    async fn edit_message(
        &self,
        conversation_id: &str,
        message_id: &str,
        new_text: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;
        let req = api::UpdateMessageRequest {
            channel: conversation_id.to_string(),
            ts: message_id.to_string(),
            text: new_text.to_string(),
        };
        let resp = match api::update_message(&self.http, &self.config.api_base_url, bot_token, &req)
            .await
        {
            Ok(r) => r,
            Err(e) => return Err(self.outbound_error("chat.update", e).await),
        };
        let ts = resp.ts.unwrap_or_else(|| message_id.to_string());
        let secs: i64 = ts
            .split('.')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Ok(MessageReceipt {
            id: ts,
            conversation_id: resp.channel.unwrap_or_else(|| conversation_id.to_string()),
            ts_secs: secs,
        })
    }

    /// `chat.delete` — see [`api::delete_message`].
    async fn delete_message(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<(), ChannelError> {
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;
        let req = api::DeleteMessageRequest {
            channel: conversation_id.to_string(),
            ts: message_id.to_string(),
        };
        match api::delete_message(&self.http, &self.config.api_base_url, bot_token, &req).await {
            Ok(_) => Ok(()),
            Err(e) => Err(self.outbound_error("chat.delete", e).await),
        }
    }

    async fn fetch_media(
        &self,
        attachment: &wcore_channels::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;
        match api::download_file(&self.http, &attachment.url, bot_token, api::MEDIA_HOSTS).await {
            Ok(bytes) => Ok(bytes),
            Err(e) => Err(self.outbound_error("files.download", e).await),
        }
    }

    /// Setup and authentication probe — the pre-flight surface Slack never had.
    ///
    /// `channel probe` answers "is this channel ready" WITHOUT starting the
    /// gateway and without sending a message. Slack implemented no probe, so it
    /// took the trait default and reported `Unsupported`. That default is
    /// honest — it is not `Ok`, and it does not read as ready — but it means the
    /// one platform whose inbound is webhooks, and therefore the one platform
    /// with no connection to reject a bad token, was also the one an operator
    /// could not pre-check. The only way to find out a Slack token was refused
    /// was to start a gateway and read `channel health`.
    ///
    /// [`api::auth_test`] already answers the credential question with no
    /// traffic, and already separates the three verdicts the three outcomes
    /// need: a missing handle is `Incomplete` (fill the credentials store), a
    /// refusal is `Unauthenticated` (rotate the token), and an unreachable
    /// slack.com is `Unreachable` (no verdict — retry). Collapsing those is how
    /// an operator ends up rotating a working token because the network blipped.
    ///
    /// The identity is `auth.test`'s own `<user_id>/<team>`. The token never
    /// enters the report — only the HANDLE is ever named in a finding.
    async fn probe(&self) -> Result<wcore_channels::ProbeReport, ChannelError> {
        use wcore_channels::ProbeReport;

        let mut missing = Vec::new();
        if self.config.credential_handle_bot_token.trim().is_empty() {
            missing.push("options.credential_handle_bot_token".to_string());
        }
        if self
            .config
            .credential_handle_signing_secret
            .trim()
            .is_empty()
        {
            missing.push("options.credential_handle_signing_secret".to_string());
        }
        if !missing.is_empty() {
            return Ok(ProbeReport::incomplete(&self.name, "slack", missing));
        }

        // The signing secret is checked for PRESENCE only. It is verified
        // against inbound webhook signatures, and there is no Slack API that
        // will tell us whether it is the right one — claiming otherwise would
        // be the probe attesting to something it did not measure.
        for handle in [
            &self.config.credential_handle_bot_token,
            &self.config.credential_handle_signing_secret,
        ] {
            match self.credentials.get(handle) {
                Ok(Some(_)) => {}
                Ok(None) => missing.push(format!(
                    "credential {handle:?} is not present in the credentials store"
                )),
                Err(e) => missing.push(format!("credential {handle:?} unreadable: {e}")),
            }
        }
        if !missing.is_empty() {
            return Ok(ProbeReport::incomplete(&self.name, "slack", missing));
        }

        let bot_token = match self
            .credentials
            .get(&self.config.credential_handle_bot_token)
        {
            Ok(Some(t)) => t,
            // Re-read raced with a delete, or the store broke between the two
            // calls. Either way nothing was measured, so say nothing was.
            Ok(None) | Err(_) => {
                return Ok(ProbeReport::incomplete(
                    &self.name,
                    "slack",
                    vec![format!(
                        "credential {:?} is not present in the credentials store",
                        self.config.credential_handle_bot_token
                    )],
                ));
            }
        };

        match api::auth_test(&self.http, &self.config.api_base_url, &bot_token).await {
            Ok(identity) => Ok(ProbeReport::ok(&self.name, "slack", identity)),
            // `SlackError::Auth` is exactly "slack.com looked at this token and
            // said no"; everything else is "we never got an answer".
            Err(SlackError::Auth(reason)) => {
                Ok(ProbeReport::unauthenticated(&self.name, "slack", reason))
            }
            Err(other) => Ok(ProbeReport::unreachable(
                &self.name,
                "slack",
                other.to_string(),
            )),
        }
    }

    /// This adapter's inbound intake policy — see [`MEDIA_BOUNDS`], which is
    /// the same constant [`api::download_file`] caps the streamed body at.
    fn media_bounds(&self) -> wcore_channels::MediaBounds {
        MEDIA_BOUNDS
    }

    /// Verify a Slack Events API POST and enqueue any resulting event.
    ///
    /// Pulls the `X-Slack-Signature` + `X-Slack-Request-Timestamp` headers
    /// the platform sends, then delegates to [`Self::ingest_event`] (which
    /// runs the signing-secret HMAC + timestamp window). A
    /// `url_verification` challenge surfaces as a `200` echoing the
    /// challenge string; everything else is an empty `200`.
    async fn ingest_webhook(&self, req: &WebhookRequest) -> Result<WebhookResponse, ChannelError> {
        let (sig, ts) = match (
            req.header("x-slack-signature"),
            req.header("x-slack-request-timestamp"),
        ) {
            (Some(sig), Some(ts)) => (sig, ts),
            _ => {
                return Err(ChannelError::Auth("missing slack signature headers".into()));
            }
        };
        match self.ingest_event(&req.body, sig, ts).await {
            Ok(Some(challenge)) => Ok(WebhookResponse::challenge(challenge)),
            Ok(None) => Ok(WebhookResponse::ok()),
            Err(e) => Err(ChannelError::Rejected(e.to_string())),
        }
    }
}

impl SlackChannel {
    async fn post(
        &mut self,
        msg: OutgoingMessage,
        idempotency_key: Option<&str>,
    ) -> Result<MessageReceipt, ChannelError> {
        if self.state != ConnectionState::Connected {
            return Err(ChannelError::NotStarted);
        }
        let bot_token = self
            .bot_token
            .as_deref()
            .ok_or_else(|| ChannelError::Auth("bot token not loaded".to_string()))?;

        let conversation_id = if msg.conversation_id.is_empty() {
            if self.config.default_channel_id.is_empty() {
                return Err(ChannelError::Rejected(
                    "no conversation_id and no default_channel_id configured".to_string(),
                ));
            }
            self.config.default_channel_id.clone()
        } else {
            msg.conversation_id.clone()
        };

        let req = api::PostMessageRequest {
            channel: conversation_id.clone(),
            text: msg.text.clone(),
            thread_ts: msg.reply_to.clone(),
        };

        let send_result = api::post_message_keyed(
            &self.http,
            &self.config.api_base_url,
            bot_token,
            &req,
            self.config.max_retry_attempts,
            idempotency_key,
        )
        .await;

        let resp = match send_result {
            Ok(r) => r,
            // A token that authenticated at start() can be revoked underneath a
            // running gateway. For a webhook-driven adapter an outbound refusal
            // is the ONLY moment that becomes observable — see
            // [`Self::publish_auth_expired`].
            Err(e) => return Err(self.outbound_error("chat.postMessage", e).await),
        };

        let ts = resp
            .ts
            .ok_or_else(|| ChannelError::Rejected("slack response missing ts".to_string()))?;
        let secs: i64 = ts
            .split('.')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        Ok(MessageReceipt {
            id: ts,
            conversation_id: resp.channel.unwrap_or(conversation_id),
            ts_secs: secs,
        })
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

    fn cfg_for(server_url: &str) -> SlackConfig {
        SlackConfig::new_for_test(server_url)
    }

    fn store_for_test() -> Arc<MapStore> {
        MapStore::new(&[
            ("slack.test.bot_token", "xoxb-test-token"),
            ("slack.test.signing_secret", "shhh"),
        ])
    }

    /// Drain the inbox and report which auth-relevant events it held.
    async fn auth_events_of(ch: &SlackChannel) -> (usize, usize) {
        let inbox = ch.inbox.lock().await;
        let expired = inbox
            .iter()
            .filter(|e| matches!(e, ChannelEvent::AuthExpired { .. }))
            .count();
        let connected = inbox
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ChannelEvent::ConnectionStateChanged {
                        state: ConnectionState::Connected
                    }
                )
            })
            .count();
        (expired, connected)
    }

    /// UAT-C2. A token Slack REFUSES must publish the event that becomes
    /// `HealthState::Unauthenticated`, and must NOT publish `Connected`.
    #[tokio::test]
    async fn a_rejected_bot_token_publishes_auth_expired_and_never_connects() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/auth.test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"invalid_auth"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        // start() still succeeds: a rejected credential is a HEALTH verdict the
        // poll loop must observe, not a start failure (which reads Disconnected).
        ch.start()
            .await
            .expect("start reports Ok and defers to health");
        mock.assert_async().await;

        let (expired, connected) = auth_events_of(&ch).await;
        assert_eq!(expired, 1, "exactly one AuthExpired must be queued");
        assert_eq!(
            connected, 0,
            "a refused token must never publish Connected — that is the false \
             Healthy this fixes"
        );

        let evs = ch.poll_events().await.expect("first poll drains the inbox");
        match &evs[0] {
            ChannelEvent::AuthExpired { reason } => {
                assert!(
                    reason.contains("invalid_auth"),
                    "the operator needs the platform's code: {reason}"
                );
                assert!(
                    !reason.contains("xoxb-test-token"),
                    "the reason must never carry the credential: {reason}"
                );
            }
            other => panic!("expected AuthExpired, got {other:?}"),
        }
    }

    /// The known-negative. An ACCEPTED token must take the old path exactly:
    /// Connected, no AuthExpired. Without this, a producer that fired
    /// unconditionally would pass the test above.
    #[tokio::test]
    async fn an_accepted_bot_token_connects_and_publishes_no_auth_expired() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/auth.test")
            .match_header("authorization", "Bearer xoxb-test-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"user_id":"U123","team":"acme"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.expect("a live token starts");
        mock.assert_async().await;

        let (expired, connected) = auth_events_of(&ch).await;
        assert_eq!(expired, 0, "a live token must not be accused");
        assert_eq!(connected, 1);
    }

    /// Reachability is not a credential verdict. A 500 must NOT flip the
    /// channel to Unauthenticated — that would make every Slack outage look
    /// like a revoked token and send operators rotating good credentials.
    #[tokio::test]
    async fn an_unreachable_auth_test_is_not_treated_as_a_rejection() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/auth.test")
            .with_status(500)
            .with_body("upstream boom")
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start()
            .await
            .expect("an unreachable probe does not fail start");
        mock.assert_async().await;

        let (expired, connected) = auth_events_of(&ch).await;
        assert_eq!(
            expired, 0,
            "a 5xx says nothing about the credential and must publish no AuthExpired"
        );
        assert_eq!(connected, 1);
    }

    /// A Slack that accepts the connection and never answers must not hang the
    /// gateway's startup path, and must not be mistaken for a rejection.
    ///
    /// Without [`AUTH_TEST_BUDGET`] this inherits the egress client's 300s read
    /// timeout, so one unresponsive workspace would stall the boot of every
    /// channel behind it.
    #[tokio::test]
    async fn a_slack_that_never_answers_is_bounded_and_is_not_a_rejection() {
        // Accept the TCP connection, then say nothing at all — the shape a
        // 5xx mock cannot produce.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = listener.accept().await {
                held.push(s); // hold the socket open, never reply
            }
        });

        let mut ch = SlackChannel::new(
            "test",
            cfg_for(&format!("http://127.0.0.1:{port}")),
            store_for_test(),
        );
        let began = std::time::Instant::now();
        ch.start()
            .await
            .expect("a stalled probe must not fail start");
        let took = began.elapsed();

        assert!(
            took < AUTH_TEST_BUDGET * 3,
            "start() took {took:?}; the auth.test budget is not being enforced"
        );
        let (expired, connected) = auth_events_of(&ch).await;
        assert_eq!(
            expired, 0,
            "a silent server says nothing about the credential"
        );
        assert_eq!(connected, 1, "the channel must still come up");
    }

    /// Mid-run revocation. A token that authenticated at start() and is later
    /// revoked is only observable on an outbound send for a webhook adapter.
    #[tokio::test]
    async fn a_token_revoked_after_start_publishes_auth_expired_on_send() {
        let mut server = mockito::Server::new_async().await;
        let auth_mock = server
            .mock("POST", "/api/auth.test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"user_id":"U123","team":"acme"}"#)
            .create_async()
            .await;
        let send_mock = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"token_revoked"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        auth_mock.assert_async().await;
        // Clear the Connected event so the count below is unambiguous.
        let _ = ch.poll_events().await.unwrap();

        let err = ch
            .send_message(OutgoingMessage::text("C1", "hi"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        send_mock.assert_async().await;

        let evs = ch.poll_events().await.unwrap();
        let expired: Vec<_> = evs
            .iter()
            .filter(|e| matches!(e, ChannelEvent::AuthExpired { .. }))
            .collect();
        assert_eq!(
            expired.len(),
            1,
            "a revoked token discovered on send must reach the health surface, \
             not die as a one-off send error: {evs:?}"
        );
    }

    /// The same lifecycle, with the code a Slack ADMIN produces rather than the
    /// one a token rotation produces.
    ///
    /// Deactivating the bot user is the other way a live Slack credential dies,
    /// and it is not recoverable by retrying — the token must be reissued
    /// against a live account, which is the same operator action as a rotation.
    /// `account_inactive` was in `api::is_auth_rejection` but missing from the
    /// hand-rolled list in `post_message_keyed`, so it returned
    /// `SlackError::Api`, `post()` never took the `SlackError::Auth` arm, no
    /// `AuthExpired` was published, and the channel went on reporting
    /// `Healthy` while every send failed.
    #[tokio::test]
    async fn a_deactivated_bot_user_publishes_auth_expired_on_send() {
        let mut server = mockito::Server::new_async().await;
        let auth_mock = server
            .mock("POST", "/api/auth.test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"user_id":"U123","team":"acme"}"#)
            .create_async()
            .await;
        let send_mock = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"account_inactive"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        auth_mock.assert_async().await;
        let _ = ch.poll_events().await.unwrap();

        let err = ch
            .send_message(OutgoingMessage::text("C1", "hi"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ChannelError::Auth(_)),
            "a deactivated bot user is a credential verdict, not a request \
             fault: got {err:?}"
        );
        send_mock.assert_async().await;

        let evs = ch.poll_events().await.unwrap();
        let expired: Vec<_> = evs
            .iter()
            .filter(|e| matches!(e, ChannelEvent::AuthExpired { .. }))
            .collect();
        assert_eq!(
            expired.len(),
            1,
            "a deactivated bot user must reach the health surface: {evs:?}"
        );
    }

    /// `channel probe` must answer the credential question for Slack too.
    ///
    /// Slack took the trait default (`Unsupported`) — honest, but it left the
    /// one webhook-inbound platform, the one with no connection to reject a bad
    /// token, as the one an operator could not pre-check.
    #[tokio::test]
    async fn probe_reports_ok_with_the_authenticated_identity() {
        use wcore_channels::ProbeOutcome;
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/auth.test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"user_id":"U123","team":"acme"}"#)
            .create_async()
            .await;

        // NOT started — a probe must need no gateway and no start().
        let ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        let r = ch.probe().await.unwrap();
        mock.assert_async().await;

        assert_eq!(r.outcome, ProbeOutcome::Ok);
        assert!(r.outcome.is_ready());
        assert!(r.config_complete && r.authenticated);
        assert_eq!(r.identity.as_deref(), Some("U123/acme"));
    }

    /// The three verdicts are three different operator actions and must not
    /// collapse: rotate the token, fill the store, or retry later.
    #[tokio::test]
    async fn probe_separates_a_rejected_token_from_an_unreachable_slack() {
        use wcore_channels::ProbeOutcome;

        let mut rejecting = mockito::Server::new_async().await;
        let _m = rejecting
            .mock("POST", "/api/auth.test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"invalid_auth"}"#)
            .create_async()
            .await;
        let ch = SlackChannel::new("test", cfg_for(&rejecting.url()), store_for_test());
        let r = ch.probe().await.unwrap();
        assert_eq!(r.outcome, ProbeOutcome::Unauthenticated);
        assert!(
            r.findings.iter().any(|f| f.contains("invalid_auth")),
            "the platform's own rejection label is what makes this actionable: {:?}",
            r.findings
        );

        let mut down = mockito::Server::new_async().await;
        let _m2 = down
            .mock("POST", "/api/auth.test")
            .with_status(503)
            .create_async()
            .await;
        let ch = SlackChannel::new("test", cfg_for(&down.url()), store_for_test());
        let r = ch.probe().await.unwrap();
        assert_eq!(
            r.outcome,
            ProbeOutcome::Unreachable,
            "a 5xx says nothing about the credential; calling it Unauthenticated \
             makes an operator rotate a working token because slack.com blipped"
        );
    }

    /// An absent credential is `Incomplete` (fill the store), never
    /// `Unauthenticated` (rotate the token) — and no network call is made,
    /// because there is nothing to authenticate with.
    #[tokio::test]
    async fn probe_reports_incomplete_when_the_bot_token_is_absent() {
        use wcore_channels::ProbeOutcome;
        let mut server = mockito::Server::new_async().await;
        let never = server
            .mock("POST", "/api/auth.test")
            .expect(0)
            .create_async()
            .await;

        let store = MapStore::new(&[("slack.test.signing_secret", "shhh")]);
        let ch = SlackChannel::new("test", cfg_for(&server.url()), store);
        let r = ch.probe().await.unwrap();
        never.assert_async().await;

        assert_eq!(r.outcome, ProbeOutcome::Incomplete);
        assert!(!r.outcome.is_ready());
        assert!(
            r.findings
                .iter()
                .any(|f| f.contains("slack.test.bot_token")),
            "the report must name the handle the operator has to fill: {:?}",
            r.findings
        );
    }

    /// The report names handles, never values.
    #[tokio::test]
    async fn probe_output_never_carries_the_bot_token() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/api/auth.test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"invalid_auth"}"#)
            .create_async()
            .await;
        let ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        let r = ch.probe().await.unwrap();
        let rendered = format!("{r:?}");
        assert!(
            !rendered.contains("xoxb-test-token"),
            "the probe report leaked the bot token: {rendered}"
        );
    }

    /// Start a channel against `server` with a token Slack accepts, and drain
    /// the `Connected` event so later counts are unambiguous.
    async fn started_against(server: &mockito::Server) -> SlackChannel {
        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.expect("auth.test mock accepts the token");
        let _ = ch.poll_events().await.unwrap();
        ch
    }

    fn ok_false(code: &str) -> String {
        format!(r#"{{"ok":false,"error":"{code}"}}"#)
    }

    /// Every outbound surface must publish, not just `send_message`.
    ///
    /// `react` is the first outbound call the engine makes on an inbound
    /// message (the ack emoji), so on a token revoked mid-run it is the most
    /// likely discovery point — and it mapped the refusal straight to
    /// `ChannelError::Auth`, telling the caller and telling health nothing.
    /// `edit_message`, `delete_message` and `fetch_media` had the same hole.
    #[tokio::test]
    async fn every_outbound_surface_publishes_auth_expired_on_a_refusal() {
        for (method, path) in [
            ("react", "/api/reactions.add"),
            ("edit", "/api/chat.update"),
            ("delete", "/api/chat.delete"),
        ] {
            let mut server = mockito::Server::new_async().await;
            let _auth = server
                .mock("POST", "/api/auth.test")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(r#"{"ok":true,"user_id":"U123","team":"acme"}"#)
                .create_async()
                .await;
            let _refuse = server
                .mock("POST", path)
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(ok_false("token_revoked"))
                .create_async()
                .await;

            let mut ch = started_against(&server).await;
            let err = match method {
                "react" => ch.react("C1", "1234.5678", "👀").await.unwrap_err(),
                "edit" => ch.edit_message("C1", "1234.5678", "new").await.unwrap_err(),
                _ => ch.delete_message("C1", "1234.5678").await.unwrap_err(),
            };
            assert!(
                matches!(err, ChannelError::Auth(_)),
                "{method}: got {err:?}"
            );

            let evs = ch.poll_events().await.unwrap();
            let expired = evs
                .iter()
                .filter(|e| matches!(e, ChannelEvent::AuthExpired { .. }))
                .count();
            assert_eq!(
                expired, 1,
                "{method}: a refused credential discovered here must reach the \
                 health surface, or the channel keeps reading Healthy: {evs:?}"
            );
        }
    }

    /// The publish must be able to NOT happen. A surface that published on
    /// every failure would pass the test above and would drive a live channel
    /// to `Unauthenticated` the first time an edit targeted a deleted message.
    #[tokio::test]
    async fn a_request_fault_on_an_outbound_surface_publishes_nothing() {
        let mut server = mockito::Server::new_async().await;
        let _auth = server
            .mock("POST", "/api/auth.test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"user_id":"U123","team":"acme"}"#)
            .create_async()
            .await;
        let _refuse = server
            .mock("POST", "/api/chat.update")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(ok_false("message_not_found"))
            .create_async()
            .await;

        let mut ch = started_against(&server).await;
        let err = ch.edit_message("C1", "1234.5678", "new").await.unwrap_err();
        assert!(
            !matches!(err, ChannelError::Auth(_)),
            "a missing message is not a credential verdict: {err:?}"
        );
        let evs = ch.poll_events().await.unwrap_or_default();
        assert_eq!(
            evs.iter()
                .filter(|e| matches!(e, ChannelEvent::AuthExpired { .. }))
                .count(),
            0,
            "a request fault must not accuse the credential: {evs:?}"
        );
    }

    #[tokio::test]
    async fn send_message_hits_chat_postmessage_with_bearer() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .match_header("authorization", "Bearer xoxb-test-token")
            .match_header(
                "content-type",
                mockito::Matcher::Regex("application/json.*".to_string()),
            )
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "channel": "C1",
                "text": "hello"
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"1234.567","channel":"C1"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("C1", "hello"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "1234.567");
        assert_eq!(receipt.conversation_id, "C1");
        assert_eq!(receipt.ts_secs, 1234);

        mock.assert_async().await;
    }

    /// A keyed send still puts the key on the wire — **and that is a different
    /// claim from the capability bit.**
    ///
    /// This test was named `slack_declares_idempotency_only_because_it_sends_
    /// the_header` and asserted `supports_outbound_idempotency() == true` right
    /// here, treating header-on-wire as evidence for destination-deduplicates.
    /// A mock cannot tell those apart: it answers whatever it was told to
    /// answer. Driving the real API on 2026-07-30 showed a replayed key
    /// producing **two** messages, so the two claims are now separated — what a
    /// mock can prove is asserted here, and what only the platform can answer
    /// is asserted in `wcore-channels-registry/tests/live_slack_actions.rs`.
    ///
    /// The header is deliberately still sent: a Slack-compatible destination
    /// reached through `api_base_url` may honour it, and this mock is what
    /// stops it being dropped silently if it ever becomes load-bearing again.
    #[tokio::test]
    async fn a_keyed_send_puts_the_key_on_the_wire_though_slack_ignores_it() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .match_header("idempotency-key", "cron:job-a:1785121776528")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"1.0","channel":"C1"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        assert!(
            !ch.supports_outbound_idempotency(),
            "Slack ignores the Idempotency-Key header — measured live 2026-07-30, a replayed key \
             produced two messages. The delivery spine reads this bit to decide whether to re-send \
             an outcome-unknown delivery, so `true` here is a production duplicate."
        );
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        ch.send_message_idempotent(
            OutgoingMessage::text("C1", "hello"),
            "cron:job-a:1785121776528",
        )
        .await
        .unwrap();

        mock.assert_async().await;
    }

    /// An unkeyed send must NOT carry the header, or every ordinary message
    /// would present a key the destination could collapse against.
    #[tokio::test]
    async fn an_unkeyed_send_carries_no_idempotency_header() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .match_header("idempotency-key", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"1.0","channel":"C1"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();
        ch.send_message(OutgoingMessage::text("C1", "hello"))
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_retries_on_5xx() {
        let mut server = mockito::Server::new_async().await;
        let fail = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(503)
            .with_body("upstream")
            .expect(1)
            .create_async()
            .await;
        let succeed = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"42.0","channel":"C1"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("C1", "hi"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "42.0");

        fail.assert_async().await;
        succeed.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_honours_retry_after_on_429() {
        let mut server = mockito::Server::new_async().await;
        let throttled = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(429)
            .with_header("Retry-After", "0")
            .with_body("rate-limited")
            .expect(1)
            .create_async()
            .await;
        let succeed = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"99.0","channel":"C1"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let receipt = ch
            .send_message(OutgoingMessage::text("C1", "hi"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "99.0");

        throttled.assert_async().await;
        succeed.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_4xx_is_permanent() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(401)
            .with_body("invalid_auth")
            .expect(1)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let err = ch
            .send_message(OutgoingMessage::text("C1", "hi"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_message_ok_false_invalid_auth_surfaces_as_auth_error() {
        // Slack 200-with-ok:false surface for permanent auth failure.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.postMessage")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"invalid_auth"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let err = ch
            .send_message(OutgoingMessage::text("C1", "hi"))
            .await
            .unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn ingest_event_valid_signature_enqueues_message() {
        let cfg = cfg_for("https://unused.example");
        let store = store_for_test();
        let mut ch = SlackChannel::new("test", cfg, store);
        ch.start().await.unwrap();
        let _ = ch.poll_events().await.unwrap();

        let body = r#"{"type":"event_callback","event":{"type":"message","channel":"C1","user":"U1","text":"hi","ts":"1700000000.000100"}}"#;
        let ts = Utc::now().timestamp().to_string();
        let sig = auth::expected_signature("shhh", &ts, body);

        let out = ch.ingest_event(body, &sig, &ts).await.unwrap();
        assert!(out.is_none(), "no challenge expected for event_callback");

        let evs = ch.poll_events().await.unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.text, "hi");
                assert_eq!(msg.author, "U1");
                assert_eq!(msg.conversation_id, "C1");
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ingest_event_invalid_signature_errors() {
        let mut ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        ch.start().await.unwrap();

        let body = r#"{"type":"event_callback","event":{"type":"message","channel":"C1","user":"U1","text":"hi","ts":"1700000000.000100"}}"#;
        let ts = Utc::now().timestamp().to_string();
        // Wrong signature.
        let err = ch.ingest_event(body, "v0=deadbeef", &ts).await.unwrap_err();
        assert!(matches!(err, SlackError::SignatureMismatch));
    }

    #[tokio::test]
    async fn ingest_event_stale_timestamp_errors() {
        let mut ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        ch.start().await.unwrap();

        let body = r#"{"type":"event_callback","event":{"type":"message","channel":"C1","user":"U1","text":"hi","ts":"1700000000.000100"}}"#;
        // 1 hour ago — outside the 5-minute replay window.
        let stale_ts = (Utc::now().timestamp() - 3600).to_string();
        let sig = auth::expected_signature("shhh", &stale_ts, body);

        let err = ch.ingest_event(body, &sig, &stale_ts).await.unwrap_err();
        assert!(matches!(err, SlackError::StaleTimestamp(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn ingest_event_url_verification_surfaces_challenge() {
        let mut ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        ch.start().await.unwrap();

        let body = r#"{"type":"url_verification","challenge":"hello-world","token":"x"}"#;
        let ts = Utc::now().timestamp().to_string();
        let sig = auth::expected_signature("shhh", &ts, body);

        let out = ch.ingest_event(body, &sig, &ts).await.unwrap();
        assert_eq!(out.as_deref(), Some("hello-world"));
    }

    #[tokio::test]
    async fn config_schema_is_valid_json() {
        let ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        let parsed: serde_json::Value =
            serde_json::from_str(ch.config_schema()).expect("schema parses");
        assert_eq!(parsed["title"].as_str(), Some("SlackChannelConfig"));
    }

    #[tokio::test]
    async fn max_message_len_is_slack_cap() {
        // wayland#934: this asserts the literal the function returns one line above, so it
        // restates the code rather than testing it. It is retained as a change-detector, but
        // the check that can actually catch a wrong cap is
        // `wcore-channels-registry/tests/delivery_semantics_declaration.rs`, which binds this
        // number to `slack.cap` in `docs/delivery-semantics.md` through the PRODUCTION
        // factory. The PLATFORM limit is now MEASURED (wayland#934, 2026-08-27):
        // 4,040 chars is the largest single Slack message; at 4,041 the API splits
        // into 4,000-char messages. 4,000 is used because it is the split size
        // itself, so a full-length chunk can never be re-split by Slack.
        let ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        assert_eq!(ch.max_message_len(), Some(4_000));
    }

    #[tokio::test]
    async fn start_missing_bot_token_errors() {
        let store = MapStore::new(&[("slack.test.signing_secret", "shhh")]);
        let mut ch = SlackChannel::new("test", cfg_for("https://unused.example"), store);
        let err = ch.start().await.unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        assert_eq!(ch.state(), ConnectionState::Connecting);
    }

    // ---- native actions: edit / delete (Phase 24 C3) ----------------------

    /// The edit reaches `chat.update` with the bearer, the `ts` and the new
    /// text — asserted on the WIRE, not on the return value.
    ///
    /// `mock.assert_async()` is the load-bearing line: without it the test
    /// would pass against an adapter that never issued a request at all, which
    /// is precisely the shape a defaulted `Unsupported` would produce if the
    /// override were removed and the error swallowed.
    #[tokio::test]
    async fn edit_hits_chat_update_with_bearer_ts_and_text() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.update")
            .match_header("authorization", "Bearer xoxb-test-token")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "channel": "C1",
                "ts": "1234.567",
                "text": "edited body"
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"1234.567","channel":"C1"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();

        let receipt = ch
            .edit_message("C1", "1234.567", "edited body")
            .await
            .expect("edit succeeds");
        assert_eq!(receipt.id, "1234.567");
        assert_eq!(receipt.conversation_id, "C1");
        assert_eq!(receipt.ts_secs, 1234);

        mock.assert_async().await;
    }

    /// The delete reaches `chat.delete` with `channel` + `ts` and nothing else.
    #[tokio::test]
    async fn delete_hits_chat_delete_with_channel_and_ts() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/chat.delete")
            .match_header("authorization", "Bearer xoxb-test-token")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "channel": "C1",
                "ts": "1234.567"
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"ts":"1234.567","channel":"C1"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();

        ch.delete_message("C1", "1234.567")
            .await
            .expect("delete succeeds");

        mock.assert_async().await;
    }

    /// **The failing direction.** A platform `ok:false` must surface as an
    /// error, never as a silent success — a caller that believes a message was
    /// deleted when it still exists is the worst outcome this operation has.
    ///
    /// This is the control for the two cases above: they prove the gate can
    /// pass, this proves it can fail.
    #[tokio::test]
    async fn a_platform_refusal_is_an_error_not_a_silent_success() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/api/chat.delete")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"message_not_found"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();

        let err = ch.delete_message("C1", "9999.000").await.unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("message_not_found"),
            "the platform's own code must reach the operator, got {rendered}"
        );
        // …and specifically NOT `Unsupported`, which would mean the override
        // vanished and the trait default answered instead.
        assert!(
            !matches!(err, ChannelError::Unsupported { .. }),
            "got Unsupported — the edit/delete override is missing"
        );
    }

    /// Auth failure on a mutate is `Auth`, distinctly from a platform refusal,
    /// so an operator is told to fix the token rather than to fix the message.
    #[tokio::test]
    async fn an_invalid_token_on_edit_is_auth_not_rejected() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/api/chat.update")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error":"invalid_auth"}"#)
            .create_async()
            .await;

        let mut ch = SlackChannel::new("test", cfg_for(&server.url()), store_for_test());
        ch.start().await.unwrap();

        let err = ch.edit_message("C1", "1.0", "x").await.unwrap_err();
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
    }

    /// The declaration and the behaviour must agree — the same binding
    /// `slack_declares_idempotency_only_because_it_sends_the_header` makes for
    /// outbound idempotency, applied to native actions.
    #[tokio::test]
    async fn native_action_declaration_matches_behaviour() {
        use wcore_channels::ActionSupport;
        let ch = SlackChannel::new("test", cfg_for("https://unused.example"), store_for_test());
        let a = ch.native_actions();
        assert_eq!(a.edit, ActionSupport::Implemented);
        assert_eq!(a.delete, ActionSupport::Implemented);
        assert_eq!(a.react, ActionSupport::Implemented);
        // Slack genuinely has no bot typing API. This must be the PERMANENT
        // state, not the backlog one — recording it as `NotImplemented` would
        // put a task on a list that can never be completed.
        assert_eq!(a.typing, ActionSupport::PlatformHasNoApi);
        assert!(!a.note.is_empty(), "a non-implemented op must say why");

        // Not started → the ops answer Auth (token not loaded), which proves
        // the override exists. A missing override would answer Unsupported.
        let e = ch.edit_message("C1", "1.0", "x").await.unwrap_err();
        assert!(
            !matches!(e, ChannelError::Unsupported { .. }),
            "declared Implemented but the trait default answered: {e:?}"
        );
        let d = ch.delete_message("C1", "1.0").await.unwrap_err();
        assert!(
            !matches!(d, ChannelError::Unsupported { .. }),
            "declared Implemented but the trait default answered: {d:?}"
        );
    }
}
