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

use std::future::Future;
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

tokio::task_local! {
    /// Policy selected for the runtime/session currently constructing clients.
    ///
    /// `EgressClientBuilder::build` snapshots this handle into the client, so
    /// clients keep their originating session's authority after bootstrap and
    /// cannot be repointed by a later session in the same process.
    static SESSION_DEFAULT_POLICY: SharedPolicy;
}

thread_local! {
    /// Synchronous provider constructors cannot await a Tokio task-local
    /// scope. This stack supplies the same immutable snapshot for the duration
    /// of a non-yielding constructor call and restores correctly when nested.
    static SYNC_DEFAULT_POLICIES: std::cell::RefCell<Vec<SharedPolicy>> = const {
        std::cell::RefCell::new(Vec::new())
    };
}

/// Run `future` with a session-owned default egress policy.
///
/// Every `EgressClient` constructed inside the scope captures `policy` as its
/// immutable default. Explicit per-client policies still take precedence.
pub async fn with_default_policy<F>(policy: SharedPolicy, future: F) -> F::Output
where
    F: Future,
{
    SESSION_DEFAULT_POLICY.scope(policy, future).await
}

/// Run a synchronous, non-yielding client constructor with session authority.
pub fn with_default_policy_sync<R>(policy: SharedPolicy, build: impl FnOnce() -> R) -> R {
    struct ScopeGuard;
    impl Drop for ScopeGuard {
        fn drop(&mut self) {
            SYNC_DEFAULT_POLICIES.with(|policies| {
                policies.borrow_mut().pop();
            });
        }
    }

    SYNC_DEFAULT_POLICIES.with(|policies| policies.borrow_mut().push(policy));
    let _guard = ScopeGuard;
    build()
}

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

/// Compatibility policy for clients built outside a session scope. It consults
/// the one-shot process policy at send time and falls back to allow-all.
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

/// The default policy handle for a freshly-built client: the current session's
/// immutable policy when scoped, otherwise the compatibility global proxy.
pub fn default_policy() -> SharedPolicy {
    SESSION_DEFAULT_POLICY
        .try_with(Clone::clone)
        .ok()
        .or_else(|| SYNC_DEFAULT_POLICIES.with(|policies| policies.borrow().last().cloned()))
        .unwrap_or_else(|| Arc::new(GlobalDefaultPolicy))
}

#[cfg(test)]
mod session_scope_tests {
    use super::*;

    #[derive(Debug)]
    struct NamedDeny(&'static str);

    #[async_trait::async_trait]
    impl EgressPolicy for NamedDeny {
        async fn check(&self, _request: &reqwest::Request) -> EgressDecision {
            EgressDecision::Deny {
                reason: self.0.to_string(),
            }
        }
    }

    fn request() -> reqwest::Request {
        reqwest::Request::new(
            reqwest::Method::GET,
            "https://example.test/".parse().unwrap(),
        )
    }

    #[tokio::test]
    async fn parallel_scopes_capture_distinct_session_policies() {
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let build = |name: &'static str, barrier: Arc<tokio::sync::Barrier>| async move {
            with_default_policy(Arc::new(NamedDeny(name)), async move {
                barrier.wait().await;
                crate::EgressClient::tool()
            })
            .await
        };

        let (first, second) = tokio::join!(
            build("session-one", barrier.clone()),
            build("session-two", barrier)
        );

        assert!(matches!(
            first.policy().check(&request()).await,
            EgressDecision::Deny { reason } if reason == "session-one"
        ));
        assert!(matches!(
            second.policy().check(&request()).await,
            EgressDecision::Deny { reason } if reason == "session-two"
        ));
    }

    #[tokio::test]
    async fn captured_policy_survives_scope_exit() {
        let client = with_default_policy(Arc::new(NamedDeny("retained")), async {
            crate::EgressClient::tool()
        })
        .await;

        assert!(matches!(
            client.policy().check(&request()).await,
            EgressDecision::Deny { reason } if reason == "retained"
        ));
    }

    #[test]
    fn synchronous_scope_is_exact_nested_and_restored() {
        let outer: SharedPolicy = Arc::new(NamedDeny("outer"));
        let inner: SharedPolicy = Arc::new(NamedDeny("inner"));

        with_default_policy_sync(outer.clone(), || {
            assert!(Arc::ptr_eq(&default_policy(), &outer));
            with_default_policy_sync(inner.clone(), || {
                assert!(Arc::ptr_eq(&default_policy(), &inner));
            });
            assert!(Arc::ptr_eq(&default_policy(), &outer));
        });
        assert!(!Arc::ptr_eq(&default_policy(), &outer));
    }
}
