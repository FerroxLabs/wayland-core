//! Anvil gate → 06C parent-owned gate authorization translation (20-08 piece 2C).
//!
//! Pure, allocation-light translation: given the winner's pinned Anvil gate
//! ([`super::gates::GateClosure`]) plus the subject facts the 2E caller sources
//! from the durable opening and the winner workspace, construct the exact
//! parent-owned inputs that
//! [`crate::child_transaction::ChildTransactionLifecycle::accept_selected_winner`]
//! consumes: an [`ChildGatePlan`], a [`GateExecutionSubject`], and the
//! [`AuthorizedGateClosure`] list.
//!
//! This module runs NO gate and allocates no workspace. It only constructs the
//! authorized closure and digest-aligns the plan/subject so the 06C acceptance
//! machine's fail-closed `resolve` accepts the pinned closure rather than
//! rejecting it as a substitution.
//!
//! ## The digest landmine (why this file exists)
//! The Anvil `GateClosure` carries its own `gate_closure_digest`
//! ([`super::gates::GateClosure::digest_hex`], domain `anvil-gate-closure-v1`).
//! The 06C acceptance machine pins and resolves a DIFFERENT digest: the
//! [`AuthorizedGateClosure`]'s digest (domain
//! `wayland-core:authorized-gate-closure:v1`). Both are 64 lowercase hex chars,
//! so pinning the Anvil digest passes every shape check yet is rejected at
//! `resolve` time as `SubstitutedClosure` — the candidate never lands, silently.
//! This translator pins the 06C digest (via
//! [`AuthorizedGateClosure::closure_digest`], the exact value the acceptance
//! registry recomputes as `live`), so the plan the 2E caller opens the
//! transaction with matches what the gate executor will resolve.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use thiserror::Error;
use wcore_types::child_transaction::{
    ChildGatePlan, ChildGateRequirement, ChildTransactionValidationError,
};

use crate::child_transaction::{AuthorizedGateClosure, GateClosureError, GateExecutionSubject};
use crate::orchestration::anvil::gates::GateClosure;

/// Inputs for [`build_winner_gate_authorization`], decoupled from the spawner and
/// the durable opening: the 2E caller sources each field (the winner's pinned
/// Anvil gate, the caller-chosen gate slug, the host toolchain marker, the gate
/// timeout, the transaction-private writable roots, and the six subject facts)
/// and hands them here as plain borrowed data.
pub struct WinnerGateInputs<'a> {
    /// The winner's pinned Anvil gate (the selected candidate's Tier-1 gate).
    pub anvil_gate: &'a GateClosure,
    /// Caller-chosen stable slug identifying the gate (e.g. `"cargo-test"`).
    pub gate_id: &'a str,
    /// Coarse host-toolchain marker sealed into the 06C closure.
    pub toolchain_identity: &'a str,
    /// Per-gate wall-clock budget in ms (`GATE_TIMEOUT.as_millis() as u64`).
    pub timeout_ms: u64,
    /// The transaction-private writable roots the gate may write to
    /// (`spawner.mutation_writable_roots(guard)` = `[scratch]`).
    pub private_writable_roots: Vec<PathBuf>,
    /// MUST equal the durable opening `base_revision`.
    pub base_revision: &'a str,
    /// Winner workspace head commit.
    pub candidate_revision: &'a str,
    /// Winner diff digest.
    pub diff_digest: &'a str,
    /// MUST equal the durable opening `request_digest`.
    pub request_digest: &'a str,
    /// MUST equal the durable opening `policy_digest`.
    pub policy_digest: &'a str,
}

/// The parent-owned gate authorization the 06C acceptance machine consumes: the
/// orchestrator-owned plan, the execution subject bound to it, and the authorized
/// closure list to seal into the acceptance registry.
#[derive(Debug)]
pub struct WinnerGateAuthorization {
    pub plan: ChildGatePlan,
    pub subject: GateExecutionSubject,
    pub closures: Vec<AuthorizedGateClosure>,
}

