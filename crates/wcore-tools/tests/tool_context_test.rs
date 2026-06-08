//! W8a A.3 — `ToolContext` + `VirtualFs` integration tests.
//!
//! Covers RealFs round-trip, InMemoryFs isolation from disk, and
//! SandboxedFs rejection of out-of-root reads and writes. Wave SD
//! removed the `fallthrough_reads` escape hatch; reads are now
//! sandbox-checked the same way writes are.

use std::path::Path;
use std::sync::Arc;

use wcore_tools::context::ToolContext;
use wcore_tools::vfs::{InMemoryFs, RealFs, SandboxedFs, VfsError, VirtualFs};

#[tokio::test]
async fn real_fs_round_trips_a_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("hello.txt");
    let fs = RealFs;
    fs.write(&path, b"hello").await.unwrap();
    let got = fs.read(&path).await.unwrap();
    assert_eq!(got, b"hello");
}

#[tokio::test]
async fn in_memory_fs_isolates_from_real_disk() {
    let mem = InMemoryFs::new();
    let p = Path::new("/in/mem.txt");
    mem.write(p, b"in-memory only").await.unwrap();
    assert_eq!(mem.read(p).await.unwrap(), b"in-memory only");
    // RealFs cannot read the in-memory entry.
    let real = RealFs;
    assert!(real.read(p).await.is_err());
}

#[tokio::test]
async fn sandboxed_fs_blocks_writes_outside_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let sb = SandboxedFs::new(RealFs, &root);
    // Lexical escape attempt — `../etc/passwd`-style — must reject.
    let escape = root.join("..").join("etc").join("oops.txt");
    let err = sb.write(&escape, b"escape").await.unwrap_err();
    matches!(err, VfsError::OutsideSandbox { .. });
}

#[tokio::test]
async fn sandboxed_fs_allows_writes_inside_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    let sb = SandboxedFs::new(RealFs, &root);
    let p = root.join("a.txt");
    sb.write(&p, b"ok").await.unwrap();
    let got = sb.read(&p).await.unwrap();
    assert_eq!(got, b"ok");
}

#[tokio::test]
async fn sandboxed_fs_blocks_reads_outside_root() {
    // Wave SD SECURITY MAJOR #13: `fallthrough_reads` is gone. Reads
    // outside the sandbox must be refused.
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("tempdir2");
    let outside_path = outside.path().join("outside.txt");
    tokio::fs::write(&outside_path, b"outside-data")
        .await
        .unwrap();

    let root = tmp.path().to_path_buf();
    let sb = SandboxedFs::new(RealFs, &root);
    let err = sb.read(&outside_path).await.unwrap_err();
    assert!(
        matches!(err, VfsError::OutsideSandbox { .. }),
        "expected OutsideSandbox, got {err:?}"
    );
    // Writes outside the root remain rejected.
    let err = sb.write(&outside_path, b"clobber").await.unwrap_err();
    assert!(matches!(err, VfsError::OutsideSandbox { .. }));
}

#[tokio::test]
async fn tool_context_test_default_constructs() {
    let ctx = ToolContext::test_default();
    assert!(ctx.call_id.is_empty());
    assert!(!ctx.cancel.is_cancelled());
    assert!(ctx.source_agent.is_none());
    // Smoke: vfs can be cloned/shared.
    let _vfs: Arc<dyn VirtualFs> = ctx.vfs.clone();
}
