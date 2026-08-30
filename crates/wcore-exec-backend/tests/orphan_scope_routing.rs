//! core#366 d2 ROUTING: a scan must reach the enumeration its scope NAMES, and
//! `orphan::scan_all` must carry the scope it was handed all the way down.
//!
//! # The seam this closes, and how it was found
//!
//! The fix for this ticket made the scope a required parameter, so which
//! callers ask which question became whatever `cargo check` accepts rather
//! than whatever a ledger note claimed. That moved the caller set out of a
//! hand-written list — and left exactly one hand-written decision behind: the
//! two-arm dispatcher in `ExecutionBackend::scan_orphans_in_scope` that turns
//! a scope into a call.
//!
//! A reviewer mutated its unscoped arm to
//! `self.scan_orphans("<a nonce nobody holds>")`. It COMPILED
//! (`cargo check -p wcore-exec-backend -p wcore-cli --all-targets` → 0), the
//! three-crate suite stayed green at 4068 passed, and the shipped product went
//! straight back to `count 0 (MEASURED)` over a real labelled leftover. The
//! defect was fully restored and nothing in the tree said so, because every
//! test that covers the unscoped scan calls
//! `ContainerBackend::scan_orphans_any_nonce()` DIRECTLY and never routes —
//! and the anti-rot guard beside this file checks the SPELLING nobody writes
//! a bare `scan_orphans` outside the crate, which is a different property from
//! the MEANING a caller that says `AnyNonce` reaches the unscoped enumeration.
//!
//! # Why this is not one more test for one more arm
//!
//! "Is this arm right" is a question asked per variant, and the variant added
//! next month answers it by never being asked. So the property here does not
//! name an arm at all:
//!
//! **the nonce the backend is handed must equal the nonce the scope names.**
//!
//! `OrphanScope::nonce()` is the single definition of what a scope means, the
//! spy records the question it was actually asked, and the two are compared.
//! That statement is total over the enum — it needs no case per variant and no
//! maintenance when one is added — and it is what the mutation violates:
//! `AnyNonce` names no nonce, so handing the backend one is a failure whatever
//! the invented string is.
//!
//! # What is NOT derived here, said plainly
//!
//! Stable Rust cannot enumerate an enum's variants, so `every_scope()` below
//! is a written list. The compile-time tripwire beside it is wildcard-free, so
//! a third `OrphanScope` variant is a BUILD ERROR in this file rather than a
//! case that silently goes untested — that is the strongest available
//! guarantee, and it is a build error rather than a green run.

use std::sync::Mutex;

use async_trait::async_trait;
use wcore_exec_backend::conformance::reference_budget;
use wcore_exec_backend::contract::{
    Availability, BackendCapabilities, BackendKind, CleanupObservation, ExecutionBackend,
    ExecutionTask, Health, OrphanScan, OrphanScope, SecretChannel,
};
use wcore_exec_backend::error::{ExecError, Result};
use wcore_exec_backend::policy::EffectivePolicy;
use wcore_exec_backend::receipt::ExecutionReceipt;

const PROBE_NONCE: &str = "d2-routing-probe-nonce";
const SPY_ID: &str = "d2-routing-spy";

/// The question the dispatcher actually asked the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Asked {
    /// `scan_orphans(nonce)` — restricted to one run.
    Scoped(String),
    /// `scan_orphans_any_nonce()` — restricted to nothing.
    Unscoped,
}

/// A backend that answers nothing and records everything.
///
/// Deliberately not one of the four reference backends: this test is about the
/// dispatcher, and a real backend's answer depends on a docker daemon, an ssh
/// target or a vendor credential — none of which decide whether the routing is
/// right, and all of which would make the test skip on the hosts where it
/// matters.
struct SpyBackend {
    capabilities: BackendCapabilities,
    asked: Mutex<Vec<Asked>>,
    /// When set, the unscoped answer LIES about its own scope: it claims to be
    /// restricted to this nonce. Used as the positive control for the
    /// dispatcher's self-check.
    unscoped_claims_nonce: Option<String>,
}

