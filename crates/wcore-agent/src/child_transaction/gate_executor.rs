//! Parent-owned fixed gate execution and authoritative receipt closure.
//!
//! This module owns the [`AuthorizedGateClosureRegistry`]: the parent's sealed
//! record of every execution-affecting field for each authorized acceptance
//! gate — the fixed argv, the timeout, the sanitized environment, the pinned
//! toolchain / input identities, and the transaction-private writable roots.
//! The candidate working directory is deliberately *not* a sealed literal path;
//! it resolves exclusively from the live [`wcore_swarm::worktree::CandidateSeal`]
//! at spawn (see [`GateExecutor`]), so a gate can only ever run against the exact
//! clean isolated checkout the parent currently holds authority over.
//!
//! A gate closure is bound (its digest captured) at authorization. At spawn the
//! executor recomputes the digest from the live closure and cross-checks it
//! against BOTH the authorization ledger (drift) AND the orchestrator-owned
//! [`ChildGatePlan`] requirement (substitution). An unknown, substituted, or
//! drifted closure fails closed BEFORE any candidate code executes.
//!
//! Nothing here mints acceptance or writes the durable receipt: the executor
//! only produces module-private [`ObservedGateResult`]s from consumed live
//! containment spawns. Turning those into a durable receipt and an
//! [`super::gates::AcceptedCandidate`] is the acceptance machine's job.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use thiserror::Error;
use wcore_sandbox::{HardContainmentFilesystem, SandboxCommand, SandboxError, SandboxRegistry};
use wcore_types::child_transaction::{
    ChildGateOutcome, ChildGatePlan, ChildGateReceipt, ChildGateRequirement, ChildGateSubject,
};

use crate::session_journal::{JournalError, state_payload_digest};

/// Domain separator for the authorized-gate-closure digest. Distinct from every
/// other digest domain so a closure digest can never be confused with a receipt
/// or gate-plan digest.
const CLOSURE_DIGEST_DOMAIN: &str = "wayland-core:authorized-gate-closure:v1";

/// One parent-authorized executable gate closure. Every field that can change
/// what the gate actually runs is sealed here; the digest over these fields is
/// what the orchestrator-owned [`ChildGatePlan`] pins.
///
/// The candidate working directory is intentionally absent — it is bound to the
/// live [`wcore_swarm::worktree::CandidateSeal`] at spawn, never to a literal
/// path captured here, so a stale or substituted path cannot become the gate's
/// cwd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedGateClosure {
    gate_id: String,
    argv: Vec<String>,
    timeout_ms: u64,
    /// Sanitized environment: variable names AND values are sealed. An added,
    /// removed, or mutated variable perturbs the digest.
    environment: BTreeMap<String, String>,
    /// Pinned toolchain identity (e.g. the exact compiler/runtime the gate runs
    /// under). A different toolchain is a different closure.
    toolchain_identity: String,
    /// Pinned transitive input identities (lockfile digests, pinned tool
    /// versions, …). Order-independent.
    input_identities: BTreeSet<String>,
    /// Transaction-private writable roots the gate may write to. These are the
    /// ONLY writable mounts; the candidate itself is read-only.
    private_writable_roots: Vec<PathBuf>,
}

impl AuthorizedGateClosure {
    pub fn new(
        gate_id: impl Into<String>,
        argv: Vec<String>,
        timeout_ms: u64,
        environment: BTreeMap<String, String>,
        toolchain_identity: impl Into<String>,
        input_identities: BTreeSet<String>,
        private_writable_roots: Vec<PathBuf>,
    ) -> Self {
        Self {
            gate_id: gate_id.into(),
            argv,
            timeout_ms,
            environment,
            toolchain_identity: toolchain_identity.into(),
            input_identities,
            private_writable_roots,
        }
    }

    pub(crate) fn gate_id(&self) -> &str {
        &self.gate_id
    }

