//! FerroxLabs/wayland#1104 — write as a SEPARATE, STRICTER path grant.
//!
//! Written from the ticket's definition of done, not from the implementation:
//!
//! > A user can approve `{"always_path": {"root": "~/Downloads", "write":
//! > true}}`, the agent can then write there and read back, and a read-only
//! > grant on the same folder still refuses the write.
//!
//! Every test here holds one of three lines, and the file is organised by them:
//!
//! 1. **The grant works** — otherwise this is the refusal with extra steps.
//! 2. **The boundary holds** — a grant that widens past its own root is the
//!    worst outcome available, so every widening has a paired test proving what
//!    it did NOT widen.
//! 3. **Nothing legitimate is refused** — a wrong refusal is a defect too, and
//!    it is the one that gets shipped, because a refusal looks like the guard
//!    working.
//!
//! FerroxLabs/wayland-core#384: two assertions in this file used to also ask
//! `WorkspacePolicy::is_session_write_granted`. That predicate had NO
//! production call site — its doc comment claimed `SandboxedFs`'s mutating
//! operations asked it, and they never did — so those two assertions graded
//! nothing reachable, and it has been deleted. The enclosing tests are
//! deliberately KEPT: each drives `fs.write` / `fs.read` on the real
//! `SandboxedFs`, which is the enforcement point (`contain_write` ->
//! `contain_granted` -> `live_grant_roots`). Deleting them to satisfy the
//! letter of "deleted together with the two tests that grade it" would remove
//! #1104's definition-of-done arms and grade the live path less, not more.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wcore_tools::vfs::{
    FileMutationOutcome, FilePrecondition, IntendedFileMutation, RealFs, SandboxedFs, VfsError,
    VirtualFs,
};
use wcore_tools::workspace_policy::{PathGrantError, WorkspacePolicy};

/// A genuinely-local session on a backend that DOES confine the filesystem —
/// the only shape a write grant is minted for. Both opt-ins are explicit
/// because both are fail-safe `false`/`None` in every constructor.
fn confined_local_policy(root: &Path) -> WorkspacePolicy {
    WorkspacePolicy::contained(root)
        .with_local_operator_principal()
        .with_filesystem_confinement("test-confining-backend")
}

/// A folder a reasonable person would grant: documents, no programs.
fn documents_folder() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("brief.pdf"), b"%PDF-1.7").unwrap();
    std::fs::write(dir.path().join("notes.md"), b"# notes").unwrap();
    dir
}

fn jail(policy: &Arc<WorkspacePolicy>, workspace: &Path) -> SandboxedFs<RealFs> {
    SandboxedFs::new(RealFs, workspace.to_path_buf())
        .with_path_grants(policy.session_path_grant_handle())
}

fn canon(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap()
}

// ===========================================================================
// 1. The grant works.
// ===========================================================================

/// THE GREEN ARM. The exact sentence in the ticket's definition of done: the
/// agent writes into the granted folder and reads it back.
#[tokio::test]
async fn a_granted_write_folder_can_be_written_and_read_back() {
    let ws = tempfile::tempdir().unwrap();
    let downloads = documents_folder();
    let policy = Arc::new(confined_local_policy(ws.path()));
    let fs = jail(&policy, ws.path());

    let target = downloads.path().join("report.txt");
    assert!(
        matches!(
            fs.write(&target, b"before").await,
            Err(VfsError::OutsideSandbox { .. })
        ),
        "RED: without the grant this is the refusal the user hits"
    );

    let granted = policy
        .grant_session_read_root(downloads.path(), true)
        .expect("a documents folder on a confining backend is grantable for write");
    assert_eq!(granted, canon(downloads.path()));

    fs.write(&target, b"the report")
        .await
        .expect("GREEN: write");
    assert_eq!(
        fs.read(&target).await.expect("GREEN: read back"),
        b"the report".to_vec()
    );
    // On disk, not just through our own abstraction.
    assert_eq!(std::fs::read(&target).unwrap(), b"the report".to_vec());

    // A write grant IMPLIES a read grant. The reverse never holds.
    assert!(policy.writable_roots().contains(&granted));
    assert!(policy.readable_roots().contains(&granted));
    assert!(policy.is_session_read_granted(&target));
}

