//! F26-SC3-O1 — `backup create` must round-trip a LIVE SQLite database.
//!
//! The live proof for this lives in `scripts/sqlite-backup-consistency-proof.py`
//! and needs three concurrent writer processes and a 150 MiB database to open a
//! wide enough tearing window. That is the right instrument for demonstrating
//! the defect, and the wrong one to leave as the only guard: it cannot run in
//! CI.
//!
//! These tests hold the INVARIANTS the fix established, in a form the suite can
//! keep. Each is written so it would FAIL on the pre-fix code, which read every
//! file in the home independently:
//!
//! * the archive carries the database and NOT its `-wal`/`-shm` sidecars —
//!   pre-fix it carried all three;
//! * the restored database contains content that was still UNCHECKPOINTED in
//!   the WAL at archive time — pre-fix that content survived only by accident of
//!   the sidecar being copied, and only if the two reads happened to agree;
//! * a file that merely ENDS in `-wal` but is not a database's sidecar is still
//!   carried — the guard against over-eager dropping;
//! * the manifest NAMES what it captured and what it dropped.

use std::path::Path;

use wcore_cli::backup::{archive, restore};

/// A home holding a WAL database with content still in the WAL, plus a decoy.
///
/// Returns the row count AND the live connection. The caller must hold the
/// connection for the duration of the archive: dropping it makes this the last
/// connection, and SQLite then checkpoints and deletes the `-wal` on close —
/// quietly converting the live-database case into the quiescent one the test is
/// specifically not about.
fn seeded_home(home: &Path) -> (usize, rusqlite::Connection) {
    std::fs::write(home.join("config.toml"), "default_profile = \"main\"\n").unwrap();

    let db = home.join("memory.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
        .unwrap();
    for i in 0..500 {
        conn.execute(
            "INSERT INTO t (id, v) VALUES (?1, ?2)",
            (i as i64, "seeded"),
        )
        .unwrap();
    }

    // The load-bearing precondition: content that has COMMITTED but has not
    // been checkpointed into the main file. Without it, `memory.db` alone would
    // already be complete and every assertion below would pass on the pre-fix
    // code too — the test would prove nothing.
    assert!(
        home.join("memory.db-wal").exists(),
        "no -wal was produced; this test would be vacuous"
    );
    assert!(
        std::fs::metadata(home.join("memory.db-wal")).unwrap().len() > 0,
        "the -wal is empty; this test would be vacuous"
    );

    // A decoy that must NOT be mistaken for a sidecar.
    std::fs::write(home.join("notes-wal"), b"user content, not a sidecar").unwrap();
    // ... and one that is named like a database but is not one.
    std::fs::write(home.join("lookalike.db"), b"not a database at all").unwrap();

    (500, conn)
}

#[test]
fn a_live_wal_database_is_archived_as_one_consistent_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let (seeded, live) = seeded_home(&home);

    let out = dir.path().join("backup.tar.gz");
    let manifest = archive::create_archive(&home, &out, false).unwrap();
    // Held across the archive on purpose; released only now.
    drop(live);

    let carried: Vec<&str> = manifest.payloads.iter().map(|p| p.path.as_str()).collect();

    // Known-positive in the same assertion set: the database IS carried, and so
    // is unrelated content. A test that only asserted absences would pass on an
    // archive that carried nothing at all.
    assert!(carried.contains(&"memory.db"), "carried: {carried:?}");
    assert!(carried.contains(&"config.toml"), "carried: {carried:?}");
    assert!(
        carried.contains(&"notes-wal"),
        "a user file ending in -wal was dropped as if it were a sidecar: {carried:?}"
    );
    assert!(
        carried.contains(&"lookalike.db"),
        "a non-database named .db was dropped: {carried:?}"
    );

    // The defect: pre-fix these were carried as independently-read files.
    assert!(
        !carried.contains(&"memory.db-wal"),
        "the WAL sidecar was carried: {carried:?}"
    );
    assert!(
        !carried.contains(&"memory.db-shm"),
        "the shm sidecar was carried: {carried:?}"
    );

    // The manifest says what it did, rather than leaving it to be inferred.
    assert_eq!(manifest.sqlite_captures, vec!["memory.db".to_string()]);
    assert!(
        manifest
            .omitted_sqlite_sidecars
            .contains(&"memory.db-wal".to_string()),
        "dropped sidecars were not named: {:?}",
        manifest.omitted_sqlite_sidecars
    );
    assert!(
        !manifest
            .sqlite_captures
            .contains(&"lookalike.db".to_string()),
        "a non-database was captured as one"
    );

    // And the round trip: the restored database must hold the UNCHECKPOINTED
    // rows and pass integrity_check.
    let restored = dir.path().join("restored");
    restore::restore_archive(
        &out,
        &restored,
        restore::RestoreOptions {
            replace: false,
            accept_missing_secrets: true,
            pace_ms: 0,
        },
    )
    .unwrap();

    assert!(!restored.join("memory.db-wal").exists());
    assert!(!restored.join("memory.db-shm").exists());

    let conn = rusqlite::Connection::open(restored.join("memory.db")).unwrap();
    let verdict: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(verdict, "ok", "the restored database is corrupt");
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        n, seeded as i64,
        "the restored database lost rows that were committed before the archive"
    );
}

