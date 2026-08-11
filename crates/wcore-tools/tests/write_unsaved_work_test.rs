//! P2 — the Write tool must not destroy work the user has not saved.
//!
//! Reproduces the shape measured by the job corpus on 2026-08-10 (Linux A-2,
//! Windows A-2 / A-8): the user leaves an in-progress line on disk, the agent
//! is legitimately asked to change that same file, and rewrites it wholesale.
//! Before the fix the line was silently gone; the tool must now refuse and say
//! why, while still allowing the rewrite that carries the line through.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::json;
use tempfile::TempDir;
use wcore_tools::Tool;
use wcore_tools::write::WriteTool;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git must be installed to run these tests");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// The corpus fixture in miniature: a committed parser plus an unsaved line the
/// user is in the middle of writing.
const COMMITTED: &str = "def parse(text):\n    return [l for l in text.splitlines() if l]\n";
const UNSAVED_LINE: &str = "# JOBCORPUS-UNSAVED-USER-WORK in-progress edit, do not touch";

fn workspace_with_unsaved_work() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "user@example.com"]);
    git(&root, &["config", "user.name", "user"]);
    git(&root, &["config", "commit.gpgsign", "false"]);

    let file = root.join("parser.py");
    std::fs::write(&file, COMMITTED).unwrap();
    git(&root, &["add", "parser.py"]);
    git(&root, &["commit", "-qm", "initial parser"]);

    // The user's editor buffer, flushed to disk but committed nowhere.
    std::fs::write(&file, format!("{COMMITTED}{UNSAVED_LINE}\n")).unwrap();
    (dir, file)
}

#[tokio::test]
async fn wholesale_rewrite_that_drops_the_users_unsaved_line_is_refused() {
    let (_dir, file) = workspace_with_unsaved_work();
    let before = std::fs::read_to_string(&file).unwrap();

    let tool = WriteTool::new(None);
    let result = tool
        .execute(json!({
            "file_path": file.to_str().unwrap(),
            "content": "def parse(text):\n    return [l.strip() for l in text.splitlines() if l.strip()]\n",
        }))
        .await;

    assert!(
        result.is_error,
        "the overwrite should have been refused, got: {}",
        result.content
    );
    assert!(
        result.content.contains(UNSAVED_LINE),
        "the refusal must name the line at risk, got: {}",
        result.content
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        before,
        "a refused write must leave the file byte-identical"
    );
}

#[tokio::test]
async fn the_same_rewrite_is_allowed_once_it_carries_the_unsaved_line_through() {
    let (_dir, file) = workspace_with_unsaved_work();

    let fixed = format!(
        "def parse(text):\n    return [l.strip() for l in text.splitlines() if l.strip()]\n{UNSAVED_LINE}\n"
    );
    let tool = WriteTool::new(None);
    let result = tool
        .execute(json!({
            "file_path": file.to_str().unwrap(),
            "content": fixed.clone(),
        }))
        .await;

    assert!(
        !result.is_error,
        "expected success, got: {}",
        result.content
    );
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert_eq!(on_disk, fixed);
    assert!(
        on_disk.contains(UNSAVED_LINE),
        "the user's unsaved line must still be there"
    );
}

#[tokio::test]
async fn creating_a_new_file_in_a_repo_is_untouched_by_the_guard() {
    let (dir, _file) = workspace_with_unsaved_work();
    let fresh = dir.path().join("brand_new.py");

    let tool = WriteTool::new(None);
    let first = tool
        .execute(json!({"file_path": fresh.to_str().unwrap(), "content": "v1\n"}))
        .await;
    assert!(!first.is_error, "create failed: {}", first.content);

    // A file this session created is the agent's own work, so rewriting it
    // must stay free.
    let second = tool
        .execute(json!({"file_path": fresh.to_str().unwrap(), "content": "v2\n"}))
        .await;
    assert!(!second.is_error, "rewrite failed: {}", second.content);
    assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "v2\n");
}

#[tokio::test]
async fn a_committed_file_with_no_unsaved_work_may_be_rewritten_wholesale() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "user@example.com"]);
    git(&root, &["config", "user.name", "user"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    let file = root.join("clean.py");
    std::fs::write(&file, COMMITTED).unwrap();
    git(&root, &["add", "clean.py"]);
    git(&root, &["commit", "-qm", "clean"]);

    let tool = WriteTool::new(None);
    let result = tool
        .execute(json!({"file_path": file.to_str().unwrap(), "content": "totally different\n"}))
        .await;
    assert!(
        !result.is_error,
        "a clean tree must not be blocked, got: {}",
        result.content
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "totally different\n"
    );
}

#[tokio::test]
async fn outside_a_git_repo_write_behaves_exactly_as_before() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("loose.txt");
    std::fs::write(&file, "whatever the user had\n").unwrap();

    let tool = WriteTool::new(None);
    let result = tool
        .execute(json!({"file_path": file.to_str().unwrap(), "content": "replaced\n"}))
        .await;
    assert!(!result.is_error, "got: {}", result.content);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "replaced\n");
}
