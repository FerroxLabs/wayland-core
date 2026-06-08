// Schema-migration runner for v2 cognitive memory.
//
// v1.sql is embedded at compile time. apply_migrations() reads the
// schema_version table; if installed < CURRENT_VERSION, it applies the
// missing versions in order.
//
// v2 (M4.4): adds the `evolved_prompts` table that wcore-evolve writes
// winning variants into.
//
// v3 (M4.8): adds the `vec_episodes` virtual table (sqlite-vec vec0)
// for KNN-backed semantic recall. The extension is loaded process-wide
// via `db::register_sqlite_vec` so this migration's CREATE VIRTUAL
// TABLE succeeds on every connection.
//
// v4 (M5.7): adds the `vec_episodes_registry` table so dim-aware
// per-dim virtual tables (vec_episodes_384 / _1024 / _1536) can be
// lazily created on first use. Per-dim virtual tables themselves are
// NOT created here — `db::ensure_vec_table_for_dim(dim)` does that
// on demand because `CREATE VIRTUAL TABLE` cannot run inside a
// transaction and pre-creating empty backend-specific tables on
// every fresh db is wasteful.

use crate::error::{MemoryError, Result};

pub const CURRENT_VERSION: u32 = 4;

const V1_SQL: &str = include_str!("v1.sql");
const V2_SQL: &str = include_str!("v2_evolved_prompts.sql");
const V3_SQL: &str = include_str!("v3_vec_episodes.sql");
const V4_SQL: &str = include_str!("v4_vec_episodes_dim.sql");

/// Apply all pending migrations on the given connection.
pub fn apply_migrations(conn: &mut rusqlite::Connection) -> Result<()> {
    // Always set WAL mode (idempotent).
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    let installed = current_schema_version(conn)?;
    if installed < 1 {
        apply_v1(conn)?;
    }
    if installed < 2 {
        apply_v2(conn)?;
    }
    if installed < 3 {
        apply_v3(conn)?;
    }
    if installed < 4 {
        apply_v4(conn)?;
    }
    Ok(())
}

/// Read the current schema_version (0 if the table doesn't exist yet).
pub fn current_schema_version(conn: &rusqlite::Connection) -> Result<u32> {
    let row: rusqlite::Result<i64> = conn.query_row(
        "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
        [],
        |r| r.get(0),
    );
    match row {
        Ok(v) => Ok(v.max(0) as u32),
        Err(rusqlite::Error::SqliteFailure(_, Some(msg))) if msg.contains("no such table") => Ok(0),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
        // Other errors (including SqliteFailure without "no such table") fall through.
        Err(e) => {
            // Some rusqlite versions report missing tables via different error
            // shapes; if it's any error, attempt to detect "no such table" by
            // string match.
            let s = e.to_string();
            if s.contains("no such table") {
                Ok(0)
            } else {
                Err(MemoryError::Db(e))
            }
        }
    }
}

fn apply_v1(conn: &mut rusqlite::Connection) -> Result<()> {
    let tx = conn.transaction().map_err(MemoryError::Db)?;
    tx.execute_batch(V1_SQL)
        .map_err(|e| MemoryError::Migration {
            version: 1,
            source: e,
        })?;
    tx.commit().map_err(MemoryError::Db)?;
    Ok(())
}

fn apply_v2(conn: &mut rusqlite::Connection) -> Result<()> {
    let tx = conn.transaction().map_err(MemoryError::Db)?;
    tx.execute_batch(V2_SQL)
        .map_err(|e| MemoryError::Migration {
            version: 2,
            source: e,
        })?;
    // Record the version bump so a re-open observes installed >= 2 and
    // doesn't re-apply.
    tx.execute(
        "INSERT OR IGNORE INTO schema_version (version) VALUES (2)",
        [],
    )
    .map_err(|e| MemoryError::Migration {
        version: 2,
        source: e,
    })?;
    tx.commit().map_err(MemoryError::Db)?;
    Ok(())
}

fn apply_v3(conn: &mut rusqlite::Connection) -> Result<()> {
    // CREATE VIRTUAL TABLE cannot run inside a transaction in SQLite,
    // so we apply v3 with auto-commit and record the version bump
    // separately. The `IF NOT EXISTS` guard makes the CREATE idempotent
    // across re-opens before the version bump lands.
    conn.execute_batch(V3_SQL)
        .map_err(|e| MemoryError::Migration {
            version: 3,
            source: e,
        })?;
    conn.execute(
        "INSERT OR IGNORE INTO schema_version (version) VALUES (3)",
        [],
    )
    .map_err(|e| MemoryError::Migration {
        version: 3,
        source: e,
    })?;
    Ok(())
}

fn apply_v4(conn: &mut rusqlite::Connection) -> Result<()> {
    // v4 is the per-dim registry — only a regular table + a seed row,
    // so it CAN run in a transaction (no CREATE VIRTUAL TABLE here).
    // Keeps the seed atomic with the table create across crashes.
    let tx = conn.transaction().map_err(MemoryError::Db)?;
    tx.execute_batch(V4_SQL)
        .map_err(|e| MemoryError::Migration {
            version: 4,
            source: e,
        })?;
    tx.execute(
        "INSERT OR IGNORE INTO schema_version (version) VALUES (4)",
        [],
    )
    .map_err(|e| MemoryError::Migration {
        version: 4,
        source: e,
    })?;
    tx.commit().map_err(MemoryError::Db)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type IN ('table','index') ORDER BY name",
            )
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    /// Construct a Connection that has sqlite-vec auto-registered (the
    /// production path in `db::TierConn::open_memory` does this; tests
    /// that touch `apply_migrations` directly must do the same so v3's
    /// CREATE VIRTUAL TABLE USING vec0 succeeds).
    fn open_conn_with_vec() -> Connection {
        // Side-effect: registers the sqlite-vec auto-extension if not
        // already registered. Using the public `Db::open_memory` path
        // would also work but pulls in more surface than we need.
        let _ = crate::db::TierConn::open_memory().unwrap();
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn fresh_db_lands_at_current_version() {
        let mut conn = open_conn_with_vec();
        apply_migrations(&mut conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn v2_creates_evolved_prompts_table_and_indexes() {
        let mut conn = open_conn_with_vec();
        apply_migrations(&mut conn).unwrap();
        let n = names(&conn);
        assert!(n.iter().any(|x| x == "evolved_prompts"), "{n:?}");
        assert!(
            n.iter().any(|x| x == "idx_evolved_prompts_skill_gen"),
            "{n:?}"
        );
        assert!(
            n.iter()
                .any(|x| x == "idx_evolved_prompts_skill_scorer_score"),
            "{n:?}"
        );
    }

    #[test]
    fn v3_creates_vec_episodes_virtual_table() {
        let mut conn = open_conn_with_vec();
        apply_migrations(&mut conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'vec_episodes' AND type = 'table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "vec_episodes virtual table must exist");
    }

    #[test]
    fn migrations_are_idempotent() {
        let mut conn = open_conn_with_vec();
        apply_migrations(&mut conn).unwrap();
        // Second invocation must be a no-op and must not error on
        // duplicate CREATE TABLE / INSERT.
        apply_migrations(&mut conn).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), CURRENT_VERSION);
    }
}
