//! FerroxLabs/wayland-core#375 — **GrepTool must not return the plaintext of a
//! VCS content store, whatever name the search path reaches it under.**
//!
//! `GrepTool::execute_with_ctx` gates only the top-level `path` argument
//! through `ctx.vfs.exists()`, then spawns `rg`/`grep` with
//! `shell_command_argv` — outside `ctx.vfs` AND outside BashTool's OS sandbox,
//! so neither `SecretDenyFs` nor the bwrap `fs_read_deny` sees the traversal.
//! `is_vcs_content_store` matches the STORE, not the control directory that
//! owns it, so `.git` and `.svn` passed the gate and the backend then descended
//! into `.git/lfs/objects/**` and `.svn/pristine/**`.
//!
//! This is strictly worse than #244, which was filed INFO-only because git
//! loose objects are zlib-compressed. `.svn/pristine` and `.git/lfs/objects`
//! store file content VERBATIM, so the parent-named spelling returned a
//! committed secret's plaintext to the model in the CONTAINED posture — the
//! posture whose whole premise is that a committed secret cannot be
//! reconstructed from the object store.
//!
//! **The stack under test is the production contained one** —
//! `SandboxedFs::new(SecretDenyFs::new(RealFs, WorkspacePolicy::contained(root)), root)`
//! with the same policy on `ToolContext::workspace`, which is how all three
//! production installation sites wire it (`bootstrap.rs:3348`,
//! `spawner.rs:3023`, `channel_tools.rs:185`, each followed by
//! `set_workspace_policy`).
//!
//! **Red arm:** delete the `.filter_entry(...)` block from
//! `grep_policy::scope_for` in `crates/wcore-tools/src/grep_policy.rs`. The two
//! parent-named tests must go red; the controls must stay green.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::grep::GrepTool;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

/// A workspace holding both uncompressed stores, an ordinary file that also
/// matches the pattern, and the `.git` metadata that must stay searchable.
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("workspace");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");

    std::fs::create_dir_all(root.join(".svn/pristine/aa")).unwrap();
    std::fs::write(
        root.join(".svn/pristine/aa/deadbeef.svn-base"),
        b"SVN-CANARY-244 AWS_SECRET_ACCESS_KEY=abc123\n",
    )
    .unwrap();

    std::fs::create_dir_all(root.join(".git/lfs/objects/aa/bb")).unwrap();
    std::fs::write(
        root.join(".git/lfs/objects/aa/bb/deadbeef"),
        b"LFS-CANARY-244 password=hunter2\n",
    )
    .unwrap();

    // Metadata that is NOT a content store and must stay readable — the
    // `git rev-parse` carve-out `vcs_content_stores` documents.
    std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
    std::fs::write(root.join(".git/HEAD"), b"ref: refs/heads/CANARY-branch\n").unwrap();

    // An ordinary working-tree file carrying the same pattern, so "no bytes
    // returned" cannot pass because the search found nothing at all.
    std::fs::write(root.join("notes.txt"), b"ORDINARY-CANARY-244 hello\n").unwrap();

    (dir, root)
}

/// The production contained stack, wired the way the three installation sites
/// wire it.
fn contained_ctx(root: &Path) -> ToolContext {
    let policy = Arc::new(WorkspacePolicy::contained(root));
    let jail = SandboxedFs::new(
        SecretDenyFs::new(RealFs, Arc::clone(&policy)),
        root.to_path_buf(),
    );
    let mut ctx = ToolContext::test_default();
    ctx.vfs = Arc::new(jail);
    ctx.with_workspace(policy)
}

async fn grep(ctx: &ToolContext, path: &str) -> String {
    GrepTool
        .execute_with_ctx(json!({"pattern": "CANARY", "path": path}), ctx)
        .await
        .content
}

