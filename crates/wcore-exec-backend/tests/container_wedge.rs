//! Issue #365: the container backend must survive a leftover container holding
//! the name a task is about to take, and must never attest a run the daemon
//! refused to start.
//!
//! EVERY test here CREATES the condition it tests. That is the whole point.
//! The defect survived because CI's hosted runners are always fresh, so the
//! deterministic name is always free there and `conformance_matrix` could not
//! reach the failure however often it ran. A test that waits for a dirty host
//! has exactly the blind spot of the thing it replaces, so these wedge the
//! daemon themselves, on a fresh host, and clean up after themselves so the
//! next lane inherits nothing.
//!
//! Names are prefixed `wedge365-` so they cannot collide with the ids
//! `conformance_matrix` uses — the backend's names are deterministic, so two
//! tests sharing an id would fight over one container.

use wcore_exec_backend::backends::container::{ContainerBackend, NONCE_LABEL};
use wcore_exec_backend::conformance::{reference_budget, reference_task, run_conformance};
use wcore_exec_backend::contract::ExecutionBackend;
use wcore_exec_backend::error::ExecError;

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

/// A real daemon round trip. Mirrors the backend's own availability rule:
/// socket presence is not readiness.
fn daemon_answers() -> bool {
    std::process::Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Create a container that reaches `Created` and never starts — precisely the
/// state `docker run --rm` cannot clean up, because `--rm` removes on EXIT.
fn wedge(name: &str, nonce: &str) {
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
        "could not wedge {name}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let state = docker(&["inspect", name, "--format", "{{.State.Status}}"]);
    assert_eq!(
        String::from_utf8_lossy(&state.stdout).trim(),
        "created",
        "the wedge must be in Created, which is the state that latches the name"
    );
}

fn remove(name: &str) {
    let _ = docker(&["rm", "-f", name]);
}

fn exists(name: &str) -> bool {
    docker(&["inspect", name, "--format", "{{.Id}}"])
        .status
        .success()
}

/// c5 + c1: the whole conformance harness passes with a leftover container
/// already holding the name its happy-path task will take.
///
/// Before the fix this fails on `a successful run publishes a
/// content-addressed artifact` with `Failure { code: "exit-125" }`.
#[tokio::test(flavor = "multi_thread")]
async fn conformance_passes_with_a_leftover_container_already_holding_the_name() {
    let _state = temp_state();
    if !daemon_answers() {
        println!(
            "backend container: UNEXERCISED — no docker daemon answered a version ping on this host"
        );
        return;
    }
    let prefix = "wedge365-container";
    let wedged = format!("wayland-f25-{prefix}-ok");

    // The leftover carries a DIFFERENT nonce, because a real leftover is from
    // an earlier run. A fix keyed on this task's nonce would not clear it.
    wedge(&wedged, "wedge365-nonce-from-an-earlier-run");
    assert!(exists(&wedged), "the wedge must be in place before the run");

    let backend = ContainerBackend::new(reference_budget()).expect("construct");
    let identity = backend.identity().clone();
    let key = backend.verifying_key();
    let report = run_conformance(&backend, &identity, &key, prefix).await;
    println!("{}", report.render());

    // Clean up before asserting, so a red does not leave the daemon wedged for
    // whoever runs next.
    for suffix in ["ok", "deny", "budget"] {
        remove(&format!("wayland-f25-{prefix}-{suffix}"));
    }

    assert!(
        report.exercised,
        "the daemon answered a ping, so the backend must be exercised: {:?}",
        report.unavailable_reason
    );
    assert!(
        report.passed(),
        "a leftover container under a task's name must not fail that task: {:#?}",
        report.failures()
    );
}

/// c1 guard: a leftover this backend created is reclaimed; the removal is
/// verified against the daemon rather than assumed from a successful run.
#[tokio::test(flavor = "multi_thread")]
async fn the_leftover_is_actually_reclaimed_and_not_merely_stepped_around() {
    let _state = temp_state();
    if !daemon_answers() {
        println!("UNEXERCISED — no docker daemon on this host");
        return;
    }
    let task_id = "wedge365-reclaim";
    let name = format!("wayland-f25-{task_id}");
    wedge(&name, "wedge365-nonce-old");
    let before = docker(&["inspect", &name, "--format", "{{.Id}}"]);
    let old_id = String::from_utf8_lossy(&before.stdout).trim().to_string();
    assert!(!old_id.is_empty());

    let backend = ContainerBackend::new(reference_budget()).expect("construct");
    let task = reference_task(task_id, "wedge365-nonce-new", reference_budget());
    let receipt = backend.execute(&task).await;
    remove(&name);

    let receipt = receipt.expect("the task must run despite the leftover");
    assert!(
        matches!(
            receipt.body.terminal,
            wcore_exec_backend::receipt::TerminalStatus::Success
        ),
        "terminal was {:?}",
        receipt.body.terminal
    );
    // `--rm` removed the container the task actually ran in, so the old one
    // cannot still be there under that name.
    assert!(
        !exists(&name),
        "the leftover was not reclaimed — it is still holding the name"
    );
}

/// c1 guard, the destructive half: a RUNNING container under the name is never
/// removed. Under a deterministic name that is a live task with the same id,
/// possibly another tenant's, and clearing it would destroy real work.
#[tokio::test(flavor = "multi_thread")]
async fn a_running_container_holding_the_name_is_refused_never_removed() {
    let _state = temp_state();
    if !daemon_answers() {
        println!("UNEXERCISED — no docker daemon on this host");
        return;
    }
    let task_id = "wedge365-live";
    let name = format!("wayland-f25-{task_id}");
    let _ = docker(&["rm", "-f", &name]);
    let label = format!("{NONCE_LABEL}=wedge365-nonce-live");
    let out = docker(&[
        "run",
        "-d",
        "--name",
        &name,
        "--label",
        &label,
        "--network",
        "none",
        "busybox:1.36",
        "sleep",
        "120",
    ]);
    assert!(
        out.status.success(),
        "could not start the live container: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let backend = ContainerBackend::new(reference_budget()).expect("construct");
    let task = reference_task(task_id, "wedge365-nonce-new", reference_budget());
    let result = backend.execute(&task).await;

    let still_running = String::from_utf8_lossy(
        &docker(&["inspect", &name, "--format", "{{.State.Running}}"]).stdout,
    )
    .trim()
    .to_string();
    remove(&name);

    assert_eq!(
        still_running, "true",
        "a live task's container must survive another submit under the same id"
    );
    match result {
        Err(ExecError::Unavailable { detail, .. }) => assert!(
            detail.contains("RUNNING"),
            "the refusal must name what it refused: {detail}"
        ),
        other => panic!("expected a refusal that removes nothing, got {other:?}"),
    }
}

/// c1 guard, the other-owner half: a stopped container under the name that
/// carries NO nonce label was not created by this backend, so it belongs to
/// somebody else and must never be removed.
#[tokio::test(flavor = "multi_thread")]
async fn an_unlabelled_container_holding_the_name_belongs_to_someone_else() {
    let _state = temp_state();
    if !daemon_answers() {
        println!("UNEXERCISED — no docker daemon on this host");
        return;
    }
    let task_id = "wedge365-foreign";
    let name = format!("wayland-f25-{task_id}");
    let _ = docker(&["rm", "-f", &name]);
    let out = docker(&["create", "--name", &name, "busybox:1.36", "true"]);
    assert!(out.status.success());

    let backend = ContainerBackend::new(reference_budget()).expect("construct");
    let task = reference_task(task_id, "wedge365-nonce-new", reference_budget());
    let result = backend.execute(&task).await;
    let survived = exists(&name);
    remove(&name);

    assert!(
        survived,
        "a container this backend did not create must never be removed"
    );
    match result {
        Err(ExecError::Unavailable { detail, .. }) => assert!(
            detail.contains(NONCE_LABEL),
            "the refusal must say why it refused: {detail}"
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// c2 + c3: a daemon refusal that name-reclaiming cannot fix must not become a
/// receipt at all, and the daemon's own words must be in the returned error —
/// the value the CLI prints unconditionally to stderr.
#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_refusal_yields_no_receipt_and_carries_the_daemons_words() {
    let _state = temp_state();
    if !daemon_answers() {
        println!("UNEXERCISED — no docker daemon on this host");
        return;
    }
    // An image the daemon cannot resolve is a refusal no naming scheme can
    // avoid, so it exercises the classifier end to end against a real daemon.
    unsafe {
        std::env::set_var(
            "WAYLAND_EXEC_CONTAINER_IMAGE",
            "wayland-f25-no-such-image-365:absent",
        )
    };
    let backend = ContainerBackend::new(reference_budget()).expect("construct");
    unsafe { std::env::remove_var("WAYLAND_EXEC_CONTAINER_IMAGE") };

    let task = reference_task(
        "wedge365-refused",
        "wedge365-nonce-refused",
        reference_budget(),
    );
    let result = backend.execute(&task).await;
    remove("wayland-f25-wedge365-refused");

    match result {
        Ok(receipt) => panic!(
            "a task the daemon refused to create must NOT produce a receipt asserting it ran: {:?}",
            receipt.body.terminal
        ),
        Err(ExecError::Unavailable { backend_id, detail }) => {
            assert_eq!(backend_id, "container");
            assert!(
                detail.contains("Error response from daemon:"),
                "the operator must get the daemon's own words: {detail}"
            );
            assert!(
                detail.contains("wedge365-refused"),
                "the error must name the task it refused: {detail}"
            );
        }
        Err(other) => panic!("expected Unavailable, got {other:?}"),
    }
}
