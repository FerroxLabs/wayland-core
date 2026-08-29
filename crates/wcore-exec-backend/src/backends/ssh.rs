//! The SSH reference backend.
//!
//! Two rules govern this module and neither is negotiable.
//!
//! 1. ARGV MODE ONLY, PLUS EXPLICIT QUOTING FOR THE FAR END. AGENTS.md states
//!    that any command whose arguments include non-literal data must use
//!    `shell_command_argv`, never shell-string mode, because argv mode never
//!    lets a metacharacter reach an interpreter. That is necessary here and it
//!    is NOT sufficient, and this file used to claim otherwise.
//!
//!    **`ssh` does not carry an argument vector.** The client joins its
//!    remote-command arguments with single spaces and the far end's LOGIN
//!    SHELL re-splits the resulting string. So `shell_command_argv` protects
//!    the LOCAL spawn only; across the connection every value is shell text.
//!    Measured against a real far end on 2026-07-28, through the shipped
//!    binary:
//!
//!    * an EMPTY value vanished entirely, shifting every later argument left;
//!    * a value containing a space arrived as two arguments;
//!    * a value containing `;` was EXECUTED on the far end — `backend scan
//!      --task-id 'x;id>/tmp/w;echo y'` ran `id` as root there.
//!
//!    Every value that crosses the connection therefore goes through
//!    [`posix_quote`], so the far end's shell re-assembles the original argv
//!    instead of re-parsing the values as script. There is still no
//!    `format!`-into-a-command anywhere below, and no shell-string mode on the
//!    local spawn.
//!
//! 2. CANCELLATION IS REMOTE, NOT LOCAL. The local backends inherit
//!    process-tree ownership; over ssh nothing inherits anything. So the
//!    remote side starts its work in its own session via `setsid`, tagged with
//!    the per-task nonce, and cancellation opens a SECOND connection and kills
//!    that remote session. Closing the local connection is NOT cancellation
//!    and plan 25-04 looks at the remote process table, where a
//!    connection-close implementation is caught immediately.
//!
//! The system `ssh` client is used deliberately. It exists on macOS, Linux and
//! Windows 10+, and the live targets are already reachable with existing keys.
//! A pure-Rust SSH crate would be a large new dependency buying nothing this
//! criterion needs.

use async_trait::async_trait;

use crate::contract::{
    Availability, BackendCapabilities, BackendKind, CleanupObservation, ExecutionBackend,
    ExecutionTask, Health, HibernationObservation, OrphanScan, OrphanSweep, ProbeBasis,
    ResourceBudget, SecretChannel, validate_identifier,
};
use crate::error::{ExecError, Result};
use crate::policy::{EffectivePolicy, declared_secret_exposure};
use crate::receipt::{BackendIdentity, ExecutionReceipt, ReceiptSigner};
use crate::registry::{self, LiveTask, now_unix_ms};

use super::local::{cancel_marker_taken, instance_id, write_cancel_marker};
use super::{
    RunOutcome, denial_receipt, load_or_create_seed, outcome_receipt, pre_acceptance_denial,
};

pub const BACKEND_ID: &str = "ssh";
pub const TARGET_ENV: &str = "WAYLAND_EXEC_SSH_TARGET";
/// An operator-pinned `ssh_config` file, passed as `-F`.
///
/// Deliberately a FILE PATH and not a free-form option string. A
/// space-separated "extra ssh options" variable would be an argument-injection
/// surface pointed straight at the most attacker-adjacent binary in the
/// phase; `ssh_config` already expresses ports, identities, jump hosts and
/// known-hosts files, so nothing is lost by refusing the general case.
pub const CONFIG_ENV: &str = "WAYLAND_EXEC_SSH_CONFIG";

pub struct SshBackend {
    capabilities: BackendCapabilities,
    identity: BackendIdentity,
    signer: ReceiptSigner,
    target: Option<String>,
}

impl SshBackend {
    pub fn new(limits: ResourceBudget) -> Result<Self> {
        let seed = load_or_create_seed(BACKEND_ID)?;
        let signer = ReceiptSigner::from_seed(seed);
        let target = std::env::var(TARGET_ENV).ok().filter(|t| !t.is_empty());
        Ok(Self {
            capabilities: BackendCapabilities {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Ssh,
                version: env!("CARGO_PKG_VERSION").into(),
                limits,
                supports_artifact_transfer: true,
                supports_cancellation: true,
                supports_hibernation: false,
                secret_channel: SecretChannel::RemoteTransport,
            },
            identity: BackendIdentity {
                backend_id: BACKEND_ID.into(),
                instance_id: instance_id(),
                version: env!("CARGO_PKG_VERSION").into(),
                key_id: signer.key_id().to_string(),
            },
            signer,
            target,
        })
    }