/// c1, `.svn` half. The store's CONTROL directory is named; the pristine text
/// base underneath it is verbatim plaintext.
#[tokio::test]
async fn naming_the_svn_control_dir_returns_no_pristine_bytes() {
    let (_dir, root) = fixture();
    let ctx = contained_ctx(&root);

    let out = grep(&ctx, ".svn").await;
    assert!(
        !out.contains("SVN-CANARY-244"),
        "Grep(path=\".svn\") returned the pristine text base verbatim:\n{out}"
    );
    assert!(
        !out.contains("AWS_SECRET_ACCESS_KEY"),
        "Grep(path=\".svn\") returned committed credential plaintext:\n{out}"
    );
}

/// c1, `.git` half. `.git/lfs/objects` is uncompressed, unlike the loose
/// objects #244 was filed against.
#[tokio::test]
async fn naming_the_git_control_dir_returns_no_lfs_object_bytes() {
    let (_dir, root) = fixture();
    let ctx = contained_ctx(&root);

    let out = grep(&ctx, ".git").await;
    assert!(
        !out.contains("LFS-CANARY-244"),
        "Grep(path=\".git\") returned LFS object bytes verbatim:\n{out}"
    );
    assert!(
        !out.contains("hunter2"),
        "Grep(path=\".git\") returned committed credential plaintext:\n{out}"
    );
    // Not silent: the withholding is reported (grep_policy rule 6). Without
    // this the two assertions above would also pass for a Grep that refused to
    // run at all, and the model would be told "nothing here".
    assert!(
        out.contains("[Grep policy:") || out.contains("Refused to search"),
        "the withholding must be reported, never rendered as an empty result:\n{out}"
    );
}

/// c4, control 1 — `.git/HEAD` is not a content store and stays searchable, so
/// the fix is a store prune and not a blanket `.git` refusal.
#[tokio::test]
async fn git_metadata_outside_the_store_stays_searchable() {
    let (_dir, root) = fixture();
    let ctx = contained_ctx(&root);

    let out = grep(&ctx, ".git").await;
    assert!(
        out.contains("CANARY-branch"),
        "`.git/HEAD` must stay searchable — the deny covers CONTENT stores, not \
         the whole control directory:\n{out}"
    );
}

/// c4, control 2 — an ordinary in-workspace search is unchanged, and a
/// whole-workspace search still WITHHOLDS the ignored matches rather than
/// returning or silently dropping them.
#[tokio::test]
async fn an_ordinary_search_is_unchanged_and_the_dot_search_still_withholds() {
    let (_dir, root) = fixture();
    let ctx = contained_ctx(&root);

    let out = grep(&ctx, ".").await;
    assert!(
        out.contains("ORDINARY-CANARY-244"),
        "an ordinary working-tree match must still be returned:\n{out}"
    );
    assert!(
        !out.contains("SVN-CANARY-244") && !out.contains("LFS-CANARY-244"),
        "Grep(path=\".\") must not return store bytes:\n{out}"
    );
    assert!(
        out.contains("ignored paths"),
        "Grep(path=\".\") must still REPORT what it withheld:\n{out}"
    );
}

/// c4, control 3 — naming the store directly is still refused with a reason,
/// not rendered as "No matches found". This arm was already green before #375;
/// it is carried here so a regression that made the direct spelling silent
/// fails somewhere.
#[tokio::test]
async fn naming_the_store_directly_is_refused_with_a_reason() {
    let (_dir, root) = fixture();
    let ctx = contained_ctx(&root);

    for named in [".svn/pristine", ".git/lfs"] {
        let out = grep(&ctx, named).await;
        assert!(
            out.contains("Refused to search") || out.contains("protected secret path"),
            "Grep(path={named:?}) must be refused with a reason, got:\n{out}"
        );
        assert!(
            !out.contains("CANARY-244"),
            "Grep(path={named:?}) leaked store bytes:\n{out}"
        );
    }
}

