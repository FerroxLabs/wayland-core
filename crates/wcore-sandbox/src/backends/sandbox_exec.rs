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

/// Collect every proper ancestor DIRECTORY of the granted manifest paths.
///
/// Seatbelt grants are per-node: `(subpath "/Users/me/.cargo/bin")` makes that
/// directory and its contents readable but leaves `/Users`, `/Users/me` and
/// `/Users/me/.cargo` denied. Opening a file deep inside a granted subpath
/// still works, but `realpath(3)` / `lstat(2)` resolve a path COMPONENT AT A
/// TIME and fail on the first ungranted ancestor — which is why node aborts
/// with `EPERM: operation not permitted, lstat '/Users'` and Homebrew's
/// python3 with `realpath: /opt/homebrew/bin/: Operation not permitted` even
/// though both interpreters live inside a granted subpath.
///
/// The root `/` is skipped: `build_profile` already grants it a `literal`
/// read for the dyld bootstrap.
fn ancestor_directories(
    manifest: &SandboxManifest,
    darwin_temp: Option<&std::path::Path>,
) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let granted = manifest
        .fs_read_allow
        .iter()
        .chain(manifest.fs_write_allow.iter())
        .map(|p| p.as_path())
        .chain(darwin_temp);
    for path in granted {
        for ancestor in path.ancestors().skip(1) {
            let text = ancestor.to_string_lossy();
            if text.is_empty() || text == "/" {
                continue;
            }
            out.insert(text.into_owned());
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

/// The per-user temporary directory Darwin's own tools use, read from
/// `confstr(_CS_DARWIN_USER_TEMP_DIR)` — the value `/var/folders/<a>/<b>/T`.
///
/// This is NOT `$TMPDIR`, and that distinction is the whole reason this
/// function exists. `WorkspacePolicy` points `TMPDIR`/`TMP`/`TEMP` at the
/// session's own granted scratch, which rescues every tool that reads the
/// environment (clang, python3's `tempfile`, node's `os.tmpdir()`). Several
/// Apple tools do not: `mktemp(1)` and the `xcrun` cache shim call `confstr`
/// FIRST and only fall back to `$TMPDIR`, so under a deny-default profile they
/// fail against a directory no environment variable can move. Measured on
/// Darwin 25.3.0 with `TMPDIR` redirected: `mktemp` still returned
/// `/var/folders/…/T/tmp.XXXXXXXXXX`, and inside the sandbox that became
/// `mktemp: mkstemp failed on …: Operation not permitted`, while `git`,
/// `python3` and `clang` each printed
/// `couldn't create cache file '…/T/xcrun_db-XXXXXXXX'` on every invocation.
///
/// Canonicalized, because seatbelt matches the `/private`-rooted spelling: the
/// `/var` the caller sees is a symlink and a `subpath` grant written through it
/// does not match.
///
/// Returns `None` when `confstr` reports nothing or the directory does not
/// resolve — the profile then simply omits the grant, which is the status quo
/// ante rather than a widened one.
fn darwin_user_temp_dir() -> Option<std::path::PathBuf> {
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: `confstr` writes at most `buf.len()` bytes (including the NUL)
    // into the buffer we own and keep alive for the call, and returns the
    // length it needed. It touches no other caller memory.
    let len = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
        )
    };
    if len == 0 || len > buf.len() {
        return None;
    }
    buf.truncate(len - 1); // drop the trailing NUL confstr counts
    let raw = String::from_utf8(buf).ok()?;
    std::fs::canonicalize(raw).ok()
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
        Self::build_profile_with_darwin_temp(manifest, darwin_user_temp_dir().as_deref())
    }

    /// [`build_profile`](Self::build_profile) with the Darwin per-user temp
    /// directory supplied instead of read from `confstr`.
    ///
    /// The seam exists so the grant can be asserted against a fixed, known path
    /// instead of whatever `/var/folders/<random>/<random>/T` the running host
    /// happens to own — a test that reads the same `confstr` the code under test
    /// reads would pass no matter what the code emitted.
    ///
    /// Public for testing.
    pub fn build_profile_with_darwin_temp(
        manifest: &SandboxManifest,
        darwin_temp: Option<&std::path::Path>,
    ) -> Result<String> {
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
        // macOS spells `/var`, `/tmp` and `/etc` as SYMLINKS into `/private`.
        // Seatbelt evaluates the symlink node itself before it follows the
        // link, so a path spelled through one of them (`/tmp/x`,
        // `$TMPDIR/x` — TMPDIR is `/var/folders/…` on every macOS host) is
        // denied at the link lookup even when the canonical target
        // (`/private/var/folders/…`) is granted by an explicit `(subpath …)`
        // allow. Granting the same `/var` spelling as another `(subpath …)`
        // does NOT help — the denial is on the link node, so it needs a
        // `literal` READ grant on the three link nodes themselves. This grants
        // read of three symlink inodes, not of their targets: everything under
        // `/private` stays governed by the manifest allow/deny rules below.
        // Manifest paths are canonicalized upstream, but the shell command is
        // LLM-authored text that we neither can nor should rewrite.
        p.push_str("(allow file-read* (literal \"/var\") (literal \"/tmp\") (literal \"/etc\"))\n");
        // `/private/var/select/sh` is the selector symlink macOS's `sh` reads
        // at startup; without it every sandboxed shell prints
        // `Error opening /private/var/select/sh: Operation not permitted`.
        p.push_str("(allow file-read* (subpath \"/usr\") (subpath \"/System\") (subpath \"/Library\") (subpath \"/bin\") (subpath \"/sbin\") (subpath \"/private/var/db/dyld\") (subpath \"/private/var/select\"))\n");
        p.push_str("(allow file-read* (literal \"/dev/null\") (literal \"/dev/urandom\") (literal \"/dev/random\") (literal \"/dev/dtracehelper\"))\n");
        p.push_str("(allow file-write* (literal \"/dev/null\"))\n");
        // LibreSSL — the TLS stack Apple links into `cargo`, `curl` and every
        // other system-linked client — opens `/private/etc/ssl/openssl.cnf`
        // unconditionally at init. Without this grant `cargo --version` prints
        // `Auto configuration failed … fopen('/private/etc/ssl/openssl.cnf',
        // 'rb')` and exits before doing any work. A single `literal`, not a
        // `subpath`: the sibling `x509v3.cnf` and the rest of `/private/etc`
        // stay denied. The file is the world-readable system OpenSSL
        // configuration and carries no key material, so the read grants an
        // attacker nothing beyond the host's default cipher/CA settings.
        p.push_str("(allow file-read* (literal \"/private/etc/ssl/openssl.cnf\"))\n");
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
        // The Darwin per-user temp directory — see [`darwin_user_temp_dir`] for
        // why `$TMPDIR` cannot stand in for it. Without this grant `mktemp(1)`
        // is simply broken on macOS under BOTH the contained and the
        // trusted_local profile, and `git`, `python3` and `clang` each emit two
        // `couldn't create cache file` lines per invocation.
        //
        // EXPOSURE, stated plainly: this is read+write over the directory the
        // user's other applications also use for transient files, so a
        // sandboxed command can read and overwrite their temp state. It is
        // per-uid — `confstr` returns a directory owned by, and reachable only
        // by, the invoking user — so it grants no cross-user reach, and the
        // session's own scratch grant (`$TMPDIR`, from `scratch_dirs`) already
        // sits INSIDE this same tree, so the change widens an existing foothold
        // from one subdirectory to its parent rather than opening a new region
        // of the filesystem. `fs_read_deny` still wins over it: the deny block
        // is emitted after this, and SBPL is last-match-wins.
        if let Some(dir) = darwin_temp {
            reject_unsafe_path(dir)?;
            let escaped = escape_sbpl_string(&dir.to_string_lossy());
            p.push_str(&format!("(allow file-read* (subpath \"{escaped}\"))\n"));
            p.push_str(&format!("(allow file-write* (subpath \"{escaped}\"))\n"));
        }
        // Path-resolution ancestors. `file-read-metadata` is the narrowest
        // seatbelt operation that satisfies `stat`/`lstat`/`realpath`: it does
        // NOT permit reading file contents and does NOT permit listing a
        // directory (`readdir` needs `file-read-data` on the directory node),
        // both externally re-confirmed under a live `sandbox-exec` — `ls
        // /Users`, `cat ~/.ssh/id_ed25519` and `cat /etc/passwd` all stay
        // "Operation not permitted" while `stat /Users` succeeds. Every path
        // emitted here is a prefix of a path the manifest ALREADY grants in
        // full, so an attacker learns nothing it could not derive from its own
        // grant list. Emitted before the deny block so a `fs_read_deny` entry
        // still wins under SBPL last-match-wins.
        for ancestor in ancestor_directories(manifest, darwin_temp) {
            p.push_str(&format!(
                "(allow file-read-metadata (literal \"{}\"))\n",
                escape_sbpl_string(&ancestor)
            ));
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
        // Keep the direct-child kill as defense in depth, and isolate the
        // wrapper into its own process group so the guard below also owns
        // every sandboxed descendant.
        child_cmd.kill_on_drop(true);
        super::process_tree::isolate(&mut child_cmd);

        let run_fut = async {
            let mut child = child_cmd
                .spawn()
                .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
            let mut process_tree = super::process_tree::ProcessTreeGuard::new(child.id())
                .map_err(|e| SandboxError::ExecFailed(format!("process-tree ownership: {e}")))?;
            let output =
                super::wait_with_bounded_output_on_exit(&mut child, || process_tree.disarm())
                    .await?;
            Ok::<_, SandboxError>(output)
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

    // ── macOS toolchain grant-set tests ───────────────────────────────────────
    //
    // Measured RED these encode (live `sandbox-exec` on darwin 25.3.0, profile
    // reproduced from this generator, evidence in
    // `RED-baseline.txt` / `negative-controls.txt`):
    //   node    -> `Error: EPERM: operation not permitted, lstat '/Users'`
    //   python3 -> `realpath: /opt/homebrew/bin/: Operation not permitted`
    //   cargo   -> `fopen('/private/etc/ssl/openssl.cnf', 'rb')` Operation not permitted
    // Both are grant-set defects in the generated profile, so both are
    // falsifiable at generator level.

    /// The manifest a genuinely-local macOS session produces: a workspace
    /// under $HOME plus toolchain roots several levels below $HOME and /opt.
    fn toolchain_manifest() -> SandboxManifest {
        SandboxManifest {
            fs_read_allow: vec!["/Users/alice/.cargo/bin".into(), "/opt/homebrew/bin".into()],
            fs_write_allow: vec!["/Users/alice/proj".into()],
            ..Default::default()
        }
    }

    #[test]
    fn profile_grants_metadata_read_on_every_granted_path_ancestor() {
        let p = SandboxExecBackend::build_profile(&toolchain_manifest()).expect("profile builds");
        for ancestor in [
            "/Users",
            "/Users/alice",
            "/Users/alice/.cargo",
            "/opt",
            "/opt/homebrew",
        ] {
            assert!(
                p.contains(&format!(
                    "(allow file-read-metadata (literal \"{ancestor}\"))"
                )),
                "missing ancestor metadata grant for {ancestor}; realpath()/lstat() \
                 walks a path component at a time and EPERMs on the first \
                 ungranted ancestor. Profile:\n{p}"
            );
        }
    }

    #[test]
    fn profile_ancestor_grants_are_metadata_only_and_never_widen_to_home() {
        let p = SandboxExecBackend::build_profile(&toolchain_manifest()).expect("profile builds");
        // The ancestors must NOT become readable subtrees — that would hand a
        // sandboxed command every file under $HOME.
        for widened in [
            "(allow file-read* (subpath \"/Users\"))",
            "(allow file-read* (literal \"/Users\"))",
            "(allow file-read* (subpath \"/Users/alice\"))",
            "(allow file-read* (literal \"/Users/alice\"))",
            "(allow file-write* (subpath \"/Users\"))",
            "(allow file-write* (subpath \"/Users/alice\"))",
            "(allow file-read* (subpath \"/opt\"))",
        ] {
            assert!(
                !p.contains(widened),
                "profile widened an ancestor into a full grant: {widened}\n{p}"
            );
        }
    }

    #[test]
    fn profile_grants_system_openssl_config_as_a_literal() {
        // LibreSSL (cargo/curl on macOS) opens this file at init.
        let p =
            SandboxExecBackend::build_profile(&SandboxManifest::default()).expect("profile builds");
        assert!(
            p.contains("(allow file-read* (literal \"/private/etc/ssl/openssl.cnf\"))"),
            "missing the LibreSSL config grant; `cargo --version` fails with \
             `Auto configuration failed … fopen('/private/etc/ssl/openssl.cnf')`.\n{p}"
        );
        // Narrow: the surrounding directory and its siblings stay denied.
        for widened in [
            "(allow file-read* (subpath \"/private/etc\"))",
            "(allow file-read* (subpath \"/private/etc/ssl\"))",
            "(allow file-read* (literal \"/private/etc/ssl/x509v3.cnf\"))",
        ] {
            assert!(
                !p.contains(widened),
                "openssl.cnf grant widened beyond the single file: {widened}\n{p}"
            );
        }
    }

    #[test]
    fn profile_emits_ancestor_metadata_before_read_deny() {
        // SBPL is last-match-wins: a secret-read deny must still override any
        // grant, including the ancestor metadata block.
        let m = SandboxManifest {
            fs_read_allow: vec!["/Users/alice/proj".into()],
            fs_read_deny: vec!["/Users/alice/proj/.env".into()],
            ..Default::default()
        };
        let p = SandboxExecBackend::build_profile(&m).expect("profile builds");
        let metadata_pos = p
            .find("(allow file-read-metadata (literal \"/Users/alice\"))")
            .expect("ancestor metadata grant must exist");
        let deny_pos = p
            .find("(deny file-read* (subpath \"/Users/alice/proj/.env\"))")
            .expect("read-deny must exist");
        assert!(
            deny_pos > metadata_pos,
            "read-deny must be emitted after the ancestor metadata grants \
             (last-match-wins); metadata_pos={metadata_pos} deny_pos={deny_pos}"
        );
    }

    #[test]
    fn profile_ancestor_grants_are_escaped_and_deduplicated() {
        // Two grants under one parent must not emit the parent twice, and a
        // quote in a path must not break out of the SBPL string literal.
        let m = SandboxManifest {
            fs_read_allow: vec!["/Users/alice/a".into(), "/Users/alice/b".into()],
            fs_write_allow: vec!["/Users/al\"ice/c".into()],
            ..Default::default()
        };
        let p = SandboxExecBackend::build_profile(&m).expect("profile builds");
        assert_eq!(
            p.matches("(allow file-read-metadata (literal \"/Users/alice\"))")
                .count(),
            1,
            "shared ancestor emitted more than once:\n{p}"
        );
        assert!(
            p.contains("(allow file-read-metadata (literal \"/Users/al\\\"ice\"))"),
            "quote in an ancestor path must be escaped:\n{p}"
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

    /// 21-C3-01 cross-backend check: macOS must NOT have bubblewrap's
    /// overlapping-deny defect.
    ///
    /// bubblewrap enforces a deny by MOUNTING over the denied path, so a deny
    /// nested inside a directory deny needed a mount point in a read-only mask
    /// and aborted the spawn (`bwrap: Can't mkdir …/.git: Read-only file
    /// system`). SBPL has no mounts: `(deny file-read* (subpath …))` rules are
    /// independent predicates under last-match-wins, so a nested pair should
    /// simply be enforced twice.
    ///
    /// That is a claim about the OS, so it is MEASURED here rather than argued:
    /// the profile is built by the production `build_profile` from the exact
    /// pair `spawner.rs` hands the sandbox for an isolated-mutation child, and
    /// run through the production `execute`. Arm 1 is the instrument control —
    /// with no deny the probe must read BOTH secrets, otherwise a "refused"
    /// reading in arm 2 would be free.
    #[tokio::test]
    #[cfg_attr(not(target_os = "macos"), ignore = "sandbox-exec is macOS-only")]
    async fn overlapping_read_deny_runs_shell_and_still_contains() {
        let backend = SandboxExecBackend::new();
        assert!(
            backend.is_available(),
            "sandbox-exec must be usable on a macOS host"
        );

        let parent = tempfile::tempdir().expect("parent workspace");
        let parent_root = std::fs::canonicalize(parent.path()).expect("canonicalize parent");
        std::fs::create_dir_all(parent_root.join(".git")).expect("parent .git");
        std::fs::write(parent_root.join("secret.txt"), b"PARENTSECRET\n").expect("parent secret");
        std::fs::write(parent_root.join(".git").join("config"), b"GITSECRET\n")
            .expect("git secret");

        // Both denies must reach the profile as independent rules — the SBPL
        // analogue of the two bwrap mounts, minus the mount.
        let overlapping = vec![parent_root.clone(), parent_root.join(".git")];
        let profile = SandboxExecBackend::build_profile(&SandboxManifest {
            fs_read_allow: vec![parent_root.clone()],
            fs_read_deny: overlapping.clone(),
            ..Default::default()
        })
        .expect("profile builds from an overlapping deny pair");
        for path in &overlapping {
            assert!(
                profile.contains(&format!(
                    "(deny file-read* (subpath \"{}\"))",
                    path.to_string_lossy()
                )),
                "both nested denies must be emitted as independent SBPL rules; profile={profile}"
            );
        }

        // The marker is joined by the shell at runtime, so only a shell that
        // actually ran can produce it (21-C3 §6).
        let script = format!(
            "printf %s%s SHELL RAN; echo; \
             cat {parent}/secret.txt 2>/dev/null; \
             cat {parent}/.git/config 2>/dev/null; \
             exit 0",
            parent = parent_root.display()
        );
        let run = |deny: Vec<std::path::PathBuf>| {
            let manifest = SandboxManifest {
                fs_read_allow: vec![parent_root.clone()],
                fs_read_deny: deny,
                env: vec![("PATH".into(), "/usr/bin:/bin".into())],
                ..Default::default()
            };
            let script = script.clone();
            async move {
                SandboxExecBackend::new()
                    .execute(
                        &manifest,
                        SandboxCommand {
                            argv: vec!["/bin/sh".into(), "-c".into(), script],
                            cwd: None,
                        },
                    )
                    .await
            }
        };

        // ── Arm 1: control. No deny — the probe MUST see both secrets. ───────
        let control = run(Vec::new()).await.expect("control execution");
        let control_stdout = String::from_utf8_lossy(&control.stdout).into_owned();
        assert!(
            control_stdout.contains("SHELLRAN")
                && control_stdout.contains("PARENTSECRET")
                && control_stdout.contains("GITSECRET"),
            "instrument is dead: with NO deny the probe must run and read both secrets; \
             stdout={control_stdout:?} stderr={:?}",
            String::from_utf8_lossy(&control.stderr)
        );

        // ── Arm 2: the overlapping pair. ─────────────────────────────────────
        let denied = run(overlapping).await.expect("overlapping-deny execution");
        let denied_stdout = String::from_utf8_lossy(&denied.stdout).into_owned();
        let denied_stderr = String::from_utf8_lossy(&denied.stderr).into_owned();
        assert!(
            denied_stdout.contains("SHELLRAN"),
            "sandbox-exec must run the shell under an overlapping deny pair — the bubblewrap \
             defect must NOT have a macOS analogue; stdout={denied_stdout:?} \
             stderr={denied_stderr:?}"
        );
        assert!(
            !denied_stdout.contains("PARENTSECRET") && !denied_stdout.contains("GITSECRET"),
            "both nested denies must still be enforced; stdout={denied_stdout:?}"
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
    fn profile_grants_read_of_the_private_symlink_nodes() {
        // Without a `literal` read grant on `/var`, `/tmp` and `/etc`, seatbelt
        // denies at the symlink node, so any path spelled through one of them
        // fails even though its canonical target is allowed.
        let m = SandboxManifest::default();
        let p = SandboxExecBackend::build_profile(&m).expect("default profile builds");
        assert!(
            p.contains(
                "(allow file-read* (literal \"/var\") (literal \"/tmp\") (literal \"/etc\"))"
            ),
            "the three /private symlink nodes need literal read grants; profile={p}"
        );
        assert!(
            p.contains("(subpath \"/private/var/select\")"),
            "`sh` reads /private/var/select/sh at startup; profile={p}"
        );
        // The grant must stay a `literal` on the link node — a `subpath "/var"`
        // would re-open the whole of /private/var through the alias.
        assert!(
            !p.contains("(subpath \"/var\")") && !p.contains("(subpath \"/tmp\")"),
            "symlink nodes must be granted as literals, never as subpaths; profile={p}"
        );
    }

    /// The R1 defect, measured rather than argued: with the workspace granted
    /// only under its canonical `/private/var/…` spelling, a write addressed
    /// through the `/var/…` alias must still land, and the shell must not emit
    /// a sandbox denial on stderr.
    ///
    /// Arm 1 is the instrument control — the same write through the canonical
    /// spelling must succeed, otherwise a success in arm 2 would be free.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn symlinked_temp_spelling_reaches_the_same_allowed_root() {
        let backend = SandboxExecBackend::new();
        assert!(backend.is_available(), "sandbox-exec must be usable");

        // `tempfile` builds from `std::env::temp_dir()`, i.e. `$TMPDIR`, which
        // on macOS is always spelled `/var/folders/…`.
        let dir = tempfile::tempdir().expect("tempdir");
        let aliased = dir.path().to_path_buf();
        let canonical = std::fs::canonicalize(&aliased).expect("canonicalize");
        assert_ne!(
            aliased, canonical,
            "fixture is dead: $TMPDIR must be spelled through the /var symlink"
        );

        let manifest = SandboxManifest {
            fs_read_allow: vec![canonical.clone()],
            fs_write_allow: vec![canonical.clone()],
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            ..Default::default()
        };
        let write_to = |target: std::path::PathBuf| {
            let manifest = manifest.clone();
            let cwd = canonical.clone();
            async move {
                SandboxExecBackend::new()
                    .execute(
                        &manifest,
                        SandboxCommand {
                            argv: vec![
                                "/bin/sh".into(),
                                "-c".into(),
                                format!("echo MARKER > {}", target.display()),
                            ],
                            cwd: Some(cwd),
                        },
                    )
                    .await
                    .expect("execution")
            }
        };

        let control = write_to(canonical.join("canon.txt")).await;
        assert_eq!(
            control.exit_code,
            0,
            "instrument is dead: the canonical spelling must be writable; stderr={:?}",
            String::from_utf8_lossy(&control.stderr)
        );

        let aliased_run = write_to(aliased.join("alias.txt")).await;
        let stderr = String::from_utf8_lossy(&aliased_run.stderr).into_owned();
        assert_eq!(
            aliased_run.exit_code, 0,
            "a path spelled through the /var symlink must reach the same allowed root; \
             stderr={stderr:?}"
        );
        assert!(
            canonical.join("alias.txt").is_file(),
            "the aliased write must have landed on the real file"
        );
        // The profile gap also printed a denial on EVERY sandboxed shell; the
        // fix must leave stderr clean so no later filter is tempted to hide it.
        assert!(
            !stderr.contains("Operation not permitted"),
            "no sandbox denial may remain on a fully-allowed command; stderr={stderr:?}"
        );
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

    /// The Darwin per-user temp directory must be granted read AND write, and
    /// its ancestors must get the metadata grants `realpath(3)` needs — a
    /// `subpath` grant on `/private/var/folders/a/b/T` alone leaves
    /// `/private/var/folders` denied and the resolve fails before the grant is
    /// ever consulted.
    ///
    /// A fixed path is injected rather than read from `confstr`, because a test
    /// that called the same `confstr` the code calls would agree with the code
    /// no matter which directory the code emitted.
    #[test]
    fn profile_grants_the_darwin_per_user_temp_dir() {
        let t = std::path::Path::new("/private/var/folders/8h/probe/T");
        let m = SandboxManifest::default();

        let p = SandboxExecBackend::build_profile_with_darwin_temp(&m, Some(t))
            .expect("profile builds");
        assert!(
            p.contains("(allow file-read* (subpath \"/private/var/folders/8h/probe/T\"))"),
            "mkstemp opens O_RDWR, so a write-only grant is not enough: {p}"
        );
        assert!(
            p.contains("(allow file-write* (subpath \"/private/var/folders/8h/probe/T\"))"),
            "profile must let Darwin's confstr temp dir be written: {p}"
        );
        for ancestor in [
            "/private",
            "/private/var",
            "/private/var/folders",
            "/private/var/folders/8h",
            "/private/var/folders/8h/probe",
        ] {
            assert!(
                p.contains(&format!(
                    "(allow file-read-metadata (literal \"{ancestor}\"))"
                )),
                "path resolution needs a metadata grant on {ancestor}: {p}"
            );
        }

        // And the grant must be absent when the host reports no such directory,
        // so the `None` arm cannot silently widen anything.
        let none =
            SandboxExecBackend::build_profile_with_darwin_temp(&m, None).expect("profile builds");
        assert!(
            !none.contains("/private/var/folders"),
            "no confstr temp dir means no grant: {none}"
        );
    }

    /// The new grant must not outrank a secret deny. SBPL is last-match-wins
    /// and `fs_read_deny` is emitted after every allow, so a secret that
    /// happens to live under the Darwin temp directory must still be denied.
    #[test]
    fn darwin_temp_grant_still_loses_to_a_read_deny() {
        let t = std::path::Path::new("/private/var/folders/8h/probe/T");
        let m = SandboxManifest {
            fs_read_deny: vec!["/private/var/folders/8h/probe/T/secret".into()],
            ..Default::default()
        };
        let p = SandboxExecBackend::build_profile_with_darwin_temp(&m, Some(t))
            .expect("profile builds");
        let allow = p
            .find("(allow file-write* (subpath \"/private/var/folders/8h/probe/T\"))")
            .expect("temp grant present");
        let deny = p
            .find("(deny file-read* (subpath \"/private/var/folders/8h/probe/T/secret\"))")
            .expect("deny present");
        assert!(
            deny > allow,
            "the deny must come after the temp grant or last-match-wins hands the secret over: {p}"
        );
    }

    /// The string tests above prove the profile grants the directory it was
    /// GIVEN. This proves the directory it is given is the one Darwin's own
    /// tools actually use — the failure mode that motivated the fix was
    /// granting `$TMPDIR` and discovering `mktemp` never reads it.
    #[test]
    #[cfg_attr(not(target_os = "macos"), ignore = "macOS only")]
    fn darwin_user_temp_dir_is_where_mktemp_puts_files() {
        let dir = darwin_user_temp_dir().expect("confstr reports a per-user temp dir");

        // Deliberately point TMPDIR somewhere else: if `mktemp` honoured the
        // environment there would be no defect to fix, and this assertion
        // would fail rather than quietly agreeing.
        let decoy = tempfile::tempdir().expect("decoy tempdir");
        let out = std::process::Command::new("/usr/bin/mktemp")
            .env("TMPDIR", decoy.path())
            .output()
            .expect("run mktemp");
        let printed = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let canon = std::fs::canonicalize(&printed).expect("mktemp's file exists");
        let _ = std::fs::remove_file(&canon);

        assert!(
            canon.starts_with(&dir),
            "mktemp wrote {canon:?} but the profile would grant {dir:?}"
        );
    }

    /// End to end, through the production `execute`: `mktemp` must produce a
    /// file this test can see from OUTSIDE the sandbox.
    ///
    /// This is the row the live macOS matrix reported red on the merged tree —
    /// `mktemp: mkstemp failed on /var/folders/…/T/tmp.XXXXXXXXXX: Operation
    /// not permitted` — under BOTH the contained and the trusted_local profile.
    /// `WorkspacePolicy` redirects `TMPDIR`/`TMP`/`TEMP` into the session's
    /// granted scratch, which is why clang, python3 and node recovered, but
    /// Darwin's `mktemp(1)` reads `confstr(_CS_DARWIN_USER_TEMP_DIR)` first and
    /// never consults the environment, so it still targets a directory the
    /// profile does not grant.
    ///
    /// The manifest carries only the workspace, exactly like the `contained`
    /// profile, so a pass cannot come from some broader grant this test
    /// invented. `TMPDIR` is set to the workspace on purpose: it must not be
    /// what makes the test pass.
    #[tokio::test]
    #[cfg_attr(not(target_os = "macos"), ignore = "macOS only")]
    async fn sandboxed_mktemp_creates_a_file_the_host_can_see() {
        let backend = SandboxExecBackend::new();
        if !backend.is_available() {
            return;
        }
        let work = tempfile::tempdir().expect("workspace");
        let canon_work = std::fs::canonicalize(work.path()).expect("canonicalize workspace");
        let m = SandboxManifest {
            fs_write_allow: vec![canon_work.clone()],
            env: vec![
                ("PATH".into(), "/usr/bin:/bin".into()),
                // The redirect the workspace policy applies in production. It
                // must not be what makes this pass.
                ("TMPDIR".into(), canon_work.to_string_lossy().into_owned()),
            ],
            ..Default::default()
        };

        let out = backend
            .execute(
                &m,
                SandboxCommand {
                    argv: vec!["/usr/bin/mktemp".into()],
                    cwd: None,
                },
            )
            .await
            .expect("execute returns Ok");

        let stdout = String::from_utf8_lossy(&out.stdout);
        let printed = stdout.trim();
        assert_eq!(
            out.exit_code,
            0,
            "mktemp must succeed inside the sandbox; stderr={:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        let path = std::path::Path::new(printed);
        assert!(
            path.is_absolute() && path.exists(),
            "the host must see the file the sandboxed mktemp created, got {printed:?}"
        );
        let _ = std::fs::remove_file(path);
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

    #[cfg(target_os = "macos")]
    fn delayed_sentinel_command(
        started: &std::path::Path,
        sentinel: &std::path::Path,
    ) -> SandboxCommand {
        SandboxCommand {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "/usr/bin/touch \"$1\"; (sleep 2; /usr/bin/touch \"$2\") & wait".into(),
                "wcore-sentinel".into(),
                started.to_string_lossy().into_owned(),
                sentinel.to_string_lossy().into_owned(),
            ],
            cwd: None,
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn timeout_reaps_delayed_background_descendant() {
        let backend = SandboxExecBackend::new();
        assert!(backend.is_available(), "sandbox-exec must be available");
        let dir = tempfile::tempdir().expect("create sentinel directory");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize sentinel directory");
        let started = root.join("started");
        let sentinel = root.join("escaped");
        let manifest = SandboxManifest {
            fs_read_allow: vec![root.clone()],
            fs_write_allow: vec![root],
            timeout: Some(std::time::Duration::from_secs(1)),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            ..Default::default()
        };

        let result = backend
            .execute(&manifest, delayed_sentinel_command(&started, &sentinel))
            .await;
        assert!(matches!(result, Err(SandboxError::Timeout)));
        assert!(
            started.exists(),
            "sandboxed command must start before timeout"
        );
        tokio::time::sleep(std::time::Duration::from_millis(2_250)).await;
        assert!(
            !sentinel.exists(),
            "background descendant wrote after sandbox timeout"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn future_drop_reaps_delayed_background_descendant() {
        let backend = SandboxExecBackend::new();
        assert!(backend.is_available(), "sandbox-exec must be available");
        let dir = tempfile::tempdir().expect("create sentinel directory");
        let root = std::fs::canonicalize(dir.path()).expect("canonicalize sentinel directory");
        let started = root.join("started");
        let sentinel = root.join("escaped");
        let manifest = SandboxManifest {
            fs_read_allow: vec![root.clone()],
            fs_write_allow: vec![root],
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            ..Default::default()
        };

        {
            let execution =
                backend.execute(&manifest, delayed_sentinel_command(&started, &sentinel));
            tokio::pin!(execution);
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    tokio::select! {
                        result = &mut execution => {
                            panic!("sandboxed command exited before cancellation: {result:?}");
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_millis(25)) => {
                            if started.exists() {
                                break;
                            }
                        }
                    }
                }
            })
            .await
            .expect("sandboxed command must start");
        }

        tokio::time::sleep(std::time::Duration::from_millis(2_250)).await;
        assert!(
            !sentinel.exists(),
            "background descendant wrote after execution future drop"
        );
    }
}
