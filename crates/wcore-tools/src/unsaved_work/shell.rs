//! The shell half of the INV-2 / P2 unsaved-work guard.
//!
//! Split out of `unsaved_work.rs` to keep that file from growing further,
//! matching the existing `unsaved_work/tests.rs` pattern. The doctrine — what
//! this covers, what it deliberately does not, and why the error has to fall
//! towards not firing — is stated once, under "The shell surface" in the
//! parent module's documentation. Its unit tests live beside the rest of the
//! guard's in `unsaved_work/tests.rs`, which owns the git fixtures.

use std::path::{Path, PathBuf};

use super::{MAX_QUOTED_LINES, UnsavedWorkGuard, git_run, repository_marker_present};

/// Git subcommands that throw away uncommitted changes in the work tree.
///
/// Deliberately short. Every entry here is a command whose *purpose* is to
/// discard, so a refusal cannot be mistaken for over-reach. `git commit`,
/// `git add`, `git switch -c` and the rest are untouched.
const DISCARDING_SUBCOMMANDS: [&str; 5] = ["checkout", "restore", "stash", "clean", "reset"];

/// The refusal a shell caller should return, or `None` when nothing the
/// command would discard is unsaved.
///
/// `cwd` is the directory the shell will run in, because that is what git
/// resolves the command's relative paths against. The scope this covers, and
/// the scope it does not, is stated under "The shell surface" in the module
/// documentation.
pub fn shell_refusal(command: &str, cwd: &Path) -> Option<String> {
    // A discard outside a work tree discards nothing — git refuses to run at
    // all. Answered from the filesystem marker rather than a git exit code,
    // for the reason `resolve_baseline` gives.
    if !repository_marker_present(cwd) {
        return None;
    }
    // The one process-wide guard, so this answers from the same pinned
    // baseline and the same agent-authored tallies as Write and Edit.
    let guard = UnsavedWorkGuard::shared();
    let mut at_risk: Vec<(String, Vec<String>)> = Vec::new();
    for segment in shell_segments(command) {
        let Some((dir, paths)) = discarding_git_paths(&segment, cwd) else {
            continue;
        };
        let candidates = if paths.is_empty() {
            unsaved_work_tree_paths(&dir)
        } else {
            paths
        };
        for rel in candidates {
            if at_risk.iter().any(|(p, _)| *p == rel) {
                continue;
            }
            let lines = guard.unsaved_lines(&dir.join(&rel));
            if !lines.is_empty() {
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
            // These go into the model's context, so they are scrubbed with the
            // same scrubber the Write refusals use.
            let shown = wcore_safety::PIIScrubber.scrub(line);
            detail.push_str(&format!("    {shown}\n"));
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

/// The directory a single segment's discarding git command runs in, and the
/// paths it would revert there.
///
/// `None` when the segment is not a discarding git command at all. An empty
/// path list when it is one that names no path, and therefore reaches
/// everything.
fn discarding_git_paths(segment: &str, cwd: &Path) -> Option<(PathBuf, Vec<String>)> {
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
    // Skip git's own pre-subcommand options (`--no-pager`, `-c k=v`, ...), but
    // not `-C`: git resolves every path after it against that directory, so a
    // guard that ignored it would answer about the wrong tree — and answering
    // about the wrong tree is how a guard produces a refusal that is simply
    // wrong.
    let mut dir = cwd.to_path_buf();
    let mut subcommand = None;
    while let Some(token) = tokens.next() {
        if token == "-C" {
            if let Some(target) = tokens.next() {
                dir = dir.join(target);
            }
            continue;
        }
        if token == "-c" {
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
            .filter(|t| dir.join(t).exists())
            .map(|t| t.to_string())
            .collect(),
    };
    // A pathspec of `.` is the whole tree, not a file called ".".
    if paths.iter().any(|p| p == "." || p == "*") {
        return Some((dir, Vec::new()));
    }
    // `reset --hard` and a bare `stash` take the whole work tree with no
    // pathspec at all. `checkout`, `restore` and `clean` given no resolvable
    // path are a branch or a no-op, and discard nothing.
    if paths.is_empty() && !matches!(subcommand, "reset" | "stash") {
        return None;
    }
    Some((dir, paths))
}

/// Every tracked path under `dir` that git reports as modified.
///
/// Used only when the command names no path and therefore reaches the whole
/// work tree. Routed through [`git_run`] like every other git call here, so
/// an ambient `GIT_DIR` or `GIT_WORK_TREE` cannot point the enumeration at a
/// different tree than the one the guard then judges.
fn unsaved_work_tree_paths(dir: &Path) -> Vec<String> {
    let Some(run) = git_run(
        dir,
        &["status", "--porcelain", "-z", "--untracked-files=no"],
        None,
    ) else {
        return Vec::new();
    };
    if !run.ok() {
        return Vec::new();
    }
    let text = run.stdout_text();
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    let mut paths = Vec::new();
    while let Some(entry) = fields.next() {
        // `XY <path>`: two status codes, a space, then the path. A rename or a
        // copy is followed by a second NUL-separated field holding the
        // original path, which is a value and not an entry. Sliced with `get`
        // rather than `[3..]`, which panics when byte 3 of a non-ASCII path
        // lands mid-character.
        let (Some(status), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        if status.contains('R') || status.contains('C') {
            fields.next();
        }
        if !path.is_empty() {
            paths.push(path.to_owned());
        }
    }
    paths
}
