//! The git-tool half of the INV-2 unsaved-work guard.
//!
//! [`shell_refusal`](super::shell_refusal) covers `git` typed into `Bash`. It
//! does not cover [`crate::git::GitTool`], and under the STRICT sandbox that
//! tool is the ONLY git surface a session has (see the module documentation
//! on `crate::git`) — so the guarded route is the one the product tells the
//! model not to use, and the unguarded one is the route it is pushed onto.
//! Job corpus row A-8 (2026-08-11) is that gap arriving:
//! `{"op":"add_paths","paths":["README.md", ...]}` then `{"op":"commit"}` put
//! the user's uncommitted `README.md` edit into a commit, and INV-2 read it
//! back as "the user's unsaved work was committed on their behalf".
//!
//! # What refuses, and what deliberately does not
//!
//! Staging is not destruction, so the bar here is not the shell surface's. A
//! file the agent has genuinely worked on has to stay committable while the
//! user's own half-finished line sits in it: refusing that would make the
//! tool useless on the working state it exists for, and there is no way to
//! commit a change to a file without carrying the rest of that file with it.
//!
//! The candidate shape is **a path whose entire difference from the pinned
//! commit is the user's unsaved work** — this session contributed nothing to
//! it, so putting it in a commit is a decision about the user's own work.
//! What is then done about it depends on how the path got into the commit,
//! and the split is deliberate:
//!
//! * **A path the caller named** (`add_paths`) is refused. Naming it is a
//!   choice, and A-8's `["README.md", …]` is that choice being made wrongly.
//! * **A path swept in by `add -A`, or already sitting in the index, that the
//!   pinned commit has never recorded at all** is refused. A brand-new file
//!   nobody asked to add is the untracked-scratch shape from A-2.
//! * **Everything else is staged and reported**, with a note naming the
//!   unsaved lines riding along.
//!
//! That third case is not a shrug, it is the wrong-refusal boundary. This
//! module cannot tell a line the agent wrote through `sed -i`, a shell
//! redirect or anything else outside `Write`/`Edit` from a line the user
//! typed — they are both "on disk, in no commit, not attributed to a tool".
//! Refusing every such path would refuse ordinary work:
//! `git_branch_and_pr_test` writes a tracked file directly and then commits
//! it, which is a normal thing a session does and must keep working. So a
//! tracked file that is merely modified gets the note, and the residual is
//! stated rather than hidden: an `add_all` + `commit` can still carry a
//! tracked file's unsaved user edit into a commit.
//!
//! The measurement is the one the rest of the guard makes, read from the same
//! [`UnsavedWorkGuard::shared`] instance: lines on disk that the pinned commit
//! does not record and that this session did not author.
//!
//! It fails towards not firing, in these named cases:
//!
//! * a baseline git would not settle — nothing is proven, so nothing is
//!   refused;
//! * a repository with no commits yet — there is no saved state to compare
//!   against, and refusing the first commit of a fresh tree would be absurd;
//! * files that are not UTF-8 text — no line model, nothing to quote;
//! * lines this session wrote through `Write` or `Edit` — attributed to the
//!   tool, so they are not the user's unsaved work.

use std::path::{Path, PathBuf};

use super::{
    Baseline, MAX_QUOTED_LINES, UnsavedWorkGuard, git_run, recorded_raw, repo_relative,
    repository_marker_present, tally,
};

/// Which paths an op is about to put into a commit.
pub enum Staging<'a> {
    /// `git add -A` — everything the work tree holds.
    Everything,
    /// `git add -- <p>...` — the paths the caller named, relative to `cwd`.
    Named(&'a [String]),
    /// `git commit` — whatever is already in the index.
    Index,
}

