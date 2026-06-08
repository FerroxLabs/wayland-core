// W5 Task A.7 acceptance: legacy YAML → P2 import is correct + idempotent.

use std::fs;

use wcore_memory::db::Db;
use wcore_memory::embed::HashedEmbedder;
use wcore_memory::legacy_import;

#[tokio::test]
async fn imports_yaml_fixtures_into_p2_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let yaml_dir = tmp.path().join("legacy");
    fs::create_dir_all(&yaml_dir).unwrap();

    // 5 fixtures with v1 frontmatter shape.
    for (i, ty) in ["user", "feedback", "project", "reference", "user"]
        .iter()
        .enumerate()
    {
        let path = yaml_dir.join(format!("note-{i}.md"));
        let body = format!(
            "---\ntitle: \"note {i}\"\ntype: {ty}\n---\nThis is body line {i}.\nSecond line.\n"
        );
        fs::write(&path, body).unwrap();
    }
    // MEMORY.md must be skipped.
    fs::write(yaml_dir.join("MEMORY.md"), "ignored").unwrap();

    let db = Db::open_memory().unwrap();
    let embedder = HashedEmbedder::new().await.unwrap();

    let r1 = legacy_import::import_if_present(&db, &embedder, &yaml_dir)
        .await
        .unwrap();
    assert_eq!(r1.episodes_inserted, 5, "report: {r1:?}");
    assert!(!r1.already_imported);

    // SELECT COUNT shows 5 episodes with source='legacy'.
    let count: i64 = {
        let tc = db.global.clone();
        let conn = tc.conn.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM episodes WHERE source = 'legacy'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(count, 5);

    // Re-run: idempotent.
    let r2 = legacy_import::import_if_present(&db, &embedder, &yaml_dir)
        .await
        .unwrap();
    assert!(r2.already_imported);
    assert_eq!(r2.episodes_inserted, 0);

    let count2: i64 = {
        let tc = db.global.clone();
        let conn = tc.conn.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM episodes WHERE source = 'legacy'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(count2, 5);
}

#[tokio::test]
async fn missing_dir_returns_empty_report() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("nope");

    let db = Db::open_memory().unwrap();
    let embedder = HashedEmbedder::new().await.unwrap();

    let r = legacy_import::import_if_present(&db, &embedder, &missing)
        .await
        .unwrap();
    assert_eq!(r.episodes_inserted, 0);
    assert!(!r.already_imported);
}
