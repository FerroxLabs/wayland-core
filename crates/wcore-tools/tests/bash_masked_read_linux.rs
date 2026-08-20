//! wayland#1078 (Linux, live bubblewrap) — **a policy denial that the command's
//! own exit status hides.**
//!
//! The issue reports that a denied file "reads as an empty file, successfully".
//! Measured against HEAD on real bubblewrap, that is NOT what happens: the
//! `/dev/null` mask is present (`ls -l` shows `crw-rw-rw- … 1, 3`) but a `cat`
//! of it fails LOUDLY with `Permission denied`, which `annotate_sandbox_denial`
//! already explains because the result is an error.
//!
//! What remains is narrower and is what these tests pin: a COMPOUND command
//! lets the shell swallow that failure. `cat secret.pem; echo rc=$?` exits 0,
//! so the tool result carries `is_error = false`, `annotate_sandbox_denial`
//! returns early, and the agent is left with emptiness and no cause. `ls -l` on
//! a masked path is the same shape.
//!
//! The containment half is asserted throughout and is not weakened: the
//! secret's bytes must never appear in any result.

#![cfg(target_os = "linux")]

use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

const SECRET: &str = "TOKEN=super-secret-value-1078";

fn contained_ctx() -> Option<(ToolContext, std::path::PathBuf, tempfile::TempDir)> {
    let dir = tempfile::tempdir().ok()?;
    let root = std::fs::canonicalize(dir.path()).ok()?;
    std::fs::write(root.join("deploy-cert.pem"), SECRET.as_bytes()).ok()?;
    std::fs::write(root.join("readme.txt"), b"plain contents\n").ok()?;
    let policy = Arc::new(WorkspacePolicy::contained(&root));
    Some((
        ToolContext::test_default().with_workspace(policy),
        root,
        dir,
    ))
}

#[tokio::test]
async fn a_denial_hidden_by_the_shell_is_still_reported() {
    if !wcore_tools::bash::platform_enforces_read_deny() {
        eprintln!("skip: no hard read-deny sandbox on this host");
        return;
    }
    let Some((ctx, root, _keep)) = contained_ctx() else {
        eprintln!("skip: could not build a contained workspace policy");
        return;
    };

    // POSITIVE CONTROL: an ordinary granted read works and is NOT annotated.
    // Without it these assertions could pass on a sandbox that refuses
    // everything, which would prove nothing.
    let granted = BashTool
        .execute_with_ctx(json!({"command": "cat readme.txt"}), &ctx)
        .await;
    if granted.is_error {
        eprintln!("skip: the live sandbox rejected a legitimate read: {granted:?}");
        return;
    }
    assert!(
        granted.content.contains("plain contents"),
        "precondition: a granted file must be readable: {}",
        granted.content
    );
    assert!(
        !granted.content.contains("policy DENIES"),
        "a granted read must not be annotated: {}",
        granted.content
    );

    // THE GAP. The shell swallows the denial, so is_error is false and the
    // existing failure-path annotation cannot fire.
    let hidden = BashTool
        .execute_with_ctx(json!({"command": "cat deploy-cert.pem; echo rc=$?"}), &ctx)
        .await;
    assert!(
        !hidden.is_error,
        "precondition: the shell must have swallowed the failure, else this \
         test is exercising the failure path instead of the gap: {}",
        hidden.content
    );
    assert!(
        !hidden.content.contains("super-secret-value-1078"),
        "CONTAINMENT REGRESSION: the secret leaked: {}",
        hidden.content
    );
    assert!(
        hidden.content.contains("deploy-cert.pem"),
        "the denied path must be named: {}",
        hidden.content
    );
    assert!(
        hidden.content.contains("POLICY"),
        "the result must attribute the outcome to policy: {}",
        hidden.content
    );

    // The `ls -l` shape: succeeds, prints a character device, explains nothing
    // on its own.
    let listed = BashTool
        .execute_with_ctx(json!({"command": "ls -l deploy-cert.pem"}), &ctx)
        .await;
    assert!(
        !listed.is_error,
        "precondition: ls of a masked path succeeds: {}",
        listed.content
    );
    assert!(
        listed.content.contains("deploy-cert.pem") && listed.content.contains("POLICY"),
        "a masked stat must be explained too: {}",
        listed.content
    );

    assert!(
        root.join("deploy-cert.pem").is_file(),
        "host file untouched"
    );
    let on_host = std::fs::read_to_string(root.join("deploy-cert.pem")).unwrap();
    assert_eq!(
        on_host, SECRET,
        "the real file must be unchanged on the host"
    );
}
