//! core#366 d5: the unscoped sweep must report a leftover this process never
//! created.
//!
//! EVERY test here CREATES the condition it tests and cleans up after itself,
//! per core#365 c5. A test that waits for a dirty host has exactly the blind
//! spot of the thing it replaces — and the specific blind spot here is worse
//! than usual, because the check this replaces (`conformance.rs`, arm 5) scans
//! a nonce chosen so that nothing can ever be found, so it cannot fail on the
//! axis it appears to cover.
//!
//! Names are prefixed `sweep366-` so they cannot collide with `wedge365-` or
//! with the ids `conformance_matrix` uses; the backend's names are
//! deterministic, so two tests sharing an id would fight over one container.

use wcore_exec_backend::backends::container::{ContainerBackend, NONCE_LABEL};
use wcore_exec_backend::conformance::reference_budget;
use wcore_exec_backend::contract::ExecutionBackend;

fn docker(args: &[&str]) -> std::process::Output {
    std::process::Command::new("docker")
        .args(args)
        .output()
        .expect("the docker client is launchable")
}

fn daemon_answers() -> bool {
    std::process::Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A labelled container in `Created`, exactly as the backend creates one.
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

/// d5 + d1 + d3. The leftover carries a nonce THIS PROCESS HAS NEVER USED, so
/// nothing in the registry holds it and no nonce-scoped scan could ever ask
/// for it — which is the whole defect.
#[tokio::test(flavor = "multi_thread")]
async fn the_unscoped_sweep_reports_a_leftover_from_a_nonce_this_process_never_used() {
    if !daemon_answers() {
        println!("UNEXERCISED — no docker daemon answered a version ping on this host");
        return;
    }
    let name = "sweep366-from-an-earlier-run";
    let nonce = "sweep366-nonce-from-an-earlier-run";
    plant(name, nonce);

    let backend = ContainerBackend::new(reference_budget()).expect("construct");

    // The CONTROL, in the same run: the nonce-scoped scan is asked for a fresh
    // nonce — the shape every real caller supplies — and finds nothing. It is
    // not that the label is missing or the filter is broken; it is that nothing
    // ever asks the question that would find this.
    let scoped = backend
        .scan_orphans("sweep366-nonce-from-todays-run")
        .await
        .expect("scan");
    let sweep = backend.sweep_orphans().await.expect("sweep");

    // Clean up BEFORE asserting, so a red does not leave the next lane a
    // container to trip over.
    remove(name);

    assert!(
        scoped.enumerated,
        "the control must have actually run: {}",
        scoped.method
    );
    assert!(
        !scoped.found.iter().any(|f| f.contains(name)),
        "CONTROL FAILED — a nonce-scoped scan for a fresh nonce must not find an earlier \
         run's leftover; if it does, this test is not measuring what it claims: {:?}",
        scoped.found
    );

    assert!(
        sweep.enumerated,
        "the sweep must have actually enumerated: {}",
        sweep.method
    );
    let hit = sweep.found.iter().find(|s| s.id == name);
    let hit = hit.unwrap_or_else(|| {
        panic!(
            "the unscoped sweep must report a leftover carrying the label under ANY nonce; \
             method={} found={:?}",
            sweep.method, sweep.found
        )
    });
    assert_eq!(
        hit.nonce, nonce,
        "the sweep must carry each surface's own nonce, which is what makes 'this process \
         never created it' answerable"
    );
}

/// d3, at the layer the operator surface reads. The planted leftover must come
/// back marked UNCLAIMED: no live-task registry entry carries its nonce,
/// because the run that would have registered it never happened in this
/// process.
#[tokio::test(flavor = "multi_thread")]
async fn a_swept_leftover_no_live_task_claims_is_marked_unclaimed() {
    if !daemon_answers() {
        println!("UNEXERCISED — no docker daemon answered a version ping on this host");
        return;
    }
    let name = "sweep366-unclaimed";
    let nonce = "sweep366-nonce-nobody-holds";
    plant(name, nonce);

    let swept = wcore_exec_backend::orphan::sweep_all(reference_budget()).await;

    remove(name);

    let swept = swept.expect("sweep_all");
    let container = swept
        .iter()
        .find(|(sweep, _)| sweep.backend_id == "container")
        .expect("the container backend must be a reference backend");
    let hit = container
        .1
        .iter()
        .find(|s| s.surface.id == name)
        .unwrap_or_else(|| {
            panic!(
                "sweep_all must surface the planted leftover: {:?}",
                container.1
            )
        });
    assert!(
        hit.unclaimed,
        "a leftover whose nonce no live task holds must be marked unclaimed: {hit:?}"
    );
}

/// NEGATIVE CONTROL — passes in BOTH arms. A container with NO wayland label is
/// not ours, and the sweep must not report it. Without this, the sweep is
/// satisfied by listing every container on the host, which on a shared daemon
/// would name other people's work as our leftovers.
#[tokio::test(flavor = "multi_thread")]
async fn the_sweep_does_not_claim_an_unlabelled_container() {
    if !daemon_answers() {
        println!("UNEXERCISED — no docker daemon answered a version ping on this host");
        return;
    }
    let name = "sweep366-not-ours";
    let _ = docker(&["rm", "-f", name]);
    let out = docker(&[
        "create",
        "--name",
        name,
        "--network",
        "none",
        "busybox:1.36",
        "true",
    ]);
    assert!(
        out.status.success(),
        "could not create the control: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let backend = ContainerBackend::new(reference_budget()).expect("construct");
    let sweep = backend.sweep_orphans().await;

    remove(name);

    let sweep = sweep.expect("sweep");
    assert!(sweep.enumerated, "{}", sweep.method);
    assert!(
        !sweep.found.iter().any(|s| s.id == name),
        "an unlabelled container belongs to someone else and must not be swept up: {:?}",
        sweep.found
    );
}
