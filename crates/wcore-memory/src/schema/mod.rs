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
//
// v5: adds the `last_latency_ms` column to `procedures` so `record_use`
// can persist the latency measured by `ProceduralSkillTelemetrySink`
// (previously underscore-ignored, leaving regression detection blind).
//
// v6 (F23-03): adds `memory_privacy_scope` and `memory_retention` — the
// operator's controls over what may be recalled into a prompt. Both are
// keyed by the same (partition, tier) grid cell the access gate governs.
//
// v7 (#694): rebuilds `evolved_prompts` with a NULLABLE `score` so "no scorer
// has ever measured this row" is representable instead of being spelled with
// a made-up number.

use crate::error::{MemoryError, Result};

pub const CURRENT_VERSION: u32 = 7;

const V1_SQL: &str = include_str!("v1.sql");
const V2_SQL: &str = include_str!("v2_evolved_prompts.sql");
const V3_SQL: &str = include_str!("v3_vec_episodes.sql");
const V4_SQL: &str = include_str!("v4_vec_episodes_dim.sql");
const V5_SQL: &str = include_str!("v5_procedure_latency.sql");
const V6_SQL: &str = include_str!("v6_recall_control.sql");
const V7_SQL: &str = include_str!("v7_evolved_prompts_score_nullable.sql");

