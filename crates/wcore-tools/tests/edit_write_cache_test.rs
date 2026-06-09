//! Integration tests for EditTool / WriteTool file-state cache integration
//! (TC-5.4 and TC-5.4-W series).
//!
//! Black-box tests: exercise Edit/Write tools through their public API with
//! a real filesystem and shared FileStateCache, validating "must Read first"
//! guard, staleness detection, and post-write cache updates.

use std::path::Path;
use std::sync::{Arc, RwLock};

use serde_json::json;

use wcore_config::file_cache::FileCacheConfig;
use wcore_tools::Tool;
use wcore_tools::edit::EditTool;
use wcore_tools::file_cache::{FileStateCache, file_mtime_ms};
use wcore_tools::read::ReadTool;
use wcore_tools::write::WriteTool;

fn make_cache() -> Arc<RwLock<FileStateCache>> {
    let config = FileCacheConfig {
        max_entries: 100,
        max_size_bytes: 25 * 1024 * 1024,
        enabled: true,
    };
    Arc::new(RwLock::new(FileStateCache::new(&config)))
}

/// Populate cache by actually reading the file through ReadTool.
async fn read_file(tool: &ReadTool, path: &Path) {
    let input = json!({ "file_path": path.to_str().unwrap() });
    let r = tool.execute(input).await;
    assert!(!r.is_error, "read failed: {}", r.content);
}

const UNCHANGED_MARKER: &str = "File unchanged since last read";

// ==========================================================================
// TC-5.4: EditTool guard and staleness detection
// ==========================================================================

/// TC-5.4-01: Normal Read → Edit succeeds.
#[tokio::test]
async fn tc_5_4_01_read_then_edit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("normal.txt");
    std::fs::write(&file, "hello world").unwrap();

    let cache = make_cache();
    let read_tool = ReadTool::new(Some(cache.clone()));
    let edit_tool = EditTool::new(Some(cache));

    read_file(&read_tool, &file).await;

    let input = json!({
        "file_path": file.to_str().unwrap(),
        "old_string": "hello",
        "new_string": "goodbye"
    });
    let result = edit_tool.execute(input).await;

    assert!(
        !result.is_error,
        "Edit after Read should succeed: {}",
        result.content
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "goodbye world");
}

/// TC-5.4-02: Edit without prior Read returns "must Read first" error.
#[tokio::test]
async fn tc_5_4_02_edit_without_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("no_read.txt");
    std::fs::write(&file, "content").unwrap();

    let cache = make_cache();
    let edit_tool = EditTool::new(Some(cache));

    let input = json!({
        "file_path": file.to_str().unwrap(),
        "old_string": "content",
        "new_string": "new"
    });
    let result = edit_tool.execute(input).await;

    assert!(result.is_error, "Edit without Read should fail");
    assert!(
        result.content.contains("must Read"),
        "Error should mention 'must Read': {}",
        result.content
    );
    // File must be unchanged.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "content");
}

/// TC-5.4-03: External modification after Read triggers staleness error.
#[tokio::test]
async fn tc_5_4_03_external_modification_detected() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("stale.txt");
    std::fs::write(&file, "original content").unwrap();

    let cache = make_cache();
    let read_tool = ReadTool::new(Some(cache.clone()));
    let edit_tool = EditTool::new(Some(cache));

    read_file(&read_tool, &file).await;

    // External modification.
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&file, "externally changed").unwrap();

    let input = json!({
        "file_path": file.to_str().unwrap(),
        "old_string": "original content",
        "new_string": "new"
    });
    let result = edit_tool.execute(input).await;

    assert!(
        result.is_error,
        "Edit of externally modified file should fail"
    );
    assert!(
        result.content.contains("modified externally"),
        "Error should mention external modification: {}",
        result.content
    );
}

