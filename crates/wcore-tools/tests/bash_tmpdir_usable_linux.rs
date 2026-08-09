//! SEC-06 / SEC-10 rework (Linux) — making ungranted writes fail honestly must
//! not take the temp directory away from the two profiles a real user gets.
//!
//! `bash_write_visibility_linux.rs` proves the honesty half: an ungranted write
//! must not be reported as `Exit code: 0`. It measures only
//! `delegated_mutation`, which is the ONLY profile that redirects
//! `TMPDIR`/`TMP`/`TEMP` into its grant. `trusted_local` and `contained` leave
//! those vars pointing at the host `/tmp`, which `--remount-ro /tmp` makes
//! read-only — so under those two profiles `mktemp` returns "Read-only file
//! system" and `sort` (which spills to `$TMPDIR`) prints NOTHING while the tool
//! result still says success. That is the same disease at a new address: a
//! silent wrong answer.
//!
//! These tests run at the **BashTool surface** on purpose. The regression they
//! guard was introduced under a manifest-level test that could not see it.

#![cfg(target_os = "linux")]

use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;
use wcore_types::tool::ToolResult;

#[derive(Clone, Copy, Debug)]
enum Profile {
    TrustedLocal,
    Contained,
}

/// A real session context for one of the two user-facing profiles, rooted at a
/// fresh workspace. `None` means this host has no hard sandbox and the row
/// cannot be measured here.
fn session(profile: Profile) -> Option<(ToolContext, tempfile::TempDir)> {
    if !wcore_tools::bash::platform_enforces_read_deny() {
        return None;
    }
    let dir = tempfile::tempdir().ok()?;
    let root = std::fs::canonicalize(dir.path()).ok()?;
    let policy = match profile {
        Profile::TrustedLocal => WorkspacePolicy::trusted_local(&root),
        Profile::Contained => WorkspacePolicy::contained(&root),
    };
    Some((
        ToolContext::test_default().with_workspace(Arc::new(policy)),
        dir,
    ))
}

async fn bash(ctx: &ToolContext, command: &str) -> ToolResult {
    BashTool
        .execute_with_ctx(json!({ "command": command }), ctx)
        .await
}

/// POSITIVE CONTROL, run at the top of every test below.
///
/// A sandbox that fails to build its namespace kills every child with an empty
/// stdout, which would make "the ungranted write failed" pass for the wrong
/// reason. Prove a legitimate in-workspace write works first; if it does not,
/// nothing measured afterwards means anything.
async fn workspace_is_writable(ctx: &ToolContext) -> Result<(), String> {
    let r = bash(ctx, "printf control > control.txt && cat control.txt").await;
    if r.is_error || !r.content.contains("control") {
        return Err(format!(
            "positive control FAILED — the sandbox rejected a legitimate \
             in-workspace write, so every other assertion here is vacuous. \
             Got: {}",
            r.content
        ));
    }
    Ok(())
}

/// (a) `mktemp` must succeed and hand back a file the child can write.
async fn mktemp_works(profile: Profile) {
    let Some((ctx, _keep)) = session(profile) else {
        eprintln!("skip ({profile:?}): no hard sandbox on this host");
        return;
    };
    workspace_is_writable(&ctx).await.expect("positive control");

    // Print the path too: the assertion below grades the HOST filesystem, so
    // the temp file must be a real file outside the namespace, not a phantom
    // that reads back correctly inside it and evaporates at teardown.
    let r = bash(
        &ctx,
        "f=$(mktemp) && printf 'payload\\n' > \"$f\" && cat \"$f\" && echo \"PATH=$f\"",
    )
    .await;
    eprintln!("MODEL SEES ({profile:?} mktemp):\n{}", r.content);
    assert!(
        !r.is_error && r.content.contains("payload"),
        "{profile:?}: mktemp must yield a writable file. Got: {}",
        r.content
    );

    let path = r
        .content
        .lines()
        .find_map(|l| l.strip_prefix("PATH="))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| panic!("{profile:?}: probe did not report the temp path"));
    let on_host = std::fs::read(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        on_host.as_deref().ok(),
        Some(&b"payload\n"[..]),
        "{profile:?}: the temp file must exist ON THE HOST at {} — a write \
         that only exists inside the namespace is the phantom write this whole \
         change set exists to eliminate",
        path.display()
    );
}

/// (b) The measured silent wrong answer: `sort` spills to `$TMPDIR` and, with a
/// read-only one, prints nothing while the tool reports success.
async fn sort_returns_every_line(profile: Profile) {
    let Some((ctx, _keep)) = session(profile) else {
        eprintln!("skip ({profile:?}): no hard sandbox on this host");
        return;
    };
    workspace_is_writable(&ctx).await.expect("positive control");

    let r = bash(&ctx, "seq 1 200000 | sort -R | wc -l").await;
    eprintln!("MODEL SEES ({profile:?} sort):\n{}", r.content);
    assert!(
        r.content.contains("200000"),
        "{profile:?}: `seq 1 200000 | sort -R | wc -l` must return 200000 \
         lines, not a silently truncated answer. Got: {}",
        r.content
    );
}

/// (c) The honesty win must survive: a write outside every grant still fails
/// visibly, and never reaches the host.
async fn ungranted_write_still_fails_visibly(profile: Profile) {
    let Some((ctx, _keep)) = session(profile) else {
        eprintln!("skip ({profile:?}): no hard sandbox on this host");
        return;
    };
    workspace_is_writable(&ctx).await.expect("positive control");

    let stamp = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is after the epoch")
            .as_nanos()
    );
    // Both boundaries: the namespace root, and — the one this rework moves —
    // the host temp tree. `TMPDIR` now points INSIDE the grant, so an
    // ungranted sibling under the same host `/tmp` must still be refused.
    let escapes = [
        std::path::PathBuf::from(format!("/wayland-ungranted-{stamp}")),
        std::env::temp_dir().join(format!("wayland-ungranted-{stamp}")),
    ];

    for escape in escapes {
        let r = bash(&ctx, &format!("printf escaped > {}", escape.display())).await;
        eprintln!(
            "MODEL SEES ({profile:?} escape {}):\n{}",
            escape.display(),
            r.content
        );

        assert!(
            !escape.exists(),
            "{profile:?}: containment — {} must not exist on the host",
            escape.display()
        );
        assert!(
            r.is_error && !r.content.contains("Exit code: 0"),
            "{profile:?}: honesty — a write to {} landed nowhere and must be \
             reported as a failure. Got: {}",
            escape.display(),
            r.content
        );
    }
}

#[tokio::test]
async fn trusted_local_mktemp_works() {
    mktemp_works(Profile::TrustedLocal).await;
}

#[tokio::test]
async fn contained_mktemp_works() {
    mktemp_works(Profile::Contained).await;
}

#[tokio::test]
async fn trusted_local_sort_returns_every_line() {
    sort_returns_every_line(Profile::TrustedLocal).await;
}

#[tokio::test]
async fn contained_sort_returns_every_line() {
    sort_returns_every_line(Profile::Contained).await;
}

#[tokio::test]
async fn trusted_local_ungranted_write_still_fails_visibly() {
    ungranted_write_still_fails_visibly(Profile::TrustedLocal).await;
}

#[tokio::test]
async fn contained_ungranted_write_still_fails_visibly() {
    ungranted_write_still_fails_visibly(Profile::Contained).await;
}
