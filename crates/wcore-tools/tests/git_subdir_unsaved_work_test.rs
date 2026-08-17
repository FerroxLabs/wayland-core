//! INV-2 when the session is working in a SUBDIRECTORY of the repository.
//!
//! Every path `git status --porcelain` and `git diff --cached --name-only`
//! print is relative to the REPOSITORY ROOT, never to the directory git was
//! run from. A guard that resolves them against its own `cwd` therefore looks
//! for `<cwd>/<repo-relative path>`, finds nothing there, reads every file as
//! holding no unsaved work, and passes silently — a total fail-open of the
//! whole surface for any session whose cwd is not the repository root.
//!
//! `git add -A`, `git commit` and `git reset --hard` are all repository-wide
//! from a subdirectory, so the blast radius is the entire tree.
//!
//! Each arm is graded from the world after the call — `git log --name-only`,
//! or the bytes on disk — never from the tool's own message. The first arm is
//! the POSITIVE CONTROL: the identical shape driven from the repository root,
//! where the guard is known to fire. It has to pass for the failing arms
//! below to mean anything, because a probe that cannot observe the guard
//! working would "fail" against a correct implementation too.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::git::GitTool;
use wcore_tools::workspace_policy::WorkspacePolicy;

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

/// Every path any commit reachable from HEAD has ever recorded.
fn committed_paths(root: &Path) -> String {
    git(root, &["log", "--pretty=format:", "--name-only"])
}

fn staged(root: &Path) -> String {
    git(root, &["diff", "--cached", "--name-only"])
}

/// A repository whose only committed file lives in `pkg/`, plus the user's
/// own untracked scratch file beside it. Nothing in this session wrote either.
///
/// Returns (keep-alive, repository root, the `pkg` subdirectory).
fn repo_with_subdir() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    let pkg = root.join("pkg");
    std::fs::create_dir(&pkg).unwrap();
    std::fs::write(pkg.join("app.py"), "def a():\n    return 1\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "base"]);
    // The user's own uncommitted scratch file, never named by anyone.
    std::fs::write(
        pkg.join("notes.txt"),
        format!("{USER_LINE}\nring Ana back\n"),
    )
    .unwrap();
    (dir, root, pkg)
}

/// POSITIVE CONTROL. The identical `add_all` shape, driven from the repository
/// root, where the guard is known to fire. If this arm does not refuse, the
/// probe cannot observe the defect and none of the arms below are evidence of
/// anything.
#[tokio::test]
async fn positive_control_add_all_from_the_repo_root_refuses() {
    let (_keep, root, _pkg) = repo_with_subdir();

    let added = GitTool
        .execute(json!({"op": "add_all", "cwd": root.to_string_lossy()}))
        .await;
    assert!(
        added.is_error,
        "POSITIVE CONTROL BROKEN: add_all from the repo root must refuse: {added:?}"
    );
    assert!(
        added.content.contains("notes.txt") && added.content.contains(USER_LINE),
        "the refusal must name the file and quote the line: {}",
        added.content
    );
    assert!(
        staged(&root).trim().is_empty(),
        "nothing may have been staged, found: {:?}",
        staged(&root)
    );
}

/// POSITIVE CONTROL for the stash route, for the same reason.
#[tokio::test]
async fn positive_control_stash_save_from_the_repo_root_refuses() {
    let (_keep, root, pkg) = repo_with_subdir();
    std::fs::write(
        pkg.join("app.py"),
        format!("def a():\n    return 1\n{USER_LINE}\n"),
    )
    .unwrap();

    let stashed = GitTool
        .execute(json!({"op": "stash_save", "cwd": root.to_string_lossy()}))
        .await;
    assert!(
        stashed.is_error,
        "POSITIVE CONTROL BROKEN: stash from the repo root must refuse: {stashed:?}"
    );
    let on_disk = std::fs::read_to_string(pkg.join("app.py")).unwrap();
    assert!(
        on_disk.contains(USER_LINE),
        "the line must still be on disk: {on_disk:?}"
    );
}

/// THE DEFECT. The same shape with `cwd` one directory down. `git add -A` from
/// a subdirectory stages the WHOLE repository, so the user's untracked scratch
/// file goes into the commit exactly as it does from the root.
#[tokio::test]
async fn add_all_from_a_subdirectory_refuses() {
    let (_keep, root, pkg) = repo_with_subdir();

    let added = GitTool
        .execute(json!({"op": "add_all", "cwd": pkg.to_string_lossy()}))
        .await;
    let committed = GitTool
        .execute(json!({
            "op": "commit",
            "message": "wire up the retry",
            "allow_default_branch": true,
            "cwd": pkg.to_string_lossy(),
        }))
        .await;

    assert!(
        !committed_paths(&root).contains("notes.txt"),
        "the user's unsaved file was committed on their behalf from a subdirectory.\n\
         add_all said: {}\ncommit said: {}\ncommitted: {:?}",
        added.content,
        committed.content,
        committed_paths(&root)
    );
}

/// The `commit` route on its own: the index was filled somewhere else, and the
/// commit op is the irreversible step that has to look at it. Driven from the
/// subdirectory.
#[tokio::test]
async fn commit_from_a_subdirectory_refuses_an_index_holding_unsaved_work() {
    let (_keep, root, pkg) = repo_with_subdir();
    // Staged outside the tool, which is the case the Index check exists for.
    git(&root, &["add", "-A"]);

    let committed = GitTool
        .execute(json!({
            "op": "commit",
            "message": "wire up the retry",
            "allow_default_branch": true,
            "cwd": pkg.to_string_lossy(),
        }))
        .await;

    assert!(
        !committed_paths(&root).contains("notes.txt"),
        "the user's unsaved file was committed from a subdirectory.\n\
         commit said: {}\ncommitted: {:?}",
        committed.content,
        committed_paths(&root)
    );
}

