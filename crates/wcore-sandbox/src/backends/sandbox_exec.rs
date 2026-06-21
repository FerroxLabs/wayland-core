//! sandbox-exec backend (macOS) — uses macOS's built-in SBPL (Sandbox
//! Profile Language) via the `sandbox-exec(1)` tool.
//!
//! Tier 0 default on macOS per cross-platform strategy.
//!
//! IMPORTANT — Tahoe (macOS 26.x / darwin25) regression:
//! sandbox-exec's engine works fine on Tahoe. The "Sandbox failed to
//! initialize" error reported in Claude Code (anthropics/claude-code#55849)
//! was a PROFILE-CONTENT bug — zsh 5.9 reads new `hw.*` sysctls
//! (hw.targettype, hw.osenvironment) at shell init that Claude Code's
//! profile didn't whitelist. The deny-default profile killed zsh startup.
//!
//! Wayland's bash usage (BashTool calls sh/bash) doesn't hit the zsh
//! issue, but we still bake the fix into every profile for safety AND
//! because future tools may invoke zsh.
//!
//! Fix details:
//!   (allow sysctl-read (sysctl-name-prefix "hw."))
//! This is the SINGLE LINE that fixes the regression. Apple has not
//! deprecated sandbox-exec in Tahoe (the warning is documentation-only).
//!
//! Resource limits: SBPL has NO rlimit primitive — we return
//! `ResourceLimitEnforcement::None`. Callers (BashTool) can warn the
//! user if max_memory_bytes is set on macOS but they wanted hard caps.

use super::SandboxBackend;
use crate::error::{Result, SandboxError};
use crate::manifest::{NetworkPolicy, SandboxManifest};
use crate::{ResourceLimitEnforcement, SandboxCommand, SandboxOutput};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

pub struct SandboxExecBackend {
    /// Cached result of the startup probe. Set on first `is_available()` call.
    probed_available: std::sync::OnceLock<bool>,
}

/// Escape a filesystem path for safe interpolation into an SBPL string
/// literal (`"..."`).
///
/// D.1 Round 1 (MEDIUM — SBPL profile injection): manifest paths were
/// previously interpolated raw via `format!("... \"{}\"", path.display())`.
/// A path containing a `"` (or a backslash) could close the SBPL string
/// literal early and inject arbitrary profile directives — e.g.
/// `(allow default)` — defeating the deny-default sandbox. SBPL string
/// literals follow C-style escaping, so a backslash and a double-quote
/// are escaped with a leading backslash. A newline is rejected upstream
/// (see [`reject_unsafe_path`]); here it is also escaped defensively.
fn escape_sbpl_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// Reject a manifest path that cannot be safely represented in an SBPL
/// profile. A NUL or a newline cannot appear in a profile string at all;
/// rather than silently mangling such a path we fail the whole execution
/// so the caller learns the manifest is malformed.
fn reject_unsafe_path(path: &std::path::Path) -> Result<()> {
    let s = path.to_string_lossy();
    if s.contains('\0') || s.contains('\n') || s.contains('\r') {
        return Err(SandboxError::PolicyNotSupported(format!(
            "sandbox-exec: manifest path {s:?} contains a NUL or newline; \
             refusing to build an SBPL profile from it"
        )));
    }
    Ok(())
}

impl SandboxExecBackend {
    pub fn new() -> Self {
        Self {
            probed_available: std::sync::OnceLock::new(),
        }
    }