    /// Recompute the canonical, domain-separated digest over every sealed
    /// execution-affecting field. This is recomputed live immediately before
    /// spawn; a mismatch against the authorization ledger is drift.
    pub(crate) fn closure_digest(&self) -> Result<String, GateClosureError> {
        let roots: Vec<String> = self
            .private_writable_roots
            .iter()
            .map(|root| root.to_string_lossy().into_owned())
            .collect();
        let payload = serde_json::json!({
            "domain": CLOSURE_DIGEST_DOMAIN,
            "gate_id": self.gate_id,
            "argv": self.argv,
            "timeout_ms": self.timeout_ms,
            "environment": self.environment,
            "toolchain_identity": self.toolchain_identity,
            "input_identities": self.input_identities.iter().collect::<Vec<_>>(),
            "private_writable_roots": roots,
        });
        state_payload_digest(&payload).map_err(GateClosureError::Digest)
    }

    /// Build the read-only-candidate / private-writable-roots containment policy
    /// for this closure against a live candidate root. The candidate root is the
    /// live seal's checkout; it is never a field of the closure.
    fn filesystem(
        &self,
        candidate_root: &Path,
    ) -> Result<HardContainmentFilesystem, GateStageError> {
        HardContainmentFilesystem::new(
            candidate_root.to_path_buf(),
            self.private_writable_roots.clone(),
        )
        .map_err(GateStageError::Filesystem)
    }

    fn command(&self, candidate_root: &Path) -> SandboxCommand {
        SandboxCommand {
            argv: self.argv.clone(),
            cwd: Some(candidate_root.to_path_buf()),
        }
    }
}

/// Parent-owned registry of authorized gate closures.
///
/// Holds each closure keyed by `gate_id` and, separately, the digest that was
/// authorized for that gate at authorization time. Resolution recomputes the
/// live digest and refuses on unknown / drifted / substituted closures.
#[derive(Debug, Clone, Default)]
pub(crate) struct AuthorizedGateClosureRegistry {
    closures: BTreeMap<String, AuthorizedGateClosure>,
    /// The digest authorized for each gate at authorization time. Kept separate
    /// from the closure so post-authorization drift in the closure's fields is
    /// detectable (live recompute != authorized).
    authorized_digests: BTreeMap<String, String>,
}

impl AuthorizedGateClosureRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Authorize a closure. Captures the closure's digest into the ledger and
    /// returns it so the caller can pin it in a [`ChildGatePlan`].
    pub(crate) fn authorize(
        &mut self,
        closure: AuthorizedGateClosure,
    ) -> Result<String, GateClosureError> {
        let digest = closure.closure_digest()?;
        let gate_id = closure.gate_id().to_owned();
        self.authorized_digests
            .insert(gate_id.clone(), digest.clone());
        self.closures.insert(gate_id, closure);
        Ok(digest)
    }

    /// Resolve the closure the plan requires, failing closed on any deviation.
    ///
    /// * Unknown — no closure (or no authorization ledger entry) for this gate.
    /// * Config drift — the live closure recomputes to a digest other than the
    ///   one authorized for it (its execution-affecting fields changed after
    ///   authorization).
    /// * Substituted — the (undrifted) closure's digest is not the one the
    ///   orchestrator-owned plan pinned for this gate.
    pub(crate) fn resolve(
        &self,
        requirement: &ChildGateRequirement,
    ) -> Result<&AuthorizedGateClosure, GateClosureError> {
        let closure = self
            .closures
            .get(&requirement.gate_id)
            .ok_or_else(|| GateClosureError::UnknownClosure(requirement.gate_id.clone()))?;
        let authorized = self
            .authorized_digests
            .get(&requirement.gate_id)
            .ok_or_else(|| GateClosureError::UnknownClosure(requirement.gate_id.clone()))?;
        let live = closure.closure_digest()?;
        if &live != authorized {
            return Err(GateClosureError::ConfigDrift(requirement.gate_id.clone()));
        }
        if live != requirement.gate_closure_digest {
            return Err(GateClosureError::SubstitutedClosure(
                requirement.gate_id.clone(),
            ));
        }
        Ok(closure)
    }

    /// Test-only: corrupt an authorized closure's argv WITHOUT updating the
    /// authorization ledger, so the next [`Self::resolve`] observes drift. This
    /// exercises the real live-recompute-vs-authorized comparison; it never
    /// touches the ledger digest, so a passing drift test proves the recompute
    /// path, not a bookkeeping artifact.
    #[cfg(test)]
    pub(crate) fn corrupt_closure_argv(&mut self, gate_id: &str, argv: Vec<String>) {
        if let Some(closure) = self.closures.get_mut(gate_id) {
            closure.argv = argv;
        }
    }
}

