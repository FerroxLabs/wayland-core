//! v0.6.5 — Wasmtime component bindings for Wayland's WIT worlds.
//!
//! Split per-world so Task 2.2 (tool) and Task 2.3 (hook) can land
//! independently without git conflicts.
//! - `tool` — Task 2.2 export world
//! - `hook` — Task 2.3 export world
pub mod hook;
pub mod tool;