    /// Build the SBPL profile from a manifest.
    ///
    /// Returns an error if a manifest path cannot be safely represented
    /// in an SBPL profile (NUL / newline). All interpolated paths are
    /// escaped for the SBPL string-literal context — see
    /// [`escape_sbpl_string`] — so a path containing a `"` cannot break
    /// out of the profile string and inject directives (D.1 Round 1).
    ///
    /// Public for testing.
    pub fn build_profile(manifest: &SandboxManifest) -> Result<String> {
        let mut p = String::new();
        p.push_str("(version 1)\n");
        p.push_str("(deny default)\n");
        // ALWAYS allowed (POSIX-minimum + Tahoe regression fix):
        p.push_str("(allow process-fork)\n");
        p.push_str("(allow process-exec)\n");
        p.push_str("(allow signal (target self))\n");
        // Root directory probe (`(literal "/")`) is required by macOS's
        // dyld bootstrap — without it, even `/bin/echo` aborts with
        // SIGABRT before main(). The deny-default profile MUST whitelist
        // the root inode lookup explicitly; allowlisting `/usr` etc. is
        // not enough.
        p.push_str("(allow file-read* (literal \"/\"))\n");
        p.push_str("(allow file-read* (subpath \"/usr\") (subpath \"/System\") (subpath \"/Library\") (subpath \"/bin\") (subpath \"/sbin\") (subpath \"/private/var/db/dyld\"))\n");
        p.push_str("(allow file-read* (literal \"/dev/null\") (literal \"/dev/urandom\") (literal \"/dev/random\") (literal \"/dev/dtracehelper\"))\n");
        p.push_str("(allow file-write* (literal \"/dev/null\"))\n");
        // TAHOE FIX: bake hw.* sysctl-read for zsh + future tools.
        p.push_str("(allow sysctl-read (sysctl-name-prefix \"hw.\"))\n");
        p.push_str("(allow sysctl-read (sysctl-name-prefix \"kern.\"))\n");
        // sandbox-5: `(allow mach-lookup)` is INTENTIONALLY unfiltered.
        //
        // Rationale / residual exposure (documented, not a silent gap):
        //   * macOS process bootstrap (dyld, libsystem, libxpc) performs
        //     mach-lookup against core system services (e.g.
        //     com.apple.system.opendirectoryd.libinfo,
        //     com.apple.system.notification_center) before `main()` runs.
        //     A `(global-name ...)` allowlist that misses one of these
        //     aborts even `/bin/echo` with SIGABRT — exactly the class of
        //     deny-default-too-tight failure that caused the Tahoe zsh
        //     regression handled above.
        //   * DNS resolution (mDNSResponder), locale, and TZ lookups also
        //     route through mach services; an incomplete allowlist breaks
        //     ordinary shell commands non-deterministically per macOS rev.
        //
        // The mach bootstrap namespace is per-user, so this does NOT grant
        // cross-user reach; the practical residual is that a sandboxed
        // command can talk to user-scoped system daemons. Filesystem and
        // (when `NetworkPolicy::Deny`) network egress remain confined, which
        // are the primary exfil channels. Tightening to a curated
        // `(global-name ...)` allowlist is deferred to a future macOS-rev
        // matrix pass — it MUST be validated against each supported macOS
        // version before it can replace the broad rule without breaking the
        // sandbox open (a too-tight profile that fails to launch would push
        // callers toward NoSandbox, a worse outcome).
        p.push_str("(allow mach-lookup)\n");
        // FS allowlist from manifest. Each path is rejected if it cannot
        // be represented in an SBPL profile, then escaped for the
        // string-literal context before interpolation (D.1 Round 1 —
        // profile-injection fix).
        for path in &manifest.fs_read_allow {
            reject_unsafe_path(path)?;
            p.push_str(&format!(
                "(allow file-read* (subpath \"{}\"))\n",
                escape_sbpl_string(&path.to_string_lossy())
            ));
        }
        for path in &manifest.fs_write_allow {
            reject_unsafe_path(path)?;
            let escaped = escape_sbpl_string(&path.to_string_lossy());
            p.push_str(&format!("(allow file-read* (subpath \"{escaped}\"))\n"));
            p.push_str(&format!("(allow file-write* (subpath \"{escaped}\"))\n"));
        }
        // Secret-read-deny: emitted AFTER all allows so SBPL last-match-wins
        // semantics make the deny authoritative even under an allowed subtree.
        // Paths must be canonicalized by the caller (WorkspacePolicy) before
        // they reach the manifest. The reject+escape pipeline matches the
        // allow-list paths above.
        for path in &manifest.fs_read_deny {
            reject_unsafe_path(path)?;
            p.push_str(&format!(
                "(deny file-read* (subpath \"{}\"))\n",
                escape_sbpl_string(&path.to_string_lossy())
            ));
        }
        // Network policy.
        match &manifest.network {
            NetworkPolicy::Inherit => {
                p.push_str("(allow network*)\n");
            }
            NetworkPolicy::Deny => {
                // No network rule = denied by deny-default.
            }
            NetworkPolicy::AllowHosts(_) => {
                // SBPL has no DNS-name allowlist; only port + protocol filters,
                // so AllowHosts returns PolicyNotSupported (see execute()). A
                // per-IP filter would need a DNS-resolution shim first.
            }
        }
        Ok(p)
    }
}

