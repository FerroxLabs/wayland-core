//! Worker spawn + run logic for `Swarm::dispatch`.
//!
//! The locked surface is `dispatch(&self, brief, count) -> Vec<WorkerHandle>`.
//! Each worker is spawned in its own worktree as a subprocess of the
//! orchestrator (process boundary; no shared memory).

use std::path::Path;
use std::time::{Duration, Instant};

use tokio::process::Command;
use tokio::time::timeout;

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
) -> WorkerHandle {
    let branch = format!("{}/{}", brief.worker_branch_prefix, worker_id);
    let start = Instant::now();

    // 1. Create the worker worktree.
    let tree_path = match manager
        .create_worker_tree(&worker_id, &branch, &brief.base_branch)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            return WorkerHandle::failed(
                worker_id,
                branch,
                format!("worktree create: {e}"),
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

    // 3. Spawn the subprocess in the worker's tree.
    let output_fut = build_worker_command(&program, &args, &tree_path, &brief.env).output();

    // 4. Wait up to timeout. tokio::process::Command has kill_on_drop set
    //    by default only when constructed via shell::shell_command_argv; we
    //    construct directly here for the worker (the helper requires the
    //    program to be a known shell binary in some signatures). Use
    //    `.kill_on_drop(true)` explicitly to mirror the safety invariant.
    let output = match timeout(brief.timeout, output_fut).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return WorkerHandle::failed(worker_id, branch, format!("spawn: {e}"), start.elapsed());
        }
        Err(_) => {
            return WorkerHandle {
                worker_id,
                branch,
                status: WorkerStatus::TimedOut,
                stdout: String::new(),
                stderr: String::new(),
                duration: start.elapsed(),
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let status = if output.status.success() {
        WorkerStatus::Succeeded
    } else {
        WorkerStatus::Failed(format!("exit {:?}", output.status.code()))
    };
    WorkerHandle {
        worker_id,
        branch,
        status,
        stdout,
        stderr,
        duration: start.elapsed(),
    }
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
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd).kill_on_drop(true);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
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
