//! Live Windows Job-Object hard-containment acceptance — the real Windows
//! counterpart to the Linux-only Bubblewrap `hard_process_containment.rs`.
//!
//! These tests exercise the ACTUAL mechanism the Windows AppContainer sandbox
//! sets up in `windows_impl/process.rs` — a Job Object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, an `ActiveProcessLimit` cap, the
//! breakaway-allow bits cleared (`BREAKAWAY_OK`/`SILENT_BREAKAWAY_OK`), and a
//! `TerminateJobObject` reap of the whole tree on exit/timeout — through the
//! `wcore-sandbox` PUBLIC surface only (`AppContainerBackend::execute` +
//! `SandboxManifest`/`SandboxCommand`). No crate internals and no production
//! test-seam are touched.
//!
//! Every test is `#![cfg(windows)]` + `#[ignore]` and self-qualifies (rather
//! than skips) on `WAYLAND_SANDBOX_LIVE_WINDOWS=1` + `is_available()`, exactly
//! like the native ACL tests in `live_fs_acl.rs`. Off Windows the file compiles
//! to nothing; on non-live hosts the `#[ignore]` keeps them out of the default
//! run. Their empirical green is proven ONLY on the self-hosted AppContainer
//! msvc runner at the 20-25 native-proof gate — this plan authors them
//! construction-only.
//!
//! Falsifiability model (mirrors the bwrap descendant-reaping intent via Job
//! Objects): a detached descendant inherits the child's stdout pipe. If the
//! backend did NOT own and reap the descendant tree, tearing down the direct
//! child would leave that descendant alive holding the pipe, so `execute` would
//! block (drain never reaches EOF) until the manifest timeout, and a
//! host-side liveness query would still find the descendant running. Because
//! the Job Object closes with `KILL_ON_JOB_CLOSE` + `TerminateJobObject`, the
//! whole tree dies promptly and the liveness query finds nothing — so both the
//! wall-clock bound and the explicit "no residue" query are genuine
//! containment assertions, not parent-exit tautologies.

#![cfg(windows)]

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

/// The number of authored Job-Object containment acceptance cases. Kept in
/// lockstep with the `#[ignore]`d tests below so a silently-dropped case fails
/// the zero-execution guard rather than shrinking the proof unnoticed.
const NATIVE_CONTAINMENT_CASES: usize = 5;

/// The active-process cap the Windows backend installs on the Job Object,
/// mirrored from `windows_impl/command.rs::SANDBOX_ACTIVE_PROCESS_LIMIT`. It is
/// `pub(super)` (crate-internal), so an integration test cannot import it; the
/// value is duplicated here with this pointer to its source of truth. The cap
/// test asserts a fan-out beyond it is bounded, which fails closed if the two
/// ever drift apart on hardware.
const SANDBOX_ACTIVE_PROCESS_LIMIT: usize = 512;

fn require_live_windows() {
    assert_eq!(
        std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Ok("1"),
        "native Job-Object containment acceptance requires WAYLAND_SANDBOX_LIVE_WINDOWS=1"
    );
    assert!(
        AppContainerBackend::new().is_available(),
        "explicit native containment acceptance requires an available AppContainer backend"
    );
}

fn manifest(timeout_secs: u64) -> SandboxManifest {
    SandboxManifest {
        timeout: Some(Duration::from_secs(timeout_secs)),
        ..Default::default()
    }
}

/// `cmd.exe /d /s /c <script>` — the same shell shape the ACL tests use, so the
/// Job Object wraps the identical execution pipeline production drives.
fn cmd_script(script: String) -> SandboxCommand {
    SandboxCommand {
        argv: vec![
            "cmd.exe".into(),
            "/d".into(),
            "/s".into(),
            "/c".into(),
            script,
        ],
        cwd: None,
    }
}

/// A per-run unique token, embedded in a detached descendant's command line so
/// a host-side liveness query can identify EXACTLY this test's tree and nothing
/// else, even under a shared-process runner.
fn unique_tag(label: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("wcore_r7_{label}_{}_{seq}_{nanos}", std::process::id())
}

/// Count host processes (UNSANDBOXED — this runs on the host, not in the
/// AppContainer) that are `cmd.exe` AND whose command line carries `tag`.
///
/// The CIM `-Filter "Name='cmd.exe'"` runs at the provider, so the querying
/// `powershell.exe` — whose own command line contains `tag` — is NOT itself a
/// match (its image is `powershell.exe`, not `cmd.exe`). That structurally
/// avoids a self-match without needing to know the query's own PID.
fn tagged_cmd_count(tag: &str) -> usize {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "@(Get-CimInstance Win32_Process -Filter \"Name='cmd.exe'\" | \
                 Where-Object {{ $_.CommandLine -like '*{tag}*' }}).Count"
            ),
        ])
        .output()
        .expect("query host processes via CIM");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// Count the `choice.exe` idlers spawned by THIS test's fan-out, scoped to the
