//! Error types for the sandbox crate.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SandboxError {
    /// Persistent or remotely supplied configuration attempted to disable
    /// containment. Full bypass requires a resolver-produced local launch
    /// grant and cannot be selected through config or environment state.
    #[error(
        "sandbox bypass cannot be activated by configuration or environment; use an explicit local Dangerous launch"
    )]
    UnsafeBypassSource,
    #[error("unknown sandbox backend selection: {0}")]
    UnknownBackend(String),
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
    /// Captured stdout + stderr exceeded the fixed host-memory ceiling.
    #[error("sandbox child output exceeded {limit_bytes} bytes")]
    OutputLimitExceeded { limit_bytes: usize },
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
    /// Windows share arbitration refused an operation because another handle on
    /// the object omits `FILE_SHARE_DELETE`.
    ///
    /// This is a DISTINCT variant, not a `SandboxError::Io`, because both
    /// properties are load-bearing and a bare `io::Error` can only carry one of
    /// them. The retry gate matches on `raw_os_error()`, so the errno must stay
    /// raw — `io::Error::new(kind, message)` reports `raw_os_error() == None`
    /// and re-hides it. But a human reading a soak failure needs to know WHICH
    /// object was refused and by WHICH operation. Returning the bare errno to
    /// keep the retry working cost exactly that: every Windows soak failure read
    /// `durable recovery failed (os error 32)` with no path and no operation, so
    /// three candidate call sites could not be told apart from the log.
    #[error("share arbitration refused {operation} on {path}: {source}")]
    ShareViolation {
        operation: String,
        path: std::path::PathBuf,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, SandboxError>;
