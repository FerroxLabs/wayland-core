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
pub mod config;
pub mod dispatch;
pub mod error;
pub mod event;
pub mod manager;
pub mod mock;
pub mod outgoing;

pub use config::{ChannelConfig, ChannelConfigLoader};
pub use dispatch::{
    build_session_key, classify, decide_access, evaluate, AccessDecision, DedupeCache, DedupeKey,
    DispatchOutcome, DmPolicy, GroupPolicy, InboundPolicy, TurnAdmission,
};
pub use error::ChannelError;
pub use event::{
    Attachment, ChannelEvent, ChatType, ConnectionState, IncomingMessage, MediaKind, MentionKind,
    MessageReceipt,
};
pub use manager::{ChannelManager, TaggedEvent};
pub use mock::MockChannel;
pub use outgoing::OutgoingMessage;

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
}