/// What a [`crate::git::GitTool`] staging or commit op should do.
///
/// `Err` is a refusal to return instead of running the op. `Ok(Some(note))`
/// is a note to append to the result of an op that does run.
pub fn staging_verdict(cwd: &Path, staging: Staging<'_>) -> Result<Option<String>, String> {
    if !repository_marker_present(cwd) {
        return Ok(None);
    }
    let guard = UnsavedWorkGuard::shared();
    let named = matches!(staging, Staging::Named(_));
    let candidates: Vec<PathBuf> = match staging {
        Staging::Everything => work_tree_paths(cwd, true),
        Staging::Named(paths) => paths.iter().map(PathBuf::from).collect(),
        Staging::Index => index_paths(cwd),
    };

    let mut refused: Vec<(String, Vec<String>)> = Vec::new();
    let mut noted: Vec<String> = Vec::new();
    for rel in candidates {
        let shown = rel.to_string_lossy().into_owned();
        if refused.iter().any(|(p, _)| *p == shown) || noted.contains(&shown) {
            continue;
        }
        let abs = cwd.join(&rel);
        let unsaved = guard.unsaved_lines(&abs);
        if unsaved.is_empty() {
            continue;
        }
        let Some(state) = state_of(&guard, &abs, unsaved.len()) else {
            // Cannot be established, so nothing is claimed and nothing is
            // refused.
            continue;
        };
        if state.agent_also_changed || (!named && state.head_records_it) {
            noted.push(shown);
        } else {
            refused.push((shown, unsaved));
        }
    }

    if !refused.is_empty() {
        return Err(format!(
            "Refused: {n} path(s) below would go into a commit, and the only thing in them that \
             is not already committed is work the user has not saved. This session wrote none of \
             it, so committing it is a decision about the user's own work rather than part of \
             the change being made.\n\
             Would be committed on the user's behalf:{detail}\
             Stage the paths this session actually changed, by name, and leave these alone. If \
             the user does want them in the commit, say what is in them and let them confirm \
             first.",
            n = refused.len(),
            detail = quote_at_risk(&refused),
        ));
    }
    if noted.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "\nNote: {n} staged path(s) also carry line(s) that are on disk, in no commit, and not \
         written by this session ({paths}). They go into the commit as part of the change to \
         those files. If the user meant to keep them uncommitted, say so.",
        n = noted.len(),
        paths = noted.join(", "),
    )))
}

/// The refusal for `git stash push`, which takes the whole work tree away.
///
/// The shell surface already refuses that command when it is typed into
/// `Bash`. The tool op reaching the same git through a different door is the
/// same act and gets the same answer.
pub fn stash_refusal(cwd: &Path) -> Option<String> {
    if !repository_marker_present(cwd) {
        return None;
    }
    let guard = UnsavedWorkGuard::shared();
    let mut at_risk: Vec<(String, Vec<String>)> = Vec::new();
    for rel in work_tree_paths(cwd, false) {
        let lines = guard.unsaved_lines(&cwd.join(&rel));
        if !lines.is_empty() {
            at_risk.push((rel.to_string_lossy().into_owned(), lines));
        }
    }
    if at_risk.is_empty() {
        return None;
    }
    Some(format!(
        "Refused to stash: this takes the work tree away, and {n} file(s) below hold line(s) \
         that are on disk and in no commit. Moving the user's unsaved work onto the stash is not \
         leaving it alone, and a stash nobody pops loses it.\n\
         At risk:{detail}\
         Work around those changes instead. If the user really does want them stashed, say what \
         is in them and let them confirm first.",
        n = at_risk.len(),
        detail = quote_at_risk(&at_risk),
    ))
}

/// What the pinned commit and the disk say about one candidate path.
struct PathState {
    /// The pinned commit records this path under some content of its own.
    head_records_it: bool,
    /// Something other than the `unsaved` user lines was added to this file
    /// since the pinned commit — so this session, or something the guard can
    /// attribute to it, worked on the file.
    agent_also_changed: bool,
}

