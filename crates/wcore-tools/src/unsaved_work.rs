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
//!
//! P2b — the same work, thrown away through the shell instead.
//!
//! Measured defect (job corpus row B-1, 2026-08-11, case `k5-after`): the agent
//! finished the job, noticed it had touched `SHIPPING-API.md`, and tidied up
//! with `git checkout -- SHIPPING-API.md`. That file also carried a line the
//! user had never committed anywhere, and the revert took it. Write's guard saw
//! nothing because Write was never called. So the same question — "would this
//! destroy a line that exists nowhere else?" — is asked of a shell command that
//! discards the work tree, by [`shell_refusal`], before the shell is spawned.
//!
//! The guard is shared with Write through [`shared`]: a file Write created this
//! session has an empty baseline, so the agent is never blocked from reverting
//! its own new file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

/// The process-wide guard.
///
/// Write and Bash must agree about what counts as unsaved, and Write's
/// first-touch baselines are what keep the agent's own new files unprotected
/// against its own later revert. Two independent instances would not share
/// them.
pub fn shared() -> &'static UnsavedWorkGuard {
    static GUARD: OnceLock<UnsavedWorkGuard> = OnceLock::new();
    GUARD.get_or_init(UnsavedWorkGuard::new)
}

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

    /// Record the baseline for `path` without judging any content.
    ///
    /// Called when Write is creating a file rather than replacing one: the
    /// baseline of a file that does not exist yet is empty, and memoizing that
    /// now is what keeps the agent's own later rewrites of its own file free.
    pub fn observe(&self, path: &Path) {
        let _ = self.baseline(path);
    }

    /// Lines of unsaved user work in `path`, i.e. every line a wholesale
    /// revert of that path would destroy.
    ///
    /// This is [`Self::dropped_lines`] against content that keeps nothing,
    /// which is exactly what `git checkout --`, `git restore`, `git stash` and
    /// `git clean` do to a path.
    pub fn unsaved_lines(&self, path: &Path) -> Vec<String> {
        self.dropped_lines(path, "")
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

/// Git subcommands that throw away uncommitted changes in the work tree.
///
/// Deliberately short. Every entry here is a command whose *purpose* is to
/// discard, so a refusal cannot be mistaken for over-reach. `git commit`,
/// `git add`, `git switch -c` and the rest are untouched.
const DISCARDING_SUBCOMMANDS: [&str; 5] = ["checkout", "restore", "stash", "clean", "reset"];

/// The refusal a shell caller should return, or `None` when nothing the
/// command discards is unsaved.
///
/// `cwd` is the directory the shell will run in, because that is what git
/// resolves the command's relative paths against.
///
/// Scope, stated as honestly as Write's guard:
/// * Only the git subcommands in [`DISCARDING_SUBCOMMANDS`] are inspected, and
///   `reset` only in its `--hard` form — a mixed or soft reset keeps the work
///   tree.
/// * A discarding command that names paths is judged on those paths. One that
///   names none (`git checkout -- .`, `git stash`, `git reset --hard`) reaches
///   the whole work tree and is judged on every file that has unsaved work.
/// * Outside a git work tree there is no baseline, so nothing is claimed and
///   nothing is blocked.
pub fn shell_refusal(command: &str, cwd: &Path) -> Option<String> {
    let guard = shared();
    let mut at_risk: Vec<(String, Vec<String>)> = Vec::new();
    for segment in shell_segments(command) {
        let Some(paths) = discarding_git_paths(&segment, cwd) else {
            continue;
        };
        let candidates = if paths.is_empty() {
            unsaved_work_tree_paths(cwd)
        } else {
            paths
        };
        for rel in candidates {
            let abs = cwd.join(&rel);
            let lines = guard.unsaved_lines(&abs);
            if !lines.is_empty() && !at_risk.iter().any(|(p, _)| *p == rel) {
                at_risk.push((rel, lines));
            }
        }
    }
    if at_risk.is_empty() {
        return None;
    }
    let mut detail = String::new();
    for (path, lines) in &at_risk {
        detail.push_str(&format!("\n  {path}\n"));
        for line in lines.iter().take(MAX_QUOTED_LINES) {
            detail.push_str(&format!("    {line}\n"));
        }
        let more = lines.len().saturating_sub(MAX_QUOTED_LINES);
        if more > 0 {
            detail.push_str(&format!("    ... and {more} more line(s)\n"));
        }
    }
    Some(format!(
        "Refused to run this command: it discards uncommitted changes, and {n} file(s) below \
         hold line(s) that are on disk but in no commit. That is unsaved work which exists \
         nowhere else, so throwing it away is irreversible.\n\
         At risk:{detail}\
         If you only need to undo your OWN change, revert just the part you changed with Edit. \
         If the user really does want these lines gone, say what will be lost and let them \
         confirm before you run it.",
        n = at_risk.len(),
    ))
}

/// Split a command line into the segments a shell would run separately.
///
/// Enough to find a `git` invocation after `&&`, `||`, `;`, `|` or a newline.
/// It is not a shell parser and does not need to be: a discarding git command
/// hidden from this split still has to survive every other guard, and the
/// worst case is the refusal not firing, never a wrong refusal.
fn shell_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ';' | '\n' | '|' | '&' => {
                if (c == '|' || c == '&') && chars.peek() == Some(&c) {
                    chars.next();
                }
                segments.push(std::mem::take(&mut current));
            }
            _ => current.push(c),
        }
    }
    segments.push(current);
    segments
}

