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
//! the tool layer.
//!
//! # What this module actually guarantees
//!
//! Stated narrowly, because a broad statement of it would not be true:
//!
//! > **Through the Write tool, a line that is on disk and that this module
//! > cannot prove is recorded in the session's pinned commit never leaves the
//! > disk unless the prior bytes have first been written to the repository's
//! > own object store *and read back byte-for-byte*. Where no such copy can be
//! > made, the write is refused.**
//!
//! Nothing in that sentence depends on the model choosing to cooperate, and
//! nothing in it is claimed when it has not been checked. Three limits are part
//! of the statement, not footnotes to it:
//!
//! * It covers **two of the three write surfaces**. `Write` is refused-or-
//!   copied as above. `Edit` is never refused (see below) but never claims a
//!   copy it did not make. **`Bash` is not covered at all** — `sed -i '2d'`,
//!   `>` and `rm` do not route through here and cannot at this altitude. In
//!   the round-1 `adv-armB` arm `sed -i`, not Edit, is what actually destroyed
//!   the line. A guarantee described as holding "at the tool layer" without
//!   that carve-out would be false.
//! * It is a guarantee about **dropped** lines, and this module cannot tell a
//!   dropped line from a **modified** one. A whole-file transformation that
//!   renames a symbol occurring on an unsaved line reads here as a drop and is
//!   refused.
//! * **Non-UTF-8 files** have no line model, so no protection.
//!
//! # The four moving parts
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
//! **A repository test that does not depend on git working.** Round 2 asked
//! `git rev-parse --is-inside-work-tree` and read *any* non-zero exit as an
//! authoritative "there is no repository here". git exits 128 for a missing
//! repository **and** for every repository it refuses to open: dubious
//! ownership (the default for Docker bind mounts, CI checkouts and sudo-run
//! agents), an unreadable `.git/config`, a bad `GIT_DIR`. All of those became
//! "nothing here is recorded", which makes every line unsaved, which makes
//! every rewrite wholesale, which means the guard never refuses. So the
//! question git cannot answer honestly is answered from the filesystem
//! instead: is there a `.git` marker on any ancestor? A missing repository is
//! then a fact that holds even with no `git` binary at all, and a repository
//! git will not open is [`Baseline::Unknown`] rather than "no repository".
//!
//! **Fail closed on an unresolved baseline.** [`Baseline::Unknown`] proves
//! nothing, so nothing is claimed: a `Write` that would drop lines is refused
//! and names git's own reason, and an `Edit` proceeds while saying plainly
//! that no copy was made.
//!
//! **Nothing repairable is memoized.** Round 2 cached [`Baseline::Unknown`],
//! so a single transient git fault at startup disarmed the whole session,
//! every sub-agent with it, and repairing git did not bring the guard back.
//! Round 3 stopped caching `Unknown` and, in the same change, made
//! [`Baseline::NoRepo`] a *refusing* state for the first time while still
//! caching it — the same latch one class over. Measured: no repository ->
//! refuse; `git init` plus a commit, so every line is now provably recorded ->
//! the **same** guard still refuses, while a fresh guard on identical inputs
//! proceeds. Process-wide via the `OnceLock`, so every sub-agent inherited it.
//! Only [`Baseline::Repo`] is cached now, and it is immutable for the session
//! by construction because the commit is pinned.
//!
//! **A partial/wholesale split.** The discriminator is a property of the
//! file's prior state alone — *is any of the user's content in this file
//! recorded in the pinned commit?* — so the model cannot reach it by choosing
//! what to write.
//!
//! * *Partial* — part of the file is recorded and part is not. A rewrite that
//!   drops the unrecorded part is exactly the measured harm shape: the file
//!   still looks right, so the user never notices. **Refused.**
//! * *Wholesale* — none of the user's content in this file is recorded (an
//!   untracked file, a fully-uncommitted rewrite). Replacing it is the evident
//!   request and the user sees the result immediately, so it is allowed —
//!   **but only against a verified copy**, and refused when none can be made.
//!
//! # Where the recovery copy goes, and why not somewhere new
//!
//! Into the repository's own object database, via `git hash-object -w`. It is
//! then read back with `git cat-file` and compared byte-for-byte before the
//! tool result says a word about recoverability.
//!
//! Round 2 instead copied the prior bytes to `~/.wayland/unsaved-work`. The
//! adversarial seat measured what that bought: a gitignored `.env` holding a
//! live `STRIPE_SK` and an `AWS_SECRET_ACCESS_KEY` was copied there in clear,
//! the scrubber having been applied only to the copy that goes to the *model*
//! and never to the copy that goes to *disk*; the hardening that made it
//! owner-only was a `#[cfg(not(unix))]` no-op on Windows, where this project's
//! own machine grants `(OI)(CI)(RX)` on the profile directory to the very
//! sandbox principal that confines agent subprocesses; and nothing ever
//! removed any of it.
//!
//! The object store has none of those properties to get wrong. It is the
//! user's existing security domain with the user's existing permissions, it
//! needs no new directory and no new mode bits. It is also self-documenting:
//! `git cat-file blob <oid>` is the whole recovery procedure.
//!
//! What it is **not** is self-disposing, and round 3's tool result told the
//! user it was ("the repository's normal garbage collection removes it in due
//! course"). Measured on git 2.43.0 (Linux) and 2.54.0 (Windows): `git gc`
//! does not remove an unreferenced object. `gc.cruftPacks` has been on by
//! default since git 2.42, so gc *moves* it into a cruft pack
//! (`pack-*.mtimes`) and `git cat-file blob <oid>` still prints it — still
//! readable after six consecutive `git gc` runs. Only `git gc --prune=now`, or
//! an ordinary gc once `gc.pruneExpire` (**two weeks** by default) has passed,
//! disposes of it; `git gc --auto` needs more than 6700 loose objects before
//! it fires at all, so one guard copy never triggers it. Realistic persistence
//! is **unbounded**, so the note names the command instead of promising a
//! schedule.
//!
//! Also measured, and also in the note: the object travels with a filesystem
//! copy of the repository (`cp -a`, `tar`, `rsync`) and with
//! `git clone /local/path` — including `--local`, `--no-hardlinks`, and after
//! a `gc` — and `git fsck --lost-found` materialises it as a **plaintext
//! file** under `.git/lost-found/other/`. It does not travel with `git push`,
//! `git bundle --all`, or `git clone file://`.
//!
//! The prior bytes are by construction in no commit, so `.git/objects` becomes
//! the only place they exist. Round 3 conditioned that warning on "if this
//! file is gitignored"; the exposure is identical for a merely **untracked**
//! file, and for a tracked one that has been wholly rewritten, so it is now
//! stated unconditionally.
//!
//! When there is **no repository at all** there is no object store, so there is
//! nowhere to put a copy. A `Write` that would drop lines is refused. This is
//! narrower than round 2, which allowed it against a profile-home copy; that
//! copy is the one the adversary broke, so the allowance goes with it.
//!
//! # An enclosing repository is not necessarily this file's archive
//!
//! Measured live (`armD`): `$HOME` is a dotfiles repository — a very common
//! setup — and the private file is `~/work/env.local`, holding a Stripe key
//! and a database password. The file is inside that repository's work tree, so
//! the marker walk is right and this is not a bug in it. Round 3 nevertheless
//! measured **zero refusals**: the write proceeded and the user's secrets were
//! filed into the dotfiles repository's object store, still recoverable after
//! `git gc`.
//!
//! So an enclosing repository counts as this file's object store only when the
//! pinned commit records something under the file's **own directory**, or the
//! file sits at the repository root. Where it does not, the repository is
//! treated exactly as no repository: a `Write` that would drop lines is
//! refused and **nothing is copied into it**, and an `Edit` proceeds saying
//! plainly that no copy was made. That way the user is told where their bytes
//! would have gone *before* the write, which a tool-result note cannot do —
//! the note is assembled after `fs::write` has already returned.
//!
//! # Secrets, and the one lever there is
//!
//! A recovery copy is **verbatim by construction**. Scrubbing it would make it
//! not a recovery copy: the guarantee is that the user's bytes come back, and
//! bytes that come back with the secret replaced are lost work wearing a
//! disguise. So the copy is never scrubbed, and no claim is made that it is.
//! (The *quoted* lines in a refusal are a different thing — those go into the
//! model's context, not into a store, and they are scrubbed with the engine's
//! own [`wcore_safety::PIIScrubber`]; see [`quote_dropped`].)
//!
//! What can be controlled is **where the bytes are allowed to go**, and there
//! are two rules, both of which refuse rather than copy:
//!
//! * the armD rule above — a repository that records nothing under the file's
//!   directory is not its archive;
//! * **a file the repository is configured to ignore is not the repository's
//!   to hold.** `.gitignore` is the user saying, in that repository's own
//!   configuration, that this file does not belong in it. Round 3 filed a
//!   gitignored `.env` into `.git/objects` anyway, and that is not a wash: the
//!   object then travels with `git clone <path>` and `git fsck --lost-found`
//!   materialises it as plaintext, so a copy the user believes their ignore
//!   rules filtered carries the key. Now the `Write` is refused and the `Edit`
//!   says no copy was made.
//!
//! The store's *permissions* are the user's own: there is no directory of this
//! guard's making anywhere, on any platform. Round 2 kept snapshots under the
//! profile home and tried to lock them down with `restrict_dir`/`restrict_file`
//! helpers that were `#[cfg(not(unix))]` no-ops, so on Windows
//! `%USERPROFILE%\.wayland` inherited `CodexSandboxUsers:(OI)(CI)(RX)` plus two
//! AppContainer package SIDs and the agent's own sandbox could read the copy.
//! Those helpers and that store are gone. A recovery object is an ordinary
//! object in `.git/objects`, with byte-identical permissions to every other
//! object the user's own `git` writes there — which is the same statement on
//! Windows as on Unix, and is why nothing here is `#[cfg]`-ed.
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
//! So an Edit that removes unrecorded content copies first where it can, and
//! where it cannot it says so instead of pretending otherwise.
//!
//! # Other limits, stated
//!
//! * **A repository first touched mid-session** pins at that moment rather
//!   than at session start. The session's own working repository is pinned
//!   eagerly at construction. A repository that was *unresolvable* at session
//!   start pins whenever it first resolves, which is later than session start
//!   — strictly better than round 2, which stayed disarmed forever.
//! * **Trim-normalised comparison.** Counting is exact (N copies of a line are
//!   not the same as 1), but an unsaved line whose trimmed text matches a
//!   recorded line, at equal counts, is still invisible.
//! * **Agent-authored lines are exempted per instance, never per text.** Round
//!   3 keyed the exemption to the trimmed text, so once this tool had written
//!   one line of text into a file, *every* later user line with that same text
//!   in that file was permanently unprotected — reachable through ordinary
//!   boilerplate (`import logging`, a repeated log call). Measured: agent
//!   writes `TOKEN = load()`, user adds their own second copy, a rewrite
//!   dropping both returns `Proceed`. The exemption is now a count — as many
//!   copies as this tool actually introduced, never more than are on disk —
//!   so the user's own copy stays the user's.
//! * **The ambient git environment is removed, not inherited.** `GIT_DIR`,
//!   `GIT_COMMON_DIR`, `GIT_WORK_TREE`, `GIT_OBJECT_DIRECTORY`,
//!   `GIT_ALTERNATE_OBJECT_DIRECTORIES` and `GIT_QUARANTINE_PATH` are cleared
//!   for every invocation; see [`git_run`] for what each of them broke.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

