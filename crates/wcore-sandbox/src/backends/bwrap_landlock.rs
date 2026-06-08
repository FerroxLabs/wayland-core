//! S3 — Landlock LSM ruleset construction for the bwrap backend.
//!
//! Build a `Ruleset` from the manifest's `fs_read_allow` and
//! `fs_write_allow` lists, then apply it via `restrict_self()` inside a
//! `Command::pre_exec` closure. Landlock rules set with `restrict_self`
//! propagate across `execve(2)`, so the bwrap child inherits them and
//! cannot escape them with a subsequent exec.
//!
//! bwrap already sets `PR_SET_NO_NEW_PRIVS` before its own work, which
//! Landlock requires; we do NOT re-set it here. (Out-of-tree unit tests
//! that exercise this helper without going through bwrap MUST set
//! `PR_SET_NO_NEW_PRIVS` themselves first — see audit B L3.)
//!
//! Compiled ONLY when the `landlock` cargo feature is enabled AND target
//! is Linux. Failure to load the Landlock kernel ABI is NOT fatal: callers
//! warn-once and continue with bwrap-only sandbox.

#![cfg(all(target_os = "linux", feature = "landlock"))]

use crate::error::{Result, SandboxError};
use landlock::{
    ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetError, RulesetStatus,
};
use std::path::Path;

/// Outcome of applying a Landlock ruleset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandlockOutcome {
    /// Ruleset fully enforced by the kernel.
    Enforced,
    /// Ruleset partially enforced (kernel supports an older ABI than we
    /// asked for). Still strictly tighter than no Landlock.
    PartiallyEnforced,
    /// Kernel does not support Landlock at all (pre-5.13 or LSM disabled).
    /// Caller should warn-once and continue with bwrap-only.
    Unsupported,
}

/// Build a `Ruleset` from the manifest and call `restrict_self()` on the
/// current process. MUST be invoked from a `pre_exec` closure (i.e. inside
/// the child after `fork()` but before `execve()`).
///
/// Returns `Ok(Enforced)` / `Ok(PartiallyEnforced)` on success, or
/// `Ok(Unsupported)` if the kernel has no Landlock support. Errors are
/// reserved for hard failures (paths that exist on the manifest but can't
/// be opened, kernel returning EINVAL with rules, etc.).
pub fn restrict_self_from_paths(
    fs_read_allow: &[std::path::PathBuf],
    fs_write_allow: &[std::path::PathBuf],
) -> Result<LandlockOutcome> {
    // No paths declared → no point building a ruleset. Caller treats this
    // as "Landlock not engaged for this manifest" and proceeds with bwrap.
    if fs_read_allow.is_empty() && fs_write_allow.is_empty() {
        return Ok(LandlockOutcome::Unsupported);
    }

    // ABI::V2 = Linux 5.19+ (adds LANDLOCK_ACCESS_FS_REFER). We ask for V2
    // and use `best_effort` semantics: if the running kernel is older we
    // degrade to the supported subset rather than erroring out.
    let abi = ABI::V2;
    let access_read = AccessFs::from_read(abi);
    let access_all = AccessFs::from_all(abi);

    // Build read rules. PathFd::new opens the path read-only with
    // O_PATH|O_CLOEXEC, which we use just to obtain a stable dirfd; the
    // kernel uses it as the rule anchor.
    let mut read_rules = Vec::with_capacity(fs_read_allow.len());
    for p in fs_read_allow {
        let fd = path_fd_for(p)?;
        read_rules.push(PathBeneath::new(fd, access_read));
    }

    let mut write_rules = Vec::with_capacity(fs_write_allow.len());
    for p in fs_write_allow {
        let fd = path_fd_for(p)?;
        // Writable paths also need read access — otherwise reading a file
        // before overwriting it would deny.
        write_rules.push(PathBeneath::new(fd, access_all));
    }

    let ruleset = Ruleset::default()
        .handle_access(access_all)
        .map_err(|e| SandboxError::ExecFailed(format!("landlock: handle_access: {e}")))?
        .create()
        .map_err(|e| SandboxError::ExecFailed(format!("landlock: ruleset create: {e}")))?
        // `add_rules` is generic over the iterator item's error type `E`
        // (bound `E: From<RulesetError>`). Our rules are infallible — built
        // locally and never `Err` — but `.map(Ok)` leaves `E` unconstrained,
        // which the compiler cannot resolve (E0283). Pin `E = RulesetError`
        // explicitly: `RulesetError: From<RulesetError>` satisfies the bound,
        // and the returned `Result<_, RulesetError>` flows straight into the
        // existing `.map_err(...)` (which only needs `Display`).
        .add_rules(read_rules.into_iter().map(Ok::<_, RulesetError>))
        .map_err(|e| SandboxError::ExecFailed(format!("landlock: add read rules: {e}")))?
        .add_rules(write_rules.into_iter().map(Ok::<_, RulesetError>))
        .map_err(|e| SandboxError::ExecFailed(format!("landlock: add write rules: {e}")))?;

    let status = ruleset
        .restrict_self()
        .map_err(|e| SandboxError::ExecFailed(format!("landlock: restrict_self: {e}")))?;

    Ok(match status.ruleset {
        RulesetStatus::FullyEnforced => LandlockOutcome::Enforced,
        RulesetStatus::PartiallyEnforced => LandlockOutcome::PartiallyEnforced,
        RulesetStatus::NotEnforced => LandlockOutcome::Unsupported,
    })
}

fn path_fd_for(p: &Path) -> Result<PathFd> {
    PathFd::new(p).map_err(|e| {
        SandboxError::PathDenied(format!(
            "landlock: cannot open path {} for ruleset: {e}",
            p.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn empty_lists_return_unsupported() {
        let out = restrict_self_from_paths(&[], &[]).expect("ok");
        assert_eq!(out, LandlockOutcome::Unsupported);
    }

    #[test]
    fn rejects_nonexistent_path() {
        // Cannot actually call restrict_self() outside a child (it would
        // restrict the test process for the rest of the suite). We only
        // assert the path-open error path here, which fires before the
        // kernel call.
        let res = restrict_self_from_paths(
            &[PathBuf::from(
                "/this/path/does/not/exist/wcore-sandbox-test",
            )],
            &[],
        );
        assert!(
            matches!(res, Err(SandboxError::PathDenied(_))),
            "expected PathDenied, got {res:?}"
        );
    }
}
