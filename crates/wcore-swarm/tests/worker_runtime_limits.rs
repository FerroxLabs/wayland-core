//! Real-process proofs for bounded output and cancellation-owned worker trees.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use wcore_config::shell;
use wcore_swarm::{Swarm, SwarmBrief, WorkerStatus};

const OUTPUT_LIMIT_BYTES: usize = 8 * 1024 * 1024;
const OUTPUT_EXHAUSTION_WORKERS: usize = 5;

#[tokio::test]
async fn multi_worker_output_exhaustion_fails_without_retaining_buffers() {
    for stream in ["stdout", "stderr"] {
        let tmp = tempfile::tempdir().expect("temp repo");
        init_repo(tmp.path()).await;
        let swarm = Swarm::new(tmp.path()).expect("create swarm");
        let brief = SwarmBrief {
            task: format!("{stream}-flood"),
            base_branch: "main".into(),
            worker_branch_prefix: format!("swarm/{stream}-flood"),
            worker_command: fixture_argv("flood_worker_fixture"),
            timeout: Duration::from_secs(20),
            env: vec![("WCORE_SWARM_FLOOD_STREAM".into(), stream.into())],
        };

        let started = Instant::now();
        let handles = swarm
            .dispatch(brief, OUTPUT_EXHAUSTION_WORKERS)
            .await
            .expect("dispatch floods");
        assert!(started.elapsed() < Duration::from_secs(20));
        assert_eq!(handles.len(), OUTPUT_EXHAUSTION_WORKERS);
        for handle in handles {
            let reason = match &handle.status {
                WorkerStatus::Failed(reason) => reason,
                status => panic!("flood worker reported {status:?}"),
            };
            assert!(
                reason.contains("output limit exceeded") && reason.contains(stream),
                "{reason}"
            );
            assert!(handle.stdout.is_empty() && handle.stderr.is_empty());
        }
        assert_eq!(
            transaction_entries(tmp.path()),
            0,
            "failed workers must release transaction reservations"
        );
        swarm.cleanup().await.expect("cleanup flood worktree");
    }
}