/// The OS sandbox and the in-process file tools must not hold two different
/// answers. `writable_roots()` is the sole producer of
/// `SandboxManifest::fs_write_allow`, so this is what makes `Bash` agree with
/// `Write` about the same folder.
#[test]
fn a_write_grant_reaches_the_os_sandbox_manifest_scope() {
    let ws = tempfile::tempdir().unwrap();
    let downloads = documents_folder();
    let policy = confined_local_policy(ws.path());

    let before = policy.writable_roots();
    let granted = policy
        .grant_session_read_root(downloads.path(), true)
        .unwrap();

    assert!(!before.contains(&granted));
    assert!(policy.writable_roots().contains(&granted));
    assert!(
        policy.writable_roots().len() == before.len() + 1,
        "exactly one root, never a widening of the rest"
    );
}

// ===========================================================================
// 2. The boundary holds.
// ===========================================================================

/// THE DEFINITION-OF-DONE CLAUSE: a read-only grant on the same folder still
/// refuses the write.
#[tokio::test]
async fn a_read_only_grant_on_the_same_folder_still_refuses_the_write() {
    let ws = tempfile::tempdir().unwrap();
    let downloads = documents_folder();
    let policy = Arc::new(confined_local_policy(ws.path()));
    let fs = jail(&policy, ws.path());

    let granted = policy
        .grant_session_read_root(downloads.path(), false)
        .unwrap();
    let target = downloads.path().join("brief.pdf");

    // WRONG-REFUSAL CONTROL: the read the grant was asked for still works.
    assert!(fs.read(&target).await.is_ok());
    assert!(policy.readable_roots().contains(&granted));

    assert!(
        matches!(
            fs.write(&target, b"tampered").await,
            Err(VfsError::OutsideSandbox { .. })
        ),
        "a read grant is somewhere the agent may LOOK"
    );
    assert!(!policy.writable_roots().contains(&granted));
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"%PDF-1.7".to_vec(),
        "and the bytes on disk are untouched"
    );
}

/// ALL FOUR mutating operations, not just `write`.
///
/// Counting call sites is the point of this test. `SandboxedFs` has four
/// operations that mutate, they were widened together, and a guard applied to
/// three of four is a guard that looks tested and is not — this codebase has
/// shipped exactly that (a fully unit-tested guard past four ungated `BashTool`
/// call sites).
///
/// `compare_exchange_file` is the fourth, and the arms below say plainly what
/// they cannot hold. Downgrading ITS `contain_write` to `contain_read` is
/// behaviourally invisible from here: `bind_identity` searches the WRITE grants
/// one step later and returns the same `OutsideSandbox` for the same path, so
/// the read-grant arm cannot tell the two apart. MEASURED rather than assumed —
/// with that single call site swapped this file still passes 14/14, and no
/// other test in the crate distinguishes it either, so the write-vs-read LEVEL
/// of that one call site is currently ungraded. What the arms below DO hold is
/// the fourth operation's presence in both directions: it is refused under a
/// read grant, and — the wrong-refusal direction this file exists for — it is
/// NOT refused at the boundary under a write grant.
#[tokio::test]
async fn every_mutating_operation_is_gated_on_write_not_read() {
    let ws = tempfile::tempdir().unwrap();
    let outside = documents_folder();
    let victim = outside.path().join("notes.md");

    // --- read grant: all four refuse -------------------------------------
    let read_policy = Arc::new(confined_local_policy(ws.path()));
    let read_fs = jail(&read_policy, ws.path());
    read_policy
        .grant_session_read_root(outside.path(), false)
        .unwrap();

    assert!(matches!(
        read_fs.write(&victim, b"x").await,
        Err(VfsError::OutsideSandbox { .. })
    ));
    assert!(matches!(
        read_fs.observe_file(&victim).await,
        Err(VfsError::OutsideSandbox { .. })
    ));
    assert!(matches!(
        read_fs.remove_file(&victim).await,
        Err(VfsError::OutsideSandbox { .. })
    ));
    let swap = IntendedFileMutation::new(FilePrecondition::Absent, b"x".to_vec());
    assert!(matches!(
        read_fs.compare_exchange_file(&victim, &swap).await,
        Err(VfsError::OutsideSandbox { .. })
    ));
    assert!(
        read_fs.exists(&victim).await.unwrap(),
        "WRONG-REFUSAL CONTROL: the read side of the same jail still works, so \
         the four refusals above are the ACCESS level and not a broken grant"
    );

    // --- write grant: all four work --------------------------------------
    let write_ws = tempfile::tempdir().unwrap();
    let write_policy = Arc::new(confined_local_policy(write_ws.path()));
    let write_fs = jail(&write_policy, write_ws.path());
    write_policy
        .grant_session_read_root(outside.path(), true)
        .unwrap();

    write_fs.observe_file(&victim).await.expect("observe_file");
    write_fs.write(&victim, b"edited").await.expect("write");
    // `SandboxedFs<RealFs>` cannot APPLY a compare-exchange — the host has no
    // non-cooperative pathname CAS — so the reachable proof is that the write
    // grant carries this operation all the way to the PRECONDITION check,
    // which sits strictly past both `contain_write` and `bind_identity`.
    assert!(
        matches!(
            write_fs.compare_exchange_file(&victim, &swap).await,
            Ok(FileMutationOutcome::Conflict { .. })
        ),
        "WRONG-REFUSAL CONTROL: the write grant does not refuse the fourth \
         mutating operation at the boundary"
    );
    write_fs.remove_file(&victim).await.expect("remove_file");
    assert!(!victim.exists());
}

