//! W8b C.7 — `RollbackTool` consumes `FileHistory` to restore a file
//! to a previous edit state. Lives in `wcore-agent` (not `wcore-tools`)
//! because it consumes the `FileHistory` snapshot store which itself
//! depends on the engine's root-level RealFs handle (F9).

use std::sync::Arc;

use serde_json::json;

use wcore_agent::file_history::FileHistory;
use wcore_agent::rollback_tool::RollbackTool;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::vfs::{RealFs, VirtualFs};

#[tokio::test]
async fn rollback_restores_file_to_n_steps_back() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let shadow = tempfile::tempdir().expect("shadow");
    let history = Arc::new(FileHistory::new(
        Arc::new(RealFs),
        shadow.path().to_path_buf(),
    ));

    let path = tmp.path().join("doc.txt");
    let vfs: Arc<dyn VirtualFs> = Arc::new(RealFs);

    // Three edits, each with a snapshot of the *pre*-edit state.
    tokio::fs::write(&path, b"v1").await.unwrap();
    history.snapshot(&path, &*vfs).await.unwrap();
    tokio::fs::write(&path, b"v2").await.unwrap();
    history.snapshot(&path, &*vfs).await.unwrap();
    tokio::fs::write(&path, b"v3").await.unwrap();

    // Rollback 1 step → state after v2 was written but before v3.
    // Snapshot index 0 is the *most recent* snapshot (= state right before
    // the v3 write, which is "v2"). So steps=0 brings us to v2.
    let tool = RollbackTool::new(history.clone());
    let ctx = ToolContext::test_default();
    let result = tool
        .execute_with_ctx(json!({ "path": path.to_str().unwrap(), "steps": 0 }), &ctx)
        .await;

    assert!(!result.is_error, "rollback failed: {}", result.content);
    let after = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(after, "v2");
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
    // record the post-write digest so the rollback guard has something
    // to compare against.
    tokio::fs::write(&path, b"v1").await.unwrap();
    history.snapshot(&path, &*vfs).await.unwrap();
    tokio::fs::write(&path, b"v2").await.unwrap();
    history.record_post_write_digest(&path, b"v2");

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
