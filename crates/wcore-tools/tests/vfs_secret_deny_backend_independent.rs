//! A7 (#922 R1) — **the in-process VFS secret predicate is NOT gated on the
//! sandbox backend.**
//!
//! #922's fix gates ONE thing: `manifest.fs_read_deny`, the list the OS
//! backend applies. It is skipped when the backend answers
//! `enforces_read_deny() == false`, because a `false` answer is the definition
//! of "this backend discards that field".
//!
//! The file tools do not go through the OS backend at all. `vfs::SecretDenyFs`
//! calls `WorkspacePolicy::is_project_secret` per access, in THIS process, and
//! its refusal is enforced by this process. Routing that predicate through the
//! same backend capability would hand `Read`/`Edit`/`Grep` every project
//! secret on the shipped Windows default — turning a latency fix into the
//! exact leak #234/#667 closed. The design calls this "the easiest thing to
//! get wrong", so it gets its own pin.
//!
//! **Red arm:** make `SecretDenyFs::guard` (or `is_project_secret`) consult the
//! backend's `enforces_read_deny()` the way `secret_deny_paths_for_backend`
//! does. Every assertion below must go red.
//!
//! Note on scope: core#244 (denying a raw `.git/objects/ab/cdef…` read through
//! the VFS) is still OPEN and is deliberately NOT bundled here — see the
//! design's §3. This test therefore pins the property on a path the predicate
//! covers TODAY (`.env`, and a secret buried under `node_modules/`). When #244
//! lands it extends this file rather than replacing it.

use std::sync::Arc;
use wcore_sandbox::SandboxRegistry;
use wcore_sandbox::backends::windows_job_object::WindowsJobObjectBackend;
use wcore_tools::vfs::{RealFs, SecretDenyFs, VfsError, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;

#[tokio::test]
async fn vfs_secret_denial_survives_a_backend_that_enforces_no_read_deny() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(root.join(".env"), b"TOKEN=abc").unwrap();
    std::fs::create_dir_all(root.join("node_modules/vendor")).unwrap();
    std::fs::write(root.join("node_modules/vendor/x.pem"), b"KEY").unwrap();
    std::fs::write(root.join("main.rs"), b"fn main() {}").unwrap();

    // The shipped Windows session default, wrapped exactly as a session wraps
    // it. This is the backend #922's 76,367 ms was being paid for.
    let registry = SandboxRegistry::new(Arc::new(WindowsJobObjectBackend::new()));
    assert!(
        !registry.enforces_read_deny(),
        "precondition: this must be the NON-enforcing backend, or the test \
         proves nothing about the gate"
    );

    let policy = Arc::new(WorkspacePolicy::contained(&root));

    // The OS list IS gated — this is R1 firing, and it is what makes the
    // assertions below non-vacuous: they hold on the very configuration where
    // the OS layer has been switched off.
    assert!(
        policy
            .secret_deny_paths_for_backend(registry.enforces_read_deny())
            .is_empty(),
        "R1: the OS deny list must be skipped for a non-enforcing backend"
    );
    assert!(
        !policy.secret_deny_paths_for_backend(true).is_empty(),
        "control: the same policy DOES produce a list for an enforcing backend"
    );

    // ...and the in-process layer is NOT gated.
    let fs = SecretDenyFs::new(RealFs, Arc::clone(&policy));
    for secret in [".env", "node_modules/vendor/x.pem"] {
        assert!(
            matches!(
                fs.read(&root.join(secret)).await,
                Err(VfsError::SecretDenied { .. })
            ),
            "{secret} must still be refused by the in-process predicate on a \
             backend that enforces no OS read-deny"
        );
        assert!(
            matches!(
                fs.write(&root.join(secret), b"x").await,
                Err(VfsError::SecretDenied { .. })
            ),
            "{secret} write must still be refused"
        );
    }

    // Not a blanket refusal: an ordinary file still reads.
    assert_eq!(
        fs.read(&root.join("main.rs")).await.unwrap(),
        b"fn main() {}",
        "control: the predicate must not have become deny-everything"
    );

    // And the predicate itself takes no backend and cannot be made to.
    assert!(
        policy.is_project_secret(&root.join(".env")),
        "is_project_secret is a lexical, per-access predicate — no backend input"
    );
}