/// The exact revisions the gate evaluated, resolved from the live seal and the
/// transaction opening. Bound into each observed result so a result can never be
/// attached to a different candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateExecutionSubject {
    pub base_revision: String,
    pub candidate_revision: String,
    pub diff_digest: String,
    pub request_digest: String,
    pub policy_digest: String,
    pub gate_plan_digest: String,
}

/// A module-private observed result of one gate spawn. Only the acceptance
/// machine consumes these; they are never deserialized from a caller and never
/// minted from a serialized receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedGateResult {
    gate_id: String,
    closure_digest: String,
    subject: GateExecutionSubject,
    outcome: ChildGateOutcome,
    exit_code: Option<i32>,
    evidence_digest: String,
}

impl ObservedGateResult {
    pub(crate) fn gate_id(&self) -> &str {
        &self.gate_id
    }

    pub(crate) fn outcome(&self) -> ChildGateOutcome {
        self.outcome
    }

    pub(crate) fn subject(&self) -> &GateExecutionSubject {
        &self.subject
    }

    /// Lower this observed result into the path-free [`ChildGateReceipt`] the
    /// durable receipt carries. Command text, host paths, captured output, and
    /// environment values never cross into the receipt.
    pub(crate) fn to_gate_receipt(&self) -> ChildGateReceipt {
        ChildGateReceipt {
            gate_id: self.gate_id.clone(),
            subject: ChildGateSubject {
                base_revision: self.subject.base_revision.clone(),
                candidate_revision: self.subject.candidate_revision.clone(),
                diff_digest: self.subject.diff_digest.clone(),
                request_digest: self.subject.request_digest.clone(),
                policy_digest: self.subject.policy_digest.clone(),
                gate_plan_digest: self.subject.gate_plan_digest.clone(),
                gate_closure_digest: self.closure_digest.clone(),
            },
            evidence_digest: self.evidence_digest.clone(),
            outcome: self.outcome,
            exit_code: self.exit_code,
        }
    }
}

/// Live source of the candidate working directory for a gate spawn.
///
/// The production implementation ([`super::gates::SealedCandidateRoot`]) proves
/// the owning transaction is still live and clean (by minting a fresh
/// [`wcore_swarm::worktree::CandidateSeal`]) before yielding the checkout root,
/// so the cwd can only ever be that of the exact live sealed candidate. It fails
/// closed if the seal cannot be minted (drift, released transaction, identity
/// change).
// `Send + Sync` so a `&dyn LiveCandidateRoot` can be held across the `.await` in
// `execute_plan` without making the gate-acceptance future `!Send`. Both
// implementors already satisfy it — the production `SealedCandidateRoot` borrows
// a `MutationAttemptGuard` (an `Arc`/plain-data `TransactionWorkspace`, already
// `Send` where the spawner returns it across awaits), and the test fake is plain
// data. This is required for the Anvil landing (20-08) to compile in the
// `Send`-required Tool execution context; it is a bound declaration with no
// behavior change to the 20-14-audited acceptance logic.
pub(crate) trait LiveCandidateRoot: Send + Sync {
    /// Re-prove liveness/cleanliness and return the sealed checkout root.
    fn resolve_root(&self) -> Result<PathBuf, GateStageError>;
}

/// Executes an authorized gate plan against a live sealed candidate under hard
/// containment, producing module-private observed results in declared order.
pub(crate) struct GateExecutor<'a> {
    registry: &'a AuthorizedGateClosureRegistry,
    sandbox: &'a SandboxRegistry,
}

impl<'a> GateExecutor<'a> {
    pub(crate) fn new(
        registry: &'a AuthorizedGateClosureRegistry,
        sandbox: &'a SandboxRegistry,
    ) -> Self {
        Self { registry, sandbox }
    }

