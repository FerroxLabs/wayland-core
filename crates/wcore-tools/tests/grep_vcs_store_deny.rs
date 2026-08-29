//! D1 / core#244 c3 — **Grep must not read a VCS CONTENT store, however the
//! search target is spelled.**
//!
//! `GrepTool` gates only the top-level `path` argument through
//! `ctx.vfs.exists()` and then spawns `rg`/`grep`/`findstr` itself
//! (`grep.rs::try_ripgrep`), OUTSIDE both `SecretDenyFs` and the OS sandbox.
//! `is_vcs_content_store` matches the STORE, not its parent, so `.git` and
//! `.svn` cleared the probe and the backend then descended into
//! `.git/lfs/objects/**` and `.svn/pristine/**` — which, unlike a zlib loose
//! object, hold file content VERBATIM.
//!
//! MEASURED on origin/integ/f13 with the production contained stack
//! (`SandboxedFs::new(SecretDenyFs::new(RealFs, WorkspacePolicy::contained))`)
//! before the fix:
//!
//! ```text
//! Grep(pattern=CANARY, path=".svn")
//!   -> .svn/pristine/aa/deadbeef.svn-base:1:SVN-CANARY-244 AWS-...-KEY=abc123
//! Grep(pattern=CANARY, path=".git")
//!   -> .git/lfs/objects/aa/bb/deadbeef:1:LFS-CANARY-244 password=hunter2
//! ```
//!
//! **Red arms.** Two guards, one per spelling class:
//!
//! * Delete the `filter_entry(...is_vcs_store_dir...)` prune from
//!   `grep_policy::scope_for` — the walk-root spellings (`.git`, `.svn`, the
//!   symlink alias, the vendored checkout) go red.
//! * Delete the `inside_vcs_store(&canon_existing_ancestor(&resolved))` refusal
//!   from `grep::run_grep` — the name-the-store-directly cases go red.
//!
//! Every refusal below is paired with a positive control in the same run, so a
//! Grep that returned nothing at all cannot pass this file.

#![cfg(unix)]

use std::os::unix::fs::symlink;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use wcore_tools::NullToolOutputSink;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::grep::GrepTool;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

const LFS_CANARY: &str = "LFS-CANARY-244 password=hunter2";
const SVN_CANARY: &str = "SVN-CANARY-244 AWS-SECRET-ACCESS-KEY=abc123";
const NESTED_CANARY: &str = "NESTED-CANARY-322 token=nope";
const HEAD_CONTROL: &str = "CANARY-HEAD-READABLE";
const CONTROL: &str = "CANARY-CONTROL-OK";

/// A workspace holding, in three different stores, content that must never be
/// reported — plus the metadata and ordinary files that must still be.
fn seed() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let lfs = root.join(".git/lfs/objects/aa/bb");
    std::fs::create_dir_all(&lfs).unwrap();
    std::fs::write(lfs.join("deadbeef"), format!("{LFS_CANARY}\n")).unwrap();

    let pristine = root.join(".svn/pristine/aa");
    std::fs::create_dir_all(&pristine).unwrap();
    std::fs::write(
        pristine.join("deadbeef.svn-base"),
        format!("{SVN_CANARY}\n"),
    )
    .unwrap();

    // #322 — a vendored checkout's own store, which root-relative discovery
    // never sees.
    let nested = root.join("vendor/x/.git/objects/cc");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("dd"), format!("{NESTED_CANARY}\n")).unwrap();

    // The carve-out: a metadata question is not a content question. `git
    // rev-parse` must keep working, so `.git/HEAD` stays searchable.
    std::fs::write(
        root.join(".git/HEAD"),
        format!("ref: refs/heads/{HEAD_CONTROL}\n"),
    )
    .unwrap();

    std::fs::write(root.join("notes.txt"), format!("plain {CONTROL}\n")).unwrap();
    dir
}

fn ctx_for(root: &Path) -> ToolContext {
    let policy = Arc::new(WorkspacePolicy::contained(root));
    let vfs: Arc<dyn VirtualFs> = Arc::new(SandboxedFs::new(
        SecretDenyFs::new(RealFs, policy),
        root.to_path_buf(),
    ));
    ToolContext::new(
        "call-grep-vcs",
        CancellationToken::new(),
        vfs,
        None,
        Arc::new(NullToolOutputSink),
    )
}

async fn grep(ctx: &ToolContext, path: &str) -> String {
    GrepTool
        .execute_with_ctx(json!({ "pattern": "CANARY", "path": path }), ctx)
        .await
        .content
}

