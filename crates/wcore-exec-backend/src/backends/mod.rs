//! The four reference backends, plus the machinery every one of them shares.
//!
//! Receipt ASSEMBLY is shared deliberately. What the four references are
//! supposed to differ on is how the work actually runs — a contained fork, a
//! container, an ssh session, a cloud machine — not how a run is described
//! afterwards. Sharing the description is what makes a disagreement in the
//! normalized body mean something: if two backends normalize differently, the
//! WORK differed, because the describing code did not.

pub mod cloud;
pub mod container;
pub mod local;
pub mod ssh;

use std::path::PathBuf;

use crate::contract::{
    BackendCapabilities, ExecutionTask, HibernationObservation, INPUT_FILE_NAME, ResourceKind,
};
use crate::error::{ExecError, Result};
use crate::policy::EffectivePolicy;
use crate::receipt::{
    ArtifactEvidence, BackendIdentity, EventKind, ExecutionReceipt, OutputChannel,
    PROTOCOL_VERSION, ReceiptBody, ReceiptEvent, ReceiptSigner, TaskEvidence, TerminalStatus,
    Timing, Transport, sha256,
};

/// What a transport actually produced. Every backend reduces its own world to
/// this, and nothing below this point knows which transport it came from.
pub struct RunOutcome {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
    /// Host, container id or machine id — never a credential.
    pub endpoint: String,
    pub cancelled: Option<String>,
    pub hibernation: HibernationObservation,
    pub started_unix_ms: u64,
    pub finished_unix_ms: u64,
}

/// Deny a task whose request exceeds this backend's ceiling, BEFORE acceptance.
/// Returns the first offending dimension in a fixed order so two backends
/// asked the same impossible question give the same answer.
pub fn pre_acceptance_denial(
    task: &ExecutionTask,
    capabilities: &BackendCapabilities,
) -> Option<(ResourceKind, u64, u64)> {
    let requested = task.resources;
    let limit = capabilities.limits;
    if requested.cpu_millis > limit.cpu_millis {
        return Some((
            ResourceKind::CpuMillis,
            requested.cpu_millis,
            limit.cpu_millis,
        ));
    }
    if requested.memory_bytes > limit.memory_bytes {
        return Some((
            ResourceKind::MemoryBytes,
            requested.memory_bytes,
            limit.memory_bytes,
        ));
    }
    if requested.wall_time_ms > limit.wall_time_ms {
        return Some((
            ResourceKind::WallTimeMs,
            requested.wall_time_ms,
            limit.wall_time_ms,
        ));
    }
    if requested.output_bytes > limit.output_bytes {
        return Some((
            ResourceKind::OutputBytes,
            requested.output_bytes,
            limit.output_bytes,
        ));
    }
    None
}

/// The pre-acceptance denial receipt: ONE event, no `task_accepted`.
pub fn denial_receipt(
    task: &ExecutionTask,
    capabilities: &BackendCapabilities,
    identity: &BackendIdentity,
    signer: &ReceiptSigner,
    policy: &EffectivePolicy,
    denial: (ResourceKind, u64, u64),
) -> Result<ExecutionReceipt> {
    let (resource, requested, limit) = denial;
    let events = vec![ReceiptEvent {
        sequence: 1,
        event: EventKind::ResourceDenied {
            resource,
            requested,
            limit,
        },
    }];
    let body = finish_body(
        task,
        capabilities,
        identity,
        policy,
        events,
        None,
        TerminalStatus::ResourceDenied {
            resource,
            requested,
            limit,
        },
        Transport {
            kind: capabilities.kind,
            endpoint: "<denied-before-acceptance>".into(),
        },
        Timing {
            started_unix_ms: 0,
            finished_unix_ms: 0,
            wall_ms: 0,
        },
        HibernationObservation::NotApplicable,
    )?;
    signer.seal(body)
}

