//! REFUTATION + regression guard: the "pseudo-file defeats the size bound"
//! residual on `READ_MAX_BYTES` is not reachable through `Read`.
//!
//! The claim under test was: `READ_MAX_BYTES` is enforced on
//! `metadata().len()`, and procfs / sysfs / devices / FIFOs report 0 and then
//! stream unbounded, so the guard passes trivially and the read is not
//! bounded at all.
//!
//! The premise about stat is TRUE and is measured below. The conclusion is
//! FALSE: none of those sources ever reaches the size guard, because
//! `validate_user_path` runs first at BOTH entry points and refuses them.
//!
//! Polarity, stated so this is not graded backwards: the file-type rule in
//! `validate_user_path` is an ALLOWLIST —
//! `if !ft.is_file() && !ft.is_dir() { reject }` — so a FIFO, a character or
//! block device, and a socket are all refused, and only regular files and
//! directories get through. `is_denied_proc_path` then denies the per-process
//! procfs subtree and `/proc/kcore` by location. Both rules predate the size
//! bound (#644), and both cite this exact hazard by name: the comment on the
//! file-type rule reads "`/dev/zero` reports a metadata length of 0 then
//! streams unbounded into `fs::read` (OOM); a FIFO with no writer blocks the
//! read forever (DoS)."
//!
//! This file exists so that stays true. The refusals are what makes the
//! stat-based bound sufficient, so anything that weakens them silently
//! re-opens a hole the size guard cannot catch.
//!
//! Procfs refusal is already covered at this level by
//! `legacy_execute_path_validation_test.rs`; this file covers the file-TYPE
//! rule, which was only unit-tested inside `path_validation`.

#![cfg(unix)]

use std::io::Write as _;
use std::sync::Arc;

use serde_json::json;

use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::read::{READ_MAX_BYTES, ReadTool};
use wcore_tools::vfs::RealFs;

/// Bytes the FIFO writer offers: past the limit, but bounded so that a
/// regression fails the test rather than exhausting the host. `/dev/zero`
/// below is the unbounded version of the same class and is why the refusal
/// matters.
const OVERRUN: u64 = READ_MAX_BYTES + (4 << 20);

fn realfs_ctx() -> ToolContext {
    ToolContext::new(
        String::new(),
        tokio_util::sync::CancellationToken::new(),
        Arc::new(RealFs),
        None,
        Arc::new(wcore_tools::NullToolOutputSink),
    )
}

/// A FIFO with a detached writer offering `OVERRUN` bytes. The writer ignores
/// every error: once the reader goes away the remaining writes get EPIPE,
/// which is the correct end of that thread.
fn fifo_streaming_past_the_limit(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("streamer.fifo");
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("cstring");
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives the call.
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    assert_eq!(rc, 0, "mkfifo failed: {}", std::io::Error::last_os_error());

    let writer_path = path.clone();
    std::thread::spawn(move || {
        // Opening for write blocks until a reader opens the other end. If the
        // refusal lands before any open, this thread simply never proceeds.
        let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&writer_path) else {
            return;
        };
        let chunk = vec![b'z'; 64 * 1024];
        let mut sent: u64 = 0;
        while sent < OVERRUN && f.write_all(&chunk).is_ok() {
            sent += chunk.len() as u64;
        }
    });
    path
}

/// The PREMISE, measured rather than recalled: these sources really do report
/// a length of 0. Any bound that trusted `metadata().len()` would indeed be
/// bounding a number the source made up — which is why the refusals below,
/// not the size guard, are what actually holds.
#[test]
fn pseudo_sources_really_do_report_a_length_of_zero() {
    let dev_zero = std::fs::metadata("/dev/zero").expect("stat /dev/zero").len();
    let proc_maps = std::fs::metadata("/proc/self/maps")
        .expect("stat /proc/self/maps")
        .len();
    let real_maps = std::fs::read("/proc/self/maps").expect("read").len();
    eprintln!(
        "MEASURED premise: /dev/zero stat_len={dev_zero}, /proc/self/maps stat_len={proc_maps} \
         real_len={real_maps}"
    );
    assert_eq!(dev_zero, 0, "premise: a character device stats as 0 bytes");
    assert_eq!(proc_maps, 0, "premise: procfs stats as 0 bytes");
    assert!(
        real_maps > proc_maps as usize,
        "premise: procfs yields more than it claims"
    );
}

