//! W7 S4: in-process Approval bridge. Producers call
//! `bridge.request(...)` and await an `ApprovalOutcome`; the engine's
//! command loop calls `bridge.resolve(correlation_id, outcome)` when
//! an `ApprovalResume` command arrives.
//!
//! Wave SC SECURITY MAJOR remediations:
//!
//! - **Correlation ID model (was: bare resume token).** Each pending
//!   approval is keyed by an opaque random `correlation_id`. The
//!   bridge's pending-map is keyed by that id; the wire shape carries
//!   the same value. The terminology shift makes the role explicit —
//!   the on-wire value is a CORRELATION HANDLE for UI matching, not a
//!   secret. The actual security boundary is the redaction pass in
//!   `protocol_sink::redact_tokens` (defense-in-depth that prevents
//!   tools that read tool output from lifting active tokens).
//!
//! - **TTL with reaper (was: tokens lived forever).** Each pending
//!   entry carries an `expires_at` instant. A background tokio task
//!   wakes every reap interval (default 30s), scans the map, and
//!   auto-resolves expired entries as `ApprovalOutcome::Cancelled`
//!   (drops the `oneshot::Sender`). Prevents memory growth +
//!   indefinite-Suspend DoS when a host walks away.
//!
//! - **Active-token snapshot for redaction.** `active_tokens()` exposes
//!   the set of correlation ids in flight so `ProtocolSink` can scrub
//!   them out of streaming tool output. This is defense-in-depth — the
//!   bridge holder is the authoritative resolver; the redaction pass
//!   makes the wire stream show only the ids that the host UI already
//!   has via the `ApprovalRequired` event.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, oneshot};

/// Default time-to-live for a pending approval. Set to 5 minutes so a
/// human HITL flow has time to read + decide; abandoned approvals
/// auto-expire and free the slot.
pub const DEFAULT_APPROVAL_TTL: Duration = Duration::from_secs(300);

/// Default reap interval. The reaper task wakes every 30s and scans
/// the pending map; expired entries are auto-resolved as Cancelled.
pub const DEFAULT_REAP_INTERVAL: Duration = Duration::from_secs(30);

/// Long TTL for the Crucible proposal card. A multi-vendor cost card is a
/// deliberation-worthy, expensive decision, so it must not be reaped mid-read
/// by the 5-minute default (spec §7: long/no-expire approval TTL). 24h is
/// effectively no-expire for a single sitting while still bounding the pending
/// map; a closed channel (host crash) is still reaped immediately regardless.
pub const CRUCIBLE_APPROVAL_TTL: Duration = Duration::from_secs(86_400);

