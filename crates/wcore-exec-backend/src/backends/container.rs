//! The CONTAINER reference backend.
//!
//! Availability is answered from a REAL DAEMON PING, never from socket
//! presence. `wcore_sandbox::backends::docker::DockerBackend::is_available()`
//! documents that it only answers `docker_socket_present()` — DOCKER_HOST set,
//! or the unix socket / named pipe existing — and points security-sensitive
//! callers at `connect()`, which does a real daemon round trip. A stale socket
//! left behind by a stopped daemon would otherwise let this backend advertise
//! readiness it does not have, and shipping that is shipping a capability lie.
//!
//! Every container this backend creates carries `wayland.task.nonce` as a
//! label, so plan 25-04's orphan scan is one `docker ps --filter label=` away
//! from an answer instead of a guess.

use async_trait::async_trait;

use crate::contract::{
    Availability, BackendCapabilities, BackendKind, CleanupObservation, ExecutionBackend,
    ExecutionTask, Health, HibernationObservation, OrphanScan, ProbeBasis, ResourceBudget,
    SecretChannel, validate_identifier,
};
use crate::error::{ExecError, Result};
use crate::policy::{EffectivePolicy, declared_secret_exposure};
use crate::receipt::{BackendIdentity, ExecutionReceipt, ReceiptSigner};
use crate::registry::{self, LiveTask, now_unix_ms};

use super::local::{cancel_marker_taken, instance_id, write_cancel_marker};
use super::{
    RunOutcome, denial_receipt, load_or_create_seed, materialize_workspace, outcome_receipt,
    pre_acceptance_denial,
};

pub const BACKEND_ID: &str = "container";
pub const NONCE_LABEL: &str = "wayland.task.nonce";
const DEFAULT_IMAGE: &str = "docker.io/library/busybox:1.36";

pub struct ContainerBackend {
    capabilities: BackendCapabilities,
    identity: BackendIdentity,
    signer: ReceiptSigner,
    image: String,
}

impl ContainerBackend {
    pub fn new(limits: ResourceBudget) -> Result<Self> {
        let seed = load_or_create_seed(BACKEND_ID)?;
        let signer = ReceiptSigner::from_seed(seed);
        let image =
            std::env::var("WAYLAND_EXEC_CONTAINER_IMAGE").unwrap_or_else(|_| DEFAULT_IMAGE.into());
        Ok(Self {
            capabilities: BackendCapabilities {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Container,
                version: env!("CARGO_PKG_VERSION").into(),
                limits,
                supports_artifact_transfer: true,
                supports_cancellation: true,
                supports_hibernation: false,
                secret_channel: SecretChannel::ContainerEnv,
            },
            identity: BackendIdentity {
                backend_id: BACKEND_ID.into(),
                instance_id: instance_id(),
                version: env!("CARGO_PKG_VERSION").into(),
                key_id: signer.key_id().to_string(),
            },
            signer,
            image,
        })
    }

    pub fn identity(&self) -> &BackendIdentity {
        &self.identity
    }

    pub fn verifying_key(&self) -> ed25519_dalek::VerifyingKey {
        self.signer.verifying_key()
    }

    fn container_name(task_id: &str) -> String {
        format!("wayland-f25-{task_id}")
    }
}

