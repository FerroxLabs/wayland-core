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
