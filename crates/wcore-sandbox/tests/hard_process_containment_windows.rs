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

/// Resolve THIS test's sandbox anchor — the top-level `cmd.exe` the backend
/// launched for `execute()`. `windows_impl/process.rs` calls `CreateProcessAsUserW`
/// directly from the test process with NO `PROC_THREAD_ATTRIBUTE_PARENT_PROCESS`
/// reparenting, so the anchor cmd's `ParentProcessId` is this test process's PID.
/// Under nextest's process-per-test that PID is unique to this test, and only ONE
/// `execute()` is ever in flight during an observation, so at most one such cmd.exe
/// exists. The fan-out descendants are grandchildren (their parent is the anchor,
/// not the test process), so this query returns the anchor ALONE — never a
/// descendant — and the observer `powershell.exe` children are excluded by image
/// name.
///
/// This replaces the former window-title / `.hs` PID handshake, which could NEVER
/// yield a PID under the sandbox (Class D): a console-less sandbox cmd has no
/// matchable window title, and the handshake file was never created under the
/// Low-IL restricted token. `ProcessId`/`ParentProcessId`/`Name` are WMI-readable
/// even for AppContainer processes (only `CommandLine` is NULL — never relied on
/// anywhere), so a plain PPID anchor is both available and unique.
///
/// Returns `None` WHILE no anchor is running yet (execute not launched) — a
/// legitimate "not observed yet" that keeps the alive-phase poll waiting. Fails
/// CLOSED once a query IS issued: a non-success `powershell` exit, an unparseable
/// `ProcessId`, or MORE THAN ONE candidate anchor (an ambiguous scope that would
/// make descendant selection untrustworthy) PANICS rather than silently yielding a
/// wrong/empty anchor that would make the observers vacuously report an empty tree.
fn resolve_anchor_pid() -> Option<u32> {
    let self_pid = std::process::id();
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$ErrorActionPreference='Stop'; trap {{ exit 1 }}; \
                 @(Get-CimInstance Win32_Process -ErrorAction Stop \
                 -Filter \"Name='cmd.exe' AND ParentProcessId={self_pid}\" | \
                 Select-Object -ExpandProperty ProcessId)"
            ),
        ])
        .output()
        .expect("resolve this test's sandbox anchor cmd via CIM");
    assert!(
        out.status.success(),
        "resolve_anchor_pid CIM query failed (exit {:?}): {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let pids: Vec<u32> = stdout
        .split_whitespace()
        .map(|s| {
            s.parse::<u32>().unwrap_or_else(|err| {
                panic!("resolve_anchor_pid could not parse anchor ProcessId token {s:?}: {err}")
            })
        })
        .collect();
    assert!(
        pids.len() <= 1,
        "resolve_anchor_pid found {} candidate anchors (cmd.exe children of pid {self_pid}); \
         the descendant scope would be ambiguous",
        pids.len()
    );
    pids.first().copied()
}

/// Count host processes (UNSANDBOXED — this runs on the host, not in the
/// AppContainer) that are this test's own live sandbox descendants: `cmd.exe`
/// busy-loop idlers whose `ParentProcessId` is the anchor from
/// [`resolve_anchor_pid`]. The anchor itself is excluded — its parent is the test
/// process, not the anchor. The querying `powershell.exe` is not a match either —
/// its image is `powershell.exe`, not `cmd.exe`.
///
/// Descendants are `cmd.exe` (each a `start "" /b cmd /d /s /c "for /L ..."`
/// idler), NOT `choice.exe`: every external exe — choice/waitfor/timeout/ping —
/// exits in <80ms under the Low-IL AppContainer restricted token, so it is never
/// observed alive; a bare `for /L` cmd builtin is the only primitive that holds.
///
/// Returns 0 WHILE no anchor is running yet — the alive poll keeps waiting. Once a
/// query IS issued it fails CLOSED at BOTH layers. PowerShell layer:
/// `$ErrorActionPreference='Stop'` + `-ErrorAction Stop` on the CIM query + a
/// leading `trap` that exits non-zero escalate any non-terminating CIM/PowerShell
/// query error to a TERMINATING error that exits `powershell.exe` non-zero, so a
/// failed query can never print `@(...).Count == '0'` at exit 0. Rust layer
/// (preserved): a non-success `powershell` exit, or a `.Count` that does not parse
/// on a success exit, is a hard test failure (panic) — never silently read as a
/// passing count.
fn live_descendant_count() -> usize {
    let Some(anchor) = resolve_anchor_pid() else {
        return 0;
    };
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$ErrorActionPreference='Stop'; trap {{ exit 1 }}; \
                 @(Get-CimInstance Win32_Process -ErrorAction Stop \
                 -Filter \"Name='cmd.exe' AND ParentProcessId={anchor}\").Count"
            ),
        ])
        .output()
        .expect("query this test's live sandbox descendants via CIM");
    assert!(
        out.status.success(),
        "live_descendant_count CIM query failed (exit {:?}): {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = stdout.trim();
    text.parse().unwrap_or_else(|err| {
        panic!("live_descendant_count could not parse CIM .Count output {text:?}: {err}")
    })
}