    /// Run every gate in the plan, in declared order, against the live sealed
    /// candidate. Any stage failure aborts the whole run fail-closed — a partial
    /// result set can never enter the acceptance machine.
    pub(crate) async fn execute_plan(
        &self,
        plan: &ChildGatePlan,
        subject: &GateExecutionSubject,
        candidate: &dyn LiveCandidateRoot,
    ) -> Result<Vec<ObservedGateResult>, GateStageError> {
        let mut observed = Vec::with_capacity(plan.required_gates.len());
        for requirement in &plan.required_gates {
            observed.push(self.execute_gate(requirement, subject, candidate).await?);
        }
        Ok(observed)
    }

    /// Execute one gate through the fixed fail-closed stage sequence. Each stage
    /// is a discrete checked step returning a specific [`GateStageError`]; no
    /// stage is skipped and none can be short-circuited by candidate output.
    pub(crate) async fn execute_gate(
        &self,
        requirement: &ChildGateRequirement,
        subject: &GateExecutionSubject,
        candidate: &dyn LiveCandidateRoot,
    ) -> Result<ObservedGateResult, GateStageError> {
        // Stage 1 — live seal: resolve the candidate root exclusively from the
        // live seal. A released/drifted transaction fails here before anything
        // else is set up.
        let candidate_root = candidate.resolve_root()?;

        // Stage 2 — closure authority: unknown / drifted / substituted closures
        // fail before any filesystem or containment work.
        let closure = self
            .registry
            .resolve(requirement)
            .map_err(GateStageError::Closure)?;
        let closure_digest = closure.closure_digest().map_err(GateStageError::Closure)?;

        // Stage 3 — containment filesystem: the read-only candidate plus the
        // transaction-private writable roots. A denied or invalid location fails
        // closed here.
        let fs = closure.filesystem(&candidate_root)?;
        let cmd = closure.command(&candidate_root);

        // Stage 4 — spawn-parameter consistency: the command cwd MUST be the
        // sealed candidate root that the filesystem bound as its read-only
        // candidate. This closes the gap between the fs policy and the spawn.
        if cmd.cwd.as_deref() != Some(fs.candidate()) {
            return Err(GateStageError::SpawnParameters(requirement.gate_id.clone()));
        }

        // Stage 5 — containment mint: a one-use authority bound to THIS backend,
        // policy, and exact spawn parameters. A registry that cannot hard-contain
        // fails closed here.
        let authority = self
            .sandbox
            .establish_hard_containment(&fs, &cmd)
            .await
            .map_err(GateStageError::ContainmentMint)?;

        // Stage 6 — containment verify (one-use): re-prove no drift between mint
        // and spawn. Consuming the authority makes it one-use, so the observed
        // result comes only from a spent live containment authority.
        self.sandbox
            .verify_hard_containment(authority, &fs, &cmd)
            .map_err(GateStageError::ContainmentDrift)?;

        // Stage 7 — execute the contained gate and observe its terminal exit.
        let output = self
            .sandbox
            .execute(&fs.to_manifest(), cmd)
            .await
            .map_err(GateStageError::Execution)?;
        let outcome = if output.exit_code == 0 {
            ChildGateOutcome::Passed
        } else {
            ChildGateOutcome::Failed
        };
        let evidence_digest = evidence_digest(
            &requirement.gate_id,
            &closure_digest,
            subject,
            output.exit_code,
        )?;
        Ok(ObservedGateResult {
            gate_id: requirement.gate_id.clone(),
            closure_digest,
            subject: subject.clone(),
            outcome,
            exit_code: Some(output.exit_code),
            evidence_digest,
        })
    }
}

/// Domain-separated evidence digest binding the gate id, the sealed closure, the
/// exact subject, and the observed exit into one 64-hex value.
fn evidence_digest(
    gate_id: &str,
    closure_digest: &str,
    subject: &GateExecutionSubject,
    exit_code: i32,
) -> Result<String, GateStageError> {
    let payload = serde_json::json!({
        "domain": "wayland-core:gate-observed-evidence:v1",
        "gate_id": gate_id,
        "closure_digest": closure_digest,
        "base_revision": subject.base_revision,
        "candidate_revision": subject.candidate_revision,
        "diff_digest": subject.diff_digest,
        "request_digest": subject.request_digest,
        "policy_digest": subject.policy_digest,
        "gate_plan_digest": subject.gate_plan_digest,
        "exit_code": exit_code,
    });
    state_payload_digest(&payload)
        .map_err(|error| GateStageError::Closure(GateClosureError::Digest(error)))
}

