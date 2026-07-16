//! Worker spawn + run logic for `Swarm::dispatch`.
//!
//! The locked surface is `dispatch(&self, brief, count) -> Vec<WorkerHandle>`.
//! Each worker is spawned in its own worktree as a subprocess of the
//! orchestrator (process boundary; no shared memory).

use std::path::Path;
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use wcore_config::shell;
use wcore_sandbox::process_capture::{CaptureLimits, ProcessCaptureError, capture_bounded_process};

use crate::worktree::WorktreeManager;
use crate::{SwarmBrief, SwarmResult, WorkerHandle, WorkerStatus};

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
    cancel: CancellationToken,
) -> WorkerHandle {
    let branch = format!("{}/{}", brief.worker_branch_prefix, worker_id);
    let start = Instant::now();

    // 1. Create the worker worktree.
    let create_result = {
        let create_tree = manager.create_worker_tree(&worker_id, &branch, &brief.base_branch);
        tokio::pin!(create_tree);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            result = &mut create_tree => Some(result),
        }
    };
    let tree_path = match create_result {
        None => return cancelled(worker_id, branch, start.elapsed()),
        Some(Ok(path)) => path,
        Some(Err(error)) => {
            return WorkerHandle::failed(
                worker_id,
                branch,
                format!("worktree create: {error}"),
                start.elapsed(),
            );
        }
    };

    // 2. Parse the worker command (argv mode — no shell interpretation).
    let mut iter = brief.worker_command.iter();
    let program = match iter.next() {
        Some(p) => p.clone(),
        None => {
            return WorkerHandle::failed(
                worker_id,
                branch,
                "empty worker_command".into(),
                start.elapsed(),
            );
        }
    };
    let args: Vec<String> = iter.cloned().collect();

    // 3. Capture the worker under the same platform-owned process-tree
    // primitive used by the sandbox. Output is capped while it is read, not
    // truncated after an unbounded allocation.
    let command = build_worker_command(&program, &args, &tree_path, &brief.env);
    let output = match capture_bounded_process(
        command,
        CaptureLimits {
            stdout_bytes: stream_output_bytes,
            stderr_bytes: stream_output_bytes,
            timeout: brief.timeout,
        },
        Some(&cancel),
    )
    .await
    {
        Ok(output) => output,
        Err(ProcessCaptureError::Cancelled) => {
            return cancelled(worker_id, branch, start.elapsed());
        }
        Err(ProcessCaptureError::Timeout(_)) => {
            return WorkerHandle {
                worker_id,
                branch,
                status: WorkerStatus::TimedOut,
                stdout: String::new(),
                stderr: String::new(),
                duration: start.elapsed(),
            };
        }
        Err(ProcessCaptureError::OutputLimit { stream, limit }) => {
            return WorkerHandle::failed(
                worker_id,
                branch,
                format!("output limit exceeded: {stream} exceeded the {limit}-byte limit"),
                start.elapsed(),
            );
        }
        Err(error) => {
            return WorkerHandle::failed(
                worker_id,
                branch,
                format!("worker process: {error}"),
                start.elapsed(),
            );
        }
    };

    let status = if output.status.success() {
        WorkerStatus::Succeeded
    } else {
        WorkerStatus::Failed(format!("exit {:?}", output.status.code()))
    };
    // Valid UTF-8 reuses the bounded capture allocations rather than briefly
    // duplicating the complete dispatch output during result conversion.
    let stdout = into_lossy_string(output.stdout);
    let stderr = into_lossy_string(output.stderr);
    WorkerHandle {
        worker_id,
        branch,
        status,
        stdout,
        stderr,
        duration: start.elapsed(),
    }
}

fn into_lossy_string(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

/// Build the worker subprocess Command. Always argv mode (no shell).
///
/// `program` is resolved via the OS's PATH (and PATHEXT on Windows) by
/// `Command::new`. `args` are passed as separate argv entries — shell
/// metacharacters in args are NEVER interpreted by a shell.
fn build_worker_command(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
) -> Command {
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let mut cmd = shell::shell_command_argv(program, &args);
    cmd.current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
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
