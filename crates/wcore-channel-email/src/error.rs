//! Internal error surface for the email adapter. Public surface
//! through the `Channel` trait is `wcore_channels::ChannelError`; the
//! local error converts via `From`.

use thiserror::Error;
use wcore_channels::ChannelError;

#[derive(Debug, Error)]
pub enum EmailError {
    #[error("smtp transport: {0}")]
    Smtp(String),
    #[error("imap transport: {0}")]
    Imap(String),
    #[error("auth: smtp/imap credentials missing or rejected: {0}")]
    Auth(String),
    #[error("rejected by mail server: {0}")]
    Rejected(String),
    #[error("config: {0}")]
    Config(String),
    #[error("decode rfc5322: {0}")]
    Decode(String),
    #[error("envelope build: {0}")]
    Envelope(String),
}

impl From<EmailError> for ChannelError {
    fn from(e: EmailError) -> Self {
        match e {
            EmailError::Smtp(m) => ChannelError::Transport(m),
            EmailError::Imap(m) => ChannelError::Transport(m),
            EmailError::Auth(m) => ChannelError::Auth(m),
            EmailError::Rejected(m) => ChannelError::Rejected(m),
            EmailError::Config(m) => ChannelError::Config(m),
            EmailError::Decode(m) => ChannelError::Other(format!("decode: {m}")),
            EmailError::Envelope(m) => ChannelError::Rejected(format!("envelope: {m}")),
        }
    }
}
