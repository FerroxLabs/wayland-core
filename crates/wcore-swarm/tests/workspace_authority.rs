//! Cross-process workspace admission authority proofs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use wcore_config::shell;
use wcore_swarm::worktree::{WorkspaceCapacity, WorktreeManager};

#[tokio::test]
async fn independent_cli_processes_cannot_overbook_shared_capacity() {
    let repo = tempfile::tempdir().expect("repo");
    init_repo(repo.path()).await;
    let workspace_parent = tempfile::tempdir().expect("workspace parent");
    let workspace_root = workspace_parent.path().join("shared-swarm");
    let coordination = tempfile::tempdir().expect("coordination");
    let test_executable = std::env::current_exe().expect("current test executable");
    let test_executable = test_executable.to_string_lossy().into_owned();

    let mut children = Vec::new();
    for worker in ["worker-a", "worker-b"] {
        let mut command = shell::shell_command_argv(
            &test_executable,
            &[
                "--ignored",
                "--exact",
                "capacity_registration_fixture",
                "--nocapture",
            ],
        );
        command
            .env("WCORE_CAPACITY_REPO", repo.path())
            .env("WCORE_CAPACITY_ROOT", &workspace_root)
            .env("WCORE_CAPACITY_COORD", coordination.path())
            .env("WCORE_CAPACITY_WORKER", worker);
        children.push(command.spawn().expect("spawn capacity fixture"));
    }

    for worker in ["worker-a", "worker-b"] {
        wait_for_path(&coordination.path().join(format!("{worker}.ready"))).await;
    }
    std::fs::write(coordination.path().join("go"), b"go").unwrap();
    let result_paths = [
        coordination.path().join("worker-a.result"),
        coordination.path().join("worker-b.result"),
    ];
    for path in &result_paths {
        wait_for_path(path).await;
    }
    let results = result_paths
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        results
            .iter()
            .filter(|result| result.as_str() == "ok")
            .count(),
        1,
        "exactly one process must own the aggregate reservation: {results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result.contains("aggregate workspace budget exhausted"))
            .count(),
        1,
        "losing process must fail admission: {results:?}"
    );

    std::fs::write(coordination.path().join("release"), b"release").unwrap();
    for mut child in children {
        assert!(child.wait().await.unwrap().success());
    }
    assert_eq!(
        std::fs::read_dir(&workspace_root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() != ".wayland-control")
            .count(),
        0
    );
}

