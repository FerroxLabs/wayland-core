//! Worker spawn + run logic for `Swarm::dispatch`.
//!
//! The locked surface is `dispatch(&self, brief, count) -> Vec<WorkerHandle>`.
//! Each worker is spawned in its own worktree as a subprocess of the
//! orchestrator (process boundary; no shared memory).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use wcore_sandbox::{
    NetworkPolicy, RetainedWorkspaceAuthority, SandboxCommand, SandboxError, SandboxManifest,
    SandboxOutput, SandboxRegistry, SyscallPolicy,
};

use crate::error::SwarmError;
use crate::heartbeat::{self, WorkerStatusFile};
use crate::worktree::{TransactionWorkspace, WorkspaceCapacity, WorktreeManager};
use crate::{SwarmBrief, SwarmResult, WorkerHandle, WorkerStatus};

pub(crate) struct WorkerTerminal {
    pub handle: WorkerHandle,
    pub heartbeat: Option<WorkerStatusFile>,
}

enum SandboxTerminal {
    Output(SandboxOutput),
    Cancelled,
    WorkspaceAccountingExceeded(u64),
    Error(SandboxError),
}

fn admit_delegated_backend(registry: &SandboxRegistry) -> Result<(), String> {
    if !registry.enforces_read_deny() {
        return Err(format!(
            "sandbox backend {} cannot enforce delegated read denial",
            registry.backend_name()
        ));
    }
    if registry.bypasses_containment() {
        return Err(format!(
            "sandbox backend {} bypasses delegated containment",
            registry.backend_name()
        ));
    }
    if !registry.owns_descendants_hard() {
        return Err(format!(
            "sandbox backend {} cannot own descendants that escape a process group; select Docker for delegated Swarm execution on this host",
            registry.backend_name()
        ));
    }
    if !registry.binds_workspace_authority() {
        return Err(format!(
            "sandbox backend {} cannot bind retained delegated workspace authority",
            registry.backend_name()
        ));
    }
    Ok(())
}

async fn select_delegated_backend() -> Result<SandboxRegistry, String> {
    let primary = SandboxRegistry::required_for_session(None)
        .map_err(|error| format!("sandbox selection: {error}"))?;
    match admit_delegated_backend(&primary) {
        Ok(()) => Ok(primary),
        Err(primary_error) => {
            #[cfg(target_os = "macos")]
            {
                use std::sync::Arc;
                let docker = wcore_sandbox::backends::docker::DockerBackend::connect()
                    .await
                    .map_err(|error| {
                        format!(
                            "{primary_error}; qualified Docker fallback is unavailable on this macOS host: {error}"
                        )
                    })?;
                let fallback = SandboxRegistry::new(Arc::new(docker));
                admit_delegated_backend(&fallback).map_err(|fallback_error| {
                    format!(
                        "{primary_error}; qualified Docker fallback was rejected: {fallback_error}"
                    )
                })?;
                Ok(fallback)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(primary_error)
            }
        }
    }
}

/// Defense-in-depth logical accounting after capacity admission.
///
/// This polling observer is deliberately not described as a hard filesystem
/// quota: portable sandbox backends do not supply one, and a process can grow
/// between observations. The authoritative guarantee for F20 is atomic
/// pre-materialization capacity admission plus a retained aggregate
/// reservation. The observer shortens exposure and records an honest failure.
struct WorkspaceMonitor {
    receiver: tokio::sync::mpsc::UnboundedReceiver<crate::error::Result<u64>>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl WorkspaceMonitor {
    fn start(workspace: TransactionWorkspace) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            loop {
                let scan_workspace = workspace.clone();
                let scan_cancel = task_cancel.clone();
                let scan = tokio::task::spawn_blocking(move || {
                    scan_workspace.logical_used_bytes_with_cancel(Some(&scan_cancel))
                });
                let result = tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => break,
                    result = scan => match result {
                        Ok(result) => result,
                        Err(error) => Err(SwarmError::WorktreeIo(format!(
                            "workspace accounting task failed: {error}"
                        ))),
                    },
                };
                if sender.send(result).is_err() {
                    break;
                }
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
            }
        });
        Self {
            receiver,
            cancel,
            task,
        }
    }

    async fn recv(&mut self) -> Option<crate::error::Result<u64>> {
        self.receiver.recv().await
    }
}

