//! SEC-06 / SEC-10 (Linux) — what the MODEL is told when a sandboxed write
//! lands nowhere.
//!
//! `bash_sandbox_routing_test.rs` already proves the containment half of this
//! row: a `printf > $TMPDIR-outside-the-grant` never reaches the host. It
//! discards the `ToolResult` while doing so (`let _ = BashTool…`), and that is
//! exactly how the honesty half stayed broken for so long — under bubblewrap
//! the same command returned `Exit code: 0`, read back correctly inside the
//! namespace, and evaporated at teardown. Containment was right and the report
//! was a lie.
//!
//! These tests grade the rendered tool result — the only thing the model ever
//! sees — plus the host filesystem from outside the sandbox.

#![cfg(target_os = "linux")]

use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

/// A real workspace grant: a writable checkout plus its private scratch.
fn granted_ctx() -> Option<(ToolContext, std::path::PathBuf, tempfile::TempDir)> {
    let transaction = tempfile::tempdir().ok()?;
    let checkout = transaction.path().join("checkout");
    let scratch = transaction.path().join("scratch");
    std::fs::create_dir(&checkout).ok()?;
    std::fs::create_dir(&scratch).ok()?;
    std::fs::create_dir(scratch.join("tmp")).ok()?;
    std::fs::create_dir(scratch.join("cache")).ok()?;
    let checkout = std::fs::canonicalize(checkout).ok()?;
    let scratch = std::fs::canonicalize(scratch).ok()?;
    let policy = Arc::new(
        WorkspacePolicy::delegated_mutation(&checkout, &scratch, Vec::<std::path::PathBuf>::new())
            .ok()?,
    );
    Some((
        ToolContext::test_default().with_workspace(policy),
        checkout,
        transaction,
    ))
}

#[tokio::test]
async fn a_write_that_lands_nowhere_is_reported_as_a_failure() {
    if !wcore_tools::bash::platform_enforces_read_deny() {
        eprintln!("skip: no hard read-deny sandbox on this host");
        return;
    }
    let Some((ctx, checkout, _keep)) = granted_ctx() else {
        eprintln!("skip: could not build a delegated workspace policy");
        return;
    };

    // POSITIVE CONTROL first: the granted checkout is writable and says so.
    let granted = BashTool
        .execute_with_ctx(json!({"command": "printf ok > granted.txt"}), &ctx)
        .await;
    if granted.is_error {
        eprintln!("skip: the live sandbox rejected a legitimate write: {granted:?}");
        return;
    }
    assert!(
        checkout.join("granted.txt").is_file(),
        "the granted write must land on the host"
    );

    // The escape: a path outside every grant.
    let escape = std::env::temp_dir().join(format!(
        "wayland-write-honesty-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is after the epoch")
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&escape);

    let result = BashTool
        .execute_with_ctx(
            json!({"command": format!("printf escaped > {}", escape.display())}),
            &ctx,
        )
        .await;

    // Visible under `--nocapture` / on failure: this is the literal text the
    // model is handed for a write that landed nowhere.
    eprintln!("MODEL SEES:\n{}", result.content);

    assert!(
        !escape.exists(),
        "containment: {} must not exist on the host",
        escape.display()
    );
    assert!(
        result.is_error,
        "honesty: the write landed nowhere, so the model must be told it \
         FAILED. Got: {}",
        result.content
    );
    assert!(
        !result.content.contains("Exit code: 0"),
        "honesty: a lost write must not be rendered as exit 0. Got: {}",
        result.content
    );
}
