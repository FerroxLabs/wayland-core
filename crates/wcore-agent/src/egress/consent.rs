//! B2.5 — the interactive ask-with-memory consent doorbell.
//!
//! When the egress policy hits an [`Ask`](super::classify::EgressVerdict::Ask)
//! verdict — a new, non-exfil-shaped destination (a data-less read to a domain
//! the operator hasn't approved yet) — it rings this doorbell instead of
//! silently allowing. The doorbell surfaces a once/always/no prompt to the
//! operator through whatever consent surface is wired (the JSON-stream host's
//! approval modal today; a TUI card next), and returns the decision.
//!
//! The doorbell is async because resolving it is an out-of-band human
//! round-trip. It is injected onto [`AgentEgressPolicy`](super::policy::AgentEgressPolicy)
//! at bootstrap; when no interactive surface exists (headless `-p`, one-shot,
//! tests), no doorbell is set and a data-less read falls back to *allow* — the
//! exfil boundary (the `Exfil` verdict) stays hard-denied regardless, so the
//! fallback never widens the exfil surface.

/// The operator's decision when the agent reaches a new registrable domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentDecision {
    /// Allow this one request; do not remember the domain.
    Once,
    /// Allow and persist the registrable domain to the live allowlist so
    /// subsequent reaches are silent (the "memory" in ask-with-memory).
    Always,
    /// Deny this request.
    No,
    /// wayland#1219: the prompt could not be shown at all — this session has
    /// a doorbell but no surface able to render an `ApprovalRequired` the
    /// operator can answer.
    ///
    /// Distinct from [`No`](Self::No) on purpose. `No` means a human saw the
    /// prompt and said no; this means nobody was ever asked. Collapsing the
    /// two is exactly the defect wayland#1219 reports — the request stalled
    /// for the full approval TTL and the user was then told they had
    /// "declined at the consent prompt".
    ///
    /// Resolves fail-CLOSED (the caller denies), because a doorbell being
    /// installed means an interactive answer was expected; but the message
    /// the user gets must say what actually happened.
    Unavailable,
    /// wayland#1219: the prompt WAS shown, but no answer ever came back — the
    /// approval TTL reaped it, or the host's command stream closed with it
    /// still parked.
    ///
    /// Also distinct from [`No`](Self::No), for the same reason
    /// [`ApprovalCancelCause`](crate::approval::ApprovalCancelCause) exists:
    /// #1083 already established that a bridge self-resolution must stay
    /// distinguishable from a decision a human made. Silence is not a
    /// decline. Fails closed all the same.
    Unanswered,
}

/// The consent surface the egress policy rings on an `Ask` verdict.
///
/// Implementations own the round-trip to the operator (emit an approval prompt,
/// await the answer) and map it to a [`ConsentDecision`]. The default,
/// non-interactive behavior (no doorbell installed) is *allow* — see the module
/// docs — so an implementation is only needed where a real consent surface
/// exists.
#[async_trait::async_trait]
pub trait ConsentDoorbell: Send + Sync {
    /// Ask whether egress to `registrable` (exact request host `host`) should be
    /// allowed. `reason` is a short human-readable string for the prompt
    /// (e.g. "data-less GET to a new domain").
    async fn ask(&self, host: &str, registrable: &str, reason: &str) -> ConsentDecision;
}
