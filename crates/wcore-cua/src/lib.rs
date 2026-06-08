//! `wcore-cua` — multi-platform computer-use (CUA) tool family.
//!
//! Design contract: `docs/superpowers/specs/2026-05-14-wcore-super-agent-design.md`
//! §5.18. Background-mode invariant on every platform: synthesized input
//! and screenshot capture MUST NOT steal focus from the user's foreground
//! window. Each backend ships a `focus_invariance_test` to lock that
//! contract in.
//!
//! Crate-graph position (mid-tier): depends DOWNWARD on `wcore-types`,
//! `wcore-tools`, `wcore-protocol`, `wcore-config`. MUST NOT depend on
//! `wcore-agent`, `wcore-cli`, `wcore-evolve`, `wcore-eval`, or
//! `wcore-browser`. Plugin shells (`wayland-cua`) MUST NOT depend on this
//! crate directly; they flow through
//! `wcore-plugin-api::cua_spec::CuaToolSpec` mirrors (audit F2).
//!
//! Four platform backends:
//! - **macOS** (`backends::macos`): `CGEvent` + `CGDisplayCreateImage`.
//!   Gated `#[cfg(target_os = "macos")]`. Background-clean.
//! - **Linux X11** (`backends::linux_x11`): `xdotool` (sendevent mode) +
//!   `scrot` + AT-SPI. Gated `#[cfg(target_os = "linux")]`.
//! - **Linux Wayland** (`backends::linux_wayland`): `wlrctl` + `grim` +
//!   AT-SPI. Refuses to register on restricted compositors (audit F7
//!   positive invariance).
//! - **Windows** (`backends::windows`): UI Automation + GDI + `SendInput`.
//!   Gated `#[cfg(target_os = "windows")]`. Background-clean on Win10+.

pub mod adapter;
pub mod backend;
pub mod backends;
pub mod error;
pub mod op;
pub mod policy;
pub mod redact;
pub mod tool;

pub use adapter::{CuaToolSpecLocal, from_spec};
pub use backend::{
    AxNode, AxTree, ComputerUseBackend, CuaSession, KeyMods, MouseButton, Platform, Region,
    ScreenshotFormat,
};
pub use error::{CuaError, CuaResult};
pub use op::{CUA_OP_LOCKED_VARIANT_COUNT, CuaOp, CuaOpResult};
pub use policy::{CuaPolicy, CuaPolicyOutcome};
pub use tool::CuaTool;