/// A closure-authority failure discovered before any candidate code runs.
#[derive(Debug, Error)]
pub enum GateClosureError {
    #[error("gate closure for '{0}' is not authorized")]
    UnknownClosure(String),
    #[error("gate closure for '{0}' drifted from its authorized configuration")]
    ConfigDrift(String),
    #[error("gate closure for '{0}' does not match the authorized gate plan")]
    SubstitutedClosure(String),
    #[error("gate closure digest could not be computed: {0}")]
    Digest(#[source] JournalError),
}

/// A fail-closed failure at one gate-execution stage. Each variant names the
/// exact stage that refused so a later audit can bind a stage to its guard.
#[derive(Debug, Error)]
pub enum GateStageError {
    #[error("live candidate seal refused: {0}")]
    Seal(String),
    #[error("gate closure authority refused: {0}")]
    Closure(#[source] GateClosureError),
    #[error("hard-containment filesystem refused: {0}")]
    Filesystem(#[source] SandboxError),
    #[error("gate spawn parameters do not bind the sealed candidate for '{0}'")]
    SpawnParameters(String),
    #[error("hard-containment authority could not be minted: {0}")]
    ContainmentMint(#[source] SandboxError),
    #[error("hard-containment authority drifted before spawn: {0}")]
    ContainmentDrift(#[source] SandboxError),
    #[error("contained gate execution failed: {0}")]
    Execution(#[source] SandboxError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use wcore_sandbox::FailClosedBackend;

    fn closure(gate_id: &str, argv: &[&str]) -> AuthorizedGateClosure {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_owned(), "/usr/bin".to_owned());
        let mut inputs = BTreeSet::new();
        inputs.insert("lockfile:".to_owned() + &"a".repeat(64));
        AuthorizedGateClosure::new(
            gate_id,
            argv.iter().map(|a| (*a).to_owned()).collect(),
            120_000,
            env,
            "rustc:1.999.0",
            inputs,
            vec![PathBuf::from("/srv/wayland/private/scratch")],
        )
    }

    fn requirement(gate_id: &str, closure_digest: &str) -> ChildGateRequirement {
        ChildGateRequirement {
            gate_id: gate_id.to_owned(),
            gate_closure_digest: closure_digest.to_owned(),
        }
    }

    fn subject() -> GateExecutionSubject {
        GateExecutionSubject {
            base_revision: "1".repeat(40),
            candidate_revision: "2".repeat(40),
            diff_digest: "d".repeat(64),
            request_digest: "a".repeat(64),
            policy_digest: "b".repeat(64),
            gate_plan_digest: "e".repeat(64),
        }
    }

    /// A fake live-candidate source. Production uses the real seal; the fail
    /// path here proves the executor refuses when the seal cannot be re-proven.
    struct FakeCandidate {
        root: Result<PathBuf, String>,
    }
    impl LiveCandidateRoot for FakeCandidate {
        fn resolve_root(&self) -> Result<PathBuf, GateStageError> {
            self.root.clone().map_err(GateStageError::Seal)
        }
    }

    #[test]
    fn rejects_unknown_closure_digest() {
        let registry = AuthorizedGateClosureRegistry::new();
        // Nothing authorized: any requirement references an unknown closure and
        // must be refused before any filesystem or containment work.
        let error = registry
            .resolve(&requirement("cargo-test", &"c".repeat(64)))
            .expect_err("unknown closure must be refused");
        assert!(
            matches!(error, GateClosureError::UnknownClosure(ref id) if id == "cargo-test"),
            "expected UnknownClosure, got {error:?}"
        );
    }

    #[test]
    fn rejects_substituted_closure() {
        let mut registry = AuthorizedGateClosureRegistry::new();
        // Authorize the real closure and pin its true digest into the ledger.
        let authorized_digest = registry
            .authorize(closure("cargo-test", &["cargo", "test"]))
            .expect("authorize");
        // Build the digest of a DIFFERENT closure for the same gate id and pin
        // THAT into the plan requirement. The authorized closure is undrifted,
        // but the plan asks for a substituted one — refuse.
        let substitute_digest = closure("cargo-test", &["cargo", "test", "--all-features"])
            .closure_digest()
            .expect("digest");
        assert_ne!(authorized_digest, substitute_digest);
        let error = registry
            .resolve(&requirement("cargo-test", &substitute_digest))
            .expect_err("substituted closure must be refused");
        assert!(
            matches!(error, GateClosureError::SubstitutedClosure(ref id) if id == "cargo-test"),
            "expected SubstitutedClosure, got {error:?}"
        );
        // Sanity: the true digest resolves cleanly, so the refusal above is
        // about substitution, not a broken registry.
        registry
            .resolve(&requirement("cargo-test", &authorized_digest))
            .expect("authorized closure resolves");
    }

    #[test]
    fn rejects_closure_config_drift() {
        let mut registry = AuthorizedGateClosureRegistry::new();
        let authorized_digest = registry
            .authorize(closure("cargo-test", &["cargo", "test"]))
            .expect("authorize");
        // The plan legitimately pins the authorized digest.
        let req = requirement("cargo-test", &authorized_digest);
        // Before drift, resolution succeeds.
        registry.resolve(&req).expect("undrifted closure resolves");
        // Now the closure's argv changes after authorization WITHOUT updating
        // the ledger — exactly the drift the live recompute must catch.
        registry.corrupt_closure_argv("cargo-test", vec!["cargo".into(), "build".into()]);
        let error = registry
            .resolve(&req)
            .expect_err("drifted closure must be refused");
        assert!(
            matches!(error, GateClosureError::ConfigDrift(ref id) if id == "cargo-test"),
            "expected ConfigDrift, got {error:?}"
        );
    }

    #[tokio::test]
    async fn fails_closed_at_each_gate_execution_stage() {
        let mut registry = AuthorizedGateClosureRegistry::new();
        let authorized_digest = registry
            .authorize(closure("cargo-test", &["cargo", "test"]))
            .expect("authorize");
        // A registry whose backend cannot hard-contain: establish fails closed.
        let sandbox = SandboxRegistry::new(Arc::new(FailClosedBackend::new()));
        let executor = GateExecutor::new(&registry, &sandbox);
        let subject = subject();

        // Stage 1 — live seal cannot be re-proven: refuse before any closure or
        // containment work.
        let dead_seal = FakeCandidate {
            root: Err("owning transaction was released".to_owned()),
        };
        let error = executor
            .execute_gate(
                &requirement("cargo-test", &authorized_digest),
                &subject,
                &dead_seal,
            )
            .await
            .expect_err("dead seal must fail closed");
        assert!(
            matches!(error, GateStageError::Seal(_)),
            "stage 1: {error:?}"
        );

        // A live candidate rooted at a denied location (global temp) so stage 3
        // (filesystem construction) fails closed for the fs-stage assertion.
        let denied = FakeCandidate {
            root: Ok(std::env::temp_dir().join("wayland-gate-denied-candidate")),
        };
        // Stage 2 — unknown closure: refuse even though the seal is live.
        let error = executor
            .execute_gate(&requirement("nope", &authorized_digest), &subject, &denied)
            .await
            .expect_err("unknown closure must fail closed");
        assert!(
            matches!(
                error,
                GateStageError::Closure(GateClosureError::UnknownClosure(_))
            ),
            "stage 2: {error:?}"
        );

        // Stage 3 — filesystem construction refuses a denied candidate location.
        let error = executor
            .execute_gate(
                &requirement("cargo-test", &authorized_digest),
                &subject,
                &denied,
            )
            .await
            .expect_err("denied candidate must fail closed");
        assert!(
            matches!(error, GateStageError::Filesystem(_)),
            "stage 3: {error:?}"
        );

        // Stage 5 — containment mint refuses on a non-containing backend, using a
        // real absolute candidate root outside any denied location so stages 3/4
        // pass and the mint stage is the one that refuses.
        let live = FakeCandidate {
            root: Ok(PathBuf::from("/srv/wayland/candidate/checkout")),
        };
        let error = executor
            .execute_gate(
                &requirement("cargo-test", &authorized_digest),
                &subject,
                &live,
            )
            .await
            .expect_err("non-containing backend must fail closed");
        assert!(
            matches!(error, GateStageError::ContainmentMint(_)),
            "stage 5: {error:?}"
        );
    }
}
