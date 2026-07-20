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

#[cfg(target_os = "macos")]
#[tokio::test]
async fn required_live_macos_docker_rejects_over_budget_result() {
    use std::sync::Arc;
    use wcore_sandbox::backends::docker::DockerBackend;
    use wcore_sandbox::{
        DirectoryAuthority, NetworkPolicy, RetainedWorkspaceAuthority, SandboxCommand,
        SandboxManifest, SandboxRegistry,
    };

    let backend = DockerBackend::connect()
        .await
        .expect("required Docker Desktop daemon");
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
            std::fs::write(coordination.join(format!("{worker}.result")), b"ok").unwrap();
            wait_for_path(&coordination.join("release")).await;
            manager.release_transaction(&workspace).unwrap();
        }
        Err(error) => {
            std::fs::write(
                coordination.join(format!("{worker}.result")),
                error.to_string(),
            )
            .unwrap();
        }
    }
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
