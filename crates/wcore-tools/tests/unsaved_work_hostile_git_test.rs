//! INV-2 — the hostile-environment matrix.
//!
//! Round 2's headline improvement was telling "git ran and said no" apart from
//! "git could not be run". It then wired the error cases into the *authoritative*
//! bucket: `git_output` mapped every non-zero exit to `Ok(None)`, and the caller
//! read `Ok(None)` as "this path is in no work tree". Measured on git 2.43.0,
//! that bucket contains
//!
//! * `safe.directory` dubious ownership — **the default for Docker bind mounts,
//!   CI checkouts and sudo-run agents**,
//! * an unreadable or corrupt `.git/config`,
//! * a bad `GIT_DIR`,
//!
//! all of which exit 128 exactly as a genuinely missing repository does. Each
//! became `Baseline::NoRepo`, which makes every line count unsaved, which makes
//! every rewrite "wholesale", which means the Write is never refused. On such a
//! host round 2 was strictly worse than round 1, and the tool result asserted
//! "None of this file was in any commit" about a file that plainly is committed.
//!
//! So this file is the acceptance test for B1: for every environment below, a
//! rewrite that would drop the user's unsaved line must end in **a refusal** or
//! in **a copy that has been read back**. Never in a silent allow.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::unsaved_work::UnsavedWorkGuard;
use wcore_tools::write::WriteTool;

const COMMITTED: &str = "def parse(text):\n    return text.split()\n";
const UNSAVED: &str = "# JOBCORPUS-UNSAVED-USER-WORK do not touch";

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git must be installed to run these tests");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// What actually happened to the user's line, judged from the tool result and
/// from the world rather than from the sentence the tool chose to print.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// Nothing was written.
    Refused,
    /// Written, and the prior bytes came back byte-for-byte from the recovery
    /// command the result names.
    AllowedCopyVerified,
    /// Written, and the result says plainly that no copy exists.
    AllowedNoCopyClaimed,
    /// Written, and the result claims a copy that could not be recovered.
    /// This is round 2's B3 shape.
    AllowedCopyBroken,
    /// Written with nothing said at all. The silent-loss shape this whole
    /// module exists to prevent.
    AllowedSilently,
}

/// Drive a real `WriteTool` — the tool layer, not the guard in isolation — and
/// classify what it did.
async fn probe(repo_for_recovery: &Path, file: &Path, new_content: &str) -> (Outcome, String) {
    let before = std::fs::read_to_string(file).unwrap();
    let tool = WriteTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()));
    let r = tool
        .execute(json!({"file_path": file.to_str().unwrap(), "content": new_content}))
        .await;

    if r.is_error {
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            before,
            "a refusal must leave the file exactly as it was"
        );
        return (Outcome::Refused, r.content);
    }

    let outcome = if let Some(oid) = oid_in(&r.content) {
        let out = Command::new("git")
            .args(["cat-file", "blob", &oid])
            .current_dir(repo_for_recovery)
            .output()
            .unwrap();
        if out.status.success() && out.stdout == before.as_bytes() {
            Outcome::AllowedCopyVerified
        } else {
            Outcome::AllowedCopyBroken
        }
    } else if r.content.contains("not recoverable") || r.content.contains("No recovery copy") {
        Outcome::AllowedNoCopyClaimed
    } else {
        Outcome::AllowedSilently
    };
    (outcome, r.content)
}

fn oid_in(s: &str) -> Option<String> {
    let marker = "cat-file blob ";
    let start = s.find(marker)? + marker.len();
    s[start..].split_whitespace().next().map(str::to_owned)
}