    pub fn identity(&self) -> &BackendIdentity {
        &self.identity
    }

    pub fn verifying_key(&self) -> ed25519_dalek::VerifyingKey {
        self.signer.verifying_key()
    }

    fn require_target(&self) -> Result<&str> {
        self.target
            .as_deref()
            .ok_or_else(|| ExecError::Unavailable {
                backend_id: BACKEND_ID.into(),
                detail: format!("{TARGET_ENV} is not set, so there is no remote host to reach"),
            })
    }
}

/// Quote one value so the far end's shell reproduces it EXACTLY as one
/// argument.
///
/// This exists because `ssh host cmd a b c` is not an argv call: the client
/// concatenates `cmd a b c` with spaces and the remote login shell parses the
/// result. Wrapping each value in single quotes makes the remote shell treat
/// every byte inside as literal — no word splitting, no globbing, no command
/// substitution, no `;`. A literal single quote is the one character that
/// cannot appear inside a single-quoted string, so it is emitted as
/// `'\''` — close, escaped quote, reopen — which is the standard POSIX form.
///
/// An empty value becomes `''`, which is why an empty task input survives
/// instead of disappearing.
///
/// Public so `tests/ssh_far_end_quoting.rs` can round-trip its output through a
/// real shell. That round-trip cannot live in this file: the guard below
/// asserts this module's source contains no shell-string execution path, and a
/// shell invocation written here — even in a test — would trip it.
pub fn posix_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn ssh_base_args(target: &str) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if let Ok(config) = std::env::var(CONFIG_ENV)
        && !config.trim().is_empty()
    {
        args.push("-F".into());
        args.push(config);
    }
    args.extend([
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "ConnectTimeout=15".to_string(),
        target.to_string(),
    ]);
    args
}

/// The remote runner, sent on STDIN and executed by `sh -s` on the far end.
///
/// It is a CONSTANT. Nothing task-specific is interpolated into it; every
/// task-specific value arrives as a positional argument that `sh` binds to
/// `$1`, `$2`, … so it is never re-parsed as script text BY THIS SCRIPT.
///
/// That last qualification is the whole point and this comment used to omit
/// it. Binding to `$1` protects the value only once it has already arrived as
/// one argument. Getting it there is [`posix_quote`]'s job, because the far
/// end's LOGIN shell parses ssh's remote command string before this script
/// ever runs. Unquoted, a value carrying `;` was executed by that login shell
/// and this script never saw it.
///
/// # Cleanup, and the trap that is deliberately NOT here
///
/// The task root is removed on the way out, and — since 2026-07-29 — on the
/// FAILING way out too. It previously was not: `set -e` aborted the script at
/// `wait` whenever the task exited non-zero, so `rm -rf "$root"` never ran and
/// `input.bin` was left on the far end by every failing task. Six such roots
/// were found on a real node (`25-HOSTS-SUMMARY.md` FINDING 5).
///
/// The tempting general fix is `trap 'rm -rf "$root"' EXIT`. It is NOT used,
/// and that is a decision rather than an oversight. When the controller dies
/// mid-task the ssh connection drops and this script is signalled, while the
/// `setsid` child deliberately survives — that surviving child is the ONLY
/// unplanted positive control the orphan sweep has, and `$root/.pid` is the
/// primary signal [`REMOTE_SCAN`] reads to find it. An EXIT trap would delete
/// that evidence out from under a live orphan and turn a real finding into a
/// clean zero. Cleanup therefore runs only where the child has already exited;
/// cancellation cleans up through [`REMOTE_KILL`], which ends in its own
/// `rm -rf "$root"`.
pub const REMOTE_RUNNER: &str = r#"
set -eu
nonce="$1"; shift
b64input="$1"; shift
root="${TMPDIR:-/tmp}/wayland-f25-$nonce"
mkdir -p "$root"
printf '%s' "$b64input" | base64 -d > "$root/input.bin"
cd "$root"
export WAYLAND_TASK_NONCE="$nonce"
# setsid puts the work in its OWN session on the remote host, so a later
# cancellation can signal that session even though this connection owns
# nothing. Without it, killing the ssh client would leave the work running.
setsid "$@" &
child=$!
echo "$child" > "$root/.pid"
# `wait` REPORTS the child's status, and under `set -e` a non-zero status
# aborts the script right here — before the two lines below. That is how a
# failing task left its whole task root on the far end, `input.bin` (the
# task's own input bytes) included. `|| status=$?` takes this one command out
# of `set -e`'s reach; the status is still the child's and is still what this
# script exits with, so nothing about the reported outcome changes.
status=0
wait "$child" || status=$?
cd /
rm -rf "$root"
exit "$status"
"#;

