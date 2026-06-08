//! Wave SC SECURITY MAJOR fix — `ApprovalBridge` entries expire on TTL.
//!
//! Closes the audit finding: abandoned approvals (LLM session crashed,
//! human walked away) previously leaked `oneshot::Sender` + map entries
//! indefinitely. Now: each pending entry carries an `expires_at`
//! instant; the background reaper scans and auto-resolves expired
//! entries as Cancelled, dropping the sender + map entry.

use std::time::Duration;

use wcore_agent::approval::{
    ApprovalBridge, ApprovalRequest, DEFAULT_APPROVAL_TTL, DEFAULT_REAP_INTERVAL,
};

#[tokio::test]
async fn expired_token_auto_resolves_as_cancelled() {
    let bridge = ApprovalBridge::with_ttl(Duration::from_millis(50));
    let (correlation_id, rx) = bridge
        .request(ApprovalRequest {
            call_id: "c-1".into(),
            reason: "test".into(),
            context: "ctx".into(),
        })
        .await;

    // Pending count = 1 before expiry.
    assert_eq!(bridge.pending_count().await, 1);
    assert!(
        bridge.active_tokens().await.contains(&correlation_id),
        "active set must contain the token before expiry"
    );

    // Wait past TTL.
    tokio::time::sleep(Duration::from_millis(80)).await;
    let reaped = bridge.reap_now().await;
    assert_eq!(reaped, 1, "reaper must collect the one expired entry");

    // The receiver must observe a Cancelled outcome (approved=false).
    let outcome = rx.await.expect("sender must have been dropped after send");
    assert!(!outcome.approved, "expired outcome must be !approved");
    assert!(outcome.modifications.is_none());

    // Map entry + active set must be cleared.
    assert_eq!(bridge.pending_count().await, 0);
    assert!(bridge.active_tokens().await.is_empty());
}

#[tokio::test]
async fn non_expired_pending_survives_reap() {
    // TTL = 10s, way past the 50ms sleep — reap_now should leave
    // the entry untouched.
    let bridge = ApprovalBridge::with_ttl(Duration::from_secs(10));
    let (_correlation_id, _rx) = bridge
        .request(ApprovalRequest {
            call_id: "c-1".into(),
            reason: "".into(),
            context: "".into(),
        })
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let reaped = bridge.reap_now().await;
    assert_eq!(reaped, 0, "non-expired entries must survive reap");
    assert_eq!(bridge.pending_count().await, 1);
}

#[tokio::test]
async fn background_reaper_task_collects_expired_entries() {
    let bridge = ApprovalBridge::with_ttl(Duration::from_millis(50));
    // Reap interval = 30ms; the task ticks once at startup then every
    // 30ms thereafter. The entry expires at +50ms; the second tick
    // (~+60ms) should catch it.
    let handle = bridge.spawn_reaper(Duration::from_millis(30));

    let (_correlation_id, rx) = bridge
        .request(ApprovalRequest {
            call_id: "c-1".into(),
            reason: "".into(),
            context: "".into(),
        })
        .await;

    // Wait for the reaper to do its job. 300ms is plenty of slack —
    // even with tokio scheduling jitter the second tick fires well
    // within this window.
    let outcome = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("background reaper must resolve within 2s")
        .expect("sender must send before being dropped");
    assert!(!outcome.approved, "expired outcome must be !approved");
    assert_eq!(bridge.pending_count().await, 0);

    handle.abort();
}

#[tokio::test]
async fn ttl_constants_have_sane_defaults() {
    // Pin the documented defaults so a future "tighten the TTL" PR
    // makes the change explicit.
    assert_eq!(DEFAULT_APPROVAL_TTL, Duration::from_secs(300));
    assert_eq!(DEFAULT_REAP_INTERVAL, Duration::from_secs(30));
}

#[tokio::test]
async fn explicit_resolve_after_expiry_returns_false() {
    // After expiry + reap, a late ApprovalResume command with the
    // (now stale) correlation id resolves to nothing — the bridge
    // returns false so the CLI can emit a "stale token" Info event.
    let bridge = ApprovalBridge::with_ttl(Duration::from_millis(50));
    let (correlation_id, _rx) = bridge
        .request(ApprovalRequest {
            call_id: "c-1".into(),
            reason: "".into(),
            context: "".into(),
        })
        .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    bridge.reap_now().await;
    let resolved = bridge
        .resolve(
            &correlation_id,
            wcore_agent::approval::ApprovalOutcome {
                approved: true,
                modifications: None,
            },
        )
        .await;
    assert!(!resolved, "stale resolve after expiry must return false");
}
