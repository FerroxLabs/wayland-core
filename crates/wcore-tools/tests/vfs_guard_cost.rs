//! FerroxLabs/wayland-core#376 c3 — **pin what one ordinary-path
//! `SecretDenyFs` guard costs, so the rebuild cannot come back silently.**
//!
//! MEASURED on hetzner at `origin/integ/f13` before any change, by differential
//! `strace -f -c` over the release example `secret_deny_cost` at two operation
//! counts (the difference divided by the difference in ops cancels all one-off
//! and harness cost):
//!
//! ```text
//!                        BEFORE          AFTER
//!   is_project_secret     6 syscalls      6      (one path canonicalization)
//!   is_vcs_content_store 18 syscalls      9
//!   ---------------------------------------------
//!   guard total          24 syscalls      9
//!   SandboxedFs+guard    39 syscalls     24      (one whole `exists()`)
//! ```
//!
//! The 24 were: two INDEPENDENT canonicalizations of the same path (6 readlink
//! each, one per predicate) plus a full rebuild of the arm-2 store list on
//! every call (6 `exists` for the root-relative store leaves, 1 `metadata` for
//! the gitfile probe, 1 `openat` for the `alternates` read, 4 `readlink`
//! canonicalizing the leaf that exists). `SecretDenyFs` is installed
//! unconditionally for every sub-agent and every channel/remote session, and
//! sub-agents are read-heavy.
//!
//! Wall-clock is NOT the instrument. The build host runs eight lanes at load
//! ~125 and two runs of the identical binary differed by 2.5x on an unchanged
//! code path; a duration cannot tell a skipped scan from a fast one. The
//! counters below are incremented by our own probe calls, not by the OS, so
//! they are identical on every platform.
//!
//! **Red arm:** in `WorkspacePolicy::vcs_stores_memoized`, delete the
//! `vcs_store_cache_hit()` early return — the memo's only reader. The steady
//! state assertion goes red (one scan per guard). Separately, restore
//! `is_project_secret` / `is_vcs_content_store` to resolving the path
//! themselves inside `denies_read_content` and the resolve count doubles.
//!
//! The invalidation tests are not decoration: a memo that never rebuilds passes
//! every cost assertion in this file and denies nothing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs, VfsError, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

/// The scan is trusted only once its witnesses' mtimes lag its own start
/// instant by more than one filesystem tick (#1145). A fixture built
/// microseconds earlier is deliberately NOT settled, so the wait is part of
/// reaching the steady state this test is about — not a flake mitigation.
const SETTLE: Duration = Duration::from_millis(60);

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("workspace");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
    std::fs::write(root.join(".git/objects/ab/cdef"), b"x").unwrap();
    std::fs::create_dir_all(root.join("src/deep/deeper")).unwrap();
    std::fs::write(root.join("src/deep/deeper/main.rs"), b"fn main() {}\n").unwrap();
    (dir, root)
}

fn stack(policy: &Arc<WorkspacePolicy>, root: &Path) -> SandboxedFs<SecretDenyFs<RealFs>> {
    SandboxedFs::new(
        SecretDenyFs::new(RealFs, Arc::clone(policy)),
        root.to_path_buf(),
    )
}

/// One guard on an ordinary path: exactly ONE path resolution, and after the
/// first, ZERO rebuilds of the store list and exactly THREE filesystem probes.
///
/// Three is the whole witness set for an ordinary git checkout: the workspace
/// root (whether any control directory exists at all), `<root>/.git` (whether
/// its store leaves appear, vanish or are re-pointed, and the gitfile's own
/// content) and `<root>/.git/objects/info/alternates` (a borrowed store, whose
/// creation moves nothing else that is stamped).
#[tokio::test]
async fn one_ordinary_path_guard_resolves_once_and_does_not_rescan() {
    let (_dir, root) = fixture();
    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    let ordinary = root.join("src/deep/deeper/main.rs");
    tokio::time::sleep(SETTLE).await;

    fs.exists(&ordinary).await.expect("ordinary path readable");
    let (resolves, scans, first_probes) = policy.guard_cost();
    assert_eq!(resolves, 1, "one guard must resolve the path exactly once");
    assert_eq!(scans, 1, "the first guard scans");
    assert_eq!(
        first_probes, 17,
        "the store scan's filesystem probe count moved; if that is intended, \
         update this number and re-measure the syscall figures in this file's \
         header"
    );

    const N: u64 = 50;
    for _ in 0..N {
        fs.exists(&ordinary).await.expect("ordinary path readable");
    }
    let (resolves, scans, probes) = policy.guard_cost();
    assert_eq!(resolves, N + 1, "still exactly one resolution per guard");
    assert_eq!(
        scans, 1,
        "the store list was rebuilt from the filesystem on the common path — \
         core#376 has regressed"
    );
    assert_eq!(
        probes - first_probes,
        N * 3,
        "an ordinary-path guard must cost exactly three filesystem probes once \
         the scan is warm"
    );
}

