//! Observability primitives for wayland-core: trace schema, span sinks,
//! prompt-cache discipline. Sits between `wcore-types`/`wcore-config` and
//! `wcore-agent`. Owns nothing the protocol crate needs (events.rs ships
//! `TraceEvent` carrying an opaque `serde_json::Value`).

pub mod cache;
pub mod cost;
pub mod env_gate;
pub mod sink;
pub mod trace;

/// S5: every emitted trace and every memory write tags itself with this
/// constant so a future IJFW absorb step can attribute records back to
/// the engine that produced them.
pub const SOURCE_PRODUCT: &str = "wayland-core";