/// Most dropped lines quoted back in a refusal message.
const MAX_QUOTED_LINES: usize = 5;

/// Largest file this guard will copy. An overwrite that would drop unrecorded
/// content from a file bigger than this is refused rather than allowed
/// unprotected — the guarantee has no size exemption.
const MAX_RECOVERY_BYTES: usize = 16 * 1024 * 1024;

/// Longest reason quoted back from git's own stderr.
const MAX_GIT_REASON_CHARS: usize = 200;

/// How the caller is replacing the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Whole-file replacement authored from the model's own picture of the
    /// contents (the Write tool). Omission is silent here, so a drop of
    /// content that cannot be proven recorded is refused unless it can be
    /// copied first.
    Rewrite,
    /// Targeted replacement of bytes the model quoted from disk (the Edit
    /// tool). Never refused; a drop is copied where a copy is possible, and
    /// reported accurately either way.
    Surgical,
}

/// What the tool layer must do with a proposed overwrite.
#[derive(Debug)]
pub enum Verdict {
    /// No unrecorded content leaves the disk. Write it.
    Proceed,
    /// Do not write; return this message.
    Refuse(String),
    /// Write it, and append this note to the tool result. The note states
    /// exactly what happened to the prior bytes and never claims a copy that
    /// was not made and verified.
    ProceedWithNote(String),
}

