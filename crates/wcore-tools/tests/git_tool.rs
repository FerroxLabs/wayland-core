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

// ── A-4: reviewing a pull request means seeing what it changed ──────────────

/// Build the shape the A-4 job corpus row hands the agent: a `main` commit and
/// a branch on top of it that rewrites one file.
async fn repo_with_a_branch(tmp: &std::path::Path) {
    make_repo(tmp).await;
    std::fs::write(tmp.join("limiter.py"), "def allow():\n    return True\n").unwrap();
    git_in(tmp, &["add", "-A"]);
    git_in(tmp, &["commit", "-qm", "fixed window"]);
    git_in(tmp, &["branch", "-M", "main"]);
    git_in(tmp, &["checkout", "-qb", "pr/sliding-window"]);
    std::fs::write(
        tmp.join("limiter.py"),
        "def allow():\n    # sliding window\n    return False\n",
    )
    .unwrap();
    git_in(tmp, &["add", "-A"]);
    git_in(tmp, &["commit", "-qm", "sliding window"]);
}

/// `diff` must be able to name a revision.
///
/// Measured (job corpus A-4, Linux, sealed binary): under the STRICT sandbox
/// `git` cannot run from Bash at all, so this tool is the only git surface a
/// contained session has. Asked to review a pull request, the agent called
/// `diff` with the branch name and got an empty result every time — a
/// revision handed to `path` lands after `--` as a pathspec, matches no file,
/// and exits 0. It never saw the pull request and burned every turn it had
/// without leaving the user a review.
#[tokio::test]
async fn diff_can_name_the_revision_a_pull_request_is_against() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_branch(tmp.path()).await;
    let cwd = tmp.path().to_str().unwrap();
    let tool = GitTool;

    for rev in ["main", "main...HEAD", "main..pr/sliding-window"] {
        let result = tool
            .run_op(json!({"op": "diff", "rev": rev, "cwd": cwd}))
            .await;
        assert!(
            !result.is_error,
            "diff rev={rev:?} errored: {}",
            result.content
        );
        assert!(
            result.content.contains("sliding window"),
            "diff rev={rev:?} must show what the branch changed, but the \
             caller was handed {:?} — an empty diff reads as 'this branch \
             changed nothing'",
            result.content
        );
    }
}

/// The control, and the reason `rev` is a separate field: `path` stays a
/// pathspec. A caller narrowing a revision diff to one file must still get
/// only that file, and a `path` that happens to look like a branch name must
/// never be silently promoted to a revision.
#[tokio::test]
async fn path_remains_a_pathspec_and_narrows_the_revision_diff() {
    let tmp = tempfile::tempdir().unwrap();
    repo_with_a_branch(tmp.path()).await;
    let cwd = tmp.path().to_str().unwrap();
    std::fs::write(tmp.path().join("README.md"), "docs\n").unwrap();
    git_in(tmp.path(), &["add", "-A"]);
    git_in(tmp.path(), &["commit", "-qm", "docs"]);
    let tool = GitTool;

    let narrowed = tool
        .run_op(json!({"op": "diff", "rev": "main", "path": "limiter.py", "cwd": cwd}))
        .await;
    assert!(!narrowed.is_error, "{}", narrowed.content);
    assert!(
        narrowed.content.contains("limiter.py") && !narrowed.content.contains("README.md"),
        "path must still narrow to a pathspec: {}",
        narrowed.content
    );

    // `path: "main"` is a file that does not exist, not the branch. An empty
    // diff is the CORRECT answer here; promoting it to a revision would be a
    // silent reinterpretation of the caller's argument.
    let as_path = tool
        .run_op(json!({"op": "diff", "path": "main", "cwd": cwd}))
        .await;
    assert!(
        as_path.content.trim().is_empty(),
        "`path` must stay a pathspec: {}",
        as_path.content
    );

    // An option-shaped revision is refused rather than handed to git.
    let injected = tool
        .run_op(json!({"op": "diff", "rev": "--output=/tmp/pwned", "cwd": cwd}))
        .await;
    assert!(
        injected.is_error && injected.content.starts_with("Git: revision"),
        "an option-shaped revision must be refused by name, never handed to \
         git as a flag: {}",
        injected.content
    );
}
