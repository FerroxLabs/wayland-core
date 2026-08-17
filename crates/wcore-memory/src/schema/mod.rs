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
// v7 (#694): adds `evolved_prompts.score_measured` so "no scorer has ever
// measured this row" is representable instead of being spelled with a made-up
// number. Deliberately ADDITIVE — `score` stays REAL NOT NULL and retired rows
// store 0.0 — so an already-released <=v6 binary can still read a v7 store.
// See the SQL file for why a nullable `score` was rejected.

use crate::error::{MemoryError, Result};

pub const CURRENT_VERSION: u32 = 7;

const V1_SQL: &str = include_str!("v1.sql");
const V2_SQL: &str = include_str!("v2_evolved_prompts.sql");
const V3_SQL: &str = include_str!("v3_vec_episodes.sql");
const V4_SQL: &str = include_str!("v4_vec_episodes_dim.sql");
const V5_SQL: &str = include_str!("v5_procedure_latency.sql");
const V6_SQL: &str = include_str!("v6_recall_control.sql");
const V7_SQL: &str = include_str!("v7_evolved_prompts_score_measured.sql");

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
    // Fail closed on a store from the future. Every arm below is
    // `installed < N`, so a newer store matches no arm: nothing runs, nothing
    // errors, and the binary carries on against a schema it does not
    // understand.
    //
    // Scope note, so this is not mistaken for protection it cannot give: this
    // guard ships in v7, so it only ever runs in v7-or-later binaries. It does
    // nothing for a rollback to an already-released <=v6 build, which has no
    // guard at all — that case is handled instead by keeping v7 additive and
    // forward-readable (see the v7 SQL). What this closes is the NEXT
    // downgrade: a v8+ store opened by this build stops here rather than
    // proceeding blind. No attempt is made to rewrite the store to fit.
    if installed > CURRENT_VERSION {
        return Err(MemoryError::SchemaTooNew {
            found: installed,
            supported: CURRENT_VERSION,
        });
    }
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
    // v7 adds `evolved_prompts.score_measured` and retires the drafter's
    // fabricated scores in place. ALTER TABLE ADD COLUMN plus an UPDATE, no
    // rebuild: the table keeps `score REAL NOT NULL` so a rolled-back <=v6
    // binary can still read it. The version bump rides the same transaction,
    // so a crash leaves the pre-migration table intact.
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
    /// genuinely measured score untouched, (b) retire the fabricated ones so
    /// they stop reading as measurements, and (c) leave the absence of a
    /// measurement recordable from now on.
    ///
    /// It must do all that WITHOUT dropping NOT NULL from `score`, which is
    /// what keeps a rolled-back pre-v7 binary able to read the store — see
    /// `v7_store_is_still_readable_by_a_pre_v7_reader`.
    #[test]
    fn v7_retires_fabricated_auto_draft_scores_on_an_existing_store() {
        let mut conn = open_conn_with_vec();
        seed_v6_store_with_a_measured_and_a_fabricated_row(&conn);

        apply_migrations(&mut conn, None).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 7);

        let measured: (f64, i64) = conn
            .query_row(
                "SELECT score, score_measured FROM evolved_prompts WHERE id = 'measured'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            measured,
            (0.42, 1),
            "a real measurement must survive the migration unchanged and stay flagged measured"
        );

        let fabricated: (f64, i64) = conn
            .query_row(
                "SELECT score, score_measured FROM evolved_prompts WHERE id = 'fabricated'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            fabricated.1, 0,
            "the drafter's hardcoded score must be retired, not migrated"
        );
        assert_ne!(
            fabricated.0, 0.7,
            "the fabricated value itself must not survive where an old reader can find it"
        );

        // The absence of a measurement is now recordable.
        conn.execute(
            "INSERT INTO evolved_prompts              (id, skill_name, prompt_body, score, score_measured, scorer, generation, created_at)              VALUES ('fresh', 'auto-sig', 'body', 0.0, 0, 'auto_drafter', 1, 300)",
            [],
        )
        .unwrap();

        // `score` must STILL reject NULL. This is not incidental tidiness: it
        // is the property a rolled-back `score: f64` reader depends on.
        assert!(
            conn.execute(
                "INSERT INTO evolved_prompts                  (id, skill_name, prompt_body, score, scorer, generation, created_at)                  VALUES ('n', 's', 'b', NULL, 'bench', 0, 1)",
                [],
            )
            .is_err(),
            "v7 must not drop NOT NULL from `score` — a pre-v7 reader cannot map NULL"
        );

        // Unmeasured rows sort below measured ones under the ordering the
        // readers use, so ranking cannot be misled by the placeholder value.
        let top: String = conn
            .query_row(
                "SELECT id FROM evolved_prompts ORDER BY score_measured DESC, score DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(top, "measured");

        // Additive migration: the v2 indexes are untouched and the ordering
        // index for the new column exists.
        let n = names(&conn);
        assert!(
            n.iter()
                .any(|x| x == "idx_evolved_prompts_skill_scorer_score"),
            "{n:?}"
        );
        assert!(
            n.iter().any(|x| x == "idx_evolved_prompts_skill_gen"),
            "{n:?}"
        );
        assert!(
            n.iter()
                .any(|x| x == "idx_evolved_prompts_skill_scorer_measured"),
            "{n:?}"
        );
    }

    /// #694 forward-compatibility — a v7 store must stay readable by an
    /// already-released pre-v7 binary.
    ///
    /// Rollback is a supported route here, and every shipped <=v6 build runs a
    /// migration ladder of bare `installed < N` arms. Opened against a v7
    /// store it matches no arm: nothing runs, nothing errors, and it goes
    /// straight to reading. The newer-store guard in `apply_migrations` cannot
    /// help, because it ships IN v7 — it exists only in builds newer than the
    /// ones at risk. Keeping v7 additive is therefore the only thing that
    /// makes the downgrade survivable, and this test is what holds it there.
    ///
    /// This is a SIMULATION, not a live rollback. It transcribes the pre-v7
    /// reader from `wcore-evolve`'s `PromptStore` at the v0.13.0 release tree
    /// (commit 9d5a472a): the same explicit-column SELECT, the same positional
    /// `row.get(i)` mapping, the same `score: f64` field, and the same
    /// `seed_pairs_for` arithmetic. No v0.13.0 binary is executed.
    #[test]
    fn v7_store_is_still_readable_by_a_pre_v7_reader() {
        // The pre-v7 reader, transcribed. Note the explicit column list: the
        // positional indexes are bound to THAT list, not to the table, which
        // is why adding a column to the table does not shift them. Had it been
        // `SELECT *` into a positional struct, an additive migration would
        // break it too and this whole approach would be unavailable.
        fn pre_v7_best_for_skill(
            conn: &Connection,
            skill: &str,
            scorer: &str,
        ) -> rusqlite::Result<Vec<(String, f64)>> {
            let mut stmt = conn.prepare(
                "SELECT id, skill_name, parent_id, prompt_body, score, scorer, generation, created_at, metadata                  FROM evolved_prompts                  WHERE skill_name = ?1 AND scorer = ?2                  ORDER BY score DESC, created_at DESC                  LIMIT ?3",
            )?;
            let rows = stmt.query_map(rusqlite::params![skill, scorer, 10i64], |row| {
                let id: String = row.get(0)?;
                let _skill_name: String = row.get(1)?;
                let _parent_id: Option<String> = row.get(2)?;
                let _prompt_body: String = row.get(3)?;
                // Pre-v7 `EvolvedPrompt` declared `pub score: f64`. This is
                // the exact line a nullable `score` would have broken.
                let score: f64 = row.get(4)?;
                let _scorer: String = row.get(5)?;
                let _generation: i64 = row.get(6)?;
                let _created_at: i64 = row.get(7)?;
                let _metadata: Option<String> = row.get(8)?;
                Ok((id, score))
            })?;
            rows.collect()
        }

        // Pre-v7 `seed_pairs_for`: clamp x5, rounded; 0 is skipped.
        fn pre_v7_seed(score: f64) -> Option<u64> {
            let scaled = (score.clamp(0.0, 1.0) * 5.0).round() as u64;
            (scaled > 0).then_some(scaled)
        }

        // Positive control for every "reads fine" assertion below: the same
        // `get::<_, f64>` mapping DOES fail on a NULL. Without this, a green
        // result could just mean rusqlite never errors here.
        {
            let control = Connection::open_in_memory().unwrap();
            control
                .execute_batch(
                    "CREATE TABLE nullable_probe (score REAL);                      INSERT INTO nullable_probe (score) VALUES (NULL);",
                )
                .unwrap();
            let got: rusqlite::Result<f64> =
                control.query_row("SELECT score FROM nullable_probe", [], |r| r.get(0));
            assert!(
                matches!(got, Err(rusqlite::Error::InvalidColumnType(..))),
                "the pre-v7 mapping must be able to fail on NULL, else this test proves nothing: {got:?}"
            );
        }

        let mut conn = open_conn_with_vec();
        seed_v6_store_with_a_measured_and_a_fabricated_row(&conn);
        apply_migrations(&mut conn, None).unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), 7);

        // 1. The retired row reads without error, as a plain f64.
        let retired = pre_v7_best_for_skill(&conn, "auto-sig", "auto_drafter")
            .expect("a pre-v7 reader must still be able to read a v7 store");
        assert_eq!(retired.len(), 1);
        assert_eq!(retired[0].0, "fabricated");

        // 2. ...and what it reads makes the old binary behave correctly: the
        //    retired row seeds nothing, exactly as the new reader skips it.
        assert_eq!(
            pre_v7_seed(retired[0].1),
            None,
            "the retired placeholder must scale to zero simulated successes in              the OLD arithmetic too, got score {}",
            retired[0].1
        );

        // 3. A genuine measurement is untouched and still pays out.
        let measured = pre_v7_best_for_skill(&conn, "real-skill", "bench").unwrap();
        assert_eq!(measured.len(), 1);
        assert_eq!(measured[0].1, 0.42);
        assert_eq!(pre_v7_seed(measured[0].1), Some(2));

        // 4. A row written by THIS build as unmeasured is also readable, so
        //    the break is not merely deferred to post-upgrade writes.
        conn.execute(
            "INSERT INTO evolved_prompts              (id, skill_name, prompt_body, score, score_measured, scorer, generation, created_at)              VALUES ('fresh', 'auto-sig', 'body', 0.0, 0, 'auto_drafter', 1, 300)",
            [],
        )
        .unwrap();
        let after = pre_v7_best_for_skill(&conn, "auto-sig", "auto_drafter")
            .expect("rows this build writes must also be readable by a pre-v7 reader");
        assert_eq!(after.len(), 2);
        assert!(after.iter().all(|(_, s)| pre_v7_seed(*s).is_none()));

        // 5. The old binary can still WRITE, using the v6 column list. The new
        //    column's DEFAULT covers it.
        conn.execute(
            "INSERT INTO evolved_prompts              (id, skill_name, prompt_body, score, scorer, generation, created_at)              VALUES ('old-writer', 'real-skill', 'body', 0.6, 'bench', 1, 400)",
            [],
        )
        .expect("a pre-v7 writer's INSERT must still be accepted");
        let flag: i64 = conn
            .query_row(
                "SELECT score_measured FROM evolved_prompts WHERE id = 'old-writer'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            flag, 1,
            "a pre-v7 writer means what it always meant by `score`"
        );
    }

    /// Build a store stopped at v6 holding one genuinely measured row and one
    /// row carrying the drafter's fabricated 0.7 — i.e. what is already on
    /// real users' disks before this migration runs.
    fn seed_v6_store_with_a_measured_and_a_fabricated_row(conn: &Connection) {
        conn.execute_batch(V1_SQL).unwrap();
        conn.execute_batch(V2_SQL).unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch(V5_SQL).unwrap();
        conn.execute_batch(V6_SQL).unwrap();
        assert_eq!(current_schema_version(conn).unwrap(), 6);

        // Control: the old shape really did reject NULL, which is why the
        // drafter had a number to invent in the first place.
        assert!(
            conn.execute(
                "INSERT INTO evolved_prompts                  (id, skill_name, prompt_body, score, scorer, generation, created_at)                  VALUES ('n', 's', 'b', NULL, 'bench', 0, 1)",
                [],
            )
            .is_err(),
            "pre-v7 `score` must be NOT NULL or these tests prove nothing"
        );

        conn.execute(
            "INSERT INTO evolved_prompts              (id, skill_name, prompt_body, score, scorer, generation, created_at)              VALUES ('measured', 'real-skill', 'body', 0.42, 'bench', 0, 100)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO evolved_prompts              (id, skill_name, prompt_body, score, scorer, generation, created_at)              VALUES ('fabricated', 'auto-sig', 'body', 0.7, 'auto_drafter', 0, 200)",
            [],
        )
        .unwrap();
    }

    /// #694 follow-up — a store written by a build newer than this one must be
    /// refused, not silently accepted.
    ///
    /// The runner is a ladder of `installed < N` arms, so a newer store makes
    /// every arm false: no migration runs, no error is raised, and the binary
    /// carries on against a schema it does not understand.
    ///
    /// What this does and does not cover: the guard ships in v7, so it protects
    /// v7-and-later builds from a v8+ store. It does nothing for a rollback to
    /// an already-released <=v6 build, which has no guard — that direction is
    /// covered instead by keeping v7 additive
    /// (`v7_store_is_still_readable_by_a_pre_v7_reader`).
    #[test]
    fn refuses_a_store_written_by_a_newer_schema_version() {
        let mut conn = open_conn_with_vec();
        apply_migrations(&mut conn, None).unwrap();

        // Stamp a version this build has never heard of. Nothing else about
        // the file changes — that is exactly the downgrade case: a newer
        // binary migrated the store, then the operator rolled back.
        let future = CURRENT_VERSION + 1;
        conn.execute("INSERT INTO schema_version (version) VALUES (?1)", [future])
            .unwrap();
        assert_eq!(current_schema_version(&conn).unwrap(), future);

        let err = apply_migrations(&mut conn, None)
            .expect_err("a store newer than this build must be refused, not opened");
        let msg = err.to_string();
        assert!(
            msg.contains(&format!("v{future}")),
            "must name the version found: {msg}"
        );
        assert!(
            msg.contains(&format!("v{CURRENT_VERSION}")),
            "must name the version supported: {msg}"
        );
        assert!(
            msg.contains("newer"),
            "must say the store is newer than this build: {msg}"
        );
        assert!(
            msg.contains("upgrade") && msg.contains("backup"),
            "must tell the operator what to do about it: {msg}"
        );
    }

    /// Control for the guard above: it must refuse *only* newer stores.
    ///
    /// Without this, a guard that refused everything would pass the newer-store
    /// test while bricking every normal open and upgrade.
    #[test]
    fn equal_and_older_schema_versions_still_open() {
        // Equal — a store already at CURRENT_VERSION re-opens as a no-op.
        let mut current = open_conn_with_vec();
        apply_migrations(&mut current, None).unwrap();
        assert_eq!(current_schema_version(&current).unwrap(), CURRENT_VERSION);
        apply_migrations(&mut current, None)
            .expect("a store at exactly CURRENT_VERSION must still open");
        assert_eq!(current_schema_version(&current).unwrap(), CURRENT_VERSION);

        // Lower — a v6-era store must still be upgraded. The guard sits in
        // front of the upgrade path and must not block it.
        let mut old = open_conn_with_vec();
        old.execute_batch(V1_SQL).unwrap();
        old.execute_batch(V2_SQL).unwrap();
        old.execute_batch(V3_SQL).unwrap();
        old.execute_batch(V4_SQL).unwrap();
        old.execute_batch(V5_SQL).unwrap();
        old.execute_batch(V6_SQL).unwrap();
        assert_eq!(current_schema_version(&old).unwrap(), 6);
        apply_migrations(&mut old, None).expect("a pre-v7 store must still upgrade");
        assert_eq!(current_schema_version(&old).unwrap(), CURRENT_VERSION);
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
