//! Docker backend — feature-gated under `live-docker`. Default builds
//! compile the type but every method returns `SandboxError::DockerDisabled`
//! so the public surface stays stable and downstream code never needs
//! per-feature `cfg` plumbing.
//!
//! v0.6.3 migration:
//! - Implements the new `SandboxBackend::execute(&manifest, cmd)` trait.
//! - Filesystem allowlists use `Path::starts_with` (component-aware) so
//!   `/etc` does NOT match `/etcd` (Audit A M1).
//! - `NetworkPolicy::AllowHosts` returns `PolicyNotSupported` rather than
//!   silently falling through, because Docker has no DNS gate (Audit B H4).
//! - Reports `ResourceLimitEnforcement::Enforced` because `--memory` and
//!   `--cpus` are enforced by the Docker daemon / kernel cgroups.
//!
//! Lazy, cheap availability probing:
//! - `new()` is sync and does NOT contact dockerd. The client is
//!   constructed lazily on the first `execute()` call via `OnceCell`,
//!   so `default_for_platform()` can poll `is_available()` cheaply.
//! - `is_available()` probes for the docker socket / named pipe rather
//!   than issuing a network call. Real failures still surface from
//!   `execute()` if the daemon is down despite the socket existing.

use super::SandboxBackend;
#[cfg(feature = "live-docker")]
use crate::RetainedWorkspaceAuthority;
use crate::error::{Result, SandboxError};
use crate::manifest::SandboxManifest;
use crate::{SandboxCommand, SandboxOutput};
use async_trait::async_trait;

#[cfg(feature = "live-docker")]
use crate::ResourceLimitEnforcement;
#[cfg(feature = "live-docker")]
use crate::manifest::NetworkPolicy;
#[cfg(feature = "live-docker")]
use tokio::sync::OnceCell;

#[cfg(feature = "live-docker")]
const DOCKER_CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(feature = "live-docker")]
struct ContainerCleanup {
    client: bollard::Docker,
    id: Option<String>,
}

#[cfg(feature = "live-docker")]
impl ContainerCleanup {
    fn new(client: bollard::Docker, id: String) -> Self {
        Self {
            client,
            id: Some(id),
        }
    }

    async fn remove(&mut self) -> Result<()> {
        use bollard::container::RemoveContainerOptions;

        let Some(id) = self.id.as_ref().cloned() else {
            return Ok(());
        };
        let removal = self.client.remove_container(
            &id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        );
        require_container_removal(&id, DOCKER_CLEANUP_TIMEOUT, removal).await?;
        // Disarm only after Docker confirms force-removal. If this future is
        // cancelled, errors, or times out, Drop retains the id and schedules
        // a detached retry.
        self.id = None;
        Ok(())
    }
}

#[cfg(feature = "live-docker")]
async fn require_container_removal<E, F>(
    id: &str,
    timeout: std::time::Duration,
    removal: F,
) -> Result<()>
where
    E: std::fmt::Display,
    F: std::future::Future<Output = std::result::Result<(), E>>,
{
    match tokio::time::timeout(timeout, removal).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(SandboxError::DockerIo(format!(
            "force-removal of Docker sandbox {id} failed: {error}"
        ))),
        Err(_) => Err(SandboxError::DockerIo(format!(
            "force-removal of Docker sandbox {id} was not confirmed before timeout"
        ))),
    }
}

#[cfg(feature = "live-docker")]
impl Drop for ContainerCleanup {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let client = self.client.clone();
        // `execute` is async, so a Tokio handle is normally present. The
        // detached cleanup is the cancellation path: dropping the backend
        // future must still force-remove the already-created container.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                use bollard::container::RemoveContainerOptions;
                let removal = client.remove_container(
                    &id,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                );
                match tokio::time::timeout(DOCKER_CLEANUP_TIMEOUT, removal).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::error!(
                        target: "wcore_sandbox",
                        container = %id,
                        %error,
                        "Docker sandbox detached force-removal failed"
                    ),
                    Err(_) => tracing::error!(
                        target: "wcore_sandbox",
                        container = %id,
                        "Docker sandbox detached force-removal timed out"
                    ),
                }
            });
        } else {
            tracing::error!(
                target: "wcore_sandbox",
                container = %id,
                "Docker sandbox cleanup lost its Tokio runtime; force-remove this container manually"
            );
        }
    }
}

pub struct DockerBackend {
    #[cfg(feature = "live-docker")]
    client: OnceCell<bollard::Docker>,
}

