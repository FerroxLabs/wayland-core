//! FerroxLabs/wayland-core#390, #394, #396, #406 — **a VCS content store
//! nested under the workspace root is refused through the in-process VFS,
//! whatever it is called and whichever control file names it.**
//!
//! One root cause behind four tickets: `is_vcs_content_store` decided from a
//! NAME. Arm 1 read the query path's spelling, arm 2 read what `<root>/.git`
//! names, and a store that neither could name was handed back in full. The
//! shapes below are the ones that were measured admitted, each with the
//! wrong-refusal control that keeps a blanket refusal from passing.
//!
//! The stack is the production one — `SandboxedFs ∘ SecretDenyFs ∘ RealFs`
//! over `WorkspacePolicy::contained` — because the guard is what is under
//! test, not the predicate. Driving `is_vcs_content_store` directly would grade
//! a helper the production path might not call.
//!
//! **Red arm** (compiled first, `cargo check -p wcore-tools --tests` clean):
//! delete the arm-3 and arm-4 branches from
//! `WorkspacePolicy::is_vcs_content_store_resolved` and every refusal here goes
//! red with the canary in the returned bytes, while every control stays green.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::grep::GrepTool;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

/// A scan's answer is trusted only once its witnesses' mtimes lag the scan's
/// own start instant by more than one filesystem tick (#1145), so a fixture
/// built microseconds ago is deliberately NOT settled and every guard rescans.
/// Reaching the steady state is part of what the post-walk tests below are
/// about, not a flake mitigation.
const SETTLE: Duration = Duration::from_millis(60);

const CANARY: &str = "WLCANARY-NESTED-STORE";
const CONTROL: &str = "WLCANARY-CONTROL-OK";

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("{body}\n")).unwrap();
}

/// The layout `git` itself requires of a repository top level: `HEAD` plus
/// `refs`, which is what a bare repo and a gitfile's gitdir both have.
fn repository_shape(dir: &Path) {
    write(&dir.join("HEAD"), "ref: refs/heads/main");
    std::fs::create_dir_all(dir.join("refs/heads")).unwrap();
    write(&dir.join("config"), "[core]\n\trepositoryformatversion = 0");
}

fn stack(policy: &Arc<WorkspacePolicy>, root: &Path) -> SandboxedFs<SecretDenyFs<RealFs>> {
    SandboxedFs::new(
        SecretDenyFs::new(RealFs, Arc::clone(policy)),
        root.to_path_buf(),
    )
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    // The wrong-refusal control, present in every fixture: an ordinary source
    // file must still be readable, or a stack that refuses everything passes
    // every assertion below.
    write(&root.join("src/main.rs"), &format!("// {CONTROL}"));
    (dir, root)
}

/// `read` through the production stack, as a result the assertions can name.
async fn read(fs: &SandboxedFs<SecretDenyFs<RealFs>>, path: &Path) -> Result<String, String> {
    match fs.read(path).await {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(err) => Err(err.to_string()),
    }
}

async fn assert_control_readable(fs: &SandboxedFs<SecretDenyFs<RealFs>>, root: &Path, arm: &str) {
    let out = read(fs, &root.join("src/main.rs")).await;
    assert!(
        out.as_deref().is_ok_and(|body| body.contains(CONTROL)),
        "wrong-refusal control ({arm}): an ordinary source file must still be \
         readable, or the refusal under test proves nothing. Got: {out:?}"
    );
}

async fn assert_store_refused(fs: &SandboxedFs<SecretDenyFs<RealFs>>, object: &Path, ticket: &str) {
    let out = read(fs, object).await;
    assert!(
        out.is_err(),
        "{ticket}: {} was handed back through the VFS. Got: {out:?}",
        object.display()
    );
    assert!(
        !out.unwrap_or_default().contains(CANARY),
        "{ticket}: the canary reached the caller",
    );
}

/// core#390 c1 — a VENDORED checkout whose `.git` is a gitfile.
///
/// `<root>/vendor/pkg/.git` is the FILE `gitdir: ../pkg-git`; the object lives
/// at `<root>/vendor/pkg-git/objects/12/3456`, which is not lexically a
/// `(control, store)` pair and is not named by `<root>/.git`. Measured
/// admitted at `integ/f13`.
#[tokio::test]
async fn a_vendored_gitfiles_object_store_is_refused() {
    let (_dir, root) = workspace();
    let object = root.join("vendor/pkg-git/objects/12/3456");
    write(&object, CANARY);
    std::fs::create_dir_all(root.join("vendor/pkg")).unwrap();
    std::fs::write(root.join("vendor/pkg/.git"), "gitdir: ../pkg-git\n").unwrap();
    write(
        &root.join("vendor/pkg/README.md"),
        &format!("{CONTROL} pkg"),
    );

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);

    assert_store_refused(&fs, &object, "core#390 c1").await;
    assert_control_readable(&fs, &root, "vendored gitfile").await;
    let checkout = read(&fs, &root.join("vendor/pkg/README.md")).await;
    assert!(
        checkout.as_deref().is_ok_and(|b| b.contains(CONTROL)),
        "core#390 c1 names its own control: THAT checkout's working tree must \
         stay readable. Got: {checkout:?}"
    );
    assert!(
        policy.is_vcs_content_store(&object),
        "the point predicate and the guard must agree about the same path"
    );
}

