//! Public error surface for the cua tool family.

use thiserror::Error;

pub type CuaResult<T> = std::result::Result<T, CuaError>;

#[derive(Debug, Error)]
pub enum CuaError {
    /// Cooperative cancellation observed via `ctx.cancel.cancelled()`.
    #[error("cua op cancelled")]
    Cancelled,

    /// Op was blocked by `CuaPolicy` (forbidden app, key combo, or rate limit).
    #[error("policy denied: {reason}")]
    PolicyDenied { reason: String },

    /// Op needs HITL approval before continuing (S4 Suspend route).
    #[error("policy requires approval (suspend): {reason}")]
    PolicySuspended { reason: String },

    /// Backend-specific failure (shell exit code, FFI error, etc.).
    #[error("backend error: {0}")]
    Backend(String),

    /// IO-level failure (read/write screenshot, AT-SPI socket, etc.).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Image decode / encode failure during screenshot or redaction.
    #[error("image: {0}")]
    Image(String),

    /// Linux Wayland: the active compositor does not permit cross-application
    /// background input. The tool refuses to register at bootstrap rather
    /// than silently fall back (audit F7 positive invariance).
    #[error("wayland compositor restricted: {reason}")]
    WaylandRestricted { reason: String },

    /// The current host does not have `Capabilities.computer_use = true`
    /// advertised. The plugin layer refuses to mint a tool in that case.
    #[error("computer-use capability disabled on host")]
    CapabilityDisabled,

    /// The current platform doesn't have a backend implemented yet.
    #[error("platform not supported: {0}")]
    UnsupportedPlatform(&'static str),

    /// Invalid argument shape — typed surfacing for the JSON input layer.
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

impl From<image::ImageError> for CuaError {
    fn from(e: image::ImageError) -> Self {
        Self::Image(e.to_string())
    }
}
