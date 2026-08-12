//! P2b — the unsaved-work guard as `BashTool` actually reaches it.
//!
//! `unsaved_work::tests` grades `shell_refusal` directly. That leaves the part
//! that actually shipped ungraded: the four `BashTool` entry points that have
//! to call it, and the process-wide guard the two tools have to share. The
//! measured B-1 loss (`git checkout -- SHIPPING-API.md`, one uncommitted user
//! line gone) happened through the tool, not through the function, so it is
//! graded here through the tool.
//!
//! Each test asserts the world, not the receipt: the file on disk is read back
//! after the call and the user's line has to still be in it.

use std::process::Command;
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

const USER_LINE: &str = "# WIP do not touch";

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git must be available for these tests")
        .success();
    assert!(ok, "git {args:?} failed");
}

/// A repo whose `file.py` is committed, plus one uncommitted user line on
/// disk, reached through a real workspace grant rooted at that repo.
fn repo_ctx() -> (ToolContext, std::path::PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("file.py"), "def a():\n    return 1\n").unwrap();
    git(&root, &["add", "file.py"]);
    git(&root, &["commit", "-qm", "base"]);
    std::fs::write(
        root.join("file.py"),
        format!("def a():\n    return 1\n{USER_LINE}\n"),
    )
    .unwrap();
    let policy = Arc::new(WorkspacePolicy::trusted_local(&root));
    let ctx = ToolContext::test_default().with_workspace(policy);
    (ctx, root, dir)
}

/// The measured defect, driven through the tool the agent actually calls.
#[tokio::test]
async fn bash_refuses_a_checkout_that_would_destroy_an_uncommitted_line() {
    let (ctx, root, _keep) = repo_ctx();

    let result = BashTool
        .execute_with_ctx(json!({"command": "git checkout -- file.py"}), &ctx)
        .await;

    assert!(result.is_error, "the revert must be refused: {result:?}");
    assert!(
        result.content.contains("file.py") && result.content.contains(USER_LINE),
        "the refusal must name the file and quote the line: {}",
        result.content
    );
    // Graded from the disk, not from the message: the shell must never have run.
    let on_disk = std::fs::read_to_string(root.join("file.py")).unwrap();
    assert!(
        on_disk.contains(USER_LINE),
        "the user's uncommitted line must still be on disk, found: {on_disk:?}"
    );
}

/// Negative control on the same fixture: a git command that keeps the work
/// tree runs normally. Without this the test above could be passing because
/// Bash refuses everything.
#[tokio::test]
async fn bash_still_runs_a_git_command_that_discards_nothing() {
    let (ctx, _root, _keep) = repo_ctx();

    let result = BashTool
        .execute_with_ctx(json!({"command": "git status --porcelain"}), &ctx)
        .await;

    assert!(!result.is_error, "git status must run: {result:?}");
    assert!(
        result.content.contains("file.py"),
        "git status must have actually run and reported the modified file: {}",
        result.content
    );
}

/// The carve-out, end to end across both tools: `WriteTool` creates a file
/// through the process-wide guard, and `BashTool` — a different tool, holding
/// no reference to the first — must then let the agent revert it. This is the
/// only arm that measures that the two surfaces really share one guard.
#[tokio::test]
async fn write_then_bash_lets_the_agent_revert_its_own_new_file() {
    let (ctx, root, _keep) = repo_ctx();
    let scratch = root.join("agent_note.md");

    let written = wcore_tools::write::WriteTool::new(None)
        .execute_with_ctx(
            json!({
                "file_path": scratch.to_string_lossy(),
                "content": "notes the agent generated\nsecond generated line\n",
            }),
            &ctx,
        )
        .await;
    assert!(!written.is_error, "the write must succeed: {written:?}");

    let result = BashTool
        .execute_with_ctx(
            json!({"command": "git checkout -- agent_note.md || true"}),
            &ctx,
        )
        .await;
    assert!(
        !result.is_error,
        "the agent must stay free to revert a file it wrote itself: {}",
        result.content
    );

    // Positive control: the same untracked shape that the guard did NOT write
    // is still defended, so the pass above is the shared attribution and not a
    // blanket exemption for untracked files.
    std::fs::write(
        root.join("user_note.md"),
        "notes the user generated\nsecond user line\n",
    )
    .unwrap();
    let defended = BashTool
        .execute_with_ctx(json!({"command": "git checkout -- user_note.md"}), &ctx)
        .await;
    assert!(
        defended.is_error && defended.content.contains("notes the user generated"),
        "a file the guard never wrote is still the user's: {}",
        defended.content
    );
}

// ---------------------------------------------------------------------------
// `rm` — job corpus row A-2 (2026-08-11). The module documentation named this
// shape as escaping the shell surface, and the row is it arriving: the agent
// tidied up with
// `rm -rf .wayland-core/memory/... __pycache__ .jobcorpus-user-work`
// and the last operand was a directory holding a file the user had written and
// never committed. INV-2 reported it "deleted".
// ---------------------------------------------------------------------------

