//! INV-2 — no rewrite may drop content that was on disk when the job started,
//! and the guarantee holds at the tool layer rather than by model cooperation.
//!
//! Reproduces the shape measured by the job corpus on 2026-08-10 (Linux A-2,
//! Windows A-2 / A-8): the user leaves an in-progress line on disk, the agent
//! is legitimately asked to change that same file, and rewrites it wholesale.
//!
//! Round 2 adds the routes the round-1 adversarial seat measured going around
//! that refusal — Edit, and a mid-session commit — plus the over-refusal it
//! measured the guard causing (`/root/adv-armB`: a plain "rewrite notes.md as
//! a runbook" was refused and the task was never accomplished).
//!
//! Round 3 moves the recovery copy into the repository's own object store and
//! makes every claim of recoverability an exercised one, so these tests
//! recover the bytes with `git cat-file` rather than trusting a sentence.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use wcore_tools::Tool;
use wcore_tools::edit::EditTool;
use wcore_tools::unsaved_work::UnsavedWorkGuard;
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

/// A workspace plus the one guard both its tools share, exactly as the product
/// shares one guard across Write, Edit and every sub-agent.
struct Ws {
    dir: TempDir,
    guard: Arc<UnsavedWorkGuard>,
}

impl Ws {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "user@example.com"]);
        git(dir.path(), &["config", "user.name", "user"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        Self {
            dir,
            guard: Arc::new(UnsavedWorkGuard::new_isolated()),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }
    fn writer(&self) -> WriteTool {
        WriteTool::new(None).with_unsaved_guard(self.guard.clone())
    }
    fn editor(&self) -> EditTool {
        EditTool::new(None).with_unsaved_guard(self.guard.clone())
    }
    fn put(&self, name: &str, body: &str) -> PathBuf {
        let p = self.root().join(name);
        std::fs::write(&p, body).unwrap();
        p
    }
    fn text(&self, name: &str) -> String {
        std::fs::read_to_string(self.root().join(name)).unwrap()
    }
}

/// The corpus fixture: `parser.py` committed, then an unsaved line appended.
fn workspace_with_unsaved_work() -> (Ws, PathBuf) {
    let ws = Ws::new();
    ws.put("parser.py", COMMITTED);
    git(ws.root(), &["add", "parser.py"]);
    git(ws.root(), &["commit", "-qm", "initial parser"]);
    let file = ws.put("parser.py", &format!("{COMMITTED}{UNSAVED_LINE}\n"));
    (ws, file)
}

/// Recover the bytes a tool result claims are recoverable, by running the
/// result's own recovery command. A test that only matched the sentence would
/// have passed against round 2's false snapshot claim.
fn recovered(repo: &Path, result: &str) -> String {
    let marker = "cat-file blob ";
    let start = result
        .find(marker)
        .unwrap_or_else(|| panic!("result names no recovery object: {result}"))
        + marker.len();
    let oid = result[start..]
        .split_whitespace()
        .next()
        .expect("result terminates the object id");
    let out = Command::new("git")
        .args(["cat-file", "blob", oid])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "the result's own recovery command failed: {result}"
    );
    String::from_utf8(out.stdout).unwrap()
}

// ---------------------------------------------------------------------------
// The measured defect, and the round-1 behaviour that must not regress.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wholesale_rewrite_that_drops_the_users_unsaved_line_is_refused() {
    let (ws, file) = workspace_with_unsaved_work();
    let before = ws.text("parser.py");

    let result = ws
        .writer()
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
        ws.text("parser.py"),
        before,
        "a refused write must leave the file byte-identical"
    );
}

#[tokio::test]
async fn the_same_rewrite_is_allowed_once_it_carries_the_unsaved_line_through() {
    let (ws, file) = workspace_with_unsaved_work();
    let fixed = format!(
        "def parse(text):\n    return [l.strip() for l in text.splitlines() if l.strip()]\n{UNSAVED_LINE}\n"
    );
    let result = ws
        .writer()
        .execute(json!({"file_path": file.to_str().unwrap(), "content": fixed.clone()}))
        .await;
    assert!(
        !result.is_error,
        "expected success, got: {}",
        result.content
    );
    assert_eq!(ws.text("parser.py"), fixed);
}