impl DockerBackend {
    /// Construct a backend handle without contacting `dockerd`. The
    /// client is initialised lazily on the first `execute()` call. This
    /// keeps `default_for_platform()` (sync) and `is_available()` cheap.
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "live-docker")]
            client: OnceCell::new(),
        }
    }

    /// Eagerly connect to the Docker daemon. Useful for tests and for
    /// callers that want a fail-fast signal rather than deferring the
    /// connection error to the first `execute()`.
    #[cfg(feature = "live-docker")]
    pub async fn connect() -> Result<Self> {
        let backend = Self::new();
        // Force initialisation and a real daemon round-trip. A socket pathname
        // alone is not runtime availability (Docker Desktop commonly leaves a
        // stale socket or has not started its VM yet).
        let client = backend.client_ref().await?;
        tokio::time::timeout(std::time::Duration::from_secs(5), client.ping())
            .await
            .map_err(|_| SandboxError::DockerIo("Docker daemon ping timed out".to_owned()))?
            .map_err(|error| SandboxError::DockerIo(error.to_string()))?;
        Ok(backend)
    }

    #[cfg(not(feature = "live-docker"))]
    pub async fn connect() -> Result<Self> {
        Err(SandboxError::DockerDisabled)
    }

    #[cfg(feature = "live-docker")]
    async fn client_ref(&self) -> Result<&bollard::Docker> {
        self.client
            .get_or_try_init(|| async {
                configured_docker_client().map_err(|e| SandboxError::DockerIo(e.to_string()))
            })
            .await
    }
}

impl Default for DockerBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a client from the user's configured Docker endpoint. Docker Desktop
/// on macOS may expose only `$HOME/.docker/run/docker.sock`, while remote and
/// alternate contexts arrive through `DOCKER_HOST`.
#[cfg(feature = "live-docker")]
fn configured_docker_client() -> std::result::Result<bollard::Docker, bollard::errors::Error> {
    #[cfg(target_os = "macos")]
    if std::env::var_os("DOCKER_HOST").is_none() {
        if let Some(socket) = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .map(|home| home.join(".docker/run/docker.sock"))
            .filter(|path| path.exists())
        {
            return bollard::Docker::connect_with_socket(
                &socket.to_string_lossy(),
                bollard::docker::DEFAULT_TIMEOUT,
                bollard::API_DEFAULT_VERSION,
            );
        }
    }
    bollard::Docker::connect_with_defaults()
}

/// Cheap endpoint-presence hint for synchronous generic registry selection.
/// Security-sensitive delegated dispatch uses `connect()` and its daemon ping.
#[cfg(feature = "live-docker")]
fn docker_socket_present() -> bool {
    if std::env::var_os("DOCKER_HOST").is_some() {
        return true;
    }
    #[cfg(unix)]
    return std::path::Path::new("/var/run/docker.sock").exists()
        || cfg!(target_os = "macos")
            && std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .is_some_and(|home| home.join(".docker/run/docker.sock").exists());
    #[cfg(windows)]
    return std::path::Path::new(r"\\.\pipe\docker_engine").exists();
    #[cfg(not(any(unix, windows)))]
    false
}