/// Return the ProcessIds of the `cmd.exe` busy-loop idlers spawned by THIS test's
/// fan-out, scoped to the test's own tree by the anchor's `ProcessId` (from
/// [`resolve_anchor_pid`]) rather than host-wide by image name.
///
/// Every `start "" /b cmd /d /s /c "for /L ..."` idler is a direct child of the
/// anchor, so its `ParentProcessId` is the anchor's `ProcessId`. Selecting only
/// cmd.exe whose parent is this test's anchor means a concurrent cmd-spawning
/// target on the same runner (e.g. `live_fs_acl`) cannot pollute the capture — its
/// idlers hang off a different anchor.
///
/// This is the ALIVE-phase half of a two-phase reap check: the returned PIDs are
/// captured WHILE the anchor is still alive (during the peak-sampling window), and
/// the `ParentProcessId` scope is what makes that capture immune to a concurrent
/// target. Once the job closes the anchor is dead, so this parent-scoped query
/// would go structurally empty regardless of a leaked survivor — the post-close
/// survivor check is therefore done by fixed ProcessId via
/// [`surviving_captured_descendant_pids`], NOT by re-running this parent-scoped
/// query.
///
/// Fails CLOSED at BOTH layers exactly as [`live_descendant_count`]: the `trap` +
/// `-ErrorAction Stop` escalate any non-terminating CIM error to a non-zero
/// `powershell.exe` exit, so a query error can no longer yield an empty token
/// stream at exit 0 — an empty stdout can ONLY mean a genuine
/// success-with-no-descendants. A non-success exit is a hard test failure (panic),
/// and each token is parsed with a panicking parse. A LEGITIMATE empty result (no
/// descendants yet, or no anchor yet) still yields an empty `Vec`.
fn live_descendant_pids() -> Vec<u32> {
    let Some(anchor) = resolve_anchor_pid() else {
        return Vec::new();
    };
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$ErrorActionPreference='Stop'; trap {{ exit 1 }}; \
                 @(Get-CimInstance Win32_Process -ErrorAction Stop \
                 -Filter \"Name='cmd.exe' AND ParentProcessId={anchor}\" | \
                 Select-Object -ExpandProperty ProcessId)"
            ),
        ])
        .output()
        .expect("query this test's live sandbox descendant PIDs via CIM");
    assert!(
        out.status.success(),
        "live_descendant_pids CIM query failed (exit {:?}): {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .split_whitespace()
        .map(|s| {
            s.parse::<u32>().unwrap_or_else(|err| {
                panic!("live_descendant_pids could not parse ProcessId token {s:?}: {err}")
            })
        })
        .collect()
}