/// The guard's two halves must resolve ONE path, not two.
///
/// Graded separately from the scan because the two savings are independent: the
/// memo can be intact while the predicates each canonicalize again.
#[tokio::test]
async fn the_two_guard_predicates_share_one_resolution() {
    let (_dir, root) = fixture();
    let policy = Arc::new(WorkspacePolicy::contained(&root));
    tokio::time::sleep(SETTLE).await;

    policy.denies_read_content(&root.join("src/deep/deeper/main.rs"));
    let (resolves, _, _) = policy.guard_cost();
    assert_eq!(
        resolves, 1,
        "`denies_read_content` resolved the path once per predicate instead of \
         once per call"
    );
}

/// CONTROL — a store that did not exist when the memo was built is denied on
/// the very next guard.
///
/// Graded through ARM 1, not the memo: `<root>/.svn/pristine` is under the root
/// and carries the `(control, store)` shape, so the lexical arm answers before
/// arm 2 is consulted at all. MEASURED: with the memo's revalidation loop
/// short-circuited to always hit, this test still passes. It is carried anyway
/// because "the answer is still right for a store created mid-session" is the
/// property a reader will assume the cost work could break, and the arm that
/// actually grades invalidation is
/// `an_alternates_borrow_written_after_the_scan_is_denied` below — an
/// arm-2-only store, reachable through no lexical shape.
#[tokio::test]
async fn a_store_created_after_the_scan_is_denied_on_the_next_guard() {
    let (_dir, root) = fixture();
    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    tokio::time::sleep(SETTLE).await;

    // Warm the memo on an ordinary path, so the store list in hand predates the
    // store below.
    fs.exists(&root.join("src/deep/deeper/main.rs"))
        .await
        .expect("ordinary path readable");
    let (_, scans_before, _) = policy.guard_cost();
    assert_eq!(
        scans_before, 1,
        "the memo must be warm for this to mean anything"
    );

    // A checkout appears mid-session. `.svn/pristine` holds VERBATIM committed
    // content, which is why #375 cares about it.
    std::fs::create_dir_all(root.join(".svn/pristine/aa")).unwrap();
    std::fs::write(root.join(".svn/pristine/aa/deadbeef.svn-base"), b"SECRET\n").unwrap();

    let refusal = fs
        .read(&root.join(".svn/pristine/aa/deadbeef.svn-base"))
        .await;
    assert!(
        matches!(refusal, Err(VfsError::SecretDenied { .. })),
        "a content store created after the memo was built was READ: {refusal:?}"
    );
}

/// INVALIDATION — a BORROWED store, named by an `objects/info/alternates` file
/// written after the memo was built.
///
/// A different witness from the one above: creating `.svn` moves the workspace
/// root's mtime, while creating `.git/objects/info/alternates` moves neither
/// the root's nor `.git`'s. If the alternates file were not stamped in its own
/// right this arm would pass the cost test and leak.
#[tokio::test]
async fn an_alternates_borrow_written_after_the_scan_is_denied() {
    let (_dir, root) = fixture();
    let borrowed = tempfile::tempdir().expect("borrowed store");
    let borrowed_root = std::fs::canonicalize(borrowed.path()).unwrap();
    std::fs::create_dir_all(borrowed_root.join("objects/ab")).unwrap();
    std::fs::write(borrowed_root.join("objects/ab/cdef"), b"BORROWED\n").unwrap();

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    tokio::time::sleep(SETTLE).await;
    fs.exists(&root.join("src/deep/deeper/main.rs"))
        .await
        .expect("ordinary path readable");

    // CONTROL: before the borrow is declared, the policy does not deny it.
    assert!(
        !policy.denies_read_content(&borrowed_root.join("objects/ab/cdef")),
        "CONTROL: an undeclared external directory is not this workspace's store"
    );

    std::fs::create_dir_all(root.join(".git/objects/info")).unwrap();
    std::fs::write(
        root.join(".git/objects/info/alternates"),
        format!("{}\n", borrowed_root.join("objects").display()),
    )
    .unwrap();

    assert!(
        policy.denies_read_content(&borrowed_root.join("objects/ab/cdef")),
        "an alternates borrow declared after the memo was built was not denied — \
         the alternates file is not being stamped"
    );
}

/// The memo must not make the answer WRONG in the ordinary direction either:
/// the store that existed all along stays denied, and the ordinary file stays
/// readable, after the memo is warm.
#[tokio::test]
async fn the_warm_memo_still_answers_both_directions() {
    let (_dir, root) = fixture();
    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    tokio::time::sleep(SETTLE).await;

    for _ in 0..5 {
        fs.exists(&root.join("src/deep/deeper/main.rs"))
            .await
            .expect("ordinary path stays readable through a warm memo");
        let refusal = fs.read(&root.join(".git/objects/ab/cdef")).await;
        assert!(
            matches!(refusal, Err(VfsError::SecretDenied { .. })),
            "the store stayed denied? got {refusal:?}"
        );
    }
}
