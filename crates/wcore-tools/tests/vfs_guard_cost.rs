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
//! FerroxLabs/wayland-core#390 re-measured the OTHER call shape this function
//! has — `grep_policy::scope_for` calls `vcs_content_stores(dir)` once per
//! TRAVERSED DIRECTORY of a `Grep(".")`, where no memo applies because the root
//! is different every time. Same instrument, differential `strace -f -c` over
//! the `probe_vcs_content_stores_per_traversed_directory` loop at
//! `WL_PROBE_DIRS=100` and `1100`, hetzner:
//!
//! Every figure below is TOTAL syscalls charged per traversed directory — not
//! the `statx` sub-count — differenced over three operation counts (100 / 1100
//! / 2100), `1 passed` asserted in every run, identical across two repetitions:
//!
//! ```text
//!  4c55f5ac6  origin/integ/f13 (before StoreScan)   8 / dir = 7 statx + 1 openat
//!  1b9cb34d5  + lane/f13-sec-secrets (StoreScan)   17 / dir = 16 statx + 1 openat
//!  875bf32cb  + this file's arm-3 change (#390)     5 / dir = 5 statx
//! ```
//!
//! Naming the unit is not pedantry: this header and `witness_if_present`'s
//! doc comment carried 17 and 16 for the same measurement, because one quoted
//! the total and the other the `statx` sub-count. Both now quote the TOTAL, and
//! the claim is checkable in the tree rather than only in a ledger note —
//! `grep -n "17 total" crates/wcore-tools/src/workspace_policy.rs`.
//!
//! **UNRESOLVED, and recorded rather than reconciled away.** A third,
//! independently built measurement of the same middle arm returned 16 TOTAL
//! where the two above return 17. Base (8) and head (5) agree exactly across
//! all three sets, so it is not an instrument-wide offset and the unit
//! explanation does not cover it. It is not grade-bearing — every measurement
//! agrees the drop is TO 5, which is the figure `scan_control_dirs_in` claims —
//! but a one-syscall disagreement on one arm of a differential measurement is
//! left visible here rather than argued into agreement.
//!
//! The middle row is a real 2.1x regression on a shape NEITHER lane measured:
//! #376's memo made the ordinary GUARD path cheaper while making the untouched
//! per-directory path twice as expensive, because `StoreScan`'s witness
//! bookkeeping (`symlink_metadata` per probed leaf, link-target stamping)
//! runs whether or not anything is found. `scan_control_dirs_in` closes it by
//! not probing store leaves under a control directory that is absent — which is
//! every directory of an ordinary tree — and lands BELOW the pre-StoreScan
//! figure.
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
    assert_eq!(
        scans, 2,
        "the first guard scans TWICE: the root scan, plus arm 3's nested walk. \
         core#390 c2 made the arm-3 gate a function of that walk's own output, \
         and a gate cannot consult an output that does not exist yet — so the \
         walk runs ONCE per policy, on whichever guard comes first. Every \
         assertion below is about the steady state, which is what c3's number \
         is about"
    );
    assert_eq!(
        first_probes, FIRST_GUARD_PROBES,
        "the store scan's filesystem probe count moved; if that is intended, \
         update this number and re-measure the syscall figures in this file's \
         header. See `FIRST_GUARD_PROBES` for what the two halves are"
    );

    const N: u64 = 50;
    for _ in 0..N {
        fs.exists(&ordinary).await.expect("ordinary path readable");
    }
    let (resolves, scans, probes) = policy.guard_cost();
    assert_eq!(resolves, N + 1, "still exactly one resolution per guard");
    assert_eq!(
        scans, 2,
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
        scans_before, 2,
        "the memos must be warm for this to mean anything: arm 2's root scan AND arm 3's one-off walk, both of which a policy pays once (core#390 c2). A third scan here would mean the memo is missing and this arm grades a rescan rather than a cache."
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
    assert_eq!(
        scans, 2,
        "the memos must be warm for this to mean anything: arm 2's root scan AND arm 3's one-off walk, both of which a policy pays once (core#390 c2). A third scan here would mean the memo is missing and this arm grades a rescan rather than a cache."
    );

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
    assert_eq!(
        scans, 2,
        "the memos must be warm for this to mean anything: arm 2's root scan AND arm 3's one-off walk, both of which a policy pays once (core#390 c2). A third scan here would mean the memo is missing and this arm grades a rescan rather than a cache."
    );

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

/// The probes ONE cold guard on the `fixture()` tree charges: 12 for the root
/// scan (17 before core#390, when the five store leaves under an absent control
/// directory were still probed) plus 31 for arm 3's one-off walk of this
/// fixture's four directories. Named rather than repeated so the two tests that
/// pin it cannot drift apart.
const FIRST_GUARD_PROBES: u64 = 43;

/// FerroxLabs/wayland-core#390 c3 — arm 3's walk is paid ONCE, not per
/// operation.
///
/// c3 reads: "Whatever caching the fix introduces is measured against #376's
/// complaint: the per-operation cost of `is_vcs_content_store` does not get
/// worse than it is today, stated as a number." The number is the one
/// `one_ordinary_path_guard_resolves_once_and_does_not_rescan` pins — one
/// resolution, one scan, three warm probes — and this test is what keeps arm 3
/// from moving it.
///
/// The number c3 is about is the PER-OPERATION one, and the walk is not on it:
/// arm 3's gate is decided by the walk's own output (core#390 c2), so the walk
/// runs once per policy and every guard after it costs the same three warm
/// probes an ordinary path cost before arm 3 existed. This grades the steady
/// state as a difference over N guards, which is exactly the quantity a
/// one-off cannot inflate.
///
/// The second half is the KNOWN-POSITIVE CONTROL, in the same test: the ONE
/// walk that the first ordinary guard paid for is the one that finds the
/// vendored store, so the store is refused with NO further scan. Without it an
/// arm 3 that never ran at all would pass the first half, and this test would
/// be pinning the absence of a feature rather than the shape of its cost.
#[tokio::test]
async fn an_ordinary_path_pays_for_the_nested_store_walk_at_most_once() {
    let (_dir, root) = fixture();
    // A vendored checkout, so the walk has something to find and cannot be
    // trivially cheap.
    std::fs::create_dir_all(root.join("vendor/pkg-git/objects/12")).unwrap();
    std::fs::write(root.join("vendor/pkg-git/objects/12/3456"), b"blob").unwrap();
    std::fs::create_dir_all(root.join("vendor/pkg")).unwrap();
    std::fs::write(root.join("vendor/pkg/.git"), b"gitdir: ../pkg-git\n").unwrap();

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    let ordinary = root.join("src/deep/deeper/main.rs");
    tokio::time::sleep(SETTLE).await;

    fs.exists(&ordinary).await.expect("ordinary path readable");
    let (_, scans_after_first, probes_after_first) = policy.guard_cost();
    assert_eq!(
        scans_after_first, 2,
        "the first guard scans TWICE — the root scan plus arm 3's one-off walk"
    );

    const N: u64 = 20;
    for _ in 0..N {
        fs.exists(&ordinary).await.expect("ordinary path readable");
    }
    let (_, scans, probes) = policy.guard_cost();
    assert_eq!(
        scans, 2,
        "core#390 c3: after the one-off walk, an ordinary-path guard must \
         never reach the nested store walk again — the cost c3 bounds is the \
         per-operation one"
    );
    assert_eq!(
        probes - probes_after_first,
        N * 3,
        "core#390 c3: an ordinary-path guard still costs exactly three warm \
         probes with arm 3 present"
    );

    // KNOWN-POSITIVE CONTROL: the ONE walk those guards paid for is the walk
    // that finds the vendored store, so the store is refused and NO further
    // scan is needed to do it. A gate that never opens proves nothing about
    // what it is gating, and a walk whose result is thrown away proves nothing
    // about the one-off being useful.
    let vendored = root.join("vendor/pkg-git/objects/12/3456");
    assert!(
        matches!(
            fs.read(&vendored).await,
            Err(wcore_tools::vfs::VfsError::SecretDenied { .. })
        ),
        "control: the vendored store must be refused, or the guards above are \
         paying for a walk that finds nothing"
    );
    let (_, scans_with_walk, _) = policy.guard_cost();
    assert_eq!(
        scans_with_walk, scans,
        "control: the refusal must come from the walk already paid for \
         (scans {scans} -> {scans_with_walk}) — a rescan here would mean the \
         one-off is not actually memoised"
    );
}

/// FerroxLabs/wayland-core#398 — **what the arm-3 gate ADMITS costs, measured
/// as a slope rather than pinned as a constant.**
///
/// `store_shaped` is a lexical any-component test over the store LEAF names in
/// `VCS_CONTENT_STORES` — `objects`, `modules`, `lfs`, `store`, `pristine`,
/// `repository`. Every one of those is also an ordinary project directory name
/// (a Terraform `modules/`, a Redux `store/`, an asset `objects/`), so a
/// workspace containing no nested checkout anywhere still has paths that open
/// the gate. Each guard on such a path revalidates the nested walk's witness
/// set, and the walk stamps ONE witness per directory it descended.
///
/// So the admitted path's cost is not a constant and must not be pinned as one.
/// It is graded as a SLOPE: two workspaces differing by `EXTRA` ordinary
/// directories must differ by exactly `EXTRA` warm probes on the admitted path.
/// The NON-admitted path staying at three in both is the known-positive
/// control — it proves the difference is the gate's doing and not the fixture's.
///
/// This is deliberately the whole CLASS and not the `modules` instance: nothing
/// here asks whether a particular directory name is a false positive, which is
/// undecidable over the open alphabet of project layouts. It asks whether an
/// admitted path's cost depends on workspace size, which is decidable and
/// total. A fix that makes arm 3 O(1) in the workspace turns the slope to zero
/// and reddens this test — that is the intended way for it to go red.
#[tokio::test]
async fn a_gate_admitted_path_costs_one_probe_per_workspace_directory() {
    /// Warm probes charged to ONE guard on the gate-admitted path and to ONE
    /// guard on the ordinary path, in a workspace carrying `extra` ordinary
    /// directories. Both memos are warm before either measurement starts.
    async fn warm_cost(extra: usize) -> (u64, u64) {
        let dir = tempfile::tempdir().expect("workspace");
        let root = std::fs::canonicalize(dir.path()).expect("canonical root");
        std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
        std::fs::write(root.join(".git/objects/ab/cdef"), b"x").unwrap();
        std::fs::create_dir_all(root.join("src/deep/deeper")).unwrap();
        std::fs::write(root.join("src/deep/deeper/main.rs"), b"fn main() {}\n").unwrap();
        // A BENIGN directory whose name is a store leaf: no control directory,
        // no gitfile, no store — an ordinary Terraform layout.
        std::fs::create_dir_all(root.join("modules/vpc")).unwrap();
        std::fs::write(root.join("modules/vpc/main.tf"), b"# terraform\n").unwrap();
        for i in 0..extra {
            std::fs::create_dir_all(root.join(format!("pkg{i}"))).unwrap();
        }

        let policy = Arc::new(WorkspacePolicy::contained(&root));
        let fs = stack(&policy, &root);
        let admitted = root.join("modules/vpc/main.tf");
        let ordinary = root.join("src/deep/deeper/main.rs");
        tokio::time::sleep(SETTLE).await;

        // Warm both memos before measuring anything.
        fs.exists(&ordinary).await.expect("ordinary path readable");
        fs.exists(&admitted).await.expect("admitted path readable");
        let (_, scans_warm, before) = policy.guard_cost();

        fs.exists(&admitted).await.expect("admitted path readable");
        let (_, scans_mid, mid) = policy.guard_cost();
        fs.exists(&ordinary).await.expect("ordinary path readable");
        let (_, scans_end, after) = policy.guard_cost();

        // A rescan would inflate the probe count with cold-scan work and the
        // slope would stop meaning what it says. Assert the memos held.
        assert_eq!(
            (scans_warm, scans_mid),
            (scans_end, scans_end),
            "a memo rebuilt during the warm measurement — the figures below \
             would be cold-scan cost, not revalidation cost"
        );
        (mid - before, after - mid)
    }

    const SMALL: usize = 8;
    const EXTRA: usize = 40;

    let (admitted_small, ordinary_small) = warm_cost(SMALL).await;
    let (admitted_large, ordinary_large) = warm_cost(SMALL + EXTRA).await;
    println!(
        "GATE COST: dirs={SMALL} admitted={admitted_small} ordinary={ordinary_small} | \
         dirs={} admitted={admitted_large} ordinary={ordinary_large}",
        SMALL + EXTRA
    );

    // CONTROL: the path the gate REFUSES is flat. #376's number is untouched.
    assert_eq!(
        (ordinary_small, ordinary_large),
        (3, 3),
        "control: a path the arm-3 gate does not admit must still cost exactly \
         three warm probes at any workspace size — if this moved, the slope \
         below is measuring the fixture and not the gate"
    );

    assert_eq!(
        admitted_large - admitted_small,
        EXTRA as u64,
        "core#398: a guard on a path the arm-3 gate admits costs ONE filesystem \
         probe per directory in the workspace ({admitted_small} at {SMALL} \
         directories, {admitted_large} at {}). `store_shaped` opens on ordinary \
         project directory names, so this is what an ordinary read of \
         `modules/vpc/main.tf` pays. If this difference is now zero the cost has \
         been made independent of workspace size and core#398 is closed — delete \
         nothing, invert this assertion.",
        SMALL + EXTRA
    );
}
