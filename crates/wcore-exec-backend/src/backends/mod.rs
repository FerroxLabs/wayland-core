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

/// Per-backend Ed25519 seed, persisted so one backend keeps one identity
/// across processes. Generated on first use with the OS RNG.
pub fn load_or_create_seed(backend_id: &str) -> Result<[u8; 32]> {
    crate::contract::validate_identifier("backend_id", backend_id)?;
    let dir: PathBuf = crate::registry::state_dir().join("keys");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{backend_id}.key"));
    if let Ok(bytes) = std::fs::read(&path) {
        if bytes.len() == 32 {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&bytes);
            return Ok(seed);
        }
        return Err(ExecError::Receipt(format!(
            "backend signing seed at {} is not 32 bytes",
            path.display()
        )));
    }
    let mut seed = [0u8; 32];
    {
        use rand::RngCore as _;
        rand::rngs::OsRng.fill_bytes(&mut seed);
    }
    std::fs::write(&path, seed)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(seed)
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
