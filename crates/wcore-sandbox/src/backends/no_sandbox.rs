//! NoSandbox backend — runs the command directly via
//! `tokio::process::Command`, NO isolation. Used when the platform's
//! primary sandbox is unavailable. Emits a warn-once log so operators
//! know they are running unsandboxed.
//!
//! The host env is NOT inherited: the child receives only the explicit
//! `env` entries from the manifest. This matches the security
//! contract of the real backends so flipping `WAYLAND_SANDBOX=none`
//! does not silently widen env exposure (Audit B H5).

use super::SandboxBackend;
use crate::error::{Result, SandboxError};
use crate::manifest::SandboxManifest;
use crate::{ResourceLimitEnforcement, SandboxChunk, SandboxCommand, SandboxOutput};
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::{Arc, Once};
use tokio::io::AsyncReadExt;

static WARN_ONCE: Once = Once::new();

/// Emit a single warn-level log for the lifetime of the process telling
/// the operator that sandboxing is disabled.
pub fn warn_once_sandbox_disabled() {
    WARN_ONCE.call_once(|| {
        tracing::warn!(
            target: "wcore_sandbox",
            "sandbox DISABLED — child processes run with host permissions. \
             Install bubblewrap (Linux), or set WAYLAND_SANDBOX=docker for opt-in Docker.",
        );
    });
}

pub struct NoSandboxBackend;

impl NoSandboxBackend {
    pub fn new() -> Self {
        Self
    }

    fn command(
        manifest: &SandboxManifest,
        cmd: &SandboxCommand,
    ) -> Result<tokio::process::Command> {
        let program = cmd
            .argv
            .first()
            .ok_or_else(|| SandboxError::ExecFailed("empty argv".into()))?;
        let mut builder = tokio::process::Command::new(program);
        if cmd.argv.len() > 1 {
            builder.args(&cmd.argv[1..]);
        }
        if let Some(cwd) = &cmd.cwd {
            builder.current_dir(cwd);
        }
        builder.kill_on_drop(true);
        super::process_tree::isolate(&mut builder);
        builder.env_clear();
        for (k, v) in &manifest.env {
            builder.env(k, v);
        }
        builder.stdout(Stdio::piped()).stderr(Stdio::piped());
        Ok(builder)
    }
}

