//! Smoke test: 4 noop workers in parallel.

use std::path::Path;
use std::time::Duration;

use wcore_config::shell;
use wcore_swarm::{Swarm, SwarmBrief, WorkerStatus};

#[tokio::test]
async fn dispatches_4_noop_workers_in_parallel() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;

    let swarm = Swarm::new(tmp.path()).unwrap();

    let brief = SwarmBrief {
        task: "noop".into(),
        base_branch: "main".into(),
        worker_branch_prefix: "swarm/noop".into(),
        worker_command: noop_argv(),
        timeout: Duration::from_secs(30),
        env: vec![],
    };

    let handles = swarm.dispatch(brief, 4).await.unwrap();
    assert_eq!(handles.len(), 4, "expected 4 handles");

    let results = swarm.collect(handles).await.unwrap();
    assert_eq!(results.len(), 4, "expected 4 results");
    for r in &results {
        assert!(
            matches!(r.status, WorkerStatus::Succeeded),
            "worker {} failed: {:?} (stderr: {})",
            r.worker_id,
            r.status,
            r.stderr
        );
        assert!(r.branch.starts_with("swarm/noop/"));
    }

    swarm.cleanup().await.unwrap();
}

/// Cross-platform "do nothing successfully" argv. On Unix `true` exits
/// 0 with no args. On Windows we spawn `cmd /c rem` (rem is a no-op
/// builtin).
fn noop_argv() -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/c".into(), "rem".into()]
    } else {
        vec!["true".into()]
    }
}

async fn init_repo(path: &Path) {
    let cwd = path.to_path_buf();
    run_git(&cwd, &["init", "-q", "-b", "main"]).await;
    std::fs::write(path.join("README.md"), "swarm-test\n").unwrap();
    run_git(&cwd, &["add", "."]).await;
    run_git(
        &cwd,
        &[
            "-c",
            "user.email=t@e.com",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    )
    .await;
}

async fn run_git(cwd: &Path, args: &[&str]) {
    let mut cmd = shell::shell_command_argv("git", args);
    cmd.current_dir(cwd);
    let st = cmd.status().await.expect("spawn git");
    assert!(st.success(), "git {args:?} failed");
}
