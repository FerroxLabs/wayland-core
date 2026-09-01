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

/// A private state directory for THIS TEST, injected PER THREAD.
///
/// Deliberately NOT `WAYLAND_EXEC_BACKEND_STATE_DIR`. That variable is a
/// PROCESS global, and `cargo test` -- which the shared-process CI leg runs,
/// and which nextest's process-per-test can never see -- puts every test of
/// this binary on a thread of ONE process. The env var therefore pointed
/// every concurrently-running sibling's registry at this `TempDir`, which was
/// then deleted out from under them when it dropped. That is the race that
/// failed `conformance_matrix` on ci-linux at e37e72f0b: `reference_backends`
/// could not construct because its state dir had just been removed by a
/// sibling finishing first (gh#1233).
///
/// `StateDirGuard` is the per-thread override built for exactly this; see
/// `registry.rs`, whose doc comment names this defect. `fail_closed_matrix`
/// was migrated to it and this binary was not. Both halves are returned so
/// the directory outlives every read taken through the guard.
fn temp_state() -> (
    tempfile::TempDir,
    wcore_exec_backend::registry::StateDirGuard,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let guard = wcore_exec_backend::registry::StateDirGuard::set(dir.path());
    (dir, guard)
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
    // The backend removes the container it created itself now that `--rm` is
    // gone (see `remove_container`), so the old one cannot still be there
    // under that name either.
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
    let backend =
        ContainerBackend::with_image(reference_budget(), "wayland-f25-no-such-image-365:absent")
            .expect("construct");

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

/// c2, THE CLASS NOBODY THOUGHT OF — docker's CLIENT-side 125.
///
/// The first fix keyed on docker's `Error response from daemon:` line. That
/// line only appears on DAEMON-side refusals, and docker's 125 class is bigger
/// than that: an unparsable image reference is rejected by the CLI before the
/// daemon is ever contacted, exit 125, marker count ZERO. At 099b83e6 this
/// printed `terminal: Failure { code: "exit-125" }`, exited 0, and wrote a
/// 2527-byte receipt carrying a real ed25519 attestation — for a run in which
/// no container was created and no argv executed. A signed lie.
///
/// This test drives the REAL backend against the REAL docker client, so it
/// fails if any future discriminator regresses to reading message text.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_side_refusal_yields_no_receipt_either() {
    let _state = temp_state();
    if !daemon_answers() {
        println!("UNEXERCISED — no docker daemon on this host");
        return;
    }
    // Uppercase is not a legal repository name, so the docker CLI rejects the
    // reference itself. Nothing reaches the daemon and nothing is created.
    let backend =
        ContainerBackend::with_image(reference_budget(), "BadImage:Tag").expect("construct");

    let task_id = "wedge365-clientside";
    let task = reference_task(task_id, "wedge365-nonce-clientside", reference_budget());
    let result = backend.execute(&task).await;
    remove(&format!("wayland-f25-{task_id}"));

    match result {
        Ok(receipt) => panic!(
            "a refusal the docker CLIENT made must NOT produce a receipt asserting the task ran: {:?}",
            receipt.body.terminal
        ),
        Err(ExecError::Unavailable { backend_id, detail }) => {
            assert_eq!(backend_id, "container");
            assert!(
                detail.contains(task_id),
                "the error must name the task it refused: {detail}"
            );
            // The proof this is the client-side class and not the daemon one:
            // docker's daemon marker is absent, which is exactly why the old
            // discriminator let it through.
            assert!(
                !detail.contains("Error response from daemon:"),
                "this test is only meaningful for the class that carries NO daemon line: {detail}"
            );
            assert!(
                detail.contains("invalid reference format"),
                "the operator must get docker's own words: {detail}"
            );
        }
        Err(other) => panic!("expected Unavailable, got {other:?}"),
    }
}

/// c2 POLARITY, live: a task that genuinely runs and exits 125 by itself MUST
/// still be attested as a real run.
///
/// Overcorrecting is the inverse defect and just as much a lie in a signed
/// receipt. This is the direction the previous discriminator got right, and it
/// is asserted here against a real daemon rather than only over fixtures — the
/// unit test could be made to pass vacuously by a rule that never looks at the
/// daemon at all, and this one cannot.
#[tokio::test(flavor = "multi_thread")]
async fn a_task_that_exits_125_on_its_own_is_attested_as_a_real_run() {
    let _state = temp_state();
    if !daemon_answers() {
        println!("UNEXERCISED — no docker daemon on this host");
        return;
    }
    let task_id = "wedge365-self125";
    let name = format!("wayland-f25-{task_id}");
    remove(&name);

    let mut task = reference_task(task_id, "wedge365-nonce-self125", reference_budget());
    // The container starts, runs a shell, and picks 125 for itself.
    task.argv = vec!["sh".into(), "-c".into(), "exit 125".into()];

    let result = backend_execute(&task).await;
    remove(&name);

    let receipt = result.expect("a task that really ran must produce a receipt, not a refusal");
    match &receipt.body.terminal {
        wcore_exec_backend::receipt::TerminalStatus::Failure { code } => assert_eq!(
            code, "exit-125",
            "the task's own 125 must be reported as its own status"
        ),
        other => panic!("expected an honest ran-and-failed receipt, got {other:?}"),
    }
    // And the container really is gone afterwards, `--rm` or not.
    assert!(
        !exists(&name),
        "the backend must remove the container it created"
    );
}

async fn backend_execute(
    task: &wcore_exec_backend::contract::ExecutionTask,
) -> Result<wcore_exec_backend::receipt::ExecutionReceipt, ExecError> {
    let backend = ContainerBackend::new(reference_budget()).expect("construct");
    backend.execute(task).await
}
