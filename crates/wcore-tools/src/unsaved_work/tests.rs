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
        std::fs::write(&p, body).unwrap();
        p
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
