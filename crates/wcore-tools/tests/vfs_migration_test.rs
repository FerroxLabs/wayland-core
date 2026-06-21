//! W8b — vfs migration smoke tests for Read/Write/Edit.
//!
//! Each test mints a `ToolContext` whose `vfs` is a `SandboxedFs` rooted
//! at a tempdir. The tools are then invoked via `execute_with_ctx` so
//! the dispatcher path is exercised end-to-end.

use std::sync::Arc;

use serde_json::json;

use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::edit::EditTool;
use wcore_tools::glob::GlobTool;
use wcore_tools::grep::GrepTool;
use wcore_tools::read::ReadTool;
use wcore_tools::vfs::{RealFs, SandboxedFs};
use wcore_tools::write::WriteTool;

fn sandboxed_ctx(root: &std::path::Path) -> ToolContext {
    let vfs = SandboxedFs::new(RealFs, root.to_path_buf());
    ToolContext {
        call_id: String::new(),
        cancel: tokio_util::sync::CancellationToken::new(),
        vfs: Arc::new(vfs),
        source_agent: None,
        sink: Arc::new(wcore_tools::NullToolOutputSink),
        file_write_notifier: None,
        workspace: None,
    }
}

#[tokio::test]
async fn write_through_ctx_vfs_succeeds_inside_sandbox() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ctx = sandboxed_ctx(tmp.path());
    let tool = WriteTool::new(None);

    let target = tmp.path().join("hello.txt");
    let result = tool
        .execute_with_ctx(
            json!({ "file_path": target.to_str().unwrap(), "content": "hi" }),
            &ctx,
        )
        .await;

    assert!(
        !result.is_error,
        "write inside sandbox failed: {}",
        result.content
    );
    let bytes = tokio::fs::read(&target).await.unwrap();
    assert_eq!(bytes, b"hi");
}

#[tokio::test]
async fn write_through_ctx_vfs_rejected_outside_sandbox() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let other = tempfile::tempdir().expect("other");
    let ctx = sandboxed_ctx(tmp.path());
    let tool = WriteTool::new(None);

    let outside = other.path().join("escape.txt");
    let result = tool
        .execute_with_ctx(
            json!({ "file_path": outside.to_str().unwrap(), "content": "should not land" }),
            &ctx,
        )
        .await;

    assert!(
        result.is_error,
        "write outside sandbox must be rejected, got: {}",
        result.content
    );
    // The file must NOT exist on disk.
    assert!(
        !tokio::fs::try_exists(&outside).await.unwrap_or(false),
        "rejected write must not land on disk"
    );
}

#[tokio::test]
async fn read_through_ctx_vfs_inside_sandbox() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("hello.txt");
    tokio::fs::write(&target, b"line1\nline2\n").await.unwrap();

    let ctx = sandboxed_ctx(tmp.path());
    let tool = ReadTool::new(None);
    let result = tool
        .execute_with_ctx(json!({ "file_path": target.to_str().unwrap() }), &ctx)
        .await;

    assert!(
        !result.is_error,
        "read inside sandbox failed: {}",
        result.content
    );
    assert!(result.content.contains("line1"));
    assert!(result.content.contains("line2"));
}

#[tokio::test]
async fn glob_refused_for_path_outside_sandbox() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let other = tempfile::tempdir().expect("other");
    let ctx = sandboxed_ctx(tmp.path());
    let tool = GlobTool;

    let result = tool
        .execute_with_ctx(
            json!({ "pattern": "*", "path": other.path().to_str().unwrap() }),
            &ctx,
        )
        .await;
    assert!(result.is_error, "glob outside sandbox must be refused");
    assert!(
        result.content.contains("sandbox") || result.content.to_lowercase().contains("refused"),
        "expected sandbox-rejection message, got: {}",
        result.content
    );
}

#[tokio::test]
async fn grep_refused_for_path_outside_sandbox() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let other = tempfile::tempdir().expect("other");
    let ctx = sandboxed_ctx(tmp.path());
    let tool = GrepTool;

    let result = tool
        .execute_with_ctx(
            json!({ "pattern": "foo", "path": other.path().to_str().unwrap() }),
            &ctx,
        )
        .await;
    assert!(result.is_error, "grep outside sandbox must be refused");
    assert!(
        result.content.contains("sandbox") || result.content.to_lowercase().contains("refused"),
        "expected sandbox-rejection message, got: {}",
        result.content
    );
}

#[tokio::test]
async fn edit_through_ctx_vfs_inside_sandbox() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("editme.txt");
    tokio::fs::write(&target, b"hello world").await.unwrap();

    let ctx = sandboxed_ctx(tmp.path());
    let tool = EditTool::new(None);
    let result = tool
        .execute_with_ctx(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "hello",
                "new_string": "goodbye"
            }),
            &ctx,
        )
        .await;
    assert!(!result.is_error, "edit failed: {}", result.content);
    let after = tokio::fs::read_to_string(&target).await.unwrap();
    assert_eq!(after, "goodbye world");
}

