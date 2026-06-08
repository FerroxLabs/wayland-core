//! `IMessageError` — iMessage-specific error variants.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum IMessageError {
    #[error("iMessage is macOS-only")]
    NotMacOs,
    #[error("osascript failed (exit {exit_code}): {stderr}")]
    AppleScript { exit_code: i32, stderr: String },
    #[error(
        "Automation consent denied — grant in System Settings → Privacy & Security → Automation"
    )]
    AutomationDenied,
    #[error("target chat not found — open it once in Messages.app to refresh, then retry")]
    ChatNotFound,
    #[error("chat.db error: {0}")]
    Database(String),
    #[error("send queue full ({0} in-flight)")]
    QueueFull(usize),
}

impl From<IMessageError> for wcore_channels::error::ChannelError {
    fn from(e: IMessageError) -> Self {
        match e {
            IMessageError::NotMacOs => Self::Config("iMessage is macOS-only".to_string()),
            IMessageError::AutomationDenied => Self::Auth(e.to_string()),
            IMessageError::AppleScript { .. } => Self::Transport(e.to_string()),
            IMessageError::ChatNotFound => Self::Rejected(e.to_string()),
            IMessageError::Database(s) => Self::Transport(s),
            IMessageError::QueueFull(_) => Self::Rejected(e.to_string()),
        }
    }
}
