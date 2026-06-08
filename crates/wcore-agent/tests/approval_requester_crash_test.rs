//! Wave RB RELIABILITY MAJOR: ApprovalBridge requester-crash leak.
//!
//! Even after the SC reaper landed (TTL-based expiry, default 5min), the
//! audit MAJOR #4 still flagged a leak: if the receiver (requester
//! future) is dropped BEFORE the TTL fires, the `oneshot::Sender` stays
//! in the pending map until TTL — and during that window the active-
//! token snapshot keeps emitting a stale correlation id onto the wire.
//!
//! This test pins the RB additional fix: the reaper now also collects
//! entries whose sender is `is_closed()` (i.e. requester dropped),
//! independent of TTL. On the next reaper tick those entries vanish
//! and `pending_count()` returns to 0.

use std::time::Duration;

use wcore_agent::approval::{ApprovalBridge, ApprovalRequest};

/// Requester drops the receiver before TTL fires; the next reaper tick
/// collects the abandoned entry via `Sender::is_closed()`.
#[tokio::test]
async fn dropped_receiver_is_reaped_before_ttl() {
    // Use a long TTL (60s) so the test cannot rely on TTL expiry to
    // pass; the only path to reap must be the `is_closed()` check.
    let bridge = ApprovalBridge::with_ttl(Duration::from_secs(60));
    let (_correlation_id, rx) = bridge
        .request(ApprovalRequest {
            call_id: "c-drop".into(),
            reason: "test".into(),
            context: "ctx".into(),
        })
        .await;

    // Sanity: entry is in the map.
    assert_eq!(bridge.pending_count().await, 1);
    let active_before = bridge.active_tokens().await;
    assert_eq!(active_before.len(), 1);

    // Requester crashes — drop the receiver. This closes the
    // underlying `oneshot::Sender::is_closed()` from the bridge side.
    drop(rx);

    // Manually drive a reaper tick. We don't wait for the background
    // task because spawning it is the caller's responsibility (engine
    // bootstrap); the regression check is `reap_now()` collects the
    // abandoned entry without TTL having fired.
    let n = bridge.reap_now().await;
    assert_eq!(
        n, 1,
        "reaper must collect the abandoned (sender-closed) entry"
    );
    assert_eq!(
        bridge.pending_count().await,
        0,
        "pending map must be empty after reaping the crashed requester"
    );
    let active_after = bridge.active_tokens().await;
    assert!(
        active_after.is_empty(),
        "active_tokens snapshot must drop the reaped correlation id"
    );
}

/// Mixed scenario: two entries, one with the receiver still alive
/// (legitimate in-flight approval) and one with the receiver dropped
/// (requester crash). The reaper collects exactly the crashed one.
#[tokio::test]
async fn reaper_only_collects_crashed_entry_not_live_one() {
    let bridge = ApprovalBridge::with_ttl(Duration::from_secs(60));

    let (cid_alive, _rx_alive) = bridge
        .request(ApprovalRequest {
            call_id: "c-alive".into(),
            reason: "alive".into(),
            context: "".into(),
        })
        .await;
    let (cid_dead, rx_dead) = bridge
        .request(ApprovalRequest {
            call_id: "c-dead".into(),
            reason: "dead".into(),
            context: "".into(),
        })
        .await;
    drop(rx_dead); // requester crash

    assert_eq!(bridge.pending_count().await, 2);

    let n = bridge.reap_now().await;
    assert_eq!(n, 1, "exactly one entry should reap (the crashed one)");
    let active = bridge.active_tokens().await;
    assert_eq!(active.len(), 1);
    assert!(
        active.contains(&cid_alive),
        "live requester must NOT be reaped"
    );
    assert!(
        !active.contains(&cid_dead),
        "crashed requester must be removed"
    );
}

/// Idempotency: calling reap_now twice on the same crashed entry
/// returns 1 then 0 — there is no double-counting and no double-drop.
#[tokio::test]
async fn reap_now_is_idempotent_on_subsequent_calls() {
    let bridge = ApprovalBridge::with_ttl(Duration::from_secs(60));
    let (_cid, rx) = bridge
        .request(ApprovalRequest {
            call_id: "c-idem".into(),
            reason: "".into(),
            context: "".into(),
        })
        .await;
    drop(rx);

    let first = bridge.reap_now().await;
    assert_eq!(first, 1);
    let second = bridge.reap_now().await;
    assert_eq!(second, 0, "second reap must be a no-op");
}
