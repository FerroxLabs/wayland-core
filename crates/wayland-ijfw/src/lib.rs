//! Wayland-IJFW — anchor plugin that exercises every `register_*` surface
//! on [`PluginContext`].
//!
//! This is the validating consumer for the W2.5 plugin-api contract: a
//! single plugin that registers tools, hooks, agents, skills, rules, and
//! an MCP server through the [`wcore-plugin-api`] mirror types.
//!
//! REV-2 audit F2 invariant: this crate does NOT depend on any internal
//! `wcore-*` crate beyond `wcore-plugin-api`, `wcore-types`, and
//! `wcore-protocol`. Verify the isolation:
//!
//! ```bash
//! rg "wcore-(agent|tools|mcp|skills|memory|config|providers|compact|browser|cua)" \
//!     crates/wayland-ijfw/
//! ```
//!
//! ...must return zero hits.
//!
//! All payloads (skill bodies, agent YAML, rule prose, MCP server spec)
//! reference content committed at `snapshots/ijfw-source/` (a frozen
//! copy of the IJFW project tree at the time of the snapshot). The
//! snapshot is the canonical CI-reproducible source — no symlinks,
//! no external paths.

pub mod agents;
pub mod hooks;
pub mod mcp;
pub mod plugin;
pub mod rules;
pub mod skills;
pub mod tools;

pub use plugin::{MANIFEST_TOML, WaylandIjfw, WaylandIjfwFactory};
