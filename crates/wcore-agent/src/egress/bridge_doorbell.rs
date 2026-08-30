//! B2.5 — a [`ConsentDoorbell`] backed by the engine's [`ApprovalBridge`].
//!
//! This is the production doorbell: it rides the **existing** approval journey
//! (the same `ApprovalRequired` event + bridge resolution the ScriptTool HITL
//! path uses). On an `Ask` verdict the policy calls [`ask`](BridgeConsentDoorbell::ask),
//! which:
//!   1. registers a pending approval on the bridge (`request` → correlation id +
//!      a one-shot receiver),
//!   2. emits `ApprovalRequired` through the [`OutputSink`] so the host renders
//!      a prompt,
//!   3. awaits the operator's decision and maps it to a [`ConsentDecision`].
//!
//! The host resolves the request through the engine's existing
//! `ApprovalResume` arm (`engine.approval_bridge().resolve(...)`). A binary
//! approve/deny maps to `Once`/`No`; an `always` scope — carried in the
//! resolved [`ApprovalOutcome::modifications`] as `{"egress_scope":"always"}` —
//! maps to `Always`, which the policy persists. A closed channel or a TTL
//! timeout (the operator walked away) is treated as **deny** — fail-closed,
//! since a doorbell being present means an interactive answer was expected.

use std::sync::Arc;

use crate::approval::{ApprovalBridge, ApprovalRequest};
use crate::output::OutputSink;

use super::consent::{ConsentDecision, ConsentDoorbell};

/// A consent doorbell that surfaces the prompt through the engine's approval
/// bridge + output sink.
pub struct BridgeConsentDoorbell {
    bridge: Arc<ApprovalBridge>,
    sink: Arc<dyn OutputSink>,
}

impl BridgeConsentDoorbell {
    /// Wire a doorbell to the engine's shared approval bridge and output sink.
    pub fn new(bridge: Arc<ApprovalBridge>, sink: Arc<dyn OutputSink>) -> Self {
        Self { bridge, sink }
    }
}

