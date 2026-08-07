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

/// Credential disclosure via procfs: `/proc/self/environ` is the agent's OWN
/// environment (every provider API key) and is a regular file, so it slipped
/// past every other guard. On Linux this is a REAL file, so this is genuinely
/// end-to-end there. `cfg(unix)` for the same reason as the `/etc/shadow`
/// tests above — on Windows the path is not absolute and would be refused for
/// the wrong reason.
///
/// HOW THIS FAILS IF THE DEFECT RETURNS: delete the
/// `if is_denied_proc_path(path) { return true; }` block in
/// `is_denied_system_path` (`crates/wcore-tools/src/path_validation.rs`) and
/// `ReadTool::execute` returns the environment block instead of a refusal.
#[cfg(unix)]
#[tokio::test]
async fn read_legacy_refuses_proc_self_environ() {
    let tool = ReadTool::new(None);
    let result = tool
        .execute(json!({ "file_path": "/proc/self/environ" }))
        .await;
    assert!(
        result.is_error,
        "must refuse /proc/self/environ: {}",
        result.content
    );
    assert!(
        result.content.contains("Refused"),
        "expected refusal message, got: {}",
        result.content
    );
}

/// The ctx entry is the one the ENGINE actually uses, so a fix pinned only to
/// the legacy `execute()` path would be worthless. Drives `execute_with_ctx`
/// against the same procfs target, plus a per-pid spelling to prove the deny
/// is not keyed on the literal `self`.
///
/// HOW THIS FAILS IF THE DEFECT RETURNS: delete the
/// `if is_denied_proc_path(path) { return true; }` block in
/// `is_denied_system_path` (`crates/wcore-tools/src/path_validation.rs`), or
/// drop the `is_ascii_digit` pid arm from `is_denied_proc_path`, and
/// `ReadTool::execute_with_ctx` stops refusing.
#[cfg(unix)]
#[tokio::test]
async fn read_ctx_variant_also_refuses_proc_environ() {
    let tool = ReadTool::new(None);
    let ctx = wcore_tools::context::ToolContext::test_default();
    for p in ["/proc/self/environ", "/proc/1/environ", "/proc/self/mem"] {
        let result = tool.execute_with_ctx(json!({ "file_path": p }), &ctx).await;
        assert!(
            result.is_error,
            "ctx variant must refuse {p}: {}",
            result.content
        );
        assert!(
            result.content.contains("Refused"),
            "expected refusal for {p}, got: {}",
            result.content
        );
    }
}

/// Over-match guard on the tool surface, through the ctx entry: a real
/// workspace file literally named `proc/self/environ` must still be READABLE.
/// A `contains()`-style deny would pass the two tests above while silently
/// breaking legitimate reads.
///
/// HOW THIS FAILS IF THE DEFECT RETURNS: replace the component walk in
/// `is_denied_proc_path` (`crates/wcore-tools/src/path_validation.rs`) with a
/// substring test and this read starts returning a refusal.
#[cfg(unix)]
#[tokio::test]
async fn read_ctx_variant_allows_lookalike_proc_path() {
    let dir = tempdir().expect("tempdir");
    let target = dir.path().join("proc/self/environ");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"workspace notes").unwrap();

    let tool = ReadTool::new(None);
    let ctx = wcore_tools::context::ToolContext::test_default();
    let result = tool
        .execute_with_ctx(json!({ "file_path": target.to_str().unwrap() }), &ctx)
        .await;
    assert!(
        !result.is_error,
        "a workspace file named proc/self/environ must stay readable: {}",
        result.content
    );
    assert!(
        result.content.contains("workspace notes"),
        "expected file contents, got: {}",
        result.content
    );
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
