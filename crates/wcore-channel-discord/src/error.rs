//! Internal error surface for the Discord adapter. Public surface
//! through the `Channel` trait is `wcore_channels::ChannelError`; the
//! local error converts via `From`.

use thiserror::Error;
use wcore_channels::ChannelError;

#[derive(Debug, Error)]
pub enum DiscordError {
    #[error("http transport: {0}")]
    Http(String),
    #[error("api returned non-success: {0}")]
    ApiNotOk(String),
    #[error("auth: bot token missing or rejected: {0}")]
    Auth(String),
    #[error("rejected by platform ({code}): {description}")]
    Rejected { code: u16, description: String },
    #[error("rate limited; retry_after = {retry_after_secs}s")]
    RateLimited { retry_after_secs: f64 },
    #[error("config: {0}")]
    Config(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("gateway: {0}")]
    Gateway(String),
}

impl From<DiscordError> for ChannelError {
    fn from(e: DiscordError) -> Self {
        match e {
            DiscordError::Http(m) => ChannelError::Transport(m),
            DiscordError::ApiNotOk(m) => ChannelError::Other(m),
            DiscordError::Auth(m) => ChannelError::Auth(m),
            DiscordError::Rejected { code, description } => {
                ChannelError::Rejected(format!("{code}: {description}"))
            }
            DiscordError::RateLimited { retry_after_secs } => {
                ChannelError::Transport(format!("rate limited after retries ({retry_after_secs}s)"))
            }
            DiscordError::Config(m) => ChannelError::Config(m),
            DiscordError::Decode(m) => ChannelError::Other(format!("decode: {m}")),
            DiscordError::Gateway(m) => ChannelError::Transport(format!("gateway: {m}")),
        }
    }
}