/// Turn a real run into an attested receipt.
///
/// The shared output budget is charged HERE, once, so streamed text and
/// artifact bytes cannot be counted against two different ceilings by two
/// different backends.
pub fn outcome_receipt(
    task: &ExecutionTask,
    capabilities: &BackendCapabilities,
    identity: &BackendIdentity,
    signer: &ReceiptSigner,
    policy: &EffectivePolicy,
    outcome: RunOutcome,
) -> Result<ExecutionReceipt> {
    let transport = Transport {
        kind: capabilities.kind,
        endpoint: outcome.endpoint.clone(),
    };
    let timing = Timing {
        started_unix_ms: outcome.started_unix_ms,
        finished_unix_ms: outcome.finished_unix_ms,
        wall_ms: outcome
            .finished_unix_ms
            .saturating_sub(outcome.started_unix_ms),
    };

    let mut events = vec![ReceiptEvent {
        sequence: 1,
        event: EventKind::TaskAccepted {
            task_id: task.task_id.clone(),
            backend_id: identity.backend_id.clone(),
            workspace_sha256: task.workspace_sha256(),
            input_sha256: task.input_sha256(),
        },
    }];

    let mut sequence = 2u64;
    if !outcome.stdout.is_empty() {
        events.push(ReceiptEvent {
            sequence,
            event: EventKind::Output {
                channel: OutputChannel::Stdout,
                text_sha256: sha256(&outcome.stdout),
                bytes: outcome.stdout.len() as u64,
            },
        });
        sequence += 1;
    }
    if !outcome.stderr.is_empty() {
        events.push(ReceiptEvent {
            sequence,
            event: EventKind::Output {
                channel: OutputChannel::Stderr,
                text_sha256: sha256(&outcome.stderr),
                bytes: outcome.stderr.len() as u64,
            },
        });
        sequence += 1;
    }

    // Cancellation wins over an exit code: a child that was killed may still
    // report a plausible-looking status.
    if let Some(reason) = outcome.cancelled {
        events.push(ReceiptEvent {
            sequence,
            event: EventKind::Cancelled {
                reason: reason.clone(),
            },
        });
        let body = finish_body(
            task,
            capabilities,
            identity,
            policy,
            events,
            None,
            TerminalStatus::Cancelled { reason },
            transport,
            timing,
            outcome.hibernation,
        )?;
        return signer.seal(body);
    }

    // ONE shared output budget across streamed text and artifact bytes.
    let artifact_bytes = outcome.stdout.len() as u64;
    let produced = (outcome.stdout.len() + outcome.stderr.len()) as u64 + artifact_bytes;
    if produced > task.resources.output_bytes {
        events.push(ReceiptEvent {
            sequence,
            event: EventKind::ResourceDenied {
                resource: ResourceKind::OutputBytes,
                requested: produced,
                limit: task.resources.output_bytes,
            },
        });
        let body = finish_body(
            task,
            capabilities,
            identity,
            policy,
            events,
            None,
            TerminalStatus::ResourceDenied {
                resource: ResourceKind::OutputBytes,
                requested: produced,
                limit: task.resources.output_bytes,
            },
            transport,
            timing,
            outcome.hibernation,
        )?;
        return signer.seal(body);
    }

    if outcome.exit_code != 0 {
        let code = format!("exit-{}", outcome.exit_code);
        events.push(ReceiptEvent {
            sequence,
            event: EventKind::Failed { code: code.clone() },
        });
        let body = finish_body(
            task,
            capabilities,
            identity,
            policy,
            events,
            None,
            TerminalStatus::Failure { code },
            transport,
            timing,
            outcome.hibernation,
        )?;
        return signer.seal(body);
    }

    // The artifact IS the captured stdout, content-addressed. Nothing about
    // the host, the clock or the transport enters it, which is what lets four
    // transports produce the same digest.
    let artifact_sha = sha256(&outcome.stdout);
    let artifact = ArtifactEvidence {
        name: task.artifact_name.clone(),
        sha256: artifact_sha.clone(),
        bytes: artifact_bytes,
    };
    events.push(ReceiptEvent {
        sequence,
        event: EventKind::ArtifactPublished {
            name: artifact.name.clone(),
            sha256: artifact.sha256.clone(),
            bytes: artifact.bytes,
        },
    });
    events.push(ReceiptEvent {
        sequence: sequence + 1,
        event: EventKind::Succeeded {
            artifact_sha256: artifact_sha,
        },
    });

    let body = finish_body(
        task,
        capabilities,
        identity,
        policy,
        events,
        Some(artifact),
        TerminalStatus::Success,
        transport,
        timing,
        outcome.hibernation,
    )?;
    signer.seal(body)
}

