//! INV-2 — the arm of the hostile matrix that needs `git` to be unreachable.
//!
//! Its own test binary on purpose: it mutates `PATH`, which is process-global,
//! so it must not run beside anything that shells out to git.
//!
//! Two outcomes have to be told apart here, and round 2 could tell neither:
//!
//! * inside a repository with no usable `git`, nothing can be established, so
//!   a rewrite that drops the user's line is **refused**;
//! * outside any repository, "nothing is recorded" is true whether or not git
//!   exists — the filesystem says so — and the refusal comes from there being
//!   nowhere to put a recovery copy, not from an unresolved baseline.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::unsaved_work::UnsavedWorkGuard;
use wcore_tools::write::WriteTool;

/// Put `PATH` somewhere with no `git` in it, for the rest of the process.
///
/// SAFETY: this binary contains exactly one test, so there is no other thread
/// reading or writing the environment concurrently.
fn hide_git(empty_dir: &Path) {
    unsafe { std::env::set_var("PATH", empty_dir) };
    assert!(
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err(),
        "positive control: git must genuinely be unreachable, or this arm \
         proves nothing"
    );
}

#[tokio::test]
async fn with_no_git_binary_a_repository_refuses_and_a_bare_directory_still_answers() {
    // Build both worlds while git still works.
    let repo = tempfile::tempdir().unwrap();
    let repo_root = std::fs::canonicalize(repo.path()).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "u@example.com"],
        vec!["config", "user.name", "u"],
    ] {
        let ok = Command::new("git")
            .args(&args)
            .current_dir(&repo_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(ok.success());
    }
    let tracked = repo_root.join("parser.py");
    std::fs::write(&tracked, "def parse():\n    pass\n# unsaved user line\n").unwrap();

    let loose = tempfile::tempdir().unwrap();
    let loose_root = std::fs::canonicalize(loose.path()).unwrap();
    let loose_file = loose_root.join("loose.txt");
    std::fs::write(&loose_file, "the user's only copy\nsecond line\n").unwrap();

    let empty = tempfile::tempdir().unwrap();
    hide_git(empty.path());

    // Inside a repository: `.git` is right there on the filesystem, so this is
    // a repository whose baseline cannot be read — not "no repository".
    let tool = WriteTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()));
    let r = tool
        .execute(json!({"file_path": tracked.to_str().unwrap(),
                        "content": "def parse():\n    pass\n"}))
        .await;
    println!("[MATRIX] no-git-in-repo           => is_error={}\n           {}",
             r.is_error, r.content.lines().next().unwrap_or(""));
    assert!(r.is_error, "got: {}", r.content);
    assert!(r.content.contains("could not be established"), "{}", r.content);
    assert_eq!(
        std::fs::read_to_string(&tracked).unwrap(),
        "def parse():\n    pass\n# unsaved user line\n",
        "a refusal must leave the file exactly as it was"
    );

    // Outside any repository: answerable without git at all.
    let tool = WriteTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()));
    let r = tool
        .execute(json!({"file_path": loose_file.to_str().unwrap(), "content": "replaced\n"}))
        .await;
    println!("[MATRIX] no-git-no-repo           => is_error={}\n           {}",
             r.is_error, r.content.lines().next().unwrap_or(""));
    assert!(r.is_error, "got: {}", r.content);
    assert!(r.content.contains("in no repository"), "{}", r.content);

    // ...and that same no-git, no-repo directory stays fully usable for a
    // write that drops nothing, so fail-closed has not become fail-useless.
    let tool = WriteTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()));
    let kept = "the user's only copy\nsecond line\nand a new one\n";
    let r = tool
        .execute(json!({"file_path": loose_file.to_str().unwrap(), "content": kept}))
        .await;
    println!("[MATRIX] no-git-no-repo-additive  => is_error={}", r.is_error);
    assert!(!r.is_error, "got: {}", r.content);
    assert_eq!(std::fs::read_to_string(&loose_file).unwrap(), kept);
}
