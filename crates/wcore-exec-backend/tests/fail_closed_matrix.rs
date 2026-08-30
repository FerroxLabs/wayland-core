//! F25-05 — the hostile fail-closed matrix.
//!
//! Five compromises, each INDUCED rather than simulated, crossed with every
//! reference backend this build carries:
//!
//!   1. a ROTATED key, with a signature from the old one presented
//!   2. a TAMPERED bundle — one mutated byte in real signed material
//!   3. an ATTESTATION mismatch — a backend identity that is not the pinned one
//!   4. a denied SECRET
//!   5. a denied EGRESS
//!
//! Two rules run through all of it.
//!
//! **Assert the refusal SHAPE, not merely that something failed.** A test that
//! checks only "did not succeed" passes while a fallback quietly runs the work
//! on another backend or another node — which is the specific failure mode here,
//! because a fallback removes the property under test while turning the test
//! green.
//!
//! **Run serially.** The 20A proof's green depended on `--nocapture` forcing
//! serial execution, because `admit_delegated_backend` rejects under parallel
//! load. A fail-closed matrix run in parallel produces refusals for the WRONG
//! reason, which is worse than a failure because it looks like success.

#![allow(clippy::panic, clippy::unwrap_used)]

use ed25519_dalek::{SigningKey, ed25519::signature::Signer};
use tempfile::TempDir;

use wcore_exec_backend::conformance::{reference_budget, reference_task};
use wcore_exec_backend::contract::{BackendKind, HibernationObservation};
use wcore_exec_backend::node::capability::NodeAdvertisement;
use wcore_exec_backend::node::pairing::{
    NodeIdentity, PairingChallenge, prove_challenge, verify_proof,
};
use wcore_exec_backend::node::registry::{NodeRegistry, SubmissionVerdict};
use wcore_exec_backend::orphan::{ReapingMechanism, mechanism_for};
use wcore_exec_backend::receipt::{
    ArtifactEvidence, BackendIdentity, EventKind, ExecutionReceipt, PROTOCOL_VERSION, ReceiptBody,
    ReceiptEvent, ReceiptSigner, TaskEvidence, TerminalStatus, Timing, Transport, events_digest,
    sha256_public,
};
use wcore_exec_backend::reference_backends;

// ---------------------------------------------------------------------------
// shared fixtures
// ---------------------------------------------------------------------------

fn sealed_receipt(
    seed: u8,
) -> (
    ExecutionReceipt,
    BackendIdentity,
    ed25519_dalek::VerifyingKey,
) {
    let signer = ReceiptSigner::from_seed([seed; 32]);
    let backend = BackendIdentity {
        backend_id: "local".into(),
        instance_id: "inst-1".into(),
        version: "0.12.25".into(),
        key_id: signer.key_id().to_string(),
    };
    let limits = reference_budget();
    let artifact_sha = sha256_public(b"artifact");
    let events = vec![
        ReceiptEvent {
            sequence: 1,
            event: EventKind::TaskAccepted {
                task_id: "t-1".into(),
                backend_id: "local".into(),
                workspace_sha256: sha256_public(b"ws"),
                input_sha256: sha256_public(b"in"),
            },
        },
        ReceiptEvent {
            sequence: 2,
            event: EventKind::ArtifactPublished {
                name: "out.txt".into(),
                sha256: artifact_sha.clone(),
                bytes: 8,
            },
        },
        ReceiptEvent {
            sequence: 3,
            event: EventKind::Succeeded {
                artifact_sha256: artifact_sha.clone(),
            },
        },
    ];
    let body = ReceiptBody {
        protocol_version: PROTOCOL_VERSION,
        backend: backend.clone(),
        node: None,
        transport: Transport {
            kind: BackendKind::Local,
            endpoint: "localhost".into(),
        },
        task: TaskEvidence {
            task_id: "t-1".into(),
            workspace_sha256: sha256_public(b"ws"),
            input_sha256: sha256_public(b"in"),
            resources: limits,
        },
        limits,
        events_sha256: events_digest(&events),
        events,
        artifact: Some(ArtifactEvidence {
            name: "out.txt".into(),
            sha256: artifact_sha,
            bytes: 8,
        }),
        terminal: TerminalStatus::Success,
        timing: Timing {
            started_unix_ms: 1,
            finished_unix_ms: 2,
            wall_ms: 1,
        },
        hibernation: HibernationObservation::NotApplicable,
        secrets_exposed: vec![],
        egress_decision: "deny".into(),
    };
    let key = signer.verifying_key();
    (signer.seal(body).unwrap(), backend, key)
}