#[allow(clippy::too_many_arguments)] // every argument is a distinct named
// receipt surface; collapsing them into a struct would hide which ones a
// backend is allowed to influence.
fn finish_body(
    task: &ExecutionTask,
    capabilities: &BackendCapabilities,
    identity: &BackendIdentity,
    policy: &EffectivePolicy,
    events: Vec<ReceiptEvent>,
    artifact: Option<ArtifactEvidence>,
    terminal: TerminalStatus,
    transport: Transport,
    timing: Timing,
    hibernation: HibernationObservation,
) -> Result<ReceiptBody> {
    let events_sha256 = sha256(&serde_json::to_vec(&events)?);
    Ok(ReceiptBody {
        protocol_version: PROTOCOL_VERSION,
        backend: identity.clone(),
        // F25-03: filled in when this Core is acting as a paired node. A
        // controller running work locally has no node identity to attest, and
        // `None` says exactly that rather than inventing a placeholder.
        node: local_node_attribution(),
        transport,
        task: TaskEvidence {
            task_id: task.task_id.clone(),
            workspace_sha256: task.workspace_sha256(),
            input_sha256: task.input_sha256(),
            resources: task.resources,
        },
        limits: capabilities.limits,
        events_sha256,
        events,
        artifact,
        terminal,
        timing,
        hibernation,
        secrets_exposed: policy.secrets_exposed.clone(),
        egress_decision: policy.egress_decision.clone(),
    })
}

/// Materialise the task's workspace and input into `root`.
///
/// Every path was validated as non-escaping before it got here; this re-checks
/// rather than trusting, because the same function runs on the far end of a
/// transport where the caller is not necessarily us.
pub fn materialize_workspace(root: &std::path::Path, task: &ExecutionTask) -> Result<()> {
    std::fs::create_dir_all(root)?;
    for file in &task.workspace {
        crate::contract::validate_relative_name("workspace path", &file.path)?;
        let target = root.join(&file.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &file.bytes)?;
    }
    std::fs::write(root.join(INPUT_FILE_NAME), &task.input)?;
    Ok(())
}

/// F25-03: this host's node attribution, when it has been given a node
/// identity to run under.
///
/// Set by `WAYLAND_NODE_ID` — the name an operator paired this machine under.
/// Absent means "not running as a paired node", which is a legitimate state
/// and is recorded as `None` rather than as a placeholder string, because a
/// placeholder would later be indistinguishable from a real attribution.
pub fn local_node_attribution() -> Option<crate::node::attribution::NodeAttribution> {
    let node_id = std::env::var("WAYLAND_NODE_ID")
        .ok()
        .filter(|v| !v.is_empty())?;
    let seed = crate::node::pairing::load_or_create_node_seed().ok()?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let identity = crate::node::pairing::NodeIdentity::local(&node_id, &signing_key).ok()?;
    Some(crate::node::attribution::NodeAttribution::from_identity(
        &identity,
    ))
}

/// Per-backend Ed25519 seed, persisted so one backend keeps one identity
/// across processes. Generated on first use with the OS RNG.
pub fn load_or_create_seed(backend_id: &str) -> Result<[u8; 32]> {
    crate::contract::validate_identifier("backend_id", backend_id)?;
    let dir: PathBuf = crate::registry::state_dir().join("keys");
    let path = dir.join(format!("{backend_id}.key"));
    load_or_create_seed_at(&path, "backend signing seed")
}

