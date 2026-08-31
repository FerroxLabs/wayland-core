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
//!
//! **core#394 c3 / #396 c3 / #398 c3, measured 2026-08-31** — the OTHER call
//! shape, the one `grep_policy::scope_for` pays once per directory it
//! traverses, differential `strace -f -c` on an ordinary directory with the
//! arms interleaved and every known-positive control green:
//!
//! ```text
//!                                      before   after
//!   vcs_content_stores(dir)            17.000    5.000   syscalls/directory
//!   denies_read_content(dir)            8.000    8.000
//!   ------------------------------------------------
//!   the pair scope_for pays            25.000   13.000
//! ```
//!
//! 5.000 is the figure those three criteria pin, and it is what
//! `scan_control_dirs_in` restores: probing the store leaves of a control
//! directory that is not there cost twelve syscalls at every directory of
//! every `Grep(".")`, and nothing can be found under a directory that does
//! not exist.

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
        first_probes, 36,
        "the store scan's filesystem probe count moved; if that is intended, \
         update this number and re-measure the syscall figures in this file's \
         header"
    );
    // FerroxLabs/wayland-core#398 — arm 4's discovery walk is a per-policy
    // ONE-OFF. 36 is 12 (the arm-2 scan) + 24 for one walk of this fixture;
    // the whole point of the design is that the second number is paid once and
    // never revalidated, which the steady-state assertion below is what proves.
    //
    // It was 41 until core#394 c3 / #396 c3 / #398 c3 were measured with the
    // instrument that set their 5-syscalls/directory bar: `scan_control_dirs_in`
    // now skips the store leaves of an ABSENT control directory and
    // deduplicates the four control names out of six rows, so the arm-2 scan
    // costs 12 probes here where it cost 17. The WARM number below is
    // untouched at three, because an absent control directory was never
    // stamped and the witness set did not change.
    assert_eq!(
        policy.nested_walk_count(),
        1,
        "the nested-store walk must run at most once for the life of a policy"
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
    assert_eq!(
        policy.nested_walk_count(),
        1,
        "core#398: the nested-store walk came back on the common path — a \
         whole-tree walk per guard is the regression this file exists to catch"
    );
}