impl SpyBackend {
    fn new() -> Self {
        Self {
            capabilities: BackendCapabilities {
                backend_id: SPY_ID.into(),
                kind: BackendKind::Local,
                version: "0".into(),
                limits: reference_budget(),
                supports_artifact_transfer: false,
                supports_cancellation: false,
                supports_hibernation: false,
                secret_channel: SecretChannel::None,
            },
            asked: Mutex::new(Vec::new()),
            unscoped_claims_nonce: None,
        }
    }

    fn lying_about_its_scope(nonce: &str) -> Self {
        Self {
            unscoped_claims_nonce: Some(nonce.to_owned()),
            ..Self::new()
        }
    }

    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().expect("spy lock").clone()
    }

    /// The nonce the backend was actually handed, `None` when it was asked the
    /// unscoped question. Panics unless EXACTLY one enumeration ran, because a
    /// dispatcher that asks both questions, or neither, is as wrong as one that
    /// asks the other one.
    fn asked_nonce(&self) -> Option<String> {
        let asked = self.asked();
        assert_eq!(
            asked.len(),
            1,
            "the dispatcher must run EXACTLY one enumeration per scan; it ran {asked:?}"
        );
        match &asked[0] {
            Asked::Scoped(nonce) => Some(nonce.clone()),
            Asked::Unscoped => None,
        }
    }
}

#[async_trait]
impl ExecutionBackend for SpyBackend {
    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn availability(&self) -> Availability {
        unimplemented!("the routing test never probes availability")
    }

    fn effective_policy(&self, _task: &ExecutionTask) -> Result<EffectivePolicy> {
        unimplemented!("the routing test never asks for a policy")
    }

    async fn execute(&self, _task: &ExecutionTask) -> Result<ExecutionReceipt> {
        unimplemented!("the routing test never executes")
    }

    async fn cancel(&self, _task_id: &str) -> Result<CleanupObservation> {
        unimplemented!("the routing test never cancels")
    }

    async fn health(&self) -> Result<Health> {
        unimplemented!("the routing test never asks for health")
    }

    async fn scan_orphans(&self, nonce: &str) -> Result<OrphanScan> {
        self.asked
            .lock()
            .expect("spy lock")
            .push(Asked::Scoped(nonce.to_owned()));
        Ok(OrphanScan {
            backend_id: SPY_ID.into(),
            kind: BackendKind::Local,
            nonce: Some(nonce.to_owned()),
            method: "spy: scoped".into(),
            found: Vec::new(),
            enumerated: true,
        })
    }

    async fn scan_orphans_any_nonce(&self) -> Result<OrphanScan> {
        self.asked.lock().expect("spy lock").push(Asked::Unscoped);
        Ok(OrphanScan {
            backend_id: SPY_ID.into(),
            kind: BackendKind::Local,
            nonce: self.unscoped_claims_nonce.clone(),
            method: "spy: unscoped".into(),
            found: Vec::new(),
            enumerated: true,
        })
    }
}

/// Every `OrphanScope` there is.
///
/// The match below is the tripwire, not the derivation: it has no wildcard, so
/// a third variant fails to COMPILE here and whoever adds it is standing in
/// this file with the list in front of them. Stable Rust offers nothing
/// stronger without a derive macro, and a build error beats a silent pass.
fn every_scope() -> Vec<OrphanScope<'static>> {
    let all = vec![OrphanScope::Nonce(PROBE_NONCE), OrphanScope::AnyNonce];
    for scope in &all {
        match scope {
            OrphanScope::Nonce(_) => {}
            OrphanScope::AnyNonce => {}
        }
    }
    all
}

