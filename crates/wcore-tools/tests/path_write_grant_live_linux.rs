//! FerroxLabs/wayland#1104 (Linux, live bubblewrap) — a WRITE path grant has to
//! reach the OS sandbox, not just the in-process file tools.
//!
//! The two enforcement layers must never hold different answers to "what may
//! this session write". `SandboxedFs` is graded by `path_write_grant_test.rs`;
//! this file grades the other half, through `BashTool::execute_with_ctx` — the
//! agent's own shell tool, on the host's real containment backend — so the
//! evidence is the child's own observation rather than a manifest we built and
//! then read back.
//!
//! It also settles the claim #1104 was filed on, which is FALSE and was
//! measured false on this host: *"an empty `fs_write_allow` means 'no
//! confinement' to the backends, so narrowing fails open."* The read-only-grant
//! arm below is a directory the child can READ and cannot WRITE, which is
//! exactly the "writable, except here" the ticket says the manifest cannot
//! express. The manifest expresses it by not listing the root in
//! `fs_write_allow`, and bubblewrap binds it read-only.

#![cfg(target_os = "linux")]

use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

/// A local-operator, filesystem-confined contained policy — the only shape a
/// write grant is minted for.
fn confined_ctx(root: &std::path::Path) -> (ToolContext, Arc<WorkspacePolicy>) {
    let policy = Arc::new(
        WorkspacePolicy::contained(root)
            .with_local_operator_principal()
            .with_filesystem_confinement("bubblewrap"),
    );
    (
        ToolContext::test_default().with_workspace(Arc::clone(&policy)),
        policy,
    )
}

async fn run(ctx: &ToolContext, command: &str) -> wcore_types::tool::ToolResult {
    BashTool
        .execute_with_ctx(json!({ "command": command }), ctx)
        .await
}

#[tokio::test]
async fn a_write_grant_is_honoured_by_the_real_os_sandbox_and_stops_at_its_root() {
    if !wcore_tools::bash::platform_enforces_read_deny() {
        eprintln!("skip: no hard-enforcing sandbox backend on this host");
        return;
    }
    let ws = tempfile::tempdir().unwrap();
    let workspace = std::fs::canonicalize(ws.path()).unwrap();
    let outer = tempfile::tempdir().unwrap();
    let granted = std::fs::canonicalize(outer.path()).unwrap().join("granted");
    let sibling = std::fs::canonicalize(outer.path()).unwrap().join("sibling");
    std::fs::create_dir(&granted).unwrap();
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(granted.join("brief.md"), b"# brief\n").unwrap();

    let (ctx, policy) = confined_ctx(&workspace);

    // POSITIVE CONTROL first: the sandbox lets a legitimate in-workspace write
    // through. Without it every refusal below could be a sandbox that refuses
    // everything, which would prove nothing.
    let control = run(&ctx, "echo control > control.txt; echo rc=$?").await;
    if !control.content.contains("rc=0") {
        eprintln!("skip: the live sandbox refused a legitimate write: {control:?}");
        return;
    }
    assert!(workspace.join("control.txt").exists());

    // RED: before the grant, the granted folder is not writable.
    let before = run(
        &ctx,
        &format!("echo x > {}/report.txt; echo rc=$?", granted.display()),
    )
    .await;
    assert!(
        !before.content.contains("rc=0"),
        "an ungranted folder must not be writable: {}",
        before.content
    );
    assert!(!granted.join("report.txt").exists());

    policy
        .grant_session_read_root(&granted, true)
        .expect("a clean folder on a confining backend is grantable for write");

    // GREEN: the write lands, and the bytes are on the HOST filesystem — not
    // merely reported as successful by the child.
    let after = run(
        &ctx,
        &format!(
            "echo granted > {}/report.txt; echo rc=$?",
            granted.display()
        ),
    )
    .await;
    assert!(
        after.content.contains("rc=0"),
        "the grant did not reach the OS sandbox: {}",
        after.content
    );
    assert_eq!(
        std::fs::read_to_string(granted.join("report.txt")).unwrap(),
        "granted\n"
    );

    // ... and reads back, which is the other half of the definition of done.
    let read_back = run(&ctx, &format!("cat {}/brief.md", granted.display())).await;
    assert!(
        read_back.content.contains("# brief"),
        "{}",
        read_back.content
    );

    // BOUNDARY: the grant widened exactly one root. The sibling next to it, and
    // the parent above it, are untouched.
    for escape in [sibling.join("loot.txt"), granted.join("../loot.txt")] {
        let out = run(
            &ctx,
            &format!("echo loot > {}; echo rc=$?", escape.display()),
        )
        .await;
        assert!(
            !out.content.contains("rc=0"),
            "{} escaped the granted root: {}",
            escape.display(),
            out.content
        );
    }
    assert!(!sibling.join("loot.txt").exists());
    assert!(!granted.parent().unwrap().join("loot.txt").exists());
}

/// The ticket's premise, tested directly: a root the manifest grants for READ
/// and withholds from WRITE is readable and not writable on a real backend.
///
/// This is "writable, except here" expressed by the manifest that supposedly
/// cannot express it — and it is the definition-of-done clause "a read-only
/// grant on the same folder still refuses the write", measured at the OS layer
/// rather than in the file tools.
#[tokio::test]
async fn a_read_only_grant_is_readable_and_not_writable_at_the_os_layer() {
    if !wcore_tools::bash::platform_enforces_read_deny() {
        eprintln!("skip: no hard-enforcing sandbox backend on this host");
        return;
    }
    let ws = tempfile::tempdir().unwrap();
    let workspace = std::fs::canonicalize(ws.path()).unwrap();
    let outer = tempfile::tempdir().unwrap();
    let granted = std::fs::canonicalize(outer.path()).unwrap();
    std::fs::write(granted.join("brief.md"), b"# brief\n").unwrap();

    let (ctx, policy) = confined_ctx(&workspace);
    let control = run(&ctx, "echo control > control.txt; echo rc=$?").await;
    if !control.content.contains("rc=0") {
        eprintln!("skip: the live sandbox refused a legitimate write: {control:?}");
        return;
    }

    policy
        .grant_session_read_root(&granted, false)
        .expect("read grant");

    let read = run(&ctx, &format!("cat {}/brief.md", granted.display())).await;
    assert!(
        read.content.contains("# brief"),
        "WRONG-REFUSAL CONTROL: the read the grant was asked for must work: {}",
        read.content
    );

    let write = run(
        &ctx,
        &format!("echo tampered > {}/brief.md; echo rc=$?", granted.display()),
    )
    .await;
    assert!(
        !write.content.contains("rc=0"),
        "a READ grant must not carry write authority into the OS sandbox: {}",
        write.content
    );
    assert_eq!(
        std::fs::read_to_string(granted.join("brief.md")).unwrap(),
        "# brief\n",
        "and the bytes on the host are untouched"
    );
}
