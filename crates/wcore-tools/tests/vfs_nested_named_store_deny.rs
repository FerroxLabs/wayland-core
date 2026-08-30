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

/// The gap that WAS arm 3's lexical gate, now closed and asserted from the
/// other side.
///
/// Arm 3 used to be reached only for a path that lexically carried a store-leaf
/// name (`objects`, `modules`, `lfs`, `store`, `pristine`, `repository`) among
/// its components. An `alternates` borrow escapes that: the entry may name any
/// directory at all, and `push_store` canonicalizes, so the store landed in
/// arm 3's list under a name the gate could not see. INVERTED, not deleted,
/// exactly as this test's own failure message instructed: the gate is now
/// decided by the scan's own output
/// (`WorkspacePolicy::nested_walk_admits`), so the borrow target is refused
/// whatever it is called.
///
/// FerroxLabs/wayland-core#390 c2 / #394.
#[tokio::test]
async fn a_nested_alternates_borrow_named_nothing_store_like_is_refused() {
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

    // Wrong-refusal control, asserted first and in the same fixture: an
    // ordinary workspace file must stay readable, or the refusal below is a
    // guard that refuses everything rather than one that discriminates.
    let ordinary = root.join("main.rs");
    write(&ordinary, b"fn main() {}\n");
    let fs = deny_fs(&root);
    assert!(
        fs.read(&ordinary).await.is_ok(),
        "wrong-refusal control: an ordinary workspace file must stay readable"
    );
    assert!(
        fs.read(&borrowed).await.is_err(),
        "core#390 c2: a store borrowed by a NESTED checkout's alternates must \
         be refused however the borrow target is named. If this is readable \
         again the arm-3 gate has gone back to grading the query path's \
         spelling"
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
/// Both declarations are now REFUSED, and the assertion is the SYMMETRY
/// itself. It used to be an asymmetry: arm 2's store list is consulted with no
/// lexical pre-gate, so `push_store` put a borrow target of ANY name into it,
/// while arm 3's list was reachable only through `store_shaped`. That
/// difference between root and nested is precisely what c2 says must not
/// exist, and it is what held c2 at `not-met` with core#394 as its carrier.
///
/// INVERTED, not deleted, as the previous version instructed. The
/// wrong-refusal control in each arm is what keeps the symmetry from being
/// satisfied by a guard that refuses everything.
#[tokio::test]
async fn the_same_alternates_borrow_is_refused_from_either_declaring_control_dir() {
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
         the symmetry below is being graded by two broken arms"
    );
    assert!(
        !when_nested,
        "core#390 c2: the SAME borrow declared by a NESTED checkout must be \
         refused exactly as the root declaration is. c2's axis is \
         root-versus-nested and this is that axis with everything else held \
         fixed — if this is admitted again, the two arms are answered by \
         predicates that disagree"
    );
}

/// **The N+1 the `alternates` pair above did NOT cover, on the same axis.**
///
/// `store_shaped`'s doc used to say a control directory's own store leaves are
/// FIXED names, so a gitfile-named store could not escape the gate. They are
/// fixed as WRITTEN; `push_store` CANONICALIZES before it stores. A `.git`
/// whose `objects` leaf is a SYMLINK — which `git` supports, and which is how
/// an object database is routinely put on another filesystem — therefore enters
/// the arm-3 store list under its TARGET's name, and the gate is applied to the
/// query path.
///
/// Same construction as
/// `the_same_alternates_borrow_is_refused_from_either_declaring_control_dir`:
/// the SAME target `<root>/odb`, the same object, only the DECLARING control
/// directory differs. Both are now refused. This is the N+1 that proved the
/// gap was a CLASS and not the `alternates` instance — core#394's scope note
/// said the shape needed a hand-written `alternates` entry, and it does not,
/// which is why the fix had to stop grading the query path's spelling
/// altogether.
#[cfg(unix)]
#[tokio::test]
async fn the_same_symlinked_store_leaf_is_refused_from_either_declaring_control_dir() {
    async fn read_ok(control: &Path) -> bool {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let borrowed = root.join("odb/ab/cd1234");
        write(&borrowed, b"\x78\x01borrowed-blob");
        let owner = root.join(control);
        std::fs::create_dir_all(&owner).unwrap();
        std::os::unix::fs::symlink(root.join("odb"), owner.join("objects")).unwrap();
        // Wrong-refusal control, in the same fixture and asserted first.
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

    let at_root = read_ok(Path::new(".git")).await;
    let when_nested = read_ok(Path::new("vendor/pkg/.git")).await;

    assert!(
        !at_root,
        "control: a store leaf symlinked from the ROOT `.git` must be refused \
         whatever its target is named — if this is now admitted, arm 2 has \
         regressed and the symmetry below is being graded by two broken arms"
    );
    assert!(
        !when_nested,
        "core#390 c2: the SAME object database, reached through a \
         `.git/objects` SYMLINK, must be refused whether the ROOT or a NESTED \
         checkout declares it. This is the route that needed no hand-written \
         `alternates` entry, so a gate over store NAMES can never cover it"
    );
}
