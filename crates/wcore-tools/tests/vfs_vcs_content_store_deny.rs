//! core#244 + core#322 — **the in-process file tools must not read a VCS
//! CONTENT store, at the workspace root or at any depth beneath it.**
//!
//! Two halves of one gap:
//!
//! * **#244** `Bash` has been denied `<root>/.git/objects` since #234 (it is in
//!   `WorkspacePolicy::secret_deny_paths_dynamic`, which the OS sandbox applies
//!   as `fs_read_deny`). The VFS never was: `SecretDenyFs`'s only predicate was
//!   `is_project_secret`, which matches secret NAMES, and an object file is
//!   named after its hash. So `Read(".git/objects/ab/cdef…")` succeeded against
//!   bytes the shell could not touch. The blobs are zlib-compressed, so this is
//!   a gap rather than a plaintext leak — but the two layers are supposed to
//!   agree, and they did not.
//! * **#322** discovery was root-relative only (`root.join(".git/objects")`), so
//!   a vendored or nested checkout — `<root>/vendor/x/.git/objects`, a submodule
//!   working copy, a bundled example repo — was denied by NEITHER layer.
//!
//! **Red arm for the VFS half:** delete `|| self.policy.is_vcs_content_store(path)`
//! from `SecretDenyFs::guard` in `crates/wcore-tools/src/vfs.rs`. Every
//! `#[test]` in this file except `nested_store_reaches_the_os_deny_list` must go
//! red. That call site is the whole of the in-process wiring; the predicate
//! answering correctly on its own denies nothing.
//!
//! **Red arm for the walk half:** delete the `vcs_store_entry` call from either
//! arm of `project_committed_secrets` in `workspace_policy.rs`.
//!
//! The wrong-refusal controls are not decoration. `vcs_content_stores`
//! deliberately leaves `.git/HEAD` and `.git/refs` readable so `git rev-parse`
//! (a SHA, no content) still works, and a deny that swallowed those would break
//! ordinary session work in both the root repo and the vendored one.

use std::path::Path;
use std::sync::Arc;
use wcore_tools::vfs::{RealFs, SecretDenyFs, VfsError, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

/// A root repo plus a vendored one, each carrying a loose object and the
/// metadata files that must stay readable.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    for repo in ["", "vendor/x"] {
        let git = root.join(repo).join(".git");
        std::fs::create_dir_all(git.join("objects/ab")).unwrap();
        std::fs::create_dir_all(git.join("refs/heads")).unwrap();
        std::fs::write(git.join("objects/ab/cd1234"), b"\x78\x01zlib-blob").unwrap();
        std::fs::write(git.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        std::fs::write(git.join("refs/heads/main"), b"deadbeef\n").unwrap();
    }
    std::fs::write(root.join("main.rs"), b"fn main() {}").unwrap();
    (dir, root)
}

fn deny_fs(root: &Path) -> SecretDenyFs<RealFs> {
    SecretDenyFs::new(RealFs, Arc::new(WorkspacePolicy::contained(root)))
}

async fn assert_refused(fs: &SecretDenyFs<RealFs>, path: &Path, why: &str) {
    assert!(
        matches!(fs.read(path).await, Err(VfsError::SecretDenied { .. })),
        "{why}: {} must be refused by the in-process VFS, got {:?}",
        path.display(),
        fs.read(path).await.map(|b| b.len()),
    );
}

async fn assert_readable(fs: &SecretDenyFs<RealFs>, path: &Path, why: &str) {
    assert!(
        fs.read(path).await.is_ok(),
        "{why}: {} must stay readable, got {:?}",
        path.display(),
        fs.read(path).await.err(),
    );
}

/// #244 — the root repository's object store, through the layer that was open.
#[tokio::test]
async fn root_object_store_is_refused_by_the_vfs() {
    let (_dir, root) = fixture();
    let fs = deny_fs(&root);

    assert_refused(
        &fs,
        &root.join(".git/objects/ab/cd1234"),
        "#244 loose object",
    )
    .await;
    assert_refused(&fs, &root.join(".git/objects"), "#244 store directory").await;

    // Control: the layer is not simply refusing everything.
    assert_readable(&fs, &root.join("main.rs"), "ordinary source file").await;
}