#[tokio::test]
async fn timeout_releases_workspace_and_capacity_before_return() {
    let tmp = tempfile::tempdir().expect("temp repo");
    init_repo(tmp.path()).await;
    let swarm = Swarm::new(tmp.path()).expect("create swarm");
    let timed_out = swarm
        .dispatch(
            SwarmBrief {
                task: "timeout cleanup".into(),
                base_branch: "main".into(),
                worker_branch_prefix: "swarm/timeout-cleanup".into(),
                worker_command: fixture_argv("sleeping_worker_fixture"),
                timeout: Duration::from_millis(100),
                env: vec![],
            },
            1,
        )
        .await
        .expect("dispatch timeout fixture");
    assert_eq!(timed_out.len(), 1);
    assert_eq!(timed_out[0].status, WorkerStatus::TimedOut);
    assert_eq!(
        transaction_entries(tmp.path()),
        0,
        "timeout must release the owned workspace"
    );

    let successor = swarm
        .dispatch(
            SwarmBrief {
                task: "capacity successor".into(),
                base_branch: "main".into(),
                worker_branch_prefix: "swarm/capacity-successor".into(),
                worker_command: noop_argv(),
                timeout: Duration::from_secs(10),
                env: vec![],
            },
            1,
        )
        .await
        .expect("released capacity admits successor");
    assert_eq!(successor[0].status, WorkerStatus::Succeeded);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn workspace_growth_observer_kills_worker_releases_reservation_and_admits_successor() {
    let tmp = tempfile::tempdir().expect("temp repo");
    init_repo(tmp.path()).await;
    let swarm = Swarm::new(tmp.path()).expect("create swarm");
    let handles = swarm
        .dispatch(
            SwarmBrief {
                task: "workspace growth".into(),
                base_branch: "main".into(),
                worker_branch_prefix: "swarm/growth".into(),
                worker_command: fixture_argv("workspace_growth_fixture"),
                timeout: Duration::from_secs(20),
                env: vec![],
            },
            1,
        )
        .await
        .expect("dispatch growth fixture");
    let reason = match &handles[0].status {
        WorkerStatus::Failed(reason) => reason,
        other => panic!("oversized worker reported {other:?}"),
    };
    assert!(reason.contains("workspace accounting observed"), "{reason}");
    assert_eq!(transaction_entries(tmp.path()), 0);

    let successor = swarm
        .dispatch(
            SwarmBrief {
                task: "growth capacity successor".into(),
                base_branch: "main".into(),
                worker_branch_prefix: "swarm/growth-successor".into(),
                worker_command: noop_argv(),
                timeout: Duration::from_secs(10),
                env: vec![],
            },
            1,
        )
        .await
        .expect("growth cleanup admits successor");
    assert_eq!(successor[0].status, WorkerStatus::Succeeded);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn aborting_dispatch_future_kills_tree_and_releases_workspace() {
    let tmp = tempfile::tempdir().expect("temp repo");
    init_repo(tmp.path()).await;
    let swarm = Arc::new(Swarm::new(tmp.path()).expect("create swarm"));
    let worker_swarm = Arc::clone(&swarm);
    let task = tokio::spawn(async move {
        worker_swarm
            .dispatch(
                SwarmBrief {
                    task: "abort process tree".into(),
                    base_branch: "main".into(),
                    worker_branch_prefix: "swarm/abort".into(),
                    worker_command: fixture_argv("parent_worker_fixture"),
                    timeout: Duration::from_secs(20),
                    env: vec![],
                },
                1,
            )
            .await
    });
    let (workspace, pid_file) = wait_for_worker_pid(tmp.path()).await;
    let descendant_pid = read_host_pid(&pid_file, &workspace.join("checkout"));
    task.abort();
    assert!(
        task.await
            .expect_err("aborted dispatch completed")
            .is_cancelled()
    );
    wait_for_absent(&workspace).await;
    wait_for_process_exit(descendant_pid).await;

    let successor = swarm
        .dispatch(
            SwarmBrief {
                task: "abort capacity successor".into(),
                base_branch: "main".into(),
                worker_branch_prefix: "swarm/abort-successor".into(),
                worker_command: noop_argv(),
                timeout: Duration::from_secs(10),
                env: vec![],
            },
            1,
        )
        .await
        .expect("aborted workspace released capacity");
    assert_eq!(successor[0].status, WorkerStatus::Succeeded);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cleanup_preserves_live_worker_until_owner_releases_it() {
    let tmp = tempfile::tempdir().expect("temp repo");
    init_repo(tmp.path()).await;
    let swarm = Arc::new(Swarm::new(tmp.path()).expect("create swarm"));
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker_swarm = Arc::clone(&swarm);
    let worker_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        worker_swarm
            .dispatch_with_cancel(
                SwarmBrief {
                    task: "live cleanup".into(),
                    base_branch: "main".into(),
                    worker_branch_prefix: "swarm/live-cleanup".into(),
                    worker_command: fixture_argv("parent_worker_fixture"),
                    timeout: Duration::from_secs(20),
                    env: vec![],
                },
                1,
                worker_cancel,
            )
            .await
    });
    let (workspace, pid_file) = wait_for_worker_pid(tmp.path()).await;
    let descendant_pid = read_host_pid(&pid_file, &workspace.join("checkout"));
    let cleanup_error = swarm
        .cleanup()
        .await
        .expect_err("cleanup deleted live worker");
    assert!(
        cleanup_error
            .to_string()
            .contains("active transaction preserved")
    );
    assert!(workspace.exists(), "cleanup deleted live transaction");
    assert!(process_is_running(descendant_pid));

    cancel.cancel();
    let handles = task.await.unwrap().unwrap();
    assert_eq!(handles[0].status, WorkerStatus::Cancelled);
    wait_for_absent(&workspace).await;
    wait_for_process_exit(descendant_pid).await;
}

fn transaction_entries(repo: &Path) -> usize {
    std::fs::read_dir(repo.join(".swarm-worktrees"))
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.file_name() != ".wayland-control")
        })
        .count()
}

