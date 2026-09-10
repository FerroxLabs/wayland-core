//! FerroxLabs/wayland#1351 c1 — two openers racing a pending migration must
//! BOTH end with a working memory backend.
//!
//! THE DEFECT. One memory base directory is one set of database files, and
//! `apply_migrations` is not serialized across connections. Two agents started
//! at once against a store that needs a migration both read
//! `current_schema_version` below the target and both run the same step. The
//! steps that cannot be made idempotent are the `ALTER TABLE ADD COLUMN`s (v5
//! `procedures.last_latency_ms`, v7 `evolved_prompts.score_measured`) — SQLite
//! has no `IF NOT EXISTS` for a column — so the loser gets
//! `duplicate column name: last_latency_ms`, `Memory::open` returns `Err`, and
//! `bootstrap.rs` falls back to `NullMemory`. That user's long-term memory is
//! gone for the session. It is the shape of the first minutes after an upgrade:
//! a pending migration plus concurrent starts.
//!
//! WHY THIS IS A THREAD TEST AND NOT A PROCESS TEST. The race is between
//! CONNECTIONS, not processes: SQLite serializes on file locks, and two
//! `rusqlite::Connection`s in one process contend exactly as two processes do
//! (no shared cache, separate transactions, separate write locks). Threads give
//! the same collision with a barrier that a `Command::spawn` pair could only
//! approximate, and they let the test read
//! `CONCURRENT_MIGRATION_RECOVERIES` — see the vacuity guard below.
//!
//! RED ARM. Revert `run_ladder`'s retry in `crates/wcore-memory/src/schema/mod.rs`
//! (call `apply_pending` once and return its result) and
//! `both_openers_of_a_migrating_store_get_a_working_backend` fails with
//! `duplicate column name: last_latency_ms` on every loser. Revert the scoped
//! `busy_timeout` as well and the losers fail with `database is locked`
//! instead — a different error, the same NullMemory for the user.
//!
//! VACUITY. The repair turns a failure into a success and leaves no trace in
//! the store, so a run in which the race never happened would pass identically
//! to one in which it did. `CONCURRENT_MIGRATION_RECOVERIES` is therefore
//! asserted to have MOVED: if the openers never actually collided this test
//! fails as vacuous rather than certifying a fix it did not exercise.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use wcore_memory::db::TierConn;
use wcore_memory::schema::{CONCURRENT_MIGRATION_RECOVERIES, CURRENT_VERSION};

/// Openers per round. Three losers per winner is enough to make the collision
/// overwhelming while keeping every loser inside the 5 s busy budget.
const OPENERS: usize = 4;

/// Rounds, each on its own fresh store. More than one because the interleaving
/// is scheduler-dependent: a single round could in principle serialize cleanly.
const ROUNDS: usize = 6;

/// Open one store from `OPENERS` threads released together, and return every
/// opener's result.
fn race_one_store(path: std::path::PathBuf) -> Vec<Result<(), String>> {
    let barrier = Arc::new(std::sync::Barrier::new(OPENERS));
    let handles: Vec<_> = (0..OPENERS)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                // The production open path: create dirs, register sqlite-vec,
                // run the ladder, harden perms. This is the call
                // `Db::open` -> `Memory::open_with_config_in` -> bootstrap
                // makes, so a failure here is exactly the `Err` that becomes
                // `NullMemory`.
                TierConn::open(path).map(|_| ()).map_err(|e| e.to_string())
            })
        })
        .collect();
    handles
        .into_iter()
        .map(|h| h.join().expect("opener thread did not panic"))
        .collect()
}

/// THE #1351 c1 GUARD.
///
/// `serial` because `CONCURRENT_MIGRATION_RECOVERIES` is process-wide: a
/// sibling test racing its own store would inflate this one's delta and, worse,
/// could make the negative test below read a recovery it did not cause.
#[serial_test::serial]
#[test]
fn both_openers_of_a_migrating_store_get_a_working_backend() {
    let before = CONCURRENT_MIGRATION_RECOVERIES.load(Ordering::Relaxed);

    for round in 0..ROUNDS {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("memory.db");

        let results = race_one_store(path.clone());

        let failures: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
        assert!(
            failures.is_empty(),
            "round {round}: {} of {OPENERS} concurrent openers lost the migration race and \
             would have been dropped to NullMemory — #1351. Failures: {failures:?}",
            failures.len()
        );

        // A store every opener could open is not yet a store that MIGRATED.
        // Assert the end state on a fresh connection, including the exact
        // column the losers collide on.
        let conn = rusqlite::Connection::open(&path).expect("reopen the raced store");
        let version = wcore_memory::schema::current_schema_version(&conn)
            .expect("read schema_version off the raced store");
        assert_eq!(
            version, CURRENT_VERSION,
            "round {round}: the race left the store at v{version}, not v{CURRENT_VERSION}"
        );
        let latency_col: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('procedures') \
                 WHERE name = 'last_latency_ms'",
                [],
                |r| r.get(0),
            )
            .expect("inspect procedures columns");
        assert_eq!(
            latency_col, 1,
            "round {round}: the v5 column the race collides on is not on the table"
        );
    }

    // The vacuity guard. See the module header.
    let recoveries = CONCURRENT_MIGRATION_RECOVERIES.load(Ordering::Relaxed) - before;
    assert!(
        recoveries > 0,
        "{ROUNDS} rounds of {OPENERS} concurrent openers produced ZERO concurrent-migration \
         recoveries, so the openers never actually collided and this run asserted nothing \
         about #1351. The test is vacuous, not the product green."
    );
}

/// The other direction, and the reason the retry is keyed on the store's
/// version rather than on the text of the error: a migration that fails for a
/// reason nobody else has repaired must still FAIL, not be retried into a loop
/// or swallowed.
///
/// `procedures` is dropped out from under a v4 store, so v5's
/// `ALTER TABLE procedures ADD COLUMN` cannot succeed and no concurrent opener
/// exists to install v5 either. Without the version predicate — if the retry
/// keyed off `duplicate column name` or simply retried every `Migration` error
/// — this would spin `CURRENT_VERSION` times and then return the same error
/// anyway; with it, the first failure is returned untouched.
///
/// `serial` for the same reason as the guard above: the recovery counter it
/// asserts on is process-wide.
#[serial_test::serial]
#[test]
fn a_migration_nobody_else_repaired_still_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("memory.db");

    // Build a real store, then walk it back to v4 and remove the table v5
    // needs. `DROP TABLE` is honest here: the point is a v5 step that cannot
    // succeed, not a specific corruption.
    TierConn::open(path.clone()).expect("first open migrates the store");
    {
        let conn = rusqlite::Connection::open(&path).expect("open for setup");
        conn.execute("DELETE FROM schema_version WHERE version >= 5", [])
            .expect("walk the store back to v4");
        conn.execute("DROP TABLE procedures", [])
            .expect("remove the table v5 alters");
    }

    let before = CONCURRENT_MIGRATION_RECOVERIES.load(Ordering::Relaxed);
    let err = TierConn::open(path)
        .err()
        .expect("a v5 step with no `procedures` table must not report success");
    assert!(
        err.to_string().contains("migration error at v5"),
        "the failure was reported as something other than the v5 step: {err}"
    );
    assert_eq!(
        CONCURRENT_MIGRATION_RECOVERIES.load(Ordering::Relaxed),
        before,
        "an unrepaired migration was counted as a concurrent recovery, so the retry is \
         keyed on the error rather than on the store's installed version"
    );
}
