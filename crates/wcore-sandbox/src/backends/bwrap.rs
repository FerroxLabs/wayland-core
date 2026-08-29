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
//!   --ro-bind-try <SYSTEM_RO_ETC>   (curated /etc — NEVER the whole directory)
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
//!
//! `SandboxManifest::fs_metadata_read_allow` is deliberately NOT translated
//! here, and that is not an oversight. bwrap builds the child's mount namespace
//! CONSTRUCTIVELY: a path that was never bound is absent, so `stat` on it
//! returns **ENOENT**, not EPERM. Measured on Ubuntu 24.04 / kernel 6.8 with
//! `/root` unbound: `os.stat("/root/.gitconfig")` → `errno 2 No such file or
//! directory`. That is strictly LESS information than a metadata grant asks
//! for, and it is the answer libgit2 already tolerates — which is why `cargo
//! new` works on Linux and died on macOS. Binding the path `--ro-bind` to
//! satisfy the grant would hand the child the file's CONTENTS, a widening the
//! caller never asked for, so the entry is dropped instead.
//! `bwrap_argv_never_binds_a_metadata_only_path` pins that.

use super::SandboxBackend;
use crate::error::{Result, SandboxError};
use crate::manifest::{NetworkPolicy, SandboxManifest};
use crate::{DirectoryAuthority, ResourceLimitEnforcement, SandboxCommand, SandboxOutput};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Once;

/// System directories bound read-only into every bwrap sandbox so the inner
/// command can find standard binaries and their shared libraries.
///
/// `/etc` is deliberately NOT here — see [`SYSTEM_RO_ETC`].
const SYSTEM_RO_DIRS: [&str; 5] = ["/usr", "/lib", "/lib64", "/bin", "/sbin"];

/// The ONLY host `/etc` entries bound read-only into the sandbox.
///
/// SEC-05 / SEC-07 / SEC-10: this list replaces a blanket `--ro-bind /etc /etc`
/// that handed the child the host's entire system-configuration directory.
/// Measured under the ACTIVE sandbox on Ubuntu 24.04, `cat /etc/passwd` (the
/// host's full account list, real names included), `cat /etc/hosts` (the host's
/// public IP and hostname) and their `../../../..`-traversal spellings all
/// returned real host content, and those bytes reached the provider. The macOS
/// backend never had this hole — its profile grants `(literal "/etc")`, the
/// symlink node only, never `(subpath "/etc")` — so Linux was the permissive
/// outlier of the three backends.
///
/// Every entry here is public, machine-invariant toolchain plumbing (dynamic
/// linker configuration, the CA trust store, the timezone), NOT host-private
/// state. Host identity, accounts, network topology and service configuration
/// stay outside the namespace. Anything else a caller genuinely needs must be
/// requested explicitly through `fs_read_allow` — which is exactly what
/// `WorkspacePolicy::trusted_config_and_certificate_reads` already does.
///
/// Bound with `--ro-bind-try`, so a spelling a given distro does not use
/// (`/etc/pki` on Debian, `/etc/ca-certificates.conf` on RHEL) is skipped
/// rather than aborting the spawn.
const SYSTEM_RO_ETC: [&str; 10] = [
    // Dynamic linker: needed on distros whose library directories are not in
    // glibc's built-in search path.
    "/etc/ld.so.cache",
    "/etc/ld.so.conf",
    "/etc/ld.so.conf.d",
    // Symlink farm many distros route toolchain binaries through.
    "/etc/alternatives",
    // CA trust store — Debian and RHEL spellings.
    "/etc/ssl",
    "/etc/pki",
    "/etc/ca-certificates",
    "/etc/ca-certificates.conf",
    // Clock.
    "/etc/localtime",
    "/etc/timezone",
];

