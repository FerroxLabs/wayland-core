//! INV-2 / P2 — no rewrite may drop content that was on disk when the job
//! started.
//!
//! Measured defect (job corpus, 2026-08-10): the user has an in-progress edit
//! on disk that is committed nowhere. The agent is legitimately asked to change
//! that same file, rewrites it wholesale through the Write tool from its own
//! picture of the contents, and the user's line is gone for good. Observed
//! three times across two platforms — Linux A-2 and Windows A-2 / A-8, on
//! `src/receipts/parser.py` and `retry.py`.
//!
//! A prompt cannot make a whole-file overwrite safe, so the check lives here at
//! the tool layer. The guarantee this module enforces is:
//!
//! > **Content that is on disk and recorded in no commit never leaves the disk
//! > through Write or Edit without either being refused or being copied to a
//! > durable snapshot whose path is reported back to the caller.**
//!
//! Nothing in that sentence depends on the model choosing to cooperate.
//!
//! # The three moving parts
//!
//! **A pinned baseline.** "Saved" means "present in the commit this session
//! started at", not "present in current `HEAD`". The commit is resolved once,
//! per repository, and never re-read. A commit made *during* the session —
//! which the corpus A-2 agent does routinely, straight onto `main` — therefore
//! cannot launder the user's unsaved line into the baseline and disarm the
//! guard. The path a file occupies in that commit is derived from the
//! repository root, not from `git ls-files`, so `git rm --cached` cannot
//! disarm it either.
//!
//! **An agent-authored set.** A line becomes agent-authored the moment this
//! tool puts it on disk when it was not there before. Agent-authored lines are
//! not user work: they are excluded from both the numerator and the
//! denominator of every judgement below. That is what keeps the agent's own
//! files, and its own iterating on an uncommitted branch, completely free —
//! while a line the user typed, even one the agent's own write carried
//! through, stays protected. There is no memoized "baseline at first touch":
//! the disk is re-read on every call, so unsaved work the user creates
//! mid-session is protected from that moment, and a line legitimately removed
//! earlier is never cited again.
//!
//! **A partial/wholesale split.** The discriminator is a property of the file's
//! prior state alone — *is any of the user's content in this file recorded in
//! the pinned commit?* — so the model cannot reach it by choosing what to
//! write.
//!
//! * *Partial* — part of the file is recorded and part is not. A rewrite that
//!   drops the unrecorded part is exactly the measured harm shape: the file
//!   still looks right, so the user never notices. **Refused.**
//! * *Wholesale* — none of the user's content in this file is recorded
//!   anywhere (an untracked scratch file, a file in no repository at all, a
//!   fully-uncommitted rewrite). Replacing it is the evident request and the
//!   user sees the result immediately. **Snapshotted, then allowed**, with the
//!   snapshot path in the tool result.
//!
//! Against the two measured cases the split is not close: `parser.py` is 1
//! unsaved line out of 9 (partial → refuse, the round-1 behaviour is
//! preserved), and the `notes.md` that round 1 wrongly refused is 4 out of 4
//! (wholesale → snapshot and proceed, the task now completes).
//!
//! # Edit
//!
//! Edit is guarded too, but it never refuses. Two reasons, both measured:
//!
//! 1. Edit's `old_string` must match the bytes on disk exactly, so every line
//!    it removes was quoted from disk by the model. That is the opposite of
//!    the silent-omission shape Write produces when it reconstructs a file
//!    from memory.
//! 2. Refusing would make Edit unusable on a dirty tree — the single most
//!    common working state. Every uncommitted line the *user* wrote would
//!    become uneditable.
//!
//! So an Edit that removes unrecorded content snapshots first and says so.
//! Routing around a Write refusal into Edit — which the round-1 adversarial
//! seat measured the model doing unprompted — no longer loses anything.
//!
//! # Where the guarantee does not reach — stated, not implied
//!
//! * **Bash.** `sed -i`, `>`, `rm` and friends are not routed through here and
//!   cannot be at this altitude. In the round-1 `adv-armB` arm this, not Edit,
//!   is what actually destroyed the line.
//! * **Non-UTF-8 files.** No line model, so no protection.
//! * **A repository first touched mid-session** pins at that moment rather
//!   than at session start. The session's own working repository is pinned
//!   eagerly at construction.
//! * **Trim-normalised comparison.** Counting is exact (N copies of a line are
//!   not the same as 1), but an unsaved line whose trimmed text matches a
//!   recorded line, at equal counts, is still invisible.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

