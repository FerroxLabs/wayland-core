//! `wcore-channels` — runtime abstraction for chat-platform adapters
//! (Slack, Discord, Telegram, WhatsApp, Signal, email, SMS, …).
//!
//! Defines the `Channel` trait + `ChannelEvent` enum + config loader
//! (landed in the v0.7.0 channels foundation). Individual channel impls
//! land as their own crates (`wcore-channel-slack` etc.) in the
//! v0.8 channels release. The `ChannelManager` that drives them lives
//! in `manager.rs`.
//!
//! Channels are message-passing surfaces, not transport primitives —
//! they wrap whatever platform-native API exists (HTTP REST, WS
//! gateway, subprocess, IMAP/SMTP) behind a uniform send + poll
//! interface so the engine + UI don't care which platform a message
//! came from.

pub mod auto_register;
pub mod chunk;
pub mod config;
pub mod dispatch;
pub mod error;
pub mod event;
pub mod manager;
pub mod mock;
pub mod outgoing;
pub mod webhook;

pub use chunk::chunk_message;
pub use config::{ChannelConfig, ChannelConfigLoader};
pub use dispatch::{
    AccessDecision, AckMode, AutoReplyRateLimiter, ChannelToolPosture, DEFAULT_AUTO_REPLY_WINDOW,
    DEFAULT_CONVERSATION_CAP, DEFAULT_MAX_AUTO_REPLIES, DedupeCache, DedupeKey, DispatchOutcome,
    DmPolicy, GroupPolicy, InboundPolicy, TurnAdmission, build_session_key, classify,
    decide_access, evaluate,
};
pub use error::ChannelError;
pub use event::{
    Attachment, ChannelEvent, ChatType, ConnectionState, IncomingMessage, MAX_INBOX, MediaKind,
    MentionKind, MessageReceipt, push_bounded,
};
pub use manager::{ChannelManager, TaggedEvent};
pub use mock::MockChannel;
pub use outgoing::OutgoingMessage;
pub use webhook::{WebhookRequest, WebhookResponse};

use async_trait::async_trait;

/// One chat-platform adapter — wraps the platform's native API
/// behind a uniform send + poll surface.
///
/// Lifecycle: construct → `start()` → loop `poll_events()` /
/// `send_message()` until `stop()` is called. `start`/`stop` are
/// idempotent (calling `start` on an already-started channel is a
/// no-op, same for `stop` on a stopped one).
#[async_trait]
pub trait Channel: Send + Sync {
    /// Stable identifier for this channel. Matches the config file
    /// stem at `~/.wayland/channels/<name>.toml`. Used for routing.
    fn name(&self) -> &str;

    /// Platform tag — `"slack"`, `"discord"`, `"telegram"`, etc.
    /// Multiple channel instances can share a platform (two Slack
    /// workspaces, for example) but each has a unique `name()`.
    fn platform(&self) -> &str;

    /// Open the underlying connection / start polling. Idempotent.
    async fn start(&mut self) -> Result<(), ChannelError>;

    /// Close the underlying connection. Idempotent. After `stop()`
    /// further `poll_events` / `send_message` calls surface
    /// `ChannelError::NotStarted`.
    async fn stop(&mut self) -> Result<(), ChannelError>;

    /// Poll for any events that have arrived since the last call.
    /// Returns an empty vec if no events are ready. Non-blocking by
    /// contract — channels that need to wait spawn an internal task
    /// in `start()` and buffer into a queue.
    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError>;

    /// Send a message through this channel. Returns a receipt with
    /// the platform-assigned ID (so callers can correlate with
    /// later `ChannelEvent::MessageReceived` echoes).
    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError>;

    /// Returns the JSON-schema doc string for this channel's
    /// config TOML. UI uses this to render a setup form; tests use
    /// it to validate config files.
    fn config_schema(&self) -> &str;