/// Where this file's prior bytes may be copied.
enum Store<'a> {
    /// A repository that records the file's own directory, or that the file
    /// sits at the root of. This is the file's archive.
    Owned(&'a Path),
    /// A repository encloses the file but is not its archive — it records
    /// nothing under the file's directory (measured: a `$HOME` dotfiles
    /// repository holding `~/work/env.local`), or its own configuration says
    /// to ignore the file. Treated as no store at all. `why` completes the
    /// sentence "that repository ...".
    Foreign { root: &'a Path, why: String },
    /// No repository, so no object store.
    Absent,
}

/// What is known about where a path's saved state lives.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Baseline {
    /// No repository governs this path. Established from the filesystem, so it
    /// holds even when `git` cannot be run at all.
    NoRepo,
    /// A work tree, pinned to the commit it was at when first resolved. `None`
    /// means the repository had no commits at pin time — a real state, not a
    /// failure to establish one.
    Repo {
        root: PathBuf,
        commit: Option<String>,
    },
    /// A repository is present but git would not answer for it. Nothing can be
    /// certified, so nothing is claimed, and this is never memoized.
    Unknown(String),
}

/// Session-scoped enforcement of the unsaved-work guarantee.
///
/// One instance is shared by every Write and Edit tool in the process (see
/// [`UnsavedWorkGuard::shared`]) so that the pinned baseline and the
/// agent-authored set are the same set of facts for both tools and for every
/// sub-agent.
pub struct UnsavedWorkGuard {
    /// Directory -> its settled baseline. Memoized because a settled answer is
    /// immutable for the session by construction. [`Baseline::Unknown`] is
    /// never stored here.
    dirs: Mutex<HashMap<PathBuf, Baseline>>,
    /// Repository root -> pinned commit, so two directories in one repository
    /// share one pin.
    pins: Mutex<HashMap<PathBuf, Option<String>>>,
    /// Blob cache keyed by (commit, repo-relative path). Immutable content.
    blobs: Mutex<HashMap<(String, String), String>>,
    /// Path -> how many copies of each trimmed line this tool itself
    /// introduced. A count, not a set: exempting every copy of a text the
    /// agent wrote once leaves the user's own later copies unprotected.
    authored: Mutex<HashMap<PathBuf, HashMap<String, usize>>>,
}

static SHARED: OnceLock<Arc<UnsavedWorkGuard>> = OnceLock::new();

impl UnsavedWorkGuard {
    /// The process-wide guard. Every Write and Edit tool shares it, including
    /// the ones sub-agents build, so one agent cannot escape a sibling's
    /// baseline. On first call the working directory's repository is pinned
    /// immediately — that call happens while the tool registry is being built,
    /// which is session start.
    ///
    /// If that eager resolution fails it is simply not cached, so the next
    /// call retries it. Round 2 cached the failure and never recovered.
    pub fn shared() -> Arc<UnsavedWorkGuard> {
        SHARED
            .get_or_init(|| {
                let guard = Arc::new(UnsavedWorkGuard::new_isolated());
                if let Ok(cwd) = std::env::current_dir() {
                    guard.baseline_for_dir(&cwd);
                }
                guard
            })
            .clone()
    }

    /// A guard with its own empty state, sharing no pinned baseline with any
    /// other. For tests, and for callers that must not join the session-wide
    /// guard.
    pub fn new_isolated() -> Self {
        Self {
            dirs: Mutex::new(HashMap::new()),
            pins: Mutex::new(HashMap::new()),
            blobs: Mutex::new(HashMap::new()),
            authored: Mutex::new(HashMap::new()),
        }
    }

