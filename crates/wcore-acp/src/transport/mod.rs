//! ACP transport implementations.
//!
//! - 1.A.3 — stdio (line-delimited JSON-RPC).
//! - 1.A.4 — HTTP/SSE (this module's [`http`]).
//! - 1.A.5 — WebSocket (this module's [`ws`]).
//!
//! Each transport carries [`crate::protocol::JsonRpcRequest`] from client
//! to server and [`crate::protocol::JsonRpcResponse`] + streamed
//! [`crate::protocol::MessageEvent`]s back, framed appropriately for the
//! wire.

pub mod http;
pub mod rest;
pub mod stdio;
pub mod ws;

pub use http::{HttpHandler, HttpSseTransport};
pub use rest::{RestExt, RestTransport};
pub use stdio::StdioTransport;
pub use ws::WsTransport;