/// TC-5.4-04: Edit → Edit succeeds because first Edit updates the cache.
#[tokio::test]
async fn tc_5_4_04_edit_then_edit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("double.txt");
    std::fs::write(&file, "aaa bbb ccc").unwrap();

    let cache = make_cache();
    let read_tool = ReadTool::new(Some(cache.clone()));
    let edit_tool = EditTool::new(Some(cache));

    read_file(&read_tool, &file).await;

    // First edit.
    let input1 = json!({
        "file_path": file.to_str().unwrap(),
        "old_string": "aaa",
        "new_string": "AAA"
    });
    let r1 = edit_tool.execute(input1).await;
    assert!(!r1.is_error, "First edit failed: {}", r1.content);

    // Second edit — should work because first edit updated cache mtime.
    let input2 = json!({
        "file_path": file.to_str().unwrap(),
        "old_string": "bbb",
        "new_string": "BBB"
    });
    let r2 = edit_tool.execute(input2).await;
    assert!(!r2.is_error, "Second edit failed: {}", r2.content);

    assert_eq!(std::fs::read_to_string(&file).unwrap(), "AAA BBB ccc");
}

/// TC-5.4-05: With cache disabled (None), Edit works without prior Read.
#[tokio::test]
async fn tc_5_4_05_no_cache_edit_bypasses_guard() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("nocache.txt");
    std::fs::write(&file, "hello").unwrap();

    let edit_tool = EditTool::new(None);

    let input = json!({
        "file_path": file.to_str().unwrap(),
        "old_string": "hello",
        "new_string": "bye"
    });
    let result = edit_tool.execute(input).await;

    assert!(
        !result.is_error,
        "Edit without cache should succeed: {}",
        result.content
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "bye");
}

/// TC-5.4-06: replace_all updates cache mtime correctly.
#[tokio::test]
async fn tc_5_4_06_replace_all_updates_cache() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("replaceall.txt");
    std::fs::write(&file, "x-x-x-x").unwrap();

    let cache = make_cache();
    let read_tool = ReadTool::new(Some(cache.clone()));
    let edit_tool = EditTool::new(Some(cache.clone()));

    read_file(&read_tool, &file).await;

    let input = json!({
        "file_path": file.to_str().unwrap(),
        "old_string": "x",
        "new_string": "y",
        "replace_all": true
    });
    let result = edit_tool.execute(input).await;
    assert!(!result.is_error, "replace_all failed: {}", result.content);

    assert_eq!(std::fs::read_to_string(&file).unwrap(), "y-y-y-y");

    // Verify cache mtime matches disk.
    let disk_mtime = file_mtime_ms(&file).unwrap();
    let mut c = cache.write().unwrap();
    let cached = c.get(&file).expect("file should be in cache");
    assert_eq!(cached.mtime_ms, disk_mtime);
}

// ==========================================================================
// TC-5.4-W: WriteTool cache update
// ==========================================================================

/// TC-5.4-W01: Write then Read returns the full current content, NOT the
/// "unchanged since last read" stub. A Write populates the cache as a
/// `WriteEcho` (post-write disk state the model has not seen as a read), so the
/// stub — which tells the model to "refer to the earlier Read tool_result" —
/// would point at a Read that never happened. The Read must return real content.
#[tokio::test]
async fn tc_5_4_w01_write_then_read_returns_content() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("write_read.txt");

    let cache = make_cache();
    let write_tool = WriteTool::new(Some(cache.clone()));
    let read_tool = ReadTool::new(Some(cache));

    // Write creates file and populates cache (as WriteEcho).
    let write_input = json!({
        "file_path": file.to_str().unwrap(),
        "content": "written content"
    });
    let wr = write_tool.execute(write_input).await;
    assert!(!wr.is_error, "write failed: {}", wr.content);

    // Read immediately after: must return the actual content, not a stub that
    // references a nonexistent earlier Read.
    let read_input = json!({ "file_path": file.to_str().unwrap() });
    let rr = read_tool.execute(read_input).await;
    assert!(!rr.is_error);
    assert!(
        !rr.content.contains(UNCHANGED_MARKER),
        "Read after Write must not emit the unchanged stub, got: {}",
        rr.content
    );
    assert!(
        rr.content.contains("written content"),
        "Read after Write must return the real content, got: {}",
        rr.content
    );
}