/// core#390 c2 + core#394 c1 — an `objects/info/alternates` borrow declared by
/// a NESTED checkout, whose target is named nothing store-like.
///
/// This is the criterion as WRITTEN: refused REGARDLESS of the borrow target's
/// directory name. `<root>/odb` carries no `VCS_CONTENT_STORES` component, so
/// no lexical test of the query path can reach it; the walk resolves the borrow
/// eagerly and the set is tested by prefix, which is why the target's name
/// never enters the decision.
#[tokio::test]
async fn a_nested_alternates_borrow_is_refused_whatever_its_target_is_named() {
    let (_dir, root) = workspace();
    let object = root.join("odb/ab/cd1234");
    write(&object, CANARY);
    std::fs::create_dir_all(root.join("vendor/pkg/.git/objects/info")).unwrap();
    write(&root.join("vendor/pkg/.git/HEAD"), "ref: refs/heads/main");
    write(
        &root.join("vendor/pkg/.git/objects/info/alternates"),
        "../../../../odb",
    );
    write(
        &root.join("vendor/pkg/README.md"),
        &format!("{CONTROL} pkg"),
    );

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);

    assert_store_refused(&fs, &object, "core#394 c1").await;
    assert_control_readable(&fs, &root, "nested alternates").await;
    let checkout = read(&fs, &root.join("vendor/pkg/README.md")).await;
    assert!(
        checkout.as_deref().is_ok_and(|b| b.contains(CONTROL)),
        "core#394 c1: the declaring checkout's working tree stays readable. \
         Got: {checkout:?}"
    );
}

/// core#394, second route — a `.git/objects` SYMLINK on a nested checkout,
/// pointing at a directory named nothing store-like.
///
/// Reported on #394 as the route that needs no hand-written file at all: `git`
/// supports it and pointing an object database at another filesystem that way
/// is ordinary practice. Read here by the RESOLVED path, which is the harder
/// arm — reaching it through the symlink's own name is answered lexically.
// UNIX-ONLY, and the gate is the fix for a BUILD break, not a test failure:
// `std::os::unix::fs::symlink` does not exist on Windows, so without this the
// whole `vfs_nested_store_deny` target fails to compile and `nextest` can run
// NO wcore-tools test on Windows at all. Found by cross-checking
// `--target x86_64-pc-windows-gnu`, after a real Windows run died here before
// reaching a single test.
//
// Ported rather than gated would be wrong: Windows has symlinks, but creating
// one needs SeCreateSymbolicLinkPrivilege unless Developer Mode is on, so the
// ported arm would fail on an ordinary Windows host for a reason unrelated to
// the property under test. The coverage loss is stated here rather than
// silently taken -- every other arm in this file is platform-neutral and still
// grades the nested-store refusal on Windows.
#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_nested_store_leaf_is_refused_by_its_resolved_path() {
    let (_dir, root) = workspace();
    let object = root.join("odb-link/ab/cd1234");
    write(&object, CANARY);
    std::fs::create_dir_all(root.join("vendor/pkg/.git")).unwrap();
    write(&root.join("vendor/pkg/.git/HEAD"), "ref: refs/heads/main");
    std::os::unix::fs::symlink(root.join("odb-link"), root.join("vendor/pkg/.git/objects"))
        .unwrap();

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);

    assert_store_refused(&fs, &object, "core#394 (symlinked leaf)").await;
    assert_store_refused(
        &fs,
        &root.join("vendor/pkg/.git/objects/ab/cd1234"),
        "core#394 (symlinked leaf, by its own name)",
    )
    .await;
    assert_control_readable(&fs, &root, "symlinked leaf").await;
}

