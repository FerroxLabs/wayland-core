//! T2-D1 integration round-trip — build a small graph through the public
//! `kg::*` API and verify BFS reachability + edge accessors.

use rusqlite::Connection;
use wcore_memory::kg::{
    BfsLimit, EdgeKind, NodeKind, bfs_neighbors, edges_from, edges_to, init_kg, upsert_edge,
    upsert_node,
};

#[test]
fn kg_full_round_trip() {
    let conn = Connection::open_in_memory().unwrap();
    init_kg(&conn).unwrap();

    // Build a 4-node graph through the public API:
    //   root -uses-> child_a -relates_to-> grandchild
    //   root -mentions-> child_b
    let root = upsert_node(&conn, "root", &NodeKind::Entity).unwrap();
    let child_a = upsert_node(&conn, "child_a", &NodeKind::Concept).unwrap();
    let child_b = upsert_node(&conn, "child_b", &NodeKind::Tool).unwrap();
    let grandchild = upsert_node(&conn, "grandchild", &NodeKind::Other("custom".into())).unwrap();

    upsert_edge(&conn, root, child_a, &EdgeKind::Uses, 0.8).unwrap();
    upsert_edge(&conn, root, child_b, &EdgeKind::Mentions, 0.5).unwrap();
    upsert_edge(&conn, child_a, grandchild, &EdgeKind::RelatesTo, 0.6).unwrap();

    // Outgoing from root: child_a + child_b.
    let out = edges_from(&conn, root).unwrap();
    assert_eq!(out.len(), 2, "root has two outgoing edges");

    // Incoming to grandchild: only the relates_to from child_a.
    let inc = edges_to(&conn, grandchild).unwrap();
    assert_eq!(inc.len(), 1);
    assert_eq!(inc[0].kind, EdgeKind::RelatesTo);
    assert_eq!(inc[0].src, child_a);

    // BFS depth 2 from root must reach all four nodes (root, child_a,
    // child_b at depth 1, grandchild at depth 2).
    let visited = bfs_neighbors(&conn, root, BfsLimit::new(2, 100)).unwrap();
    let ids: std::collections::HashSet<i64> = visited.iter().map(|(n, _)| *n).collect();
    assert!(ids.contains(&root));
    assert!(ids.contains(&child_a));
    assert!(ids.contains(&child_b));
    assert!(
        ids.contains(&grandchild),
        "grandchild must be reachable at depth 2"
    );
    assert_eq!(visited.len(), 4, "exactly four distinct nodes reached");

    // BFS depth 1 must EXCLUDE grandchild.
    let visited_d1 = bfs_neighbors(&conn, root, BfsLimit::new(1, 100)).unwrap();
    let ids_d1: std::collections::HashSet<i64> = visited_d1.iter().map(|(n, _)| *n).collect();
    assert!(!ids_d1.contains(&grandchild), "depth 1 excludes grandchild");

    // Reverse traversal: starting at grandchild reaches root via
    // undirected expansion within depth 2.
    let from_gc = bfs_neighbors(&conn, grandchild, BfsLimit::new(2, 100)).unwrap();
    let ids_gc: std::collections::HashSet<i64> = from_gc.iter().map(|(n, _)| *n).collect();
    assert!(
        ids_gc.contains(&root),
        "undirected BFS from grandchild must reach root"
    );
}
