//! Deductive inference over the knowledge graph (v0.6.4 Task 6.1).
//!
//! One-pass transitive inference: given edges `(a, b)` and `(b, c)`, materialize
//! `(a, c, "inferred_transitive", w_ab * w_bc)` provided
//!   - `a != c`
//!   - `w_ab * w_bc >= CONFIDENCE_FLOOR` (0.30)
//!   - no existing `(a, c, *)` edge already has weight >= the new product
//!
//! Edges already marked `inferred_transitive` are excluded from the SOURCE set,
//! so a single `infer_once` call cannot chain inferred edges into deeper hops.
//! Matches Forge `DeductiveInference.ts:82-178`.
//!
//! Edges are written through [`crate::kg::upsert_edge`] using
//! `EdgeKind::Other("inferred_transitive")` so no schema migration is needed
//! and round-tripping through `EdgeKind::from_db_str` survives.

use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::{MemoryError, Result};
use crate::kg::edges::{EdgeKind, upsert_edge};

/// Minimum product weight required to materialize an inferred edge.
pub const CONFIDENCE_FLOOR: f32 = 0.30;

/// Kind tag used for inferred edges. Stored in `kg_edges.kind`.
pub const INFERRED_KIND: &str = "inferred_transitive";

/// Outcome of a single `infer_once` pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InferenceResult {
    pub edges_created: u64,
    pub edges_skipped: u64,
}

