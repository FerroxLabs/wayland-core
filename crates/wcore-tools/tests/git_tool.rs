//! A1 GitTool smoke tests against a tmp repo.

use std::process::Command;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::git::GitTool;

/// Drive `git` directly via Command::new with `.current_dir(tmp)` — no
/// shell interpreter involved. The previous approach used
/// `shell_command("cd '$tmp' && git init && ...")` which is unix-only:
/// single-quotes don't quote in `cmd /C` on Windows, so the entire
/// command line broke (CI run 25955844929). Per AGENTS.md "Centralize
/// Platform Differences" — direct argv invocation of `git` (not a
/// shell) is the canonical cross-platform pattern.
fn git_in(cwd: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed in {cwd:?}");
}

async fn make_repo(tmp: &std::path::Path) {
    git_in(tmp, &["init", "-q"]);
    git_in(tmp, &["config", "user.email", "a@b"]);
    git_in(tmp, &["config", "user.name", "A B"]);
}

#[tokio::test]
async fn git_status_on_empty_repo() {
    let tmp = tempfile::tempdir().unwrap();
    make_repo(tmp.path()).await;
    let tool = GitTool;
    let result = tool
        .run_op(json!({"op": "status", "cwd": tmp.path().to_str().unwrap()}))
        .await;
    assert!(
        !result.is_error,
        "expected success, got: {}",
        result.content
    );
}

trait RunOp {
    async fn run_op(&self, input: serde_json::Value) -> wcore_types::tool::ToolResult;
}
impl RunOp for GitTool {
    async fn run_op(&self, input: serde_json::Value) -> wcore_types::tool::ToolResult {
        self.execute(input).await
    }
}

#[tokio::test]
async fn git_log_empty_returns_clean_error_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    make_repo(tmp.path()).await;
    let tool = GitTool;
    let _ = tool
        .run_op(json!({"op": "log", "limit": 5, "cwd": tmp.path().to_str().unwrap()}))
        .await;
}

#[test]
fn read_only_ops_are_concurrency_safe() {
    let tool = GitTool;
    assert!(tool.is_concurrency_safe(&json!({"op": "status"})));
    assert!(tool.is_concurrency_safe(&json!({"op": "log"})));
    assert!(tool.is_concurrency_safe(&json!({"op": "diff"})));
    assert!(tool.is_concurrency_safe(&json!({"op": "branch_current"})));
    assert!(tool.is_concurrency_safe(&json!({"op": "branch_list"})));
    assert!(!tool.is_concurrency_safe(&json!({"op": "commit", "message": "x"})));
    assert!(!tool.is_concurrency_safe(&json!({"op": "add_paths", "paths": ["a"]})));
    assert!(!tool.is_concurrency_safe(&json!({"op": "branch_checkout", "name": "main"})));
    assert!(!tool.is_concurrency_safe(&json!({"op": "stash_save"})));
}

#[test]
fn git_category_is_exec() {
    use wcore_protocol::events::ToolCategory;
    let tool = GitTool;
    assert!(matches!(tool.category(), ToolCategory::Exec));
}

#[test]
fn git_name_and_schema() {
    let tool = GitTool;
    assert_eq!(tool.name(), "Git");
    let schema = tool.input_schema();
    let required = schema.get("required").and_then(|v| v.as_array()).unwrap();
    assert_eq!(required[0], "op");
}

#[tokio::test]
async fn missing_op_field_returns_error() {
    let tool = GitTool;
    let result = tool.run_op(json!({})).await;
    assert!(result.is_error);
    assert!(result.content.contains("'op'"));
}

#[tokio::test]
async fn commit_with_empty_message_returns_error() {
    let tool = GitTool;
    let result = tool.run_op(json!({"op": "commit", "message": ""})).await;
    assert!(result.is_error);
    assert!(result.content.contains("non-empty"));
}

#[tokio::test]
async fn unknown_op_returns_error() {
    let tool = GitTool;
    let result = tool.run_op(json!({"op": "rewrite_history"})).await;
    assert!(result.is_error);
    assert!(result.content.contains("unknown op"));
}

#[tokio::test]
async fn add_paths_empty_array_returns_error() {
    let tool = GitTool;
    let result = tool.run_op(json!({"op": "add_paths", "paths": []})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn git_log_with_commit_returns_subject() {
    let tmp = tempfile::tempdir().unwrap();
    make_repo(tmp.path()).await;
    let file_path = tmp.path().join("a.txt");
    std::fs::write(&file_path, "x").unwrap();
    let cwd = tmp.path().to_str().unwrap();
    git_in(tmp.path(), &["add", "a.txt"]);
    git_in(tmp.path(), &["commit", "-q", "-m", "initial"]);

    let tool = GitTool;
    let result = tool
        .run_op(json!({"op": "log", "limit": 5, "cwd": cwd}))
        .await;
    assert!(!result.is_error, "log failed: {}", result.content);
    assert!(
        result.content.contains("initial"),
        "missing commit subject in log: {}",
        result.content
    );
}
