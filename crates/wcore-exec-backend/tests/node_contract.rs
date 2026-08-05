//! F25-03 — the node contract, prosecuted hostilely.
//!
//! Every test here asks the attribution question AFTER the disruption, because
//! Success Criterion 2's load-bearing clause is "…without losing authority
//! attribution", not the four verbs in front of it. A node that pairs, works,
//! vanishes, returns, and then produces work nobody can tie back to the
//! authority that requested it has failed the criterion with every verb
//! reporting success.

#![allow(clippy::panic, clippy::unwrap_used)]

use ed25519_dalek::SigningKey;
use tempfile::TempDir;

use wcore_exec_backend::contract::{BackendKind, HibernationObservation, ResourceBudget};
use wcore_exec_backend::node::attribution::{
    AttributionVerdict, NodeAttribution, verify_node_attribution,
};
use wcore_exec_backend::node::capability::{AdvertisedBackend, NodeAdvertisement};
use wcore_exec_backend::node::pairing::{
    NodeIdentity, PairingChallenge, PairingProof, prove_challenge, verify_proof,
};
use wcore_exec_backend::node::registry::{Liveness, NodeRecord, NodeRegistry, SubmissionVerdict};
use wcore_exec_backend::node::version::{
    NODE_CONTRACT_MAJOR, NODE_CONTRACT_MINOR, NODE_CONTRACT_VERSION, NodeContractVersion,
    VersionVerdict, evaluate_version,
};
use wcore_exec_backend::receipt::{
    ArtifactEvidence, BackendIdentity, EventKind, ExecutionReceipt, PROTOCOL_VERSION, ReceiptBody,
    ReceiptEvent, ReceiptSigner, TaskEvidence, TerminalStatus, Timing, Transport, events_digest,
    sha256_public,
};

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn node_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// Drive [`NodeIdentity::local`] in a CHILD PROCESS whose environment has the
/// three machine-id override variables REMOVED, and read the derived values
/// back as `KEY=VALUE` lines.
///
/// **Why a child process rather than an in-process call.** These two tests
/// measure the *file-fallback* branch of `local_machine_id()`, which is only
/// reached when `WAYLAND_NODE_MACHINE_ID`, `HOSTNAME` and `COMPUTERNAME` are
/// all unset. The original pair asserted that as an ambient precondition and
/// failed loudly when it did not hold — deliberately, because a skip is
/// indistinguishable from a pass. That was the right instinct and the wrong
/// instrument: **GitHub Actions containers always export `HOSTNAME`**, so on
/// `CI (linux-containerized)` the Linux arm could never pass, and it went red
/// on every run of the suite (run 30434804220, `TRY 3 FAIL` on all retries)
/// while asserting nothing about the code.
///
/// Clearing the variables in-process is not an option either: `std::env::set_var`
/// / `remove_var` are `unsafe` and process-global, and this binary runs its
/// tests in parallel threads — a mutation here would race every other test.
///
/// A child process removes the ambient dependency entirely. The measurement
/// becomes deterministic on ANY host, in CI or over non-login ssh, and the
/// "never skip" property is kept because nothing is conditional: the child
/// always runs and the parent always asserts.
///
/// The child is [`local_identity_probe_fixture`], `#[ignore]`d so nextest never
/// schedules it on its own. Same re-exec idiom as
/// `crates/wcore-swarm/tests/workspace_authority.rs`.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn probe_local_identity_in_a_cleaned_environment() -> std::collections::BTreeMap<String, String> {
    let exe = std::env::current_exe().expect("current test executable");
    let mut command = wcore_config::shell::shell_command_argv(
        &exe.to_string_lossy(),
        &[
            "--ignored",
            "--exact",
            "local_identity_probe_fixture",
            "--nocapture",
        ],
    );
    for var in ["WAYLAND_NODE_MACHINE_ID", "HOSTNAME", "COMPUTERNAME"] {
        command.env_remove(var);
    }
    // `shell_command_argv` hands back a `tokio::process::Command` (argv mode, no
    // shell interpreter). This test is synchronous, so run it through the inner
    // `std` command rather than pulling a runtime in — same idiom as
    // `wcore-exec-backend/src/backends/local.rs:189`.
    let output = command
        .as_std_mut()
        .output()
        .expect("run the local-identity probe fixture");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "the local-identity probe fixture failed: {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );

    let values: std::collections::BTreeMap<String, String> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("PROBE_"))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();

    // Anti-vacuity (LANE-BRIEF 3.2). `libtest` exits 0 printing
    // `0 passed; 0 filtered out` when `--exact <name>` matches NOTHING, so a
    // renamed fixture would make every assertion below vanish rather than fail.
    // Requiring the keys makes that impossible: an empty map fails here.
    for key in [
        "OS",
        "MACHINE_ID",
        "KEY_ID",
        "SECOND_MACHINE_ID",
        "SECOND_KEY_ID",
        "VALIDATES",
    ] {
        assert!(
            values.contains_key(key),
            "the probe fixture did not report PROBE_{key} — it probably did not run \
             (a `--exact` filter that matches no test exits 0 having run nothing).\
             \nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
    values
}

/// The child half of [`probe_local_identity_in_a_cleaned_environment`]. Makes
/// no assertion about the ambient environment — the PARENT owns the cleaning —
/// so running it directly (`--run-ignored all`) can never produce a false
/// claim; it only prints what this environment derived.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
#[ignore = "child fixture: driven by the local-identity tests via a re-exec with a cleaned environment"]
fn local_identity_probe_fixture() {
    let identity =
        NodeIdentity::local("probe-node", &node_signing_key(9)).expect("build a local identity");
    let second = NodeIdentity::local("second-node", &node_signing_key(11))
        .expect("build a second local identity");
    let validates = match identity.validate() {
        Ok(()) => "yes".to_string(),
        Err(error) => format!("no:{error}"),
    };
    println!("PROBE_OS={}", identity.os);
    println!("PROBE_MACHINE_ID={}", identity.machine_id);
    println!("PROBE_KEY_ID={}", identity.key_id);
    println!("PROBE_SECOND_MACHINE_ID={}", second.machine_id);
    println!("PROBE_SECOND_KEY_ID={}", second.key_id);
    println!("PROBE_VALIDATES={validates}");
}

/// `NodeIdentity::local()` — the ONLY function that derives a real host's
/// identity — had no test on any platform before this one. Every other test in
/// this file builds `NodeIdentity` from a struct literal via [`identity_for`],
/// so the derivation path was never executed by the suite. That is how the
/// Linux-shaped assumption below survived.
///
/// `local_machine_id()` tries, in order: `WAYLAND_NODE_MACHINE_ID`, `HOSTNAME`,
/// `COMPUTERNAME`, then `read_hostname_file()`, then gives up and returns the
/// constant `"unknown-host"`. `read_hostname_file` is documented *"Unix hosts
/// publish the hostname on disk regardless of shell environment"* and reads
/// `/etc/hostname` and `/proc/sys/kernel/hostname`.
///
/// **That comment is false on macOS.** Darwin has neither file and no `/proc`
/// at all; it keeps the hostname in the SystemConfiguration store. So on a Mac
/// the chain runs off its end and every node reports the same constant.
///
/// The field's declared job (`pairing.rs`) is *"Stable per-host discriminator.
/// Distinguishes two nodes an operator happened to give confusingly similar
/// names."* A constant discriminates nothing, and the value still passes
/// `validate()`, so nothing downstream rejects it.
#[cfg(target_os = "macos")]
#[test]
fn on_darwin_local_identity_falls_back_to_a_constant_because_the_hostname_files_are_linux_only() {
    // The two paths the fallback reads. Both absent on Darwin.
    for path in ["/etc/hostname", "/proc/sys/kernel/hostname"] {
        assert!(
            !std::path::Path::new(path).exists(),
            "{path} exists on this Darwin host, which contradicts the premise"
        );
    }
    // Known-positive control in the same test: without it the two assertions
    // above would also pass on a filesystem probe that answered "no" to
    // everything (LANE-BRIEF 3b-i).
    assert!(
        std::path::Path::new("/etc/hosts").exists(),
        "instrument check failed: /etc/hosts must exist, so the two ABSENT \
         results above are a measurement rather than a blind probe"
    );

    let probe = probe_local_identity_in_a_cleaned_environment();

    assert_eq!(probe["OS"], "macos", "sanity: this arm is Darwin-only");
    assert_eq!(
        probe["MACHINE_ID"], "unknown-host",
        "on Darwin the machine_id fallback chain runs off its end. If this now \
         reads a real hostname, the Linux-shaped fallback has been repaired and \
         this test should be updated to assert the real value."
    );

    // The consequence that makes it a defect rather than a cosmetic gap: the
    // degenerate value is accepted by the contract's own validator, so it
    // propagates into the registry and into `node list` unchallenged.
    assert_eq!(
        probe["VALIDATES"], "yes",
        "the degenerate machine_id still validates — nothing rejects it"
    );

    // machine_id carries ZERO host-distinguishing information here: it is a
    // compile-time constant, independent of this host. Two different Darwin
    // hosts therefore cannot be told apart by it. Demonstrated within one
    // process by showing the value does not vary with the identity's key.
    assert_eq!(
        probe["SECOND_MACHINE_ID"], probe["MACHINE_ID"],
        "machine_id is a constant on Darwin"
    );
    assert_ne!(
        probe["SECOND_KEY_ID"], probe["KEY_ID"],
        "key_id still differs — the security identity is unaffected, which is \
         why this is a MEDIUM operator-facing defect and not a key-forgery one"
    );

    println!(
        "DARWIN machine_id fallback: machine_id={} os={} validates=yes key_id_differs=yes",
        probe["MACHINE_ID"], probe["OS"]
    );
}

/// The Linux half of the divergence above, so the pair is executable on both
/// sides rather than measured on one and inferred on the other.
///
/// Same code, same cleaned environment, opposite outcome: Linux publishes the
/// hostname at `/etc/hostname` and `/proc/sys/kernel/hostname`, so the fallback
/// returns a real per-host value and `machine_id` does the job it is documented
/// to do. This is the shape a controller reaches a node with over non-login
/// ssh — and, since the environment is cleaned by the probe rather than assumed,
/// it is now also the shape it has inside a CI container that exports `HOSTNAME`.
#[cfg(target_os = "linux")]
#[test]
fn on_linux_local_identity_reads_a_real_per_host_value_from_the_hostname_file() {
    // At least one of the two fallback paths must exist and be non-empty —
    // this is the thing Darwin lacks.
    let from_file = ["/etc/hostname", "/proc/sys/kernel/hostname"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .expect("Linux publishes the hostname on disk; neither path was readable");
    assert!(
        !from_file.is_empty(),
        "instrument check: the hostname file read back empty"
    );

    let probe = probe_local_identity_in_a_cleaned_environment();

    assert_eq!(probe["OS"], "linux", "sanity: this arm is Linux-only");
    assert_ne!(
        probe["MACHINE_ID"], "unknown-host",
        "on Linux the fallback must find the hostname file. Reading \
         'unknown-host' here would mean the Darwin defect has spread."
    );
    assert_eq!(probe["VALIDATES"], "yes", "identity validates");
    // The fallback must return THIS host's published hostname, not merely
    // something that is not the constant — otherwise any non-empty string
    // would satisfy the assertion above.
    assert_eq!(
        probe["MACHINE_ID"],
        sanitized_hostname(&from_file),
        "machine_id must be derived from the hostname file this host publishes"
    );

    println!(
        "LINUX machine_id fallback: machine_id={} os={} hostname_file={} \
         (Darwin returns the constant 'unknown-host' here)",
        probe["MACHINE_ID"], probe["OS"], from_file
    );
}

/// Faithful mirror of the private `sanitize_identifier` in
/// `crates/wcore-exec-backend/src/node/pairing.rs:117`, for the one assertion
/// that compares the derived `machine_id` against the raw hostname file.
/// If that function changes, change this one — a failure here is a real
/// contract change, not noise.
#[cfg(target_os = "linux")]
fn sanitized_hostname(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "unknown-host".to_string()
    } else {
        trimmed
    }
}

fn identity_for(node_id: &str, machine: &str, os: &str, key: &SigningKey) -> NodeIdentity {
    NodeIdentity {
        node_id: node_id.into(),
        machine_id: machine.into(),
        os: os.into(),
        contract_version: NODE_CONTRACT_VERSION,
        key_id: sha256_public(key.verifying_key().as_bytes()),
    }
}

fn advertisement(
    node_id: &str,
    os: &str,
    backends: &[(&str, BackendKind, bool)],
) -> NodeAdvertisement {
    NodeAdvertisement {
        node_id: node_id.into(),
        os: os.into(),
        contract_version: NODE_CONTRACT_VERSION,
        observed_unix_ms: 1_000_000,
        backends: backends
            .iter()
            .map(|(id, kind, available)| AdvertisedBackend {
                backend_id: (*id).into(),
                kind: *kind,
                version: "0.12.25".into(),
                available: *available,
                probe_basis: if *available {
                    "daemon_ping".into()
                } else {
                    "probe_failed".into()
                },
                detail: "observed".into(),
            })
            .collect(),
    }
}

fn registry() -> (TempDir, NodeRegistry) {
    let tmp = TempDir::new().unwrap();
    let reg = NodeRegistry::new(tmp.path());
    (tmp, reg)
}

fn pair_node(
    reg: &NodeRegistry,
    node_id: &str,
    machine: &str,
    os: &str,
    seed: u8,
    ad: NodeAdvertisement,
) -> NodeRecord {
    let key = node_signing_key(seed);
    let identity = identity_for(node_id, machine, os, &key);
    // Go through a REAL challenge/proof round trip rather than shortcutting to
    // `record_paired` — a test that skips the proof would not notice if the
    // proof stopped being required.
    let challenge = PairingChallenge::new("controller-key-id");
    let proof = prove_challenge(&key, &identity, &challenge, ad.clone()).unwrap();
    let verified = verify_proof(&challenge, &proof).unwrap();
    reg.record_paired(
        identity,
        verified,
        "ssh",
        "host.example",
        "wayland-core",
        ad,
    )
    .unwrap()
}

/// A sealed, verifiable receipt attributed to `node`.
fn receipt_for(
    node: Option<&NodeIdentity>,
    backend_seed: u8,
) -> (
    ExecutionReceipt,
    BackendIdentity,
    ed25519_dalek::VerifyingKey,
) {
    let signer = ReceiptSigner::from_seed([backend_seed; 32]);
    let backend = BackendIdentity {
        backend_id: "local".into(),
        instance_id: "inst-1".into(),
        version: "0.12.25".into(),
        key_id: signer.key_id().to_string(),
    };
    let limits = ResourceBudget::new(1_000, 1 << 20, 5_000, 1 << 16).unwrap();
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
        node: node.map(NodeAttribution::from_identity),
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

// ---------------------------------------------------------------------------
// PAIRING
// ---------------------------------------------------------------------------

#[test]
fn pairing_records_a_node_only_after_a_real_proof() {
    let (_t, reg) = registry();
    let record = pair_node(
        &reg,
        "alpha",
        "linux-box",
        "linux",
        7,
        advertisement("alpha", "linux", &[("local", BackendKind::Local, true)]),
    );
    assert_eq!(record.identity.node_id, "alpha");
    assert_eq!(reg.list().unwrap().len(), 1);
    assert!(reg.evaluate_submission("alpha").unwrap().is_accepted());
}

/// A far end that cannot prove possession of its key is REFUSED, and — the part
/// that matters — nothing is written. Recording it as "unverified" would be the
/// quiet failure this contract exists to prevent.
#[test]
fn an_unprovable_far_end_is_refused_and_leaves_no_record() {
    let (_t, reg) = registry();
    let real = node_signing_key(7);
    let impostor = node_signing_key(8);
    let stolen_identity = identity_for("alpha", "linux-box", "linux", &real);
    let challenge = PairingChallenge::new("controller-key-id");

    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let forged = PairingProof {
        identity: stolen_identity,
        verifying_key_base64: b64.encode(impostor.verifying_key().as_bytes()),
        signature_base64: b64.encode([0u8; 64]),
        nonce: challenge.nonce.clone(),
        advertisement: NodeAdvertisement::empty("alpha"),
    };

    assert!(verify_proof(&challenge, &forged).is_err());
    assert!(
        reg.list().unwrap().is_empty(),
        "a refused pairing must leave NOTHING behind"
    );
    let verdict = reg.evaluate_submission("alpha").unwrap();
    assert!(!verdict.is_accepted());
    assert!(verdict.reason().unwrap().contains("not paired"));
}

// ---------------------------------------------------------------------------
// CAPABILITY ADVERTISEMENT
// ---------------------------------------------------------------------------

/// Two different operating systems must be able to advertise differently. An
/// identical hardcoded list across a Linux and a Windows host would be a defect
/// wearing a pass, so the contract has to be able to express the difference.
#[test]
fn two_nodes_on_different_operating_systems_advertise_differently() {
    let (_t, reg) = registry();
    let linux = pair_node(
        &reg,
        "alpha",
        "linux-box",
        "linux",
        7,
        advertisement(
            "alpha",
            "linux",
            &[
                ("local", BackendKind::Local, true),
                ("container", BackendKind::Container, true),
            ],
        ),
    );
    let windows = pair_node(
        &reg,
        "beta",
        "win-box",
        "windows",
        8,
        advertisement(
            "beta",
            "windows",
            &[
                ("local", BackendKind::Local, true),
                ("container", BackendKind::Container, false),
            ],
        ),
    );
    assert_ne!(linux.identity.os, windows.identity.os);
    assert_eq!(linux.advertisement.available_backends().len(), 2);
    assert_eq!(windows.advertisement.available_backends().len(), 1);
}

/// A node whose daemon dies must stop claiming that backend. Refreshing is what
/// makes that possible; a cached advertisement never can.
#[test]
fn a_refreshed_advertisement_drops_a_backend_that_died() {
    let (_t, reg) = registry();
    pair_node(
        &reg,
        "alpha",
        "linux-box",
        "linux",
        7,
        advertisement(
            "alpha",
            "linux",
            &[
                ("local", BackendKind::Local, true),
                ("container", BackendKind::Container, true),
            ],
        ),
    );
    assert_eq!(
        reg.get("alpha")
            .unwrap()
            .unwrap()
            .advertisement
            .available_backends()
            .len(),
        2
    );

    reg.set_advertisement(
        "alpha",
        advertisement(
            "alpha",
            "linux",
            &[
                ("local", BackendKind::Local, true),
                ("container", BackendKind::Container, false),
            ],
        ),
    )
    .unwrap();
    let after = reg.get("alpha").unwrap().unwrap();
    assert_eq!(after.advertisement.available_backends().len(), 1);
    assert_eq!(
        after.advertisement.available_backends()[0].backend_id,
        "local"
    );
}

// ---------------------------------------------------------------------------
// REVOCATION — a refusal, not a forget, and never a reroute
// ---------------------------------------------------------------------------

#[test]
fn revocation_refuses_work_terminates_nothing_silently_and_never_reroutes() {
    let (_t, reg) = registry();
    pair_node(
        &reg,
        "alpha",
        "linux-box",
        "linux",
        7,
        advertisement("alpha", "linux", &[("local", BackendKind::Local, true)]),
    );
    pair_node(
        &reg,
        "beta",
        "win-box",
        "windows",
        8,
        advertisement("beta", "windows", &[("local", BackendKind::Local, true)]),
    );

    reg.revoke("alpha", "key suspected compromised").unwrap();

    let verdict = reg.evaluate_submission("alpha").unwrap();
    match verdict {
        SubmissionVerdict::Refused { node_id, reason } => {
            assert_eq!(node_id, "alpha", "the refusal must name the node asked for");
            assert!(reason.contains("REVOKED"), "{reason}");
            assert!(
                reason.contains("NOT falling back"),
                "the refusal must state that nothing reroutes: {reason}"
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }

    // The record is RETAINED, not deleted — that is what stops the far end
    // pairing itself back in.
    assert!(reg.get("alpha").unwrap().unwrap().is_revoked());
    assert_eq!(reg.list().unwrap().len(), 2);
}

#[test]
fn a_revoked_node_cannot_re_pair_itself_but_an_operator_can() {
    let (_t, reg) = registry();
    let ad = advertisement("alpha", "linux", &[("local", BackendKind::Local, true)]);
    pair_node(&reg, "alpha", "linux-box", "linux", 7, ad.clone());
    reg.revoke("alpha", "compromised").unwrap();

    let key = node_signing_key(7);
    let identity = identity_for("alpha", "linux-box", "linux", &key);
    let challenge = PairingChallenge::new("controller-key-id");
    let proof = prove_challenge(&key, &identity, &challenge, ad.clone()).unwrap();
    let verified = verify_proof(&challenge, &proof).unwrap();
    // The proof is GENUINE — and it still does not get the node back in.
    let err = reg
        .record_paired(
            identity.clone(),
            verified,
            "ssh",
            "host.example",
            "wayland-core",
            ad.clone(),
        )
        .unwrap_err();
    assert!(err.to_string().contains("REVOKED"), "{err}");

    reg.clear_revocation("alpha").unwrap();
    pair_node(&reg, "alpha", "linux-box", "linux", 7, ad);
    assert!(reg.evaluate_submission("alpha").unwrap().is_accepted());
}

// ---------------------------------------------------------------------------
// MIXED VERSIONS — three verdicts, no silent down-negotiation
// ---------------------------------------------------------------------------

#[test]
fn version_handling_produces_exactly_three_distinct_verdicts() {
    assert_eq!(
        evaluate_version(NODE_CONTRACT_VERSION),
        VersionVerdict::Same
    );

    let newer_major = evaluate_version(NodeContractVersion {
        major: NODE_CONTRACT_MAJOR + 1,
        minor: 0,
    });
    assert!(matches!(newer_major, VersionVerdict::Unsupported { .. }));
    assert!(!newer_major.accepts_work());

    let newer_minor = evaluate_version(NodeContractVersion {
        major: NODE_CONTRACT_MAJOR,
        minor: NODE_CONTRACT_MINOR + 1,
    });
    assert!(matches!(newer_minor, VersionVerdict::Unsupported { .. }));
    assert!(!newer_minor.accepts_work());
}

/// The forbidden move, tested against directly: an accepted older node must
/// NAME what it cannot honour. A reduced set that is empty is exactly what
/// silent down-negotiation looks like from the outside.
#[test]
fn an_older_supported_node_names_its_reduced_capability_set() {
    let verdict = VersionVerdict::OlderSupported {
        node: NodeContractVersion {
            major: NODE_CONTRACT_MAJOR,
            minor: 0,
        },
        local: NODE_CONTRACT_VERSION,
        reduced: vec!["attested-receipts".into()],
    };
    assert!(verdict.accepts_work());
    let label = verdict.label();
    assert!(label.contains("reduced"), "{label}");
    assert!(label.contains("cannot honour"), "{label}");
    assert!(label.contains("attested-receipts"), "{label}");
}

#[test]
fn an_unsupported_version_refuses_work_at_the_registry() {
    let (_t, reg) = registry();
    let key = node_signing_key(9);
    let mut identity = identity_for("gamma", "old-box", "linux", &key);
    identity.contract_version = NodeContractVersion {
        major: 99,
        minor: 0,
    };
    let ad = NodeAdvertisement::empty("gamma");
    let challenge = PairingChallenge::new("controller-key-id");
    let proof = prove_challenge(&key, &identity, &challenge, ad.clone()).unwrap();
    let verified = verify_proof(&challenge, &proof).unwrap();
    reg.record_paired(
        identity,
        verified,
        "ssh",
        "host.example",
        "wayland-core",
        ad,
    )
    .unwrap();

    let verdict = reg.evaluate_submission("gamma").unwrap();
    assert!(!verdict.is_accepted());
    let reason = verdict.reason().unwrap();
    assert!(reason.contains("unsupported"), "{reason}");
    // And `node list` can show the same named verdict.
    let record = reg.get("gamma").unwrap().unwrap();
    assert!(record.version_verdict().label().contains("unsupported"));
}

// ---------------------------------------------------------------------------
// OFFLINE + RECOVERY
// ---------------------------------------------------------------------------

#[test]
fn a_node_observed_offline_refuses_work_and_does_not_reroute() {
    let (_t, reg) = registry();
    pair_node(
        &reg,
        "alpha",
        "linux-box",
        "linux",
        7,
        advertisement("alpha", "linux", &[("local", BackendKind::Local, true)]),
    );
    pair_node(
        &reg,
        "beta",
        "win-box",
        "windows",
        8,
        advertisement("beta", "windows", &[("local", BackendKind::Local, true)]),
    );

    reg.set_liveness(
        "alpha",
        Liveness::Offline {
            observed_unix_ms: 10,
            detail: "ssh handshake did not reach the far end".into(),
        },
    )
    .unwrap();

    let verdict = reg.evaluate_submission("alpha").unwrap();
    assert!(!verdict.is_accepted());
    assert!(verdict.reason().unwrap().contains("NOT falling back"));
    match verdict {
        SubmissionVerdict::Refused { node_id, .. } => assert_eq!(node_id, "alpha"),
        other => panic!("expected refusal, got {other:?}"),
    }
}

/// "Never probed" and "probed and gone" are different facts. Collapsing them
/// makes a controller report confidence it does not have.
#[test]
fn unknown_liveness_is_not_the_same_as_offline() {
    let (_t, reg) = registry();
    let record = pair_node(
        &reg,
        "alpha",
        "linux-box",
        "linux",
        7,
        NodeAdvertisement::empty("alpha"),
    );
    assert_eq!(record.liveness, Liveness::Unknown);
    assert!(record.liveness.label().contains("not probed"));
    assert!(!record.liveness.is_live());
    assert!(reg.evaluate_submission("alpha").unwrap().is_accepted());
}

#[test]
fn the_disconnected_terminal_status_exists_and_is_terminal() {
    let terminal = TerminalStatus::Disconnected {
        reason: "node vanished mid-task".into(),
    };
    // The receipt vocabulary already carries it, inherited from the F04 oracle.
    let event = EventKind::Disconnected {
        reason: "node vanished mid-task".into(),
    };
    assert!(event.is_terminal());
    assert!(matches!(terminal, TerminalStatus::Disconnected { .. }));
}

// ---------------------------------------------------------------------------
// ATTRIBUTION — asked AFTER every disruption
// ---------------------------------------------------------------------------

#[test]
fn attribution_holds_for_work_a_paired_node_did() {
    let (_t, reg) = registry();
    let key = node_signing_key(7);
    let identity = identity_for("alpha", "linux-box", "linux", &key);
    let record = pair_node(
        &reg,
        "alpha",
        "linux-box",
        "linux",
        7,
        NodeAdvertisement::empty("alpha"),
    );
    let (receipt, backend, backend_key) = receipt_for(Some(&identity), 30);
    let verdict = verify_node_attribution(&receipt, &backend, &backend_key, &record);
    assert!(verdict.holds(), "{}", verdict.label());
}

/// THE test. Every disruption the criterion names, with attribution re-asked
/// after each one.
#[test]
fn attribution_survives_every_disruption_the_criterion_names() {
    let (_t, reg) = registry();
    let key = node_signing_key(7);
    let identity = identity_for("alpha", "linux-box", "linux", &key);
    let ad = advertisement("alpha", "linux", &[("local", BackendKind::Local, true)]);
    pair_node(&reg, "alpha", "linux-box", "linux", 7, ad.clone());

    // Work happens while the node is healthy.
    let (receipt, backend, backend_key) = receipt_for(Some(&identity), 30);
    let check = |record: &NodeRecord, after: &str| {
        let verdict = verify_node_attribution(&receipt, &backend, &backend_key, record);
        assert!(
            verdict.holds(),
            "attribution BROKEN after {after}: {}",
            verdict.label()
        );
    };
    check(&reg.get("alpha").unwrap().unwrap(), "pairing");

    // 1. DISCONNECT
    reg.set_liveness(
        "alpha",
        Liveness::Offline {
            observed_unix_ms: 10,
            detail: "far end unreachable".into(),
        },
    )
    .unwrap();
    check(&reg.get("alpha").unwrap().unwrap(), "disconnect");

    // 2. RETURN
    reg.set_liveness(
        "alpha",
        Liveness::Live {
            observed_unix_ms: 20,
        },
    )
    .unwrap();
    check(&reg.get("alpha").unwrap().unwrap(), "return");

    // 3. REVOKE — future authority is gone; past work stays attributable,
    //    which is exactly what an audit needs.
    reg.revoke("alpha", "operator withdrew authority").unwrap();
    check(&reg.get("alpha").unwrap().unwrap(), "revoke");

    // 4. RE-PAIR
    reg.clear_revocation("alpha").unwrap();
    pair_node(&reg, "alpha", "linux-box", "linux", 7, ad.clone());
    check(&reg.get("alpha").unwrap().unwrap(), "re-pair");

    // 5. VERSION MISMATCH — the node starts advertising a version this build
    //    refuses. Work is refused going FORWARD, and prior work stays attributable.
    reg.clear_revocation("alpha").unwrap();
    let mut future = identity_for("alpha", "linux-box", "linux", &key);
    future.contract_version = NodeContractVersion {
        major: 99,
        minor: 0,
    };
    let challenge = PairingChallenge::new("controller-key-id");
    let proof = prove_challenge(&key, &future, &challenge, ad.clone()).unwrap();
    let verified = verify_proof(&challenge, &proof).unwrap();
    reg.record_paired(future, verified, "ssh", "host.example", "wayland-core", ad)
        .unwrap();
    assert!(
        !reg.evaluate_submission("alpha").unwrap().is_accepted(),
        "a version-mismatched node must refuse NEW work"
    );
    // The identity's key_id is unchanged, so prior work is still attributable —
    // the version changed, the machine did not.
    let after_mismatch = reg.get("alpha").unwrap().unwrap();
    let verdict = verify_node_attribution(&receipt, &backend, &backend_key, &after_mismatch);
    assert!(
        verdict.holds(),
        "attribution BROKEN after version-mismatch: {}",
        verdict.label()
    );
}

/// The negative control for the test above: if attribution could not break,
/// "it held after every disruption" would prove nothing.
#[test]
fn attribution_does_break_when_it_should() {
    let (_t, reg) = registry();
    let record = pair_node(
        &reg,
        "alpha",
        "linux-box",
        "linux",
        7,
        NodeAdvertisement::empty("alpha"),
    );

    // Work done by a DIFFERENT machine.
    let other_key = node_signing_key(8);
    let other = identity_for("beta", "win-box", "windows", &other_key);
    let (receipt, backend, backend_key) = receipt_for(Some(&other), 30);
    let verdict = verify_node_attribution(&receipt, &backend, &backend_key, &record);
    assert!(!verdict.holds());
    assert!(matches!(verdict, AttributionVerdict::Broken { .. }));
}

/// Tampering with the node identity has to fail EXACTLY as tampering with the
/// backend identity does — same mechanism, not a parallel weaker check.
#[test]
fn a_tampered_node_identity_fails_verification_like_a_tampered_backend_one() {
    let (_t, reg) = registry();
    let key = node_signing_key(7);
    let identity = identity_for("alpha", "linux-box", "linux", &key);
    let record = pair_node(
        &reg,
        "alpha",
        "linux-box",
        "linux",
        7,
        NodeAdvertisement::empty("alpha"),
    );

    // Baseline: intact receipt verifies.
    let (intact, backend, backend_key) = receipt_for(Some(&identity), 30);
    assert!(intact.verify(&backend, &backend_key).is_ok());

    // Tamper the NODE identity.
    let mut node_tampered = intact.clone();
    node_tampered.body.node.as_mut().unwrap().node_id = "beta".into();
    let node_err = node_tampered.verify(&backend, &backend_key).unwrap_err();

    // Tamper the BACKEND identity.
    let mut backend_tampered = intact.clone();
    backend_tampered.body.backend.instance_id = "inst-2".into();
    let backend_err = backend_tampered.verify(&backend, &backend_key).unwrap_err();

    // Both are refusals from the same verification path.
    assert!(
        node_err.to_string().contains("digest") || node_err.to_string().contains("attestation"),
        "node tamper: {node_err}"
    );
    assert!(!backend_err.to_string().is_empty());

    let verdict = verify_node_attribution(&node_tampered, &backend, &backend_key, &record);
    assert!(!verdict.holds());
}

/// A receipt with no node identity is UNATTRIBUTED, not BROKEN. Two different
/// facts; collapsing them would make pre-node receipts look like tampering.
#[test]
fn a_receipt_without_a_node_identity_is_unattributed_not_broken() {
    let (_t, reg) = registry();
    let record = pair_node(
        &reg,
        "alpha",
        "linux-box",
        "linux",
        7,
        NodeAdvertisement::empty("alpha"),
    );
    let (receipt, backend, backend_key) = receipt_for(None, 30);
    let verdict = verify_node_attribution(&receipt, &backend, &backend_key, &record);
    assert_eq!(verdict, AttributionVerdict::Unattributed);
}

/// Backwards compatibility, proven rather than asserted: a receipt sealed
/// WITHOUT a node field serializes to bytes that verify unchanged, so every
/// receipt plan 25-01 produced is still valid.
#[test]
fn adding_the_node_slot_did_not_invalidate_receipts_that_predate_it() {
    let (receipt, backend, key) = receipt_for(None, 30);
    assert!(receipt.body.node.is_none());
    let json = serde_json::to_string(&receipt).unwrap();
    assert!(
        !json.contains("\"node\""),
        "an absent node must not appear in the wire form at all, or the bytes changed"
    );
    let round: ExecutionReceipt = serde_json::from_str(&json).unwrap();
    assert!(round.verify(&backend, &key).is_ok());
}