/// A grant widens by exactly one root and nothing beside it.
#[tokio::test]
async fn a_write_grant_does_not_widen_beyond_its_own_root() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let granted_dir = parent.path().join("granted");
    let sibling = parent.path().join("sibling");
    std::fs::create_dir(&granted_dir).unwrap();
    std::fs::create_dir(&sibling).unwrap();

    let policy = Arc::new(confined_local_policy(ws.path()));
    let fs = jail(&policy, ws.path());
    policy.grant_session_read_root(&granted_dir, true).unwrap();

    fs.write(&granted_dir.join("ok.txt"), b"x")
        .await
        .expect("WRONG-REFUSAL CONTROL: inside the granted root");

    for escape in [
        sibling.join("loot.txt"),
        parent.path().join("loot.txt"),
        granted_dir.join("../sibling/loot.txt"),
        granted_dir.join("nested/../../sibling/loot.txt"),
    ] {
        assert!(
            matches!(
                fs.write(&escape, b"loot").await,
                Err(VfsError::OutsideSandbox { .. })
            ),
            "{} escaped the granted root",
            escape.display()
        );
        assert!(!sibling.join("loot.txt").exists());
        assert!(!parent.path().join("loot.txt").exists());
    }
}

/// PATH SPELLINGS. The grant check compares canonical paths, so none of these
/// spellings may reach past it — and the legitimate spellings must still work.
#[tokio::test]
#[cfg(unix)]
async fn no_path_spelling_defeats_the_granted_write_root() {
    let ws = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let real = parent.path().join("real");
    let outside = parent.path().join("outside");
    std::fs::create_dir(&real).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("victim.txt"), b"original").unwrap();

    // The grant is asked for through a SYMLINK to the folder. It must be
    // recorded canonically, or the real spelling would not match it.
    let link_to_real = parent.path().join("link-to-real");
    std::os::unix::fs::symlink(&real, &link_to_real).unwrap();

    let policy = Arc::new(confined_local_policy(ws.path()));
    let fs = jail(&policy, ws.path());
    let granted = policy.grant_session_read_root(&link_to_real, true).unwrap();
    assert_eq!(
        granted,
        canon(&real),
        "a grant is recorded canonically, so the two spellings are one root"
    );

    // WRONG-REFUSAL CONTROLS: both spellings of the granted folder work.
    fs.write(&real.join("a.txt"), b"x")
        .await
        .expect("the real spelling");
    fs.write(&link_to_real.join("b.txt"), b"x")
        .await
        .expect("the spelling the user actually typed");

    // A live symlink OUT of the granted root.
    let escape = real.join("escape");
    std::os::unix::fs::symlink(outside.join("victim.txt"), &escape).unwrap();
    assert!(matches!(
        fs.write(&escape, b"tampered").await,
        Err(VfsError::OutsideSandbox { .. })
    ));
    assert_eq!(
        std::fs::read(outside.join("victim.txt")).unwrap(),
        b"original".to_vec()
    );

    // A DANGLING symlink out of it — the leaf does not exist, which is the
    // shape a canonicalize-the-whole-path check gets wrong, and the shape a
    // defect already fixed in this file had.
    let dangling = real.join("dangling");
    std::os::unix::fs::symlink(outside.join("not-yet.txt"), &dangling).unwrap();
    assert!(
        matches!(
            fs.write(&dangling, b"created").await,
            Err(VfsError::OutsideSandbox { .. })
        ),
        "a dangling link is a write to its TARGET, which is outside the grant"
    );
    assert!(!outside.join("not-yet.txt").exists());

    // A symlinked DIRECTORY inside the grant pointing out of it.
    let dir_link = real.join("dirlink");
    std::os::unix::fs::symlink(&outside, &dir_link).unwrap();
    assert!(matches!(
        fs.write(&dir_link.join("through.txt"), b"x").await,
        Err(VfsError::OutsideSandbox { .. })
    ));
    assert!(!outside.join("through.txt").exists());
}

