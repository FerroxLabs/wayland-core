//! B2.3 — the async egress policy installed into the B1 `wcore-egress`
//! chokepoint at bootstrap.
//!
//! Wraps the pure [`classify`](super::classify) core with the live allow state
//! and the posture (enforce / off — the C8 hard off switch). The `check` runs
//! on every outbound request after the URL is built.
//!
//! ## Posture (B2.3)
//!
//! - **Allowlisted** destination → allow.
//! - **Exfil-class** to a non-allowlisted host (POST/PUT/PATCH body,
//!   shared-platform host, or GET/HEAD carrying a long/high-entropy
//!   path/query) → **deny**, with an actionable message. This is the exfil
//!   boundary: data cannot leave to an unapproved host.
//! - **Plain new-destination read** (`Ask` verdict — a data-less GET/HEAD) →
//!   allow for now. Nothing sensitive leaves on a data-less read; the
//!   interactive `ask`-with-memory doorbell (which would prompt + persist an
//!   "always" allow here) is the B2.5 upgrade of [`resolve_ask`].
//! - **Off** posture → allow everything (operator accepted the risk via the
//!   config-file switch + explicit CLI flag — C8).

use std::sync::Arc;
use std::sync::RwLock as StdRwLock;

use tokio::sync::RwLock;
use wcore_egress::{EgressDecision, EgressPolicy};

use super::classify::{AllowList, EgressVerdict, classify};
use super::consent::{ConsentDecision, ConsentDoorbell};

/// Whether the egress boundary is enforced or disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressPosture {
    /// On by default — classify and gate.
    Enforce,
    /// The hard off switch (C8): allow all egress. Reached only via the
    /// config-file `[security] enabled = false` plus the explicit
    /// `--i-accept-exfil-risk` CLI flag (wired at bootstrap, B2.4).
    Off,
}

/// The real egress policy. Cheap to clone (Arc-shared allow state + doorbell).
#[derive(Clone)]
pub struct AgentEgressPolicy {
    allow: Arc<RwLock<AllowList>>,
    posture: EgressPosture,
    /// B2.5 — the consent doorbell, injected into this session's policy when
    /// an interactive surface exists. Shared only by clones of that policy.
    /// `None` ⇒ no consent surface ⇒ `Ask` falls back to allow (see
    /// [`resolve_ask`](Self::resolve_ask)).
    doorbell: Arc<StdRwLock<Option<Arc<dyn ConsentDoorbell>>>>,
}

impl AgentEgressPolicy {
    /// Build an enforcing policy over the given allowlist.
    pub fn enforcing(allow: AllowList) -> Self {
        Self {
            allow: Arc::new(RwLock::new(allow)),
            posture: EgressPosture::Enforce,
            doorbell: Arc::new(StdRwLock::new(None)),
        }
    }

    /// Build a disabled (allow-all) policy — the hard off switch.
    pub fn disabled() -> Self {
        Self {
            allow: Arc::new(RwLock::new(AllowList::default())),
            posture: EgressPosture::Off,
            doorbell: Arc::new(StdRwLock::new(None)),
        }
    }

    /// Shared handle to the live allow state, so the consent doorbell can
    /// persist an "always" allow that takes effect immediately.
    pub fn allow_handle(&self) -> Arc<RwLock<AllowList>> {
        self.allow.clone()
    }

    /// Attach the consent doorbell (B2.5). Idempotent/last-writer-wins within
    /// one session policy.
    pub fn set_doorbell(&self, doorbell: Arc<dyn ConsentDoorbell>) {
        if let Ok(mut slot) = self.doorbell.write() {
            *slot = Some(doorbell);
        }
    }

