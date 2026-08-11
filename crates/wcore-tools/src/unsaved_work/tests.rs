//! Unit tests for the INV-2 unsaved-work guarantee.
//!
//! Split out of `unsaved_work.rs` to keep that file under the 1000-line
//! limit, matching the existing `bash.rs` / `bash/tests.rs` pattern.

use super::*;
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
}

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

fn repo() -> Fixture {
    let dir = TempDir::new().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    Fixture { _dir: dir, root }
}

impl Fixture {
    fn guard(&self) -> UnsavedWorkGuard {
        UnsavedWorkGuard::new_isolated()
    }
    fn write(&self, name: &str, body: &str) -> PathBuf {
        let p = self.root.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, body).unwrap();
        p
    }
    /// Can this object still be read out of the repository?
    fn blob_readable(&self, oid: &str) -> bool {
        Command::new("git")
            .args(["cat-file", "blob", oid])
            .current_dir(&self.root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap()
    }
    /// Make git refuse to open this repository, the way dubious ownership and
    /// an unreadable config do in the field: exit 128 while `.git` is plainly
    /// still there.
    fn break_git(&self) {
        std::fs::write(
            self.root.join(".git/config.wcore-bak"),
            self.read(".git/config"),
        )
        .unwrap();
        std::fs::write(self.root.join(".git/config"), "[[[not a config\n").unwrap();
    }
    fn repair_git(&self) {
        let saved = self.read(".git/config.wcore-bak");
        std::fs::write(self.root.join(".git/config"), saved).unwrap();
    }
    /// Recover the bytes a note says are recoverable, exactly as the note's
    /// own instructions say to.
    fn recover(&self, note: &str) -> String {
        let oid = oid_in(note);
        let out = Command::new("git")
            .args(["cat-file", "blob", &oid])
            .current_dir(&self.root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "the note's own recovery command failed"
        );
        String::from_utf8(out.stdout).unwrap()
    }
}

/// The measured defect: a tracked file with one uncommitted line.
fn parser_fixture() -> (Fixture, PathBuf) {
    let f = repo();
    f.write("parser.py", "def a():\n    return 1\n");
    git(&f.root, &["add", "parser.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.write("parser.py", "def a():\n    return 1\n# WIP do not touch\n");
    (f, p)
}

fn assert_refused(v: Verdict) -> String {
    match v {
        Verdict::Refuse(m) => m,
        other => panic!("expected Refuse, got {other:?}"),
    }
}

fn assert_noted(v: Verdict) -> String {
    match v {
        Verdict::ProceedWithNote(m) => m,
        other => panic!("expected ProceedWithNote, got {other:?}"),
    }
}

fn oid_in(note: &str) -> String {
    let marker = "cat-file blob ";
    let start = note.find(marker).expect("note names a recovery object") + marker.len();
    note[start..]
        .split_whitespace()
        .next()
        .expect("note terminates the object id")
        .to_owned()
}

// ---- the original round-1 behaviours that must not regress -------------

#[test]
fn partial_rewrite_dropping_the_unsaved_line_is_refused() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    let msg = assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("# WIP do not touch"), "{msg}");
}

#[test]
fn rewrite_that_carries_the_unsaved_line_through_is_allowed() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    assert!(matches!(
        g.assess(
            &p,
            "parser.py",
            &f.read("parser.py"),
            "def a():\n    return 2\n# WIP do not touch\n",
            Mode::Rewrite,
        ),
        Verdict::Proceed
    ));
}

#[test]
fn committed_lines_may_be_deleted_freely() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    assert!(matches!(
        g.assess(
            &p,
            "parser.py",
            &f.read("parser.py"),
            "# WIP do not touch\n",
            Mode::Rewrite
        ),
        Verdict::Proceed
    ));
}

#[test]
fn blank_lines_are_not_treated_as_unsaved_work() {
    let (f, p) = parser_fixture();
    f.write("parser.py", "def a():\n    return 1\n\n\n");
    let g = f.guard();
    assert!(matches!(
        g.assess(
            &p,
            "parser.py",
            &f.read("parser.py"),
            "def a():\n    return 2\n",
            Mode::Rewrite
        ),
        Verdict::Proceed
    ));
}

#[test]
fn a_moved_unsaved_line_still_counts_as_surviving() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    assert!(matches!(
        g.assess(
            &p,
            "parser.py",
            &f.read("parser.py"),
            "# WIP do not touch\ndef a():\n    return 2\n",
            Mode::Rewrite,
        ),
        Verdict::Proceed
    ));
}

// ---- residual 2: a commit made during the session must not disarm ------

#[test]
fn committing_the_unsaved_line_mid_session_does_not_disarm_the_guard() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    // Pin happens here, before the commit — exactly as it does at session
    // start in the product.
    assert!(matches!(
        g.assess(
            &p,
            "parser.py",
            &f.read("parser.py"),
            &f.read("parser.py"),
            Mode::Rewrite
        ),
        Verdict::Proceed
    ));
    // The A-2 agent's documented habit: commit straight onto main.
    git(&f.root, &["add", "parser.py"]);
    git(&f.root, &["commit", "-qm", "wip"]);
    let msg = assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("# WIP do not touch"), "{msg}");
}

#[test]
fn a_file_first_touched_after_a_mid_session_commit_is_judged_against_the_pin() {
    // The blob cache can mask a broken pin. Once a file's recorded contents
    // are cached under the pinned commit, later calls never ask git again, so
    // a test that assesses the same file before and after the commit passes
    // even if the lookup has been switched to live HEAD — measured: the
    // mutation of the pinned commit survived until this test existed.
    //
    // Pin on one file, then judge a *different* one for the first time after
    // the commit. That is also the path the product actually takes when an
    // agent commits and then turns to something it has not touched yet.
    let f = repo();
    f.write("seed.py", "seed\n");
    f.write("other.py", "def b():\n    return 1\n");
    git(&f.root, &["add", "seed.py", "other.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let g = f.guard();

    // Session start: the repository is pinned here, via a file that is not
    // the one under test, so nothing about `other.py` is cached.
    let seed = f.root.join("seed.py");
    g.assess(&seed, "seed.py", "seed\n", "seed\n", Mode::Rewrite);

    // The user's unsaved line, then the A-2 agent's habit of committing it.
    let p = f.write("other.py", "def b():\n    return 1\n# WIP do not touch\n");
    git(&f.root, &["add", "other.py"]);
    git(&f.root, &["commit", "-qm", "wip"]);

    let msg = assert_refused(g.assess(
        &p,
        "other.py",
        &f.read("other.py"),
        "def b():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("# WIP do not touch"), "{msg}");
}

#[test]
fn git_rm_cached_mid_session_does_not_disarm_the_guard() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        &f.read("parser.py"),
        Mode::Rewrite,
    );
    // Round 1 resolved the repo-relative path through `git ls-files`, so
    // emptying the index made every line look unrecorded.
    git(&f.root, &["rm", "-q", "--cached", "parser.py"]);
    let msg = assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("# WIP do not touch"), "{msg}");
    // and the committed body is still recognised as recorded
    assert!(!msg.contains("def a():"), "{msg}");
}

// ---- B1: a git that will not answer is not an answer -------------------

#[test]
fn a_repository_git_refuses_to_open_refuses_the_rewrite() {
    // Round 2 read git's exit 128 as an authoritative "no repository here",
    // which made every line unsaved, which made the rewrite wholesale, which
    // meant it was allowed. On these hosts that was strictly worse than round
    // 1: `safe.directory` rejection is the default for Docker bind mounts, CI
    // checkouts and sudo-run agents.
    let (f, p) = parser_fixture();
    f.break_git();
    let g = f.guard();
    let msg = assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("could not be established"), "{msg}");
    assert!(
        msg.contains("Reason: fatal:"),
        "git's own reason is missing: {msg}"
    );
    assert!(!msg.contains("in no repository"), "{msg}");
}

