//! GHSA-8r7g part (a) — secret resume_token vs public correlation_id separation.
//!
//! Each test asserts one security property of the `ApprovalBridge` fix:
//! the bridge now mints an opaque `apr-{uuid}` secret for every pending
//! entry and maps a caller-supplied, possibly model-known `correlation_id`
//! (the `call_id`) to it via a secondary index. A wire/host peer MUST
//! present the secret; presenting the model-known `call_id` to `resolve`
//! returns false. The local TUI path resolves by `correlation_id` via
//! `resolve_by_correlation`, which is unreachable from the wire ingress.

use std::time::Duration;

use wcore_agent::approval::{ApprovalBridge, ApprovalOutcome, ApprovalRequest};

fn req(tag: &str) -> ApprovalRequest {
    ApprovalRequest {
        call_id: format!("call-{tag}"),
        reason: format!("reason-{tag}"),
        context: format!("ctx-{tag}"),
    }
}

fn approved() -> ApprovalOutcome {
    ApprovalOutcome {
        approved: true,
        modifications: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1
// ---------------------------------------------------------------------------

/// The secret token returned by `request_with_id` must be an opaque
/// `apr-{uuid}` value and must NOT equal the caller-supplied correlation id.
/// If they were equal a model-known `call_id` would be its own wire key.
#[tokio::test]
async fn request_with_id_returns_secret_distinct_from_correlation() {
    let bridge = ApprovalBridge::new();
    let corr = "tool:my-call-id".to_string();
    let (secret, _rx) = bridge.request_with_id(corr.clone(), req("t1")).await;

    assert!(
        secret.starts_with("apr-"),
        "secret token must be an opaque apr-{{uuid}}; got: {secret:?}"
    );
    assert_ne!(
        secret, corr,
        "secret must NOT equal the correlation id — a model-known call_id must not self-approve"
    );
}

// ---------------------------------------------------------------------------
// Test 2
// ---------------------------------------------------------------------------

/// GHSA-8r7g core property: the wire `resolve` path only accepts the SECRET
/// token. Presenting the model-known `call_id` (the `correlation_id`) to
/// `resolve` must return false; presenting the secret must return true and
/// deliver `approved=true` to the waiting receiver.
#[tokio::test]
async fn wire_resolve_requires_the_secret_not_the_call_id() {
    let bridge = ApprovalBridge::new();
    let corr = "egress:abc".to_string();
    let (secret, rx) = bridge.request_with_id(corr.clone(), req("t2")).await;

    // Attempting wire-resolve with the model-visible call_id must fail.
    let call_id_resolve = bridge.resolve("egress:abc", approved()).await;
    assert!(
        !call_id_resolve,
        "resolve(call_id) over the wire path must return false — \
         a model-known id must not act as a self-approval token"
    );

    // Resolving with the secret must succeed.
    let secret_resolve = bridge.resolve(&secret, approved()).await;
    assert!(
        secret_resolve,
        "resolve(secret) must return true for a pending entry"
    );

    let outcome = rx.await.expect("oneshot must deliver an outcome");
    assert!(
        outcome.approved,
        "receiver must observe approved=true after secret-path resolve"
    );
}

// ---------------------------------------------------------------------------
// Test 3
// ---------------------------------------------------------------------------

/// The LOCAL TUI path (`resolve_by_correlation`) resolves by the public
/// correlation id and correctly delivers the outcome to the receiver.
/// This path is unreachable from the wire ingress — it is the safe in-process
/// resolution surface for keypress / egress-event resolvers.
#[tokio::test]
async fn tui_resolves_by_correlation() {
    let bridge = ApprovalBridge::new();
    let corr = "tui-handle".to_string();
    let (_secret, rx) = bridge.request_with_id(corr.clone(), req("t3")).await;

    let result = bridge.resolve_by_correlation(&corr, approved()).await;
    assert!(
        result,
        "resolve_by_correlation must return true for a known correlation id"
    );

    let outcome = rx.await.expect("oneshot must deliver an outcome");
    assert!(
        outcome.approved,
        "TUI resolve path must deliver approved=true to the waiting receiver"
    );
}

// ---------------------------------------------------------------------------
// Test 4
// ---------------------------------------------------------------------------

/// The sync `secret_for_correlation` mirror is populated after `request_with_id`
/// and is cleared (along with the pending count) after the entry is resolved.
/// Frame synthesizers (GatingProtocolWriter, ChannelEmitter) read this mirror
/// synchronously to stamp the secret onto outbound frames.
#[tokio::test]
async fn secret_for_correlation_maps_then_clears() {
    let bridge = ApprovalBridge::new();
    let corr = "lifecycle-corr".to_string();
    let (secret, rx) = bridge.request_with_id(corr.clone(), req("t4")).await;

    // Mirror is populated immediately after request.
    assert_eq!(
        bridge.secret_for_correlation(&corr),
        Some(secret.clone()),
        "secret_for_correlation must return the minted secret while entry is pending"
    );
    assert_eq!(
        bridge.secret_for_correlation("unknown-id"),
        None,
        "secret_for_correlation must return None for an unregistered correlation id"
    );

    // Resolve and verify both the pending map and the mirror are cleared.
    bridge.resolve(&secret, approved()).await;
    let _ = rx.await;

    assert_eq!(
        bridge.secret_for_correlation(&corr),
        None,
        "secret_for_correlation must return None after the entry is resolved"
    );
    assert_eq!(
        bridge.pending_count().await,
        0,
        "pending_count must be 0 after resolve"
    );
}

// ---------------------------------------------------------------------------
// Test 5
// ---------------------------------------------------------------------------

/// `reap_now` on a zero-TTL entry purges the primary `by_token` map, the
/// secondary `by_corr` index, and the sync `corr_secrets` mirror in one shot.
/// The waiting receiver observes a cancelled (approved=false) outcome.
#[tokio::test]
async fn reap_purges_secret_and_index_and_mirror() {
    let bridge = ApprovalBridge::new();
    let corr = "reap-target".to_string();
    let (secret, rx) = bridge
        .request_with_id_and_ttl(corr.clone(), req("t5"), Duration::from_secs(0))
        .await;

    let reaped = bridge.reap_now().await;
    assert_eq!(reaped, 1, "reap_now must collect the zero-TTL entry");

    let outcome = rx.await.expect("reaper must send a cancelled outcome");
    assert!(
        !outcome.approved,
        "reaped entry must deliver approved=false (cancelled)"
    );

    // Both the secret token and its correlation mapping must be gone.
    assert_eq!(
        bridge.secret_for_correlation(&corr),
        None,
        "corr_secrets mirror must be cleared after reap"
    );
    // Attempting wire-resolve with the (now reaped) secret must fail.
    let stale = bridge.resolve(&secret, approved()).await;
    assert!(
        !stale,
        "resolve on a reaped secret must return false (entry already removed)"
    );
    assert_eq!(
        bridge.pending_count().await,
        0,
        "pending_count must be 0 after reap"
    );
}

// ---------------------------------------------------------------------------
// Test 6
// ---------------------------------------------------------------------------

/// When the same `correlation_id` is registered twice, the `by_corr` mirror
/// points to the SECOND (newest) secret (last-writer-wins). The FIRST secret
/// remains resolvable via the wire path — no dangling entry, no use-after-free
/// of the older `oneshot::Sender`.
#[tokio::test]
async fn duplicate_correlation_last_writer_wins_mirror() {
    let bridge = ApprovalBridge::new();
    let corr = "shared-corr".to_string();

    let (first_secret, _rx1) = bridge.request_with_id(corr.clone(), req("t6a")).await;
    let (second_secret, _rx2) = bridge.request_with_id(corr.clone(), req("t6b")).await;

    // Secrets must be distinct (each call mints a fresh uuid).
    assert_ne!(
        first_secret, second_secret,
        "each request_with_id call must mint a distinct secret"
    );

    // The mirror must point at the most-recent secret (last writer wins).
    assert_eq!(
        bridge.secret_for_correlation(&corr),
        Some(second_secret.clone()),
        "corr_secrets mirror must point to the second (latest) secret after duplicate registration"
    );

    // The first secret must still live in by_token and resolve cleanly,
    // proving the older entry is not dangling.
    let first_resolved = bridge.resolve(&first_secret, approved()).await;
    assert!(
        first_resolved,
        "first secret must remain resolvable via the wire path after a duplicate registration"
    );
}

// ---------------------------------------------------------------------------
// Test 7
// ---------------------------------------------------------------------------

/// The plain `request` path (no explicit correlation id) registers no
/// `by_corr` entry. `secret_for_correlation` returns `None` for any key,
/// including the secret itself, while `resolve(&secret, …)` works normally.
#[tokio::test]
async fn random_request_path_has_no_correlation_entry() {
    let bridge = ApprovalBridge::new();
    let (secret, rx) = bridge.request(req("t7")).await;

    // No correlation index entry exists for any key on the random-request path.
    assert_eq!(
        bridge.secret_for_correlation("any-call-id"),
        None,
        "secret_for_correlation must return None when no correlation was registered"
    );
    assert_eq!(
        bridge.secret_for_correlation(&secret),
        None,
        "the secret itself is not a correlation key — secret_for_correlation must return None"
    );

    // The secret is still the wire-resolve key.
    let resolved = bridge.resolve(&secret, approved()).await;
    assert!(
        resolved,
        "resolve by secret must work for an entry registered via the plain request() path"
    );

    let outcome = rx.await.expect("oneshot must deliver an outcome");
    assert!(
        outcome.approved,
        "receiver must observe approved=true after resolve"
    );
}
