//! W7 F2: first-class sub-agents.
//!
//! - `AgentRegistry` loads `AgentManifest`s from filesystem and the
//!   plugin surface (W2.5 `ScopedAgentRegistry`) and answers
//!   `get(&str)` lookups.
//! - `ChannelSink` is an `OutputSink` that forwards every event from
//!   a sub-agent's engine to the parent via an mpsc channel, tagged
//!   with the parent's `call_id`.
//! - `AgentBus` is a `tokio::broadcast` channel for cross-agent
//!   messages (W7 wires the channel; only `StatusUpdate` is emitted).

pub mod bus;
pub mod channel_sink;
pub mod observer;
pub mod registry;

pub use observer::AgentBusObserver;
