//! The SSH reference backend.
//!
//! Two rules govern this module and neither is negotiable.
//!
//! 1. ARGV MODE ONLY. AGENTS.md states that any command whose arguments
//!    include non-literal data must use `shell_command_argv`, never
//!    shell-string mode, because argv mode never lets a metacharacter reach an
//!    interpreter. A remote-execution backend is the most attacker-adjacent
//!    surface in this phase, so there is NO shell-string path in this file at
//!    all. There is no `format!`-into-a-command anywhere below.
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
    ExecutionTask, Health, HibernationObservation, OrphanScan, ProbeBasis, ResourceBudget,
    SecretChannel, validate_identifier,
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
/// `$1`, `$2`, … so it is never re-parsed as script text. That is the same
/// safety property argv mode gives locally, extended across the connection.
const REMOTE_RUNNER: &str = r#"
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
wait "$child"
status=$?
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
        args.push(task.nonce.clone());
        args.push(input_b64);
        args.extend(task.argv.iter().cloned());
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
}

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
# Secondary sweep for a stray that left the session, excluding this scan's own
# process and its children.
ps -eo pid,ppid,args 2>/dev/null \
  | grep -F -- "$nonce" \
  | grep -v -F -- "grep" \
  | awk -v s="$self" '$1 != s && $2 != s' \
  || true
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
for p in $(ps -eo pid,ppid,args 2>/dev/null \
             | grep -F -- "$nonce" \
             | grep -v -F -- "grep" \
             | awk -v s="$self" '$1 != s && $2 != s {print $1}'); do
  kill -KILL "$p" 2>/dev/null || true
done
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
    args.push(argument.to_string());
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

async fn remote_scan(target: &str, nonce: &str) -> std::result::Result<Vec<String>, String> {
    let out = remote_exec(target, REMOTE_SCAN, nonce).await?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_contains_no_shell_string_execution_path() {
        // The safety property this whole module rests on, asserted rather than
        // left in a comment: nothing here builds a `sh -c` string.
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
        for script in [REMOTE_SCAN, REMOTE_KILL] {
            assert!(
                script.contains("self=$$"),
                "the script must know its own pid to exclude itself"
            );
            assert!(
                script.contains(r#"'$1 != s && $2 != s"#),
                "the script must exclude its own pid and its children from the match"
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