/// Most dropped lines quoted back in a refusal message.
const MAX_QUOTED_LINES: usize = 5;

/// Largest file this guard will copy into a snapshot. An overwrite that would
/// drop unrecorded content from a file bigger than this is refused rather than
/// allowed unprotected — the guarantee has no size exemption.
const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

/// How the caller is replacing the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Whole-file replacement authored from the model's own picture of the
    /// contents (the Write tool). Omission is silent here, so a partial drop
    /// of unrecorded content is refused.
    Rewrite,
    /// Targeted replacement of bytes the model quoted from disk (the Edit
    /// tool). Never refused; a drop is snapshotted and reported.
    Surgical,
}

/// What the tool layer must do with a proposed overwrite.
#[derive(Debug)]
pub enum Verdict {
    /// No unrecorded content leaves the disk. Write it.
    Proceed,
    /// The measured silent-drop shape. Do not write; return this message.
    Refuse(String),
    /// Unrecorded content does leave the disk, but the prior bytes are already
    /// saved. Write it, and append this note to the tool result.
    ProceedWithSnapshot(String),
}

/// The commit a repository was pinned to, or why there is no pin.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Baseline {
    /// git answered authoritatively: this path is in no work tree. Nothing
    /// about it is recorded anywhere.
    NoRepo,
    /// git could not be consulted at all (no binary, spawn failure). The
    /// baseline is unknowable, so no protection may be *claimed* — every drop
    /// is treated as unrecorded and snapshotted.
    Unknown,
    /// Repository root plus the commit pinned when it was first seen. `None`
    /// commit means the repository has no commits yet: nothing is recorded.
    Pinned {
        root: PathBuf,
        commit: Option<String>,
    },
}

/// Session-scoped enforcement of the unsaved-work guarantee.
///
/// One instance is shared by every Write and Edit tool in the process (see
/// [`UnsavedWorkGuard::shared`]) so that the pinned baseline and the
/// agent-authored set are the same set of facts for both tools and for every
/// sub-agent.
pub struct UnsavedWorkGuard {
    /// Directory -> its resolved baseline. Memoized because it is immutable
    /// for the session by construction.
    dirs: Mutex<HashMap<PathBuf, Baseline>>,
    /// Repository root -> pinned commit, so two directories in one repository
    /// share one pin.
    pins: Mutex<HashMap<PathBuf, Option<String>>>,
    /// Blob cache keyed by (commit, repo-relative path). Immutable content.
    blobs: Mutex<HashMap<(String, String), String>>,
    /// Path -> trimmed lines this tool itself introduced.
    authored: Mutex<HashMap<PathBuf, HashSet<String>>>,
    /// Where snapshots go. Outside the user's repository, always.
    snapshot_root: PathBuf,
}

static SHARED: OnceLock<Arc<UnsavedWorkGuard>> = OnceLock::new();

impl UnsavedWorkGuard {
    /// The process-wide guard. Every Write and Edit tool shares it, including
    /// the ones sub-agents build, so one agent cannot escape a sibling's
    /// baseline. On first call the working directory's repository is pinned
    /// immediately — that call happens while the tool registry is being built,
    /// which is session start.
    pub fn shared() -> Arc<UnsavedWorkGuard> {
        SHARED
            .get_or_init(|| {
                let guard = Arc::new(UnsavedWorkGuard::with_snapshot_root(default_snapshot_root()));
                if let Ok(cwd) = std::env::current_dir() {
                    guard.baseline_for_dir(&cwd);
                }
                guard
            })
            .clone()
    }

    /// An isolated guard with its own snapshot root. For tests, which must not
    /// share a pinned baseline with each other.
    pub fn with_snapshot_root(snapshot_root: PathBuf) -> Self {
        Self {
            dirs: Mutex::new(HashMap::new()),
            pins: Mutex::new(HashMap::new()),
            blobs: Mutex::new(HashMap::new()),
            authored: Mutex::new(HashMap::new()),
            snapshot_root,
        }
    }

    /// Where this guard puts recovery copies.
    pub fn snapshot_root(&self) -> &Path {
        &self.snapshot_root
    }