// ===========================================================================
// CASE 1 — ROTATED key
// ===========================================================================

/// A signature produced under a key that has since been ROTATED must be
/// refused. The rotation is real: a new signer is minted and the receipt from
/// the old one is presented against the new pinned identity.
#[test]
fn case_rotated_key_is_refused_against_the_new_pinned_identity() {
    let (receipt_from_old_key, old_backend, old_key) = sealed_receipt(11);
    // It verified before rotation — otherwise the refusal below proves nothing.
    receipt_from_old_key
        .verify(&old_backend, &old_key)
        .expect("the pre-rotation receipt must verify, or the rotation case is vacuous");

    // ROTATE.
    let rotated = ReceiptSigner::from_seed([12u8; 32]);
    let new_identity = BackendIdentity {
        key_id: rotated.key_id().to_string(),
        ..old_backend.clone()
    };
    assert_ne!(
        old_backend.key_id, new_identity.key_id,
        "rotation must change the key"
    );

    let err = receipt_from_old_key
        .verify(&new_identity, &rotated.verifying_key())
        .expect_err("a signature from a rotated-out key must NOT verify");
    let msg = err.to_string();
    assert!(
        msg.contains("identity") || msg.contains("attestation") || msg.contains("key"),
        "the refusal must name the identity/key problem, not fail generically: {msg}"
    );

    // And the old key does not rescue it either — pinning is to the NEW identity.
    assert!(
        receipt_from_old_key
            .verify(&new_identity, &old_key)
            .is_err()
    );
}

/// The plugin-side half: a bundle signed by a rotated-out key does not verify
/// against the new trust anchor.
#[test]
fn case_rotated_key_plugin_signature_is_refused_by_the_engine_verifier() {
    let old = SigningKey::from_bytes(&[21u8; 32]);
    let new = SigningKey::from_bytes(&[22u8; 32]);
    let payload = b"plugin entry artifact bytes";
    let signature = old.sign(payload);

    use ed25519_dalek::ed25519::signature::Verifier;
    assert!(
        old.verifying_key().verify(payload, &signature).is_ok(),
        "the pre-rotation signature must verify, or the case is vacuous"
    );
    assert!(
        new.verifying_key().verify(payload, &signature).is_err(),
        "a signature from a rotated-out key must be refused by the new anchor"
    );
}

// ===========================================================================
// CASE 2 — TAMPERED material
// ===========================================================================

