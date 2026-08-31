//! The LOCAL reference backend.
//!
//! It runs the task on this host, in its own process group, with the workspace
//! materialised into a per-task directory that is torn down on completion.
//!
//! CAPABILITY HONESTY, stated here rather than buried: this backend CONSULTS
//! `wcore_sandbox::default_for_platform()` and refuses to run when the platform
//! has no real containment backend and no explicit opt-out — but it does NOT
//! currently route the child through `SandboxBackend::execute`. The reason is
//! concrete: that trait's buffered and streaming entry points both own the
//! child internally and return no handle, so a `wayland-core backend cancel`
//! issued from a DIFFERENT process has nothing to signal. Cancellation that
//! only works in-process is not cancellation, and plan 25-04 looks at the real
//! process table. So the backend owns its own process group, and the
//! containment mechanism it did NOT use is reported by name in the effective
//! policy instead of being quietly implied. Closing that gap needs a
//! pid-or-handle surface on `SandboxBackend`, which is a wcore-sandbox change
//! and is recorded as a finding rather than done here.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::contract::{
    Availability, BackendCapabilities, BackendKind, CleanupObservation, ExecutionBackend,
    ExecutionTask, Health, HibernationObservation, OrphanScan, ProbeBasis, ResourceBudget,
    SecretChannel, UnscopedOrphanScan,
};
use crate::error::{ExecError, Result};
use crate::policy::{EffectivePolicy, declared_secret_exposure};
use crate::receipt::{BackendIdentity, ExecutionReceipt, ReceiptSigner};
use crate::registry::{self, LiveTask, now_unix_ms};

use super::{
    RunOutcome, denial_receipt, load_or_create_seed, materialize_workspace, outcome_receipt,
    pre_acceptance_denial,
};

pub const BACKEND_ID: &str = "local";

pub struct LocalBackend {
    capabilities: BackendCapabilities,
    identity: BackendIdentity,
    signer: ReceiptSigner,
    /// The containment mechanism the platform sandbox WOULD select, recorded
    /// by name so the receipt never implies containment it did not apply.
    containment: String,
    root: PathBuf,
}

impl LocalBackend {
    pub fn new(limits: ResourceBudget) -> Result<Self> {
        let seed = load_or_create_seed(BACKEND_ID)?;
        let signer = ReceiptSigner::from_seed(seed);
        let containment = wcore_sandbox::default_for_platform().name().to_string();
        Ok(Self {
            capabilities: BackendCapabilities {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Local,
                version: env!("CARGO_PKG_VERSION").into(),
                limits,
                supports_artifact_transfer: true,
                supports_cancellation: true,
                supports_hibernation: false,
                secret_channel: SecretChannel::None,
            },
            identity: BackendIdentity {
                backend_id: BACKEND_ID.into(),
                instance_id: instance_id(),
                version: env!("CARGO_PKG_VERSION").into(),
                key_id: signer.key_id().to_string(),
            },
            signer,
            containment,
            root: registry::state_dir().join("work"),
        })
    }

    pub fn identity(&self) -> &BackendIdentity {
        &self.identity
    }

    pub fn verifying_key(&self) -> ed25519_dalek::VerifyingKey {
        self.signer.verifying_key()
    }
}