/// TC-5.4-W02: Write then Edit succeeds (Write populates cache for Edit guard).
#[tokio::test]
async fn tc_5_4_w02_write_then_edit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("write_edit.txt");

    let cache = make_cache();
    let write_tool = WriteTool::new(Some(cache.clone()));
    let edit_tool = EditTool::new(Some(cache));

    let write_input = json!({
        "file_path": file.to_str().unwrap(),
        "content": "hello world"
    });
    let wr = write_tool.execute(write_input).await;
    assert!(!wr.is_error, "write failed: {}", wr.content);

    let edit_input = json!({
        "file_path": file.to_str().unwrap(),
        "old_string": "hello",
        "new_string": "goodbye"
    });
    let er = edit_tool.execute(edit_input).await;
    assert!(
        !er.is_error,
        "Edit after Write should succeed: {}",
        er.content
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "goodbye world");
}

/// TC-5.4-W03: Write → Write → Read returns the fresh content, not a stub.
/// Both writes cache `WriteEcho` entries, so the Read must materialize the
/// current on-disk content (version 2) rather than dedup against content the
/// model never saw as a read.
#[tokio::test]
async fn tc_5_4_w03_write_overwrite_then_read() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("overwrite.txt");

    let cache = make_cache();
    let write_tool = WriteTool::new(Some(cache.clone()));
    let read_tool = ReadTool::new(Some(cache));

    // First write.
    let w1 = json!({
        "file_path": file.to_str().unwrap(),
        "content": "version 1"
    });
    write_tool.execute(w1).await;

    // Brief delay to change mtime.
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Second write (overwrite).
    let w2 = json!({
        "file_path": file.to_str().unwrap(),
        "content": "version 2"
    });
    write_tool.execute(w2).await;

    // Read: the cache entry is a WriteEcho, so the Read returns the real
    // current content instead of a misleading unchanged stub.
    let read_input = json!({ "file_path": file.to_str().unwrap() });
    let rr = read_tool.execute(read_input).await;
    assert!(!rr.is_error);
    assert!(
        !rr.content.contains(UNCHANGED_MARKER),
        "Read after second Write must not dedup against unseen content, got: {}",
        rr.content
    );
    assert!(
        rr.content.contains("version 2"),
        "Read must return the current content, got: {}",
        rr.content
    );

    // Verify disk has version 2.
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "version 2");
}

// ==========================================================================
// Supplementary: Cross-tool interaction tests
// ==========================================================================

/// Read → Edit → Read must return the POST-edit content, not the unchanged
/// stub. This is the load-bearing correctness case: the stub tells the model
/// "the earlier Read is still current — refer to that", but the earlier Read is
/// the PRE-edit content (`alpha beta`). After the edit the file is `ALPHA beta`,
/// and a verify-read exists precisely to confirm that. The WriteEcho provenance
/// guard forces the real current content.
#[tokio::test]
async fn read_edit_read_returns_post_edit_content() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cross.txt");
    std::fs::write(&file, "alpha beta").unwrap();

    let cache = make_cache();
    let read_tool = ReadTool::new(Some(cache.clone()));
    let edit_tool = EditTool::new(Some(cache));

    // Read (model sees `alpha beta`).
    read_file(&read_tool, &file).await;

    // Edit.
    let edit_input = json!({
        "file_path": file.to_str().unwrap(),
        "old_string": "alpha",
        "new_string": "ALPHA"
    });
    let er = edit_tool.execute(edit_input).await;
    assert!(!er.is_error);

    // Read again: must surface the post-edit content, NOT a stub pointing at the
    // stale pre-edit Read.
    let read_input = json!({ "file_path": file.to_str().unwrap() });
    let rr = read_tool.execute(read_input).await;
    assert!(!rr.is_error);
    assert!(
        !rr.content.contains(UNCHANGED_MARKER),
        "Read after Edit must not point the model at stale pre-edit content, got: {}",
        rr.content
    );
    assert!(
        rr.content.contains("ALPHA beta"),
        "Read after Edit must return the post-edit content, got: {}",
        rr.content
    );
}
