//! Wayland-Browser — plugin shell for the wcore-browser tool family.
//!
//! REV-2 audit F2 invariant: this crate does NOT depend on `wcore-browser`.
//! It registers a `BrowserToolSpec` through the `wcore-plugin-api` mirror;
//! the host adapter (in `wcore-agent`, which DOES depend on `wcore-browser`)
//! translates the spec into a concrete `BrowserTool` via
//! `wcore_browser::adapter::from_spec`.
//!
//! Verify the isolation:
//!
//! ```bash
//! rg "wcore-browser|wcore_browser" crates/wayland-browser/
//! ```
//!
//! ...must return zero hits.

pub mod plugin;

pub use plugin::{MANIFEST_TOML, WaylandBrowser, WaylandBrowserFactory, default_browser_spec};