/// A stable-per-host instance id. Deliberately NOT a hostname: a hostname is
/// operator-identifying and would land in every receipt.
pub(crate) fn instance_id() -> String {
    let dir = registry::state_dir();
    let path = dir.join("instance-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    let generated = uuid::Uuid::new_v4().simple().to_string();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&path, &generated);
    generated
}

#[async_trait]
impl ExecutionBackend for LocalBackend {
    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn availability(&self) -> Availability {
        // A real probe: the platform sandbox registry actually selects a
        // backend, and a fail-closed selection means this host cannot run
        // contained work.
        let backend = wcore_sandbox::default_for_platform();
        let name = backend.name();
        if backend.is_available() {
            Availability::up(
                ProbeBasis::SandboxBackendProbe,
                format!("platform containment backend '{name}' probed available"),
            )
        } else {
            Availability::down(
                ProbeBasis::SandboxBackendProbe,
                format!("platform containment backend '{name}' is not available on this host"),
            )
        }
    }

    fn effective_policy(&self, task: &ExecutionTask) -> Result<EffectivePolicy> {
        let (egress_decision, egress_source) = crate::policy::observed_egress_decision();
        let policy = EffectivePolicy {
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Local,
            egress_decision,
            egress_source,
            secret_channel: SecretChannel::None,
            secrets_exposed: declared_secret_exposure(BackendKind::Local, task),
            containment: format!(
                "process-group-owned; platform containment backend '{}' selected but NOT applied \
                 to this child (see module docs)",
                self.containment
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

        let workdir = self.root.join(&task.task_id);
        materialize_workspace(&workdir, task)?;
        let started = now_unix_ms();

        let program = task.argv[0].clone();
        let args: Vec<&str> = task.argv[1..].iter().map(String::as_str).collect();
        // ARGV MODE. Every element is a separate argv entry, so a workspace
        // path or an input value carrying `;` or `$(...)` reaches the program
        // as literal bytes.
        let mut command = wcore_config::shell::shell_command_argv(&program, &args);
        command.current_dir(&workdir);
        // Null stdin, NOT inherited. An inherited stdin lets the task's child
        // consume bytes from whatever is feeding the caller — measured live on
        // 2026-07-26, where a `bash -s` operator script was being read from
        // stdin and the first task swallowed the rest of it. A task's input
        // arrives as workspace bytes; it has no business reading the operator's
        // terminal.
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command.env("WAYLAND_TASK_NONCE", &task.nonce);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            // Own the whole descendant tree: a child that forks cannot outlive
            // a cancellation aimed at the group.
            command.as_std_mut().process_group(0);
        }

        let child = command.spawn().map_err(|e| {
            ExecError::Exec(format!(
                "could not spawn '{program}' for the local backend: {e}"
            ))
        })?;
        let pid = child.id();

        registry::record(&LiveTask {
            task_id: task.task_id.clone(),
            nonce: task.nonce.clone(),
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Local,
            pid,
            handle: None,
            started_unix_ms: started,
        })?;

        let wall = std::time::Duration::from_millis(task.resources.wall_time_ms);
        let output = match tokio::time::timeout(wall, child.wait_with_output()).await {
            Ok(result) => result.map_err(|e| ExecError::Exec(e.to_string()))?,
            Err(_) => {
                let _ = self.cancel(&task.task_id).await;
                return Err(ExecError::Exec(format!(
                    "task {} exceeded its {}ms wall clock",
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
                endpoint: "localhost".into(),
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
        let mut residual = Vec::new();
        if let Some(pid) = entry.pid {
            kill_process_group(pid);
            // Give the group a moment, then look again — the observation is
            // what the process table says, not what the signal returned.
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if process_alive(pid) {
                residual.push(format!("pid {pid} still alive after group termination"));
            }
        }
        registry::forget(task_id)?;
        Ok(CleanupObservation {
            task_id: task_id.into(),
            backend_id: BACKEND_ID.into(),
            method: "SIGKILL to the child's own process group, then re-read the process table"
                .into(),
            residual,
        })
    }

    async fn health(&self) -> Result<Health> {
        let live = registry::list()
            .into_iter()
            .filter(|t| t.backend_id == BACKEND_ID)
            .count();
        let availability = self.availability().await;
        Ok(Health {
            healthy: availability.available,
            detail: availability.detail,
            live_tasks: live,
        })
    }

    /// F25-05 FINDING (HIGH), fixed here.
    ///
    /// This used to consult ONLY the live-task registry. That makes the scan
    /// structurally blind to the exact thing an orphan scan exists to find: a
    /// terminal event REMOVES the registry entry, so a process that outlived
    /// its task is, by construction, no longer listed — and the scan returned
    /// zero while `ps` showed the process. Measured on hetzner-dsm: the
    /// independent enumeration found 1 row carrying the nonce and this scan
    /// reported 0.
    ///
    /// It now takes the UNION of the registry crossing and a real enumeration
    /// of the host process table, so a surviving process is found whether or
    /// not any bookkeeping still remembers it.
    async fn scan_orphans(&self, nonce: &str) -> Result<OrphanScan> {
        let mut found: Vec<String> = registry::list()
            .into_iter()
            .filter(|t| t.backend_id == BACKEND_ID && t.nonce == nonce)
            .filter(|t| t.pid.map(process_alive).unwrap_or(false))
            .map(|t| format!("registry: task {} pid {:?}", t.task_id, t.pid))
            .collect();

        // The half the registry cannot see. Failure to enumerate is reported
        // as a failure to enumerate — never as zero.
        let (rows, enumerated, detail) = match crate::orphan::local_process_rows(nonce).await {
            Ok(rows) => (rows, true, String::new()),
            Err(e) => (Vec::new(), false, e.to_string()),
        };
        found.extend(rows.into_iter().map(|row| format!("process table: {row}")));
        found.sort();
        found.dedup();

        Ok(OrphanScan {
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Local,
            nonce: nonce.into(),
            method: if enumerated {
                "live-task registry UNION a real enumeration of the host process table".into()
            } else {
                format!(
                    "live-task registry only; the process table could not be enumerated ({detail})"
                )
            },
            found,
            enumerated,
        })
    }

    /// UNSUPPORTED, and it says so rather than answering zero.
    ///
    /// This backend's surface is the host process table, and nothing in a
    /// wayland child's argv marks it as ours except the per-task NONCE itself
    /// — which is the value an unscoped scan is defined as not having. The
    /// registry crossing is no help either: it can only return tasks this
    /// process already knows about, which is the question #366 d3 says nobody
    /// needed asked. Closing this needs a product-wide argv marker, which is a
    /// change to the spawn path and not to the scanner, so it is stated here
    /// rather than faked.
    async fn scan_all_orphans(&self) -> Result<UnscopedOrphanScan> {
        Ok(UnscopedOrphanScan {
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Local,
            method: "no unscoped enumeration exists for the host process table".into(),
            found: Vec::new(),
            enumerated: false,
            unsupported_reason: Some(
                "a local child carries no product-wide marker in its argv, only its own \
                 task nonce, so the process table cannot be filtered for `any wayland run`. \
                 This is NOT a report of zero orphans."
                    .into(),
            ),
        })
    }
}

fn cancel_marker_path(task_id: &str) -> PathBuf {
    registry::state_dir()
        .join("cancels")
        .join(format!("{task_id}.cancel"))
}

pub(crate) fn write_cancel_marker(task_id: &str, reason: &str) -> Result<()> {
    crate::contract::validate_identifier("task_id", task_id)?;
    let path = cancel_marker_path(task_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, reason)?;
    Ok(())
}

/// Consume the cancellation marker, if any. Consuming rather than peeking
/// means a later unrelated task with a recycled id cannot inherit it.
pub(crate) fn cancel_marker_taken(task_id: &str) -> Option<String> {
    let path = cancel_marker_path(task_id);
    let reason = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(reason)
}

#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    // Negative pid targets the whole group, which is the point: signalling the
    // direct child alone would leave a forked descendant running.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(windows)]
pub(crate) fn kill_process_group(pid: u32) {
    // Windows has no process groups in the POSIX sense. taskkill /T walks the
    // tree, which is the nearest true equivalent.
    let pid = pid.to_string();
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid, "/T", "/F"])
        .output();
}

/// Is the backend's child process still RUNNING?
///
/// PRODUCTION path, and the reason this matters more than the test probes it
/// shares a defect with. The unix arm was `kill(pid, 0) == 0`, which a
/// **zombie** satisfies, so a child that had already exited but had not been
/// reaped read as still executing — on any host without a reaping init, which
/// includes containers started without `--init`. The Windows arm shelled out
/// to `tasklist` and substring-matched its output, which also matches the pid
/// appearing in any other column.
///
/// Both are replaced by the one zombie-aware probe in
/// `wcore_types::process_liveness`; see `.planning/ZOMBIE-PROBE.md`.
pub(crate) fn process_alive(pid: u32) -> bool {
    wcore_types::process_liveness::process_is_alive(pid)
}