/// Apply all pending migrations on the given connection.
///
/// `db_path` is the file backing `conn`, or `None` for an in-memory
/// database. It selects the journal mode: WAL on local disks, rollback
/// journaling on network filesystems where WAL corrupts the database.
pub fn apply_migrations(
    conn: &mut rusqlite::Connection,
    db_path: Option<&std::path::Path>,
) -> Result<()> {
    // Journal mode is idempotent, and is chosen from the backing filesystem
    // rather than hardcoded. In-memory databases have no filesystem and no
    // journal to speak of, so they are left alone.
    if let Some(path) = db_path {
        wcore_config::sqlite_journal::SqliteJournalMode::configure(conn, path)?;
    }
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
    if installed < 5 {
        apply_v5(conn)?;
    }
    if installed < 6 {
        apply_v6(conn)?;
    }
    if installed < 7 {
        apply_v7(conn)?;
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

fn apply_v5(conn: &mut rusqlite::Connection) -> Result<()> {
    // v5 is a single ALTER TABLE ADD COLUMN — runs in a transaction so the
    // version bump is atomic with the column add across crashes.
    let tx = conn.transaction().map_err(MemoryError::Db)?;
    tx.execute_batch(V5_SQL)
        .map_err(|e| MemoryError::Migration {
            version: 5,
            source: e,
        })?;
    tx.execute(
        "INSERT OR IGNORE INTO schema_version (version) VALUES (5)",
        [],
    )
    .map_err(|e| MemoryError::Migration {
        version: 5,
        source: e,
    })?;
    tx.commit().map_err(MemoryError::Db)?;
    Ok(())
}

fn apply_v6(conn: &mut rusqlite::Connection) -> Result<()> {
    // v6 is two regular tables plus the version bump — all transactional,
    // so a crash mid-migration leaves either both tables and the bump or
    // neither. The `IF NOT EXISTS` guards keep it idempotent regardless.
    let tx = conn.transaction().map_err(MemoryError::Db)?;
    tx.execute_batch(V6_SQL)
        .map_err(|e| MemoryError::Migration {
            version: 6,
            source: e,
        })?;
    tx.commit().map_err(MemoryError::Db)?;
    Ok(())
}

fn apply_v7(conn: &mut rusqlite::Connection) -> Result<()> {
    // v7 rebuilds `evolved_prompts` to drop NOT NULL from `score`, which SQLite
    // cannot do in place. The rebuild copies every row and the version bump
    // rides the same transaction, so a crash leaves the pre-migration table
    // intact rather than a half-copied one.
    let tx = conn.transaction().map_err(MemoryError::Db)?;
    tx.execute_batch(V7_SQL)
        .map_err(|e| MemoryError::Migration {
            version: 7,
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
        apply_migrations(&mut conn, None).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), CURRENT_VERSION);
    }

    #[test]
    fn v2_creates_evolved_prompts_table_and_indexes() {
        let mut conn = open_conn_with_vec();
        apply_migrations(&mut conn, None).unwrap();
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
        apply_migrations(&mut conn, None).unwrap();
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
    fn v5_adds_last_latency_ms_column_to_procedures() {
        let mut conn = open_conn_with_vec();
        apply_migrations(&mut conn, None).unwrap();
        // Column must exist with a 0 default so legacy rows and call sites
        // without a timing remain insertable.
        let has_col: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('procedures') WHERE name = 'last_latency_ms'")
            .unwrap()
            .query_map([], |_| Ok(()))
            .unwrap()
            .next()
            .is_some();
        assert!(has_col, "procedures.last_latency_ms must exist after v5");
    }

    /// #694 — upgrading a store that was already written under v6.
    ///
    /// The v6 table declared `score REAL NOT NULL`, so the auto-skill drafter
    /// had to put *something* there and put a hardcoded 0.7. Those rows are
    /// already on real users' disks. The migration must (a) preserve every
    /// genuinely measured score untouched, (b) retire the fabricated ones to
    /// NULL rather than carrying the defect forward, and (c) leave the column
    /// able to accept NULL from now on.
    #[test]
    fn v7_retires_fabricated_auto_draft_scores_on_an_existing_store() {
        let mut conn = open_conn_with_vec();
        // Stop at v6 so this is a genuine upgrade of a pre-existing store,
        // not a fresh database that never held the old shape.
        conn.execute_batch(V1_SQL).unwrap();
        conn.execute_batch(V2_SQL).unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch(V5_SQL).unwrap();
        conn.execute_batch(V6_SQL).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 6);

        // Control: the old shape really did reject NULL, which is why the
        // drafter had a number to invent in the first place.
        assert!(
            conn.execute(
                "INSERT INTO evolved_prompts \
                 (id, skill_name, prompt_body, score, scorer, generation, created_at) \
                 VALUES ('n', 's', 'b', NULL, 'bench', 0, 1)",
                [],
            )
            .is_err(),
            "pre-v7 `score` must be NOT NULL or this test proves nothing"
        );

        conn.execute(
            "INSERT INTO evolved_prompts \
             (id, skill_name, prompt_body, score, scorer, generation, created_at) \
             VALUES ('measured', 'real-skill', 'body', 0.42, 'bench', 0, 100)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO evolved_prompts \
             (id, skill_name, prompt_body, score, scorer, generation, created_at) \
             VALUES ('fabricated', 'auto-sig', 'body', 0.7, 'auto_drafter', 0, 200)",
            [],
        )
        .unwrap();

        apply_migrations(&mut conn, None).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 7);

        let measured: Option<f64> = conn
            .query_row(
                "SELECT score FROM evolved_prompts WHERE id = 'measured'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            measured,
            Some(0.42),
            "a real measurement must survive the rebuild unchanged"
        );

        let fabricated: Option<f64> = conn
            .query_row(
                "SELECT score FROM evolved_prompts WHERE id = 'fabricated'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            fabricated, None,
            "the drafter's hardcoded score must be retired, not migrated"
        );

        // The column now accepts the absence of a measurement.
        conn.execute(
            "INSERT INTO evolved_prompts \
             (id, skill_name, prompt_body, score, scorer, generation, created_at) \
             VALUES ('fresh', 'auto-sig', 'body', NULL, 'auto_drafter', 1, 300)",
            [],
        )
        .unwrap();

        // Unmeasured rows sort below measured ones, so the ranking reads that
        // consume this table cannot be misled by the NULL.
        let top: String = conn
            .query_row(
                "SELECT id FROM evolved_prompts ORDER BY score DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(top, "measured");

        // The rebuild must not leave the scratch table or drop the indexes.
        let n = names(&conn);
        assert!(!n.iter().any(|x| x == "evolved_prompts_v7"), "{n:?}");
        assert!(
            n.iter()
                .any(|x| x == "idx_evolved_prompts_skill_scorer_score"),
            "{n:?}"
        );
        assert!(
            n.iter().any(|x| x == "idx_evolved_prompts_skill_gen"),
            "{n:?}"
        );
    }

    #[test]
    fn migrations_are_idempotent() {
        let mut conn = open_conn_with_vec();
        apply_migrations(&mut conn, None).unwrap();
        // Second invocation must be a no-op and must not error on
        // duplicate CREATE TABLE / INSERT.
        apply_migrations(&mut conn, None).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), CURRENT_VERSION);
    }
}