#[async_trait]
impl ExecutionBackend for SshBackend {
    fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    async fn availability(&self) -> Availability {
        let Some(target) = self.target.as_deref() else {
            return Availability::down(
                ProbeBasis::CredentialAbsent,
                format!("{TARGET_ENV} is not set"),
            );
        };
        let mut args = ssh_base_args(target);
        args.push("true".into());
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut command = wcore_config::shell::shell_command_argv("ssh", &borrowed);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        match tokio::time::timeout(std::time::Duration::from_secs(20), command.output()).await {
            Ok(Ok(output)) if output.status.success() => Availability::up(
                ProbeBasis::SshHandshake,
                format!("ssh handshake to {target} reached the far end"),
            ),
            Ok(Ok(output)) => Availability::down(
                ProbeBasis::SshHandshake,
                format!(
                    "ssh to {target} failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ),
            Ok(Err(e)) => Availability::down(
                ProbeBasis::ProbeFailed,
                format!("could not launch the ssh client: {e}"),
            ),
            Err(_) => Availability::down(
                ProbeBasis::ProbeFailed,
                format!("ssh handshake to {target} did not complete within 20s"),
            ),
        }
    }

    fn effective_policy(&self, task: &ExecutionTask) -> Result<EffectivePolicy> {
        let (egress_decision, egress_source) = crate::policy::observed_egress_decision();
        let policy = EffectivePolicy {
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Ssh,
            egress_decision,
            egress_source,
            secret_channel: SecretChannel::RemoteTransport,
            secrets_exposed: declared_secret_exposure(BackendKind::Ssh, task),
            containment: "remote setsid session owned by the task nonce; no local containment \
                          crosses the connection"
                .into(),
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
        let target = self.require_target()?.to_string();
        validate_identifier("nonce", &task.nonce)?;

        // The workspace crosses as base64 on the argument vector, so no byte
        // of it is ever interpreted by a shell on either side.
        use base64::Engine as _;
        let input_b64 = base64::engine::general_purpose::STANDARD.encode(&task.input);

        let mut args = ssh_base_args(&target);
        args.push("sh".into());
        args.push("-s".into());
        args.push("--".into());
        // Quoted for the FAR END's shell, not just handed to the local ssh
        // client as separate argv entries. See `posix_quote`: without this an
        // empty input silently shifts `argv` left and a task argument
        // containing `;` executes on the far end.
        args.push(posix_quote(&task.nonce));
        args.push(posix_quote(&input_b64));
        args.extend(task.argv.iter().map(|a| posix_quote(a)));
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();

        let started = now_unix_ms();
        registry::record(&LiveTask {
            task_id: task.task_id.clone(),
            nonce: task.nonce.clone(),
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Ssh,
            pid: None,
            handle: Some(target.clone()),
            started_unix_ms: started,
        })?;

        let mut command = wcore_config::shell::shell_command_argv("ssh", &borrowed);
        command.stdin(std::process::Stdio::piped());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| ExecError::Transport(format!("could not launch the ssh client: {e}")))?;
        {
            use tokio::io::AsyncWriteExt as _;
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| ExecError::Transport("ssh stdin was not piped".into()))?;
            stdin.write_all(REMOTE_RUNNER.as_bytes()).await?;
            stdin.shutdown().await?;
        }

        let wall = std::time::Duration::from_millis(task.resources.wall_time_ms);
        let output = match tokio::time::timeout(wall, child.wait_with_output()).await {
            Ok(result) => result.map_err(|e| ExecError::Transport(e.to_string()))?,
            Err(_) => {
                let _ = self.cancel(&task.task_id).await;
                return Err(ExecError::Exec(format!(
                    "ssh task {} exceeded its {}ms wall clock",
                    task.task_id, task.resources.wall_time_ms
                )));
            }
        };

        let finished = now_unix_ms();
        let cancelled = cancel_marker_taken(&task.task_id);
        registry::forget(&task.task_id)?;

        outcome_receipt(
            task,
            &self.capabilities,
            &self.identity,
            &self.signer,
            &policy,
            RunOutcome {
                stdout: output.stdout,
                stderr: output.stderr,
                exit_code: output.status.code().unwrap_or(-1),
                endpoint: target,
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
        let target = entry.handle.clone().unwrap_or_default();
        // A SECOND connection kills the REMOTE session. Dropping the first
        // connection would leave the far end running, which is the exact
        // failure plan 25-04 hunts for.
        let killed = remote_kill(&target, &entry.nonce).await;
        let residual = match remote_scan(&target, &entry.nonce).await {
            Ok(found) => found,
            Err(detail) => vec![format!(
                "could not re-enumerate the remote process table: {detail}"
            )],
        };
        registry::forget(task_id)?;
        Ok(CleanupObservation {
            task_id: task_id.into(),
            backend_id: BACKEND_ID.into(),
            method: format!(
                "second ssh connection kills the remote session carrying the nonce ({killed}), \
                 then the remote process table is re-read"
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
        let Some(target) = self.target.as_deref() else {
            return Ok(OrphanScan {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Ssh,
                nonce: nonce.into(),
                method: format!("{TARGET_ENV} is not set, so no remote host could be scanned"),
                found: Vec::new(),
                enumerated: false,
            });
        };
        match remote_scan(target, nonce).await {
            Ok(found) => Ok(OrphanScan {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Ssh,
                nonce: nonce.into(),
                method: "remote `ps -eo pid,args` filtered on the task nonce".into(),
                found,
                enumerated: true,
            }),
            Err(detail) => Ok(OrphanScan {
                backend_id: BACKEND_ID.into(),
                kind: BackendKind::Ssh,
                nonce: nonce.into(),
                method: format!("remote scan failed: {detail}"),
                found: Vec::new(),
                enumerated: false,
            }),
        }
    }

    /// core#366: NOT ENUMERABLE, for the same reason as the local backend and
    /// one more. The far end is swept with `ps -eo pid,args`, which shows argv
    /// and not the environment the nonce travels in; and this backend has no
    /// marker of its own on the remote host at all, so an unscoped query there
    /// could not tell a wayland process from any other. Answering
    /// `enumerated: false` is the whole point of that field.
    async fn sweep_orphans(&self) -> Result<OrphanSweep> {
        Ok(OrphanSweep {
            backend_id: BACKEND_ID.into(),
            kind: BackendKind::Ssh,
            method: "NOT SWEPT: the far end carries no wayland marker an unscoped query could \
                     match, and the remote process table is read for argv rather than for the \
                     environment the nonce travels in."
                .into(),
            found: Vec::new(),
            enumerated: false,
        })
    }
}

/// Emitted by [`REMOTE_SCAN`] when no process-table reader on the far end
/// worked. The Rust side turns it into `enumerated: false`, so the surface
/// reports NOT MEASURED instead of a clean zero.
///
/// THIRD defect found live, on 2026-07-28, against a real Windows far end:
/// `ps -eo pid,ppid,args` is procps-specific and Git-for-Windows' msys `ps`
/// rejects it (`ps: unknown option -- o`). Its stderr went to `/dev/null` and
/// the pipeline ended in `|| true`, so a sweep that could not run produced an
/// empty result indistinguishable from a clean one. The surface reported
/// `0 (MEASURED)` while two independent instruments — msys `ps -ef` and
/// `Win32_Process` — both showed the orphan. Same class as the `tasklist`
/// false zero plan 25-04 found, on a different surface.
const SWEEP_UNAVAILABLE: &str = "__WAYLAND_SWEEP_UNAVAILABLE__";

/// Constant remote scanner. The nonce arrives as `$1`, never as script text.
///
/// TWO defects found by the live cancellation run on 2026-07-26 are fixed
/// here, and both were false answers rather than crashes — the class that
/// survives a green test suite:
///
/// 1. THE SCANNER FOUND ITSELF. The nonce travels on this very script's own
///    argv, so `ps | grep <nonce>` matched `sh -s -- <nonce>` — the scan's own
///    process. Every scan reported one orphan that did not exist, which is
///    worse than useless: a scanner that always cries orphan is a scanner
///    nobody reads. Self and children are now excluded by pid.
/// 2. THE SCANNER COULD NOT SEE THE REAL WORK. The work runs as the task's own
///    argv (`sleep 120`), which does not contain the nonce anywhere, so a
///    genuine orphan would have been INVISIBLE. The runner records the session
///    leader in `$root/.pid`, so the scan now checks that pid for liveness as
///    its primary signal and keeps the `ps` sweep as a secondary one for a
///    stray that escaped the session.
const REMOTE_SCAN: &str = r#"
set -u
nonce="$1"
self=$$
root="${TMPDIR:-/tmp}/wayland-f25-$nonce"
# Primary signal: the session leader this nonce's runner recorded.
if [ -f "$root/.pid" ]; then
  pid=$(cat "$root/.pid" 2>/dev/null || echo "")
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    echo "session-leader $pid still alive for nonce $nonce"
  fi
fi
# Secondary sweep for a stray that left the session.
#
# Pick a reader this far end actually SUPPORTS, and say so when none does.
# `ps -eo` is procps; msys and BusyBox ps both reject it. The column layout
# differs between the two forms, so the self-exclusion columns move with it:
#   ps -eo pid,ppid,args  ->  pid $1, ppid $2
#   ps -ef                ->  pid $2, ppid $3
table=""
pidcol=1
ppidcol=2
if table=$(ps -eo pid,ppid,args 2>/dev/null) && [ -n "$table" ]; then
  pidcol=1; ppidcol=2
elif table=$(ps -ef 2>/dev/null) && [ -n "$table" ]; then
  pidcol=2; ppidcol=3
else
  # A sweep that could not run is NOT a sweep that found nothing.
  echo "__WAYLAND_SWEEP_UNAVAILABLE__ no supported ps invocation on this far end"
  table=""
fi
if [ -n "$table" ]; then
  # Exclude this scan's own process and its children: the nonce travels on
  # this script's own argv, so an unguarded match finds the scan itself.
  echo "$table" \
    | grep -F -- "$nonce" \
    | grep -v -F -- "grep" \
    | awk -v s="$self" -v p="$pidcol" -v q="$ppidcol" '$p != s && $q != s' \
    || true
fi
"#;

/// Constant remote killer. Signals the remote SESSION, not the connection.
///
/// The same self-match defect applied here: the original `pkill -f <nonce>`
/// matched this script's own `sh -s -- <nonce>` argv and killed the killer
/// mid-run, which is why the live run reported `remote kill failed:` with an
/// EMPTY stderr while the work had in fact died. A cleanup that reports
/// failure when it succeeded trains the reader to ignore it.
const REMOTE_KILL: &str = r#"
set -u
nonce="$1"
self=$$
root="${TMPDIR:-/tmp}/wayland-f25-$nonce"
if [ -f "$root/.pid" ]; then
  pid=$(cat "$root/.pid" 2>/dev/null || echo "")
  if [ -n "$pid" ]; then
    kill -TERM "-$pid" 2>/dev/null || true
    sleep 1
    kill -KILL "-$pid" 2>/dev/null || true
  fi
fi
# Sweep any stray that left the session, never this script or its children.
# Same reader selection as REMOTE_SCAN: `ps -eo` is procps-only, so a far end
# with msys or BusyBox ps would otherwise sweep nothing while looking fine.
table=""
pidcol=1
ppidcol=2
if table=$(ps -eo pid,ppid,args 2>/dev/null) && [ -n "$table" ]; then
  pidcol=1; ppidcol=2
elif table=$(ps -ef 2>/dev/null) && [ -n "$table" ]; then
  pidcol=2; ppidcol=3
fi
if [ -n "$table" ]; then
  for p in $(echo "$table" \
               | grep -F -- "$nonce" \
               | grep -v -F -- "grep" \
               | awk -v s="$self" -v p="$pidcol" -v q="$ppidcol" \
                     '$p != s && $q != s {print $p}'); do
    kill -KILL "$p" 2>/dev/null || true
  done
else
  echo "stray-sweep-unavailable: no supported ps invocation on this far end"
fi
rm -rf "$root" 2>/dev/null || true
echo "remote-kill-issued"
exit 0
"#;

async fn remote_exec(
    target: &str,
    script: &str,
    argument: &str,
) -> std::result::Result<String, String> {
    if target.is_empty() {
        return Err("no ssh target recorded for this task".into());
    }
    let mut args = ssh_base_args(target);
    args.push("sh".into());
    args.push("-s".into());
    args.push("--".into());
    // The nonce reaching here is NOT always a validated identifier: `backend
    // scan --task-id <X>` passes an operator-or-caller string straight through
    // to `scan_orphans`, so this argument is the one that was measured
    // executing `id` as root on the far end before it was quoted.
    args.push(posix_quote(argument));
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut command = wcore_config::shell::shell_command_argv("ssh", &borrowed);
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    {
        use tokio::io::AsyncWriteExt as _;
        let mut stdin = child.stdin.take().ok_or("ssh stdin was not piped")?;
        stdin
            .write_all(script.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stdin.shutdown().await.map_err(|e| e.to_string())?;
    }
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait_with_output())
        .await
        .map_err(|_| "the remote command did not answer within 30s".to_string())?
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn remote_kill(target: &str, nonce: &str) -> String {
    match remote_exec(target, REMOTE_KILL, nonce).await {
        Ok(out) => out.trim().to_string(),
        Err(detail) => format!("remote kill failed: {detail}"),
    }
}

/// Rows the far end reported, or an error naming why the sweep could not run.
///
/// An `Err` here becomes `enumerated: false` at the call site, i.e. NOT
/// MEASURED — never zero. When the primary signal DID find something before
/// the sweep failed, those rows are carried into the error text so the operator
/// still sees them: the count is unknowable, but "at least this one" is not.
async fn remote_scan(target: &str, nonce: &str) -> std::result::Result<Vec<String>, String> {
    let out = remote_exec(target, REMOTE_SCAN, nonce).await?;
    let mut rows: Vec<String> = Vec::new();
    for line in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(detail) = line.strip_prefix(SWEEP_UNAVAILABLE) {
            let mut reason = format!(
                "the far end's process table could not be enumerated ({}), so a count \
                 would omit an unknown number of processes",
                detail.trim()
            );
            if !rows.is_empty() {
                // Do not lose a positive finding to an unmeasurable total.
                reason.push_str(" — and the primary signal DID find: ");
                reason.push_str(&rows.join("; "));
            }
            return Err(reason);
        }
        rows.push(line.to_string());
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The quoting the far end's shell needs, checked against the exact three
    /// shapes that were measured breaking on a live far end on 2026-07-28.
    #[test]
    fn every_value_crossing_the_connection_is_quoted_for_the_far_end_shell() {
        // An EMPTY value. Unquoted this vanished from ssh's remote command
        // string entirely and every later argument shifted left, so the task's
        // base64 input was read as its argv. It must survive as `''`.
        assert_eq!(posix_quote(""), "''");

        // A value with a SPACE. Unquoted the far end split it into two
        // arguments.
        assert_eq!(posix_quote("hello world"), "'hello world'");

        // A value with shell METACHARACTERS. Unquoted this executed on the far
        // end as root. Quoted, every byte is literal — and critically the
        // result contains no unquoted `;`.
        let payload = "x;id>/tmp/w;echo y";
        let quoted = posix_quote(payload);
        assert_eq!(quoted, "'x;id>/tmp/w;echo y'");
        assert!(quoted.starts_with('\'') && quoted.ends_with('\''));

        // A literal single quote is the one byte that cannot appear inside a
        // single-quoted string. Close, escaped quote, reopen.
        assert_eq!(posix_quote("it's"), r#"'it'\''s'"#);
        // The escape must not be a way back out: a value that TRIES to close
        // the quoting and append a command stays inert.
        let escape = posix_quote("a';id>/tmp/w;'b");
        assert_eq!(escape, r#"'a'\'';id>/tmp/w;'\''b'"#);
        // NOTE: whether that string is actually inert is a question about a
        // SHELL, and this file deliberately does not answer it by hand. A
        // first draft of this test hand-rolled an even/odd quote counter and
        // called the correct output an escape, because the counter did not
        // model `\'`. The real round-trip — feed the quoted form to a real
        // `sh` and compare what comes back — lives in
        // `tests/ssh_far_end_quoting.rs`, where it can use a shell without
        // tripping this module's own no-shell-string guard.
    }

    /// A positive control on the test above: the assertions must be capable of
    /// failing. An identity "quoting" function has to break every one of them.
    #[test]
    fn the_quoting_assertions_would_fail_without_quoting() {
        fn unquoted(value: &str) -> String {
            value.to_string()
        }
        assert_ne!(unquoted(""), "''", "the empty-value assertion is vacuous");
        assert_ne!(
            unquoted("hello world"),
            "'hello world'",
            "the space assertion is vacuous"
        );
        assert_ne!(
            unquoted("x;id>/tmp/w;echo y"),
            "'x;id>/tmp/w;echo y'",
            "the metacharacter assertion is vacuous"
        );
    }

    /// The three places task-supplied bytes reach the wire must all quote.
    ///
    /// Every needle is ASSEMBLED at runtime, for the reason the guard below
    /// already states: a literal needle appears in this file's own source, so
    /// the scan finds itself. A first draft wrote them as literals and the
    /// negative assertions failed against the test's own text — a self-match
    /// that would have been read as a real regression.
    #[test]
    fn every_wire_path_quotes_its_arguments() {
        let source = include_str!("ssh.rs");
        let q = ["posix", "_quote"].concat();

        // `execute` — nonce, base64 input, and every element of task argv.
        for tail in ["(&task.nonce)", "(&input_b64)", "(a)"] {
            let needle = format!("{q}{tail}");
            assert!(
                source.contains(&needle),
                "a value crossing the connection is unquoted: {needle}"
            );
        }
        // `remote_exec` — the argument carrying the nonce for the scan and the
        // kill. This is the one `backend scan --task-id` reaches, and unlike
        // `execute`'s nonce it is NOT identifier-validated first.
        assert!(source.contains(&format!("{q}(argument)")));

        // And no raw task value may sit alongside them.
        let raw_nonce = ["args.push(task.non", "ce.clone());"].concat();
        assert!(
            !source.contains(&raw_nonce),
            "an unquoted nonce is back on the wire"
        );
        let raw_argv = ["args.extend(task.argv.iter().clon", "ed());"].concat();
        assert!(
            !source.contains(&raw_argv),
            "unquoted task argv is back on the wire"
        );

        // Positive control: the needle-assembly must be capable of finding
        // something that is genuinely absent, or the negatives prove nothing.
        let absent = ["args.push(nothing_like_this", "_exists());"].concat();
        assert!(!source.contains(&absent));
        assert!(source.contains(&q), "the assembled needle matches nothing");
    }

    /// A sweep that could not run must be distinguishable from one that found
    /// nothing. Measured against a real Windows far end on 2026-07-28: msys
    /// `ps` rejects `-eo`, stderr went to `/dev/null`, the pipeline ended in
    /// `|| true`, and the surface reported `0 (MEASURED)` while two independent
    /// instruments saw the orphan.
    #[test]
    fn a_far_end_with_no_supported_ps_reports_not_measured_rather_than_zero() {
        // The scan must offer a fallback reader and a marker when neither works.
        assert!(REMOTE_SCAN.contains("ps -eo pid,ppid,args"));
        assert!(
            REMOTE_SCAN.contains("ps -ef"),
            "a far end whose ps rejects -eo must still be enumerable"
        );
        assert!(
            REMOTE_SCAN.contains(SWEEP_UNAVAILABLE),
            "an unrunnable sweep must announce itself, not return empty"
        );
        // The column layout differs between the two readers, so the
        // self-exclusion must move with it or the scan excludes the wrong pid.
        assert!(REMOTE_SCAN.contains("pidcol=2; ppidcol=3"));
        assert!(REMOTE_SCAN.contains(r#"'$p != s && $q != s'"#));
        // The kill's stray sweep has the same blindness and the same fallback.
        assert!(REMOTE_KILL.contains("ps -ef"));

        // And the parser must turn the marker into an error — which the call
        // site renders as NOT MEASURED — rather than treating it as a row.
        let marked = format!("session-leader 42 still alive\n{SWEEP_UNAVAILABLE} msys ps\n");
        let mut rows: Vec<String> = Vec::new();
        let mut failed: Option<String> = None;
        for line in marked.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if line.starts_with(SWEEP_UNAVAILABLE) {
                failed = Some(line.to_string());
                break;
            }
            rows.push(line.to_string());
        }
        assert!(
            failed.is_some(),
            "the marker must stop the scan being read as a clean list"
        );
        assert_eq!(
            rows.len(),
            1,
            "a positive primary finding must survive to be reported in the reason"
        );

        // Positive control: an ordinary two-row answer must NOT trip the marker,
        // or every scan would report NOT MEASURED and the check proves nothing.
        let ordinary = "session-leader 42 still alive\n123 456 sh -c work\n";
        assert!(
            !ordinary
                .lines()
                .any(|l| l.trim().starts_with(SWEEP_UNAVAILABLE)),
            "an ordinary scan must still be measurable"
        );
    }

    #[test]
    fn the_module_contains_no_shell_string_execution_path() {
        // Necessary, and — as the module doc now says — NOT sufficient. This
        // checks the LOCAL spawn only. It passed unchanged while the far end
        // was executing injected shell, which is exactly why the quoting tests
        // above exist alongside it rather than instead of it.
        //
        // The needles are ASSEMBLED at runtime rather than written as literals.
        // A literal would appear in this file's own source and the scan would
        // find itself — a self-matching guard that fails for the wrong reason
        // is worse than no guard, because the next person deletes it.
        let source = include_str!("ssh.rs");
        let builder_needle = ["shell_command", "_builder"].concat();
        assert!(
            !source.contains(&builder_needle),
            "shell-string mode must not appear in the ssh backend"
        );
        let shell_string_needle = ["shell::shell_command", "("].concat();
        assert_eq!(
            source.matches(&shell_string_needle).count(),
            0,
            "the ssh backend must use argv mode exclusively"
        );
        // Positive control: the guard must actually be capable of finding
        // something, or it proves nothing.
        let argv_needle = ["shell_command", "_argv("].concat();
        assert!(
            source.matches(&argv_needle).count() >= 3,
            "the ssh backend should be driving ssh through argv mode in several places"
        );
    }

    #[test]
    fn the_remote_runner_binds_task_values_as_positional_arguments() {
        // If a task value were interpolated into the script text it could be
        // re-parsed as script. It must arrive as $1/$2/"$@" instead.
        assert!(REMOTE_RUNNER.contains(r#"nonce="$1""#));
        assert!(REMOTE_RUNNER.contains(r#"b64input="$1""#));
        assert!(REMOTE_RUNNER.contains(r#"setsid "$@""#));
        assert!(REMOTE_SCAN.contains(r#"nonce="$1""#));
        assert!(REMOTE_KILL.contains(r#"nonce="$1""#));
    }

    #[test]
    fn the_remote_runner_starts_its_own_session_so_cancellation_can_reach_it() {
        assert!(
            REMOTE_RUNNER.contains("setsid"),
            "without its own session the remote work survives a cancellation"
        );
        assert!(
            REMOTE_KILL.contains(r#"kill -TERM "-$pid""#),
            "cancellation must signal the remote process GROUP, not one pid"
        );
    }

    #[test]
    fn the_remote_scan_and_kill_exclude_their_own_process() {
        // Found live on 2026-07-26: the nonce travels on these scripts' own
        // argv, so an unguarded `ps | grep <nonce>` matches the scan itself.
        // The scan then always reports one orphan that does not exist, and the
        // killer kills itself mid-run and reports a failure that did not happen.
        //
        // The exclusion is unchanged in intent; only its spelling moved. The
        // pid/ppid COLUMNS differ between `ps -eo pid,ppid,args` and the
        // `ps -ef` fallback added for far ends whose ps rejects `-eo`, so the
        // comparison is now against the column variables rather than the
        // literal `$1`/`$2`. Weakening this guard was never an option — a scan
        // that finds itself reports an orphan that does not exist.
        for script in [REMOTE_SCAN, REMOTE_KILL] {
            assert!(
                script.contains("self=$$"),
                "the script must know its own pid to exclude itself"
            );
            assert!(
                script.contains(r#"$p != s && $q != s"#),
                "the script must exclude its own pid and its children from the match"
            );
            // And the columns must actually be set for BOTH readers, or the
            // exclusion compares the wrong field and silently stops working.
            assert!(
                script.contains("pidcol=1; ppidcol=2") && script.contains("pidcol=2; ppidcol=3"),
                "the self-exclusion columns must track the reader that was chosen"
            );
        }
    }

    #[test]
    fn the_scan_can_see_work_whose_argv_does_not_carry_the_nonce() {
        // The second live defect: the task's own argv (`sleep 120`) contains
        // no nonce, so a pure `ps | grep <nonce>` sweep could never have found
        // a genuine orphan. The recorded session leader is the primary signal.
        assert!(REMOTE_RUNNER.contains(r#"echo "$child" > "$root/.pid""#));
        assert!(REMOTE_SCAN.contains(r#""$root/.pid""#));
        assert!(REMOTE_SCAN.contains("session-leader"));
    }

    #[test]
    fn input_bytes_never_reach_the_far_end_as_script_text() {
        assert!(
            REMOTE_RUNNER.contains("base64 -d"),
            "input crosses as base64 on the argument vector, never as script"
        );
        assert!(REMOTE_RUNNER.contains(crate::contract::INPUT_FILE_NAME));
    }
}