#[tokio::test]
async fn creating_a_new_file_in_a_repo_is_untouched_by_the_guard() {
    let ws = Ws::new();
    let fresh = ws.root().join("brand_new.py");
    let tool = ws.writer();

    let first = tool
        .execute(json!({"file_path": fresh.to_str().unwrap(), "content": "v1\n"}))
        .await;
    assert!(!first.is_error, "create failed: {}", first.content);

    // A file this session created is the agent's own work, so rewriting it
    // must stay free — and must not leave a recovery snapshot behind either.
    let second = tool
        .execute(json!({"file_path": fresh.to_str().unwrap(), "content": "v2\n"}))
        .await;
    assert!(!second.is_error, "rewrite failed: {}", second.content);
    assert!(
        !second.content.contains("copied to"),
        "the agent's own file needs no snapshot: {}",
        second.content
    );
    assert_eq!(ws.text("brand_new.py"), "v2\n");
}

#[tokio::test]
async fn a_committed_file_with_no_unsaved_work_may_be_rewritten_wholesale() {
    let ws = Ws::new();
    let file = ws.put("clean.py", COMMITTED);
    git(ws.root(), &["add", "clean.py"]);
    git(ws.root(), &["commit", "-qm", "clean"]);

    let result = ws
        .writer()
        .execute(json!({"file_path": file.to_str().unwrap(), "content": "totally different\n"}))
        .await;
    assert!(
        !result.is_error,
        "a clean tree must not be blocked, got: {}",
        result.content
    );
    assert!(
        !result.content.contains("copied to"),
        "committed content is already recoverable from git: {}",
        result.content
    );
    assert_eq!(ws.text("clean.py"), "totally different\n");
}

// ---------------------------------------------------------------------------
// Residual 2 — a commit made during the session must not disarm the guard.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_mid_session_commit_does_not_disarm_the_guard() {
    let (ws, file) = workspace_with_unsaved_work();
    let tool = ws.writer();
    let rewrite = json!({"file_path": file.to_str().unwrap(), "content": "def parse(text):\n    return []\n"});

    // Pin the baseline the way session start does.
    let first = tool.execute(rewrite.clone()).await;
    assert!(first.is_error, "control: {}", first.content);

    // The A-2 agent's documented habit: commit straight onto main. Under a
    // live `HEAD` baseline this puts the user's line into "saved" and the next
    // rewrite drops it silently.
    git(ws.root(), &["add", "-A"]);
    git(ws.root(), &["commit", "-qm", "wip"]);

    let after = tool.execute(rewrite).await;
    assert!(
        after.is_error && after.content.contains(UNSAVED_LINE),
        "a commit must not launder unsaved work into the baseline, got: {}",
        after.content
    );
    assert!(ws.text("parser.py").contains(UNSAVED_LINE));
}

/// The same defence, on the path where the blob cache cannot cover for it.
///
/// Once a file's recorded contents are cached under the pinned commit, later
/// calls never ask git again — so a test that touches one file before and
/// after the commit still passes even if the lookup has been switched to live
/// HEAD. Measured: the mutation of the pinned commit survived this suite until
/// this test existed. Pin on one file, then write a different one for the
/// first time after the commit.
#[tokio::test]
async fn a_file_first_written_after_a_mid_session_commit_is_judged_against_the_pin() {
    let ws = Ws::new();
    ws.put("seed.py", "seed = 1\n");
    ws.put("other.py", COMMITTED);
    git(ws.root(), &["add", "seed.py", "other.py"]);
    git(ws.root(), &["commit", "-qm", "initial"]);

    // Session start pins here, through a file that is not the one under test.
    let seed = ws.root().join("seed.py");
    let pinned = ws
        .writer()
        .execute(json!({"file_path": seed.to_str().unwrap(), "content": "seed = 1\n"}))
        .await;
    assert!(!pinned.is_error, "got: {}", pinned.content);

    // The user's unsaved line, then the A-2 agent committing straight to main.
    let other = ws.put("other.py", &format!("{COMMITTED}{UNSAVED_LINE}\n"));
    git(ws.root(), &["add", "other.py"]);
    git(ws.root(), &["commit", "-qm", "wip"]);

    let result = ws
        .writer()
        .execute(json!({"file_path": other.to_str().unwrap(),
                        "content": "def parse(text):\n    return []\n"}))
        .await;
    assert!(
        result.is_error && result.content.contains(UNSAVED_LINE),
        "a commit made during the session must not launder unsaved work into \
         the baseline: {}",
        result.content
    );
}

