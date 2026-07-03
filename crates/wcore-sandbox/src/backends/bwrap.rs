//! Bubblewrap backend (Linux) — wraps `bwrap` binary as a child process.
//! Tier 0 default on Linux per cross-platform strategy.
//!
//! Audit-corrected flag set:
//!   --die-with-parent          (kill child if engine dies)
//!   --unshare-all              (PID/IPC/network/UTS/cgroup/user — includes net so --unshare-net is redundant)
//!   --clearenv                 (drop host env; manifest.env injected via --setenv)
//!   --new-session              (block terminal-escape vectors)
//!   --tmpfs /tmp               (many commands need /tmp; without it commands fail EACCES)
//!   --proc /proc --dev /dev    (minimal /proc + /dev)
//!   --ro-bind /usr /usr        (allow standard binaries to run)
//!   --ro-bind /lib /lib        (and libs for executables)
//!   --ro-bind /lib64 /lib64    (64-bit libs if present)
//!   --bind <fs_write_allow> <fs_write_allow>      (writable mounts)
//!   --ro-bind <fs_read_allow> <fs_read_allow>     (readable mounts)
//!   --setenv KEY VAL           (per-key env injection)
//!   --chdir <cwd>              (working dir)
//!
//! NetworkPolicy::Inherit → omit `--unshare-net` (use `--unshare-pid --unshare-ipc` etc.)
//! NetworkPolicy::Deny    → `--unshare-net` (no network namespace)
//! NetworkPolicy::AllowHosts(_) → Err(PolicyNotSupported) — bwrap has no DNS gate.
//!   (Future v0.6.4: nftables egress filter inside namespace.)
//!
//! Resource limits enforced via `--rlimit-as` / pre-exec setrlimit wrapper.
//! Returns `ResourceLimitEnforcement::BestEffort` because rlimit is subject
//! to OOM-killer races and Linux's overcommit semantics.

use super::SandboxBackend;
use crate::error::{Result, SandboxError};
use crate::manifest::{NetworkPolicy, SandboxManifest};
use crate::{ResourceLimitEnforcement, SandboxCommand, SandboxOutput};
use async_trait::async_trait;
use std::path::Path;
use std::process::Stdio;
use std::sync::Once;

/// System directories bound read-only into every bwrap sandbox so the inner
/// command can find standard binaries and their shared libraries.
const SYSTEM_RO_DIRS: [&str; 6] = ["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc"];

#[cfg(all(target_os = "linux", feature = "seccomp"))]
static SECCOMP_UNAVAILABLE_WARN: Once = Once::new();
/// Warns once if a manifest asks for `SyscallPolicy::Strict` but this
/// build was compiled without the `seccomp` feature — so the operator
/// knows the strict syscall filter is NOT being applied rather than
/// silently assuming it is.
#[cfg(not(all(target_os = "linux", feature = "seccomp")))]
static SECCOMP_FEATURE_OFF_WARN: Once = Once::new();

pub struct BubblewrapBackend {
    bwrap_path: Option<String>,
}

impl BubblewrapBackend {
    pub fn new() -> Self {
        Self {
            bwrap_path: which::which("bwrap")
                .ok()
                .map(|p| p.to_string_lossy().into_owned()),
        }
    }
}