/// The product's REAL memory schema — `fts5` plus `vec0` virtual tables from the
/// loadable `sqlite-vec` extension — must survive a capture.
///
/// This is the assertion the design decision rests on. `VACUUM INTO` was
/// rejected partly because it rebuilds from the schema and would therefore need
/// every virtual-table module registered; the online backup API copies PAGES and
/// never executes the schema, so it should not care. "Should not care" is a
/// belief until something runs, and this is that something.
#[test]
fn a_database_carrying_vec0_and_fts5_virtual_tables_is_captured() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.toml"), "a = 1").unwrap();

    let db_path = home.join("memory.db");
    {
        let db = wcore_memory::db::Db::open_global(db_path.clone()).unwrap();
        db.ensure_vec_table_for_dim(384).unwrap();
    }

    // Known-positive: the virtual tables really are in this file. Without this
    // the test would pass just as happily on an empty database.
    let probe = rusqlite::Connection::open(&db_path).unwrap();
    let vtabs: i64 = probe
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE sql LIKE '%USING vec0%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let ftss: i64 = probe
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE sql LIKE '%USING fts5%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    drop(probe);
    assert!(
        vtabs > 0,
        "no vec0 virtual table was created; test is vacuous"
    );
    assert!(
        ftss > 0,
        "no fts5 virtual table was created; test is vacuous"
    );

    let out = dir.path().join("backup.tar.gz");
    let manifest = archive::create_archive(&home, &out, false).unwrap();
    assert_eq!(manifest.sqlite_captures, vec!["memory.db".to_string()]);

    let restored = dir.path().join("restored");
    restore::restore_archive(
        &out,
        &restored,
        restore::RestoreOptions {
            replace: false,
            accept_missing_secrets: true,
            pace_ms: 0,
        },
    )
    .unwrap();

    let conn = rusqlite::Connection::open(restored.join("memory.db")).unwrap();
    let verdict: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(verdict, "ok");
    let restored_vtabs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE sql LIKE '%USING vec0%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        restored_vtabs, vtabs,
        "the capture lost virtual-table definitions"
    );
}

#[test]
fn a_home_with_no_database_is_unaffected() {
    // The control that keeps the capture path from being credited for work it
    // did not do: a home with no SQLite in it must archive exactly as before,
    // with an EMPTY capture record rather than an absent one.
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(home.join("skills")).unwrap();
    std::fs::write(home.join("config.toml"), "a = 1").unwrap();
    std::fs::write(home.join("skills/SKILL.md"), "body").unwrap();

    let out = dir.path().join("backup.tar.gz");
    let manifest = archive::create_archive(&home, &out, false).unwrap();

    assert_eq!(manifest.payloads.len(), 2);
    assert!(manifest.sqlite_captures.is_empty());
    assert!(manifest.omitted_sqlite_sidecars.is_empty());
}