/// Read `abs` against the pinned commit, or `None` when the commit cannot
/// answer for it.
///
/// `agent_also_changed` compares the whole added-since-the-pinned-commit count
/// against the count of unsaved user lines. Equal means the only thing this
/// path would carry into a commit is the user's own work.
fn state_of(guard: &UnsavedWorkGuard, abs: &Path, unsaved: usize) -> Option<PathState> {
    let disk = std::fs::read_to_string(abs).ok()?;
    let Baseline::Repo {
        root,
        commit: Some(commit),
    } = guard.baseline_for(abs)
    else {
        return None;
    };
    let rel = repo_relative(&root, abs)?;
    let head_records_it = recorded_raw(&root, &commit, &rel).ok()?.is_some();
    let recorded = guard.recorded_blob(abs, &root, &commit).ok()?;
    let before = tally(&recorded);
    let mut added = 0usize;
    for (line, now) in tally(&disk) {
        added += now.saturating_sub(before.get(line).copied().unwrap_or(0));
    }
    Some(PathState {
        head_records_it,
        agent_also_changed: added > unsaved,
    })
}

/// Every path `git add -A` would pick up under `dir`.
fn work_tree_paths(dir: &Path, include_untracked: bool) -> Vec<PathBuf> {
    let arg = if include_untracked {
        "--untracked-files=all"
    } else {
        "--untracked-files=no"
    };
    let Some(run) = git_run(dir, &["status", "--porcelain", "-z", arg], None) else {
        return Vec::new();
    };
    if !run.ok() {
        return Vec::new();
    }
    porcelain_paths(&run.stdout_text())
}

/// Every path in the index that `HEAD` does not already record identically.
fn index_paths(dir: &Path) -> Vec<PathBuf> {
    let Some(run) = git_run(dir, &["diff", "--cached", "--name-only", "-z"], None) else {
        return Vec::new();
    };
    if !run.ok() {
        return Vec::new();
    }
    run.stdout_text()
        .split('\0')
        .filter(|f| !f.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// `XY <path>` entries out of `git status --porcelain -z`.
///
/// Sliced with `get` rather than `[3..]`: byte 3 of a non-ASCII path can land
/// mid-character, which panics.
fn porcelain_paths(text: &str) -> Vec<PathBuf> {
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    let mut paths = Vec::new();
    while let Some(entry) = fields.next() {
        let (Some(status), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        if status.contains('R') || status.contains('C') {
            fields.next();
        }
        if !path.is_empty() {
            paths.push(PathBuf::from(path));
        }
    }
    paths
}

/// One formatting of the at-risk list, shared by both refusals here and by
/// the shell surface, so the same fact never reads two ways.
pub(super) fn quote_at_risk(at_risk: &[(String, Vec<String>)]) -> String {
    let mut detail = String::new();
    for (path, lines) in at_risk {
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
    detail
}

/// Files reachable under `operand`, for a shell command that removes it.
///
/// A file is itself; a directory is every file beneath it. Bounded, because
/// this runs before every `rm` a session issues and a deep tree must not turn
/// one command into an unbounded filesystem walk. Hitting the bound means the
/// refusal may not fire, which is the direction the shell surface already
/// chooses. Symlinks are not followed: removing a link does not touch what it
/// points at.
pub(super) fn files_under(operand: &Path, budget: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![operand.to_path_buf()];
    while let Some(next) = stack.pop() {
        if found.len() >= budget {
            break;
        }
        let Ok(meta) = std::fs::symlink_metadata(&next) else {
            continue;
        };
        if meta.is_file() {
            found.push(next);
            continue;
        }
        if !meta.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            stack.push(entry.path());
        }
    }
    found
}

/// Whether the repository holding `path` says to ignore it.
///
/// Keeps an `rm` of build output, caches and other ignored scratch out of the
/// refusal. [`UnsavedWorkGuard::repository_ignores`] answers "ignored" for a
/// question it could not decide, which here means "do not refuse" — the same
/// direction the rest of the shell surface takes.
pub(super) fn ignored(guard: &UnsavedWorkGuard, path: &Path) -> bool {
    let Baseline::Repo { root, .. } = guard.baseline_for(path) else {
        return false;
    };
    match repo_relative(&root, path) {
        Some(rel) => guard.repository_ignores(&root, &rel),
        None => false,
    }
}
