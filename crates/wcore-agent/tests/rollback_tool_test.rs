//! W8b C.7 — `RollbackTool` consumes `FileHistory` to restore a file
//! to a previous edit state. Lives in `wcore-agent` (not `wcore-tools`)
//! because it consumes the `FileHistory` snapshot store which itself
//! depends on the engine's root-level RealFs handle (F9).

#![cfg(unix)]

use std::sync::Arc;

use serde_json::json;

use wcore_agent::file_history::FileHistory;
use wcore_agent::rollback_tool::RollbackTool;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::vfs::{InMemoryFs, RealFs, VirtualFs};

#[tokio::test]
async fn cooperative_vfs_rollback_restores_file_to_n_steps_back() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");
    let history = Arc::new(FileHistory::new(
        Arc::new(RealFs),
        shadow.path().to_path_buf(),
    ));

    let path = tmp.path().join("doc.txt");
    let vfs: Arc<dyn VirtualFs> = Arc::new(InMemoryFs::new());

    // Three edits, each with a snapshot of the *pre*-edit state.
    vfs.write(&path, b"v1").await.unwrap();
    history.snapshot(&path, &*vfs).await.unwrap();
    vfs.write(&path, b"v2").await.unwrap();
    history.snapshot(&path, &*vfs).await.unwrap();
    vfs.write(&path, b"v3").await.unwrap();
    history
        .record_committed_postimage(&path, &*vfs)
        .await
        .unwrap();

    // Rollback 1 step → state after v2 was written but before v3.
    // Snapshot index 0 is the *most recent* snapshot (= state right before
    // the v3 write, which is "v2"). So steps=0 brings us to v2.
    let tool = RollbackTool::new(history.clone());
    let mut ctx = ToolContext::test_default();
    ctx.vfs = vfs.clone();
    let result = tool
        .execute_with_ctx(json!({ "path": path.to_str().unwrap(), "steps": 0 }), &ctx)
        .await;

    assert!(!result.is_error, "rollback failed: {}", result.content);
    assert_eq!(vfs.read(&path).await.unwrap(), b"v2");
}

#[tokio::test]
async fn rollback_with_too_many_steps_fails_cleanly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");
    let history = Arc::new(FileHistory::new(
        Arc::new(RealFs),
        shadow.path().to_path_buf(),
    ));

    let path = tmp.path().join("one.txt");
    let vfs: Arc<dyn VirtualFs> = Arc::new(RealFs);
    tokio::fs::write(&path, b"only").await.unwrap();
    history.snapshot(&path, &*vfs).await.unwrap();
    history
        .record_committed_postimage(&path, &*vfs)
        .await
        .unwrap();

    let tool = RollbackTool::new(history);
    let ctx = ToolContext::test_default();
    let result = tool
        .execute_with_ctx(json!({ "path": path.to_str().unwrap(), "steps": 99 }), &ctx)
        .await;

    assert!(result.is_error);
    assert!(
        result.content.contains("snapshots"),
        "expected snapshot-count error, got: {}",
        result.content
    );
}

#[tokio::test]
async fn rollback_emits_suspend_if_file_changed_externally_after_snapshot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");
    let history = Arc::new(FileHistory::new(
        Arc::new(RealFs),
        shadow.path().to_path_buf(),
    ));

    let path = tmp.path().join("clobber.txt");
    let vfs: Arc<dyn VirtualFs> = Arc::new(RealFs);

    // Simulate the engine: snapshot pre-v1 state (there is no pre-state
    // so we just start by writing v1 ourselves), then "engine writes v2"
    // — snapshot the pre-v2 state (= v1), perform the v2 write, and
    // persist the exact post-write object identity so rollback has durable
    // compare-and-swap authority.
    tokio::fs::write(&path, b"v1").await.unwrap();
    history.snapshot(&path, &*vfs).await.unwrap();
    tokio::fs::write(&path, b"v2").await.unwrap();
    history
        .record_committed_postimage(&path, &*vfs)
        .await
        .unwrap();

    // Now the *user* externally edits the live file before the engine
    // has a chance to rollback. The current bytes no longer match the
    // engine's recorded post-write digest => suspend.
    tokio::fs::write(&path, b"user-typed-this").await.unwrap();

    let tool = RollbackTool::new(history);
    let ctx = ToolContext::test_default();
    let result = tool
        .execute_with_ctx(json!({ "path": path.to_str().unwrap(), "steps": 0 }), &ctx)
        .await;

    assert!(result.is_error, "expected suspend marker, got success");
    assert!(
        result.content.to_lowercase().contains("suspend")
            || result.content.contains("changed externally"),
        "expected suspend / external-change message, got: {}",
        result.content
    );
    // And the live file must NOT be clobbered with v1.
    let after = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(after, "user-typed-this");
}

#[tokio::test]
async fn cooperative_vfs_restart_with_unchanged_postimage_rolls_back_by_compare_exchange() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");
    let path = tmp.path().join("restart-success.txt");
    let vfs: Arc<dyn VirtualFs> = Arc::new(InMemoryFs::new());

    vfs.write(&path, b"before").await.unwrap();
    let first = FileHistory::new(Arc::new(RealFs), shadow.path().to_path_buf());
    first.snapshot(&path, &*vfs).await.unwrap();
    vfs.write(&path, b"committed-after").await.unwrap();
    first
        .record_committed_postimage(&path, &*vfs)
        .await
        .unwrap();
    drop(first);

    let restarted = Arc::new(FileHistory::new(
        Arc::new(RealFs),
        shadow.path().to_path_buf(),
    ));
    let mut ctx = ToolContext::test_default();
    ctx.vfs = vfs.clone();
    let result = RollbackTool::new(restarted)
        .execute_with_ctx(json!({ "path": path.to_str().unwrap(), "steps": 0 }), &ctx)
        .await;

    assert!(
        !result.is_error,
        "restart rollback failed: {}",
        result.content
    );
    assert_eq!(vfs.read(&path).await.unwrap(), b"before");
}