impl Default for SandboxExecBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxBackend for SandboxExecBackend {
    fn name(&self) -> &'static str {
        "sandbox_exec"
    }

    fn enforces_read_deny(&self) -> bool {
        true
    }

    fn is_available(&self) -> bool {
        *self.probed_available.get_or_init(|| {
            // Probe: invoke sandbox-exec with the minimum known-good
            // profile against /usr/bin/true. If it exits 0, the engine
            // works. If it errors out, the engine is broken (very rare —
            // not even Tahoe broke this; only profile content broke).
            let probe_profile = "(version 1)(allow default)";
            std::process::Command::new("sandbox-exec")
                .arg("-p")
                .arg(probe_profile)
                .arg("/usr/bin/true")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
    }

    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        if matches!(manifest.network, NetworkPolicy::AllowHosts(_)) {
            return Err(SandboxError::PolicyNotSupported(
                "sandbox-exec has no DNS-name allowlist; use NetworkPolicy::Deny + future v0.6.4 per-IP filter".into(),
            ));
        }
        if !self.is_available() {
            return Err(SandboxError::ExecFailed(
                "sandbox-exec probe failed; sandboxing unavailable on this macOS host".into(),
            ));
        }

        // Write profile to a temp file (audit-corrected: -f file is more
        // robust than -p inline; avoids shell escaping cliff). build_profile
        // rejects manifest paths that cannot be safely represented.
        let profile = Self::build_profile(manifest)?;
        let mut profile_file = tempfile::Builder::new()
            .prefix("wcore-sbx-")
            .suffix(".sb")
            .tempfile()
            .map_err(|e| SandboxError::ExecFailed(format!("tempfile: {e}")))?;
        std::io::Write::write_all(&mut profile_file, profile.as_bytes())
            .map_err(|e| SandboxError::ExecFailed(format!("write profile: {e}")))?;
        let profile_path = profile_file.path().to_string_lossy().into_owned();

        // env -i isolation: scrub host env then inject only the manifest
        // env explicitly. Mirrors the no_sandbox backend's contract so
        // flipping backends does not silently widen env exposure.
        let mut child_cmd = Command::new("/usr/bin/sandbox-exec");
        child_cmd.arg("-f").arg(&profile_path);
        for a in &cmd.argv {
            child_cmd.arg(a);
        }
        child_cmd.env_clear();
        for (k, v) in &manifest.env {
            child_cmd.env(k, v);
        }
        if let Some(cwd) = &cmd.cwd {
            child_cmd.current_dir(cwd);
        }
        child_cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Reap the child if this future is dropped — e.g. when the
        // `tokio::time::timeout` below elapses and drops the in-flight
        // `run_fut` (which owns the Child). Without this the sandboxed
        // process tree escapes on timeout. Mirrors no_sandbox.rs.
        child_cmd.kill_on_drop(true);

        let run_fut = async {
            let child = child_cmd
                .spawn()
                .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
            child
                .wait_with_output()
                .await
                .map_err(|e| SandboxError::ExecFailed(e.to_string()))
        };

        let output = if let Some(timeout) = manifest.timeout {
            tokio::time::timeout(timeout, run_fut)
                .await
                .map_err(|_| SandboxError::Timeout)??
        } else {
            run_fut.await?
        };

        // Profile file dropped here, deleted from disk.
        drop(profile_file);

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

    // ── Task 2 tests ──────────────────────────────────────────────────────────

    #[test]
    fn profile_emits_read_deny_after_allows() {
        // Deny rules must appear AFTER the allow rules for the same subtree so
        // SBPL last-match-wins semantics make the deny authoritative.
        let m = SandboxManifest {
            fs_read_allow: vec!["/tmp/workspace".into()],
            fs_write_allow: vec!["/tmp/scratch".into()],
            fs_read_deny: vec!["/tmp/workspace/.env".into()],
            ..Default::default()
        };
        let p = SandboxExecBackend::build_profile(&m).expect("profile builds");

        // All three rules must be present.
        assert!(
            p.contains("(allow file-read* (subpath \"/tmp/workspace\"))"),
            "read-allow must be present"
        );
        assert!(
            p.contains("(allow file-write* (subpath \"/tmp/scratch\"))"),
            "write-allow must be present"
        );
        assert!(
            p.contains("(deny file-read* (subpath \"/tmp/workspace/.env\"))"),
            "read-deny must be present"
        );

        // The deny line must appear AFTER the allow line in the profile string
        // (SBPL is last-match-wins).
        let allow_pos = p
            .find("(allow file-read* (subpath \"/tmp/workspace\"))")
            .expect("allow line must exist");
        let deny_pos = p
            .find("(deny file-read* (subpath \"/tmp/workspace/.env\"))")
            .expect("deny line must exist");
        assert!(
            deny_pos > allow_pos,
            "deny rule must appear AFTER the allow rule (last-match-wins); \
             allow_pos={allow_pos} deny_pos={deny_pos}"
        );
    }

    #[test]
    fn profile_read_deny_escapes_paths() {
        // A path containing a double-quote in the deny list must be escaped
        // in the same way as an allow-list path — no SBPL injection possible.
        let m = SandboxManifest {
            fs_read_deny: vec![
                "/tmp/secret\") (allow default) (allow file-read* (subpath \"/x".into(),
            ],
            ..Default::default()
        };
        let p = SandboxExecBackend::build_profile(&m).expect("profile builds");

        // The injected `(allow default)` substring must NOT appear as an
        // unescaped directive in the profile.
        let deny_line = p
            .lines()
            .find(|l| l.contains("deny file-read*") && l.contains("allow default"))
            .expect("the deny line must be present");

        // Verify every `"` in the deny line is either a delimiter or escaped.
        let bytes: Vec<char> = deny_line.chars().collect();
        for (i, &c) in bytes.iter().enumerate() {
            if c == '"' {
                let escaped = i > 0 && bytes[i - 1] == '\\';
                let is_open =
                    deny_line[..deny_line.char_indices().nth(i).unwrap().0].ends_with("(subpath ");
                let is_close =
                    deny_line[deny_line.char_indices().nth(i).unwrap().0..].starts_with("\"))");
                assert!(
                    escaped || is_open || is_close,
                    "unescaped, non-delimiter quote at index {i} — SBPL injection possible: {deny_line}"
                );
            }
        }
        // Sanity: the path's quote really was escaped.
        assert!(
            deny_line.contains("\\\""),
            "expected escaped quote in: {deny_line}"
        );
    }

    #[test]
    fn enforces_read_deny_is_true() {
        // The capability override must be set to true on this backend.
        let backend = SandboxExecBackend::new();
        assert!(
            backend.enforces_read_deny(),
            "SandboxExecBackend must report enforces_read_deny() = true"
        );
    }

    #[tokio::test]
    #[cfg_attr(not(target_os = "macos"), ignore = "macOS only")]
    async fn sandbox_exec_denies_read_of_secret_under_allowed_root() {
        // Live test: a file is read-allowed via fs_read_allow (the parent
        // directory), but its path is also in fs_read_deny. The SBPL
        // last-match-wins deny should prevent the file from being read.
        let backend = SandboxExecBackend::new();
        if !backend.is_available() {
            return;
        }

        // Create a temp dir with a secret file.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let secret = root.join(".env");
        std::fs::write(&secret, b"SECRET=hunter2").expect("write secret");

        // Canonicalize both for the manifest.
        let canon_root = std::fs::canonicalize(root).expect("canonicalize root");
        let canon_secret = std::fs::canonicalize(&secret).expect("canonicalize secret");

        let m = SandboxManifest {
            fs_read_allow: vec![canon_root.clone()],
            fs_read_deny: vec![canon_secret.clone()],
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            ..Default::default()
        };

        // Attempt to cat the secret — should be denied (non-zero exit or empty).
        let out = backend
            .execute(
                &m,
                SandboxCommand {
                    argv: vec![
                        "/bin/cat".into(),
                        canon_secret.to_string_lossy().into_owned(),
                    ],
                    cwd: None,
                },
            )
            .await
            .expect("execute returns Ok");

        // The sandbox should deny the read: either non-zero exit code or
        // empty stdout (no secret bytes readable).
        let stdout_str = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.exit_code != 0 || !stdout_str.contains("SECRET"),
            "secret bytes must not be readable; exit={} stdout={:?}",
            out.exit_code,
            stdout_str
        );
    }

    // ── End Task 2 tests ──────────────────────────────────────────────────────

    #[test]
    fn profile_includes_tahoe_fix() {
        let m = SandboxManifest::default();
        let p = SandboxExecBackend::build_profile(&m).expect("default profile builds");
        assert!(
            p.contains("(allow sysctl-read (sysctl-name-prefix \"hw.\"))"),
            "Tahoe fix MUST be in profile"
        );
        assert!(p.contains("(allow sysctl-read (sysctl-name-prefix \"kern.\"))"));
        assert!(p.contains("(deny default)"));
    }

    #[test]
    fn profile_mach_lookup_is_documented_broad_not_allow_default() {
        // sandbox-5: mach-lookup is intentionally broad, but the profile
        // must remain deny-default and must NOT have been widened to
        // `(allow default)` (which would defeat FS confinement). This
        // pins the documented residual exposure so an accidental broadening
        // is caught.
        let m = SandboxManifest::default();
        let p = SandboxExecBackend::build_profile(&m).expect("default profile builds");
        assert!(p.contains("(deny default)"), "must stay deny-default");
        assert!(
            p.contains("(allow mach-lookup)"),
            "mach-lookup intentionally broad for macOS bootstrap"
        );
        assert!(
            !p.contains("(allow default)"),
            "profile must never grant (allow default) — that defeats the sandbox"
        );
    }

    #[test]
    fn profile_emits_fs_allowlist() {
        let m = SandboxManifest {
            fs_read_allow: vec!["/tmp/work".into()],
            fs_write_allow: vec!["/var/tmp/scratch".into()],
            ..Default::default()
        };
        let p = SandboxExecBackend::build_profile(&m).expect("profile builds");
        assert!(p.contains("(allow file-read* (subpath \"/tmp/work\"))"));
        assert!(p.contains("(allow file-read* (subpath \"/var/tmp/scratch\"))"));
        assert!(p.contains("(allow file-write* (subpath \"/var/tmp/scratch\"))"));
    }

    #[test]
    fn profile_escapes_quote_in_path_no_injection() {
        // D.1 Round 1 (MEDIUM): a path containing a double-quote must NOT
        // be able to close the SBPL string literal and inject directives.
        let m = SandboxManifest {
            fs_read_allow: vec![
                "/tmp/evil\") (allow default) (allow file-read* (subpath \"/x".into(),
            ],
            ..Default::default()
        };
        let p = SandboxExecBackend::build_profile(&m).expect("profile builds");
        // Security property: every `"` in the profile is either the
        // intentional delimiter (preceded by `(subpath ` or by `"))`) or
        // an escaped quote from the path (preceded by `\`). A path quote
        // that is NOT preceded by a backslash would break out of the
        // literal — assert no such bare quote exists in the manifest line.
        let line = p
            .lines()
            .find(|l| l.contains("allow default"))
            .expect("the manifest path line must be present");
        let bytes: Vec<char> = line.chars().collect();
        for (i, &c) in bytes.iter().enumerate() {
            if c == '"' {
                let escaped = i > 0 && bytes[i - 1] == '\\';
                // The two legitimate delimiter quotes: the opening one
                // after `(subpath ` and the closing one before `))`.
                let is_open = line[..line.char_indices().nth(i).unwrap().0].ends_with("(subpath ");
                let is_close = line[line.char_indices().nth(i).unwrap().0..].starts_with("\"))");
                assert!(
                    escaped || is_open || is_close,
                    "unescaped, non-delimiter quote at index {i} — SBPL injection possible: {line}"
                );
            }
        }
        // Sanity: the path's quote really was escaped (`\"` present).
        assert!(
            line.contains("\\\""),
            "expected an escaped quote in: {line}"
        );
    }

    #[test]
    fn profile_rejects_path_with_newline() {
        let m = SandboxManifest {
            fs_read_allow: vec!["/tmp/bad\n(allow default)".into()],
            ..Default::default()
        };
        let res = SandboxExecBackend::build_profile(&m);
        assert!(
            matches!(res, Err(SandboxError::PolicyNotSupported(_))),
            "a newline-bearing path must be rejected, got: {res:?}"
        );
    }

    #[tokio::test]
    #[cfg_attr(not(target_os = "macos"), ignore = "macOS only")]
    async fn allow_hosts_unsupported() {
        let backend = SandboxExecBackend::new();
        let m = SandboxManifest {
            network: NetworkPolicy::AllowHosts(vec!["example.com".into()]),
            ..Default::default()
        };
        let res = backend
            .execute(
                &m,
                SandboxCommand {
                    argv: vec!["/usr/bin/true".into()],
                    cwd: None,
                },
            )
            .await;
        assert!(matches!(res, Err(SandboxError::PolicyNotSupported(_))));
    }

    #[tokio::test]
    #[cfg_attr(not(target_os = "macos"), ignore = "macOS only")]
    async fn probe_runs() {
        let backend = SandboxExecBackend::new();
        // On macOS the probe should succeed.
        assert!(
            backend.is_available(),
            "sandbox-exec probe failed on macOS host"
        );
    }

    #[tokio::test]
    #[cfg_attr(not(target_os = "macos"), ignore = "macOS only")]
    async fn echo_runs_under_sandbox() {
        let backend = SandboxExecBackend::new();
        if !backend.is_available() {
            return;
        }
        let m = SandboxManifest {
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            ..Default::default()
        };
        let out = backend
            .execute(
                &m,
                SandboxCommand {
                    argv: vec!["/bin/echo".into(), "hi".into()],
                    cwd: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
        assert_eq!(out.resource_limits, ResourceLimitEnforcement::None);
    }
}
