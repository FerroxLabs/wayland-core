//! D11 / core#356 — **every security predicate in `workspace_policy` must
//! resolve a DANGLING symlink to where the operation would actually land.**
//!
//! #356 closed this on `is_skill_source_path` and `is_repo_control_path` by
//! moving them onto `canon_existing_ancestor`. The siblings were left on
//! `canon_for_scope`, which calls `canonicalize(parent).join(name)` — so a
//! symlink whose TARGET does not exist yet is judged where the LINK sits, not
//! where the write lands. `std::fs::write` follows the link.
//!
//! Each escape assertion is paired with two controls in the same run — the
//! name spelled directly, and a link to an EXISTING target — so a predicate
//! that answered `true` to everything could not pass this file.

#![cfg(unix)]

use std::os::unix::fs::symlink;
use std::path::Path;

use wcore_tools::workspace_policy::WorkspacePolicy;

fn ws() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// `<root>/.env` does NOT exist; `<root>/notes.txt` dangles at it. A `Write`
/// through the link creates the project secret.
#[test]
fn a_dangling_link_to_a_not_yet_created_project_secret_is_a_project_secret() {
    let dir = ws();
    let root = dir.path();
    let policy = WorkspacePolicy::contained(root);

    let link = root.join("notes.txt");
    symlink(root.join(".env"), &link).unwrap();

    // CONTROL 1 — the secret named directly.
    assert!(
        policy.is_project_secret(&root.join(".env")),
        "control: a directly-named project secret must be caught"
    );

    // CONTROL 2 — a link whose target EXISTS is already caught.
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/.env"), b"K=v").unwrap();
    let live = root.join("live-link");
    symlink(root.join("sub/.env"), &live).unwrap();
    assert!(
        policy.is_project_secret(&live),
        "control: a link to an existing project secret must be caught"
    );

    // THE ESCAPE.
    assert!(
        policy.is_project_secret(&link),
        "a dangling link to a not-yet-created project secret escaped \
         is_project_secret; a Full-posture write through it lands as {}",
        root.join(".env").display()
    );
}

/// The same resolver bug on `is_vcs_content_store`: a dangling link into the
/// object store lets a write plant bytes the store deny exists to protect.
#[test]
fn a_dangling_link_into_the_object_store_is_a_vcs_content_store() {
    let dir = ws();
    let root = dir.path();
    let policy = WorkspacePolicy::contained(root);

    std::fs::create_dir_all(root.join(".git/objects/aa")).unwrap();
    std::fs::write(root.join(".git/objects/aa/live"), b"x").unwrap();

    // CONTROL 1 — the store named directly.
    assert!(
        policy.is_vcs_content_store(&root.join(".git/objects/aa/live")),
        "control: a directly-named object must be caught"
    );
    // CONTROL 2 — a link to an EXISTING object.
    let live = root.join("live-obj");
    symlink(root.join(".git/objects/aa/live"), &live).unwrap();
    assert!(
        policy.is_vcs_content_store(&live),
        "control: a link to an existing object must be caught"
    );

    // THE ESCAPE — the target does not exist yet.
    let dangling = root.join("plant.txt");
    symlink(root.join(".git/objects/aa/planted"), &dangling).unwrap();
    assert!(
        policy.is_vcs_content_store(&dangling),
        "a dangling link into the object store escaped is_vcs_content_store"
    );
}

/// `is_read_reachable` decides whether a path is inside the workspace at all.
/// A dangling link that lands OUTSIDE must not read as reachable just because
/// the link itself sits inside.
#[test]
fn a_dangling_link_out_of_the_workspace_is_not_read_reachable() {
    let dir = ws();
    let outside = ws();
    let root = dir.path();
    let policy = WorkspacePolicy::contained(root);

    // CONTROL 1 — an ordinary in-workspace path IS reachable.
    assert!(
        policy.is_read_reachable(&root.join("ordinary.txt")),
        "control: an in-workspace path must stay reachable"
    );
    // CONTROL 2 — a link to an EXISTING outside file is already refused.
    let live_target = outside.path().join("loot.txt");
    std::fs::write(&live_target, b"loot").unwrap();
    let live = root.join("live-out");
    symlink(&live_target, &live).unwrap();
    assert!(
        !policy.is_read_reachable(&live),
        "control: a link to an existing outside file must be refused"
    );

    // THE ESCAPE.
    let dangling = root.join("dangling-out");
    symlink(outside.path().join("missing.txt"), &dangling).unwrap();
    assert!(
        !policy.is_read_reachable(&dangling),
        "a dangling link out of the workspace read as reachable: {}",
        Path::new(&dangling).display()
    );
}
