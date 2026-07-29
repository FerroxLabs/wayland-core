//! Consistent point-in-time capture of a live SQLite database — the single
//! place that decides how any SQLite database in this workspace is copied.
//!
//! Sibling of [`crate::sqlite_journal`], which decides how a database
//! *journals*. That module chooses the mode; this one copies the result. They
//! are deliberately adjacent: both exist because a SQLite database is not a
//! file, and code that treats it as one loses data silently.
//!
//! # Why (measured, not assumed)
//!
//! A WAL-mode database is a TRIO — `x.db`, `x.db-wal`, `x.db-shm` — whose
//! members are only meaningful together. `wayland-core backup create` read each
//! one independently with `std::fs::read`, at whatever instant the archive walk
//! reached it.
//!
//! Measured on `hetzner-dsm`, three stock-Python SQLite writers committing
//! across a 10.5-second archive of a 158 MiB home:
//!
//! | arm | writers live during archive | restored `integrity_check` |
//! |-----|-----------------------------|----------------------------|
//! | concurrent | yes (+23,131 / +23,654 / +23,776 commits) | **corrupt**, 100 problem lines |
//! | sequenced  | no (same writers, same 64,141 rows, stopped first) | `ok` |
//!
//! Both `backup create` and `backup restore` exited **0** in the corrupt arm and
//! printed nothing unusual. As with the WAL-on-NFS defect that
//! [`crate::sqlite_journal`] closes, the failure is silent: there is no error
//! for a caller to handle, so the copy must be taken correctly up front.
//!
//! # The primitive, and why this one
//!
//! [`snapshot_database`] uses SQLite's **online backup API**
//! (`sqlite3_backup_*`, via `rusqlite::backup`) with a single `step(-1)`.
//!
//! A single `step(-1)` copies the entire database inside ONE read transaction,
//! so in WAL mode it observes one snapshot and cannot see a half-applied
//! checkpoint. The documented restart-on-external-write happens *between*
//! successive `step()` calls; with one call there is no between.
//!
//! Two alternatives were considered and rejected on evidence:
//!
//! * **`BEGIN IMMEDIATE` + byte-copy the trio.** Holding the writer lock stops
//!   commits, but a PASSIVE checkpoint does NOT take the writer lock (only
//!   FULL/RESTART/TRUNCATE do), so another connection may still write pages into
//!   the main file mid-copy. The byte copy is therefore not actually frozen. Two
//!   of three cross-auditors identified this independently.
//! * **`VACUUM INTO`.** Consistent, but it REBUILDS the database from its
//!   schema, which makes it depend on things a copy should not depend on: every
//!   virtual-table module being registered on the connection (this workspace
//!   ships `vec0` from the loadable `sqlite-vec` extension), and — per SQLite's
//!   own documentation — it "may change the ROWIDs of entries in tables that do
//!   not have an explicit INTEGER PRIMARY KEY". `wcore-memory` declares
//!   `episodes (id TEXT PRIMARY KEY)` with `episodes_fts` as an external-content
//!   FTS5 index keyed on that IMPLICIT rowid, so a renumbering would silently
//!   desynchronise every full-text search while leaving `integrity_check` clean.
//!
//!   **Honesty note:** that renumbering was PROBED and did NOT reproduce —
//!   `scripts/sqlite-snapshot-primitive-probe.py` shows VACUUM INTO preserving
//!   rowids and the FTS mapping on exactly this schema at SQLite 3.53.2. It is
//!   rejected for depending on a documented "may" and on module registration,
//!   not for an observed failure. The backup API needs neither: it copies PAGES
//!   and never executes the schema.
//!
//! # What this does NOT promise
//!
//! The captured bytes are a consistent *database*, not a byte-identical *file*.
//! Folding a WAL into the main file is the entire point, so it cannot be. What
//! is promised is that the capture opens, passes `integrity_check`, and contains
//! every transaction committed before the capture began.

use std::path::{Path, PathBuf};

/// The 16-byte string every SQLite database file begins with, including its
/// terminating NUL. Detection is by CONTENT, never by filename: a home may hold
/// a `.db` that is not SQLite, and a SQLite database that is not named `.db`.
pub const SQLITE_HEADER_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Suffixes SQLite appends to a database path for state that is derived from,
/// and meaningless without, the database itself.
///
/// `-wal` and `-shm` are WAL sidecars; `-journal` is the rollback journal. All
/// three are excluded from a capture: the snapshot has already absorbed
/// everything they contain, and a restored `-shm` in particular is a wal-index
/// belonging to a process that no longer exists.
pub const DERIVED_SIDECAR_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];

