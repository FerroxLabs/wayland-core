//! FerroxLabs/wayland-core#244 c3 — the VCS content store must be unreachable
//! to a SHELL SUBPROCESS, not only to the in-process VFS.
//!
//! `SecretDenyFs::guard` refuses `.git/objects/**` to Read/Write/Edit, and
//! `tests/vfs_vcs_content_store_deny.rs` grades that. It grades the FUNCTION.
//! Nothing graded the WIRING on the other side of the boundary: whether the
//! paths `WorkspacePolicy::secret_deny_paths_for_backend` emits actually reach
//! an OS backend that refuses them to `Bash`. A predicate that answers
//! correctly and a manifest field that is never enforced look identical from
//! inside the process, and #244's own residual note said as much.
//!
//! So this test asks the subprocess. It runs the real `BashTool` against the
//! live platform backend (bubblewrap on this host) and requires that the bytes
//! of a committed object never appear in the tool result — for the ROOT store
//! and for the NESTED one #322 added, because those two are discovered by
//! different code paths (`vcs_content_stores`'s root-relative join versus
//! `vcs_store_entry` inside the walk) and only one of them was ever exercised.
//!
//! Three controls, all load-bearing:
//!
//! * `readme.txt` reads back — the sandbox is not refusing everything, which
//!   is the way a containment assertion passes while proving nothing.
//! * `.git/HEAD` reads back — the deny is scoped to the CONTENT store. A guard
//!   that swallowed the whole of `.git` would satisfy the denial assertions and
//!   break `git status` for every session.
//! * the marker strings are distinct per store, so a result carrying one cannot
//!   be mistaken for the other.
//!
//! Linux-only and skipped when the host has no read-deny-enforcing backend: on
//! such a host NOTHING in this process can make a file unreadable to a child,
//! and a test that passed there would be measuring its own skip.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

const ROOT_OBJECT: &str = "ROOT-OBJECT-BYTES-244";
const NESTED_OBJECT: &str = "NESTED-OBJECT-BYTES-322";
const PLAIN: &str = "ordinary working-tree contents";

/// A workspace holding a root repository and a vendored nested one, each with
/// a loose object carrying its own marker.
fn workspace() -> Option<(ToolContext, PathBuf, tempfile::TempDir)> {
    let dir = tempfile::tempdir().ok()?;
    let root = std::fs::canonicalize(dir.path()).ok()?;

    std::fs::write(root.join("readme.txt"), PLAIN.as_bytes()).ok()?;

    let git = root.join(".git");
    std::fs::create_dir_all(git.join("objects/ab")).ok()?;
    std::fs::write(git.join("HEAD"), b"ref: refs/heads/main\n").ok()?;
    std::fs::write(git.join("objects/ab/cdef"), ROOT_OBJECT.as_bytes()).ok()?;

    let nested = root.join("vendor/pkg/.git");
    std::fs::create_dir_all(nested.join("objects/12")).ok()?;
    std::fs::write(nested.join("objects/12/3456"), NESTED_OBJECT.as_bytes()).ok()?;

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    Some((
        ToolContext::test_default().with_workspace(policy),
        root,
        dir,
    ))
}

async fn run(ctx: &ToolContext, command: &str) -> String {
    BashTool
        .execute_with_ctx(json!({ "command": command }), ctx)
        .await
        .content
}

#[tokio::test]
async fn a_shell_subprocess_cannot_read_the_vcs_content_store() {
    if !wcore_tools::bash::platform_enforces_read_deny() {
        eprintln!("skip: this host has no read-deny-enforcing sandbox backend");
        return;
    }
    let Some((ctx, _root, _keep)) = workspace() else {
        eprintln!("skip: could not build a contained workspace");
        return;
    };

    // POSITIVE CONTROL first. Without it every assertion below is satisfied by
    // a sandbox that refuses the whole filesystem.
    let plain = run(&ctx, "cat readme.txt").await;
    assert!(
        plain.contains(PLAIN),
        "control failed: an ordinary working-tree read must succeed, got: {plain}"
    );

    // WRONG-REFUSAL CONTROL. The deny covers the CONTENT store only; ordinary
    // repository metadata is how a session answers "what branch am I on".
    let head = run(&ctx, "cat .git/HEAD").await;
    assert!(
        head.contains("refs/heads/main"),
        "control failed: .git/HEAD must stay readable to the shell, got: {head}"
    );

    let root_store = run(&ctx, "cat .git/objects/ab/cdef").await;
    assert!(
        !root_store.contains(ROOT_OBJECT),
        "the root object store's bytes reached the shell: {root_store}"
    );

    // #322's arm: discovered by the walk, not by the root-relative join.
    let nested_store = run(&ctx, "cat vendor/pkg/.git/objects/12/3456").await;
    assert!(
        !nested_store.contains(NESTED_OBJECT),
        "the nested object store's bytes reached the shell: {nested_store}"
    );
}

/// The store must not be readable by ENUMERATION either. `cat` names one file;
/// `grep -r` and `find -exec cat` need no name at all, and a deny that only
/// masks a named path would let the second shape walk straight in.
#[tokio::test]
async fn a_recursive_shell_read_cannot_harvest_the_object_store() {
    if !wcore_tools::bash::platform_enforces_read_deny() {
        eprintln!("skip: this host has no read-deny-enforcing sandbox backend");
        return;
    }
    let Some((ctx, _root, _keep)) = workspace() else {
        eprintln!("skip: could not build a contained workspace");
        return;
    };

    // POSITIVE CONTROL: the same recursive shape finds the working-tree file,
    // so a clean result below means the store was masked and not that the
    // command failed to run.
    let sweep = run(&ctx, "grep -r ordinary . 2>/dev/null; echo done").await;
    assert!(
        sweep.contains("ordinary"),
        "control failed: a recursive read must find the working-tree file, got: {sweep}"
    );

    let harvest = run(&ctx, "grep -rh OBJECT-BYTES . 2>/dev/null; echo done").await;
    assert!(
        !harvest.contains(ROOT_OBJECT) && !harvest.contains(NESTED_OBJECT),
        "a recursive shell read harvested the object store: {harvest}"
    );
}
