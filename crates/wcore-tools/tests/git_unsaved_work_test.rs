//! INV-2 as `GitTool` actually reaches it.
//!
//! `unsaved_work::git_ops` is graded here through the tool, not through the
//! function, because the measured defect happened through the tool: job corpus
//! row A-8 (2026-08-11) drove `{"op":"add_paths","paths":["README.md", ...]}`
//! and then `{"op":"commit"}`, and INV-2 read the commit back as "the user's
//! unsaved work was committed on their behalf". Row A-2 drove
//! `{"op":"add_all"}` and swept the user's untracked scratch file into the
//! index the same way.
//!
//! Every arm asserts the world after the call — the index, or `git log` — and
//! never the tool's own message. Every refusing arm has a negative control on
//! the same fixture, because a guard that refuses everything would pass the
//! refusing arms on its own.

use std::path::Path;
use std::process::Command;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::git::GitTool;

const USER_LINE: &str = "# WIP do not touch";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be available for these tests");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// What is currently staged, one path per line.
fn staged(root: &Path) -> String {
    git(root, &["diff", "--cached", "--name-only"])
}

fn commit_count(root: &Path) -> usize {
    git(root, &["rev-list", "--count", "HEAD"])
        .trim()
        .parse()
        .unwrap()
}

/// A repository with `README.md` and `retry.py` committed, then the user's own
/// uncommitted line appended to `README.md`. Nothing in this session has
/// written to either file.
fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("README.md"), "# receipts\n\nAdds up lines.\n").unwrap();
    std::fs::write(root.join("retry.py"), "def fetch():\n    return 1\n").unwrap();
    git(&root, &["add", "README.md", "retry.py"]);
    git(&root, &["commit", "-qm", "base"]);
    std::fs::write(
        root.join("README.md"),
        format!("# receipts\n\nAdds up lines.\n<!-- {USER_LINE}\n"),
    )
    .unwrap();
    (dir, root)
}

/// Row A-8, driven through the op the model actually called.
#[tokio::test]
async fn add_paths_refuses_a_file_whose_only_change_is_the_users_unsaved_work() {
    let (_keep, root) = repo();

    let result = GitTool
        .execute(json!({
            "op": "add_paths",
            "paths": ["README.md"],
            "cwd": root.to_string_lossy(),
        }))
        .await;

    assert!(result.is_error, "staging must be refused: {result:?}");
    assert!(
        result.content.contains("README.md") && result.content.contains(USER_LINE),
        "the refusal must name the file and quote the line: {}",
        result.content
    );
    assert!(
        staged(&root).trim().is_empty(),
        "nothing may have been staged, found: {:?}",
        staged(&root)
    );
}

/// The wrong-refusal direction on the same fixture: a file this session really
/// did change stays committable, even though the user's own line is in it.
/// Without this arm the test above passes for a guard that refuses every
/// `add_paths`.
#[tokio::test]
async fn add_paths_still_stages_a_file_this_session_changed() {
    let (_keep, root) = repo();
    let target = root.join("retry.py");
    // The user's unsaved line goes into this file too, so the only difference
    // from the arm above is that the agent also worked on it.
    std::fs::write(
        &target,
        format!("def fetch():\n    return 1\n{USER_LINE}\n"),
    )
    .unwrap();

    let edited = wcore_tools::edit::EditTool::new(None)
        .execute(json!({
            "file_path": target.to_string_lossy(),
            "old_string": "    return 1",
            "new_string": "    return 2  # fixed",
        }))
        .await;
    assert!(!edited.is_error, "the edit must succeed: {edited:?}");

    let result = GitTool
        .execute(json!({
            "op": "add_paths",
            "paths": ["retry.py"],
            "cwd": root.to_string_lossy(),
        }))
        .await;

    assert!(
        !result.is_error,
        "a file the agent changed must stay stageable: {}",
        result.content
    );
    assert!(
        staged(&root).contains("retry.py"),
        "retry.py must actually be in the index, found: {:?}",
        staged(&root)
    );
    assert!(
        result.content.contains(USER_LINE) || result.content.contains("retry.py"),
        "the user's line riding along must be reported: {}",
        result.content
    );
}

/// Row A-2's `add_all`: `git add -A` stages paths nobody named, which is how
/// the user's untracked scratch file reached the index.
#[tokio::test]
async fn add_all_refuses_while_untracked_user_work_is_on_disk() {
    let (_keep, root) = repo();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(
        root.join("notes/scratch.md"),
        "half-written notes the user has not saved anywhere else\ncounter=41\n",
    )
    .unwrap();

    let result = GitTool
        .execute(json!({"op": "add_all", "cwd": root.to_string_lossy()}))
        .await;

    assert!(result.is_error, "add -A must be refused: {result:?}");
    assert!(
        result.content.contains("scratch.md"),
        "the refusal must name the file: {}",
        result.content
    );
    assert!(
        staged(&root).trim().is_empty(),
        "nothing may have been staged, found: {:?}",
        staged(&root)
    );
}

