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

/// The marker docker puts on its OWN diagnostics. This is used ONLY to pick
/// which stderr lines an operator is shown — never to decide whether a task
/// ran. See `classify_run` for why that distinction is the whole of #365 c2.
const DAEMON_ERROR_MARKER: &str = "Error response from daemon:";

/// The zero `time.Time` docker renders in `.State.StartedAt` for a container
/// that was created and whose process was never started. MEASURED on docker
/// 29.2.1: it is a rendered zero value, not a human-readable message, and it
/// is the daemon's own answer to "did anything of this task execute?".
const NEVER_STARTED_AT: &str = "0001-01-01T00:00:00Z";

/// The one inspect template this backend needs after a run. `.State.ExitCode`
/// is the DAEMON's code for the container, which is not the same thing as the
/// docker CLI's process exit status — see the table on `classify_run`.
const FINAL_STATE_FORMAT: &str = "{{.State.StartedAt}}|{{.State.ExitCode}}|{{.State.Error}}";

pub struct ContainerBackend {
    capabilities: BackendCapabilities,
    identity: BackendIdentity,
    signer: ReceiptSigner,
    image: String,
}

impl ContainerBackend {
    pub fn new(limits: ResourceBudget) -> Result<Self> {
        let image =
            std::env::var("WAYLAND_EXEC_CONTAINER_IMAGE").unwrap_or_else(|_| DEFAULT_IMAGE.into());
        Self::with_image(limits, image)
    }