/// Preconditions for the `required_live_macos_*` delegated-Docker case below.
///
/// Returns `None` after printing a `skip:` line that NAMES the missing
/// precondition, rather than returning a silent green. A skip is not a pass:
/// `.github/workflows/macos-docker-gate.yml` runs these under `--nocapture`
/// (libtest DISCARDS a passing test's stderr otherwise) and fails its gate on
/// any `skip:` in the output, so an un-opted-in or Docker-less host can never
/// be read as certification. `DockerBackend::connect` is the single probe because it
/// covers BOTH ways this can be unavailable — a build without the
/// `wcore-sandbox/live-docker` feature (`DockerDisabled`) and a host with no
/// reachable daemon (`DockerIo`).
#[cfg(target_os = "macos")]
async fn live_macos_docker_backend() -> Option<wcore_sandbox::backends::docker::DockerBackend> {
    if std::env::var("WAYLAND_SANDBOX_LIVE_DOCKER").is_err() {
        eprintln!(
            "skip: WAYLAND_SANDBOX_LIVE_DOCKER not set \
             (host has not opted into live delegated-Docker execution)"
        );
        return None;
    }
    match wcore_sandbox::backends::docker::DockerBackend::connect().await {
        Ok(backend) => Some(backend),
        Err(error) => {
            eprintln!(
                "skip: delegated Docker backend unavailable ({error}) — needs the \
                 `wcore-sandbox/live-docker` feature and a running Docker daemon"
            );
            None
        }
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "live macOS delegated-Docker acceptance; run via `--run-ignored all` (or `-- --ignored`) with WAYLAND_SANDBOX_LIVE_DOCKER=1"]
async fn required_live_macos_docker_rejects_over_budget_result() {
    use std::sync::Arc;
    use wcore_sandbox::{
        DirectoryAuthority, NetworkPolicy, RetainedWorkspaceAuthority, SandboxCommand,
        SandboxManifest, SandboxRegistry,
    };

    let Some(backend) = live_macos_docker_backend().await else {
        return;
    };
    let owner = tempfile::tempdir().expect("owner");
    let checkout = owner.path().join("checkout");
    let scratch = owner.path().join("scratch");
    std::fs::create_dir(&checkout).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    std::fs::write(checkout.join("authoritative"), b"before").unwrap();
    let root = DirectoryAuthority::open(owner.path()).unwrap();
    let retained = RetainedWorkspaceAuthority::new(
        root.clone(),
        root.open_child_directory("checkout").unwrap(),
        "required-macos-over-budget",
    )
    .unwrap();
    let manifest = SandboxManifest {
        fs_read_allow: vec![checkout.clone(), scratch.clone()],
        fs_write_allow: vec![checkout.clone(), scratch],
        network: NetworkPolicy::Deny,
        image: "alpine:3.19".to_owned(),
        ..Default::default()
    };
    let error = SandboxRegistry::new(Arc::new(backend))
        .execute_with_workspace_authority(
            &manifest,
            SandboxCommand {
                argv: vec![
                    "sh".into(),
                    "-c".into(),
                    "dd if=/dev/zero of=oversized bs=2048 count=1 2>/dev/null".into(),
                ],
                cwd: Some(checkout.clone()),
            },
            retained,
            1024,
            || Ok(()),
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect_err("over-budget Docker result must fail closed");
    assert!(
        error.to_string().contains("exceeds 1024 bytes"),
        "{error:?}"
    );
    assert_eq!(
        std::fs::read(checkout.join("authoritative")).unwrap(),
        b"before"
    );
    assert!(!checkout.join("oversized").exists());
}

#[tokio::test]
#[ignore = "subprocess fixture"]
async fn capacity_registration_fixture() {
    let repo = PathBuf::from(std::env::var("WCORE_CAPACITY_REPO").unwrap());
    let root = PathBuf::from(std::env::var("WCORE_CAPACITY_ROOT").unwrap());
    let coordination = PathBuf::from(std::env::var("WCORE_CAPACITY_COORD").unwrap());
    let worker = std::env::var("WCORE_CAPACITY_WORKER").unwrap();
    let manager = WorktreeManager::new_with_workspace_root(&repo, &root).unwrap();
    let head = manager.pinned_head().await.unwrap();
    std::fs::write(coordination.join(format!("{worker}.ready")), b"ready").unwrap();
    wait_for_path(&coordination.join("go")).await;
    let capacity = WorkspaceCapacity {
        available_bytes: 1024 * 1024 * 1024,
        safety_margin_bytes: 0,
        max_transaction_bytes: 64 * 1024 * 1024,
        max_aggregate_bytes: 64 * 1024 * 1024,
    };
    match manager
        .create_isolated_checkout(&worker, &format!("swarm/{worker}"), &head, capacity)
        .await
    {
        Ok(workspace) => {
            publish_result(&coordination, &worker, "ok");
            wait_for_path(&coordination.join("release")).await;
            manager.release_transaction(&workspace).unwrap();
        }
        Err(error) => {
            publish_result(&coordination, &worker, &error.to_string());
        }
    }
}

/// Publish a worker verdict so the parent can never read it half-written.
///
/// The parent waits on the result path EXISTING and then reads it, but
/// `std::fs::write` creates-and-truncates before it writes, so the file is
/// observable as zero bytes for a window in between. Measured on
/// `macos-latest` under 12 concurrent copies of this test: one run failed with
/// `["", "dispatch admission refused: aggregate workspace budget exhausted"]`
/// — an empty verdict, which reads as a product defect and is a harness race.
/// A rename into place is atomic, so the parent sees either nothing or the
/// whole verdict.
fn publish_result(coordination: &Path, worker: &str, verdict: &str) {
    let staged = coordination.join(format!("{worker}.result.staged"));
    std::fs::write(&staged, verdict).unwrap();
    std::fs::rename(&staged, coordination.join(format!("{worker}.result"))).unwrap();
}

async fn wait_for_path(path: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !path.exists() {
        assert!(tokio::time::Instant::now() < deadline, "{}", path.display());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn init_repo(path: &Path) {
    run_git(path, &["init", "-q", "-b", "main"]).await;
    std::fs::write(path.join("README.md"), "capacity fixture\n").unwrap();
    run_git(path, &["add", "."]).await;
    run_git(
        path,
        &[
            "-c",
            "user.email=swarm@test.invalid",
            "-c",
            "user.name=Swarm Test",
            "commit",
            "-qm",
            "fixture",
        ],
    )
    .await;
}

async fn run_git(cwd: &Path, args: &[&str]) {
    let mut command = shell::shell_command_argv("git", args);
    command.current_dir(cwd);
    let status = command.status().await.expect("spawn git");
    assert!(status.success(), "git {args:?} failed");
}