#[async_trait]
impl SandboxBackend for DockerBackend {
    fn name(&self) -> &'static str {
        "docker"
    }

    #[cfg(feature = "live-docker")]
    fn is_available(&self) -> bool {
        docker_socket_present()
    }

    /// The live-docker build enforces `fs_read_deny` via `/dev/null` bind
    /// mounts (files) and empty-dir overlays (directories). The non-live
    /// build cannot enforce anything, so it must keep the trait default
    /// `false` — the exec-time capability gate in `bash.rs` depends on this
    /// being truthful.
    #[cfg(feature = "live-docker")]
    fn enforces_read_deny(&self) -> bool {
        true
    }

    #[cfg(feature = "live-docker")]
    fn owns_descendants_hard(&self) -> bool {
        true
    }

    #[cfg(feature = "live-docker")]
    fn binds_workspace_authority(&self) -> bool {
        true
    }

    #[cfg(not(feature = "live-docker"))]
    fn is_available(&self) -> bool {
        // sandbox-4: when the `live-docker` feature is compiled out, a
        // `WAYLAND_SANDBOX=docker` request can never be satisfied. Returning
        // a bare `false` made that indistinguishable from "daemon down" and
        // let selection silently degrade. Emit a loud, attributable warning
        // (once per process) so the operator learns the binary was built
        // without Docker support rather than chasing a missing daemon.
        static WARN_ONCE: std::sync::Once = std::sync::Once::new();
        WARN_ONCE.call_once(|| {
            tracing::error!(
                target: "wcore_sandbox",
                "Docker backend requested but this build was compiled WITHOUT \
                 the `live-docker` feature — the Docker sandbox is unavailable. \
                 Rebuild with `--features live-docker`, choose a different \
                 sandbox, or set WAYLAND_ALLOW_NO_SANDBOX=1 to run unsandboxed."
            );
        });
        false
    }

    #[cfg(feature = "live-docker")]
    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        use bollard::container::{
            Config, CreateContainerOptions, LogOutput, LogsOptions, StartContainerOptions,
            WaitContainerOptions,
        };
        use bollard::models::HostConfig;
        use futures::stream::StreamExt;

        // Enforce filesystem allowlist before issuing any Docker calls.
        // Read-allowed paths bind read-only; write-allowed paths bind rw.
        // Audit A M1: paths must be absolute (component-aware checks in
        // future overlap logic use `Path::starts_with`, not string prefix).
        let mut binds: Vec<String> = Vec::new();
        for ro in &manifest.fs_read_allow {
            if !ro.is_absolute() {
                return Err(SandboxError::PathDenied(format!(
                    "fs_read_allow entry not absolute: {}",
                    ro.display()
                )));
            }
            // Skip if this read path is also in fs_write_allow — write
            // subsumes read for the same path, and Docker rejects duplicate
            // binds. We compare full Path equality, not string prefix.
            let shadowed = manifest
                .fs_write_allow
                .iter()
                .any(|rw| rw.as_path() == ro.as_path());
            if shadowed {
                continue;
            }
            binds.push(format!("{}:{}:ro", ro.display(), ro.display()));
        }
        for rw in &manifest.fs_write_allow {
            if !rw.is_absolute() {
                return Err(SandboxError::PathDenied(format!(
                    "fs_write_allow entry not absolute: {}",
                    rw.display()
                )));
            }
            binds.push(format!("{}:{}:rw", rw.display(), rw.display()));
        }

        // Secret-read-deny: shadow each denied path. Caller emits only paths
        // under a mounted root, so the bind target's parent exists. /dev/null
        // for files; an empty read-only tmpfs is not expressible via -v, so
        // for directories bind an empty host dir read-only.
        //
        // `empty_dir` is a TempDir bound to a local that lives until AFTER
        // the container is removed (≈ remove_container below) so the directory
        // exists on the host for the entire lifetime of the container bind.
        let empty_dir = if manifest
            .fs_read_deny
            .iter()
            .any(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()))
        {
            Some(
                tempfile::TempDir::new()
                    .map_err(|e| SandboxError::ExecFailed(format!("tempdir for deny: {e}")))?,
            )
        } else {
            None
        };
        for p in &manifest.fs_read_deny {
            // Skip if the deny path exactly matches an existing allow bind —
            // Docker rejects duplicate-bind entries for the same target path.
            let already_bound = manifest
                .fs_read_allow
                .iter()
                .any(|a| a.as_path() == p.as_path())
                || manifest
                    .fs_write_allow
                    .iter()
                    .any(|a| a.as_path() == p.as_path());
            if already_bound {
                continue;
            }
            match std::fs::symlink_metadata(p) {
                Ok(md) if md.is_dir() => {
                    // Mask a denied dir by binding an empty, ephemeral dir
                    // read-only. Docker has no tmpfs-over-existing-bind.
                    let dir = empty_dir
                        .as_ref()
                        .expect("empty_dir constructed above when a dir deny exists");
                    binds.push(format!("{}:{}:ro", dir.path().display(), p.display()));
                }
                Ok(_) => binds.push(format!("/dev/null:{}:ro", p.display())),
                Err(_) => { /* path gone since enumeration — nothing to mask */ }
            }
        }

        // Network policy.
        let network_mode = match &manifest.network {
            NetworkPolicy::Inherit => None,
            NetworkPolicy::Deny => Some("none".to_string()),
            NetworkPolicy::AllowHosts(_) => {
                return Err(SandboxError::PolicyNotSupported(
                    "Docker backend has no DNS gate for AllowHosts; \
                     use bubblewrap with a TCP egress filter instead"
                        .into(),
                ));
            }
        };

        // Resource limits (Docker enforces these via cgroups).
        let memory = manifest.max_memory_bytes.map(|b| b as i64);
        // `nano_cpus` is fractional CPU * 1e9. We map max_cpu_secs as a
        // CPU-quota proxy: 1 "cpu second per wallclock second" == 1.0 CPU.
        // For now, only pass nano_cpus when max_cpu_secs is set (interpret
        // as "this many vCPUs" — matches the v0.6.2 semantics where
        // `cpu_quota` was already a fractional CPU count).
        let nano_cpus = manifest.max_cpu_secs.map(|s| (s as i64) * 1_000_000_000);

        // env: scrubbed by default — only what the manifest declared.
        let env_pairs: Vec<String> = manifest
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();

        let host_config = HostConfig {
            binds: if binds.is_empty() { None } else { Some(binds) },
            network_mode,
            memory,
            nano_cpus,
            ..Default::default()
        };
        let working_dir = cmd.cwd.as_ref().map(|p| p.display().to_string());
        let config = Config {
            image: Some(manifest.image.clone()),
            cmd: Some(cmd.argv.clone()),
            env: if env_pairs.is_empty() {
                None
            } else {
                Some(env_pairs)
            },
            working_dir,
            host_config: Some(host_config),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };
        let client = self.client_ref().await?;
        let created = client
            .create_container(None::<CreateContainerOptions<String>>, config)
            .await
            .map_err(|e| SandboxError::DockerIo(e.to_string()))?;
        let id = created.id;
        let mut cleanup = ContainerCleanup::new(client.clone(), id.clone());
        client
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| SandboxError::DockerIo(e.to_string()))?;
        let mut wait = client.wait_container(&id, None::<WaitContainerOptions<String>>);
        let mut logs = client.logs(
            &id,
            Some(LogsOptions::<String> {
                follow: true,
                stdout: true,
                stderr: true,
                ..Default::default()
            }),
        );
        let timeout = manifest
            .timeout
            .unwrap_or_else(|| std::time::Duration::from_secs(60));
        let execution = async {
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr: Vec<u8> = Vec::new();
            let mut exit_code = None;
            let mut logs_done = false;

            while exit_code.is_none() || !logs_done {
                tokio::select! {
                    waited = wait.next(), if exit_code.is_none() => {
                        exit_code = Some(match waited {
                            Some(Ok(response)) => response.status_code as i32,
                            Some(Err(error)) => {
                                return Err(SandboxError::DockerIo(error.to_string()));
                            }
                            None => -1,
                        });
                    }
                    chunk = logs.next(), if !logs_done => {
                        match chunk {
                            Some(Ok(LogOutput::StdOut { message })) => {
                                reserve_docker_output(&stdout, &stderr, message.len())?;
                                stdout.extend_from_slice(&message);
                            }
                            Some(Ok(LogOutput::StdErr { message })) => {
                                reserve_docker_output(&stdout, &stderr, message.len())?;
                                stderr.extend_from_slice(&message);
                            }
                            Some(Ok(_)) => {}
                            Some(Err(error)) => {
                                return Err(SandboxError::DockerIo(error.to_string()));
                            }
                            None => logs_done = true,
                        }
                    }
                }
            }

            Ok((exit_code.unwrap_or(-1), stdout, stderr))
        };
        let result = tokio::time::timeout(timeout, execution)
            .await
            .map_err(|_| SandboxError::Timeout);
        cleanup.remove().await?;
        let (exit_code, stdout, stderr) = result??;
        Ok(SandboxOutput {
            exit_code,
            stdout,
            stderr,
            resource_limits: ResourceLimitEnforcement::Enforced,
        })
    }

    #[cfg(feature = "live-docker")]
    async fn execute_with_workspace_authority(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
        workspace: RetainedWorkspaceAuthority,
        max_workspace_bytes: u64,
        reauthorize: &(dyn Fn() -> Result<()> + Send + Sync),
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<SandboxOutput> {
        use bollard::container::{
            Config, CreateContainerOptions, DownloadFromContainerOptions, LogOutput, LogsOptions,
            StartContainerOptions, UploadToContainerOptions, WaitContainerOptions,
        };
        use bollard::models::HostConfig;
        use futures::stream::StreamExt;
        use std::collections::HashMap;

        let limits = crate::directory_authority::archive::DirectoryArchiveLimits {
            max_entries: 100_000,
            max_bytes: max_workspace_bytes,
            max_depth: 128,
        };
        if cancel.is_cancelled() {
            return Err(SandboxError::ExecFailed(
                "Docker workspace execution cancelled".into(),
            ));
        }
        workspace.recover_pending_import(limits)?;
        let plan = retained_container_plan(manifest, cmd, &workspace)?;
        let source = workspace.export_tar_bounded("workspace", &plan.denied, limits)?;
        let network_mode = match &manifest.network {
            NetworkPolicy::Inherit => None,
            NetworkPolicy::Deny => Some("none".to_owned()),
            NetworkPolicy::AllowHosts(_) => {
                return Err(SandboxError::PolicyNotSupported(
                    "Docker backend has no DNS gate for AllowHosts".to_owned(),
                ));
            }
        };
        let mut tmpfs = HashMap::new();
        tmpfs.insert(
            "/scratch".to_owned(),
            format!("rw,nosuid,nodev,size={max_workspace_bytes}"),
        );
        let config = Config {
            image: Some(manifest.image.clone()),
            cmd: Some(plan.argv),
            env: (!plan.env.is_empty()).then_some(plan.env),
            working_dir: Some("/workspace".to_owned()),
            host_config: Some(HostConfig {
                binds: None,
                tmpfs: Some(tmpfs),
                network_mode,
                memory: manifest.max_memory_bytes.map(|bytes| bytes as i64),
                nano_cpus: manifest
                    .max_cpu_secs
                    .map(|seconds| (seconds as i64) * 1_000_000_000),
                ..Default::default()
            }),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };
        let client = self.client_ref().await?;
        let created = client
            .create_container(None::<CreateContainerOptions<String>>, config)
            .await
            .map_err(|error| SandboxError::DockerIo(error.to_string()))?;
        let id = created.id;
        let mut cleanup = ContainerCleanup::new(client.clone(), id.clone());
        let timeout = manifest
            .timeout
            .unwrap_or_else(|| std::time::Duration::from_secs(60));
        let execution = async {
            client
                .upload_to_container(
                    &id,
                    Some(UploadToContainerOptions {
                        path: "/",
                        no_overwrite_dir_non_dir: "true",
                    }),
                    source.into(),
                )
                .await
                .map_err(|error| SandboxError::DockerIo(error.to_string()))?;
            client
                .start_container(&id, None::<StartContainerOptions<String>>)
                .await
                .map_err(|error| SandboxError::DockerIo(error.to_string()))?;
            let mut wait = client.wait_container(&id, None::<WaitContainerOptions<String>>);
            let mut logs = client.logs(
                &id,
                Some(LogsOptions::<String> {
                    follow: true,
                    stdout: true,
                    stderr: true,
                    ..Default::default()
                }),
            );
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut exit_code = None;
            let mut logs_done = false;
            while exit_code.is_none() || !logs_done {
                tokio::select! {
                    waited = wait.next(), if exit_code.is_none() => {
                        exit_code = Some(match waited {
                            Some(Ok(response)) => response.status_code as i32,
                            Some(Err(error)) => return Err(SandboxError::DockerIo(error.to_string())),
                            None => -1,
                        });
                    }
                    chunk = logs.next(), if !logs_done => match chunk {
                        Some(Ok(LogOutput::StdOut { message })) => {
                            reserve_docker_output(&stdout, &stderr, message.len())?;
                            stdout.extend_from_slice(&message);
                        }
                        Some(Ok(LogOutput::StdErr { message })) => {
                            reserve_docker_output(&stdout, &stderr, message.len())?;
                            stderr.extend_from_slice(&message);
                        }
                        Some(Ok(_)) => {}
                        Some(Err(error)) => return Err(SandboxError::DockerIo(error.to_string())),
                        None => logs_done = true,
                    }
                }
            }
            let mut downloaded = Vec::new();
            let mut stream = client.download_from_container(
                &id,
                Some(DownloadFromContainerOptions { path: "/workspace" }),
            );
            let encoded_limit = limits.encoded_limit()?;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| SandboxError::DockerIo(error.to_string()))?;
                let next = downloaded.len().checked_add(chunk.len()).ok_or_else(|| {
                    SandboxError::PathDenied("Docker archive length overflowed".to_owned())
                })?;
                if next as u64 > encoded_limit {
                    return Err(SandboxError::PathDenied(format!(
                        "Docker workspace result exceeds {encoded_limit} encoded bytes"
                    )));
                }
                downloaded.extend_from_slice(&chunk);
            }
            Ok((exit_code.unwrap_or(-1), stdout, stderr, downloaded))
        };
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(SandboxError::ExecFailed(
                "Docker workspace execution cancelled".to_owned()
            )),
            result = tokio::time::timeout(timeout, execution) => {
                result.map_err(|_| SandboxError::Timeout)
            }
        };
        cleanup.remove().await?;
        let (exit_code, stdout, stderr, downloaded) = result??;
        reauthorize()?;
        workspace.validate()?;
        workspace.replace_from_tar_bounded(&downloaded, "workspace", limits)?;
        reauthorize()?;
        workspace.validate()?;
        Ok(SandboxOutput {
            exit_code,
            stdout,
            stderr,
            resource_limits: ResourceLimitEnforcement::Enforced,
        })
    }

    #[cfg(not(feature = "live-docker"))]
    async fn execute(
        &self,
        _manifest: &SandboxManifest,
        _cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        Err(SandboxError::DockerDisabled)
    }
}