/// A FIFO stats as 0 and streams past the limit. It must be refused, and
/// refused as a FILE-TYPE violation — before any read is attempted, so the
/// size guard's blind spot is never reached.
#[tokio::test]
async fn a_fifo_is_refused_before_the_size_guard_can_be_defeated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = fifo_streaming_past_the_limit(dir.path());
    let stat_len = std::fs::metadata(&path).expect("stat").len();
    assert_eq!(stat_len, 0, "premise: a FIFO stats as 0 bytes");

    let tool = ReadTool::new(None);
    let legacy = tool
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;
    let vfs = tool
        .execute_with_ctx(
            json!({ "file_path": path.to_str().unwrap() }),
            &realfs_ctx(),
        )
        .await;

    eprintln!(
        "MEASURED fifo stat_len={stat_len} offered={OVERRUN} legacy_bytes={} vfs_bytes={}",
        legacy.content.len(),
        vfs.content.len()
    );

    for (label, r) in [("legacy", &legacy), ("vfs", &vfs)] {
        assert!(
            r.is_error,
            "{label}: a FIFO streaming {OVERRUN} bytes was ACCEPTED — the file-type \
             allowlist has been weakened, and the size guard cannot catch this \
             because the FIFO stats as 0. returned {} bytes",
            r.content.len()
        );
        assert!(
            r.content.contains("not a regular file"),
            "{label}: must be refused by the file-TYPE rule, not incidentally by \
             something else that could be relaxed independently: {}",
            r.content
        );
        assert!(
            (r.content.len() as u64) < READ_MAX_BYTES,
            "{label}: the refusal must be small, not the stream it refused"
        );
    }
}

/// `/dev/zero` is the unbounded member of the class: it stats as 0 and never
/// ends. This is the one that would take the host with it, so the refusal is
/// load-bearing rather than merely tidy.
#[tokio::test]
async fn dev_zero_is_refused_at_both_entry_points() {
    let tool = ReadTool::new(None);
    let legacy = tool.execute(json!({ "file_path": "/dev/zero" })).await;
    let vfs = tool
        .execute_with_ctx(json!({ "file_path": "/dev/zero" }), &realfs_ctx())
        .await;

    for (label, r) in [("legacy", &legacy), ("vfs", &vfs)] {
        assert!(r.is_error, "{label}: /dev/zero must be refused");
        assert!(
            r.content.contains("not a regular file"),
            "{label}: {}",
            r.content
        );
    }
}

/// NEGATIVE CONTROL. The refusals above must not have been bought by refusing
/// ordinary files: a regular file still reads normally through both entry
/// points. Without this, a `validate_user_path` that rejected everything would
/// pass every assertion above.
#[tokio::test]
async fn an_ordinary_regular_file_still_reads_through_both_entry_points() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ordinary.txt");
    std::fs::write(&path, "alpha\nbeta\ngamma\n").expect("write");

    let tool = ReadTool::new(None);
    let legacy = tool
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;
    let vfs = tool
        .execute_with_ctx(
            json!({ "file_path": path.to_str().unwrap() }),
            &realfs_ctx(),
        )
        .await;

    for (label, r) in [("legacy", &legacy), ("vfs", &vfs)] {
        assert!(!r.is_error, "{label}: {}", r.content);
        assert!(r.content.contains("1\talpha"), "{label}: {}", r.content);
        assert!(r.content.contains("3\tgamma"), "{label}: {}", r.content);
    }
}

/// NEGATIVE CONTROL for the size bound itself: exactly at the limit is
/// accepted, one byte over is refused. Pins that the stat-based bound is
/// still doing its own job on the sources that DO reach it — honest regular
/// files, the only kind that get past the rules above.
#[tokio::test]
async fn the_size_bound_is_inclusive_of_the_limit() {
    let dir = tempfile::tempdir().expect("tempdir");

    let exact = dir.path().join("exact.txt");
    std::fs::write(&exact, vec![b'a'; READ_MAX_BYTES as usize]).expect("write");
    let ok = ReadTool::new(None)
        .execute(json!({ "file_path": exact.to_str().unwrap() }))
        .await;
    assert!(
        !ok.is_error,
        "a file of exactly READ_MAX_BYTES must be accepted: {}",
        ok.content
    );

    let over = dir.path().join("over.txt");
    std::fs::write(&over, vec![b'a'; READ_MAX_BYTES as usize + 1]).expect("write");
    let refused = ReadTool::new(None)
        .execute(json!({ "file_path": over.to_str().unwrap() }))
        .await;
    assert!(refused.is_error, "one byte over the limit must be refused");
    assert!(refused.content.contains("over the Read tool's"));
}