#[test]
fn a_repository_git_refuses_to_open_is_never_called_no_repository() {
    let (f, _p) = parser_fixture();
    f.break_git();
    let g = f.guard();
    match g.baseline_for_dir(&f.root) {
        Baseline::Unknown(_) => {}
        other => panic!("a broken repository must not resolve to {other:?}"),
    }
}

#[test]
fn the_unresolved_refusal_quotes_gits_own_reason() {
    let (f, p) = parser_fixture();
    f.break_git();
    let g = f.guard();
    let msg = assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    // git's own words, so the user learns the actual remedy rather than being
    // told something generic.
    assert!(msg.contains("bad config"), "{msg}");
}

#[test]
fn an_unresolved_baseline_never_claims_a_recovery_copy() {
    let (f, p) = parser_fixture();
    f.break_git();
    let g = f.guard();
    let note = assert_noted(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 1\n",
        Mode::Surgical,
    ));
    assert!(note.contains("No recovery copy was made"), "{note}");
    assert!(!note.contains("nothing is lost"), "{note}");
}

#[test]
fn an_unresolved_baseline_still_allows_a_write_that_drops_nothing() {
    // Fail-closed must not mean fail-useless: a broken-git environment stays
    // fully usable for every write that only adds.
    let (f, p) = parser_fixture();
    f.break_git();
    let g = f.guard();
    assert!(matches!(
        g.assess(
            &p,
            "parser.py",
            &f.read("parser.py"),
            "def a():\n    return 1\n# WIP do not touch\nnew line\n",
            Mode::Rewrite,
        ),
        Verdict::Proceed
    ));
}

#[test]
fn a_genuinely_absent_repository_is_still_recognised_without_git() {
    // The filesystem answers this one, so it holds even with no git binary:
    // no `.git` on any ancestor means nothing is recorded, and that is a fact
    // rather than a failure to establish one.
    let dir = TempDir::new().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    assert!(!repository_marker_present(&root));
    let g = UnsavedWorkGuard::new_isolated();
    assert_eq!(g.baseline_for_dir(&root), Baseline::NoRepo);
}

#[test]
fn a_broken_repository_is_told_apart_from_an_absent_one_by_the_filesystem() {
    let (f, _p) = parser_fixture();
    f.break_git();
    // git exits 128 for this directory exactly as it does for a directory
    // with no repository at all. The marker is what separates them.
    assert!(repository_marker_present(&f.root));
}

#[test]
fn a_corrupt_index_is_not_a_broken_baseline() {
    // Measured on git 2.43.0: rev-parse and ls-tree never read the index, so
    // a corrupt one is a non-event. Asserted so that a future change which
    // starts consulting the index is caught here rather than in the field.
    let (f, p) = parser_fixture();
    std::fs::write(f.root.join(".git/index"), "JUNKJUNKJUNK").unwrap();
    let g = f.guard();
    let msg = assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("# WIP do not touch"), "{msg}");
    assert!(!msg.contains("git did not answer"), "{msg}");
}

// ---- B2: the degradation must not latch --------------------------------

#[test]
fn repairing_git_rearms_the_same_guard() {
    // Round 2 memoized the failure, so one transient fault disarmed the whole
    // session: break git -> allow, repair git -> still allow, and only a
    // brand-new guard on the identical repository refused. Here the same
    // instance must recover.
    let (f, p) = parser_fixture();
    let g = f.guard();

    f.break_git();
    // A rewrite that drops only a COMMITTED line. With a baseline it is
    // provably safe; without one it cannot be, so the verdicts differ in kind
    // and the recovery is observable in both directions.
    let broken = g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "# WIP do not touch\n",
        Mode::Rewrite,
    );
    let msg = assert_refused(broken);
    assert!(msg.contains("could not be established"), "{msg}");
    assert!(!msg.contains("in no repository"), "{msg}");

    f.repair_git();
    assert!(
        matches!(
            g.assess(
                &p,
                "parser.py",
                &f.read("parser.py"),
                "# WIP do not touch\n",
                Mode::Rewrite
            ),
            Verdict::Proceed
        ),
        "the same guard must recover once git works again"
    );
}

#[test]
fn a_failed_resolution_is_never_memoized() {
    let (f, _p) = parser_fixture();
    let g = f.guard();
    f.break_git();
    assert!(matches!(g.baseline_for_dir(&f.root), Baseline::Unknown(_)));
    assert!(
        !g.dirs.lock().unwrap().contains_key(&f.root),
        "an Unknown baseline must not be cached"
    );
    f.repair_git();
    assert!(matches!(g.baseline_for_dir(&f.root), Baseline::Repo { .. }));
}

// ---- residual 3: the over-refusal, and its replacement -----------------