/// A repository with the measured harm shape in it: `parser.py` committed,
/// then one unsaved line appended.
fn corpus_repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "u@example.com"]);
    git(&root, &["config", "user.name", "u"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    let file = root.join("parser.py");
    std::fs::write(&file, COMMITTED).unwrap();
    git(&root, &["add", "parser.py"]);
    git(&root, &["commit", "-qm", "initial"]);
    std::fs::write(&file, format!("{COMMITTED}{UNSAVED}\n")).unwrap();
    (dir, file)
}

/// The rewrite that drops the unsaved line: the exact round-1 harm shape.
const DROPS_IT: &str = "def parse(text):\n    return text.split()\n    # rewritten\n";

fn report(arm: &str, outcome: &Outcome, detail: &str) {
    let first = detail.lines().next().unwrap_or("").trim();
    let shown: String = first.chars().take(150).collect();
    println!("[MATRIX] {arm:<28} => {outcome:?}\n           {shown}");
}

// --- arm 1: the baseline case that must never regress -------------------

#[tokio::test]
async fn healthy_repository_refuses_the_measured_harm_shape() {
    let (dir, file) = corpus_repo();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let (outcome, detail) = probe(&root, &file, DROPS_IT).await;
    report("healthy", &outcome, &detail);
    assert_eq!(outcome, Outcome::Refused);
    assert!(detail.contains(UNSAVED), "{detail}");
}

// --- arm 2: dubious ownership. B1's most important case -----------------

#[cfg(unix)]
#[tokio::test]
async fn dubious_ownership_refuses_rather_than_calling_it_an_empty_baseline() {
    // git's `safe.directory` rejection. Round 2 turned this into
    // SNAPSHOT+ALLOW with the claim "None of this file was in any commit".
    let uid = Command::new("id").arg("-u").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&uid.stdout).trim(),
        "0",
        "this arm must run as root so the chown below actually changes \
         ownership — skipping it instead would make the matrix permanently \
         green, which is worse than not having it"
    );
    let (dir, file) = corpus_repo();
    let root = dunce::canonicalize(dir.path()).unwrap();
    chown_tree(&root, 65534);

    let (outcome, detail) = probe(&root, &file, DROPS_IT).await;
    report("dubious-ownership", &outcome, &detail);

    // Restore before any assertion can unwind past the cleanup.
    chown_tree(&root, 0);

    assert_eq!(outcome, Outcome::Refused);
    assert!(
        detail.contains("could not be established"),
        "the refusal must say the baseline is unknown, not invent one: {detail}"
    );
    assert!(
        !detail.contains("None of this file was in any commit"),
        "round 2's false claim must not reappear: {detail}"
    );
}

#[cfg(unix)]
fn chown_tree(root: &Path, uid: u32) {
    let status = Command::new("chown")
        .args(["-R", &format!("{uid}:{uid}"), root.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success(), "chown failed");
}

// --- arm 3: an unreadable config ----------------------------------------

#[tokio::test]
async fn a_corrupt_git_config_refuses() {
    let (dir, file) = corpus_repo();
    let root = dunce::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join(".git/config"), "[[[not a config\n").unwrap();

    let (outcome, detail) = probe(&root, &file, DROPS_IT).await;
    report("corrupt-config", &outcome, &detail);
    assert_eq!(outcome, Outcome::Refused);
    assert!(detail.contains("could not be established"), "{detail}");
}

// --- arm 4: a corrupt index is measured, not assumed --------------------

#[tokio::test]
async fn a_corrupt_index_still_resolves_the_real_baseline() {
    // Measured on git 2.43.0: neither `rev-parse` nor `ls-tree` reads the
    // index, so this is a non-event and the guard must behave exactly as it
    // does on a healthy repository — citing the unsaved line, not hiding
    // behind "git did not answer".
    let (dir, file) = corpus_repo();
    let root = dunce::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join(".git/index"), "JUNKJUNKJUNKJUNK").unwrap();

    let (outcome, detail) = probe(&root, &file, DROPS_IT).await;
    report("corrupt-index", &outcome, &detail);
    assert_eq!(outcome, Outcome::Refused);
    assert!(detail.contains(UNSAVED), "{detail}");
    assert!(!detail.contains("git did not answer"), "{detail}");
}

// --- arm 5: no repository at all ----------------------------------------

#[tokio::test]
async fn no_repository_at_all_refuses_because_no_copy_is_possible() {
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let file = root.join("loose.txt");
    std::fs::write(&file, "the user's only copy\nsecond line\n").unwrap();

    let (outcome, detail) = probe(&root, &file, "replaced\n").await;
    report("no-repo", &outcome, &detail);
    assert_eq!(outcome, Outcome::Refused);
    assert!(detail.contains("in no repository"), "{detail}");
}

// --- arm 6: a repository whose HEAD has no commits ----------------------