    /// Handle of the connector's internal background task, if any. The manager
    /// uses this to detect a dead task and trigger supervised reconnect even when
    /// `poll_events` returns `Ok(vec![])` (the inbox-drain connectors whose
    /// background task can die silently). Default `None`: webhook-only connectors
    /// have no task.
    fn task_handle(&self) -> Option<&tokio::task::JoinHandle<()>> {
        None
    }

    /// Maximum length (in Unicode scalar values) of a single outbound
    /// message this platform accepts, or `None` when effectively
    /// unbounded / unknown. [`ChannelManager::send_to`] splits longer
    /// bodies into in-order chunks via
    /// [`chunk_message`](crate::chunk::chunk_message) before sending, so
    /// an over-long agent reply is delivered in pieces instead of being
    /// rejected and dropped by the platform. Each connector declares its
    /// own cap here — the shared layer never hardcodes a per-platform
    /// limit.
    fn max_message_len(&self) -> Option<usize> {
        None
    }

    /// Send a transient "typing…" indicator to `conversation_id`.
    ///
    /// Default: no-op `Ok(())` — platforms without a typing API simply do
    /// nothing. The inbound subscriber calls this periodically while a turn
    /// is running (when the channel's ack mode enables typing) so a human
    /// sees the bot is working. Must be cheap and best-effort; a failure is
    /// logged and ignored, never fatal to the turn.
    async fn send_typing(&self, _conversation_id: &str) -> Result<(), ChannelError> {
        Ok(())
    }

    /// React to a message with a single unicode emoji — the ack/status
    /// signal used by the subscriber's ack state machine (👀 received →
    /// ✅ done / ❌ failed).
    ///
    /// Default: `Rejected` (the platform has no reaction API, or it isn't
    /// implemented for this connector). The subscriber treats a reaction
    /// failure as non-fatal.
    async fn react(
        &self,
        _conversation_id: &str,
        _message_id: &str,
        _emoji: &str,
    ) -> Result<(), ChannelError> {
        Err(ChannelError::Rejected("reactions unsupported".to_string()))
    }

    /// Handle an inbound webhook HTTP request routed to this channel by
    /// the inbound webhook host.
    ///
    /// Default: **unsupported** — poll-based connectors (telegram,
    /// matrix, signal, …) and any connector whose inbound path is not yet
    /// signature-verified return `Rejected`, so the host never exposes an
    /// unauthenticated parse to the network. Webhook connectors that
    /// verify the platform signature (Slack, WhatsApp, Twilio SMS)
    /// override this to verify → parse → enqueue (mirroring their existing
    /// `ingest_*` methods) and return a [`WebhookResponse`].
    ///
    /// Takes `&self` (not `&mut self`): connectors enqueue through their
    /// interior-mutable inbox, so the host can ingest concurrently with
    /// the poll loop without an exclusive borrow.
    async fn ingest_webhook(&self, _req: &WebhookRequest) -> Result<WebhookResponse, ChannelError> {
        Err(ChannelError::Rejected(
            "channel does not accept inbound webhooks".to_string(),
        ))
    }

    /// Fetch the raw bytes of an inbound [`Attachment`](crate::event::Attachment)
    /// using THIS connector's own credentials and platform media protocol.
    ///
    /// Media URLs differ per platform: Telegram/Discord expose a directly
    /// fetchable URL; Slack needs a bearer on `url_private`; WhatsApp resolves
    /// a media-id to a short-lived URL then downloads it; Matrix translates an
    /// `mxc://` URI to the authenticated download endpoint. The agent-side
    /// media enricher calls this through [`ChannelManager::fetch_media_on`] so
    /// credentials never leave the connector boundary.
    ///
    /// Default: **unsupported** — a connector that doesn't override this (no
    /// inbound media, or none wired yet) returns `Rejected`, and the enricher
    /// falls back to the bare-URL summary. Takes `&self` (like `react` /
    /// `ingest_webhook`): the read-only download uses the connector's
    /// immutable client + token.
    async fn fetch_media(
        &self,
        _attachment: &crate::event::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        Err(ChannelError::Rejected(
            "media fetch unsupported".to_string(),
        ))
    }
}
