//! v0.6.5 — WASM plugin host for Wayland-Core.
//! Built on wasmtime + Component Model. See `.blackboard/v0.6.5-PLUGIN-SDK-PLAN.md`.

pub mod bindings; // Task 2.2 + 2.3 — WIT world bindings
pub mod error;
pub mod host_adapters; // Task 2.5
mod host_impl; // Wave 6B.1 — impl Host for HostState
pub mod limiter; // Task 2.4
pub mod runner; // Task 2.6 — composition root
pub mod runtime; // Task 2.4

pub use error::{Result as WasmResult, WasmPluginError};
pub use limiter::{
    DEFAULT_FUEL, DEFAULT_MEMORY_BYTES, DEFAULT_TIMEOUT_SECS, WasmPluginLimits, WasmResourceLimiter,
};
pub use runner::{
    AdapterKind, AdapterSelection, HostState, LoadedWasmPlugin, PluginToolCaps, ToolOutput,
    WasmPermissionsSnapshot, WasmPluginRunner, WasmToolMetadata,
};
pub use runtime::{EPOCH_TICK_INTERVAL, EpochTicker, build_engine};
