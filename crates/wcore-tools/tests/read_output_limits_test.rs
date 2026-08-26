//! `ReadTool` must honour `tool_output_limits` (FerroxLabs/wayland#947).
//!
//! `tool_output_limits` documents `max_lines` as the "read_file pagination +
//! truncation cap" and `max_line_length` as the file-ops per-line cap, and
//! `ReadTool` consulted neither: it numbered every line of the file and
//! returned the join. The only thing standing between that and the model was
//! `orchestration::truncate_result`, a head/tail BYTE cut at
//! `max_result_size()` that slices mid-line and deletes the middle of the
//! file without preserving line numbering.
//!
//! These tests pin the cap, the per-line clamp, its disclosure, and — the part
//! that is easy to get wrong — that `offset`/`limit` keep working across it.
//! `offset`/`limit` are LINE-based and applied after the file is materialised,
//! so they were never a way to bypass the cap; the cap must not become a way
//! to bypass them either.

use std::io::Write;
use std::sync::Arc;

use serde_json::json;

use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::read::ReadTool;
use wcore_tools::tool_output_limits::{DEFAULT_MAX_LINE_LENGTH, DEFAULT_MAX_LINES};
use wcore_tools::vfs::{RealFs, SandboxedFs};

/// Lines well past `DEFAULT_MAX_LINES`, each wide enough that an uncapped
/// result is measurably large.
const HUGE_LINES: usize = 60_000;
const LINE_WIDTH: usize = 200;

fn sandboxed_ctx(root: &std::path::Path) -> ToolContext {
    ToolContext::new(
        String::new(),
        tokio_util::sync::CancellationToken::new(),
        Arc::new(SandboxedFs::new(RealFs, root.to_path_buf())),
        None,
        Arc::new(wcore_tools::NullToolOutputSink),
    )
}

/// Write `n` lines of `width` printable bytes each. Returns the path.
fn write_wide_file(dir: &std::path::Path, name: &str, n: usize, width: usize) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::io::BufWriter::new(std::fs::File::create(&path).expect("create"));
    let filler = "x".repeat(width);
    for i in 1..=n {
        writeln!(f, "{i} {filler}").expect("write");
    }
    f.flush().expect("flush");
    path
}

/// Numbered body lines only — the trailing truncation notice is not one.
fn body_lines(content: &str) -> Vec<&str> {
    content
        .lines()
        .filter(|l| !l.starts_with("... [truncated"))
        .collect()
}

/// RED ARM: on the unbounded tree this returns the whole file — measured at
/// 60_000 lines x 200 bytes, a ToolResult of ~12.7 MB. The size is printed so
/// a `--nocapture` run records the before/after figure directly.
#[tokio::test]
async fn full_read_of_huge_file_is_line_capped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_wide_file(dir.path(), "huge.txt", HUGE_LINES, LINE_WIDTH);
    let on_disk = std::fs::metadata(&path).expect("stat").len();

    let tool = ReadTool::new(None);
    let result = tool
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;

    eprintln!(
        "MEASURED file_bytes={} tool_result_bytes={} body_lines={}",
        on_disk,
        result.content.len(),
        body_lines(&result.content).len()
    );

    assert!(!result.is_error, "read should succeed: {}", result.content);
    assert_eq!(
        body_lines(&result.content).len(),
        DEFAULT_MAX_LINES,
        "Read must cap the returned window at max_lines"
    );
    // Each numbered line is ~210 bytes, so the capped result cannot exceed a
    // small multiple of max_lines * line width. Guards against a cap that
    // counts lines but still emits the whole file some other way.
    assert!(
        result.content.len() < on_disk as usize / 10,
        "capped result ({} bytes) should be far smaller than the file ({on_disk} bytes)",
        result.content.len()
    );
}

/// The cap must be DISCLOSED. A model that cannot tell a 2000-line file from
/// the first 2000 lines of a 60000-line one will reason about a tail it never
/// received.
#[tokio::test]
async fn line_cap_is_disclosed_with_a_continuation_offset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_wide_file(dir.path(), "huge.txt", HUGE_LINES, 20);

    let tool = ReadTool::new(None);
    let result = tool
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;

    let notice = result
        .content
        .lines()
        .find(|l| l.starts_with("... [truncated"))
        .unwrap_or_else(|| panic!("no truncation notice in result"));

    assert!(
        notice.contains(&format!("showing {DEFAULT_MAX_LINES} of {HUGE_LINES} lines")),
        "notice must name both the shown and the available line counts: {notice}"
    );
    assert!(
        notice.contains(&format!("offset={DEFAULT_MAX_LINES}")),
        "notice must name the offset that continues the read: {notice}"
    );
}

/// A line longer than `max_line_length` is clamped, and the clamp is visible.
#[tokio::test]
async fn over_long_line_is_clamped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("minified.js");
    let long = "a".repeat(DEFAULT_MAX_LINE_LENGTH * 4);
    std::fs::write(&path, format!("short\n{long}\nshort\n")).expect("write");

    let tool = ReadTool::new(None);
    let result = tool
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;

    let lines = body_lines(&result.content);
    assert_eq!(lines.len(), 3);
    assert!(
        lines[1].ends_with("... [truncated]"),
        "over-long line must carry the clamp marker"
    );
    assert!(
        lines[1].len() < DEFAULT_MAX_LINE_LENGTH + 64,
        "clamped line still {} bytes",
        lines[1].len()
    );
    // Clamping must not disturb the neighbours or the numbering.
    assert!(lines[0].contains("1\tshort"));
    assert!(lines[2].contains("3\tshort"));
}

