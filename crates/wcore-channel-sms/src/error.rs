//! Twilio SMS adapter error surface.
//!
//! Converts to `wcore_channels::ChannelError` via `From` so the
//! `Channel` trait surface stays in the parent crate's vocabulary.

use thiserror::Error;
use wcore_channels::ChannelError;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SmsError {
    #[error("http transport: {0}")]
    Http(String),

    #[error("twilio rejected the request: {0}")]
    Api(String),

    #[error("twilio auth failed: {0}")]
    Auth(String),

    #[error("config: {0}")]
    Config(String),

    #[error("credentials lookup failed: {0}")]
    Credentials(String),

    #[error("malformed webhook payload: {0}")]
    MalformedPayload(String),

    #[error("webhook signature mismatch")]
    SignatureMismatch,

    #[error("retry budget exhausted after {attempts} attempts: {last}")]
    RetryExhausted { attempts: u32, last: String },
}

impl From<SmsError> for ChannelError {
    fn from(e: SmsError) -> Self {
        match e {
            SmsError::Auth(m) => ChannelError::Auth(m),
            SmsError::Http(m) => ChannelError::Transport(m),
            SmsError::Config(m) => ChannelError::Config(m),
            SmsError::Credentials(m) => ChannelError::Auth(m),
            SmsError::Api(m) => ChannelError::Rejected(m),
            SmsError::MalformedPayload(m) => ChannelError::Rejected(m),
            SmsError::SignatureMismatch => {
                ChannelError::Auth("twilio webhook signature mismatch".to_string())
            }
            SmsError::RetryExhausted { attempts, last } => {
                ChannelError::Transport(format!("retry budget exhausted ({attempts}): {last}"))
            }
        }
    }
}
