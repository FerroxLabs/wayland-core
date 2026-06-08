use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SubprocessPluginError {
    #[error("subprocess spawn failed: {0}")]
    SpawnFailed(#[source] std::io::Error),

    #[error("subprocess exited unexpectedly (status: {0:?})")]
    UnexpectedExit(Option<i32>),

    #[error("subprocess RPC parse error: {0}")]
    RpcParse(String),

    #[error("subprocess broken pipe (stdin/stdout dropped)")]
    BrokenPipe,

    #[error("subprocess plugin permission denied: {0}")]
    PermissionDenied(String),

    #[error("subprocess timeout (request exceeded deadline)")]
    Timeout,

    /// v0.6.5 Task 3.2 — the plugin returned a `SubprocessResponseBody::Error`
    /// envelope (a domain-level protocol error, distinct from transport
    /// failure). `code` is the plugin-supplied stable identifier.
    #[error("subprocess plugin protocol error [{code}]: {message}")]
    ProtocolError { code: String, message: String },

    /// v0.6.5 Task 3.2 — the plugin returned a response whose `id` did not
    /// match any in-flight request, or whose body variant did not match
    /// the verb that was sent.
    #[error("subprocess plugin response mismatch: {0}")]
    ResponseMismatch(String),

    /// v0.6.5 Task 3.2 — stdio worker task was cancelled or panicked. The
    /// subprocess (if any) has been killed and the runner is unusable.
    #[error("subprocess plugin worker terminated")]
    WorkerTerminated,
}

pub type Result<T> = std::result::Result<T, SubprocessPluginError>;
