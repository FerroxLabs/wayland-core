//! FerroxLabs/wayland#1105 — the path-grant check-then-open window.
//!
//! `SandboxedFs::contain_read` canonicalizes a path, compares the canonical
//! form against the granted roots, and then hands the PATH back to the inner
//! filesystem, which resolves it a second time. Check and use are two separate
//! resolutions of the same name, so an actor who can write inside the granted
//! folder can swap the leaf between them.
//!
//! The ESCAPE-by-symlink variant is already closed (see
//! `a_symlink_out_of_a_granted_folder_is_still_refused` in path_grant_test.rs):
//! a symlink that is in place WHEN we canonicalize resolves outside the grant
//! and is refused. What this file is about is the swap performed AFTER that
//! canonicalize and BEFORE the open, which is the class Snyk demonstrated
//! against OpenClaw with `renameat2(RENAME_EXCHANGE)` and explicitly stated
//! more `lstat`/`realpath` checks cannot fix.
//!
//! Threat model, stated so these tests are not read as claiming more than they
//! prove: this needs an actor who can already create files inside the granted
//! folder. That is real for a shared or agent-writable directory and not real
//! for a read-only folder the user picked.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wcore_tools::vfs::{RealFs, SandboxedFs, VfsError, VfsMetadata, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

const BENIGN: &[u8] = b"<html>morning brief</html>";
const SENTINEL: &[u8] = b"SENTINEL-OUTSIDE-THE-GRANT-0dd7f1";

fn local_policy(root: &Path) -> WorkspacePolicy {
    WorkspacePolicy::contained(root).with_local_operator_principal()
}

/// THE REPRODUCTION.
///
/// One thread flips `<granted>/report.html` between a regular file holding
/// benign bytes and a symlink pointing at a sentinel file OUTSIDE the granted
/// root. Both flips are a `rename` onto the same name, which is atomic, so the
/// name always resolves to one of the two objects and never to nothing.
///
/// The other thread reads that name through the jail. Every `Ok` must be the
/// benign bytes. A single `Ok(SENTINEL)` is the check-then-open window being
/// won: the containment check saw the regular file, the open resolved the
/// symlink, and bytes from outside the grant reached the caller.
///
/// GUARD: `libc::O_NOFOLLOW` on the leaf `openat` in `vfs_pinned.rs`. Drop it
/// and the sentinel comes back.
#[cfg(unix)]
#[test]
fn a_leaf_swapped_for_a_symlink_between_the_check_and_the_open_is_refused() {
    let ws = tempfile::tempdir().unwrap();
    let granted = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();

    let secret = elsewhere.path().join("outside-the-grant.txt");
    std::fs::write(&secret, SENTINEL).unwrap();
    let secret = std::fs::canonicalize(&secret).unwrap();

    let granted_root = std::fs::canonicalize(granted.path()).unwrap();
    let report = granted_root.join("report.html");
    std::fs::write(&report, BENIGN).unwrap();

    let policy = Arc::new(local_policy(ws.path()));
    let jail = SandboxedFs::new(RealFs, ws.path().to_path_buf())
        .with_read_grants(policy.session_read_grant_handle());
    policy
        .grant_session_read_root(granted.path(), false)
        .unwrap();

    // Sanity: the ordinary granted read works, so a "no escape observed"
    // verdict below cannot be an artefact of the read never succeeding at all.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime.block_on(jail.read(&report)).unwrap(),
        BENIGN.to_vec(),
        "positive control: the granted folder is readable at all"
    );

    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let stop = Arc::clone(&stop);
        let report: PathBuf = report.clone();
        let secret = secret.clone();
        let staging_evil = report.with_extension("evil");
        let staging_benign = report.with_extension("benign");
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(&staging_evil);
                if std::os::unix::fs::symlink(&secret, &staging_evil).is_ok() {
                    let _ = std::fs::rename(&staging_evil, &report);
                }
                let _ = std::fs::remove_file(&staging_benign);
                if std::fs::write(&staging_benign, BENIGN).is_ok() {
                    let _ = std::fs::rename(&staging_benign, &report);
                }
            }
        })
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut attempts = 0_u64;
    let mut escapes = 0_u64;
    let mut refusals = 0_u64;
    let mut benign_reads = 0_u64;
    runtime.block_on(async {
        while Instant::now() < deadline {
            attempts += 1;
            match jail.read(&report).await {
                Ok(bytes) if bytes == SENTINEL => escapes += 1,
                Ok(bytes) => {
                    assert_eq!(
                        bytes,
                        BENIGN.to_vec(),
                        "a successful granted read must return the object the \
                         containment check approved, not a third thing"
                    );
                    benign_reads += 1;
                }
                Err(_) => refusals += 1,
            }
        }
    });
    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();

    assert!(
        benign_reads > 0,
        "the race loop never once read the benign object ({attempts} attempts, \
         {refusals} refusals) — the test is measuring nothing"
    );
    assert_eq!(
        escapes, 0,
        "path-grant TOCTOU: {escapes} of {attempts} reads returned bytes from \
         OUTSIDE the granted root. The containment check approved a regular \
         file and the open resolved a symlink planted after it."
    );
}

/// THE FAIL-OPEN GUARD.
///
/// `SandboxedFs::read` must route through the handle-pinned read and must NOT
/// fall back to the path-based `read` when a backend cannot pin. A fallback
/// would be a silent downgrade to the exact window this issue is about, and it
/// would be invisible, because the read would still succeed.
///
/// GUARD: the ABSENCE of an `Err(Unsupported) => self.inner.read(&p)` arm in
/// `SandboxedFs::read`. Add one and this test fails.
#[tokio::test]
async fn the_jail_refuses_rather_than_falling_back_when_the_backend_cannot_pin() {
    /// A backend that reads fine by path but leaves `read_pinned` at the trait
    /// default. Stands in for any out-of-tree `VirtualFs` implementor.
    struct UnpinnableFs;

    #[async_trait::async_trait]
    impl VirtualFs for UnpinnableFs {
        async fn read(&self, path: &Path) -> Result<Vec<u8>, VfsError> {
            Ok(std::fs::read(path)?)
        }
        async fn write(&self, _path: &Path, _contents: &[u8]) -> Result<(), VfsError> {
            unreachable!("not exercised by this test")
        }
        async fn exists(&self, path: &Path) -> Result<bool, VfsError> {
            Ok(path.exists())
        }
        async fn list(&self, _dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
            unreachable!("not exercised by this test")
        }
        async fn remove_file(&self, _path: &Path) -> Result<(), VfsError> {
            unreachable!("not exercised by this test")
        }
        async fn metadata(&self, _path: &Path) -> Result<VfsMetadata, VfsError> {
            unreachable!("not exercised by this test")
        }
    }

    let ws = tempfile::tempdir().unwrap();
    let inside = ws.path().join("in.txt");
    std::fs::write(&inside, b"in").unwrap();
    let canonical = std::fs::canonicalize(&inside).unwrap();

    let jail = SandboxedFs::new(UnpinnableFs, ws.path().to_path_buf());

    // Positive control: the backend really can serve these bytes by path, so
    // the refusal below is the jail declining to and not a broken fixture.
    assert_eq!(UnpinnableFs.read(&canonical).await.unwrap(), b"in".to_vec());

    let error = jail
        .read(&inside)
        .await
        .expect_err("a jail must refuse rather than silently read by path");
    assert!(
        matches!(&error, VfsError::Io(io) if io.kind() == std::io::ErrorKind::Unsupported),
        "the refusal must name the missing capability, got: {error}"
    );
}
