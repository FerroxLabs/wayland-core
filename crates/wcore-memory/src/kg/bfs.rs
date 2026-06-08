//! Bounded BFS over `kg_edges`. Edges are followed in BOTH directions
//! (undirected at the read layer), matching the IJFW v1.3.0 D-pillar
//! traverse semantics. Two caps:
//!   - `max_depth` — stop expanding once the next frontier would exceed.
//!   - `max_nodes` — stop emitting once the result hits the cap.

use std::collections::{HashSet, VecDeque};

use rusqlite::{Connection, params};

use crate::error::{MemoryError, Result};

/// BFS bounds. `max_depth = 0` returns just the start node.
#[derive(Debug, Clone, Copy)]
pub struct BfsLimit {
    pub max_depth: u32,
    pub max_nodes: usize,
}

impl BfsLimit {
    pub fn new(max_depth: u32, max_nodes: usize) -> Self {
        Self {
            max_depth,
            max_nodes,
        }
    }
}

/// Breadth-first walk from `start`, returning `(node_id, depth)` pairs.
/// The start node itself is included at depth 0.
///
/// Direction: kg_edges are read in BOTH directions for each frontier
/// node, so the traversal is undirected. Visited-set guarantees no node
/// is emitted twice. Caps:
///   - `limit.max_depth`: hops from start; depth-0 = start node only.
///   - `limit.max_nodes`: hard cap on emitted nodes (including start).
pub fn bfs_neighbors(conn: &Connection, start: i64, limit: BfsLimit) -> Result<Vec<(i64, u32)>> {
    let mut out: Vec<(i64, u32)> = Vec::new();
    if limit.max_nodes == 0 {
        return Ok(out);
    }

    let mut visited: HashSet<i64> = HashSet::new();
    let mut queue: VecDeque<(i64, u32)> = VecDeque::new();
    queue.push_back((start, 0));
    visited.insert(start);

    let mut stmt_out = conn
        .prepare("SELECT dst FROM kg_edges WHERE src = ?1")
        .map_err(MemoryError::Db)?;
    let mut stmt_in = conn
        .prepare("SELECT src FROM kg_edges WHERE dst = ?1")
        .map_err(MemoryError::Db)?;

    while let Some((node, depth)) = queue.pop_front() {
        out.push((node, depth));
        if out.len() >= limit.max_nodes {
            break;
        }
        if depth >= limit.max_depth {
            continue;
        }
        let next_depth = depth + 1;

        // Outgoing neighbours.
        let dsts = stmt_out
            .query_map(params![node], |r| r.get::<_, i64>(0))
            .map_err(MemoryError::Db)?;
        for d in dsts {
            let d = d.map_err(MemoryError::Db)?;
            if visited.insert(d) {
                queue.push_back((d, next_depth));
            }
        }

        // Incoming neighbours (undirected expansion).
        let srcs = stmt_in
            .query_map(params![node], |r| r.get::<_, i64>(0))
            .map_err(MemoryError::Db)?;
        for s in srcs {
            let s = s.map_err(MemoryError::Db)?;
            if visited.insert(s) {
                queue.push_back((s, next_depth));
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kg::edges::{EdgeKind, upsert_edge};
    use crate::kg::nodes::{NodeKind, upsert_node};
    use crate::kg::schema;

    /// Build a small star + chain:
    ///   a -mentions-> b
    ///   a -mentions-> c
    ///   c -mentions-> d
    ///   d -mentions-> e
    /// Returns (conn, [a,b,c,d,e]).
    fn build_graph() -> (Connection, Vec<i64>) {
        let conn = Connection::open_in_memory().unwrap();
        schema::init(&conn).unwrap();
        let ids: Vec<i64> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(|n| upsert_node(&conn, n, &NodeKind::Entity).unwrap())
            .collect();
        upsert_edge(&conn, ids[0], ids[1], &EdgeKind::Mentions, 1.0).unwrap();
        upsert_edge(&conn, ids[0], ids[2], &EdgeKind::Mentions, 1.0).unwrap();
        upsert_edge(&conn, ids[2], ids[3], &EdgeKind::Mentions, 1.0).unwrap();
        upsert_edge(&conn, ids[3], ids[4], &EdgeKind::Mentions, 1.0).unwrap();
        (conn, ids)
    }

    #[test]
    fn bfs_depth_one_returns_direct_neighbors() {
        let (conn, ids) = build_graph();
        let out = bfs_neighbors(&conn, ids[0], BfsLimit::new(1, 100)).unwrap();
        let visited: std::collections::HashSet<i64> = out.iter().map(|(n, _)| *n).collect();
        // a (depth 0) + b, c (depth 1). NOT d or e.
        assert!(visited.contains(&ids[0]));
        assert!(visited.contains(&ids[1]));
        assert!(visited.contains(&ids[2]));
        assert!(!visited.contains(&ids[3]));
        assert!(!visited.contains(&ids[4]));
    }

    #[test]
    fn bfs_respects_max_depth() {
        let (conn, ids) = build_graph();
        let out = bfs_neighbors(&conn, ids[0], BfsLimit::new(2, 100)).unwrap();
        let visited: std::collections::HashSet<i64> = out.iter().map(|(n, _)| *n).collect();
        // depth 0: a, depth 1: b,c, depth 2: d. e is at depth 3 — excluded.
        assert!(
            visited.contains(&ids[3]),
            "d should be reachable at depth 2"
        );
        assert!(
            !visited.contains(&ids[4]),
            "e must be excluded — it is at depth 3"
        );
        // Depths assigned correctly.
        let depth_for = |id: i64| out.iter().find(|(n, _)| *n == id).map(|(_, d)| *d);
        assert_eq!(depth_for(ids[0]), Some(0));
        assert_eq!(depth_for(ids[1]), Some(1));
        assert_eq!(depth_for(ids[2]), Some(1));
        assert_eq!(depth_for(ids[3]), Some(2));
    }

    #[test]
    fn bfs_respects_max_nodes() {
        let (conn, ids) = build_graph();
        let out = bfs_neighbors(&conn, ids[0], BfsLimit::new(10, 2)).unwrap();
        assert_eq!(out.len(), 2, "max_nodes cap must short-circuit emission");
        // First emitted is always the start node.
        assert_eq!(out[0].0, ids[0]);
    }
}