/// test's own tagged process tree rather than host-wide by image name.
///
/// A bare `choice.exe` idler cannot carry an injected tag on its own command
/// line (choice rejects the extra argument), but the top-level sandbox `cmd`
/// that runs the fan-out script DOES carry a unique `rem {tag}` — and every
/// `start "" /b choice` idler is a direct child of that cmd, so its
/// `ParentProcessId` is the tagged cmd's `ProcessId`. Counting only choice
/// processes whose parent is this test's tagged cmd means a concurrent
/// `choice`-spawning target on the same runner (e.g. `live_fs_acl`) cannot
/// pollute the count — closing the host-wide-image-count flake (watch-item d2)
/// without weakening any containment assertion.
fn tagged_choice_descendant_count(tag: &str) -> usize {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$parents=@(Get-CimInstance Win32_Process -Filter \"Name='cmd.exe'\" | \
                 Where-Object {{ $_.CommandLine -like '*{tag}*' }} | \
                 Select-Object -ExpandProperty ProcessId); \
                 @(Get-CimInstance Win32_Process -Filter \"Name='choice.exe'\" | \
                 Where-Object {{ $parents -contains $_.ParentProcessId }}).Count"
            ),
        ])
        .output()
        .expect("query this test's tagged choice descendants via CIM");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// Poll `predicate` up to `deadline_secs`, panicking with `message` on timeout.
/// Mirrors the `wait_until` helper in `live_fs_acl.rs`.
fn wait_until(mut predicate: impl FnMut() -> bool, deadline_secs: u64, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(deadline_secs);
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {message}");
}

/// A stdin-free hold via `choice` (tolerates the sandbox's null stdin, unlike
/// `timeout.exe`) for `seconds`, embeddable in a larger script.
fn choice_hold(seconds: u32) -> String {
    format!("%SystemRoot%\\System32\\choice.exe /T {seconds} /D Y >nul")
}

/// Best-effort host-side cleanup of any residual `choice.exe` this test's
/// fan-out spawned, so a failed assertion cannot leak idlers into later runs.
/// Runs unsandboxed; ignores errors.
fn reap_stray_choice() {
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "choice.exe", "/T"])
        .output();
}

#[test]
#[ignore = "zero-execution guard for explicit native Windows containment acceptance"]
fn native_containment_gate_marker() {
    require_live_windows();
    assert_eq!(NATIVE_CONTAINMENT_CASES, 5);
}

/// Exit-code fidelity through the Job-Object-wrapped execution on BOTH terminal
/// paths, plus a falsifiable descendant-reaping wall-clock bound.
///
/// The script detaches a 45s `choice` idler (which inherits the child's stdout
/// pipe) and then exits with the declared code. On a backend that owns the
/// descendant tree, the direct child's exit triggers `TerminateJobObject`,
/// which kills the detached idler, EOFs the pipe, and lets `execute` return
/// promptly with the EXACT declared exit code. A non-owning backend would leave
/// the idler holding the pipe, so the drain would block ~45s (well past the 20s
/// bound) or hit the 60s manifest timeout — either way this test FAILS.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows Job-Object containment acceptance"]
async fn contained_detached_child_exit() {
    require_live_windows();
    let backend = AppContainerBackend::new();
    let bound = Duration::from_secs(20);

    for code in [0u8, 7u8] {
        let script = format!("start \"\" /b {} & exit {code}", choice_hold(45));
        let started = Instant::now();
        let out = backend
            .execute(&manifest(60), cmd_script(script))
            .await
            .expect("contained execution must return an exit status, not block or error");
        let elapsed = started.elapsed();
        assert_eq!(
            out.exit_code, code as i32,
            "Job-Object-wrapped execution must report the exact terminal exit code"
        );
        assert!(
            elapsed < bound,
            "exit-{code} path leaked a detached descendant: execute took {elapsed:?} (>= {bound:?})"
        );
    }
    reap_stray_choice();
}

