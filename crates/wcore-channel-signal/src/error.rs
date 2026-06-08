//! Internal error surface for the Signal adapter. Public surface
//! through the `Channel` trait is `wcore_channels::ChannelError`; the
//! local error converts via `From`.

use thiserror::Error;
use wcore_channels::ChannelError;

#[derive(Debug, Error)]
pub enum SignalError {
    /// Failed to spawn `signal-cli` (binary not found, permission
    /// denied, etc.).
    #[error("spawn: {0}")]
    Spawn(String),

    /// I/O error talking to the subprocess.
    #[error("io: {0}")]
    Io(String),

    /// signal-cli responded but the JSON-RPC envelope contained an
    /// `error` object.
    #[error("signal-cli error ({code}): {message}")]
    Rpc { code: i64, message: String },

    /// signal-cli's response could not be decoded.
    #[error("decode: {0}")]
    Decode(String),

    /// signal-cli did not respond within `send_timeout_secs`.
    #[error("timeout waiting for response (id={request_id})")]
    Timeout { request_id: u64 },

    /// `send_message` / `poll_events` called before `start()` (or
    /// after `stop()`).
    #[error("channel not started")]
    NotStarted,

    /// Reader task observed EOF on the child's stdout before a pending
    /// request resolved.
    #[error("subprocess closed stdout before responding")]
    SubprocessClosed,
}

impl From<SignalError> for ChannelError {
    fn from(e: SignalError) -> Self {
        match e {
            SignalError::Spawn(m) => ChannelError::Transport(format!("spawn signal-cli: {m}")),
            SignalError::Io(m) => ChannelError::Transport(m),
            SignalError::Rpc { code, message } => {
                ChannelError::Rejected(format!("{code}: {message}"))
            }
            SignalError::Decode(m) => ChannelError::Other(format!("decode: {m}")),
            SignalError::Timeout { request_id } => {
                ChannelError::Transport(format!("timeout waiting for response id={request_id}"))
            }
            SignalError::NotStarted => ChannelError::NotStarted,
            SignalError::SubprocessClosed => {
                ChannelError::Transport("signal-cli subprocess closed unexpectedly".into())
            }
        }
    }
}