/// A fail-closed refusal while translating the Anvil gate into 06C authorization.
/// No variant yields a partial or plausible authorization; each keeps the winner
/// non-accepting.
#[derive(Debug, Error)]
pub enum GateAuthorizationError {
    /// The gate has no command. A pinned Anvil [`GateClosure`] can never carry an
    /// empty argv (`GateClosure::pin` rejects it), so this is a redundant
    /// belt-and-suspenders guard against any future non-pinned construction.
    #[error("winner gate has no argv (a Tier-1 gate must have a command)")]
    EmptyArgv,
    /// The caller-chosen gate id is not a valid identifier (empty, surrounded by
    /// whitespace, or containing control characters).
    #[error("winner gate id is not a valid identifier")]
    InvalidGateId,
    /// The 06C authorized-gate-closure digest could not be computed.
    #[error("06C authorized-gate-closure digest could not be computed: {0}")]
    ClosureDigest(#[source] GateClosureError),
    /// The orchestrator-owned gate plan digest could not be computed.
    #[error("child gate plan digest could not be computed: {0}")]
    GatePlan(#[source] ChildTransactionValidationError),
}

/// Translate the winner's pinned Anvil gate into the 06C parent-owned gate
/// authorization inputs. Pure construction + digest alignment; runs no gate.
///
/// See the module docs for the digest landmine this avoids: the pinned
/// `gate_closure_digest` is the 06C [`AuthorizedGateClosure`] digest, NOT the
/// Anvil [`GateClosure::digest_hex`].
pub fn build_winner_gate_authorization(
    inputs: WinnerGateInputs<'_>,
) -> Result<WinnerGateAuthorization, GateAuthorizationError> {
    // Fail closed on an invalid gate id BEFORE any digest work. `canonical_digest`
    // would also reject it, but an explicit guard yields a precise variant.
    if !is_valid_gate_id(inputs.gate_id) {
        return Err(GateAuthorizationError::InvalidGateId);
    }

    let spec = inputs.anvil_gate.spec();
    let argv = spec.argv.clone();
    // Redundant with `GateClosure::pin` (which forbids empty argv), kept as a
    // fail-closed guard: never authorize a gate with no command.
    if argv.is_empty() {
        return Err(GateAuthorizationError::EmptyArgv);
    }

    // Pinned input identities. NOTE: the Anvil `GateClosure` exposes the input
    // PATHS (`spec.inputs`) but NOT their per-input content digests
    // (`GateClosure::input_digests` is private with no accessor), so we seal the
    // input paths alone as identities. The per-input CONTENT is still covered
    // end-to-end by the Anvil closure digest / drift machinery at climb time;
    // the 06C closure only needs a deterministic, order-independent identity set,
    // and the same set is recomputed on this same closure at resolve time.
    let input_identities: BTreeSet<String> = spec
        .inputs
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();

    let closure = AuthorizedGateClosure::new(
        inputs.gate_id,
        argv,
        inputs.timeout_ms,
        // EMPTY env is REQUIRED for parity, not an oversight: the production forge
        // builds the Anvil `GateSpec` with `env_allowlist: Vec::new()`, and the
        // 06C re-run's `HardContainmentFilesystem::to_manifest` hardcodes an empty
        // env. Sealing any env value here would be hashed into the 06C digest but
        // never injected at execution, so the sealed closure would misrepresent
        // what actually runs.
        BTreeMap::new(),
        inputs.toolchain_identity,
        input_identities,
        inputs.private_writable_roots,
    );

    // Pin the 06C digest (domain `wayland-core:authorized-gate-closure:v1`), i.e.
    // exactly the value the acceptance registry recomputes as `live` and compares
    // against `ChildGateRequirement.gate_closure_digest`. This is the value
    // `AuthorizedGateClosureRegistry::authorize` returns; we compute it directly
    // because that registry is `pub(crate)` inside a private module and is not
    // reachable from here (see the commit message / report). NEVER pin
    // `anvil_gate.digest_hex()` (domain `anvil-gate-closure-v1`) — same 64-hex
    // shape, but rejected as `SubstitutedClosure` at resolve.
    let gate_closure_digest = closure
        .closure_digest()
        .map_err(GateAuthorizationError::ClosureDigest)?;

    let plan = ChildGatePlan {
        required_gates: vec![ChildGateRequirement {
            gate_id: inputs.gate_id.to_owned(),
            gate_closure_digest,
        }],
    };
    let gate_plan_digest = plan
        .canonical_digest()
        .map_err(GateAuthorizationError::GatePlan)?;

    let subject = GateExecutionSubject {
        base_revision: inputs.base_revision.to_owned(),
        candidate_revision: inputs.candidate_revision.to_owned(),
        diff_digest: inputs.diff_digest.to_owned(),
        request_digest: inputs.request_digest.to_owned(),
        policy_digest: inputs.policy_digest.to_owned(),
        gate_plan_digest,
    };

    Ok(WinnerGateAuthorization {
        plan,
        subject,
        closures: vec![closure],
    })
}

/// Whether `gate_id` is a valid child-transaction identifier: non-empty, not
/// whitespace-padded, and free of control characters. Mirrors the essential
/// checks `ChildGatePlan::validate` enforces so we fail closed with a precise
/// variant before any digest work.
fn is_valid_gate_id(gate_id: &str) -> bool {
    !gate_id.is_empty() && gate_id.trim() == gate_id && !gate_id.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::anvil::gates::GateSpec;

    /// A pinned Anvil gate with a real command, an empty env allowlist, and no
    /// transitive inputs — so `pin` performs no filesystem reads and the cwd need
    /// not exist. This keeps the whole test pure (construction + digest math), so
    /// it runs on every platform.
    fn pinned_anvil_gate() -> GateClosure {
        let spec = GateSpec {
            argv: vec!["true".to_string()],
            cwd: PathBuf::from("/anvil/baseline"),
            env_allowlist: Vec::new(),
            inputs: Vec::new(),
        };
        GateClosure::pin(spec, &[]).expect("pin gate with a command and no inputs")
    }

    fn sample_inputs<'a>(
        gate: &'a GateClosure,
        gate_id: &'a str,
        base: &'a str,
        candidate: &'a str,
        diff: &'a str,
        request: &'a str,
        policy: &'a str,
    ) -> WinnerGateInputs<'a> {
        WinnerGateInputs {
            anvil_gate: gate,
            gate_id,
            toolchain_identity: "rustc:1.999.0",
            timeout_ms: 120_000,
            private_writable_roots: vec![PathBuf::from("/srv/wayland/private/scratch")],
            base_revision: base,
            candidate_revision: candidate,
            diff_digest: diff,
            request_digest: request,
            policy_digest: policy,
        }
    }

