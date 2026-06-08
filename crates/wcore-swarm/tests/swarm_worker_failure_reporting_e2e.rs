//! Swarm worker failure reporting tests.
//!
//! Creates two independent swarms (one with 2 succeeding workers, one with
//! 1 failing worker) and asserts that each swarm correctly reports final
//! worker status: `WorkerStatus::Succeeded` and `WorkerStatus::Failed`.
//!
//! This is NOT a test of intra-swarm failover (rerouting live work from a
//! failed worker to a healthy peer). Real failover testing is a follow-up.

use std::path::Path;
use std::time::Duration;

use wcore_config::shell;
use wcore_swarm::{Swarm, SwarmBrief, WorkerStatus};

#[tokio::test]
async fn swarm_reports_failed_worker_status_and_succeeding_workers_complete() {
    // Swarm refuses dispatch on a dirty checkout. Use two separate repos:
    // one for the 2 succeeding workers, one for the 1 failing worker.
    // This matches the dispatch_smoke pattern of one brief per repo.

    // --- 2 workers that succeed ---
    let tmp_ok = tempfile::tempdir().unwrap();
    init_repo(tmp_ok.path()).await;
    let swarm_ok = Swarm::new(tmp_ok.path()).unwrap();
    let ok_brief = SwarmBrief {
        task: "failover-ok".into(),
        base_branch: "main".into(),
        worker_branch_prefix: "swarm/failover/ok".into(),
        worker_command: noop_argv(),
        timeout: Duration::from_secs(30),
        env: vec![],
    };
    let ok_handles = swarm_ok.dispatch(ok_brief, 2).await.unwrap();
    let ok_results = swarm_ok.collect(ok_handles).await.unwrap();
    swarm_ok.cleanup().await.unwrap();

    // --- 1 worker that fails ---
    let tmp_fail = tempfile::tempdir().unwrap();
    init_repo(tmp_fail.path()).await;
    let swarm_fail = Swarm::new(tmp_fail.path()).unwrap();
    let fail_brief = SwarmBrief {
        task: "failover-fail".into(),
        base_branch: "main".into(),
        worker_branch_prefix: "swarm/failover/fail".into(),
        worker_command: fail_argv(),
        timeout: Duration::from_secs(30),
        env: vec![],
    };
    let fail_handles = swarm_fail.dispatch(fail_brief, 1).await.unwrap();
    let fail_results = swarm_fail.collect(fail_handles).await.unwrap();
    swarm_fail.cleanup().await.unwrap();

    // The 2 surviving workers must have succeeded.
    assert_eq!(ok_results.len(), 2, "expected 2 successful workers");
    for r in &ok_results {
        assert!(
            matches!(r.status, WorkerStatus::Succeeded),
            "expected Succeeded, got {:?} (worker: {}, stderr: {})",
            r.status,
            r.worker_id,
            r.stderr,
        );
    }

    // The failed worker must be reported as Failed, not silently dropped.
    assert_eq!(fail_results.len(), 1, "expected 1 failed worker result");
    assert!(
        matches!(fail_results[0].status, WorkerStatus::Failed(_)),
        "expected Failed, got {:?}",
        fail_results[0].status,
    );
}

fn noop_argv() -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/c".into(), "rem".into()]
    } else {
        vec!["true".into()]
    }
}

fn fail_argv() -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/c".into(), "exit 1".into()]
    } else {
        vec!["false".into()]
    }
}

async fn init_repo(path: &Path) {
    let cwd = path.to_path_buf();
    run_git(&cwd, &["init", "-q", "-b", "main"]).await;
    std::fs::write(path.join("README.md"), "failover-test\n").unwrap();
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