    /// Judge replacing `previous` (the bytes currently at `path`, read by the
    /// caller through whichever filesystem it is actually writing to) with
    /// `new_content`.
    ///
    /// Takes the snapshot itself, before returning, so that a caller acting on
    /// [`Verdict::ProceedWithSnapshot`] cannot write ahead of the copy.
    pub fn assess(
        &self,
        path: &Path,
        display_path: &str,
        previous: &str,
        new_content: &str,
        mode: Mode,
    ) -> Verdict {
        // Everything below counts trimmed, non-blank lines, and counts them
        // exactly: three copies of a line are not one copy.
        let authored = self.authored_lines(path);
        let disk = tally_excluding(previous, &authored);
        let user_lines: usize = disk.values().sum();
        if user_lines == 0 {
            return Verdict::Proceed;
        }

        let baseline = self.baseline_for(path);
        let (saved, baseline_known) = match &baseline {
            Baseline::Pinned {
                root,
                commit: Some(commit),
            } => match self.recorded_blob(path, root, commit) {
                Ok(text) => (text, true),
                // Inside a work tree, but git would not answer. Claim nothing.
                Err(()) => (String::new(), false),
            },
            // Repository with no commits yet, or authoritatively no repository:
            // nothing about this file is recorded, and that is a fact, not a
            // failure to establish one.
            Baseline::Pinned { commit: None, .. } | Baseline::NoRepo => (String::new(), true),
            Baseline::Unknown => (String::new(), false),
        };

        let recorded = tally(&saved);

        let mut unsaved: HashMap<&str, usize> = HashMap::new();
        for (text, on_disk) in &disk {
            let in_commit = recorded.get(*text).copied().unwrap_or(0);
            if *on_disk > in_commit {
                unsaved.insert(text, on_disk - in_commit);
            }
        }
        let unsaved_total: usize = unsaved.values().sum();
        if unsaved_total == 0 {
            return Verdict::Proceed;
        }

        // A copy survives only if the new content keeps as many copies as the
        // disk held. Where copies are lost, assume the unrecorded ones went.
        let surviving = tally(new_content);
        let mut dropped: HashMap<&str, usize> = HashMap::new();
        for (text, unsaved_copies) in &unsaved {
            let lost = disk[*text].saturating_sub(surviving.get(*text).copied().unwrap_or(0));
            let lost_unsaved = (*unsaved_copies).min(lost);
            if lost_unsaved > 0 {
                dropped.insert(text, lost_unsaved);
            }
        }
        let dropped_total: usize = dropped.values().sum();
        if dropped_total == 0 {
            return Verdict::Proceed;
        }

        // The discriminator. Derived only from the file's prior state and the
        // pinned commit, so no choice of `new_content` can move it.
        let wholesale = unsaved_total == user_lines;

        if mode == Mode::Rewrite && !wholesale {
            return Verdict::Refuse(refusal_text(
                display_path,
                previous,
                &dropped,
                dropped_total,
            ));
        }

        match self.snapshot(path, previous) {
            Ok(saved_to) => Verdict::ProceedWithSnapshot(snapshot_note(
                dropped_total,
                baseline_known,
                mode,
                &saved_to,
            )),
            Err(why) => Verdict::Refuse(format!(
                "Refused to overwrite {display_path}: it holds {dropped_total} line(s) that are \
                 on disk and in no commit, and the copy that would make replacing them \
                 recoverable could not be written ({why}). Nothing was changed."
            )),
        }
    }

