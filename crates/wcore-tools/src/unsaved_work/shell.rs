//! The shell half of the INV-2 / P2 unsaved-work guard.
//!
//! Split out of `unsaved_work.rs` to keep that file from growing further,
//! matching the existing `unsaved_work/tests.rs` pattern. The doctrine — what
//! this covers, what it deliberately does not, and why the error has to fall
//! towards not firing — is stated once, under "The shell surface" in the
//! parent module's documentation. Its unit tests live beside the rest of the
//! guard's in `unsaved_work/tests.rs`, which owns the git fixtures.

use std::path::{Path, PathBuf};

use super::git_ops::{files_under, ignored, quote_at_risk};
use super::{UnsavedWorkGuard, git_run, repository_marker_present, work_tree_root};

/// Git subcommands that throw away uncommitted changes in the work tree.
///
/// Deliberately short. Every entry here is a command whose *purpose* is to
/// discard, so a refusal cannot be mistaken for over-reach. `git commit`,
/// `git add`, `git switch -c` and the rest are untouched.
const DISCARDING_SUBCOMMANDS: [&str; 5] = ["checkout", "restore", "stash", "clean", "reset"];

/// Most files one `rm` operand is expanded to before the walk gives up.
///
/// `rm -rf node_modules` must not turn one guard call into a walk of a
/// hundred thousand files. Hitting the bound means the refusal may not fire
/// for the rest of that operand, which is the direction this whole surface
/// already chooses.
const RM_WALK_BUDGET: usize = 4096;

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
        if let Some((dir, paths)) = discarding_git_paths(&segment, cwd) {
            let candidates: Vec<(String, PathBuf)> = if paths.is_empty() {
                unsaved_work_tree_paths(&dir)
            } else {
                // Pathspecs the caller typed, which git resolves against the
                // directory it runs in — unlike anything git itself prints.
                paths
                    .into_iter()
                    .map(|p| {
                        let abs = dir.join(&p);
                        (p, abs)
                    })
                    .collect()
            };
            for (shown, abs) in candidates {
                if at_risk.iter().any(|(p, _)| *p == shown) {
                    continue;
                }
                let lines = guard.unsaved_lines(&abs);
                if !lines.is_empty() {
                    at_risk.push((shown, lines));
                }
            }
            continue;
        }
        for shown in truncating_targets(&segment) {
            let abs = cwd.join(&shown);
            if at_risk.iter().any(|(p, _)| *p == shown) {
                continue;
            }
            let lines = guard.unsaved_lines(&abs);
            if !lines.is_empty() {
                at_risk.push((shown, lines));
            }
        }
        for operand in removing_operands(&segment) {
            for file in files_under(&cwd.join(&operand), RM_WALK_BUDGET) {
                let shown = file
                    .strip_prefix(cwd)
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .into_owned();
                if at_risk.iter().any(|(p, _)| *p == shown) {
                    continue;
                }
                // Build output, caches and everything else the repository
                // itself says does not belong in it are not the user's
                // unsaved work, and refusing an `rm` of them would be the
                // wrong-refusal this surface is written to avoid.
                if ignored(&guard, &file) {
                    continue;
                }
                let lines = guard.unsaved_lines(&file);
                if !lines.is_empty() {
                    at_risk.push((shown, lines));
                }
            }
        }
    }
    if at_risk.is_empty() {
        return None;
    }
    Some(format!(
        "Refused to run this command: it destroys work-tree content, and {n} file(s) below hold \
         line(s) that are on disk but in no commit. That is unsaved work which exists nowhere \
         else, so throwing it away is irreversible.\n\
         At risk:{detail}\
         If you only need to undo your OWN change, revert just the part you changed with Edit. \
         If the user really does want these lines gone, say what will be lost and let them \
         confirm before you run it.",
        n = at_risk.len(),
        detail = quote_at_risk(&at_risk),
    ))
}