/// `stash_save` takes the whole work tree away, from whatever directory it is
/// called in.
#[tokio::test]
async fn stash_save_from_a_subdirectory_refuses() {
    let (_keep, root, pkg) = repo_with_subdir();
    // A stash without -u leaves untracked files alone, so the work at risk
    // here is an uncommitted line in a TRACKED file.
    std::fs::write(
        pkg.join("app.py"),
        format!("def a():\n    return 1\n{USER_LINE}\n"),
    )
    .unwrap();

    let stashed = GitTool
        .execute(json!({"op": "stash_save", "cwd": pkg.to_string_lossy()}))
        .await;

    let on_disk = std::fs::read_to_string(pkg.join("app.py")).unwrap();
    assert!(
        on_disk.contains(USER_LINE),
        "the user's uncommitted line was stashed away from a subdirectory.\n\
         stash said: {}\non disk: {on_disk:?}",
        stashed.content
    );
    assert!(
        !root.join(".git").join("refs").join("stash").exists(),
        "a stash must not have been created"
    );
}

/// The shell surface has the same shape, reached through `git -C <subdir>`.
///
/// A workspace granted at the repository root is the ordinary case, and the
/// sandbox requires it — a workspace rooted at `pkg/` puts `.git` out of
/// reach and git will not run at all. `git -C pkg reset --hard` is therefore
/// the shell instance of this defect that a real session can actually issue:
/// the guard follows `-C` to `pkg/` (deliberately, so it judges the tree git
/// will act on), enumerates the work tree from there, and gets back
/// repo-root-relative paths it then resolves against `pkg/`. The reset itself
/// is repository-wide.
///
/// The first two calls are controls: one proving the shell really runs git
/// here, one proving the guard fires on the same command without `-C`.
#[tokio::test]
async fn bash_reset_hard_via_dash_c_into_a_subdirectory_refuses() {
    let (_keep, root, pkg) = repo_with_subdir();
    std::fs::write(
        pkg.join("app.py"),
        format!("def a():\n    return 1\n{USER_LINE}\n"),
    )
    .unwrap();
    let ctx =
        ToolContext::test_default().with_workspace(Arc::new(WorkspacePolicy::trusted_local(&root)));

    // LIVENESS CONTROL. If the shell cannot run git here at all, the arm below
    // would "pass" because nothing happened rather than because the guard fired.
    let alive = BashTool
        .execute_with_ctx(json!({"command": "git -C pkg status --porcelain"}), &ctx)
        .await;
    assert!(
        !alive.is_error && alive.content.contains("pkg/app.py"),
        "LIVENESS CONTROL BROKEN: the shell must actually run git here: {alive:?}"
    );

    // POSITIVE CONTROL. The same command without `-C` — where the guard's cwd
    // already is the repository root — must refuse.
    let control = BashTool
        .execute_with_ctx(json!({"command": "git reset --hard"}), &ctx)
        .await;
    assert!(
        control.is_error && control.content.contains(USER_LINE),
        "POSITIVE CONTROL BROKEN: a reset from the repo root must refuse: {control:?}"
    );

    let result = BashTool
        .execute_with_ctx(json!({"command": "git -C pkg reset --hard"}), &ctx)
        .await;

    let on_disk = std::fs::read_to_string(pkg.join("app.py")).unwrap();
    assert!(
        on_disk.contains(USER_LINE),
        "the user's uncommitted line was destroyed by a reset aimed at a subdirectory.\n\
         bash said: {}\non disk: {on_disk:?}",
        result.content
    );
}

/// WRONG-DIRECTION CONTROL. The guard's own promise is that it never refuses
/// work it should allow. A commit of a file this session really did change,
/// driven from the subdirectory, with the user's untracked scratch file
/// sitting right beside it and NOT staged, has to go through.
#[tokio::test]
async fn a_legitimate_commit_from_a_subdirectory_still_succeeds() {
    let (_keep, root, pkg) = repo_with_subdir();
    let target = pkg.join("app.py");

    let edited = wcore_tools::edit::EditTool::new(None)
        .execute(json!({
            "file_path": target.to_string_lossy(),
            "old_string": "    return 1",
            "new_string": "    return 2  # fixed",
        }))
        .await;
    assert!(!edited.is_error, "the edit must succeed: {edited:?}");

    let added = GitTool
        .execute(json!({
            "op": "add_paths",
            "paths": ["app.py"],
            "cwd": pkg.to_string_lossy(),
        }))
        .await;
    assert!(
        !added.is_error,
        "staging the file the agent changed must not be refused: {}",
        added.content
    );

    let committed = GitTool
        .execute(json!({
            "op": "commit",
            "message": "return 2",
            "allow_default_branch": true,
            "cwd": pkg.to_string_lossy(),
        }))
        .await;
    assert!(
        !committed.is_error,
        "the commit must not be refused: {}",
        committed.content
    );
    assert!(
        committed_paths(&root).contains("pkg/app.py"),
        "the agent's own change must actually be committed, found: {:?}",
        committed_paths(&root)
    );
    assert!(
        !committed_paths(&root).contains("notes.txt"),
        "the user's scratch file must not have ridden along: {:?}",
        committed_paths(&root)
    );
}
