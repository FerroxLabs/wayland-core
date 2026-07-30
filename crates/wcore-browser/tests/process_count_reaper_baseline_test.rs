//! 27-C2(c) BASELINE 3 — process count before / during / after a session, plus one
//! reaper interval. Measured in BOTH directions against REAL OS processes.
//!
//! The criterion clause (`ROADMAP.md:152`, ledger row `27-C2`) asks that browser surfaces
//! "preserve ... cleanup policy". The ledger records the process-count baseline as
//! **absent entirely**.
//!
//! ## What is measured, and why these three metrics
//!
//!   * **A — supervisor-tracked sessions.** `BrowserSupervisor::live_sessions().len()`.
//!     Cheap, but on its own it is bookkeeping: a supervisor that forgot a session would
//!     report 0 while the process ran on. So it is never asserted alone.
//!   * **B — OS-level liveness of the spawned PID**, read from `/proc/<pid>`. This is the
//!     one that catches a leak: the supervisor can say whatever it likes, the kernel
//!     cannot.
//!   * **C — descendant count** of the spawned process, from a `/proc` walk. The real
//!     Camoufox sidecar spawns its own `Xvfb` and `camoufox-bin` children (observed on
//!     `hetzner-dsm`: two children under the node process), so counting only the direct
//!     PID would miss exactly the leak an operator cares about.
//!
//! ## Both directions
//!
//! Every arm is paired. A reaper that killed everything unconditionally, and a reaper that
//! killed nothing, must both be distinguishable from a correct one:
//!
//!   * orphan (parent dead) ⇒ child **is** terminated within one reaper interval;
//!   * **control**: live parent ⇒ after the *same* wait, the child is **still alive**.
//!
//! Without the control arm, "the process is gone" proves nothing — the child could have
//! exited on its own, been killed by the harness, or never have started (LANE-BRIEF
//! §6a-i: an actor that never launched is a dead instrument, so every arm below asserts
//! the child reached a live state BEFORE the behaviour under test is exercised).
//!
//! ## Platform scope, stated
//!
//! `#[cfg(target_os = "linux")]` — metrics B and C read `/proc`. macOS and Windows are
//! **NOT MEASURED** by this file; that is a gap, not a pass.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use wcore_browser::supervisor::{BackendHandle, BrowserSupervisor, SupervisorConfig};

// ── /proc instrumentation ────────────────────────────────────────────────

/// Is this PID present in the kernel's process table? `/proc/<pid>` exists for
/// live processes AND for un-reaped zombies, so `alive()` additionally rejects
/// state `Z` — a zombie is a terminated process and must count as cleaned up.
fn alive(pid: u32) -> bool {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // Field 3 is the state char, after the parenthesised comm which may itself
    // contain spaces — so split on the LAST ')'.
    match stat.rsplit_once(')') {
        Some((_, rest)) => !rest.trim_start().starts_with('Z'),
        None => false,
    }
}

/// Parent PID of `pid`, from `/proc/<pid>/stat` field 4.
fn ppid_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Every live PID currently in the process table.
fn all_pids() -> Vec<u32> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            if let Some(p) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) {
                v.push(p);
            }
        }
    }
    v
}

/// `root` plus every live descendant of it. This is metric C.
fn process_tree_size(root: u32) -> usize {
    if !alive(root) {
        return 0;
    }
    let pids = all_pids();
    let mut tree = vec![root];
    // Bounded fixpoint — process trees here are shallow (sidecar → Xvfb/browser).
    for _ in 0..8 {
        let before = tree.len();
        for &p in &pids {
            if tree.contains(&p) {
                continue;
            }
            if let Some(pp) = ppid_of(p) {
                if tree.contains(&pp) && alive(p) {
                    tree.push(p);
                }
            }
        }
        if tree.len() == before {
            break;
        }
    }
    tree.len()
}

// ── stand-in sidecar ─────────────────────────────────────────────────────

