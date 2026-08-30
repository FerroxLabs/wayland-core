//! FerroxLabs/wayland-core#390 — the stores a NESTED checkout's control files
//! NAME rather than contain, through the in-process VFS.
//!
//! `WorkspacePolicy::is_vcs_content_store` had two arms. Arm 1 is lexical at
//! any depth (`inside_vcs_store`): a `(control, store)` ANCESTOR PAIR such as
//! `.git/objects`. Arm 2 resolves what `<root>/.git` NAMES — a gitfile's
//! `gitdir:`/`commondir` and an `objects/info/alternates` borrow — and it reads
//! `<root>/.git` ONLY.
//!
//! A VENDORED checkout fell between them. `<root>/vendor/pkg/.git` is a gitfile
//! reading `gitdir: ../pkg-git`, so the real objects live at
//! `<root>/vendor/pkg-git/objects/**`: not a lexical `(control, store)` pair
//! (arm 1 misses), and never named by the root's own `.git` (arm 2 never
//! looks). `Grep(".")` covered it from #244 c3 because it traverses; `Read` did
//! not. Arm 3 closes that, by DISCOVERING the control directories nested under
//! the root and reading each with exactly the code that reads the root's.
//!
//! **Red arm.** Replace `WorkspacePolicy::nested_stores_memoized`'s body with
//! `Vec::new()` — it compiles, and it is the whole of arm 3's answer. Both
//! refusals below go red; both wrong-refusal controls stay green, which is what
//! separates "arm 3 does the work" from "the guard refuses more".
//!
//! Every refusal here carries its own wrong-refusal control in the same test.
//! Denying a vendored checkout's WORKING TREE would break ordinary session work
//! on exactly the repositories this is meant to protect, and a guard that
//! refused everything would satisfy the refusal assertions on its own.

use std::path::Path;
use std::sync::Arc;
use wcore_tools::vfs::{RealFs, SecretDenyFs, VfsError, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

fn write(path: &Path, body: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
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

/// **c1** — a VENDORED checkout whose `.git` is a gitfile. The store it names
/// is refused; that same checkout's working tree is not.
///
/// `<root>/vendor/pkg/.git` = `gitdir: ../pkg-git`, objects at
/// `<root>/vendor/pkg-git/objects/12/3456`. Note what makes this arm 3 and not
/// arm 1: `pkg-git` is not a control-directory name, so `pkg-git/objects` is
/// not a `(control, store)` pair and the lexical arm cannot see it.
#[tokio::test]
async fn a_vendored_gitfile_named_store_is_refused_through_the_vfs() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();

    let store_obj = root.join("vendor/pkg-git/objects/12/3456");
    write(&store_obj, b"\x78\x01zlib-blob");
    write(&root.join("vendor/pkg/.git"), b"gitdir: ../pkg-git\n");
    // The vendored checkout's WORKING TREE and its metadata — the wrong-refusal
    // controls. `git rev-parse` inside a vendored repo is ordinary work.
    let worktree = root.join("vendor/pkg/src/lib.rs");
    write(&worktree, b"pub fn f() {}\n");
    let head = root.join("vendor/pkg-git/HEAD");
    write(&head, b"ref: refs/heads/main\n");
    write(&root.join("main.rs"), b"fn main() {}\n");

    let fs = deny_fs(&root);

    // The predicate and the VFS must agree, or one of them is doing the work
    // alone. This is the same assertion `grep_vcs_named_store_deny.rs` carries
    // in its inverted form (c4) — the two layers re-tied from both sides.
    assert!(
        WorkspacePolicy::contained(&root).is_vcs_content_store(&store_obj),
        "core#390 c1: the point-predicate must call the vendored gitfile's \
         store a content store"
    );
    assert_refused(
        &fs,
        &store_obj,
        "core#390 c1: a VENDORED gitfile-named object store",
    )
    .await;

    assert_readable(
        &fs,
        &worktree,
        "wrong-refusal control: the vendored checkout's own working tree",
    )
    .await;
    assert_readable(
        &fs,
        &head,
        "wrong-refusal control: a vendored gitdir's HEAD is metadata, not content",
    )
    .await;
    assert_readable(
        &fs,
        &root.join("main.rs"),
        "wrong-refusal control: an ordinary workspace file",
    )
    .await;
}

/// **c2** — the same, for an `objects/info/alternates` borrow declared by a
/// NESTED checkout rather than by the workspace root.
///
/// `<root>/vendor/pkg/.git/objects/info/alternates` names
/// `<root>/borrowed/objects`, which is what `git clone --reference` writes.
/// `borrowed` is not a control-directory name, so `borrowed/objects` is not a
/// `(control, store)` pair and arm 1 cannot see it either; only the nested walk
/// reading that nested `alternates` file can.
#[tokio::test]
async fn a_nested_checkouts_alternates_borrow_is_refused_through_the_vfs() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();

    let borrowed = root.join("borrowed/objects/ab/cd1234");
    write(&borrowed, b"\x78\x01borrowed-blob");
    let nested_git = root.join("vendor/pkg/.git");
    std::fs::create_dir_all(nested_git.join("objects/info")).unwrap();
    std::fs::write(
        nested_git.join("objects/info/alternates"),
        // Relative to the nested `objects` dir, as git resolves it.
        "../../../../borrowed/objects\n",
    )
    .unwrap();
    write(&nested_git.join("HEAD"), b"ref: refs/heads/main\n");
    let worktree = root.join("vendor/pkg/src/lib.rs");
    write(&worktree, b"pub fn f() {}\n");
    write(&root.join("main.rs"), b"fn main() {}\n");

    let fs = deny_fs(&root);

    assert!(
        WorkspacePolicy::contained(&root).is_vcs_content_store(&borrowed),
        "core#390 c2: the point-predicate must call a NESTED checkout's \
         borrowed store a content store"
    );
    assert_refused(
        &fs,
        &borrowed,
        "core#390 c2: a store borrowed by a NESTED checkout's alternates",
    )
    .await;

    assert_readable(
        &fs,
        &worktree,
        "wrong-refusal control: the nested checkout's own working tree",
    )
    .await;
    assert_readable(
        &fs,
        &nested_git.join("HEAD"),
        "wrong-refusal control: a nested .git/HEAD is metadata, not content",
    )
    .await;
    assert_readable(
        &fs,
        &root.join("main.rs"),
        "wrong-refusal control: an ordinary workspace file",
    )
    .await;
}