/// Peak-sample the live `cmd.exe` descendants of this test's anchor WHILE the job
/// is held open, returning the largest PID set observed (captured while the anchor
/// is still alive). Requires at least `min_expected` concurrently live so the
/// captured set is non-empty and the post-close reap check via
/// [`surviving_captured_descendant_pids`] is non-vacuous; panics (fail-closed) if
/// that many are never observed within `deadline_secs`, rather than returning an
/// empty set that would let the reap pass without evidence.
fn capture_alive_descendant_pids(min_expected: usize, deadline_secs: u64) -> Vec<u32> {
    let deadline = Instant::now() + Duration::from_secs(deadline_secs);
    let mut peak: Vec<u32> = Vec::new();
    while Instant::now() < deadline {
        let pids = live_descendant_pids();
        if pids.len() > peak.len() {
            peak = pids;
        }
        if peak.len() >= min_expected {
            return peak;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "timed out capturing >= {min_expected} live cmd.exe descendants of the anchor \
         (peak observed {})",
        peak.len()
    );
}

/// Count how many of the `pids` fan-out `cmd.exe` ProcessIds are STILL alive,
/// matched by fixed `ProcessId` intersected with image `cmd.exe`.
///
/// This is the POST-CLOSE half of the two-phase reap check. Because it filters
/// on the exact PIDs captured while the anchor was alive — not on the now-dead
/// anchor, and not host-wide by image name — it is:
///   * non-vacuous — a leaked/orphaned captured idler (same PID, still `cmd.exe`)
///     is counted, so a survivor stays detectable; and
///   * not host-wide-flaky — a concurrent target's `cmd.exe` carries a different,
///     non-captured PID and is excluded.
///
/// An empty `pids` slice yields 0 without issuing a malformed filter.
///
/// Fails CLOSED at BOTH layers exactly as [`live_descendant_count`]: past the
/// legitimate empty-set short-circuit, a non-success `powershell` exit, or a
/// `.Count` that does not parse on a success exit, is a hard test failure (panic)
/// — never silently read as a passing survivor count. A post-close query failure
/// therefore cannot satisfy the reap `wait_until(... == 0)` without evidence.
fn surviving_captured_descendant_pids(pids: &[u32]) -> usize {
    if pids.is_empty() {
        return 0;
    }
    let pid_list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$ErrorActionPreference='Stop'; trap {{ exit 1 }}; $pids=@({pid_list}); \
                 @(Get-CimInstance Win32_Process -ErrorAction Stop -Filter \"Name='cmd.exe'\" | \
                 Where-Object {{ $pids -contains $_.ProcessId }}).Count"
            ),
        ])
        .output()
        .expect("query survival of captured descendant PIDs via CIM");
    assert!(
        out.status.success(),
        "surviving_captured_descendant_pids CIM query failed (exit {:?}): {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = stdout.trim();
    text.parse().unwrap_or_else(|err| {
        panic!(
            "surviving_captured_descendant_pids could not parse CIM .Count output {text:?}: {err}"
        )
    })
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

/// Iterations for a pure-cmd `for /L` busy-loop hold. Every external exe —
/// `choice.exe`, `waitfor.exe`, `timeout.exe`, `ping` — exits in <80ms under the
/// Low-IL AppContainer restricted token (console/DLL/network deps fail to load),
/// so NONE actually hold; a `for /L` loop is a cmd BUILTIN (no child image, no DLL,
/// no stdin, no network) and is the only primitive that holds under this sandbox
/// (hardware-verified ~2s), exactly as `live_fs_acl.rs` does. Capped via `clamp` so
/// the hold is ~2s on reference hardware regardless of the nominal `seconds` —
/// above the 100ms observe-poll, below the manifest timeout on slow CI — rather
/// than a machine-timed value that could overrun the timeout.
fn hold_iterations(seconds: u32) -> u64 {
    4_000_000 * u64::from(seconds).clamp(1, 2)
}

/// A bare, inline cmd-builtin busy-loop that holds the CURRENT cmd (the anchor)
/// alive for ~`seconds` (clamped) WITHOUT spawning any child process — so it does
/// not add a spurious `cmd.exe` descendant to the observers, and (unlike a detached
/// `start "" /b` hold) it runs SYNCHRONOUSLY, which is what actually keeps the
/// anchor — and thus the Job Object — open across the observation window. MUST stay
/// bare: a parenthesized `(for /L ...)` fails to parse under `cmd /d /s /c`. Uses
/// command-line single `%i` (NOT batch `%%i`).
fn inline_hold(seconds: u32) -> String {
    format!("for /L %i in (1,1,{}) do @rem", hold_iterations(seconds))
}

/// A DETACHED descendant `cmd.exe` that busy-holds ~`seconds` (clamped). Wrapped by
/// the caller in `start "" /b`, it is a distinct `cmd.exe` process whose parent is
/// the anchor — the shape the observers count. The same bare `for /L` builtin is the
/// only hold that survives the sandbox; a descendant built on `choice.exe` et al.
/// would exit in <80ms and never be observed alive. Uses single `%i`; MUST stay
/// bare. Where this is nested inside another `for /L` fan-out (the cap test), that
/// OUTER loop deliberately uses a different variable (`%k`) so it cannot clobber
/// this inner `%i` during the outer loop's per-iteration substitution.
fn descendant_hold(seconds: u32) -> String {
    format!(
        "cmd /d /s /c \"for /L %i in (1,1,{}) do @rem\"",
        hold_iterations(seconds)
    )
}

/// Best-effort host-side cleanup of any residual `cmd.exe` idlers this test's
/// fan-out spawned under its anchor, so a failed assertion cannot leak idlers into
/// later runs. Scoped to the anchor's own children by `ParentProcessId` — NEVER a
/// blanket `taskkill /IM cmd.exe`, which would kill unrelated shells (the nextest
/// runner, CI cmd, other tests). If the anchor is already gone (the job closed),
/// its descendants were reaped with it and there is nothing to do. Runs
/// unsandboxed; ignores every error (never panics — this is cleanup, not an
/// assertion, so it does NOT reuse the fail-closed [`resolve_anchor_pid`]).
fn reap_stray_descendants() {
    let self_pid = std::process::id();
    let _ = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "foreach($a in @(Get-CimInstance Win32_Process \
                 -Filter \"Name='cmd.exe' AND ParentProcessId={self_pid}\")) {{ \
                 Get-CimInstance Win32_Process \
                 -Filter \"Name='cmd.exe' AND ParentProcessId=$($a.ProcessId)\" | \
                 ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }} }}"
            ),
        ])
        .output();
}

