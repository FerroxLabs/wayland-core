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
use super::policy::AgentEgressPolicy;

/// Wire the consent doorbell onto `policy` -- but ONLY where `sink` can put
/// the prompt in front of a human. Returns whether it was installed.
///
/// #1180. The doorbell BLOCKS the turn on the operator's answer, so installing
/// one on a surfaceless sink does not degrade to "no prompt": it degrades to a
/// five-minute stall ending in `ConsentDecision::No`, which the policy renders
/// as "declined at the consent prompt" -- naming a prompt that was never
/// shown. The documented fallback for a surfaceless session is the opposite,
/// and is already implemented: with NO doorbell `resolve_ask` allows the
/// data-less read, while the `Exfil` verdict stays hard-denied regardless, so
/// declining to install never widens the exfiltration boundary.
///
/// This is a function rather than a comment on the caller because the caller
/// already had the comment -- `BridgeConsentDoorbell::ask` still says "this
/// doorbell is only installed where a real surface exists" -- and bootstrap
/// installed it unconditionally anyway.
pub fn install_consent_doorbell(
    policy: &AgentEgressPolicy,
    bridge: &Arc<ApprovalBridge>,
    sink: &Arc<dyn OutputSink>,
) -> bool {
    if !sink.can_prompt_for_approval() {
        return false;
    }
    policy.set_doorbell(Arc::new(BridgeConsentDoorbell::new(
        bridge.clone(),
        sink.clone(),
    )));
    true
}

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
        // public correlation handle. A no-op on sinks without an approval
        // surface (then the request only resolves via TTL → deny), so this
        // doorbell is only installed where a real surface exists.
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
            // Explicit deny, or the channel closed / TTL-cancelled (operator
            // walked away): fail-closed.
            Ok(_) | Err(_) => ConsentDecision::No,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::ApprovalOutcome;
    use crate::output::null_sink::NullSink;

    fn doorbell() -> (Arc<ApprovalBridge>, BridgeConsentDoorbell) {
        let bridge = Arc::new(ApprovalBridge::new());
        let db = BridgeConsentDoorbell::new(bridge.clone(), Arc::new(NullSink));
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

    #[tokio::test]
    async fn deny_is_no() {
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
        assert_eq!(decision, ConsentDecision::No);
    }
}