#[cfg(feature = "live-docker")]
#[derive(Debug)]
pub(super) struct RetainedContainerPlan {
    pub(super) argv: Vec<String>,
    pub(super) env: Vec<String>,
    pub(super) denied: Vec<std::path::PathBuf>,
}

#[cfg(feature = "live-docker")]
pub(super) fn retained_container_plan(
    manifest: &SandboxManifest,
    cmd: SandboxCommand,
    authority: &RetainedWorkspaceAuthority,
) -> Result<RetainedContainerPlan> {
    let workspace = authority.workspace().display_path();
    if cmd.cwd.as_deref() != Some(workspace) {
        return Err(SandboxError::PathDenied(
            "Docker command cwd does not match retained workspace".to_owned(),
        ));
    }
    let scratch = manifest
        .fs_write_allow
        .iter()
        .find(|path| path.as_path() != workspace)
        .cloned();
    for allowed in manifest
        .fs_read_allow
        .iter()
        .chain(&manifest.fs_write_allow)
    {
        if allowed.as_path() != workspace && Some(allowed) != scratch.as_ref() {
            return Err(SandboxError::PathDenied(format!(
                "Docker retained transport refuses ambient host grant: {}",
                allowed.display()
            )));
        }
    }
    let mut denied = Vec::new();
    for path in &manifest.fs_read_deny {
        if path.as_path() == workspace {
            return Err(SandboxError::PathDenied(
                "Docker retained transport cannot deny its workspace root".to_owned(),
            ));
        }
        if let Ok(relative) = path.strip_prefix(workspace) {
            if relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
            {
                denied.push(relative.to_path_buf());
            } else {
                return Err(SandboxError::PathDenied(format!(
                    "Docker deny path is not canonical: {}",
                    path.display()
                )));
            }
        }
    }
    let rewrite = |value: String, program: bool| -> Result<String> {
        let path = std::path::Path::new(&value);
        if let Ok(relative) = path.strip_prefix(workspace) {
            if relative.as_os_str().is_empty() {
                return Ok("/workspace".to_owned());
            }
            return Ok(std::path::Path::new("/workspace")
                .join(relative)
                .to_string_lossy()
                .into_owned());
        }
        if let Some(root) = scratch.as_deref()
            && let Ok(relative) = path.strip_prefix(root)
        {
            if relative.as_os_str().is_empty() {
                return Ok("/scratch".to_owned());
            }
            return Ok(std::path::Path::new("/scratch")
                .join(relative)
                .to_string_lossy()
                .into_owned());
        }
        if program && path.is_absolute() {
            return Err(SandboxError::PathDenied(
                "Docker retained worker executable must be supplied by the image or workspace"
                    .to_owned(),
            ));
        }
        Ok(value)
    };
    let mut argv = Vec::with_capacity(cmd.argv.len());
    for (index, value) in cmd.argv.into_iter().enumerate() {
        argv.push(rewrite(value, index == 0)?);
    }
    let mut env = Vec::with_capacity(manifest.env.len());
    for (name, value) in &manifest.env {
        env.push(format!("{name}={}", rewrite(value.clone(), false)?));
    }
    Ok(RetainedContainerPlan { argv, env, denied })
}

#[cfg(feature = "live-docker")]
fn reserve_docker_output(stdout: &[u8], stderr: &[u8], amount: usize) -> Result<()> {
    let next = stdout
        .len()
        .checked_add(stderr.len())
        .and_then(|bytes| bytes.checked_add(amount));
    if next.is_none_or(|bytes| bytes > super::BUFFERED_OUTPUT_LIMIT_BYTES) {
        return Err(SandboxError::OutputLimitExceeded {
            limit_bytes: super::BUFFERED_OUTPUT_LIMIT_BYTES,
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "docker_tests.rs"]
mod tests;