/// How long to wait for a lock before giving up. A backup that silently skipped
/// a busy database would be the same silent-staleness failure in a new costume,
/// so exhaustion is an error, never a skip.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("cannot open {path} as a SQLite database: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    #[error("consistent capture of {path} failed: {source}")]
    Capture {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    /// The capture completed but did not verify. Reported rather than shipped:
    /// an archive that restores a corrupt database is worse than no archive,
    /// because the operator only finds out when they need it.
    #[error("the consistent capture of {path} did not pass integrity_check: {detail}")]
    Unverified { path: PathBuf, detail: String },

    #[error("io error while {context} for {path}: {source}")]
    Io {
        context: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Whether `path` is a SQLite database, decided by its header magic.
///
/// Returns `false` for anything shorter than the header, unreadable, or not
/// beginning with [`SQLITE_HEADER_MAGIC`] — including a zero-length file that a
/// connection has created but never written.
pub fn is_sqlite_database(path: &Path) -> bool {
    use std::io::Read as _;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 16];
    match f.read_exact(&mut head) {
        Ok(()) => &head == SQLITE_HEADER_MAGIC,
        Err(_) => false,
    }
}

/// Whether `name` is a derived sidecar of `db_name`.
///
/// Compared against a KNOWN database name rather than pattern-matched on its
/// own, so an unrelated file that merely ends in `-wal` is never dropped from an
/// archive.
pub fn is_derived_sidecar_of(name: &str, db_name: &str) -> bool {
    DERIVED_SIDECAR_SUFFIXES.iter().any(|suffix| {
        name.len() == db_name.len() + suffix.len() && name == format!("{db_name}{suffix}")
    })
}

/// Capture `src` into `dest` as a consistent point-in-time snapshot.
///
/// `dest` must not exist. On success `dest` is a single self-contained SQLite
/// database with no sidecars, containing every transaction committed before the
/// call began, and it has passed `PRAGMA integrity_check`.
///
/// # Why the source is opened read-write
///
/// SQLite cannot open a LIVE WAL database read-only: the reader needs the
/// `-shm` wal-index, and creating it requires write access. A read-only open
/// therefore fails in exactly the case this function exists for. The only
/// mutation this can cause to the source is SQLite's ordinary
/// checkpoint-on-last-close, which is data-preserving and is what any clean
/// shutdown does.
pub fn snapshot_database(src: &Path, dest: &Path) -> Result<(), SnapshotError> {
    use rusqlite::Connection;

    let source = Connection::open(src).map_err(|source| SnapshotError::Open {
        path: src.to_path_buf(),
        source,
    })?;
    source
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(|source| SnapshotError::Open {
            path: src.to_path_buf(),
            source,
        })?;

    let mut target = Connection::open(dest).map_err(|source| SnapshotError::Open {
        path: dest.to_path_buf(),
        source,
    })?;

    {
        let backup = rusqlite::backup::Backup::new(&source, &mut target).map_err(|source| {
            SnapshotError::Capture {
                path: src.to_path_buf(),
                source,
            }
        })?;
        // ONE call. `Progress { remaining: 0 }` is the whole database copied
        // inside a single read transaction; stepping in chunks would reopen the
        // restart-on-external-write window this function exists to avoid.
        backup.step(-1).map_err(|source| SnapshotError::Capture {
            path: src.to_path_buf(),
            source,
        })?;
    }

    // The capture is VERIFIED, not assumed. Without this the fix would rest on
    // the same kind of unchecked belief as the defect it replaces.
    let verdict: String = target
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|source| SnapshotError::Capture {
            path: dest.to_path_buf(),
            source,
        })?;
    if verdict != "ok" {
        return Err(SnapshotError::Unverified {
            path: src.to_path_buf(),
            detail: verdict,
        });
    }
    drop(target);

    // A freshly written target may carry its own sidecars. They describe the
    // capture process, not the data, and must not reach the archive.
    for suffix in DERIVED_SIDECAR_SUFFIXES {
        let side = PathBuf::from(format!("{}{suffix}", dest.display()));
        if side.exists() {
            std::fs::remove_file(&side).map_err(|source| SnapshotError::Io {
                context: "removing a capture sidecar",
                path: side,
                source,
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded_db(dir: &Path, name: &str, rows: usize) -> PathBuf {
        let path = dir.join(name);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
            .unwrap();
        for i in 0..rows {
            conn.execute("INSERT INTO t (id, v) VALUES (?1, ?2)", (i as i64, "x"))
                .unwrap();
        }
        path
    }

    #[test]
    fn detects_a_sqlite_database_by_content_and_rejects_a_lookalike() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(dir.path(), "memory.db", 4);
        assert!(is_sqlite_database(&db), "a real database was not detected");

        // Known-negative in the SAME sweep: named like a database, is not one.
        let fake = dir.path().join("notreally.db");
        std::fs::write(&fake, b"this is plainly not a database").unwrap();
        assert!(
            !is_sqlite_database(&fake),
            "a lookalike was detected as SQLite"
        );

        // Shorter than the header — the boundary the read must not panic on.
        let stub = dir.path().join("stub.db");
        std::fs::write(&stub, b"SQLite").unwrap();
        assert!(!is_sqlite_database(&stub));

        // Zero-length: a connection created it and never wrote.
        let empty = dir.path().join("empty.db");
        std::fs::write(&empty, b"").unwrap();
        assert!(!is_sqlite_database(&empty));

        assert!(!is_sqlite_database(&dir.path().join("absent.db")));
    }

    #[test]
    fn sidecars_are_matched_against_their_own_database_only() {
        assert!(is_derived_sidecar_of("memory.db-wal", "memory.db"));
        assert!(is_derived_sidecar_of("memory.db-shm", "memory.db"));
        assert!(is_derived_sidecar_of("memory.db-journal", "memory.db"));
        assert!(!is_derived_sidecar_of("memory.db", "memory.db"));
        // An unrelated file that merely ends in `-wal` must survive an archive.
        assert!(!is_derived_sidecar_of("notes-wal", "memory.db"));
        assert!(!is_derived_sidecar_of("other.db-wal", "memory.db"));
        // Prefix-only matching would drop this; length equality must not.
        assert!(!is_derived_sidecar_of("memory.db-wal.bak", "memory.db"));
    }

    #[test]
    fn a_wal_database_is_captured_whole_with_no_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let db = seeded_db(dir.path(), "memory.db", 64);

        // Put uncheckpointed content in the WAL, so the capture is only correct
        // if it folds the WAL in. A test on a checkpointed database would pass
        // even on an implementation that ignored the WAL entirely.
        let live = rusqlite::Connection::open(&db).unwrap();
        live.pragma_update(None, "journal_mode", "WAL").unwrap();
        for i in 1000..1200 {
            live.execute("INSERT INTO t (id, v) VALUES (?1, ?2)", (i as i64, "wal"))
                .unwrap();
        }
        let wal = dir.path().join("memory.db-wal");
        assert!(
            wal.exists(),
            "no WAL was produced; this test proves nothing"
        );
        let wal_len = std::fs::metadata(&wal).unwrap().len();
        assert!(wal_len > 0, "the WAL is empty; this test proves nothing");

        let out = dir.path().join("cap").join("memory.db");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        snapshot_database(&db, &out).unwrap();
        drop(live);

        assert!(!out.with_file_name("memory.db-wal").exists());
        assert!(!out.with_file_name("memory.db-shm").exists());

        let cap = rusqlite::Connection::open(&out).unwrap();
        let n: i64 = cap
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 264, "the capture lost the uncheckpointed WAL content");
        let verdict: String = cap
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(verdict, "ok");
    }

    #[test]
    fn a_non_database_is_refused_rather_than_captured() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.db");
        std::fs::write(&junk, b"SQLite format 3\0 but the rest is garbage").unwrap();
        // Detection passes on the magic alone — this is the case where the
        // capture itself must be the thing that refuses.
        assert!(is_sqlite_database(&junk));
        let err = snapshot_database(&junk, &dir.path().join("out.db")).unwrap_err();
        assert!(
            matches!(
                err,
                SnapshotError::Capture { .. } | SnapshotError::Unverified { .. }
            ),
            "expected a refusal, got {err:?}"
        );
    }
}
