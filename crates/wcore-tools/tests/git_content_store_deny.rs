//! FerroxLabs/wayland-core#388 — **`GitTool` must not reconstruct, through git
//! porcelain, the content two other layers refuse.**
//!
//! `SecretDenyFs` refuses `<root>/.env`; the read-deny sandbox shadows
//! `<root>/.git/objects` from `Bash`. `GitTool` spawns `git` through
//! `shell_command_argv`, outside both, and `git diff HEAD~1` reconstructs a
//! deleted `.env` from the object store in plaintext. Measured on the ticket at
//! `integ/f13`: `leaked=true is_error=false`, with and without a `path`
//! argument.
//!
//! Every arm drives `execute_with_ctx` — the production entry point the
//! dispatcher calls, and the one the ticket's own probe used. Driving the
//! filter helper directly would grade a function the tool might not call.
//!
//! **Red arm** (compiled first): delete the `op == "blame"` branch and the
//! `withhold_denied_hunks` call from `GitTool::execute_with_ctx`; the three
//! `Contained` arms go red with `SECRET` in the returned bytes, and the
//! `Trusted` and wrong-refusal arms stay green.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::git::GitTool;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

const SECRET: &str = "WLCANARY-COMMITTED-388";
const ORDINARY: &str = "WLCANARY-ORDINARY-OK";

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=ci",
            "-c",
            "user.email=ci@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be on PATH for this test");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A real repository whose object store holds the ONLY copy of a secret.
///
/// `.env` is committed and then deleted from the working tree, so nothing on
/// disk carries `SECRET` any more: anything that returns it reconstructed it
/// from the store. `src/main.rs` is modified in the same commit range and is
/// the wrong-refusal control — its hunks must survive the SAME invocation.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join(".env"), format!("SECRET={SECRET}\n")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "first"]);
    std::fs::remove_file(root.join(".env")).unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        format!("fn main() {{ /* {ORDINARY} */ }}\n"),
    )
    .unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "second"]);
    (dir, root)
}

fn ctx_for(policy: Arc<WorkspacePolicy>, root: &Path) -> ToolContext {
    let mut ctx = ToolContext::test_default();
    ctx.vfs = Arc::new(SandboxedFs::new(
        SecretDenyFs::new(RealFs, Arc::clone(&policy)),
        root.to_path_buf(),
    ));
    ctx.with_workspace(policy)
}

async fn run(ctx: &ToolContext, input: serde_json::Value) -> String {
    GitTool.execute_with_ctx(input, ctx).await.content
}

/// core#388 c1 + c2 + c3 — the whole-repo form, which is the one that matters:
/// it needs no `path` argument at all.
#[tokio::test]
async fn a_contained_diff_withholds_a_denied_files_hunks_and_says_so() {
    let (_dir, root) = repo();
    let ctx = ctx_for(Arc::new(WorkspacePolicy::contained(&root)), &root);

    let out = run(
        &ctx,
        json!({ "op": "diff", "rev": "HEAD~1", "cwd": root.to_str().unwrap() }),
    )
    .await;

    // c1 — the content is not there.
    assert!(
        !out.contains(SECRET),
        "core#388 c1: `git diff HEAD~1` reconstructed the committed secret \
         from the object store. Output:\n{out}"
    );
    // c2 — and the caller is TOLD, by name. A diff that silently drops a hunk
    // is a diff the model reasons from as if it were complete.
    assert!(
        out.contains("hunks withheld") && out.contains(".env"),
        "core#388 c2: the withholding must be reported and the file named. \
         Output:\n{out}"
    );
    // c3 — the wrong-refusal control, in the SAME invocation.
    assert!(
        out.contains(ORDINARY),
        "core#388 c3: an ordinary source file's hunks must still come back \
         from the same `git diff`. Output:\n{out}"
    );
}

/// core#388 c1 — the same refusal when the denied file IS named in `path`.
#[tokio::test]
async fn a_contained_diff_withholds_a_denied_file_named_in_path() {
    let (_dir, root) = repo();
    let ctx = ctx_for(Arc::new(WorkspacePolicy::contained(&root)), &root);

    let out = run(
        &ctx,
        json!({ "op": "diff", "rev": "HEAD~1", "path": ".env", "cwd": root.to_str().unwrap() }),
    )
    .await;
    assert!(
        !out.contains(SECRET),
        "core#388 c1: naming the file directly reconstructed it. Output:\n{out}"
    );
    assert!(
        out.contains("hunks withheld"),
        "core#388 c2: reported, not silent. Output:\n{out}"
    );
}