#[tokio::test]
async fn dropping_the_file_from_the_index_does_not_disarm_the_guard() {
    let (ws, file) = workspace_with_unsaved_work();
    let tool = ws.writer();
    // Round 1 resolved the file's committed path through `git ls-files`, so an
    // empty index made the whole file look unrecorded.
    git(ws.root(), &["rm", "-q", "--cached", "parser.py"]);
    let result = tool
        .execute(json!({"file_path": file.to_str().unwrap(), "content": "def parse(text):\n    return []\n"}))
        .await;
    assert!(
        result.is_error && result.content.contains(UNSAVED_LINE),
        "got: {}",
        result.content
    );
    assert!(
        !result.content.contains("def parse(text):"),
        "the committed body is still recorded and must not be cited: {}",
        result.content
    );
}

// ---------------------------------------------------------------------------
// Residual 1 — Edit is the whole-file-equivalent route the model actually took.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_refusal_does_not_teach_the_caller_another_tool_to_use() {
    let (ws, file) = workspace_with_unsaved_work();
    let result = ws
        .writer()
        .execute(json!({"file_path": file.to_str().unwrap(), "content": "def parse(text):\n    return []\n"}))
        .await;
    assert!(result.is_error);
    assert!(
        !result.content.contains("Edit"),
        "round 1 shipped an error string recommending the unguarded path: {}",
        result.content
    );
}

#[tokio::test]
async fn editing_out_the_line_a_write_refusal_named_preserves_it_elsewhere() {
    let (ws, file) = workspace_with_unsaved_work();
    let before = ws.text("parser.py");

    let refused = ws
        .writer()
        .execute(json!({"file_path": file.to_str().unwrap(), "content": "def parse(text):\n    return []\n"}))
        .await;
    assert!(refused.is_error);

    // `/root/adv-armB`, exactly: having been refused, delete the named line
    // with Edit instead. That must no longer make the content unrecoverable.
    let edited = ws
        .editor()
        .execute(json!({
            "file_path": file.to_str().unwrap(),
            "old_string": format!("{UNSAVED_LINE}\n"),
            "new_string": "",
        }))
        .await;
    assert!(!edited.is_error, "got: {}", edited.content);
    assert!(
        !ws.text("parser.py").contains(UNSAVED_LINE),
        "the edit itself should have applied"
    );
    assert!(
        edited.content.contains("in no commit"),
        "the result must say unsaved work left the file: {}",
        edited.content
    );
    assert_eq!(
        recovered(ws.root(), &edited.content),
        before,
        "the pre-edit bytes must be recoverable by the command the result names"
    );
}

#[tokio::test]
async fn an_edit_that_leaves_unsaved_work_alone_says_nothing() {
    let (ws, file) = workspace_with_unsaved_work();
    let edited = ws
        .editor()
        .execute(json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "return [l for l in text.splitlines() if l]",
            "new_string": "return list(text.splitlines())",
        }))
        .await;
    assert!(!edited.is_error, "got: {}", edited.content);
    assert!(
        !edited.content.contains("Note:"),
        "no unsaved work moved, so there is nothing to report: {}",
        edited.content
    );
    assert!(ws.text("parser.py").contains(UNSAVED_LINE));
}

#[tokio::test]
async fn an_edit_to_the_users_own_uncommitted_line_is_never_refused() {
    let (ws, file) = workspace_with_unsaved_work();
    // A dirty tree is the commonest working state there is. Refusing here
    // would make Edit unusable, which is its own defect.
    let edited = ws
        .editor()
        .execute(json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "do not touch",
            "new_string": "do not touch (reworded)",
        }))
        .await;
    assert!(!edited.is_error, "got: {}", edited.content);
    assert!(ws.text("parser.py").contains("(reworded)"));
}

// ---------------------------------------------------------------------------
// Residual 3 — the over-refusal, and what replaces it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rewriting_a_pre_existing_untracked_file_completes_and_is_recoverable() {
    // `/root/adv-armB`: "rewrite notes.md as a runbook". Round 1 refused this
    // and the task was never accomplished.
    let ws = Ws::new();
    ws.put("app.py", "print(1)\n");
    git(ws.root(), &["add", "app.py"]);
    git(ws.root(), &["commit", "-qm", "init"]);
    let notes = "# Deploy notes\nold step 1: ssh to the box\nold step 2: restart by hand\n";
    let file = ws.put("notes.md", notes);

    let runbook = "# Deploy Runbook\n\n1. `make deploy`\n2. verify health\n";
    let result = ws
        .writer()
        .execute(json!({"file_path": file.to_str().unwrap(), "content": runbook}))
        .await;

    assert!(
        !result.is_error,
        "a wholesale rewrite of a file recorded nowhere is the request, not an accident: {}",
        result.content
    );
    assert_eq!(ws.text("notes.md"), runbook, "the task must complete");
    assert_eq!(
        recovered(ws.root(), &result.content),
        notes,
        "and the prior contents must still be recoverable"
    );
}