/// A real daemon round trip, with a bound so an unreachable daemon cannot hang
/// `backend list`.
async fn daemon_ping() -> std::result::Result<String, String> {
    let mut command = wcore_config::shell::shell_command_argv(
        "docker",
        &["version", "--format", "{{.Server.Version}}"],
    );
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let fut = command.output();
    match tokio::time::timeout(std::time::Duration::from_secs(5), fut).await {
        Err(_) => Err("docker daemon did not answer a version ping within 5s".into()),
        Ok(Err(e)) => Err(format!("docker client could not be launched: {e}")),
        Ok(Ok(output)) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
        Ok(Ok(output)) => Err(format!(
            "docker daemon refused the version ping: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

#[async_trait]
impl ExecutionBackend for ContainerBackend {
    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn availability(&self) -> Availability {
        match daemon_ping().await {
            Ok(version) => Availability::up(
                ProbeBasis::DaemonPing,
                format!("container daemon answered a version ping: server {version}"),
            ),
            Err(detail) => Availability::down(ProbeBasis::DaemonPing, detail),
        }
    }

    fn effective_policy(&self, task: &ExecutionTask) -> Result<EffectivePolicy> {
        let (egress_decision, egress_source) = crate::policy::observed_egress_decision();
        let policy = EffectivePolicy {
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Container,
            egress_decision,
            egress_source,
            secret_channel: SecretChannel::ContainerEnv,
            secrets_exposed: declared_secret_exposure(BackendKind::Container, task),
            containment: format!(
                "container image {} with --network none and a per-task nonce label",
                self.image
            ),
        };
        policy.validate()?;
        Ok(policy)
    }

    async fn execute(&self, task: &ExecutionTask) -> Result<ExecutionReceipt> {
        task.validate()?;
        let policy = self.effective_policy(task)?;
        if let Some(denial) = pre_acceptance_denial(task, &self.capabilities) {
            return denial_receipt(
                task,
                &self.capabilities,
                &self.identity,
                &self.signer,
                &policy,
                denial,
            );
        }

        let availability = self.availability().await;
        if !availability.available {
            return Err(ExecError::Unavailable {
                backend_id: BACKEND_ID.into(),
                detail: availability.detail,
            });
        }

        let workdir = registry::state_dir().join("work").join(&task.task_id);
        materialize_workspace(&workdir, task)?;
        let name = Self::container_name(&task.task_id);
        validate_identifier("container_name", &name)?;
        let started = now_unix_ms();

        registry::record(&LiveTask {
            task_id: task.task_id.clone(),
            nonce: task.nonce.clone(),
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Container,
            pid: None,
            handle: Some(name.clone()),
            started_unix_ms: started,
        })?;

        let mount = format!("{}:/task", workdir.display());
        let label = format!("{NONCE_LABEL}={}", task.nonce);
        let memory = format!("{}b", task.resources.memory_bytes);
        // ARGV MODE throughout. Nothing here is interpolated into a shell
        // string, so a workspace path carrying a metacharacter cannot reach an
        // interpreter on either side of the container boundary.
        let mut args: Vec<String> = vec![
            "run".into(),
            "--rm".into(),
            "--name".into(),
            name.clone(),
            "--label".into(),
            label,
            "--network".into(),
            "none".into(),
            "--memory".into(),
            memory,
            "--workdir".into(),
            "/task".into(),
            "--volume".into(),
            mount,
            "--env".into(),
            format!("WAYLAND_TASK_NONCE={}", task.nonce),
            self.image.clone(),
        ];
        args.extend(task.argv.iter().cloned());
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();

        let mut command = wcore_config::shell::shell_command_argv("docker", &borrowed);
        // Null stdin, never inherited — see the note in backends/local.rs.
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let wall = std::time::Duration::from_millis(task.resources.wall_time_ms);
        let output = match tokio::time::timeout(wall, command.output()).await {
            Ok(result) => result.map_err(|e| ExecError::Exec(e.to_string()))?,
            Err(_) => {
                let _ = self.cancel(&task.task_id).await;
                return Err(ExecError::Exec(format!(
                    "container task {} exceeded its {}ms wall clock",
                    task.task_id, task.resources.wall_time_ms
                )));
            }
        };

        let finished = now_unix_ms();
        let cancelled = cancel_marker_taken(&task.task_id);
        registry::forget(&task.task_id)?;
        let _ = std::fs::remove_dir_all(&workdir);

        outcome_receipt(
            task,
            &self.capabilities,
            &self.identity,
            &self.signer,
            &policy,
            RunOutcome {
                stdout: output.stdout,
                stderr: output.stderr,
                exit_code: output.status.code().unwrap_or(-1),
                endpoint: name,
                cancelled,
                hibernation: HibernationObservation::NotApplicable,
                started_unix_ms: started,
                finished_unix_ms: finished,
            },
        )
    }

    async fn cancel(&self, task_id: &str) -> Result<CleanupObservation> {
        let entry = registry::load(task_id)?;
        write_cancel_marker(task_id, "operator cancelled")?;
        let name = entry
            .handle
            .clone()
            .unwrap_or_else(|| Self::container_name(task_id));
        let mut command = wcore_config::shell::shell_command_argv("docker", &["rm", "-f", &name]);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let _ = command.output().await;

        // The observation is what the daemon says AFTER the removal, not what
        // the removal returned.
        let residual = match list_containers_with_nonce(&entry.nonce).await {
            Ok(found) => found,
            Err(detail) => vec![format!("could not re-enumerate containers: {detail}")],
        };
        registry::forget(task_id)?;
        Ok(CleanupObservation {
            task_id: task_id.into(),
            backend_id: BACKEND_ID.into(),
            method: format!(
                "docker rm -f, then docker ps -a --filter label={NONCE_LABEL}=<nonce> re-read"
            ),
            residual,
        })
    }

    async fn health(&self) -> Result<Health> {
        let availability = self.availability().await;
        let live = registry::list()
            .into_iter()
            .filter(|t| t.backend_id == BACKEND_ID)
            .count();
        Ok(Health {
            healthy: availability.available,
            detail: availability.detail,
            live_tasks: live,
        })
    }

    async fn scan_orphans(&self, nonce: &str) -> Result<OrphanScan> {
        match list_containers_with_nonce(nonce).await {
            Ok(found) => Ok(OrphanScan {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Container,
                nonce: nonce.into(),
                method: format!("docker ps -a --filter label={NONCE_LABEL}=<nonce>"),
                found,
                enumerated: true,
            }),
            Err(detail) => Ok(OrphanScan {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Container,
                nonce: nonce.into(),
                // An unscannable surface reports enumerated=false. Reporting
                // zero orphans because the scan failed is how an orphan hides.
                method: format!("docker ps failed: {detail}"),
                found: Vec::new(),
                enumerated: false,
            }),
        }
    }
}

async fn list_containers_with_nonce(nonce: &str) -> std::result::Result<Vec<String>, String> {
    let filter = format!("label={NONCE_LABEL}={nonce}");
    let mut command = wcore_config::shell::shell_command_argv(
        "docker",
        &["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"],
    );
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| "docker ps did not answer within 10s".to_string())?
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}