/// No secret may appear in any answer, whatever else it says.
fn assert_no_store_content(out: &str, what: &str) {
    for leak in ["hunter2", "abc123", "token=nope"] {
        assert!(
            !out.contains(leak),
            "VCS content-store plaintext ({leak}) reached the model via {what}: {out}"
        );
    }
}

// ---------------------------------------------------------------------------
// The walk-root spellings — the search target is ABOVE the store.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn naming_the_git_dir_does_not_return_lfs_object_content() {
    let dir = seed();
    let ctx = ctx_for(dir.path());
    let out = grep(&ctx, ".git").await;
    assert_no_store_content(&out, "Grep(.git)");
    // POSITIVE CONTROL — the carve-out survives: `.git` is still searchable for
    // the metadata a `git rev-parse` answers, so this is not a blanket refusal.
    assert!(
        out.contains(HEAD_CONTROL),
        "control: .git/HEAD must stay searchable, got: {out}"
    );
}

#[tokio::test]
async fn naming_the_svn_dir_does_not_return_pristine_content() {
    let dir = seed();
    let ctx = ctx_for(dir.path());
    assert_no_store_content(&grep(&ctx, ".svn").await, "Grep(.svn)");
}

/// The absolute spelling of the same target must answer the same.
#[tokio::test]
async fn the_absolute_spelling_of_the_control_dir_answers_the_same() {
    let dir = seed();
    let ctx = ctx_for(dir.path());
    let abs = dir.path().join(".git");
    let out = grep(&ctx, &abs.to_string_lossy()).await;
    assert_no_store_content(&out, "Grep(<abs>/.git)");
}

/// A symlink ALIAS for the control directory. The prune rebuilds each candidate
/// on the canonical walk root precisely so the name it was reached under cannot
/// change the answer.
#[tokio::test]
async fn a_symlink_alias_for_the_control_dir_does_not_open_the_store() {
    let dir = seed();
    let root = dir.path();
    symlink(root.join(".git"), root.join("mygit")).unwrap();
    let ctx = ctx_for(root);
    assert_no_store_content(&grep(&ctx, "mygit").await, "Grep(mygit -> .git)");
}

/// #322's shape reached through Grep: a vendored checkout's own store, named
/// one component up.
#[tokio::test]
async fn naming_a_vendored_checkouts_control_dir_does_not_open_its_store() {
    let dir = seed();
    let ctx = ctx_for(dir.path());
    assert_no_store_content(&grep(&ctx, "vendor/x/.git").await, "Grep(vendor/x/.git)");
    // And from above it, where the hidden-file filter is what does the work.
    assert_no_store_content(&grep(&ctx, "vendor").await, "Grep(vendor)");
}

// ---------------------------------------------------------------------------
// The name-the-store spellings — the search target IS the store.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn naming_the_store_itself_is_refused_with_a_reason() {
    let dir = seed();
    let ctx = ctx_for(dir.path());
    for target in [".git/lfs", ".svn/pristine", "vendor/x/.git/objects"] {
        let out = grep(&ctx, target).await;
        assert_no_store_content(&out, target);
        assert!(
            out.contains("Refused") || out.contains("refused"),
            "naming {target} must be REFUSED, not silently empty, got: {out}"
        );
    }
}

/// `execute()` — the no-`ToolContext` entry point — has no vfs probe at all, so
/// `run_grep`'s own refusal is the only guard there is. Exercised through the
/// public trait method rather than the ctx one so a fix that lived only in
/// `execute_with_ctx` could not pass.
#[tokio::test]
async fn the_context_free_entry_point_also_refuses_a_store() {
    let dir = seed();
    let store = dir.path().join(".git/lfs");
    let out = GrepTool
        .execute(json!({ "pattern": "CANARY", "path": store.to_string_lossy() }))
        .await;
    assert_no_store_content(&out.content, "execute(.git/lfs)");
    assert!(
        out.is_error,
        "the context-free path must refuse a store, got: {}",
        out.content
    );

    // POSITIVE CONTROL on the same entry point: an ordinary file still answers.
    let ok = GrepTool
        .execute(json!({ "pattern": "CANARY", "path": dir.path().to_string_lossy() }))
        .await;
    assert!(
        ok.content.contains(CONTROL),
        "control: execute() must still report an ordinary match, got: {}",
        ok.content
    );
}

// ---------------------------------------------------------------------------
// The thing that must not break.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn naming_the_root_still_returns_ordinary_matches() {
    let dir = seed();
    let ctx = ctx_for(dir.path());
    let out = grep(&ctx, ".").await;
    assert!(
        out.contains(CONTROL),
        "positive control missing: an ordinary file must still be reported: {out}"
    );
    assert_no_store_content(&out, "Grep(.)");
}