/// The paths an `rm` in this segment would remove, relative to the shell's
/// working directory.
///
/// Empty when the segment is not an `rm` at all. Deliberately covers exactly
/// one command: `rm` is the shape the module documentation named as escaping
/// this surface, and job corpus row A-2 (2026-08-11) is it arriving —
/// `rm -rf ... .jobcorpus-user-work` took a file the user had written and
/// never committed. `mv`, `truncate`, `sed -i` and shell redirection still
/// route around this and still cannot be seen from here.
///
/// No glob expansion: a pattern reaches the filesystem as a literal here and
/// resolves to nothing, so the refusal does not fire. That is the direction
/// this surface takes everywhere else.
/// The paths this segment would truncate in place, relative to the shell's
/// working directory.
///
/// Measured, job corpus row A-8 run `fix-r1`, 2026-08-12: Write refused the
/// rewrite of `retry.py` twice and the model rerouted to
/// `cat > retry.py << 'PYEOF'`, which took the user's in-progress line off
/// disk with no guard in the path. Closing Write and Edit without closing
/// this one only moves the destruction to the route that is still open.
///
/// Deliberately narrow, in the direction this surface always errs:
///
/// * `>` and `>|` truncate, so they are covered; `>>` appends and is not.
/// * `tee FILE` truncates; `tee -a FILE` appends and is not covered.
/// * `sed -i` / `perl -i` rewrite their operands in place.
/// * A redirection target that is not a plain relative or absolute path —
///   a process substitution, a variable, a glob, `/dev/null` — is skipped
///   rather than guessed at.
fn truncating_targets(segment: &str) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    let push = |raw: &str, targets: &mut Vec<String>| {
        let t = raw.trim().trim_matches(|c| c == '"' || c == '\'');
        if t.is_empty() || t.starts_with('$') || t.starts_with('&') || t.starts_with("/dev/") {
            return;
        }
        if t.contains('*') || t.contains('?') || t.contains('`') || t.contains('$') {
            return;
        }
        if !targets.iter().any(|e| e == t) {
            targets.push(t.to_owned());
        }
    };

    // Redirections. Scanned over the raw segment because the target is not
    // always whitespace-separated from the operator (`cat >file`).
    let bytes: Vec<char> = segment.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == '>' {
            // `>>` appends: skip both characters and the operand after them.
            if i + 1 < bytes.len() && bytes[i + 1] == '>' {
                i += 2;
                continue;
            }
            // `2>` and `1>` are the same truncation, and `>|` is the forced
            // form of it.
            let mut j = i + 1;
            if j < bytes.len() && bytes[j] == '|' {
                j += 1;
            }
            while j < bytes.len() && bytes[j] == ' ' {
                j += 1;
            }
            let start = j;
            while j < bytes.len() && !bytes[j].is_whitespace() && bytes[j] != ';' && bytes[j] != '|'
            {
                j += 1;
            }
            if j > start {
                let raw: String = bytes[start..j].iter().collect();
                push(&raw, &mut targets);
            }
            i = j.max(i + 1);
            continue;
        }
        i += 1;
    }

    // In-place rewriters, and `tee` without `-a`.
    let tokens: Vec<&str> = segment
        .split_whitespace()
        .map(|t| t.trim_matches(|c| c == '"' || c == '\''))
        .filter(|t| !t.is_empty())
        .collect();
    let mut idx = 0usize;
    while idx < tokens.len() {
        let program = tokens[idx];
        let base = program.rsplit('/').next().unwrap_or(program);
        if base == "tee" {
            let rest = &tokens[idx + 1..];
            let appending = rest
                .iter()
                .take_while(|t| t.starts_with('-'))
                .any(|t| *t == "-a" || *t == "--append");
            if !appending {
                for t in rest.iter().filter(|t| !t.starts_with('-')) {
                    push(t, &mut targets);
                }
            }
        } else if base == "sed" || base == "perl" {
            let rest = &tokens[idx + 1..];
            if rest
                .iter()
                .any(|t| t.starts_with("-i") || *t == "--in-place")
            {
                for t in rest.iter().filter(|t| !t.starts_with('-')) {
                    push(t, &mut targets);
                }
            }
        }
        idx += 1;
    }
    targets
}

fn removing_operands(segment: &str) -> Vec<String> {
    let mut tokens = segment
        .split_whitespace()
        .map(|t| t.trim_matches(|c| c == '"' || c == '\''))
        .filter(|t| !t.is_empty())
        .peekable();

    // Skip a leading `sudo` and any VAR=value assignments, exactly as the git
    // detector above does.
    while let Some(token) = tokens.peek() {
        if *token == "sudo" || (token.contains('=') && !token.starts_with('-')) {
            tokens.next();
        } else {
            break;
        }
    }
    let Some(program) = tokens.next() else {
        return Vec::new();
    };
    if program != "rm" && !program.ends_with("/rm") {
        return Vec::new();
    }
    let mut operands = Vec::new();
    let mut literal = false;
    for token in tokens {
        if !literal && token == "--" {
            literal = true;
            continue;
        }
        if !literal && token.starts_with('-') {
            continue;
        }
        operands.push(token.to_owned());
    }
    operands
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

/// Every tracked path git reports as modified in the repository holding
/// `dir`, paired with where each one actually is on disk.
///
/// Used only when the command names no path and therefore reaches the whole
/// work tree — and that is the repository's work tree, not `dir`'s:
/// `git -C pkg reset --hard` resets everything. The paths come back relative
/// to the repository root, so they are enumerated from it and resolved against
/// it; [`work_tree_root`] states what resolving them against `dir` costs.
///
/// Routed through [`git_run`] like every other git call here, so an ambient
/// `GIT_DIR` or `GIT_WORK_TREE` cannot point the enumeration at a different
/// tree than the one the guard then judges.
fn unsaved_work_tree_paths(dir: &Path) -> Vec<(String, PathBuf)> {
    let Some(root) = work_tree_root(dir) else {
        return Vec::new();
    };
    let Some(run) = git_run(
        &root,
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
            paths.push((path.to_owned(), root.join(path)));
        }
    }
    paths
}