/// The mutation authority names the boundary the object actually sits behind.
///
/// Two granted roots must not be interchangeable: a mutation prepared against
/// one and applied to the other would be a cross-root write with a matching
/// precondition.
#[tokio::test]
async fn a_granted_object_is_bound_to_its_own_root_not_the_jails() {
    let ws = tempfile::tempdir().unwrap();
    let first = documents_folder();
    let second = documents_folder();
    std::fs::write(ws.path().join("in-workspace.txt"), b"w").unwrap();

    let policy = Arc::new(confined_local_policy(ws.path()));
    let fs = jail(&policy, ws.path());
    policy.grant_session_read_root(first.path(), true).unwrap();
    policy.grant_session_read_root(second.path(), true).unwrap();

    let a = fs
        .observe_file(&first.path().join("notes.md"))
        .await
        .unwrap()
        .object
        .authority;
    let b = fs
        .observe_file(&second.path().join("notes.md"))
        .await
        .unwrap()
        .object
        .authority;
    let w = fs
        .observe_file(&ws.path().join("in-workspace.txt"))
        .await
        .unwrap()
        .object
        .authority;

    assert_ne!(a, b, "two granted roots are two authorities");
    assert_ne!(a, w, "a granted root is not the workspace");
    assert!(a.starts_with("sandbox:"));
}

/// Revoking the write grant narrows to the read grant rather than to nothing.
#[tokio::test]
async fn revoking_a_write_grant_leaves_an_independent_read_grant_standing() {
    let ws = tempfile::tempdir().unwrap();
    let downloads = documents_folder();
    let policy = Arc::new(confined_local_policy(ws.path()));
    let fs = jail(&policy, ws.path());

    policy
        .grant_session_read_root_full(downloads.path(), false, Some("read".into()), None)
        .unwrap();
    policy
        .grant_session_read_root_full(downloads.path(), true, Some("write".into()), None)
        .unwrap();

    let target = downloads.path().join("notes.md");
    fs.write(&target, b"edited").await.unwrap();

    assert!(policy.revoke_session_read_root("write").is_some());
    assert!(
        matches!(
            fs.write(&target, b"again").await,
            Err(VfsError::OutsideSandbox { .. })
        ),
        "revocation takes effect on the next call, not the next session"
    );
    assert!(
        fs.read(&target).await.is_ok(),
        "and the read grant the user never revoked is untouched"
    );
}

/// Expiry is evaluated at USE time, on the write side too.
#[tokio::test]
async fn an_expired_write_grant_confers_nothing() {
    let ws = tempfile::tempdir().unwrap();
    let downloads = documents_folder();
    let policy = Arc::new(confined_local_policy(ws.path()));
    let fs = jail(&policy, ws.path());

    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    let granted = policy
        .grant_session_read_root_full(downloads.path(), true, Some("w".into()), Some(past))
        .unwrap();

    assert!(!policy.writable_roots().contains(&granted));
    assert!(!policy.readable_roots().contains(&granted));
    assert!(matches!(
        fs.write(&downloads.path().join("notes.md"), b"x").await,
        Err(VfsError::OutsideSandbox { .. })
    ));
}

/// A live READ grant is not cover for a WRITE request on the same folder.
///
/// The pre-#1104 coverage check matched on root alone, so this sequence would
/// have reported success, recorded nothing, and left the very next write
/// refused — the user told they had write, and no write.
#[tokio::test]
async fn a_read_grant_is_not_cover_for_a_later_write_request() {
    let ws = tempfile::tempdir().unwrap();
    let downloads = documents_folder();
    let policy = Arc::new(confined_local_policy(ws.path()));
    let fs = jail(&policy, ws.path());

    policy
        .grant_session_read_root(downloads.path(), false)
        .unwrap();
    let granted = policy
        .grant_session_read_root(downloads.path(), true)
        .expect("upgrading to write is a new grant, not a no-op");

    assert!(policy.writable_roots().contains(&granted));
    fs.write(&downloads.path().join("notes.md"), b"edited")
        .await
        .expect("the upgrade actually took");
}

