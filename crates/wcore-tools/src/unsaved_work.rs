//! P2 — protects work the user has not saved from a whole-file overwrite.
//!
//! Measured defect (job corpus, 2026-08-10): the user has an in-progress edit
//! on disk that is committed nowhere. The agent is legitimately asked to change
//! that same file, rewrites it wholesale through the Write tool from its own
//! picture of the contents, and the user's line is gone for good. Observed
//! three times across two platforms — Linux A-2 and Windows A-2 / A-8, on
//! `src/receipts/parser.py` and `retry.py`.
//!
//! A prompt cannot make a whole-file overwrite safe, so the check lives here at
//! the tool layer. Before Write replaces an existing file, any line that is on
//! disk but absent from the file's last **saved** version (its git HEAD blob)
//! counts as unsaved user work. Dropping such a line is refused, and the
//! refusal names the lines so the model can carry them through or switch to a
//! surgical Edit.
//!
//! Scope, stated honestly:
//! * Git is the oracle for "saved". Outside a git work tree there is no
//!   baseline to compare against, so no protection is claimed and none is
//!   applied — Write behaves exactly as it did before.
//! * The baseline for a path is captured once, the first time Write touches it,
//!   and reused for the rest of the session. A file the agent itself creates
//!   has an empty baseline, so repeated Writes to it are never blocked.
//! * Edit is not guarded: its deletions are named explicitly by the model in
//!   `old_string`, which is the opposite of an accidental wholesale rewrite.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// Most dropped lines quoted back in a refusal message.
const MAX_QUOTED_LINES: usize = 5;

/// Session-scoped record of unsaved user work, per file.
///
/// Cheap to construct and safe to share: one instance lives inside the Write
/// tool for the life of a session.
#[derive(Default)]
pub struct UnsavedWorkGuard {
    /// path -> the lines that were on disk but not in the last saved version,
    /// captured the first time Write touched that path.
    baselines: Mutex<HashMap<PathBuf, Vec<String>>>,
}

impl UnsavedWorkGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lines of unsaved user work in `path` that `new_content` would destroy.
    ///
    /// Empty when nothing is at risk: the file is new, it is not in a git work
    /// tree, it has no uncommitted content, or every uncommitted line survives
    /// into `new_content`.
    pub fn dropped_lines(&self, path: &Path, new_content: &str) -> Vec<String> {
        let unsaved = self.baseline(path);
        if unsaved.is_empty() {
            return Vec::new();
        }
        let surviving: HashSet<&str> = new_content.lines().map(str::trim).collect();
        unsaved
            .into_iter()
            .filter(|line| !surviving.contains(line.trim()))
            .collect()
    }

    /// The refusal a caller should return, or `None` when the write is safe.
    pub fn refusal(&self, path: &Path, display_path: &str, new_content: &str) -> Option<String> {
        let dropped = self.dropped_lines(path, new_content);
        if dropped.is_empty() {
            return None;
        }
        let quoted: Vec<String> = dropped
            .iter()
            .take(MAX_QUOTED_LINES)
            .map(|l| format!("    {l}"))
            .collect();
        let more = dropped.len().saturating_sub(quoted.len());
        let tail = if more > 0 {
            format!("\n    ... and {more} more line(s)")
        } else {
            String::new()
        };
        Some(format!(
            "Refused to overwrite {display_path}: this write would delete {n} line(s) that are \
             in the file on disk but not in its last committed version. That is unsaved work \
             which exists nowhere else, so losing it is irreversible.\n\
             Lines that would be lost:\n{quoted}{tail}\n\
             Read the file again, then either include these lines in the content you write, or \
             use the Edit tool to change only the part you mean to change. If the user did ask \
             for these lines to go, remove them explicitly with Edit and say so.",
            n = dropped.len(),
            quoted = quoted.join("\n"),
        ))
    }

    /// Unsaved lines for `path`, computed once and memoized for the session.
    fn baseline(&self, path: &Path) -> Vec<String> {
        let key = path.to_path_buf();
        if let Ok(map) = self.baselines.lock()
            && let Some(hit) = map.get(&key)
        {
            return hit.clone();
        }
        let computed = compute_unsaved_lines(path);
        if let Ok(mut map) = self.baselines.lock() {
            return map.entry(key).or_insert(computed).clone();
        }
        computed
    }
}

/// Lines present on disk at `path` but absent from its git HEAD blob.
///
/// Returns empty (no protection) when the answer cannot be established: the
/// file does not exist, is not UTF-8 text, or is not inside a git work tree.
fn compute_unsaved_lines(path: &Path) -> Vec<String> {
    let Ok(disk) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Some(saved) = saved_version(path) else {
        return Vec::new();
    };
    let saved_lines: HashSet<&str> = saved.lines().map(str::trim).collect();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut unsaved = Vec::new();
    for line in disk.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || saved_lines.contains(trimmed) {
            continue;
        }
        if seen.insert(trimmed) {
            unsaved.push(line.to_string());
        }
    }
    unsaved
}

