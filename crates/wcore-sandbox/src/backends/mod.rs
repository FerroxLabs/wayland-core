//! Sandbox backend trait + implementations.

use std::sync::Arc;

use crate::error::Result;
use crate::manifest::{HardContainmentFilesystem, SandboxManifest};
use crate::{
    DirectoryAuthority, RetainedWorkspaceAuthority, SandboxChunk, SandboxCommand, SandboxOutput,
};
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

/// The concrete OS mechanism a backend proves it will use for hard
/// containment.
///
/// There is deliberately NO variant for `sandbox-exec`, a bare process group,
/// the no-sandbox / Dangerous runtime, or a stub: those cannot appear here
/// because only the three qualifying backends ever construct a
/// [`HardContainmentIdentity`] (whose fields are crate-private), and only after
/// a successful live probe of the exact mechanism named below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // per-target: not every variant is constructed on every OS/feature build.
pub enum HardContainmentMechanism {
    /// Bubblewrap running the child in its own PID namespace (Linux).
    BubblewrapPidNamespace,
    /// A Docker container with the daemon owning the process tree (Linux).
    DockerContainer,
    /// A Windows AppContainer child governed by a kill-on-close Job Object.
    WindowsAppContainerJobObject,
}

/// Stable, cheap-to-recompute identity of a backend's hard-containment
/// mechanism.
///
/// The fields are `pub(crate)` on purpose: this is the structural seal. An
/// external crate can implement [`SandboxBackend`] but cannot construct a
/// `HardContainmentIdentity` (or a [`HardContainmentProbe`]), so no foreign,
/// spoofed, or non-qualifying backend can ever produce the value the registry
/// needs to mint a [`crate::HardContainmentAuthority`]. Only the in-crate
/// bubblewrap / docker / AppContainer backends build one.
#[derive(Clone, PartialEq, Eq)]
pub struct HardContainmentIdentity {
    pub(crate) mechanism: HardContainmentMechanism,
    /// Identity of the executable/runtime that will host the child (e.g. the
    /// resolved `bwrap` path, or the Docker image reference).
    pub(crate) executable_identity: String,
    /// Identity of the runtime endpoint (e.g. the bwrap mechanism tag, or the
    /// Docker daemon endpoint). Stable across mint and spawn.
    pub(crate) runtime_identity: String,
    /// The process-tree mechanism that will own and reap the child's whole
    /// tree. An ordinary process group is not representable here.
    pub(crate) process_tree_mechanism: process_tree::ProcessTreeMechanism,
}

/// Proof that a backend performed a successful semantic LIVE probe of its exact
/// hard-containment mechanism under the caller's normalized policy.
///
/// Returned only by [`SandboxBackend::probe_hard_containment`] and consumed only
/// by the registry when minting a [`crate::HardContainmentAuthority`]. Like
/// [`HardContainmentIdentity`], its field is crate-private so it cannot be
/// forged from outside the crate.
pub struct HardContainmentProbe {
    pub(crate) identity: HardContainmentIdentity,
}

impl std::fmt::Debug for HardContainmentIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redacted: executable_identity / runtime_identity are the resolved
        // backend paths/endpoints of a contained execution. Only the mechanism
        // discriminants are shown.
        f.debug_struct("HardContainmentIdentity")
            .field("mechanism", &self.mechanism)
            .field("process_tree_mechanism", &self.process_tree_mechanism)
            .field("executable_identity", &"<redacted>")
            .field("runtime_identity", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Debug for HardContainmentProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Delegates to the redacted HardContainmentIdentity Debug.
        f.debug_struct("HardContainmentProbe")
            .field("identity", &self.identity)
            .finish()
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

    /// Execute `cmd` with its working directory bound to a retained directory
    /// authority rather than a re-openable pathname. The default fails closed;
    /// a real backend opts in by overriding this AND [`Self::binds_cwd_authority`]
    /// so it reaches the child without reopening the directory's display path.
    async fn execute_with_cwd_authority(
        &self,
        _manifest: &SandboxManifest,
        _cmd: SandboxCommand,
        _cwd: DirectoryAuthority,
    ) -> Result<SandboxOutput> {
        Err(crate::SandboxError::PolicyNotSupported(format!(
            "sandbox backend {} cannot bind retained cwd authority",
            self.name()
        )))
    }

    /// Execute against an owner-bound disposable workspace. Native backends bind
    /// the retained checkout directly; container backends (Task 1D) export and
    /// import it without an ambient host bind mount. The default delegates to
    /// [`Self::execute_with_cwd_authority`] using the retained checkout, so a
    /// backend that binds a cwd authority also carries a workspace authority.
    ///
    /// `reauthorize` is re-run by import-capable backends at each trust
    /// boundary; `cancel` lets a container backend tear its own tree down. The
    /// native default relies on the backend's own process ownership and manifest
    /// timeout and therefore consumes neither.
    async fn execute_with_workspace_authority(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
        workspace: RetainedWorkspaceAuthority,
        _max_workspace_bytes: u64,
        _reauthorize: &(dyn Fn() -> Result<()> + Send + Sync),
        _cancel: tokio_util::sync::CancellationToken,
    ) -> Result<SandboxOutput> {
        self.execute_with_cwd_authority(manifest, cmd, workspace.workspace().clone())
            .await
    }

    /// True only when [`Self::execute_with_cwd_authority`] reaches the child
    /// without reopening the directory's display path. Default `false`.
    fn binds_cwd_authority(&self) -> bool {
        false
    }

    /// True only when [`Self::execute_with_workspace_authority`] preserves
    /// workspace identity for the complete execution. Defaults to
    /// [`Self::binds_cwd_authority`] because the native path binds the retained
    /// checkout directly.
    fn binds_workspace_authority(&self) -> bool {
        self.binds_cwd_authority()
    }

    /// The backend's stable hard-containment identity, or `None` when this
    /// backend can NEVER provide hard containment.
    ///
    /// The default is `None`. A backend qualifies ONLY by overriding this to
    /// return a [`HardContainmentIdentity`] — which it can construct solely
    /// because the struct's fields are crate-private. `sandbox-exec`, the
    /// no-sandbox / Dangerous runtime, the fail-closed backend, every stub, and
    /// every external backend keep this default and are therefore structurally
    /// incapable of minting hard containment, not merely refused at runtime.
    ///
    /// This is a CHEAP identity check (no spawn); it is what the authority
    /// re-derives at spawn to detect executable / runtime / mechanism drift.
    fn hard_containment_identity(&self) -> Option<HardContainmentIdentity> {
        None
    }

    /// Run a semantic LIVE probe of this backend's exact hard-containment
    /// mechanism under `fs`'s normalized policy, returning proof on success.
    ///
    /// The probe MUST exercise the real isolation mechanism (spawn a namespaced
    /// / containerized / job-object child) on a BENIGN internal command — never
    /// candidate-controlled argv, so a failed admission never runs attacker
    /// code. On ANY failure (spawn, identity, containment, timeout, output
    /// overflow, capture/wait, descendant cleanup) the backend MUST kill the
    /// complete owned process tree and return an error (fail closed).
    ///
    /// The default fails closed with `PolicyNotSupported`: a non-qualifying
    /// backend cannot probe, and — because it cannot construct a
    /// [`HardContainmentProbe`] — cannot fabricate a success either.
    async fn probe_hard_containment(
        &self,
        _fs: &HardContainmentFilesystem,
    ) -> Result<HardContainmentProbe> {
        Err(crate::SandboxError::PolicyNotSupported(format!(
            "sandbox backend {} cannot establish hard containment",
            self.name()
        )))
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