/// The NAMED GAP in arm 3's gate, pinned rather than left to be discovered.
///
/// Arm 3 is reached only for a path that lexically carries a store-leaf name
/// (`objects`, `modules`, `lfs`, `store`, `pristine`, `repository`) among its
/// components — that gate is what keeps an ordinary path from paying for the
/// walk (core#376). A git control directory's own store leaves are FIXED names,
/// so a gitfile-named store cannot escape it. An `alternates` borrow CAN: the
/// entry may name any directory at all. This test records that this shape is
/// still admitted, so the day it is closed the test fails and is inverted the
/// way c4's was, rather than quietly agreeing with either answer.
///
/// Tracked as FerroxLabs/wayland-core#394.
#[tokio::test]
async fn a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();

    let borrowed = root.join("odb/ab/cd1234");
    write(&borrowed, b"\x78\x01borrowed-blob");
    let nested_git = root.join("vendor/pkg/.git");
    std::fs::create_dir_all(nested_git.join("objects/info")).unwrap();
    std::fs::write(
        nested_git.join("objects/info/alternates"),
        "../../../../odb\n",
    )
    .unwrap();

    let fs = deny_fs(&root);
    assert!(
        fs.read(&borrowed).await.is_ok(),
        "core#394: this shape is a KNOWN gap in arm 3's lexical gate. If it is \
         now refused, invert this assertion and close #394 — do not delete it"
    );
}

/// **The gap above is on core#390 c2's OWN axis, not beside it.**
///
/// c2 reads "the same holds for an `objects/info/alternates` borrow declared by
/// a NESTED checkout, not only by the workspace root". The axis it names is
/// root-versus-nested, and this test drives that axis with everything else held
/// fixed: the SAME borrow target, `<root>/odb`, named by the same relative
/// spelling, holding the same object — declared once by the root's `.git` and
/// once by a nested checkout's.
///
/// The root declaration is REFUSED and the nested one is ADMITTED
/// (`a_nested_alternates_borrow_named_nothing_store_like_is_still_admitted`,
/// one function up). Arm 2's store list is consulted with no lexical pre-gate,
/// so `push_store` puts a borrow target of ANY name into it; arm 3's is reached
/// only through `store_shaped`. That is a difference between root and nested,
/// which is precisely what c2 says must not exist — so c2 is graded `not-met`
/// with core#394 as its carrier rather than `met` with a footnote.
///
/// When #394 closes, this test and its sibling flip together: the sibling's
/// `is_ok` becomes a refusal and this pair becomes a symmetry assertion.
#[tokio::test]
async fn the_same_alternates_borrow_is_refused_at_the_root_and_admitted_when_nested() {
    async fn read_ok(spelling: &str, control: &Path) -> bool {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let borrowed = root.join("odb/ab/cd1234");
        write(&borrowed, b"\x78\x01borrowed-blob");
        let owner = root.join(control);
        std::fs::create_dir_all(owner.join("objects/info")).unwrap();
        std::fs::write(owner.join("objects/info/alternates"), spelling).unwrap();
        // Wrong-refusal control, in the same fixture: an ordinary workspace
        // file must stay readable in BOTH arms, or the difference below is a
        // guard that refuses everything rather than one that discriminates.
        let ordinary = root.join("main.rs");
        write(&ordinary, b"fn main() {}\n");
        let fs = deny_fs(&root);
        assert!(
            fs.read(&ordinary).await.is_ok(),
            "wrong-refusal control for `{}`: an ordinary workspace file must \
             stay readable",
            control.display()
        );
        fs.read(&borrowed).await.is_ok()
    }

    // `<root>/.git/objects` + `../../odb` and
    // `<root>/vendor/pkg/.git/objects` + `../../../../odb` both resolve to
    // `<root>/odb`. Same target, same object, same VFS: only the DECLARING
    // control directory differs.
    let at_root = read_ok("../../odb\n", Path::new(".git")).await;
    let when_nested = read_ok("../../../../odb\n", Path::new("vendor/pkg/.git")).await;

    assert!(
        !at_root,
        "control: a borrow declared by the ROOT must be refused whatever the \
         target is named — if this is now admitted, arm 2 has regressed and \
         the asymmetry below is not the one core#394 is about"
    );
    assert!(
        when_nested,
        "core#390 c2 / core#394: the SAME borrow declared by a nested checkout \
         is admitted while the root declaration is refused. When #394 closes, \
         invert this to `!when_nested` and grade c2 `met` — do not delete it"
    );
}