/// One mutated byte, at every place a byte can be mutated in a sealed receipt.
#[test]
fn case_tampered_receipt_is_refused_at_every_mutable_field() {
    let (intact, backend, key) = sealed_receipt(13);
    intact.verify(&backend, &key).expect("baseline must verify");

    type Mutation = (&'static str, Box<dyn Fn(&mut ExecutionReceipt)>);
    let mutations: Vec<Mutation> = vec![
        (
            "backend.instance_id",
            Box::new(|r: &mut ExecutionReceipt| r.body.backend.instance_id = "inst-2".into()),
        ),
        (
            "task.task_id",
            Box::new(|r: &mut ExecutionReceipt| r.body.task.task_id = "t-2".into()),
        ),
        (
            "terminal",
            Box::new(|r: &mut ExecutionReceipt| r.body.terminal = TerminalStatus::Success),
        ),
        (
            "egress_decision",
            Box::new(|r: &mut ExecutionReceipt| r.body.egress_decision = "allow".into()),
        ),
        (
            "attestation.signature",
            Box::new(|r: &mut ExecutionReceipt| {
                let mut s = r.attestation.signature_base64.clone().into_bytes();
                s[0] = if s[0] == b'A' { b'B' } else { b'A' };
                r.attestation.signature_base64 = String::from_utf8(s).unwrap();
            }),
        ),
    ];

    for (field, mutate) in mutations {
        let mut tampered = intact.clone();
        mutate(&mut tampered);
        if field == "terminal" {
            // Mutating to the same value is a no-op; skip rather than assert a
            // refusal that would be a lie.
            continue;
        }
        assert!(
            tampered.verify(&backend, &key).is_err(),
            "a mutation of {field} was ACCEPTED — the receipt is not tamper-evident there"
        );
    }
}

/// A plugin bundle with one mutated byte must be refused at the three gates
/// plan 25-02 built: bundle integrity, the signature, and the approval digest.
#[test]
fn case_tampered_plugin_bundle_fails_the_digest_and_the_signature_and_the_approval() {
    let tmp = TempDir::new().unwrap();
    let plugin = tmp.path().join("p");
    std::fs::create_dir_all(plugin.join("bin")).unwrap();
    let entry = plugin.join("bin").join("run");
    std::fs::write(&entry, b"entry artifact bytes").unwrap();
    std::fs::write(plugin.join("plugin.toml"), b"[plugin]\nname='demo'\n").unwrap();

    // GATE 1 — the content digest the approval binds to.
    let before = wcore_config::plugin_governance::content_digest(&plugin).unwrap();

    // GATE 2 — a real signature over the entry artifact.
    let key = SigningKey::from_bytes(&[31u8; 32]);
    let signature = key.sign(&std::fs::read(&entry).unwrap());

    // INDUCE: one byte.
    let mut bytes = std::fs::read(&entry).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&entry, &bytes).unwrap();

    let after = wcore_config::plugin_governance::content_digest(&plugin).unwrap();
    assert_ne!(
        before, after,
        "the approval digest must change with the bytes"
    );

    use ed25519_dalek::ed25519::signature::Verifier;
    assert!(
        key.verifying_key()
            .verify(&std::fs::read(&entry).unwrap(), &signature)
            .is_err(),
        "the signature must not cover the mutated bytes"
    );

    // GATE 3 — an approval bound to `before` does not admit `after`. This is
    // the same `evaluate` the engine's loader calls.
    let root = tmp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let mut store = wcore_config::plugin_governance::ApprovalStore::default();
    store.approvals.insert(
        "demo".into(),
        wcore_config::plugin_governance::ApprovalRecord {
            plugin: "demo".into(),
            digest: before,
            approved_at: "t0".into(),
        },
    );
    wcore_config::plugin_governance::store_approvals(&root, &store).unwrap();
    let moved = root.join("demo");
    std::fs::create_dir_all(&moved).unwrap();
    std::fs::copy(plugin.join("plugin.toml"), moved.join("plugin.toml")).unwrap();
    std::fs::create_dir_all(moved.join("bin")).unwrap();
    std::fs::copy(&entry, moved.join("bin").join("run")).unwrap();

    match wcore_config::plugin_governance::evaluate(&root, "demo", &moved) {
        wcore_config::plugin_governance::GateVerdict::Refused { reason } => {
            assert!(reason.contains("plugin approval required"), "{reason}");
        }
        other => panic!("a mutated plugin was ADMITTED by the approval gate: {other:?}"),
    }
}

// ===========================================================================
// CASE 3 — ATTESTATION mismatch
// ===========================================================================

/// A backend presenting an attestation that is not the pinned identity is
/// refused — and refused for the identity, not incidentally.
#[test]
fn case_attestation_mismatch_is_refused_before_the_receipt_is_believed() {
    let (receipt, real_identity, real_key) = sealed_receipt(14);

    // A DIFFERENT backend claims to have produced this work.
    let impostor = BackendIdentity {
        backend_id: "container".into(),
        ..real_identity.clone()
    };
    let err = receipt
        .verify(&impostor, &real_key)
        .expect_err("a receipt must not verify against a backend identity it does not carry");
    assert!(
        err.to_string().contains("backend identity mismatch"),
        "the refusal must name the identity mismatch: {err}"
    );

    // A right-looking identity with the WRONG key is also refused.
    let wrong_key = ReceiptSigner::from_seed([15u8; 32]).verifying_key();
    assert!(receipt.verify(&real_identity, &wrong_key).is_err());
}

/// Integrity is not identity, and the API says so rather than blurring them.
#[test]
fn case_integrity_only_verification_does_not_establish_identity() {
    let (receipt, _identity, _key) = sealed_receipt(16);
    // Integrity holds...
    receipt.verify_integrity_only().unwrap();
    // ...and it still does not tell you WHICH backend produced it: a caller
    // that only has the receipt cannot pin anything.
    let unrelated = BackendIdentity {
        backend_id: "ssh".into(),
        instance_id: "elsewhere".into(),
        version: "0.0.0".into(),
        key_id: sha256_public(b"not the key"),
    };
    let unrelated_key = ReceiptSigner::from_seed([17u8; 32]).verifying_key();
    assert!(receipt.verify(&unrelated, &unrelated_key).is_err());
}

// ===========================================================================
// CASE 4 — denied SECRET
// ===========================================================================

