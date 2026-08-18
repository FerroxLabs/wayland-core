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
//! Linux and **macOS**. The four instrumentation primitives (`alive`, `ppid_of`,
//! `all_pids`, `descendant_names`) have two implementations — `/proc` on Linux, `ps`
//! on macOS — and every measurement above them is shared, so the two platforms run
//! the *same* baseline rather than two different ones.
//!
//! **Windows is NOT MEASURED by this file; that is a gap, not a pass.** Note what a
//! bare `cfg` exclusion looks like from the outside: before macOS was added, running
//! this binary on a Mac printed `test result: ok. 0 passed; 0 failed` and exited 0 —
//! a green line for a baseline that did not exist. Any consumer of this file must
//! read the harness's own `--list` count, not its exit status.

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use wcore_browser::supervisor::{BackendHandle, BrowserSupervisor, SupervisorConfig};

// ── process-table instrumentation ────────────────────────────────────────
//
// Linux reads `/proc`; macOS has no `/proc`, so it shells out to `ps`. Both
// implementations answer the same four questions and reject zombies identically —
// a terminated-but-unreaped process must count as cleaned up on either platform.

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
/// Parent PID of `pid`, from `/proc/<pid>/stat` field 4.
fn ppid_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "linux")]
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

// ── macOS: the same four answers, read from `ps` ─────────────────────────

/// One row of the macOS process table.
#[cfg(target_os = "macos")]
struct PsRow {
    pid: u32,
    ppid: u32,
    /// First char of `ps -o state=`; `Z` is a zombie.
    state: char,
    /// Basename of `ps -o comm=`, so it matches Linux's `/proc/<pid>/comm`.
    comm: String,
}

/// The whole process table in one `ps` call. `ps -axo` is the only supported way
/// to read ppid + state on macOS — there is no `/proc`.
///
/// Returns an EMPTY vec only when `ps` itself fails, which every caller must treat
/// as "could not look", never as "nothing is running". The instrument self-test in
/// `ps_instrument_is_live` is what makes that distinction observable: if this ever
/// returns an empty table, that test fails rather than every other test passing
/// vacuously.
#[cfg(target_os = "macos")]
fn ps_snapshot() -> Vec<PsRow> {
    let out = match std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid=,state=,comm="])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&out)
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let state = it.next()?.chars().next()?;
            // `comm` is the remainder — it can contain spaces, and on macOS it is a
            // full path, so take everything left and reduce it to a basename.
            let rest: Vec<&str> = it.collect();
            let comm = rest
                .join(" ")
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            Some(PsRow {
                pid,
                ppid,
                state,
                comm,
            })
        })
        .collect()
}

#[cfg(target_os = "macos")]
/// Live in the process table and not a zombie — the same predicate the Linux arm
/// applies to `/proc/<pid>/stat` field 3.
fn alive(pid: u32) -> bool {
    ps_snapshot().iter().any(|r| r.pid == pid && r.state != 'Z')
}

// `ppid_of` / `all_pids` have no macOS twin on purpose: the two callers that need
// ancestry (`process_tree_size`, `descendant_names`) each take ONE `ps` snapshot and
// walk it locally, so a per-PID accessor would only add process spawns and let the
// table shift mid-walk.

/// Instrument self-test — the `ps` reader must be able to find THIS process, with
/// its own real ppid and a non-empty `comm`. Without it an empty or malformed `ps`
/// table would silently make every "the process is gone" assertion in this file
/// free (LANE-BRIEF §3b-i; and the Windows `tasklist` measured-zero defect).
#[cfg(target_os = "macos")]
#[test]
fn ps_instrument_is_live() {
    let me = std::process::id();
    let snap = ps_snapshot();
    assert!(
        !snap.is_empty(),
        "ps returned an EMPTY process table — the instrument is dead, and every \
         absence measured by this file would be free"
    );
    let row = snap
        .iter()
        .find(|r| r.pid == me)
        .unwrap_or_else(|| panic!("ps cannot see this test process (pid {me}) — dead instrument"));
    assert!(
        !row.comm.is_empty(),
        "ps returned an empty comm for our own pid {me}"
    );
    assert!(row.ppid > 0, "ps returned ppid 0 for our own pid {me}");
    assert!(alive(me), "alive() is false for our own live pid {me}");
    // Both directions: a PID that cannot exist must read as not-alive.
    assert!(
        !alive(0x7fff_fffe),
        "alive() is true for an impossible pid — the predicate does not discriminate"
    );
    println!(
        "EV3-INSTRUMENT: ps_rows={} self_pid={} self_ppid={} self_comm={} \
         alive_self=true alive_impossible=false discrimination=PASS",
        snap.len(),
        me,
        row.ppid,
        row.comm
    );
}

#[cfg(target_os = "linux")]
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
            if let Some(pp) = ppid_of(p)
                && tree.contains(&pp)
                && alive(p)
            {
                tree.push(p);
            }
        }
        if tree.len() == before {
            break;
        }
    }
    tree.len()
}

