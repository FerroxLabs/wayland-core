//! Internal error surface for the Telegram adapter. Public surface
//! through the `Channel` trait is `wcore_channels::ChannelError`; the
//! local error converts via `From`.

use thiserror::Error;
use wcore_channels::ChannelError;

#[derive(Debug, Error)]
pub enum TelegramError {
    #[error("http transport: {0}")]
    Http(String),
    #[error("api responded ok=false: {0}")]
    ApiNotOk(String),
    #[error("auth: bot token missing or rejected: {0}")]
    Auth(String),
    #[error("rejected by platform ({code}): {description}")]
    Rejected { code: i64, description: String },
    #[error("rate limited; retry_after = {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("config: {0}")]
    Config(String),
    #[error("decode: {0}")]
    Decode(String),
}

impl From<TelegramError> for ChannelError {
    fn from(e: TelegramError) -> Self {
        match e {
            TelegramError::Http(m) => ChannelError::Transport(m),
            TelegramError::ApiNotOk(m) => ChannelError::Other(m),
            TelegramError::Auth(m) => ChannelError::Auth(m),
            TelegramError::Rejected { code, description } => {
                ChannelError::Rejected(format!("{code}: {description}"))
            }
            TelegramError::RateLimited { retry_after_secs } => {
                ChannelError::Transport(format!("rate limited after retries ({retry_after_secs}s)"))
            }
            TelegramError::Config(m) => ChannelError::Config(m),
            TelegramError::Decode(m) => ChannelError::Other(format!("decode: {m}")),
        }
    }
}