impl Default for BubblewrapBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxBackend for BubblewrapBackend {
    fn name(&self) -> &'static str {
        "bubblewrap"
    }

    fn is_available(&self) -> bool {
        self.bwrap_path.is_some()
    }

    fn enforces_read_deny(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        // 1. AllowHosts unsupported: bwrap has no DNS gate.
        if let NetworkPolicy::AllowHosts(_) = manifest.network {
            return Err(SandboxError::PolicyNotSupported(
                "bubblewrap has no DNS gate; NetworkPolicy::AllowHosts is unsupported".into(),
            ));
        }

        // 2. Backend availability.
        let bwrap_path = self.bwrap_path.as_deref().ok_or_else(|| {
            SandboxError::ExecFailed("bwrap not in PATH; install bubblewrap".into())
        })?;

        // 3. Validate every fs_read_allow / fs_write_allow / fs_read_deny path is absolute.
        for p in manifest
            .fs_read_allow
            .iter()
            .chain(manifest.fs_write_allow.iter())
            .chain(manifest.fs_read_deny.iter())
        {
            if !p.is_absolute() {
                return Err(SandboxError::PathDenied(format!(
                    "sandbox manifest paths must be absolute: {}",
                    p.display()
                )));
            }
        }

        // argv[0] must exist as a sanity check (don't bother probing inside the
        // namespace; bwrap will fail clearly enough if the binary is missing).
        let program = cmd
            .argv
            .first()
            .ok_or_else(|| SandboxError::ExecFailed("empty argv".into()))?
            .clone();

        // 4. Assemble bwrap argv.
        let mut bwrap_argv: Vec<String> = Vec::with_capacity(64 + cmd.argv.len() * 2);

        // Lifecycle + isolation.
        bwrap_argv.push("--die-with-parent".into());
        bwrap_argv.push("--unshare-all".into());
        // --unshare-all already shares-nothing including network. If the
        // manifest requested Inherit network, give the child the host net ns
        // back via --share-net.
        match manifest.network {
            NetworkPolicy::Inherit => {
                bwrap_argv.push("--share-net".into());
            }
            NetworkPolicy::Deny => { /* default of unshare-all */ }
            NetworkPolicy::AllowHosts(_) => unreachable!("rejected above"),
        }
        bwrap_argv.push("--clearenv".into());
        bwrap_argv.push("--new-session".into());

        // Minimal filesystem skeleton.
        bwrap_argv.push("--tmpfs".into());
        bwrap_argv.push("/tmp".into());
        bwrap_argv.push("--proc".into());
        bwrap_argv.push("/proc".into());
        bwrap_argv.push("--dev".into());
        bwrap_argv.push("/dev".into());

        // Standard system mounts (best-effort: skip silently if the path does
        // not exist on this host, e.g. /lib64 on pure-multilib distros).
        for sys in SYSTEM_RO_DIRS {
            if Path::new(sys).exists() {
                bwrap_argv.push("--ro-bind".into());
                bwrap_argv.push(sys.into());
                bwrap_argv.push(sys.into());
            }
        }

        // Manifest-declared mounts. Use the `--*-bind-try` variants so a
        // declared source that does not exist on THIS host is silently
        // skipped instead of aborting the whole spawn (bwrap treats a plain
        // `--bind` with a missing source as a fatal "Can't find source
        // path"). wayland#552: `WorkspacePolicy::trusted_local` adds the
        // user's `~/.cache`/`.cargo`/`.npm`/`.rustup` unconditionally, but on
        // a fresh HOME (fresh profile, container, CI, a user who has never run
        // cargo/npm) those dirs are absent — with the plain bind that made
        // EVERY bash command fail-spawn with empty stdout, which a persistent
        // agent then loops on. `-try` matches the `Path::exists()` guard on
        // the system dirs above and the AppContainer backend's absent-cache
        // skip. A skipped WRITE mount is strictly better than a dead shell:
        // the command runs, and a build that needs the (still-absent) dir
        // fails on its own terms.
        for p in &manifest.fs_read_allow {
            let s = p.to_string_lossy().into_owned();
            bwrap_argv.push("--ro-bind-try".into());
            bwrap_argv.push(s.clone());
            bwrap_argv.push(s);
        }
        for p in &manifest.fs_write_allow {
            let s = p.to_string_lossy().into_owned();
            bwrap_argv.push("--bind-try".into());
            bwrap_argv.push(s.clone());
            bwrap_argv.push(s);
        }

        // Secret-read-deny overlays, after the positive binds so later-arg-wins
        // mount ordering shadows them. Caller (secret_deny_paths) emits ONLY
        // paths under a mounted root, so the parent always exists in the
        // namespace and the bind cannot fail-spawn. Stat at bind time to pick
        // file (mask with /dev/null) vs dir (mask with empty tmpfs); a vanished
        // path is skipped (nothing to read).
        for p in &manifest.fs_read_deny {
            match std::fs::symlink_metadata(p) {
                Ok(md) if md.is_dir() => {
                    bwrap_argv.push("--tmpfs".into());
                    bwrap_argv.push(p.to_string_lossy().into_owned());
                }
                Ok(_) => {
                    bwrap_argv.push("--ro-bind".into());
                    bwrap_argv.push("/dev/null".into());
                    bwrap_argv.push(p.to_string_lossy().into_owned());
                }
                Err(_) => { /* path gone since enumeration — nothing to mask */ }
            }
        }

        // Env injection (manifest-only; host env is dropped by --clearenv).
        for (k, v) in &manifest.env {
            bwrap_argv.push("--setenv".into());
            bwrap_argv.push(k.clone());
            bwrap_argv.push(v.clone());
        }

        // Working directory.
        if let Some(cwd) = &cmd.cwd {
            bwrap_argv.push("--chdir".into());
            bwrap_argv.push(cwd.to_string_lossy().into_owned());
        }

        // Resource limits — best-effort via bwrap's --rlimit-as for address
        // space.
        if let Some(max_mem) = manifest.max_memory_bytes {
            bwrap_argv.push("--rlimit-as".into());
            bwrap_argv.push(max_mem.to_string());
        }

        // S4 — seccomp-bpf (feature-gated, Linux-only). Compile the BPF
        // filter in-process and hand the fd to bwrap via `--seccomp <fd>`.
        // The tempfile is held alive until after spawn so the fd stays
        // valid; bwrap dup's it internally before the kernel applies it.
        #[allow(unused_variables, unused_mut)]
        let mut seccomp_file: Option<std::fs::File> = None;
        #[cfg(all(target_os = "linux", feature = "seccomp"))]
        {
            use std::os::fd::AsRawFd;
            match super::bwrap_seccomp::export_filter_to_tempfile(manifest.syscall_policy) {
                Ok(Some(file)) => {
                    let raw = file.as_raw_fd();
                    // SAFETY: fcntl(F_SETFD) on a fd we own is safe.
                    let rc = unsafe { libc::fcntl(raw, libc::F_SETFD, 0) };
                    if rc == -1 {
                        return Err(SandboxError::ExecFailed(format!(
                            "seccomp: clear FD_CLOEXEC failed: {}",
                            std::io::Error::last_os_error()
                        )));
                    }
                    bwrap_argv.push("--seccomp".into());
                    bwrap_argv.push(raw.to_string());
                    seccomp_file = Some(file);
                }
                Ok(None) => { /* SyscallPolicy::Inherit — no filter */ }
                Err(e) => {
                    SECCOMP_UNAVAILABLE_WARN.call_once(|| {
                        tracing::warn!(
                            target: "wcore_sandbox",
                            error = %e,
                            "seccomp filter could not be built; continuing with bwrap-only sandbox"
                        );
                    });
                }
            }
        }

        // If the manifest asked for a strict syscall filter but this build
        // has the `seccomp` feature compiled out, warn once so the
        // operator does not silently assume `SyscallPolicy::Strict` is
        // being enforced when it is not. The bwrap namespace + bind-mount
        // isolation still applies — only the seccomp-bpf layer is absent.
        #[cfg(not(all(target_os = "linux", feature = "seccomp")))]
        if matches!(
            manifest.syscall_policy,
            crate::manifest::SyscallPolicy::Strict
        ) {
            SECCOMP_FEATURE_OFF_WARN.call_once(|| {
                tracing::warn!(
                    target: "wcore_sandbox",
                    "SyscallPolicy::Strict requested but this build has the \
                     `seccomp` feature disabled; the strict syscall filter is \
                     NOT applied (bwrap namespace isolation still active)"
                );
            });
        }

        // Separator + user command.
        bwrap_argv.push("--".into());
        bwrap_argv.push(program);
        for a in &cmd.argv[1..] {
            bwrap_argv.push(a.clone());
        }

        // 5. Spawn.
        let mut command = tokio::process::Command::new(bwrap_path);
        command
            .args(&bwrap_argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            // Reap the bwrap process if our Child handle is dropped — the
            // timeout arm below relies on this to kill the namespace tree
            // instead of leaking it. Mirrors no_sandbox.rs. bwrap's
            // --die-with-parent then tears down the inner sandboxed process.
            .kill_on_drop(true);

        // NOTE: Landlock is deliberately NOT applied around the bwrap backend.
        // A `pre_exec` ruleset is inherited by bwrap and confines bwrap's OWN
        // privileged setup (writing /proc/self/uid_map to build its user
        // namespace), which fails with EACCES the moment the allowlist is
        // non-empty. bwrap's `--unshare-all` + the constructive `--ro-bind`
        // set already provide a deny-by-default filesystem view that is a strict
        // superset of any Landlock allowlist built from the same paths, and the
        // secret-read-deny enforcement rides on the `--ro-bind /dev/null` /
        // `--tmpfs` overlays above — not on Landlock. bwrap sets NO_NEW_PRIVS
        // itself. The `landlock` feature + `bwrap_landlock.rs` remain compiled
        // (exercised by --all-features CI) as the foundation for a future
        // inner-command re-exec shim, but production runs seccomp-only.

        let child = command
            .spawn()
            .map_err(|e| SandboxError::ExecFailed(format!("bwrap spawn failed: {e}")))?;

        // Now safe to drop the BPF tempfile — bwrap has read the fd into
        // its child setup. Holding it longer wastes a fd until return.
        drop(seccomp_file);

        // 6. Timeout + wait.
        let timeout = manifest
            .timeout
            .unwrap_or_else(|| std::time::Duration::from_secs(30));

        let wait_fut = child.wait_with_output();
        let output = match tokio::time::timeout(timeout, wait_fut).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return Err(SandboxError::ExecFailed(format!("bwrap wait failed: {e}")));
            }
            Err(_elapsed) => {
                // `timeout` dropped `wait_fut` on elapse, which drops the
                // Child it owns. With `kill_on_drop(true)` set above, that
                // drop reaps the bwrap process; bwrap's --die-with-parent
                // then tears down the inner namespace tree — no pid escapes
                // our handle.
                return Err(SandboxError::Timeout);
            }
        };

        // 7. Return.
        Ok(SandboxOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            resource_limits: ResourceLimitEnforcement::BestEffort,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_available_reflects_path() {
        let backend = BubblewrapBackend::new();
        // Cannot assert true/false absolutely; just ensure no panic.
        let _ = backend.is_available();
    }

    #[tokio::test]
    #[cfg_attr(not(target_os = "linux"), ignore = "bwrap is Linux-only")]
    async fn allow_hosts_unsupported() {
        let backend = BubblewrapBackend::new();
        if !backend.is_available() {
            return;
        }
        let m = SandboxManifest {
            network: NetworkPolicy::AllowHosts(vec!["api.example.com".into()]),
            ..Default::default()
        };
        let res = backend
            .execute(
                &m,
                SandboxCommand {
                    argv: vec!["true".into()],
                    cwd: None,
                },
            )
            .await;
        assert!(matches!(res, Err(SandboxError::PolicyNotSupported(_))));
    }

    #[tokio::test]
    #[cfg_attr(not(target_os = "linux"), ignore = "bwrap is Linux-only")]
    async fn echo_runs_under_bwrap() {
        let backend = BubblewrapBackend::new();
        if !backend.is_available() {
            eprintln!("bwrap not available; skipping");
            return;
        }
        let m = SandboxManifest::default();
        let out = backend
            .execute(
                &m,
                SandboxCommand {
                    argv: vec!["/bin/echo".into(), "hi".into()],
                    cwd: None,
                },
            )
            .await;
        // Could fail if /bin not bound; this is informational.
        let _ = out;
    }

    /// wayland#552 regression: a manifest-declared mount whose SOURCE does
    /// not exist on this host must be SKIPPED, not fatal. Pre-fix (`--bind`)
    /// bwrap aborted the spawn with "Can't find source path", turning every
    /// bash command into an empty-output error on a fresh HOME (no
    /// `~/.cache`/`.cargo`/`.npm`/`.rustup`). With `--bind-try` the command
    /// runs and the absent mount is quietly dropped.
    #[tokio::test]
    #[cfg_attr(not(target_os = "linux"), ignore = "bwrap is Linux-only")]
    async fn missing_bind_source_is_skipped_not_fatal() {
        let backend = BubblewrapBackend::new();
        if !backend.is_available() {
            eprintln!("bwrap not available; skipping");
            return;
        }
        // A path guaranteed absent — the exact failure shape from #552.
        let ghost = std::path::PathBuf::from("/tmp/wl552-does-not-exist-ghost-mount");
        assert!(
            !ghost.exists(),
            "test precondition: ghost path must be absent"
        );
        let m = SandboxManifest {
            fs_write_allow: vec![ghost.clone()],
            fs_read_allow: vec![ghost],
            ..Default::default()
        };
        let out = backend
            .execute(
                &m,
                SandboxCommand {
                    argv: vec!["/bin/echo".into(), "hello-552".into()],
                    cwd: None,
                },
            )
            .await
            .expect("spawn must not fail on a missing bind source");
        assert_eq!(
            out.exit_code,
            0,
            "command must run despite the absent mount; got exit={} stderr={}",
            out.exit_code,
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("hello-552"),
            "stdout must carry the command output, not a sandbox spawn error; stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[tokio::test]
    #[cfg_attr(not(target_os = "linux"), ignore = "bwrap is Linux-only")]
    async fn bwrap_denies_read_of_secret_under_allowed_root() {
        let backend = BubblewrapBackend::new();
        if !backend.is_available() {
            eprintln!("bwrap not available; skipping");
            return;
        }
        // Create a temp dir with a secret file inside it.
        let root = tempfile::tempdir().expect("tempdir");
        let secret_path = root.path().join(".env");
        std::fs::write(&secret_path, "SECRET=supersecret").expect("write secret");

        let manifest = SandboxManifest {
            fs_read_allow: vec![root.path().to_path_buf()],
            fs_read_deny: vec![secret_path.clone()],
            ..Default::default()
        };
        // cat of a /dev/null-overlaid file exits 0 with empty output.
        // Assert secret bytes are absent — NOT non-zero exit.
        let denied = backend
            .execute(
                &manifest,
                SandboxCommand {
                    argv: vec!["cat".into(), secret_path.to_string_lossy().into()],
                    cwd: None,
                },
            )
            .await
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&denied.stdout).contains("secret"),
            "secret bytes must not be readable; got: {:?}",
            String::from_utf8_lossy(&denied.stdout)
        );
    }

    #[tokio::test]
    #[cfg_attr(not(target_os = "linux"), ignore = "bwrap is Linux-only")]
    async fn bwrap_enforces_read_deny_returns_true() {
        let backend = BubblewrapBackend::new();
        assert!(backend.enforces_read_deny());
    }

    #[tokio::test]
    #[cfg_attr(not(target_os = "linux"), ignore = "bwrap is Linux-only")]
    async fn relative_path_rejected() {
        let backend = BubblewrapBackend::new();
        if !backend.is_available() {
            return;
        }
        let m = SandboxManifest {
            fs_read_allow: vec!["relative/path".into()],
            ..Default::default()
        };
        let res = backend
            .execute(
                &m,
                SandboxCommand {
                    argv: vec!["true".into()],
                    cwd: None,
                },
            )
            .await;
        assert!(matches!(res, Err(SandboxError::PathDenied(_))));
    }
}