/// The wrong-refusal direction for `add_all`: a tree whose only untracked file
/// this session wrote itself stages normally.
#[tokio::test]
async fn add_all_still_stages_a_tree_the_agent_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("seed.txt"), "seed\n").unwrap();
    git(&root, &["add", "seed.txt"]);
    git(&root, &["commit", "-qm", "base"]);

    let written = wcore_tools::write::WriteTool::new(None)
        .execute(json!({
            "file_path": root.join("generated.md").to_string_lossy(),
            "content": "notes the agent generated\nsecond generated line\n",
        }))
        .await;
    assert!(!written.is_error, "the write must succeed: {written:?}");

    let result = GitTool
        .execute(json!({"op": "add_all", "cwd": root.to_string_lossy()}))
        .await;

    assert!(
        !result.is_error,
        "the agent must stay able to stage its own work: {}",
        result.content
    );
    assert!(
        staged(&root).contains("generated.md"),
        "the agent's file must be in the index, found: {:?}",
        staged(&root)
    );
}

/// The index is checked again at commit time, so a brand-new file staged
/// outside this tool cannot carry the user's work into a commit.
#[tokio::test]
async fn commit_refuses_an_index_holding_a_new_file_that_is_only_user_work() {
    let (_keep, root) = repo();
    std::fs::write(
        root.join("scratch.md"),
        "half-written notes the user has not saved anywhere else\ncounter=41\n",
    )
    .unwrap();
    // Staged through raw git, the way a `Bash` call would do it.
    git(&root, &["add", "scratch.md"]);
    let before = commit_count(&root);

    let result = GitTool
        .execute(json!({
            "op": "commit",
            "message": "wip",
            "allow_default_branch": true,
            "cwd": root.to_string_lossy(),
        }))
        .await;

    assert!(result.is_error, "the commit must be refused: {result:?}");
    assert_eq!(
        commit_count(&root),
        before,
        "no commit may have been created"
    );
    assert!(
        root.join("scratch.md").exists(),
        "the user's file must still be on disk"
    );
}

/// The stated residual, pinned so it is known rather than discovered.
///
/// A file the pinned commit already records, modified outside `Write`/`Edit`,
/// is indistinguishable from the user's own unsaved edit — so `add_all` plus
/// `commit` stages and commits it, and only says so. Refusing it instead would
/// refuse ordinary work (`git_branch_and_pr_test` does exactly this shape), and
/// a guard that refuses ordinary work is a regression, not a fix.
#[tokio::test]
async fn add_all_reports_but_does_not_refuse_a_tracked_files_unsaved_edit() {
    let (_keep, root) = repo();

    let result = GitTool
        .execute(json!({"op": "add_all", "cwd": root.to_string_lossy()}))
        .await;

    assert!(
        !result.is_error,
        "a tracked modification must not be refused: {}",
        result.content
    );
    assert!(
        result.content.contains("README.md"),
        "the note must name the path riding along: {}",
        result.content
    );
    assert!(
        staged(&root).contains("README.md"),
        "README.md must actually be in the index, found: {:?}",
        staged(&root)
    );
}

/// Negative control for the commit arm: an index holding the session's own
/// work commits normally.
#[tokio::test]
async fn commit_still_lands_the_sessions_own_work() {
    let (_keep, root) = repo();
    let target = root.join("retry.py");
    let edited = wcore_tools::edit::EditTool::new(None)
        .execute(json!({
            "file_path": target.to_string_lossy(),
            "old_string": "    return 1",
            "new_string": "    return 2  # fixed",
        }))
        .await;
    assert!(!edited.is_error, "the edit must succeed: {edited:?}");
    git(&root, &["add", "retry.py"]);
    let before = commit_count(&root);

    let result = GitTool
        .execute(json!({
            "op": "commit",
            "message": "retry: return 2",
            "allow_default_branch": true,
            "cwd": root.to_string_lossy(),
        }))
        .await;

    assert!(!result.is_error, "the commit must run: {}", result.content);
    assert_eq!(commit_count(&root), before + 1, "a commit must exist");
}

/// `git stash push` through the tool is the same act as through the shell, and
/// INV-2 counts a stash that grew as the work not being left alone.
#[tokio::test]
async fn stash_save_refuses_while_the_users_unsaved_work_is_in_the_tree() {
    let (_keep, root) = repo();

    let result = GitTool
        .execute(json!({"op": "stash_save", "cwd": root.to_string_lossy()}))
        .await;

    assert!(result.is_error, "the stash must be refused: {result:?}");
    assert!(
        git(&root, &["stash", "list"]).trim().is_empty(),
        "the stash must still be empty"
    );
    assert!(
        std::fs::read_to_string(root.join("README.md"))
            .unwrap()
            .contains(USER_LINE),
        "the user's line must still be on disk"
    );
}

