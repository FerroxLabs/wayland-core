//! Slack-adapter error surface.
//!
//! Converts to `wcore_channels::ChannelError` via `From` so the
//! `Channel` trait surface stays in the parent crate's vocabulary.

use thiserror::Error;
use wcore_channels::ChannelError;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SlackError {
    #[error("http transport: {0}")]
    Http(String),

    #[error("slack rejected the request: {0}")]
    Api(String),

    #[error("slack auth failed: {0}")]
    Auth(String),

    #[error("config: {0}")]
    Config(String),

    #[error("credentials lookup failed: {0}")]
    Credentials(String),

    #[error("malformed event payload: {0}")]
    MalformedPayload(String),

    #[error("webhook signature mismatch")]
    SignatureMismatch,

    #[error("webhook timestamp outside replay-protection window (delta {0}s)")]
    StaleTimestamp(i64),

    #[error("retry budget exhausted after {attempts} attempts: {last}")]
    RetryExhausted { attempts: u32, last: String },
}

impl From<SlackError> for ChannelError {
    fn from(e: SlackError) -> Self {
        match e {
            SlackError::Auth(m) => ChannelError::Auth(m),
            SlackError::Http(m) => ChannelError::Transport(m),
            SlackError::Config(m) => ChannelError::Config(m),
            SlackError::Credentials(m) => ChannelError::Auth(m),
            SlackError::Api(m) => ChannelError::Rejected(m),
            SlackError::MalformedPayload(m) => ChannelError::Rejected(m),
            SlackError::SignatureMismatch => {
                ChannelError::Auth("slack webhook signature mismatch".to_string())
            }
            SlackError::StaleTimestamp(delta) => ChannelError::Rejected(format!(
                "slack webhook timestamp outside replay window (delta {delta}s)"
            )),
            SlackError::RetryExhausted { attempts, last } => {
                ChannelError::Transport(format!("retry budget exhausted ({attempts}): {last}"))
            }
        }
    }
}