#[tokio::test]
async fn an_unborn_head_is_a_real_answer_and_allows_a_verified_copy() {
    // `rev-parse --verify --quiet HEAD` exits 1 here and 128 when git is
    // failing, which is the whole reason this case can be told apart from a
    // broken repository at all.
    let dir = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-q"]);
    let file = root.join("draft.md");
    std::fs::write(&file, "first thoughts\nsecond thoughts\n").unwrap();

    let (outcome, detail) = probe(&root, &file, "rewritten\n").await;
    report("unborn-head", &outcome, &detail);
    assert_eq!(outcome, Outcome::AllowedCopyVerified);
}

// --- arm 7: untracked file in a healthy repo ----------------------------

#[tokio::test]
async fn an_untracked_file_in_a_healthy_repo_allows_a_verified_copy() {
    let (dir, _) = corpus_repo();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let notes = root.join("notes.md");
    std::fs::write(&notes, "# Deploy notes\nstep one\nstep two\n").unwrap();

    let (outcome, detail) = probe(&root, &notes, "# Runbook\n1. make deploy\n").await;
    report("untracked-in-repo", &outcome, &detail);
    assert_eq!(outcome, Outcome::AllowedCopyVerified);
}

// --- arm 8: the enumeration ---------------------------------------------

/// Bar 1. The three arms above name the git failures round 2 was measured on.
/// This one exists because "the failures we thought of" is not the same set as
/// "the failures", and a guard that is fail-closed only on an enumerated list
/// is fail-open on everything else. Every way of breaking a repository that
/// still leaves `.git` on the filesystem lands here, and the assertion is the
/// property rather than the message: the file is not touched, and the tool
/// never says "no repository" about a repository that is plainly there.
#[tokio::test]
async fn every_way_git_can_refuse_to_answer_ends_in_a_refusal() {
    /// One way of breaking a repository while leaving `.git` on disk.
    struct Break {
        name: &'static str,
        apply: fn(&Path),
    }
    let b = |name, apply| Break { name, apply };

    let breaks = vec![
        b("dangling-worktree-gitdir", |root| {
            // An abandoned linked worktree: `.git` is a file pointing at a
            // gitdir that no longer exists. Extremely ordinary — it is what a
            // worktree becomes when its main repository is deleted or moved.
            std::fs::remove_dir_all(root.join(".git")).unwrap();
            std::fs::write(
                root.join(".git"),
                "gitdir: /nonexistent/main/.git/worktrees/gone\n",
            )
            .unwrap();
        }),
        b("garbage-head", |root| {
            std::fs::write(root.join(".git/HEAD"), "not a ref at all\n").unwrap();
        }),
        b("head-points-at-a-missing-object", |root| {
            std::fs::write(
                root.join(".git/HEAD"),
                "0123456789abcdef0123456789abcdef01234567\n",
            )
            .unwrap();
        }),
        b("no-object-database", |root| {
            std::fs::remove_dir_all(root.join(".git/objects")).unwrap();
        }),
        b("unreadable-global-config", |root| {
            // The shape a broken `$HOME` gives every command in the process.
            std::fs::write(root.join("bad-global"), "[[[not a config\n").unwrap();
            // SAFETY: set for the duration of one probe in a single-threaded
            // async test; every arm here runs on the same thread.
            unsafe { std::env::set_var("GIT_CONFIG_GLOBAL", root.join("bad-global")) };
        }),
    ];

    for Break { name, apply } in breaks {
        let (dir, file) = corpus_repo();
        let root = dunce::canonicalize(dir.path()).unwrap();
        apply(&root);

        let (outcome, detail) = probe(&root, &file, DROPS_IT).await;
        report(name, &outcome, &detail);
        unsafe { std::env::remove_var("GIT_CONFIG_GLOBAL") };

        assert_eq!(
            outcome,
            Outcome::Refused,
            "[{name}] a broken repository must refuse, not allow: {detail}"
        );
        assert!(
            !detail.contains("in no repository"),
            "[{name}] `.git` is right there on the filesystem: {detail}"
        );
        assert!(
            !detail.contains("None of this file was in any commit"),
            "[{name}] round 2's false claim is back: {detail}"
        );
    }

    // The control for the whole table: the same probe on an unbroken
    // repository must reach a different verdict for a different reason, or
    // every row above is passing because the harness always refuses.
    let (dir, _) = corpus_repo();
    let root = dunce::canonicalize(dir.path()).unwrap();
    let notes = root.join("notes.md");
    std::fs::write(&notes, "# Deploy notes\nstep one\n").unwrap();
    let (outcome, detail) = probe(&root, &notes, "# Runbook\n").await;
    report("control-healthy-allows", &outcome, &detail);
    assert_eq!(
        outcome,
        Outcome::AllowedCopyVerified,
        "control failed: the harness refuses everything, so the table proves \
         nothing: {detail}"
    );
}