/// core#388 c3 — `status` and `log` carry no committed content and are
/// unaffected.
#[tokio::test]
async fn contained_status_and_log_are_unaffected() {
    let (_dir, root) = repo();
    let ctx = ctx_for(Arc::new(WorkspacePolicy::contained(&root)), &root);
    let cwd = root.to_str().unwrap();

    let status = run(&ctx, json!({ "op": "status", "cwd": cwd })).await;
    assert!(
        status.contains("main"),
        "core#388 c3: `Git(op=status)` must still report the branch. \
         Output:\n{status}"
    );
    let log = run(&ctx, json!({ "op": "log", "cwd": cwd })).await;
    assert!(
        log.contains("second") && log.contains("first"),
        "core#388 c3: `Git(op=log)` must still list both commits. Output:\n{log}"
    );
    assert!(
        !log.contains("hunks withheld"),
        "core#388 c3: nothing was withheld from a log, so nothing must be \
         reported. Output:\n{log}"
    );
}

/// core#388 c4 — `blame` under the same posture, with a fixture where the path
/// DOES exist in the named revision.
///
/// The ticket's own probe hit `fatal: no such path '.env' in HEAD`, so blame
/// was UNGRADED rather than proven safe. Here `.env` exists at `HEAD~1` and
/// `git blame HEAD~1 -- .env` prints the committed line itself — there is no
/// hunk to strip, so the refusal is taken before `git` runs.
#[tokio::test]
async fn a_contained_blame_of_a_denied_path_is_refused_and_reported() {
    let (_dir, root) = repo();
    let cwd = root.to_str().unwrap();

    // Fixture control: the path really is blameable in that revision, so a
    // refusal below is the filter's and not git's.
    let raw = Command::new("git")
        .args(["blame", "-L", "1,1", "HEAD~1", "--", ".env"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        raw.status.success() && String::from_utf8_lossy(&raw.stdout).contains(SECRET),
        "fixture control: `git blame HEAD~1 -- .env` must succeed and print \
         the secret, or this arm grades nothing. stdout={} stderr={}",
        String::from_utf8_lossy(&raw.stdout),
        String::from_utf8_lossy(&raw.stderr)
    );

    let ctx = ctx_for(Arc::new(WorkspacePolicy::contained(&root)), &root);
    let out = run(
        &ctx,
        json!({ "op": "blame", "path": ".env", "line": 1, "rev": "HEAD~1", "cwd": cwd }),
    )
    .await;
    assert!(
        !out.contains(SECRET),
        "core#388 c4: `Git(op=blame)` printed the committed secret. \
         Output:\n{out}"
    );
    assert!(
        out.contains("withheld") && out.contains(".env"),
        "core#388 c4: the blame refusal must be reported and name the file. \
         Output:\n{out}"
    );

    // Wrong-refusal control: an ordinary path still blames.
    let ok = run(
        &ctx,
        json!({ "op": "blame", "path": "src/main.rs", "line": 1, "cwd": cwd }),
    )
    .await;
    assert!(
        ok.contains(ORDINARY),
        "core#388 c4: an ordinary file must still be blameable. Output:\n{ok}"
    );
}

/// core#388 c5 — **the posture boundary, pinned in both directions.**
///
/// Sean's #667 ruling is that a genuinely-local operator may still read their
/// own `.env`; that ruling was made for `Trusted`, and this filter is scoped to
/// what `denies_read_content` refuses, which a `trusted_local` policy does not.
/// So the two postures give different answers on the SAME repository and the
/// same call, and both halves are asserted here — a filter that refuses
/// everywhere would silently overturn #667.
#[tokio::test]
async fn the_posture_decides_and_trusted_local_is_left_alone() {
    let (_dir, root) = repo();
    let cwd = root.to_str().unwrap();
    let call = json!({ "op": "diff", "rev": "HEAD~1", "cwd": cwd });

    let trusted = run(
        &ctx_for(Arc::new(WorkspacePolicy::trusted_local(&root)), &root),
        call.clone(),
    )
    .await;
    assert!(
        trusted.contains(SECRET),
        "core#388 c5: a genuinely-local Trusted session keeps #667's carve-out \
         — its own `.env` is still readable. Output:\n{trusted}"
    );
    assert!(
        !trusted.contains("hunks withheld"),
        "core#388 c5: nothing was withheld in Trusted, so nothing is reported"
    );

    let contained = run(
        &ctx_for(Arc::new(WorkspacePolicy::contained(&root)), &root),
        call,
    )
    .await;
    assert!(
        !contained.contains(SECRET),
        "core#388 c5: the same call in Contained must withhold. \
         Output:\n{contained}"
    );
    assert_ne!(
        trusted.contains(SECRET),
        contained.contains(SECRET),
        "core#388 c5: the two postures must be DIFFERENT, or the boundary is \
         decoration"
    );
}