// --- W8b.2.A D.4: FileWriteNotifier wiring on Write/Edit ----------------

use async_trait::async_trait;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use wcore_tools::file_write_notifier::FileWriteNotifier;

#[derive(Default)]
struct RecordingNotifier {
    seen: Mutex<Vec<PathBuf>>,
}

#[async_trait]
impl FileWriteNotifier for RecordingNotifier {
    async fn note_self_originated_write(&self, path: &Path) {
        self.seen.lock().push(path.to_path_buf());
    }
}

fn ctx_with_notifier(
    root: &std::path::Path,
    notifier: Arc<RecordingNotifier>,
) -> (ToolContext, Arc<RecordingNotifier>) {
    let vfs = SandboxedFs::new(RealFs, root.to_path_buf());
    let ctx = ToolContext {
        call_id: String::new(),
        cancel: tokio_util::sync::CancellationToken::new(),
        vfs: Arc::new(vfs),
        source_agent: None,
        sink: Arc::new(wcore_tools::NullToolOutputSink),
        file_write_notifier: Some(notifier.clone() as Arc<dyn FileWriteNotifier>),
        workspace: None,
    };
    (ctx, notifier)
}

#[tokio::test]
async fn write_with_notifier_marks_path_before_write() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let notifier = Arc::new(RecordingNotifier::default());
    let (ctx, n) = ctx_with_notifier(tmp.path(), notifier);

    let tool = WriteTool::new(None);
    let target = tmp.path().join("notified.txt");
    let result = tool
        .execute_with_ctx(
            json!({ "file_path": target.to_str().unwrap(), "content": "x" }),
            &ctx,
        )
        .await;
    assert!(!result.is_error, "write failed: {}", result.content);

    let seen = n.seen.lock().clone();
    assert_eq!(
        seen.len(),
        1,
        "expected exactly one note_self_originated_write call, got: {:?}",
        seen
    );
    assert_eq!(seen[0], target);
}

#[tokio::test]
async fn edit_with_notifier_marks_path_before_write() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let notifier = Arc::new(RecordingNotifier::default());
    let (ctx, n) = ctx_with_notifier(tmp.path(), notifier);

    let target = tmp.path().join("editme.txt");
    tokio::fs::write(&target, b"alpha beta").await.unwrap();

    let tool = EditTool::new(None);
    let result = tool
        .execute_with_ctx(
            json!({
                "file_path": target.to_str().unwrap(),
                "old_string": "alpha",
                "new_string": "ALPHA"
            }),
            &ctx,
        )
        .await;
    assert!(!result.is_error, "edit failed: {}", result.content);

    let seen = n.seen.lock().clone();
    assert_eq!(
        seen.len(),
        1,
        "expected exactly one note_self_originated_write call, got: {:?}",
        seen
    );
    assert_eq!(seen[0], target);
}

#[tokio::test]
async fn write_without_notifier_does_not_panic() {
    // Sanity: confirms the legacy default (no notifier on ToolContext)
    // continues to write successfully. Same as the existing sandbox
    // test, but pins behaviour AFTER D.4 wiring.
    let tmp = tempfile::tempdir().expect("tempdir");
    let ctx = sandboxed_ctx(tmp.path());
    let tool = WriteTool::new(None);
    let target = tmp.path().join("nonotify.txt");
    let result = tool
        .execute_with_ctx(
            json!({ "file_path": target.to_str().unwrap(), "content": "y" }),
            &ctx,
        )
        .await;
    assert!(!result.is_error, "write failed: {}", result.content);
    let bytes = tokio::fs::read(&target).await.unwrap();
    assert_eq!(bytes, b"y");
}

#[tokio::test]
async fn write_failure_still_marks_before_attempt() {
    // The notify happens BEFORE the vfs write. If the write fails (e.g.
    // outside-sandbox rejection), the mark is still recorded — that's
    // fine because the FileWatcher's mark TTL prunes stale marks after
    // DEBOUNCE. This test pins the "mark first, write after" order.
    let tmp = tempfile::tempdir().expect("tempdir");
    let other = tempfile::tempdir().expect("other");
    let notifier = Arc::new(RecordingNotifier::default());
    let (ctx, n) = ctx_with_notifier(tmp.path(), notifier);

    let tool = WriteTool::new(None);
    let outside = other.path().join("escape.txt");
    let result = tool
        .execute_with_ctx(
            json!({ "file_path": outside.to_str().unwrap(), "content": "should fail" }),
            &ctx,
        )
        .await;
    assert!(
        result.is_error,
        "write outside sandbox should fail, got: {}",
        result.content
    );
    // Mark was emitted before the (failed) vfs write — pinned ordering.
    let seen = n.seen.lock().clone();
    assert_eq!(seen, vec![outside]);
}