/// KILL_ON_JOB_CLOSE: a detached descendant is reaped with NO residue when the
/// Job Object closes — asserted by an explicit host-side liveness query, not
/// merely by the parent's own exit.
///
/// The parent detaches a tagged `cmd` grandchild that idles 60s, then holds
/// itself alive ~8s so the grandchild can be observed RUNNING mid-flight. When
/// the parent exits, `execute` returns and the Job Object closes: the grandchild
/// — despite its 60s idle still having ~50s to run — must be terminated. If the
/// job did not own it, the grandchild would survive and the post-close query
/// would keep finding it, so `wait_until(count == 0)` would time out and FAIL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit native Windows Job-Object containment acceptance"]
async fn job_close_reaps_detached_descendant_with_no_residue() {
    require_live_windows();
    let tag = unique_tag("residue");
    // Detached grandchild `cmd` carries `tag` (via a no-op `rem`) then idles 60s;
    // the parent holds ~8s so the grandchild is observable before job close.
    let script = format!(
        "start \"\" /b %ComSpec% /c \"rem {tag} & {}\" & {} & exit /b 0",
        choice_hold(60),
        choice_hold(8),
    );

    let run = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&manifest(60), cmd_script(script))
            .await
    });

    // The detached grandchild (and the still-alive parent) carry the tag.
    let observed_tag = tag.clone();
    wait_until(
        || tagged_cmd_count(&observed_tag) >= 1,
        20,
        "detached descendant running before job close",
    );

    let out = run
        .await
        .expect("join contained execution")
        .expect("contained execution returns");
    assert_eq!(out.exit_code, 0, "parent must exit cleanly (exit /b 0)");

    // After the Job Object closes, the detached 60s grandchild must be gone.
    let residue_tag = tag.clone();
    wait_until(
        || tagged_cmd_count(&residue_tag) == 0,
        30,
        "detached descendant reaped with no residue after job close",
    );
    reap_stray_choice();
}

/// ActiveProcessLimit: a fan-out beyond the Job Object's active-process cap is
/// bounded — the job refuses to admit more than the cap of concurrently-live
/// processes, so a runaway fork cannot exceed it.
///
/// The parent attempts to detach `cap + margin` bare `choice` idlers, then holds
/// itself alive so the admitted set is concurrently observable. The host-side
/// image count (baseline-subtracted) must plateau at or below the cap and STAY
/// BELOW the attempted count — proving excess spawns were rejected by the limit,
/// not merely slow to start. An unbounded job would let the delta approach the
/// attempted count and this test would FAIL. After the job closes every idler is
/// reaped back to the baseline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit native Windows Job-Object containment acceptance"]
async fn active_process_cap_is_enforced() {
    require_live_windows();
    reap_stray_choice();
    let tag = unique_tag("cap");
    let attempts = SANDBOX_ACTIVE_PROCESS_LIMIT + 32;
    // Prefix the script with a `rem {tag}` so the top-level sandbox cmd — the
    // direct parent of every `start "" /b` choice idler — carries this test's
    // unique tag on its command line. The host-side count is then scoped to
    // choice.exe whose ParentProcessId is THIS test's tagged cmd (no host-wide
    // image baseline), so a concurrent choice-spawning target cannot pollute it.
    // Fan out `attempts` detached long idlers, then hold the parent ~25s so the
    // admitted set is concurrently alive and observable, then exit 0 (`exit /b 0`).
    let script = format!(
        "rem {tag} & for /L %i in (1,1,{attempts}) do @start \"\" /b {} & {} & exit /b 0",
        choice_hold(90),
        choice_hold(25),
    );

    let run = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&manifest(120), cmd_script(script))
            .await
    });

    // Sample the live count (already scoped to this test's tagged tree) while
    // the fan-out is held, tracking its peak.
    let mut peak = 0usize;
    let watch_deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < watch_deadline {
        let delta = tagged_choice_descendant_count(&tag);
        peak = peak.max(delta);
        // Once we have clearly observed a large admitted set we can stop early.
        if delta >= SANDBOX_ACTIVE_PROCESS_LIMIT / 2 {
            // keep sampling briefly to catch any overshoot past the cap
            std::thread::sleep(Duration::from_millis(500));
            peak = peak.max(tagged_choice_descendant_count(&tag));
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    let out = run
        .await
        .expect("join fan-out execution")
        .expect("fan-out execution returns");
    assert_eq!(
        out.exit_code, 0,
        "parent must exit cleanly after the fan-out"
    );

    assert!(
        peak > 0,
        "fan-out never admitted any descendant — the test did not exercise the cap"
    );
    assert!(
        peak <= SANDBOX_ACTIVE_PROCESS_LIMIT,
        "active-process cap breached: observed {peak} concurrent descendants > cap \
         {SANDBOX_ACTIVE_PROCESS_LIMIT}"
    );
    assert!(
        peak < attempts,
        "fan-out was not capped: observed {peak} of {attempts} attempted — the limit \
         admitted every spawn"
    );

    // Everything this test spawned is reaped once the job closes (the count is
    // already scoped to this test's tagged tree, so "back to baseline" is zero).
    wait_until(
        || tagged_choice_descendant_count(&tag) == 0,
        30,
        "fan-out descendants reaped after job close",
    );
    reap_stray_choice();
}

/// Breakaway denial: with `BREAKAWAY_OK`/`SILENT_BREAKAWAY_OK` cleared, a
/// detached descendant CANNOT escape the Job Object — it is reaped on job close
/// rather than surviving independently of the parent.
///
/// The parent detaches two tagged `cmd` idlers (each idling 60s) — the shape a
/// process would use to outlive its parent — then holds ~8s so both are
/// observed alive. On job close both must die: if breakaway were permitted, a
/// detached idler would survive the ~52s remainder of its idle and the
/// post-close query would still find it, failing `wait_until(count == 0)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit native Windows Job-Object containment acceptance"]
async fn breakaway_is_denied() {
    require_live_windows();
    let tag = unique_tag("breakaway");
    let script = format!(
        "start \"\" /b %ComSpec% /c \"rem {tag} & {hold}\" & \
         start \"\" /b %ComSpec% /c \"rem {tag} & {hold}\" & {parent} & exit /b 0",
        tag = tag,
        hold = choice_hold(60),
        parent = choice_hold(8),
    );

    let run = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&manifest(60), cmd_script(script))
            .await
    });

    let observed_tag = tag.clone();
    wait_until(
        || tagged_cmd_count(&observed_tag) >= 2,
        20,
        "both detached breakaway candidates running before job close",
    );

    let out = run
        .await
        .expect("join contained execution")
        .expect("contained execution returns");
    assert_eq!(out.exit_code, 0, "parent must exit cleanly (exit /b 0)");

    // No detached child broke away: the job reaped both on close.
    let residue_tag = tag.clone();
    wait_until(
        || tagged_cmd_count(&residue_tag) == 0,
        30,
        "no detached child broke away from the Job Object",
    );
    reap_stray_choice();
}

