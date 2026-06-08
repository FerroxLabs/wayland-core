//! `MsTeamsError` — MS Teams-specific error variants.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MsTeamsError {
    #[error("OAuth2 token fetch failed ({status}): {body}")]
    TokenFetch { status: u16, body: String },
    #[error("send failed ({status}): {body}")]
    SendFailed { status: u16, body: String },
    #[error("network: {0}")]
    Network(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("invalid chat_id format (expected serviceUrl|conversationId)")]
    InvalidChatId,
}