#[cfg(target_os = "linux")]
async fn wait_for_worker_pid(repo: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(workspace) = std::fs::read_dir(repo.join(".swarm-worktrees"))
            .expect("worktree root")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name() != ".wayland-control")
            .map(|entry| entry.path())
        {
            let pid_file = workspace.join("checkout/descendant.pid");
            if pid_file.exists() {
                return (workspace, pid_file);
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "worker never started"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(target_os = "linux")]
fn read_host_pid(path: &Path, checkout: &Path) -> u32 {
    let namespace_pid: u32 = std::fs::read_to_string(path)
        .expect("descendant pid")
        .parse()
        .expect("numeric descendant pid");
    for entry in std::fs::read_dir("/proc")
        .expect("read host proc")
        .filter_map(Result::ok)
    {
        let Ok(host_pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        let namespace_matches = status
            .lines()
            .find_map(|line| line.strip_prefix("NSpid:"))
            .and_then(|pids| pids.split_whitespace().next_back())
            .and_then(|pid| pid.parse::<u32>().ok())
            .is_some_and(|pid| pid == namespace_pid);
        let cwd_matches =
            std::fs::read_link(entry.path().join("cwd")).is_ok_and(|cwd| cwd == checkout);
        if namespace_matches && cwd_matches {
            return host_pid;
        }
    }
    panic!(
        "could not resolve namespace PID {namespace_pid} under {}",
        checkout.display()
    );
}

#[cfg(target_os = "linux")]
async fn wait_for_absent(path: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while path.exists() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!path.exists(), "workspace survived owner release");
}

#[cfg(target_os = "linux")]
async fn wait_for_process_exit(pid: u32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while process_is_running(pid) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !process_is_running(pid),
        "descendant {pid} survived owner release: {}",
        std::fs::read_to_string(format!("/proc/{pid}/status"))
            .unwrap_or_else(|error| error.to_string())
    );
}

