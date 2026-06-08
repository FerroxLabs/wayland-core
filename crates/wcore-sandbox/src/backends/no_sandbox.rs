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
use crate::{ResourceLimitEnforcement, SandboxCommand, SandboxOutput};
use async_trait::async_trait;
use std::sync::Once;

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
        // S9: kill the child if this future is dropped (e.g. when a caller
        // races us against a timeout / cancellation token). Without this
        // a dropped `output()` future leaves a zombie subprocess — the
        // same reliability blocker `wcore_config::shell` fixed for the
        // shell helpers. Routing BashTool through the sandbox must not
        // reintroduce that leak.
        builder.kill_on_drop(true);
        // Scrub host env, then inject only what the manifest declares.
        // Mirrors the real backends so disabling sandbox does not silently
        // leak host secrets to the child.
        builder.env_clear();
        for (k, v) in &manifest.env {
            builder.env(k, v);
        }
        let output = builder
            .output()
            .await
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        Ok(SandboxOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            resource_limits: ResourceLimitEnforcement::None,
        })
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
