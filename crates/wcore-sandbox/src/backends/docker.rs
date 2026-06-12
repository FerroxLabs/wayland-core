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
            Config, CreateContainerOptions, LogOutput, LogsOptions, RemoveContainerOptions,
            StartContainerOptions, WaitContainerOptions,
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
        client
            .start_container(&id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| SandboxError::DockerIo(e.to_string()))?;
        let mut wait = client.wait_container(&id, None::<WaitContainerOptions<String>>);
        let exit_code: i32 = match wait.next().await {
            Some(Ok(resp)) => resp.status_code as i32,
            Some(Err(e)) => return Err(SandboxError::DockerIo(e.to_string())),
            None => -1,
        };
        let mut logs = client.logs(
            &id,
            Some(LogsOptions::<String> {
                stdout: true,
                stderr: true,
                ..Default::default()
            }),
        );
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        while let Some(chunk) = logs.next().await {
            match chunk {
                Ok(LogOutput::StdOut { message }) => stdout.extend_from_slice(&message),
                Ok(LogOutput::StdErr { message }) => stderr.extend_from_slice(&message),
                Ok(_) => {}
                Err(e) => return Err(SandboxError::DockerIo(e.to_string())),
            }
        }
        let _ = client
            .remove_container(
                &id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;
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
}