impl Default for NoSandboxBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxBackend for NoSandboxBackend {
    fn name(&self) -> &'static str {
        "no_sandbox"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        // S9: kill the child if this future is dropped (e.g. when a caller
        // races us against a timeout / cancellation token). Without this
        // a dropped `output()` future leaves a zombie subprocess — the
        // same reliability blocker `wcore_config::shell` fixed for the
        // shell helpers. Routing BashTool through the sandbox must not
        // reintroduce that leak.
        let child = Self::command(manifest, &cmd)?
            .spawn()
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        let mut process_tree = super::process_tree::ProcessTreeGuard::new(child.id());
        let output = child
            .wait_with_output()
            .await
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        process_tree.disarm();
        Ok(SandboxOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            resource_limits: ResourceLimitEnforcement::None,
        })
    }

    fn execute_streaming(
        self: Arc<Self>,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<tokio::sync::mpsc::Receiver<SandboxChunk>> {
        let mut child = Self::command(manifest, &cmd)?
            .spawn()
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::ExecFailed("child stdout was not piped".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::ExecFailed("child stderr was not piped".into()))?;
        let process_tree = super::process_tree::ProcessTreeGuard::new(child.id());
        let (tx, rx) = tokio::sync::mpsc::channel(super::STREAM_CHANNEL_CAP);

        tokio::spawn(async move {
            let mut process_tree = process_tree;
            let mut stdout_open = true;
            let mut stderr_open = true;
            let mut stdout_buf = [0_u8; 8 * 1024];
            let mut stderr_buf = [0_u8; 8 * 1024];
            let mut exit_code = None;
            let wait = child.wait();
            tokio::pin!(wait);

            while stdout_open || stderr_open || exit_code.is_none() {
                tokio::select! {
                    _ = tx.closed() => return,
                    read = stdout.read(&mut stdout_buf), if stdout_open => match read {
                        Ok(0) => stdout_open = false,
                        Ok(n) => {
                            if tx.send(SandboxChunk::Stdout(stdout_buf[..n].to_vec())).await.is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = tx.send(SandboxChunk::Stderr(
                                format!("failed to read child stdout: {error}").into_bytes(),
                            )).await;
                            return;
                        }
                    },
                    read = stderr.read(&mut stderr_buf), if stderr_open => match read {
                        Ok(0) => stderr_open = false,
                        Ok(n) => {
                            if tx.send(SandboxChunk::Stderr(stderr_buf[..n].to_vec())).await.is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = tx.send(SandboxChunk::Stderr(
                                format!("failed to read child stderr: {error}").into_bytes(),
                            )).await;
                            return;
                        }
                    },
                    status = &mut wait, if exit_code.is_none() => match status {
                        Ok(status) => exit_code = Some(status.code().unwrap_or(-1)),
                        Err(error) => {
                            let _ = tx.send(SandboxChunk::Stderr(
                                format!("failed to wait for child: {error}").into_bytes(),
                            )).await;
                            return;
                        }
                    },
                }
            }

            process_tree.disarm();
            let _ = tx
                .send(SandboxChunk::Exit {
                    exit_code: exit_code.expect("loop exits only after child status is available"),
                    resource_limits: ResourceLimitEnforcement::None,
                })
                .await;
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve a real `echo` binary on disk. We do NOT inherit PATH (env
    /// is scrubbed by the backend), so the test passes an absolute path.
    fn echo_path() -> Option<&'static str> {
        ["/bin/echo", "/usr/bin/echo"]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())
    }

    #[tokio::test]
    async fn echo_runs() {
        let Some(echo) = echo_path() else {
            eprintln!("skip: no /bin/echo or /usr/bin/echo on this host");
            return;
        };
        let backend = NoSandboxBackend::new();
        let out = backend
            .execute(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec![echo.into(), "hi".into()],
                    cwd: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
        assert_eq!(out.resource_limits, ResourceLimitEnforcement::None);
    }

    #[tokio::test]
    async fn empty_argv_is_error() {
        let backend = NoSandboxBackend::new();
        let err = backend
            .execute(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec![],
                    cwd: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, SandboxError::ExecFailed(_)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_stream_kills_direct_child_and_background_descendant() {
        use std::sync::Arc;

        fn process_running(pid: u32) -> bool {
            // SAFETY: signal 0 only checks whether the process exists.
            if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
                return false;
            }
            #[cfg(target_os = "linux")]
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                && let Some((_, fields)) = stat.rsplit_once(") ")
                && fields.starts_with('Z')
            {
                return false;
            }
            true
        }

        async fn read_pid(path: &std::path::Path) -> u32 {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if let Ok(raw) = std::fs::read_to_string(path)
                        && let Ok(pid) = raw.trim().parse()
                    {
                        break pid;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("child must publish its PID")
        }

        async fn wait_gone(pid: u32) {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while process_running(pid) {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("process group member must die after receiver drop");
        }

        let dir = tempfile::tempdir().unwrap();
        let shell_pid_file = dir.path().join("shell.pid");
        let child_pid_file = dir.path().join("child.pid");
        let script = format!(
            "echo $$ > '{}'; sleep 30 & echo $! > '{}'; wait",
            shell_pid_file.display(),
            child_pid_file.display()
        );
        let backend = Arc::new(NoSandboxBackend::new());
        let rx = backend
            .execute_streaming(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec!["/bin/sh".into(), "-c".into(), script],
                    cwd: None,
                },
            )
            .unwrap();
        let shell_pid = read_pid(&shell_pid_file).await;
        let child_pid = read_pid(&child_pid_file).await;
        assert!(process_running(shell_pid));
        assert!(process_running(child_pid));

        drop(rx);

        wait_gone(shell_pid).await;
        wait_gone(child_pid).await;
    }

    #[test]
    fn warn_once_is_idempotent() {
        // The warn-once contract: calling `warn_once_sandbox_disabled` any
        // number of times is safe and produces exactly one warn over the
        // lifetime of the process. We cannot directly observe the log line
        // from inside the test binary (no tracing subscriber wired here),
        // so we instead assert via `Once::is_completed()` that the `Once`
        // transitions to the completed state and stays there across
        // repeated calls.
        //
        // Note: `WARN_ONCE` is process-global; other tests in this binary
        // may have already invoked it. That is fine — completion is
        // monotonic, so the assertions below hold either way.
        warn_once_sandbox_disabled();
        assert!(
            WARN_ONCE.is_completed(),
            "first call must mark Once complete"
        );
        // Repeated calls must not panic and must not flip the state.
        for _ in 0..5 {
            warn_once_sandbox_disabled();
        }
        assert!(
            WARN_ONCE.is_completed(),
            "Once remains complete after repeats"
        );
    }

    #[tokio::test]
    async fn execute_streaming_yields_chunks_then_exit() {
        use crate::SandboxChunk;
        use std::sync::Arc;
        let Some(echo) = echo_path() else {
            eprintln!("skip: no /bin/echo or /usr/bin/echo on this host");
            return;
        };
        let backend: Arc<NoSandboxBackend> = Arc::new(NoSandboxBackend::new());
        let mut rx = backend
            .execute_streaming(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec![echo.into(), "stream_hi".into()],
                    cwd: None,
                },
            )
            .expect("execute_streaming must return a receiver");

        let mut stdout = Vec::new();
        let mut exit = None;
        while let Some(chunk) = rx.recv().await {
            match chunk {
                SandboxChunk::Stdout(b) => stdout.extend_from_slice(&b),
                SandboxChunk::Stderr(_) => {}
                SandboxChunk::Exit {
                    exit_code,
                    resource_limits,
                } => {
                    exit = Some((exit_code, resource_limits));
                }
            }
        }
        assert_eq!(
            String::from_utf8_lossy(&stdout).trim(),
            "stream_hi",
            "stdout chunk must carry the child's output"
        );
        let (code, limits) = exit.expect("a terminal Exit chunk must arrive");
        assert_eq!(code, 0);
        assert_eq!(limits, ResourceLimitEnforcement::None);
    }

    #[tokio::test]
    async fn env_is_scrubbed_then_repopulated() {
        // Skip on hosts without `/usr/bin/env` (e.g. Windows CI). The
        // backend MUST scrub host env then inject only manifest env.
        let env_bin = "/usr/bin/env";
        if !std::path::Path::new(env_bin).exists() {
            eprintln!("skip: no /usr/bin/env on this host");
            return;
        }
        // SAFETY: test-only env mutation; serial-tests would be nicer but
        // the key is unique to this test and no other thread reads it.
        unsafe {
            std::env::set_var("WAYLAND_SANDBOX_TEST_LEAK", "leaked");
        }
        let backend = NoSandboxBackend::new();
        let mut manifest = SandboxManifest::default();
        manifest.env.push(("FOO".into(), "bar".into()));
        let out = backend
            .execute(
                &manifest,
                SandboxCommand {
                    argv: vec![env_bin.into()],
                    cwd: None,
                },
            )
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("FOO=bar"), "FOO must be set: {stdout}");
        assert!(
            !stdout.contains("WAYLAND_SANDBOX_TEST_LEAK"),
            "host env must be scrubbed: {stdout}"
        );
    }
}