    /// The core landmine proof: the plan pins the 06C authorized-gate-closure
    /// digest (the value `resolve` recomputes as `live`), NOT the Anvil closure
    /// digest — so the acceptance machine accepts it instead of rejecting it as a
    /// substitution. Direct digest equality/inequality proves the exact property
    /// `AuthorizedGateClosureRegistry::resolve` checks (`live == requirement
    /// .gate_closure_digest` => Ok; otherwise `SubstitutedClosure`). The registry
    /// itself is `pub(crate)` inside a private module and unreachable here, so we
    /// assert the recomputed digests directly.
    #[test]
    fn pins_06c_digest_not_anvil_digest() {
        let gate = pinned_anvil_gate();
        let base = "1".repeat(40);
        let candidate = "2".repeat(40);
        let diff = "d".repeat(64);
        let request = "a".repeat(64);
        let policy = "b".repeat(64);
        let auth = build_winner_gate_authorization(sample_inputs(
            &gate,
            "cargo-test",
            &base,
            &candidate,
            &diff,
            &request,
            &policy,
        ))
        .expect("translation succeeds");

        // Exactly one gate, keyed by the caller slug.
        assert_eq!(auth.plan.required_gates.len(), 1);
        assert_eq!(auth.closures.len(), 1);
        let requirement = &auth.plan.required_gates[0];
        assert_eq!(requirement.gate_id, "cargo-test");

        // The pinned digest is the 06C authorized-closure digest (what `resolve`
        // recomputes as `live` on this same closure).
        let live_06c = auth.closures[0]
            .closure_digest()
            .expect("06C closure digest");
        assert_eq!(
            requirement.gate_closure_digest, live_06c,
            "plan must pin the 06C digest so resolve() returns Ok, not SubstitutedClosure"
        );

        // ...and it is NOT the Anvil closure digest, even though both are 64-hex.
        let anvil_digest = gate.digest_hex();
        assert_eq!(anvil_digest.len(), 64);
        assert_eq!(live_06c.len(), 64);
        assert_ne!(
            requirement.gate_closure_digest, anvil_digest,
            "pinning the Anvil digest would be rejected as SubstitutedClosure at resolve"
        );

        // The subject binds the orchestrator-owned plan digest.
        assert_eq!(
            auth.subject.gate_plan_digest,
            auth.plan.canonical_digest().expect("plan canonical digest"),
            "subject.gate_plan_digest must equal plan.canonical_digest()"
        );

        // Subject facts round-trip unchanged (the acceptance machine binds these
        // to the durable opening; any mutation here would fail SubjectPlanMismatch
        // or the receipt subject bind).
        assert_eq!(auth.subject.base_revision, base);
        assert_eq!(auth.subject.candidate_revision, candidate);
        assert_eq!(auth.subject.diff_digest, diff);
        assert_eq!(auth.subject.request_digest, request);
        assert_eq!(auth.subject.policy_digest, policy);
    }

