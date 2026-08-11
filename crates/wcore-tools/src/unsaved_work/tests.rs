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
    assert!(msg.contains("git did not answer"), "{msg}");
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
    assert!(msg.contains("git did not answer"), "{msg}");

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
fn a_gitignored_file_is_allowed_against_a_verified_copy() {
    let f = repo();
    f.write(".gitignore", ".env\n");
    git(&f.root, &["add", ".gitignore"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.write(".env", "DB=postgres://u:p@h/db\nFEATURE=1\n");
    let g = f.guard();
    let note = assert_noted(g.assess(&p, ".env", &f.read(".env"), "FEATURE=2\n", Mode::Rewrite));
    assert_eq!(f.recover(&note), "DB=postgres://u:p@h/db\nFEATURE=1\n");
    // The one honest consequence of using the repository's own store, stated
    // in the tool result rather than left quietly true.
    assert!(note.contains("gitignored"), "{note}");
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
        msg.contains("1 line(s)"),
        "exactly the user's one copy is protected, not both and not neither: {msg}"
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
        other => panic!("the same guard is still latched to NoRepo: {other:?}"),
    }
}

/// The other way into `NoRepo`: a work tree git reports as bare.
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
    assert!(first.contains("in no repository"), "{first}");

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
        msg.contains("git did not answer"),
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
