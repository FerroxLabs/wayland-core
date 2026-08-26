//! wayland#1103 (Linux, live bubblewrap) — **a typo is not a sandbox denial.**
//!
//! When a sandboxed command fails, Core appends an advisory that opens by
//! ruling out the two causes it is most often mistaken for: "not a broken
//! machine and not a missing tool". It was appending that to
//! `No such file or directory`, and then offering `--trust-workspace` or
//! `--dangerously-skip-permissions-and-sandbox` as the remedy. A mistyped path
//! therefore argued the reader into turning their sandbox off.
//!
//! The two cases cannot be told apart from inside the child, which is why this
//! test is live rather than a string fixture: under bubblewrap a path outside
//! every granted root is simply absent from the child's mount namespace, so a
//! GENUINE denial and a genuine typo produce the identical `No such file or
//! directory`. Only the parent — unsandboxed — can ask the filesystem which
//! one happened, and this file asserts that it does.

#![cfg(target_os = "linux")]

use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

/// The advisory's own opening line. Present ⇔ the sandbox was blamed.
const BLAME: &str = "out of reach of this command";

#[tokio::test]
async fn a_missing_path_is_not_blamed_on_the_live_sandbox() {
    let Ok(ws) = tempfile::tempdir() else {
        eprintln!("skip: no temp dir");
        return;
    };
    let Ok(root) = std::fs::canonicalize(ws.path()) else {
        eprintln!("skip: could not canonicalize the workspace");
        return;
    };
    std::fs::write(root.join("readme.txt"), b"plain contents\n").expect("seed the workspace");

    // A real directory outside the workspace, holding one real file and one
    // path that has never existed. Both are equally out of the child's reach;
    // only one of them is out of reach BECAUSE of the sandbox.
    let outside = tempfile::tempdir().expect("a second temp dir");
    let outside = std::fs::canonicalize(outside.path()).expect("canonicalize it");
    let present = outside.join("notes-that-exist.md");
    std::fs::write(&present, b"real contents\n").expect("seed the outside file");
    let missing = outside.join("todo-that-never-existed.md");
    assert!(
        std::fs::symlink_metadata(&missing).is_err(),
        "precondition: the missing path must really be missing"
    );

    let ctx =
        ToolContext::test_default().with_workspace(Arc::new(WorkspacePolicy::contained(&root)));

    // PRECONDITION: an ordinary granted read works, so a later silence cannot
    // be explained by a sandbox that refuses everything.
    let granted = BashTool
        .execute_with_ctx(json!({"command": "cat readme.txt"}), &ctx)
        .await;
    if granted.is_error || !granted.content.contains("plain contents") {
        eprintln!("skip: the live sandbox rejected a legitimate read: {granted:?}");
        return;
    }

    // POSITIVE CONTROL, and the anti-vacuity guard for everything below: a file
    // that really is there and really is outside every granted root is a real
    // denial, and must still be named. If this does not fire there is no live
    // sandbox here and the test grades nothing.
    let denied = BashTool
        .execute_with_ctx(
            json!({"command": format!("cat {}", present.display())}),
            &ctx,
        )
        .await;
    if !denied.content.contains(BLAME) {
        eprintln!(
            "skip: no scoped sandbox on this host — a real ungranted read was \
             not attributed: {}",
            denied.content
        );
        return;
    }
    assert!(
        denied.content.contains("notes-that-exist.md"),
        "the genuinely denied path must be named: {}",
        denied.content
    );
    assert!(
        !denied.content.contains("real contents"),
        "CONTAINMENT REGRESSION: the outside file's bytes leaked: {}",
        denied.content
    );

    // THE DEFECT. Same command shape, same scope, same child-visible error —
    // and nothing was ever at this path.
    let typo = BashTool
        .execute_with_ctx(
            json!({"command": format!("cat {}", missing.display())}),
            &ctx,
        )
        .await;
    assert!(
        typo.is_error,
        "precondition: the command must have failed, or the advisory could not \
         have fired either way: {}",
        typo.content
    );
    assert!(
        !typo.content.contains(BLAME),
        "the shell already gave the correct answer; the sandbox must not be \
         blamed for a file that does not exist: {}",
        typo.content
    );
    assert!(
        !typo
            .content
            .contains("--dangerously-skip-permissions-and-sandbox"),
        "a typo in a path must never steer the reader toward disabling the \
         sandbox: {}",
        typo.content
    );
}
