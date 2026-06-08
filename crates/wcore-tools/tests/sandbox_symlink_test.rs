//! Wave SD — SandboxedFs symlink containment tests.
//!
//! Closes SECURITY MAJOR #13 verification:
//!
//!   * A symlink planted INSIDE the sandbox that points OUTSIDE must
//!     be refused — even though lex-normalization of the path-as-string
//!     looks in-bounds, canonicalize() resolves through the symlink and
//!     reveals the real target.
//!
//!   * A symlink planted INSIDE the sandbox that points to another
//!     entry INSIDE the sandbox is allowed (positive test).
//!
//!   * `fallthrough_reads` is no longer a constructor argument — the
//!     compile-time absence of that surface is itself the load-bearing
//!     finding for SECURITY MAJOR #13.

#![cfg(unix)] // symlink test requires Unix `symlink`

use std::os::unix::fs::symlink;

use wcore_tools::vfs::{RealFs, SandboxedFs, VfsError, VirtualFs};

#[tokio::test]
async fn symlink_pointing_outside_sandbox_is_refused() {
    let sandbox = tempfile::tempdir().expect("sandbox dir");
    let outside = tempfile::tempdir().expect("outside dir");

    let outside_target = outside.path().join("secret.txt");
    tokio::fs::write(&outside_target, b"super-secret-bytes")
        .await
        .unwrap();

    // Place a symlink INSIDE the sandbox that points to a file OUTSIDE.
    // From inside the sandbox the path looks contained; canonicalize
    // resolves through the symlink to `outside_target` which the
    // containment check must reject.
    let link = sandbox.path().join("escape");
    symlink(&outside_target, &link).expect("symlink");

    let sb = SandboxedFs::new(RealFs, sandbox.path().to_path_buf());

    let err = sb.read(&link).await.unwrap_err();
    assert!(
        matches!(err, VfsError::OutsideSandbox { .. }),
        "expected OutsideSandbox, got {err:?}"
    );
}

#[tokio::test]
async fn symlink_pointing_inside_sandbox_succeeds() {
    let sandbox = tempfile::tempdir().expect("sandbox dir");

    let inner_target = sandbox.path().join("inner.txt");
    tokio::fs::write(&inner_target, b"inner-data")
        .await
        .unwrap();

    // Symlink inside-pointing-inside: allowed.
    let link = sandbox.path().join("inner-link");
    symlink(&inner_target, &link).expect("symlink");

    let sb = SandboxedFs::new(RealFs, sandbox.path().to_path_buf());

    let got = sb.read(&link).await.expect("read through inside symlink");
    assert_eq!(got, b"inner-data");
}

#[tokio::test]
async fn writes_through_outside_symlink_also_refused() {
    let sandbox = tempfile::tempdir().expect("sandbox dir");
    let outside = tempfile::tempdir().expect("outside dir");

    let outside_target = outside.path().join("victim.txt");
    tokio::fs::write(&outside_target, b"original")
        .await
        .unwrap();

    let link = sandbox.path().join("clobber");
    symlink(&outside_target, &link).expect("symlink");

    let sb = SandboxedFs::new(RealFs, sandbox.path().to_path_buf());
    let err = sb.write(&link, b"hostile").await.unwrap_err();
    assert!(matches!(err, VfsError::OutsideSandbox { .. }));

    // Verify the outside file was not touched.
    let after = tokio::fs::read(&outside_target).await.unwrap();
    assert_eq!(after, b"original");
}

#[tokio::test]
async fn reads_outside_sandbox_are_refused() {
    // Wave SD: removed `fallthrough_reads`. Any read whose canonical
    // target sits outside the sandbox root must be refused. This is
    // the surface change that closes SECURITY MAJOR #13.
    let sandbox = tempfile::tempdir().expect("sandbox dir");
    let outside = tempfile::tempdir().expect("outside dir");

    let outside_file = outside.path().join("secret.txt");
    tokio::fs::write(&outside_file, b"nope").await.unwrap();

    let sb = SandboxedFs::new(RealFs, sandbox.path().to_path_buf());
    let err = sb.read(&outside_file).await.unwrap_err();
    assert!(matches!(err, VfsError::OutsideSandbox { .. }));
}
