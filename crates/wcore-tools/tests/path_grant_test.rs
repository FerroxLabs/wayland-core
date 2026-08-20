//! Standing folder grants (`ApprovalScope::AlwaysPath`) — the "always allow
//! this folder" answer to an out-of-workspace escalation prompt.
//!
//! Written from the contract, not from the implementation: a grant must widen
//! READS by exactly one root, must never widen writes, must be refused for a
//! session that is not a local operator's, and must never be a way to reach a
//! credential store or `$HOME`.

use std::path::PathBuf;
use std::sync::Arc;
use wcore_protocol::PathGrantSink;
use wcore_tools::vfs::{RealFs, SandboxedFs, VfsError, VirtualFs};
use wcore_tools::workspace_policy::{PathGrantError, WorkspacePolicy};

/// A genuinely-local session: the only posture a folder grant is minted for.
fn local_policy(root: &std::path::Path) -> WorkspacePolicy {
    WorkspacePolicy::contained(root).with_local_operator_principal()
}

#[test]
fn a_granted_folder_becomes_readable() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let policy = local_policy(ws.path());

    assert!(
        !policy.is_session_read_granted(&outside.path().join("report.html")),
        "nothing outside the workspace is reachable before a grant"
    );

    let granted = policy
        .grant_session_read_root(outside.path(), false)
        .unwrap();
    assert!(policy.is_session_read_granted(&granted.join("report.html")));
    assert!(
        policy.readable_roots().contains(&granted),
        "readable_roots() feeds the OS sandbox manifest, so the grant has to \
         appear there or Bash still cannot see the folder"
    );
    assert!(
        !policy.writable_roots().contains(&granted),
        "a read grant must never appear in the writable set"
    );
}

#[test]
fn granting_the_file_the_user_was_looking_at_grants_its_folder() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let report = outside.path().join("morning-brief.html");
    std::fs::write(&report, b"<html/>").unwrap();

    let policy = local_policy(ws.path());
    let granted = policy.grant_session_read_root(&report, false).unwrap();

    assert!(granted.is_dir(), "a grant names a folder, never a file");
    assert_eq!(granted, std::fs::canonicalize(outside.path()).unwrap());
}

#[test]
fn write_is_not_grantable() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let policy = local_policy(ws.path());

    let error = policy
        .grant_session_read_root(outside.path(), true)
        .expect_err("write authority outside the workspace is a bigger ask");
    assert!(matches!(error, PathGrantError::WriteNotGrantable));
    assert!(
        policy.session_read_grant_roots().is_empty(),
        "a refused write grant must not leave a read grant behind"
    );
}

#[test]
fn a_session_that_is_not_a_local_operators_cannot_be_granted() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    // No `with_local_operator_principal()`: a channel / remote / managed
    // engine. This is the GHSA-8r7g shape — a wire peer may ask, only a local
    // operator may permit.
    let policy = WorkspacePolicy::contained(ws.path());

    let error = policy
        .grant_session_read_root(outside.path(), false)
        .expect_err("a remote peer must not be able to widen the sandbox");
    assert!(matches!(error, PathGrantError::RequiresLocalOperator));
    assert!(policy.session_read_grant_roots().is_empty());
}

#[test]
fn home_and_the_filesystem_root_are_refused() {
    let ws = tempfile::tempdir().unwrap();
    let policy = local_policy(ws.path());

    let root = if cfg!(windows) {
        PathBuf::from("C:\\")
    } else {
        PathBuf::from("/")
    };
    assert!(
        policy.grant_session_read_root(&root, false).is_err(),
        "granting the filesystem root is not a scoped escape hatch, it is the \
         removal of the sandbox"
    );

    if let Some(home) = dirs::home_dir() {
        let error = policy
            .grant_session_read_root(&home, false)
            .expect_err("$HOME reaches everything the sandbox stands between");
        assert!(matches!(error, PathGrantError::TooBroad(_)));
    }
}

