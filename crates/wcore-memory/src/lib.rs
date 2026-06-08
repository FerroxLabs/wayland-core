// Long-term memory system for wayland-core.
//
// v1 surface (YAML flat-file store) is in `index`, `prompt`, `store`, `types`.
// v2 surface (5-partition × 3-tier cognitive memory, SQLite-backed) lives in
// `v2_*` and the v2 submodules below. v1 is removed in Group G of the W5
// rollout, at which point the v2 modules are promoted to crate root exports.

pub mod cross_project;
pub mod error;
pub mod fact_extractor;
pub mod index;
pub mod paths;
pub mod prompt;
pub mod store;
pub mod types;

// ----- v2 surface (W5) -----
pub mod api;
pub mod audit;
pub mod auto_memorize;
pub mod cdc;
pub mod compact;
pub mod consolidate;
pub mod contradiction;
pub mod db;
pub mod embed;
pub mod gate;
pub mod kg;
pub mod legacy_import;
pub mod memory;
pub mod null;
pub mod partition;
pub mod propagation;
pub mod retrieve;
pub mod schema;
pub mod staleness;
pub mod tier;
pub mod v2_prompt;
pub mod v2_types;

pub use api::MemoryApi;
pub use contradiction::{
    ContradictionCandidate, ContradictionResolution, ContradictionResolver, ResolutionResult,
};
pub use memory::Memory;
pub use null::NullMemory;
pub use propagation::MemoryLineage;
pub use v2_types::AccessToken;

/// Test-only constructor: returns a fully-wired in-memory `Memory`
/// (no on-disk DBs, no audit log files). Implements `MemoryApi` so it
/// can be wrapped in `Arc::new(...)` and handed to W9 fixtures.
///
/// Gated behind the `test-utils` Cargo feature so it never ships in
/// release binaries. The `_path` argument is accepted for API symmetry
/// with `Memory::open` (fixtures can pass `tempfile::tempdir().path()`);
/// the underlying store is in-memory regardless.
#[cfg(any(test, feature = "test-utils"))]
pub async fn open_for_test(_path: &std::path::Path) -> error::Result<memory::Memory> {
    memory::Memory::open_in_memory().await
}
