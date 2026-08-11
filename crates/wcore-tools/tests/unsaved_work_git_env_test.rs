//! INV-2 — the ambient git environment must not be able to redirect the guard.
//!
//! Its own test binary on purpose: it mutates the ambient git environment,
//! which is process-global, so it must not run beside anything that shells out
//! to git. One test for the same reason.
//!
//! Round 4 named six variables as cleared and exercised two of them; removing
//! `GIT_WORK_TREE`, `GIT_COMMON_DIR` or `GIT_ALTERNATE_OBJECT_DIRECTORIES`
//! from the cleared set survived its whole suite. Every one of them has an
//! arm here, each with its own positive control proving git honours the
//! variable in that fixture.
//!
//! Two measured failures, both round 3:
//!
//! * `GIT_OBJECT_DIRECTORY` (the shape a hook inherits inside a push
//!   quarantine) redirects the recovery write **and** the read-back to the same
//!   non-repository store, so the byte-for-byte check passes, the write
//!   proceeds, and the `git -C <root> cat-file blob <oid>` the note advertises
//!   finds nothing — allow, plus a recovery claim that does not recover.
//! * `GIT_DIR` puts a file that is in no repository at all inside one, and its
//!   prior bytes into an unrelated repository.
//!
//! Each arm carries its own positive control, so a fix that merely stopped the
//! variables from mattering to `git` itself could not pass it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use wcore_tools::unsaved_work::{Mode, UnsavedWorkGuard, Verdict};

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git must be available for this test");
    assert!(status.success(), "git {args:?} failed");
}

fn repo(root: &Path) {
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["config", "commit.gpgsign", "false"]);
}