/// Job corpus row A-8's own shape, and the wrong refusal it produced.
///
/// Mid-merge the pinned commit is `HEAD`, so every line the other side brings
/// in is "on disk and in no commit" — including a whole file `HEAD` has never
/// seen. Reading only the pin, the tool refused to stage the resolution and
/// the row was left with an uncommitted merge in the work tree, which is
/// exactly the failure the row grades. `MERGE_HEAD` has to count as recorded.
#[tokio::test]
async fn a_merge_resolution_is_stageable_even_though_head_has_never_seen_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(&root.join("retry.py"), "def fetch():\n    return 1\n").unwrap();
    git(&root, &["add", "retry.py"]);
    git(&root, &["commit", "-qm", "base"]);

    // The other side adds a file main has never recorded, plus a line in one
    // main does.
    git(&root, &["checkout", "-q", "-b", "feature"]);
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("tests/test_retry_after.py"),
        "def test_header_is_obeyed():\n    assert True\n",
    )
    .unwrap();
    std::fs::write(
        root.join("retry.py"),
        "def fetch():\n    honour_retry_after()\n    return 1\n",
    )
    .unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "feature work"]);
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(
        root.join("retry.py"),
        "def fetch():\n    back_off_with_jitter()\n    return 1\n",
    )
    .unwrap();
    git(&root, &["add", "retry.py"]);
    git(&root, &["commit", "-qm", "main work"]);

    // The merge conflicts, as the row's does.
    let merged = Command::new("git")
        .args(["merge", "--no-commit", "feature"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        !merged.status.success(),
        "the fixture must actually conflict, or this arm proves nothing"
    );
    // The resolution, written the way a resolution is written.
    std::fs::write(
        root.join("retry.py"),
        "def fetch():\n    back_off_with_jitter()\n    honour_retry_after()\n    return 1\n",
    )
    .unwrap();

    let staged_result = GitTool
        .execute(json!({
            "op": "add_paths",
            "paths": ["retry.py", "tests/test_retry_after.py"],
            "cwd": root.to_string_lossy(),
        }))
        .await;
    assert!(
        !staged_result.is_error,
        "a merge resolution must be stageable: {}",
        staged_result.content
    );
    assert!(
        staged(&root).contains("tests/test_retry_after.py"),
        "the incoming file must be in the index, found: {:?}",
        staged(&root)
    );

    let committed = GitTool
        .execute(json!({
            "op": "commit",
            "message": "Merge feature into main",
            "allow_default_branch": true,
            "cwd": root.to_string_lossy(),
        }))
        .await;
    assert!(
        !committed.is_error,
        "the merge commit must land: {}",
        committed.content
    );
    assert!(
        !root.join(".git/MERGE_HEAD").exists(),
        "the merge must not still be in progress"
    );
}

/// The positive control for the arm above: mid-merge, a file that is the
/// user's own unsaved work — recorded by neither side — is still refused.
/// Without this, teaching the guard about `MERGE_HEAD` could have disarmed it
/// for the whole duration of a merge and this suite would not have noticed.
#[tokio::test]
async fn a_merge_does_not_disarm_the_guard_for_the_users_own_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("retry.py"), "def fetch():\n    return 1\n").unwrap();
    git(&root, &["add", "retry.py"]);
    git(&root, &["commit", "-qm", "base"]);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(root.join("retry.py"), "def fetch():\n    return 2\n").unwrap();
    git(&root, &["add", "retry.py"]);
    git(&root, &["commit", "-qm", "feature"]);
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(root.join("retry.py"), "def fetch():\n    return 3\n").unwrap();
    git(&root, &["add", "retry.py"]);
    git(&root, &["commit", "-qm", "main"]);
    let merged = Command::new("git")
        .args(["merge", "--no-commit", "feature"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(!merged.status.success(), "the fixture must conflict");

    // Neither side has ever heard of this; it is the user's.
    std::fs::write(
        root.join("scratch.md"),
        "notes the user has not saved anywhere else\ncounter=41\n",
    )
    .unwrap();

    let result = GitTool
        .execute(json!({
            "op": "add_paths",
            "paths": ["scratch.md"],
            "cwd": root.to_string_lossy(),
        }))
        .await;

    assert!(
        result.is_error,
        "a merge must not disarm the guard: {}",
        result.content
    );
    assert!(
        !staged(&root).contains("scratch.md"),
        "the user's file must not be in the index, found: {:?}",
        staged(&root)
    );
}