    /// Record what this tool just put on disk at `path`.
    ///
    /// A line counts as agent-authored from the moment the tool writes it and
    /// it was not already there. Lines that were already on disk stay the
    /// user's, however many times the agent's own writes carry them through —
    /// otherwise two writes would launder any line into being unprotected.
    pub fn note_written(&self, path: &Path, previous: &str, written: &str) {
        let already: HashSet<&str> = previous
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let introduced: Vec<String> = written
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !already.contains(l))
            .map(str::to_owned)
            .collect();
        if introduced.is_empty() {
            return;
        }
        if let Ok(mut map) = self.authored.lock() {
            map.entry(path.to_path_buf())
                .or_default()
                .extend(introduced);
        }
    }

    fn authored_lines(&self, path: &Path) -> HashSet<String> {
        self.authored
            .lock()
            .ok()
            .and_then(|m| m.get(path).cloned())
            .unwrap_or_default()
    }

    fn baseline_for(&self, path: &Path) -> Baseline {
        match path.parent() {
            Some(dir) => self.baseline_for_dir(dir),
            None => Baseline::Unknown,
        }
    }

    /// Resolve — and pin — the baseline for `dir`, once.
    fn baseline_for_dir(&self, dir: &Path) -> Baseline {
        if let Ok(map) = self.dirs.lock()
            && let Some(hit) = map.get(dir)
        {
            return hit.clone();
        }
        let resolved = self.resolve_baseline(dir);
        if let Ok(mut map) = self.dirs.lock() {
            return map.entry(dir.to_path_buf()).or_insert(resolved).clone();
        }
        resolved
    }

    fn resolve_baseline(&self, dir: &Path) -> Baseline {
        match git_output(dir, &["rev-parse", "--is-inside-work-tree"]) {
            // git ran and said no. Authoritative.
            Ok(None) => return Baseline::NoRepo,
            // git could not be run at all.
            Err(()) => return Baseline::Unknown,
            Ok(Some(answer)) if answer.trim() != "true" => return Baseline::NoRepo,
            Ok(Some(_)) => {}
        }
        let Ok(Some(top)) = git_output(dir, &["rev-parse", "--show-toplevel"]) else {
            return Baseline::Unknown;
        };
        let root = PathBuf::from(top.trim());
        if let Ok(map) = self.pins.lock()
            && let Some(commit) = map.get(&root)
        {
            return Baseline::Pinned {
                root,
                commit: commit.clone(),
            };
        }
        // A repository with no commits yet answers non-zero here; that is a
        // real state (nothing is recorded), not a failure to ask.
        let commit = match git_output(dir, &["rev-parse", "HEAD"]) {
            Ok(Some(sha)) => Some(sha.trim().to_owned()),
            Ok(None) => None,
            Err(()) => return Baseline::Unknown,
        };
        if let Ok(mut map) = self.pins.lock() {
            map.entry(root.clone()).or_insert_with(|| commit.clone());
        }
        Baseline::Pinned { root, commit }
    }

    /// The file's content in the pinned commit, or `""` if that commit did not
    /// contain it. `Err(())` only when git could not answer.
    fn recorded_blob(&self, path: &Path, root: &Path, commit: &str) -> Result<String, ()> {
        // Derived from the repository root, never from `git ls-files`: the
        // index is mutable during the session, the pinned commit is not.
        let rel = repo_relative(root, path).ok_or(())?;
        let key = (commit.to_owned(), rel.clone());
        if let Ok(cache) = self.blobs.lock()
            && let Some(hit) = cache.get(&key)
        {
            return Ok(hit.clone());
        }
        let spec = format!("{commit}:{rel}");
        let text = match git_output(root, &["show", &spec]) {
            Ok(Some(t)) => t,
            // Not present in that commit: recorded nowhere, which is an answer.
            Ok(None) => String::new(),
            Err(()) => return Err(()),
        };
        if let Ok(mut cache) = self.blobs.lock() {
            cache.insert(key, text.clone());
        }
        Ok(text)
    }

    /// Copy `bytes` somewhere durable and outside the user's repository.
    ///
    /// Content-addressed, so repeatedly overwriting the same prior state
    /// during one session reuses one file rather than accumulating copies.
    fn snapshot(&self, path: &Path, bytes: &str) -> Result<PathBuf, String> {
        if bytes.len() > MAX_SNAPSHOT_BYTES {
            return Err(format!(
                "file is {} bytes, over the {}-byte snapshot limit",
                bytes.len(),
                MAX_SNAPSHOT_BYTES
            ));
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed");
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let target = self.snapshot_root.join(format!(
            "{safe}.{:016x}.{:016x}",
            hash_of(&path.to_string_lossy()),
            hash_of(bytes)
        ));
        if target.exists() {
            return Ok(target);
        }
        std::fs::create_dir_all(&self.snapshot_root).map_err(|e| e.to_string())?;
        // Both the per-session directory and the `unsaved-work` root above it:
        // `create_dir_all` uses the process umask, which is commonly 022.
        if let Some(parent) = self.snapshot_root.parent() {
            restrict_dir(parent);
        }
        restrict_dir(&self.snapshot_root);
        std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
        // The prior contents can hold anything the user had in that file,
        // including a credential they pasted. Owner-only.
        restrict_file(&target);
        Ok(target)
    }
}

/// `~/.wayland/unsaved-work/<start>-<pid>` — the profile state directory, never
/// the user's working tree. Writing uninvited files into a user's repository is
/// an open corpus defect against this product; a recovery copy must not add to
/// it.
fn default_snapshot_root() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    wcore_config::config::profile_home()
        .join("unsaved-work")
        .join(format!("{stamp}-{}", std::process::id()))
}