    /// Judge replacing `previous` (the bytes currently at `path`, read by the
    /// caller through whichever filesystem it is actually writing to) with
    /// `new_content`.
    ///
    /// Makes the recovery copy itself, and verifies it, before returning — so
    /// a caller acting on [`Verdict::ProceedWithNote`] cannot write ahead of
    /// the copy, and a note that mentions a copy always refers to one that has
    /// been read back.
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
        // `saved` is only what can be *proven* recorded. Where git will not
        // answer, nothing is proven and `unresolved` carries git's own reason;
        // the empty baseline that results is the safe direction, because it
        // makes every line count as unsaved and forces the drop to justify
        // itself below.
        let mut unresolved: Option<String> = None;
        let saved = match &baseline {
            Baseline::Repo {
                root,
                commit: Some(commit),
            } => match self.recorded_blob(path, root, commit) {
                Ok(text) => text,
                Err(why) => {
                    unresolved = Some(why);
                    String::new()
                }
            },
            // A repository with no commits yet, or no repository at all:
            // nothing about this file is recorded, and that is an answer.
            Baseline::Repo { commit: None, .. } | Baseline::NoRepo => String::new(),
            Baseline::Unknown(why) => {
                unresolved = Some(why.clone());
                String::new()
            }
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
            // Nothing leaves the disk. An unresolved baseline does not matter
            // here, which is what keeps a broken-git environment usable for
            // every write that only adds.
            return Verdict::Proceed;
        }

        // Fail closed. Without a baseline the partial/wholesale split is not
        // knowable either, so neither branch below may be taken on a guess.
        if let Some(why) = unresolved {
            return match mode {
                Mode::Rewrite => Verdict::Refuse(unresolved_refusal(
                    display_path,
                    previous,
                    &dropped,
                    dropped_total,
                    &why,
                )),
                Mode::Surgical => Verdict::ProceedWithNote(unresolved_note(dropped_total, &why)),
            };
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

        match self.object_store(&baseline, path) {
            Store::Owned(root) => match self.recoverable_copy(root, previous) {
                Ok(oid) => Verdict::ProceedWithNote(copy_note(
                    dropped_total,
                    mode,
                    root,
                    &oid,
                    wholesale,
                    objects_dir(root),
                )),
                Err(why) => Verdict::Refuse(format!(
                    "Refused to overwrite {display_path}: it holds {dropped_total} line(s) that \
                     are on disk and in no commit, and the copy that would make replacing them \
                     recoverable could not be made ({why}). Nothing was changed."
                )),
            },
            // A repository encloses the file but is not its archive. Copying
            // into it is the armD harm, so it is treated as no store at all.
            Store::Foreign { root, why } => match mode {
                Mode::Rewrite => Verdict::Refuse(foreign_store_refusal(
                    display_path,
                    dropped_total,
                    root,
                    &why,
                )),
                Mode::Surgical => {
                    Verdict::ProceedWithNote(foreign_store_note(dropped_total, root, &why))
                }
            },
            // No repository, so no object store, so nowhere to put a copy.
            Store::Absent => match mode {
                Mode::Rewrite => Verdict::Refuse(format!(
                    "Refused to overwrite {display_path}: this content would delete \
                     {dropped_total} line(s) that are on disk. This file is in no repository, so \
                     nothing about it is recorded anywhere and there is nowhere to put a copy \
                     that would make replacing them recoverable — losing them would be \
                     irreversible. Nothing was changed. Carry those lines into the content you \
                     write, or have the file recorded somewhere first."
                )),
                Mode::Surgical => Verdict::ProceedWithNote(format!(
                    "\nNote: {dropped_total} line(s) that were on disk are not in the new \
                     content. This file is in no repository, so no recovery copy was made and \
                     those lines are not recoverable."
                )),
            },
        }
    }

    /// Record what this tool just put on disk at `path`.
    ///
    /// Only the copies this tool actually introduced count as agent-authored.
    /// Copies that were already on disk stay the user's, however many times
    /// the agent's own writes carry them through — otherwise two writes would
    /// launder any line into being unprotected — and so does a second copy the
    /// user adds later of a line the agent wrote once, which is the
    /// granularity round 3 got wrong.
    pub fn note_written(&self, path: &Path, previous: &str, written: &str) {
        let before = tally(previous);
        let after = tally(written);
        let Ok(mut map) = self.authored.lock() else {
            return;
        };
        let entry = map.entry(path.to_path_buf()).or_default();
        let mut owned: HashMap<String, usize> = HashMap::new();
        for (line, now) in &after {
            let introduced = now.saturating_sub(before.get(line).copied().unwrap_or(0));
            let carried = entry.get(*line).copied().unwrap_or(0);
            // Never claim more copies than are on disk: a line the agent wrote
            // and later removed stops being agent-authored, so a user who
            // types that text back gets the protection.
            let count = (carried + introduced).min(*now);
            if count > 0 {
                owned.insert((*line).to_owned(), count);
            }
        }
        *entry = owned;
    }

    fn authored_lines(&self, path: &Path) -> HashMap<String, usize> {
        self.authored
            .lock()
            .ok()
            .and_then(|m| m.get(path).cloned())
            .unwrap_or_default()
    }

    fn baseline_for(&self, path: &Path) -> Baseline {
        match path.parent() {
            Some(dir) => self.baseline_for_dir(dir),
            None => Baseline::Unknown("the file has no parent directory".to_owned()),
        }
    }

    /// Resolve the baseline for `dir`, pinning it once it settles.
    fn baseline_for_dir(&self, dir: &Path) -> Baseline {
        if let Ok(map) = self.dirs.lock()
            && let Some(hit) = map.get(dir)
        {
            return hit.clone();
        }
        let resolved = self.resolve_baseline(dir);
        // Only an answer that cannot change is remembered, and the only such
        // answer is a pinned repository. Round 2 memoized `Unknown` — break
        // git -> allow, repair git -> still allow. Round 3 fixed that and then
        // memoized `NoRepo`, which it had just made a *refusing* state: no
        // repository -> refuse, `git init` + commit -> the same guard still
        // refuses. Both faults are repairable mid-session, so neither is
        // cached.
        if !matches!(resolved, Baseline::Repo { .. }) {
            return resolved;
        }
        if let Ok(mut map) = self.dirs.lock() {
            return map.entry(dir.to_path_buf()).or_insert(resolved).clone();
        }
        resolved
    }