#[test]
fn wholesale_rewrite_of_an_untracked_file_is_allowed_against_a_verified_copy() {
    let f = repo();
    f.write("seed", "x");
    git(&f.root, &["add", "seed"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.write("notes.md", "# Deploy notes\nold step 1\nold step 2\n");
    let g = f.guard();
    let note = assert_noted(g.assess(
        &p,
        "notes.md",
        &f.read("notes.md"),
        "# Runbook\n1. deploy\n",
        Mode::Rewrite,
    ));
    assert_eq!(f.recover(&note), "# Deploy notes\nold step 1\nold step 2\n");
}

#[test]
fn a_staged_but_never_committed_file_is_allowed_against_a_verified_copy() {
    let f = repo();
    f.write("seed", "x");
    git(&f.root, &["add", "seed"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.write("new.py", "a\nb\nc\n");
    git(&f.root, &["add", "new.py"]);
    let g = f.guard();
    let note = assert_noted(g.assess(&p, "new.py", &f.read("new.py"), "z\n", Mode::Rewrite));
    assert_eq!(f.recover(&note), "a\nb\nc\n");
}

#[test]
fn a_repository_with_no_commits_yet_is_allowed_against_a_verified_copy() {
    let f = repo();
    let p = f.write("draft.md", "first thoughts\nsecond thoughts\n");
    let g = f.guard();
    let note = assert_noted(g.assess(
        &p,
        "draft.md",
        &f.read("draft.md"),
        "rewritten\n",
        Mode::Rewrite,
    ));
    assert_eq!(f.recover(&note), "first thoughts\nsecond thoughts\n");
}

#[test]
fn a_file_outside_any_repository_is_refused_because_no_copy_is_possible() {
    // Narrower than round 2, deliberately. Round 2 allowed this against a
    // plaintext copy in `~/.wayland/unsaved-work`, which is the store the
    // adversarial seat broke: secrets in clear, a no-op hardening call on
    // Windows, and no garbage collection. With no object store to write to
    // there is nowhere safe for the copy, so the honest answer is to refuse
    // rather than to claim a recoverability that does not exist.
    let dir = TempDir::new().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let p = root.join("loose.txt");
    std::fs::write(&p, "user content\n").unwrap();
    let g = UnsavedWorkGuard::new_isolated();
    let msg = assert_refused(g.assess(
        &p,
        "loose.txt",
        "user content\n",
        "different\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("in no repository"), "{msg}");
    assert!(!msg.contains("recovered with"), "{msg}");
}

#[test]
fn an_edit_outside_any_repository_says_no_copy_was_made() {
    let dir = TempDir::new().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let p = root.join("loose.txt");
    std::fs::write(&p, "user content\nsecond\n").unwrap();
    let g = UnsavedWorkGuard::new_isolated();
    let note = assert_noted(g.assess(
        &p,
        "loose.txt",
        "user content\nsecond\n",
        "second\n",
        Mode::Surgical,
    ));
    assert!(note.contains("not recoverable"), "{note}");
}

// ---- residual 4: no stale memoized baseline ----------------------------

#[test]
fn unsaved_work_created_after_the_first_write_is_still_protected() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    // First touch memoized nothing that can go stale.
    g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        &f.read("parser.py"),
        Mode::Rewrite,
    );
    f.write(
        "parser.py",
        "def a():\n    return 1\n# WIP do not touch\n# second thought, added later\n",
    );
    let msg = assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 1\n# WIP do not touch\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("second thought"), "{msg}");
}

#[test]
fn a_line_already_gone_from_disk_is_never_cited_again() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    // The line leaves disk by some other route (a Bash edit, the user).
    f.write("parser.py", "def a():\n    return 1\n");
    assert!(matches!(
        g.assess(
            &p,
            "parser.py",
            &f.read("parser.py"),
            "def a():\n    return 2\n",
            Mode::Rewrite
        ),
        Verdict::Proceed
    ));
}

#[test]
fn the_agents_own_file_is_never_protected_from_the_agent() {
    let f = repo();
    f.write("seed", "x");
    git(&f.root, &["add", "seed"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.root.join("out.txt");
    let g = f.guard();
    // create
    assert!(matches!(
        g.assess(&p, "out.txt", "", "v1 line\n", Mode::Rewrite),
        Verdict::Proceed
    ));
    g.note_written(&p, "", "v1 line\n");
    std::fs::write(&p, "v1 line\n").unwrap();
    // and rewrite it freely
    assert!(matches!(
        g.assess(&p, "out.txt", "v1 line\n", "v2 line\n", Mode::Rewrite),
        Verdict::Proceed
    ));
}

#[test]
fn carrying_a_user_line_through_does_not_launder_it_into_agent_authored() {
    let f = repo();
    f.write("seed", "x");
    git(&f.root, &["add", "seed"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.write("notes.md", "user line\ncommitted-nowhere\n");
    let g = f.guard();
    // A wholesale rewrite that happens to keep both lines plus one of its
    // own. Nothing drops, so it proceeds.
    let carried = "user line\ncommitted-nowhere\nagent line\n";
    assert!(matches!(
        g.assess(&p, "notes.md", &f.read("notes.md"), carried, Mode::Rewrite),
        Verdict::Proceed
    ));
    g.note_written(&p, &f.read("notes.md"), carried);
    f.write("notes.md", carried);
    // Second write drops the user's lines. They are still the user's.
    let note = assert_noted(g.assess(
        &p,
        "notes.md",
        &f.read("notes.md"),
        "agent line\n",
        Mode::Rewrite,
    ));
    assert_eq!(f.recover(&note), carried);
}

// ---- residual 5: counting ----------------------------------------------

#[test]
fn dropping_one_of_several_identical_unsaved_lines_is_caught() {
    let f = repo();
    f.write("p.py", "def f():\n    pass\n");
    git(&f.root, &["add", "p.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    // The user's uncommitted edit adds a second `pass`, a line that already
    // exists elsewhere in the committed file. Round 1's set comparison made
    // it invisible.
    let p = f.write("p.py", "def f():\n    pass\ndef g():\n    pass\n");
    let g = f.guard();
    let msg = assert_refused(g.assess(
        &p,
        "p.py",
        &f.read("p.py"),
        "def f():\n    pass\ndef g():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("pass"), "{msg}");
}

// ---- B3: recoverability is verified, never asserted --------------------

#[test]
fn the_recovery_copy_is_read_back_before_it_is_claimed() {
    let f = repo();
    let p = f.write("notes.md", "line one\nline two\n");
    let g = f.guard();
    let note = assert_noted(g.assess(
        &p,
        "notes.md",
        &f.read("notes.md"),
        "replaced\n",
        Mode::Rewrite,
    ));
    // The object exists, in the repository's own store, and holds exactly the
    // prior bytes — checked through the very command the note tells the user
    // to run.
    assert_eq!(f.recover(&note), "line one\nline two\n");
}

#[test]
fn a_copy_that_cannot_be_written_refuses_rather_than_proceeding() {
    let f = repo();
    let p = f.write("notes.md", "user content\n");

    // Block the exact object this copy will need, leaving the repository
    // itself perfectly healthy — so the refusal below is the copy failing and
    // not the baseline going unresolved. A mode bit would not do: this suite
    // runs as root on the build host and root ignores mode bits, so the check
    // would pass for the wrong reason. Emptying `.git/objects` would not do
    // either: git then calls the whole directory "not a git repository" and
    // the fail-closed path answers first, which is what the earlier version
    // of this test actually measured.
    let out = Command::new("git")
        .args(["hash-object", "--stdin"])
        .current_dir(&f.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write as _;
            c.stdin.take().unwrap().write_all(b"user content\n")?;
            c.wait_with_output()
        })
        .unwrap();
    let oid = String::from_utf8(out.stdout).unwrap().trim().to_owned();
    // A file where git needs the fanout directory.
    std::fs::write(f.root.join(".git/objects").join(&oid[..2]), b"blocked").unwrap();

    let g = f.guard();
    let verdict = g.assess(&p, "notes.md", "user content\n", "z\n", Mode::Rewrite);
    let msg = assert_refused(verdict);
    assert!(msg.contains("could not be made"), "{msg}");
    assert!(
        !msg.contains("git did not answer"),
        "the repo must still be readable: {msg}"
    );
}

#[test]
fn a_file_too_large_to_copy_refuses_rather_than_proceeding() {
    // The guarantee has no size exemption: past the limit the answer is a
    // refusal, never an unprotected write.
    let f = repo();
    let big = "x".repeat(MAX_RECOVERY_BYTES + 1) + "\n";
    let p = f.write("big.txt", &big);
    let g = f.guard();
    let msg = assert_refused(g.assess(&p, "big.txt", &big, "small\n", Mode::Rewrite));
    assert!(msg.contains("over the"), "{msg}");
    assert!(msg.contains("could not be made"), "{msg}");
}

#[test]
fn nothing_is_ever_written_outside_the_repository() {
    // Round 2 created `~/.wayland/unsaved-work/<session>` from the tool layer
    // and nothing ever removed it: the build box accumulated 6 session
    // directories and 21 plaintext files just from running the test suite.
    // There is no such directory to create any more, and this asserts it.
    let f = repo();
    let p = f.write("notes.md", "line one\nline two\n");
    let g = f.guard();
    let before = std::fs::read_dir(&f.root).unwrap().count();
    assert_noted(g.assess(
        &p,
        "notes.md",
        &f.read("notes.md"),
        "replaced\n",
        Mode::Rewrite,
    ));
    assert_eq!(
        std::fs::read_dir(&f.root).unwrap().count(),
        before,
        "the guard must not create anything in the work tree"
    );
}

#[test]
fn identical_prior_states_reuse_one_object() {
    let f = repo();
    let p = f.write("notes.md", "a\nb\n");
    let g = f.guard();
    let one = oid_in(&assert_noted(g.assess(
        &p,
        "notes.md",
        "a\nb\n",
        "z\n",
        Mode::Rewrite,
    )));
    let two = oid_in(&assert_noted(g.assess(
        &p,
        "notes.md",
        "a\nb\n",
        "y\n",
        Mode::Rewrite,
    )));
    // Content addressing is git's, not ours: the same prior state is the same
    // object, so repeated overwrites in one session do not accumulate.
    assert_eq!(one, two);
}

// ---- residual 7: what gets echoed back ---------------------------------

#[test]
fn quoted_lines_are_scrubbed_before_they_reach_the_model() {
    let (f, p) = parser_fixture();
    f.write(
        "parser.py",
        "def a():\n    return 1\nAWS_SECRET_ACCESS_KEY=wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLEKEY\n",
    );
    let g = f.guard();
    let msg = assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("[REDACTED:"), "{msg}");
    assert!(
        !msg.contains("wJalrXUtnFEMIK7MDENGbPxRfiCYEXAMPLEKEY"),
        "{msg}"
    );
}

#[test]
fn the_refusal_does_not_name_another_tool_to_route_around_it() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    let msg = assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(!msg.contains("Edit"), "{msg}");
}

#[test]
fn the_refusal_does_not_tell_a_transformation_to_undo_itself() {
    // Measured by the adversarial seat: a legitimate whole-file rename of a
    // symbol occurring on an unsaved line was refused with "reproduce those
    // lines", which would undo the rename. The refusal still fires — this
    // module cannot tell a modified line from a dropped one — but it must not
    // give instructions that are wrong.
    let (f, p) = parser_fixture();
    let g = f.guard();
    let msg = assert_refused(g.assess(
        &p,
        "parser.py",
        &f.read("parser.py"),
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(!msg.contains("reproduce those lines"), "{msg}");
    assert!(msg.contains("in their changed form"), "{msg}");
}

// ---- residual 1: Edit ---------------------------------------------------

#[test]
fn a_surgical_edit_that_removes_unsaved_work_copies_it() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    let before = f.read("parser.py");
    let note = assert_noted(g.assess(
        &p,
        "parser.py",
        &before,
        "def a():\n    return 1\n",
        Mode::Surgical,
    ));
    assert_eq!(f.recover(&note), before);
}

#[test]
fn a_surgical_edit_is_never_refused_on_a_dirty_tree() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    // Editing the user's own uncommitted line must stay possible: this is
    // the most common working state there is.
    assert!(!matches!(
        g.assess(
            &p,
            "parser.py",
            &f.read("parser.py"),
            "def a():\n    return 1\n# WIP do not touch it\n",
            Mode::Surgical,
        ),
        Verdict::Refuse(_)
    ));
}

#[test]
fn a_surgical_edit_that_touches_nothing_unsaved_is_silent() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    assert!(matches!(
        g.assess(
            &p,
            "parser.py",
            &f.read("parser.py"),
            "def a():\n    return 2\n# WIP do not touch\n",
            Mode::Surgical,
        ),
        Verdict::Proceed
    ));
}

// ---- round 4 --------------------------------------------------------------

/// The disposal sentence round 3 shipped was measurably false, and this pins
/// the replacement against git rather than against itself: `gc` does not
/// remove the copy, `gc --prune=now` does.
#[test]
fn the_note_names_the_command_that_actually_disposes_of_the_copy() {
    let f = repo();
    f.write("keep.txt", "keep\n");
    git(&f.root, &["add", "keep.txt"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let prior = "DEPLOY_TOKEN=abc\nother\n";
    let p = f.write("secret.env", prior);

    let note = assert_noted(f.guard().assess(
        &p,
        "secret.env",
        prior,
        "DEPLOY_TOKEN=placeholder\n",
        Mode::Rewrite,
    ));
    let oid = oid_in(&note);

    assert!(
        !note.contains("removes it in due course"),
        "the round-3 disposal claim is back: {note}"
    );
    assert!(note.contains("gc --prune=now"), "{note}");
    assert!(
        note.contains("cp -a") && note.contains("rsync") && note.contains("git clone"),
        "the note must say what carries the bytes off this machine: {note}"
    );
    assert!(note.contains("lost-found"), "{note}");
    assert!(
        note.contains("git push") && note.contains("bundle"),
        "the note must say what does NOT carry them: {note}"
    );

    // Measured, not asserted. `gc` moves it into a cruft pack and leaves it
    // readable; only `--prune=now` disposes of it.
    for _ in 0..3 {
        git(&f.root, &["gc", "-q"]);
    }
    assert!(
        f.blob_readable(&oid),
        "gc disposed of the copy after all — the note is now wrong the other way"
    );
    git(&f.root, &["gc", "-q", "--prune=now"]);
    assert!(
        !f.blob_readable(&oid),
        "--prune=now did not dispose of the copy, so the note names the wrong command"
    );
}

/// F1. The agent writes one line of text; the user writes their own copy of
/// the same text. Round 3 keyed the exemption to the text, so the user's copy
/// was unprotected for the rest of the session.
///
/// Here the user's copy arrives by editing the file, so
/// `attribution_expires_when_the_file_changes_outside_the_tool` subsumes the
/// count and *both* copies are protected. That is the stricter answer, and it
/// is the honest one: with the file no longer what the tool wrote, nothing on
/// disk says which of the two identical lines was the agent's.
#[test]
fn a_user_copy_of_a_line_the_agent_also_wrote_is_still_protected() {
    let f = repo();
    f.write("app.py", "start\n");
    git(&f.root, &["add", "app.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.root.join("app.py");
    let g = f.guard();

    // The agent introduces the line.
    g.note_written(&p, "start\n", "start\nTOKEN = load()\n");
    // The user adds their own second copy of the same text.
    let disk = "start\nTOKEN = load()\nTOKEN = load()\n";
    std::fs::write(&p, disk).unwrap();

    let msg = assert_refused(g.assess(&p, "app.py", disk, "start\n", Mode::Rewrite));
    assert!(
        msg.contains("2 line(s)"),
        "the user's copy must be protected, and once the file has moved the \
         agent's own copy cannot be told from it: {msg}"
    );
}

/// The count is still load-bearing where attribution survives: one tool write
/// that adds a second copy of a line the user already had. Exempting the text
/// would take the user's original with it — round 3's F1 defect, on the one
/// path the expiry rule does not reach.
#[test]
fn a_tool_write_that_duplicates_a_user_line_exempts_only_its_own_copy() {
    let f = repo();
    f.write("app.py", "start\n");
    git(&f.root, &["add", "app.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.root.join("app.py");
    let g = f.guard();

    // The user's own uncommitted line is already there when the tool writes.
    let before = "start\nTOKEN = load()\n";
    let after = "start\nTOKEN = load()\nTOKEN = load()\n";
    std::fs::write(&p, after).unwrap();
    g.note_written(&p, before, after);

    let msg = assert_refused(g.assess(&p, "app.py", after, "start\n", Mode::Rewrite));
    assert!(
        msg.contains("1 line(s)"),
        "exactly the user's copy is protected, not both and not neither: {msg}"
    );
}

/// The other half of the same granularity: a line the agent wrote and then
/// removed stops being agent-authored, so a user who types that text back
/// afterwards gets the protection.
#[test]
fn a_line_the_agent_wrote_and_removed_is_no_longer_its_own() {
    let f = repo();
    f.write("app.py", "start\n");
    git(&f.root, &["add", "app.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.root.join("app.py");
    let g = f.guard();

    g.note_written(&p, "start\n", "start\nTMP = 1\n");
    g.note_written(&p, "start\nTMP = 1\n", "start\n");

    let disk = "start\nTMP = 1\n";
    std::fs::write(&p, disk).unwrap();
    let msg = assert_refused(g.assess(&p, "app.py", disk, "start\n", Mode::Rewrite));
    assert!(msg.contains("1 line(s)"), "{msg}");
}

/// R1. Round 3 stopped memoizing `Unknown` and started memoizing `NoRepo`,
/// which it had just made a refusing state — the same latch one class over.
#[test]
fn initialising_a_repository_rearms_the_same_guard() {
    let dir = TempDir::new().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let p = root.join("notes.txt");
    std::fs::write(&p, "one\ntwo\n").unwrap();
    let g = UnsavedWorkGuard::new_isolated();

    let first = assert_refused(g.assess(&p, "notes.txt", "one\ntwo\n", "one\n", Mode::Rewrite));
    assert!(first.contains("in no repository"), "{first}");

    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["add", "notes.txt"]);
    git(&root, &["commit", "-qm", "base"]);

    // Every line is now provably recorded, so nothing unsaved can be dropped.
    match g.assess(&p, "notes.txt", "one\ntwo\n", "one\n", Mode::Rewrite) {
        Verdict::Proceed => {}
        other => panic!("the same guard is still latched: {other:?}"),
    }
}

/// A work tree git reports as bare. Round 4 routed this to `NoRepo` and so
/// told the user "this file is in no repository, so nothing about it is
/// recorded anywhere" about a repository whose HEAD records the file — fail
/// closed, but false, with a remedy that would not have helped. It is now the
/// unresolved state, and like every other unresolved state it must not latch.
#[test]
fn repairing_core_bare_rearms_the_same_guard() {
    let f = repo();
    f.write("notes.txt", "one\ntwo\n");
    git(&f.root, &["add", "notes.txt"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.root.join("notes.txt");
    let g = f.guard();

    git(&f.root, &["config", "core.bare", "true"]);
    let first = assert_refused(g.assess(&p, "notes.txt", "one\ntwo\n", "one\n", Mode::Rewrite));
    assert!(first.contains("no work tree"), "{first}");
    assert!(
        !first.contains("in no repository"),
        "the repository is right there and its HEAD records this file: {first}"
    );

    git(&f.root, &["config", "core.bare", "false"]);
    match g.assess(&p, "notes.txt", "one\ntwo\n", "one\n", Mode::Rewrite) {
        Verdict::Proceed => {}
        other => panic!("the same guard is still latched to NoRepo: {other:?}"),
    }
}

/// S2. The marker walk goes upward, and that is the whole basis of the
/// filesystem repository test: a file in a subdirectory of a repository git
/// refuses to open must still be told apart from a file in no repository.
#[test]
fn a_subdirectory_of_a_repository_git_refuses_to_open_still_refuses_for_the_right_reason() {
    let f = repo();
    f.write("src/parser.py", "def a():\n    return 1\n");
    git(&f.root, &["add", "src/parser.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.write("src/parser.py", "def a():\n    return 1\n# WIP\n");
    f.break_git();

    let msg = assert_refused(f.guard().assess(
        &p,
        "src/parser.py",
        "def a():\n    return 1\n# WIP\n",
        "def a():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(
        msg.contains("could not be established") && !msg.contains("in no repository"),
        "a subdirectory of a broken repository read as no repository at all — B1: {msg}"
    );
}

/// S3. The erring-to-present rule is what stops a probe failure becoming a
/// fail-open. Both directions, plus a real filesystem error that is not
/// `NotFound`.
#[test]
fn only_a_definite_not_found_is_read_as_no_marker() {
    use std::io::{Error, ErrorKind};

    assert!(!marker_probe_is_present(Err(Error::from(
        ErrorKind::NotFound
    ))));
    assert!(marker_probe_is_present(Err(Error::from(
        ErrorKind::PermissionDenied
    ))));

    let dir = TempDir::new().unwrap();
    let regular = dir.path().join("regular");
    std::fs::write(&regular, "not a directory\n").unwrap();
    assert!(marker_probe_is_present(std::fs::symlink_metadata(
        regular.join("sub/.git")
    )));
    assert!(marker_probe_is_present(std::fs::symlink_metadata(
        dir.path()
    )));
    assert!(repository_marker_present(&regular.join("sub")));
}

/// armD, measured live: `$HOME` is a dotfiles repository and the private file
/// is `~/work/env.local`. The repository encloses the file but records nothing
/// under its directory, so it is not this file's archive and the secrets do
/// not go into it.
#[test]
fn a_repository_that_records_nothing_under_the_files_directory_is_not_its_store() {
    let f = repo();
    f.write(".zshrc", "export EDITOR=vi\n");
    git(&f.root, &["add", ".zshrc"]);
    git(&f.root, &["commit", "-qm", "dotfiles"]);
    let prior = "STRIPE_KEY=sk_live_x\nDB_PASSWORD=hunter2\n";
    let p = f.write("work/env.local", prior);

    let msg = assert_refused(f.guard().assess(
        &p,
        "work/env.local",
        prior,
        "STRIPE_KEY=<placeholder>\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("records nothing under work"), "{msg}");
    assert!(
        msg.contains("nothing was copied"),
        "the refusal must say the bytes did not go anywhere: {msg}"
    );

    // Nothing was filed into the dotfiles repository.
    let out = Command::new("git")
        .args(["fsck", "--no-progress"])
        .current_dir(&f.root)
        .output()
        .unwrap();
    let report =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(
        !report.contains("dangling blob"),
        "prior bytes were written into a repository that is not this file's archive: {report}"
    );
}

/// The same shape through Edit: never refused, but no copy goes into a
/// repository that is not this file's archive, and it says so.
#[test]
fn an_edit_in_a_repository_that_is_not_this_files_archive_makes_no_copy() {
    let f = repo();
    f.write(".zshrc", "export EDITOR=vi\n");
    git(&f.root, &["add", ".zshrc"]);
    git(&f.root, &["commit", "-qm", "dotfiles"]);
    let prior = "STRIPE_KEY=sk_live_x\nDB_PASSWORD=hunter2\n";
    let p = f.write("work/env.local", prior);

    let note = assert_noted(f.guard().assess(
        &p,
        "work/env.local",
        prior,
        "STRIPE_KEY=sk_live_x\n",
        Mode::Surgical,
    ));
    assert!(note.contains("no recovery copy"), "{note}");
    assert!(note.contains("not recoverable"), "{note}");
    assert!(
        !note.contains("cat-file blob"),
        "a copy was claimed that was not made: {note}"
    );
}

/// The negative control for the rule above: a subdirectory the repository does
/// record is still its own, so an untracked file there is copied as before.
#[test]
fn a_subdirectory_the_repository_records_is_still_its_store() {
    let f = repo();
    f.write("src/a.py", "tracked\n");
    git(&f.root, &["add", "src/a.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let prior = "scratch one\nscratch two\n";
    let p = f.write("src/b.py", prior);

    let note =
        assert_noted(
            f.guard()
                .assess(&p, "src/b.py", prior, "scratch one\n", Mode::Rewrite),
        );
    assert_eq!(f.recover(&note), prior);
}

// ---- round 4: every sentence in the note, measured against git -------------

/// Bar 3. The note makes six checkable claims about where the recovery object
/// goes and what carries it. Round 3 shipped a disposal sentence that was
/// simply false, and the way that got through was that nothing ever executed
/// it. Each claim here is exercised against real git, and the two negative
/// claims (`git push`, `git bundle`) are what stop the arm being the vacuous
/// "everything carries everything".
#[cfg(unix)] // cp / tar / touch
#[test]
fn every_travel_claim_the_note_makes_is_executed_against_git() {
    let f = repo();
    f.write("keep.txt", "keep\n");
    git(&f.root, &["add", "keep.txt"]);
    git(&f.root, &["commit", "-qm", "base"]);
    const CANARY: &str = "STRIPE_SECRET=sk_live_INV2R4CANARY";
    let prior = format!("{CANARY}\nsecond line\n");
    let p = f.write("notes.md", &prior);

    let note =
        assert_noted(
            f.guard()
                .assess(&p, "notes.md", &prior, "second line\n", Mode::Rewrite),
        );
    let oid = oid_in(&note);

    let out = TempDir::new().unwrap();
    let out = std::fs::canonicalize(out.path()).unwrap();

    // The note says the bytes travel with a filesystem copy.
    let cp = out.join("cp");
    let st = Command::new("cp")
        .args(["-a", f.root.to_str().unwrap(), cp.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(st.success());
    assert!(readable_in(&cp, &oid), "cp -a did not carry the copy");

    let tarball = out.join("t.tar");
    let untar = out.join("untar");
    std::fs::create_dir_all(&untar).unwrap();
    assert!(
        Command::new("tar")
            .args(["cf", tarball.to_str().unwrap(), "-C"])
            .arg(f.root.parent().unwrap())
            .arg(f.root.file_name().unwrap())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("tar")
            .args([
                "xf",
                tarball.to_str().unwrap(),
                "-C",
                untar.to_str().unwrap()
            ])
            .status()
            .unwrap()
            .success()
    );
    let untarred = untar.join(f.root.file_name().unwrap());
    assert!(readable_in(&untarred, &oid), "tar did not carry the copy");

    // And with `git clone` of the local path — the channel that matters most,
    // because a clone is the one copy a user believes is filtered.
    let cloned = out.join("clone");
    assert!(
        Command::new("git")
            .args(["clone", "-q"])
            .arg(&f.root)
            .arg(&cloned)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        readable_in(&cloned, &oid),
        "git clone of the local path did not carry the copy"
    );

    // `git fsck --lost-found` writes it out as plaintext, which is how it
    // stops being a thing only git can read.
    assert!(
        Command::new("git")
            .args(["fsck", "--lost-found", "--no-progress"])
            .current_dir(&f.root)
            .output()
            .unwrap()
            .status
            .success()
    );
    let found = f.root.join(".git/lost-found/other").join(&oid);
    let plaintext = std::fs::read_to_string(&found)
        .unwrap_or_else(|e| panic!("lost-found did not materialise {}: {e}", found.display()));
    assert!(
        plaintext.contains(CANARY),
        "the note claims fsck writes plaintext; it wrote {plaintext:?}"
    );
    std::fs::remove_dir_all(f.root.join(".git/lost-found")).unwrap();

    // The negative half. Without these two the arm proves nothing: a test that
    // only ever expects "yes" cannot fail for the right reason.
    let bare = out.join("remote.git");
    assert!(
        Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&bare)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["push", "-q"])
            .arg(&bare)
            .arg("--all")
            .current_dir(&f.root)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        !readable_in(&bare, &oid),
        "the note says push does not carry the copy, and it did"
    );

    let bundle = out.join("all.bundle");
    assert!(
        Command::new("git")
            .args(["bundle", "create", "-q"])
            .arg(&bundle)
            .arg("--all")
            .current_dir(&f.root)
            .status()
            .unwrap()
            .success()
    );
    let from_bundle = out.join("from-bundle");
    assert!(
        Command::new("git")
            .args(["clone", "-q"])
            .arg(&bundle)
            .arg(&from_bundle)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        !readable_in(&from_bundle, &oid),
        "the note says a bundle does not carry the copy, and it did"
    );
}

/// The other measured half of the disposal sentence: an *ordinary* gc does
/// dispose of the copy once `gc.pruneExpire` has passed. Backdating the loose
/// object is the only way to observe the two-week default inside a test, and
/// without this arm the note's "two weeks" is an unexecuted claim.
#[cfg(unix)] // cp / tar / touch
#[test]
fn an_ordinary_gc_disposes_of_the_copy_once_the_prune_window_has_passed() {
    let f = repo();
    f.write("keep.txt", "keep\n");
    git(&f.root, &["add", "keep.txt"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let prior = "one\ntwo\n";
    let p = f.write("notes.md", prior);
    let note = assert_noted(
        f.guard()
            .assess(&p, "notes.md", prior, "two\n", Mode::Rewrite),
    );
    let oid = oid_in(&note);

    let loose = f
        .root
        .join(format!(".git/objects/{}/{}", &oid[..2], &oid[2..]));
    assert!(loose.exists(), "the copy is not a loose object: {loose:?}");

    // The control travels in the same gc run: an object of identical shape
    // whose only difference is its age. Without it, "gc removed it" would not
    // distinguish the prune window from gc removing unreferenced objects
    // outright — which is exactly what round 3 believed.
    let fresh = hash_object(&f.root, "a fresh unreferenced object\n");
    assert_ne!(fresh, oid);

    // The note's other unexecuted sentence: `git gc --auto` does not fire for
    // one object. Measured here, on both sides of the prune window, so it is
    // a result rather than a quotation of gc.auto's 6700 default.
    git(&f.root, &["gc", "-q", "--auto"]);
    assert!(
        f.blob_readable(&oid),
        "gc --auto disposed of the copy, so the note is wrong about it"
    );

    assert!(
        Command::new("touch")
            .args(["-d", "3 weeks ago"])
            .arg(&loose)
            .status()
            .unwrap()
            .success()
    );
    git(&f.root, &["gc", "-q", "--auto"]);
    assert!(
        f.blob_readable(&oid),
        "gc --auto fired once the object was old enough; the note says it \
         does not fire at all for one object"
    );
    git(&f.root, &["gc", "-q"]);
    assert!(
        f.blob_readable(&fresh),
        "control failed: gc removed a fresh unreferenced object too, so this \
         arm does not measure the prune window at all"
    );
    assert!(
        !f.blob_readable(&oid),
        "an ordinary gc did not prune a three-week-old copy, so the note's \
         two-week window is wrong"
    );
}

// Only the unix-gated prune-window arm needs this.
#[cfg(unix)]
/// `git hash-object -w --stdin`, the way the guard itself makes the copy.
fn hash_object(root: &Path, bytes: &str) -> String {
    use std::io::Write as _;
    let mut child = Command::new("git")
        .args(["hash-object", "-w", "--stdin"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(bytes.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A13. "read back byte-for-byte" is the flagship phrase of this module's
/// guarantee, and until round 5 nothing exercised the comparison: swapping it
/// for a length check survived the entire suite, because no live `git` will
/// ever hand back the wrong bytes at the right length. The predicate is named
/// so it can be handed exactly that.
#[test]
fn a_read_back_of_the_right_length_and_the_wrong_bytes_is_not_a_match() {
    let original = b"aws_secret_access_key = AKIAREAL0000\n";
    let same_length_different_bytes = b"aws_secret_access_key = AKIAFAKE0000\n";
    assert_eq!(
        original.len(),
        same_length_different_bytes.len(),
        "the fixture only means something if the lengths match"
    );
    assert!(read_back_matches(original, original));
    assert!(
        !read_back_matches(same_length_different_bytes, original),
        "a length-only comparison would call these two the same bytes"
    );
    // Truncation is caught as well, and each is described for what it is.
    assert!(!read_back_matches(b"aws", original));
    let by_byte = read_back_mismatch(same_length_different_bytes, original);
    assert!(by_byte.contains("differs at byte"), "{by_byte}");
    let by_length = read_back_mismatch(b"aws", original);
    assert!(by_length.contains("bytes rather than"), "{by_length}");
}

/// And end to end: whatever the file held is what comes back out of the
/// object store, through the very command the note prints. Content chosen to
/// break anything that normalises on the way through — CRLF, trailing
/// whitespace, a lone CR, no final newline, and non-ASCII.
#[test]
fn the_recovered_copy_is_the_prior_file_byte_for_byte() {
    let f = repo();
    f.write("keep.txt", "keep\n");
    git(&f.root, &["add", "keep.txt"]);
    git(&f.root, &["commit", "-qm", "base"]);

    let prior = "first\r\nsecond   \ntabs\there\rcarriage\nnaïve — ünicode\nno final newline";
    let p = f.write("draft.txt", prior);
    let note = assert_noted(
        f.guard()
            .assess(&p, "draft.txt", prior, "replaced\n", Mode::Rewrite),
    );
    let recovered = f.recover(&note);
    assert_eq!(
        recovered.as_bytes(),
        prior.as_bytes(),
        "the recovery copy is not the prior file"
    );
}

/// R22. The pre-write re-read compares bytes, not lengths. A save that
/// replaces a line with one of the same length is exactly the shape a length
/// check would wave through, and no end-to-end arm can produce it reliably:
/// the interleaving one measures a save that adds a line.
#[test]
fn the_pre_write_re_read_compares_bytes_and_not_lengths() {
    let f = repo();
    let judged = "TOKEN=aaaaaaaa\n";
    let same_length = "TOKEN=bbbbbbbb\n";
    assert_eq!(judged.len(), same_length.len());
    let p = f.write("x.env", judged);

    assert!(pre_image_unchanged(&p, Some(judged.as_bytes())).is_ok());

    f.write("x.env", same_length);
    let moved = pre_image_unchanged(&p, Some(judged.as_bytes()))
        .expect_err("a same-length change is still a change");
    assert!(moved.contains("changed on disk"), "{moved}");

    // Deleted, and created underneath a create, are both changes too.
    std::fs::remove_file(&p).unwrap();
    assert!(
        pre_image_unchanged(&p, Some(judged.as_bytes()))
            .expect_err("deleted")
            .contains("deleted")
    );
    assert!(pre_image_unchanged(&p, None).is_ok());
    f.write("x.env", judged);
    assert!(
        pre_image_unchanged(&p, None)
            .expect_err("created underneath")
            .contains("created")
    );
}

/// Bar 3. The note names the store git named, and the copy is in it. Round
/// 4's first draft printed `<root>/.git/objects` unconditionally, which is a
/// path that does not exist whenever `.git` is a file.
#[test]
fn the_note_names_the_object_store_that_actually_holds_the_copy() {
    let f = repo();
    f.write("keep.txt", "keep\n");
    git(&f.root, &["add", "keep.txt"]);
    git(&f.root, &["commit", "-qm", "base"]);

    let prior = "draft one\ndraft two\n";
    let p = f.write("draft.md", prior);

    let note = assert_noted(
        f.guard()
            .assess(&p, "draft.md", prior, "draft two\n", Mode::Rewrite),
    );
    let oid = oid_in(&note);

    let claimed = objects_dir_in(&note);
    assert!(
        Path::new(&claimed).is_dir(),
        "the note names {claimed}, which is not a directory"
    );
    let loose = Path::new(&claimed).join(&oid[..2]).join(&oid[2..]);
    let packed = Path::new(&claimed).join("pack");
    assert!(
        loose.exists() || packed.exists(),
        "the note names {claimed}, and the copy is not in it"
    );
}

/// And in a linked worktree there is no such store to name: `--git-path
/// objects` resolves into the **main** repository, so a copy would leave the
/// tree the user is working in entirely. Round 4 made that copy and told the
/// user about it; round 5 refuses instead, because a copy whose exposure is
/// not this tree's cannot be bounded from here.
#[test]
fn a_linked_worktree_refuses_rather_than_copying_into_the_main_repository() {
    let f = repo();
    f.write("keep.txt", "keep\n");
    git(&f.root, &["add", "keep.txt"]);
    git(&f.root, &["commit", "-qm", "base"]);

    let elsewhere = TempDir::new().unwrap();
    let wt = std::fs::canonicalize(elsewhere.path())
        .unwrap()
        .join("linked-worktree");
    git(
        &f.root,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "wtb"],
    );
    assert!(
        wt.join(".git").is_file(),
        "positive control: a linked worktree's .git must be a file, or this \
         arm is testing an ordinary clone"
    );

    let prior = "TOKEN=CANARY-WORKTREE-4242\ndraft two\n";
    let p = wt.join("draft.md");
    std::fs::write(&p, prior).unwrap();

    let message =
        assert_refused(
            f.guard()
                .assess(&p, "draft.md", prior, "draft two\n", Mode::Rewrite),
        );
    assert!(
        message.contains("outside the tree this file is in"),
        "the refusal must name the reason: {message}"
    );
    assert!(
        !object_store_contains(&f.root, "CANARY-WORKTREE-4242"),
        "the bytes were filed into the main repository anyway"
    );
}

/// The directory the note tells the user their bytes are sitting in.
fn objects_dir_in(note: &str) -> String {
    let marker = "these bytes live in ";
    let start = note.find(marker).expect("the note names an object store") + marker.len();
    note[start..]
        .split_whitespace()
        .next()
        .expect("the object store path is terminated")
        .trim_end_matches(',')
        .to_owned()
}

// Only the unix-gated travel arm needs this.
#[cfg(unix)]
fn readable_in(root: &Path, oid: &str) -> bool {
    Command::new("git")
        .args(["cat-file", "blob", oid])
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---- round 4: a file the repository is told to ignore is not its to hold ---

/// Bar 4. A recovery copy is verbatim by construction — scrubbing it would
/// make it not a recovery copy — so the only lever on secrets is *where* the
/// bytes are allowed to go. `.gitignore` is the user saying, in the
/// repository's own configuration, that this file does not belong in this
/// repository. Filing its prior bytes into that repository's object store
/// contradicts that instruction, and it is not a wash: the object then travels
/// with `git clone <path>` (measured in
/// `every_travel_claim_the_note_makes_is_executed_against_git`), which is the
/// one copy a user believes their ignore rules filtered.
///
/// Round 3 shipped exactly this: the arm below found the key in the object
/// store, unscrubbed, after a write that was allowed.
#[test]
fn a_file_the_repository_ignores_is_refused_rather_than_filed_into_it() {
    const CANARY: &str = "STRIPE_SECRET=sk_live_INV2R4IGNORED";
    let f = repo();
    f.write(".gitignore", ".env\n");
    git(&f.root, &["add", ".gitignore"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let prior = format!("{CANARY}\nFEATURE=1\n");
    let p = f.write(".env", &prior);

    // Positive control: git itself agrees this path is ignored, so the arm is
    // not passing because the ignore rule never applied.
    assert!(
        check_ignore(&f.root, ".env"),
        "control failed: git does not consider .env ignored here"
    );

    let message =
        assert_refused(
            f.guard()
                .assess(&p, ".env", &prior, "FEATURE=2\n", Mode::Rewrite),
        );
    assert!(
        message.contains("ignore"),
        "the refusal must say why this repository is not the file's archive: {message}"
    );
    assert_eq!(f.read(".env"), prior, "a refusal changes nothing");
    assert!(
        !object_store_contains(&f.root, CANARY),
        "the user's key was filed into a repository their own .gitignore \
         says this file does not belong in"
    );
}

/// The Edit half: never refused, but the bytes still do not go in, and it says
/// so instead of claiming a recovery that would sit somewhere the user
/// excluded.
#[test]
fn an_edit_to_an_ignored_file_makes_no_copy_and_says_so() {
    const CANARY: &str = "AWS_SECRET_ACCESS_KEY=INV2R4EDITCANARY";
    let f = repo();
    f.write(".gitignore", "*.env\n");
    f.write("secrets/keep", "x\n");
    git(&f.root, &["add", ".gitignore", "secrets/keep"]);
    git(&f.root, &["commit", "-qm", "base"]);
    // The directory IS recorded, so nothing but the ignore rule can make this
    // repository a non-archive. Without that the arm would pass for the armD
    // reason and prove nothing about ignoring.
    let prior = format!("{CANARY}\nkeep me\n");
    let p = f.write("secrets/prod.env", &prior);

    let note =
        assert_noted(
            f.guard()
                .assess(&p, "secrets/prod.env", &prior, "keep me\n", Mode::Surgical),
        );
    assert!(note.contains("not recoverable"), "{note}");
    assert!(
        !note.contains("cat-file blob"),
        "an Edit that made no copy must not print a recovery command: {note}"
    );
    assert!(!object_store_contains(&f.root, CANARY), "{note}");
}

/// The negative control for the rule above, and the reason it is `.gitignore`
/// specifically rather than "untracked": a merely untracked file in a
/// directory the repository records is one `git add` from being tracked, so
/// the repository plainly is its archive and the copy still goes in.
#[test]
fn a_merely_untracked_file_is_still_copied_into_its_own_repository() {
    let f = repo();
    f.write(".gitignore", ".env\n");
    git(&f.root, &["add", ".gitignore"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let prior = "# Deploy notes\nstep one\n";
    let p = f.write("notes.md", prior);
    assert!(!check_ignore(&f.root, "notes.md"));

    let note = assert_noted(
        f.guard()
            .assess(&p, "notes.md", prior, "# Runbook\n", Mode::Rewrite),
    );
    assert_eq!(f.recover(&note), prior);
}

/// A tracked file is never "ignored" even when a rule would match it, so
/// adding the ignore probe cannot start refusing writes to ordinary committed
/// files. Measured: `git check-ignore` consults the index and exits 1 here.
#[test]
fn a_tracked_file_matched_by_an_ignore_rule_is_still_its_repositorys() {
    let f = repo();
    f.write("build.log", "line one\n");
    git(&f.root, &["add", "-f", "build.log"]);
    git(&f.root, &["commit", "-qm", "base"]);
    f.write(".gitignore", "*.log\n");
    git(&f.root, &["add", ".gitignore"]);
    git(&f.root, &["commit", "-qm", "ignore logs"]);
    let prior = "line one\nuncommitted line\n";
    let p = f.write("build.log", prior);
    assert!(
        !check_ignore(&f.root, "build.log"),
        "control failed: git calls a tracked file ignored, so this arm proves \
         nothing about the tracked case"
    );

    // Tracked and partially committed: the ordinary partial-rewrite refusal,
    // which quotes the line — not the ignore refusal.
    let message =
        assert_refused(
            f.guard()
                .assess(&p, "build.log", prior, "line one\n", Mode::Rewrite),
        );
    assert!(message.contains("uncommitted line"), "{message}");
    assert!(!message.contains("ignore"), "{message}");
}

fn check_ignore(root: &Path, rel: &str) -> bool {
    Command::new("git")
        .args(["check-ignore", "-q", "--", rel])
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.code() == Some(0))
        .unwrap_or(false)
}

/// Is `needle` anywhere in this repository's object database? Exhaustive, so
/// it cannot miss a copy by looking in the wrong place.
fn object_store_contains(root: &Path, needle: &str) -> bool {
    let out = Command::new("git")
        .args(["cat-file", "--batch-all-objects", "--batch"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(needle)
}

// ---- round 4: attribution expires the moment the file moves underneath -----

/// Bar 5, the half a count does not reach. The whole premise of this guard is
/// that the user is editing the file in their own editor, so the file *will*
/// change between two tool writes — and `note_written` is the only thing that
/// ever updates the agent-authored tally. Round 3's map, and round 4's count,
/// both went on exempting a line after the file underneath had become
/// something the tool never wrote.
///
/// Measured: the agent writes `log('start')`, the user edits the file in their
/// editor, and a later rewrite drops that line silently — because the guard is
/// still attributing it to the agent on the strength of a write two states ago.
#[test]
fn attribution_expires_when_the_file_changes_outside_the_tool() {
    let f = repo();
    f.write("app.py", "import os\n");
    git(&f.root, &["add", "app.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.root.join("app.py");
    let g = f.guard();

    let agent_wrote = "import os\nlog('start')\n";
    f.write("app.py", agent_wrote);
    g.note_written(&p, "import os\n", agent_wrote);

    // Control: with the disk still exactly as the tool left it, the agent's
    // own line is still the agent's and dropping it proceeds. Without this the
    // arm below could pass simply because the exemption never worked.
    assert!(
        matches!(
            g.assess(&p, "app.py", agent_wrote, "import os\n", Mode::Rewrite),
            Verdict::Proceed
        ),
        "the agent's own file must not be protected from the agent"
    );

    // Now the user edits it in their editor.
    let user_edited = "import os\nlog('start')\n# WIP: do not lose this\n";
    f.write("app.py", user_edited);

    // A rewrite that keeps the user's visible note and drops only the line the
    // tool once wrote. Nothing on disk can tell that line apart from one the
    // user typed themselves, so it is not the tool's to drop.
    let msg = assert_refused(g.assess(
        &p,
        "app.py",
        user_edited,
        "import os\n# WIP: do not lose this\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("log('start')"), "{msg}");
}

/// The same expiry seen from the other end: once the file has moved, a *new*
/// tool write re-establishes attribution from the state actually on disk, so
/// the guard does not stay locked out for the rest of the session.
#[test]
fn a_later_tool_write_re_establishes_attribution_from_the_disk() {
    let f = repo();
    f.write("app.py", "import os\n");
    git(&f.root, &["add", "app.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.root.join("app.py");
    let g = f.guard();

    g.note_written(&p, "import os\n", "import os\nlog('start')\n");
    let user_edited = "import os\nlog('start')\n# user note\n";
    f.write("app.py", user_edited);

    // The tool writes again, from the state that is really there, adding a
    // line of its own.
    let now = "import os\nlog('start')\n# user note\nlog('end')\n";
    g.note_written(&p, user_edited, now);
    f.write("app.py", now);

    // Its own new line is its own to drop...
    assert!(matches!(
        g.assess(&p, "app.py", now, user_edited, Mode::Rewrite),
        Verdict::Proceed
    ));
    // ...and the line it wrote before the user's edit is not.
    let msg = assert_refused(g.assess(
        &p,
        "app.py",
        now,
        "import os\n# user note\nlog('end')\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("log('start')"), "{msg}");
}

/// P2b. The pin is per *repository root*, and `dirs` memoizes per directory,
/// so the only way a re-pin is observable is a second directory of the same
/// repository resolving for the first time after a mid-session commit. The
/// existing pin arm judges a second *file* in the same directory, which the
/// directory memo answers without ever re-entering the pin — so switching the
/// pin from `or_insert_with` to `insert` survived the whole suite.
#[test]
fn a_second_directory_of_the_same_repository_inherits_the_original_pin() {
    let f = repo();
    f.write("a/seed.py", "seed\n");
    f.write("b/other.py", "def b():\n    return 1\n");
    git(&f.root, &["add", "a/seed.py", "b/other.py"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let g = f.guard();

    // Session start pins the repository, through a directory that is not the
    // one under test.
    let seed = f.root.join("a/seed.py");
    g.assess(&seed, "a/seed.py", "seed\n", "seed\n", Mode::Rewrite);

    // The user's unsaved line, and the agent's habit of committing it.
    let p = f.write("b/other.py", "def b():\n    return 1\n# WIP do not touch\n");
    git(&f.root, &["add", "b/other.py"]);
    git(&f.root, &["commit", "-qm", "wip"]);

    // `b/` has never been resolved before, so this goes through the pin map
    // rather than the directory memo. Judged against the pinned commit, the
    // line is still unsaved work.
    let msg = assert_refused(g.assess(
        &p,
        "b/other.py",
        &f.read("b/other.py"),
        "def b():\n    return 2\n",
        Mode::Rewrite,
    ));
    assert!(msg.contains("# WIP do not touch"), "{msg}");

    // Control: a guard that starts *after* the commit sees the same line as
    // recorded and proceeds, so the arm above is measuring the pin and not
    // some blanket refusal.
    let fresh = f.guard();
    assert!(matches!(
        fresh.assess(
            &p,
            "b/other.py",
            &f.read("b/other.py"),
            "def b():\n    return 2\n",
            Mode::Rewrite
        ),
        Verdict::Proceed
    ));
}