/// Decode the once/always scope a host may attach to its approval. Absent or
/// unrecognized ⇒ `Once` (a plain approve does not persist).
fn scope_is_always(modifications: &Option<serde_json::Value>) -> bool {
    modifications
        .as_ref()
        .and_then(|v| v.get("egress_scope"))
        .and_then(|s| s.as_str())
        .map(|s| s.eq_ignore_ascii_case("always"))
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl ConsentDoorbell for BridgeConsentDoorbell {
    async fn ask(&self, host: &str, registrable: &str, reason: &str) -> ConsentDecision {
        // The `call_id` is the PUBLIC correlation handle (`request_with_id`
        // indexes the pending entry under it), so a LOCAL resolver (a TUI
        // keypress) resolves via `resolve_by_correlation(call_id)` with the id
        // it already has. GHSA-8r7g: the bridge mints a SEPARATE secret
        // `resume_token`, returned below, which is what the host/wire echoes to
        // resolve — a model-known `call_id` can no longer self-approve. A uuid
        // keeps concurrent asks (even to the same host) from colliding. The
        // `egress:` prefix lets the TUI/host recognize this as egress consent.
        // wayland#1219: ask BEFORE registering a pending approval. On a sink
        // that cannot render one (`--json-stream` without
        // `with_hitl_suspend`), `emit_approval_required` below is a silent
        // no-op: nothing reaches the host, nothing resolves the oneshot, and
        // `rx.await` blocks until the TTL reaper cancels it ~300s later —
        // which the policy then reported as a decline of a prompt that was
        // never shown. Return a decision that says so, immediately.
        if !self.sink.approval_surface_available() {
            return ConsentDecision::Unavailable;
        }

        let call_id = format!("egress:{}", uuid::Uuid::new_v4());
        let prompt = format!("Allow network access to `{registrable}`? ({reason})");
        // Structured context so a host UI can render richly and a resolver can
        // recognize this as an egress-consent request (vs a tool approval).
        let context = serde_json::json!({
            "kind": "egress_consent",
            "host": host,
            "registrable": registrable,
        })
        .to_string();

        let (resume_token, rx) = self
            .bridge
            .request_with_id(
                call_id.clone(),
                ApprovalRequest {
                    call_id: call_id.clone(),
                    reason: prompt.clone(),
                    context: context.clone(),
                },
            )
            .await;

        // Surface the prompt. GHSA-8r7g: emit the secret `resume_token` (what
        // the host echoes back to resolve over the wire), with `call_id` as the
        // public correlation handle. This emit is a no-op on a sink without an
        // approval surface; wayland#1219 replaced the comment that merely
        // ASSERTED "only installed where a real surface exists" with the
        // `approval_surface_available()` check above, which enforces it.
        self.sink
            .emit_approval_required(&call_id, &resume_token, &prompt, &context);

        match rx.await {
            Ok(outcome) if outcome.approved => {
                if scope_is_always(&outcome.modifications) {
                    ConsentDecision::Always
                } else {
                    ConsentDecision::Once
                }
            }
            // wayland#1219: fail-closed either way, but say which happened.
            // `cancellation` is `Some` ONLY when the bridge resolved this
            // itself with no host answer (#1083) — TTL reap or host-stream
            // EOF. That is silence, not a decline, and reporting it as a
            // decline is the lie this ticket is about. `None` means a human
            // or host actually decided: that is a real `No`.
            Ok(outcome) => {
                if outcome.cancellation.is_some() {
                    ConsentDecision::Unanswered
                } else {
                    ConsentDecision::No
                }
            }
            // Sender dropped without resolving — nothing was ever decided.
            Err(_) => ConsentDecision::Unanswered,
        }
    }
}

/// wayland#1219 — install [`BridgeConsentDoorbell`] on `policy`, but ONLY if
/// `output` can actually render an approval the operator can answer.
///
/// This is the enforcement point for the doorbell's own premise. Before
/// wayland#1219, bootstrap wired the doorbell unconditionally onto every
/// session egress policy; on the `--json-stream` path the sink's
/// hitl_suspend gate was permanently shut, so an `Ask` verdict stalled for
/// the whole `DEFAULT_APPROVAL_TTL` and then denied with "declined at the
/// consent prompt".
///
/// Returns whether a doorbell was installed. Declining to install leaves the
/// policy in its documented no-doorbell posture: a data-less GET to a new
/// domain is allowed, and the `Exfil` verdict stays hard-denied regardless —
/// so this never widens the exfil boundary.
pub fn install_consent_doorbell(
    policy: &super::policy::AgentEgressPolicy,
    bridge: Arc<ApprovalBridge>,
    output: Arc<dyn OutputSink>,
) -> bool {
    if !output.approval_surface_available() {
        return false;
    }
    policy.set_doorbell(Arc::new(BridgeConsentDoorbell::new(bridge, output)));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ApprovalOutcome;
    use crate::output::null_sink::NullSink;
    use crate::test_utils::TestSink;

    /// wayland#1219: these used to run over `NullSink`, whose
    /// `emit_approval_required` is the trait's no-op default. That is exactly
    /// the mute surface this ticket is about — the doorbell now refuses to
    /// park on one, so the once/always/no cases need a sink that really
    /// renders. `TestSink` does.
    fn doorbell() -> (Arc<ApprovalBridge>, BridgeConsentDoorbell) {
        let bridge = Arc::new(ApprovalBridge::new());
        let db = BridgeConsentDoorbell::new(bridge.clone(), Arc::new(TestSink::new()));
        (bridge, db)
    }

    #[tokio::test]
    async fn approve_without_scope_is_once() {
        let (bridge, db) = doorbell();
        let resolver = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                // Wait for the request to register, then approve it.
                loop {
                    let pending = bridge.pending_tokens().await;
                    if let Some(token) = pending.first() {
                        bridge
                            .resolve(
                                token,
                                ApprovalOutcome {
                                    approved: true,
                                    modifications: None,
                                    cancellation: None,
                                },
                            )
                            .await;
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
        };
        let decision = db.ask("react.dev", "react.dev", "data-less GET").await;
        resolver.await.unwrap();
        assert_eq!(decision, ConsentDecision::Once);
    }

    #[tokio::test]
    async fn approve_with_always_scope_is_always() {
        let (bridge, db) = doorbell();
        let resolver = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                loop {
                    let pending = bridge.pending_tokens().await;
                    if let Some(token) = pending.first() {
                        bridge
                            .resolve(
                                token,
                                ApprovalOutcome {
                                    approved: true,
                                    modifications: Some(serde_json::json!({
                                        "egress_scope": "always"
                                    })),
                                    cancellation: None,
                                },
                            )
                            .await;
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
        };
        let decision = db.ask("react.dev", "react.dev", "data-less GET").await;
        resolver.await.unwrap();
        assert_eq!(decision, ConsentDecision::Always);
    }

    /// wayland#1219: `ApprovalOutcome::cancelled()` is the TTL reaper's
    /// outcome, not an operator's. It now maps to `Unanswered`, and this test
    /// was renamed to say what it actually drives — it never exercised a
    /// human deny.
    #[tokio::test]
    async fn a_reaped_approval_is_unanswered_not_a_decline() {
        let (bridge, db) = doorbell();
        let resolver = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                loop {
                    let pending = bridge.pending_tokens().await;
                    if let Some(token) = pending.first() {
                        bridge.resolve(token, ApprovalOutcome::cancelled()).await;
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
        };
        let decision = db.ask("evil.test", "evil.test", "data-less GET").await;
        resolver.await.unwrap();
        assert_eq!(
            decision,
            ConsentDecision::Unanswered,
            "a bridge-reaped approval is silence, not a decline"
        );
    }

    /// An operator who really said no. `ApprovalOutcome` with no
    /// `cancellation` is the shape a host/operator decision has (#1083).
    #[tokio::test]
    async fn an_operator_deny_is_no() {
        let (bridge, db) = doorbell();
        let resolver = {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                loop {
                    let pending = bridge.pending_tokens().await;
                    if let Some(token) = pending.first() {
                        bridge
                            .resolve(
                                token,
                                ApprovalOutcome {
                                    approved: false,
                                    modifications: None,
                                    cancellation: None,
                                },
                            )
                            .await;
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
        };
        let decision = db.ask("evil.test", "evil.test", "data-less GET").await;
        resolver.await.unwrap();
        assert_eq!(decision, ConsentDecision::No);
    }

    /// wayland#1219 — the doorbell must not park on a sink that cannot render
    /// the prompt. `NullSink` inherits the trait's no-op
    /// `emit_approval_required`, which is precisely the `--json-stream`
    /// situation the ticket reports.
    #[tokio::test]
    async fn a_mute_sink_is_unavailable_and_does_not_park() {
        let bridge = Arc::new(ApprovalBridge::new());
        let db = BridgeConsentDoorbell::new(bridge.clone(), Arc::new(NullSink));
        let decision = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            db.ask("react.dev", "react.dev", "data-less GET"),
        )
        .await
        .expect("the doorbell parked on a sink with no approval surface");
        assert_eq!(decision, ConsentDecision::Unavailable);
        assert!(
            bridge.pending_tokens().await.is_empty(),
            "a request that can never be shown must not be registered"
        );
    }

    /// wayland#1219 — the install guard, both arms.
    #[test]
    fn the_install_guard_refuses_a_mute_sink_and_accepts_a_real_one() {
        use crate::egress::classify::AllowList;
        use crate::egress::policy::AgentEgressPolicy;

        let mute = AgentEgressPolicy::enforcing(AllowList::default());
        let installed =
            install_consent_doorbell(&mute, Arc::new(ApprovalBridge::new()), Arc::new(NullSink));
        assert!(!installed, "installed a blocking doorbell over a mute sink");
        assert!(!mute.has_doorbell());

        let real = AgentEgressPolicy::enforcing(AllowList::default());
        let installed = install_consent_doorbell(
            &real,
            Arc::new(ApprovalBridge::new()),
            Arc::new(TestSink::new()),
        );
        assert!(installed);
        assert!(real.has_doorbell());
    }
}
