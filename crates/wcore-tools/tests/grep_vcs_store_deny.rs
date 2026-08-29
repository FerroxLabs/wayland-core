//! core#244 c3 — **"The store is also unreachable to a shell subprocess"**, for
//! every subprocess this product spawns, not only the sandboxed `Bash` one.
//!
//! `bash_vcs_store_deny_linux.rs` grades the OS-sandbox half: `BashTool` runs
//! under a read-deny backend that shadows the VCS content stores, so `cat
//! .git/objects/ab/cdef` cannot reach them. That test is real and its red arm
//! resolves. It is also the ONLY subprocess it covers.
//!
//! `GrepTool` spawns `rg` / `grep` / `findstr` itself, through
//! `shell_command_argv`, OUTSIDE the OS sandbox — the deny list the sandbox
//! backend consumes is never applied to it. Its only path gate is
//! `ctx.vfs.exists()` on the TOP-LEVEL `path` argument, and `grep_policy` then
//! filters output with `is_secret_path_static`, which matches secret NAMES. A
//! loose object is named after its hash, so nothing in that chain sees it.
//!
//! MEASURED at integ/f13 a278f8c3b before the fix: naming the directory ONE
//! COMPONENT ABOVE the store walks straight in — `Grep(pattern, path=".git")`
//! returned `.git/lfs/objects/aa/bb/deadbeef:1:<canary>` and
//! `Grep(pattern, path=".svn")` returned
//! `.svn/pristine/aa/deadbeef.svn-base:1:<canary>`, in PLAINTEXT, in the exact
//! `WorkspacePolicy::contained` posture c3 claims closes the store.
//!
//! Every refusal below is paired with a wrong-refusal control that must still
//! come back, so a Grep that broke entirely cannot pass this file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::grep::GrepTool;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

/// One canary per store, so a failure names WHICH store leaked.
const ROOT_OBJ: &str = "WLCANARY-ROOTOBJ-244";
const LFS_OBJ: &str = "WLCANARY-LFSOBJ-244";
const SVN_OBJ: &str = "WLCANARY-SVNOBJ-244";
const NESTED_OBJ: &str = "WLCANARY-NESTED-322";
/// In an ORDINARY file. Must survive every filter.
const CONTROL: &str = "WLCANARY-CONTROL-OK";
/// In `.git/HEAD` — repository METADATA, which `vcs_content_stores`
/// deliberately leaves readable so `git rev-parse` still works. A deny that
/// swallowed this would break ordinary session work.
const HEAD_CONTROL: &str = "WLCANARY-HEAD-OK";

/// Everything matches this, so one search reaches every canary at once.
const PATTERN: &str = "WLCANARY";

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();

    // Root git repo: loose object + LFS object, both PLAINTEXT here so the
    // assertion is about reachability, not about zlib.
    std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
    std::fs::write(root.join(".git/objects/ab/cd1234"), format!("{ROOT_OBJ}\n")).unwrap();
    std::fs::create_dir_all(root.join(".git/lfs/objects/aa/bb")).unwrap();
    std::fs::write(
        root.join(".git/lfs/objects/aa/bb/deadbeef"),
        format!("{LFS_OBJ}\n"),
    )
    .unwrap();
    // Metadata that must stay readable.
    std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
    std::fs::write(
        root.join(".git/HEAD"),
        format!("ref: refs/heads/{HEAD_CONTROL}\n"),
    )
    .unwrap();

    // Subversion pristine text base — #243's half of the same mechanism.
    std::fs::create_dir_all(root.join(".svn/pristine/aa")).unwrap();
    std::fs::write(
        root.join(".svn/pristine/aa/deadbeef.svn-base"),
        format!("{SVN_OBJ}\n"),
    )
    .unwrap();

    // #322 — a vendored checkout root-relative discovery never sees.
    std::fs::create_dir_all(root.join("vendor/x/.git/objects/12")).unwrap();
    std::fs::write(
        root.join("vendor/x/.git/objects/12/3456"),
        format!("{NESTED_OBJ}\n"),
    )
    .unwrap();

    std::fs::write(root.join("notes.txt"), format!("plain line {CONTROL}\n")).unwrap();
    (dir, root)
}

