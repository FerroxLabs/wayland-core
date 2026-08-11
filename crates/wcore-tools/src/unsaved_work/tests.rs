//! Unit tests for the INV-2 unsaved-work guarantee.
//!
//! Split out of `unsaved_work.rs` to keep that file under the 1000-line
//! limit, matching the existing `bash.rs` / `bash/tests.rs` pattern.

use super::*;
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    snaps: PathBuf,
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
    let snaps = root.join("_snapshots_outside");
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    Fixture {
        _dir: dir,
        root,
        snaps,
    }
}

impl Fixture {
    fn guard(&self) -> UnsavedWorkGuard {
        UnsavedWorkGuard::with_snapshot_root(self.snaps.clone())
    }
    fn write(&self, name: &str, body: &str) -> PathBuf {
        let p = self.root.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }
    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap()
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

fn assert_snapshotted(v: Verdict) -> String {
    match v {
        Verdict::ProceedWithSnapshot(m) => m,
        other => panic!("expected ProceedWithSnapshot, got {other:?}"),
    }
}

fn snapshot_path(note: &str) -> PathBuf {
    let start = note.find("copied to ").expect("note names a path") + "copied to ".len();
    let rest = &note[start..];
    let end = rest.find(" first,").expect("note terminates the path");
    PathBuf::from(rest[..end].trim())
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

// ---- residual 3: the over-refusal, and its replacement -----------------

#[test]
fn wholesale_rewrite_of_an_untracked_file_is_allowed_and_snapshotted() {
    let f = repo();
    f.write("seed", "x");
    git(&f.root, &["add", "seed"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.write("notes.md", "# Deploy notes\nold step 1\nold step 2\n");
    let g = f.guard();
    let note = assert_snapshotted(g.assess(
        &p,
        "notes.md",
        &f.read("notes.md"),
        "# Runbook\n1. deploy\n",
        Mode::Rewrite,
    ));
    let snap = snapshot_path(&note);
    assert_eq!(
        std::fs::read_to_string(&snap).unwrap(),
        "# Deploy notes\nold step 1\nold step 2\n"
    );
    assert!(!snap.starts_with(f.root.join(".git")), "not in the repo db");
}

#[test]
fn a_gitignored_file_is_allowed_and_snapshotted_not_refused() {
    let f = repo();
    f.write(".gitignore", ".env\n");
    git(&f.root, &["add", ".gitignore"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.write(".env", "DB=postgres://u:p@h/db\nFEATURE=1\n");
    let g = f.guard();
    let note =
        assert_snapshotted(g.assess(&p, ".env", &f.read(".env"), "FEATURE=2\n", Mode::Rewrite));
    assert_eq!(
        std::fs::read_to_string(snapshot_path(&note)).unwrap(),
        "DB=postgres://u:p@h/db\nFEATURE=1\n"
    );
}

#[test]
fn a_staged_but_never_committed_file_is_allowed_and_snapshotted() {
    let f = repo();
    f.write("seed", "x");
    git(&f.root, &["add", "seed"]);
    git(&f.root, &["commit", "-qm", "base"]);
    let p = f.write("new.py", "a\nb\nc\n");
    git(&f.root, &["add", "new.py"]);
    let g = f.guard();
    assert_snapshotted(g.assess(&p, "new.py", &f.read("new.py"), "z\n", Mode::Rewrite));
}

#[test]
fn a_file_outside_any_repository_is_allowed_and_snapshotted() {
    let dir = TempDir::new().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let p = root.join("loose.txt");
    std::fs::write(&p, "user content\n").unwrap();
    let g = UnsavedWorkGuard::with_snapshot_root(root.join("_snaps"));
    let note = assert_snapshotted(g.assess(
        &p,
        "loose.txt",
        "user content\n",
        "different\n",
        Mode::Rewrite,
    ));
    assert_eq!(
        std::fs::read_to_string(snapshot_path(&note)).unwrap(),
        "user content\n"
    );
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
    let note = assert_snapshotted(g.assess(
        &p,
        "notes.md",
        &f.read("notes.md"),
        "agent line\n",
        Mode::Rewrite,
    ));
    assert_eq!(
        std::fs::read_to_string(snapshot_path(&note)).unwrap(),
        carried
    );
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

// ---- residual 6: git that will not answer ------------------------------

#[test]
fn an_unanswerable_baseline_snapshots_and_says_so() {
    // `PATH` without git: `Command::new("git")` cannot spawn.
    let dir = TempDir::new().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let p = root.join("x.txt");
    std::fs::write(&p, "user content\n").unwrap();
    let g = UnsavedWorkGuard::with_snapshot_root(root.join("_snaps"));
    // Force the Unknown branch directly: no git binary is reachable.
    g.dirs
        .lock()
        .unwrap()
        .insert(root.clone(), Baseline::Unknown);
    let note = assert_snapshotted(g.assess(&p, "x.txt", "user content\n", "z\n", Mode::Rewrite));
    assert!(note.contains("could not be established"), "{note}");
    assert_eq!(
        std::fs::read_to_string(snapshot_path(&note)).unwrap(),
        "user content\n"
    );
}

#[test]
fn a_snapshot_that_cannot_be_written_refuses_rather_than_proceeding() {
    let f = repo();
    let p = f.write("notes.md", "user content\n");
    // Snapshot root is a path that cannot be created: an existing FILE.
    let blocker = f.root.join("blocked");
    std::fs::write(&blocker, "").unwrap();
    let g = UnsavedWorkGuard::with_snapshot_root(blocker.join("nested"));
    let msg = assert_refused(g.assess(&p, "notes.md", "user content\n", "z\n", Mode::Rewrite));
    assert!(msg.contains("could not be written"), "{msg}");
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

// ---- residual 1: Edit ---------------------------------------------------

#[test]
fn a_surgical_edit_that_removes_unsaved_work_snapshots_it() {
    let (f, p) = parser_fixture();
    let g = f.guard();
    let before = f.read("parser.py");
    let note = assert_snapshotted(g.assess(
        &p,
        "parser.py",
        &before,
        "def a():\n    return 1\n",
        Mode::Surgical,
    ));
    assert_eq!(
        std::fs::read_to_string(snapshot_path(&note)).unwrap(),
        before
    );
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

#[test]
fn identical_prior_states_reuse_one_snapshot_file() {
    let f = repo();
    let p = f.write("notes.md", "a\nb\n");
    let g = f.guard();
    let one = snapshot_path(&assert_snapshotted(g.assess(
        &p,
        "notes.md",
        "a\nb\n",
        "z\n",
        Mode::Rewrite,
    )));
    let two = snapshot_path(&assert_snapshotted(g.assess(
        &p,
        "notes.md",
        "a\nb\n",
        "y\n",
        Mode::Rewrite,
    )));
    assert_eq!(one, two);
    assert_eq!(std::fs::read_dir(&f.snaps).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn a_snapshot_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let f = repo();
    let p = f.write("notes.md", "secret-ish\n");
    let g = f.guard();
    let note = assert_snapshotted(g.assess(&p, "notes.md", "secret-ish\n", "z\n", Mode::Rewrite));
    let mode = std::fs::metadata(snapshot_path(&note))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "snapshot must not be world-readable");
}
