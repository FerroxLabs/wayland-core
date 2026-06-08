//! T2-E1: MCP server transports.
//!
//! Two transports for the `McpServer` defined in `crate::server`:
//!
//! - [`stdio`] — newline-delimited JSON-RPC over stdin/stdout. Standard
//!   MCP transport used by IDE integrations spawning the server as a
//!   child process.
//! - [`sse`] — local HTTP listener emitting `text/event-stream`
//!   responses to JSON-RPC POSTs. Used by browser-based clients and
//!   for in-process / cross-process scenarios where a TCP port is
//!   easier than a pipe.

pub mod sse;
pub mod stdio;

pub use sse::{SseConfig, serve_sse};
pub use stdio::{serve_stdio, serve_stdio_with};
