//! Scoped registries — the only surface plugins use to register capabilities.

pub mod agents;
// W8c.1 E.13: ScopedBrowserRegistry for the wayland-browser plugin shell.
pub mod browser;
pub mod config;
// W8c.2 F.8: ScopedCuaRegistry for the wayland-cua plugin shell.
pub mod cua;
pub mod hooks;
pub mod logger;
pub mod mcp;
pub mod memory;
pub mod providers;
pub mod rules;
pub mod skills;
pub mod tools;
// v0.6.4 Task 2.1: ScopedUserModelRegistry for plugin-supplied
// user-model backends (Honcho et al.).
pub mod user_models;