    /// The 06C digest depends on the toolchain identity, proving the sealed
    /// closure actually binds it (a different toolchain is a different closure).
    #[test]
    fn toolchain_identity_changes_the_06c_digest() {
        let gate = pinned_anvil_gate();
        let base = "1".repeat(40);
        let candidate = "2".repeat(40);
        let diff = "d".repeat(64);
        let request = "a".repeat(64);
        let policy = "b".repeat(64);

        let auth_a = build_winner_gate_authorization(sample_inputs(
            &gate,
            "cargo-test",
            &base,
            &candidate,
            &diff,
            &request,
            &policy,
        ))
        .expect("translation a");

        let mut inputs_b = sample_inputs(
            &gate,
            "cargo-test",
            &base,
            &candidate,
            &diff,
            &request,
            &policy,
        );
        inputs_b.toolchain_identity = "rustc:2.000.0";
        let auth_b = build_winner_gate_authorization(inputs_b).expect("translation b");

        assert_ne!(
            auth_a.plan.required_gates[0].gate_closure_digest,
            auth_b.plan.required_gates[0].gate_closure_digest,
            "toolchain identity must affect the 06C closure digest"
        );
    }

    /// Fail closed on an invalid gate id (empty, or containing a control char).
    /// `EmptyArgv` cannot be exercised through a real `GateClosure` because
    /// `GateClosure::pin` already forbids an empty argv, so the reachable
    /// fail-closed surface is the gate-id guard.
    #[test]
    fn rejects_invalid_gate_id() {
        let gate = pinned_anvil_gate();
        let base = "1".repeat(40);
        let candidate = "2".repeat(40);
        let diff = "d".repeat(64);
        let request = "a".repeat(64);
        let policy = "b".repeat(64);

        for bad_id in ["", " cargo-test", "cargo\ttest", "cargo\ntest"] {
            let error = build_winner_gate_authorization(sample_inputs(
                &gate, bad_id, &base, &candidate, &diff, &request, &policy,
            ))
            .expect_err("invalid gate id must fail closed");
            assert!(
                matches!(error, GateAuthorizationError::InvalidGateId),
                "expected InvalidGateId for {bad_id:?}, got {error:?}"
            );
        }
    }
}
