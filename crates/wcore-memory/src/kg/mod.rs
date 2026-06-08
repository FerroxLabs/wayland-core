//! Knowledge-graph substrate for semantic memory (T2-D1).
//!
//! # Status: WIRED (v0.6.3 — W2 + W5)
//!
//! Schema, BFS, node/edge upsert, and `kg_enabled()` all work. `init_kg`
//! runs in `AgentBootstrap::build()` on the production Memory instance
//! (gated by `kg_enabled()`), and the fact-extraction pipeline (W5) upserts
//! nodes/edges at session/turn end. The `WAYLAND_KG=off` rollback flag
//! disables KG init.
//!
//! Sibling to `propagation.rs` (session-lineage forest) — NO shared abstraction.
//! Tables `kg_nodes` and `kg_edges` are additive on the existing memory database.
//!
//! Concepts ported from IJFW v1.3.0 D-pillar (extract.js + traverse.js):
//!   - Nodes carry `(name, kind)` with UNIQUE composite key.
//!   - Edges are `(src, dst, kind, weight)` with PK on the triple.
//!   - BFS reads kg_edges in BOTH directions (undirected at the read layer)
//!     and caps depth + visited count.
//!
//! Rollback: set `WAYLAND_KG=off` (callers gate `kg::init` on this env var)
//! to skip KG creation and bfs operations.
//! Migration: `schema::init()` is idempotent — safe to run on existing
//! memory dbs. Not auto-called from `apply_migrations`; consumers opt in.

pub mod bfs;
pub mod edges;
pub mod inference;
pub mod nodes;
pub mod schema;

pub use bfs::{BfsLimit, bfs_neighbors};
pub use edges::{Edge, EdgeKind, edges_from, edges_to, upsert_edge};
pub use inference::{INFERRED_KIND, InferenceResult, infer_once};
pub use nodes::{Node, NodeKind, find_nodes_by_name, get_node, upsert_node};
pub use schema::init as init_kg;

/// Env var controlling KG behavior. Set to `"off"` to disable.
/// Anything else (including unset) keeps the KG enabled.
pub const ENV_KG: &str = "WAYLAND_KG";

/// Returns `true` unless `WAYLAND_KG` is set to (case-insensitive) `"off"`.
/// Mirrors the [`crate::staleness::staleness_enabled`] /
/// [`crate::auto_memorize::consent_granted`] opt-out pattern.
pub fn kg_enabled() -> bool {
    std::env::var(ENV_KG)
        .map(|v| v.to_lowercase() != "off")
        .unwrap_or(true)
}
