//! P5 / corpus row A-2 — the Git tool has to be able to put work on its own
//! branch, get that branch onto the remote, and open a pull request for it.
//!
//! The A-2 corpus row measured a session that WANTED a branch, asked for one,
//! was told `fatal: … a branch '--' cannot be created from it`, and fell back
//! to committing on `main` — leaving the user nothing to review and nothing to
//! revert. Both `branch_checkout` forms were broken:
//!
//! * `create: true`  → `git checkout -b -- <name>`, so `-b` consumed the `--`
//!   as the new branch name and `<name>` became the start point.
//! * `create: false` → `git checkout -- <name>`, which is a pathspec restore,
//!   not a branch switch — it never changed HEAD.
//!
//! These tests fail against the pre-fix tool and pass after it, and they cover
//! the two ops that were missing entirely (`push`, `pr_create`) plus the
//! default-branch refusal that stops the trunk being the path of least
//! resistance.

use std::path::Path;
use std::process::Command;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::git::GitTool;

fn git_in(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed in {cwd:?}");
}

fn git_out(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repo on `main` with one commit, so HEAD is born and branchable.
fn repo_with_a_commit(dir: &Path) {
    git_in(dir, &["init", "-q", "-b", "main"]);
    git_in(dir, &["config", "user.email", "a@b"]);
    git_in(dir, &["config", "user.name", "A B"]);
    std::fs::write(dir.join("a.txt"), "x\n").unwrap();
    git_in(dir, &["add", "a.txt"]);
    git_in(dir, &["commit", "-q", "-m", "initial"]);
}

fn head_branch(dir: &Path) -> String {
    git_out(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
}

#[tokio::test]
async fn branch_checkout_create_actually_creates_the_branch() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap();

    let result = GitTool
        .execute(json!({
            "op": "branch_checkout",
            "cwd": cwd,
            "name": "fix/412-blank-lines",
            "create": true,
        }))
        .await;

    assert!(
        !result.is_error,
        "creating a branch must succeed, got: {}",
        result.content
    );
    assert_eq!(
        head_branch(tmp.path()),
        "fix/412-blank-lines",
        "HEAD did not move onto the new branch"
    );
}

#[tokio::test]
async fn branch_checkout_without_create_actually_switches_branch() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap();
    git_in(tmp.path(), &["branch", "sidebar"]);

    let result = GitTool
        .execute(json!({"op": "branch_checkout", "cwd": cwd, "name": "sidebar"}))
        .await;

    assert!(
        !result.is_error,
        "switching to an existing branch must succeed, got: {}",
        result.content
    );
    assert_eq!(head_branch(tmp.path()), "sidebar", "HEAD did not switch");
}

/// A branch that shares its name with a tracked file must still be read as a
/// branch — that is what the trailing `--` is for.
#[tokio::test]
async fn branch_name_that_is_also_a_filename_switches_the_branch() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap();
    std::fs::write(tmp.path().join("release"), "notes\n").unwrap();
    git_in(tmp.path(), &["add", "release"]);
    git_in(tmp.path(), &["commit", "-q", "-m", "add release file"]);
    git_in(tmp.path(), &["branch", "release"]);

    let result = GitTool
        .execute(json!({"op": "branch_checkout", "cwd": cwd, "name": "release"}))
        .await;

    assert!(
        !result.is_error,
        "ambiguous branch/file name must resolve to the branch: {}",
        result.content
    );
    assert_eq!(head_branch(tmp.path()), "release");
}

#[tokio::test]
async fn option_shaped_branch_name_is_refused_before_git_runs() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap();

    let result = GitTool
        .execute(json!({"op": "branch_checkout", "cwd": cwd, "name": "-f", "create": true}))
        .await;

    assert!(result.is_error);
    assert!(
        result.content.contains("must not"),
        "expected a refusal naming the shape, got: {}",
        result.content
    );
    assert_eq!(head_branch(tmp.path()), "main", "HEAD must not have moved");
}

