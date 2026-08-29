//! The CONTAINER reference backend.
//!
//! Availability is answered from a REAL DAEMON PING, never from socket
//! presence. `wcore_sandbox::backends::docker::DockerBackend::is_available()`
//! documents that it only answers `docker_socket_present()` — DOCKER_HOST set,
//! or the unix socket / named pipe existing — and points security-sensitive
//! callers at `connect()`, which does a real daemon round trip. A stale socket
//! left behind by a stopped daemon would otherwise let this backend advertise
//! readiness it does not have, and shipping that is shipping a capability lie.
//!
//! Every container this backend creates carries `wayland.task.nonce` as a
//! label, so plan 25-04's orphan scan is one `docker ps --filter label=` away
//! from an answer instead of a guess.

use async_trait::async_trait;

use crate::contract::{
    Availability, BackendCapabilities, BackendKind, CleanupObservation, ExecutionBackend,
    ExecutionTask, Health, HibernationObservation, OrphanScan, ProbeBasis, ResourceBudget,
    SecretChannel, validate_identifier,
};
use crate::error::{ExecError, Result};
use crate::policy::{EffectivePolicy, declared_secret_exposure};
use crate::receipt::{BackendIdentity, ExecutionReceipt, ReceiptSigner};
use crate::registry::{self, LiveTask, now_unix_ms};

use super::local::{cancel_marker_taken, instance_id, write_cancel_marker};
use super::{
    RunOutcome, denial_receipt, load_or_create_seed, materialize_workspace, outcome_receipt,
    pre_acceptance_denial,
};

pub const BACKEND_ID: &str = "container";
pub const NONCE_LABEL: &str = "wayland.task.nonce";
const DEFAULT_IMAGE: &str = "docker.io/library/busybox:1.36";

/// The marker docker puts on its OWN diagnostics, as distinct from anything
/// the contained process writes. See `classify_exit` for why a marker is
/// needed at all.
const DAEMON_ERROR_MARKER: &str = "Error response from daemon:";

pub struct ContainerBackend {
    capabilities: BackendCapabilities,
    identity: BackendIdentity,
    signer: ReceiptSigner,
    image: String,
}

impl ContainerBackend {
    pub fn new(limits: ResourceBudget) -> Result<Self> {
        let seed = load_or_create_seed(BACKEND_ID)?;
        let signer = ReceiptSigner::from_seed(seed);
        let image =
            std::env::var("WAYLAND_EXEC_CONTAINER_IMAGE").unwrap_or_else(|_| DEFAULT_IMAGE.into());
        Ok(Self {
            capabilities: BackendCapabilities {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Container,
                version: env!("CARGO_PKG_VERSION").into(),
                limits,
                supports_artifact_transfer: true,
                supports_cancellation: true,
                supports_hibernation: false,
                secret_channel: SecretChannel::ContainerEnv,
            },
            identity: BackendIdentity {
                backend_id: BACKEND_ID.into(),
                instance_id: instance_id(),
                version: env!("CARGO_PKG_VERSION").into(),
                key_id: signer.key_id().to_string(),
            },
            signer,
            image,
        })
    }

    pub fn identity(&self) -> &BackendIdentity {
        &self.identity
    }

    pub fn verifying_key(&self) -> ed25519_dalek::VerifyingKey {
        self.signer.verifying_key()
    }

    fn container_name(task_id: &str) -> String {
        format!("wayland-f25-{task_id}")
    }
}

