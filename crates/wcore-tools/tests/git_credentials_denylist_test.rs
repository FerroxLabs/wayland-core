//! `~/.git-credentials` must be refused by every tool that can read it.
//!
//! #644 part 3 named it as readable. Its whole class was already denied —
//! `.netrc`, `.npmrc`, `.pypirc`, `.docker/config.json` — but this one was
//! denied NOWHERE: not `path_validation` (Read/Grep/Glob), not
//! `bash/policy.rs`, not `file_safety.rs`.
//!
//! It is the most direct of the set. Git's `store` credential helper writes
//! bare `https://user:token@host` lines in cleartext, so a single read returns
//! a usable push credential for every remote the user has authenticated to.
//!
//! These run on EVERY platform. An earlier cut gated the Read and Grep cases
//! `#[cfg(unix)]`, which hid that only the forward-slash suffix had been added
//! to the deny list — `%USERPROFILE%\.git-credentials` stayed readable on
//! Windows and no test could say so.
//!
//! Each refusal is paired with a negative control that must still SUCCEED, so
//! a guard that refuses everything cannot pass this file.

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::check_denylist;
use wcore_tools::context::ToolContext;
use wcore_tools::grep::GrepTool;
use wcore_tools::read::ReadTool;

/// A `.git-credentials` that ACTUALLY EXISTS, with readable content.
///
/// The first version of these tests pointed at `$HOME/.git-credentials`, which
/// does not exist on CI. Read then failed with "no such file", the assertion
/// only checked `is_error`, and the test passed WITH THE DENY-LIST ENTRY
/// DELETED — caught by the mutation run, not by the green suite. The file must
/// exist, or the test proves nothing.
///
/// The deny keys on the PATH suffix (`/.git-credentials`), never on the bytes,
/// so the marker below is deliberately not credential-shaped: the commit
/// ratchet correctly rejects key-shaped fixtures, and a real-looking token
/// would add nothing.
fn planted_git_credentials(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join(".git-credentials");
    std::fs::write(&path, b"GIT_CREDENTIALS_FIXTURE_MARKER\n").expect("write fixture");
    assert!(path.exists(), "fixture must exist or the test is vacuous");
    path
}

/// Read is the tool the deny-list was originally written for.
#[tokio::test]
async fn read_refuses_git_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = planted_git_credentials(dir.path());

    let result = ReadTool::new(None)
        .execute(json!({ "file_path": path.to_str().unwrap() }))
        .await;

    assert!(
        result.is_error,
        "Read must refuse a .git-credentials store — git's `store` helper keeps \
         cleartext https://user:token@host lines there, got: {}",
        result.content
    );
    assert!(
        !result.content.contains("GIT_CREDENTIALS_FIXTURE_MARKER"),
        "the file's contents must never reach the model, got: {}",
        result.content
    );
}

/// Grep returns matched LINE CONTENT, so an ungated search is a direct
/// credential disclosure, not merely an enumeration.
#[tokio::test]
async fn grep_ctx_refuses_git_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = planted_git_credentials(dir.path());

    let ctx = ToolContext::test_default();
    let result = GrepTool
        .execute_with_ctx(
            json!({ "pattern": "MARKER", "path": path.to_str().unwrap() }),
            &ctx,
        )
        .await;

    assert!(
        result.is_error,
        "Grep must refuse the git credential store, got: {}",
        result.content
    );
    assert!(
        !result.content.contains("GIT_CREDENTIALS_FIXTURE_MARKER"),
        "the matched line must never reach the model, got: {}",
        result.content
    );
}

/// Bash returns full stdout to the model, so the reader and encoder dodges
/// both have to be closed — `base64 ~/.git-credentials` is the same leak.
#[test]
fn bash_denylist_refuses_reading_git_credentials() {
    for cmd in [
        "cat ~/.git-credentials",
        "cat /home/user/.git-credentials",
        "head -1 ~/.git-credentials",
        "base64 ~/.git-credentials",
        "xxd ~/.git-credentials",
    ] {
        assert!(
            check_denylist(cmd).is_some(),
            "bash denylist must refuse {cmd:?}"
        );
    }
}

/// NEGATIVE CONTROL for Bash. Denying the credential store must not deny
/// ordinary git usage, including commands whose text merely contains
/// "credential".
#[test]
fn bash_denylist_still_allows_ordinary_git() {
    for cmd in [
        "git status",
        "git log --oneline -5",
        "git config --global credential.helper",
        "cat README.md",
        "cat ~/.gitconfig",
    ] {
        let reason = check_denylist(cmd);
        assert!(
            reason.is_none(),
            "bash denylist must still allow {cmd:?}, refused with {reason:?}"
        );
    }
}

/// NEGATIVE CONTROL for the read path: an ordinary file stays readable, and a
/// near-miss name (`.gitconfig`, which is NOT a credential store) is not
/// caught by an over-broad prefix.
#[tokio::test]
async fn read_still_allows_ordinary_files_and_gitconfig() {
    let dir = tempfile::tempdir().expect("tempdir");

    let ordinary = dir.path().join("notes.txt");
    std::fs::write(&ordinary, b"hello\n").expect("write");

    let gitconfig = dir.path().join(".gitconfig");
    std::fs::write(&gitconfig, b"[user]\n\tname = Test\n").expect("write");

    for path in [ordinary, gitconfig] {
        let result = ReadTool::new(None)
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;
        assert!(
            !result.is_error,
            "{} must stay readable, got: {}",
            path.display(),
            result.content
        );
    }
}
