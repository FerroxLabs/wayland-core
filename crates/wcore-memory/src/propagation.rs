// M5.7 — cross-session lineage graph.
//
// Tracks parent/child relationships between session IDs in a swarm so the
// orchestrator's `SwarmMemoryBridge` (in `wcore-swarm`) can:
//   1. Bootstrap a fresh worker with the parent's semantic-tier snapshot.
//   2. Merge worker outcomes back into the parent's procedural tier on join.
//   3. Refuse a read where the target is downstream of the reader (cycle /
//      direction guard).
//
// Stored in-memory in the orchestrator process — workers run as
// subprocesses; the bridge owns the lineage state. Cycle-safe:
// `record_parent` rejects edges that would create a cycle (or a
// self-edge). The graph is intentionally a *forest of parent pointers*
// (one parent per child) — that is sufficient for the v0.6 swarm model
// where every worker descends from a single orchestrator session and
// fan-out is one-deep most of the time. Multi-parent merges are a v0.7+
// concern (see milestone-5 §M5.7 out-of-scope).

use std::collections::HashMap;

use crate::error::{MemoryError, Result};

/// Forest of `child_id -> parent_id` edges across swarm sessions.
///
/// Cheap to clone (`HashMap<String, String>`) but the bridge wraps it in
/// `Arc<Mutex<...>>` so concurrent worker recordings serialise. Cycle
/// detection is O(depth) — the 10_000-hop guard inside `is_ancestor`
/// prevents pathological loops if some external state injection
/// bypasses `record_parent`.
#[derive(Debug, Default, Clone)]
pub struct MemoryLineage {
    parent_of: HashMap<String, String>,
}

impl MemoryLineage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `child`'s parent is `parent`. Returns
    /// `MemoryError::LineageCycle` if the edge would create a cycle —
    /// either a self-edge (`child == parent`) or one where `child` is
    /// already an ancestor of `parent`.
    pub fn record_parent(&mut self, child: &str, parent: &str) -> Result<()> {
        if child == parent {
            return Err(MemoryError::LineageCycle(format!("self-edge {child}")));
        }
        // Would adding (child -> parent) create a cycle? Yes iff `child`
        // is already an ancestor of `parent`.
        if self.is_ancestor(child, parent) {
            return Err(MemoryError::LineageCycle(format!(
                "{parent} -> {child} would form a cycle"
            )));
        }
        self.parent_of.insert(child.to_string(), parent.to_string());
        Ok(())
    }

    /// Is `maybe_ancestor` an ancestor of `node` in the parent chain?
    pub fn is_ancestor(&self, maybe_ancestor: &str, node: &str) -> bool {
        let mut cur = node;
        let mut hops = 0usize;
        while let Some(parent) = self.parent_of.get(cur) {
            if parent == maybe_ancestor {
                return true;
            }
            cur = parent.as_str();
            hops += 1;
            if hops > 10_000 {
                // Belt + suspenders: never spin forever even if a buggy
                // external injection bypassed `record_parent`.
                return false;
            }
        }
        false
    }

    /// Direct-parent accessor.
    pub fn parent_of(&self, child: &str) -> Option<&str> {
        self.parent_of.get(child).map(|s| s.as_str())
    }

    /// Number of recorded edges (sessions with a known parent).
    pub fn len(&self) -> usize {
        self.parent_of.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parent_of.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_lineage_is_empty() {
        let lin = MemoryLineage::new();
        assert!(lin.is_empty());
        assert_eq!(lin.len(), 0);
        assert!(!lin.is_ancestor("a", "b"));
        assert_eq!(lin.parent_of("a"), None);
    }

    #[test]
    fn linear_chain_is_acyclic() {
        let mut lin = MemoryLineage::new();
        lin.record_parent("a", "root").unwrap();
        lin.record_parent("b", "a").unwrap();
        lin.record_parent("c", "b").unwrap();
        assert!(lin.is_ancestor("a", "c"));
        assert!(lin.is_ancestor("root", "c"));
        assert!(!lin.is_ancestor("c", "a"));
        assert_eq!(lin.parent_of("b"), Some("a"));
        assert_eq!(lin.len(), 3);
    }

    #[test]
    fn cycle_returns_typed_error() {
        let mut lin = MemoryLineage::new();
        lin.record_parent("a", "root").unwrap();
        lin.record_parent("b", "a").unwrap();
        // a's parent = b would create a -> b -> a -> b cycle.
        let err = lin.record_parent("a", "b").unwrap_err();
        let s = err.to_string().to_lowercase();
        assert!(s.contains("cycle"), "got: {s}");
        assert!(matches!(err, MemoryError::LineageCycle(_)));
    }

    #[test]
    fn self_edge_rejected() {
        let mut lin = MemoryLineage::new();
        let err = lin.record_parent("a", "a").unwrap_err();
        assert!(matches!(err, MemoryError::LineageCycle(_)));
        assert!(err.to_string().to_lowercase().contains("self-edge"));
    }

    #[test]
    fn rewriting_parent_is_allowed_when_no_cycle() {
        // a -> root, then rewrite a -> b: fine because b is not a
        // descendant of a.
        let mut lin = MemoryLineage::new();
        lin.record_parent("a", "root").unwrap();
        lin.record_parent("b", "root").unwrap();
        lin.record_parent("a", "b").unwrap();
        assert_eq!(lin.parent_of("a"), Some("b"));
        assert!(lin.is_ancestor("root", "a"));
    }
}
