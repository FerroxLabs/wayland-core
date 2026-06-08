//! Heartbeat e2e: dispatch a worker that writes 3 increasing heartbeats,
//! assert the orchestrator sees them grow through `Swarm::worker_status`.
//!
//! Driver is a small shell script that writes `.swarm-status.json` in
//! its cwd (which the swarm sets to the worker worktree). Unix-only —
//! the heartbeat mechanism itself is platform-agnostic (see
//! `crates/wcore-swarm/src/heartbeat.rs`), but driving a subprocess to
//! emit JSON files requires a shell. Windows is covered by the unit-
//! test below (`writer_then_reader_roundtrip`).

use std::time::Duration;

#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use wcore_config::shell;
#[cfg(unix)]
use wcore_swarm::heartbeat::WorkerStatusFile;
#[cfg(unix)]
use wcore_swarm::{Swarm, SwarmBrief};

#[cfg(unix)]
#[tokio::test]
async fn worker_writes_heartbeat_during_long_running_task() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path()).await;

    let swarm = Swarm::new(tmp.path()).unwrap();

    // The worker writes 3 heartbeats spaced ~150ms apart, then exits 0.
    // The orchestrator polls between writes via worker_status.
    //
    // Each heartbeat is a syntactically-valid JSON file with a
    // monotonically increasing last_alive_at field. We mark the file
    // with a sentinel filename (`.swarm-status.json.<n>`) before the
    // final rename to `.swarm-status.json` to avoid the test ever
    // reading a partial JSON write (the heartbeat itself does an
    // in-place write, which is fine — partial reads surface as serde
    // errors, but for the test we want determinism).
    let script = r#"
set -eu
for n in 1 2 3; do
  ts=$(($(date +%s%N) / 1000000))
  printf '{"last_alive_at":%d,"step":"step-%d"}' "$ts" "$n" > .swarm-status.tmp
  mv .swarm-status.tmp .swarm-status.json
  sleep 0.15
done
"#;

    let brief = SwarmBrief {
        task: "heartbeat-emitter".into(),
        base_branch: "main".into(),
        worker_branch_prefix: "swarm/hb".into(),
        worker_command: vec!["bash".into(), "-c".into(), script.into()],
        timeout: Duration::from_secs(15),
        env: vec![],
    };

    // Dispatch ONE worker in this test (count=1) so we have a single
    // handle to poll. Run dispatch concurrently with a polling loop so
    // we can observe the heartbeats grow before the worker exits.
    let dispatch_fut = swarm.dispatch(brief, 1);
    tokio::pin!(dispatch_fut);

    // Drive both the dispatch future and our poll loop on the runtime.
    let mut observed: Vec<WorkerStatusFile> = Vec::new();
    let handles = loop {
        tokio::select! {
            res = &mut dispatch_fut => break res.unwrap(),
            _ = tokio::time::sleep(Duration::from_millis(40)) => {
                // We don't have a worker_id until dispatch returns, so
                // probe the swarm root directly for any worktree-with-
                // status-file. This mirrors what worker_status does
                // under the hood, just without the handle.
                if let Some(status) = probe_any_worker_status(tmp.path())
                    && observed.last().map(|p| p.last_alive_at) != Some(status.last_alive_at)
                {
                    observed.push(status);
                }
            }
        }
    };
    assert_eq!(handles.len(), 1);

    // After dispatch returns, the worker has exited. The final
    // heartbeat is still on disk; read it via the public API to
    // confirm the handle-based accessor works.
    let final_status = swarm
        .worker_status(&handles[0])
        .expect("worker_status read")
        .expect("worker wrote a heartbeat");
    if observed.last().map(|p| p.last_alive_at) != Some(final_status.last_alive_at) {
        observed.push(final_status);
    }

    assert!(
        observed.len() >= 3,
        "expected to observe >=3 distinct heartbeats, got {} ({:?})",
        observed.len(),
        observed
    );
    for win in observed.windows(2) {
        assert!(
            win[1].last_alive_at >= win[0].last_alive_at,
            "heartbeats should be monotonically increasing: {win:?}"
        );
    }
    // Strictly-increasing check on the first 3 we observed.
    assert!(
        observed[2].last_alive_at > observed[0].last_alive_at,
        "expected strict growth over the run, got {observed:?}"
    );

    swarm.cleanup().await.unwrap();
}

#[cfg(unix)]
fn probe_any_worker_status(repo_root: &Path) -> Option<WorkerStatusFile> {
    let swarm_root = repo_root.join(".swarm-worktrees");
    let entries = std::fs::read_dir(&swarm_root).ok()?;
    for ent in entries.flatten() {
        let p = ent.path().join(wcore_swarm::heartbeat::STATUS_FILE);
        if let Ok(bytes) = std::fs::read(&p)
            && let Ok(payload) = serde_json::from_slice::<WorkerStatusFile>(&bytes)
        {
            return Some(payload);
        }
    }
    None
}

// ----- shared helpers (unix-only — only the e2e test uses git) ------------

#[cfg(unix)]
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

#[cfg(unix)]
async fn run_git(cwd: &Path, args: &[&str]) {
    let mut cmd = shell::shell_command_argv("git", args);
    cmd.current_dir(cwd);
    let st = cmd.status().await.expect("spawn git");
    assert!(st.success(), "git {args:?} failed");
}

// ----- unit-style heartbeat roundtrip (runs everywhere) -------------------

#[test]
fn writer_then_reader_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = wcore_swarm::heartbeat::HeartbeatWriter::new(tmp.path());

    // No file yet — read_status returns Ok(None).
    let none = wcore_swarm::heartbeat::read_status(tmp.path()).unwrap();
    assert!(none.is_none(), "expected no heartbeat before write");

    writer.write(Some("first")).unwrap();
    let s1 = wcore_swarm::heartbeat::read_status(tmp.path())
        .unwrap()
        .expect("heartbeat present after write");
    assert_eq!(s1.step.as_deref(), Some("first"));

    // Force a clock advance by sleeping at least 2ms (most platforms
    // resolve SystemTime at ms granularity or finer).
    std::thread::sleep(Duration::from_millis(5));
    writer.write(Some("second")).unwrap();
    let s2 = wcore_swarm::heartbeat::read_status(tmp.path())
        .unwrap()
        .unwrap();
    assert!(
        s2.last_alive_at >= s1.last_alive_at,
        "second heartbeat must be >= first ({} < {})",
        s2.last_alive_at,
        s1.last_alive_at
    );
    assert_eq!(s2.step.as_deref(), Some("second"));
}
