//! D1 / core#244 c3 — **Grep must not read a VCS CONTENT store, however the
//! search target is spelled.**
//!
//! `GrepTool` gates only the top-level `path` argument through
//! `ctx.vfs.exists()` and then spawns `rg`/`grep`/`findstr` itself
//! (`grep.rs::try_ripgrep`), OUTSIDE both `SecretDenyFs` and the OS sandbox.
//! `is_vcs_content_store` matches the STORE, not its parent, so `.git` and
//! `.svn` clear the probe and the backend then descends into
//! `.git/lfs/objects/**` and `.svn/pristine/**` — which, unlike a zlib loose
//! object, hold file content VERBATIM.
//!
//! Every refusal below is paired with a positive control in the same run, so a
//! Grep that returned nothing at all cannot pass this file.

#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::grep::GrepTool;
use wcore_tools::NullToolOutputSink;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

const LFS_CANARY: &str = "LFS-CANARY-244 password=hunter2";
const SVN_CANARY: &str = "SVN-CANARY-244 AWS-SECRET-ACCESS-KEY=abc123";
const CONTROL: &str = "CANARY-CONTROL-OK";

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
    // Metadata that must STAY readable: a deny that swallowed .git/HEAD would
    // break ordinary session work.
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
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

#[tokio::test]
async fn naming_the_git_dir_does_not_return_lfs_object_content() {
    let dir = seed();
    let ctx = ctx_for(dir.path());
    let out = grep(&ctx, ".git").await;
    assert!(
        !out.contains("hunter2"),
        "the LFS object store's plaintext reached the model via Grep(.git): {out}"
    );
}

#[tokio::test]
async fn naming_the_svn_dir_does_not_return_pristine_content() {
    let dir = seed();
    let ctx = ctx_for(dir.path());
    let out = grep(&ctx, ".svn").await;
    assert!(
        !out.contains("abc123"),
        "the svn pristine store's plaintext reached the model via Grep(.svn): {out}"
    );
}

#[tokio::test]
async fn naming_the_root_still_returns_ordinary_matches() {
    let dir = seed();
    let ctx = ctx_for(dir.path());
    let out = grep(&ctx, ".").await;
    assert!(
        out.contains(CONTROL),
        "positive control missing: an ordinary file must still be reported: {out}"
    );
    assert!(!out.contains("hunter2") && !out.contains("abc123"), "{out}");
}