/// FerroxLabs/wayland-core#398 c1 — **the warm per-guard cost of a path that
/// carries a store LEAF NAME does not scale with the workspace.**
///
/// `objects`, `modules`, `store` and `lfs` are ordinary project directory
/// names: a Terraform `modules/`, a Redux store, an asset pipeline. #398
/// measured a tree at which such a path cost one filesystem probe per
/// workspace DIRECTORY — slope 1.000 across an 8-directory and a 48-directory
/// workspace — because a whole-tree walk had been gated on that spelling and
/// its witness set was re-`stat`ed on every guard.
///
/// The slope assertion is INVERTED here rather than deleted, as #398 c1 asks:
/// the same two workspace sizes, the same admitted path, and the difference
/// must now be ZERO. It is zero by construction and not by tuning — arm 4
/// answers from a set built once and never revalidated, so there is nothing
/// per-directory left to pay, and arm 3 probes only the ancestors of the path
/// in hand.
///
/// **Red arm:** make `WorkspacePolicy::nested_content_stores` call
/// `discover_nested_content_stores` on every invocation instead of through the
/// `OnceLock`, and this goes red with a slope of one probe per workspace
/// directory — the exact shape #398 reports.
#[tokio::test]
async fn a_store_named_path_costs_the_same_at_any_workspace_size() {
    async fn warm_probes_per_guard(extra_dirs: usize) -> u64 {
        let dir = tempfile::tempdir().expect("workspace");
        let root = std::fs::canonicalize(dir.path()).expect("canonical root");
        std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
        std::fs::write(root.join(".git/objects/ab/cdef"), b"x").unwrap();
        // The admitted path: a `modules` component, and no control directory,
        // gitfile, bare repository or store anywhere beneath it.
        std::fs::create_dir_all(root.join("modules/vpc")).unwrap();
        std::fs::write(root.join("modules/vpc/main.tf"), b"# tf\n").unwrap();
        for i in 0..extra_dirs {
            std::fs::create_dir_all(root.join(format!("pkg{i}/src"))).unwrap();
        }
        let policy = Arc::new(WorkspacePolicy::contained(&root));
        let fs = stack(&policy, &root);
        let admitted = root.join("modules/vpc/main.tf");
        tokio::time::sleep(SETTLE).await;

        // Reach the steady state FIRST: the cold walk is a one-off and this
        // measures what a session actually spends, not its first instant.
        fs.exists(&admitted).await.expect("admitted path readable");
        let (_, _, before) = policy.guard_cost();
        const N: u64 = 20;
        for _ in 0..N {
            fs.exists(&admitted).await.expect("admitted path readable");
        }
        let (_, _, after) = policy.guard_cost();
        assert_eq!(
            policy.nested_walk_count(),
            1,
            "the walk must stay a one-off for this measurement to be about the \
             warm state"
        );
        (after - before) / N
    }

    let small = warm_probes_per_guard(4).await;
    let large = warm_probes_per_guard(44).await;
    assert_eq!(
        large, small,
        "core#398 c1: a guard on `modules/vpc/main.tf` cost {small} probes in a \
         small workspace and {large} in one with 40 more directories — the \
         per-guard cost is scaling with the tree again"
    );
    assert_eq!(
        small, 4,
        "core#398 c1/c2 state this as a NUMBER: three arm-2 revalidation \
         probes plus one arm-3 repository probe (`<root>/HEAD`, absent). If \
         this moved, re-measure and re-state it on the ticket rather than \
         editing it here"
    );
}

