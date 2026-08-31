//! Issue #366: the container orphan scan was NONCE-SCOPED, so it could never
//! see a leftover from an earlier run.
//!
//! Every caller supplied a nonce that cannot match a leftover — `cancel()`
//! re-reads the nonce of the task it is cancelling, ordinary operation carries
//! a fresh nonce per run, and the conformance check deliberately scans a nonce
//! nothing ever ran under. The product could therefore only ever answer "are
//! there orphans from the run whose nonce I am already holding".
//!
//! THIS FILE PLANTS THE CONDITION IT TESTS, per #365 c5. A test that waits for
//! a dirty host has exactly the blind spot of the thing it replaces: CI's
//! runners are always fresh, so a leftover never exists there however often the
//! suite runs. Each test creates its leftover, asserts, and removes it again,
//! so a failing run leaves the next lane nothing.
//!
//! Names are prefixed `orphan366-` so they cannot collide with the ids
//! `conformance_matrix` or `container_wedge` use.

use wcore_exec_backend::backends::container::{ContainerBackend, NONCE_LABEL};
use wcore_exec_backend::conformance::reference_budget;
use wcore_exec_backend::contract::ExecutionBackend;

fn temp_state() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    unsafe { std::env::set_var("WAYLAND_EXEC_BACKEND_STATE_DIR", dir.path()) };
    dir
}

fn docker(args: &[&str]) -> std::process::Output {
    std::process::Command::new("docker")
        .args(args)
        .output()
        .expect("the docker client is launchable")
}

