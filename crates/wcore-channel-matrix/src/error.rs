//! `MatrixError` — Matrix-specific error variants.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MatrixError {
    #[error("HTTP error {status}: {body}")]
    Http { status: u16, body: String },
    #[error("network: {0}")]
    Network(String),
    #[error("parse: {0}")]
    Parse(String),
}