#[cfg(target_os = "linux")]
fn process_is_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    stat.rsplit_once(") ")
        .and_then(|(_, fields)| fields.chars().next())
        .is_some_and(|state| state != 'Z')
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancellation_kills_worker_descendant_and_releases_owned_workspace() {
    let tmp = tempfile::tempdir().expect("temp repo");
    init_repo(tmp.path()).await;
    let swarm = Arc::new(Swarm::new(tmp.path()).expect("create swarm"));
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker_swarm = Arc::clone(&swarm);
    let worker_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        worker_swarm
            .dispatch_with_cancel(
                SwarmBrief {
                    task: "cancel process tree".into(),
                    base_branch: "main".into(),
                    worker_branch_prefix: "swarm/cancel".into(),
                    worker_command: fixture_argv("parent_worker_fixture"),
                    timeout: Duration::from_secs(20),
                    env: vec![],
                },
                1,
                worker_cancel,
            )
            .await
    });
    let (workspace, pid_file) = wait_for_worker_pid(tmp.path()).await;
    let descendant_pid = read_host_pid(&pid_file, &workspace.join("checkout"));

    cancel.cancel();
    let handles = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("dispatch remained blocked after cancellation")
        .expect("dispatch task")
        .expect("dispatch result");
    assert_eq!(handles.len(), 1);
    assert_eq!(handles[0].status, WorkerStatus::Cancelled);
    assert!(
        !workspace.exists(),
        "cancelled worker retained its transaction workspace"
    );

    wait_for_process_exit(descendant_pid).await;
    let successor = swarm
        .dispatch(
            SwarmBrief {
                task: "cancel capacity successor".into(),
                base_branch: "main".into(),
                worker_branch_prefix: "swarm/cancel-successor".into(),
                worker_command: noop_argv(),
                timeout: Duration::from_secs(10),
                env: vec![],
            },
            1,
        )
        .await
        .expect("cancelled workspace released capacity");
    assert_eq!(successor[0].status, WorkerStatus::Succeeded);
    swarm.cleanup().await.expect("explicit cleanup");
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn required_live_macos_docker_cancellation_cleans_container_workspace() {
    let mut info = shell::shell_command_argv("docker", &["info"]);
    assert!(
        info.status()
            .await
            .expect("required Docker Desktop CLI")
            .success(),
        "required Docker Desktop daemon is unavailable"
    );
    let tmp = tempfile::tempdir().expect("temp repo");
    init_repo(tmp.path()).await;
    let swarm = Arc::new(Swarm::new(tmp.path()).expect("create swarm"));
    let cancel = tokio_util::sync::CancellationToken::new();
    let marker = format!("wcore-macos-cancel-{}", uuid::Uuid::new_v4().simple());
    let worker_swarm = Arc::clone(&swarm);
    let worker_cancel = cancel.clone();
    let worker_marker = marker.clone();
    let task = tokio::spawn(async move {
        worker_swarm
            .dispatch_with_cancel(
                SwarmBrief {
                    task: "required macOS Docker cancellation".into(),
                    base_branch: "main".into(),
                    worker_branch_prefix: "swarm/required-macos-cancel".into(),
                    worker_command: vec!["sh".into(), "-c".into(), "sleep 30".into()],
                    timeout: Duration::from_secs(40),
                    env: vec![("WCORE_DOCKER_MARKER".into(), worker_marker)],
                },
                1,
                worker_cancel,
            )
            .await
    });
    wait_for_docker_marker(&marker, true).await;
    cancel.cancel();
    let handles = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("Docker cancellation did not finish teardown")
        .expect("dispatch task")
        .expect("dispatch result");
    assert_eq!(handles[0].status, WorkerStatus::Cancelled);
    assert_eq!(transaction_entries(tmp.path()), 0);
    wait_for_docker_marker(&marker, false).await;
}