#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub call_id: String,
    pub reason: String,
    pub context: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDisposition {
    Approved,
    Denied,
    /// Auto-resolution path — the TTL reaper fired or the requester
    /// dropped. Tools should treat this as "host did not respond".
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct ApprovalOutcome {
    pub approved: bool,
    pub modifications: Option<serde_json::Value>,
}

impl ApprovalOutcome {
    /// Cancelled / auto-expired outcome — used by the TTL reaper when
    /// no host response arrived in time.
    pub fn cancelled() -> Self {
        Self {
            approved: false,
            modifications: None,
        }
    }
}

/// Per-pending-entry record. Owns the response sender + the expiry
/// instant; the reaper task scans these for `expires_at < now`.
struct Pending {
    sender: oneshot::Sender<ApprovalOutcome>,
    expires_at: Instant,
    /// GHSA-8r7g: the public correlation handle (a caller-supplied, possibly
    /// model-known `call_id`) this entry is indexed under in `by_corr`, if any.
    /// Stored so removing the entry (resolve/reap) also purges the secondary
    /// index — never leave a `by_corr` mapping dangling to a freed token.
    correlation_id: Option<String>,
}

/// GHSA-8r7g: the bridge's pending state under a SINGLE mutex. The primary
/// map is keyed by a SECRET `resume_token` (`apr-{uuid}`, unguessable) — that
/// is the only value a wire/host peer may present to resolve an approval. The
/// secondary `by_corr` index maps a public `correlation_id` (a caller-supplied
/// `call_id`, which the model can see) to its secret token, so a LOCAL resolver
/// (a TUI keypress, an in-process egress event) can resolve by the public
/// handle without the secret ever reaching a model-reachable surface. Both maps
/// live under one lock so an entry can never appear in one and not the other.
#[derive(Default)]
struct PendingMaps {
    by_token: HashMap<String, Pending>,
    by_corr: HashMap<String, String>,
}

#[derive(Clone)]
pub struct ApprovalBridge {
    pending: Arc<Mutex<PendingMaps>>,
    ttl: Duration,
    /// Wave SC: shared active-token redactor. The bridge holds an
    /// `Arc<RwLock<...>>` so callers (sinks, tests) can clone the
    /// redactor and observe the same set. The bridge refreshes this
    /// snapshot on every `request` / `resolve` / `reap` so the
    /// protocol sink's redaction pass always sees current in-flight
    /// correlation ids.
    redactor: crate::output::protocol_sink::ActiveTokenRedactor,
    /// GHSA-8r7g: a SYNC-readable snapshot of `by_corr` (public correlation id
    /// → secret `resume_token`). The approval-frame synthesizers
    /// (`GatingProtocolWriter`, `ChannelEmitter`) run in a synchronous `emit`
    /// and cannot lock the async `pending` mutex, so they read this mirror to
    /// stamp the SECRET token onto the host-visible frame (empty for a
    /// regular tool with no bridge entry). Refreshed on every mutation
    /// alongside the redactor snapshot, so it never lags the pending state.
    corr_secrets: Arc<std::sync::RwLock<HashMap<String, String>>>,
}

impl Default for ApprovalBridge {
    fn default() -> Self {
        Self {
            pending: Arc::new(Mutex::new(PendingMaps::default())),
            ttl: DEFAULT_APPROVAL_TTL,
            redactor: crate::output::protocol_sink::ActiveTokenRedactor::new(),
            corr_secrets: Arc::new(std::sync::RwLock::new(HashMap::new())),
        }
    }
}

impl ApprovalBridge {
    /// Construct a bridge with the default 5-minute TTL.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a bridge with a custom TTL. Useful for tests that want
    /// to assert expiry behavior in < 1s.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            pending: Arc::new(Mutex::new(PendingMaps::default())),
            ttl,
            redactor: crate::output::protocol_sink::ActiveTokenRedactor::new(),
            corr_secrets: Arc::new(std::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Accessor for the bridge's shared active-token redactor. The
    /// CLI clones this onto the `ProtocolSink` via
    /// `with_token_redactor` so streaming tool output gets scrubbed
    /// of in-flight correlation ids before emission.
    pub fn redactor(&self) -> crate::output::protocol_sink::ActiveTokenRedactor {
        self.redactor.clone()
    }

    /// Snapshot the pending set into the redactor. Called after every
    /// mutation (request / resolve / reap). The redactor's internal
    /// set replaces atomically — readers never observe a torn state.
    async fn refresh_redactor(&self) {
        // GHSA-8r7g: snapshot the SECRET tokens only for the redactor. The
        // public `by_corr` handles (call_ids) are already model-visible, so
        // redacting them from tool output would be pointless and could scrub
        // legitimate content. Also refresh the sync correlation→secret mirror
        // the frame synthesizers read (under the SAME async lock, so both
        // snapshots reflect one consistent pending state).
        let (tokens, corr): (Vec<String>, HashMap<String, String>) = {
            let map = self.pending.lock().await;
            (map.by_token.keys().cloned().collect(), map.by_corr.clone())
        };
        self.redactor.set(tokens);
        if let Ok(mut mirror) = self.corr_secrets.write() {
            *mirror = corr;
        }
    }

    /// GHSA-8r7g: sync lookup of the SECRET `resume_token` for a public
    /// correlation id (a `call_id`). Used by the approval-frame synthesizers to
    /// stamp the unguessable token onto the host-visible frame. Returns `None`
    /// for a `call_id` with no bridge-backed approval (a regular tool gated by
    /// the `ToolApprovalManager`), so the synthesizer emits an EMPTY resume
    /// token there — a regular tool is never resolved through this bridge.
    pub fn secret_for_correlation(&self, correlation_id: &str) -> Option<String> {
        self.corr_secrets
            .read()
            .ok()
            .and_then(|m| m.get(correlation_id).cloned())
    }

    /// Spawn the background reaper task. Returns a `tokio::task::JoinHandle`
    /// so the caller can abort on shutdown. The reaper wakes every
    /// `interval` and resolves any pending entry whose `expires_at`
    /// has passed.
    ///
    /// **Idempotent in production:** call once at engine bootstrap. If
    /// the bridge is cloned (Arc) the spawned task observes the shared
    /// pending map. Tests can spawn a new reaper per bridge.
    pub fn spawn_reaper(&self, interval: Duration) -> tokio::task::JoinHandle<()> {
        let bridge = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // Tick once at startup to align with the test's expectations,
            // then on every interval thereafter.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let _ = bridge.reap_now().await;
            }
        })
    }

    /// Scan the pending map; resolve every expired entry as Cancelled
    /// and drop the sender. Exposed for tests that drive expiry without
    /// waiting for the background interval. Also refreshes the
    /// shared redactor snapshot.
    pub async fn reap_now(&self) -> usize {
        let count = Self::reap_expired(&self.pending).await;
        if count > 0 {
            self.refresh_redactor().await;
        }
        count
    }

    async fn reap_expired(pending: &Arc<Mutex<PendingMaps>>) -> usize {
        let now = Instant::now();
        // Wave RB RELIABILITY MAJOR (requester-crash leak): an entry
        // also counts as reapable if its `sender.is_closed()` — that
        // happens when the receiver-side future has been dropped
        // (requester crashed, awaited future was cancelled, etc.).
        // Without this check the entry sits in the map until TTL
        // fires (up to 5 minutes by default), and on every
        // `refresh_redactor()` snapshot we leak a stale correlation
        // id onto the wire. With this check, the next reaper tick
        // (every 30s by default) collects the abandoned entry.
        let reapable_keys: Vec<String> = {
            let map = pending.lock().await;
            map.by_token
                .iter()
                .filter(|(_, p)| p.expires_at <= now || p.sender.is_closed())
                .map(|(k, _)| k.clone())
                .collect()
        };
        let count = reapable_keys.len();
        if count > 0 {
            let mut map = pending.lock().await;
            for key in reapable_keys {
                if let Some(p) = map.by_token.remove(&key) {
                    // GHSA-8r7g: purge the secondary index too, so no
                    // `by_corr` mapping dangles to a freed secret token — but
                    // only if it still points to THIS token (a duplicate
                    // re-registration to a newer secret must survive).
                    if let Some(corr) = &p.correlation_id
                        && map.by_corr.get(corr).map(String::as_str) == Some(key.as_str())
                    {
                        map.by_corr.remove(corr);
                    }
                    // For TTL-expired entries the requester is still
                    // waiting on `rx`; surface the cancelled outcome
                    // so it can react. For requester-crashed entries
                    // the receiver has already been dropped, so the
                    // send returns Err — that's expected and harmless.
                    let _ = p.sender.send(ApprovalOutcome::cancelled());
                }
            }
        }
        count
    }

    /// Producer side: returns `(correlation_id, future)`. The
    /// `correlation_id` is emitted on the wire as
    /// `ApprovalRequired.correlation_id` (and, for backwards-compat,
    /// also as `resume_token` — same opaque value); the future
    /// resolves when the host's `ApprovalResume` command arrives OR
    /// when the TTL reaper auto-cancels.
    ///
    /// The `_req` argument is accepted for ergonomic symmetry — current
    /// implementation only generates a correlation id. A future
    /// iteration may surface the request to a host-side queue/log.
    pub async fn request(
        &self,
        _req: ApprovalRequest,
    ) -> (String, oneshot::Receiver<ApprovalOutcome>) {
        self.request_with_ttl(_req, self.ttl).await
    }

    /// Per-request TTL override. Used by tests; production callers
    /// should use [`request`] which inherits the bridge default.
    pub async fn request_with_ttl(
        &self,
        _req: ApprovalRequest,
        ttl: Duration,
    ) -> (String, oneshot::Receiver<ApprovalOutcome>) {
        // GHSA-8r7g: the token IS a random secret, so it doubles as its own
        // public handle here — there is no separate model-known correlation id
        // to protect (the caller emits this same value as both `resume_token`
        // and `correlation_id`). No `by_corr` entry is needed.
        let resume_token = format!("apr-{}", uuid::Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        let expires_at = Instant::now() + ttl;
        self.pending.lock().await.by_token.insert(
            resume_token.clone(),
            Pending {
                sender: tx,
                expires_at,
                correlation_id: None,
            },
        );
        self.refresh_redactor().await;
        (resume_token, rx)
    }

    /// Register a pending approval indexed by a **caller-supplied** public
    /// `correlation_id` (e.g. the egress-consent `call_id`), so a LOCAL resolver
    /// can resolve by that stable, self-describing handle.
    ///
    /// GHSA-8r7g: the pending entry is keyed internally by a fresh SECRET
    /// `resume_token` (`apr-{uuid}`), which is returned and is the ONLY value a
    /// wire/host peer may present to [`resolve`](Self::resolve). The public
    /// `correlation_id` (which a model can see — it appears in the tool_calls
    /// the model itself emitted) is indexed in `by_corr` and only ever resolves
    /// via [`resolve_by_correlation`](Self::resolve_by_correlation), the local
    /// path. This severs the old "resume_token == call_id" identity where a
    /// model-known id could self-approve over the wire. Callers MUST emit the
    /// returned secret as `ApprovalRequired.resume_token` and the
    /// `correlation_id` as `ApprovalRequired.correlation_id`. A duplicate
    /// `correlation_id` re-points `by_corr` to the newer token (last writer
    /// wins); the older token remains resolvable by the wire until it is reaped.
    pub async fn request_with_id(
        &self,
        correlation_id: String,
        _req: ApprovalRequest,
    ) -> (String, oneshot::Receiver<ApprovalOutcome>) {
        self.request_with_id_and_ttl(correlation_id, _req, self.ttl)
            .await
    }

    /// Like [`request_with_id`](Self::request_with_id) but with an explicit TTL
    /// instead of the bridge default. The Crucible front door uses this with
    /// [`CRUCIBLE_APPROVAL_TTL`] so an expensive multi-vendor proposal card is
    /// not auto-cancelled mid-deliberation by the 5-minute default (spec §7).
    pub async fn request_with_id_and_ttl(
        &self,
        correlation_id: String,
        _req: ApprovalRequest,
        ttl: Duration,
    ) -> (String, oneshot::Receiver<ApprovalOutcome>) {
        let resume_token = format!("apr-{}", uuid::Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        let expires_at = Instant::now() + ttl;
        {
            let mut map = self.pending.lock().await;
            map.by_token.insert(
                resume_token.clone(),
                Pending {
                    sender: tx,
                    expires_at,
                    correlation_id: Some(correlation_id.clone()),
                },
            );
            map.by_corr.insert(correlation_id, resume_token.clone());
        }
        self.refresh_redactor().await;
        (resume_token, rx)
    }

    /// Consumer side for the WIRE/host path: called from the engine's command
    /// loop when `ApprovalResume { resume_token }` arrives off the JSON stream.
    /// The argument MUST be the SECRET `resume_token` (`apr-{uuid}`) the bridge
    /// minted and emitted — GHSA-8r7g: a model-known `call_id` is NOT accepted
    /// here (it is a `correlation_id`, resolvable only via the local
    /// [`resolve_by_correlation`](Self::resolve_by_correlation)). Returns false
    /// if the token is unknown (host sent a stale, expired, or guessed value).
    pub async fn resolve(&self, resume_token: &str, outcome: ApprovalOutcome) -> bool {
        let resolved = {
            let mut map = self.pending.lock().await;
            if let Some(pending) = map.by_token.remove(resume_token) {
                // Purge the secondary index — but only if it still points to
                // THIS token. GHSA-8r7g duplicate-overwrite: if the same
                // correlation id was re-registered to a newer secret, that
                // newer mapping must survive resolution of the stale token.
                if let Some(corr) = &pending.correlation_id
                    && map.by_corr.get(corr).map(String::as_str) == Some(resume_token)
                {
                    map.by_corr.remove(corr);
                }
                let _ = pending.sender.send(outcome);
                true
            } else {
                false
            }
        };
        if resolved {
            self.refresh_redactor().await;
        }
        resolved
    }

    /// Consumer side for the LOCAL path: resolve by the public `correlation_id`
    /// (a caller-supplied, possibly model-known `call_id`). Used by in-process
    /// resolvers that hold the correlation handle — a TUI keypress or an egress
    /// event — NOT by anything reading the wire. GHSA-8r7g: this is safe
    /// precisely because it is unreachable from the protocol ingress; a wire
    /// peer can only present a `resume_token`, which routes to [`resolve`].
    /// Returns false if the correlation id has no pending entry.
    pub async fn resolve_by_correlation(
        &self,
        correlation_id: &str,
        outcome: ApprovalOutcome,
    ) -> bool {
        let resolved = {
            let mut map = self.pending.lock().await;
            if let Some(token) = map.by_corr.remove(correlation_id) {
                if let Some(pending) = map.by_token.remove(&token) {
                    let _ = pending.sender.send(outcome);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if resolved {
            self.refresh_redactor().await;
        }
        resolved
    }

    /// Snapshot of currently-pending correlation ids. Consumed by
    /// `protocol_sink::redact_tokens` to scrub active tokens from
    /// streaming tool output (defense-in-depth — the wire surface
    /// already carries the same ids, but tool output streams MUST
    /// NOT echo them back where a snooping tool could lift them).
    pub async fn active_tokens(&self) -> Vec<String> {
        self.pending.lock().await.by_token.keys().cloned().collect()
    }

    /// Test helper: snapshot the currently-pending correlation ids.
    /// Used by integration tests that race a script dispatch against
    /// an approver task. Not for production callers.
    pub async fn pending_tokens(&self) -> Vec<String> {
        self.active_tokens().await
    }

    /// Test helper: number of currently-pending entries.
    pub async fn pending_count(&self) -> usize {
        self.pending.lock().await.by_token.len()
    }
}

/// W7 S4: blanket adapter so `ApprovalBridge` satisfies
/// `wcore_tools::script::ApprovalProducer` without `wcore-tools`
/// depending on `wcore-agent`. The wcore-tools-side trait defines its
/// own `ApprovalOutcomeLite`; this impl unwraps from local
/// `ApprovalOutcome` after the oneshot resolves by chaining a tiny
/// converter task.
#[async_trait::async_trait]
impl wcore_tools::script::ApprovalProducer for ApprovalBridge {
    async fn request(
        &self,
        call_id: String,
        reason: String,
        context: String,
    ) -> (
        String,
        tokio::sync::oneshot::Receiver<wcore_tools::script::ApprovalOutcomeLite>,
    ) {
        let (correlation_id, rx) = self
            .request(ApprovalRequest {
                call_id,
                reason,
                context,
            })
            .await;
        // Convert ApprovalOutcome → ApprovalOutcomeLite via a forwarder task.
        let (tx_lite, rx_lite) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if let Ok(outcome) = rx.await {
                let _ = tx_lite.send(wcore_tools::script::ApprovalOutcomeLite {
                    approved: outcome.approved,
                    modifications: outcome.modifications,
                });
            }
        });
        (correlation_id, rx_lite)
    }
}

/// W7 S4: thin adapter that bridges a parent `OutputSink` to the
/// `wcore_tools::script::ScriptOutputSink` trait, gated on
/// `with_hitl_suspend(true)` at the parent sink builder. Provides the
/// emit-only side that `ScriptTool::with_approval` requires.
pub struct OutputSinkScriptAdapter {
    output: Arc<dyn crate::output::OutputSink>,
}

impl OutputSinkScriptAdapter {
    pub fn new(output: Arc<dyn crate::output::OutputSink>) -> Self {
        Self { output }
    }
}

impl wcore_tools::script::ScriptOutputSink for OutputSinkScriptAdapter {
    fn emit_approval_required(
        &self,
        call_id: &str,
        resume_token: &str,
        reason: &str,
        context: &str,
    ) {
        self.output
            .emit_approval_required(call_id, resume_token, reason, context);
    }
    fn emit_suspend(&self, reason: &str, resume_token: &str) {
        self.output.emit_suspend(reason, resume_token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn approval_round_trip_approved() {
        let bridge = ApprovalBridge::new();
        let (correlation_id, rx) = bridge
            .request(ApprovalRequest {
                call_id: "c-1".into(),
                reason: "test".into(),
                context: "ctx".into(),
            })
            .await;
        let bridge2 = bridge.clone();
        let cid_clone = correlation_id.clone();
        let resolver = tokio::spawn(async move {
            bridge2
                .resolve(
                    &cid_clone,
                    ApprovalOutcome {
                        approved: true,
                        modifications: None,
                    },
                )
                .await
        });
        let outcome = rx.await.unwrap();
        assert!(outcome.approved);
        assert!(
            resolver.await.unwrap(),
            "resolve must report a found pending request"
        );
    }

    #[tokio::test]
    async fn request_with_id_and_ttl_honors_per_request_expiry() {
        // The Crucible front door registers its card with CRUCIBLE_APPROVAL_TTL
        // so a slow human decision is NOT reaped by the 5-minute default. Prove
        // the per-request TTL is honored: a zero-TTL entry is reaped while a
        // long-TTL entry survives the SAME reap.
        let bridge = ApprovalBridge::new();
        let (_tok_short, rx_short) = bridge
            .request_with_id_and_ttl(
                "short".into(),
                ApprovalRequest {
                    call_id: "c".into(),
                    reason: "r".into(),
                    context: "x".into(),
                },
                Duration::from_secs(0),
            )
            .await;
        let (tok_long, rx_long) = bridge
            .request_with_id_and_ttl(
                "long".into(),
                ApprovalRequest {
                    call_id: "c".into(),
                    reason: "r".into(),
                    context: "x".into(),
                },
                CRUCIBLE_APPROVAL_TTL,
            )
            .await;
        let reaped = bridge.reap_now().await;
        assert_eq!(
            reaped, 1,
            "only the already-expired short-TTL entry is reaped"
        );
        assert!(
            !rx_short.await.unwrap().approved,
            "the reaped entry resolves to cancelled (no spend)"
        );
        // The long-TTL crucible card must still be pending + resolvable by its
        // SECRET token (the wire/host path). GHSA-8r7g: the correlation string
        // ("long") no longer resolves via `resolve` — only the secret does.
        assert!(
            bridge
                .resolve(
                    &tok_long,
                    ApprovalOutcome {
                        approved: true,
                        modifications: None
                    }
                )
                .await,
            "the long-TTL card must survive a reap that expired the short one"
        );
        assert!(rx_long.await.unwrap().approved);
    }

    #[tokio::test]
    async fn approval_resolve_unknown_token_returns_false() {
        let bridge = ApprovalBridge::new();
        assert!(
            !bridge
                .resolve(
                    "nope",
                    ApprovalOutcome {
                        approved: false,
                        modifications: None
                    }
                )
                .await
        );
    }

    #[tokio::test]
    async fn approval_round_trip_rejected() {
        let bridge = ApprovalBridge::new();
        let (correlation_id, rx) = bridge
            .request(ApprovalRequest {
                call_id: "c-1".into(),
                reason: "test".into(),
                context: "ctx".into(),
            })
            .await;
        bridge
            .resolve(
                &correlation_id,
                ApprovalOutcome {
                    approved: false,
                    modifications: None,
                },
            )
            .await;
        let outcome = rx.await.unwrap();
        assert!(!outcome.approved);
    }

    #[tokio::test]
    async fn reap_expired_cancels_pending() {
        let bridge = ApprovalBridge::with_ttl(Duration::from_millis(50));
        let (_correlation_id, rx) = bridge
            .request(ApprovalRequest {
                call_id: "c-1".into(),
                reason: "test".into(),
                context: "ctx".into(),
            })
            .await;
        // Wait for the TTL to elapse, then reap manually.
        tokio::time::sleep(Duration::from_millis(80)).await;
        let n = bridge.reap_now().await;
        assert_eq!(n, 1, "reaper must collect the expired entry");
        let outcome = rx.await.unwrap();
        assert!(!outcome.approved, "expired outcome must be !approved");
        assert_eq!(bridge.pending_count().await, 0);
    }

    #[tokio::test]
    async fn active_tokens_returns_in_flight_correlation_ids() {
        let bridge = ApprovalBridge::new();
        let (cid_a, _rx_a) = bridge
            .request(ApprovalRequest {
                call_id: "a".into(),
                reason: "".into(),
                context: "".into(),
            })
            .await;
        let (cid_b, _rx_b) = bridge
            .request(ApprovalRequest {
                call_id: "b".into(),
                reason: "".into(),
                context: "".into(),
            })
            .await;
        let active = bridge.active_tokens().await;
        assert!(active.contains(&cid_a));
        assert!(active.contains(&cid_b));
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn approval_request_is_clone() {
        let req = ApprovalRequest {
            call_id: "c-1".into(),
            reason: "r".into(),
            context: "ctx".into(),
        };
        let req2 = req.clone();
        assert_eq!(req.call_id, req2.call_id);
    }
}
