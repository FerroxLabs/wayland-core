//! Windows RELAXED containment backend — kill-on-close Job Object only.
//!
//! This is the Windows session default. It keeps the two containment
//! properties that measured well on Windows and drops the ones that did not:
//!
//! | property | this backend |
//! |---|---|
//! | process-tree ownership (kill-on-close Job Object, no breakaway) | **ACTIVE** |
//! | host environment scrubbed to `manifest.env` only | **ACTIVE** |
//! | AppContainer profile + Low-integrity restricted token | not active |
//! | `fs_read_allow` / `fs_write_allow` / `fs_read_deny` at the OS layer | not active |
//! | filesystem confinement (`confines_filesystem`) | **`false` — a child can write anywhere this user can** |
//! | `NetworkPolicy::Deny` at the OS layer | not active |
//! | Job Object memory / CPU / active-process caps | not active |
//!
//! **The point of this type is what it does NOT claim.** It keeps the trait
//! default [`SandboxBackend::enforces_read_deny`] (`false`), so the two
//! independent-looking consumers of that predicate both fire as designed: the
//! agent drops `Bash` from the `Workspace` channel posture, and the exec-time
//! gate in `wcore_tools::bash` refuses a shell whose workspace requires
//! secret-read-deny. The alternative implementation of a relaxed Windows
//! default — keeping `AppContainerBackend` and emptying `fs_read_deny` — cannot
//! move that predicate at all (it is derived from the availability probe, not
//! from the manifest), so it would hand a channel sender a shell in silence.
//! Relaxation MUST therefore be a backend swap; see `platform_candidate` in
//! the crate root.
//!
//! Execution is delegated to [`no_sandbox::NoSandboxBackend`], which is already
//! exactly "spawn under `ProcessTreeGuard` with a scrubbed environment" — the
//! mechanism this backend wants. The distinct type exists because the *identity*
//! differs: `no_sandbox` is the operator-requested containment BYPASS (and is
//! reported as such by `sandbox status`, by the swarm admission gate and by the
//! `assert_ne!(backend_name(), "no_sandbox")` session invariants), whereas this
//! backend is a real, non-bypass session default that simply enforces less.
//!
//! Compiled on every target — like `appcontainer`, `bwrap` and `sandbox_exec` —
//! so the Windows selection decision and this backend's capability claims are
//! unit-testable off Windows. Selection still reaches for it only on Windows.

use std::sync::Arc;

use async_trait::async_trait;

use super::{SandboxBackend, no_sandbox};
use crate::error::{Result, SandboxError};
use crate::manifest::{NetworkPolicy, SandboxManifest};
use crate::{SandboxChunk, SandboxCommand, SandboxOutput};

pub struct WindowsJobObjectBackend {
    inner: Arc<no_sandbox::NoSandboxBackend>,
}

impl WindowsJobObjectBackend {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(no_sandbox::NoSandboxBackend::new()),
        }
    }

    /// `AllowHosts` has no DNS gate here, exactly as on AppContainer. The
    /// manifest contract is that a backend without a gate returns
    /// `PolicyNotSupported` rather than silently downgrading, so this arm is
    /// refused instead of quietly running unfiltered.
    ///
    /// `NetworkPolicy::Deny` is NOT refused: it is the default policy for every
    /// agent-initiated shell, so refusing it would refuse every command. It is
    /// not enforced either — that is stated by the activation log line this
    /// backend's selection emits, and it is the honest cost of the relaxed
    /// default.
    fn refuse_unsupported_policy(manifest: &SandboxManifest) -> Result<()> {
        if matches!(manifest.network, NetworkPolicy::AllowHosts(_)) {
            return Err(SandboxError::PolicyNotSupported(
                "the Windows Job Object backend has no DNS-name allowlist; \
                 use NetworkPolicy::Deny or select the AppContainer backend"
                    .into(),
            ));
        }
        Ok(())
    }
}

impl Default for WindowsJobObjectBackend {
    fn default() -> Self {
        Self::new()
    }
}