/// The paths a single segment's discarding git command would revert.
///
/// `None` when the segment is not a discarding git command at all. `Some(vec![])`
/// when it is one that names no path, and therefore reaches everything.
fn discarding_git_paths(segment: &str, cwd: &Path) -> Option<Vec<String>> {
    let mut tokens = segment
        .split_whitespace()
        .map(|t| t.trim_matches(|c| c == '"' || c == '\''))
        .filter(|t| !t.is_empty())
        .peekable();

    // Skip a leading `sudo` and any VAR=value assignments.
    while let Some(token) = tokens.peek() {
        if *token == "sudo" || (token.contains('=') && !token.starts_with('-')) {
            tokens.next();
        } else {
            break;
        }
    }
    let program = tokens.next()?;
    if program != "git" && !program.ends_with("/git") {
        return None;
    }
    // Skip git's own pre-subcommand options (`-C dir`, `--no-pager`, ...).
    let mut subcommand = None;
    while let Some(token) = tokens.next() {
        if token == "-C" || token == "-c" {
            tokens.next();
            continue;
        }
        if token.starts_with('-') {
            continue;
        }
        subcommand = Some(token);
        break;
    }
    let subcommand = subcommand?;
    if !DISCARDING_SUBCOMMANDS.contains(&subcommand) {
        return None;
    }

    let rest: Vec<&str> = tokens.collect();
    // A reset that keeps the work tree discards nothing on disk.
    if subcommand == "reset" && !rest.contains(&"--hard") {
        return None;
    }
    // `git stash list|show|pop|apply|drop` reads or restores; only a push
    // (the bare form, or an explicit push/save) takes the work tree away.
    if subcommand == "stash"
        && let Some(first) = rest.iter().find(|t| !t.starts_with('-'))
        && !matches!(*first, "push" | "save")
    {
        return None;
    }

    // Everything after a `--` is a pathspec, unambiguously.
    let explicit = rest.iter().position(|t| *t == "--");
    let paths: Vec<String> = match explicit {
        Some(i) => rest[i + 1..].iter().map(|t| t.to_string()).collect(),
        // Without `--` the operand is ambiguous: `git checkout foo` is a
        // branch switch when `foo` is a branch and a revert when it is a file.
        // Resolve it by asking the disk, which is the same thing git does. A
        // branch switch is NOT a discard — git refuses one that would lose
        // uncommitted work — so an operand that is not a path leaves nothing
        // to check.
        None => rest
            .iter()
            .filter(|t| !t.starts_with('-'))
            .filter(|t| cwd.join(t).exists())
            .map(|t| t.to_string())
            .collect(),
    };
    // A pathspec of `.` is the whole tree, not a file called ".".
    if paths.iter().any(|p| p == "." || p == "*") {
        return Some(Vec::new());
    }
    // `reset --hard` and a bare `stash` take the whole work tree with no
    // pathspec at all. `checkout`, `restore` and `clean` given no resolvable
    // path are a branch or a no-op, and discard nothing.
    if paths.is_empty() && !matches!(subcommand, "reset" | "stash") {
        return None;
    }
    Some(paths)
}