/// The measured A-2 shape: one `rm -rf` whose operands mix disposable scratch
/// with the user's own untracked notes.
#[tokio::test]
async fn bash_refuses_an_rm_that_would_delete_untracked_user_work() {
    let (ctx, root, _keep) = repo_ctx();
    std::fs::create_dir_all(root.join("user-work")).unwrap();
    std::fs::write(
        root.join("user-work/scratch-notes.md"),
        "half-written notes the user has not saved anywhere else\ncounter=41\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("__pycache__")).unwrap();
    std::fs::write(root.join("__pycache__/x.txt"), "1\n").unwrap();

    let result = BashTool
        .execute_with_ctx(
            json!({"command": "rm -rf __pycache__ user-work && ls -la"}),
            &ctx,
        )
        .await;

    assert!(result.is_error, "the removal must be refused: {result:?}");
    assert!(
        result.content.contains("scratch-notes.md"),
        "the refusal must name the file: {}",
        result.content
    );
    // Graded from the disk: the shell must never have run, so BOTH operands
    // are still there.
    assert!(
        root.join("user-work/scratch-notes.md").exists(),
        "the user's notes must still be on disk"
    );
    assert!(
        root.join("__pycache__/x.txt").exists(),
        "the whole command must have been refused, not partly run"
    );
}

/// The wrong-refusal direction: a build directory the repository itself says
/// to ignore is not the user's unsaved work, and removing it must still work.
/// Without this arm the test above passes for a guard that refuses every `rm`.
#[tokio::test]
async fn bash_still_removes_a_directory_the_repository_ignores() {
    let (ctx, root, _keep) = repo_ctx();
    std::fs::write(root.join(".gitignore"), "build/\n").unwrap();
    git(&root, &["add", ".gitignore"]);
    git(&root, &["commit", "-qm", "ignore build"]);
    std::fs::create_dir_all(root.join("build")).unwrap();
    std::fs::write(
        root.join("build/out.txt"),
        "generated output\nsecond line\n",
    )
    .unwrap();

    let result = BashTool
        .execute_with_ctx(json!({"command": "rm -rf build"}), &ctx)
        .await;

    assert!(
        !result.is_error,
        "removing ignored build output must not be refused: {}",
        result.content
    );
    assert!(
        !root.join("build").exists(),
        "the directory must actually be gone"
    );
}

/// The attribution carve-out, on the `rm` surface: a file this session wrote
/// is not the user's unsaved work, so the agent stays free to clean up after
/// itself. The positive control on the same fixture keeps it from passing as a
/// blanket exemption for untracked files.
#[tokio::test]
async fn bash_lets_the_agent_rm_a_file_it_wrote_itself() {
    let (ctx, root, _keep) = repo_ctx();

    let written = wcore_tools::write::WriteTool::new(None)
        .execute_with_ctx(
            json!({
                "file_path": root.join("agent_scratch.md").to_string_lossy(),
                "content": "scratch the agent generated\nsecond generated line\n",
            }),
            &ctx,
        )
        .await;
    assert!(!written.is_error, "the write must succeed: {written:?}");

    let mine = BashTool
        .execute_with_ctx(json!({"command": "rm -f agent_scratch.md"}), &ctx)
        .await;
    assert!(
        !mine.is_error,
        "the agent must stay able to remove its own scratch: {}",
        mine.content
    );
    assert!(
        !root.join("agent_scratch.md").exists(),
        "the agent's own file must actually be gone"
    );

    std::fs::write(
        root.join("user_scratch.md"),
        "scratch the user generated\nsecond user line\n",
    )
    .unwrap();
    let theirs = BashTool
        .execute_with_ctx(json!({"command": "rm -f user_scratch.md"}), &ctx)
        .await;
    assert!(
        theirs.is_error && theirs.content.contains("scratch the user generated"),
        "a file the guard never wrote is still the user's: {}",
        theirs.content
    );
    assert!(
        root.join("user_scratch.md").exists(),
        "the user's file must still be on disk"
    );
}

/// A file whose every line is in the commit holds no unsaved work, so removing
/// it is a normal operation. Distinguishes "refuses unsaved work" from
/// "refuses every `rm` of a tracked file".
#[tokio::test]
async fn bash_still_removes_a_file_with_nothing_unsaved_in_it() {
    let (ctx, root, _keep) = repo_ctx();
    std::fs::write(root.join("clean.py"), "def b():\n    return 2\n").unwrap();
    git(&root, &["add", "clean.py"]);
    git(&root, &["commit", "-qm", "add clean"]);

    let result = BashTool
        .execute_with_ctx(json!({"command": "rm -f clean.py"}), &ctx)
        .await;

    assert!(
        !result.is_error,
        "removing a fully committed file must not be refused: {}",
        result.content
    );
    assert!(!root.join("clean.py").exists(), "the file must be gone");
}