// NOTE: this backend deliberately overrides NOTHING that would raise a
// containment claim. `confines_filesystem`, `enforces_read_deny`,
// `owns_descendants_hard`, `binds_cwd_authority`, `binds_workspace_authority`,
// `blocks_powershell` and `hard_containment_identity` all keep their trait
// defaults.
//
// `confines_filesystem == false` is not an oversight to be tidied up later: a
// Windows Job Object bounds process LIFETIME and RESOURCE USE. It has no
// filesystem filter and never had one, so this backend cannot confine a write
// no matter how the manifest is written. The measured proof is
// `wcore-cli/tests/sandbox_activeness.rs`, which runs the escape and asserts
// the claim against what actually happened.
//
// `owns_descendants_hard` is the one that is arguably understated: a
// kill-on-close Job Object with no breakaway really is a hard descendant
// boundary on Windows. It stays `false` because this type also compiles on
// Unix, where `ProcessTreeGuard` degrades to a process group an adversarial
// child can leave with `setsid` — and because nothing downstream needs it
// (swarm's delegated-dispatch gate refuses on `enforces_read_deny` first
// regardless). Understating a containment claim is the safe direction; a
// separate lane may raise it behind a `cfg(windows)` live probe.
/// The declared Windows containment posture, as product text (`#368`, `#369`).
///
/// Deliberately says what IS in force as well as what is not: a limitation
/// that lists only absences reads as "there is no sandbox at all", which is
/// its own inaccuracy and makes an operator discount the whole notice.
pub(crate) const WINDOWS_NO_FILESYSTEM_SANDBOX_LIMITATION: &str = "there is NO filesystem sandbox on this backend, by design and not by \
     accident: a Job Object bounds the process TREE, not the filesystem. Treat \
     every command run through it -- the agent's Bash tool included -- as \
     having the full authority of your user account, able to read and write \
     anywhere you can, regardless of the workspace policy. In force: \
     kill-on-close process-tree ownership and a scrubbed child environment. \
     NOT in force: OS filesystem confinement, OS network denial, OS secret \
     read-deny. `WAYLAND_SANDBOX=appcontainer` selects the strict backend, \
     which enforces them and has its own declared limitations.";