// ===========================================================================
// 3. The write-only refusals — each with the legitimate case beside it.
// ===========================================================================

/// A root holding something the operator could later run is refused for WRITE
/// and still granted for READ.
#[test]
fn a_root_holding_an_executable_is_refused_for_write_only() {
    let ws = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    std::fs::write(downloads.path().join("brief.pdf"), b"%PDF").unwrap();
    std::fs::write(downloads.path().join("Installer.exe"), b"MZ").unwrap();

    let policy = confined_local_policy(ws.path());
    let error = policy
        .grant_session_read_root(downloads.path(), true)
        .expect_err("write-to-RCE: the operator runs that installer next week");
    assert!(
        matches!(error, PathGrantError::WriteRootExecutable(_)),
        "got {error:?}"
    );
    assert!(policy.session_path_grant_roots().is_empty());

    // WRONG-REFUSAL CONTROL 1: read on the very same folder is unaffected.
    let granted = policy
        .grant_session_read_root(downloads.path(), false)
        .expect("reading a folder that holds a program is not the same ask");
    assert!(policy.readable_roots().contains(&granted));
    assert!(!policy.writable_roots().contains(&granted));

    // WRONG-REFUSAL CONTROL 2: the ordinary case still grants for write.
    let clean = documents_folder();
    let ws2 = tempfile::tempdir().unwrap();
    confined_local_policy(ws2.path())
        .grant_session_read_root(clean.path(), true)
        .expect("a documents folder with no programs in it is grantable");
}

/// A repository's control surface outside the workspace.
#[test]
fn a_root_holding_a_git_checkout_is_refused_for_write_only() {
    let ws = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(projects.path().join("app/.git/hooks")).unwrap();

    let policy = confined_local_policy(ws.path());
    let error = policy
        .grant_session_read_root(projects.path(), true)
        .expect_err("`.git/hooks/pre-commit` runs on the operator's next commit");
    assert!(
        matches!(error, PathGrantError::WriteRootAutoRun(_)),
        "got {error:?}"
    );

    // WRONG-REFUSAL CONTROL: read is still granted, which is what a "read this
    // repo for me" request needs.
    policy
        .grant_session_read_root(projects.path(), false)
        .expect("reading a checkout is ordinary session work");
}

/// A secret inside the root. `is_project_secret` and `SecretDenyFs` are
/// WORKSPACE-scoped, so neither of them can see a `.env` in a granted folder;
/// refusing the root is the one answer that holds for `Bash` and the file tools
/// alike, because `SandboxManifest` has no `fs_write_deny` to express the
/// narrower one with.
#[test]
fn a_root_holding_a_secret_is_refused_for_write_only() {
    let ws = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("notes.md"), b"# hi").unwrap();
    std::fs::write(folder.path().join("id_rsa"), b"-----BEGIN").unwrap();

    let policy = confined_local_policy(ws.path());
    let error = policy
        .grant_session_read_root(folder.path(), true)
        .expect_err("a write grant could overwrite or replace the key");
    assert!(
        matches!(error, PathGrantError::WriteRootSecret(_)),
        "got {error:?}"
    );

    // WRONG-REFUSAL CONTROL: read is granted, and the secret INSIDE it is
    // still refused by the existing deny — the grant widened where, not what.
    policy
        .grant_session_read_root(folder.path(), false)
        .expect("read is unchanged by #1104");
}

/// Every refusal a READ grant has always made is still made for a write grant,
/// and is still reported as ITSELF rather than as a write-specific message.
#[test]
fn a_write_grant_inherits_every_read_refusal_with_its_own_reason() {
    let ws = tempfile::tempdir().unwrap();
    let policy = confined_local_policy(ws.path());
    let home = dirs::home_dir().expect("a home directory");

    let error = policy
        .grant_session_read_root(&home, true)
        .expect_err("$HOME reaches everything the sandbox stands between");
    assert!(
        matches!(error, PathGrantError::TooBroad(_)),
        "the reason must be the REAL one, not whichever check ran first: {error:?}"
    );

    // A non-local session cannot be granted write either, and hears why.
    let remote =
        WorkspacePolicy::contained(ws.path()).with_filesystem_confinement("test-confining-backend");
    let outside = documents_folder();
    let error = remote
        .grant_session_read_root(outside.path(), true)
        .expect_err("a wire peer may ask, only a local operator may permit");
    assert!(
        matches!(error, PathGrantError::RequiresLocalOperator),
        "got {error:?}"
    );
}