/// Every tracked path under `cwd` that git reports as modified.
///
/// Used only when the command names no path and therefore reaches the whole
/// work tree. Empty outside a work tree.
fn unsaved_work_tree_paths(cwd: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .args(["status", "--porcelain", "-z", "--untracked-files=no"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|entry| entry.len() > 3)
        .map(|entry| entry[3..].to_string())
        .collect()
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

    // ---- P2b: the same work discarded through the shell -------------------

    /// The measured B-1 defect, exactly: the job is done, the agent tidies up
    /// with `git checkout --` on a file that also carries the user's uncommitted
    /// line, and the line is gone. The refusal must fire and must name the line,
    /// because a refusal that does not say what is at risk teaches nothing.
    #[test]
    fn shell_refusal_blocks_git_checkout_of_a_file_holding_unsaved_work() {
        let (dir, _file) = repo_with_unsaved_line();
        let root = dir.path();
        let refusal = shell_refusal("git checkout -- file.py", root)
            .expect("reverting a file with an uncommitted line must be refused");
        assert!(refusal.contains("file.py"), "must name the file: {refusal}");
        assert!(
            refusal.contains("# WIP do not touch"),
            "must quote the line at risk: {refusal}"
        );
    }

    /// The same command reaching the whole tree by naming no path at all.
    #[test]
    fn shell_refusal_blocks_whole_tree_discards() {
        let (dir, _file) = repo_with_unsaved_line();
        let root = dir.path();
        for command in [
            "git checkout -- .",
            "git reset --hard",
            "git stash",
            "git restore .",
        ] {
            assert!(
                shell_refusal(command, root).is_some(),
                "{command:?} discards the whole work tree and must be refused"
            );
        }
    }

    /// Found after a `&&`, and with git reached by absolute path — the two
    /// dodges a single-token check would miss.
    #[test]
    fn shell_refusal_sees_past_chaining_and_an_absolute_git() {
        let (dir, _file) = repo_with_unsaved_line();
        let root = dir.path();
        assert!(
            shell_refusal("echo hi && git checkout -- file.py", root).is_some(),
            "a discard after && must still be refused"
        );
        assert!(
            shell_refusal("/usr/bin/git checkout -- file.py", root).is_some(),
            "an absolute git path must still be refused"
        );
    }

    /// The guard must not become a general git ban. Every command here either
    /// keeps the work tree or does not touch it, and blocking one would make
    /// the refusal noise rather than signal.
    #[test]
    fn shell_refusal_leaves_non_discarding_git_alone() {
        let (dir, _file) = repo_with_unsaved_line();
        let root = dir.path();
        for command in [
            "git status",
            "git add file.py",
            "git commit -m x",
            "git checkout -b feature",
            "git reset HEAD file.py",
            "git reset --soft HEAD~1",
            "git stash list",
            "git stash pop",
            "git log --oneline",
            "git diff",
        ] {
            assert!(
                shell_refusal(command, root).is_none(),
                "{command:?} does not discard the work tree and must be allowed"
            );
        }
    }

    /// A discard aimed at a different file is not the user's problem.
    #[test]
    fn shell_refusal_ignores_a_path_with_nothing_unsaved() {
        let (dir, _file) = repo_with_unsaved_line();
        let root = dir.path();
        std::fs::write(root.join("clean.py"), "x = 1\n").unwrap();
        git(root, &["add", "clean.py"]);
        git(root, &["commit", "-q", "-m", "clean"]);
        assert!(
            shell_refusal("git checkout -- clean.py", root).is_none(),
            "a file with no uncommitted line has nothing to lose"
        );
    }

    /// Outside a work tree there is no baseline, so no protection is claimed.
    #[test]
    fn shell_refusal_stands_down_outside_a_git_work_tree() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("file.py"), "line\n").unwrap();
        assert!(
            shell_refusal("git checkout -- file.py", dir.path()).is_none(),
            "with no git baseline the guard must not guess"
        );
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
