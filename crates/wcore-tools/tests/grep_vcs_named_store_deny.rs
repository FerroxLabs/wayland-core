//! core#244 c3, SECOND ARM — the stores a workspace's own VCS control files
//! NAME rather than contain.
//!
//! `WorkspacePolicy::is_vcs_content_store` has four arms. Arm 1 is lexical
//! (`inside_vcs_store`): a path with a `(control, store)` pair among its
//! ancestors. Arm 2 resolves the stores this root's `.git` NAMES — a gitfile's
//! `gitdir:`/`commondir` (#242) and an `objects/info/alternates` borrow
//! (`git clone --shared` / `--reference`). Both can resolve to a directory
//! INSIDE the workspace root, where nothing about the path is lexically a
//! store.
//!
//! `grep_policy` asked only arm 1. So the whole-workspace search — `Grep(".")`,
//! the commonest call there is — walked into an arm-2 store in PLAINTEXT while
//! the in-process VFS refused the very same bytes. c3's sentence is "unreachable
//! to a shell subprocess, NOT ONLY to the in-process VFS"; that asymmetry is
//! exactly what it forbids.
//!
//! Every arm below carries its own positive controls: the policy predicate must
//! SAY store (otherwise the fixture proves nothing) and an ordinary file must
//! still come back (otherwise a Grep that returned nothing would pass).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::grep::GrepTool;
use wcore_tools::vfs::{RealFs, SandboxedFs, SecretDenyFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

const GITFILE_OBJ: &str = "WLCANARY-GITFILE-244";
const ALT_OBJ: &str = "WLCANARY-ALTERNATES-244";
const NESTED_GITFILE_OBJ: &str = "WLCANARY-NESTEDGITFILE-244";
const CONTROL: &str = "WLCANARY-CONTROL-OK";
const PATTERN: &str = "WLCANARY";

/// The production stack the dispatcher installs in `Contained` mode — copied
/// verbatim from `grep_vcs_store_deny.rs` so the two files cannot diverge on
/// the posture under test.
fn contained_ctx(root: &Path) -> ToolContext {
    let policy = Arc::new(WorkspacePolicy::contained(root));
    let vfs = SandboxedFs::new(
        SecretDenyFs::new(RealFs, policy.clone()),
        root.to_path_buf(),
    );
    let mut ctx = ToolContext::test_default();
    ctx.vfs = Arc::new(vfs);
    ctx.with_workspace(policy)
}

async fn grep(ctx: &ToolContext, path: &str) -> String {
    GrepTool
        .execute_with_ctx(json!({ "pattern": PATTERN, "path": path }), ctx)
        .await
        .content
}

fn write(path: PathBuf, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("{body}\n")).unwrap();
}

/// A gitfile at the root points `gitdir:` at a directory INSIDE the root
/// (`git init --separate-git-dir`, and the shape a submodule checkout has).
fn gitfile_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let store_file = root.join("mygit/objects/ab/cd1234");
    write(store_file.clone(), GITFILE_OBJ);
    std::fs::write(root.join(".git"), "gitdir: mygit\n").unwrap();
    write(root.join("notes.txt"), &format!("plain {CONTROL}"));
    (dir, root, store_file)
}

/// `.git/objects/info/alternates` borrows an object store from a sibling
/// directory inside the root — what `git clone --shared` / `--reference` write.
fn alternates_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let store_file = root.join("shared-objects/ab/cd1234");
    write(store_file.clone(), ALT_OBJ);
    std::fs::create_dir_all(root.join(".git/objects")).unwrap();
    write(
        root.join(".git/objects/info/alternates"),
        "../../shared-objects",
    );
    write(root.join(".git/HEAD"), "ref: refs/heads/main");
    write(root.join("notes.txt"), &format!("plain {CONTROL}"));
    (dir, root, store_file)
}

/// A gitfile on a VENDORED checkout, not the root. `vcs_content_stores` reads
/// only `<root>/.git`, so the product's own predicate does NOT see this one —
/// recorded here as measurement, not as a claim about arm 2.
fn nested_gitfile_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let store_file = root.join("vendor/pkg-git/objects/12/3456");
    write(store_file.clone(), NESTED_GITFILE_OBJ);
    std::fs::create_dir_all(root.join("vendor/pkg")).unwrap();
    std::fs::write(root.join("vendor/pkg/.git"), "gitdir: ../pkg-git\n").unwrap();
    write(root.join("notes.txt"), &format!("plain {CONTROL}"));
    (dir, root, store_file)
}