#[cfg(target_os = "linux")]
/// `comm` names of `root`'s live descendants (excluding `root` itself). Used so
/// the evidence names the browser process rather than asserting on a bare count.
fn descendant_names(root: u32) -> Vec<String> {
    let mut out = Vec::new();
    for p in all_pids() {
        if p == root {
            continue;
        }
        let mut cur = p;
        for _ in 0..8 {
            match ppid_of(cur) {
                Some(pp) if pp == root => {
                    if let Ok(c) = std::fs::read_to_string(format!("/proc/{p}/comm")) {
                        out.push(c.trim().to_string());
                    }
                    break;
                }
                Some(pp) if pp > 1 => cur = pp,
                _ => break,
            }
        }
    }
    out.sort();
    out
}

#[cfg(target_os = "macos")]
/// Same as the Linux arm, but over ONE `ps` snapshot — walking the table with a
/// separate `ps` per ancestry step would be hundreds of process spawns and would
/// also let the table shift underneath the walk.
fn process_tree_size(root: u32) -> usize {
    let snap = ps_snapshot();
    if !snap.iter().any(|r| r.pid == root && r.state != 'Z') {
        return 0;
    }
    let mut tree = vec![root];
    for _ in 0..8 {
        let before = tree.len();
        for r in &snap {
            if r.state == 'Z' || tree.contains(&r.pid) {
                continue;
            }
            if tree.contains(&r.ppid) {
                tree.push(r.pid);
            }
        }
        if tree.len() == before {
            break;
        }
    }
    tree.len()
}

#[cfg(target_os = "macos")]
fn descendant_names(root: u32) -> Vec<String> {
    let snap = ps_snapshot();
    let mut out = Vec::new();
    for r in &snap {
        if r.pid == root {
            continue;
        }
        let mut cur = r.ppid;
        for _ in 0..8 {
            if cur == root {
                out.push(r.comm.clone());
                break;
            }
            match snap.iter().find(|x| x.pid == cur) {
                Some(parent) if parent.ppid > 1 => cur = parent.ppid,
                _ => break,
            }
        }
    }
    out.sort();
    out
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
        ..SupervisorConfig::default()
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
    assert_eq!(
        sessions_before, 0,
        "BEFORE: supervisor must track no sessions"
    );
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
        sup.healthcheck(Duration::from_secs(2))
            .await
            .unwrap_or(false),
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
        ..SupervisorConfig::default()
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
    assert_eq!(
        before_tree, 1,
        "ARM 1: child must be live before the reaper runs"
    );

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
        ..SupervisorConfig::default()
    }));
    let (mut keep_child, keep_pid) = spawn_real_child();
    assert!(alive(keep_pid), "ARM 2: the child never started");
    let live_parent = std::process::id();
    assert!(
        alive(live_parent),
        "ARM 2 precondition: our own PID is alive"
    );
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

    sup.ensure_ready()
        .await
        .expect("real camoufox sidecar must become healthy");
    let live = sup.live_sessions();
    assert_eq!(live.len(), 1, "DURING: one tracked session");
    let pid = live[0].pid;
    assert!(alive(pid), "DURING: real sidecar PID {pid} must be alive");

    // Wait for the sidecar's BROWSER children to actually exist before measuring
    // cleanup. Observed steady state on `hetzner-dsm` is a tree of 3: the node
    // sidecar plus `Xvfb` plus `camoufox-bin`.
    //
    // This wait is load-bearing. The first run of this test tore down 1.12s after
    // `ensure_ready` and recorded `during_tree=2` — i.e. it measured the cleanup of
    // a sidecar whose browser had not spawned yet, and "after_tree=0" would have
    // said nothing about whether a real browser process gets cleaned up. The leak
    // an operator cares about is a leaked BROWSER, not a leaked node process.
    let mut tree_during = process_tree_size(pid);
    for _ in 0..600 {
        tree_during = process_tree_size(pid);
        if tree_during >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        tree_during >= 3,
        "DURING: the real sidecar must have spawned BOTH its Xvfb and its browser \
         process (expected tree >= 3, got {tree_during}). Measuring teardown before \
         the browser exists would not measure browser cleanup at all."
    );
    // Name the descendants in the evidence, so a reader can see a real browser was
    // present rather than taking the count on trust.
    let names = descendant_names(pid);
    assert!(
        names
            .iter()
            .any(|n| n.contains("camoufox") || n.contains("firefox")),
        "DURING: no browser process among the sidecar's descendants: {names:?}"
    );
    println!(
        "EV3C: phase=during tracked_sessions=1 sidecar_pid={pid} tree_size={tree_during} \
         descendants={names:?} health=2xx"
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
    println!("EV3C: phase=after tracked_sessions=0 tree_size=0 returned_to_baseline=true");
    println!(
        "EV3C-SUMMARY: backend=real-camoufox before_tree=0 during_tree={tree_during} \
         after_tree=0 leaked_processes=0"
    );
}