/// The file's last saved content: its blob at git HEAD.
///
/// `Some("")` for a file inside a work tree that git has never recorded — none
/// of its content is saved anywhere, so all of it counts as unsaved. `None`
/// when the path is not inside a git work tree at all, which stands the guard
/// down rather than guessing.
fn saved_version(path: &Path) -> Option<String> {
    let dir = path.parent()?;
    let name = path.file_name()?.to_str()?;
    if git_output(dir, &["rev-parse", "--is-inside-work-tree"])?.trim() != "true" {
        return None;
    }
    // `--full-name` yields the repo-root-relative, forward-slashed path git
    // wants in a `HEAD:<path>` spec on every platform; `-z` keeps it unquoted.
    let listed = git_output(dir, &["ls-files", "--full-name", "-z", "--", name])?;
    let Some(rel) = listed.split('\0').find(|s| !s.is_empty()) else {
        // Inside a work tree but git has never recorded this file.
        return Some(String::new());
    };
    Some(git_output(dir, &["show", &format!("HEAD:{rel}")]).unwrap_or_default())
}

/// Run `git` in `dir`, returning stdout on success only.
///
/// `--literal-pathspecs` so an LLM-supplied file name containing `*`, `:` or a
/// leading `-` is never read as a pattern or an option. Argv mode throughout —
/// no shell is involved.
fn git_output(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("--literal-pathspecs")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git must be available for these tests");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo(root: &Path) {
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "t@example.com"]);
        git(root, &["config", "user.name", "t"]);
        git(root, &["config", "commit.gpgsign", "false"]);
    }

    /// A repo with `file.py` committed, then an extra uncommitted line on disk.
    fn repo_with_unsaved_line() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        init_repo(&root);
        let file = root.join("file.py");
        std::fs::write(&file, "def a():\n    return 1\n").unwrap();
        git(&root, &["add", "file.py"]);
        git(&root, &["commit", "-qm", "base"]);
        std::fs::write(&file, "def a():\n    return 1\n# WIP do not touch\n").unwrap();
        (dir, file)
    }

    #[test]
    fn rewrite_that_drops_the_unsaved_line_is_refused() {
        let (_dir, file) = repo_with_unsaved_line();
        let guard = UnsavedWorkGuard::new();
        let rewritten = "def a():\n    return 2\n";
        assert_eq!(
            guard.dropped_lines(&file, rewritten),
            vec!["# WIP do not touch".to_string()]
        );
        let msg = guard
            .refusal(&file, "file.py", rewritten)
            .expect("must refuse");
        assert!(msg.contains("# WIP do not touch"), "message: {msg}");
    }

    #[test]
    fn rewrite_that_carries_the_unsaved_line_through_is_allowed() {
        let (_dir, file) = repo_with_unsaved_line();
        let guard = UnsavedWorkGuard::new();
        let rewritten = "def a():\n    return 2\n# WIP do not touch\n";
        assert!(guard.dropped_lines(&file, rewritten).is_empty());
        assert!(guard.refusal(&file, "file.py", rewritten).is_none());
    }

    #[test]
    fn committed_lines_may_be_deleted_freely() {
        let (_dir, file) = repo_with_unsaved_line();
        let guard = UnsavedWorkGuard::new();
        // Drops the whole committed body but keeps the unsaved line.
        assert!(
            guard
                .dropped_lines(&file, "# WIP do not touch\n")
                .is_empty()
        );
    }

    #[test]
    fn a_file_the_agent_creates_is_never_protected() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        init_repo(&root);
        let file = root.join("new.txt");
        let guard = UnsavedWorkGuard::new();
        // First touch: the file does not exist yet, so the baseline is empty
        // and stays empty for the rest of the session.
        assert!(guard.dropped_lines(&file, "v1\n").is_empty());
        std::fs::write(&file, "v1\n").unwrap();
        assert!(guard.dropped_lines(&file, "v2\n").is_empty());
    }

    #[test]
    fn an_untracked_file_that_predates_the_agent_is_protected() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        init_repo(&root);
        std::fs::write(root.join("seed"), "x").unwrap();
        git(&root, &["add", "seed"]);
        git(&root, &["commit", "-qm", "base"]);
        let file = root.join("notes.md");
        std::fs::write(&file, "user notes nobody saved\n").unwrap();
        let guard = UnsavedWorkGuard::new();
        assert_eq!(
            guard.dropped_lines(&file, "replaced\n"),
            vec!["user notes nobody saved".to_string()]
        );
    }

    #[test]
    fn outside_a_git_work_tree_the_guard_stands_down() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("loose.txt");
        std::fs::write(&file, "some content\n").unwrap();
        let guard = UnsavedWorkGuard::new();
        assert!(guard.dropped_lines(&file, "different\n").is_empty());
    }

    #[test]
    fn blank_lines_are_not_treated_as_unsaved_work() {
        let (_dir, file) = repo_with_unsaved_line();
        std::fs::write(&file, "def a():\n    return 1\n\n\n").unwrap();
        let guard = UnsavedWorkGuard::new();
        assert!(
            guard
                .dropped_lines(&file, "def a():\n    return 2\n")
                .is_empty()
        );
    }

    #[test]
    fn a_moved_unsaved_line_still_counts_as_surviving() {
        let (_dir, file) = repo_with_unsaved_line();
        let guard = UnsavedWorkGuard::new();
        // Same line, different position and indentation-trimmed match.
        let rewritten = "# WIP do not touch\ndef a():\n    return 2\n";
        assert!(guard.dropped_lines(&file, rewritten).is_empty());
    }
}