fn assert_control(out: &str, arm: &str) {
    assert!(
        out.contains(CONTROL),
        "positive control ({arm}): an ordinary file's match must still be \
         returned, or every refusal below is satisfied by a broken Grep. \
         Output:\n{out}"
    );
}

/// Rule 6 — a withheld hit is REPORTED. "could not show you" and "there was
/// nothing" are different answers and the model acts on them differently; at
/// the measured leak no footer appeared at all, because nothing had classified
/// the line as a store hit in the first place.
fn assert_reported(out: &str, arm: &str) {
    assert!(
        out.contains("in VCS content stores withheld"),
        "core#244 c3 ({arm}): the store was withheld SILENTLY — the footer must \
         say so. Output:\n{out}"
    );
}

#[tokio::test]
async fn grep_cannot_harvest_a_gitfile_named_store() {
    let (_dir, root, store_file) = gitfile_fixture();
    let policy = WorkspacePolicy::contained(&root);
    assert!(
        policy.is_vcs_content_store(&store_file),
        "fixture control: the PRODUCT's own predicate must call {} a content \
         store, or this arm is testing nothing",
        store_file.display()
    );

    let ctx = contained_ctx(&root);
    let out = grep(&ctx, ".").await;
    assert_control(&out, "gitfile");
    assert_reported(&out, "gitfile");
    assert!(
        !out.contains(GITFILE_OBJ),
        "core#244 c3: a gitfile-pointed object store the in-process VFS \
         REFUSES reached the model in plaintext through Grep(\".\"). \
         Output:\n{out}"
    );

    let out = grep(&ctx, "mygit").await;
    assert!(
        !out.contains(GITFILE_OBJ),
        "core#244 c3: naming the gitdir one component above the store walked \
         straight in. Output:\n{out}"
    );
}

#[tokio::test]
async fn grep_cannot_harvest_an_alternates_borrowed_store() {
    let (_dir, root, store_file) = alternates_fixture();
    let policy = WorkspacePolicy::contained(&root);
    assert!(
        policy.is_vcs_content_store(&store_file),
        "fixture control: the PRODUCT's own predicate must call {} a content \
         store, or this arm is testing nothing",
        store_file.display()
    );

    let ctx = contained_ctx(&root);
    let out = grep(&ctx, ".").await;
    assert_control(&out, "alternates");
    assert_reported(&out, "alternates");
    assert!(
        !out.contains(ALT_OBJ),
        "core#244 c3: an `objects/info/alternates`-borrowed store the \
         in-process VFS REFUSES reached the model in plaintext through \
         Grep(\".\"). Output:\n{out}"
    );
}

#[tokio::test]
async fn grep_cannot_harvest_a_nested_gitfile_named_store() {
    let (_dir, root, store_file) = nested_gitfile_fixture();
    // FerroxLabs/wayland-core#390 c4 — INVERTED, not deleted. This assertion
    // stood as the record that Grep and the point-predicate disagreed about a
    // VENDORED gitfile's store: Grep denied it because it TRAVERSES, while
    // `is_vcs_content_store` read only `<root>/.git` and said no. Arm 4 reads
    // every nested gitfile where it lies, so the two layers agree again, and
    // keeping the assertion in its inverted form is what re-ties them.
    assert!(
        WorkspacePolicy::contained(&root).is_vcs_content_store(&store_file),
        "core#390 c1/c4: the point-predicate must see a VENDORED gitfile's \
         store, or the VFS and Grep have drifted apart again"
    );

    let ctx = contained_ctx(&root);
    let out = grep(&ctx, ".").await;
    assert_control(&out, "nested gitfile");
    assert_reported(&out, "nested gitfile");
    assert!(
        !out.contains(NESTED_GITFILE_OBJ),
        "core#244 c3: a VENDORED gitfile-pointed object store reached the \
         model in plaintext through Grep(\".\"). Output:\n{out}"
    );
}