/// #322 — the vendored repository's object store, which root-relative
/// discovery never sees.
#[tokio::test]
async fn nested_object_store_is_refused_by_the_vfs() {
    let (_dir, root) = fixture();
    let fs = deny_fs(&root);

    assert_refused(
        &fs,
        &root.join("vendor/x/.git/objects/ab/cd1234"),
        "#322 nested loose object",
    )
    .await;
    assert_refused(
        &fs,
        &root.join("vendor/x/.git/objects"),
        "#322 nested store directory",
    )
    .await;
}

/// The wrong-refusal controls: a deny that broke these would break every
/// session that runs `git status` / `git rev-parse`, in the root repo and in
/// the vendored one alike.
#[tokio::test]
async fn repository_metadata_stays_readable() {
    let (_dir, root) = fixture();
    let fs = deny_fs(&root);

    for rel in [
        ".git/HEAD",
        ".git/refs/heads/main",
        "vendor/x/.git/HEAD",
        "vendor/x/.git/refs/heads/main",
    ] {
        assert_readable(&fs, &root.join(rel), "git rev-parse carve-out").await;
    }
}

/// Every VCS the deny list covers, at both depths. `.hg/dirstate` and
/// `.svn/wc.db` carry working STATE, not committed content, and stay readable
/// for the same reason `.git/HEAD` does.
#[tokio::test]
async fn every_vcs_store_shape_is_refused_at_any_depth() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let stores = [
        (".git", "objects"),
        (".git", "modules"),
        (".git", "lfs"),
        (".hg", "store"),
        (".svn", "pristine"),
        (".bzr", "repository"),
    ];
    for prefix in ["", "vendor/x"] {
        for (ctl, store) in stores {
            let d = root.join(prefix).join(ctl).join(store);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("payload"), b"content").unwrap();
        }
        std::fs::write(root.join(prefix).join(".hg/dirstate"), b"state").unwrap();
        std::fs::write(root.join(prefix).join(".svn/wc.db"), b"db").unwrap();
    }
    let fs = deny_fs(&root);

    for prefix in ["", "vendor/x"] {
        for (ctl, store) in stores {
            let payload = root.join(prefix).join(ctl).join(store).join("payload");
            assert_refused(&fs, &payload, "committed content store").await;
        }
        for rel in [".hg/dirstate", ".svn/wc.db"] {
            assert_readable(&fs, &root.join(prefix).join(rel), "working state").await;
        }
    }
}

/// #322 for the OTHER layer: the walk must emit the nested store DIRECTORY into
/// the OS deny list, so `Bash` inside `<root>/vendor/x` cannot `git show
/// HEAD:.env` either.
///
/// It must emit the directory and NOT its members — that is what keeps the fix
/// off the per-object `canonicalize` the no-prune walk would otherwise pay.
#[test]
fn nested_store_reaches_the_os_deny_list() {
    let (_dir, root) = fixture();
    let deny = WorkspacePolicy::contained(&root).secret_deny_paths_for_backend(true);

    for rel in [".git/objects", "vendor/x/.git/objects"] {
        let want = std::fs::canonicalize(root.join(rel)).unwrap();
        assert!(
            deny.contains(&want),
            "{rel} must be in the OS deny list; got {deny:?}"
        );
    }
    assert!(
        !deny
            .iter()
            .any(|p| p.ends_with("objects/ab/cd1234") || p.ends_with("objects/ab")),
        "the walk must emit the STORE, never its members; got {deny:?}"
    );
    // Control: the metadata the deny deliberately leaves alone is absent too,
    // so the assertion above is not passing because the list is empty.
    assert!(
        !deny.iter().any(|p| p.ends_with(".git/HEAD")),
        "control: .git/HEAD must not be denied; got {deny:?}"
    );
}
