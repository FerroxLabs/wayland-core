//! Capability-gated host adapters for WASM plugins.
//!
//! Every host capability has a fail-closed `Deny*` default + a `Gated*` impl
//! consulting [`PluginAccessGate`](wcore_plugin_api::access_gate::PluginAccessGate).
//! Composition root (Task 2.6 runner) selects which to link based on plugin
//! manifest permissions.
//!
//! Pattern lifted from `ironclaw_wasm::host` (Ironclaw reference
//! `crates/ironclaw_wasm/src/host.rs:39-496`): one trait per capability, ZST
//! `Deny*` default that returns a fail-closed error, real `Gated*` impl that
//! consults the host's permission gate before executing.
//!
//! For v0.6.5 the traits are defined LOCALLY in each module. Task 2.6 will
//! wire them to the wasmtime-generated host imports.

pub mod http;
pub mod log;
pub mod secrets;
pub mod tools;
pub mod workspace;