#[tokio::test]
async fn commit_onto_the_default_branch_is_refused_and_names_the_way_out() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "y\n").unwrap();
    GitTool.execute(json!({"op": "add_all", "cwd": cwd})).await;

    let result = GitTool
        .execute(json!({"op": "commit", "cwd": cwd, "message": "fix: thing"}))
        .await;

    assert!(result.is_error, "committing onto main must be refused");
    assert!(
        result.content.contains("branch_checkout")
            && result.content.contains("allow_default_branch"),
        "the refusal must name both ways forward, got: {}",
        result.content
    );
    assert_eq!(
        git_out(tmp.path(), &["rev-list", "--count", "HEAD"]),
        "1",
        "no commit may have been created"
    );
}

#[tokio::test]
async fn commit_onto_the_default_branch_proceeds_with_explicit_intent() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "y\n").unwrap();
    GitTool.execute(json!({"op": "add_all", "cwd": cwd})).await;

    let result = GitTool
        .execute(json!({
            "op": "commit",
            "cwd": cwd,
            "message": "fix: thing",
            "allow_default_branch": true,
        }))
        .await;

    assert!(
        !result.is_error,
        "explicit intent must be honoured: {}",
        result.content
    );
    assert_eq!(git_out(tmp.path(), &["rev-list", "--count", "HEAD"]), "2");
}

#[tokio::test]
async fn commit_on_a_feature_branch_is_not_gated() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap();
    git_in(tmp.path(), &["checkout", "-q", "-b", "fix/412", "--"]);
    std::fs::write(tmp.path().join("a.txt"), "y\n").unwrap();
    GitTool.execute(json!({"op": "add_all", "cwd": cwd})).await;

    let result = GitTool
        .execute(json!({"op": "commit", "cwd": cwd, "message": "fix: thing"}))
        .await;

    assert!(
        !result.is_error,
        "feature-branch commit must not be gated: {}",
        result.content
    );
    assert_eq!(git_out(tmp.path(), &["rev-list", "--count", "HEAD"]), "2");
}

/// `origin/HEAD` is authoritative when the remote published it: a repo whose
/// trunk is called something unconventional is still protected, and a branch
/// that merely LOOKS conventional is not.
#[tokio::test]
async fn the_remotes_published_head_decides_which_branch_is_the_trunk() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let bare = tmp.path().join("origin.git");
    std::fs::create_dir_all(&repo).unwrap();
    git_in(
        tmp.path(),
        &["init", "--bare", "-q", "-b", "release-line", "origin.git"],
    );
    repo_with_a_commit(&repo);
    git_in(&repo, &["branch", "-m", "main", "release-line"]);
    git_in(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git_in(&repo, &["push", "-q", "origin", "release-line"]);
    git_in(&repo, &["remote", "set-head", "origin", "release-line"]);
    let cwd = repo.to_str().unwrap();

    std::fs::write(repo.join("a.txt"), "y\n").unwrap();
    GitTool.execute(json!({"op": "add_all", "cwd": cwd})).await;
    let refused = GitTool
        .execute(json!({"op": "commit", "cwd": cwd, "message": "fix: thing"}))
        .await;
    assert!(
        refused.is_error,
        "the published trunk must be protected even when it is not called main: {}",
        refused.content
    );

    // `main` here is NOT the trunk — origin/HEAD says otherwise.
    git_in(&repo, &["checkout", "-q", "-b", "main", "--"]);
    let allowed = GitTool
        .execute(json!({"op": "commit", "cwd": cwd, "message": "fix: thing"}))
        .await;
    assert!(
        !allowed.is_error,
        "a branch that only looks conventional must not be gated: {}",
        allowed.content
    );
}

