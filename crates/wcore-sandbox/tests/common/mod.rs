//! Shared live-Windows sandbox observation helpers.
//!
//! These are the ONLY primitives proven to work under the Low-IL AppContainer
//! restricted token on real hardware, and they are shared rather than copied on
//! purpose. `live_future_drop_reaps_descendant_job_tree` (the `KR-01` test)
//! originally grew its own bespoke construction — a `choice.exe` heartbeat
//! writing to a file under `%PUBLIC%` — and that construction could not run at
//! all: `choice.exe` exits in <80ms under this token, and the nested spawn shape
//! it used returns `Access is denied.` before any descendant exists. The test
//! therefore aborted in its own setup and never reached its assertion, while its
//! red was attributed to the defect it was written to catch (F-WR-01).
//!
//! A second private copy of these primitives is how that happened. Keep one.

#![allow(dead_code)]

use std::process::Command;
use std::time::{Duration, Instant};

use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

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
pub fn resolve_anchor_pid() -> Option<u32> {
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
pub fn live_descendant_count() -> usize {
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
pub fn live_descendant_pids() -> Vec<u32> {
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
pub fn capture_alive_descendant_pids(min_expected: usize, deadline_secs: u64) -> Vec<u32> {
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
pub fn surviving_captured_descendant_pids(pids: &[u32]) -> usize {
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
pub fn wait_until(mut predicate: impl FnMut() -> bool, deadline_secs: u64, message: &str) {
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
pub fn hold_iterations(seconds: u32) -> u64 {
    4_000_000 * u64::from(seconds).clamp(1, 2)
}

/// A bare, inline cmd-builtin busy-loop that holds the CURRENT cmd (the anchor)
/// alive for ~`seconds` (clamped) WITHOUT spawning any child process — so it does
/// not add a spurious `cmd.exe` descendant to the observers, and (unlike a detached
/// `start "" /b` hold) it runs SYNCHRONOUSLY, which is what actually keeps the
/// anchor — and thus the Job Object — open across the observation window. MUST stay
/// bare: a parenthesized `(for /L ...)` fails to parse under `cmd /d /s /c`. Uses
/// command-line single `%i` (NOT batch `%%i`).
pub fn inline_hold(seconds: u32) -> String {
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
pub fn descendant_hold(seconds: u32) -> String {
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
pub fn reap_stray_descendants() {
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

pub fn require_live_windows() {
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
pub fn manifest(timeout_secs: u64) -> SandboxManifest {
    SandboxManifest {
        timeout: Some(Duration::from_secs(timeout_secs)),
        ..Default::default()
    }
}
/// `cmd.exe /d /s /c <script>` — the same shell shape the ACL tests use, so the
/// Job Object wraps the identical execution pipeline production drives.
pub fn cmd_script(script: String) -> SandboxCommand {
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