    /// Construct against an EXPLICIT image, without consulting the process
    /// environment.
    ///
    /// `WAYLAND_EXEC_CONTAINER_IMAGE` is a process GLOBAL, and a test that
    /// sets it to steer one construction steers every sibling test running in
    /// the same binary at that moment. `serial_test` does not help: it orders
    /// only the tests carrying the attribute. Stating the image is what makes
    /// the test independent of what else is running.
    pub fn with_image(limits: ResourceBudget, image: impl Into<String>) -> Result<Self> {
        let image = image.into();
        let seed = load_or_create_seed(BACKEND_ID)?;
        let signer = ReceiptSigner::from_seed(seed);
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

/// What the daemon says about a container once `docker run` has returned.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContainerState {
    /// `.State.StartedAt` is not the zero value, i.e. the container's process
    /// was actually started. Only then does an exit code describe a run.
    started: bool,
    /// `.State.ExitCode` — the daemon's code for the container.
    exit_code: i32,
    /// `.State.Error` — empty on a clean run, the start failure otherwise.
    error: String,
}

/// Whether a `docker run` produced a task execution at all.
///
/// #365 c2 asks that a refusal never be reported as a task exit. The FIRST
/// attempt keyed on the conjunction of exit code 125 and docker's
/// `Error response from daemon:` line, and a live reproduction refuted it: the
/// marker only covers DAEMON-side refusals, and docker's 125 class is bigger
/// than that. Both of these exit 125 with the marker count at ZERO, and both
/// therefore produced a signed receipt asserting the task ran and returned
/// 125 — measured end to end through the CLI on docker 29.2.1:
///
/// * `docker run --rm BadImage:Tag true`
///   → `docker: invalid reference format: repository name (library/BadImage)
///     must be lowercase`
/// * `docker run --rm --memory notanumber busybox:1.36 true`
///   → `invalid argument "notanumber" for "-m, --memory" flag`
///
/// A text match on stderr cannot be repaired by adding more strings; the whole
/// approach guesses at an authority docker will happily answer directly. So
/// the discriminator is now the DAEMON'S OWN RECORD of the container, obtained
/// from two structural facts and no message text:
///
/// 1. `--cidfile` — docker writes the container id there when, and only when,
///    ContainerCreate succeeds. Its absence is positive proof that no
///    container exists, which is a stronger statement than any diagnostic.
/// 2. `.State.StartedAt` — the zero value means the container was created and
///    its process never started, so nothing of the task executed.
///
/// FULL ENUMERATION, every row measured on docker 29.2.1 (issue #365). "CLI"
/// is `docker run`'s own exit status; the last three columns are the daemon's:
///
/// | class                            | CLI | cidfile | StartedAt | .State.ExitCode | verdict  |
/// |----------------------------------|-----|---------|-----------|-----------------|----------|
/// | client: bad image reference      | 125 | absent  | –         | –               | NeverRan |
/// | client: bad flag value           | 125 | absent  | –         | –               | NeverRan |
/// | client: daemon unreachable       |   1 | absent  | –         | –               | NeverRan |
/// | client: stale cidfile            | 125 | (fresh) | –         | –               | NeverRan |
/// | daemon: name conflict            | 125 | absent  | –         | –               | NeverRan |
/// | daemon: `--memory 1`             | 125 | absent  | –         | –               | NeverRan |
/// | daemon: image absent / pull fail | 125 | absent  | –         | –               | NeverRan |
/// | daemon: start failed (network)   | 125 | written | zero      | 128             | NeverRan |
/// | daemon: start failed (device)    | 127 | written | zero      | 128             | NeverRan |
/// | argv not executable              | 126 | written | zero      | 126             | TaskExit |
/// | argv not found                   | 127 | written | zero      | 127             | TaskExit |
/// | task exits 0 / 7 / 125 / 126     |   = | written | real      | =               | TaskExit |
///
/// The two rows that matter most:
///
/// * `task exits 125` keeps a real receipt. The container started, so the
///   daemon's own `.State.StartedAt` proves the run happened and 125 is the
///   task's own status. This is the polarity the previous discriminator got
///   right and that must not be lost while fixing the one it got wrong.
/// * `start failed (device)` exits **127** at the CLI while the daemon records
///   `.State.ExitCode` 128 and a zero `StartedAt`. The CLI code alone would
///   have called that a task exit; the daemon's record does not.
///
/// 126/127 DISPOSITION, unchanged from the previous pass but now resting on a
/// structural field instead of a string. The daemon assigns 126/127 for a
/// failed exec OF THE TASK'S ARGV and 128 for a start failure of its own
/// making, so `.State.ExitCode` carries the attribution the stderr text used
/// to. They stay task exits: `Failure { code: "exit-127" }` is the
/// conventional truthful encoding of "the argv you submitted is not there",
/// the same one every POSIX shell uses, and reclassifying it as an unavailable
/// BACKEND would make a typo in a task's argv read as a broken transport.
///
/// DIRECTION OF ERROR. Every uncertain case resolves to `NeverRan`, which is a
/// refusal and a strictly WEAKER claim than any receipt. A lost cidfile or an
/// inspect the daemon will not answer costs a real run its attestation; it can
/// never manufacture one. For a surface whose product is a signed statement
/// that is the correct direction to be wrong in.
#[derive(Debug, PartialEq, Eq)]
enum DockerExit {
    /// No process of this task ever executed. There is nothing to attest: no
    /// argv ran and no workspace was read. This must NEVER become a receipt.
    NeverRan(String),
    /// The contained process's own terminal status, 126 and 127 included.
    TaskExit(i32),
}

/// Resolve a run against the daemon's record of it.
///
/// `observed` is `None` when docker wrote no container id, i.e. no container
/// was ever created. `docker_stderr` is used for the operator-facing message
/// only, never for the verdict.
fn classify_run(observed: Option<&ContainerState>, docker_stderr: &str) -> DockerExit {
    let Some(state) = observed else {
        // Nothing was created, so whatever docker exited with, it is docker's
        // status and not a task's. This is the row the string match missed.
        return DockerExit::NeverRan(docker_diagnostic(docker_stderr));
    };
    if state.started {
        return DockerExit::TaskExit(state.exit_code);
    }
    match state.exit_code {
        // The daemon attributes a failed exec of the TASK'S ARGV to 126/127.
        126 | 127 => DockerExit::TaskExit(state.exit_code),
        // Anything else with a zero StartedAt is the daemon failing to start a
        // container it had already created — infrastructure, not the task.
        _ => {
            let detail = if state.error.trim().is_empty() {
                docker_diagnostic(docker_stderr)
            } else {
                state.error.trim().to_string()
            };
            DockerExit::NeverRan(detail)
        }
    }
}

/// Parse the `FINAL_STATE_FORMAT` render into a state.
fn parse_final_state(rendered: &str) -> std::result::Result<ContainerState, String> {
    let line = rendered.trim();
    // `.State.Error` is taken as the REMAINDER: the daemon's message may itself
    // contain a '|', and truncating it would drop the operator's diagnostic.
    let mut parts = line.splitn(3, '|');
    let started_at = parts.next().unwrap_or_default().trim();
    let code = parts
        .next()
        .ok_or_else(|| format!("docker inspect returned an unparsable state: {line:?}"))?;
    let error = parts.next().unwrap_or_default().trim().to_string();
    if started_at.is_empty() {
        return Err(format!("docker inspect returned no StartedAt: {line:?}"));
    }
    let exit_code: i32 = code
        .trim()
        .parse()
        .map_err(|_| format!("docker inspect returned an unparsable exit code: {code:?}"))?;
    Ok(ContainerState {
        started: started_at != NEVER_STARTED_AT,
        exit_code,
        error,
    })
}

/// Ask the daemon what became of the container this run created.
async fn inspect_final_state(id: &str) -> std::result::Result<ContainerState, String> {
    let mut command = wcore_config::shell::shell_command_argv(
        "docker",
        &["inspect", id, "--format", FINAL_STATE_FORMAT],
    );
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| "docker inspect did not answer within 10s".to_string())?
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    parse_final_state(&String::from_utf8_lossy(&output.stdout))
}