/// Run one pass of transitive inference over the KG. See module docs for
/// the algorithm and Forge alignment notes.
///
/// v0.6.4 Task 6.6c follow-up: the original ZST `DeductiveInference` shim
/// was dropped after audit confirmed no external callers (only the
/// dream-cycle wiring in `consolidate.rs::infer_kg`). Re-introduce a
/// stateful handle if/when configurable tunables (e.g. per-call floor)
/// are needed.
pub fn infer_once(conn: &Connection) -> Result<InferenceResult> {
    // Load every edge once. The KG is small (per-tenant) so a full scan is
    // cheap and lets us compute `existing_max_w` in one pass.
    struct Row {
        src: i64,
        dst: i64,
        kind: String,
        weight: f32,
    }
    let mut stmt = conn
        .prepare("SELECT src, dst, kind, weight FROM kg_edges")
        .map_err(MemoryError::Db)?;
    let rows: Vec<Row> = stmt
        .query_map([], |r| {
            Ok(Row {
                src: r.get(0)?,
                dst: r.get(1)?,
                kind: r.get(2)?,
                weight: r.get::<_, f64>(3)? as f32,
            })
        })
        .map_err(MemoryError::Db)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(MemoryError::Db)?;

    // Source edges = anything NOT already inferred. These are the only edges
    // we use as legs `a->b` or `b->c`. Excluding inferred edges prevents
    // multi-hop chaining in one pass (matches Forge).
    let src_edges: Vec<&Row> = rows.iter().filter(|r| r.kind != INFERRED_KIND).collect();

    // Group source edges by their src for O(1) "what flows out of b?" lookup.
    let mut outgoing_by_src: HashMap<i64, Vec<&Row>> = HashMap::new();
    for e in &src_edges {
        outgoing_by_src.entry(e.src).or_default().push(e);
    }

    // existing_max_w covers ALL edges (including prior inferred) so we don't
    // re-emit edges weaker than what is already on the graph.
    let mut existing_max_w: HashMap<(i64, i64), f32> = HashMap::new();
    for r in &rows {
        let entry = existing_max_w.entry((r.src, r.dst)).or_insert(f32::MIN);
        if r.weight > *entry {
            *entry = r.weight;
        }
    }

    // Bridges = every node that is the destination of at least one source
    // edge. Iterating bridges (rather than all nodes) keeps the inner loop
    // tight and matches the Forge structure.
    let mut bridges: Vec<i64> = src_edges.iter().map(|e| e.dst).collect();
    bridges.sort_unstable();
    bridges.dedup();

    let mut created: u64 = 0;
    let mut skipped: u64 = 0;

    for b in bridges {
        let ab_legs: Vec<&&Row> = src_edges.iter().filter(|e| e.dst == b).collect();
        let bc_legs = match outgoing_by_src.get(&b) {
            Some(v) => v,
            None => continue,
        };
        for ab in &ab_legs {
            for bc in bc_legs {
                let a = ab.src;
                let c = bc.dst;
                if a == c {
                    skipped += 1;
                    continue;
                }
                let w = ab.weight * bc.weight;
                if w < CONFIDENCE_FLOOR {
                    skipped += 1;
                    continue;
                }
                let prior = existing_max_w.get(&(a, c)).copied().unwrap_or(f32::MIN);
                if prior >= w {
                    skipped += 1;
                    continue;
                }
                upsert_edge(conn, a, c, &EdgeKind::Other(INFERRED_KIND.to_string()), w)?;
                // Prevent same-pass dupes if multiple bridges produce the same
                // (a,c) pair — only the highest-weight materialization wins.
                existing_max_w.insert((a, c), w);
                created += 1;
            }
        }
    }

    Ok(InferenceResult {
        edges_created: created,
        edges_skipped: skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kg::edges::{EdgeKind, edges_from, upsert_edge};
    use crate::kg::nodes::{NodeKind, upsert_node};
    use crate::kg::schema;
    use std::collections::HashMap;

    /// Build a fresh kg with named nodes; returns (conn, name->id map).
    fn fresh_with_nodes(names: &[&str]) -> (Connection, HashMap<String, i64>) {
        let conn = Connection::open_in_memory().unwrap();
        schema::init(&conn).unwrap();
        let mut ids = HashMap::new();
        for n in names {
            let id = upsert_node(&conn, n, &NodeKind::Entity).unwrap();
            ids.insert((*n).to_string(), id);
        }
        (conn, ids)
    }

    fn get_edge_weight(conn: &Connection, src: i64, dst: i64, kind_str: &str) -> Option<f32> {
        edges_from(conn, src)
            .unwrap()
            .into_iter()
            .find(|e| e.dst == dst && e.kind.as_str() == kind_str)
            .map(|e| e.weight)
    }

    #[test]
    fn transitive_basic() {
        // [(A,B,mentions,0.9), (B,C,mentions,0.8)] -> create (A,C,0.72)
        let (conn, ids) = fresh_with_nodes(&["A", "B", "C"]);
        upsert_edge(&conn, ids["A"], ids["B"], &EdgeKind::Mentions, 0.9).unwrap();
        upsert_edge(&conn, ids["B"], ids["C"], &EdgeKind::Mentions, 0.8).unwrap();

        let out = infer_once(&conn).unwrap();
        assert_eq!(out.edges_created, 1, "expected one inferred edge");
        assert_eq!(out.edges_skipped, 0);

        let w = get_edge_weight(&conn, ids["A"], ids["C"], INFERRED_KIND)
            .expect("A->C inferred edge must exist");
        assert!(
            (w - 0.72).abs() < 1e-5,
            "weight should be 0.9*0.8=0.72, got {w}"
        );
    }

    #[test]
    fn confidence_floor_skip() {
        // 0.5 * 0.4 = 0.20 < 0.30 floor -> skip
        let (conn, ids) = fresh_with_nodes(&["A", "B", "C"]);
        upsert_edge(&conn, ids["A"], ids["B"], &EdgeKind::Mentions, 0.5).unwrap();
        upsert_edge(&conn, ids["B"], ids["C"], &EdgeKind::Mentions, 0.4).unwrap();

        let out = infer_once(&conn).unwrap();
        assert_eq!(out.edges_created, 0);
        assert_eq!(out.edges_skipped, 1);
        assert!(get_edge_weight(&conn, ids["A"], ids["C"], INFERRED_KIND).is_none());
    }

    #[test]
    fn self_loop_skip() {
        // (A,B,0.9) + (B,A,0.9) -> both bridges produce A==C, both skipped.
        let (conn, ids) = fresh_with_nodes(&["A", "B"]);
        upsert_edge(&conn, ids["A"], ids["B"], &EdgeKind::Uses, 0.9).unwrap();
        upsert_edge(&conn, ids["B"], ids["A"], &EdgeKind::Uses, 0.9).unwrap();

        let out = infer_once(&conn).unwrap();
        assert_eq!(out.edges_created, 0);
        assert_eq!(out.edges_skipped, 2);
    }

    #[test]
    fn existing_higher_skip() {
        // Existing A->C at 0.9 dominates the would-be inferred 0.64.
        let (conn, ids) = fresh_with_nodes(&["A", "B", "C"]);
        upsert_edge(&conn, ids["A"], ids["B"], &EdgeKind::Mentions, 0.8).unwrap();
        upsert_edge(&conn, ids["B"], ids["C"], &EdgeKind::Mentions, 0.8).unwrap();
        upsert_edge(&conn, ids["A"], ids["C"], &EdgeKind::Mentions, 0.9).unwrap();

        let out = infer_once(&conn).unwrap();
        assert_eq!(out.edges_created, 0);
        assert_eq!(out.edges_skipped, 1);
        // No new inferred edge — the original A->C "mentions" survives.
        assert!(get_edge_weight(&conn, ids["A"], ids["C"], INFERRED_KIND).is_none());
        let mentions =
            get_edge_weight(&conn, ids["A"], ids["C"], "mentions").expect("mentions stays");
        assert!((mentions - 0.9).abs() < 1e-6);
    }

    #[test]
    fn chain_three() {
        // (A,B,1.0) (B,C,1.0) (C,D,1.0)
        // bridges = {B, C}: B yields A->C (1.0); C yields B->D (1.0).
        // A->D requires the inferred A->C as a leg, which is excluded — matches Forge.
        let (conn, ids) = fresh_with_nodes(&["A", "B", "C", "D"]);
        upsert_edge(&conn, ids["A"], ids["B"], &EdgeKind::Uses, 1.0).unwrap();
        upsert_edge(&conn, ids["B"], ids["C"], &EdgeKind::Uses, 1.0).unwrap();
        upsert_edge(&conn, ids["C"], ids["D"], &EdgeKind::Uses, 1.0).unwrap();

        let out = infer_once(&conn).unwrap();
        assert_eq!(out.edges_created, 2, "A->C and B->D, but NOT A->D");
        assert_eq!(out.edges_skipped, 0);

        let ac = get_edge_weight(&conn, ids["A"], ids["C"], INFERRED_KIND)
            .expect("A->C inferred edge must exist");
        assert!((ac - 1.0).abs() < 1e-6);
        let bd = get_edge_weight(&conn, ids["B"], ids["D"], INFERRED_KIND)
            .expect("B->D inferred edge must exist");
        assert!((bd - 1.0).abs() < 1e-6);
        assert!(
            get_edge_weight(&conn, ids["A"], ids["D"], INFERRED_KIND).is_none(),
            "A->D must NOT be inferred in a single pass"
        );
    }

    #[test]
    fn empty_graph_is_a_noop() {
        let conn = Connection::open_in_memory().unwrap();
        schema::init(&conn).unwrap();
        let out = infer_once(&conn).unwrap();
        assert_eq!(out, InferenceResult::default());
    }
}
