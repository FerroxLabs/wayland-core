//! Bounded, cancellation-aware ownership for non-sandboxed child processes.
//!
//! This is for trusted host utilities that cannot run through a sandbox
//! backend. It caps both pipes while bytes are being read and owns the whole
//! process tree, so post-hoc result truncation cannot hide an unbounded
//! allocation or a surviving descendant.

use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

use crate::backends::process_tree::{self, ProcessTreeGuard};

/// Per-stream and wall-clock limits for one child invocation.
#[derive(Debug, Clone, Copy)]
pub struct CaptureLimits {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub timeout: Duration,
}

/// Fully bounded output from a completed child.
#[derive(Debug)]
pub struct CapturedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessCaptureError {
    #[error("failed to spawn process: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("failed to establish process-tree containment: {0}")]
    Containment(#[source] std::io::Error),
    #[error("failed to capture process {stream}: {source}")]
    Read {
        stream: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to wait for process: {0}")]
    Wait(#[source] std::io::Error),
    #[error("process {stream} exceeded the {limit}-byte output limit")]
    OutputLimit { stream: &'static str, limit: usize },
    #[error("process timed out after {0:?}")]
    Timeout(Duration),
    #[error("process cancelled")]
    Cancelled,
}

impl ProcessCaptureError {
    /// Whether the executable could not be started because it was absent.
    pub fn is_not_found(&self) -> bool {
        matches!(Self::root_io_error(self), Some(error) if error.kind() == std::io::ErrorKind::NotFound)
    }

    fn root_io_error(&self) -> Option<&std::io::Error> {
        match self {
            Self::Spawn(error) | Self::Containment(error) | Self::Wait(error) => Some(error),
            Self::Read { source, .. } => Some(source),
            Self::OutputLimit { .. } | Self::Timeout(_) | Self::Cancelled => None,
        }
    }
}

/// Run `command` with bounded stdout/stderr, a hard deadline, optional
/// cancellation, and whole-tree cleanup on every exit path.
pub async fn capture_bounded_process(
    mut command: tokio::process::Command,
    limits: CaptureLimits,
    cancel: Option<&CancellationToken>,
) -> Result<CapturedOutput, ProcessCaptureError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    process_tree::isolate(&mut command);

    let mut child = command.spawn().map_err(ProcessCaptureError::Spawn)?;
    let process_tree = match ProcessTreeGuard::new(child.id()) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(ProcessCaptureError::Containment(error));
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ProcessCaptureError::Read {
            stream: "stdout",
            source: std::io::Error::other("stdout pipe was not captured"),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ProcessCaptureError::Read {
            stream: "stderr",
            source: std::io::Error::other("stderr pipe was not captured"),
        })?;

    let execution = async move {
        let (stdout, stderr, status) = tokio::try_join!(
            read_bounded(stdout, "stdout", limits.stdout_bytes),
            read_bounded(stderr, "stderr", limits.stderr_bytes),
            async { child.wait().await.map_err(ProcessCaptureError::Wait) },
        )?;
        Ok(CapturedOutput {
            status,
            stdout,
            stderr,
        })
    };

    let result = tokio::select! {
        biased;
        _ = wait_for_cancel(cancel) => Err(ProcessCaptureError::Cancelled),
        result = tokio::time::timeout(limits.timeout, execution) => {
            result.map_err(|_| ProcessCaptureError::Timeout(limits.timeout))?
        }
    };

    // On success the direct child has already been waited. Dropping the guard
    // still kills any background descendants. On every error path it tears
    // down the direct child and descendants after the capture future is
    // dropped, closing both pipes without retaining additional bytes.
    drop(process_tree);
    result
}

async fn wait_for_cancel(cancel: Option<&CancellationToken>) {
    match cancel {
        Some(cancel) => cancel.cancelled().await,
        None => std::future::pending().await,
    }
}

async fn read_bounded<R>(
    mut reader: R,
    stream: &'static str,
    limit: usize,
) -> Result<Vec<u8>, ProcessCaptureError>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let count = reader
            .read(&mut chunk)
            .await
            .map_err(|source| ProcessCaptureError::Read { stream, source })?;
        if count == 0 {
            return Ok(output);
        }
        let remaining = limit.saturating_sub(output.len());
        if count > remaining {
            return Err(ProcessCaptureError::OutputLimit { stream, limit });
        }
        output.extend_from_slice(&chunk[..count]);
    }
}
