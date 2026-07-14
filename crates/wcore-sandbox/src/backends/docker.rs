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

    async fn remove(&mut self) {
        use bollard::container::RemoveContainerOptions;

        let Some(id) = self.id.as_ref().cloned() else {
            return;
        };
        let removal = self.client.remove_container(
            &id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        );
        match tokio::time::timeout(DOCKER_CLEANUP_TIMEOUT, removal).await {
            Ok(Ok(())) => {
                // Disarm only after Docker confirms force-removal. If this
                // future is cancelled, errors, or times out, Drop retains the
                // id and schedules a detached retry.
                self.id = None;
            }
            Ok(Err(error)) => {
                tracing::warn!(
                    target: "wcore_sandbox",
                    container = %id,
                    %error,
                    "Docker sandbox force-removal failed; scheduling a retry"
                );
            }
            Err(_) => {
                tracing::warn!(
                    target: "wcore_sandbox",
                    container = %id,
                    "Docker sandbox force-removal timed out; scheduling a retry"
                );
            }
        }
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
        // Force initialisation; surface the connection error to the caller.
        backend.client_ref().await?;
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
                bollard::Docker::connect_with_local_defaults()
                    .map_err(|e| SandboxError::DockerIo(e.to_string()))
            })
            .await
    }
}

impl Default for DockerBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Cheap, cached probe for the local Docker control socket / named pipe.
/// We do NOT issue a daemon ping here — `default_for_platform()` must be
/// sync and `is_available()` is called by ordinary trait dispatch.
#[cfg(feature = "live-docker")]
fn docker_socket_present() -> bool {
    static PROBED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PROBED.get_or_init(|| {
        #[cfg(unix)]
        {
            std::path::Path::new("/var/run/docker.sock").exists()
        }
        #[cfg(windows)]
        {
            std::path::Path::new(r"\\.\pipe\docker_engine").exists()
        }
        #[cfg(not(any(unix, windows)))]
        {
            false
        }
    })
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
        cleanup.remove().await;
        let (exit_code, stdout, stderr) = result??;
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
mod tests {
    use super::*;

    #[test]
    fn backend_name_is_stable() {
        assert_eq!(DockerBackend::new().name(), "docker");
    }

    /// sandbox-4: with the `live-docker` feature OFF a docker backend can
    /// never be available and execution is refused with `DockerDisabled`
    /// rather than silently degrading. (The loud warning is emitted via
    /// `is_available`; we assert the security-relevant outcomes here.)
    #[cfg(not(feature = "live-docker"))]
    #[tokio::test]
    async fn docker_disabled_is_unavailable_and_refuses() {
        let backend = DockerBackend::new();
        assert!(
            !backend.is_available(),
            "without live-docker the backend must be unavailable"
        );
        let err = backend
            .execute(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec!["/bin/echo".into()],
                    cwd: None,
                },
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, SandboxError::DockerDisabled),
            "execute must refuse with DockerDisabled, got {err:?}"
        );
    }

    /// Task 5: without the `live-docker` feature the backend enforces nothing
    /// and must keep the trait default `false` so the exec-time capability
    /// gate remains truthful.
    #[cfg(not(feature = "live-docker"))]
    #[test]
    fn enforces_read_deny_is_false_without_live_docker() {
        assert!(
            !DockerBackend::new().enforces_read_deny(),
            "non-live-docker build must not claim to enforce read-deny"
        );
    }

    /// Task 5 (live): with the `live-docker` feature ON the backend declares
    /// it enforces `fs_read_deny`. This is a capability claim without needing
    /// a running daemon — the implementation is in `execute` and CI exercises
    /// it end-to-end.
    #[cfg(feature = "live-docker")]
    #[test]
    fn enforces_read_deny_is_true_with_live_docker() {
        assert!(
            DockerBackend::new().enforces_read_deny(),
            "live-docker build must claim to enforce read-deny"
        );
    }

    #[cfg(feature = "live-docker")]
    #[test]
    fn buffered_output_accepts_exact_limit() {
        let stdout = vec![0_u8; super::super::BUFFERED_OUTPUT_LIMIT_BYTES - 1];
        assert!(reserve_docker_output(&stdout, &[], 1).is_ok());
    }

    #[cfg(feature = "live-docker")]
    #[test]
    fn buffered_output_rejects_first_byte_over_limit() {
        let stdout = vec![0_u8; super::super::BUFFERED_OUTPUT_LIMIT_BYTES];
        assert!(matches!(
            reserve_docker_output(&stdout, &[], 1),
            Err(SandboxError::OutputLimitExceeded { limit_bytes })
                if limit_bytes == super::super::BUFFERED_OUTPUT_LIMIT_BYTES
        ));
    }

    /// Task 5 (live integration): a file that is read-allowed under a mounted
    /// root but also listed in `fs_read_deny` must read as empty inside the
    /// container (the `/dev/null` bind shadows it).
    ///
    /// Skips when the Docker daemon is unavailable — this is a live-only test.
    #[cfg(feature = "live-docker")]
    #[tokio::test]
    async fn docker_denies_read_of_secret_under_allowed_root() {
        let backend = match DockerBackend::connect().await {
            Ok(b) => b,
            Err(_) => {
                eprintln!("skip: docker daemon unavailable");
                return;
            }
        };

        // Create a temporary directory on the host containing a "secret" file.
        let workspace = tempfile::TempDir::new().expect("tempdir");
        let secret = workspace.path().join(".env");
        std::fs::write(&secret, b"SECRET=hunter2").expect("write secret");

        let manifest = SandboxManifest {
            // Allow the workspace root (so the container can see the dir).
            fs_read_allow: vec![workspace.path().to_path_buf()],
            // Deny the specific secret file inside the allowed root.
            fs_read_deny: vec![secret.clone()],
            network: NetworkPolicy::Deny,
            image: "alpine:3.19".into(),
            ..Default::default()
        };

        let out = match backend
            .execute(
                &manifest,
                SandboxCommand {
                    argv: vec!["cat".into(), secret.to_string_lossy().into_owned()],
                    cwd: None,
                },
            )
            .await
        {
            Ok(o) => o,
            Err(e) => {
                eprintln!("skip: docker execute failed ({e:?})");
                return;
            }
        };

        // The deny bind shadows .env with /dev/null — `cat /dev/null` exits 0
        // and produces empty output. Assert that secret bytes are absent.
        let output = String::from_utf8_lossy(&out.stdout);
        assert!(
            !output.contains("SECRET"),
            "secret bytes must not be readable under Docker read-deny; got: {output:?}"
        );
    }

    /// Cancellation proof for the RAII path: once a container exists, dropping
    /// the future that owns its cleanup guard must schedule force-removal.
    /// Skips if Docker or the small live-test image is unavailable.
    #[cfg(feature = "live-docker")]
    #[tokio::test]
    async fn cancelled_owner_force_removes_live_container() {
        use bollard::container::{
            Config, CreateContainerOptions, InspectContainerOptions, RemoveContainerOptions,
            StartContainerOptions,
        };

        let backend = match DockerBackend::connect().await {
            Ok(backend) => backend,
            Err(_) => {
                eprintln!("skip: docker daemon unavailable");
                return;
            }
        };
        let client = backend.client_ref().await.unwrap().clone();
        let created = match client
            .create_container(
                None::<CreateContainerOptions<String>>,
                Config {
                    image: Some("alpine:3.19".to_string()),
                    cmd: Some(vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        "sleep 30".to_string(),
                    ]),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(created) => created,
            Err(error) => {
                eprintln!("skip: live-test image unavailable ({error})");
                return;
            }
        };
        let id = created.id;
        let cleanup = ContainerCleanup::new(client.clone(), id.clone());
        client
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
            .unwrap();

        let owner = tokio::spawn(async move {
            let _cleanup = cleanup;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        owner.abort();
        let _ = owner.await;

        let mut removed = false;
        for _ in 0..50 {
            if client
                .inspect_container(&id, None::<InspectContainerOptions>)
                .await
                .is_err()
            {
                removed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if !removed {
            let _ = client
                .remove_container(
                    &id,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await;
        }
        assert!(removed, "cancelled Docker owner leaked container {id}");
    }
}
