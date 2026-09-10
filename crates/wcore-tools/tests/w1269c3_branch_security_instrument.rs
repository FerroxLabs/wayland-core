//! wayland#1269 c3 — the `lane/f13-n-security` instrument, run against THIS
//! tree.
//!
//! Ported VERBATIM from `lane/f13-n-security`
//! (228c5c4d29db2391235df1c31e5d945810bb8474)
//! `crates/wcore-tools/tests/git_secret_content_test.rs`, with exactly two
//! harness adaptations, both necessary and both recorded:
//!
//! 1. `ctx_for` attaches the `WorkspacePolicy` to the `ToolContext`
//!    (`with_workspace`). The branch put its guard INSIDE the vfs
//!    (`refuse_secret_path` / `withhold_secret_diff_sections`), so installing
//!    `SecretDenyFs` was enough there. This tree gates on
//!    `ctx.workspace.secret_read_deny_required()` (git.rs:755-761) and returns
//!    `self.execute(input)` unfiltered when `workspace` is `None`. Running the
//!    branch file unadapted therefore grades NOTHING about this tree's guard —
//!    it measures a context production never builds. The tree's own
//!    `git_content_store_deny.rs::ctx_for` is copied here so the wiring is the
//!    tree's canonical one.
//! 2. The repo root is canonicalized, as the tree's own harness does, so a
//!    symlinked temp root cannot make the policy root and the git cwd disagree
//!    and produce a false green.
//!
//! Every assertion below is the branch's, unchanged.

#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::git::GitTool;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

const SECRET: &str = "AWS-SECRET-ACCESS-KEY=PROBE-GIT-9931";
const CONTROL: &str = "PROBE-CONTROL-OK";

fn git(cwd: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "p")
        .env("GIT_AUTHOR_EMAIL", "p@p")
        .env("GIT_COMMITTER_NAME", "p")
        .env("GIT_COMMITTER_EMAIL", "p@p")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repo where the secret exists ONLY in the object store, alongside an
/// ordinary file that changed in the same two commits.
fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join(".env"), format!("{SECRET}\n")).unwrap();
    std::fs::write(root.join("a.txt"), format!("first {CONTROL}\n")).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "seed"]);

    std::fs::remove_file(root.join(".env")).unwrap();
    std::fs::write(root.join("a.txt"), format!("second {CONTROL}\n")).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "drop"]);
    (dir, root)
}

fn ctx_for(root: &Path) -> ToolContext {
    let policy = Arc::new(WorkspacePolicy::contained(root));
    let mut ctx = ToolContext::test_default();
    ctx.vfs = Arc::new(SandboxedFs::new(
        SecretDenyFs::new(RealFs, Arc::clone(&policy)),
        root.to_path_buf(),
    ));
    ctx.with_workspace(policy)
}

async fn run(ctx: &ToolContext, input: serde_json::Value) -> wcore_types::tool::ToolResult {
    GitTool.execute_with_ctx(input, ctx).await
}

/// Naming the secret outright. The measured defect.
///
/// THE ONE DIVERGENCE, recorded rather than hidden. The branch asserted
/// `out.is_error` here — a hard refusal. MEASURED against this tree at
/// 0602d2da6: that assertion FAILS, and it is the ONLY branch assertion in
/// this file that does. The payload was
///
/// ```text
/// it must be refused, not empty: diff --git a/.env b/.env
/// [Git] hunks withheld: .env is denied for content reads in this workspace posture
/// [Git] 1 file(s)' hunks withheld (.env)
/// ```
///
/// so the secret did NOT come back — the confidentiality assertion above
/// passed, unchanged — and the tree instead withholds the hunks and names the
/// file, with `is_error == false`. That shape is deliberate here and pinned by
/// this tree's own
/// `git_content_store_deny.rs::a_contained_diff_withholds_a_denied_file_named_in_path`.
/// The test name is changed to match what is actually asserted; renaming it
/// `..._is_refused` while asserting withholding would be the lie.
#[tokio::test]
async fn diff_pointed_at_a_committed_secret_does_not_return_it() {
    let (_dir, root) = repo();
    let cwd = root.to_string_lossy().into_owned();
    let ctx = ctx_for(&root);

    let out = run(
        &ctx,
        json!({"op": "diff", "cwd": cwd, "rev": "HEAD~1", "path": ".env"}),
    )
    .await;
    assert!(
        !out.content.contains("PROBE-GIT-9931"),
        "GitTool::diff reconstructed the committed secret: {}",
        out.content
    );
    assert!(
        out.content.contains("withheld") && out.content.contains(".env"),
        "it must be withheld and the file named, not silently empty: {}",
        out.content
    );

    // POSITIVE CONTROL — the same op on an ordinary file still works, so this
    // is not a GitTool that refuses everything.
    let ok = run(
        &ctx,
        json!({"op": "diff", "cwd": cwd, "rev": "HEAD~1", "path": "a.txt"}),
    )
    .await;
    assert!(!ok.is_error, "control diff errored: {}", ok.content);
    assert!(
        ok.content.contains(CONTROL),
        "control: an ordinary file's diff must still come back: {}",
        ok.content
    );
}

