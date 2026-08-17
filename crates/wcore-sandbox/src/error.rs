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
    /// The request could not be delivered to a child intact, so no child was
    /// started and nothing ran.
    ///
    /// Deliberately NOT [`Self::ExecFailed`]: the two say opposite things
    /// about the machine. `ExecFailed` means a spawn or wait broke, which can
    /// be transient and can mean the host is unwell. A refusal is
    /// deterministic and caller-fixable — the identical request will always be
    /// refused and a differently shaped one will not — so it is never evidence
    /// that anything is unhealthy, and callers that track tool health must not
    /// count it.
    #[error("request refused before any child started: {0}")]
    RequestRefused(String),
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
}

pub type Result<T> = std::result::Result<T, SandboxError>;
