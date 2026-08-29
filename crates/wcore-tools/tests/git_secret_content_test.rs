//! D1 / core#244, GitTool half — **`diff` and `blame` return file CONTENT, so a
//! committed secret must not come back through them either.**
//!
//! `GitTool` gates only `cwd` through `ctx.vfs.exists()` and then runs `git`
//! itself via `shell_command_argv`, so `SecretDenyFs` never sees the read. And
//! under the STRICT sandbox `Bash` cannot run `git` at all (`.git/config` is on
//! the secret deny-list), which makes this the ONLY door — so it is the door
//! that has to hold.
//!
//! MEASURED on origin/integ/f13 before the fix, with the production contained
//! stack and `.env` committed then DELETED from the working tree, so its bytes
//! live only in the object store:
//!
//! ```text
//! Git{op: diff, rev: HEAD~1, path: ".env"}
//!   -> diff --git a/.env b/.env
//!      deleted file mode 100644
//!      -AWS_SECRET_ACCESS_KEY=PROBE-GIT-9931
//! ```
//!
//! **Red arm:** delete the `refuse_secret_path` call from the `diff` arm and
//! the `apply_diff_secret_policy` wrapper around its `run_git`, and the same
//! call from the `blame` arm.
//!
//! Every refusal below is paired with a positive control in the same run — an
//! ORDINARY file whose diff and blame must still come back — so a GitTool that
//! refused everything could not pass this file.

#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use wcore_tools::NullToolOutputSink;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::git::GitTool;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs, VirtualFs};
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
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join(".env"), format!("{SECRET}\n")).unwrap();
    std::fs::write(root.join("a.txt"), format!("first {CONTROL}\n")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "seed"]);

    std::fs::remove_file(root.join(".env")).unwrap();
    std::fs::write(root.join("a.txt"), format!("second {CONTROL}\n")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "drop"]);
    dir
}

fn ctx_for(root: &Path) -> ToolContext {
    let policy = Arc::new(WorkspacePolicy::contained(root));
    let vfs: Arc<dyn VirtualFs> = Arc::new(SandboxedFs::new(
        SecretDenyFs::new(RealFs, policy),
        root.to_path_buf(),
    ));
    ToolContext::new(
        "call-git-secret",
        CancellationToken::new(),
        vfs,
        None,
        Arc::new(NullToolOutputSink),
    )
}

async fn run(ctx: &ToolContext, input: serde_json::Value) -> wcore_types::tool::ToolResult {
    GitTool.execute_with_ctx(input, ctx).await
}

/// Naming the secret outright. The measured defect.
#[tokio::test]
async fn diff_pointed_at_a_committed_secret_is_refused() {
    let dir = repo();
    let cwd = dir.path().to_string_lossy().into_owned();
    let ctx = ctx_for(dir.path());

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
        out.is_error,
        "it must be refused, not empty: {}",
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
    let dir = repo();
    let cwd = dir.path().to_string_lossy().into_owned();
    let ctx = ctx_for(dir.path());

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
///
/// The secret is put BACK in the working tree first. `blame` has no `rev`
/// parameter, so against a deleted path it fails with "no such path" and the
/// refusal assertion is satisfied by an unrelated error — MEASURED: written
/// that way, this test passed against the PRE-FIX tree. A guard is only graded
/// where the op would otherwise succeed.
#[tokio::test]
async fn blame_pointed_at_a_secret_is_refused() {
    let dir = repo();
    let root = dir.path();
    let cwd = root.to_string_lossy().into_owned();
    std::fs::write(root.join(".env"), format!("{SECRET}\n")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "restore the secret to the tree"]);
    let ctx = ctx_for(root);

    // CONTROL ON THE FIXTURE: git itself can blame this path now, so a refusal
    // below is the product's and not git's.
    let raw = std::process::Command::new("git")
        .args(["blame", "-L", "1,1", "--", ".env"])
        .current_dir(root)
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
    let dir = repo();
    let root = dir.path();
    let cwd = root.to_string_lossy().into_owned();
    std::fs::write(root.join("plain.txt"), "placeholder\n").unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "add plain"]);
    // The SECRET arrives in the same commit as the rename. A pure rename emits
    // `similarity index 100%` and NO hunk, so a test that renamed unchanged
    // content would pass against the pre-fix tree with nothing to withhold —
    // MEASURED, it did.
    git(root, &["mv", "plain.txt", "prod.pem"]);
    std::fs::write(root.join("prod.pem"), format!("{SECRET}\n")).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "rename into a secret name"]);

    let ctx = ctx_for(root);
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
