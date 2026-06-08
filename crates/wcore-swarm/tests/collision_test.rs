//! Collision-detection: dirty checkout refuses dispatch.

use std::path::Path;
use std::time::Duration;

use wcore_config::shell;
use wcore_swarm::{Swarm, SwarmBrief, SwarmError};

#[tokio::test]
async fn dirty_checkout_refuses_dispatch() {
    let tmp = tempfile::tempdir().unwrap();
    init_dirty_repo(tmp.path()).await;

    let swarm = Swarm::new(tmp.path()).unwrap();
    let brief = SwarmBrief {
        task: "noop".into(),
        base_branch: "main".into(),
        worker_branch_prefix: "swarm/dirty".into(),
        worker_command: vec!["true".into()],
        timeout: Duration::from_secs(10),
        env: vec![],
    };
    let err = swarm
        .dispatch(brief, 2)
        .await
        .expect_err("expected DirtyCheckout error");
    assert!(
        matches!(err, SwarmError::DirtyCheckout(_)),
        "expected DirtyCheckout, got {err:?}"
    );
}

async fn init_dirty_repo(path: &Path) {
    let cwd = path.to_path_buf();
    run_git(&cwd, &["init", "-q", "-b", "main"]).await;
    std::fs::write(path.join("README.md"), "x\n").unwrap();
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
    // Make the checkout dirty.
    std::fs::write(path.join("README.md"), "x-dirty\n").unwrap();
}

async fn run_git(cwd: &Path, args: &[&str]) {
    let mut cmd = shell::shell_command_argv("git", args);
    cmd.current_dir(cwd);
    let st = cmd.status().await.expect("spawn git");
    assert!(st.success(), "git {args:?} failed");
}
