//! Sandbox backend trait + implementations.

use std::sync::Arc;

use crate::error::Result;
use crate::manifest::SandboxManifest;
use crate::{SandboxChunk, SandboxCommand, SandboxOutput};
use async_trait::async_trait;

pub mod appcontainer;
pub mod bwrap;
#[cfg(all(target_os = "linux", feature = "landlock"))]
pub mod bwrap_landlock;
#[cfg(all(target_os = "linux", feature = "seccomp"))]
pub mod bwrap_seccomp;
pub mod docker;
pub mod no_sandbox;
#[cfg(target_os = "macos")]
pub mod sandbox_exec;

/// Channel buffer for the streaming receiver. The default buffered impl
/// only sends three messages, so any positive value works; a small buffer
/// keeps a native streaming backend from racing far ahead of a slow
/// consumer.
const STREAM_CHANNEL_CAP: usize = 64;

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
            match self.execute(&manifest, cmd).await {
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