/// Read a persisted 32-byte seed, or generate and publish one ATOMICALLY.
///
/// The write is write-then-rename, for the same reason `registry::record`
/// already is ("a cancel racing a run must never read a half file"): a bare
/// `fs::write` truncates and then fills, so a concurrent reader inside that
/// window sees a short file. This is not theoretical for a seed — the length
/// check below turns a short read into a HARD error rather than a retry, so a
/// torn write does not merely race, it REFUSES:
///
/// ```text
/// backend signing seed at <path> is not 32 bytes
/// ```
///
/// and after a crash or power loss mid-write it refuses FOREVER, because a
/// truncated file is never regenerated. Two ordinary wayland-core processes
/// sharing one machine reach this path concurrently by design — the product
/// supports independent CLI processes over one state dir — so the window is
/// reachable in production, not only under a test harness.
///
/// The mode is set on the temporary file BEFORE the rename, so the key is
/// never momentarily world-readable. The previous order — create, write,
/// then chmod — published a private key at the umask default first.
///
/// The temporary name is unique per CALL (pid plus a process-local counter),
/// so no two writers can collide on the staging file either; `rename(2)` over
/// an existing path is atomic, so the loser of the race simply publishes an
/// identical-length file and every reader sees one whole seed or the other,
/// never a fragment.
pub(crate) fn load_or_create_seed_at(path: &std::path::Path, what: &str) -> Result<[u8; 32]> {
    let dir = path.parent().ok_or_else(|| {
        ExecError::Receipt(format!("{what} path {} has no directory", path.display()))
    })?;
    std::fs::create_dir_all(dir)?;
    if let Ok(bytes) = std::fs::read(path) {
        if bytes.len() == 32 {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            return Ok(seed);
        }
        // Kept a hard error rather than silently regenerating: rotating an
        // identity behind the operator's back is worse than refusing. Now
        // that we can no longer PRODUCE a short file, this means the file was
        // corrupted by something else, so the message says how to recover.
        return Err(ExecError::Receipt(format!(
            "{what} at {} is not 32 bytes; it is corrupt. Delete it to have a \
             new identity generated.",
            path.display()
        )));
    }
    let mut seed = [0u8; 32];
    {
        use rand::RngCore as _;
        rand::rngs::OsRng.fill_bytes(&mut seed);
    }
    // The staging name must be unique per CALL, not per process: two threads
    // in one process sharing a staging path would tear it exactly as they
    // would have torn the target, which is the failure this helper exists to
    // remove.
    static STAGING_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let ticket = STAGING_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let staging = path.with_extension(format!("key.tmp.{}.{ticket}", std::process::id()));
    std::fs::write(&staging, seed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600));
    }
    if let Err(error) = std::fs::rename(&staging, path) {
        let _ = std::fs::remove_file(&staging);
        return Err(error.into());
    }
    // Read back rather than returning the seed we generated. Two processes
    // reaching first-use together each generate a seed and each rename; the
    // last rename wins the FILE, so a caller that returned its own seed would
    // be using an identity the disk does not have and would silently change
    // identity on its next run. Reading back makes every racer converge on the
    // one seed that was actually persisted.
    let published = std::fs::read(path)?;
    if published.len() != 32 {
        return Err(ExecError::Receipt(format!(
            "{what} at {} is not 32 bytes; it is corrupt. Delete it to have a \
             new identity generated.",
            path.display()
        )));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&published);
    Ok(seed)
}

/// The atomic-publish contract of [`load_or_create_seed_at`].
///
/// These target the helper directly rather than going through
/// [`load_or_create_seed`], which resolves its path from the process-global
/// state dir: a test that raced on THAT path would write into whatever state
/// dir the process happens to have, and its behaviour would depend on which
/// other tests ran alongside it. Pointing the helper at a `TempDir` makes the
/// concurrency the only variable.
#[cfg(test)]
mod seed_publish_tests {
    use super::load_or_create_seed_at;

    #[test]
    fn concurrent_first_use_never_observes_a_partial_seed() {
        // The wayland#1250 signature. With a bare `fs::write` the target is
        // truncated and then filled, so a reader inside that window gets a
        // short file and the length check turns it into a HARD refusal:
        // `... is not 32 bytes`. Sixteen threads against one fresh path make
        // that window overlap; the atomic publish removes it.
        //
        // Repeated because a single round can miss a race by luck, and a test
        // that samples a window once is how this class survives.
        for round in 0..24 {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("container.key");
            let seeds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
            std::thread::scope(|scope| {
                for _ in 0..16 {
                    let path = path.clone();
                    let seeds = std::sync::Arc::clone(&seeds);
                    let barrier = std::sync::Arc::clone(&barrier);
                    scope.spawn(move || {
                        // Release them together, or they queue and never race.
                        barrier.wait();
                        let seed = load_or_create_seed_at(&path, "backend signing seed")
                            .unwrap_or_else(|error| {
                                panic!("round {round}: concurrent first use refused: {error}")
                            });
                        seeds.lock().expect("seeds lock").push(seed);
                    });
                }
            });
            let seeds = seeds.lock().expect("seeds lock");
            assert_eq!(seeds.len(), 16, "round {round}: every caller must return");
            // Every racer must end up on the seed that was actually PERSISTED,
            // or a process is running an identity the disk does not have and
            // will change identity on its next start.
            let on_disk = std::fs::read(&path).expect("published seed");
            assert_eq!(
                on_disk.len(),
                32,
                "round {round}: published seed is 32 bytes"
            );
            for (i, seed) in seeds.iter().enumerate() {
                assert_eq!(
                    seed.as_slice(),
                    on_disk.as_slice(),
                    "round {round}: caller {i} returned a seed that is not the persisted one"
                );
            }
        }
    }

