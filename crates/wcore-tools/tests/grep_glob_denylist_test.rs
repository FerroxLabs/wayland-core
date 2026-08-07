//! Grep and Glob must honour the same credential deny-list as Read.
//!
//! Read refuses `/etc/shadow` and `/proc/self/environ` via `validate_user_path`.
//! Grep and Glob never called it, so the boundary was enforced on one read tool
//! and walked around with its sibling — and Grep returns matched line CONTENT,
//! not just names, so `Grep {pattern:"root", path:"/etc/shadow"}` handed the
//! hash straight to the model.
//!
//! These drive `execute_with_ctx` — the entry point the engine actually calls —
//! at TOP-LEVEL RealFs posture, which is the posture that was exposed. Under a
//! `SandboxedFs` jail the containment probe (`ctx.vfs.exists`) already refused
//! these paths; unconstrained RealFs is where the deny-list is the only guard.
//!
//! Every refusal test is paired with a negative control that must still SUCCEED,
//! so a guard that refuses everything cannot pass this file.

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::glob::GlobTool;
use wcore_tools::grep::GrepTool;

/// The exact bypass: Grep at a deny-listed path returns the file's lines.
#[cfg(unix)]
#[tokio::test]
async fn grep_ctx_refuses_etc_shadow() {
    let ctx = ToolContext::test_default();
    let result = GrepTool
        .execute_with_ctx(json!({ "pattern": "root", "path": "/etc/shadow" }), &ctx)
        .await;

    assert!(
        result.is_error,
        "Grep must refuse /etc/shadow, got: {}",
        result.content
    );
    assert!(
        result.content.contains("Refused to search"),
        "refusal must name itself as a refusal, got: {}",
        result.content
    );
}

/// The same walk-around for the credential source the /proc fix closed on Read.
/// Grep shells out to `rg`, so `SecretDenyFs` never sees this path — only the
/// deny-list can stop it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn grep_ctx_refuses_proc_self_environ() {
    let ctx = ToolContext::test_default();
    let result = GrepTool
        .execute_with_ctx(
            json!({ "pattern": "API_KEY", "path": "/proc/self/environ" }),
            &ctx,
        )
        .await;

    assert!(
        result.is_error,
        "Grep must refuse /proc/self/environ (it holds this process's own \
         provider credentials), got: {}",
        result.content
    );
}

/// A relative path is graded as the absolute path it DENOTES. Without
/// resolution before the deny-check, this string sails through: it is neither
/// absolute nor literally `/etc/shadow`.
#[cfg(unix)]
#[tokio::test]
async fn grep_ctx_refuses_traversal_that_resolves_onto_the_denylist() {
    let ctx = ToolContext::test_default();
    let result = GrepTool
        .execute_with_ctx(
            json!({ "pattern": "root", "path": "../../../../../../../../etc/shadow" }),
            &ctx,
        )
        .await;

    assert!(
        result.is_error,
        "a traversal resolving onto a denied path must be refused, got: {}",
        result.content
    );
}

/// NEGATIVE CONTROL. An ordinary directory must still be searchable, and the
/// match content must still come back. A guard that refuses everything would
/// pass every test above and fail here.
#[tokio::test]
async fn grep_ctx_still_searches_an_ordinary_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = "WAYLAND_GREP_DENYLIST_NEGATIVE_CONTROL";
    std::fs::write(dir.path().join("notes.txt"), format!("{marker}\n")).expect("write");

    let ctx = ToolContext::test_default();
    let result = GrepTool
        .execute_with_ctx(
            json!({ "pattern": marker, "path": dir.path().to_string_lossy() }),
            &ctx,
        )
        .await;

    assert!(
        !result.is_error,
        "an ordinary directory must remain searchable, got: {}",
        result.content
    );
    assert!(
        result.content.contains(marker),
        "the match content must still be returned, got: {}",
        result.content
    );
}

/// Glob returns names rather than content, but enumerating a credential path is
/// still a disclosure — and leaving one read tool ungated is how the boundary
/// got walked around in the first place.
#[cfg(unix)]
#[tokio::test]
async fn glob_refuses_a_denied_search_root() {
    let ctx = ToolContext::test_default();
    let result = GlobTool
        .execute_with_ctx(json!({ "pattern": "*", "path": "/etc/sudoers.d" }), &ctx)
        .await;

    assert!(
        result.is_error,
        "Glob must refuse a deny-listed search root, got: {}",
        result.content
    );
}

/// The legacy `execute()` entry accepts an absolute PATTERN, which bypasses the
/// search root entirely. `execute_with_ctx` already refuses anchored patterns,
/// so this pins the direct entry.
#[cfg(unix)]
#[tokio::test]
async fn glob_legacy_execute_refuses_an_absolute_denied_pattern() {
    let result = GlobTool.execute(json!({ "pattern": "/etc/shadow*" })).await;

    assert!(
        result.is_error,
        "the legacy Glob entry must refuse an absolute deny-listed pattern, got: {}",
        result.content
    );
}

/// NEGATIVE CONTROL for Glob.
#[tokio::test]
async fn glob_still_matches_in_an_ordinary_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("hit.rs"), "fn main() {}\n").expect("write");

    let ctx = ToolContext::test_default();
    let result = GlobTool
        .execute_with_ctx(
            json!({ "pattern": "*.rs", "path": dir.path().to_string_lossy() }),
            &ctx,
        )
        .await;

    assert!(
        !result.is_error,
        "an ordinary directory must remain globbable, got: {}",
        result.content
    );
    assert!(
        result.content.contains("hit.rs"),
        "the match must still be returned, got: {}",
        result.content
    );
}

// ---------------------------------------------------------------------------
// The fourth door on the same room. Bash already refuses `env` / `printenv`,
// but `/proc/self/environ` IS the env dump reached by a path the command-name
// rules never inspect. Read, Grep and Glob all deny it now; Bash returns full
// stdout to the model, so leaving it open would keep the boundary walkable.
// ---------------------------------------------------------------------------

#[test]
fn bash_denylist_refuses_proc_self_environ() {
    for cmd in [
        "cat /proc/self/environ",
        "tr '\\0' '\\n' < /proc/self/environ",
        "strings /proc/self/environ",
        "base64 /proc/thread-self/environ",
        "cat /proc/1234/environ",
    ] {
        let reason = wcore_tools::bash::check_denylist(cmd);
        assert!(
            reason.is_some(),
            "bash denylist must refuse {cmd:?} — it is this process's own environment"
        );
    }
}

/// NEGATIVE CONTROL. Denying the environ subtree must not deny `/proc` at
/// large; reading `cpuinfo`/`meminfo` is ordinary and must still work.
#[test]
fn bash_denylist_still_allows_ordinary_proc_reads() {
    for cmd in [
        "cat /proc/cpuinfo",
        "cat /proc/meminfo",
        "cat /proc/self/status",
        "ls /proc/self",
    ] {
        let reason = wcore_tools::bash::check_denylist(cmd);
        assert!(
            reason.is_none(),
            "bash denylist must still allow {cmd:?}, refused with {reason:?}"
        );
    }
}