    /// Resolve an `Ask` verdict (a data-less read to a new destination).
    ///
    /// With no doorbell wired (headless / one-shot / tests) → allow: nothing
    /// sensitive leaves on a data-less read, and the exfil boundary stays
    /// hard-denied regardless. With a doorbell, prompt once/always/no; on
    /// "always" persist the registrable domain so subsequent reaches are silent.
    async fn resolve_ask(&self, host: &str, registrable: &str, reason: &str) -> EgressDecision {
        let doorbell = self.doorbell.read().ok().and_then(|slot| slot.clone());
        let Some(doorbell) = doorbell else {
            return EgressDecision::Allow;
        };
        match doorbell.ask(host, registrable, reason).await {
            ConsentDecision::Once => EgressDecision::Allow,
            ConsentDecision::Always => {
                // Persist to the live allowlist (immediate effect). `Ask` is
                // never a shared-platform host (those classify as `Exfil`), so
                // `allow_domain` — which refuses shared-platform apexes — is the
                // right tier here.
                self.allow.write().await.allow_domain(registrable);
                EgressDecision::Allow
            }
            ConsentDecision::No => EgressDecision::Deny {
                reason: format!(
                    "Egress to `{host}` was declined at the consent prompt. \
                     Approve it next time, or add it under \
                     `[security] egress_allow = [..]` in your config."
                ),
            },
        }
    }

    /// Resolve an `Exfil` verdict — deny with an actionable message.
    fn resolve_exfil(&self, host: &str, reason: &str) -> EgressDecision {
        EgressDecision::Deny {
            reason: format!(
                "{reason}. Egress to `{host}` is blocked by the security policy. \
                 Add it under `[security] egress_allow = [..]` in your config, or \
                 disable the policy with `[security] enabled = false` + \
                 `--i-accept-exfil-risk` if you accept the exfiltration risk."
            ),
        }
    }
}

