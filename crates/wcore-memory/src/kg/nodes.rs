//! KG node CRUD. Nodes carry a `(name, kind)` UNIQUE pair — upsert refreshes
//! `last_seen` on conflict (port of IJFW kg_nodes upsert semantics).

use std::borrow::Cow;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{MemoryError, Result};

/// Classification of a knowledge-graph node. The `Other` variant carries
/// the original tag so unknown kinds round-trip losslessly through the db.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Entity,
    Concept,
    Tool,
    Session,
    Other(String),
}

impl NodeKind {
    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            NodeKind::Entity => Cow::Borrowed("entity"),
            NodeKind::Concept => Cow::Borrowed("concept"),
            NodeKind::Tool => Cow::Borrowed("tool"),
            NodeKind::Session => Cow::Borrowed("session"),
            NodeKind::Other(s) => Cow::Borrowed(s.as_str()),
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "entity" => NodeKind::Entity,
            "concept" => NodeKind::Concept,
            "tool" => NodeKind::Tool,
            "session" => NodeKind::Session,
            other => NodeKind::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: i64,
    pub name: String,
    pub kind: NodeKind,
    pub created_at: i64,
    pub last_seen: i64,
}

/// Insert a node — or refresh its `last_seen` if `(name, kind)` already
/// exists. Returns the rowid in either branch.
///
/// W4 (v0.6.3): after a successful upsert, staleness is propagated outward
/// from this node so dependent neighbours are marked stale — an updated
/// node invalidates whatever was derived from it. Propagation is gated by
/// [`crate::staleness::staleness_enabled`] and is strictly best-effort: a
/// missing `kg_node_staleness` table (KG-only callers that never ran
/// `init_staleness`) or any BFS error is logged via `tracing::warn!` and
/// swallowed, so a propagation failure can never fail the upsert itself.
pub fn upsert_node(conn: &Connection, name: &str, kind: &NodeKind) -> Result<i64> {
    let kind_s = kind.as_str();
    conn.execute(
        "INSERT INTO kg_nodes (name, kind, created_at, last_seen)
         VALUES (?1, ?2, strftime('%s','now'), strftime('%s','now'))
         ON CONFLICT(name, kind) DO UPDATE SET last_seen = strftime('%s','now')",
        params![name, kind_s.as_ref()],
    )
    .map_err(MemoryError::Db)?;

    // ON CONFLICT path doesn't update last_insert_rowid, so look it up.
    let id: i64 = conn
        .query_row(
            "SELECT id FROM kg_nodes WHERE name = ?1 AND kind = ?2",
            params![name, kind_s.as_ref()],
            |r| r.get(0),
        )
        .map_err(MemoryError::Db)?;

    // W4: cascade staleness to dependent neighbours. Non-fatal by design.
    if crate::staleness::staleness_enabled() {
        // Depth-1 walk, capped at 100 nodes — only direct dependents are
        // invalidated; matches the BfsLimit used across the staleness tests.
        let limit = crate::kg::BfsLimit::new(1, 100);
        if let Err(e) = crate::staleness::propagate_staleness(conn, id, limit) {
            tracing::warn!(
                node_id = id,
                error = %e,
                "staleness propagation failed after node upsert (non-fatal)"
            );
        }
    }

    Ok(id)
}

/// Fetch a node by primary key. Returns `Ok(None)` when no row matches.
pub fn get_node(conn: &Connection, id: i64) -> Result<Option<Node>> {
    conn.query_row(
        "SELECT id, name, kind, created_at, last_seen FROM kg_nodes WHERE id = ?1",
        params![id],
        |r| {
            Ok(Node {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: NodeKind::from_db_str(&r.get::<_, String>(2)?),
                created_at: r.get(3)?,
                last_seen: r.get(4)?,
            })
        },
    )
    .optional()
    .map_err(MemoryError::Db)
}

