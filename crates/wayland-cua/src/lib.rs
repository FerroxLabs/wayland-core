//! Wayland-Cua — plugin shell for the wcore-cua tool family.
//!
//! REV-2 audit F2 invariant: this crate does NOT depend on `wcore-cua`.
//! It registers a `CuaToolSpec` through the `wcore-plugin-api` mirror;
//! the host adapter (in `wcore-cua` itself, since `wcore-agent` doesn't
//! yet carry plugin → tool wiring) translates the spec into a concrete
//! `CuaTool` via `wcore_cua::adapter::from_spec`.
//!
//! Verify the isolation:
//!
//! ```bash
//! rg "wcore-cua|wcore_cua" crates/wayland-cua/
//! ```
//!
//! ...must return zero hits.
//!
//! REV-2 audit F7: on Linux Wayland with a restricted compositor, the
//! host adapter refuses to register the tool (positive invariance).
//! The plugin layer surfaces that as a typed registration failure —
//! `CuaError::WaylandRestricted` — rather than silently falling back.

pub mod plugin;

pub use plugin::{MANIFEST_TOML, WaylandCua, WaylandCuaFactory, default_cua_spec};