/// Write an executable, argument-free HTTP health server. `launch_camoufox_program`
/// passes NO args, so the program must be self-contained. Serves 200 on every path
/// (the supervisor only needs `/health` to be 2xx) and stays alive until signalled.
fn write_stub_sidecar(dir: &std::path::Path, port: u16) -> PathBuf {
    let path = dir.join("stub-sidecar");
    let script = format!(
        r#"#!/usr/bin/env python3
import http.server, socketserver
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.send_header("Content-Length","2"); self.end_headers()
        self.wfile.write(b"ok")
    def log_message(self, *a): pass
socketserver.TCPServer.allow_reuse_address = True
socketserver.TCPServer(("127.0.0.1", {port}), H).serve_forever()
"#
    );
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Pick a free localhost port by binding and immediately dropping.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn cfg_for(program: &std::path::Path, port: u16, pid_dir: PathBuf) -> SupervisorConfig {
    SupervisorConfig {
        pid_dir,
        reaper_interval: Duration::from_millis(200),
        healthcheck_interval: Duration::from_secs(30),
        healthcheck_url: format!("http://127.0.0.1:{port}/health"),
        sidecar_program: Some(program.to_string_lossy().into_owned()),
        startup_timeout: Duration::from_secs(20),
    }
}

// ── BASELINE 3a — before / during / after a session ──────────────────────

/// The lifecycle measurement. Reports the three counts the criterion names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn baseline_process_count_before_during_after() {
    let tmp = tempfile::tempdir().unwrap();
    let port = free_port();
    let program = write_stub_sidecar(tmp.path(), port);
    let sup = Arc::new(BrowserSupervisor::with_config(cfg_for(
        &program,
        port,
        tmp.path().join("pids"),
    )));

    // ── BEFORE ──
    let sessions_before = sup.live_sessions().len();
    assert_eq!(sessions_before, 0, "BEFORE: supervisor must track no sessions");
    println!("EV3A: phase=before tracked_sessions=0 sidecar_pid=none tree_size=0");

    // ── DURING ──
    sup.ensure_ready()
        .await
        .expect("sidecar must become healthy — this is the participant-started check");
    let live = sup.live_sessions();
    assert_eq!(
        live.len(),
        1,
        "DURING: exactly one session must be tracked, got {live:?}"
    );
    let pid = live[0].pid;
    // §6a-i — assert the participant actually STARTED before measuring anything
    // about its termination. A process that never launched would otherwise make
    // every "it is gone" assertion below free.
    assert!(
        alive(pid),
        "DURING: the spawned sidecar PID {pid} is not alive in /proc — the actor \
         never started, so nothing below would be a measurement"
    );
    let tree_during = process_tree_size(pid);
    assert!(
        tree_during >= 1,
        "DURING: process tree must contain at least the sidecar itself"
    );
    // The health gate is the product's own readiness signal; assert it, so the
    // "during" state is a genuinely running service and not just a live PID.
    assert!(
        sup.healthcheck(Duration::from_secs(2)).await.unwrap_or(false),
        "DURING: sidecar /health must be 2xx"
    );
    println!(
        "EV3A: phase=during tracked_sessions=1 sidecar_pid={pid} pid_alive=true \
         tree_size={tree_during} health=2xx"
    );

    // ── AFTER ──
    sup.on_session_end(&live[0].session_id);
    // Give the signal time to land; bounded, and the assertion is on the kernel.
    let mut settled = false;
    for _ in 0..100 {
        if !alive(pid) {
            settled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let sessions_after = sup.live_sessions().len();
    let tree_after = process_tree_size(pid);
    assert!(
        settled,
        "AFTER: sidecar PID {pid} is STILL ALIVE after on_session_end — process leak"
    );
    assert_eq!(
        sessions_after, 0,
        "AFTER: supervisor must track no sessions, got {sessions_after}"
    );
    assert_eq!(
        tree_after, 0,
        "AFTER: the whole process tree must be gone, {tree_after} still live"
    );
    println!(
        "EV3A: phase=after tracked_sessions=0 pid_alive=false tree_size=0 \
         returned_to_baseline=true"
    );
    println!(
        "EV3A-SUMMARY: before_tracked=0 before_tree=0 during_tracked=1 \
         during_tree={tree_during} after_tracked=0 after_tree=0 leaked_processes=0"
    );
}

// ── BASELINE 3b — one reaper interval, both directions ───────────────────

/// Spawn a real, long-lived child NOT owned by the supervisor's stash, so the
/// reaper's raw-PID SIGTERM path is exactly what terminates it. Returns the PID.
///
/// Deliberately `/bin/sleep` directly and NOT `sh -c "sleep 300"`: the shell form
/// forks, giving a two-process tree (measured: `tree_size` came back 2), and the
/// reaper's documented behaviour is to SIGTERM the ONE registered PID. Testing it
/// against a tree it never claimed to own would be measuring the wrong contract.
/// Whole-tree cleanup IS measured — against the real sidecar, in 3c, where the
/// supervisor owns the tree via `ProcessTreeGuard`.
fn spawn_real_child() -> (std::process::Child, u32) {
    let child = std::process::Command::new("/bin/sleep")
        .arg("300")
        .spawn()
        .expect("spawn /bin/sleep");
    let pid = child.id();
    (child, pid)
}

/// The reaper measurement. ORPHAN arm and LIVE-PARENT control arm, identical in
/// every respect except whether the registered `parent_pid` is dead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn baseline_reaper_one_interval_both_directions() {
    let interval = Duration::from_millis(200);

    // ── ARM 1: ORPHAN (registered parent is dead) ⇒ must be reaped ───────
    let tmp1 = tempfile::tempdir().unwrap();
    let sup1 = Arc::new(BrowserSupervisor::with_config(SupervisorConfig {
        pid_dir: tmp1.path().to_path_buf(),
        reaper_interval: interval,
        healthcheck_interval: Duration::from_secs(300),
        healthcheck_url: "http://127.0.0.1:1/health".into(),
        sidecar_program: None,
        startup_timeout: Duration::from_secs(1),
    }));
    let (mut orphan_child, orphan_pid) = spawn_real_child();
    // Participant-started check (§6a-i).
    assert!(
        alive(orphan_pid),
        "ARM 1: the child never started; nothing below would be a measurement"
    );
    // A PID that is certainly not a live process stands in for the dead host.
    let dead_parent = 0x7fff_fffe;
    assert!(
        !alive(dead_parent),
        "ARM 1 precondition: the stand-in parent PID must really be dead"
    );
    sup1.register(BackendHandle {
        session_id: "orphan-real".into(),
        pid: orphan_pid,
        parent_pid: dead_parent,
    });
    assert_eq!(sup1.live_sessions().len(), 1);
    let before_tree = process_tree_size(orphan_pid);
    assert_eq!(before_tree, 1, "ARM 1: child must be live before the reaper runs");

    let cancel1 = sup1.start_reaper();
    // ONE reaper interval, plus a bounded settle margin.
    let mut reaped = false;
    let deadline = std::time::Instant::now() + interval + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if !alive(orphan_pid) {
            reaped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cancel1.cancel();
    let _ = orphan_child.wait();
    assert!(
        reaped,
        "ARM 1: orphan PID {orphan_pid} survived the reaper — cleanup policy NOT preserved"
    );
    assert!(
        sup1.live_sessions().is_empty(),
        "ARM 1: the reaped session must be dropped from the registry"
    );
    println!(
        "EV3B: arm=orphan parent=dead reaper_interval_ms=200 child_alive_before=true \
         child_alive_after=false tracked_sessions_after=0 PASS"
    );

    // ── ARM 2: CONTROL — live parent ⇒ must NOT be reaped ────────────────
    // Same reaper, same interval, same kind of child. Only the parent differs.
    // Without this arm, ARM 1 is satisfied by a reaper that kills indiscriminately
    // (or by a child that simply exited on its own).
    let tmp2 = tempfile::tempdir().unwrap();
    let sup2 = Arc::new(BrowserSupervisor::with_config(SupervisorConfig {
        pid_dir: tmp2.path().to_path_buf(),
        reaper_interval: interval,
        healthcheck_interval: Duration::from_secs(300),
        healthcheck_url: "http://127.0.0.1:1/health".into(),
        sidecar_program: None,
        startup_timeout: Duration::from_secs(1),
    }));
    let (mut keep_child, keep_pid) = spawn_real_child();
    assert!(alive(keep_pid), "ARM 2: the child never started");
    let live_parent = std::process::id();
    assert!(alive(live_parent), "ARM 2 precondition: our own PID is alive");
    sup2.register(BackendHandle {
        session_id: "live-parent-real".into(),
        pid: keep_pid,
        parent_pid: live_parent,
    });
    let cancel2 = sup2.start_reaper();
    // Wait strictly LONGER than ARM 1 took to reap, so "still alive" cannot be
    // explained by not having waited long enough.
    tokio::time::sleep(interval * 10).await;
    cancel2.cancel();
    let still_alive = alive(keep_pid);
    let tracked = sup2.live_sessions().len();
    let _ = keep_child.kill();
    let _ = keep_child.wait();
    assert!(
        still_alive,
        "ARM 2 (control): the reaper killed a child whose parent is ALIVE — it is not \
         discriminating, so ARM 1 proved nothing"
    );
    assert_eq!(
        tracked, 1,
        "ARM 2 (control): the session must still be tracked"
    );
    println!(
        "EV3B: arm=live-parent-control parent=alive waited_ms=2000 child_alive_after=true \
         tracked_sessions_after=1 PASS"
    );
    println!(
        "EV3B-SUMMARY: arms=2 orphan_reaped_within_one_interval=true \
         live_parent_child_survived=true discrimination=PASS"
    );
}

// ── BASELINE 3c — the SAME lifecycle against the REAL Camoufox sidecar ────

/// `#[ignore]` by default because it needs `@askjo/camofox-browser` installed and
/// `WAYLAND_CAMOUFOX_BIN` pointing at it — not available in CI. It is **run
/// explicitly** with `-- --ignored` and its executed count reported, so it never
/// renders as a pass it did not earn. An `ignored` line is visibly not a pass;
/// a silent skip would not be (LANE-BRIEF §3.2).
///
/// When invoked, a missing binary is a **failure**, not a skip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs a real camofox-browser at $WAYLAND_CAMOUFOX_BIN; run with -- --ignored"]
async fn baseline_process_count_against_real_camoufox_sidecar() {
    let bin = std::env::var("WAYLAND_CAMOUFOX_BIN").expect(
        "WAYLAND_CAMOUFOX_BIN must be set for this test — it is run explicitly, \
         so an unset variable is a failure and not a skip",
    );
    assert!(
        std::path::Path::new(&bin).exists(),
        "WAYLAND_CAMOUFOX_BIN={bin} does not exist"
    );

    let tmp = tempfile::tempdir().unwrap();
    // Exactly the production constructor and the production port, so the path
    // under test is the shipped one. `launch_camoufox_program` passes NO args,
    // so the sidecar binds its own default (9377) — which is what
    // `local_camoufox`'s default healthcheck URL points at.
    let mut cfg = SupervisorConfig::local_camoufox("http://127.0.0.1:9377");
    cfg.pid_dir = tmp.path().join("pids");
    cfg.reaper_interval = Duration::from_millis(200);
    cfg.startup_timeout = Duration::from_secs(120);
    assert_eq!(
        cfg.sidecar_program.as_deref(),
        Some(bin.as_str()),
        "local_camoufox must have picked up WAYLAND_CAMOUFOX_BIN"
    );

    let sup = Arc::new(BrowserSupervisor::with_config(cfg));

    // PRECONDITION (§6a-i, participant-started). `ensure_ready` REUSES an
    // externally-managed sidecar if one is already healthy — in which case it
    // spawns nothing and the "during" numbers below would describe someone
    // else's process. Assert the field is clear so this run really is the
    // participant.
    assert!(
        !sup.healthcheck(Duration::from_millis(500))
            .await
            .unwrap_or(false),
        "PRECONDITION: a sidecar is ALREADY healthy on 9377. This test must spawn its \
         own, else the process counts describe a process it did not start. Stop the \
         running sidecar first."
    );
    let before = sup.live_sessions().len();
    assert_eq!(before, 0);
    println!("EV3C: phase=before tracked_sessions=0 tree_size=0 preexisting_sidecar=none");

    sup.ensure_ready().await.expect("real camoufox sidecar must become healthy");
    let live = sup.live_sessions();
    assert_eq!(live.len(), 1, "DURING: one tracked session");
    let pid = live[0].pid;
    assert!(alive(pid), "DURING: real sidecar PID {pid} must be alive");
    let tree_during = process_tree_size(pid);
    // The real sidecar spawns Xvfb + camoufox-bin, so the tree is genuinely > 1.
    // Asserting only >= 1 would let a browser-less sidecar pass silently.
    assert!(
        tree_during >= 2,
        "DURING: the real sidecar must have spawned its browser children; tree={tree_during}"
    );
    println!(
        "EV3C: phase=during tracked_sessions=1 sidecar_pid={pid} tree_size={tree_during} \
         health=2xx"
    );

    sup.on_session_end(&live[0].session_id);
    let mut settled = false;
    for _ in 0..300 {
        if process_tree_size(pid) == 0 {
            settled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let tree_after = process_tree_size(pid);
    assert!(
        settled,
        "AFTER: the real sidecar tree did not return to baseline — {tree_after} \
         process(es) leaked from PID {pid}"
    );
    assert_eq!(sup.live_sessions().len(), 0);
    println!(
        "EV3C: phase=after tracked_sessions=0 tree_size=0 returned_to_baseline=true"
    );
    println!(
        "EV3C-SUMMARY: backend=real-camoufox before_tree=0 during_tree={tree_during} \
         after_tree=0 leaked_processes=0"
    );
}
