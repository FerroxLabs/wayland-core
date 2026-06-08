//! W8b C.6 — `FileHistory` snapshot store with FIFO eviction.
//!
//! Resolves audit F9: snapshots are stored via a *root-level* `RealFs`
//! (NOT the per-tool sandboxed VFS) so the shadow dir sits outside any
//! sub-agent's sandbox while still snapshotting the live bytes the
//! sub-agent can see.

use std::path::Path;
use std::sync::Arc;

use wcore_agent::file_history::FileHistory;
use wcore_tools::vfs::{RealFs, VirtualFs};

#[tokio::test]
async fn snapshot_taken_before_write() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");

    let vfs: Arc<dyn VirtualFs> = Arc::new(RealFs);
    let history = FileHistory::new(Arc::new(RealFs), shadow.path().to_path_buf());

    // Live file present.
    let path = tmp.path().join("foo.txt");
    tokio::fs::write(&path, b"v1").await.unwrap();

    // Take the snapshot — reads via per-call vfs, writes via root vfs.
    history.snapshot(&path, &*vfs).await.expect("snapshot");

    // Overwrite the live file.
    tokio::fs::write(&path, b"v2").await.unwrap();

    let restored = history
        .read_snapshot(&path, 0)
        .await
        .expect("read_snapshot");
    assert_eq!(restored, b"v1");
}

#[tokio::test]
async fn fifo_eviction_at_default_max_snapshots() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");

    let vfs: Arc<dyn VirtualFs> = Arc::new(RealFs);
    let history = FileHistory::new(Arc::new(RealFs), shadow.path().to_path_buf());

    let path = tmp.path().join("rolling.txt");

    // Write+snapshot 12 distinct versions; only the last 10 must survive.
    for i in 0..12 {
        let content = format!("v{i}");
        tokio::fs::write(&path, content.as_bytes()).await.unwrap();
        history.snapshot(&path, &*vfs).await.unwrap();
    }

    let count = history.snapshots_count(&path).await;
    assert_eq!(count, 10, "FIFO eviction must cap snapshot count at 10");

    // Most-recent snapshot (index 0) is the v11 we just took.
    let most_recent = history.read_snapshot(&path, 0).await.unwrap();
    assert_eq!(most_recent, b"v11");

    // Oldest surviving (index 9) is v2; v0/v1 were evicted.
    let oldest_surviving = history.read_snapshot(&path, 9).await.unwrap();
    assert_eq!(oldest_surviving, b"v2");
}

#[tokio::test]
async fn read_snapshot_too_far_back_errors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");

    let vfs: Arc<dyn VirtualFs> = Arc::new(RealFs);
    let history = FileHistory::new(Arc::new(RealFs), shadow.path().to_path_buf());

    let path = tmp.path().join("one.txt");
    tokio::fs::write(&path, b"only").await.unwrap();
    history.snapshot(&path, &*vfs).await.unwrap();

    // Index 5 — way past what exists.
    let err = history.read_snapshot(&path, 5).await.unwrap_err();
    let s = err.to_string();
    assert!(
        s.contains("only") && s.contains("snapshots"),
        "error message must say how many snapshots exist, got: {s}"
    );
}

#[tokio::test]
async fn shadow_dir_lives_under_provided_root_only() {
    // Audit F9 invariant: snapshots are written via vfs_root (RealFs),
    // NOT through the sandboxed per-call vfs. The shadow dir is engine
    // state, not project state.
    let shadow = tempfile::tempdir().expect("shadow");

    let history = FileHistory::new(Arc::new(RealFs), shadow.path().to_path_buf());

    // Even when callers pass an InMemoryFs as the per-call vfs (simulating
    // a sandboxed sub-agent), shadow bytes still land on the real shadow
    // dir under `vfs_root`.
    let inmem = wcore_tools::vfs::InMemoryFs::new();
    let project_path = Path::new("/in-mem/proj/file.txt");
    inmem.write(project_path, b"sandbox-bytes").await.unwrap();

    let vfs: Arc<dyn VirtualFs> = Arc::new(inmem);
    history.snapshot(project_path, &*vfs).await.expect("snap");

    // The shadow root we passed in must now contain at least one file.
    let mut entries = tokio::fs::read_dir(shadow.path()).await.unwrap();
    let first = entries.next_entry().await.unwrap();
    assert!(
        first.is_some(),
        "shadow dir must hold snapshot bytes on real disk"
    );

    // And the round-trip via FileHistory still reads back the bytes the
    // per-call sandboxed vfs gave us.
    let bytes = history.read_snapshot(project_path, 0).await.unwrap();
    assert_eq!(bytes, b"sandbox-bytes");
}