/// FerroxLabs/wayland-core#406 c2 — **what closing #406 c1 costs, as a number,
/// counted rather than timed.**
///
/// Arm 4's store set is revalidated on the branch that is about to ADMIT, and
/// the witness set it revalidates is the DECLARATION SITES the walk read — one
/// gitfile, one `commondir`, one `alternates` and the store leaves, per NESTED
/// CHECKOUT — never the directories the walk descended. So the closure's price
/// scales with the number of vendored checkouts and not with the tree, which is
/// the whole reason #398 c1's slope stays at zero while #406 c1 closes.
///
/// Two figures, both stated here:
///
/// * **zero** extra probes when the workspace has no nested checkout — the
///   witness set is empty, and `one_ordinary_path_guard_resolves_once_and_does_not_rescan`
///   still measures exactly three;
/// * **`WITNESSES_PER_CHECKOUT`** extra probes per admitted guard for each
///   nested checkout, INDEPENDENT of the workspace's directory count, which is
///   what this test measures at two workspace sizes.
///
/// **Red arm:** witness the directories `discover_nested_content_stores`
/// descends instead of the declaration sites it reads (the shape #398 reports),
/// and the two sizes below diverge by one probe per extra directory.
#[tokio::test]
async fn the_post_walk_freshness_check_scales_with_checkouts_not_directories() {
    /// Declaration sites one vendored `.git` DIRECTORY contributes: the
    /// control directory itself (one stamp covering all six store leaves,
    /// which cannot appear or be re-pointed without moving its mtime) and its
    /// `objects/info/alternates`, whose CONTENT no directory mtime witnesses.
    const WITNESSES_PER_CHECKOUT: u64 = 2;

    async fn warm_probes_per_guard(checkouts: usize, extra_dirs: usize) -> u64 {
        let dir = tempfile::tempdir().expect("workspace");
        let root = std::fs::canonicalize(dir.path()).expect("canonical root");
        std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
        std::fs::write(root.join(".git/objects/ab/cdef"), b"x").unwrap();
        std::fs::create_dir_all(root.join("src/deep/deeper")).unwrap();
        std::fs::write(root.join("src/deep/deeper/main.rs"), b"fn main() {}\n").unwrap();
        for i in 0..checkouts {
            let git = root.join(format!("vendor/pkg{i}/.git"));
            std::fs::create_dir_all(git.join("objects/ab")).unwrap();
            std::fs::write(git.join("HEAD"), b"ref: refs/heads/main").unwrap();
        }
        for i in 0..extra_dirs {
            std::fs::create_dir_all(root.join(format!("pkg{i}/src"))).unwrap();
        }
        let policy = Arc::new(WorkspacePolicy::contained(&root));
        let fs = stack(&policy, &root);
        let ordinary = root.join("src/deep/deeper/main.rs");
        tokio::time::sleep(SETTLE).await;

        fs.exists(&ordinary).await.expect("ordinary path readable");
        let (_, _, before) = policy.guard_cost();
        let walks_before = policy.nested_walk_count();
        const N: u64 = 20;
        for _ in 0..N {
            fs.exists(&ordinary).await.expect("ordinary path readable");
        }
        let (_, _, after) = policy.guard_cost();
        assert_eq!(
            policy.nested_walk_count(),
            walks_before,
            "the freshness check must not RE-WALK on an unchanged tree — a \
             revalidation that rescans every time is not a cache"
        );
        (after - before) / N
    }

    let none_small = warm_probes_per_guard(0, 4).await;
    let none_large = warm_probes_per_guard(0, 44).await;
    assert_eq!(
        (none_small, none_large),
        (3, 3),
        "core#406 c2: with no nested checkout the witness set is EMPTY, so \
         closing core#406 c1 must cost nothing at all and the figure core#398 \
         c2 pins must be untouched"
    );

    let one_small = warm_probes_per_guard(1, 4).await;
    let one_large = warm_probes_per_guard(1, 44).await;
    assert_eq!(
        one_large, one_small,
        "core#398 c1: the post-walk freshness check cost {one_small} probes in \
         a small workspace and {one_large} in one with 40 more directories — \
         it is scaling with the tree, which is the regression core#398 exists \
         to catch"
    );
    assert_eq!(
        one_small,
        none_small + WITNESSES_PER_CHECKOUT,
        "core#406 c2 states this as a NUMBER: one vendored checkout adds \
         exactly {WITNESSES_PER_CHECKOUT} declaration-site probes to an \
         admitted guard. If this moved, re-measure and re-state it on the \
         ticket rather than editing it here"
    );

    let two_small = warm_probes_per_guard(2, 4).await;
    assert_eq!(
        two_small,
        none_small + 2 * WITNESSES_PER_CHECKOUT,
        "the cost must be LINEAR IN CHECKOUTS and nothing else: one checkout \
         cost {one_small}, two cost {two_small}"
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

/// INVALIDATION, ARM 2 ONLY — a store under a SYMLINKED control directory,
/// created after the memo was built.
///
/// The shape the first cut of this memo failed OPEN on, and the reason it was
/// worse than no memo at all: `<root>/.git` is a symlink, so nothing the scan
/// stamped moves when the store leaf appears — not `<root>` (the target
/// directory already existed), not `<root>/.git` (a link's mtime does not
/// follow its target), not `objects/info/alternates` (still absent). The stale
/// EMPTY list was then returned for the life of the process, and `Grep` handed
/// back `.git/lfs` plaintext under `WorkspacePolicy::contained`.
///
/// Arm 1 cannot rescue it, which is what makes this arm-2-ONLY and different in
/// kind from `a_store_created_after_the_scan_is_denied_on_the_next_guard`
/// above: the canonical path is `<root>/real-git/objects`, whose parent
/// component is not `.git`, so the lexical shape test never fires. The second
/// control below asserts exactly that, so the arm cannot quietly start passing
/// through arm 1.
#[tokio::test]
#[cfg(unix)]
async fn a_store_under_a_symlinked_control_dir_created_after_the_scan_is_denied() {
    let dir = tempfile::tempdir().expect("workspace");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    std::fs::create_dir_all(root.join("real-git")).unwrap();
    std::os::unix::fs::symlink(root.join("real-git"), root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src/deep/deeper")).unwrap();
    std::fs::write(root.join("src/deep/deeper/main.rs"), b"fn main() {}\n").unwrap();

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    tokio::time::sleep(SETTLE).await;

    // What every session does first: one ordinary guard, which warms the memo.
    fs.exists(&root.join("src/deep/deeper/main.rs"))
        .await
        .expect("ordinary path readable");
    let (_, scans, _) = policy.guard_cost();
    assert_eq!(scans, 1, "the memo must be warm for this to mean anything");

    // The store appears afterwards — `git init`, a submodule checkout, `git
    // lfs pull`, a clone finishing.
    let store_file = root.join("real-git/objects/ab/cdef");
    std::fs::create_dir_all(root.join("real-git/objects/ab")).unwrap();
    std::fs::write(&store_file, b"COMMITTED-SECRET\n").unwrap();

    // CONTROL 1: a policy built NOW denies it, so a failure below is the MEMO
    // and not a fixture that was never a store in the first place.
    assert!(
        WorkspacePolicy::contained(&root).is_vcs_content_store(&store_file),
        "CONTROL: even a freshly built policy does not see this store — the \
         fixture is wrong, not the memo"
    );
    // CONTROL 2: no `.git` component survives canonicalization, so arm 1's
    // lexical shape test cannot be the thing answering.
    assert!(
        !store_file
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new(".git")),
        "CONTROL: this arm only grades the memo while the store's canonical \
         path carries no `.git` component"
    );

    let refusal = fs.read(&store_file).await;
    assert!(
        matches!(refusal, Err(VfsError::SecretDenied { .. })),
        "FAIL OPEN: a store under a symlinked control directory, created after \
         the memo was built, was READ: {refusal:?}"
    );
}

/// INVALIDATION — the store LEAF is a symlink whose target does not exist when
/// the scan runs.
///
/// The same family one level down. A leaf that does not resolve is deliberately
/// dropped rather than denied (a deny for a non-existent path is noise the
/// sandbox backend still carries), so before the link itself was stamped the
/// leaf left NO witness, and `<root>/.git`'s mtime does not move when a
/// directory is created somewhere else entirely.
#[tokio::test]
#[cfg(unix)]
async fn a_store_leaf_symlinked_to_a_later_created_directory_is_denied() {
    let dir = tempfile::tempdir().expect("workspace");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    let outside = tempfile::tempdir().expect("outside store");
    let outside_root = std::fs::canonicalize(outside.path()).expect("canonical outside");

    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::os::unix::fs::symlink(outside_root.join("objects"), root.join(".git/objects")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), b"fn main() {}\n").unwrap();

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    tokio::time::sleep(SETTLE).await;
    fs.exists(&root.join("src/main.rs"))
        .await
        .expect("ordinary path readable");
    let (_, scans, _) = policy.guard_cost();
    assert_eq!(scans, 1, "the memo must be warm for this to mean anything");

    let borrowed = outside_root.join("objects/ab/cdef");
    // CONTROL: nothing is denied yet, so the assertion below cannot pass by the
    // policy refusing the outside directory unconditionally.
    assert!(
        !policy.denies_read_content(&borrowed),
        "CONTROL: a directory that does not exist yet is not this workspace's \
         content store"
    );

    std::fs::create_dir_all(outside_root.join("objects/ab")).unwrap();
    std::fs::write(&borrowed, b"COMMITTED-SECRET\n").unwrap();

    assert!(
        policy.denies_read_content(&borrowed),
        "FAIL OPEN: a store leaf symlinked to a directory created after the \
         scan was not denied — the dangling link was dropped without a witness"
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