#[async_trait]
impl SandboxBackend for WindowsJobObjectBackend {
    fn name(&self) -> &'static str {
        "windows_job_object"
    }

    /// An ordinary `CreateProcess` under a Job Object needs no profile service,
    /// no AppContainer SID and no restricted token, so there is nothing to
    /// probe and nothing that can stall. This is what removes the 15 s guarded
    /// AppContainer probe from the Windows session-startup path.
    fn is_available(&self) -> bool {
        true
    }

    /// What the Windows DEFAULT backend does not do, said in the product.
    ///
    /// # Not a defect report -- a declaration
    ///
    /// Unlike the AppContainer entries, this one tracks nothing, because there
    /// is nothing to track: a recorded decision says Windows ships WITHOUT a
    /// filesystem sandbox and the Job Object default is intended, and that
    /// decision is closed. What was open is only whether the product SAYS so.
    ///
    /// It matters because the honest reading of the capability row is
    /// counter-intuitive in exactly the dangerous direction:
    /// `bypasses_containment false` looks like containment, and it means only
    /// that a real backend was selected. Measured on real Windows, a command
    /// under this backend wrote outside the workspace while that field read
    /// `false`. The `NOTE:` block in `wayland-core sandbox status` says this in
    /// prose for a human; this string is the same fact in the `--json` output,
    /// where a host integration or a script can act on it.
    fn known_limitations(&self) -> Vec<&'static str> {
        vec![WINDOWS_NO_FILESYSTEM_SANDBOX_LIMITATION]
    }

    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        Self::refuse_unsupported_policy(manifest)?;
        self.inner.execute(manifest, cmd).await
    }

    fn execute_streaming(
        self: Arc<Self>,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<tokio::sync::mpsc::Receiver<SandboxChunk>> {
        Self::refuse_unsupported_policy(manifest)?;
        Arc::clone(&self.inner).execute_streaming(manifest, cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE property this type exists for. If this ever becomes `true` the
    /// Windows relaxed default silently re-arms `Bash` under the `Workspace`
    /// channel posture and the exec-time gate in `wcore_tools::bash` stops
    /// refusing — the exact failure the Phase 0 gate measured on AppContainer.
    #[test]
    fn relaxed_backend_never_claims_read_deny_enforcement() {
        let backend = WindowsJobObjectBackend::new();
        assert!(
            !backend.enforces_read_deny(),
            "the relaxed Windows backend must not claim OS-level secret-read-deny"
        );
    }

    /// The other claims stay at their (false) defaults, so no delegated
    /// authority path can admit this backend by accident.
    #[test]
    fn relaxed_backend_claims_no_delegated_authority() {
        let backend = WindowsJobObjectBackend::new();
        assert!(
            !backend.confines_filesystem(),
            "a Job Object has no filesystem filter; claiming confinement here \
             would tell an operator a write cannot escape when it can"
        );
        assert!(!backend.owns_descendants_hard());
        assert!(!backend.binds_cwd_authority());
        assert!(!backend.binds_workspace_authority());
        assert!(backend.hard_containment_identity().is_none());
        assert!(
            !backend.blocks_powershell(),
            "without the Low-integrity token PowerShell loads fine"
        );
    }

    /// The default backend must SAY it does not confine the filesystem, not
    /// merely answer `false` to a question the reader has to know to ask.
    ///
    /// This is the honesty half of `#368`/`#369` for the backend that is
    /// actually selected on Windows. It grades the two properties that make
    /// the notice load-bearing: it must deny filesystem confinement in words,
    /// and it must correct the specific misreading that
    /// `bypasses_containment false` implies containment -- by naming what IS
    /// enforced, so the reader can tell the notice apart from "no sandbox at
    /// all" and does not discount it.
    ///
    /// The prose is not pinned verbatim; an honest rewording must not redden.
    #[test]
    fn the_relaxed_backend_declares_that_it_does_not_confine_the_filesystem() {
        let backend = WindowsJobObjectBackend::new();
        let declared = backend.known_limitations();
        assert!(
            !declared.is_empty(),
            "a backend whose `confines_filesystem` is false must say so in \
             words; the boolean alone is what let a measured write escape \
             while the status row read reassuringly"
        );
        let joined = declared.join(" ");
        assert!(
            joined.contains("NO filesystem sandbox"),
            "the declaration must deny filesystem confinement outright: \
             {joined:?}"
        );
        assert!(
            joined.contains("full authority of your user account"),
            "the declaration must state the CONSEQUENCE, not just the absent \
             mechanism: {joined:?}"
        );
        assert!(
            joined.contains("In force:"),
            "a notice that lists only absences reads as 'no sandbox at all' \
             and gets discounted; it must name what IS enforced: {joined:?}"
        );
        assert!(
            !backend.confines_filesystem(),
            "the declaration and the boolean must agree, or one of them is \
             lying and the reader cannot tell which"
        );
    }

    /// The trait DEFAULT must stay silent. A backend with nothing recorded has
    /// to say nothing rather than reassure anybody, or `known_limitations`
    /// becomes a place where an empty list reads as a clean bill of health.
    #[test]
    fn a_backend_that_records_nothing_declares_nothing() {
        struct Bare;
        #[async_trait]
        impl SandboxBackend for Bare {
            fn name(&self) -> &'static str {
                "bare"
            }
            fn is_available(&self) -> bool {
                true
            }
            async fn execute(
                &self,
                _manifest: &SandboxManifest,
                _cmd: crate::SandboxCommand,
            ) -> Result<crate::SandboxOutput> {
                unreachable!("never executed")
            }
        }
        assert!(Bare.known_limitations().is_empty());
        assert!(Bare.unavailable_reason().is_none());
    }

    /// Selection must never spend a startup probe on this backend, and must
    /// never drop it: it is the Windows default and it is always usable.
    #[test]
    fn relaxed_backend_is_startup_safe_and_available() {
        let backend = WindowsJobObjectBackend::new();
        assert!(backend.availability_probe_is_startup_safe());
        assert!(backend.is_available());
    }

    #[tokio::test]
    async fn allow_hosts_is_refused_rather_than_silently_unfiltered() {
        let backend = WindowsJobObjectBackend::new();
        let manifest = SandboxManifest {
            network: NetworkPolicy::AllowHosts(vec!["example.com".into()]),
            ..Default::default()
        };
        let error = backend
            .execute(
                &manifest,
                SandboxCommand {
                    argv: vec!["whatever".into()],
                    cwd: None,
                },
            )
            .await
            .expect_err("a policy with no gate must be refused, not downgraded");
        assert!(matches!(error, SandboxError::PolicyNotSupported(_)));

        let streaming = Arc::new(WindowsJobObjectBackend::new()).execute_streaming(
            &manifest,
            SandboxCommand {
                argv: vec!["whatever".into()],
                cwd: None,
            },
        );
        assert!(
            matches!(streaming, Err(SandboxError::PolicyNotSupported(_))),
            "the streaming path must refuse the same policy as the buffered one"
        );
    }

    /// The delegation is real: a benign command runs through the Job Object /
    /// process-group path and returns its output.
    #[tokio::test]
    async fn benign_command_executes_through_the_delegated_pipeline() {
        let argv = if cfg!(windows) {
            let system_root = match std::env::var_os("SYSTEMROOT") {
                Some(root) => root,
                None => return,
            };
            let cmd = std::path::PathBuf::from(system_root)
                .join("System32")
                .join("cmd.exe");
            vec![
                cmd.display().to_string(),
                "/d".into(),
                "/c".into(),
                "exit 0".into(),
            ]
        } else {
            let Some(true_bin) = ["/bin/true", "/usr/bin/true"]
                .into_iter()
                .find(|p| std::path::Path::new(p).exists())
            else {
                return;
            };
            vec![true_bin.to_owned()]
        };
        let output = WindowsJobObjectBackend::new()
            .execute(
                &SandboxManifest::default(),
                SandboxCommand { argv, cwd: None },
            )
            .await
            .expect("a benign command must run");
        assert_eq!(output.exit_code, 0);
    }
}