impl Drop for WorkspaceMonitor {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

impl WorkerTerminal {
    fn without_heartbeat(handle: WorkerHandle) -> Self {
        Self {
            handle,
            heartbeat: None,
        }
    }
}

/// Run a single worker end-to-end: create the worktree, spawn the
/// subprocess, wait up to `brief.timeout`, capture stdout/stderr. Returns
/// the handle (which carries the final status — never returns an Err;
/// failures are recorded inside the handle so the caller can drain all
/// workers regardless of individual failures).
pub(crate) async fn run_worker(
    manager: &WorktreeManager,
    worker_id: String,
    brief: &SwarmBrief,
    stream_output_bytes: usize,
    pinned_head: &str,
    capacity: WorkspaceCapacity,
    cancel: CancellationToken,
) -> WorkerTerminal {
    let branch = format!("{}/{}", brief.worker_branch_prefix, worker_id);
    let start = Instant::now();

    // 1. Create a child-owned standalone repository. Keep the workspace
    // authority alive until the worker reaches a terminal state.
    let create_result = {
        let create_tree =
            manager.create_isolated_checkout(&worker_id, &branch, pinned_head, capacity);
        tokio::pin!(create_tree);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            result = &mut create_tree => Some(result),
        }
    };
    let workspace = match create_result {
        None => {
            return WorkerTerminal::without_heartbeat(cancelled(
                worker_id,
                branch,
                start.elapsed(),
            ));
        }
        Some(Ok(workspace)) => workspace,
        Some(Err(error)) => {
            return WorkerTerminal::without_heartbeat(WorkerHandle::failed(
                worker_id,
                branch,
                format!("worktree create: {error}"),
                start.elapsed(),
            ));
        }
    };

    // 2. Parse the worker command (argv mode — no shell interpretation).
    let mut iter = brief.worker_command.iter();
    let program = match iter.next() {
        Some(p) => p.clone(),
        None => {
            return release_terminal(
                manager,
                workspace,
                WorkerHandle::failed(
                    worker_id,
                    branch,
                    "empty worker_command".into(),
                    start.elapsed(),
                ),
            );
        }
    };
    let args: Vec<String> = iter.cloned().collect();

