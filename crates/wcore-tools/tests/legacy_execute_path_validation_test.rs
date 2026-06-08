//! Wave SD — verify the legacy (non-`_with_ctx`) entries on
//! Read/Write/Edit refuse paths that escape the discipline.
//!
//! Closes SECURITY MAJOR #14 verification:
//!
//!   * `Read::execute({"file_path": "/etc/shadow"})` returns an
//!     `is_error: true` ToolResult, not the file's contents.
//!   * `Write::execute({"file_path": "../etc/passwd", ...})` is
//!     refused before any disk touch.
//!   * `Edit::execute({"file_path": "<absolute path under tmp>", ...})`
//!     proceeds (positive control — the validation rejects the
//!     specifically-dangerous shapes, not all writes).

use serde_json::json;
use tempfile::tempdir;

use wcore_tools::Tool;
// EditTool is only used by edit_legacy_refuses_ssh_key_path (cfg(unix));
// gate the import to match (Windows clippy E0432, CI run 25955124617).
#[cfg(unix)]
use wcore_tools::edit::EditTool;
use wcore_tools::read::ReadTool;
use wcore_tools::write::WriteTool;

// Tests below that hardcode unix paths (/etc/shadow, /home/...) are
// gated to cfg(unix). They exercise unix-specific path validation
// semantics — Windows-equivalents would need C:\Windows\System32\
// config\SAM, %USERPROFILE%\.ssh\... and are out of scope here.
// Sweep finding from .blackboard/WINDOWS-SWEEP.md.
#[cfg(unix)]
#[tokio::test]
async fn read_legacy_refuses_etc_shadow() {
    let tool = ReadTool::new(None);
    let result = tool.execute(json!({ "file_path": "/etc/shadow" })).await;
    assert!(
        result.is_error,
        "must refuse /etc/shadow: {}",
        result.content
    );
    assert!(
        result.content.contains("Refused"),
        "expected refusal message, got: {}",
        result.content
    );
}

#[tokio::test]
async fn write_legacy_refuses_traversal() {
    let tool = WriteTool::new(None);
    let result = tool
        .execute(json!({
            "file_path": "/tmp/../etc/shadow",
            "content": "hostile",
        }))
        .await;
    assert!(
        result.is_error,
        "traversal must be refused: {}",
        result.content
    );
    assert!(result.content.contains("Refused"));
}

#[tokio::test]
async fn write_legacy_refuses_relative_path() {
    let tool = WriteTool::new(None);
    let result = tool
        .execute(json!({
            "file_path": "relative.txt",
            "content": "x",
        }))
        .await;
    assert!(result.is_error);
    assert!(result.content.contains("Refused"));
}

#[cfg(unix)]
#[tokio::test]
async fn edit_legacy_refuses_ssh_key_path() {
    let tool = EditTool::new(None);
    let result = tool
        .execute(json!({
            "file_path": "/home/alice/.ssh/id_rsa",
            "old_string": "x",
            "new_string": "y",
        }))
        .await;
    assert!(result.is_error);
    assert!(result.content.contains("Refused"));
}

#[tokio::test]
async fn write_legacy_succeeds_for_ordinary_absolute_path() {
    let dir = tempdir().expect("tempdir");
    let target = dir.path().join("ok.txt");

    let tool = WriteTool::new(None);
    let result = tool
        .execute(json!({
            "file_path": target.to_str().unwrap(),
            "content": "hello",
        }))
        .await;
    assert!(
        !result.is_error,
        "valid absolute path must succeed: {}",
        result.content
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
}

#[tokio::test]
async fn read_legacy_succeeds_for_ordinary_absolute_path() {
    let dir = tempdir().expect("tempdir");
    let target = dir.path().join("ok.txt");
    std::fs::write(&target, b"content").unwrap();

    let tool = ReadTool::new(None);
    let result = tool
        .execute(json!({ "file_path": target.to_str().unwrap() }))
        .await;
    assert!(
        !result.is_error,
        "valid absolute path must succeed: {}",
        result.content
    );
    assert!(result.content.contains("content"));
}

#[cfg(unix)]
#[tokio::test]
async fn read_ctx_variant_also_refuses_etc_shadow() {
    // The ctx variant must apply the same shape check so a top-level
    // (non-sandboxed) ctx can't bypass the discipline.
    let tool = ReadTool::new(None);
    let ctx = wcore_tools::context::ToolContext::test_default();
    let result = tool
        .execute_with_ctx(json!({ "file_path": "/etc/shadow" }), &ctx)
        .await;
    assert!(result.is_error, "ctx variant must also refuse");
    assert!(result.content.contains("Refused"));
}