/// The container id docker recorded, if it created one at all.
///
/// A file that is missing, empty or not a hex id is read as "no container".
/// Inventing a container from a malformed file would put the discriminator
/// back on a guess.
fn read_cid(path: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let id = raw.trim();
    if id.len() >= 12 && id.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(id.to_string())
    } else {
        None
    }
}

/// A `--cidfile` path for this run, PROVEN free before docker is invoked.
///
/// Two traps, both measured, both closed here:
///
/// * docker refuses to run at all when the path already exists — `docker:
///   container ID file found, make sure the other container isn't running or
///   delete <path>`, exit 125, client side and with no daemon line.
/// * a leftover file would be read back as proof of a container THIS run never
///   created, which is the forgery the discriminator exists to prevent.
///
/// Clearing the path and then re-checking it closes both. If it survives, the
/// discriminator cannot be established, and a discriminator that cannot be
/// established must not be assumed — the run is refused instead.
fn cid_path(container_name: &str) -> Result<std::path::PathBuf> {
    let dir = registry::state_dir().join("cid");
    std::fs::create_dir_all(&dir)?;
    // `container_name` has been through `validate_identifier`, so it is a
    // single ascii path segment and cannot traverse out of `dir`.
    let path = dir.join(format!("{container_name}.cid"));
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        return Err(ExecError::Unavailable {
            backend_id: BACKEND_ID.into(),
            detail: format!(
                "could not clear a stale container-id file at {}: {e}",
                path.display()
            ),
        });
    }
    if path.exists() {
        return Err(ExecError::Unavailable {
            backend_id: BACKEND_ID.into(),
            detail: format!(
                "the container-id file at {} could not be cleared, so whether a container \
                 was created cannot be established; refusing to run rather than guess",
                path.display()
            ),
        });
    }
    Ok(path)
}