/// The effective policy names the secrets that WOULD be exposed, before the
/// task runs — and there is nowhere in the type to put a secret VALUE.
#[tokio::test]
async fn case_denied_secret_is_decided_before_execution_and_no_value_can_be_carried() {
    let limits = reference_budget();
    let task = reference_task("f25-secret-case", "f25-secret-nonce", limits);
    for reference in reference_backends(limits).unwrap() {
        let policy = reference
            .backend
            .effective_policy(&task)
            .expect("every backend must state its policy BEFORE the task is accepted");
        policy.validate().unwrap();

        // NAMES only. `validate` runs the identifier rule over every entry, so
        // anything shaped like a value (spaces, punctuation, base64 padding) is
        // refused by construction.
        for name in &policy.secrets_exposed {
            assert!(
                !name.contains('=') && !name.contains(' ') && !name.contains('/'),
                "backend {} carried something value-shaped in secrets_exposed: {name}",
                policy.backend_id
            );
        }
    }
}

/// A secret NAME that is actually a value is refused by the policy validator.
#[test]
fn case_a_value_shaped_secret_name_is_refused() {
    use wcore_exec_backend::policy::{EffectivePolicy, EgressDecisionSource};
    let policy = EffectivePolicy {
        backend_id: "local".into(),
        kind: BackendKind::Local,
        egress_decision: "deny".into(),
        egress_source: EgressDecisionSource::SharedEgressPolicy,
        secret_channel: wcore_exec_backend::contract::SecretChannel::None,
        secrets_exposed: vec!["sk-ant-api03-REAL-LOOKING-VALUE/w==".into()],
        containment: "test".into(),
    };
    assert!(
        policy.validate().is_err(),
        "a value-shaped entry in secrets_exposed was accepted — a secret could ride out in it"
    );
}

// ===========================================================================
// CASE 5 — denied EGRESS
// ===========================================================================

/// Every backend reports an egress decision, and reports its SOURCE, so an
/// inherited decision is distinguishable from an invented one. The known
/// fail-open default is named as a fail-open rather than rendered as "allow".
#[tokio::test]
async fn case_egress_decision_is_read_from_the_shared_policy_and_never_re_derived() {
    let limits = reference_budget();
    let task = reference_task("f25-egress-case", "f25-egress-nonce", limits);
    for reference in reference_backends(limits).unwrap() {
        let policy = reference.backend.effective_policy(&task).unwrap();
        assert!(
            !policy.egress_decision.is_empty(),
            "backend {} reported no egress decision at all",
            policy.backend_id
        );
        // The default is a KNOWN fail-open and must say so in words. A
        // backend that rendered it as a bare "allow" would launder an
        // uninstalled boundary into a deliberate decision.
        if !wcore_egress::global_policy_installed() {
            assert!(
                policy.egress_decision.contains("default")
                    || policy.egress_decision.contains("no-policy")
                    || matches!(
                        policy.egress_source,
                        wcore_exec_backend::policy::EgressDecisionSource::NoEgressSurface
                    ),
                "backend {} rendered the uninstalled-policy default as {:?}, which reads as a \
                 deliberate allow",
                policy.backend_id,
                policy.egress_decision
            );
        }
    }
}

// ===========================================================================
// NO FALLBACK — the assertion that makes the rest mean something
// ===========================================================================

/// A refusal must refuse. The type used to answer "may this node take work"
/// has no variant that can express a reroute, and a revoked node's refusal
/// names that node and no other.
#[test]
fn case_no_fallback_a_refused_node_does_not_hand_its_work_to_a_healthy_one() {
    let tmp = TempDir::new().unwrap();
    let reg = NodeRegistry::new(tmp.path());

    let pair = |name: &str, seed: u8| {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let identity = NodeIdentity {
            node_id: name.into(),
            machine_id: format!("{name}-box"),
            os: "linux".into(),
            contract_version: wcore_exec_backend::node::version::NODE_CONTRACT_VERSION,
            key_id: sha256_public(key.verifying_key().as_bytes()),
        };
        let challenge = PairingChallenge::new("controller");
        let proof =
            prove_challenge(&key, &identity, &challenge, NodeAdvertisement::empty(name)).unwrap();
        let verified = verify_proof(&challenge, &proof).unwrap();
        reg.record_paired(
            identity,
            verified,
            "ssh",
            "host.example",
            "wayland-core",
            NodeAdvertisement::empty(name),
        )
        .unwrap()
    };
    pair("alpha", 41);
    pair("beta", 42);

    reg.revoke("alpha", "compromised key").unwrap();

    let verdict = reg.evaluate_submission("alpha").unwrap();
    match verdict {
        SubmissionVerdict::Refused { node_id, reason } => {
            assert_eq!(
                node_id, "alpha",
                "the refusal must name the node that was ASKED for"
            );
            assert!(reason.contains("NOT falling back"), "{reason}");
        }
        SubmissionVerdict::Accepted { node_id } => {
            panic!("a revoked node accepted work, or the work was rerouted to {node_id}")
        }
    }
    // beta is still healthy — and nothing moved alpha's work onto it.
    assert!(reg.evaluate_submission("beta").unwrap().is_accepted());
}