    fn resolve_baseline(&self, dir: &Path) -> Baseline {
        // git exits 128 both for "there is no repository" and for "there is a
        // repository and I refuse to open it". Only the filesystem can tell
        // those apart without trusting git to be healthy.
        let present = repository_marker_present(dir);
        let unopenable = |why: String| {
            if present {
                Baseline::Unknown(why)
            } else {
                Baseline::NoRepo
            }
        };

        match git_run(dir, &["rev-parse", "--is-inside-work-tree"], None) {
            None => return unopenable("git could not be run".to_owned()),
            Some(run) if run.ok() => {
                if run.stdout_text().trim() != "true" {
                    // A bare repository or a path inside `.git`: no work tree,
                    // so nothing here is tracked in the sense this guard means.
                    return Baseline::NoRepo;
                }
            }
            Some(run) => return unopenable(run.why("git would not open this repository")),
        }

        let root = match git_run(dir, &["rev-parse", "--show-toplevel"], None) {
            Some(run) if run.ok() => {
                let text = run.stdout_text().trim().to_owned();
                if text.is_empty() {
                    return Baseline::Unknown("git named no repository root".to_owned());
                }
                PathBuf::from(text)
            }
            Some(run) => {
                return Baseline::Unknown(run.why("git would not name the repository root"));
            }
            None => return Baseline::Unknown("git could not be run".to_owned()),
        };

        if let Ok(map) = self.pins.lock()
            && let Some(commit) = map.get(&root)
        {
            return Baseline::Repo {
                root,
                commit: commit.clone(),
            };
        }

        // `--verify --quiet` exits 1 for "that ref does not exist", which is
        // an unborn HEAD and a real state, and 128 for git failing, which is
        // not. Measured on git 2.43.0: unborn HEAD exits 1; dubious ownership
        // and an unreadable config exit 128.
        let commit = match git_run(dir, &["rev-parse", "--verify", "--quiet", "HEAD"], None) {
            Some(run) if run.ok() => {
                let sha = run.stdout_text().trim().to_owned();
                if !is_hex_oid(&sha) {
                    return Baseline::Unknown("git returned no usable commit id".to_owned());
                }
                Some(sha)
            }
            Some(run) if run.code == Some(1) => None,
            Some(run) => return Baseline::Unknown(run.why("git would not resolve HEAD")),
            None => return Baseline::Unknown("git could not be run".to_owned()),
        };

        if let Ok(mut map) = self.pins.lock() {
            map.entry(root.clone()).or_insert_with(|| commit.clone());
        }
        Baseline::Repo { root, commit }
    }

    /// The file's content in the pinned commit, or `""` if that commit did not
    /// contain it. `Err(why)` only when git could not answer.
    fn recorded_blob(&self, path: &Path, root: &Path, commit: &str) -> Result<String, String> {
        // Derived from the repository root, never from `git ls-files`: the
        // index is mutable during the session, the pinned commit is not.
        let rel = repo_relative(root, path).ok_or_else(|| {
            "the file's path inside the repository could not be resolved".to_owned()
        })?;
        let key = (commit.to_owned(), rel.clone());
        if let Ok(cache) = self.blobs.lock()
            && let Some(hit) = cache.get(&key)
        {
            return Ok(hit.clone());
        }

        // `ls-tree` reports "not in that commit" as exit 0 with no output, so
        // an absent path never has to be told apart from a broken repository
        // by its exit code. `git show <commit>:<path>` cannot do that: it
        // exits 128 for both, which is the same conflation as B1.
        let listing = git_run(
            root,
            &["ls-tree", "--full-tree", "-z", commit, "--", &rel],
            None,
        )
        .ok_or_else(|| "git could not be run".to_owned())?;
        if !listing.ok() {
            return Err(listing.why("git would not read the pinned commit"));
        }
        let text = match blob_oid(&listing.stdout_text()) {
            None => String::new(),
            Some(oid) => {
                let blob = git_run(root, &["cat-file", "blob", &oid], None)
                    .ok_or_else(|| "git could not be run".to_owned())?;
                if !blob.ok() {
                    return Err(blob.why("git would not read the recorded contents"));
                }
                String::from_utf8_lossy(&blob.stdout).into_owned()
            }
        };
        if let Ok(mut cache) = self.blobs.lock() {
            cache.insert(key, text.clone());
        }
        Ok(text)
    }