/// Remove the container this run created. This is what replaced `--rm`.
///
/// `--rm` had to go for #365 c2: it deletes the container the instant it stops,
/// and MEASURED on docker 29.2.1 that includes containers that never started —
/// `docker inspect` by id after a `--rm` run returns `no such object` for a
/// start failure exactly as it does for a clean exit. The daemon is the only
/// authority on whether a task's process ran, so the container has to outlive
/// the run long enough to be asked.
///
/// TRADEOFF, stated rather than hidden: `--rm` is daemon-side, so it cleaned up
/// even if this process was killed mid-run, and an explicit removal cannot.
/// What catches that window is the machinery #365 already built — the container
/// keeps its deterministic name, so `reclaim_container_name` clears it on the
/// next submit of the same task id, and it keeps its `wayland.task.nonce`
/// label, so `scan_orphans` can still see it. The residue is the same class
/// tracked by FerroxLabs/wayland-core#366.
async fn remove_container(id: &str) {
    let mut command = wcore_config::shell::shell_command_argv("docker", &["rm", "-f", id]);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), command.output()).await;
}

/// Pull the lines an operator needs out of a mixed stderr.
///
/// The whole of stderr is not used: docker appends `Run 'docker run --help'
/// for more information` and sometimes a `Usage:` banner, which are noise in an
/// operator-facing error. When docker's own daemon line is present it is the
/// whole answer; otherwise — the client-side classes — what is left after the
/// noise is.
fn docker_diagnostic(stderr: &str) -> String {
    let signal: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with("Usage:"))
        .filter(|line| !(line.starts_with("Run '") && line.ends_with("for more information")))
        .collect();
    if signal.is_empty() {
        return stderr.trim().to_string();
    }
    let daemon: Vec<&str> = signal
        .iter()
        .copied()
        .filter(|line| line.contains(DAEMON_ERROR_MARKER))
        .collect();
    if daemon.is_empty() {
        signal.join(" / ")
    } else {
        daemon.join(" / ")
    }
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

        // Issue #365 c2: the only authority on whether this task's process ran
        // is the daemon, so the run must leave the daemon something to be asked
        // about. `--cidfile` records the container id the moment
        // ContainerCreate succeeds and writes nothing when it does not.
        let cidfile = cid_path(&name)?;

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
            // NOT `--rm`: it would delete the daemon's record of this run
            // before the run could be classified. See `remove_container`.
            "--cidfile".into(),
            cidfile.display().to_string(),
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
                // `cancel` removes the container by name; the id file is ours
                // to clear so the next submit of this task id starts clean.
                let _ = self.cancel(&task.task_id).await;
                let _ = std::fs::remove_file(&cidfile);
                return Err(ExecError::Exec(format!(
                    "container task {} exceeded its {}ms wall clock",
                    task.task_id, task.resources.wall_time_ms
                )));
            }
        };

        let finished = now_unix_ms();

        // Issue #365 c2: resolve WHOSE exit this is before anything is
        // attested, from the daemon's record rather than from message text. A
        // run that never happened never becomes a receipt — there is nothing
        // to sign a statement about — and its diagnostic is returned as the
        // error VALUE, which the CLI prints unconditionally to stderr. It is
        // deliberately NOT a `warn!`: RUST_LOG is unset for ordinary operators,
        // so only ERROR reaches stderr and a log-level bump could never make
        // the user told.
        let stderr_text = String::from_utf8_lossy(&output.stderr).into_owned();
        // Read the id BEFORE anything is removed. Absent means no container was
        // ever created, whatever docker exited with.
        let created_id = read_cid(&cidfile);

        let abandon = |detail: String| -> ExecError {
            registry::forget(&task.task_id).ok();
            let _ = std::fs::remove_dir_all(&workdir);
            let _ = std::fs::remove_file(&cidfile);
            ExecError::Unavailable {
                backend_id: BACKEND_ID.into(),
                detail,
            }
        };

        let observed = match &created_id {
            None => None,
            Some(id) => match inspect_final_state(id).await {
                Ok(state) => Some(state),
                Err(detail) => {
                    // A container exists but the daemon will not describe it,
                    // so whether the task ran is UNKNOWN. Fail closed and
                    // refuse to attest, rather than pick the answer that
                    // happens to produce a receipt.
                    remove_container(id).await;
                    return Err(abandon(format!(
                        "task {} produced container {id}, but the daemon would not report \
                         whether it ever started, so the run cannot be attested: {detail}",
                        task.task_id
                    )));
                }
            },
        };

        let exit_code = match classify_run(observed.as_ref(), &stderr_text) {
            DockerExit::NeverRan(detail) => {
                if let Some(id) = &created_id {
                    remove_container(id).await;
                }
                return Err(abandon(format!(
                    "task {} was never executed — no container process started: {detail}",
                    task.task_id
                )));
            }
            DockerExit::TaskExit(code) => code,
        };

        // The run is classified, so the daemon's record has served its purpose.
        if let Some(id) = &created_id {
            remove_container(id).await;
        }
        let _ = std::fs::remove_file(&cidfile);

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
    const CONFLICT_STDERR: &str = "docker: Error response from daemon: Conflict. The container name \"/wayland-f25-conf-container-ok\" is already in use by container \"9e0bb9943941e2308fa2cf57db1bbb105a685036800e07cfa920b8b392ce14f5\". You have to remove (or rename) that container to be able to reuse that name.\n\nRun 'docker run --help' for more information\n";

    /// A container the daemon started, which exited with `code`.
    fn ran(code: i32) -> ContainerState {
        ContainerState {
            started: true,
            exit_code: code,
            error: String::new(),
        }
    }

    /// A container the daemon created and never started.
    fn never_started(code: i32, error: &str) -> ContainerState {
        ContainerState {
            started: false,
            exit_code: code,
            error: error.into(),
        }
    }

    /// THE REGRESSION THIS EXISTS FOR — the class nobody thought of.
    ///
    /// The first fix keyed on docker's `Error response from daemon:` line, and
    /// docker's CLIENT side never prints it. Both of these were measured live
    /// on docker 29.2.1 at exit 125 with a marker count of ZERO, and both
    /// produced a signed receipt asserting the task ran and returned 125.
    /// There is no container in either case, so there is nothing to attest.
    ///
    /// If a future change reintroduces any stderr text match, this is the test
    /// that catches it: neither message contains the daemon marker.
    #[test]
    fn a_client_side_refusal_is_not_a_task_exit_though_it_prints_no_daemon_line() {
        let client_side = [
            // docker run --rm BadImage:Tag true
            "docker: invalid reference format: repository name (library/BadImage) must be lowercase\n\nRun 'docker run --help' for more information\n",
            // docker run --rm --memory notanumber busybox:1.36 true
            "invalid argument \"notanumber\" for \"-m, --memory\" flag: invalid size: 'notanumber'\n\nUsage:  docker run [OPTIONS] IMAGE [COMMAND] [ARG...]\n\nRun 'docker run --help' for more information\n",
            // a stale --cidfile, which is also client side and also 125
            "docker: container ID file found, make sure the other container isn't running or delete /tmp/x.cid\n",
            // the daemon going away between the availability ping and the run
            "failed to connect to the docker API at unix:///var/run/docker.sock; check if the path is correct and if the daemon is running: dial unix /var/run/docker.sock: connect: no such file or directory\n",
        ];
        for stderr in client_side {
            assert!(
                !stderr.contains(DAEMON_ERROR_MARKER),
                "this test is only meaningful for stderr WITHOUT the daemon marker: {stderr}"
            );
            // No container id file was written, so `observed` is None.
            match classify_run(None, stderr) {
                DockerExit::NeverRan(detail) => {
                    assert!(!detail.is_empty(), "the operator must be told why");
                    assert!(
                        !detail.contains("for more information"),
                        "the help footer is noise: {detail}"
                    );
                    assert!(
                        !detail.starts_with("Usage:"),
                        "the usage banner is noise: {detail}"
                    );
                }
                other => panic!(
                    "a refusal that created no container must not be attested as a task exit: {other:?}"
                ),
            }
        }
    }

    #[test]
    fn a_daemon_refusal_is_never_a_task_exit() {
        // Every pre-create refusal measured: name conflict, a --memory below
        // the daemon minimum, and an absent image. None of them writes a
        // container id, which is what makes them refusals.
        for stderr in [
            CONFLICT_STDERR,
            "docker: Error response from daemon: Minimum memory limit allowed is 6MB\n",
            "docker: Error response from daemon: No such image: no-such-image:nope\n",
        ] {
            match classify_run(None, stderr) {
                DockerExit::NeverRan(detail) => assert!(
                    detail.contains(DAEMON_ERROR_MARKER),
                    "the refusal must carry the daemon's own words: {detail}"
                ),
                other => panic!("a daemon refusal must not be attested as a task exit: {other:?}"),
            }
        }
    }

    #[test]
    fn a_container_created_but_never_started_is_not_a_task_exit() {
        // MEASURED: `--network no-such-net` exits 125 at the CLI and `--device
        // /dev/nonexistent` exits 127, but the daemon records a zero StartedAt
        // and .State.ExitCode 128 for both. The CLI code alone would have
        // called the second one a task exit.
        let network = never_started(
            128,
            "failed to set up container networking: network no-such-net-f13 not found",
        );
        let device = never_started(
            128,
            "error gathering device information while adding custom device \"/dev/nonexistent-f13\": no such file or directory",
        );
        for state in [&network, &device] {
            match classify_run(Some(state), "") {
                DockerExit::NeverRan(detail) => assert_eq!(detail, state.error),
                other => panic!("a container that never started did not run: {other:?}"),
            }
        }
    }

    #[test]
    fn the_conflict_message_the_operator_needs_survives_classification() {
        // c3: the message names the problem AND the remedy, and it is the
        // thing the operator sees. The `Run 'docker run --help'` noise is not.
        let DockerExit::NeverRan(detail) = classify_run(None, CONFLICT_STDERR) else {
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
        // The inverse defect, and the polarity the discriminator must keep:
        // `docker run busybox sh -c 'exit 125'` measured 125 at the CLI with a
        // real StartedAt and .State.ExitCode 125. Erasing that run would be as
        // much a lie as attesting a refusal.
        //
        // This is NOT vacuous under the current rule: the verdict is reached
        // through `started == true`, and the two cases below differ only in
        // stderr, which no longer participates in the verdict at all.
        assert_eq!(classify_run(Some(&ran(125)), ""), DockerExit::TaskExit(125));
        assert_eq!(
            classify_run(Some(&ran(125)), "the task's own diagnostics\n"),
            DockerExit::TaskExit(125)
        );
        // Even if the task prints something that looks exactly like docker's
        // own refusal, a started container's exit is still its own.
        assert_eq!(
            classify_run(Some(&ran(125)), CONFLICT_STDERR),
            DockerExit::TaskExit(125),
            "a started container's status is the daemon's record, not its stderr"
        );
    }

    #[test]
    fn one_hundred_twenty_six_and_seven_stay_the_containers_own_status() {
        // MEASURED: an exec failure of the TASK'S ARGV never starts the
        // container, but the daemon records .State.ExitCode 126/127 for it —
        // its own attribution to the argv, as against 128 for a start failure
        // of the daemon's making. That is what keeps these task exits without
        // reading any message text.
        let not_executable = never_started(
            126,
            "failed to create task for container: ... exec: \"/etc\": is a directory: permission denied",
        );
        let not_found = never_started(
            127,
            "failed to create task for container: ... exec: \"no-such-binary\": executable file not found in $PATH",
        );
        assert_eq!(
            classify_run(Some(&not_executable), ""),
            DockerExit::TaskExit(126)
        );
        assert_eq!(
            classify_run(Some(&not_found), ""),
            DockerExit::TaskExit(127)
        );
        // And when the contained process picks those codes itself.
        assert_eq!(classify_run(Some(&ran(126)), ""), DockerExit::TaskExit(126));
        assert_eq!(classify_run(Some(&ran(127)), ""), DockerExit::TaskExit(127));
    }

    #[test]
    fn an_ordinary_exit_is_untouched_whatever_is_on_stderr() {
        assert_eq!(classify_run(Some(&ran(0)), ""), DockerExit::TaskExit(0));
        assert_eq!(
            classify_run(Some(&ran(3)), "boom\n"),
            DockerExit::TaskExit(3)
        );
        // A task is allowed to print anything, including something that looks
        // like a daemon line. stderr does not participate in the verdict.
        assert_eq!(
            classify_run(Some(&ran(3)), CONFLICT_STDERR),
            DockerExit::TaskExit(3),
        );
    }

    #[test]
    fn the_daemons_state_render_is_parsed_including_a_pipe_in_the_error() {
        let clean = parse_final_state("2026-08-29T15:05:09.475472605Z|0|").expect("parse");
        assert_eq!(clean, ran(0));

        let started_125 = parse_final_state("2026-08-29T15:05:15.103974142Z|125|").expect("parse");
        assert!(started_125.started);
        assert_eq!(started_125.exit_code, 125);

        let zero = parse_final_state(
            "0001-01-01T00:00:00Z|128|failed to set up container networking: network x not found",
        )
        .expect("parse");
        assert!(!zero.started, "the zero StartedAt means it never started");
        assert_eq!(zero.exit_code, 128);

        // The daemon's message may contain the separator; it must survive whole
        // rather than being truncated at the first '|'.
        let piped =
            parse_final_state("0001-01-01T00:00:00Z|127|exec: \"a|b\": not found").expect("parse");
        assert_eq!(piped.error, "exec: \"a|b\": not found");

        // A render this backend cannot understand is an error, never a guess.
        assert!(parse_final_state("").is_err());
        assert!(parse_final_state("0001-01-01T00:00:00Z|notanumber|").is_err());
    }

    #[test]
    fn only_a_real_container_id_counts_as_a_container() {
        let dir = std::env::temp_dir().join(format!("wayland-f25-cid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let missing = dir.join("missing.cid");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(read_cid(&missing), None, "no file means no container");

        let empty = dir.join("empty.cid");
        std::fs::write(&empty, b"").expect("write");
        assert_eq!(read_cid(&empty), None, "an empty file means no container");

        let junk = dir.join("junk.cid");
        std::fs::write(&junk, b"not-a-container-id\n").expect("write");
        assert_eq!(
            read_cid(&junk),
            None,
            "a malformed file must not invent one"
        );

        let good = dir.join("good.cid");
        let id = "9e0bb9943941e2308fa2cf57db1bbb105a685036800e07cfa920b8b392ce14f5";
        std::fs::write(&good, format!("{id}\n")).expect("write");
        assert_eq!(read_cid(&good), Some(id.to_string()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cid_path_is_proven_free_before_the_run() {
        // docker refuses to run when the cidfile already exists, and a
        // leftover would be read back as a container this run never created.
        let dir = tempfile::tempdir().expect("tempdir");
        let _state = registry::StateDirGuard::set(dir.path());

        let path = cid_path("wayland-f25-cidfree").expect("a free path");
        assert!(!path.exists());

        std::fs::write(&path, b"deadbeefdeadbeef\n").expect("plant a leftover");
        assert!(path.exists());
        let again = cid_path("wayland-f25-cidfree").expect("the leftover is cleared");
        assert_eq!(again, path);
        assert!(
            !again.exists(),
            "a stale container-id file must be gone before docker is invoked"
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