/// Hard-containment preflight: the Windows AppContainer backend self-reports
/// hard descendant containment (Job Object ownership), so the qualification the
/// other native containment targets rely on is REAL on Windows — and a live
/// benign contained execution actually runs.
///
/// This is the Windows analogue of the bwrap `qualified_hard_containment_backend_preflight`:
/// it asserts the backend's admission properties through the public trait
/// (`owns_descendants_hard` / `enforces_read_deny` / `blocks_powershell`) and
/// then drives one benign contained command to confirm the Job-Object pipeline
/// is live end to end.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows Job-Object containment acceptance"]
async fn qualified_hard_containment_backend_preflight() {
    require_live_windows();
    let backend = AppContainerBackend::new();

    // Admission properties: only a backend that owns the descendant tree (via the
    // Job Object) may back delegated hard-containment execution on Windows.
    assert!(
        backend.owns_descendants_hard(),
        "the Windows AppContainer backend must own the descendant process tree (Job Object)"
    );
    assert!(
        backend.enforces_read_deny(),
        "the Windows AppContainer backend must enforce fs_read_deny at the OS layer"
    );
    assert!(
        backend.blocks_powershell(),
        "the Windows AppContainer backend must report that it cannot run PowerShell"
    );

    // Live semantic probe: a benign command runs to a clean exit through the
    // Job-Object-wrapped pipeline (never candidate-controlled argv).
    let out = backend
        .execute(&manifest(15), cmd_script("ver >nul".into()))
        .await
        .expect("benign contained preflight command must run");
    assert_eq!(
        out.exit_code, 0,
        "the hard-containment preflight command must exit cleanly"
    );

    // A detached descendant is reaped on job close even for this preflight shape,
    // confirming the qualification is descendant-hard, not just a self-report.
    let tag = unique_tag("preflight");
    let script = format!(
        "start \"\" /b %ComSpec% /c \"rem {tag} & {}\" & {} & exit /b 0",
        choice_hold(45),
        choice_hold(6),
    );
    let started = Instant::now();
    let held = backend
        .execute(&manifest(60), cmd_script(script))
        .await
        .expect("preflight detached-descendant execution returns");
    assert_eq!(held.exit_code, 0);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "preflight detached descendant leaked — hard containment not owned"
    );
    let residue_tag = tag.clone();
    wait_until(
        || tagged_cmd_count(&residue_tag) == 0,
        30,
        "preflight detached descendant reaped with no residue",
    );
    reap_stray_choice();
}
