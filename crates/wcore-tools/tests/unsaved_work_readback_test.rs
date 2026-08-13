//! INV-2 — B3: the read-back that turns a claimed copy into a verified one.
//!
//! Round 4's property mutation run found this **unverified**. Blanking the
//! byte-for-byte compare (`if back.stdout != bytes.as_bytes()` -> `if false`)
//! and blanking the read-back's own error check both left the entire suite
//! green: every test asserted that a *healthy* git returns the right bytes,
//! which it always does, so nothing ever exercised the compare. B3 is round
//! 2's headline break, and it was one edit from being silently re-opened.
//!
//! A real git cannot be made to hand back the wrong bytes, so this arm puts a
//! wrapper on `PATH` that writes objects perfectly and misbehaves only on
//! `cat-file blob` — the read-back's own command. Its own binary because
//! `PATH` is process-global.

#![cfg(unix)]

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
        .expect("git must be installed to run this test");
    assert!(status.success(), "git {args:?} failed");
}

/// A `git` that delegates everything except `cat-file blob`.
fn shim(dir: &Path, real_path: &str, misbehaviour: &str) -> PathBuf {
    // The real PATH is restored on the first line, not just before the `exec`:
    // the caller has replaced PATH with this directory alone, so without it
    // the misbehaviour body cannot even reach `cat`. (Measured: it silently
    // printed nothing and the control caught it.)
    let script = format!(
        "#!/bin/sh\n\
         PATH='{real_path}'\n\
         for a in \"$@\"; do\n\
         \x20 if [ \"$a\" = \"cat-file\" ]; then\n\
         {misbehaviour}\n\
         \x20 fi\n\
         done\n\
         exec git \"$@\"\n"
    );
    let path = dir.join("git");
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn hash_object(root: &Path, bytes: &str) -> String {
    use std::io::Write as _;
    let mut child = Command::new("git")
        .args(["hash-object", "-w", "--stdin"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
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

fn cat_file(root: &Path, oid: &str) -> (Option<i32>, String) {
    let out = Command::new("git")
        .args(["cat-file", "blob", oid])
        .current_dir(root)
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

const PRIOR: &str = "the user's only draft\nsecond line\n";

/// A healthy repository with an untracked file holding unsaved work. Untracked
/// on purpose: `recorded_blob` then answers from `ls-tree` alone, so the only
/// `cat-file blob` in the whole assessment is the read-back itself.
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "u@example.com"]);
    git(&root, &["config", "user.name", "u"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("keep.txt"), "keep\n").unwrap();
    git(&root, &["add", "keep.txt"]);
    git(&root, &["commit", "-qm", "base"]);
    let file = root.join("draft.md");
    std::fs::write(&file, PRIOR).unwrap();
    (dir, root, file)
}

#[test]
fn a_copy_that_cannot_be_read_back_is_refused_not_claimed() {
    let real_path = std::env::var("PATH").unwrap();

    // ---- arm 1: the read-back command fails outright ---------------------
    let (_d1, root1, file1) = fixture();
    let refusing = tempfile::tempdir().unwrap();
    shim(
        refusing.path(),
        &real_path,
        "    echo 'fatal: shimmed cat-file declines' >&2; exit 1",
    );
    // SAFETY: this binary holds one test, so no other thread touches the env.
    unsafe { std::env::set_var("PATH", refusing.path()) };

    // Positive control: writing still works, reading back does not.
    let control = hash_object(&root1, "control bytes\n");
    assert_eq!(
        cat_file(&root1, &control).0,
        Some(1),
        "control failed: the shim is not intercepting cat-file, so this arm \
         proves nothing"
    );

    let verdict = UnsavedWorkGuard::new_isolated().assess(
        &file1,
        "draft.md",
        PRIOR,
        "second line\n",
        Mode::Rewrite,
    );
    unsafe { std::env::set_var("PATH", &real_path) };
    match verdict {
        Verdict::Refuse(m) => assert!(
            m.contains("could not be read back") || m.contains("copy"),
            "the refusal must say the copy failed: {m}"
        ),
        other => panic!("a copy that cannot be read back must refuse, got {other:?}"),
    }

    // ---- arm 1b: it returns the right bytes and still exits non-zero -----
    //
    // Arm 1 leaves stdout empty, so the byte-for-byte compare catches it even
    // with the exit-code check blanked — measured: mutating `if !back.ok()`
    // alone survived arm 1. This arm is the one that needs the exit code:
    // correct bytes, failing command.
    let (_d1b, root1b, file1b) = fixture();
    let half_failing = tempfile::tempdir().unwrap();
    let payload = half_failing.path().join("payload");
    std::fs::write(&payload, PRIOR).unwrap();
    shim(
        half_failing.path(),
        &real_path,
        &format!("    cat '{}'; exit 1", payload.display()),
    );
    unsafe { std::env::set_var("PATH", half_failing.path()) };

    let control = hash_object(&root1b, "control bytes\n");
    let (code, body) = cat_file(&root1b, &control);
    assert_eq!(
        (code, body.as_str()),
        (Some(1), PRIOR),
        "control failed: the shim must return the right bytes AND fail, or \
         this arm is arm 1 again"
    );

    let verdict = UnsavedWorkGuard::new_isolated().assess(
        &file1b,
        "draft.md",
        PRIOR,
        "second line\n",
        Mode::Rewrite,
    );
    unsafe { std::env::set_var("PATH", &real_path) };
    match verdict {
        Verdict::Refuse(m) => assert!(
            m.contains("could not be read back") || m.contains("copy"),
            "a read-back that exited non-zero must refuse whatever it printed: {m}"
        ),
        other => panic!(
            "a read-back command that failed must not count as a verified \
             copy, got {other:?}"
        ),
    }

    // ---- arm 2: the read-back succeeds and returns other bytes -----------
    let (_d2, root2, file2) = fixture();
    let lying = tempfile::tempdir().unwrap();
    shim(lying.path(), &real_path, "    printf 'tampered\\n'; exit 0");
    unsafe { std::env::set_var("PATH", lying.path()) };

    let control = hash_object(&root2, "control bytes\n");
    let (code, body) = cat_file(&root2, &control);
    assert_eq!(code, Some(0));
    assert_eq!(
        body, "tampered\n",
        "control failed: the shim is not substituting the bytes, so this arm \
         proves nothing"
    );

    let verdict = UnsavedWorkGuard::new_isolated().assess(
        &file2,
        "draft.md",
        PRIOR,
        "second line\n",
        Mode::Rewrite,
    );
    unsafe { std::env::set_var("PATH", &real_path) };
    match verdict {
        Verdict::Refuse(m) => assert!(
            m.contains("read back as"),
            "the refusal must name the byte-for-byte mismatch: {m}"
        ),
        other => panic!("a read-back that returned different bytes must refuse, got {other:?}"),
    }

    // ---- control: with the real git back, the same assessment proceeds ---
    let (_d3, root3, file3) = fixture();
    let verdict = UnsavedWorkGuard::new_isolated().assess(
        &file3,
        "draft.md",
        PRIOR,
        "second line\n",
        Mode::Rewrite,
    );
    match verdict {
        Verdict::ProceedWithNote(note) => {
            let marker = "cat-file blob ";
            let start = note.find(marker).expect("the note names an object") + marker.len();
            let oid = note[start..].split_whitespace().next().unwrap();
            assert_eq!(cat_file(&root3, oid).1, PRIOR);
        }
        other => panic!(
            "control failed: the guard refuses even a healthy read-back, so \
             the two arms above prove nothing: {other:?}"
        ),
    }
}
