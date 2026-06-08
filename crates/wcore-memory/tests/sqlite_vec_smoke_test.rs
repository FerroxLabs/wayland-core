// M4.8 — sqlite-vec extension loaded + vec_episodes virtual table present.
//
// This test confirms the substrate that future M5.x work needs:
// 1. The `vec0` virtual-table module is registered on every newly-opened
//    wcore-memory `Db` connection (via sqlite3_auto_extension).
// 2. The `vec_episodes` virtual table created by schema v3 accepts
//    inserts of 384-dim Vec<f32> blobs and returns them ordered by
//    `distance` under a `MATCH` query — the canonical vec0 KNN shape.
//
// Insert/retrieve wiring into the existing `EpisodicPartition::record`
// and `retrieve::search_basic` paths is M5.x scope (deliberate scope
// split documented in v3_vec_episodes.sql).

use wcore_memory::db::Db;

fn f32_blob(v: &[f32]) -> Vec<u8> {
    bytemuck::cast_slice::<f32, u8>(v).to_vec()
}

#[test]
fn vec_episodes_virtual_table_exists() {
    let db = Db::open_memory().expect("open_memory");
    let tc = db.global.clone();
    let conn = tc.conn.lock();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'vec_episodes' AND type = 'table'",
            [],
            |r| r.get(0),
        )
        .expect("query sqlite_master");
    assert_eq!(
        count, 1,
        "vec_episodes virtual table must be created by schema v3"
    );
}

#[test]
fn vec_episodes_accepts_inserts_and_returns_knn_ordering() {
    let db = Db::open_memory().expect("open_memory");
    let tc = db.global.clone();
    let conn = tc.conn.lock();

    // Build three 384-dim canonical-axis vectors so cosine distance is
    // predictable: v0 points along dim 0, v1 along dim 1, v2 along dim 2.
    let mut v0 = vec![0.0f32; 384];
    v0[0] = 1.0;
    let mut v1 = vec![0.0f32; 384];
    v1[1] = 1.0;
    let mut v2 = vec![0.0f32; 384];
    v2[2] = 1.0;

    // Insert 3 rows.
    conn.execute(
        "INSERT INTO vec_episodes (rowid, embedding) VALUES (?1, ?2)",
        rusqlite::params![1_i64, f32_blob(&v0)],
    )
    .expect("insert v0");
    conn.execute(
        "INSERT INTO vec_episodes (rowid, embedding) VALUES (?1, ?2)",
        rusqlite::params![2_i64, f32_blob(&v1)],
    )
    .expect("insert v1");
    conn.execute(
        "INSERT INTO vec_episodes (rowid, embedding) VALUES (?1, ?2)",
        rusqlite::params![3_i64, f32_blob(&v2)],
    )
    .expect("insert v2");

    // Query with a vector identical to v1 — KNN must rank rowid=2 first.
    let mut stmt = conn
        .prepare(
            "SELECT rowid FROM vec_episodes \
             WHERE embedding MATCH ?1 \
             ORDER BY distance \
             LIMIT 3",
        )
        .expect("prepare vec0 MATCH query");

    let ids: Vec<i64> = stmt
        .query_map(rusqlite::params![f32_blob(&v1)], |r| r.get::<_, i64>(0))
        .expect("query")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect");

    assert_eq!(ids.len(), 3, "expected 3 nearest neighbors");
    assert_eq!(
        ids[0], 2,
        "v1's nearest neighbor must be itself (rowid=2): got {ids:?}"
    );
}
