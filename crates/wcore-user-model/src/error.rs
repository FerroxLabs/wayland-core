use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum UserModelError {
    #[error("backend transport: {0}")]
    Transport(String),
    #[error("backend auth: {0}")]
    Auth(String),
    #[error("backend rejected: {0}")]
    Rejected(String),
    /// Backend mis-configuration — e.g. unknown backend name or missing
    /// required env var at bootstrap. Used by
    /// `wcore-honcho-adapter::select_backend_from_env`.
    #[error("backend config: {0}")]
    Config(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}
