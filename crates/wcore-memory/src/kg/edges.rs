//! KG edge CRUD. Edges are stored once per `(src, dst, kind)` triple;
//! `weight` is rewritten on conflict, `created_at` is preserved.

use std::borrow::Cow;

use rusqlite::{Connection, params};

use crate::error::{MemoryError, Result};

/// Classification of a knowledge-graph edge. `Other` carries unknown
/// tags so callers can round-trip arbitrary relation names through the
/// db without losing fidelity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeKind {
    Mentions,
    Uses,
    RelatesTo,
    Other(String),
}

impl EdgeKind {
    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            EdgeKind::Mentions => Cow::Borrowed("mentions"),
            EdgeKind::Uses => Cow::Borrowed("uses"),
            EdgeKind::RelatesTo => Cow::Borrowed("relates_to"),
            EdgeKind::Other(s) => Cow::Borrowed(s.as_str()),
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "mentions" => EdgeKind::Mentions,
            "uses" => EdgeKind::Uses,
            "relates_to" => EdgeKind::RelatesTo,
            other => EdgeKind::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub src: i64,
    pub dst: i64,
    pub kind: EdgeKind,
    pub weight: f32,
    pub created_at: i64,
}

/// Insert an edge, or refresh `weight` if the `(src, dst, kind)` triple
/// already exists. `created_at` is preserved on conflict so callers can
/// see when an edge was first observed.
pub fn upsert_edge(
    conn: &Connection,
    src: i64,
    dst: i64,
    kind: &EdgeKind,
    weight: f32,
) -> Result<()> {
    let kind_s = kind.as_str();
    conn.execute(
        "INSERT INTO kg_edges (src, dst, kind, weight, created_at)
         VALUES (?1, ?2, ?3, ?4, strftime('%s','now'))
         ON CONFLICT(src, dst, kind) DO UPDATE SET weight = excluded.weight",
        params![src, dst, kind_s.as_ref(), weight as f64],
    )
    .map_err(MemoryError::Db)?;
    Ok(())
}

/// All outgoing edges from `src`.
pub fn edges_from(conn: &Connection, src: i64) -> Result<Vec<Edge>> {
    let mut stmt = conn
        .prepare(
            "SELECT src, dst, kind, weight, created_at FROM kg_edges
             WHERE src = ?1 ORDER BY created_at ASC, dst ASC",
        )
        .map_err(MemoryError::Db)?;
    collect_edges(&mut stmt, src)
}

/// All incoming edges into `dst`.
pub fn edges_to(conn: &Connection, dst: i64) -> Result<Vec<Edge>> {
    let mut stmt = conn
        .prepare(
            "SELECT src, dst, kind, weight, created_at FROM kg_edges
             WHERE dst = ?1 ORDER BY created_at ASC, src ASC",
        )
        .map_err(MemoryError::Db)?;
    collect_edges(&mut stmt, dst)
}

fn collect_edges(stmt: &mut rusqlite::Statement<'_>, bind: i64) -> Result<Vec<Edge>> {
    let rows = stmt
        .query_map(params![bind], |r| {
            Ok(Edge {
                src: r.get(0)?,
                dst: r.get(1)?,
                kind: EdgeKind::from_db_str(&r.get::<_, String>(2)?),
                weight: r.get::<_, f64>(3)? as f32,
                created_at: r.get(4)?,
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
    use crate::kg::nodes::{NodeKind, upsert_node};
    use crate::kg::schema;

    fn fresh_with_two_nodes() -> (Connection, i64, i64) {
        let conn = Connection::open_in_memory().unwrap();
        schema::init(&conn).unwrap();
        let a = upsert_node(&conn, "alpha", &NodeKind::Entity).unwrap();
        let b = upsert_node(&conn, "beta", &NodeKind::Entity).unwrap();
        (conn, a, b)
    }

    #[test]
    fn upsert_new_edge() {
        let (conn, a, b) = fresh_with_two_nodes();
        upsert_edge(&conn, a, b, &EdgeKind::Mentions, 0.7).unwrap();
        let out = edges_from(&conn, a).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].dst, b);
        assert_eq!(out[0].kind, EdgeKind::Mentions);
        assert!((out[0].weight - 0.7).abs() < 1e-6);
    }

    #[test]
    fn upsert_existing_updates_weight() {
        let (conn, a, b) = fresh_with_two_nodes();
        upsert_edge(&conn, a, b, &EdgeKind::Uses, 0.3).unwrap();
        let first_created = edges_from(&conn, a).unwrap()[0].created_at;

        upsert_edge(&conn, a, b, &EdgeKind::Uses, 0.9).unwrap();
        let out = edges_from(&conn, a).unwrap();
        assert_eq!(out.len(), 1, "conflict must NOT duplicate the row");
        assert!((out[0].weight - 0.9).abs() < 1e-6);
        assert_eq!(
            out[0].created_at, first_created,
            "created_at must survive an upsert"
        );
    }

    #[test]
    fn edges_from_returns_outgoing_only() {
        let (conn, a, b) = fresh_with_two_nodes();
        let c = upsert_node(&conn, "gamma", &NodeKind::Entity).unwrap();
        upsert_edge(&conn, a, b, &EdgeKind::Mentions, 1.0).unwrap();
        upsert_edge(&conn, c, a, &EdgeKind::Mentions, 1.0).unwrap();

        let out = edges_from(&conn, a).unwrap();
        assert_eq!(out.len(), 1, "only the a->b outgoing edge should appear");
        assert_eq!(out[0].src, a);
        assert_eq!(out[0].dst, b);

        let inc = edges_to(&conn, a).unwrap();
        assert_eq!(inc.len(), 1, "only the c->a incoming edge should appear");
        assert_eq!(inc[0].src, c);
        assert_eq!(inc[0].dst, a);
    }
}