#[test]
fn a_folder_containing_a_credential_store_is_refused() {
    let ws = tempfile::tempdir().unwrap();
    let policy = local_policy(ws.path());
    let Some(home) = dirs::home_dir() else {
        return;
    };

    // Both directions have to hold, and the test must not be able to pass by
    // finding neither directory — a check that silently skips is a gate that
    // cannot fail.
    let mut exercised = 0;

    // The store itself.
    let ssh = home.join(".ssh");
    if ssh.is_dir() {
        assert!(
            policy.grant_session_read_root(&ssh, false).is_err(),
            "granting ~/.ssh is the disclosure the read-deny list exists to stop"
        );
        exercised += 1;
    }

    // And the other direction: a parent that CONTAINS a credential store. This
    // is the half a naive `starts_with` check misses.
    let config = home.join(".config");
    if config.is_dir() {
        assert!(
            policy.grant_session_read_root(&config, false).is_err(),
            "~/.config contains the gcloud/gh/op credential stores, so granting \
             it hands them over just as surely as naming them"
        );
        exercised += 1;
    }

    // A synthetic store, so the assertion holds on a bare CI home with neither
    // directory present. Built under the real `$HOME` because CREDENTIAL_STORES
    // is home-relative by definition.
    let synthetic = home.join(".aws");
    let created = !synthetic.exists() && std::fs::create_dir_all(&synthetic).is_ok();
    if synthetic.is_dir() {
        let refused = policy.grant_session_read_root(&synthetic, false).is_err();
        if created {
            let _ = std::fs::remove_dir(&synthetic);
        }
        assert!(refused, "~/.aws is a credential store in any home");
        exercised += 1;
    }

    assert!(
        exercised > 0,
        "no credential store was reachable, so this test asserted nothing"
    );
}

#[test]
fn re_approving_the_same_folder_is_idempotent() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let nested = outside.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    let policy = local_policy(ws.path());

    policy
        .grant_session_read_root(outside.path(), false)
        .unwrap();
    policy
        .grant_session_read_root(outside.path(), false)
        .unwrap();
    // Already covered by the parent grant.
    policy.grant_session_read_root(&nested, false).unwrap();

    assert_eq!(
        policy.session_read_grant_roots().len(),
        1,
        "clicking the same button twice must not stack grants"
    );
}

// ---------------------------------------------------------------------------
// The end-to-end shape: the in-process file tools must honour the grant too.
// Without this the user approves the folder and `Read` still says no, because
// the OS sandbox and the VFS jail hold two different answers.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_file_tools_read_a_granted_folder_but_still_cannot_write_it() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let report = outside.path().join("morning-brief.html");
    std::fs::write(&report, b"<html>brief</html>").unwrap();

    let policy = Arc::new(local_policy(ws.path()));
    let jail = SandboxedFs::new(RealFs, ws.path().to_path_buf())
        .with_read_grants(policy.session_read_grant_handle());

    // Before the grant: the dead end the escalation prompt exists to replace.
    assert!(
        matches!(
            jail.read(&report).await,
            Err(VfsError::OutsideSandbox { .. })
        ),
        "a path outside the workspace is refused before any grant"
    );

    policy
        .grant_session_read_root(outside.path(), false)
        .unwrap();

    // After the grant, on the SAME jail instance — a grant that only took
    // effect on the next session would be no use to a running turn.
    assert_eq!(
        jail.read(&report).await.unwrap(),
        b"<html>brief</html>".to_vec(),
        "the approved folder is readable without rebuilding the sandbox"
    );
    assert!(jail.exists(&report).await.unwrap());
    assert!(jail.metadata(&report).await.is_ok());
    assert!(jail.list(outside.path()).await.is_ok());

    // Writes are NOT widened. This is the whole point of a read grant.
    assert!(
        matches!(
            jail.write(&report, b"tampered").await,
            Err(VfsError::OutsideSandbox { .. })
        ),
        "a read grant must not become a write grant"
    );
    assert!(
        matches!(
            jail.remove_file(&report).await,
            Err(VfsError::OutsideSandbox { .. })
        ),
        "nor a delete grant"
    );
    assert_eq!(
        std::fs::read(&report).unwrap(),
        b"<html>brief</html>".to_vec(),
        "and the refused write really did not land"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_out_of_a_granted_folder_is_still_refused() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let escape = outside.path().join("escape");
    std::os::unix::fs::symlink("/etc", &escape).unwrap();

    let policy = Arc::new(local_policy(ws.path()));
    let jail = SandboxedFs::new(RealFs, ws.path().to_path_buf())
        .with_read_grants(policy.session_read_grant_handle());
    policy
        .grant_session_read_root(outside.path(), false)
        .unwrap();

    // The grant widens the jail by one root, not by one root plus wherever
    // that root's symlinks point.
    assert!(
        jail.read(&escape.join("hosts")).await.is_err(),
        "a granted folder is not a tunnel out of the sandbox"
    );
}

