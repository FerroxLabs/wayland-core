//! INV-2 — a `git` that exits non-zero for a reason nothing here enumerates.
//!
//! Its own test binary because it mutates `PATH`, which is process-global.
//!
//! Bar 1 is not "the git failures we listed are fail-closed", it is "no git
//! failure is fail-open". The listed ones — `safe.directory`, a corrupt
//! config, a missing binary — are all classified by an exit code this module
//! has actually seen. This arm supplies one it has not: a site wrapper that
//! exits 7 with a message no branch here matches. Round 2's `git_output`
//! mapped exactly that bucket to an authoritative "not a repository", which is
//! the fail-open the whole module exists to close.

#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::unsaved_work::UnsavedWorkGuard;
use wcore_tools::write::WriteTool;

const COMMITTED: &str = "def parse(text):\n    return text.split()\n";
const UNSAVED: &str = "# INV2R4-SHIM-UNSAVED-USER-WORK";

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

#[tokio::test]
async fn a_git_that_fails_for_an_unrecognised_reason_still_refuses() {
    // Build the world while the real git is still reachable.
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
    let before = format!("{COMMITTED}{UNSAVED}\n");
    std::fs::write(&file, &before).unwrap();

    // A site wrapper that declines, in a way no branch in this module names.
    let shim_dir = tempfile::tempdir().unwrap();
    let shim = shim_dir.path().join("git");
    std::fs::write(
        &shim,
        "#!/bin/sh\necho 'fatal: git access is declined by site policy WCORE-7' >&2\nexit 7\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // SAFETY: this binary contains exactly one test, so nothing else is
    // reading or writing the environment concurrently.
    unsafe { std::env::set_var("PATH", shim_dir.path()) };

    // Positive control: the shim is genuinely what `git` now resolves to.
    let control = Command::new("git").arg("--version").output().unwrap();
    assert_eq!(
        control.status.code(),
        Some(7),
        "positive control failed: the real git is still on PATH, so this arm \
         proves nothing"
    );

    let tool = WriteTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()));
    let r = tool
        .execute(json!({"file_path": file.to_str().unwrap(), "content": COMMITTED}))
        .await;

    println!(
        "[MATRIX] git-exits-7-unrecognised => is_error={}\n           {}",
        r.is_error,
        r.content.lines().next().unwrap_or("")
    );

    assert!(
        r.is_error,
        "an unclassifiable git must refuse: {}",
        r.content
    );
    assert!(
        r.content.contains("could not be established"),
        "the refusal must say the baseline is unknown: {}",
        r.content
    );
    assert!(
        r.content.contains("WCORE-7"),
        "the refusal must quote git's own reason so the user can fix it: {}",
        r.content
    );
    assert!(
        !r.content.contains("in no repository"),
        "`.git` is right there on the filesystem: {}",
        r.content
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        before,
        "a refusal must leave the file exactly as it was"
    );
}
