//! v0.6.5 — Subprocess plugin host (JSON-Lines over stdio).
//!
//! See `.blackboard/v0.6.5-PLUGIN-SDK-PLAN.md` §3.2 and `runner.rs` for the
//! lifecycle + security contract.

pub mod error;
pub mod mcp_bridge;
pub mod rpc;
pub mod runner;

pub use error::{Result, SubprocessPluginError};
pub use mcp_bridge::{LoadedMcpBridgePlugin, McpBridgePluginRunner, ToolOutput as McpToolOutput};
pub use runner::{LoadedSubprocessPlugin, SubprocessPluginRunner, ToolOutput};