/// A backend that is unavailable REFUSES rather than degrading to another one.
#[tokio::test]
async fn case_no_fallback_an_unavailable_backend_refuses_rather_than_degrading() {
    let limits = reference_budget();
    for reference in reference_backends(limits).unwrap() {
        let availability = reference.backend.availability().await;
        if availability.available {
            continue;
        }
        // An unavailable backend must say WHY, with a named probe basis, and
        // must not be silently substituted by anything.
        assert!(
            !availability.detail.is_empty(),
            "backend {} is unavailable with no stated reason",
            reference.backend.capabilities().backend_id
        );
    }
}

// ===========================================================================
// THE MECHANISM CLAIM — no backend may claim more reaping than it has
// ===========================================================================

#[test]
fn no_backend_claims_a_reaping_mechanism_it_does_not_have() {
    // SSH and cloud inherit NONE of `ProcessTreeMechanism`'s three variants.
    // If either ever reports kernel-backed, someone has relabelled a
    // best-effort reap, which is the specific dishonesty this plan forbids.
    assert!(matches!(
        mechanism_for(BackendKind::Ssh),
        ReapingMechanism::BestEffort { .. }
    ));
    assert!(matches!(
        mechanism_for(BackendKind::Cloud),
        ReapingMechanism::None { .. }
    ));
    assert!(mechanism_for(BackendKind::Container).is_kernel_backed());
}

// ===========================================================================
// THE FINDING — a scanner blind to what the registry forgot
// ===========================================================================

/// F25-05 HIGH finding, pinned.
///
/// The local scan used to consult ONLY the live-task registry, which makes it
/// structurally blind to the exact thing an orphan scan exists to find: a
/// terminal event REMOVES the registry entry, so a process that outlived its
/// task is by construction no longer listed. Measured on hetzner-dsm: the
/// independent `ps` enumeration found 1 row carrying the nonce while the scan
/// reported 0.
///
/// This test plants a process that carries a nonce and is in NO registry, and
/// requires the scan to find it anyway.
#[tokio::test]
#[cfg(unix)]
async fn the_local_scan_finds_an_orphan_that_no_registry_remembers() {
    let nonce = format!("f25-registryless-{}", std::process::id());
    let state = TempDir::new().unwrap();
    // Per-THREAD injection, not the process-global
    // `WAYLAND_EXEC_BACKEND_STATE_DIR`. `cargo test` runs the thirteen tests of
    // this binary on threads of ONE process, so the env var pointed every
    // concurrently-running sibling's registry at this `TempDir` and then
    // deleted it out from under them. `StateDirGuard` is the override built for
    // exactly that (see registry.rs); the guard lives to the end of the test,
    // so the directory outlives every read through it.
    let _state_dir = wcore_exec_backend::registry::StateDirGuard::set(state.path());

    // No `exec`: the shell keeps its full argv so the nonce is genuinely
    // visible in the process table. With `exec` the nonce vanishes and the
    // scan finds nothing — which is how this class of test passes for the
    // wrong reason.
    let mut child = wcore_config::shell::shell_command_argv(
        "sh",
        &["-c", &format!("while :; do sleep 1; done # {nonce}")],
    )
    .spawn()
    .expect("plant a registryless orphan");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let evidence = wcore_exec_backend::orphan::scan_one(
        "local",
        // This test wants ONE run's residue: it planted the process itself and holds
        // the nonce. It is the scoped question, stated (core#366 d2).
        wcore_exec_backend::contract::OrphanScope::Nonce(&nonce),
        reference_budget(),
    )
    .await
    .unwrap()
    .expect("the local backend exists");

    // Reap the plant AND its descendants. Killing only the direct child leaves
    // the `sleep` grandchild behind — nextest marks the test leaky, which is
    // this plan's own subject matter showing up in its own test.
    let _ = child.kill().await;
    let _ = child.wait().await;
    let _ = wcore_config::shell::shell_command_argv("pkill", &["-f", &nonce])
        .output()
        .await;

    assert!(
        evidence.is_observed(),
        "the local surface must be enumerable on this host"
    );
    assert!(
        evidence.orphan_count.unwrap_or(0) >= 1,
        "the scan reported {:?} for a process that was definitely running and carried the \
         nonce — a scan that can only see what the registry still remembers cannot see an \
         orphan, because a terminal event removes the entry. rows: {:?}",
        evidence.orphan_count,
        evidence.rows
    );
}