    #[test]
    fn no_staging_file_survives_a_successful_publish() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("local.key");
        load_or_create_seed_at(&path, "backend signing seed").expect("first use");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .filter(|name| name.to_string_lossy().contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "a completed publish must leave no staging file: {leftovers:?}"
        );
    }

    #[test]
    fn a_corrupt_seed_is_refused_with_a_recovery_instruction() {
        // Kept a hard refusal on purpose: silently regenerating would rotate
        // an identity behind the operator's back. The message must therefore
        // say how to recover, because nothing else will.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ssh.key");
        std::fs::write(&path, [7u8; 31]).expect("seed a short file");
        let error = load_or_create_seed_at(&path, "backend signing seed")
            .expect_err("a 31-byte seed must be refused");
        let message = error.to_string();
        assert!(message.contains("is not 32 bytes"), "{message}");
        assert!(
            message.contains("Delete it"),
            "the refusal must be actionable: {message}"
        );
        assert_eq!(
            std::fs::read(&path).expect("still there").len(),
            31,
            "refusing must not destroy the file the operator may want to inspect"
        );
    }

    #[test]
    fn an_existing_seed_is_returned_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("cloud.key");
        let existing = [9u8; 32];
        std::fs::write(&path, existing).expect("seed");
        let seed = load_or_create_seed_at(&path, "backend signing seed").expect("load");
        assert_eq!(seed, existing, "an existing identity must never be rotated");
    }

    #[cfg(unix)]
    #[test]
    fn the_seed_is_never_published_world_readable() {
        // The mode is set on the staging file BEFORE the rename. The previous
        // order -- create, write, then chmod -- published a private key at the
        // umask default first, so a reader in that window could take it.
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("local.key");
        load_or_create_seed_at(&path, "backend signing seed").expect("first use");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "a signing seed must be owner-only, got {mode:o}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{BackendKind, ResourceBudget, SecretChannel, WorkspaceFile};
    use crate::policy::EgressDecisionSource;

    fn capabilities(limits: ResourceBudget) -> BackendCapabilities {
        BackendCapabilities {
            backend_id: "local".into(),
            kind: BackendKind::Local,
            version: "0.12.25".into(),
            limits,
            supports_artifact_transfer: true,
            supports_cancellation: true,
            supports_hibernation: false,
            secret_channel: SecretChannel::None,
        }
    }

    fn task(resources: ResourceBudget) -> ExecutionTask {
        ExecutionTask {
            task_id: "t-1".into(),
            nonce: "n-1".into(),
            workspace: vec![WorkspaceFile {
                path: "a.txt".into(),
                bytes: b"alpha".to_vec(),
            }],
            input: b"deterministic".to_vec(),
            argv: vec!["cat".into(), INPUT_FILE_NAME.into()],
            artifact_name: "stdout.bin".into(),
            resources,
        }
    }

    fn policy() -> EffectivePolicy {
        EffectivePolicy {
            backend_id: "local".into(),
            kind: BackendKind::Local,
            egress_decision: "allow-all-default-no-policy-installed".into(),
            egress_source: EgressDecisionSource::NoEgressSurface,
            secret_channel: SecretChannel::None,
            secrets_exposed: vec![],
            containment: "test".into(),
        }
    }

    fn identity(signer: &ReceiptSigner) -> BackendIdentity {
        BackendIdentity {
            backend_id: "local".into(),
            instance_id: "inst-1".into(),
            version: "0.12.25".into(),
            key_id: signer.key_id().to_string(),
        }
    }

    #[test]
    fn an_impossible_request_is_denied_before_acceptance_and_names_the_resource() {
        let caps = capabilities(ResourceBudget::new(1000, 1 << 20, 5000, 1 << 16).unwrap());
        let task = task(ResourceBudget::new(1000, 1 << 40, 5000, 1 << 16).unwrap());
        let denial = pre_acceptance_denial(&task, &caps).expect("must deny");
        assert_eq!(denial.0, ResourceKind::MemoryBytes);

        let signer = ReceiptSigner::from_seed([3u8; 32]);
        let identity = identity(&signer);
        let receipt =
            denial_receipt(&task, &caps, &identity, &signer, &policy(), denial).expect("receipt");
        assert_eq!(receipt.body.events.len(), 1);
        assert!(matches!(
            receipt.body.events[0].event,
            EventKind::ResourceDenied { .. }
        ));
        assert!(
            !receipt
                .body
                .events
                .iter()
                .any(|e| matches!(e.event, EventKind::TaskAccepted { .. })),
            "a pre-acceptance denial must not claim the task was accepted"
        );
        receipt
            .verify(&identity, &signer.verifying_key())
            .expect("denial receipt still attests");
    }

    #[test]
    fn streamed_text_and_artifact_bytes_share_one_output_budget() {
        // stdout is 10 bytes; the artifact IS stdout, so the shared charge is
        // 20. A 15-byte ceiling must therefore deny even though neither half
        // exceeds it on its own.
        let caps = capabilities(ResourceBudget::new(1000, 1 << 20, 5000, 1 << 20).unwrap());
        let task = task(ResourceBudget::new(1000, 1 << 20, 5000, 15).unwrap());
        let signer = ReceiptSigner::from_seed([4u8; 32]);
        let identity = identity(&signer);
        let outcome = RunOutcome {
            stdout: b"0123456789".to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
            endpoint: "localhost".into(),
            cancelled: None,
            hibernation: HibernationObservation::NotApplicable,
            started_unix_ms: 1,
            finished_unix_ms: 2,
        };
        let receipt =
            outcome_receipt(&task, &caps, &identity, &signer, &policy(), outcome).expect("receipt");
        match &receipt.body.terminal {
            TerminalStatus::ResourceDenied {
                resource,
                requested,
                limit,
            } => {
                assert_eq!(*resource, ResourceKind::OutputBytes);
                assert_eq!(*requested, 20);
                assert_eq!(*limit, 15);
            }
            other => panic!("expected a shared-budget denial, got {other:?}"),
        }
        // This denial is AFTER acceptance, so the accepted event is present.
        assert!(matches!(
            receipt.body.events[0].event,
            EventKind::TaskAccepted { .. }
        ));
    }

    #[test]
    fn every_terminal_outcome_produces_exactly_one_terminal_event() {
        let caps = capabilities(ResourceBudget::new(1000, 1 << 20, 5000, 1 << 20).unwrap());
        let task = task(ResourceBudget::new(1000, 1 << 20, 5000, 1 << 20).unwrap());
        let signer = ReceiptSigner::from_seed([5u8; 32]);
        let identity = identity(&signer);

        let cases: Vec<RunOutcome> = vec![
            RunOutcome {
                stdout: b"ok".to_vec(),
                stderr: Vec::new(),
                exit_code: 0,
                endpoint: "e".into(),
                cancelled: None,
                hibernation: HibernationObservation::NotApplicable,
                started_unix_ms: 1,
                finished_unix_ms: 2,
            },
            RunOutcome {
                stdout: Vec::new(),
                stderr: b"boom".to_vec(),
                exit_code: 3,
                endpoint: "e".into(),
                cancelled: None,
                hibernation: HibernationObservation::NotApplicable,
                started_unix_ms: 1,
                finished_unix_ms: 2,
            },
            RunOutcome {
                stdout: b"partial".to_vec(),
                stderr: Vec::new(),
                exit_code: 0,
                endpoint: "e".into(),
                cancelled: Some("operator cancelled".into()),
                hibernation: HibernationObservation::NotApplicable,
                started_unix_ms: 1,
                finished_unix_ms: 2,
            },
        ];
        for outcome in cases {
            let receipt = outcome_receipt(&task, &caps, &identity, &signer, &policy(), outcome)
                .expect("receipt");
            let terminals = receipt
                .body
                .events
                .iter()
                .filter(|e| e.event.is_terminal())
                .count();
            assert_eq!(terminals, 1, "exactly one terminal event per outcome");
            receipt
                .verify(&identity, &signer.verifying_key())
                .expect("attested");
        }
    }
}