fn readable(root: &Path, oid: &str) -> bool {
    Command::new("git")
        .args(["cat-file", "blob", oid])
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
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

fn hash_object(root: &Path, bytes: &str) -> String {
    let mut child = Command::new("git")
        .args(["hash-object", "-w", "--stdin"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(bytes.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn write(path: &PathBuf, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

#[test]
fn the_ambient_git_environment_cannot_redirect_the_guard() {
    // ---- arm 1: GIT_OBJECT_DIRECTORY -------------------------------------
    let home = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(home.path()).unwrap();
    repo(&root);
    write(&root.join("keep.txt"), "keep\n");
    git(&root, &["add", "keep.txt"]);
    git(&root, &["commit", "-qm", "base"]);

    let redirect = tempfile::tempdir().unwrap();
    let objects = redirect.path().join("objects");
    std::fs::create_dir_all(&objects).unwrap();

    let prior = "DEPLOY_TOKEN=abc\nother\n";
    let target = root.join("secret.env");
    write(&target, prior);

    // SAFETY: this binary contains exactly one test, so nothing else is
    // reading or writing the environment concurrently.
    unsafe { std::env::set_var("GIT_OBJECT_DIRECTORY", &objects) };

    // Positive control: git genuinely honours the variable here, so the arm is
    // not vacuous.
    let stray = hash_object(&root, "stray control bytes\n");

    let note = match UnsavedWorkGuard::new_isolated().assess(
        &target,
        "secret.env",
        prior,
        "DEPLOY_TOKEN=<placeholder>\n",
        Mode::Rewrite,
    ) {
        Verdict::ProceedWithNote(n) => n,
        other => panic!("expected a copy to be made in the real object store, got {other:?}"),
    };
    let oid = oid_in(&note);

    unsafe { std::env::remove_var("GIT_OBJECT_DIRECTORY") };

    assert!(
        !readable(&root, &stray),
        "positive control failed: the redirect never took effect, so this arm proves nothing"
    );
    assert!(
        readable(&root, &oid),
        "the note's own recovery command cannot find the copy: it went to the redirected store"
    );

    // ---- arm 2: GIT_DIR ---------------------------------------------------
    let plain = tempfile::tempdir().unwrap();
    let plain_root = dunce::canonicalize(plain.path()).unwrap();
    let loose = plain_root.join("notes.txt");
    write(&loose, "one\ntwo\n");

    unsafe { std::env::set_var("GIT_DIR", root.join(".git")) };

    // Positive control: with the variable set, git itself calls this plain
    // directory a work tree of the other repository.
    let claim = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&plain_root)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&claim.stdout).trim(),
        "true",
        "positive control failed: git did not honour GIT_DIR, so this arm proves nothing"
    );

    let verdict = UnsavedWorkGuard::new_isolated().assess(
        &loose,
        "notes.txt",
        "one\ntwo\n",
        "one\n",
        Mode::Rewrite,
    );

    unsafe { std::env::remove_var("GIT_DIR") };

    match verdict {
        Verdict::Refuse(m) => assert!(
            m.contains("in no repository"),
            "a file in no repository was classified into one: {m}"
        ),
        other => panic!("expected a refusal for a file in no repository, got {other:?}"),
    }

    let out = Command::new("git")
        .args(["fsck", "--no-progress"])
        .current_dir(&root)
        .output()
        .unwrap();
    let report =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let dangling = report
        .lines()
        .filter(|l| l.contains("dangling blob"))
        .count();
    assert_eq!(
        dangling, 1,
        "exactly the arm-1 copy should be in this repository; arm 2's bytes must not be: {report}"
    );

    // ---- arm 3: GIT_WORK_TREE --------------------------------------------
    // Relocating the work tree somewhere that does not contain the file makes
    // `--is-inside-work-tree` answer "false" — the core.bare shape, reached
    // from the environment. Inherited, it would take a repository that
    // records every line of this file and make the guard unable to say so.
    let w = tempfile::tempdir().unwrap();
    let w_root = dunce::canonicalize(w.path()).unwrap();
    repo(&w_root);
    write(&w_root.join("f.txt"), "one\ntwo\n");
    git(&w_root, &["add", "f.txt"]);
    git(&w_root, &["commit", "-qm", "base"]);
    let elsewhere = tempfile::tempdir().unwrap();

    unsafe { std::env::set_var("GIT_WORK_TREE", elsewhere.path()) };
    let claim = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&w_root)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&claim.stdout).trim(),
        "false",
        "positive control failed: git did not honour GIT_WORK_TREE, so this arm proves nothing"
    );
    let verdict = UnsavedWorkGuard::new_isolated().assess(
        &w_root.join("f.txt"),
        "f.txt",
        "one\ntwo\n",
        "one\n",
        Mode::Rewrite,
    );
    unsafe { std::env::remove_var("GIT_WORK_TREE") };
    assert!(
        matches!(verdict, Verdict::Proceed),
        "an inherited GIT_WORK_TREE hid the commit that records every line: {verdict:?}"
    );

    // ---- arm 4: GIT_COMMON_DIR -------------------------------------------
    // Measured: it stops git opening the repository at all, which would make
    // every line of a fully committed file read as unsaved.
    let c = tempfile::tempdir().unwrap();
    let c_root = dunce::canonicalize(c.path()).unwrap();
    repo(&c_root);
    write(&c_root.join("f.txt"), "one\ntwo\n");
    git(&c_root, &["add", "f.txt"]);
    git(&c_root, &["commit", "-qm", "base"]);

    unsafe { std::env::set_var("GIT_COMMON_DIR", c_root.join("no-such-common-dir")) };
    let claim = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&c_root)
        .output()
        .unwrap();
    assert!(
        !claim.status.success(),
        "positive control failed: git ignored GIT_COMMON_DIR, so this arm proves nothing"
    );
    let verdict = UnsavedWorkGuard::new_isolated().assess(
        &c_root.join("f.txt"),
        "f.txt",
        "one\ntwo\n",
        "one\n",
        Mode::Rewrite,
    );
    unsafe { std::env::remove_var("GIT_COMMON_DIR") };
    assert!(
        matches!(verdict, Verdict::Proceed),
        "an inherited GIT_COMMON_DIR relocated the object store: {verdict:?}"
    );

    // ---- arm 5: GIT_ALTERNATE_OBJECT_DIRECTORIES -------------------------
    // An alternate store makes `cat-file` resolve objects this repository does
    // not have. Inherited, the guard would read a "recorded" version of the
    // file out of a store that is not this repository's, and call lines saved
    // that are in no commit here.
    let a = tempfile::tempdir().unwrap();
    let a_root = dunce::canonicalize(a.path()).unwrap();
    repo(&a_root);
    write(&a_root.join("f.txt"), "one\ntwo\n");
    git(&a_root, &["add", "f.txt"]);
    git(&a_root, &["commit", "-qm", "base"]);
    let blob = Command::new("git")
        .args(["rev-parse", "HEAD:f.txt"])
        .current_dir(&a_root)
        .output()
        .unwrap();
    let blob = String::from_utf8_lossy(&blob.stdout).trim().to_owned();
    let alt = tempfile::tempdir().unwrap();
    let alt_objects = alt.path().join("objects");
    let (fanout, rest) = blob.split_at(2);
    std::fs::create_dir_all(alt_objects.join(fanout)).unwrap();
    let loose = a_root.join(".git/objects").join(fanout).join(rest);
    std::fs::copy(&loose, alt_objects.join(fanout).join(rest)).unwrap();
    std::fs::remove_file(&loose).unwrap();
    assert!(
        !readable(&a_root, &blob),
        "fixture failed: this repository must no longer hold the blob"
    );

    unsafe { std::env::set_var("GIT_ALTERNATE_OBJECT_DIRECTORIES", &alt_objects) };
    assert!(
        readable(&a_root, &blob),
        "positive control failed: git ignored the alternate, so this arm proves nothing"
    );
    let verdict = UnsavedWorkGuard::new_isolated().assess(
        &a_root.join("f.txt"),
        "f.txt",
        "one\ntwo\n",
        "one\n",
        Mode::Rewrite,
    );
    unsafe { std::env::remove_var("GIT_ALTERNATE_OBJECT_DIRECTORIES") };
    match verdict {
        Verdict::Refuse(m) => assert!(
            m.contains("could not be established"),
            "refused, but not because the pinned commit could not be read: {m}"
        ),
        other => panic!("the recorded state was read out of an alternate object store: {other:?}"),
    }

    // ---- arm 6: GIT_INDEX_FILE -------------------------------------------
    // git exports this to every hook. It used to decide `check-ignore`'s
    // answer, and so whether a gitignored secret was filed into the object
    // store. `--no-index` took the index out of that question, so the
    // variable can no longer move the decision — asserted here in a fixture
    // where git demonstrably honours it.
    let x = tempfile::tempdir().unwrap();
    let x_root = dunce::canonicalize(x.path()).unwrap();
    repo(&x_root);
    write(&x_root.join(".gitignore"), ".env\n");
    git(&x_root, &["add", ".gitignore"]);
    git(&x_root, &["commit", "-qm", "base"]);
    let secret = x_root.join(".env");
    write(&secret, "SECRET=env-canary\n");
    git(&x_root, &["add", "-f", ".env"]);
    let staged = |dir: &Path| {
        let out = Command::new("git")
            .args(["ls-files", "--", ".env"])
            .current_dir(dir)
            .output()
            .unwrap();
        !out.stdout.is_empty()
    };
    assert!(staged(&x_root), "fixture failed: .env must be in the index");

    unsafe { std::env::set_var("GIT_INDEX_FILE", x_root.join("no-such-index")) };
    assert!(
        !staged(&x_root),
        "positive control failed: git ignored GIT_INDEX_FILE, so this arm proves nothing"
    );
    let verdict = UnsavedWorkGuard::new_isolated().assess(
        &secret,
        ".env",
        "SECRET=env-canary\n",
        "SECRET=rotated\n",
        Mode::Rewrite,
    );
    unsafe { std::env::remove_var("GIT_INDEX_FILE") };
    match verdict {
        Verdict::Refuse(m) => assert!(
            m.contains("configured to ignore this file"),
            "the ignore rule moved with the index: {m}"
        ),
        other => panic!("a gitignored secret was accepted for copying: {other:?}"),
    }
}
