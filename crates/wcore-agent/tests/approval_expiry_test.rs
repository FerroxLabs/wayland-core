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

/// The default TTL constant must be the one the default bridge actually uses.
///
/// # Why the constant pins alone were not a test (wayland#934, 2026-08-28)
///
/// This was `assert_eq!(DEFAULT_APPROVAL_TTL, Duration::from_secs(300))` and its
/// twin, which restate the two literals declared in `approval.rs`. **Measured:**
/// with `impl Default for ApprovalBridge` mutated to `ttl: Duration::from_secs(1)`
/// — a default bridge that reaps a human's approval after one second while both
/// constants keep their documented values — the old test passed.
///
/// The pins are kept, because "a future tighten-the-TTL PR must be explicit" is a
/// real thing to want. What is added is the wiring: the constant has to be
/// load-bearing, and the two structural invariants that make the pair coherent.
#[tokio::test]
async fn the_default_ttl_constant_is_the_one_the_default_bridge_uses() {
    // Pin the documented defaults so a future "tighten the TTL" PR
    // makes the change explicit.
    assert_eq!(DEFAULT_APPROVAL_TTL, Duration::from_secs(300));
    assert_eq!(DEFAULT_REAP_INTERVAL, Duration::from_secs(30));

    // Invariant 1: the reaper must tick faster than entries expire. At an
    // interval >= the TTL an expired approval can outlive its deadline by a whole
    // period, and the `oneshot::Sender` it holds with it.
    assert!(
        DEFAULT_REAP_INTERVAL < DEFAULT_APPROVAL_TTL,
        "a reap interval of {DEFAULT_REAP_INTERVAL:?} against a TTL of \
         {DEFAULT_APPROVAL_TTL:?} lets an expired approval outlive its deadline"
    );
    // Invariant 2: a zero TTL expires every approval before a human can read it.
    assert!(
        !DEFAULT_APPROVAL_TTL.is_zero(),
        "a zero default TTL cancels every approval at once"
    );

    // The wiring. A default bridge must not reap an approval that is barely a
    // second old — the mutation above is exactly that, and this is what sees it.
    // This proves a LOWER BOUND on the default TTL, not the whole 300 s; 300 s of
    // real sleep is not a test anyone would run. The bound is chosen to sit above
    // the shortest TTL a plausible mistake produces.
    let bridge = ApprovalBridge::new();
    let (_correlation_id, _rx) = bridge
        .request(ApprovalRequest {
            call_id: "c-1".into(),
            reason: "".into(),
            context: "".into(),
        })
        .await;

    // The known-positive control, in the same run and under the same sleep: an
    // entry that IS expired gets collected. Without it, "reaped 0" is equally well
    // explained by a reaper that never collects anything, and this test would then
    // pass against a bridge whose TTL never fires at all.
    let control = ApprovalBridge::with_ttl(Duration::from_millis(300));
    let (_cc, _crx) = control
        .request(ApprovalRequest {
            call_id: "c-2".into(),
            reason: "".into(),
            context: "".into(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(1_200)).await;

    assert_eq!(
        control.reap_now().await,
        1,
        "control: a 300ms-TTL entry must be collected after 1.2s, or the reaper is not working \
         and the assertion below would pass for the wrong reason"
    );
    assert_eq!(
        bridge.reap_now().await,
        0,
        "the DEFAULT bridge reaped a 1.2s-old approval, so its TTL is not \
         DEFAULT_APPROVAL_TTL ({DEFAULT_APPROVAL_TTL:?}) however that constant is declared"
    );
    assert_eq!(
        bridge.pending_count().await,
        1,
        "the approval must still be waiting for a human"
    );
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
                cancellation: None,
            },
        )
        .await;
    assert!(!resolved, "stale resolve after expiry must return false");
}