/// core#396 c1 — a BARE repository vendored under the root, in BOTH spellings.
///
/// A bare repo has no control directory at all: `objects/`, `HEAD` and `refs/`
/// sit at its top level. Arm 1 cannot see it (`objects`'s parent is `pkg.git`,
/// not `.git`), arm 2 never looks there, and the `.git` suffix is a convention
/// rather than a requirement — so the suffix-less spelling is graded too.
#[tokio::test]
async fn a_bare_repository_vendored_under_the_root_is_refused() {
    let (_dir, root) = workspace();
    for name in ["vendor/pkg.git", "vendor/mirror"] {
        let repo = root.join(name);
        repository_shape(&repo);
        write(&repo.join("objects/ab/cd1234"), CANARY);
    }
    write(&root.join("vendor/notes.md"), &format!("{CONTROL} notes"));

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);

    for name in ["vendor/pkg.git", "vendor/mirror"] {
        assert_store_refused(
            &fs,
            &root.join(name).join("objects/ab/cd1234"),
            "core#396 c1",
        )
        .await;
        // A bare repository's own refs and HEAD carry no content and stay
        // readable, mirroring the `git rev-parse` carve-out the root
        // repository gets.
        assert!(
            read(&fs, &root.join(name).join("HEAD")).await.is_ok(),
            "core#396 c1: HEAD carries no committed content and must stay \
             readable ({name})"
        );
    }
    assert_control_readable(&fs, &root, "bare repository").await;
    let sibling = read(&fs, &root.join("vendor/notes.md")).await;
    assert!(
        sibling.as_deref().is_ok_and(|b| b.contains(CONTROL)),
        "core#396 c1 names its own control: the repository's sibling \
         working-tree files stay readable. Got: {sibling:?}"
    );
}

/// core#396 c2 — the NEGATIVE control, in the same shape as the fix.
///
/// An ordinary directory that merely CONTAINS a subdirectory named `objects`
/// (and no `HEAD`/`refs`) must stay readable, so the fix cannot be a bare
/// `objects`-component match. Graded for every store leaf name, not only
/// `objects`: `modules`, `store`, `lfs`, `pristine` and `repository` are all
/// ordinary project directory names — a Terraform `modules/`, a Redux `store/`.
#[tokio::test]
async fn an_ordinary_directory_named_like_a_store_is_not_a_repository() {
    let (_dir, root) = workspace();
    for leaf in [
        "objects",
        "modules",
        "store",
        "lfs",
        "pristine",
        "repository",
    ] {
        write(
            &root.join("app").join(leaf).join("index.ts"),
            &format!("export const x = '{CONTROL}';"),
        );
    }

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);

    for leaf in [
        "objects",
        "modules",
        "store",
        "lfs",
        "pristine",
        "repository",
    ] {
        let path = root.join("app").join(leaf).join("index.ts");
        let out = read(&fs, &path).await;
        assert!(
            out.as_deref().is_ok_and(|b| b.contains(CONTROL)),
            "core#396 c2: `{}` is ordinary user data, not a content store — a \
             name-only match would refuse it. Got: {out:?}",
            path.display()
        );
        assert!(
            !policy.is_vcs_content_store(&path),
            "core#396 c2: the point predicate must agree that {leaf} is ordinary"
        );
    }
    assert_control_readable(&fs, &root, "negative control").await;
}

/// core#396 c4 — `Grep` and the VFS give the SAME verdict for the same object.
///
/// The two layers drifted once already (core#244): `Grep` spawns `rg` outside
/// the VFS and outside the OS sandbox, so a predicate the VFS grew did not
/// reach it. `grep_policy` asks this policy's `denies_read_content`, which is
/// the same conjunction the guard asks — this test is what holds them tied.
#[tokio::test]
async fn grep_and_the_vfs_agree_about_a_bare_repository_object() {
    let (_dir, root) = workspace();
    let repo = root.join("vendor/pkg.git");
    repository_shape(&repo);
    write(&repo.join("objects/ab/cd1234"), CANARY);

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    let object = repo.join("objects/ab/cd1234");
    assert_store_refused(&fs, &object, "core#396 c4 (vfs)").await;

    let mut ctx = ToolContext::test_default();
    ctx.vfs = Arc::new(stack(&policy, &root));
    let ctx = ctx.with_workspace(Arc::clone(&policy));
    let out = GrepTool
        .execute_with_ctx(json!({ "pattern": "WLCANARY", "path": "." }), &ctx)
        .await
        .content;
    assert!(
        out.contains(CONTROL),
        "core#396 c4 positive control: Grep must still return the ordinary \
         file's match, or its silence proves nothing. Output:\n{out}"
    );
    assert!(
        !out.contains(CANARY),
        "core#396 c4: the VFS refuses this object and Grep returned it in \
         plaintext — the two layers disagree. Output:\n{out}"
    );
}