/// Bound read-only ONLY when the manifest grants the child a network. The
/// resolver configuration names the host's DNS servers and search domains, so
/// under [`NetworkPolicy::Deny`] — the default for agent-initiated Bash — it is
/// pure leak with no corresponding capability.
///
/// This gate covers the `/etc/resolv.conf` SPELLING only. It is not sufficient
/// on its own: on a systemd-resolved host the file is a symlink, and a caller
/// that grants the CANONICALIZED target through `fs_read_allow` puts the same
/// bytes back in the namespace under `/run/systemd/resolve/stub-resolv.conf`.
/// That is exactly what `WorkspacePolicy::trusted_local` used to do, which made
/// this gate inert for the policy the product actually uses. The matching
/// policy-side gate is `wcore_tools::workspace_policy::discovery::network_scoped_reads`;
/// both are needed, and neither replaces the other.
const NETWORK_RO_ETC: [&str; 1] = ["/etc/resolv.conf"];

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

    /// bwrap builds the child's mount namespace from the manifest's grants
    /// alone — an ungranted host path is simply not present in it — so a write
    /// outside every granted root cannot land on the host.
    fn confines_filesystem(&self) -> bool {
        true
    }

    fn owns_descendants_hard(&self) -> bool {
        true
    }

    /// bwrap binds the retained checkout as `/proc/self/fd/N` inside its mount
    /// namespace (see [`BubblewrapBackend::execute_bound`]), so `--chdir`
    /// resolves to the exact retained object rather than a re-openable path.
    fn binds_cwd_authority(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        self.execute_bound(manifest, cmd, None).await
    }

    async fn execute_with_cwd_authority(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
        cwd: DirectoryAuthority,
    ) -> Result<SandboxOutput> {
        self.execute_bound(manifest, cmd, Some(cwd)).await
    }

    fn hard_containment_identity(&self) -> Option<super::HardContainmentIdentity> {
        // Only a real, resolvable `bwrap` binary qualifies. When bwrap is
        // absent this is `None`, so the backend is structurally non-qualifying.
        let path = self.bwrap_path.as_deref()?;
        Some(super::HardContainmentIdentity {
            mechanism: super::HardContainmentMechanism::BubblewrapPidNamespace,
            executable_identity: path.to_owned(),
            runtime_identity: format!("bubblewrap-pid-namespace:{path}"),
            process_tree_mechanism:
                super::process_tree::ProcessTreeMechanism::LinuxPidNamespaceReap,
        })
    }

    async fn probe_hard_containment(
        &self,
        fs: &crate::manifest::HardContainmentFilesystem,
    ) -> Result<super::HardContainmentProbe> {
        // Structural gate: no usable bwrap → cannot establish hard containment.
        let identity = self.hard_containment_identity().ok_or_else(|| {
            SandboxError::PolicyNotSupported(
                "bubblewrap is unavailable for hard containment".into(),
            )
        })?;

        // Semantic live probe: actually spawn a PID-namespaced child under the
        // EXACT normalized policy. `execute_bound` reads the namespaced
        // child-pid from bwrap's `--info-fd` (failing closed if absent) and owns
        // the complete tree via `ProcessTreeGuard`, so a probe failure at any
        // stage (spawn, child-pid read, timeout, output overflow, wait) kills
        // the owned tree and returns an error here. The probe command is the
        // benign builtin `true` — NEVER candidate argv — so a failed admission
        // never runs candidate-controlled code.
        let manifest = fs.to_manifest();
        let out = self
            .execute_bound(
                &manifest,
                SandboxCommand {
                    argv: vec!["true".into()],
                    cwd: None,
                },
                None,
            )
            .await?;
        if out.exit_code != 0 {
            return Err(SandboxError::ExecFailed(format!(
                "bubblewrap hard-containment probe exited {}; stderr={}",
                out.exit_code,
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(super::HardContainmentProbe { identity })
    }
}

impl BubblewrapBackend {
    async fn execute_bound(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
        cwd_authority: Option<DirectoryAuthority>,
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

        #[cfg(target_os = "linux")]
        let (mut status_reader, status_writer, status_fd) = bwrap_status_channel()?;
        #[cfg(target_os = "linux")]
        {
            bwrap_argv.push("--info-fd".into());
            bwrap_argv.push(status_fd.to_string());
        }

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

        // Curated `/etc` — public toolchain plumbing only. See SYSTEM_RO_ETC.
        for etc in SYSTEM_RO_ETC {
            bwrap_argv.push("--ro-bind-try".into());
            bwrap_argv.push(etc.into());
            bwrap_argv.push(etc.into());
        }
        if matches!(manifest.network, NetworkPolicy::Inherit) {
            for etc in NETWORK_RO_ETC {
                bwrap_argv.push("--ro-bind-try".into());
                bwrap_argv.push(etc.into());
                bwrap_argv.push(etc.into());
            }
        }

        // A granted network needs a RESOLVER, or it is only a route. This is
        // the SECOND half of that grant, and it is deliberately kept alongside
        // the `NETWORK_RO_ETC` bind above rather than replaced by it.
        //
        // History, because the two halves were authored against different
        // trees and the reason they now coexist is not obvious. When `/etc` was
        // bound WHOLESALE from the host, the host's own `/etc/resolv.conf`
        // symlink came with it, pointing into `/run`
        // (`../run/systemd/resolve/stub-resolv.conf` on Ubuntu 24.04). `/run`
        // is not in the namespace, so the symlink dangled: glibc found no
        // nameserver and every hostname lookup failed with EAI_NONAME while
        // raw-IP connections still worked. Measured then: `cat
        // /etc/resolv.conf` inside → "No such file or directory", `curl
        // https://example.com` → exit 6 "Could not resolve host", the same curl
        // on the host → HTTP 200. Binding at `/etc/resolv.conf` could not fix
        // it either — bwrap followed the dangling symlink when creating the
        // destination and aborted the whole spawn with "Can't create file at
        // /etc/resolv.conf" — so the fix bound the CANONICAL target at its own
        // path and let the inherited symlink land on it.
        //
        // The blanket `/etc` bind is gone (SEC-05/07/10, see SYSTEM_RO_ETC), so
        // that dangling symlink is gone with it: the namespace's `/etc` is
        // synthesized, `NETWORK_RO_ETC` creates `/etc/resolv.conf` as a fresh
        // file mount point, and bwrap resolves the SOURCE in the host namespace
        // where the symlink is intact. The `/etc/resolv.conf` bind alone is
        // therefore expected to be sufficient on a systemd host today.
        //
        // This canonical-target bind is retained anyway because it is free and
        // not equivalent: it covers a resolver whose canonical path is reached
        // through a chain `NETWORK_RO_ETC` does not reproduce, and it costs
        // nothing when `/etc/resolv.conf` is a plain file (`canonicalize`
        // returns it unchanged and the bind is a harmless self-bind). It
        // exposes no bytes the `/etc/resolv.conf` bind has not already exposed
        // — the same file, reachable under two names.
        //
        // Only when the manifest actually granted network. A `Deny` namespace
        // has no use for a resolver, so this adds nothing to the default
        // posture — and `fs_read_deny` is rendered after this point, so a
        // policy that denies the path still shadows it.
        if matches!(manifest.network, NetworkPolicy::Inherit)
            && let Ok(resolved) = std::fs::canonicalize("/etc/resolv.conf")
        {
            let s = resolved.to_string_lossy().into_owned();
            bwrap_argv.push("--ro-bind-try".into());
            bwrap_argv.push(s.clone());
            bwrap_argv.push(s);
        }

        // Synthesized replacements for the identity/name-resolution files the
        // blanket `/etc` bind used to supply from the host. Held alive until
        // this function returns so the sources still exist when bwrap builds
        // its namespace. None of SYNTHETIC_ETC_FILES is `resolv.conf`, so this
        // block cannot shadow either resolver bind above under later-arg-wins;
        // the synthetic `nsswitch.conf` carries `hosts: files dns` so DNS is
        // still consulted.
        #[cfg(target_os = "linux")]
        let _synthetic_etc = {
            let scaffold = synthetic_etc_scaffold()?;
            for name in SYNTHETIC_ETC_FILES {
                bwrap_argv.push("--ro-bind".into());
                bwrap_argv.push(scaffold.path().join(name).to_string_lossy().into_owned());
                bwrap_argv.push(format!("/etc/{name}"));
            }
            scaffold
        };

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
        // mount ordering shadows them. Directory denies use one empty,
        // read-only bind. A writable tmpfs is not a denial: it hides reads but
        // lets the child mint replacement authority at the denied pathname.
        //
        // Classify every denied path ONCE, then reduce the set to the mounts
        // bubblewrap can actually realize (see `reduce_read_deny_mounts`), then
        // render. Classifying before reducing is what lets the reduction know
        // which entries are directories, and directories are the only entries
        // that can subsume another.
        let deny_entries: Vec<(PathBuf, DenyMountKind)> = manifest
            .fs_read_deny
            .iter()
            .map(|path| {
                let kind = match std::fs::symlink_metadata(path) {
                    Ok(md) if md.is_dir() => DenyMountKind::Directory,
                    Ok(_) => DenyMountKind::NonDirectory,
                    // Path gone since enumeration — nothing to mask.
                    Err(_) => DenyMountKind::Absent,
                };
                (path.clone(), kind)
            })
            .collect();
        let deny_mounts = reduce_read_deny_mounts(&deny_entries);
        let denied_directory_mask = if deny_mounts
            .iter()
            .any(|(_, kind)| *kind == DenyMountKind::Directory)
        {
            Some(tempfile::tempdir().map_err(|error| {
                SandboxError::ExecFailed(format!("create read-deny directory mask: {error}"))
            })?)
        } else {
            None
        };
        for (path, kind) in &deny_mounts {
            let source = match kind {
                DenyMountKind::Directory => denied_directory_mask
                    .as_ref()
                    .expect("directory deny mask was created")
                    .path()
                    .to_string_lossy()
                    .into_owned(),
                DenyMountKind::NonDirectory => "/dev/null".to_owned(),
                DenyMountKind::Absent => {
                    unreachable!("reduce_read_deny_mounts drops absent denies")
                }
            };
            bwrap_argv.push("--ro-bind".into());
            bwrap_argv.push(source);
            bwrap_argv.push(path.to_string_lossy().into_owned());
        }

        // Env injection (manifest-only; host env is dropped by --clearenv).
        for (k, v) in &manifest.env {
            bwrap_argv.push("--setenv".into());
            bwrap_argv.push(k.clone());
            bwrap_argv.push(v.clone());
        }

        // Working directory. Delegated execution binds the retained directory
        // descriptor into the namespace as `/proc/self/fd/N` and chdirs there,
        // so a pathname replacement between admission and spawn cannot redirect
        // the working directory; ordinary callers keep the path-based mode. The
        // inheritable loan is held until this function returns so the descriptor
        // stays valid while bwrap builds its namespace.
        #[cfg(target_os = "linux")]
        let _cwd_handle = {
            use std::os::fd::AsRawFd;
            if let Some(authority) = cwd_authority.as_ref() {
                if cmd.cwd.as_deref() != Some(authority.display_path()) {
                    return Err(SandboxError::PathDenied(
                        "bubblewrap cwd does not match retained authority".to_owned(),
                    ));
                }
                let handle = authority.try_clone_inheritable_handle()?;
                let source = format!("/proc/self/fd/{}", handle.as_raw_fd());
                let destination = authority.display_path().to_string_lossy().into_owned();
                bwrap_argv.push("--bind".into());
                bwrap_argv.push(source);
                bwrap_argv.push(destination.clone());
                bwrap_argv.push("--chdir".into());
                bwrap_argv.push(destination);
                Some(handle)
            } else if let Some(cwd) = &cmd.cwd {
                bwrap_argv.push("--chdir".into());
                bwrap_argv.push(cwd.to_string_lossy().into_owned());
                None
            } else {
                None
            }
        };
        // bwrap runs only on Linux; on other targets this file compiles as a
        // stub, so the retained-descriptor bind is unavailable and the retained
        // authority (if any) is validated for path agreement only.
        #[cfg(not(target_os = "linux"))]
        {
            if let Some(authority) = cwd_authority.as_ref()
                && cmd.cwd.as_deref() != Some(authority.display_path())
            {
                return Err(SandboxError::PathDenied(
                    "bubblewrap cwd does not match retained authority".to_owned(),
                ));
            }
            if let Some(cwd) = &cmd.cwd {
                bwrap_argv.push("--chdir".into());
                bwrap_argv.push(cwd.to_string_lossy().into_owned());
            }
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

        // SEC-06 / SEC-10 — every filesystem the manifest did NOT grant is
        // remounted read-only, LAST, after every positive bind above.
        //
        // bubblewrap builds its new root as a fresh tmpfs and `--tmpfs /tmp`
        // adds a second one, and both are WRITABLE. So a write to any path the
        // manifest never granted — `/tmp/out.txt`, or `/opt/x` after the child
        // creates `/opt` — succeeded, exited 0, read back correctly inside the
        // namespace, and then vanished when the namespace was torn down. The
        // host path never existed. macOS/sandbox-exec denies the same write
        // with EPERM, so on macOS the agent is told the truth and on Linux it
        // was told it had saved a file it had not. Data loss reported as
        // success is worse than a refusal: an agent builds on the belief.
        //
        // `--remount-ro DEST` remounts only the mount point at DEST — SUBMOUNTS
        // are untouched. Every `fs_write_allow` entry is its own bind mount, so
        // a granted root keeps full write access even when it lives under
        // `/tmp` (which is where `std::env::temp_dir()`-derived workspaces and
        // the `wayland-scratch-u<uid>` scratch grants actually are). Verified
        // on hetzner-dsm with bubblewrap 0.9.0: a rw bind under a read-only
        // `/tmp` still writes, and both `/tmp/<ungranted>` and `/<ungranted>`
        // fail with EROFS.
        //
        // The cost is that an ungranted `/tmp` write now FAILS instead of
        // silently evaporating. Measured against git / node / python3 / gcc
        // under this exact posture: all four still work, falling back to
        // `TMPDIR` or the cwd.
        bwrap_argv.push("--remount-ro".into());
        bwrap_argv.push("/tmp".into());
        bwrap_argv.push("--remount-ro".into());
        bwrap_argv.push("/".into());

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
        super::process_tree::isolate(&mut command);

        // NOTE: Landlock is deliberately NOT applied around the bwrap backend.
        // A `pre_exec` ruleset is inherited by bwrap and confines bwrap's OWN
        // privileged setup (writing /proc/self/uid_map to build its user
        // namespace), which fails with EACCES the moment the allowlist is
        // non-empty. bwrap's `--unshare-all` + the constructive `--ro-bind`
        // set already provide a deny-by-default filesystem view that is a strict
        // superset of any Landlock allowlist built from the same paths, and the
        // secret-read-deny enforcement rides on read-only empty directory /
        // `/dev/null` overlays above — not on Landlock. bwrap sets NO_NEW_PRIVS
        // itself. The `landlock` feature + `bwrap_landlock.rs` remain compiled
        // (exercised by --all-features CI) as the foundation for a future
        // inner-command re-exec shim, but production runs seccomp-only.

        let mut child = command
            .spawn()
            .map_err(|e| SandboxError::ExecFailed(format!("bwrap spawn failed: {e}")))?;
        let mut process_tree =
            super::process_tree::ProcessTreeGuard::new(child.id()).map_err(|error| {
                SandboxError::ExecFailed(format!("process-tree ownership: {error}"))
            })?;
        // `None` means the sandboxed command had already finished by the time
        // its pid reached us — see `from_observed_root` for why that is routine
        // for a fast command on a loaded host, and why a dead PID-namespace
        // init leaves no tree to own. A completed command is not a failure.
        #[cfg(target_os = "linux")]
        let mut sandbox_tree = {
            drop(status_writer);
            let child_pid = read_bwrap_child_pid(&mut status_reader)?;
            super::process_tree::ProcessTreeGuard::from_observed_root(child_pid).map_err(
                |error| {
                    SandboxError::ExecFailed(format!("sandbox process-tree ownership: {error}"))
                },
            )?
        };

        // Now safe to drop the BPF tempfile — bwrap has read the fd into
        // its child setup. Holding it longer wastes a fd until return.
        drop(seccomp_file);

        // 6. Timeout + wait.
        //
        // The wall-clock bound is the CALLER's, and only the caller's. This
        // used to be `manifest.timeout.unwrap_or(30s)`, an invented Linux-only
        // cap: `BashTool` advertises "default 120000, max 600000" ms to the
        // model and passes no `manifest.timeout`, so every Bash command on
        // Linux — and only on Linux — was killed at 30 s with all of its
        // output discarded and nothing in the result saying why. macOS's
        // `SandboxExecBackend` imposes no cap of its own; this now matches it,
        // so the number the tool advertises is the number that is enforced.
        // Every in-tree caller that leaves `timeout` at None wraps the call in
        // its own `tokio::time::timeout`, and cancelling that future drops the
        // child, which arms the guards below exactly as the explicit-timeout
        // arm does.
        let wait_fut = super::wait_with_bounded_output_on_exit(&mut child, || {
            #[cfg(target_os = "linux")]
            if let Some(sandbox_tree) = sandbox_tree.as_mut() {
                sandbox_tree.disarm();
            }
            process_tree.disarm();
        });
        let output = match manifest.timeout {
            Some(timeout) => match tokio::time::timeout(timeout, wait_fut).await {
                Ok(Ok(out)) => out,
                Ok(Err(e)) => {
                    return Err(e);
                }
                Err(_elapsed) => {
                    // Dropping this future arms `ProcessTreeGuard` before the
                    // direct bwrap handle is dropped. Linux descendant discovery
                    // kills the PID-namespace init and its complete tree; the
                    // dedicated outer process group is the final backstop.
                    return Err(SandboxError::Timeout);
                }
            },
            None => wait_fut.await?,
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

/// How a single `fs_read_deny` entry is realized as a bubblewrap mount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DenyMountKind {
    /// A directory: masked with one empty, read-only bind.
    Directory,
    /// Exists and is not a directory: masked with a `/dev/null` bind.
    NonDirectory,
    /// Vanished between enumeration and render — nothing to mask.
    Absent,
}

/// Basenames written by [`synthetic_etc_scaffold`] and bound over `/etc/<name>`.
#[cfg(target_os = "linux")]
const SYNTHETIC_ETC_FILES: [&str; 4] = ["passwd", "group", "hosts", "nsswitch.conf"];

/// Materialise minimal stand-ins for the four `/etc` files the blanket `/etc`
/// bind used to supply from the host.
///
/// Dropping the host copies outright is not free: `getpwuid(3)` starts failing,
/// which breaks `pwd.getpwuid()` in Python and throws outright in Node's
/// `os.userInfo()`, and without `/etc/hosts` + an nsswitch policy glibc cannot
/// resolve `localhost` at all. Both were measured, not assumed. So the child
/// gets files with the same SHAPE and none of the host's content: its own uid
/// under a fixed sandbox name, loopback only, and a `files dns` policy that
/// matches what is actually present in the namespace. Nothing here is derived
/// from the host beyond the uid/gid the child already runs as and can read from
/// `getuid(2)` regardless.
///
/// KNOWN RESIDUAL (open, not reachable through `BashTool`). The synthetic
/// `passwd` gives the sandbox user `pw_dir` `/`. With `HOME` UNSET the child
/// therefore falls back to `/` as its home, and `cargo` goes exit 0 → exit 1
/// with "rustup could not choose a version of cargo to run" because the rustup
/// shim looks for its toolchain under `$HOME/.rustup`. Measured on Ubuntu
/// 24.04. It is unreachable in production because `HOME` is on
/// `BASE_SANDBOX_ENV_ALLOWLIST`, so every `BashTool` child receives a real
/// `HOME`; cargo exit 0 was re-proved through the real env builder. A caller
/// that builds a manifest by hand and omits `HOME` will hit it.
///
/// NOT to be confused with the toolchain-outside-`$HOME` defect, which printed
/// the SAME rustup message from a completely different cause and WAS reachable
/// through `BashTool`: `HOME` set and correct, but the toolchain living at
/// `RUSTUP_HOME=/usr/local/rustup` (the official `rust:*` images, most
/// devcontainers, Nix) while both the env allowlist and
/// `minimal_toolchain_read_dirs` derived only from `$HOME`. Closed — see
/// `crates/wcore-tools/tests/toolchain_outside_home.rs`. A future sighting of
/// this error string should check `RUSTUP_HOME` before assuming it is the
/// residual above.
#[cfg(target_os = "linux")]
fn synthetic_etc_scaffold() -> Result<tempfile::TempDir> {
    // SAFETY: getuid/getgid are always-successful, thread-safe syscalls that
    // take no arguments and cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let dir = tempfile::tempdir()
        .map_err(|e| SandboxError::ExecFailed(format!("create synthetic /etc scaffold: {e}")))?;
    let files = [
        (
            "passwd",
            format!(
                "sandbox:x:{uid}:{gid}:sandboxed user:/:/bin/sh\n\
                 nobody:x:65534:65534:nobody:/nonexistent:/bin/false\n"
            ),
        ),
        ("group", format!("sandbox:x:{gid}:\nnogroup:x:65534:\n")),
        (
            "hosts",
            "127.0.0.1\tlocalhost\n::1\tlocalhost ip6-localhost ip6-loopback\n".to_owned(),
        ),
        (
            "nsswitch.conf",
            "passwd: files\ngroup: files\nshadow: files\n\
             hosts: files dns\nservices: files\nprotocols: files\nnetworks: files\n"
                .to_owned(),
        ),
    ];
    for (name, body) in files {
        std::fs::write(dir.path().join(name), body)
            .map_err(|e| SandboxError::ExecFailed(format!("write synthetic /etc/{name}: {e}")))?;
    }
    Ok(dir)
}

/// Reduce classified `fs_read_deny` entries to the mounts bubblewrap can
/// actually realize, preserving input order.
///
/// **Why this exists (21-C3-01).** bubblewrap enforces a deny by MOUNTING over
/// the denied path, and a mount needs its mount point to be creatable. Once
/// `/p` carries an empty READ-ONLY mask, `/p/q` no longer exists inside the
/// namespace and cannot be `mkdir`'d, so a second `--ro-bind` at `/p/q` aborts
/// the entire spawn before `execve`:
///
/// ```text
/// bwrap: Can't mkdir /…/workspace/.git: Read-only file system
/// ```
///
/// Measured on `hetzner-dsm` (bubblewrap 0.9.0): with the same two paths,
/// `[/p, /p/q]` exits 1 having run nothing while `[/p/q, /p]` runs the shell
/// and exits 0. The rendering was therefore ORDER-DEPENDENT even though
/// `fs_read_deny` is a set, which is why the reduction lives here and not at
/// any one caller — a caller cannot be asked to know bubblewrap's mount
/// ordering, and three independent producers hand this renderer nested pairs:
///
/// - `wcore_agent::spawner`'s `[parent_workspace, git_common_dir]` for an
///   isolated-mutation child. That pair is CORRECT: `git_common_dir` is
///   `<parent>/.git` for an ordinary clone but the MAIN repo's `.git` — outside
///   the parent — when the parent is itself a linked worktree. Whether it nests
///   is a property of the parent's git layout, not a caller mistake.
/// - `WorkspacePolicy::secret_deny_paths_dynamic`, whose secret walk can return
///   both a credentials directory and a file inside it.
/// - `wcore_swarm::dispatch`'s `sandbox_read_denies`.
///
/// The other two backends have no analogue: macOS `sandbox_exec` emits
/// independent `(deny file-read* (subpath …))` SBPL rules and Windows
/// AppContainer applies a protected DACL per object, so overlapping denies are
/// simply both enforced.
///
/// **Why dropping is safe.** A deny nested under a DIRECTORY deny is redundant:
/// the ancestor's empty read-only mask removes the descendant pathname from the
/// namespace entirely, which is at least as strong as masking it directly
/// (verified: with only the ancestor denied, a read of `<parent>/.git/config`
/// fails and a write to `<parent>/.git/` fails `Directory nonexistent`).
/// A `NonDirectory` entry can have no descendants, and an entry is never
/// dropped for any other reason — this is the sole reduction.
///
/// Nesting is decided component-wise via `Path::starts_with`, so `/p-backup` is
/// not treated as nested under `/p`. Comparison is lexical, so a symlinked
/// alias of a denied ancestor is NOT recognised and both denies are emitted —
/// that is the safe direction (a redundant mount that may abort, never a
/// silently discarded denial).
fn reduce_read_deny_mounts(entries: &[(PathBuf, DenyMountKind)]) -> Vec<(PathBuf, DenyMountKind)> {
    let mut kept = Vec::with_capacity(entries.len());
    for (index, (path, kind)) in entries.iter().enumerate() {
        if *kind == DenyMountKind::Absent {
            continue;
        }
        // Collapse an exact repeat onto its first occurrence.
        if entries[..index].iter().any(|(earlier, earlier_kind)| {
            *earlier_kind != DenyMountKind::Absent && earlier == path
        }) {
            continue;
        }
        // Drop anything strictly nested under a directory deny, wherever that
        // ancestor sits in the list. Scanning the WHOLE list rather than the
        // prefix is deliberate: the abort this prevents was order-dependent, so
        // the reduction must not be.
        if entries.iter().any(|(ancestor, ancestor_kind)| {
            *ancestor_kind == DenyMountKind::Directory
                && ancestor != path
                && path.starts_with(ancestor)
        }) {
            continue;
        }
        kept.push((path.clone(), *kind));
    }
    kept
}

#[cfg(target_os = "linux")]
fn bwrap_status_channel() -> Result<(
    std::io::BufReader<std::os::unix::net::UnixStream>,
    std::os::unix::net::UnixStream,
    std::os::fd::RawFd,
)> {
    use std::os::fd::AsRawFd;

    let (reader, writer) = std::os::unix::net::UnixStream::pair()
        .map_err(|error| SandboxError::ExecFailed(format!("bwrap status channel: {error}")))?;
    let fd = writer.as_raw_fd();
    // SAFETY: F_SETFD only updates flags on the owned writer descriptor.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, 0) } == -1 {
        return Err(SandboxError::ExecFailed(format!(
            "bwrap status descriptor: {}",
            std::io::Error::last_os_error()
        )));
    }
    reader
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|error| SandboxError::ExecFailed(format!("bwrap status timeout: {error}")))?;
    Ok((std::io::BufReader::new(reader), writer, fd))
}

#[cfg(target_os = "linux")]
fn read_bwrap_child_pid(reader: &mut impl std::io::Read) -> Result<u32> {
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let status = <serde_json::Value as serde::Deserialize>::deserialize(&mut deserializer)
        .map_err(|error| SandboxError::ExecFailed(format!("bwrap status JSON: {error}")))?;
    status
        .get("child-pid")
        .and_then(serde_json::Value::as_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| SandboxError::ExecFailed("bwrap status omitted child-pid".to_owned()))
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

    // ── 21-C3-01: overlapping fs_read_deny entries aborted bubblewrap ────────
    //
    // The renderer mounts over each denied path, so a deny nested under a
    // directory deny needed a mount point inside a read-only mask and bwrap
    // aborted before `execve`. `reduce_read_deny_mounts` drops the nested,
    // redundant deny. The live proof that containment survives the drop is
    // `required_live_bwrap_overlapping_deny_runs_shell_and_still_contains`.

    fn dir(path: &str) -> (PathBuf, DenyMountKind) {
        (PathBuf::from(path), DenyMountKind::Directory)
    }
    fn file(path: &str) -> (PathBuf, DenyMountKind) {
        (PathBuf::from(path), DenyMountKind::NonDirectory)
    }
    fn gone(path: &str) -> (PathBuf, DenyMountKind) {
        (PathBuf::from(path), DenyMountKind::Absent)
    }
    fn paths(reduced: &[(PathBuf, DenyMountKind)]) -> Vec<String> {
        reduced
            .iter()
            .map(|(p, _)| p.to_string_lossy().into_owned())
            .collect()
    }

    /// The exact `spawner.rs` pair for an isolated-mutation child, in the order
    /// it is constructed: parent workspace then `<parent>/.git`. This is the
    /// input that aborted bwrap.
    #[test]
    fn nested_directory_deny_collapses_onto_its_ancestor() {
        let entries = vec![dir("/ws/parent"), dir("/ws/parent/.git")];
        let reduced = reduce_read_deny_mounts(&entries);
        assert_eq!(
            paths(&reduced),
            vec!["/ws/parent".to_owned()],
            "the nested .git deny is redundant under the parent mask and must not be mounted"
        );
        // Third assertion (LANE-BRIEF §6b-ii): the pre-fix renderer emitted one
        // mount per entry, so it would have emitted TWO here — the exact
        // sequence bwrap 0.9.0 aborts on. Without this the test would also pass
        // on the unfixed renderer.
        assert_eq!(
            entries.len(),
            2,
            "the unreduced input must still be the two-mount sequence that aborted"
        );
    }

    /// The abort was order-dependent — `[/p, /p/q]` failed while `[/p/q, /p]`
    /// succeeded — so the reduction must not be. Both orders must reduce to the
    /// same single ancestor mount.
    #[test]
    fn deny_reduction_is_independent_of_input_order() {
        let ancestor_first = reduce_read_deny_mounts(&[dir("/ws/parent"), dir("/ws/parent/.git")]);
        let descendant_first =
            reduce_read_deny_mounts(&[dir("/ws/parent/.git"), dir("/ws/parent")]);
        assert_eq!(paths(&ancestor_first), vec!["/ws/parent".to_owned()]);
        assert_eq!(paths(&descendant_first), paths(&ancestor_first));
    }

    /// A file nested under a denied directory is equally redundant, and the
    /// whole chain collapses to the outermost directory.
    #[test]
    fn nested_file_and_deep_chain_collapse_to_the_outermost_directory() {
        let reduced = reduce_read_deny_mounts(&[
            dir("/ws/parent"),
            file("/ws/parent/.env"),
            dir("/ws/parent/.git"),
            file("/ws/parent/.git/config"),
        ]);
        assert_eq!(paths(&reduced), vec!["/ws/parent".to_owned()]);
    }

    /// Nesting is component-wise, not a string prefix: `/ws/parent-backup` is a
    /// SIBLING of `/ws/parent` and dropping it would silently discard a denial.
    #[test]
    fn string_prefix_sibling_is_not_treated_as_nested() {
        let reduced = reduce_read_deny_mounts(&[dir("/ws/parent"), dir("/ws/parent-backup")]);
        assert_eq!(
            paths(&reduced),
            vec!["/ws/parent".to_owned(), "/ws/parent-backup".to_owned()],
            "a sibling sharing a string prefix must keep its own deny mount"
        );
    }

    /// Only a DIRECTORY deny masks a subtree. A non-directory deny is rendered
    /// as a `/dev/null` bind over one pathname and subsumes nothing, so nothing
    /// may be dropped on account of it. (Unreachable from a real filesystem;
    /// pinned so a future classification change cannot open a hole quietly.)
    #[test]
    fn non_directory_deny_subsumes_nothing() {
        let reduced = reduce_read_deny_mounts(&[file("/ws/parent"), dir("/ws/parent/.git")]);
        assert_eq!(
            paths(&reduced),
            vec!["/ws/parent".to_owned(), "/ws/parent/.git".to_owned()]
        );
    }

    /// A path that vanished between enumeration and render has nothing to mask,
    /// and — critically — cannot be treated as a directory that subsumes its
    /// descendants, because no mask will be mounted at it.
    #[test]
    fn absent_ancestor_is_dropped_and_does_not_subsume_its_descendant() {
        let reduced = reduce_read_deny_mounts(&[gone("/ws/parent"), dir("/ws/parent/.git")]);
        assert_eq!(
            paths(&reduced),
            vec!["/ws/parent/.git".to_owned()],
            "an absent ancestor mounts no mask, so the descendant deny must survive"
        );
    }

    /// An exact repeat collapses onto its first occurrence — one mount, one
    /// pathname.
    #[test]
    fn exact_duplicate_deny_collapses_to_one_mount() {
        let reduced = reduce_read_deny_mounts(&[dir("/ws/parent"), dir("/ws/parent")]);
        assert_eq!(paths(&reduced), vec!["/ws/parent".to_owned()]);
    }

    /// Nothing is dropped when nothing overlaps: the reduction is not a
    /// general-purpose filter and must leave a disjoint deny set untouched.
    /// This is the "would the gate fail" control for every test above.
    #[test]
    fn disjoint_denies_are_left_untouched() {
        let reduced = reduce_read_deny_mounts(&[
            dir("/ws/parent"),
            dir("/elsewhere/main-repo/.git"),
            file("/ws/other/.env"),
        ]);
        assert_eq!(
            paths(&reduced),
            vec![
                "/ws/parent".to_owned(),
                "/elsewhere/main-repo/.git".to_owned(),
                "/ws/other/.env".to_owned(),
            ],
            "a linked-worktree parent's git_common_dir sits outside the parent and must survive"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn multiline_bwrap_status_yields_child_pid_without_waiting_for_eof() {
        struct LiveStatus<'a> {
            bytes: &'a [u8],
        }

        impl std::io::Read for LiveStatus<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                if self.bytes.is_empty() {
                    return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock));
                }
                let count = output.len().min(self.bytes.len());
                output[..count].copy_from_slice(&self.bytes[..count]);
                self.bytes = &self.bytes[count..];
                Ok(count)
            }
        }

        let mut status = LiveStatus {
            bytes: b"{\n  \"child-pid\": 4242\n}\n",
        };
        assert_eq!(read_bwrap_child_pid(&mut status).unwrap(), 4242);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn truncated_bwrap_status_fails_closed() {
        let mut status = std::io::Cursor::new(b"{\n  \"child-pid\": 4242".as_slice());
        let error = read_bwrap_child_pid(&mut status).unwrap_err();
        assert!(error.to_string().contains("bwrap status JSON"));
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
    async fn bwrap_denied_directory_is_not_writable() {
        let backend = BubblewrapBackend::new();
        if !backend.is_available() {
            eprintln!("bwrap not available; skipping");
            return;
        }
        let root = tempfile::tempdir().expect("tempdir");
        let denied = root.path().join("authority");
        std::fs::create_dir(&denied).unwrap();
        let target = denied.join("replacement");
        let manifest = SandboxManifest {
            fs_write_allow: vec![root.path().to_path_buf()],
            fs_read_deny: vec![denied],
            ..Default::default()
        };
        let output = backend
            .execute(
                &manifest,
                SandboxCommand {
                    argv: vec![
                        "/usr/bin/touch".into(),
                        target.to_string_lossy().into_owned(),
                    ],
                    cwd: None,
                },
            )
            .await
            .unwrap();
        assert_ne!(output.exit_code, 0, "denied directory accepted a write");
        assert!(!target.exists());
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

    /// Required live acceptance: bwrap is installed AND usable, and the backend
    /// admits for delegated execution (enforces read-deny, owns descendants
    /// hard, binds retained cwd authority, does not bypass containment). Fails
    /// if bwrap is absent — never skips.
    #[tokio::test]
    #[cfg_attr(not(target_os = "linux"), ignore = "bwrap is Linux-only")]
    async fn required_live_bwrap_admission() {
        let backend = BubblewrapBackend::new();
        assert!(
            backend.is_available(),
            "required live bwrap must be installed and usable"
        );
        assert!(backend.enforces_read_deny());
        assert!(backend.owns_descendants_hard());
        assert!(backend.binds_cwd_authority());
        let registry = crate::SandboxRegistry::new(std::sync::Arc::new(BubblewrapBackend::new()));
        assert!(
            !registry.bypasses_containment(),
            "delegated bwrap admission must not bypass containment"
        );
        assert!(registry.binds_workspace_authority());
        // Prove real usability, not merely PATH presence.
        let output = backend
            .execute(
                &SandboxManifest {
                    network: NetworkPolicy::Deny,
                    ..Default::default()
                },
                SandboxCommand {
                    argv: vec!["true".into()],
                    cwd: None,
                },
            )
            .await
            .expect("required live bwrap admission execution");
        assert_eq!(output.exit_code, 0, "{output:?}");
    }

    /// Required live acceptance for 21-C3-01, BOTH HALVES in one run.
    ///
    /// Arm 1 is the instrument control: the same probe, the same paths, with an
    /// EMPTY deny list. Both secrets must be readable. Without it a probe that
    /// can never see the parent (wrong path, unbound mount, dead shell) would
    /// report "REFUSED" for free, and the containment half would be vacuous.
    ///
    /// Arm 2 is the fix: `fs_read_deny = [parent, parent/.git]` — the exact
    /// overlapping pair `spawner.rs` builds for an isolated-mutation child.
    /// It must (a) RUN the shell, which before the fix it could not, bwrap
    /// aborting with `Can't mkdir …/.git: Read-only file system`; and (b) still
    /// refuse BOTH the parent read and the `.git` read. Either half alone is
    /// not a pass: (a) without (b) is a hole, (b) without (a) is the broken
    /// build that made three plans record a refusal that never happened.
    ///
    /// Fails if bwrap is absent — never skips.
    #[tokio::test]
    #[cfg_attr(not(target_os = "linux"), ignore = "bwrap is Linux-only")]
    async fn required_live_bwrap_overlapping_deny_runs_shell_and_still_contains() {
        let backend = BubblewrapBackend::new();
        assert!(
            backend.is_available(),
            "required live bwrap must be installed and usable"
        );

        let parent = tempfile::tempdir().expect("parent workspace");
        let parent_root = std::fs::canonicalize(parent.path()).expect("canonicalize parent");
        std::fs::create_dir_all(parent_root.join(".git")).expect("parent .git");
        std::fs::write(parent_root.join("secret.txt"), b"PARENTSECRET\n").expect("parent secret");
        std::fs::write(parent_root.join(".git").join("config"), b"GITSECRET\n")
            .expect("git secret");
        let child = tempfile::tempdir().expect("child workspace");
        let child_root = std::fs::canonicalize(child.path()).expect("canonicalize child");

        // The liveness marker is assembled by the shell at runtime rather than
        // written literally, so only a shell that actually executed can produce
        // the joined string. (21-C3 §6: a marker embedded literally in the
        // command text is satisfiable without the command ever running.)
        let script = format!(
            "printf %s%s SHELL RAN; echo; \
             cat {parent}/secret.txt 2>/dev/null; \
             cat {parent}/.git/config 2>/dev/null; \
             exit 0",
            parent = parent_root.display()
        );
        let run = |deny: Vec<PathBuf>| {
            let manifest = SandboxManifest {
                fs_read_allow: vec![parent_root.clone()],
                fs_write_allow: vec![child_root.clone()],
                fs_read_deny: deny,
                network: NetworkPolicy::Deny,
                ..Default::default()
            };
            let command = SandboxCommand {
                argv: vec!["/bin/sh".into(), "-c".into(), script.clone()],
                cwd: Some(child_root.clone()),
            };
            async move {
                let backend = BubblewrapBackend::new();
                backend.execute(&manifest, command).await
            }
        };

        // ── Arm 1: control. No deny — the probe MUST see both secrets. ───────
        let control = run(Vec::new()).await.expect("control execution");
        let control_stdout = String::from_utf8_lossy(&control.stdout).into_owned();
        assert!(
            control_stdout.contains("SHELLRAN"),
            "control shell must run; stdout={control_stdout:?} stderr={:?}",
            String::from_utf8_lossy(&control.stderr)
        );
        assert!(
            control_stdout.contains("PARENTSECRET") && control_stdout.contains("GITSECRET"),
            "instrument is dead: the probe cannot read either secret even with NO deny, so a \
             REFUSED reading in arm 2 would prove nothing; stdout={control_stdout:?}"
        );

        // ── Arm 2: the spawner's overlapping pair, ancestor first. ───────────
        let denied = run(vec![parent_root.clone(), parent_root.join(".git")])
            .await
            .expect("overlapping-deny execution");
        let denied_stdout = String::from_utf8_lossy(&denied.stdout).into_owned();
        let denied_stderr = String::from_utf8_lossy(&denied.stderr).into_owned();
        // Half 1 — the shell RUNS. This is what 21-C3-01 broke.
        assert!(
            !denied_stderr.contains("Can't mkdir"),
            "bwrap aborted on the overlapping deny pair (21-C3-01): stderr={denied_stderr:?}"
        );
        assert!(
            denied_stdout.contains("SHELLRAN"),
            "a delegated mutating child must be able to run a shell; \
             stdout={denied_stdout:?} stderr={denied_stderr:?}"
        );
        // Half 2 — containment is NOT weakened. Both reads still refused.
        assert!(
            !denied_stdout.contains("PARENTSECRET"),
            "parent workspace leaked into the child: stdout={denied_stdout:?}"
        );
        assert!(
            !denied_stdout.contains("GITSECRET"),
            "parent .git leaked into the child — the nested deny was dropped without the \
             ancestor mask covering it: stdout={denied_stdout:?}"
        );
    }

    /// Required live acceptance: bubblewrap mints hard containment ONLY after a
    /// real PID-namespace probe, and the minted authority binds the exact
    /// backend + normalized policy. Drift in the spawn parameters refuses. Fails
    /// if bwrap is absent — never skips.
    #[tokio::test]
    #[cfg_attr(not(target_os = "linux"), ignore = "bwrap is Linux-only")]
    async fn required_live_bwrap_hard_containment_mint_and_drift() {
        let backend = BubblewrapBackend::new();
        assert!(
            backend.is_available(),
            "required live bwrap must be installed and usable"
        );
        // A cheap identity is available and names the PID-namespace mechanism.
        let identity = backend
            .hard_containment_identity()
            .expect("bwrap must offer a hard-containment identity");
        assert_eq!(
            identity.mechanism,
            crate::HardContainmentMechanism::BubblewrapPidNamespace
        );

        // Candidate/roots must be outside global temp/home; place them on a
        // synthetic absolute tree (existence is not required — bwrap binds them
        // with `-try` semantics and skips the missing sources).
        let fs = crate::manifest::HardContainmentFilesystem::new(
            std::path::PathBuf::from("/srv/wl-hard/candidate"),
            vec![std::path::PathBuf::from("/srv/wl-hard/scratch")],
        )
        .expect("policy validates");

        let registry = crate::SandboxRegistry::new(std::sync::Arc::new(BubblewrapBackend::new()));
        let cmd = SandboxCommand {
            argv: vec!["/bin/echo".into(), "hi".into()],
            cwd: None,
        };
        let authority = registry
            .establish_hard_containment(&fs, &cmd)
            .await
            .expect("live bwrap PID-namespace probe must mint hard containment");

        // Drifted argv is refused (fail closed) — this authority is one-use.
        let drifted = SandboxCommand {
            argv: vec!["/bin/echo".into(), "TAMPERED".into()],
            cwd: None,
        };
        let err = registry
            .verify_hard_containment(authority, &fs, &drifted)
            .expect_err("spawn-parameter drift must refuse");
        assert!(matches!(err, SandboxError::ExecFailed(_)), "{err:?}");
    }

    /// Required live acceptance: `execute_with_cwd_authority` binds the retained
    /// directory object as the child's cwd via `/proc/self/fd/N` + `--chdir`.
    /// After the authority is retained the parent pathname is swapped for a
    /// decoy; the child must still see and mutate the RETAINED object, proving
    /// the fd binding is not redirected by a pathname replacement. Fails if
    /// bwrap is absent — never skips.
    #[tokio::test]
    #[cfg_attr(not(target_os = "linux"), ignore = "bwrap is Linux-only")]
    async fn required_live_bwrap_retained_cwd_enforcement() {
        let backend = BubblewrapBackend::new();
        assert!(
            backend.is_available(),
            "required live bwrap must be installed and usable"
        );
        let tmp = tempfile::tempdir().unwrap();
        let checkout = tmp.path().join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::write(checkout.join("seed"), b"retained").unwrap();
        let authority = DirectoryAuthority::open(&checkout).unwrap();

        // Pathname swap AFTER retention: move the real object aside, plant a
        // decoy at the original path. The retained fd must win.
        let moved = tmp.path().join("moved");
        std::fs::rename(&checkout, &moved).unwrap();
        std::fs::create_dir(&checkout).unwrap();
        std::fs::write(checkout.join("seed"), b"decoy").unwrap();

        let output = backend
            .execute_with_cwd_authority(
                &SandboxManifest {
                    network: NetworkPolicy::Deny,
                    ..Default::default()
                },
                SandboxCommand {
                    argv: vec![
                        "sh".into(),
                        "-c".into(),
                        "cat seed; printf bound > marker".into(),
                    ],
                    cwd: Some(checkout.clone()),
                },
                authority,
            )
            .await
            .expect("required live retained-cwd bwrap execution");
        assert_eq!(output.exit_code, 0, "{output:?}");
        // The child read and mutated the RETAINED object, never the decoy.
        assert_eq!(String::from_utf8_lossy(&output.stdout), "retained");
        assert_eq!(std::fs::read(moved.join("marker")).unwrap(), b"bound");
        assert!(
            !checkout.join("marker").exists(),
            "child escaped to the swapped-in decoy"
        );
    }
}
