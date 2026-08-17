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

// `cfg(unix)` to match its only consumer below — the rest of this file's
// imports are gated the same way, and an unconditional `mod` would pull a
// module nothing references into the Windows build.
#[cfg(unix)]
mod common;

#[cfg(unix)]
#[tokio::test]
async fn worker_writes_heartbeat_during_long_running_task() {
    if common::skip_without_delegated_backend("worker_writes_heartbeat_during_long_running_task")
        .await
    {
        return;
    }
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
  # Rendezvous (wayland#935). Block until the orchestrator has actually READ
  # this heartbeat before emitting the next one. The previous version slept a
  # fixed 150ms and let the observer sample at 40ms, so under full-suite load
  # the worker could emit every heartbeat inside one poll gap and the observed
  # count came up short while the worker had behaved perfectly.
  #
  # The ack lives in the worktree (the worker's cwd) because that is the only
  # tree the delegated backend mounts into the child -- an ack in an outside
  # tempdir is invisible to a bubblewrap worker. Dot-prefixed to match the
  # existing `.swarm-status.json` convention, which is already written here.
  deadline=$(( $(date +%s) + 30 ))
  while [ ! -f ".swarm-hb-ack-$n" ]; do
    if [ "$(date +%s)" -gt "$deadline" ]; then
      echo "worker gave up waiting for ack-$n" >&2
      exit 91
    fi
    sleep 0.01
  done
done
"#;

    let brief = SwarmBrief {
        task: "heartbeat-emitter".into(),
        base_branch: "main".into(),
        worker_branch_prefix: "swarm/hb".into(),
        worker_command: vec!["bash".into(), "-c".into(), script.into()],
        // Raised from 15s: the worker now waits for the observer, so its
        // wall-clock includes the orchestrator's read latency under load.
        timeout: Duration::from_secs(60),
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
            _ = tokio::time::sleep(Duration::from_millis(10)) => {
                // We have no worker_id until dispatch returns, so locate the
                // single worker worktree and read its heartbeat through the
                // PRODUCT reader (`heartbeat::read_status`) rather than a
                // second, test-local JSON decode. A private copy of the read
                // path would keep this test green through a regression in the
                // real one.
                let Some((worktree, status)) = probe_any_worker_status(tmp.path()) else {
                    continue;
                };
                if observed.last().map(|p| p.last_alive_at) == Some(status.last_alive_at) {
                    continue;
                }
                // Acknowledge the heartbeat we just read, which is what
                // unblocks the worker to emit the next one. The worker cannot
                // run ahead of this loop, so no heartbeat can be missed and
                // the `>= 3` assertion below is deterministic rather than
                // sampled (wayland#935).
                let step = status.step.clone().unwrap_or_default();
                observed.push(status);
                if let Some(n) = step.strip_prefix("step-") {
                    // The worker runs in <transaction_root>/checkout, NOT in the
                    // transaction root itself. The root only carries the MIRROR
                    // that `dispatch::mirror_heartbeat` republishes there for the
                    // orchestrator to read, so an ack written to the root is
                    // invisible to the worker -- which is exactly what the first
                    // attempt at this rendezvous proved (worker blocked the full
                    // 30s waiting for ack-1, observed count stuck at 1).
                    let ack = worktree.join("checkout").join(format!(".swarm-hb-ack-{n}"));
                    std::fs::write(&ack, b"1")
                        .unwrap_or_else(|e| panic!("write ack {}: {e}", ack.display()));
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

/// Locate the single worker worktree and read its heartbeat.
///
/// wayland#935: this used to `std::fs::read` + `serde_json::from_slice` itself,
/// so every intermediate observation in the e2e test went through a SECOND
/// implementation of the read path and the product's own
/// [`wcore_swarm::heartbeat::read_status`] was exercised exactly once, at the
/// end. A regression in the real reader would have left this test green.
///
/// A malformed heartbeat is surfaced as `Err` by `read_status`, and a partial
/// write is not possible here (the worker renames into place), so an error is
/// treated as "nothing readable yet" and simply retried by the caller.
#[cfg(unix)]
fn probe_any_worker_status(repo_root: &Path) -> Option<(std::path::PathBuf, WorkerStatusFile)> {
    let swarm_root = repo_root.join(".swarm-worktrees");
    for ent in std::fs::read_dir(&swarm_root).ok()?.flatten() {
        let worktree = ent.path();
        if let Ok(Some(payload)) = wcore_swarm::heartbeat::read_status(&worktree) {
            return Some((worktree, payload));
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
    // Swarm::new owns this generated runtime root. Keep it out of the
    // repository's cleanliness authority, matching real callers and the
    // dispatch integration fixture.
    std::fs::write(path.join(".gitignore"), ".swarm-worktrees/\n").unwrap();
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

/// PRODUCTION DURABILITY PROOF for the heartbeat status mirror.
///
/// `dispatch::mirror_heartbeat` reads the worker's status from the checkout and
/// republishes it to the transaction root through
/// `DirectoryAuthority::atomic_write_child`, whose Windows implementation is the
/// handle-relative rename primitive. Until 20-75 that primitive had NEVER worked
/// on Windows (os error 87), so the mirror write ALWAYS failed there — a
/// production durability defect, not test debt. Nothing caught it because the
/// only status-file probe in this file (`probe_any_worker_status`) is reachable
/// solely from the `#[cfg(unix)]` end-to-end test, and `writer_then_reader_
/// roundtrip` above exercises the plain in-place write rather than the retained
/// authority publish.
///
/// This test drives the exact production shape on EVERY platform: encode a
/// status read from a checkout directory, publish it under the production
/// `STATUS_FILE` name through a retained root authority, and read it back. It is
/// deliberately NOT cfg-gated — the gap it closes is precisely that the mirror
/// had no Windows coverage.
#[test]
fn heartbeat_mirror_publishes_through_a_retained_root_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = tmp.path().join("checkout");
    let root = tmp.path().join("root");
    std::fs::create_dir(&checkout).unwrap();
    std::fs::create_dir(&root).unwrap();

    // Source side: the worker's own heartbeat, exactly as the checkout holds it.
    wcore_swarm::heartbeat::HeartbeatWriter::new(&checkout)
        .write(Some("mirrored-step"))
        .unwrap();
    let status = wcore_swarm::heartbeat::read_status(&checkout)
        .unwrap()
        .expect("heartbeat present in the checkout before mirroring");
    let encoded = serde_json::to_vec(&status).unwrap();

    // Mirror side: the retained-authority publish `mirror_heartbeat` performs.
    let root_authority = wcore_sandbox::DirectoryAuthority::open(&root).unwrap();
    root_authority
        .atomic_write_child(wcore_swarm::heartbeat::STATUS_FILE, &encoded)
        .expect("the production heartbeat mirror write must succeed on this platform");

    let mirrored = wcore_swarm::heartbeat::read_status(&root)
        .unwrap()
        .expect("mirrored heartbeat must be readable from the transaction root");
    assert_eq!(mirrored.step.as_deref(), Some("mirrored-step"));
    assert_eq!(mirrored.last_alive_at, status.last_alive_at);

    // `mirror_heartbeat` is POLLED — dispatch calls it repeatedly for the life
    // of the worker — so the publish must REPLACE an existing status file, not
    // just create the first one. A publish primitive that can only create would
    // leave every worker's mirrored status frozen at its first heartbeat.
    std::thread::sleep(Duration::from_millis(5));
    wcore_swarm::heartbeat::HeartbeatWriter::new(&checkout)
        .write(Some("second-step"))
        .unwrap();
    let next = wcore_swarm::heartbeat::read_status(&checkout)
        .unwrap()
        .expect("second heartbeat present in the checkout");
    root_authority
        .atomic_write_child(
            wcore_swarm::heartbeat::STATUS_FILE,
            &serde_json::to_vec(&next).unwrap(),
        )
        .expect("the production heartbeat mirror must REPLACE an existing status file");
    let remirrored = wcore_swarm::heartbeat::read_status(&root)
        .unwrap()
        .expect("replaced heartbeat must be readable from the transaction root");
    assert_eq!(remirrored.step.as_deref(), Some("second-step"));

    // The publish must leave no private temporary behind: `atomic_write_child`
    // renames its unguessable sibling into place, so the root holds exactly the
    // status file.
    let residue = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy() != wcore_swarm::heartbeat::STATUS_FILE)
        .count();
    assert_eq!(residue, 0, "atomic publish must leave no temporary behind");
}
