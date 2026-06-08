//! v0.8.1 U5 — channel auto-registration types.
//!
//! Defines the [`ChannelFactory`] function pointer + [`ChannelLoadError`]
//! enum that the per-platform channel-registry crate (`wcore-channels-
//! registry`) consumes. The dispatch table itself lives in the registry
//! crate because individual channel crates depend on `wcore-channels`,
//! not the other way around, so the registry is the natural meeting
//! point.
//!
//! Factory contract: given the channel name (file stem of the TOML on
//! disk) + the parsed `[options]` table from `ChannelConfig`, return a
//! constructed [`Channel`](crate::Channel) ready for
//! [`ChannelManager::register`](crate::ChannelManager::register).

use std::sync::Arc;

use thiserror::Error;
use wcore_config::credentials::CredentialsStore;

use crate::Channel;

/// Function pointer that constructs one channel from its parsed
/// `[options]` table.
///
/// The credentials store argument is the engine-wide handle every
/// production adapter (Slack, Telegram, Email, Discord, SMS, WhatsApp)
/// uses to fetch keychain-backed secrets at `start()`. Adapters that
/// don't need credentials (e.g. Signal, whose creds live in
/// signal-cli's own store) ignore the argument.
pub type ChannelFactory = fn(
    name: String,
    options: &toml::Table,
    credentials: Arc<dyn CredentialsStore>,
) -> Result<Box<dyn Channel>, ChannelLoadError>;

/// Error variants surfaced while loading + constructing a channel from
/// disk. The boot path logs + skips on every variant so one bad config
/// can't take the whole agent down.
#[derive(Debug, Error)]
pub enum ChannelLoadError {
    /// Platform string in the TOML didn't match any registered factory.
    #[error("unknown platform: {0}")]
    UnknownPlatform(String),
    /// TOML parse or `serde::Deserialize` failure on either the outer
    /// `ChannelConfig` or the inner per-platform options table.
    #[error("config parse: {0}")]
    Config(String),
    /// Channel constructor surfaced a domain-specific error (e.g.
    /// invalid handle format) the factory chose to bubble up.
    #[error("channel construct: {0}")]
    Construct(String),
}
