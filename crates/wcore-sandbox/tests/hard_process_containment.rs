//! 06D black-box proof — live qualifying hard-containment backend + descendant
//! process-tree containment, through the `wcore-sandbox` PUBLIC surface only.
//!
//! This integration test never reaches into crate internals. It exercises the
//! same public capability the parent-owned gate executor consumes:
//! `SandboxRegistry::establish_hard_containment` / `verify_hard_containment`
//! (the one-use containment authority) and `SandboxRegistry::execute` over a
//! `HardContainmentFilesystem::to_manifest` policy.
//!
//! The single required test is `required_live`-style: on Linux (the harness
//! platform) it FAILS if bubblewrap is absent — it never silently skips — so a
//! green result is a genuine live qualification. Off Linux it is `#[ignore]`d
//! so the whole suite still compiles and lists on every platform.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::bwrap::BubblewrapBackend;
use wcore_sandbox::{
    HardContainmentFilesystem, HardContainmentMechanism, SandboxCommand, SandboxError,
    SandboxRegistry,
};

/// A synthetic, absolute, non-denied candidate root. Existence is deliberately
/// not required — the hard-containment policy binds it with the backend's
/// `-try` semantics, and the descendant-containment probe below does not chdir
/// into it, so no real directory (and no denied global-temp / home location) is
/// involved.
fn candidate_root() -> PathBuf {
    PathBuf::from("/srv/wl-hard/preflight-candidate")
}

fn scratch_root() -> PathBuf {
    PathBuf::from("/srv/wl-hard/preflight-scratch")
}

fn containment_fs() -> HardContainmentFilesystem {
    HardContainmentFilesystem::new(candidate_root(), vec![scratch_root()])
        .expect("hard-containment policy over synthetic non-denied roots must validate")
}

/// Run `/bin/sh -c '<detached sleep> & exit <code>'` under the qualifying
/// backend's hard containment and return `(exit_code, wall_clock)`.
///
/// The detached `sleep` inherits the child's stdout pipe. If the backend did
/// NOT own and reap the descendant process tree, tearing down the direct child
/// would leave the grandchild alive holding that pipe, so `execute` would block
/// until the 45s sleep exits or the 30s manifest timeout fires (returning
/// `Err(Timeout)`). Because bubblewrap runs the gate in a PID namespace with
/// `--die-with-parent`, the whole tree is torn down the instant the namespace
/// init exits, so `execute` returns promptly with the declared exit code. The
/// wall-clock bound is therefore a falsifiable descendant-reaping assertion.
async fn contained_detached_child_exit(
    registry: &SandboxRegistry,
    exit_code: u8,
) -> (i32, Duration) {
    let fs = containment_fs();
    let cmd = SandboxCommand {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            format!("/bin/sleep 45 & exit {exit_code}"),
        ],
        cwd: None,
    };
    let started = Instant::now();
    let output = registry
        .execute(&fs.to_manifest(), cmd)
        .await
        .expect("contained execution must return an exit status, not block or error");
    (output.exit_code, started.elapsed())
}

/// Live qualification + descendant containment through the public sandbox
/// surface. Proven, in order:
///
/// 1. The bubblewrap backend is installed and admits for delegated execution
///    (owns descendants hard, enforces read-deny, binds cwd, never bypasses
///    containment).
/// 2. It qualifies by a real semantic probe: `establish_hard_containment`
///    mints a one-use authority bound to the PID-namespace mechanism.
/// 3. The authority is one-use and fails closed on spawn-parameter drift; a
///    fresh authority verifies clean when nothing drifts.
/// 4. On BOTH the zero-exit and non-zero-exit terminal paths, a detached
///    descendant is reaped with no residue — `execute` returns promptly with
///    the exact declared exit code instead of blocking on the inherited pipe.
#[tokio::test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "hard containment (bwrap) is Linux-only"
)]
async fn qualified_hard_containment_backend_preflight() {
    let backend = BubblewrapBackend::new();
    assert!(
        backend.is_available(),
        "required live hard-containment backend (bubblewrap) must be installed and usable"
    );
    // Backend admission: only a backend that owns descendants hard, enforces
    // read-deny, and binds cwd authority may run delegated acceptance gates.
    assert!(
        backend.owns_descendants_hard(),
        "qualifying backend must own the descendant process tree"
    );
    assert!(backend.enforces_read_deny());
    assert!(backend.binds_cwd_authority());
    // A stable hard-containment identity is exposed (its fields are crate-private,
    // so the qualifying mechanism is asserted below via the minted authority).
    assert!(
        backend.hard_containment_identity().is_some(),
        "qualifying backend must expose a stable hard-containment identity"
    );

    let registry = SandboxRegistry::new(Arc::new(BubblewrapBackend::new()));
    assert!(
        !registry.bypasses_containment(),
        "a delegated acceptance registry must never bypass containment"
    );
    assert!(registry.binds_workspace_authority());
    assert!(registry.owns_descendants_hard());

    // (2) Live semantic probe qualifies and mints a one-use authority.
    let fs = containment_fs();
    let cmd = SandboxCommand {
        argv: vec!["/bin/echo".into(), "gate".into()],
        cwd: None,
    };
    let authority = registry
        .establish_hard_containment(&fs, &cmd)
        .await
        .expect("live PID-namespace probe must mint hard containment");
    assert_eq!(
        authority.mechanism(),
        HardContainmentMechanism::BubblewrapPidNamespace
    );

    // (3a) The one-use authority fails closed on spawn-parameter drift.
    let drifted = SandboxCommand {
        argv: vec!["/bin/echo".into(), "TAMPERED".into()],
        cwd: None,
    };
    let drift_err = registry
        .verify_hard_containment(authority, &fs, &drifted)
        .expect_err("spawn-parameter drift between mint and spawn must refuse");
    assert!(
        matches!(drift_err, SandboxError::ExecFailed(_)),
        "drift must fail closed, got {drift_err:?}"
    );

    // (3b) A freshly minted authority verifies clean when nothing drifts.
    let fresh = registry
        .establish_hard_containment(&fs, &cmd)
        .await
        .expect("re-probe must mint a fresh one-use authority");
    registry
        .verify_hard_containment(fresh, &fs, &cmd)
        .expect("an undrifted spawn must verify against its own minted authority");

    // (4) Descendant containment on every terminal path. A generous 20s bound
    // sits well below both the 45s detached sleep and the 30s manifest timeout,
    // so a non-reaping backend (grandchild holding the pipe) cannot pass.
    let bound = Duration::from_secs(20);

    let (zero_code, zero_elapsed) = contained_detached_child_exit(&registry, 0).await;
    assert_eq!(zero_code, 0, "zero-exit terminal path must report exit 0");
    assert!(
        zero_elapsed < bound,
        "zero-exit path leaked a descendant: execute took {zero_elapsed:?} (>= {bound:?})"
    );

    let (nonzero_code, nonzero_elapsed) = contained_detached_child_exit(&registry, 7).await;
    assert_eq!(
        nonzero_code, 7,
        "non-zero-exit terminal path must report the exact exit code"
    );
    assert!(
        nonzero_elapsed < bound,
        "non-zero-exit path leaked a descendant: execute took {nonzero_elapsed:?} (>= {bound:?})"
    );
}
