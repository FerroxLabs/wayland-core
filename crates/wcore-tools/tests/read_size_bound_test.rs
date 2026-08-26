//! `Read` must bound what it pulls into memory (FerroxLabs/wayland#946).
//!
//! MEASURED on the pre-fix tree, on a 125,829,120-byte (120 MiB) text file:
//! `ReadTool::execute` returned `is_error=false` and a 137,412,032-byte
//! `ToolResult`, driving process peak RSS from 5,344 kB to 469,248 kB — a
//! 453 MiB delta, ~3.8x the file. A modestly larger file exhausts the process.
//!
//! Also measured, and the reason the refusal message does NOT point at
//! `offset`/`limit`: `{"offset":0,"limit":1}` on that same file returns 86
//! bytes of content but still takes peak RSS from 4,872 kB to 151,296 kB.
//! Those parameters select lines *after* the whole file has been materialised,
//! so they are not a large-file escape hatch.

use serde_json::json;
use std::io::Write;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::read::{READ_MAX_BYTES, ReadTool};

/// A file whose *metadata length* is `len`, created without writing `len`
/// bytes. The bound is enforced from a stat, so a sparse file exercises it
/// exactly as a dense one would — and a test that had to write 25 MiB four
/// times over would be paying for nothing.
fn sparse_file(dir: &std::path::Path, name: &str, len: u64) -> std::path::PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(len).unwrap();
    path
}

#[tokio::test]
async fn refuses_a_file_over_the_read_limit() {
    let dir = tempfile::tempdir().unwrap();
    let over = READ_MAX_BYTES + 1;
    let path = sparse_file(dir.path(), "over.txt", over);

    let result = ReadTool::new(None)
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;

    assert!(result.is_error, "an oversized read must refuse: {result:?}");
    // Actionable, not silent: the actual size, the limit, and a route that
    // actually works on a file this size.
    assert!(
        result.content.contains(&over.to_string()),
        "message must name the actual size: {}",
        result.content
    );
    assert!(
        result.content.contains(&READ_MAX_BYTES.to_string()),
        "message must name the limit: {}",
        result.content
    );
    assert!(
        result.content.contains("Bash") && result.content.contains("Grep"),
        "message must name a route that works on a file this size: {}",
        result.content
    );
    // The content must NOT be truncated-and-returned as if it were the file.
    assert!(
        !result.content.contains('\0'),
        "refusal must not carry file bytes"
    );
}

/// The same bound on the vfs entry point. `execute_with_ctx` is a second,
/// independent production read call site (`ctx.vfs.read`), and the dispatcher
/// routes through it — a guard on `execute` alone would leave the real path
/// unbounded.
#[tokio::test]
async fn refuses_a_file_over_the_read_limit_through_the_vfs_entry() {
    let dir = tempfile::tempdir().unwrap();
    let over = READ_MAX_BYTES + 1;
    let path = sparse_file(dir.path(), "over.txt", over);

    let ctx = ToolContext::test_default();
    let result = ReadTool::new(None)
        .execute_with_ctx(json!({ "file_path": path.to_str().unwrap() }), &ctx)
        .await;

    assert!(result.is_error, "an oversized read must refuse: {result:?}");
    assert!(result.content.contains(&over.to_string()));
    assert!(result.content.contains(&READ_MAX_BYTES.to_string()));
}

/// `offset`/`limit` must not be a way around the bound, because they do not
/// reduce what the tool allocates (see the module note). A ranged read of an
/// oversized file is refused exactly like a full one.
#[tokio::test]
async fn a_ranged_read_does_not_bypass_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = sparse_file(dir.path(), "over.txt", READ_MAX_BYTES + 1);

    let result = ReadTool::new(None)
        .execute(json!({
            "file_path": path.to_str().unwrap(),
            "offset": 0,
            "limit": 1
        }))
        .await;

    assert!(result.is_error, "ranged read must refuse too: {result:?}");
}

/// Boundary: exactly at the limit is admitted. The bound protects the process;
/// it does not shrink the tool's advertised reach by one byte.
#[tokio::test]
async fn admits_a_file_exactly_at_the_read_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = sparse_file(dir.path(), "at.txt", READ_MAX_BYTES);

    let result = ReadTool::new(None)
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;

    assert!(
        !result.is_error,
        "at the limit must be admitted: {result:?}"
    );
    assert!(
        !result.content.starts_with("Refused to read"),
        "at the limit must not be refused: {}",
        result.content
    );
}

/// An ordinary file is untouched by any of this.
#[tokio::test]
async fn an_ordinary_file_still_reads_in_full() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.txt");
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, "alpha").unwrap();
    writeln!(file, "beta").unwrap();
    drop(file);

    let result = ReadTool::new(None)
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;

    assert!(!result.is_error, "{result:?}");
    assert!(result.content.contains("1\talpha"));
    assert!(result.content.contains("2\tbeta"));
}