#[cfg(target_os = "macos")]
async fn wait_for_docker_marker(marker: &str, expected: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let mut command = shell::shell_command_argv("docker", &["ps", "-q"]);
        let output = command.output().await.expect("query Docker containers");
        assert!(output.status.success(), "docker ps failed");
        let mut present = false;
        for id in String::from_utf8_lossy(&output.stdout).lines() {
            let mut inspect = shell::shell_command_argv(
                "docker",
                &["inspect", "--format", "{{json .Config.Env}}", id],
            );
            let inspected = inspect.output().await.expect("inspect Docker container");
            assert!(inspected.status.success(), "docker inspect failed for {id}");
            present |= String::from_utf8_lossy(&inspected.stdout).contains(marker);
        }
        if present == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Docker marker {marker} presence remained {present}, expected {expected}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn many_entry_accounting_does_not_block_cancellation() {
    let tmp = tempfile::tempdir().expect("temp repo");
    init_repo(tmp.path()).await;
    let swarm = Arc::new(Swarm::new(tmp.path()).expect("create swarm"));
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker_swarm = Arc::clone(&swarm);
    let worker_cancel = cancel.clone();
    let task = tokio::spawn(async move {
        worker_swarm
            .dispatch_with_cancel(
                SwarmBrief {
                    task: "many-entry cancellation".into(),
                    base_branch: "main".into(),
                    worker_branch_prefix: "swarm/many-entry-cancel".into(),
                    worker_command: fixture_argv("many_entry_worker_fixture"),
                    timeout: Duration::from_secs(30),
                    env: vec![],
                },
                1,
                worker_cancel,
            )
            .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let ready = std::fs::read_dir(tmp.path().join(".swarm-worktrees"))
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.path().join("checkout/many-entry-ready").is_file());
        if ready {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "worker never became ready"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let started = Instant::now();
    cancel.cancel();
    let handles = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("filesystem accounting blocked cancellation")
        .expect("dispatch task")
        .expect("dispatch result");
    assert_eq!(handles[0].status, WorkerStatus::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(transaction_entries(tmp.path()), 0);
}

fn fixture_argv(name: &str) -> Vec<String> {
    vec![
        std::env::current_exe()
            .expect("current test executable")
            .to_string_lossy()
            .into_owned(),
        "--ignored".into(),
        "--exact".into(),
        name.into(),
        "--nocapture".into(),
    ]
}

fn noop_argv() -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/c".into(), "rem".into()]
    } else {
        vec!["true".into()]
    }
}

#[test]
#[ignore = "subprocess fixture"]
fn sleeping_worker_fixture() {
    std::thread::sleep(Duration::from_secs(20));
}

#[test]
#[ignore = "subprocess fixture"]
fn flood_worker_fixture() {
    let stream = std::env::var("WCORE_SWARM_FLOOD_STREAM").expect("flood stream");
    let chunk = [b'x'; 64 * 1024];
    let chunks = OUTPUT_LIMIT_BYTES / chunk.len() + 2;
    match stream.as_str() {
        "stdout" => write_flood(std::io::stdout().lock(), &chunk, chunks),
        "stderr" => write_flood(std::io::stderr().lock(), &chunk, chunks),
        other => panic!("unknown flood stream {other}"),
    }
    std::thread::sleep(Duration::from_secs(20));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "subprocess fixture"]
fn workspace_growth_fixture() {
    let scratch = std::path::PathBuf::from(
        std::env::var("WAYLAND_SWARM_SCRATCH").expect("sandbox scratch path"),
    );
    let file = std::fs::File::create(scratch.join("oversized-worker-output"))
        .expect("create oversized output");
    file.set_len(9 * 1024 * 1024 * 1024)
        .expect("grow sparse worker output");
    std::thread::sleep(Duration::from_secs(20));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "subprocess fixture"]
fn many_entry_worker_fixture() {
    let root = std::env::current_dir().unwrap().join("many-entries");
    std::fs::create_dir(&root).unwrap();
    for index in 0..20_000 {
        std::fs::write(root.join(format!("entry-{index}")), b"x").unwrap();
    }
    std::fs::write("many-entry-ready", b"ready").unwrap();
    std::thread::sleep(Duration::from_secs(30));
}

fn write_flood(mut output: impl Write, chunk: &[u8], chunks: usize) {
    for _ in 0..chunks {
        output.write_all(chunk).expect("write flood");
    }
    output.flush().expect("flush flood");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "subprocess fixture"]
#[allow(clippy::zombie_processes)] // The owning runtime must kill this descendant process tree.
fn parent_worker_fixture() {
    let cwd = std::env::current_dir().expect("worker cwd");
    let pid_file = cwd.join("descendant.pid");
    let survived_file = cwd.join("descendant-survived");
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "descendant_worker_fixture",
            "--nocapture",
        ])
        .env("WCORE_SWARM_PID_FILE", pid_file)
        .env("WCORE_SWARM_SURVIVED_FILE", survived_file)
        .spawn()
        .expect("spawn descendant");
    drop(child);
    std::thread::sleep(Duration::from_secs(20));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "subprocess fixture"]
fn descendant_worker_fixture() {
    let pid_file = std::env::var("WCORE_SWARM_PID_FILE").expect("pid file");
    let survived_file = std::env::var("WCORE_SWARM_SURVIVED_FILE").expect("survived file");
    let host_pid = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("NSpid:"))
                .and_then(|pids| pids.split_whitespace().next())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| std::process::id().to_string());
    std::fs::write(pid_file, host_pid).expect("write descendant host pid");
    std::thread::sleep(Duration::from_secs(1));
    std::fs::write(survived_file, "survived").expect("write survived marker");
    std::thread::sleep(Duration::from_secs(20));
}

async fn init_repo(path: &Path) {
    run_git(path, &["init", "-q", "-b", "main"]).await;
    std::fs::write(path.join("README.md"), "worker runtime fixture\n").unwrap();
    std::fs::write(path.join(".gitignore"), ".swarm-worktrees/\n").unwrap();
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