/// The predicate the traversal filter asks must be the one `SecretDenyFs` asks.
///
/// Without this, `scope_for` could grow its own name list and every test above
/// would still pass — which is the exact shape #375 calls out one level up
/// ("graded the function, not the wiring"). A store reached under a name no
/// list would guess is the observable that separates the two: here `.git` is a
/// SYMLINK to a real git directory elsewhere in the tree, so the store's
/// canonical path is `<root>/real-git/objects` and no `.git`-shaped name list
/// can see it.
#[tokio::test]
#[cfg(unix)]
async fn a_store_reached_under_an_unguessable_name_is_still_pruned() {
    let dir = tempfile::tempdir().expect("workspace");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    std::fs::create_dir_all(root.join("real-git/objects/ab")).unwrap();
    std::fs::write(
        root.join("real-git/objects/ab/cdef"),
        b"ALIAS-CANARY-244 secret\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(root.join("real-git"), root.join(".git")).unwrap();
    std::fs::write(root.join("notes.txt"), b"ORDINARY-CANARY-244 hello\n").unwrap();

    let ctx = contained_ctx(&root);
    // Named through the alias its own directory carries, which is neither
    // `.git/objects` nor anything a lexical list in `grep_policy` would hold.
    let out = grep(&ctx, "real-git").await;
    assert!(
        !out.contains("ALIAS-CANARY-244"),
        "a content store reached under a non-`.git` name was searched — the \
         traversal is using a name list, not the VFS deny predicate:\n{out}"
    );

    // CONTROL: the same session's ordinary file is still searchable, so this is
    // not passing because the whole workspace became unsearchable.
    let out = grep(&ctx, ".").await;
    assert!(
        out.contains("ORDINARY-CANARY-244"),
        "CONTROL: ordinary working-tree matches must survive:\n{out}"
    );
}

/// The refusal is POSTURE-INDEPENDENT, and deliberately so.
///
/// Grep's secret-file rule (rule 3) has never been gated on trust: naming a
/// `.pem` is refused in every profile, because Grep returns matched line
/// CONTENT rather than a path. The store half is the same question about the
/// same bytes, so it is answered the same way. That matters here because the
/// trusted-local profile installs no `SecretDenyFs` at all
/// (`bootstrap.rs`: `RepoControlDenyFs` over a bare `RealFs`), so the
/// `ctx.vfs.exists()` probe in `execute_with_ctx` refuses nothing — this is the
/// only layer standing between the model and `.git/lfs/objects`.
///
/// Being STRICTER than the session's VFS cannot open a boundary; it can only
/// withhold. That is the direction a divergence is allowed to fall in.
#[tokio::test]
async fn a_session_without_a_secret_deny_vfs_still_refuses_a_store() {
    let (_dir, root) = fixture();
    let policy = Arc::new(WorkspacePolicy::trusted_local(&root));
    let mut ctx = ToolContext::test_default();
    ctx.vfs = Arc::new(RealFs);
    let ctx = ctx.with_workspace(policy);

    // Absolute paths throughout: an unconfined `RealFs` has no root, so
    // `run_grep` resolves a relative target against the PROCESS cwd (F36),
    // which in a test binary is not the workspace.
    // Named outright: refused here, because nothing upstream will.
    let out = grep(&ctx, root.join(".git/lfs").to_str().unwrap()).await;
    assert!(
        out.contains("Refused to search"),
        "a named content store must be refused even with no SecretDenyFs in the \
         stack, got:\n{out}"
    );

    // Named one component up: pruned from the traversal.
    let out = grep(&ctx, root.join(".git").to_str().unwrap()).await;
    assert!(
        !out.contains("LFS-CANARY-244"),
        "the parent-named spelling leaked LFS bytes with no SecretDenyFs in the \
         stack:\n{out}"
    );

    // CONTROL: the profile is otherwise unchanged.
    let out = grep(&ctx, root.to_str().unwrap()).await;
    assert!(
        out.contains("ORDINARY-CANARY-244"),
        "CONTROL: an ordinary search must be unaffected:\n{out}"
    );
}
