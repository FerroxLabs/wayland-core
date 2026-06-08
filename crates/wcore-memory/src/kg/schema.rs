//! KG schema bootstrap. Idempotent — safe to call repeatedly on the same
//! connection. Intentionally NOT wired into `crate::schema::apply_migrations`
//! so T2-D1 ships as an opt-in additive table set (rollback via env flag).

use rusqlite::Connection;

use crate::error::{MemoryError, Result};

/// Create `kg_nodes`, `kg_edges`, and the two outgoing/incoming indices
/// if they don't already exist. Safe to run multiple times.
pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS kg_nodes (
             id          INTEGER PRIMARY KEY,
             name        TEXT NOT NULL,
             kind        TEXT NOT NULL,
             created_at  INTEGER NOT NULL,
             last_seen   INTEGER NOT NULL,
             UNIQUE(name, kind)
         );
         CREATE TABLE IF NOT EXISTS kg_edges (
             src         INTEGER NOT NULL,
             dst         INTEGER NOT NULL,
             kind        TEXT NOT NULL,
             weight      REAL NOT NULL DEFAULT 1.0,
             created_at  INTEGER NOT NULL,
             PRIMARY KEY (src, dst, kind),
             FOREIGN KEY(src) REFERENCES kg_nodes(id),
             FOREIGN KEY(dst) REFERENCES kg_nodes(id)
         );
         CREATE INDEX IF NOT EXISTS idx_kg_edges_src ON kg_edges(src);
         CREATE INDEX IF NOT EXISTS idx_kg_edges_dst ON kg_edges(dst);",
    )
    .map_err(MemoryError::Db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        // Second call must succeed (CREATE IF NOT EXISTS).
        init(&conn).unwrap();

        // Verify both tables exist.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name IN ('kg_nodes','kg_edges')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2, "kg_nodes and kg_edges must both exist");

        // Verify both indices exist.
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='index' AND name IN ('idx_kg_edges_src','idx_kg_edges_dst')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 2, "both edge indices must exist");
    }
}
