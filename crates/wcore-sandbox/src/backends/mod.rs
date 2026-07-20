//! Sandbox backend trait + implementations.

use std::sync::Arc;

use crate::error::Result;
use crate::manifest::SandboxManifest;
use crate::{SandboxChunk, SandboxCommand, SandboxOutput};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt};

pub mod appcontainer;
pub mod bwrap;
#[cfg(all(target_os = "linux", feature = "landlock"))]
pub mod bwrap_landlock;
#[cfg(all(target_os = "linux", feature = "seccomp"))]
pub mod bwrap_seccomp;
pub mod docker;
pub mod no_sandbox;
pub mod process_tree;
#[cfg(target_os = "macos")]
pub mod sandbox_exec;

/// Channel buffer for the streaming receiver. The default buffered impl
/// only sends three messages, so any positive value works; a small buffer
/// keeps a native streaming backend from racing far ahead of a slow
/// consumer.
const STREAM_CHANNEL_CAP: usize = 64;

/// Aggregate stdout + stderr ceiling for a buffered sandbox execution.
/// Streaming callers have their own bounded channel/backpressure contract;
/// buffered hooks, gates, and embedded skills must never grow host memory in
/// proportion to child output.
pub(crate) const BUFFERED_OUTPUT_LIMIT_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct BoundedChildOutput {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub(crate) fn reserve_output(used: &AtomicUsize, amount: usize) -> bool {
    let mut current = used.load(Ordering::Relaxed);
    loop {
        let Some(next) = current.checked_add(amount) else {
            return false;
        };
        if next > BUFFERED_OUTPUT_LIMIT_BYTES {
            return false;
        }
        match used.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

async fn read_bounded(
    mut reader: impl AsyncRead + Unpin,
    used: Arc<AtomicUsize>,
) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            return Ok(output);
        }
        if !reserve_output(&used, count) {
            return Err(crate::SandboxError::OutputLimitExceeded {
                limit_bytes: BUFFERED_OUTPUT_LIMIT_BYTES,
            });
        }
        output.extend_from_slice(&chunk[..count]);
    }
}

/// Bounded wait variant for callers that own a process group or Job Object.
/// `on_exit` runs as soon as the direct child exits, before waiting for pipe
/// EOF, so a background descendant cannot keep the pipes open and perform
/// delayed work after its parent has completed.
pub(crate) async fn wait_with_bounded_output_on_exit(
    child: &mut tokio::process::Child,
    on_exit: impl FnOnce(),
) -> Result<BoundedChildOutput> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| crate::SandboxError::ExecFailed("child stdout was not piped".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| crate::SandboxError::ExecFailed("child stderr was not piped".into()))?;
    let used = Arc::new(AtomicUsize::new(0));
    let drains = async {
        let (stdout, stderr) = tokio::try_join!(
            read_bounded(stdout, Arc::clone(&used)),
            read_bounded(stderr, used),
        )?;
        Ok::<_, crate::SandboxError>((stdout, stderr))
    };
    tokio::pin!(drains);
    let mut on_exit = Some(on_exit);

    tokio::select! {
        biased;
        waited = child.wait() => {
            let status = waited?;
            on_exit.take().expect("exit callback runs once")();
            let (stdout, stderr) = drains.await?;
            Ok(BoundedChildOutput { status, stdout, stderr })
        }
        drained = &mut drains => {
            let (stdout, stderr) = drained?;
            let status = child.wait().await?;
            on_exit.take().expect("exit callback runs once")();
            Ok(BoundedChildOutput { status, stdout, stderr })
        }
    }
}

