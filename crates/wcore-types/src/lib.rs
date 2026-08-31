// Pure, provider-neutral data types shared across all wayland-core crates.
// No dependencies on other wcore-* crates.

pub mod cache_tier;
pub mod child_transaction;
pub mod compact;
pub mod crucible;
pub mod execution_policy;
pub mod failure;
pub mod file_state;
pub mod goal;
// Windows kill-on-close Job Object — the only kernel-backed way to own a
// process TREE on Windows. Lives beside `process_liveness` and for the same
// AGENTS.md reason: `wcore-sandbox` and `wcore-mcp` both need it, and this is
// the only crate both already depend on. Whole module is `cfg(windows)`.
pub mod job_object;
pub mod llm;
pub mod message;
pub mod model_aliases;
// Cross-platform process-liveness probe. Lives here, not in a probe-local
// helper, because a liveness check that reads a zombie as a live process is
// how thirteen containment tests came to certify a corpse as a survivor —
// and because AGENTS.md requires platform differences to sit in exactly one
// function rather than being re-derived per crate.
pub mod process_liveness;
pub mod reasoning_filter;
pub mod skill_types;
pub mod spawner;
pub mod tool;
pub mod url_authority;
pub mod utf8_stream;
pub mod workspace_trust;

pub use cache_tier::{CacheTier, CacheTierConfig, pick_cache_tier, pick_with_config};
