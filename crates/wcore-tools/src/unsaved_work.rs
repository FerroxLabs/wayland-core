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
//! > own object store, *read back byte-for-byte*, and *anchored under a ref*
//! > that keeps them reachable through `git gc --prune=now` and listable by
//! > `git for-each-ref`. Where no such copy can be made, or can be made but
//! > not kept, the write is refused.**
//!
//! Nothing in that sentence depends on the model choosing to cooperate, and
//! nothing in it is claimed when it has not been checked. Three limits are part
//! of the statement, not footnotes to it:
//!
//! * It covers **all three write surfaces, but not equally**. `Write` is
//!   refused-or-copied as above. `Edit` is never refused for *dropping* a
//!   line (see below) and never claims a copy it did not make; the one thing
//!   that does refuse an Edit is a copy that was attempted and failed
//!   outright, where proceeding would destroy the line with nothing to show
//!   for it. **`Bash` is covered for one shape only**:
//!   a git command whose whole purpose is to throw the work tree away is
//!   refused by [`shell_refusal`], from every `BashTool` entry point, before
//!   any shell is spawned — see "The shell surface" below. `rm` is refused
//!   there too, for the paths it would actually take. Everything else a shell
//!   can do to a file — `sed -i '2d'`, `>`, `mv`, `truncate` — still does not
//!   route through here and cannot at this altitude; in the round-1
//!   `adv-armB` arm `sed -i`, not Edit, is what actually destroyed the line.
//!   A guarantee described as holding "at the tool layer" without that
//!   carve-out would be false.
//! * **`GitTool` is covered as well**, by [`staging_verdict`] and
//!   [`stash_refusal`] — see `unsaved_work::git_ops`. It has to be: under the
//!   STRICT sandbox `git` cannot run from `Bash` at all, so the surface the
//!   product routes the model onto was the unguarded one.
//! * It is a guarantee about **dropped** lines, and this module cannot tell a
//!   dropped line from a **modified** one. A whole-file transformation that
//!   renames a symbol occurring on an unsaved line reads here as a drop and is
//!   refused.
//! * **Non-UTF-8 files** have no line model, so nothing can be proven about
//!   which of their bytes are recorded. They are therefore **refused**, not
//!   waved through, with one exception that proves the whole question at
//!   once: bytes byte-for-byte identical to the pinned commit are recorded,
//!   so replacing them loses nothing. Round 4 read them — and every other
//!   pre-image failure, including a permission denied — as an *empty*
//!   pre-image and skipped the check outright.
//! * **The window between reading the pre-image and writing is narrowed, not
//!   closed.** The assessment runs about five `git` processes, measured at
//!   13.5 ms, and a save landing inside that window was destroyed 12 times out
//!   of 12 while the note claimed otherwise. The file is now re-read
//!   immediately before the write lands and the write refused if it moved, so
//!   nothing older than one syscall is ever acted on. Closing the window
//!   entirely needs a lock the user's editor would have to take too.
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
//! The object store needs no new directory and no new mode bits, and it is
//! self-documenting: `git cat-file blob <oid>` is the whole recovery
//! procedure. What it is **not** is automatically the user's own security
//! domain, which is what round 4 claimed. Measured: a `0600` file's bytes
//! become a `0444` object under a `0755` directory, so a private file would be
//! copied into a readable one. Under `%USERPROFILE%` on Windows,
//! `.git\objects` inherits `(I)(OI)(CI)(RX)` for both AppContainer package
//! SIDs. In a linked worktree or a submodule the object is not even in the
//! tree the user is working in.
//!
//! So placement is no longer argued, it is **proven**, per write — see
//! [`UnsavedWorkGuard::object_store`]. A copy is made only where it can be
//! shown to be no more exposed than the file it copies, and where that cannot
//! be shown the write is refused and nothing is copied. Refusing is always
//! safe: nothing is lost and no secret spreads.
//!
//! # The copy is anchored, because an unreferenced one is not a backup
//!
//! Round 5 stopped at the object and argued that the user's own `git gc` was
//! therefore a sufficient retention policy. It measured that argument only
//! against a *default* gc, and it was wrong on both halves of what a backup
//! has to be.
//!
//! **Durability.** `gc.pruneExpire` defaults to two weeks and
//! `git gc --prune=now` disposes of an unreferenced object at once — measured
//! on git 2.43.0: the copy is gone and `git cat-file blob <oid>` answers
//! `bad file`. That is the red arm of `unsaved_work_durable_test`.
//! **Discoverability.** Nothing referenced it, so it appeared in no
//! `git log`, no `git stash list` and no `git for-each-ref`. A user who lost
//! the terminal scrollback carrying the object id had `git fsck --lost-found`
//! and nothing else. Disclosing all of that honestly, which round 5 did, does
//! not make an expiring invisible copy into a recovery.
//!
//! So every copy is now anchored under `refs/wayland-core/unsaved/`, as an
//! **annotated tag** — see [`anchor_copy`] for the measurements that chose a
//! tag over a commit and over a bare blob ref. It survives
//! `git gc --aggressive --prune=now`, `git fsck` stays clean, and
//! `git for-each-ref --sort=-creatordate refs/wayland-core/unsaved/` lists
//! every copy with its date and the file it came from, so the object id is
//! not something the user has to have kept.
//!
//! Nothing here ever deletes one of those refs. An automatic policy that
//! discarded the wrong one would be this module's own failure mode wearing a
//! schedule, so retention is explicit instead: the note names the listing
//! command and the two-command deletion. The cost is disk, bounded by the
//! distinct pre-images actually preserved — git stores an object once, so
//! re-preserving identical bytes adds a ref and no object.
//!
//! **Anchoring widens where the bytes travel, and that is stated rather than
//! glossed.** Measured on git 2.43.0, before and after. Unchanged: they go
//! with a filesystem copy of the repository (`cp -a`, `tar`, `rsync`), and
//! `git push`, `git push --all`, `git push --tags` and `git push --follow-tags`
//! do not carry them. Newly carried, because a ref is now what git packs
//! against: `git clone --mirror`, `git push --mirror` and
//! `git bundle --all` — none of which took the dangling object. Newly *not*
//! carried: a plain `git clone` of this local path, which copies the objects
//! but not the ref, so the clone's first `git gc --prune=now` drops them; and
//! `git fsck --lost-found`, which no longer materialises them as a plaintext
//! file under `.git/lost-found/other/` because they are no longer dangling.
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
//! is one rule: copy only where the copy is provably no more exposed than the
//! file itself. Its clauses are listed on [`UnsavedWorkGuard::object_store`]
//! — an ignored file, a repository that is not this file's archive, a store
//! outside this work tree, a copy with wider permissions than the original,
//! and a platform where none of that can be measured. Each of them refuses
//! rather than copies, and they share one refusal message because they are
//! one rule.
//!
//! Two of those clauses exist because a copy was actually made and should not
//! have been. Round 3 filed a gitignored `.env` into `.git/objects`, and that
//! is not a wash: the object travels with `git clone <path>` and
//! `git fsck --lost-found` materialises it as plaintext, so a copy the user
//! believes their ignore rules filtered carries the key. Round 4 then decided
//! "ignored" with `git check-ignore`, which consults the **index** — so one
//! `git add -f`, a command the agent itself can run, filed the same secret
//! anyway. The question asked now is about the ignore *rules* alone
//! (`--no-index`), which are committed configuration.
//!
//! On **Windows** the permission clause cannot be evaluated at all from here,
//! so the copy is not made. That is a real cost — a Windows agent gets a
//! refusal where a Unix one gets a recoverable copy — and it is the honest
//! position until an ACL comparison exists: the measurement that matters
//! (whether the file itself already carries those same inherited ACEs) is one
//! this code cannot take.
//!
//! # Edit
//!
//! Edit is guarded too, and it is never refused for dropping a line. Two
//! reasons, both measured:
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
//! where it cannot it says so instead of pretending otherwise. It gets the
//! **same floor as Write**: the same pinned baseline, the same object store
//! rule, and the same anchored ref. That equality is the point. A model
//! refused on Write can reach the same file through Edit, so if Edit's copy
//! were the weaker one the Write refusal would be routing traffic onto it —
//! and before anchoring, Edit's copy *was* the weaker one, because it was the
//! expiring one. It is now the same copy, and
//! `unsaved_work_durable_test::the_edit_path_a_refused_write_reroutes_to_is_no_weaker`
//! grades that by running the reroute.
//!
//! What stays asymmetric is that Write also **refuses**, and that is
//! deliberate. Prevention beats recovery: a refusal puts the dropped lines
//! back in front of the model, which repairs the file, where a note only
//! files them away. Edit cannot be refused for a drop without becoming
//! unusable on a dirty tree, and Write can, because a whole-file rewrite that
//! silently omits lines recorded nowhere is the measured harm itself. The two
//! surfaces now differ in whether the loss is *prevented*, never in whether
//! the bytes are *kept*.
//!
//! One case does refuse an Edit, and it is not a drop: a copy that was
//! attempted and failed. There, proceeding would destroy the line and have
//! nothing to show for it.
//!
//! # The shell surface
//!
//! Measured defect (job corpus row B-1, 2026-08-11, case `k5-after`): the
//! agent finished the job, noticed it had touched `SHIPPING-API.md`, and
//! tidied up with `git checkout -- SHIPPING-API.md`. That file also carried a
//! line the user had never committed anywhere, and the revert took it. The
//! Write guard above saw nothing, because Write was never called.
//!
//! So the same question — *would this destroy a line that exists nowhere
//! else?* — is asked of the command itself, by [`shell_refusal`], before the
//! shell is spawned. It reads the same pinned baseline, the same recorded
//! blobs and the same agent-authored tallies as Write, because both surfaces
//! hold the one [`UnsavedWorkGuard::shared`] instance. That sharing is also
//! what keeps the agent free to revert a file it wrote itself this session:
//! those lines are attributed to the tool, so the file holds no unsaved *user*
//! work and there is nothing to refuse. The attribution is per line and per
//! copy, so a user line sitting in a file the agent also wrote to is still the
//! user's and is still defended.
//!
//! The scope is deliberately narrow, and stated as plainly as the rest:
//!
//! * Only five git subcommands are inspected — `checkout`, `restore`,
//!   `stash`, `clean` and `reset` — and `reset` only in its `--hard` form,
//!   because a mixed or soft reset keeps the work tree. `git commit`,
//!   `git add`, `git switch -c` and every other git command are untouched: a
//!   guard that reads as a general git ban is noise rather than signal.
//! * A discarding command that names paths is judged on those paths. One that
//!   names none (`git checkout -- .`, `git stash`, `git reset --hard`) reaches
//!   the whole work tree and is judged on every tracked path git reports as
//!   modified.
//! * **`git clean` is the gap.** It is the one discard whose victims are
//!   *untracked* files, and its bare `git clean -fd` form names no path, so
//!   there is nothing to enumerate and nothing is refused. A `clean` that
//!   names an existing path is checked like any other discard.
//! * The segmenter finds a `git` after `&&`, `||`, `;`, `|` or a newline and
//!   honours `-C`, but it is not a shell parser and does not try to be. A
//!   discard it fails to see is a refusal that does not fire; it is never a
//!   refusal that fires wrongly. That is the direction the error has to fall
//!   for something sitting in front of every Bash call.
//! * When the shell's own directory is in no work tree, git would refuse the
//!   command anyway, so nothing is claimed and nothing is blocked. As
//!   everywhere else here, that is answered from the `.git` marker on the
//!   filesystem and never from a git exit code.
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
//! * **Agent-authored lines expire when the file moves underneath the tool.**
//!   The tally is only meaningful while the file still is what this tool last
//!   wrote, and the premise of this whole guard is that the user is editing
//!   the same files. Measured: the agent writes `log('start')`, the user edits
//!   the file in their editor, and a later rewrite drops that line silently on
//!   the strength of a write two states ago. Once `previous` is not what the
//!   tool last left there, nothing on disk can tell the agent's line from the
//!   same text the user typed, so the whole file is the user's again until the
//!   tool writes once more.
//! * **The ambient git environment is removed, not inherited.** `GIT_DIR`,
//!   `GIT_COMMON_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`,
//!   `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES` and
//!   `GIT_QUARANTINE_PATH` are cleared for every invocation; see [`git_run`]
//!   for what each of them broke.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