#[tokio::test]
async fn the_recovery_copy_is_not_written_into_the_users_work_tree() {
    let ws = Ws::new();
    let file = ws.put("notes.md", "user notes nobody committed\n");
    let result = ws
        .writer()
        .execute(json!({"file_path": file.to_str().unwrap(), "content": "replaced\n"}))
        .await;
    assert!(!result.is_error, "got: {}", result.content);

    // Uninvited files in the user's tree are an open corpus defect against
    // this product; a recovery copy must not add to it.
    let tracked_dirty = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(ws.root())
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&tracked_dirty.stdout);
    assert!(
        listing.lines().all(|l| l.ends_with("notes.md")),
        "the only change in the work tree must be the file asked for: {listing}"
    );
}

/// The shipped guard must not create a recovery store of its own anywhere.
///
/// Round 2's did: `~/.wayland/unsaved-work/<start>-<pid>`, holding the prior
/// bytes in clear, hardened by a `#[cfg(not(unix))]` no-op on Windows, and
/// never garbage collected. Running the test suite alone left 6 session
/// directories and 21 plaintext files on the build host.
#[tokio::test]
async fn the_shipped_guard_creates_no_store_of_its_own() {
    let legacy = wcore_config::config::profile_home().join("unsaved-work");
    let existed_before = legacy.exists();

    let ws = Ws::new();
    let file = ws.put("notes.md", "user notes nobody committed\n");
    let tool = WriteTool::new(None).with_unsaved_guard(UnsavedWorkGuard::shared());
    let result = tool
        .execute(json!({"file_path": file.to_str().unwrap(), "content": "replaced\n"}))
        .await;

    assert!(!result.is_error, "got: {}", result.content);
    assert_eq!(
        recovered(ws.root(), &result.content),
        "user notes nobody committed\n",
        "the copy must live in the repository the file belongs to"
    );
    assert_eq!(
        legacy.exists(),
        existed_before,
        "the guard must not create {}",
        legacy.display()
    );
}

#[tokio::test]
async fn a_partial_drop_is_still_refused_even_when_most_of_the_file_goes() {
    // The discriminator is a property of the file's prior state, not of what
    // the model chooses to write: a rewrite sharing nothing with the original
    // is still refused while any of the original is committed.
    let (ws, file) = workspace_with_unsaved_work();
    let result = ws
        .writer()
        .execute(json!({
            "file_path": file.to_str().unwrap(),
            "content": "import re\n\n\ndef totally_new():\n    return None\n",
        }))
        .await;
    assert!(
        result.is_error && result.content.contains(UNSAVED_LINE),
        "got: {}",
        result.content
    );
}

// ---------------------------------------------------------------------------
// Outside git.
// ---------------------------------------------------------------------------

/// Narrower than round 2, deliberately.
///
/// Round 2 allowed this against a plaintext copy under `~/.wayland`, which is
/// the store the adversarial seat broke. With no repository there is no object
/// store, so there is nowhere to put a copy, and allowing the drop would mean
/// claiming a recoverability that does not exist.
#[tokio::test]
async fn outside_a_git_repo_a_rewrite_that_drops_content_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("loose.txt");
    std::fs::write(&file, "whatever the user had\n").unwrap();
    let tool = WriteTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()));

    let result = tool
        .execute(json!({"file_path": file.to_str().unwrap(), "content": "replaced\n"}))
        .await;
    assert!(result.is_error, "got: {}", result.content);
    assert!(
        result.content.contains("in no repository"),
        "got: {}",
        result.content
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "whatever the user had\n",
        "a refusal must leave the file exactly as it was"
    );
}

/// ...and the same place stays fully usable for every write that does not drop
/// anything, which is what keeps this from being round 1's over-refusal.
#[tokio::test]
async fn outside_a_git_repo_a_rewrite_that_keeps_the_content_completes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("loose.txt");
    std::fs::write(&file, "whatever the user had\n").unwrap();
    let tool = WriteTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()));

    let kept = "whatever the user had\nplus a line the agent adds\n";
    let result = tool
        .execute(json!({"file_path": file.to_str().unwrap(), "content": kept}))
        .await;
    assert!(!result.is_error, "got: {}", result.content);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), kept);
}
