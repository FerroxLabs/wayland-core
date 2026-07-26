//! The persistent Wayland-Core gateway runtime.
//!
//! Phase 24 plan 24-01. One runtime an operator can install, start,
//! inspect, drain, restart, upgrade and roll back — and across every one of
//! those events, no delivery is lost and none is duplicated.
//!
//! Layer: MID. It may depend on `wcore-types`, `wcore-config`,
//! `wcore-protocol`, `wcore-cron` and `wcore-channels`. It must NEVER
//! depend on `wcore-agent`, which is a top-layer crate — that would invert
//! the dependency graph AGENTS.md describes. The operator verb surface
//! lives in `crates/wcore-cli/src/gateway.rs` and drives this crate.

pub mod drain;
pub mod ledger;
pub mod lifecycle;
pub mod pidlock;
pub mod service;

use std::path::PathBuf;

/// Resolve the gateway home the way the rest of the product resolves it:
/// `$WAYLAND_HOME`, else `~/.wayland`.
///
/// Absorbed from `crates/wcore-cli/src/cron.rs` so the workspace has ONE
/// home-resolution story rather than two that can both claim a directory.
pub fn resolve_home() -> Option<PathBuf> {
    std::env::var_os("WAYLAND_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".wayland")))
        .map(pidlock::normalise_path)
}
