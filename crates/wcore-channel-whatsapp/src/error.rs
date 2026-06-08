//! WhatsApp-adapter error surface.
//!
//! Converts to `wcore_channels::ChannelError` via `From` so the
//! `Channel` trait surface stays in the parent crate's vocabulary.

use thiserror::Error;
use wcore_channels::ChannelError;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WhatsappError {
    #[error("http transport: {0}")]
    Http(String),

    #[error("whatsapp rejected the request: {0}")]
    Api(String),

    #[error("whatsapp auth failed: {0}")]
    Auth(String),

    #[error("config: {0}")]
    Config(String),

    #[error("credentials lookup failed: {0}")]
    Credentials(String),

    #[error("malformed event payload: {0}")]
    MalformedPayload(String),

    #[error("webhook signature mismatch")]
    SignatureMismatch,

    #[error("retry budget exhausted after {attempts} attempts: {last}")]
    RetryExhausted { attempts: u32, last: String },
}

impl From<WhatsappError> for ChannelError {
    fn from(e: WhatsappError) -> Self {
        match e {
            WhatsappError::Auth(m) => ChannelError::Auth(m),
            WhatsappError::Http(m) => ChannelError::Transport(m),
            WhatsappError::Config(m) => ChannelError::Config(m),
            WhatsappError::Credentials(m) => ChannelError::Auth(m),
            WhatsappError::Api(m) => ChannelError::Rejected(m),
            WhatsappError::MalformedPayload(m) => ChannelError::Rejected(m),
            WhatsappError::SignatureMismatch => {
                ChannelError::Auth("whatsapp webhook signature mismatch".to_string())
            }
            WhatsappError::RetryExhausted { attempts, last } => {
                ChannelError::Transport(format!("retry budget exhausted ({attempts}): {last}"))
            }
        }
    }
}