#[async_trait::async_trait]
impl EgressPolicy for AgentEgressPolicy {
    async fn check(&self, request: &reqwest::Request) -> EgressDecision {
        if self.posture == EgressPosture::Off {
            return EgressDecision::Allow;
        }
        // reqwest carries a `url::Url` directly — no re-parse.
        let url = request.url();
        let verdict = {
            let allow = self.allow.read().await;
            classify(request.method(), url, &allow)
        };
        match verdict {
            EgressVerdict::Allow => EgressDecision::Allow,
            EgressVerdict::Ask {
                host,
                registrable,
                reason,
            } => self.resolve_ask(&host, &registrable, &reason).await,
            EgressVerdict::Exfil { host, reason, .. } => self.resolve_exfil(&host, &reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: reqwest::Method, url: &str) -> reqwest::Request {
        reqwest::Request::new(method, url.parse().unwrap())
    }

    fn allow_with(domains: &[&str]) -> AllowList {
        let mut a = AllowList::default();
        for d in domains {
            a.allow_domain(d);
        }
        a
    }

    #[tokio::test]
    async fn off_posture_allows_everything() {
        let p = AgentEgressPolicy::disabled();
        // Even a blatant POST-exfil to a request-bin is allowed when off.
        let d = p
            .check(&req(reqwest::Method::POST, "https://webhook.site/abc"))
            .await;
        assert!(matches!(d, EgressDecision::Allow));
    }

    #[tokio::test]
    async fn allowlisted_post_is_allowed() {
        let p = AgentEgressPolicy::enforcing(allow_with(&["anthropic.com"]));
        let d = p
            .check(&req(
                reqwest::Method::POST,
                "https://api.anthropic.com/v1/messages",
            ))
            .await;
        assert!(matches!(d, EgressDecision::Allow));
    }

    #[tokio::test]
    async fn post_to_non_allowlisted_host_is_denied() {
        let p = AgentEgressPolicy::enforcing(AllowList::default());
        let d = p
            .check(&req(reqwest::Method::POST, "https://evil.test/collect"))
            .await;
        match d {
            EgressDecision::Deny { reason } => assert!(reason.contains("evil.test")),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn data_bearing_get_to_non_allowlisted_host_is_denied() {
        let p = AgentEgressPolicy::enforcing(AllowList::default());
        let secret = "A".repeat(120);
        let d = p
            .check(&req(
                reqwest::Method::GET,
                &format!("https://evil.test/x?d={secret}"),
            ))
            .await;
        assert!(matches!(d, EgressDecision::Deny { .. }));
    }

    #[tokio::test]
    async fn plain_new_get_is_allowed_in_b2_3() {
        // A data-less read to a new domain: allowed (the doorbell upgrade later
        // makes this prompt). Nothing sensitive leaves.
        let p = AgentEgressPolicy::enforcing(AllowList::default());
        let d = p
            .check(&req(reqwest::Method::GET, "https://react.dev/learn"))
            .await;
        assert!(matches!(d, EgressDecision::Allow));
    }

    #[tokio::test]
    async fn shared_platform_read_is_denied_even_dataless() {
        let p = AgentEgressPolicy::enforcing(AllowList::default());
        let d = p
            .check(&req(
                reqwest::Method::GET,
                "https://victim.s3.amazonaws.com/o",
            ))
            .await;
        assert!(matches!(d, EgressDecision::Deny { .. }));
    }

    // ── B2.5 consent doorbell ────────────────────────────────────────────────

    /// A doorbell stub that returns a fixed decision and records each ask.
    struct FixedDoorbell {
        decision: ConsentDecision,
        asked: std::sync::Mutex<Vec<String>>,
    }

    impl FixedDoorbell {
        fn new(decision: ConsentDecision) -> Arc<Self> {
            Arc::new(Self {
                decision,
                asked: std::sync::Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl ConsentDoorbell for FixedDoorbell {
        async fn ask(&self, _host: &str, registrable: &str, _reason: &str) -> ConsentDecision {
            self.asked.lock().unwrap().push(registrable.to_string());
            self.decision
        }
    }

    #[tokio::test]
    async fn ask_with_doorbell_once_allows_but_does_not_persist() {
        let p = AgentEgressPolicy::enforcing(AllowList::default());
        let bell = FixedDoorbell::new(ConsentDecision::Once);
        p.set_doorbell(bell.clone());

        let d = p
            .check(&req(reqwest::Method::GET, "https://react.dev/learn"))
            .await;
        assert!(matches!(d, EgressDecision::Allow));
        assert_eq!(bell.asked.lock().unwrap().as_slice(), &["react.dev"]);

        // "Once" must NOT persist: a second reach asks again.
        let _ = p
            .check(&req(reqwest::Method::GET, "https://react.dev/reference"))
            .await;
        assert_eq!(bell.asked.lock().unwrap().len(), 2, "Once never persists");
    }

    #[tokio::test]
    async fn ask_with_doorbell_always_allows_and_persists_silently() {
        let p = AgentEgressPolicy::enforcing(AllowList::default());
        let bell = FixedDoorbell::new(ConsentDecision::Always);
        p.set_doorbell(bell.clone());

        // First reach prompts and is allowed.
        let d = p
            .check(&req(reqwest::Method::GET, "https://react.dev/learn"))
            .await;
        assert!(matches!(d, EgressDecision::Allow));
        assert_eq!(bell.asked.lock().unwrap().len(), 1);

        // "Always" persisted the registrable domain → a subsequent reach
        // (even a subdomain) is allowed WITHOUT prompting again.
        let d2 = p
            .check(&req(reqwest::Method::GET, "https://api.react.dev/v1"))
            .await;
        assert!(matches!(d2, EgressDecision::Allow));
        assert_eq!(
            bell.asked.lock().unwrap().len(),
            1,
            "Always persists the domain — no second prompt"
        );
    }

    #[tokio::test]
    async fn ask_with_doorbell_no_denies() {
        let p = AgentEgressPolicy::enforcing(AllowList::default());
        p.set_doorbell(FixedDoorbell::new(ConsentDecision::No));
        let d = p
            .check(&req(reqwest::Method::GET, "https://react.dev/learn"))
            .await;
        match d {
            EgressDecision::Deny { reason } => assert!(reason.contains("react.dev")),
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn doorbell_is_not_consulted_for_exfil_or_allowlisted() {
        // Exfil is hard-denied without ever ringing the doorbell, and an
        // allowlisted host is allowed without a prompt.
        let p = AgentEgressPolicy::enforcing(allow_with(&["anthropic.com"]));
        let bell = FixedDoorbell::new(ConsentDecision::No);
        p.set_doorbell(bell.clone());

        // Allowlisted POST → Allow, no prompt.
        let d = p
            .check(&req(
                reqwest::Method::POST,
                "https://api.anthropic.com/v1/messages",
            ))
            .await;
        assert!(matches!(d, EgressDecision::Allow));

        // Exfil POST → Deny, no prompt (the doorbell would have said No anyway,
        // but the point is it is never asked — exfil is non-negotiable).
        let d = p
            .check(&req(reqwest::Method::POST, "https://evil.test/collect"))
            .await;
        assert!(matches!(d, EgressDecision::Deny { .. }));

        assert!(
            bell.asked.lock().unwrap().is_empty(),
            "doorbell must only be rung for the Ask verdict"
        );
    }
}