mod git_ops;
mod shell;

pub use git_ops::{Staging, staging_verdict, stash_refusal};
pub use shell::shell_refusal;

/// Most dropped lines quoted back in a refusal message.
const MAX_QUOTED_LINES: usize = 5;

/// Largest file this guard will copy. An overwrite that would drop unrecorded
/// content from a file bigger than this is refused rather than allowed
/// unprotected — the guarantee has no size exemption.
const MAX_RECOVERY_BYTES: usize = 16 * 1024 * 1024;

/// Longest reason quoted back from git's own stderr.
const MAX_GIT_REASON_CHARS: usize = 200;

/// Ref namespace every preserved pre-image is anchored under.
///
/// An unreferenced object is not a backup. It appears in no `git log`, no
/// `git stash list` and no `git for-each-ref`; the only route back to it is
/// the object id in a tool result the user may never see, or
/// `git fsck --lost-found`. And it expires: `gc.pruneExpire` defaults to two
/// weeks and `git gc --prune=now` removes it at once. Under a ref it is
/// reachable, so gc keeps it, and it is listable, so the id is not something
/// the user has to have retained.
const UNSAVED_REF_PREFIX: &str = "refs/wayland-core/unsaved";

/// Identity written onto the anchor object.
///
/// Written into the object rather than read from `user.name`/`user.email`,
/// which is why the anchor is a tag and not a commit: measured on git 2.43.0
/// in a repository with no identity configured, `commit-tree` exits non-zero
/// with "Author identity unknown" while `mktag` — which takes its tagger from
/// the object body — succeeds. Anchoring must not be the thing that fails in
/// a freshly `git init`ed tree.
const ANCHOR_TAGGER: &str = "wayland-core <unsaved-work@wayland-core.invalid>";

/// How the caller is replacing the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Whole-file replacement authored from the model's own picture of the
    /// contents (the Write tool). Omission is silent here, so a drop of
    /// content that cannot be proven recorded is refused unless it can be
    /// copied first.
    Rewrite,
    /// Targeted replacement of bytes the model quoted from disk (the Edit
    /// tool). Refused when it takes the user's unrecorded lines out as
    /// collateral to a change of recorded content; everywhere else a drop is
    /// copied where a copy is possible, and reported accurately either way.
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
    /// A store in which a copy is provably no more exposed than the file
    /// itself. `objects` is the directory git named, not one assumed from
    /// `root`.
    Owned { root: &'a Path, objects: PathBuf },
    /// No copy may be made. `why` is a full clause completing "and here it
    /// cannot: ...".
    Unproven { why: String },
    /// No repository, so no object store to reason about at all.
    Absent,
}

/// A copy of a file's prior bytes that has been made and read back.
struct Preserved {
    /// The blob. Read back byte-for-byte before this value existed.
    oid: String,
    /// The ref anchoring it, or why it could not be anchored. Unanchored is
    /// the round-5 state: a real, verified copy that `git gc --prune=now`
    /// removes and that no command lists.
    anchor: Result<String, String>,
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
    /// Path -> the exact bytes this tool last left there. The tally above is
    /// only meaningful while the file still is what the tool wrote; the whole
    /// premise of this guard is that the user edits the same files, so it
    /// often is not. Stored in full rather than hashed: an exact comparison
    /// has no collision to reason about, and the map only ever holds files
    /// this tool has written this session.
    last_written: Mutex<HashMap<PathBuf, String>>,
}