#[cfg(unix)]
fn restrict_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_dir(_dir: &Path) {
    // Windows: the copy inherits the ACL of the per-user profile directory,
    // which is already owner-scoped. There is no portable mode bit to set.
}

#[cfg(unix)]
fn restrict_file(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_file(_file: &Path) {}

fn hash_of(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// `path` expressed the way a `<commit>:<path>` spec wants it: relative to the
/// repository root, forward-slashed, on every platform.
fn repo_relative(root: &Path, path: &Path) -> Option<String> {
    let root = std::fs::canonicalize(root).ok()?;
    let full = std::fs::canonicalize(path).ok()?;
    let rel = full.strip_prefix(&root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Trimmed, non-blank line counts.
fn tally(text: &str) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        *counts.entry(trimmed).or_insert(0) += 1;
    }
    counts
}

/// As [`tally`], minus any line this tool itself introduced.
fn tally_excluding<'a>(text: &'a str, authored: &HashSet<String>) -> HashMap<&'a str, usize> {
    let mut counts = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || authored.contains(trimmed) {
            continue;
        }
        *counts.entry(trimmed).or_insert(0) += 1;
    }
    counts
}

/// The refusal. It deliberately names no other tool: round 1 shipped a message
/// recommending Edit, and the model took that route on the first refusal in
/// both live adversarial arms.
fn refusal_text(
    display_path: &str,
    previous: &str,
    dropped: &HashMap<&str, usize>,
    dropped_total: usize,
) -> String {
    let mut budget: HashMap<&str, usize> = dropped.clone();
    let mut quoted = Vec::new();
    for line in previous.lines() {
        if quoted.len() >= MAX_QUOTED_LINES {
            break;
        }
        let trimmed = line.trim();
        if let Some(left) = budget.get_mut(trimmed)
            && *left > 0
        {
            *left -= 1;
            // The prior contents are being echoed into the model's context.
            // Same scrubber the engine already applies to tool output.
            let shown = wcore_safety::PIIScrubber.scrub(line.trim_end());
            quoted.push(format!("    {shown}"));
        }
    }
    let more = dropped_total.saturating_sub(quoted.len());
    let tail = if more > 0 {
        format!("\n    ... and {more} more line(s)")
    } else {
        String::new()
    };
    format!(
        "Refused to overwrite {display_path}: this content would delete {dropped_total} line(s) \
         that are on disk but in no commit. That is unsaved work which exists nowhere else, so \
         losing it is irreversible.\n\
         Lines that would be lost:\n{quoted}{tail}\n\
         The rest of this file IS committed, so this is not the whole-file replacement the user \
         asked for — it is a partial rewrite that would silently drop their in-progress work. \
         Read the file as it stands on disk now and reproduce those lines in the content you \
         write. If the user genuinely asked for those specific lines to go, they must be \
         recorded somewhere first.",
        quoted = quoted.join("\n"),
    )
}

fn snapshot_note(
    dropped_total: usize,
    baseline_known: bool,
    mode: Mode,
    saved_to: &Path,
) -> String {
    let why = match (baseline_known, mode) {
        (false, _) => {
            " The last saved version could not be established (git did not answer), so no \
             protection was assumed."
        }
        (true, Mode::Rewrite) => {
            " None of this file was in any commit, so the whole of it counted as unsaved work."
        }
        (true, Mode::Surgical) => "",
    };
    format!(
        "\nNote: {dropped_total} line(s) that were on disk and in no commit are not in the new \
         content.{why} The previous contents were copied to {} first, so nothing is lost — tell \
         the user that path if they want any of it back.",
        saved_to.display()
    )
}

/// Run `git` in `dir`.
///
/// `Ok(Some(stdout))` when git ran and succeeded, `Ok(None)` when git ran and
/// returned non-zero (a real answer: not a repository, no commits, path not in
/// the commit), `Err(())` when git could not be run at all. Round 1 collapsed
/// the last two into "no protection, and no signal to anyone".
///
/// `--literal-pathspecs` so an LLM-supplied file name containing `*`, `:` or a
/// leading `-` is never read as a pattern or an option. Argv mode throughout —
/// no shell is involved.
fn git_output(dir: &Path, args: &[&str]) -> Result<Option<String>, ()> {
    let out = Command::new("git")
        .arg("--literal-pathspecs")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| ())?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

#[cfg(test)]
mod tests;
