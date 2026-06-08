// W5 Task A.4 acceptance: schema migration applied to a fresh tempdir DB
// surfaces every expected table and index name.

use wcore_memory::db::{TierConn, list_objects};

#[test]
fn fresh_db_has_v2_tables_and_indexes() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("memory.db");
    let tc = TierConn::open(path).unwrap();
    let conn = tc.conn.lock();
    let names = list_objects(&conn).unwrap();

    let expected_tables = [
        "episodes",
        "facts",
        "procedures",
        "user_model",
        "p1_working",
        "cdc_log",
        "schema_version",
        "legacy_import_marker",
    ];
    for tbl in expected_tables {
        assert!(
            names.contains(&tbl.to_string()),
            "missing table {tbl}; saw {names:?}"
        );
    }
    let expected_indexes = [
        "idx_episodes_tier_ts",
        "idx_episodes_status",
        "idx_facts_subject_predicate",
        "idx_procedures_name_tier",
        "idx_cdc_tier_seq",
    ];
    for ix in expected_indexes {
        assert!(
            names.contains(&ix.to_string()),
            "missing index {ix}; saw {names:?}"
        );
    }
    // FTS5 virtual table for episodes
    assert!(
        names.contains(&"episodes_fts".to_string()),
        "missing FTS5: {names:?}"
    );
}

#[test]
fn open_pool_creates_three_dbs() {
    let tmp = tempfile::tempdir().unwrap();
    let session = tmp.path().join("session.db");
    let project = tmp.path().join("project").join("memory.db");
    let global = tmp.path().join("global.db");
    let pool = wcore_memory::db::Db::open(Some(session), Some(project), global).unwrap();
    assert!(pool.session.read().is_some());
    assert!(pool.project.is_some());
    let _g = pool.global; // present unconditionally
}
