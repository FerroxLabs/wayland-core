//! Error types for the sandbox crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SandboxError {
    /// The caller asked for sandboxed execution but the command does not
    /// require it; callers should bypass to a direct exec. Returned by
    /// trait helpers (not by `execute` itself).
    #[error("sandbox not required for this command (caller should bypass)")]
    NotRequired,
    /// The backend cannot enforce the requested policy (e.g. Docker has no
    /// DNS gate, so `NetworkPolicy::AllowHosts` is not supported).
    #[error("sandbox policy not supported by this backend: {0}")]
    PolicyNotSupported(String),
    /// Child process exec or wait failed.
    #[error("sandbox child execution failed: {0}")]
    ExecFailed(String),
    /// Wall-clock timeout expired before the child exited.
    #[error("sandbox child timed out")]
    Timeout,
    #[error("docker backend disabled (feature `live-docker` off)")]
    DockerDisabled,
    #[error("docker io: {0}")]
    DockerIo(String),
    /// Filesystem path requested by the caller is not on the manifest's
    /// read/write allowlist.
    #[error("path not on filesystem allowlist: {0}")]
    PathDenied(String),
    #[error("network policy denied: {0}")]
    NetworkDenied(String),
    /// Resource limit (memory/cpu) was exceeded during execution. NOT used
    /// for "sandbox bypass" conditions — that's `NotRequired`.
    #[error("resource budget exceeded: {0}")]
    BudgetExceeded(String),
    #[error("manifest parse: {0}")]
    ManifestParse(#[from] toml::de::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, SandboxError>;