/// A multi-byte line must be clamped on a char boundary, not split mid-codepoint.
#[tokio::test]
async fn over_long_multibyte_line_is_clamped_on_a_char_boundary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("cjk.txt");
    // 3 bytes per char, so the byte cap lands mid-codepoint unless snapped.
    std::fs::write(&path, "\u{4f60}".repeat(DEFAULT_MAX_LINE_LENGTH)).expect("write");

    let tool = ReadTool::new(None);
    let result = tool
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;

    assert!(!result.is_error);
    assert!(result.content.ends_with("... [truncated]"));
}

/// An explicit `limit` NARROWER than the cap still wins — the cap is a
/// ceiling, not a floor.
#[tokio::test]
async fn explicit_limit_below_the_cap_is_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_wide_file(dir.path(), "huge.txt", HUGE_LINES, 20);

    let tool = ReadTool::new(None);
    let result = tool
        .execute(json!({ "file_path": path.to_str().unwrap(), "offset": 0, "limit": 5 }))
        .await;

    let lines = body_lines(&result.content);
    assert_eq!(lines.len(), 5, "explicit limit must not be widened to the cap");
    assert!(
        !result.content.contains("... [truncated"),
        "a fully-satisfied request must not claim truncation"
    );
}

/// `offset` still pages, and the offset the notice hands out is the RIGHT one:
/// re-reading at it resumes exactly where the capped window stopped, with no
/// gap and no repeat.
#[tokio::test]
async fn offset_resumes_exactly_where_the_cap_stopped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_wide_file(dir.path(), "huge.txt", HUGE_LINES, 20);
    let tool = ReadTool::new(None);

    let first = tool
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;
    let first_lines = body_lines(&first.content);
    let last_of_first = *first_lines.last().expect("non-empty");

    let second = tool
        .execute(json!({ "file_path": path.to_str().unwrap(), "offset": DEFAULT_MAX_LINES }))
        .await;
    let second_lines = body_lines(&second.content);
    let first_of_second = second_lines[0];

    assert!(
        last_of_first.starts_with(&format!("{:>6}\t", DEFAULT_MAX_LINES)),
        "first window should end at line {DEFAULT_MAX_LINES}, got {last_of_first}"
    );
    assert!(
        first_of_second.starts_with(&format!("{:>6}\t", DEFAULT_MAX_LINES + 1)),
        "second window should start at line {}, got {first_of_second}",
        DEFAULT_MAX_LINES + 1
    );
    assert_eq!(
        second_lines.len(),
        DEFAULT_MAX_LINES,
        "the cap applies to every page, not just the first"
    );
}

/// An explicit `limit` WIDER than the cap is clamped to the cap — `limit` was
/// never a bypass (it selects lines only after the whole file is in memory)
/// and must not become one.
#[tokio::test]
async fn explicit_limit_above_the_cap_is_clamped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_wide_file(dir.path(), "huge.txt", HUGE_LINES, 20);

    let tool = ReadTool::new(None);
    let result = tool
        .execute(json!({ "file_path": path.to_str().unwrap(), "offset": 10, "limit": 50_000 }))
        .await;

    let lines = body_lines(&result.content);
    assert_eq!(lines.len(), DEFAULT_MAX_LINES);
    assert!(lines[0].starts_with(&format!("{:>6}\t", 11)));
    assert!(
        result
            .content
            .contains(&format!("showing {DEFAULT_MAX_LINES} of 50000 lines")),
        "notice must report the REQUESTED window, not the whole file: {}",
        result.content.lines().last().unwrap_or_default()
    );
}

/// The vfs entry point (`execute_with_ctx`) is a SEPARATE call site with its
/// own copy of the windowing code. Capping one and not the other would leave
/// every sandboxed sub-agent read unbounded.
#[tokio::test]
async fn vfs_entry_point_is_capped_too() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_wide_file(dir.path(), "huge.txt", HUGE_LINES, LINE_WIDTH);
    let on_disk = std::fs::metadata(&path).expect("stat").len();
    let ctx = sandboxed_ctx(dir.path());

    let tool = ReadTool::new(None);
    let result = tool
        .execute_with_ctx(json!({ "file_path": path.to_str().unwrap() }), &ctx)
        .await;

    eprintln!(
        "MEASURED vfs file_bytes={} tool_result_bytes={}",
        on_disk,
        result.content.len()
    );

    assert!(!result.is_error, "read should succeed: {}", result.content);
    assert_eq!(body_lines(&result.content).len(), DEFAULT_MAX_LINES);
    assert!(result.content.contains("... [truncated"));
}

/// A file comfortably under both caps must come back byte-identical to the
/// pre-cap behaviour: no notice, no clamp marker, every line present.
///
/// This is the NEGATIVE CONTROL for the cap: it stays green under the IJFW
/// mutation and goes red only if the cap starts firing where it must not.
#[tokio::test]
async fn small_file_is_unchanged_by_the_caps() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_wide_file(dir.path(), "small.txt", 200, 40);

    let tool = ReadTool::new(None);
    let result = tool
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;

    let lines: Vec<&str> = result.content.lines().collect();
    assert_eq!(lines.len(), 200);
    assert!(!result.content.contains("[truncated"));
    assert!(lines[0].contains("1\t1 "));
    assert!(lines[199].contains("200\t200 "));
}