#[tokio::test]
async fn a_jail_with_no_grants_behaves_exactly_as_before() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("x.txt");
    std::fs::write(&file, b"x").unwrap();
    let inside = ws.path().join("in.txt");
    std::fs::write(&inside, b"in").unwrap();

    // Constructed the old way — no `with_read_grants` at all.
    let jail = SandboxedFs::new(RealFs, ws.path().to_path_buf());
    assert_eq!(jail.read(&inside).await.unwrap(), b"in".to_vec());
    assert!(matches!(
        jail.read(&file).await,
        Err(VfsError::OutsideSandbox { .. })
    ));
}

// ---------------------------------------------------------------------------
// The sink: what the approval manager actually calls.
// ---------------------------------------------------------------------------

#[test]
fn the_sink_reports_refusal_rather_than_swallowing_it() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();

    let local = local_policy(ws.path());
    assert!(
        PathGrantSink::grant_path(&local, outside.path(), false),
        "a permitted grant reports success so the caller can keep the scope"
    );

    let remote = WorkspacePolicy::contained(ws.path());
    assert!(
        !PathGrantSink::grant_path(&remote, outside.path(), false),
        "a refused grant reports false so the approval degrades to Once — the \
         act the user approved still happens, the standing grant does not"
    );
}

// ---------------------------------------------------------------------------
// REQUEST 3 from the Desktop lane, in writing: the credential/secret denies
// must WIN over a user-granted root. "A grant on `~` that overrode the
// credential deny is strictly worse than no feature."
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_secret_inside_a_granted_folder_is_still_refused() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();

    // An ordinary folder a user would plausibly grant — that happens to have
    // something sensitive in it. This is NOT a credential store, so the
    // grant-time refusal does not catch it; the use-time rule must.
    let ordinary = outside.path().join("Downloads");
    std::fs::create_dir(&ordinary).unwrap();
    let report = ordinary.join("report.pdf");
    std::fs::write(&report, b"fine").unwrap();
    let key = ordinary.join("id_rsa");
    std::fs::write(&key, b"PRIVATE KEY").unwrap();
    let dotenv = ordinary.join(".env");
    std::fs::write(&dotenv, b"TOKEN=1").unwrap();
    let pem = ordinary.join("server.pem");
    std::fs::write(&pem, b"CERT").unwrap();

    let policy = Arc::new(local_policy(ws.path()));
    let jail = SandboxedFs::new(RealFs, ws.path().to_path_buf())
        .with_read_grants(policy.session_read_grant_handle());
    policy.grant_session_read_root(&ordinary, false).unwrap();

    // The grant works for the thing it was for.
    assert_eq!(jail.read(&report).await.unwrap(), b"fine".to_vec());

    // And not for anything the secret rules cover.
    for secret in [&key, &dotenv, &pem] {
        assert!(
            matches!(jail.read(secret).await, Err(VfsError::SecretDenied { .. })),
            "{} must stay refused inside a granted folder — a grant says \
             WHERE the agent may look, never WHAT it may read",
            secret.display()
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_benign_name_symlinked_to_a_secret_inside_a_grant_is_refused() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("id_rsa");
    std::fs::write(&secret, b"PRIVATE KEY").unwrap();
    let decoy = outside.path().join("notes.txt");
    std::os::unix::fs::symlink(&secret, &decoy).unwrap();

    let policy = Arc::new(local_policy(ws.path()));
    let jail = SandboxedFs::new(RealFs, ws.path().to_path_buf())
        .with_read_grants(policy.session_read_grant_handle());
    policy
        .grant_session_read_root(outside.path(), false)
        .unwrap();

    // The check runs on the CANONICAL path, so renaming a secret does not
    // launder it.
    assert!(
        matches!(jail.read(&decoy).await, Err(VfsError::SecretDenied { .. })),
        "a benign filename pointing at a secret must not launder it"
    );
}

#[test]
fn a_secret_in_a_granted_folder_reaches_the_os_read_deny_list() {
    // The sibling of the two tests above, for the OS sandbox rather than the
    // in-process tools. If only the VFS refused, `Bash cat` would read what
    // `Read` refused, and the two layers would disagree.
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let key = outside.path().join("id_rsa");
    std::fs::write(&key, b"PRIVATE KEY").unwrap();
    let key = std::fs::canonicalize(&key).unwrap();

    // `contained` requires OS read-deny enforcement, which is the posture the
    // dynamic walk is gated on.
    let policy = WorkspacePolicy::contained(ws.path()).with_local_operator_principal();
    assert!(policy.secret_read_deny_required());
    assert!(
        !policy.secret_deny_paths_dynamic().contains(&key),
        "nothing outside the workspace is denied before the grant, because \
         nothing outside it is reachable"
    );

    policy
        .grant_session_read_root(outside.path(), false)
        .unwrap();
    assert!(
        policy.secret_deny_paths_dynamic().contains(&key),
        "granting the folder must also extend the read-deny walk into it"
    );
}

// ---------------------------------------------------------------------------
// Revocation and expiry — the `grant_path` / `revoke_path` command surface.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_revoked_grant_stops_working_immediately() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("x.txt");
    std::fs::write(&file, b"x").unwrap();

    let policy = Arc::new(local_policy(ws.path()));
    let jail = SandboxedFs::new(RealFs, ws.path().to_path_buf())
        .with_read_grants(policy.session_read_grant_handle());

    policy
        .grant_session_read_root_full(outside.path(), false, Some("g1".into()), None)
        .unwrap();
    assert!(jail.read(&file).await.is_ok());

    assert!(policy.revoke_session_read_root("g1").is_some());
    assert!(
        matches!(jail.read(&file).await, Err(VfsError::OutsideSandbox { .. })),
        "revocation must take effect on the next call, not the next session"
    );
    assert!(
        policy.revoke_session_read_root("g1").is_none(),
        "revoking twice is a no-op, so a host that crashed mid-flow can still \
         clean up without knowing what landed"
    );
}

#[tokio::test]
async fn an_expired_grant_stops_working_without_anyone_sweeping() {
    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("x.txt");
    std::fs::write(&file, b"x").unwrap();

    let policy = Arc::new(local_policy(ws.path()));
    let jail = SandboxedFs::new(RealFs, ws.path().to_path_buf())
        .with_read_grants(policy.session_read_grant_handle());

    // Already in the past: expiry is evaluated at USE time, so a grant cannot
    // outlive its deadline by racing whatever would otherwise reap it.
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    policy
        .grant_session_read_root_full(outside.path(), false, Some("g2".into()), Some(past))
        .unwrap();

    assert!(
        matches!(jail.read(&file).await, Err(VfsError::OutsideSandbox { .. })),
        "an expired grant grants nothing"
    );
    assert!(
        policy.session_read_grant_roots().is_empty(),
        "and it is not reported as a live root either"
    );
    assert!(!policy.is_session_read_granted(&file));
}
