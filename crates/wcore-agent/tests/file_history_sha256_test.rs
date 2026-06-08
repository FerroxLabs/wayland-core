//! Wave SD — FileHistory SHA-256 digest tests.
//!
//! Closes SECURITY MAJOR #17 verification:
//!
//!   * `byte_digest` returns a 32-byte SHA-256 (not 8 bytes of
//!     `DefaultHasher::finish()`).
//!   * Identical inputs produce identical digests.
//!   * Distinct inputs produce distinct digests.
//!   * The rollback guard's `last_engine_write_digest` round-trips
//!     through `record_post_write_digest` and matches `byte_digest`.

use std::path::PathBuf;
use std::sync::Arc;

use wcore_agent::file_history::{ByteDigest, FileHistory, byte_digest};
use wcore_tools::vfs::RealFs;

fn make_history() -> (FileHistory, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let history = FileHistory::new(Arc::new(RealFs), dir.path().join("shadow"));
    (history, dir)
}

#[test]
fn byte_digest_returns_32_bytes() {
    let d: ByteDigest = byte_digest(b"hello world");
    assert_eq!(d.len(), 32, "Wave SD: SHA-256 digest must be 32 bytes");
}

#[test]
fn byte_digest_is_sha256_of_input() {
    // Known SHA-256("hello world") = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
    let d = byte_digest(b"hello world");
    let hex: String = d.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(
        hex, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9",
        "byte_digest must be SHA-256, not DefaultHasher"
    );
}

#[test]
fn byte_digest_is_stable() {
    assert_eq!(byte_digest(b"same"), byte_digest(b"same"));
    assert_eq!(byte_digest(b""), byte_digest(b""));
}

#[test]
fn byte_digest_is_distinct_for_distinct_input() {
    assert_ne!(byte_digest(b"foo"), byte_digest(b"bar"));
    assert_ne!(byte_digest(b"a"), byte_digest(b"b"));
}

#[test]
fn record_post_write_digest_round_trips() {
    let (history, _dir) = make_history();
    let path = PathBuf::from("/tmp/wcore-sd-test.txt");
    let bytes = b"engine just wrote this";

    assert!(
        history.last_engine_write_digest(&path).is_none(),
        "no engine write yet → None"
    );

    history.record_post_write_digest(&path, bytes);
    let got = history
        .last_engine_write_digest(&path)
        .expect("digest recorded");
    assert_eq!(got, byte_digest(bytes));
}

#[test]
fn distinct_writes_record_distinct_digests() {
    let (history, _dir) = make_history();
    let a = PathBuf::from("/tmp/sd-a.txt");
    let b = PathBuf::from("/tmp/sd-b.txt");
    history.record_post_write_digest(&a, b"alpha");
    history.record_post_write_digest(&b, b"beta");
    let da = history.last_engine_write_digest(&a).unwrap();
    let db = history.last_engine_write_digest(&b).unwrap();
    assert_ne!(da, db);
}

#[tokio::test]
async fn snapshot_round_trip_with_sha256() {
    // End-to-end: snapshot a file, retrieve digest of its bytes,
    // assert it matches the SHA-256 of the original content.
    let (history, dir) = make_history();
    let live_path = dir.path().join("live.txt");
    tokio::fs::write(&live_path, b"snapshot-me").await.unwrap();

    let vfs = RealFs;
    history.snapshot(&live_path, &vfs).await.expect("snapshot");

    let d = history
        .last_snapshot_digest(&live_path)
        .await
        .expect("digest");
    assert_eq!(d, byte_digest(b"snapshot-me"));
    assert_eq!(d.len(), 32);
}