    // 3. Execute only through a real platform sandbox. The worker receives
    // checkout/scratch authority, a scrubbed environment, no network, and no
    // access to parent Git, sibling, lease, or reservation evidence.
    let registry = match select_delegated_backend().await {
        Ok(registry) => registry,
        Err(error) => {
            return release_terminal(
                manager,
                workspace,
                WorkerHandle::failed(worker_id, branch, error, start.elapsed()),
            );
        }
    };
    let container_owned_workspace = registry.backend_name() == "docker";
    let manifest = match worker_manifest(
        manager,
        &workspace,
        &program,
        &brief.env,
        brief.timeout,
        container_owned_workspace,
    )
    .await
    {
        Ok(manifest) => manifest,
        Err(error) => {
            return release_terminal(
                manager,
                workspace,
                WorkerHandle::failed(worker_id, branch, error, start.elapsed()),
            );
        }
    };
    let command = SandboxCommand {
        argv: std::iter::once(program).chain(args).collect(),
        cwd: Some(workspace.checkout.clone()),
    };
    let captured = {
        let root_authority = match workspace.root_authority() {
            Ok(authority) => authority,
            Err(error) => {
                return release_terminal(
                    manager,
                    workspace,
                    WorkerHandle::failed(
                        worker_id,
                        branch,
                        format!("worker root authority: {error}"),
                        start.elapsed(),
                    ),
                );
            }
        };
        let workspace_authority = match RetainedWorkspaceAuthority::new(
            root_authority,
            workspace.checkout_authority(),
            format!("{worker_id}:{}", workspace.reserved_bytes),
        ) {
            Ok(authority) => authority,
            Err(error) => {
                return release_terminal(
                    manager,
                    workspace,
                    WorkerHandle::failed(
                        worker_id,
                        branch,
                        format!("worker retained workspace: {error}"),
                        start.elapsed(),
                    ),
                );
            }
        };
        let backend_cancel = CancellationToken::new();
        let execute = registry.execute_with_workspace_authority(
            &manifest,
            command,
            workspace_authority,
            workspace.reserved_bytes,
            || {
                manager
                    .validate_repo_authority()
                    .and_then(|()| workspace.validate_execution_authority())
                    .map_err(|error| {
                        SandboxError::PathDenied(format!(
                            "worker filesystem authority before execution: {error}"
                        ))
                    })
            },
            backend_cancel.clone(),
        );
        tokio::pin!(execute);
        let mut workspace_monitor = WorkspaceMonitor::start(workspace.clone());
        let mut heartbeat_tick = tokio::time::interval(Duration::from_millis(20));
        let mut cancel_requested = false;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if !cancel_requested => {
                    if container_owned_workspace {
                        cancel_requested = true;
                        backend_cancel.cancel();
                    } else {
                        break SandboxTerminal::Cancelled;
                    }
                },
                result = &mut execute => break if cancel_requested {
                    SandboxTerminal::Cancelled
                } else {
                    match result {
                        Ok(output) => SandboxTerminal::Output(output),
                        Err(error) => SandboxTerminal::Error(error),
                    }
                },
                _ = heartbeat_tick.tick(), if !cancel_requested => {
                    if let Err(error) = mirror_heartbeat(&workspace) {
                        break SandboxTerminal::Error(SandboxError::PathDenied(error));
                    }
                }
                usage = workspace_monitor.recv(), if !cancel_requested => {
                    match usage {
                        None => break SandboxTerminal::Error(SandboxError::ExecFailed(
                            "workspace accounting monitor stopped unexpectedly".to_owned()
                        )),
                        Some(result) => match result {
                        Ok(bytes) if bytes > workspace.reserved_bytes => {
                            break SandboxTerminal::WorkspaceAccountingExceeded(bytes);
                        }
                        Ok(_) => {}
                        Err(error) => break SandboxTerminal::Error(SandboxError::ExecFailed(
                            format!("workspace accounting: {error}")
                        )),
                        }
                    }
                }
            }
        }
    };
    let captured = match captured {
        SandboxTerminal::Output(output) => match workspace.logical_used_bytes() {
            Ok(bytes) if bytes > workspace.reserved_bytes => {
                SandboxTerminal::WorkspaceAccountingExceeded(bytes)
            }
            Ok(_) => SandboxTerminal::Output(output),
            Err(error) => SandboxTerminal::Error(SandboxError::ExecFailed(format!(
                "workspace accounting after execution: {error}"
            ))),
        },
        terminal => terminal,
    };
    let captured = match mirror_heartbeat(&workspace) {
        Ok(()) => captured,
        Err(error) => SandboxTerminal::Error(SandboxError::PathDenied(error)),
    };
    let output = match captured {
        SandboxTerminal::Output(output) => output,
        SandboxTerminal::Cancelled => {
            return release_terminal(
                manager,
                workspace,
                cancelled(worker_id, branch, start.elapsed()),
            );
        }
        SandboxTerminal::WorkspaceAccountingExceeded(bytes) => {
            let reserved_bytes = workspace.reserved_bytes;
            return release_terminal(
                manager,
                workspace,
                WorkerHandle::failed(
                    worker_id,
                    branch,
                    format!(
                        "workspace accounting observed {bytes} bytes, exceeding the {}-byte reservation",
                        reserved_bytes
                    ),
                    start.elapsed(),
                ),
            );
        }
        SandboxTerminal::Error(SandboxError::Timeout) => {
            return release_terminal(
                manager,
                workspace,
                WorkerHandle {
                    worker_id,
                    branch,
                    status: WorkerStatus::TimedOut,
                    stdout: String::new(),
                    stderr: String::new(),
                    duration: start.elapsed(),
                },
            );
        }
        SandboxTerminal::Error(SandboxError::OutputLimitExceeded { limit_bytes }) => {
            return release_terminal(
                manager,
                workspace,
                WorkerHandle::failed(
                    worker_id,
                    branch,
                    format!(
                        "output limit exceeded: stdout/stderr exceeded the {limit_bytes}-byte sandbox limit"
                    ),
                    start.elapsed(),
                ),
            );
        }
        SandboxTerminal::Error(error) => {
            return release_terminal(
                manager,
                workspace,
                WorkerHandle::failed(
                    worker_id,
                    branch,
                    format!("worker process: {error}"),
                    start.elapsed(),
                ),
            );
        }
    };

    if output.stdout.len() > stream_output_bytes || output.stderr.len() > stream_output_bytes {
        return release_terminal(
            manager,
            workspace,
            WorkerHandle::failed(
                worker_id,
                branch,
                format!(
                    "output limit exceeded: worker stream exceeded the {stream_output_bytes}-byte limit"
                ),
                start.elapsed(),
            ),
        );
    }
    let status = if output.exit_code == 0 {
        WorkerStatus::Succeeded
    } else {
        WorkerStatus::Failed(format!("exit {}", output.exit_code))
    };
    // Valid UTF-8 reuses the bounded capture allocations rather than briefly
    // duplicating the complete dispatch output during result conversion.
    let stdout = into_lossy_string(output.stdout);
    let stderr = into_lossy_string(output.stderr);
    release_terminal(
        manager,
        workspace,
        WorkerHandle {
            worker_id,
            branch,
            status,
            stdout,
            stderr,
            duration: start.elapsed(),
        },
    )
}

