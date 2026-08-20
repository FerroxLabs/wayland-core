//! Pre-flight detection of a tool call that names a path outside every root
//! this session can reach (#1099).
//!
//! Before this existed, a read outside the workspace ran, hit
//! `VfsError::OutsideSandbox`, came back as a tool error, and the model had to
//! improvise an explanation — in the UAT that produced this work it told the
//! user to paste the path into their browser. The remedy already existed
//! (`ApprovalScope::AlwaysPath`), but nothing ever asked for one.
//!
//! This module answers one question, in front of the call rather than behind
//! it: *would this read be refused for being outside the sandbox, and is there
//! a folder grant that would fix it?* If yes, the orchestrator forces the
//! approval gate and hands the host the folder to offer.
//!
//! It is an ASK-list, not a security boundary. A tool missing from the table
//! below simply does not prompt, exactly as today; containment is still
//! enforced by `SandboxedFs`, which this module never widens.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::workspace_policy::{WorkspacePolicy, WorkspaceTrust, canon_for_scope};

/// Tools whose named path is a pure READ, and the input key that carries it.
///
/// Verified against each tool's own `input_schema()`; the
/// `path_arg_table_matches_each_tools_schema` test fails if either side drifts.
///
/// WRITE tools are deliberately absent. Write authority outside the workspace
/// is not grantable (`PathGrantError::WriteNotGrantable`), so a boundary card
/// for `Write`/`Edit` could only ever offer a button that refuses itself.
pub const READ_PATH_ARGS: &[(&str, &str)] =
    &[("Read", "file_path"), ("Grep", "path"), ("Glob", "path")];

/// A tool call that names a readable path this session cannot reach, together
/// with the folder grant that would make it reachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBoundary {
    /// The path the call named, canonicalized.
    pub target: PathBuf,
    /// The CONTAINING DIRECTORY of `target` — what a grant actually opens.
    pub suggested_root: PathBuf,
}

/// Classify one tool call against the session's policy.
///
/// `Some` only when ALL of the following hold, because every one of them is a
/// way the prompt could otherwise be useless or actively misleading:
///
/// * the policy is `Contained` — the only posture in which a jail is installed,
///   so the only posture in which an out-of-workspace read fails at all. A
///   `trusted_local` session reads outside the workspace successfully today and
///   must not start being interrupted for it.
/// * the tool is a known read tool and the path key is present as a string.
/// * the target is outside the jail root AND outside every live grant.
/// * the target is not itself a secret. A grant widens WHERE the agent may
///   look, never WHAT — `SecretDenyFs` would still refuse it after the grant,
///   so offering the folder would be offering a fix that does not fix it.
/// * a grant on the containing folder would actually be accepted
///   (`grantable_read_root`). This is what keeps "always allow this folder"
///   from being a button that lies.
pub fn read_path_boundary(
    policy: &WorkspacePolicy,
    tool: &str,
    input: &Value,
) -> Option<PathBoundary> {
    if policy.trust() != WorkspaceTrust::Contained {
        return None;
    }
    let (_, key) = READ_PATH_ARGS.iter().find(|(name, _)| *name == tool)?;
    let raw = input.get(*key)?.as_str()?;
    if raw.is_empty() {
        return None;
    }

    let requested = Path::new(raw);
    // A relative path is resolved against the workspace root, which is what
    // the tools themselves do. Skipping this would classify `src/lib.rs` as
    // outside the sandbox and prompt for the workspace's own files.
    let resolved = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        policy.root().join(requested)
    };
    let target = canon_for_scope(&resolved);

    if policy.is_read_reachable(&target) {
        return None;
    }
    if crate::workspace_policy::is_secret_path_static(&target) {
        return None;
    }

    // Derive the containing folder BEFORE the grant dry-run, so a target that
    // does not exist yet still resolves to a real directory that
    // `canonicalize` inside the dry-run can accept.
    let candidate = if target.is_dir() {
        target.clone()
    } else {
        target.parent()?.to_path_buf()
    };
    let suggested_root = policy.grantable_read_root(&candidate, false).ok()?;

    // The host receives these as JSON strings. A lossy conversion would echo
    // back a root that does not exist, and the grant would then be refused for
    // a path the user never named.
    if target.to_str().is_none() || suggested_root.to_str().is_none() {
        return None;
    }

    Some(PathBoundary {
        target,
        suggested_root,
    })
}
