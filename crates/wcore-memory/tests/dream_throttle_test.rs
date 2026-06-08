// M3.1.2 — DreamThrottle: concurrency-safe last-run gate so the dream cycle
// fires at most once per `min_interval` from session-end.

use std::time::Duration;
use wcore_memory::consolidate::DreamThrottle;

#[test]
fn throttle_first_call_runs() {
    let t = DreamThrottle::new(Duration::from_secs(60));
    assert!(t.should_run());
}

#[test]
fn throttle_blocks_within_window() {
    let t = DreamThrottle::new(Duration::from_secs(60));
    assert!(t.should_run()); // marks last_run
    assert!(!t.should_run(), "second call inside window must be blocked");
}

#[tokio::test]
async fn throttle_releases_after_window() {
    let t = DreamThrottle::new(Duration::from_millis(50));
    assert!(t.should_run());
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        t.should_run(),
        "after window, should_run must return true again"
    );
}