/// The op with NO path — the shape a path check alone cannot catch, because the
/// caller never names the secret. The whole-tree diff must come back with the
/// secret's section withheld and the ordinary file's section intact.
#[tokio::test]
async fn a_whole_tree_diff_withholds_only_the_secret_section() {
    let (_dir, root) = repo();
    let cwd = root.to_string_lossy().into_owned();
    let ctx = ctx_for(&root);

    let out = run(&ctx, json!({"op": "diff", "cwd": cwd, "rev": "HEAD~1"})).await;
    assert!(
        !out.is_error,
        "the whole-tree diff errored: {}",
        out.content
    );
    assert!(
        !out.content.contains("PROBE-GIT-9931"),
        "the secret's section survived the whole-tree diff: {}",
        out.content
    );
    // POSITIVE CONTROL — the rest of the diff is intact.
    assert!(
        out.content.contains(CONTROL) && out.content.contains("a.txt"),
        "the ordinary file's section must survive: {}",
        out.content
    );
    // Rule 5, borrowed from grep_policy: withholding is reported, never silent.
    assert!(
        out.content.contains("withheld") && out.content.contains(".env"),
        "the withholding must be reported and the file named: {}",
        out.content
    );
}

/// `blame` returns one file's content with no sections to withhold, so the
/// path check is the whole guard.
#[tokio::test]
async fn blame_pointed_at_a_secret_is_refused() {
    let (_dir, root) = repo();
    let cwd = root.to_string_lossy().into_owned();
    std::fs::write(root.join(".env"), format!("{SECRET}\n")).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "restore the secret to the tree"]);
    let ctx = ctx_for(&root);

    // CONTROL ON THE FIXTURE: git itself can blame this path now, so a refusal
    // below is the product's and not git's.
    let raw = std::process::Command::new("git")
        .args(["blame", "-L", "1,1", "--", ".env"])
        .current_dir(&root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git");
    assert!(
        raw.status.success() && String::from_utf8_lossy(&raw.stdout).contains("PROBE-GIT-9931"),
        "fixture control: git blame must succeed and show the secret, else the \
         refusal below grades nothing"
    );

    let out = run(
        &ctx,
        json!({"op": "blame", "cwd": cwd, "path": ".env", "line": 1}),
    )
    .await;
    assert!(
        !out.content.contains("PROBE-GIT-9931"),
        "GitTool::blame returned the secret: {}",
        out.content
    );
    assert!(out.is_error, "it must be refused: {}", out.content);

    // POSITIVE CONTROL.
    let ok = run(
        &ctx,
        json!({"op": "blame", "cwd": cwd, "path": "a.txt", "line": 1}),
    )
    .await;
    assert!(!ok.is_error, "control blame errored: {}", ok.content);
    assert!(
        ok.content.contains(CONTROL),
        "control: an ordinary file's blame must still come back: {}",
        ok.content
    );
}

/// A rename INTO a secret name. The post-image is what the header's `b/` side
/// carries, and it is what decides.
#[tokio::test]
async fn a_rename_into_a_secret_name_is_withheld_too() {
    let (_dir, root) = repo();
    let cwd = root.to_string_lossy().into_owned();
    std::fs::write(root.join("plain.txt"), "placeholder\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "add plain"]);
    // The SECRET arrives in the same commit as the rename. A pure rename emits
    // `similarity index 100%` and NO hunk, so a test that renamed unchanged
    // content would pass against the pre-fix tree with nothing to withhold —
    // MEASURED, it did.
    git(&root, &["mv", "plain.txt", "prod.pem"]);
    std::fs::write(root.join("prod.pem"), format!("{SECRET}\n")).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "rename into a secret name"]);

    let ctx = ctx_for(&root);
    let out = run(&ctx, json!({"op": "diff", "cwd": cwd, "rev": "HEAD~1"})).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        !out.content.contains("PROBE-GIT-9931"),
        "a rename into a secret name leaked its content: {}",
        out.content
    );
    assert!(
        out.content.contains("withheld") && out.content.contains("prod.pem"),
        "the withheld section must be reported and named: {}",
        out.content
    );
}