/// A real daemon round trip, with a bound so an unreachable daemon cannot hang
/// `backend list`.
async fn daemon_ping() -> std::result::Result<String, String> {
    let mut command = wcore_config::shell::shell_command_argv(
        "docker",
        &["version", "--format", "{{.Server.Version}}"],
    );
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let fut = command.output();
    match tokio::time::timeout(std::time::Duration::from_secs(5), fut).await {
        Err(_) => Err("docker daemon did not answer a version ping within 5s".into()),
        Ok(Err(e)) => Err(format!("docker client could not be launched: {e}")),
        Ok(Ok(output)) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
        Ok(Ok(output)) => Err(format!(
            "docker daemon refused the version ping: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// What a `docker run` exit code actually means.
///
/// Docker overloads three exit codes across two completely different
/// authorities, and this enum is where that overload is resolved ONCE.
///
/// MEASURED on docker 29.2.1 (issue #365), which is also why the exit code
/// ALONE is not a sound discriminator:
///
/// | invocation                          | exit | docker's own stderr |
/// |-------------------------------------|------|---------------------|
/// | name conflict                       | 125  | yes                 |
/// | `--memory 1` (below the daemon min) | 125  | yes                 |
/// | image absent                        | 125  | yes                 |
/// | `sh -c 'exit 125'`                  | 125  | NO                  |
/// | argv is a directory                 | 126  | yes                 |
/// | argv not on PATH                    | 127  | yes                 |
/// | `sh -c 'exit 126'` / `'exit 127'`   | 126/127 | NO               |
///
/// A contained process is free to exit 125 of its own accord, so keying on
/// the code alone would reclassify a run that really happened as one that
/// never did. In a subsystem whose product is a signed attestation that is the
/// exact inverse of the reported defect and just as much a lie. The
/// discriminator is therefore the CONJUNCTION of the reserved code and
/// docker's own error line on stderr.
///
/// What that conjunction costs: a contained process that both exits 125 AND
/// prints docker's error line has its run recorded as "did not execute". That
/// is a strictly WEAKER claim than "ran and succeeded", so the forgery cannot
/// manufacture a success attestation — it can only disclaim its own run. For a
/// fail-closed attestation surface that is the correct direction to be wrong
/// in, and it is the reason stderr shape is used to NARROW a daemon verdict
/// and never to widen one.
#[derive(Debug, PartialEq, Eq)]
enum DockerExit {
    /// The daemon refused before any of the task's code ran. There is nothing
    /// to attest: no container was created under our nonce, no argv executed,
    /// and no workspace was read. This must NEVER become a receipt.
    DaemonRefusal(String),
    /// The contained process's own terminal status, 126 and 127 included.
    TaskExit(i32),
}

/// Resolve a `docker run` result to the authority that produced it.
fn classify_exit(exit_code: i32, stderr: &[u8]) -> DockerExit {
    let text = String::from_utf8_lossy(stderr);
    let daemon_spoke = text.contains(DAEMON_ERROR_MARKER);
    match exit_code {
        // 125 is the ONLY code docker reserves for its own client/daemon
        // layer. With docker's error line present, nothing of the task ran.
        125 if daemon_spoke => DockerExit::DaemonRefusal(daemon_diagnostic(&text)),
        // 125 without docker's error line is the contained process's own
        // status. Docker does not claim this code exclusively at runtime, and
        // treating it as a refusal here would erase a run that happened.
        125 => DockerExit::TaskExit(125),
        // 126 and 127 are the CONTAINER's, not the daemon's, even though the
        // daemon is what reports them: they say the argv the TASK submitted
        // was found-but-not-executable / not found at all. That is a property
        // of task input, not of this backend's infrastructure, and
        // `Failure { code: "exit-126" }` is the conventional truthful encoding
        // of it — the same one every POSIX shell uses. Reclassifying them as
        // an unavailable BACKEND would make a task with a typo in its argv
        // read as a broken transport, which is the inverse defect and would
        // make the backend undiagnosable. They stay task exits; what they
        // gained in this pass is that their stderr now reaches the operator
        // through the receipt path rather than being the bare code alone.
        126 | 127 => DockerExit::TaskExit(exit_code),
        other => DockerExit::TaskExit(other),
    }
}

/// Pull docker's own diagnostic lines out of a mixed stderr.
///
/// The whole of stderr is not used: on a refusal docker appends
/// `Run 'docker run --help' for more information`, which is noise in an
/// operator-facing error. The lines carrying the marker are the ones that name
/// the problem and the remedy.
fn daemon_diagnostic(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| line.contains(DAEMON_ERROR_MARKER))
        .collect();
    if lines.is_empty() {
        return stderr.trim().to_string();
    }
    lines.join(" / ")
}

/// Whatever container currently holds a name a submit is about to take.
#[derive(Debug, PartialEq, Eq)]
struct NameHolder {
    running: bool,
    /// The `wayland.task.nonce` label. Empty means the holder carries none,
    /// which means this backend did not create it.
    nonce: String,
}

/// Read the state and nonce label of whatever holds `name`, in one round trip.
///
/// `Ok(None)` means the name is free. That is the common path and it mutates
/// nothing.
async fn inspect_name_holder(name: &str) -> std::result::Result<Option<NameHolder>, String> {
    let format = format!("{{{{.State.Running}}}}|{{{{index .Config.Labels \"{NONCE_LABEL}\"}}}}");
    let mut command =
        wcore_config::shell::shell_command_argv("docker", &["inspect", name, "--format", &format]);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| "docker inspect did not answer within 10s".to_string())?
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        // `no such object` is the answer "the name is free", not a failure.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such object") || stderr.contains("no such object") {
            return Ok(None);
        }
        return Err(stderr.trim().to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim();
    let (running, nonce) = line
        .split_once('|')
        .ok_or_else(|| format!("docker inspect returned an unparsable line: {line:?}"))?;
    Ok(Some(NameHolder {
        running: running.trim() == "true",
        nonce: nonce.trim().to_string(),
    }))
}

/// Clear a leftover container holding the name this submit needs (issue #365).
///
/// DESIGN — why the submit path clears the name rather than the name carrying
/// the nonce. The two candidate fixes are not equivalent, and two existing
/// call sites decide it:
///
/// * `cancel()` reconstructs the name from the task id ALONE when the registry
///   entry has no handle (`entry.handle.unwrap_or_else(|| container_name(id))`).
///   A nonce-bearing name is not reconstructable from a task id, so that
///   fallback would build a name no container ever had, `docker rm -f` would
///   quietly no-op, and `cancel()` would still report a cleanup it did not
///   perform. Keeping the name a pure function of the task id keeps that
///   fallback honest.
/// * `scan_orphans()` enumerates by LABEL, never by name, so it is indifferent
///   to the naming scheme itself. But a nonce-bearing name would leave every
///   wedged container in place forever: the scan would report a growing pile
///   of orphans that nothing in the product can reclaim, trading a loud,
///   fixable conflict for a silent unbounded leak on every operator's machine.
///   Submit time is the one moment at which removal can be PROVEN safe.
///
/// The removal is guarded so it cannot take a container that is not ours or
/// that is still doing work:
///
/// * a RUNNING holder is never removed. Under a deterministic name that is a
///   live task with the same id — possibly another tenant's — and removing it
///   would destroy real work.
/// * a holder with no `wayland.task.nonce` label is never removed. This
///   backend labels every container it creates, so an unlabelled holder
///   belongs to somebody else and merely collided with our name.
///
/// Note the label KEY is what is checked, not a match against this task's
/// nonce: the leftover being cleared is by definition from an EARLIER run,
/// which carries an earlier nonce. Requiring an exact nonce match would refuse
/// to clear precisely the case this exists for.
///
/// Both refusals return `Unavailable` naming the holder, rather than falling
/// through into a `docker run` that would fail with exit 125 anyway.
///
/// The removal uses plain `docker rm`, never `rm -f`. If the holder starts
/// between the inspect and the removal, `docker rm` fails rather than killing
/// it — the TOCTOU window closes into a refusal instead of into a casualty.
async fn reclaim_container_name(name: &str) -> Result<()> {
    let holder = match inspect_name_holder(name).await {
        Ok(None) => return Ok(()),
        Ok(Some(holder)) => holder,
        Err(detail) => {
            return Err(ExecError::Unavailable {
                backend_id: BACKEND_ID.into(),
                detail: format!("could not inspect the container named {name}: {detail}"),
            });
        }
    };

    if holder.running {
        return Err(ExecError::Unavailable {
            backend_id: BACKEND_ID.into(),
            detail: format!(
                "a RUNNING container is already named {name} (task nonce {:?}); \
                 refusing to remove a live task's container",
                holder.nonce
            ),
        });
    }
    if holder.nonce.is_empty() {
        return Err(ExecError::Unavailable {
            backend_id: BACKEND_ID.into(),
            detail: format!(
                "the name {name} is held by a container carrying no {NONCE_LABEL} label, \
                 so it was not created by this backend; refusing to remove another owner's container"
            ),
        });
    }

    let mut command = wcore_config::shell::shell_command_argv("docker", &["rm", name]);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .map_err(|_| ExecError::Unavailable {
            backend_id: BACKEND_ID.into(),
            detail: format!("docker rm {name} did not answer within 30s"),
        })?
        .map_err(|e| ExecError::Exec(e.to_string()))?;
    if !output.status.success() {
        return Err(ExecError::Unavailable {
            backend_id: BACKEND_ID.into(),
            detail: format!(
                "could not reclaim the leftover container named {name} (nonce {}): {}",
                holder.nonce,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(())
}

#[async_trait]
impl ExecutionBackend for ContainerBackend {
    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn availability(&self) -> Availability {
        match daemon_ping().await {
            Ok(version) => Availability::up(
                ProbeBasis::DaemonPing,
                format!("container daemon answered a version ping: server {version}"),
            ),
            Err(detail) => Availability::down(ProbeBasis::DaemonPing, detail),
        }
    }

    fn effective_policy(&self, task: &ExecutionTask) -> Result<EffectivePolicy> {
        let (egress_decision, egress_source) = crate::policy::observed_egress_decision();
        let policy = EffectivePolicy {
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Container,
            egress_decision,
            egress_source,
            secret_channel: SecretChannel::ContainerEnv,
            secrets_exposed: declared_secret_exposure(BackendKind::Container, task),
            containment: format!(
                "container image {} with --network none and a per-task nonce label",
                self.image
            ),
        };
        policy.validate()?;
        Ok(policy)
    }

    async fn execute(&self, task: &ExecutionTask) -> Result<ExecutionReceipt> {
        task.validate()?;
        let policy = self.effective_policy(task)?;
        if let Some(denial) = pre_acceptance_denial(task, &self.capabilities) {
            return denial_receipt(
                task,
                &self.capabilities,
                &self.identity,
                &self.signer,
                &policy,
                denial,
            );
        }

        let availability = self.availability().await;
        if !availability.available {
            return Err(ExecError::Unavailable {
                backend_id: BACKEND_ID.into(),
                detail: availability.detail,
            });
        }

        let name = Self::container_name(&task.task_id);
        validate_identifier("container_name", &name)?;
        // Issue #365: `docker run --rm` removes on EXIT, so a container that
        // reached `Created` and never started is never removed and latches
        // this name forever. Clear it here, under the guards documented on
        // `reclaim_container_name` — BEFORE the workspace is materialised and
        // before the name is committed to the registry, so a refusal leaves
        // neither a stray work directory nor a registry entry behind.
        reclaim_container_name(&name).await?;

        let workdir = registry::state_dir().join("work").join(&task.task_id);
        materialize_workspace(&workdir, task)?;
        let started = now_unix_ms();

        registry::record(&LiveTask {
            task_id: task.task_id.clone(),
            nonce: task.nonce.clone(),
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Container,
            pid: None,
            handle: Some(name.clone()),
            started_unix_ms: started,
        })?;

        let mount = format!("{}:/task", workdir.display());
        let label = format!("{NONCE_LABEL}={}", task.nonce);
        let memory = format!("{}b", task.resources.memory_bytes);
        // ARGV MODE throughout. Nothing here is interpolated into a shell
        // string, so a workspace path carrying a metacharacter cannot reach an
        // interpreter on either side of the container boundary.
        let mut args: Vec<String> = vec![
            "run".into(),
            "--rm".into(),
            "--name".into(),
            name.clone(),
            "--label".into(),
            label,
            "--network".into(),
            "none".into(),
            "--memory".into(),
            memory,
            "--workdir".into(),
            "/task".into(),
            "--volume".into(),
            mount,
            "--env".into(),
            format!("WAYLAND_TASK_NONCE={}", task.nonce),
            self.image.clone(),
        ];
        args.extend(task.argv.iter().cloned());
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();

        let mut command = wcore_config::shell::shell_command_argv("docker", &borrowed);
        // Null stdin, never inherited — see the note in backends/local.rs.
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let wall = std::time::Duration::from_millis(task.resources.wall_time_ms);
        let output = match tokio::time::timeout(wall, command.output()).await {
            Ok(result) => result.map_err(|e| ExecError::Exec(e.to_string()))?,
            Err(_) => {
                let _ = self.cancel(&task.task_id).await;
                return Err(ExecError::Exec(format!(
                    "container task {} exceeded its {}ms wall clock",
                    task.task_id, task.resources.wall_time_ms
                )));
            }
        };

        let finished = now_unix_ms();

        // Issue #365: resolve WHOSE exit this is before anything is attested.
        // A daemon refusal never becomes a receipt — there is no run to sign a
        // statement about — and its diagnostic is returned as the error value,
        // which the CLI prints unconditionally to stderr. It is deliberately
        // NOT a `warn!`: RUST_LOG is unset for ordinary operators, so only
        // ERROR reaches stderr and a log-level bump could never make the user
        // told.
        let exit_code = match classify_exit(output.status.code().unwrap_or(-1), &output.stderr) {
            DockerExit::DaemonRefusal(detail) => {
                // Nothing was created under our nonce, so there is nothing to
                // remove; the registry entry and the workspace are still ours.
                registry::forget(&task.task_id)?;
                let _ = std::fs::remove_dir_all(&workdir);
                return Err(ExecError::Unavailable {
                    backend_id: BACKEND_ID.into(),
                    detail: format!(
                        "the container daemon refused to run task {}: {detail}",
                        task.task_id
                    ),
                });
            }
            DockerExit::TaskExit(code) => code,
        };

        let cancelled = cancel_marker_taken(&task.task_id);
        registry::forget(&task.task_id)?;
        let _ = std::fs::remove_dir_all(&workdir);

        outcome_receipt(
            task,
            &self.capabilities,
            &self.identity,
            &self.signer,
            &policy,
            RunOutcome {
                stdout: output.stdout,
                stderr: output.stderr,
                exit_code,
                endpoint: name,
                cancelled,
                hibernation: HibernationObservation::NotApplicable,
                started_unix_ms: started,
                finished_unix_ms: finished,
            },
        )
    }

    async fn cancel(&self, task_id: &str) -> Result<CleanupObservation> {
        let entry = registry::load(task_id)?;
        write_cancel_marker(task_id, "operator cancelled")?;
        let name = entry
            .handle
            .clone()
            .unwrap_or_else(|| Self::container_name(task_id));
        let mut command = wcore_config::shell::shell_command_argv("docker", &["rm", "-f", &name]);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let _ = command.output().await;

        // The observation is what the daemon says AFTER the removal, not what
        // the removal returned.
        let residual = match list_containers_with_nonce(&entry.nonce).await {
            Ok(found) => found,
            Err(detail) => vec![format!("could not re-enumerate containers: {detail}")],
        };
        registry::forget(task_id)?;
        Ok(CleanupObservation {
            task_id: task_id.into(),
            backend_id: BACKEND_ID.into(),
            method: format!(
                "docker rm -f, then docker ps -a --filter label={NONCE_LABEL}=<nonce> re-read"
            ),
            residual,
        })
    }

    async fn health(&self) -> Result<Health> {
        let availability = self.availability().await;
        let live = registry::list()
            .into_iter()
            .filter(|t| t.backend_id == BACKEND_ID)
            .count();
        Ok(Health {
            healthy: availability.available,
            detail: availability.detail,
            live_tasks: live,
        })
    }

    async fn scan_orphans(&self, nonce: &str) -> Result<OrphanScan> {
        match list_containers_with_nonce(nonce).await {
            Ok(found) => Ok(OrphanScan {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Container,
                nonce: nonce.into(),
                method: format!("docker ps -a --filter label={NONCE_LABEL}=<nonce>"),
                found,
                enumerated: true,
            }),
            Err(detail) => Ok(OrphanScan {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Container,
                nonce: nonce.into(),
                // An unscannable surface reports enumerated=false. Reporting
                // zero orphans because the scan failed is how an orphan hides.
                method: format!("docker ps failed: {detail}"),
                found: Vec::new(),
                enumerated: false,
            }),
        }
    }
}

async fn list_containers_with_nonce(nonce: &str) -> std::result::Result<Vec<String>, String> {
    let filter = format!("label={NONCE_LABEL}={nonce}");
    let mut command = wcore_config::shell::shell_command_argv(
        "docker",
        &["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"],
    );
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| "docker ps did not answer within 10s".to_string())?
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact stderr docker 29.2.1 produces for a name conflict, measured
    /// against a real daemon while diagnosing issue #365.
    const CONFLICT_STDERR: &[u8] = b"docker: Error response from daemon: Conflict. The container name \"/wayland-f25-conf-container-ok\" is already in use by container \"9e0bb9943941e2308fa2cf57db1bbb105a685036800e07cfa920b8b392ce14f5\". You have to remove (or rename) that container to be able to reuse that name.\n\nRun 'docker run --help' for more information\n";

    #[test]
    fn a_daemon_refusal_is_never_a_task_exit() {
        // Every 125 measured with docker's own error line on stderr: name
        // conflict, a --memory below the daemon minimum, and an absent image.
        for stderr in [
            CONFLICT_STDERR.to_vec(),
            b"docker: Error response from daemon: Minimum memory limit allowed is 6MB\n".to_vec(),
            b"docker: Error response from daemon: No such image: no-such-image:nope\n".to_vec(),
        ] {
            match classify_exit(125, &stderr) {
                DockerExit::DaemonRefusal(detail) => assert!(
                    detail.contains(DAEMON_ERROR_MARKER),
                    "the refusal must carry the daemon's own words: {detail}"
                ),
                other => panic!("a daemon refusal must not be attested as a task exit: {other:?}"),
            }
        }
    }

    #[test]
    fn the_conflict_message_the_operator_needs_survives_classification() {
        // c3: the message names the problem AND the remedy, and it is the
        // thing the operator sees. The `Run 'docker run --help'` noise is not.
        let DockerExit::DaemonRefusal(detail) = classify_exit(125, CONFLICT_STDERR) else {
            panic!("expected a refusal");
        };
        assert!(detail.contains("Conflict. The container name"), "{detail}");
        assert!(detail.contains("already in use by container"), "{detail}");
        assert!(
            !detail.contains("docker run --help"),
            "the help footer is noise in an operator-facing error: {detail}"
        );
    }

    #[test]
    fn a_task_that_exits_125_on_its_own_is_still_a_real_run() {
        // The inverse defect, and the reason the exit code alone cannot be the
        // discriminator: `docker run busybox sh -c 'exit 125'` measured 125
        // with NO daemon line. Erasing that run would be as much a lie as
        // attesting a refusal.
        assert_eq!(classify_exit(125, b""), DockerExit::TaskExit(125));
        assert_eq!(
            classify_exit(125, b"the task's own diagnostics\n"),
            DockerExit::TaskExit(125)
        );
    }

    #[test]
    fn one_hundred_twenty_six_and_seven_stay_the_containers_own_status() {
        // Measured: both carry docker's error line, because the daemon is what
        // reports them — but what they describe is the TASK's argv, so they
        // remain task exits. They are dispositioned here explicitly rather
        // than left to surface later.
        let not_executable: &[u8] = b"docker: Error response from daemon: failed to create task for container: failed to create shim task: OCI runtime create failed: runc create failed: unable to start container process: error during container init: exec: \"/etc\": is a directory: permission denied\n";
        let not_found: &[u8] = b"docker: Error response from daemon: failed to create task for container: failed to create shim task: OCI runtime create failed: runc create failed: unable to start container process: error during container init: exec: \"no-such-binary\": executable file not found in $PATH\n";
        assert_eq!(
            classify_exit(126, not_executable),
            DockerExit::TaskExit(126)
        );
        assert_eq!(classify_exit(127, not_found), DockerExit::TaskExit(127));
        // And when the contained process picks those codes itself.
        assert_eq!(classify_exit(126, b""), DockerExit::TaskExit(126));
        assert_eq!(classify_exit(127, b""), DockerExit::TaskExit(127));
    }

    #[test]
    fn an_ordinary_exit_is_untouched_whatever_is_on_stderr() {
        assert_eq!(classify_exit(0, b""), DockerExit::TaskExit(0));
        assert_eq!(classify_exit(3, b"boom\n"), DockerExit::TaskExit(3));
        // A task is allowed to print anything, including something that looks
        // like a daemon line. Without a reserved code it changes nothing.
        assert_eq!(
            classify_exit(3, CONFLICT_STDERR),
            DockerExit::TaskExit(3),
            "the marker NARROWS a reserved code and must never widen an ordinary one"
        );
    }

    #[test]
    fn the_name_stays_a_pure_function_of_the_task_id() {
        // The property `cancel()` depends on: it reconstructs the name from
        // the task id alone when the registry entry carries no handle. If this
        // ever stops holding, that fallback silently stops cancelling.
        assert_eq!(
            ContainerBackend::container_name("conf-container-ok"),
            "wayland-f25-conf-container-ok"
        );
        assert_eq!(
            ContainerBackend::container_name("conf-container-ok"),
            ContainerBackend::container_name("conf-container-ok")
        );
    }
}
