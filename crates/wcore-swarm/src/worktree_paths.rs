//! The single worktree path-representation decision for `wcore-swarm`.
//!
//! Why the representation exists: on Windows `std::fs::canonicalize` returns a
//! verbatim `\\?\C:\...` path. A verbatim `swarm_root` silently breaks the
//! PowerShell capacity probe (`[IO.DriveInfo]::new("\\?\C:\")` throws a
//! NON-terminating `ArgumentException`, so the probe exits 0 with empty stdout)
//! and is unparseable by the MSYS layer behind git-for-Windows argv. So the
//! canonicalized root is rendered back to its plain drive-letter form.
//!
//! Why callers must never assume the result is plain: the de-verbatimizing
//! strip is CONDITIONAL. It is refused — and the verbatim form returned
//! unchanged — when any component exceeds 255 windows chars, when the path is
//! not valid UTF-8, when a component is a reserved DOS name, or when a
//! component ends in '.' or a space.
//!
//! The invariant this module enforces is therefore NOT "stored paths are
//! plain". It is: every path stored on a `WorktreeManager`, stored on a
//! `TransactionWorkspace`, opened as a `DirectoryAuthority`, or COMPARED
//! against one of those, is produced by exactly one call to the function below
//! on the real on-disk object. That function is deterministic per on-disk
//! object, so both operands of every comparison agree by construction whether
//! or not the strip actually happened.
//!
//! `Path` equality treats `Prefix::VerbatimDisk('C')` and `Prefix::Disk('C')`
//! as UNEQUAL, so a partial application of the representation splits the
//! comparison lattice in half on Windows and silently disables the guards that
//! depend on it. This is the only site in the crate allowed to name the
//! de-verbatimizing call.
//!
//! Off Windows the strip is a compile-time no-op, so unix behaviour is
//! byte-identical to a bare `std::fs::canonicalize`.

use std::path::{Path, PathBuf};

use crate::error::Result;

use super::security::io_at;

/// Canonicalize the real on-disk object named by `path` and render it in the
/// crate's one worktree path representation.
///
/// Deterministic per on-disk object: two calls for the same directory always
/// produce the identical `PathBuf`. The result is NOT guaranteed to be plain
/// (see the module docs) — correctness comes from both operands of a
/// comparison sharing this one derivation, never from an assumed prefix form.
pub(super) fn normalized_root(path: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| io_at("canonicalization of the worktree path", path, error))?;
    Ok(dunce::simplified(&canonical).to_path_buf())
}