static SHARED: OnceLock<Arc<UnsavedWorkGuard>> = OnceLock::new();

/// One spelling for every path this guard uses as a map key.
///
/// The two tools reach the guard with the same file spelled differently on
/// Windows. `WriteTool` is handed the model's own `C:\...` path, the spelling
/// `AgentBootstrap` puts in the prompt. `BashTool` derives its candidates from
/// `WorkspacePolicy::root()`, and that root went through `canon`, which
/// canonicalizes and so yields the verbatim `\\?\C:\...` form. Keyed raw, the
/// authorship a write records is invisible to the shell lookup that has to
/// honour it, and the agent is refused permission to delete or revert a file it
/// wrote itself.
///
/// `dunce::simplified` is a pure string reduction — no I/O, no symlink
/// resolution, no filesystem authority — so it cannot move a containment
/// boundary; the workspace roots keep their canonical spelling untouched. It is
/// a no-op on Unix, where the two spellings already coincide. And it declines to
/// strip the prefix for reserved DOS names and over-long paths, exactly the
/// cases where `\\?\C:\CON` and `C:\CON` are genuinely different files, so
/// normalizing can never merge two distinct paths into one key.
fn key(path: &Path) -> PathBuf {
    dunce::simplified(path).to_path_buf()
}

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
            last_written: Mutex::new(HashMap::new()),
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
        let authored = self.authored_lines(path, previous);
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

        // Is the drop collateral, or is it the point?  A line that the commit
        // records, that was on disk, and that the new content keeps fewer
        // copies of, means this edit is rewriting recorded content as well as
        // taking the user's unrecorded lines out.  Rewording the user's own
        // in-progress line touches nothing recorded and leaves this false.
        let previous_all = tally(previous);
        let mut recorded_touched = false;
        for (text, prev_copies) in &previous_all {
            let in_commit = recorded.get(*text).copied().unwrap_or(0);
            if in_commit == 0 {
                continue;
            }
            let kept = surviving.get(*text).copied().unwrap_or(0);
            if kept < (*prev_copies).min(in_commit) {
                recorded_touched = true;
                break;
            }
        }

        // A Surgical edit that puts nothing back is not a rename, a reflow or
        // any of the modifications this module admits it cannot tell from a
        // drop - it is a deletion, and Edit quoted every line it deletes from
        // disk. Deleting the user's own unrecorded lines is the measured harm
        // itself (job corpus row A-2, 2026-08-11: two Edits stripped the
        // in-progress line out of `README.md` and `src/receipts/parser.py`,
        // and a recovery copy is not what the guarantee asks for - the bytes
        // have to still be where the user left them). So this one shape
        // refuses. Every Edit that puts a line back keeps the never-refused
        // property the module documentation argues for, which is what keeps
        // Edit usable on a dirty tree.
        if mode == Mode::Surgical && adds_nothing(previous, new_content) {
            return Verdict::Refuse(deletion_refusal(
                display_path,
                previous,
                &dropped,
                dropped_total,
            ));
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

        // Measured, job corpus rows A-2 and A-8, 2026-08-12: every INV-2
        // "overwritten on disk" failure across 24 runs was an Edit whose
        // `old_string` spanned a line the user had on disk and in no commit
        // and whose `new_string` omitted it while rewriting the code around
        // it - 7 failures, 7 such Edits, and not one of them in any of the 17
        // runs that passed.  Scoping this refusal to `Mode::Rewrite` left Edit
        // as the open surface: the recovery copy was made and anchored, and
        // the bytes still left the disk.  A copy is not the guarantee.
        //
        // `recorded_touched` is what keeps this from becoming the over-refusal
        // the module already measured once: an Edit aimed AT the user's
        // unsaved line - rewording it, correcting it - changes nothing the
        // commit records and is still allowed, copied and noted.  Only an edit
        // that takes those lines out on its way to changing something else is
        // refused, and `refusal_text` tells the caller to carry them through.
        if !wholesale && (mode == Mode::Rewrite || recorded_touched) {
            return Verdict::Refuse(refusal_text(
                display_path,
                previous,
                &dropped,
                dropped_total,
            ));
        }

        match self.object_store(&baseline, path) {
            Store::Owned { root, objects } => {
                match self.recoverable_copy(root, previous, display_path, dropped_total) {
                    Ok(copy) => match &copy.anchor {
                        Ok(anchor) => Verdict::ProceedWithNote(copy_note(
                            dropped_total,
                            mode,
                            root,
                            &copy.oid,
                            anchor,
                            wholesale,
                            &objects,
                        )),
                        // The bytes were copied and verified, but nothing
                        // references them: `git gc --prune=now` disposes of
                        // them and no command lists them. A copy that expires
                        // is not the guarantee at the top of this file, so a
                        // rewrite does not get to proceed on one.
                        Err(why) => match mode {
                            Mode::Rewrite => Verdict::Refuse(unanchored_refusal(
                                display_path,
                                dropped_total,
                                why,
                            )),
                            Mode::Surgical => Verdict::ProceedWithNote(unanchored_note(
                                dropped_total,
                                root,
                                &copy.oid,
                                why,
                            )),
                        },
                    },
                    Err(why) => Verdict::Refuse(format!(
                        "Refused to overwrite {display_path}: it holds {dropped_total} line(s) \
                         that are on disk and in no commit, and the copy that would make \
                         replacing them recoverable could not be made ({why}). Nothing was \
                         changed."
                    )),
                }
            }
            // The copy could not be proven no more exposed than the file, so
            // it is not made. See [`UnsavedWorkGuard::object_store`].
            Store::Unproven { why } => match mode {
                Mode::Rewrite => {
                    Verdict::Refuse(unproven_store_refusal(display_path, dropped_total, &why))
                }
                Mode::Surgical => {
                    Verdict::ProceedWithNote(unproven_store_note(dropped_total, &why))
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

    /// Judge replacing a file whose bytes on disk could not be read as text.
    ///
    /// Round 4 read *any* pre-image failure as an empty pre-image, and the
    /// caller then skipped the guard because the pre-image was empty.
    /// Measured as uid 65534 against a root-owned `0600` file in a writable
    /// directory: no refusal, no note, no copy, and the file went
    /// `root:root 0600` -> `nobody:nogroup 0644`.
    ///
    /// There is no line model for bytes that are not text, so almost nothing
    /// can be proven here — except the one fact that settles it outright: the
    /// bytes on disk are exactly the bytes the pinned commit records, so none
    /// of them are unsaved and replacing them loses nothing. Every other case
    /// is refused, including the one where the bytes could not be read at all.
    pub fn assess_opaque(
        &self,
        path: &Path,
        display_path: &str,
        on_disk: Option<&[u8]>,
        why: &str,
    ) -> Verdict {
        let refuse = || {
            Verdict::Refuse(format!(
                "Refused to overwrite {display_path}: its current contents could not be read \
                 ({why}), so there is no way to tell whether anything in them exists anywhere \
                 else. Refused rather than write over contents this tool never saw. Nothing was \
                 changed."
            ))
        };
        let Some(bytes) = on_disk else {
            return refuse();
        };
        let Baseline::Repo {
            root,
            commit: Some(commit),
        } = self.baseline_for(path)
        else {
            return refuse();
        };
        let Some(rel) = repo_relative(&root, path) else {
            return refuse();
        };
        match recorded_raw(&root, &commit, &rel) {
            Ok(Some(recorded)) if recorded == bytes => Verdict::Proceed,
            _ => refuse(),
        }
    }

    /// Lines of unsaved user work in `path` — the lines a wholesale revert of
    /// that path would destroy.
    ///
    /// The question [`Self::assess`] asks, put against replacement content
    /// that keeps nothing, which is exactly what `git checkout --`,
    /// `git restore`, `git stash` and `git clean` do to a path. It reads the
    /// same pinned baseline and subtracts the same agent-authored tally, so a
    /// line this tool wrote itself this session is not the user's unsaved work
    /// and does not appear here.
    ///
    /// Fails closed in the same direction as `assess`: a repository git will
    /// not open, or one with no commits yet, proves nothing recorded, so every
    /// line on disk counts. Bytes that are not UTF-8 text have no line model
    /// at all, and a refusal needs a line to quote, so nothing is claimed
    /// about them.
    pub fn unsaved_lines(&self, path: &Path) -> Vec<String> {
        let Ok(disk) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let authored = self.authored_lines(path, &disk);
        let on_disk = tally_excluding(&disk, &authored);
        if on_disk.is_empty() {
            return Vec::new();
        }
        let saved = match self.baseline_for(path) {
            Baseline::Repo {
                root,
                commit: Some(commit),
            } => self.recorded_blob(path, &root, &commit).unwrap_or_default(),
            // No commits yet, no repository, or a repository git would not
            // open: nothing about this file is *proven* recorded.
            _ => String::new(),
        };
        let recorded = tally(&saved);
        let mut budget: HashMap<&str, usize> = HashMap::new();
        for (text, on_disk_copies) in &on_disk {
            let short = on_disk_copies.saturating_sub(recorded.get(*text).copied().unwrap_or(0));
            if short > 0 {
                budget.insert(text, short);
            }
        }
        if budget.is_empty() {
            return Vec::new();
        }
        // Emitted in file order, once per unrecorded copy, so a refusal reads
        // like the file the user is about to lose.
        let mut unsaved = Vec::new();
        for line in disk.lines() {
            if let Some(left) = budget.get_mut(line.trim())
                && *left > 0
            {
                *left -= 1;
                unsaved.push(line.trim_end().to_owned());
            }
        }
        unsaved
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
        let keyed = key(path);
        let path = keyed.as_path();
        // Did the file move underneath the tool since it last wrote here? If
        // so nothing carried over from that write may be claimed any more:
        // only what *this* write introduces, judged against the bytes that
        // were really there.
        let stale = match self.last_written.lock() {
            Ok(mut seen) => {
                let moved = !seen.get(path).is_some_and(|s| same_lines(s, previous));
                seen.insert(path.to_path_buf(), written.to_owned());
                moved
            }
            // Without the record there is no way to know, so claim nothing.
            Err(_) => true,
        };
        let before = tally(previous);
        let after = tally(written);
        let Ok(mut map) = self.authored.lock() else {
            return;
        };
        let entry = map.entry(path.to_path_buf()).or_default();
        if stale {
            entry.clear();
        }
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

    /// What this tool may still claim to have authored at `path`, given the
    /// bytes that are actually there now.
    ///
    /// Attribution expires the moment `on_disk` stops being what this tool
    /// last wrote. Nothing on disk distinguishes a line the agent wrote from
    /// the same text typed by the user afterwards, so once the file has moved
    /// underneath the tool, every line in it is treated as the user's. That is
    /// the fail-closed direction, and it costs a refusal telling the caller to
    /// carry the lines through — measured cost: a formatter or a hook
    /// rewriting the file between two tool writes re-protects the agent's own
    /// lines until the tool writes again.
    fn authored_lines(&self, path: &Path, on_disk: &str) -> HashMap<String, usize> {
        let keyed = key(path);
        let path = keyed.as_path();
        let Ok(written) = self.last_written.lock() else {
            return HashMap::new();
        };
        if !written.get(path).is_some_and(|s| same_lines(s, on_disk)) {
            return HashMap::new();
        }
        drop(written);
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
                    // A bare repository, or a path inside `.git`. Round 4 read
                    // this as "no repository", and then told the user "this
                    // file is in no repository, so nothing about it is
                    // recorded anywhere" about a repository whose HEAD
                    // provably records the file — measured with a single
                    // `git config core.bare true` on an ordinary work tree.
                    // Fail-closed either way, so no data was at risk, but a
                    // false statement and a wrong remedy. It is only "no
                    // repository" when the filesystem says there is no marker.
                    return unopenable(
                        "git reports no work tree at this path, so what it records here cannot \
                         be established"
                            .to_owned(),
                    );
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
        let text = match recorded_raw(root, commit, &rel)? {
            None => String::new(),
            Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        };
        if let Ok(mut cache) = self.blobs.lock() {
            cache.insert(key, text.clone());
        }
        Ok(text)
    }

    /// Which object store, if any, may hold this file's prior bytes.
    ///
    /// The rule, adopted in round 5 after placement had been broken three
    /// different ways in three rounds: **copy only where the copy can be
    /// proven no more exposed than the file itself; where it cannot be
    /// proven, refuse and copy nothing.** A recovery copy is verbatim by
    /// construction, so scrubbing it is not available and placement is the
    /// only lever there is — and an always-safe location turned out not to
    /// exist. Refusing is always safe: nothing is lost and no secret spreads.
    /// So exposure is a precondition that gets checked here rather than a
    /// property that was hoped for, and the linked-worktree, submodule,
    /// gitignored and Windows-ACL cases are one rule instead of four
    /// carve-outs.
    ///
    /// What has to hold:
    ///
    /// * the repository does not say to ignore the file — its own committed
    ///   configuration saying this file does not belong in it;
    /// * the pinned commit records something under the file's own directory,
    ///   or the file sits at the repository root. Measured armD: `$HOME` is a
    ///   dotfiles repository and `~/work/env.local` is not its business;
    /// * the store git actually uses is **inside the work tree the file is
    ///   in**. In a linked worktree the bytes land in the main repository, and
    ///   in a submodule in `<super>/.git/modules/<name>/objects` — both
    ///   outside the tree the user is working in;
    /// * the copy is no wider-permissioned than the file. Measured: a `0600`
    ///   file's bytes become a `0444` object under a `0755` directory.
    fn object_store<'a>(&self, baseline: &'a Baseline, path: &Path) -> Store<'a> {
        let Baseline::Repo { root, commit } = baseline else {
            return Store::Absent;
        };
        let root = root.as_path();
        let Some(rel) = repo_relative(root, path) else {
            return Store::Unproven {
                why: format!(
                    "this file could not be placed inside the repository at {}",
                    root.display()
                ),
            };
        };
        // The user has already said, in this repository's own configuration,
        // that this file does not belong in it.
        if self.repository_ignores(root, &rel) {
            return Store::Unproven {
                why: format!(
                    "the repository at {} is configured to ignore this file, so a copy is not \
                     that repository's to hold",
                    root.display()
                ),
            };
        }
        // armD: an enclosing repository is not necessarily this file's archive.
        if let Some((dir, _)) = rel.rsplit_once('/') {
            // Non-empty output is the only proof the directory is recorded.
            // git failing to answer is not proof, and lands in the safe
            // direction: no copy goes anywhere.
            let recorded = match commit {
                None => false,
                Some(commit) => matches!(
                    git_run(
                        root,
                        &[
                            "ls-tree",
                            "--full-tree",
                            "-z",
                            commit,
                            "--",
                            &format!("{dir}/"),
                        ],
                        None,
                    ),
                    Some(run) if run.ok() && !run.stdout.is_empty()
                ),
            };
            if !recorded {
                return Store::Unproven {
                    why: format!(
                        "the repository at {} records nothing under {dir}, so it encloses this \
                         file without being its archive",
                        root.display()
                    ),
                };
            }
        }
        // Where git would actually put the object, asked of git.
        let Some(objects) = objects_dir(root) else {
            return Store::Unproven {
                why: format!(
                    "git would not name the object store of the repository at {}",
                    root.display()
                ),
            };
        };
        // Topology. `--git-path objects` resolves to the *main* repository in
        // a linked worktree and to `<super>/.git/modules/<name>` in a
        // submodule, so a copy would leave the tree the user is working in.
        match (std::fs::canonicalize(&objects), std::fs::canonicalize(root)) {
            (Ok(obj), Ok(top)) if obj.starts_with(&top) => {}
            (Ok(obj), Ok(_)) => {
                return Store::Unproven {
                    why: format!(
                        "this work tree keeps its objects at {}, outside the tree this file is \
                         in — a linked worktree and a submodule both file them into a different \
                         repository",
                        obj.display()
                    ),
                };
            }
            _ => {
                return Store::Unproven {
                    why: format!(
                        "the object store at {} could not be resolved on disk",
                        objects.display()
                    ),
                };
            }
        }
        if let Err(why) = copy_no_wider_than_file(path, &objects) {
            return Store::Unproven { why };
        }
        Store::Owned { root, objects }
    }

    /// Does this repository's configuration say to ignore `rel`?
    ///
    /// `check-ignore` exits 0 for ignored, 1 for not ignored, and something
    /// else when it could not decide. Only a definite 1 is read as "not
    /// ignored": anything else means the question is open, and an open
    /// question must not end with the user's bytes inside the repository.
    ///
    /// `--no-index` is load-bearing. Without it `check-ignore` consults the
    /// **index**, which is mutable during the session and which the agent can
    /// write: measured on git 2.43.0, one `git add -f .env` flips a
    /// gitignored secret from "ignored" to "not ignored" and files it
    /// straight into `.git/objects` — the exact harm the ignore rule exists
    /// to refuse, reopened by an ordinary command. An inherited
    /// `GIT_INDEX_FILE` flips the same decision from the other side. The
    /// question asked here is therefore only ever about the *ignore rules*,
    /// which are committed configuration, and a tracked file matched by a
    /// rule now reads as ignored: that is the refusing direction, so it costs
    /// a refusal rather than a copy.
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
        !matches!(
            git_invoke(
                root,
                Pathspecs::AsGitTakesThem,
                &["check-ignore", "-q", "--no-index", "--stdin", "-z"],
                Some(&payload),
            ),
            Some(run) if run.code == Some(1)
        )
    }

    /// Put `bytes` in the repository's own object database, prove they can be
    /// read back, and anchor them under a ref so they stay there.
    ///
    /// Round 5 stopped at the object and called the user's own `git gc` a
    /// sufficient retention policy. It is not one. The object it left was
    /// reachable from nothing, so it appeared in no `git log`, no
    /// `git stash list` and no `git for-each-ref`, and `git gc --prune=now`
    /// removed it outright — measured, and reproduced as the red arm of
    /// `unsaved_work_durable_test`. Nothing staged and nothing committed is
    /// still right; nothing *referenced* was the defect.
    ///
    /// The blob is copied and verified first and anchored second, so a
    /// failure to anchor is reported as exactly that — a real copy that will
    /// not last — and never as a failure to copy.
    fn recoverable_copy(
        &self,
        root: &Path,
        bytes: &str,
        display_path: &str,
        dropped_total: usize,
    ) -> Result<Preserved, String> {
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
        if !read_back_matches(&back.stdout, bytes.as_bytes()) {
            return Err(read_back_mismatch(&back.stdout, bytes.as_bytes()));
        }
        Ok(Preserved {
            anchor: anchor_copy(root, &oid, display_path, dropped_total),
            oid,
        })
    }
}

/// Make the verified copy at `oid` reachable, and return the ref that does it.
///
/// **An annotated tag, not a commit and not a bare blob ref.** All three are
/// legal and all three survive `git gc --aggressive --prune=now`; the tag is
/// the one that behaves. Measured on git 2.43.0:
///
/// * a ref pointing straight at the blob carries no date and no message, so
///   `git for-each-ref --sort=-creatordate` renders an empty date and the user
///   cannot tell which file a given copy came from without reading it;
/// * a commit is listed by `git log --all` and `git log --graph`, so every
///   backup this guard ever makes turns up in the user's own history views;
///   and `git commit-tree` exits non-zero with "Author identity unknown" in a
///   repository with no `user.email`, which is a plain `git init` tree;
/// * the tag carries a tagger date and a subject naming the file, is peeled by
///   `<ref>^{}`, does **not** appear in `git log --all` and does **not**
///   appear in `git tag -l` (it is not under `refs/tags`), and `git mktag`
///   takes its identity from the object body so no configuration is consulted.
///
/// `git fsck` is clean in all three shapes.
///
/// **Retention is: nothing here ever deletes one of these refs.** No age
/// threshold, no cap, no eviction — an automatic policy that discarded the
/// wrong one would be this module's own failure mode wearing a schedule. The
/// cost is disk, and it is bounded by the distinct pre-images actually
/// preserved: git stores an object once, so re-preserving identical bytes adds
/// one ref (about sixty bytes packed) and no new object. Every ref is listed
/// by `git for-each-ref refs/wayland-core/unsaved/`, the note names that
/// command, and the note names the two-command deletion.
fn anchor_copy(
    root: &Path,
    oid: &str,
    display_path: &str,
    dropped_total: usize,
) -> Result<String, String> {
    let now = chrono::Utc::now();
    // Timestamp first so the names sort chronologically, object id second so
    // two copies in the same second do not collide. Identical bytes preserved
    // twice in one second land on the same name, and `update-ref` is happy to
    // rewrite a ref to the value it already holds.
    let leaf = format!(
        "{}-{}",
        now.format("%Y%m%dT%H%M%SZ"),
        oid.get(..12).unwrap_or(oid)
    );
    let anchor = format!("{UNSAVED_REF_PREFIX}/{leaf}");
    let tag = format!(
        "object {oid}\ntype blob\ntag {leaf}\ntagger {ANCHOR_TAGGER} {stamp} +0000\n\n\
         wayland-core: {dropped_total} unsaved line(s) from {path}\n",
        stamp = now.timestamp(),
        path = one_line(display_path),
    );

    let made = git_run(root, &["mktag"], Some(tag.as_bytes()))
        .ok_or_else(|| "git could not be run to anchor the copy".to_owned())?;
    if !made.ok() {
        return Err(made.why("git would not write the anchor object"));
    }
    let tag_oid = made.stdout_text().trim().to_owned();
    if !is_hex_oid(&tag_oid) {
        return Err("git returned no usable anchor object id".to_owned());
    }

    let set = git_run(root, &["update-ref", &anchor, &tag_oid], None)
        .ok_or_else(|| "git could not be run to write the anchor ref".to_owned())?;
    if !set.ok() {
        return Err(set.why("git would not write the anchor ref"));
    }

    // Graded from the repository rather than from the exit codes above, for
    // the same reason the blob is read back rather than assumed: an anchor
    // that does not peel to this exact object anchors nothing. `rev-parse`
    // rather than a second `cat-file blob`, because the read-back arm asserts
    // that the only `cat-file blob` in an assessment is its own.
    let peeled = git_run(root, &["rev-parse", &format!("{anchor}^{{}}")], None)
        .ok_or_else(|| "git could not be run to verify the anchor".to_owned())?;
    if !peeled.ok() {
        return Err(peeled.why("the anchor ref could not be read back"));
    }
    if peeled.stdout_text().trim() != oid {
        return Err(format!(
            "the anchor ref {anchor} does not resolve to the copy that was made"
        ));
    }
    Ok(anchor)
}

/// `text` with its control characters flattened to spaces.
///
/// The path goes into a tag message whose first line is what
/// `%(contents:subject)` shows. A newline in it would put the rest somewhere
/// the listing does not display, and the listing is the whole point.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// The file's exact recorded bytes in `commit`, or `Ok(None)` when that commit
/// does not record it. `Err` only when git could not answer.
///
/// `ls-tree` reports "not in that commit" as exit 0 with no output, so an
/// absent path never has to be told apart from a broken repository by its exit
/// code. `git show <commit>:<path>` cannot do that: it exits 128 for both,
/// which is the same conflation as B1.
///
/// Bytes rather than text, because [`UnsavedWorkGuard::assess_opaque`] has to
/// compare a file that is *not* valid UTF-8 against what the commit holds, and
/// a lossy conversion would make two different files compare equal.
fn recorded_raw(root: &Path, commit: &str, rel: &str) -> Result<Option<Vec<u8>>, String> {
    let listing = git_run(
        root,
        &["ls-tree", "--full-tree", "-z", commit, "--", rel],
        None,
    )
    .ok_or_else(|| "git could not be run".to_owned())?;
    if !listing.ok() {
        return Err(listing.why("git would not read the pinned commit"));
    }
    let Some(oid) = blob_oid(&listing.stdout_text()) else {
        return Ok(None);
    };
    let blob = git_run(root, &["cat-file", "blob", &oid], None)
        .ok_or_else(|| "git could not be run".to_owned())?;
    if !blob.ok() {
        return Err(blob.why("git would not read the recorded contents"));
    }
    Ok(Some(blob.stdout))
}

/// Is the pre-image that was judged still the one being replaced? `observed`
/// is what the destination held at the instant the new bytes were published,
/// `judged` is what the assessment saw; `None` on either side means nothing
/// was there.
///
/// ADV-7: the assessment between the pre-image read and the write runs about
/// five `git` processes, a window measured at **13.5 ms**. With the user's
/// editor saving inside it, 12 of 12 interleavings destroyed the save while
/// the tool result claimed the previous contents had been preserved — the
/// same save made *before* the call was protected 12 times out of 12, so that
/// measured the product and not the harness.
///
/// This used to be spelled `pre_image_unchanged(path, judged)`, which READ the
/// path back and compared. That can only narrow the window, never close it: a
/// read and a write are two operations, and #1155 measured what was left at
/// ~6.5% on the filesystem path and 70% on the vfs path the dispatcher
/// actually takes. The bytes are now handed in by an atomic exchange that
/// displaced them, so there is no second observation to be stale.
pub fn pre_image_matches(observed: Option<&[u8]>, judged: Option<&[u8]>) -> Result<(), String> {
    match (observed, judged) {
        (Some(now), Some(before)) if now == before => Ok(()),
        (Some(_), Some(_)) => Err("its contents changed on disk".to_owned()),
        (Some(_), None) => Err("something else created it".to_owned()),
        (None, None) => Ok(()),
        (None, Some(_)) => Err("it was deleted".to_owned()),
    }
}

/// The refusal for a pre-image that moved between the assessment and the
/// write. Not a guard verdict: the content about to be written was chosen
/// against bytes that are gone, so writing it would destroy whatever replaced
/// them, whether or not any of it was recorded.
pub fn changed_under_write(display_path: &str, why: &str) -> String {
    format!(
        "Refused to overwrite {display_path}: {why} while this write was being checked. The \
         content about to be written was composed against contents that no longer exist, so \
         writing it now would destroy whatever just arrived — most often the user saving in \
         their editor. Nothing was changed. Read the file as it stands now and redo the change \
         against that."
    )
}

/// What to tell the user about a refused checked publish.
///
/// The single place the choice is made, called from both tool call sites
/// (`edit.rs` and `write.rs`), because the choice is the whole of #1239 c2: a
/// refusal that destroyed nothing and a refusal that displaced somebody's save
/// were the same sentence, so the user could not tell them apart.
pub fn refusal_message(display_path: &str, refusal: &wcore_config::Refusal) -> String {
    match refusal.intercepted_save() {
        Some(preserved) => {
            changed_under_write_displacing_a_save(display_path, refusal.why(), preserved)
        }
        None => changed_under_write(display_path, refusal.why()),
    }
}

/// #1239 — the same refusal, rendered against what the retraction actually
/// cost.
///
/// [`changed_under_write`]'s "Nothing was changed." is a statement about the
/// DESTINATION, and it stays true here. It is not a statement about the save
/// that arrived inside the guard's own exchange→verdict window: putting the
/// original back displaces that save, and until #1239 it was then deleted and
/// the user handed a refusal byte-identical to one that had cost nobody
/// anything. This is the wording for the case where it cost somebody
/// something, so the two are no longer the same sentence.
pub fn changed_under_write_displacing_a_save(
    display_path: &str,
    why: &str,
    preserved_at: &std::path::Path,
) -> String {
    let preserved = preserved_at.display();
    format!(
        "Refused to overwrite {display_path}: {why} while this write was being checked. The \
         content about to be written was composed against contents that no longer exist, so \
         writing it now would destroy whatever just arrived — most often the user saving in \
         their editor. {display_path} itself is back to exactly what it held. A save that \
         landed WHILE the check was running was displaced by putting it back; those bytes were \
         NOT deleted — they are preserved at {preserved}. Read {display_path} as it stands now, \
         reconcile it with {preserved}, and redo the change against that."
    )
}

/// #1241 — the guard refused and the refusal could not be undone.
///
/// Distinct from every other message in this module because the destination is
/// NOT as it was: the new bytes are published and the pre-image survives only
/// under `preserved_at`. Reporting this as a plain success — which the direct
/// Write path did, by rewriting the same bytes through an unchecked
/// `fs::write` — tells the user their write landed cleanly at the one moment
/// their file is in the state they most need to know about.
pub fn refused_but_not_rolled_back(
    display_path: &str,
    why: &str,
    preserved_at: &std::path::Path,
) -> String {
    let preserved = preserved_at.display();
    format!(
        "Refused to overwrite {display_path}: {why} while this write was being checked — and \
         the refusal could NOT be undone. The new content is published at {display_path} and \
         the contents it replaced are preserved at {preserved}. Nothing was deleted, but \
         {display_path} is not the file the user last saved. Reconcile {display_path} with \
         {preserved} before making any further change to it."
    )
}

/// Did the copy come back as the same bytes — every byte of them, not the
/// same number of them?
///
/// "read back byte-for-byte" is the flagship phrase of this module's whole
/// guarantee, and a length-only comparison satisfies it against every
/// well-behaved `git` there is: swapping this for `back.len() == bytes.len()`
/// survived a 27-test suite untouched. It is a named predicate so it can be
/// exercised directly on a same-length, different-bytes read-back, which no
/// live git will ever produce.
fn read_back_matches(back: &[u8], original: &[u8]) -> bool {
    back == original
}

/// Why a read-back was rejected, in terms of the first byte that differs.
fn read_back_mismatch(back: &[u8], original: &[u8]) -> String {
    if back.len() != original.len() {
        return format!(
            "the copy read back as {} bytes rather than {}",
            back.len(),
            original.len()
        );
    }
    let at = back
        .iter()
        .zip(original)
        .position(|(a, b)| a != b)
        .unwrap_or(0);
    format!(
        "the copy read back as {} bytes as expected but differs at byte {at}",
        original.len()
    )
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
/// The absolute root of the work tree holding `dir`, straight from git.
///
/// **Every path `git status --porcelain` and `git diff --name-only` print is
/// relative to this, never to the directory git ran in.** Resolving one of
/// them against `cwd` instead is not a cosmetic slip in a message: from
/// `<root>/pkg`, a reported `pkg/a.txt` becomes `<root>/pkg/pkg/a.txt` and a
/// reported `notes.txt` becomes `<root>/pkg/notes.txt` — both nonexistent, so
/// every candidate reads as holding no unsaved work and the whole guard
/// passes silently. `git add -A`, `git commit` and `git reset --hard` are all
/// repository-wide from a subdirectory, so what that fail-open covers is the
/// entire tree, not the subdirectory the session happens to be in.
///
/// Asked of git rather than inferred by walking up to a `.git` marker: a
/// linked worktree, a submodule and a `.git` file all put the root somewhere a
/// walk would guess wrong, and `rev-parse --show-toplevel` is the same source
/// [`UnsavedWorkGuard::resolve_baseline`] already pins the baseline from.
///
/// `None` when git will not name a root. Every caller reads that as "nothing
/// can be established here", which is the direction the rest of this module
/// takes whenever git declines to answer.
fn work_tree_root(dir: &Path) -> Option<PathBuf> {
    let run = git_run(dir, &["rev-parse", "--show-toplevel"], None)?;
    if !run.ok() {
        return None;
    }
    let text = run.stdout_text();
    let named = text.lines().next()?.trim();
    if named.is_empty() {
        return None;
    }
    let root = PathBuf::from(named);
    Some(std::fs::canonicalize(&root).unwrap_or(root))
}

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

/// Do two states of a file hold exactly the same trimmed, non-blank lines,
/// the same number of times each?
///
/// This is what "the file is still what the tool last wrote" means, and it is
/// deliberately not byte equality. The measured cost of byte equality: the
/// agent writes a file, `cargo fmt` — which `just push` runs — reflows the
/// blank lines, and the agent's own next rewrite of its own file is hard
/// refused with a message calling those lines the user's. A reformat that
/// only moves whitespace around leaves this comparison equal, so attribution
/// survives it; anything that adds, removes or alters a line of content does
/// not, which is the case the expiry exists for.
fn same_lines(a: &str, b: &str) -> bool {
    tally(a) == tally(b)
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
/// It states only what has been established. Round 4 asserted "The rest of
/// this file IS committed", which is false whenever a *kept* line is also
/// unrecorded — measured with two unsaved lines, one kept and one dropped —
/// and called the dropped lines "their in-progress work", which is false
/// whenever this tool wrote them a turn earlier and attribution has since
/// expired.
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
         Part of this file IS in the commit, so this is not a whole-file replacement — it is a \
         partial rewrite that would silently drop work recorded nowhere, whoever wrote it. \
         Read the file as it stands on disk now and carry those lines into the content you write \
         — in their changed form if what you are doing changes them. If the user genuinely asked \
         for those specific lines to go, they must be recorded somewhere first.",
        quoted = quote_dropped(previous, dropped, dropped_total),
    )
}

/// Whether `new_content` introduces no line that `previous` did not already
/// hold at least as many copies of.
///
/// The discriminator between a deletion and a modification, for the one case
/// where the difference decides a refusal. Counted rather than set-compared:
/// turning two copies of a line into one introduces nothing. Blank lines and
/// surrounding whitespace are outside the model here exactly as they are for
/// every other tally in this file, so a pure re-indent also reads as
/// introducing nothing - and a re-indent that additionally removes the user's
/// unsaved line is a removal of it.
fn adds_nothing(previous: &str, new_content: &str) -> bool {
    let before = tally(previous);
    tally(new_content)
        .iter()
        .all(|(line, now)| before.get(line).copied().unwrap_or(0) >= *now)
}

/// The delete-only refusal for Edit.
///
/// Names no other tool and offers no flag: round 1 shipped a refusal that
/// recommended a different write surface, and the model took that route on
/// the first refusal in both live adversarial arms. It says what an editor
/// would do instead - leave the lines alone - and where the decision belongs
/// when the lines really are meant to go.
fn deletion_refusal(
    display_path: &str,
    previous: &str,
    dropped: &HashMap<&str, usize>,
    dropped_total: usize,
) -> String {
    format!(
        "Refused to edit {display_path}: this edit only removes lines, and {dropped_total} of \
         them are on disk but in no commit. That is unsaved work which exists nowhere else, so \
         removing it is irreversible - and filing a recovery copy is not the same thing as \
         leaving the work where the user put it.\n\
         Lines that would be lost:\n{quoted}\n\
         Nothing was changed. Edit around those lines and leave them where they are. If the \
         user genuinely asked for exactly those lines to go, say what will be lost and let them \
         confirm first. An edit that changes these lines rather than removing them is not \
         affected by this.",
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
         there is no way to tell whether any of them exist anywhere else. Reason: {why}.\n\
         Lines that would be lost:\n{quoted}\n\
         Nothing was changed. Fix the reason git could not answer, or carry those lines into the \
         content you write. Writes that do not remove existing lines are unaffected.",
        quoted = quote_dropped(previous, dropped, dropped_total),
    )
}

fn unresolved_note(dropped_total: usize, why: &str) -> String {
    format!(
        "\nNote: {dropped_total} line(s) that were on disk are not in the new content, and the \
         last saved version of this file could not be established ({why}), so it is not known \
         whether they exist anywhere else. No recovery copy was made."
    )
}

/// The refusal for a copy that cannot be proven no more exposed than the file
/// itself: an ignored file, a repository that is not this file's archive, a
/// store outside this work tree, a copy wider-permissioned than the original,
/// or a platform where none of that can be measured. One message, because it
/// is one rule — see [`UnsavedWorkGuard::object_store`].
fn unproven_store_refusal(display_path: &str, dropped_total: usize, why: &str) -> String {
    format!(
        "Refused to overwrite {display_path}: this content would delete {dropped_total} line(s) \
         that are on disk and in no commit. A recovery copy is only made where it can be proven \
         to be no more exposed than the file itself, and here it cannot: {why}. Nothing was \
         changed and nothing was copied. Carry those lines into the content you write, or have \
         this file recorded somewhere that tracks it."
    )
}

fn unproven_store_note(dropped_total: usize, why: &str) -> String {
    format!(
        "\nNote: {dropped_total} line(s) that were on disk are not in the new content, and no \
         recovery copy was made: one could not be proven to be no more exposed than the file \
         itself ({why}). Those lines are not recoverable."
    )
}

/// One class of principal, for comparing how far a file's bytes reach.
/// Only the unix comparison reads these; the Windows arm has no comparison to
/// make, so it refuses instead.
#[cfg(unix)]
const OWNER: u32 = 0b100;
#[cfg(unix)]
const GROUP: u32 = 0b010;
#[cfg(unix)]
const OTHER: u32 = 0b001;

/// Prove that a copy of `source` placed under `objects` is readable by no one
/// who cannot already read `source`.
///
/// Two comparisons, both of them measured rather than assumed:
///
/// 1. **Mode.** git writes a loose object `0444` before the umask, so every
///    class that can reach the store can read the copy. A `0600` file
///    therefore fails: measured, its bytes become a `0444` object under a
///    `0755` directory. Assuming a narrower object mode would mean assuming a
///    umask this process cannot read without racing every other thread in it.
/// 2. **Reach.** The search bits of every directory above the file, against
///    the search bits of every directory above the store. A `0644` file in a
///    `0700` directory is reachable only by its owner; `.git/objects` at
///    `0755` is reachable by everyone, so that copy is wider.
///
/// Stated limits: this compares *classes*, not principals — the object is
/// owned by this process and the file may be owned by someone else — and it
/// does not model ACLs, MAC labels or capabilities. Both are in the refusing
/// direction here, because the object's mode is taken at its widest.
#[cfg(unix)]
fn copy_no_wider_than_file(source: &Path, objects: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::symlink_metadata(source)
        .map_err(|e| format!("this file's own permissions could not be read ({e})"))?
        .permissions()
        .mode();
    let file_read = read_classes(mode);
    let wider = (OWNER | GROUP | OTHER) & !file_read;
    if wider != 0 {
        return Err(format!(
            "this file is readable by {} (mode 0{:03o}), but a recovery copy is a 0444 object \
             that {} could read as well",
            describe_classes(file_read),
            mode & 0o777,
            describe_classes(wider),
        ));
    }
    let file_reach = reachable_classes(source.parent().unwrap_or(source))?;
    let store_reach = reachable_classes(objects)?;
    let wider = store_reach & !file_reach;
    if wider != 0 {
        return Err(format!(
            "this file sits where only {} can reach it, while the object store at {} is \
             reachable by {} as well",
            describe_classes(file_reach),
            objects.display(),
            describe_classes(wider),
        ));
    }
    Ok(())
}

/// Windows has no comparison here that this module can make.
///
/// Measured on this project's own machine (git 2.54.0.windows.1): under
/// `%USERPROFILE%`, where most Windows repositories live, `.git\objects`
/// inherits `S-1-15-2-…:(I)(OI)(CI)(RX)` for both AppContainer package SIDs —
/// the principals that confine agent subprocesses. The file may well inherit
/// exactly the same ACEs, which would make the copy no more exposed; the point
/// is that this code cannot demonstrate it, and a copy that cannot be bounded
/// is not made. A real ACL comparison would lift this, and until one exists
/// the platform costs refusals rather than copies.
#[cfg(not(unix))]
fn copy_no_wider_than_file(_source: &Path, objects: &Path) -> Result<(), String> {
    Err(format!(
        "on this platform the permissions of a copy in {} cannot be compared with this file's \
         own, so the copy cannot be bounded",
        objects.display()
    ))
}

/// Which classes a mode lets read.
#[cfg(unix)]
fn read_classes(mode: u32) -> u32 {
    let mut mask = 0;
    if mode & 0o400 != 0 {
        mask |= OWNER;
    }
    if mode & 0o040 != 0 {
        mask |= GROUP;
    }
    if mode & 0o004 != 0 {
        mask |= OTHER;
    }
    mask
}

/// Which classes a mode lets search a directory.
#[cfg(unix)]
fn search_classes(mode: u32) -> u32 {
    let mut mask = 0;
    if mode & 0o100 != 0 {
        mask |= OWNER;
    }
    if mode & 0o010 != 0 {
        mask |= GROUP;
    }
    if mode & 0o001 != 0 {
        mask |= OTHER;
    }
    mask
}

/// Which classes can reach `dir` at all: the search bits of every directory
/// from the filesystem root down, intersected.
#[cfg(unix)]
fn reachable_classes(dir: &Path) -> Result<u32, String> {
    use std::os::unix::fs::PermissionsExt as _;
    let start = std::fs::canonicalize(dir)
        .map_err(|e| format!("{} could not be resolved ({e})", dir.display()))?;
    let mut mask = OWNER | GROUP | OTHER;
    for ancestor in start.ancestors() {
        let mode = std::fs::metadata(ancestor)
            .map_err(|e| format!("{} could not be inspected ({e})", ancestor.display()))?
            .permissions()
            .mode();
        mask &= search_classes(mode);
        if mask == 0 {
            break;
        }
    }
    Ok(mask)
}

#[cfg(unix)]
fn describe_classes(mask: u32) -> String {
    let mut parts = Vec::new();
    if mask & OWNER != 0 {
        parts.push("its owner");
    }
    if mask & GROUP != 0 {
        parts.push("its group");
    }
    if mask & OTHER != 0 {
        parts.push("everyone else");
    }
    if parts.is_empty() {
        return "nobody".to_owned();
    }
    parts.join(", ")
}

/// The note for a copy that was made, verified and anchored.
///
/// The first `cat-file blob` in it is the recovery command and the object id
/// follows it: several suites run this note's own command rather than matching
/// its wording, which is what caught round 2's false snapshot claim.
fn copy_note(
    dropped_total: usize,
    mode: Mode,
    root: &Path,
    oid: &str,
    anchor: &str,
    wholesale: bool,
    objects: &Path,
) -> String {
    let why = if wholesale && mode == Mode::Rewrite {
        " None of this file was in any commit, so the whole of it counted as unsaved work."
    } else {
        ""
    };
    // Never `<root>/.git/objects`: in a linked worktree or a submodule `.git`
    // is a *file* and the store belongs to the main repository, so that path
    // names a directory that does not exist. Measured on git 2.43.0. Those
    // two topologies no longer reach here at all — a store outside this work
    // tree is refused rather than used — but the path still comes from git.
    let store = objects.display();
    format!(
        "\nNote: {dropped_total} line(s) that were on disk and in no commit are not in the new \
         content.{why} The previous contents were written to this repository's own object store, \
         read back to confirm they match, and anchored at the ref {anchor} — so they survive \
         `git gc`, including `git gc --aggressive --prune=now`. Recover them with:\n    \
         git -C {root} cat-file blob {oid}\n\
         or, without needing that object id at all:\n    \
         git -C {root} show {anchor}\n\
         Every copy this guard has made is listed, newest first, by:\n    \
         git -C {root} for-each-ref --sort=-creatordate \
         --format='%(refname) %(creatordate:iso) %(contents:subject)' {prefix}/\n\
         Nothing deletes those refs automatically. To drop this one: \
         `git -C {root} update-ref -d {anchor}`, then `git -C {root} gc --prune=now`.\n\
         Until then these bytes live in {store}, and they are in no commit, so that is the only \
         place they exist — whether this file is gitignored, merely untracked, or tracked and \
         wholly rewritten. Anchoring widens where they travel, so: they go with a filesystem \
         copy of the repository (cp -a, tar, rsync), with `git clone --mirror` and with \
         `git bundle --all`. A plain `git clone` of this path leaves them unreferenced and the \
         clone's first `git gc --prune=now` drops them. `git push`, `git push --all`, \
         `git push --tags` and `git push --follow-tags` do not carry them; `git push --mirror` \
         does.",
        root = root.display(),
        prefix = UNSAVED_REF_PREFIX,
    )
}

/// The refusal for a copy that was made but could not be anchored.
///
/// The bytes are in the object store and were read back, so nothing has been
/// lost by refusing; what could not be established is that they will still be
/// there later. Round 5 shipped exactly this state as the guarantee.
fn unanchored_refusal(display_path: &str, dropped_total: usize, why: &str) -> String {
    format!(
        "Refused to overwrite {display_path}: it holds {dropped_total} line(s) that are on disk \
         and in no commit. A copy of the previous contents was made and read back, but it could \
         not be anchored under a ref ({why}), so nothing in the repository would reference it: \
         no `git for-each-ref` would list it and `git gc --prune=now` would remove it. A copy \
         that expires is not a recovery, so nothing was changed. Carry those lines into the \
         content you write."
    )
}

/// The same state on the Edit surface, which does not refuse for it.
///
/// Edit is never refused for a *drop* (see the module doc), so the honest
/// thing is to proceed and say precisely what the copy is and is not — and to
/// name the one command that turns it into a durable one.
fn unanchored_note(dropped_total: usize, root: &Path, oid: &str, why: &str) -> String {
    format!(
        "\nNote: {dropped_total} line(s) that were on disk and in no commit are not in the new \
         content. The previous contents were copied into this repository's object store and read \
         back, so they are there now:\n    \
         git -C {root} cat-file blob {oid}\n\
         but the copy could not be anchored under a ref ({why}). Nothing references it, so no \
         `git for-each-ref` lists it and `git gc --prune=now` removes it. To make it durable:\n    \
         git -C {root} update-ref {prefix}/manual-{short} {oid}",
        root = root.display(),
        prefix = UNSAVED_REF_PREFIX,
        short = oid.get(..12).unwrap_or(oid),
    )
}

/// Where this repository actually keeps its objects, asked of git rather than
/// assumed from `root`.
///
/// `<root>/.git/objects` is wrong whenever `.git` is a file: a linked worktree
/// and a submodule both keep their objects in the main repository's store, and
/// naming the worktree's own `.git` sends the user to a path that is not a
/// directory. `None` when git will not answer, which is a refusal: a store that
/// cannot be named cannot be bounded either.
fn objects_dir(root: &Path) -> Option<PathBuf> {
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
    (!text.is_empty()).then(|| PathBuf::from(text))
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
/// variables usually arrive. `GIT_INDEX_FILE` — which git exports to every
/// hook — decides `check-ignore`'s answer, and so decides whether a
/// gitignored secret is filed into the object store; it was the one variable
/// round 4 did not clear.
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
        .env_remove("GIT_INDEX_FILE")
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