// --- arm 9: armD, end to end through the real tool -----------------------

/// The shape the round-3 adversary measured **live**: `$HOME` is a dotfiles
/// repository — an extremely common setup — and the private file is
/// `~/work/env.local`, holding a Stripe key and a database password. Round 3
/// took zero refusals there and filed both secrets into the dotfiles
/// repository's object store, where they survived `git gc`.
///
/// Driven through `WriteTool::execute`, not through `assess`, because that is
/// the surface that actually ran in the live arm — and the verdict is taken
/// from the object database rather than from the sentence the tool printed.
#[tokio::test]
async fn the_armd_dotfiles_shape_refuses_and_files_nothing() {
    const STRIPE: &str = "STRIPE_SECRET_KEY=sk_live_ARMD_CANARY_0001";
    const DB: &str = "DATABASE_URL=postgres://armd:CANARYPW0002@db.internal/prod";

    let home = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(home.path()).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "u@example.com"]);
    git(&root, &["config", "user.name", "u"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    // A dotfiles repository tracks dotfiles, and nothing under ~/work.
    std::fs::write(root.join(".bashrc"), "export EDITOR=vi\n").unwrap();
    std::fs::create_dir_all(root.join(".config/nvim")).unwrap();
    std::fs::write(root.join(".config/nvim/init.lua"), "vim.o.number = true\n").unwrap();
    std::fs::write(root.join(".gitignore"), "*.local\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "dotfiles"]);

    // Two private files, each unreachable for a different reason: one in a
    // directory the repository records nothing under, one the repository is
    // configured to ignore inside a directory it does record.
    let cases = [
        ("work/env.local", format!("{STRIPE}\n{DB}\nDEBUG=0\n")),
        (".config/secrets.local", format!("{STRIPE}\nTOKEN=keep\n")),
    ];

    for (rel, body) in &cases {
        let file = root.join(rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, body).unwrap();

        let (outcome, detail) = probe(&root, &file, "DEBUG=1\n").await;
        report(&format!("armD:{rel}"), &outcome, &detail);
        assert_eq!(
            outcome,
            Outcome::Refused,
            "[{rel}] the live round-3 shape must not proceed: {detail}"
        );
        assert_eq!(
            &std::fs::read_to_string(&file).unwrap(),
            body,
            "[{rel}] a refusal must leave the file exactly as it was"
        );
    }

    // The verdict that matters: nothing of the user's went into the dotfiles
    // repository, judged by walking the object database rather than by
    // trusting the tool's own account of itself.
    let dump = Command::new("git")
        .args(["cat-file", "--batch-all-objects", "--batch"])
        .current_dir(&root)
        .output()
        .unwrap();
    let objects = String::from_utf8_lossy(&dump.stdout);
    for needle in [STRIPE, DB] {
        assert!(
            !objects.contains(needle),
            "a secret was filed into the dotfiles repository's object store"
        );
    }
    // Positive control on that walk: it does find something that IS in there,
    // so "not found" is a real answer and not an empty dump.
    assert!(
        objects.contains("export EDITOR=vi"),
        "control failed: the object walk found nothing at all, so the two \
         assertions above prove nothing"
    );

    // And the counter-case, so the rule is not simply "refuse everything in a
    // dotfiles repository": a file in a directory the repository does record,
    // not ignored, is still copied as before.
    let tracked_dir = root.join(".config/nvim/scratch.lua");
    std::fs::write(&tracked_dir, "-- notes\nlocal x = 1\n").unwrap();
    let (outcome, detail) = probe(&root, &tracked_dir, "-- rewritten\n").await;
    report("armD:control-recorded-dir", &outcome, &detail);
    assert_eq!(
        outcome,
        Outcome::AllowedCopyVerified,
        "control failed: the rule has become a blanket refusal: {detail}"
    );
}