    /// Which object store, if any, may hold this file's prior bytes.
    ///
    /// An enclosing repository qualifies only when it is plainly this file's
    /// archive: the file is not one the repository is configured to ignore,
    /// and the pinned commit records something under the file's own directory
    /// (or the file sits at the repository root). See the module docs for the
    /// measured armD shape and the ignored-secret shape this exists to refuse.
    fn object_store<'a>(&self, baseline: &'a Baseline, path: &Path) -> Store<'a> {
        let Baseline::Repo { root, commit } = baseline else {
            return Store::Absent;
        };
        let root = root.as_path();
        let Some(rel) = repo_relative(root, path) else {
            return Store::Foreign {
                root,
                why: "cannot place this file inside itself".to_owned(),
            };
        };
        // The user has already said, in this repository's own configuration,
        // that this file does not belong in it. A recovery copy is verbatim by
        // construction, so where it goes is the only lever there is.
        if self.repository_ignores(root, &rel) {
            return Store::Foreign {
                root,
                why: "is configured to ignore this file".to_owned(),
            };
        }
        let Some((dir, _)) = rel.rsplit_once('/') else {
            // At the repository root: unambiguously this repository's.
            return Store::Owned(root);
        };
        let unrecorded = || Store::Foreign {
            root,
            why: format!("records nothing under {dir}"),
        };
        // With no commit nothing is recorded anywhere, so nothing records this
        // subdirectory either.
        let Some(commit) = commit else {
            return unrecorded();
        };
        let pathspec = format!("{dir}/");
        match git_run(
            root,
            &["ls-tree", "--full-tree", "-z", commit, "--", &pathspec],
            None,
        ) {
            // Non-empty output is the only proof the directory is recorded.
            // git failing to answer is not proof, and lands in the safe
            // direction: no copy goes anywhere.
            Some(run) if run.ok() && !run.stdout.is_empty() => Store::Owned(root),
            _ => unrecorded(),
        }
    }

    /// Does this repository's configuration say to ignore `rel`?
    ///
    /// `check-ignore` exits 0 for ignored, 1 for not ignored, and something
    /// else when it could not decide. Only a definite 1 is read as "not
    /// ignored": anything else means the question is open, and an open
    /// question must not end with the user's bytes inside the repository.
    /// Measured on git 2.43.0: it consults the index, so a *tracked* file
    /// matched by an ignore rule exits 1 and keeps its own repository.
    ///
    /// It is also the one command here that **rejects** `--literal-pathspecs`
    /// ("pathspec magic not supported by this command: 'literal'", exit 128 —
    /// measured, and it broke every arm of this suite on the first attempt).
    /// The path therefore goes in through `--stdin -z`, which takes it as a
    /// pathname rather than an option. A file whose own name looks like
    /// pathspec magic still exits 128 there, which lands on "ignored" and so
    /// on "no copy" — the safe direction.
    fn repository_ignores(&self, root: &Path, rel: &str) -> bool {
        let mut payload = rel.as_bytes().to_vec();
        payload.push(0);
        match git_invoke(
            root,
            Pathspecs::AsGitTakesThem,
            &["check-ignore", "-q", "--stdin", "-z"],
            Some(&payload),
        ) {
            Some(run) if run.code == Some(1) => false,
            _ => true,
        }
    }

    /// Put `bytes` in the repository's own object database and prove they can
    /// be read back before returning.
    ///
    /// Nothing is referenced, staged or committed: the result is a loose,
    /// unreferenced object, which is why the user's own `git gc` is a
    /// sufficient retention policy and no new one is invented here.
    fn recoverable_copy(&self, root: &Path, bytes: &str) -> Result<String, String> {
        if bytes.len() > MAX_RECOVERY_BYTES {
            return Err(format!(
                "file is {} bytes, over the {MAX_RECOVERY_BYTES}-byte recovery limit",
                bytes.len()
            ));
        }
        let written = git_run(
            root,
            &["hash-object", "-w", "--stdin"],
            Some(bytes.as_bytes()),
        )
        .ok_or_else(|| "git could not be run".to_owned())?;
        if !written.ok() {
            return Err(written.why("git would not write the object"));
        }
        let oid = written.stdout_text().trim().to_owned();
        if !is_hex_oid(&oid) {
            return Err("git returned no usable object id".to_owned());
        }

        // Never claim recoverability that has not been exercised. Round 2
        // returned early from an `exists()` probe, so a directory sitting at
        // the target path produced a tool result telling the user "nothing is
        // lost" and naming something unreadable. The copy is read back here,
        // through the same command the user would run, and compared.
        let back = git_run(root, &["cat-file", "blob", &oid], None)
            .ok_or_else(|| "git could not be run to verify the copy".to_owned())?;
        if !back.ok() {
            return Err(back.why("the copy could not be read back"));
        }
        if back.stdout != bytes.as_bytes() {
            return Err(format!(
                "the copy read back as {} bytes rather than {}",
                back.stdout.len(),
                bytes.len()
            ));
        }
        Ok(oid)
    }
}

/// Is a repository plausibly present for `dir`, judged without asking git?
///
/// Deliberately errs towards "yes": anything other than a definite "no such
/// entry" counts as present, because the consequence of a false "no" is the
/// fail-open this whole module exists to close.
///
/// `GIT_DIR` and `GIT_WORK_TREE` are deliberately *not* consulted. Round 3
/// read either of them as proof of a repository, and git honoured the same
/// variables, so a file in no repository at all was classified as being in one
/// and its prior bytes landed in an unrelated repository. [`git_run`] now
/// clears them, so git answers for `dir` alone and so does this.
fn repository_marker_present(dir: &Path) -> bool {
    let start = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    // Upward, and that is the whole basis of the test: a `.git` marker for
    // `dir` normally lives several levels above it. Probe only `dir` itself
    // and every subdirectory of a repository git refuses to open classifies as
    // `NoRepo`, which is B1 re-opened.
    for ancestor in start.ancestors() {
        if marker_probe_is_present(std::fs::symlink_metadata(ancestor.join(".git"))) {
            return true;
        }
    }
    false
}

