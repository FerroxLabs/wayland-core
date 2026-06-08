//! W8b D.2 — `FileWatcher` integration test.
//!
//! Validates:
//!   * external edits surface as `ExternalEvent`s
//!   * `mark_self_originated` swallows engine-issued writes within the
//!     debounce window (D.4 invariant)

use std::time::Duration;

use wcore_agent::watch::FileWatcher;

#[tokio::test]
async fn detects_external_edit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let watcher = FileWatcher::new(tmp.path()).expect("watcher");

    // Small delay so notify has finished arming on platforms with
    // asynchronous registration (FSEvents).
    tokio::time::sleep(Duration::from_millis(50)).await;

    let target = tmp.path().join("foo.txt");
    tokio::fs::write(&target, b"external").await.unwrap();

    // Up to 1.5s for the platform notifier to publish; flake-prone on
    // CI macOS otherwise.
    let event = watcher
        .next_external_event(Duration::from_millis(1500))
        .await
        .expect("expected at least one external event");
    assert!(
        event.path.ends_with("foo.txt") || event.path == target,
        "expected event for foo.txt, got: {:?}",
        event.path
    );
}

#[tokio::test]
async fn ignores_self_originated_edit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let watcher = FileWatcher::new(tmp.path()).expect("watcher");

    tokio::time::sleep(Duration::from_millis(50)).await;

    let target = tmp.path().join("bar.txt");
    // Engine "marks" the path BEFORE writing — D.4 invariant.
    watcher.mark_self_originated(&target);
    tokio::fs::write(&target, b"by-engine").await.unwrap();

    // Within the debounce window, no external event should surface.
    let observed = watcher
        .next_external_event(Duration::from_millis(400))
        .await;
    assert!(
        observed.is_none(),
        "self-originated write must be filtered, got: {observed:?}"
    );
}