/// The production stack the dispatcher installs in `Contained` mode.
fn contained_ctx(root: &Path) -> ToolContext {
    let policy = Arc::new(WorkspacePolicy::contained(root));
    let vfs = SandboxedFs::new(
        SecretDenyFs::new(RealFs, policy.clone()),
        root.to_path_buf(),
    );
    let mut ctx = ToolContext::test_default();
    ctx.vfs = Arc::new(vfs);
    ctx.with_workspace(policy)
}

async fn grep(ctx: &ToolContext, path: &str) -> String {
    GrepTool
        .execute_with_ctx(json!({ "pattern": PATTERN, "path": path }), ctx)
        .await
        .content
}

fn assert_no_store_bytes(out: &str, where_: &str) {
    for (name, canary) in [
        ("root .git/objects", ROOT_OBJ),
        ("root .git/lfs", LFS_OBJ),
        (".svn/pristine", SVN_OBJ),
        ("vendored .git/objects", NESTED_OBJ),
    ] {
        assert!(
            !out.contains(canary),
            "core#244 c3: the {name} content store's bytes reached the model \
             through Grep({where_}) — the subprocess Grep spawns is outside the \
             OS sandbox, so the store is NOT unreachable to every shell \
             subprocess. Output:\n{out}"
        );
    }
}

/// THE MEASURED COUNTEREXAMPLE. Naming the control directory one component
/// above the store walked straight into it.
#[tokio::test]
async fn grep_cannot_harvest_a_vcs_content_store_named_one_level_up() {
    let (_dir, root) = fixture();
    let ctx = contained_ctx(&root);

    let out = grep(&ctx, ".git").await;
    assert_no_store_bytes(&out, "\".git\"");
    // Wrong-refusal control: repository METADATA under the same directory must
    // still be searchable, or the deny has broken `git rev-parse`-shaped work.
    assert!(
        out.contains(HEAD_CONTROL),
        "wrong-refusal control: .git/HEAD must stay searchable, got:\n{out}"
    );

    let out = grep(&ctx, ".svn").await;
    assert_no_store_bytes(&out, "\".svn\"");
}

/// The store directory itself, and any depth beneath it.
#[tokio::test]
async fn grep_cannot_harvest_a_vcs_content_store_named_directly() {
    let (_dir, root) = fixture();
    let ctx = contained_ctx(&root);

    for target in [
        ".git/objects",
        ".git/objects/ab",
        ".git/lfs",
        ".git/lfs/objects/aa/bb",
        ".svn/pristine",
        "vendor/x/.git/objects",
    ] {
        let out = grep(&ctx, target).await;
        assert_no_store_bytes(&out, target);
    }
}

/// A single object FILE named outright — `GrepScope::File`, no traversal.
#[tokio::test]
async fn grep_cannot_read_a_named_loose_object() {
    let (_dir, root) = fixture();
    let ctx = contained_ctx(&root);

    for target in [
        ".git/objects/ab/cd1234",
        ".git/lfs/objects/aa/bb/deadbeef",
        ".svn/pristine/aa/deadbeef.svn-base",
        "vendor/x/.git/objects/12/3456",
    ] {
        let out = grep(&ctx, target).await;
        assert_no_store_bytes(&out, target);
    }
}

/// #322 — the vendored store reached by naming its parent.
#[tokio::test]
async fn grep_cannot_harvest_a_nested_vcs_content_store() {
    let (_dir, root) = fixture();
    let ctx = contained_ctx(&root);

    for target in ["vendor", "vendor/x", "vendor/x/.git"] {
        let out = grep(&ctx, target).await;
        assert_no_store_bytes(&out, target);
    }
}

/// The whole-workspace search, and the positive control that keeps every
/// assertion above from being satisfied by a Grep that returns nothing.
#[tokio::test]
async fn an_ordinary_file_is_still_searchable() {
    let (_dir, root) = fixture();
    let ctx = contained_ctx(&root);

    let out = grep(&ctx, ".").await;
    assert!(
        out.contains(CONTROL),
        "positive control: an ordinary file's match must still be returned, \
         got:\n{out}"
    );
    assert_no_store_bytes(&out, "\".\"");

    let out = grep(&ctx, "notes.txt").await;
    assert!(
        out.contains(CONTROL),
        "positive control: an explicitly named ordinary file must still be \
         searchable, got:\n{out}"
    );
}