/// A real daemon round trip. Socket presence is not readiness.
fn daemon_answers() -> bool {
    std::process::Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Create a container that reaches `Created` and never starts — the state
/// `docker run --rm` cannot clean up, because `--rm` removes on EXIT. This is
/// the shape both leftovers in #365 were found in.
fn plant(name: &str, nonce: &str) {
    let _ = docker(&["rm", "-f", name]);
    let label = format!("{NONCE_LABEL}={nonce}");
    let out = docker(&[
        "create",
        "--name",
        name,
        "--label",
        &label,
        "--network",
        "none",
        "busybox:1.36",
        "true",
    ]);
    assert!(
        out.status.success(),
        "could not plant {name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn remove(name: &str) {
    let _ = docker(&["rm", "-f", name]);
}

/// THE CRITERION (#366 d5, and d1/d3 through it).
///
/// The leftover carries a nonce this process has NEVER used and which is not
/// in its live registry — exactly the state a real leftover is in, because its
/// own run already called `registry::forget`. The scoped scan is run in the
/// same pass, against a fresh nonce, as the DISCRIMINATING CONTROL: it is the
/// query the product used to issue, and it must come back empty on the very
/// host where the unscoped one finds the container. Without that arm this test
/// would pass against a scanner that simply returns everything.
#[tokio::test]
async fn the_unscoped_scan_reports_a_leftover_from_a_nonce_this_process_never_used() {
    if !daemon_answers() {
        eprintln!("SKIP: no docker daemon answers; this test requires a real one");
        return;
    }
    let _state = temp_state();
    let name = "orphan366-leftover";
    let stale_nonce = "orphan366-nonce-from-an-earlier-run";
    plant(name, stale_nonce);

    let backend = ContainerBackend::new(reference_budget()).expect("container backend");

    let unscoped = backend
        .scan_all_orphans()
        .await
        .expect("the unscoped scan must answer");
    assert!(
        unscoped.enumerated,
        "the unscoped scan did not enumerate ({}), so it proves nothing",
        unscoped.method
    );
    assert!(
        unscoped.unsupported_reason.is_none(),
        "the container backend must support the unscoped scan: {:?}",
        unscoped.unsupported_reason
    );

    let hit = unscoped.found.iter().find(|o| o.handle == name);
    assert!(
        hit.is_some(),
        "the unscoped scan did not report the planted leftover {name}; found {:?} via {}",
        unscoped.found,
        unscoped.method
    );
    let hit = hit.expect("checked above");
    assert_eq!(hit.nonce, stale_nonce, "the scan misread the label value");
    assert!(
        !hit.known_to_this_process,
        "a container whose nonce is not in this process's live registry must be reported as a \
         LEFTOVER; reporting only what this process already knows about answers a question \
         nobody needed asked (#366 d3)"
    );
    assert!(
        unscoped.leftovers().any(|o| o.handle == name),
        "the leftover must reach the operator-facing `leftovers()` projection, not only the \
         raw list"
    );

    // DISCRIMINATING CONTROL — the query the product used to issue. Same host,
    // same daemon, same instant, and the leftover is still there.
    let fresh_nonce = "orphan366-nonce-from-todays-run";
    let scoped = backend
        .scan_orphans(fresh_nonce)
        .await
        .expect("the scoped scan must answer");
    assert!(scoped.enumerated, "the control did not enumerate either");
    assert!(
        scoped.found.is_empty(),
        "the scoped scan was supposed to be BLIND to this leftover — if it can see it, the \
         defect #366 describes does not exist and this test is measuring something else: {:?}",
        scoped.found
    );

    remove(name);
}

/// NEGATIVE CONTROL, and it is the arm that blocks the cheap fix. A scan that
/// reported every container on the host — or that marked everything a leftover
/// — would pass the test above and be useless. A container this process DOES
/// hold a live registry entry for must come back `known_to_this_process`, and
/// an unlabelled container must not appear at all.
#[tokio::test]
async fn an_unlabelled_container_is_not_ours_and_a_known_nonce_is_not_a_leftover() {
    if !daemon_answers() {
        eprintln!("SKIP: no docker daemon answers; this test requires a real one");
        return;
    }
    let state = temp_state();
    let stranger = "orphan366-stranger";
    let _ = docker(&["rm", "-f", stranger]);
    let out = docker(&[
        "create",
        "--name",
        stranger,
        "--network",
        "none",
        "busybox:1.36",
        "true",
    ]);
    assert!(
        out.status.success(),
        "could not create the unlabelled stranger: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let live_name = "orphan366-live";
    let live_nonce = "orphan366-nonce-this-process-holds";
    plant(live_name, live_nonce);
    // Record the nonce the way a live task would, in this test's private state
    // dir, so `known_to_this_process` has something true to find.
    wcore_exec_backend::registry::record(&wcore_exec_backend::registry::LiveTask {
        task_id: "orphan366-live-task".into(),
        nonce: live_nonce.into(),
        backend_id: wcore_exec_backend::backends::container::BACKEND_ID.into(),
        kind: wcore_exec_backend::contract::BackendKind::Container,
        pid: None,
        handle: Some(live_name.into()),
        started_unix_ms: wcore_exec_backend::registry::now_unix_ms(),
    })
    .expect("recording a live task in a private state dir");

    let backend = ContainerBackend::new(reference_budget()).expect("container backend");
    let scan = backend
        .scan_all_orphans()
        .await
        .expect("the unscoped scan must answer");
    assert!(
        scan.enumerated,
        "the scan did not enumerate: {}",
        scan.method
    );

    assert!(
        !scan.found.iter().any(|o| o.handle == stranger),
        "an unlabelled container was never created by this product and must not be reported \
         as ours: {:?}",
        scan.found
    );
    let live = scan
        .found
        .iter()
        .find(|o| o.handle == live_name)
        .expect("a labelled container this process holds must still be enumerated");
    assert!(
        live.known_to_this_process,
        "a nonce that IS in the live registry must not be called a leftover, or every running \
         task becomes an orphan report"
    );
    assert!(
        !scan.leftovers().any(|o| o.handle == live_name),
        "`leftovers()` must exclude what this process is holding"
    );

    remove(stranger);
    remove(live_name);
    drop(state);
}
