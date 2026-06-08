//! `wcore-channel-imessage` — macOS-only iMessage channel adapter.
//!
//! **macOS-only**: this entire crate is gated with `#[cfg(target_os = "macos")]`.
//! On non-macOS targets the crate builds as an empty shell; the registry
//! must NOT register the factory on those targets.
//!
//! Architecture:
//! - Inbound: polls `~/Library/Messages/chat.db` (SQLite, read-only) on a
//!   configurable interval (default 2 s). Tracks a rowid cursor so each
//!   message is processed exactly once.
//! - Outbound: AppleScript via `osascript -e <script>` using
//!   `tokio::process::Command`. All user-controlled values are quoted.
//! - Credentials: no token required; access is purely OS-level (Full Disk
//!   Access + Automation TCC consent for Messages.app).
//!
//! Ported from the desktop app's TypeScript `ImessagePlugin` (OpenClaw MIT,
//! adapted under Apache-2.0). See F-045 in the wcore audit triage.

#[cfg(target_os = "macos")]
mod applescript;
#[cfg(target_os = "macos")]
mod channel;
#[cfg(target_os = "macos")]
pub mod config;
#[cfg(target_os = "macos")]
mod db;
#[cfg(target_os = "macos")]
pub mod error;

#[cfg(target_os = "macos")]
pub use channel::IMessageChannel;
#[cfg(target_os = "macos")]
pub use config::IMessageConfig;
#[cfg(target_os = "macos")]
pub use error::IMessageError;