/// The dispatcher hands the backend exactly the question the scope names.
#[tokio::test(flavor = "multi_thread")]
async fn every_scope_reaches_the_enumeration_it_names() {
    let scopes = every_scope();

    // CONTROL: an empty scope list would make the loop below vacuously true,
    // which is the failure mode this whole lane keeps meeting. Both known
    // variants must be present, and one of them must name no nonce at all —
    // if `AnyNonce` ever stopped being reachable, the unscoped route would go
    // untested exactly as it did before this file existed.
    assert!(
        scopes.len() >= 2,
        "CONTROL: only {} scope(s) to drive; the loop would prove nothing",
        scopes.len()
    );
    assert!(
        scopes.iter().any(|s| s.nonce().is_none()),
        "CONTROL: no scope in the set names the UNSCOPED question, so the arm that carried the \
         defect would not be exercised at all"
    );
    assert!(
        scopes.iter().any(|s| s.nonce().is_some()),
        "CONTROL: no scope in the set names a run, so a dispatcher that answered everything \
         unscoped would pass"
    );

    for scope in scopes {
        let spy = SpyBackend::new();
        let scan = spy
            .scan_orphans_in_scope(scope)
            .await
            .unwrap_or_else(|e| panic!("the dispatcher must answer for scope {scope:?}: {e}"));

        assert_eq!(
            spy.asked_nonce().as_deref(),
            scope.nonce(),
            "core#366 d2: asked in scope {scope:?}, the dispatcher handed the backend \
             {:?}. A scope that names no run must never be turned into a nonce-scoped \
             enumeration — that answer can only ever be `no orphans under the nonce I \
             invented`, which is the MEASURED zero this ticket records.",
            spy.asked_nonce()
        );
        assert_eq!(
            scan.nonce.as_deref(),
            scope.nonce(),
            "the scan that comes back must declare the scope it was asked in, or a reader \
             cannot tell `every run` from `one run`: {scan:?}"
        );
    }
}

/// POSITIVE CONTROL for the dispatcher's self-check: a backend whose answer
/// disagrees with the scope it was asked is REFUSED, not returned.
///
/// Without this the check above is only as good as the spy's honesty, and the
/// production guard it exercises would itself be untested code — the exact
/// complaint that refuted this lane.
#[tokio::test(flavor = "multi_thread")]
async fn an_answer_that_disagrees_with_the_scope_it_was_asked_is_refused() {
    let spy = SpyBackend::lying_about_its_scope("a-nonce-the-caller-never-named");
    let err = spy
        .scan_orphans_in_scope(OrphanScope::AnyNonce)
        .await
        .expect_err(
            "an unscoped scan that comes back claiming a nonce scope must be refused: it is \
             indistinguishable, downstream, from a clean host",
        );
    let text = err.to_string();
    assert!(
        text.contains("core#366") && text.contains("a-nonce-the-caller-never-named"),
        "the refusal must name the ticket and the scope it actually got, so an operator can \
         tell a routing bug from a clean host: {text}"
    );
    assert!(
        matches!(err, ExecError::Exec(_)),
        "the refusal is an execution error, not a malformed-task error: {err:?}"
    );
}

/// `orphan::scan_all` must CARRY the scope it was given to every backend.
///
/// The dispatcher check above cannot see this: an aggregate that quietly
/// substituted `OrphanScope::Nonce(..)` for the caller's `AnyNonce` would ask
/// a self-consistent question and every arm would agree with itself. What
/// gives it away is the evidence: `OrphanEvidence::nonce` is the scope each
/// row was taken in, so the whole set must agree with the scope the caller
/// stated.
///
/// Runs on ANY host. It never needs a docker daemon, an ssh target or a cloud
/// credential — an unreachable backend answers `enumerated: false`, and this
/// asserts on the declared scope rather than on any count.
#[tokio::test(flavor = "multi_thread")]
async fn scan_all_carries_the_callers_scope_to_every_backend() {
    for scope in every_scope() {
        let evidence = wcore_exec_backend::orphan::scan_all(scope, reference_budget())
            .await
            .unwrap_or_else(|e| panic!("scan_all must answer for scope {scope:?}: {e}"));

        // CONTROL: an empty evidence set would make the assertion below
        // vacuous. This build carries four reference backends.
        assert!(
            evidence.len() >= 4,
            "CONTROL: scan_all returned {} row(s) for scope {scope:?}; a scan of no backends \
             proves nothing about which question they were asked",
            evidence.len()
        );

        for e in &evidence {
            assert_eq!(
                e.nonce.as_deref(),
                scope.nonce(),
                "core#366 d2: `scan_all` was asked in scope {scope:?} but backend `{}` \
                 answered in scope {:?}. The aggregate must pass the caller's scope through \
                 unchanged — substituting a nonce here is how `backend scan` reported a \
                 MEASURED zero over a labelled leftover.",
                e.backend_id,
                e.nonce
            );
        }
    }
}