#[tokio::test]
async fn push_puts_the_branch_on_the_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let bare = tmp.path().join("origin.git");
    std::fs::create_dir_all(&repo).unwrap();
    git_in(
        tmp.path(),
        &["init", "--bare", "-q", "-b", "main", "origin.git"],
    );
    repo_with_a_commit(&repo);
    git_in(&repo, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git_in(&repo, &["push", "-q", "origin", "main"]);
    let cwd = repo.to_str().unwrap();

    let created = GitTool
        .execute(json!({"op": "branch_checkout", "cwd": cwd, "name": "fix/412", "create": true}))
        .await;
    assert!(!created.is_error, "branch: {}", created.content);
    std::fs::write(repo.join("a.txt"), "fixed\n").unwrap();
    GitTool.execute(json!({"op": "add_all", "cwd": cwd})).await;
    let committed = GitTool
        .execute(json!({"op": "commit", "cwd": cwd, "message": "fix(#412): parse symbols"}))
        .await;
    assert!(!committed.is_error, "commit: {}", committed.content);

    let pushed = GitTool.execute(json!({"op": "push", "cwd": cwd})).await;
    assert!(!pushed.is_error, "push: {}", pushed.content);

    let refs = Command::new("git")
        .args([
            "--git-dir",
            bare.to_str().unwrap(),
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
        ])
        .output()
        .unwrap();
    let refs = String::from_utf8_lossy(&refs.stdout);
    assert!(
        refs.lines().any(|l| l.trim() == "fix/412"),
        "the branch never reached the remote, refs were: {refs}"
    );
    let blob = Command::new("git")
        .args(["--git-dir", bare.to_str().unwrap(), "show", "fix/412:a.txt"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&blob.stdout),
        "fixed\n",
        "the pushed branch does not carry the work"
    );
}

#[tokio::test]
async fn push_on_an_unborn_head_says_so_instead_of_guessing() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap();
    git_in(tmp.path(), &["checkout", "-q", "--detach"]);

    let result = GitTool.execute(json!({"op": "push", "cwd": cwd})).await;
    assert!(result.is_error);
    assert!(
        result.content.contains("detached"),
        "expected a detached-HEAD explanation, got: {}",
        result.content
    );
}

#[tokio::test]
async fn pr_create_requires_a_title() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_commit(tmp.path());
    let result = GitTool
        .execute(json!({"op": "pr_create", "cwd": tmp.path().to_str().unwrap()}))
        .await;
    assert!(result.is_error);
    assert!(result.content.contains("title"), "got: {}", result.content);
}

/// Opening a pull request from the trunk against the trunk is the failure the
/// A-2 row recorded; refuse it here rather than let the forge do it, so the
/// message names the missing step.
#[tokio::test]
async fn pr_create_refuses_when_head_and_base_are_the_same_branch() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_commit(tmp.path());
    let result = GitTool
        .execute(json!({
            "op": "pr_create",
            "cwd": tmp.path().to_str().unwrap(),
            "title": "fix #412",
        }))
        .await;
    assert!(result.is_error, "got: {}", result.content);
    assert!(
        result.content.contains("create a branch"),
        "expected the missing step to be named, got: {}",
        result.content
    );
}

#[test]
fn the_mutating_ops_are_not_concurrency_safe() {
    let tool = GitTool;
    assert!(!tool.is_concurrency_safe(&json!({"op": "push"})));
    assert!(!tool.is_concurrency_safe(&json!({"op": "pr_create", "title": "x"})));
}

#[test]
fn the_schema_and_description_advertise_the_review_route() {
    let tool = GitTool;
    let schema = tool.input_schema();
    let props = schema.get("properties").unwrap();
    for field in [
        "remote",
        "branch",
        "title",
        "body",
        "base",
        "allow_default_branch",
    ] {
        assert!(props.get(field).is_some(), "schema is missing {field}");
    }
    let desc = tool.description();
    for token in ["push", "pr_create", "allow_default_branch"] {
        assert!(desc.contains(token), "description never mentions {token}");
    }
}