#[test]
#[ignore = "zero-execution guard for explicit native Windows containment acceptance"]
fn native_containment_gate_marker() {
    require_live_windows();
    assert_eq!(NATIVE_CONTAINMENT_CASES, 5);
}

/// Exit-code fidelity through the Job-Object-wrapped execution on BOTH terminal
/// paths, plus a descendant-reaping wall-clock bound.
///
/// The script detaches a `for /L` busy-loop idler `cmd.exe` (which inherits the
/// child's stdout pipe) and then exits with the declared code. On a backend that
/// owns the descendant tree, the direct child's exit triggers `TerminateJobObject`,
/// which kills the detached idler, EOFs the pipe, and lets `execute` return
/// promptly with the EXACT declared exit code. The idler holds via `for /L` rather
/// than `choice.exe`/`timeout.exe` because every external exe exits in <80ms under
/// the sandbox and so would never hold the pipe at all — making the drain-blocking
/// falsification vacuous. NOTE: the sandbox caps any hold at ~2s (no primitive
/// survives longer), so the wall-clock margin here is ~2s rather than the former
/// nominal 45s; the exact-exit-code fidelity is the primary assertion, and the ~2s
/// `for /L` hold keeps the reaping coverage non-vacuous (the descendant genuinely
/// persists and inherits the pipe).
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows Job-Object containment acceptance"]
async fn contained_detached_child_exit() {
    require_live_windows();
    let backend = AppContainerBackend::new();
    let bound = Duration::from_secs(20);

    for code in [0u8, 7u8] {
        let script = format!("start \"\" /b {} & exit {code}", descendant_hold(45));
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
    reap_stray_descendants();
}

/// KILL_ON_JOB_CLOSE: a detached descendant is reaped with NO residue when the
/// Job Object closes — asserted by an explicit host-side liveness query, not
/// merely by the parent's own exit.
///
/// The parent detaches a `choice` idler (a direct child of the anchor) that
/// idles 60s, then holds itself alive ~8s so the idler can be observed RUNNING
/// mid-flight (and its ProcessId captured). When the parent exits, `execute`
/// returns and the Job Object closes: the idler — despite its 60s idle still
/// having ~50s to run — must be terminated. If the job did not own it, the idler
/// would survive and the post-close fixed-ProcessId survivor query would keep
/// finding it, so `wait_until(surviving == 0)` would time out and FAIL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit native Windows Job-Object containment acceptance"]
async fn job_close_reaps_detached_descendant_with_no_residue() {
    require_live_windows();
    // The top-level sandbox cmd (the anchor, whose PPID is this test process)
    // detaches a `for /L` idler cmd.exe (a direct child of the anchor), then holds
    // itself alive with an INLINE `for /L` so the idler is observable before job
    // close. The inline hold keeps the anchor synchronous — a `start "" /b` hold
    // would return immediately and close the job before observation.
    let script = format!(
        "start \"\" /b {idle} & {parent} & exit /b 0",
        idle = descendant_hold(60),
        parent = inline_hold(8),
    );

    let run = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&manifest(60), cmd_script(script))
            .await
    });

    // Observe the detached idler running before job close (count-based observer).
    wait_until(
        || live_descendant_count() >= 1,
        20,
        "detached descendant running before job close",
    );
    // Capture the detached idler by fixed ProcessId WHILE the anchor is alive, so
    // the post-close reap check is non-vacuous. Only the idler is a descendant now
    // — the parent hold is inline and spawns no process.
    let captured = capture_alive_descendant_pids(1, 20);

    let out = run
        .await
        .expect("join contained execution")
        .expect("contained execution returns");
    assert_eq!(out.exit_code, 0, "parent must exit cleanly (exit /b 0)");

    assert!(
        !captured.is_empty(),
        "peak PID set was not captured — the post-close reap check would be vacuous"
    );

    // After the Job Object closes, the detached idler must be gone — checked by
    // the EXACT captured ProcessIds, since the anchor is dead and the parent-scoped
    // query would go structurally empty regardless of a survivor.
    wait_until(
        || surviving_captured_descendant_pids(&captured) == 0,
        30,
        "detached descendant reaped with no residue after job close",
    );
    reap_stray_descendants();
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
    reap_stray_descendants();
    let attempts = SANDBOX_ACTIVE_PROCESS_LIMIT + 32;
    // The host-side count is scoped to cmd.exe whose ParentProcessId is THIS test's
    // anchor (the top-level sandbox cmd, resolved by its PPID == this test process),
    // so no host-wide image baseline and no dependency on the NULL AppContainer
    // CommandLine — a concurrent cmd-spawning target cannot pollute it. Fan out
    // `attempts` detached `for /L` idlers, then hold the anchor with an INLINE
    // `for /L` so the admitted set is concurrently alive and observable, then exit 0.
    //
    // The OUTER fan-out loop uses `%k` (not `%i`): its per-iteration substitution
    // would otherwise clobber the inner `%i` inside each descendant's own
    // `cmd /d /s /c "for /L %i ..."`, breaking the nested hold. The outer body never
    // references its own loop variable, so `%k` vs `%i` is invisible to it.
    let script = format!(
        "for /L %k in (1,1,{attempts}) do @start \"\" /b {idle} & {parent} & exit /b 0",
        idle = descendant_hold(90),
        parent = inline_hold(25),
    );

    let run = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&manifest(120), cmd_script(script))
            .await
    });

    // Sample the live PID set (already scoped to this test's tagged tree) while
    // the fan-out is held, tracking its peak and capturing the peak PID set so
    // the post-close reap check can re-verify those exact ProcessIds by fixed
    // PID — the parent-scoped query goes structurally empty once the tagged
    // parent dies at job close.
    let mut peak = 0usize;
    let mut captured_pids: Vec<u32> = Vec::new();
    let watch_deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < watch_deadline {
        let pids = live_descendant_pids();
        if pids.len() > peak {
            peak = pids.len();
            captured_pids = pids.clone();
        }
        // Once we have clearly observed a large admitted set we can stop early.
        if pids.len() >= SANDBOX_ACTIVE_PROCESS_LIMIT / 2 {
            // keep sampling briefly to catch any overshoot past the cap
            std::thread::sleep(Duration::from_millis(500));
            let overshoot = live_descendant_pids();
            if overshoot.len() > peak {
                peak = overshoot.len();
                captured_pids = overshoot;
            }
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
    assert!(
        !captured_pids.is_empty(),
        "peak PID set was not captured — the post-close reap check would be vacuous"
    );

    // After the job closes, re-check the EXACT fan-out `cmd.exe` PIDs captured
    // while the anchor was alive, by fixed ProcessId intersected with image
    // `cmd.exe`. This is non-vacuous (a leaked/orphaned captured survivor is still
    // counted) and not host-wide-flaky (a concurrent target's `cmd.exe` carries a
    // different, non-captured PID). The parent-scoped query cannot be reused — the
    // anchor is dead once `run.await` returned.
    wait_until(
        || surviving_captured_descendant_pids(&captured_pids) == 0,
        30,
        "fan-out descendants reaped after job close (by captured PID)",
    );
    reap_stray_descendants();
}

/// Breakaway denial: with `BREAKAWAY_OK`/`SILENT_BREAKAWAY_OK` cleared, a
/// detached descendant CANNOT escape the Job Object — it is reaped on job close
/// rather than surviving independently of the parent.
///
/// The parent detaches two `choice` idlers (direct children of the anchor, each
/// idling 60s) — the shape a process would use to outlive its parent — then holds
/// ~8s so both are observed alive and their ProcessIds captured. On job close
/// both must die: if breakaway were permitted, a detached idler would survive the
/// ~52s remainder of its idle and the post-close fixed-ProcessId survivor query
/// would still find it, failing `wait_until(surviving == 0)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit native Windows Job-Object containment acceptance"]
async fn breakaway_is_denied() {
    require_live_windows();
    // Two detached `for /L` idler cmd.exe (direct children of the anchor) — the
    // shape a process would use to outlive its parent — plus the anchor's own
    // INLINE hold. The anchor is the top-level sandbox cmd, resolved host-side by
    // its PPID == this test process.
    let script = format!(
        "start \"\" /b {hold} & start \"\" /b {hold} & {parent} & exit /b 0",
        hold = descendant_hold(60),
        parent = inline_hold(8),
    );

    let run = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&manifest(60), cmd_script(script))
            .await
    });

    // Observe both detached breakaway candidates running before job close
    // (count-based observer).
    wait_until(
        || live_descendant_count() >= 2,
        20,
        "both detached breakaway candidates running before job close",
    );
    // Capture the two detached breakaway candidates by fixed ProcessId while the
    // anchor is alive, so the reap check is non-vacuous. Only the two idlers are
    // descendants now — the parent hold is inline and spawns no process.
    let captured = capture_alive_descendant_pids(2, 20);
    assert!(
        captured.len() >= 2,
        "both detached breakaway candidates must be observed alive before job close"
    );

    let out = run
        .await
        .expect("join contained execution")
        .expect("contained execution returns");
    assert_eq!(out.exit_code, 0, "parent must exit cleanly (exit /b 0)");

    // No detached child broke away: the job reaped both on close — checked by the
    // EXACT captured ProcessIds, since the anchor is dead post-close.
    wait_until(
        || surviving_captured_descendant_pids(&captured) == 0,
        30,
        "no detached child broke away from the Job Object",
    );
    reap_stray_descendants();
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
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    // The detached `for /L` idler cmd.exe is a direct child of the anchor (the
    // top-level sandbox cmd, resolved host-side by its PPID == this test process);
    // the anchor holds itself alive with an inline `for /L`.
    let script = format!(
        "start \"\" /b {idle} & {parent} & exit /b 0",
        idle = descendant_hold(45),
        parent = inline_hold(6),
    );
    let started = Instant::now();
    let run = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&manifest(60), cmd_script(script))
            .await
    });
    // Observe the detached idler running before job close (count-based observer).
    wait_until(
        || live_descendant_count() >= 1,
        20,
        "preflight detached descendant running before job close",
    );
    // Capture the detached idler by fixed ProcessId while the anchor is alive.
    let captured = capture_alive_descendant_pids(1, 20);
    let held = run
        .await
        .expect("join preflight detached-descendant execution")
        .expect("preflight detached-descendant execution returns");
    assert_eq!(held.exit_code, 0);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "preflight detached descendant leaked — hard containment not owned"
    );
    assert!(
        !captured.is_empty(),
        "preflight peak PID set was not captured — the reap check would be vacuous"
    );
    wait_until(
        || surviving_captured_descendant_pids(&captured) == 0,
        30,
        "preflight detached descendant reaped with no residue",
    );
    reap_stray_descendants();
}