fn mirror_heartbeat(workspace: &TransactionWorkspace) -> Result<(), String> {
    workspace
        .validate_execution_authority()
        .map_err(|error| format!("heartbeat filesystem authority before read: {error}"))?;
    let checkout_authority = workspace.checkout_authority();
    let root_authority = workspace
        .root_authority()
        .map_err(|error| format!("heartbeat root authority: {error}"))?;
    let Some(status) = heartbeat::read_status_authorized(&checkout_authority)
        .map_err(|error| format!("heartbeat status read: {error}"))?
    else {
        return Ok(());
    };
    let encoded =
        serde_json::to_vec(&status).map_err(|error| format!("heartbeat status encode: {error}"))?;
    root_authority
        .atomic_write_child(heartbeat::STATUS_FILE, &encoded)
        .map_err(|error| format!("heartbeat mirror write: {error}"))?;
    workspace
        .validate_execution_authority()
        .map_err(|error| format!("heartbeat filesystem authority after write: {error}"))?;
    Ok(())
}

/// Whether a worker descendant still holds a duplicate of the retained checkout
/// directory descriptor (inherited across the sandbox spawn boundary). Rebuilds
/// the owner-bound retained authority to query the shared loan counter; any
/// identity failure returns `false` so the caller falls through to
/// `release_transaction`, which independently re-validates and fails closed.
fn checkout_loan_outstanding(workspace: &TransactionWorkspace, worker_id: &str) -> bool {
    let Ok(root_authority) = workspace.root_authority() else {
        return false;
    };
    let Ok(retained) = RetainedWorkspaceAuthority::new(
        root_authority,
        workspace.checkout_authority(),
        format!("{worker_id}:{}", workspace.reserved_bytes),
    ) else {
        return false;
    };
    retained.checkout_has_outstanding_loans()
}

fn release_terminal(
    manager: &WorktreeManager,
    workspace: TransactionWorkspace,
    mut terminal: WorkerHandle,
) -> WorkerTerminal {
    let checkout_authority = workspace.checkout_authority();
    let heartbeat = match heartbeat::read_status_authorized(&checkout_authority) {
        Ok(heartbeat) => heartbeat,
        Err(error) => {
            let diagnostic =
                format!("malformed worker heartbeat preserved before cleanup: {error}");
            terminal.status = WorkerStatus::Failed(diagnostic.clone());
            if !terminal.stderr.is_empty() {
                terminal.stderr.push('\n');
            }
            terminal.stderr.push_str(&diagnostic);
            None
        }
    };
    // Defense-in-depth before owner-bound cleanup: refuse to remove the checkout
    // while a worker descendant still holds the inherited retained directory
    // descriptor. Quarantine the transaction (keep it reserved) instead of
    // releasing, so a live child cannot lose its working directory and a
    // same-path replacement cannot be substituted before the loan drops.
    if checkout_loan_outstanding(&workspace, &terminal.worker_id) {
        let diagnostic = "worker descendant still holds the retained checkout descriptor; \
             transaction quarantined and its reservation held for retry"
            .to_owned();
        terminal.status = WorkerStatus::Failed(diagnostic.clone());
        if !terminal.stderr.is_empty() {
            terminal.stderr.push('\n');
        }
        terminal.stderr.push_str(&diagnostic);
        return WorkerTerminal {
            handle: terminal,
            heartbeat,
        };
    }
    match manager.release_transaction(&workspace) {
        Ok(()) => WorkerTerminal {
            handle: terminal,
            heartbeat,
        },
        Err(error) => WorkerTerminal {
            handle: WorkerHandle::failed(
                terminal.worker_id,
                terminal.branch,
                format!("transaction cleanup: {error}"),
                terminal.duration,
            ),
            heartbeat,
        },
    }
}