/// core#406 c1 — a repository that comes into being AFTER the first guard is
/// refused on the next one.
///
/// `WorkspacePolicy` is built once in `bootstrap.rs` and `Arc`-cloned into
/// `SecretDenyFs` for the life of the session, so a cold-memo answer is an
/// answer the session keeps giving for hours. Arm 3 reads the filesystem at
/// the instant it is asked and memoises nothing, which is what makes this
/// refusal available at all — arm 4's one-shot walk cannot see it by
/// construction.
#[tokio::test]
async fn a_bare_repository_created_after_the_first_guard_is_refused_on_the_next() {
    let (_dir, root) = workspace();
    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);

    // Warm every memo and run the one-shot walk BEFORE the store exists: this
    // is the post-warm state the brief says to grade, not the cold one.
    assert_control_readable(&fs, &root, "before creation").await;
    let object = root.join("vendor/late.git/objects/ab/cd1234");
    assert!(
        read(&fs, &object).await.is_err(),
        "fixture control: the object does not exist yet"
    );

    let repo = root.join("vendor/late.git");
    repository_shape(&repo);
    write(&object, CANARY);

    assert_store_refused(&fs, &object, "core#406 c1").await;
    assert_control_readable(&fs, &root, "after creation").await;
}

/// core#406, the RESIDUAL this design does not close — recorded as a
/// measurement so it is graded rather than rediscovered.
///
/// A borrow written AFTER arm 4's one-shot walk, naming a target that carries
/// no store leaf component, is seen by no arm: arm 1 and arm 3 read the query
/// path's own ancestry and `<root>/late-odb/ab/cd1234` has nothing in it to
/// see, and arm 4 does not revalidate. Closing it costs at least one
/// filesystem probe on the REFUSE branch of every guard, which is the number
/// core#398 pins at zero.
///
/// When that trade is taken, this assertion is INVERTED rather than deleted,
/// so the gap and its closure are graded by the same test.
#[tokio::test]
async fn a_borrow_written_after_the_walk_at_a_non_store_shaped_target_is_refused() {
    let (_dir, root) = workspace();
    std::fs::create_dir_all(root.join("vendor/pkg/.git/objects/info")).unwrap();
    write(&root.join("vendor/pkg/.git/HEAD"), "ref: refs/heads/main");

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    tokio::time::sleep(SETTLE).await;
    // Warm every memo AND run arm 4's walk BEFORE the borrow exists, so this
    // grades the post-walk state rather than a cold policy that would have
    // discovered the store anyway.
    assert_control_readable(&fs, &root, "before the borrow").await;
    assert_eq!(
        policy.nested_walk_count(),
        1,
        "the walk must have run before the borrow is written, or this test \
         grades a cold policy and proves nothing about staleness"
    );

    let object = root.join("late-odb/ab/cd1234");
    write(&object, CANARY);
    write(
        &root.join("vendor/pkg/.git/objects/info/alternates"),
        "../../../../late-odb",
    );

    assert_store_refused(&fs, &object, "core#406 c1").await;
    assert_control_readable(&fs, &root, "after the borrow").await;
}

/// core#406 c1, the REMAINDER — **stated as a measurement rather than left to
/// be rediscovered.**
///
/// The witness set arm 4 revalidates is the DECLARATION SITES it read, which is
/// O(nested checkouts) and is what keeps core#398 c1's slope at zero and c2's
/// three warm probes intact. A control directory that did not exist when the
/// walk ran therefore has no declaration site in that set, so a borrow it
/// declares at a target which is neither repository-shaped (arm 3) nor
/// lexically a store (arm 1) is still admitted.
///
/// Seeing this needs a witness per DESCENDED DIRECTORY — one `stat` per
/// workspace directory on every guard, which is exactly the regression core#398
/// records and core#398 c1 forbids. The two criteria are in direct tension and
/// the tension is priced here rather than traded silently.
///
/// Strictly smaller than what it replaced: the same fixture with the control
/// directory PRESENT at walk time is refused by the test above.
#[tokio::test]
async fn a_borrow_declared_by_a_control_dir_created_after_the_walk_is_still_admitted() {
    let (_dir, root) = workspace();

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let fs = stack(&policy, &root);
    tokio::time::sleep(SETTLE).await;
    assert_control_readable(&fs, &root, "before the checkout").await;
    assert_eq!(policy.nested_walk_count(), 1, "the walk must have run");

    // The whole checkout appears after the walk.
    std::fs::create_dir_all(root.join("vendor/late/.git/objects/info")).unwrap();
    write(&root.join("vendor/late/.git/HEAD"), "ref: refs/heads/main");
    let object = root.join("late-odb/ab/cd1234");
    write(&object, CANARY);
    write(
        &root.join("vendor/late/.git/objects/info/alternates"),
        "../../../../late-odb",
    );

    let out = read(&fs, &object).await;
    assert!(
        out.as_deref().is_ok_and(|b| b.contains(CANARY)),
        "if this now REFUSES, the remainder of core#406 c1 has been closed \
         too: invert this assertion, re-measure the refuse-branch probe count \
         core#398 c1/c2 pin, and re-grade both tickets. Got: {out:?}"
    );
}