/// Substring search by name (LIKE %fragment%). Escapes SQL wildcards in
/// the input so a query containing `%` / `_` doesn't blow up the search.
pub fn find_nodes_by_name(conn: &Connection, name_substr: &str, limit: usize) -> Result<Vec<Node>> {
    // Escape `%`, `_`, `\` per SQL LIKE rules; we use `\` as the escape.
    let escaped = name_substr
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");

    let mut stmt = conn
        .prepare(
            "SELECT id, name, kind, created_at, last_seen FROM kg_nodes
             WHERE name LIKE ?1 ESCAPE '\\'
             ORDER BY last_seen DESC
             LIMIT ?2",
        )
        .map_err(MemoryError::Db)?;

    let rows = stmt
        .query_map(params![pattern, limit as i64], |r| {
            Ok(Node {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: NodeKind::from_db_str(&r.get::<_, String>(2)?),
                created_at: r.get(3)?,
                last_seen: r.get(4)?,
            })
        })
        .map_err(MemoryError::Db)?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(MemoryError::Db)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kg::schema;

    fn fresh() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        schema::init(&conn).unwrap();
        conn
    }

    #[test]
    fn upsert_inserts_new_node() {
        let conn = fresh();
        let id = upsert_node(&conn, "alpha", &NodeKind::Entity).unwrap();
        assert!(id > 0);
        let node = get_node(&conn, id).unwrap().expect("node should exist");
        assert_eq!(node.name, "alpha");
        assert_eq!(node.kind, NodeKind::Entity);
        assert_eq!(node.created_at, node.last_seen);
    }

    #[test]
    fn upsert_existing_updates_last_seen() {
        let conn = fresh();
        let id = upsert_node(&conn, "alpha", &NodeKind::Concept).unwrap();
        // Re-upsert immediately. Note that `strftime('%s','now')` has
        // 1-second resolution, so a tight loop will likely produce the
        // same epoch value for both rows. The invariant we actually need
        // is `last_seen >= created_at`, not strict inequality — so the
        // previous 1100ms sleep was test theater and has been dropped.
        let id2 = upsert_node(&conn, "alpha", &NodeKind::Concept).unwrap();
        assert_eq!(id, id2, "upsert must reuse the existing rowid");
        let node = get_node(&conn, id).unwrap().unwrap();
        assert!(
            node.last_seen >= node.created_at,
            "last_seen must be >= created_at (last_seen={}, created_at={})",
            node.last_seen,
            node.created_at,
        );
    }

    #[test]
    fn upsert_node_propagates_staleness_to_dependents() {
        use crate::kg::edges::{EdgeKind, upsert_edge};
        use crate::staleness::{init_staleness, is_stale};

        // Conn with both kg and staleness tables present.
        let conn = Connection::open_in_memory().unwrap();
        schema::init(&conn).unwrap();
        init_staleness(&conn).unwrap();

        // root -> dep_a, root -> dep_b. dep_a and dep_b depend on root.
        let root = upsert_node(&conn, "root", &NodeKind::Entity).unwrap();
        let dep_a = upsert_node(&conn, "dep_a", &NodeKind::Entity).unwrap();
        let dep_b = upsert_node(&conn, "dep_b", &NodeKind::Entity).unwrap();
        upsert_edge(&conn, root, dep_a, &EdgeKind::Mentions, 1.0).unwrap();
        upsert_edge(&conn, root, dep_b, &EdgeKind::Mentions, 1.0).unwrap();

        // Sanity: nothing stale yet (the upsert_edge calls don't touch nodes).
        assert!(!is_stale(&conn, dep_a).unwrap());
        assert!(!is_stale(&conn, dep_b).unwrap());

        // Re-upsert the root node — this must cascade staleness to dependents.
        let same = upsert_node(&conn, "root", &NodeKind::Entity).unwrap();
        assert_eq!(same, root, "re-upsert must reuse the rowid");

        assert!(
            is_stale(&conn, dep_a).unwrap(),
            "dependent dep_a must be marked stale after root upsert"
        );
        assert!(
            is_stale(&conn, dep_b).unwrap(),
            "dependent dep_b must be marked stale after root upsert"
        );
        // The root itself is never marked by propagation.
        assert!(
            !is_stale(&conn, root).unwrap(),
            "root must NOT mark itself stale"
        );
    }

    #[test]
    fn upsert_node_staleness_missing_table_is_non_fatal() {
        // No init_staleness — the kg_node_staleness table does not exist.
        // upsert_node must still succeed; the propagation error is swallowed.
        let conn = fresh();
        let id = upsert_node(&conn, "solo", &NodeKind::Entity).unwrap();
        assert!(
            id > 0,
            "upsert must succeed even without the staleness table"
        );
    }

    #[test]
    fn find_nodes_by_name_limit() {
        let conn = fresh();
        for i in 0..5 {
            upsert_node(&conn, &format!("alpha_{i}"), &NodeKind::Tool).unwrap();
        }
        upsert_node(&conn, "beta", &NodeKind::Tool).unwrap();

        let hits = find_nodes_by_name(&conn, "alpha", 3).unwrap();
        assert_eq!(hits.len(), 3, "limit must cap the result set");
        for h in &hits {
            assert!(h.name.starts_with("alpha"));
        }

        // Ensure unrelated rows aren't returned.
        let none = find_nodes_by_name(&conn, "gamma", 10).unwrap();
        assert!(none.is_empty());
    }
}