#[tokio::test]
async fn restart_with_external_edit_refuses_and_preserves_bytes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");
    let path = tmp.path().join("restart-conflict.txt");
    let vfs: Arc<dyn VirtualFs> = Arc::new(RealFs);

    tokio::fs::write(&path, b"before").await.unwrap();
    let first = FileHistory::new(Arc::new(RealFs), shadow.path().to_path_buf());
    first.snapshot(&path, &*vfs).await.unwrap();
    tokio::fs::write(&path, b"committed-after").await.unwrap();
    first
        .record_committed_postimage(&path, &*vfs)
        .await
        .unwrap();
    drop(first);
    tokio::fs::write(&path, b"external-edit").await.unwrap();

    let restarted = Arc::new(FileHistory::new(
        Arc::new(RealFs),
        shadow.path().to_path_buf(),
    ));
    let result = RollbackTool::new(restarted)
        .execute_with_ctx(
            json!({ "path": path.to_str().unwrap(), "steps": 0 }),
            &ToolContext::test_default(),
        )
        .await;

    assert!(result.is_error, "external edit unexpectedly rolled back");
    assert!(result.content.starts_with("SUSPEND:"));
    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"external-edit");
}

#[tokio::test]
async fn restart_with_byte_identical_replacement_refuses_object_identity_change() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");
    let path = tmp.path().join("restart-identity-conflict.txt");
    let replacement = tmp.path().join("replacement.txt");
    let vfs: Arc<dyn VirtualFs> = Arc::new(RealFs);

    tokio::fs::write(&path, b"before").await.unwrap();
    let first = FileHistory::new(Arc::new(RealFs), shadow.path().to_path_buf());
    first.snapshot(&path, &*vfs).await.unwrap();
    tokio::fs::write(&path, b"committed-after").await.unwrap();
    first
        .record_committed_postimage(&path, &*vfs)
        .await
        .unwrap();
    drop(first);

    // Replace the inode while preserving the committed bytes. Content-only
    // guards would incorrectly authorize this rollback.
    tokio::fs::write(&replacement, b"committed-after")
        .await
        .unwrap();
    tokio::fs::rename(&replacement, &path).await.unwrap();

    let restarted = Arc::new(FileHistory::new(
        Arc::new(RealFs),
        shadow.path().to_path_buf(),
    ));
    let result = RollbackTool::new(restarted)
        .execute_with_ctx(
            json!({ "path": path.to_str().unwrap(), "steps": 0 }),
            &ToolContext::test_default(),
        )
        .await;

    assert!(
        result.is_error,
        "replacement inode unexpectedly rolled back"
    );
    assert!(result.content.starts_with("SUSPEND:"));
    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"committed-after");
}

#[tokio::test]
async fn missing_durable_guard_fails_closed_without_writing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");
    let path = tmp.path().join("missing-guard.txt");
    let vfs: Arc<dyn VirtualFs> = Arc::new(RealFs);
    let history = Arc::new(FileHistory::new(
        Arc::new(RealFs),
        shadow.path().to_path_buf(),
    ));

    tokio::fs::write(&path, b"before").await.unwrap();
    history.snapshot(&path, &*vfs).await.unwrap();
    tokio::fs::write(&path, b"unguarded-current").await.unwrap();

    let result = RollbackTool::new(history)
        .execute_with_ctx(
            json!({ "path": path.to_str().unwrap(), "steps": 0 }),
            &ToolContext::test_default(),
        )
        .await;

    assert!(result.is_error);
    assert!(result.content.starts_with("SUSPEND:"));
    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"unguarded-current");
}

#[tokio::test]
async fn corrupt_durable_guard_fails_closed_without_writing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");
    let path = tmp.path().join("corrupt-guard.txt");
    let vfs: Arc<dyn VirtualFs> = Arc::new(RealFs);

    tokio::fs::write(&path, b"before").await.unwrap();
    let first = FileHistory::new(Arc::new(RealFs), shadow.path().to_path_buf());
    first.snapshot(&path, &*vfs).await.unwrap();
    tokio::fs::write(&path, b"committed-after").await.unwrap();
    first
        .record_committed_postimage(&path, &*vfs)
        .await
        .unwrap();
    drop(first);

    let mut buckets = tokio::fs::read_dir(shadow.path()).await.unwrap();
    let bucket = buckets
        .next_entry()
        .await
        .unwrap()
        .expect("history bucket")
        .path();
    tokio::fs::write(bucket.join("cursor-v1.json"), b"{corrupt")
        .await
        .unwrap();

    let restarted = Arc::new(FileHistory::new(
        Arc::new(RealFs),
        shadow.path().to_path_buf(),
    ));
    let result = RollbackTool::new(restarted)
        .execute_with_ctx(
            json!({ "path": path.to_str().unwrap(), "steps": 0 }),
            &ToolContext::test_default(),
        )
        .await;

    assert!(result.is_error);
    assert!(result.content.starts_with("SUSPEND:"));
    assert_eq!(tokio::fs::read(&path).await.unwrap(), b"committed-after");
}