/// How one probe of a candidate `.git` marker is read.
///
/// `.git` is a directory in a normal clone and a file in a worktree or
/// submodule, so the kind is not checked. Only `NotFound` is evidence of
/// absence: a probe that fails for any other reason — a permission denied on
/// the parent, an I/O error, a non-directory component — must not be read as
/// "there is no repository here", because that is the fail-open direction this
/// module exists to close.
fn marker_probe_is_present(probe: std::io::Result<std::fs::Metadata>) -> bool {
    match probe {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

fn is_hex_oid(s: &str) -> bool {
    s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The blob id from the first `ls-tree -z` record, if that record is a blob.
fn blob_oid(record: &str) -> Option<String> {
    let entry = record.split('\0').next()?;
    let (meta, _path) = entry.split_once('\t')?;
    let mut fields = meta.split_whitespace();
    let _mode = fields.next()?;
    if fields.next()? != "blob" {
        return None;
    }
    let oid = fields.next()?;
    is_hex_oid(oid).then(|| oid.to_owned())
}

/// `path` expressed the way a tree lookup wants it: relative to the repository
/// root, forward-slashed, on every platform.
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

/// As [`tally`], minus the copies this tool itself introduced.
///
/// Subtracts a count rather than deleting a key. Round 3 deleted the key, so
/// one agent-written line of text disarmed that text for the rest of the
/// session in that file, including copies the user typed afterwards.
fn tally_excluding<'a>(
    text: &'a str,
    authored: &HashMap<String, usize>,
) -> HashMap<&'a str, usize> {
    let mut counts = tally(text);
    counts.retain(|line, remaining| {
        let exempt = authored.get(*line).copied().unwrap_or(0);
        *remaining = remaining.saturating_sub(exempt);
        *remaining > 0
    });
    counts
}

/// The dropped lines, scrubbed, for quoting back in a refusal.
fn quote_dropped(previous: &str, dropped: &HashMap<&str, usize>, dropped_total: usize) -> String {
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
    format!("{}{tail}", quoted.join("\n"))
}

/// The partial-rewrite refusal: the measured silent-loss shape.
///
/// It deliberately names no other tool — round 1 shipped a message
/// recommending Edit, and the model took that route on the first refusal in
/// both live adversarial arms — and it no longer says "reproduce those lines",
/// which was actively wrong guidance for a rewrite that *transforms* them: the
/// adversarial seat measured a legitimate symbol rename being told to undo
/// itself.
fn refusal_text(
    display_path: &str,
    previous: &str,
    dropped: &HashMap<&str, usize>,
    dropped_total: usize,
) -> String {
    format!(
        "Refused to overwrite {display_path}: this content would delete {dropped_total} line(s) \
         that are on disk but in no commit. That is unsaved work which exists nowhere else, so \
         losing it is irreversible.\n\
         Lines that would be lost:\n{quoted}\n\
         The rest of this file IS committed, so this is not the whole-file replacement the user \
         asked for — it is a partial rewrite that would silently drop their in-progress work. \
         Read the file as it stands on disk now and carry those lines into the content you write \
         — in their changed form if what you are doing changes them. If the user genuinely asked \
         for those specific lines to go, they must be recorded somewhere first.",
        quoted = quote_dropped(previous, dropped, dropped_total),
    )
}

/// The fail-closed refusal: git would not say what is saved, so nothing is
/// assumed to be.
fn unresolved_refusal(
    display_path: &str,
    previous: &str,
    dropped: &HashMap<&str, usize>,
    dropped_total: usize,
    why: &str,
) -> String {
    format!(
        "Refused to overwrite {display_path}: this content would delete {dropped_total} line(s) \
         that are on disk, and the last saved version of this file could not be established, so \
         there is no way to tell whether any of them exist anywhere else. git did not answer: \
         {why}\n\
         Lines that would be lost:\n{quoted}\n\
         Nothing was changed. Fix the reason git could not answer, or carry those lines into the \
         content you write. Writes that do not remove existing lines are unaffected.",
        quoted = quote_dropped(previous, dropped, dropped_total),
    )
}

fn unresolved_note(dropped_total: usize, why: &str) -> String {
    format!(
        "\nNote: {dropped_total} line(s) that were on disk are not in the new content, and the \
         last saved version of this file could not be established (git did not answer: {why}), \
         so it is not known whether they exist anywhere else. No recovery copy was made."
    )
}

/// The armD refusal: a repository does enclose this file, but it is not this
/// file's archive, so its prior bytes are not going into it and it is told
/// before the write rather than after.
fn foreign_store_refusal(
    display_path: &str,
    dropped_total: usize,
    root: &Path,
    why: &str,
) -> String {
    format!(
        "Refused to overwrite {display_path}: this content would delete {dropped_total} line(s) \
         that are on disk and in no commit. The only object store that could hold a recovery \
         copy is the git repository at {root}, and that repository {why} — it encloses this \
         file but is not its archive, so filing this file's private contents into it would put \
         them somewhere the user does not think of as holding them, and somewhere a later \
         `git clone` of this path would carry them. Nothing was changed and nothing was copied. \
         Carry those lines into the content you write, or have this file recorded somewhere \
         that does track it.",
        root = root.display(),
    )
}

fn foreign_store_note(dropped_total: usize, root: &Path, why: &str) -> String {
    format!(
        "\nNote: {dropped_total} line(s) that were on disk are not in the new content. The only \
         repository enclosing this file is {root}, which {why}, so it is not this file's \
         archive and no recovery copy was put there. Those lines are not recoverable.",
        root = root.display(),
    )
}

fn copy_note(
    dropped_total: usize,
    mode: Mode,
    root: &Path,
    oid: &str,
    wholesale: bool,
    objects: Option<String>,
) -> String {
    let why = if wholesale && mode == Mode::Rewrite {
        " None of this file was in any commit, so the whole of it counted as unsaved work."
    } else {
        ""
    };
    // Never `<root>/.git/objects`: in a linked worktree or a submodule `.git`
    // is a *file* and the store belongs to the main repository, so that path
    // names a directory that does not exist. Measured on git 2.43.0.
    let store = objects.unwrap_or_else(|| "this repository's own object store".to_owned());
    format!(
        "\nNote: {dropped_total} line(s) that were on disk and in no commit are not in the new \
         content.{why} The previous contents were written to this repository's own object store \
         and read back to confirm they match, so they can be recovered with:\n    \
         git -C {root} cat-file blob {oid}\n\
         The object is unreferenced, but that does not make it short-lived: `git gc` does NOT \
         remove it — it moves it into a cruft pack and it stays readable. Disposing of it takes \
         `git -C {root} gc --prune=now`; an ordinary gc only prunes it once gc.pruneExpire (two \
         weeks by default) has passed, and `git gc --auto` will not fire for one object at all.\n\
         Until then these bytes live in {store}, and they are in no commit, so that \
         is the only place they exist — whether this file is gitignored, merely untracked, or \
         tracked and wholly rewritten. They travel with a filesystem copy of the repository \
         (cp -a, tar, rsync) and with `git clone` of this local path, and `git fsck --lost-found` \
         writes them out as a plaintext file; `git push` and `git bundle` do not carry them.",
        root = root.display(),
    )
}

/// Where this repository actually keeps its objects, asked of git rather than
/// assumed from `root`.
///
/// `<root>/.git/objects` is wrong whenever `.git` is a file: a linked worktree
/// and a submodule both keep their objects in the main repository's store, and
/// naming the worktree's own `.git` sends the user to a path that is not a
/// directory. `None` when git will not answer — the note then says "this
/// repository's own object store" rather than a path that might be a lie.
fn objects_dir(root: &Path) -> Option<String> {
    let run = git_run(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "objects",
        ],
        None,
    )?;
    if !run.ok() {
        return None;
    }
    let text = run.stdout_text().trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// One `git` invocation, with enough of its result kept to classify it.
struct GitRun {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

impl GitRun {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }

    fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// A short, scrubbed reason taken from git's own first line of stderr, so
    /// the user is told the actual remedy (`safe.directory`, a bad config
    /// line) rather than a generic failure.
    fn why(&self, fallback: &str) -> String {
        let first = self
            .stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or_default();
        if first.is_empty() {
            return fallback.to_owned();
        }
        let scrubbed = wcore_safety::PIIScrubber.scrub(first);
        if scrubbed.chars().count() > MAX_GIT_REASON_CHARS {
            return scrubbed
                .chars()
                .take(MAX_GIT_REASON_CHARS)
                .collect::<String>()
                + "…";
        }
        scrubbed.into_owned()
    }
}

/// Run `git` in `dir`, returning `None` only when it could not be started.
///
/// Argv mode throughout — no shell interpreter is involved, so an LLM-supplied
/// file name containing `;`, `$()` or a backtick reaches git as literal bytes.
/// `--literal-pathspecs` stops such a name being read as a pattern, `--`
/// separates it from options, and `core.fsmonitor=false` plus
/// `GIT_TERMINAL_PROMPT=0` stop a repository's own config from starting a
/// helper process or blocking this guard on a prompt.
///
/// The ambient git environment is **removed**, not inherited, because every
/// variable in it relocates something this guard depends on. Measured: with
/// `GIT_OBJECT_DIRECTORY` set — the shape a hook inherits inside a push
/// quarantine — `hash-object -w` and the `cat-file` read-back both redirect to
/// the same non-repository store, so the byte-for-byte check passes, the write
/// proceeds, and the `git -C <root> cat-file blob <oid>` the note advertises
/// fails: allow, plus a recovery claim that does not recover. With `GIT_DIR`
/// set, a file in no repository at all is classified as being in one and its
/// prior bytes land in an unrelated repository. `GIT_COMMON_DIR`,
/// `GIT_WORK_TREE` and `GIT_ALTERNATE_OBJECT_DIRECTORIES` are the same two
/// failures by another name, and `GIT_QUARANTINE_PATH` is how the object
/// variables usually arrive.
///
/// Declared deviation: this is `std::process::Command`, not
/// `wcore_config::shell::shell_command_argv`. The injection property is
/// identical — argv mode, `--literal-pathspecs`, `--` before every path — but
/// round 3's stated reason for the deviation was wrong. It said the call sites
/// are sync; the registration site `BootstrapBuilder::build_scoped` *is* an
/// `async fn` and `Tool::execute` is async too. The accurate statement is that
/// `assess` is a sync fn and the eager pin runs inside `OnceLock::get_or_init`,
/// which cannot await. So the real cost of the deviation is blocking `git`
/// spawns on a tokio worker — up to three at bootstrap for the eager pin —
/// with no timeout on them. A timeout is a fair follow-up.
fn git_run(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Option<GitRun> {
    git_invoke(dir, Pathspecs::Literal, args, stdin)
}

/// Whether `--literal-pathspecs` can be passed to this command at all.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pathspecs {
    /// The default, and what every path-taking command here uses.
    Literal,
    /// `check-ignore` rejects the option outright, so it takes its path on
    /// stdin instead. Nothing in this mode may pass a path in argv.
    AsGitTakesThem,
}

fn git_invoke(
    dir: &Path,
    pathspecs: Pathspecs,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> Option<GitRun> {
    let mut cmd = Command::new("git");
    if pathspecs == Pathspecs::Literal {
        cmd.arg("--literal-pathspecs");
    }
    cmd.args(["-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat")
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_QUARANTINE_PATH")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });

    let mut child = cmd.spawn().ok()?;
    if let Some(data) = stdin {
        // Fed from its own thread: git can fill its stdout pipe before it has
        // consumed all of stdin, and a single-threaded write would deadlock.
        // The handle is moved in, so it closes — signalling EOF — when the
        // write finishes.
        if let Some(mut sink) = child.stdin.take() {
            let owned = data.to_vec();
            std::thread::spawn(move || {
                let _ = sink.write_all(&owned);
            });
        }
    }
    let out = child.wait_with_output().ok()?;
    Some(GitRun {
        code: out.status.code(),
        stdout: out.stdout,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests;
