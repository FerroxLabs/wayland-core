//! Public error type for the execution-backend contract.
//!
//! `thiserror` per AGENTS.md: this is a public API error and callers must be
//! able to match on it — particularly on `CredentialAbsent`, which is the
//! fail-closed verdict a cloud backend returns instead of falling back.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, ExecError>;

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("resource budget is invalid: every field must be non-zero")]
    InvalidResourceBudget,

    #[error("malformed task: {0}")]
    MalformedTask(String),

    #[error("backend {backend_id} is unavailable: {detail}")]
    Unavailable { backend_id: String, detail: String },

    /// The fail-closed verdict. A backend that needs a credential and has none
    /// returns THIS; it never silently degrades to another backend.
    #[error("backend {backend_id} has no credential ({env}); refusing to run and NOT falling back")]
    CredentialAbsent { backend_id: String, env: String },

    #[error("task {task_id} is not known to backend {backend_id}")]
    UnknownTask { backend_id: String, task_id: String },

    #[error("execution failed: {0}")]
    Exec(String),

    #[error("transport failed: {0}")]
    Transport(String),

    #[error("receipt is invalid: {0}")]
    Receipt(String),

    #[error("receipt attestation failed verification")]
    Attestation,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialize(String),
}

impl From<serde_json::Error> for ExecError {
    fn from(value: serde_json::Error) -> Self {
        ExecError::Serialize(value.to_string())
    }
}

impl From<wcore_sandbox::SandboxError> for ExecError {
    fn from(value: wcore_sandbox::SandboxError) -> Self {
        ExecError::Exec(value.to_string())
    }
}
