//! Real-process proofs for bounded output and cancellation-owned worker trees.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use wcore_config::shell;
use wcore_swarm::{Swarm, SwarmBrief, WorkerStatus};

const OUTPUT_LIMIT_BYTES: usize = 8 * 1024 * 1024;

#[tokio::test]
async fn stdout_and_stderr_floods_fail_without_retaining_oversized_buffers() {
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
        let handles = swarm.dispatch(brief, 1).await.expect("dispatch flood");
        assert!(started.elapsed() < Duration::from_secs(10));
        let handle = handles.into_iter().next().expect("worker result");
        let reason = match &handle.status {
            WorkerStatus::Failed(reason) => reason,
            status => panic!("flood worker reported {status:?}"),
        };
        assert!(
            reason.contains("output limit exceeded") && reason.contains(stream),
            "{reason}"
        );
        assert!(handle.stdout.is_empty() && handle.stderr.is_empty());
        swarm.cleanup().await.expect("cleanup flood worktree");
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancellation_kills_worker_descendant_and_preserves_worktree_evidence() {
    let tmp = tempfile::tempdir().expect("temp repo");
    init_repo(tmp.path()).await;
    let pid_file = tmp.path().join("descendant.pid");
    let survived_file = tmp.path().join("descendant-survived");
    let swarm = Swarm::new(tmp.path()).expect("create swarm");
    let brief = SwarmBrief {
        task: "cancel process tree".into(),
        base_branch: "main".into(),
        worker_branch_prefix: "swarm/cancel".into(),
        worker_command: fixture_argv("parent_worker_fixture"),
        timeout: Duration::from_secs(20),
        env: vec![
            (
                "WCORE_SWARM_PID_FILE".into(),
                pid_file.to_string_lossy().into_owned(),
            ),
            (
                "WCORE_SWARM_SURVIVED_FILE".into(),
                survived_file.to_string_lossy().into_owned(),
            ),
        ],
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let dispatch = swarm.dispatch_with_cancel(brief, 1, cancel.clone());
    tokio::pin!(dispatch);
    let ready_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        tokio::select! {
            result = &mut dispatch => panic!("worker exited before cancellation: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(10)) => {
                if pid_file.exists() {
                    break;
                }
                assert!(tokio::time::Instant::now() < ready_deadline, "descendant never started");
            }
        }
    }
    let descendant_pid = std::fs::read_to_string(&pid_file)
        .expect("descendant pid")
        .parse::<u32>()
        .expect("numeric descendant pid");

    cancel.cancel();
    let handles = tokio::time::timeout(Duration::from_secs(3), &mut dispatch)
        .await
        .expect("dispatch remained blocked after cancellation")
        .expect("dispatch result");
    assert_eq!(handles.len(), 1);
    assert_eq!(handles[0].status, WorkerStatus::Cancelled);
    let evidence = tmp
        .path()
        .join(".swarm-worktrees")
        .join(&handles[0].worker_id);
    assert!(evidence.is_dir(), "cancelled worker evidence was removed");

    let process = format!("/proc/{descendant_pid}");
    let gone_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while Path::new(&process).exists() && tokio::time::Instant::now() < gone_deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !Path::new(&process).exists(),
        "descendant survived cancellation"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        !survived_file.exists(),
        "descendant executed after cancellation"
    );
    swarm.cleanup().await.expect("explicit cleanup");
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
    let pid_file = std::env::var("WCORE_SWARM_PID_FILE").expect("pid file");
    let survived_file = std::env::var("WCORE_SWARM_SURVIVED_FILE").expect("survived file");
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "descendant_worker_fixture",
            "--nocapture",
        ])
        .env("WCORE_SWARM_SURVIVED_FILE", survived_file)
        .spawn()
        .expect("spawn descendant");
    std::fs::write(pid_file, child.id().to_string()).expect("write descendant pid");
    std::thread::sleep(Duration::from_secs(20));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "subprocess fixture"]
fn descendant_worker_fixture() {
    let survived_file = std::env::var("WCORE_SWARM_SURVIVED_FILE").expect("survived file");
    std::thread::sleep(Duration::from_secs(1));
    std::fs::write(survived_file, "survived").expect("write survived marker");
    std::thread::sleep(Duration::from_secs(20));
}

async fn init_repo(path: &Path) {
    run_git(path, &["init", "-q", "-b", "main"]).await;
    std::fs::write(path.join("README.md"), "worker runtime fixture\n").unwrap();
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