#[async_trait]
pub trait SandboxBackend: Send + Sync + 'static {
    /// Execute `cmd` inside the sandbox defined by `manifest`.
    ///
    /// Caller is responsible for not passing interactive stdin (no
    /// streaming stdin support in v0.6.3).
    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput>;

    /// Execute `cmd` inside the sandbox, streaming output back as it is
    /// produced via an `mpsc` channel.
    ///
    /// A successful run yields zero or more [`SandboxChunk::Stdout`] /
    /// [`SandboxChunk::Stderr`] chunks followed by exactly one terminal
    /// [`SandboxChunk::Exit`]. If the channel closes without an `Exit`
    /// chunk the child failed to start or was dropped — callers should
    /// treat a missing `Exit` as an error.
    ///
    /// Takes `self: Arc<Self>` so the default implementation can move an
    /// owned handle into a background task; this stays object-safe, so
    /// `Arc<dyn SandboxBackend>` callers can invoke it directly.
    ///
    /// The default implementation wraps [`SandboxBackend::execute`]: it
    /// spawns a task that runs the buffered call to completion and emits
    /// the whole stdout buffer as one `Stdout` chunk, the whole stderr
    /// buffer as one `Stderr` chunk, then the `Exit` chunk. Backends that
    /// can stream natively (or want true incremental output) override
    /// this. This default exists so every backend satisfies the trait
    /// without each having to reimplement streaming.
    fn execute_streaming(
        self: Arc<Self>,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<tokio::sync::mpsc::Receiver<SandboxChunk>> {
        let (tx, rx) = tokio::sync::mpsc::channel(STREAM_CHANNEL_CAP);
        // Own the manifest so the task does not borrow the caller's stack.
        let manifest = manifest.clone();
        tokio::spawn(async move {
            let result = tokio::select! {
                _ = tx.closed() => return,
                result = self.execute(&manifest, cmd) => result,
            };
            match result {
                Ok(out) => {
                    if !out.stdout.is_empty() {
                        let _ = tx.send(SandboxChunk::Stdout(out.stdout)).await;
                    }
                    if !out.stderr.is_empty() {
                        let _ = tx.send(SandboxChunk::Stderr(out.stderr)).await;
                    }
                    let _ = tx
                        .send(SandboxChunk::Exit {
                            exit_code: out.exit_code,
                            resource_limits: out.resource_limits,
                        })
                        .await;
                }
                Err(e) => {
                    // Surface the failure on stderr then close without an
                    // Exit chunk — the missing terminal chunk is the
                    // documented signal that the child never ran.
                    let _ = tx
                        .send(SandboxChunk::Stderr(
                            format!("sandbox execute_streaming failed: {e}").into_bytes(),
                        ))
                        .await;
                }
            }
        });
        Ok(rx)
    }

    fn name(&self) -> &'static str;

    /// True if this backend can be used on the current host right now
    /// (e.g. `bwrap` binary in PATH, sandbox-exec probe passes, Docker
    /// daemon reachable, AppContainer profile creation works). Used by
    /// `default_for_platform` to pick a fallback when the preferred
    /// backend is unavailable.
    fn is_available(&self) -> bool;

    /// True if this backend enforces `manifest.fs_read_deny` at the OS layer.
    /// The agent uses this to decide whether `Bash` may run in the untrusted
    /// `Workspace` posture. Default `false` — a backend opts in by overriding
    /// AND actually implementing the deny.
    fn enforces_read_deny(&self) -> bool {
        false
    }

    /// True only when the backend owns the complete descendant tree even if
    /// an untrusted child calls `setsid` or `setpgid`. Process-group cleanup
    /// alone is not hard ownership and must keep the default `false`.
    fn owns_descendants_hard(&self) -> bool {
        false
    }

    /// True if this backend cannot run PowerShell (`powershell.exe` / `pwsh.exe`).
    /// The Windows AppContainer backend overrides this to `true`: PowerShell
    /// requires .NET / GAC assemblies that fail to load under the Low-integrity
    /// restricted token (`STATUS_DLL_NOT_FOUND`, 0xC0000135). Callers that pick
    /// the shell as an implementation detail (e.g. `BashTool`) use this to
    /// downgrade a powershell shell selection to `cmd` rather than failing every
    /// command. Default `false`. See FerroxLabs/wayland#413.
    fn blocks_powershell(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct PendingBackend {
        dropped: Arc<AtomicBool>,
        entered: Arc<tokio::sync::Notify>,
    }

    struct DropProbe(Arc<AtomicBool>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl SandboxBackend for PendingBackend {
        async fn execute(
            &self,
            _manifest: &SandboxManifest,
            _cmd: SandboxCommand,
        ) -> Result<SandboxOutput> {
            let _probe = DropProbe(Arc::clone(&self.dropped));
            self.entered.notify_one();
            std::future::pending().await
        }

        fn name(&self) -> &'static str {
            "pending-test"
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn dropping_stream_receiver_drops_backend_execution() {
        let dropped = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(tokio::sync::Notify::new());
        let backend = Arc::new(PendingBackend {
            dropped: Arc::clone(&dropped),
            entered: Arc::clone(&entered),
        });
        let rx = backend
            .execute_streaming(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec!["pending".into()],
                    cwd: None,
                },
            )
            .expect("stream worker must start");
        tokio::time::timeout(std::time::Duration::from_millis(250), entered.notified())
            .await
            .expect("backend execution must start before receiver drop");
        drop(rx);

        tokio::time::timeout(std::time::Duration::from_millis(250), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("receiver drop must cancel the backend execution future");
    }
}