fn into_lossy_string(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

async fn worker_manifest(
    manager: &WorktreeManager,
    workspace: &TransactionWorkspace,
    program: &str,
    env: &[(String, String)],
    timeout: Duration,
    container_owned_workspace: bool,
) -> Result<SandboxManifest, String> {
    let mut fs_read_allow = vec![workspace.checkout.clone(), workspace.scratch.clone()];
    let program_path = PathBuf::from(program);
    if program_path.is_absolute() && !container_owned_workspace {
        fs_read_allow.push(
            std::fs::canonicalize(&program_path)
                .map_err(|error| format!("worker executable: {error}"))?,
        );
    }
    let mut scrubbed = Vec::new();
    if !container_owned_workspace {
        for name in ["PATH", "PATHEXT", "SYSTEMROOT", "COMSPEC"] {
            if let Ok(value) = std::env::var(name) {
                scrubbed.push((name.to_owned(), value));
            }
        }
    }
    for (name, value) in env {
        if env_name_is_safe(name) {
            scrubbed.retain(|(existing, _)| existing != name);
            scrubbed.push((name.clone(), value.clone()));
        }
    }
    let scratch = workspace.scratch.to_string_lossy().into_owned();
    let checkout = workspace.checkout.to_string_lossy().into_owned();
    for (name, value) in [
        ("HOME", scratch.as_str()),
        ("TMPDIR", scratch.as_str()),
        ("TMP", scratch.as_str()),
        ("TEMP", scratch.as_str()),
        ("WAYLAND_SWARM_CHECKOUT", checkout.as_str()),
        ("WAYLAND_SWARM_SCRATCH", scratch.as_str()),
    ] {
        scrubbed.retain(|(existing, _)| existing != name);
        scrubbed.push((name.to_owned(), value.to_owned()));
    }
    Ok(SandboxManifest {
        fs_read_allow,
        fs_write_allow: vec![workspace.checkout.clone(), workspace.scratch.clone()],
        fs_read_deny: manager
            .sandbox_read_denies(workspace)
            .await
            .map_err(|error| format!("sandbox deny authority: {error}"))?,
        network: NetworkPolicy::Deny,
        syscall_policy: SyscallPolicy::Inherit,
        timeout: Some(timeout),
        max_memory_bytes: None,
        max_cpu_secs: None,
        env: scrubbed,
        image: if container_owned_workspace {
            "ghcr.io/tradecanyon/wcore-sandbox:base".to_owned()
        } else {
            String::new()
        },
    })
}

fn env_name_is_safe(name: &str) -> bool {
    let upper = name.trim().to_ascii_uppercase();
    if upper.is_empty()
        || matches!(
            upper.as_str(),
            "HOME"
                | "TMPDIR"
                | "TMP"
                | "TEMP"
                | "GIT_DIR"
                | "GIT_COMMON_DIR"
                | "GIT_WORK_TREE"
                | "GIT_INDEX_FILE"
                | "GIT_OBJECT_DIRECTORY"
                | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
                | "WAYLAND_SANDBOX"
                | "WAYLAND_ALLOW_NO_SANDBOX"
        )
        || upper.starts_with("GIT_CONFIG")
    {
        return false;
    }
    ![
        "API_KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "PASSPHRASE",
        "CREDENTIAL",
        "PRIVATE_KEY",
    ]
    .iter()
    .any(|marker| upper.contains(marker))
}

fn cancelled(worker_id: String, branch: String, duration: Duration) -> WorkerHandle {
    WorkerHandle {
        worker_id,
        branch,
        status: WorkerStatus::Cancelled,
        stdout: String::new(),
        stderr: String::new(),
        duration,
    }
}

impl WorkerHandle {
    pub(crate) fn failed(
        worker_id: String,
        branch: String,
        reason: String,
        duration: Duration,
    ) -> Self {
        Self {
            worker_id,
            branch,
            status: WorkerStatus::Failed(reason),
            stdout: String::new(),
            stderr: String::new(),
            duration,
        }
    }

    /// Consume the handle and produce a `SwarmResult` (the wire-friendly,
    /// `Serialize`-able twin used by callers and TOML briefs).
    pub fn into_result(self) -> SwarmResult {
        SwarmResult {
            worker_id: self.worker_id,
            branch: self.branch,
            status: self.status,
            stdout: self.stdout,
            stderr: self.stderr,
            duration: self.duration,
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::sync::Arc;

    use wcore_sandbox::backends::sandbox_exec::SandboxExecBackend;

    use super::*;

    #[test]
    fn sandbox_exec_is_refused_before_descendant_escape_can_spawn() {
        let registry = SandboxRegistry::new(Arc::new(SandboxExecBackend::new()));

        let error = admit_delegated_backend(&registry)
            .expect_err("process-group-only backend admitted public Swarm execution");

        assert!(error.contains("escape a process group"), "{error}");
        assert!(error.contains("select Docker"), "{error}");
    }
}
