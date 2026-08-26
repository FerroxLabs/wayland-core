//! FerroxLabs/wayland#1104 — `SandboxedFs` must not follow a DANGLING symlink
//! out of the boundary it is enforcing.
//!
//! `contain` finds the longest CANONICALIZABLE prefix of a path and lets the
//! not-yet-existing suffix through, on the stated reasoning that "no symlink
//! can escape through a not-yet-created node". A dangling symlink is the
//! counter-example: the node EXISTS, `canonicalize` refuses it because its
//! TARGET does not exist, so it lands in the suffix and is never re-examined.
//!
//! MEASURED on this tree before the boundary check was added: the write was
//! ADMITTED (no `OutsideSandbox`), and no bytes escaped — because
//! `RealFs::write` goes through `wcore_config::atomic_write`, which renames a
//! tempfile over the destination and so replaces the link's own dentry instead
//! of following it. The containment was real and it belonged to one backend's
//! write strategy, not to the boundary: `observe_file` opens the path, and any
//! other `VirtualFs` implementor is free to use a plain `open(O_TRUNC)`. These
//! tests grade the BOUNDARY, so the refusal has to come from the boundary.
//!
//! Graded on the WORKSPACE root as well as on a granted root, because it is one
//! `contain` behind both: a spelling that defeats one defeats the other.

// Every case here is built from a symlink, so the whole file is unix-only.
// Stated once at the top rather than as a per-test attribute: with the
// attribute on each test, the imports below are unused on Windows and
// `-D warnings` fails the cross-target clippy gate -- which is how this line
// came to be written.
#![cfg(unix)]

use std::sync::Arc;

use wcore_tools::vfs::{RealFs, SandboxedFs, VfsError, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

#[tokio::test]
async fn a_dangling_symlink_out_of_the_workspace_is_refused() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let fs = SandboxedFs::new(RealFs, ws.path().to_path_buf());

    let escape = ws.path().join("innocent.txt");
    let victim = outside.path().join("planted.sh");
    std::os::unix::fs::symlink(&victim, &escape).unwrap();

    let outcome = fs.write(&escape, b"#!/bin/sh\nid\n").await;
    assert!(
        matches!(outcome, Err(VfsError::OutsideSandbox { .. })),
        "the BOUNDARY must refuse it, not the backend's write strategy: {outcome:?}"
    );
    assert!(!victim.exists());

    // `observe_file` is the operation with no atomic-rename to hide behind.
    assert!(matches!(
        fs.observe_file(&escape).await,
        Err(VfsError::OutsideSandbox { .. })
    ));
}

/// A chain of dangling links is followed to where it lands, not stopped at the
/// first hop — otherwise one extra link is the whole bypass.
#[tokio::test]
async fn a_chain_of_dangling_symlinks_is_followed_to_where_it_lands() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let fs = SandboxedFs::new(RealFs, ws.path().to_path_buf());

    let victim = outside.path().join("planted.sh");
    let hop2 = ws.path().join("hop2");
    let hop1 = ws.path().join("hop1");
    std::os::unix::fs::symlink(&victim, &hop2).unwrap();
    std::os::unix::fs::symlink(&hop2, &hop1).unwrap();

    assert!(matches!(
        fs.write(&hop1, b"x").await,
        Err(VfsError::OutsideSandbox { .. })
    ));
    assert!(!victim.exists());
}

/// A relative dangling link is resolved against the directory HOLDING it.
#[tokio::test]
async fn a_relative_dangling_symlink_is_resolved_against_its_own_directory() {
    let parent = tempfile::tempdir().unwrap();
    let ws = parent.path().join("ws");
    std::fs::create_dir_all(ws.join("sub")).unwrap();
    let fs = SandboxedFs::new(RealFs, ws.clone());

    let escape = ws.join("sub/out");
    std::os::unix::fs::symlink("../../planted.sh", &escape).unwrap();
    assert!(matches!(
        fs.write(&escape, b"x").await,
        Err(VfsError::OutsideSandbox { .. })
    ));
    assert!(!parent.path().join("planted.sh").exists());

    // WRONG-REFUSAL CONTROL: the same relative shape landing back inside.
    let inward = ws.join("sub/in");
    std::os::unix::fs::symlink("../landed.txt", &inward).unwrap();
    fs.write(&inward, b"ok")
        .await
        .expect("a relative link that stays inside the jail is ordinary work");
}

/// WRONG-REFUSAL CONTROL. A dangling symlink pointing back INSIDE the boundary
/// is an ordinary "write this file for me" and must still work — the guard is
/// about where the link LANDS, not about links.
#[tokio::test]
async fn a_dangling_symlink_that_lands_inside_the_workspace_still_works() {
    let ws = tempfile::tempdir().unwrap();
    let fs = SandboxedFs::new(RealFs, ws.path().to_path_buf());

    let link = ws.path().join("latest.txt");
    std::os::unix::fs::symlink(ws.path().join("2026-08-25.txt"), &link).unwrap();

    fs.write(&link, b"today").await.expect("lands in the jail");
    assert_eq!(
        fs.read(&link).await.unwrap(),
        b"today".to_vec(),
        "and the bytes are readable back through the same jail"
    );
}

/// The same two properties for a GRANTED write root (#1104).
#[tokio::test]
async fn a_dangling_symlink_out_of_a_granted_write_root_is_refused() {
    let ws = tempfile::tempdir().unwrap();
    let granted = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();

    let policy = Arc::new(
        WorkspacePolicy::contained(ws.path())
            .with_local_operator_principal()
            .with_filesystem_confinement("test-confining-backend"),
    );
    let fs = SandboxedFs::new(RealFs, ws.path().to_path_buf())
        .with_path_grants(policy.session_path_grant_handle());
    policy
        .grant_session_read_root(granted.path(), true)
        .unwrap();

    let victim = outside.path().join("planted.sh");
    let escape = granted.path().join("innocent.txt");
    std::os::unix::fs::symlink(&victim, &escape).unwrap();

    let outcome = fs.write(&escape, b"x").await;
    assert!(
        matches!(outcome, Err(VfsError::OutsideSandbox { .. })),
        "the grant leaked past its own root: {outcome:?}"
    );
    assert!(!victim.exists());

    // WRONG-REFUSAL CONTROL inside the grant.
    let inner_link = granted.path().join("latest.txt");
    std::os::unix::fs::symlink(granted.path().join("real.txt"), &inner_link).unwrap();
    fs.write(&inner_link, b"ok")
        .await
        .expect("a link that lands in the grant is ordinary work");
    assert_eq!(fs.read(&inner_link).await.unwrap(), b"ok".to_vec());
}
