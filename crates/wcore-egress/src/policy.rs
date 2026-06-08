//! The egress policy seam.
//!
//! Every outbound request is shown to an [`EgressPolicy`] immediately before it
//! leaves the process, after the full [`reqwest::Request`] (method, URL,
//! headers, body) is built. B1 ships the pass-through [`AllowAllPolicy`]; B2
//! installs the real allowlist + taint + `ask`-with-memory policy via
//! [`install_global_policy`] — **without touching any call site**, because
//! every [`crate::EgressClient`] built without an explicit policy consults the
//! process-global policy at send time.
//!
//! ## Why a process-global, async policy
//!
//! `wcore-egress` is a near-leaf crate; the real policy needs the approval
//! bridge and config that live in higher crates. So the real policy is
//! *implemented up there* and *installed down here* through a trait object.
//! The check is **async** because the `ask`-with-memory consent doorbell waits
//! on the operator — the policy awaits the approval bridge internally and
//! returns a resolved [`EgressDecision`].

use std::sync::{Arc, OnceLock};

/// What the policy decided about a single outbound request.
#[derive(Debug, Clone)]
pub enum EgressDecision {
    /// Let the request proceed to the network.
    Allow,
    /// Stop the request before it is sent. The reason is surfaced to the
    /// operator via [`crate::EgressError::Denied`].
    Deny {
        /// Human-readable explanation (e.g. `"host not on allowlist: evil.test"`).
        reason: String,
    },
}

/// Decides whether an outbound HTTP request may leave the machine.
///
/// Implementors see the **fully-built** request, so the B2 implementation can
/// inspect the method, the URL path/query (GET-with-data exfil class), the
/// destination host (allowlist), and the body. The check is async so the
/// `ask`-with-memory path can await operator consent; it must otherwise be
/// cheap — it runs on the hot path of every request.
#[async_trait::async_trait]
pub trait EgressPolicy: Send + Sync {
    /// Inspect a request that is about to be sent.
    async fn check(&self, request: &reqwest::Request) -> EgressDecision;
}

/// Permit every request. The behavior before any policy is installed, and a
/// useful explicit opt-out for a single client (`EgressClient::builder().policy(...)`).
#[derive(Debug, Default, Clone, Copy)]
pub struct AllowAllPolicy;

#[async_trait::async_trait]
impl EgressPolicy for AllowAllPolicy {
    async fn check(&self, _request: &reqwest::Request) -> EgressDecision {
        EgressDecision::Allow
    }
}

/// Shared, cheaply-cloneable handle to a policy. An [`crate::EgressClient`]
/// carries one of these; cloning the client clones the `Arc`, not the policy.
pub type SharedPolicy = Arc<dyn EgressPolicy>;

/// The process-wide policy, installed once at boot by the host. Until set,
/// [`GlobalDefaultPolicy`] falls back to allow-all (B1 behavior).
static GLOBAL_POLICY: OnceLock<SharedPolicy> = OnceLock::new();

/// Install the process-wide egress policy. Call once, early in `main()`/boot,
/// before any real outbound traffic. Returns `Err` (with the rejected policy)
/// if a policy was already installed — installation is one-shot so a plugin or
/// late code path cannot swap the boundary out from under the session.
pub fn install_global_policy(policy: SharedPolicy) -> Result<(), SharedPolicy> {
    GLOBAL_POLICY.set(policy)
}

/// True if a global policy has been installed (otherwise egress is allow-all).
pub fn global_policy_installed() -> bool {
    GLOBAL_POLICY.get().is_some()
}

/// The default policy carried by every [`crate::EgressClient`] built without an
/// explicit one. It consults the process-global policy **at send time**, so a
/// client constructed before [`install_global_policy`] still honors the policy
/// once it lands. Falls back to allow-all until then.
#[derive(Debug, Default, Clone, Copy)]
pub struct GlobalDefaultPolicy;

#[async_trait::async_trait]
impl EgressPolicy for GlobalDefaultPolicy {
    async fn check(&self, request: &reqwest::Request) -> EgressDecision {
        match GLOBAL_POLICY.get() {
            Some(policy) => policy.check(request).await,
            None => EgressDecision::Allow,
        }
    }
}

/// The default policy handle for a freshly-built client: the global proxy.
pub fn default_policy() -> SharedPolicy {
    Arc::new(GlobalDefaultPolicy)
}
