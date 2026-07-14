//! Live integrity-boundary verification (negative-test style).
//!
//! The hardened AppContainer pipeline (Low integrity + disabled
//! `BUILTIN\Administrators` / `Users` / `Authenticated Users` SIDs +
//! Job Object UI restrictions) is intentionally tight enough that
//! LSA-dependent system tools (`whoami /groups`, `wmic`, `net user`)
//! fail to run inside it. We assert that as a security property: a
//! child that CAN'T enumerate its own group membership has provably
//! lost access to the LSA endpoint, which means the restricted token
//! is doing its job.
//!
//! Why this shape, rather than a positive "IL=Low" check?
//!   1. A custom probe binary (`il_probe.exe` at
//!      `src/bin/il_probe.rs`) that calls `GetTokenInformation`
//!      directly cannot load under the hardened sandbox: NTFS DACLs
//!      on `target\debug\` exclude the AppContainer SID, and copying
//!      the binary into the AppContainer package storage still leaves
//!      it unable to resolve VCRUNTIME140.dll under the
//!      disabled-Users restricted token. This is the v0.7.0 filesystem
//!      allowlist's job (queued: wire
//!      `SetNamedSecurityInfoW(GRANT, AppContainer SID)`).
//!   2. The positive Low-IL proof comes from the Procmon trace gate
//!      (verification gate #2), which observes the child's integrity
//!      level at the OS layer. The test here proves the *consequence*
//!      of Low IL + restricted token, not the property itself.
//!
//! Companion live tests:
//!   * `echo_runs_live` (in src) — proves trivial cmd.exe spawn works.
//!   * `appcontainer_execute_trivial_command_returns_exit_zero` (in
//!     `tests/backend_integration.rs`) — proves end-to-end pipeline.
//!   * THIS test — proves the boundary is tight.

#![cfg(windows)]

use std::time::Duration;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

#[tokio::test(flavor = "current_thread")]
async fn live_lsa_dependent_tool_fails_under_hardened_sandbox() {
    if std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").is_err() {
        return;
    }

    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };

    // `whoami /groups` enumerates group SIDs and calls LsaLookupSids2
    // to format friendly names like `BUILTIN\Administrators`. The lookup
    // requires the calling thread's token to grant access to the LSA
    // ALPC port `\Default`, which under our hardened pipeline it does
    // not (Admins/Users/AuthUsers SIDs are deny-only on the restricted
    // token; the AppContainer SID is not on the LSA port's DACL).
    //
    // If this test starts PASSING (whoami exit=0 with group output), it
    // means the sandbox just got LOOSER — either SidsToDisable went
    // away, the token integrity dropped to something LSA accepts, or
    // a new capability was granted. That's a security regression.
    let out = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec!["cmd.exe".into(), "/c".into(), "whoami /groups".into()],
                cwd: None,
            },
        )
        .await
        .expect("AppContainer spawn must succeed even if whoami fails inside");

    assert_ne!(
        out.exit_code,
        0,
        "whoami /groups SUCCEEDED under hardened AppContainer — sandbox just got LOOSER. \
         A successful LSA group lookup means the restricted token's SID disabling and / or \
         Low integrity pinning is no longer effective. stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Verifies the positive control: a tool with no LSA / network / USER
/// surface dependencies (just `cmd` builtins) DOES run successfully
/// inside the sandbox. This is the matched-pair to the negative test
/// above — together they prove the sandbox is "tight enough to block
/// LSA, loose enough to run a shell builtin."
#[tokio::test(flavor = "current_thread")]
async fn live_cmd_builtin_runs_under_hardened_sandbox() {
    if std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").is_err() {
        return;
    }

    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };
    let out = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec!["cmd.exe".into(), "/c".into(), "echo proof-of-life".into()],
                cwd: None,
            },
        )
        .await
        .expect("AppContainer cmd /c echo spawn failed");
    assert_eq!(
        out.exit_code,
        0,
        "cmd /c echo should run inside the hardened sandbox; stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("proof-of-life"),
        "expected 'proof-of-life' in stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Field regression (#321-324 follow-up, PR #99). The local fs allowlist
/// routinely includes optional dev caches (`~/.cache`, `~/.cargo`, `~/.npm`,
/// `~/.rustup`) that are ABSENT on non-developer machines. Before the
/// grant/deny skip-missing fix, `GetNamedSecurityInfoW` returned
/// `ERROR_FILE_NOT_FOUND` (0x2) on the absent path and aborted the whole spawn,
/// so EVERY sandboxed shell command hard-failed in the field. This proves a real
/// sandboxed `cmd` still runs end-to-end when the allowlist contains a
/// non-existent path alongside a real one.
#[tokio::test(flavor = "current_thread")]
async fn live_cmd_runs_when_allowlist_has_missing_path() {
    if std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").is_err() {
        return;
    }

    let real = std::env::temp_dir();
    let missing = std::path::PathBuf::from(r"C:\__wcore_absent_cache__\.npm");
    assert!(
        !missing.exists(),
        "precondition: the allowlist path must be absent"
    );

    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        fs_read_allow: vec![real, missing],
        timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };
    let out = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec![
                    "cmd.exe".into(),
                    "/c".into(),
                    "echo allowlist-skip-ok".into(),
                ],
                cwd: None,
            },
        )
        .await
        .expect("AppContainer spawn must succeed despite a non-existent allowlist path");
    assert_eq!(
        out.exit_code,
        0,
        "cmd must run when the allowlist has a missing path; stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("allowlist-skip-ok"),
        "expected 'allowlist-skip-ok' in stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// #100 regression: a runaway command must be bounded by the manifest timeout.
/// On timeout the backend terminates the whole job tree and reaps it before
/// draining, so the blocking `drain_pipe` can reach EOF even when the child (or
/// a helper it spawned, e.g. a console host) is still alive — otherwise the call
/// hangs far past the timeout (the 120s "command timed out, no output" symptom).
#[tokio::test(flavor = "current_thread")]
async fn live_runaway_command_is_bounded_by_timeout() {
    if std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").is_err() {
        return;
    }

    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        timeout: Some(Duration::from_secs(3)),
        ..Default::default()
    };
    let start = std::time::Instant::now();
    // `for /l %i in (0,0,1)` never reaches its end value -> infinite cmd loop.
    let r = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec![
                    "cmd.exe".into(),
                    "/c".into(),
                    "for /l %i in (0,0,1) do @rem".into(),
                ],
                cwd: None,
            },
        )
        .await;
    let secs = start.elapsed().as_secs();
    assert!(
        secs <= 8,
        "runaway command must be bounded by the 3s timeout; took {secs}s (drain hung past timeout)"
    );
    assert!(
        matches!(r, Err(wcore_sandbox::SandboxError::Timeout)),
        "expected SandboxError::Timeout, got {r:?}"
    );
}

/// Dropping the async execution future is the session-cancellation path. The
/// nested `cmd.exe` is a real descendant, so a direct-child kill would leave
/// its heartbeat advancing after this future is dropped.
#[tokio::test(flavor = "current_thread")]
async fn live_future_drop_reaps_descendant_job_tree() {
    if std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").is_err() {
        return;
    }

    let public = std::env::var_os("PUBLIC").expect("PUBLIC must be set on Windows");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must follow Unix epoch")
        .as_nanos();
    let work = std::path::PathBuf::from(public)
        .join(format!("wcore-job-cancel-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&work).expect("create cancellation test directory");
    let heartbeat = work.join("heartbeat.txt");
    let script = work.join("heartbeat.cmd");
    std::fs::write(
        &script,
        format!(
            "@echo off\r\n:loop\r\necho x>>{}\r\nchoice.exe /t 1 /d y /n >nul\r\ngoto loop\r\n",
            heartbeat.display()
        ),
    )
    .expect("write descendant heartbeat script");
    // The outer AppContainer process launches a second cmd process to execute
    // the batch file. There are no nested shell metacharacters in this outer
    // command, keeping the descendant relationship deterministic.
    let command = format!("cmd.exe /d /c {}", script.display());
    let manifest = SandboxManifest {
        fs_read_allow: vec![work.clone()],
        fs_write_allow: vec![work.clone()],
        timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    };
    let backend = AppContainerBackend::new();

    {
        let execution = backend.execute(
            &manifest,
            SandboxCommand {
                argv: vec!["cmd.exe".into(), "/d".into(), "/c".into(), command],
                cwd: Some(work.clone()),
            },
        );
        tokio::pin!(execution);

        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    result = &mut execution => {
                        panic!("runaway descendant exited before cancellation: {result:?}");
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        if std::fs::metadata(&heartbeat)
                            .map(|meta| meta.len() > 0)
                            .unwrap_or(false)
                        {
                            break;
                        }
                    }
                }
            }
        })
        .await
        .expect("descendant must begin writing its heartbeat");
    }

    // Allow one in-flight write to settle, then prove the descendant no longer
    // advances. Before shared Job cancellation this file kept growing.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let settled = std::fs::metadata(&heartbeat)
        .expect("heartbeat must remain readable")
        .len();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let final_len = std::fs::metadata(&heartbeat)
        .expect("heartbeat must remain readable")
        .len();
    assert_eq!(
        final_len, settled,
        "descendant heartbeat advanced after execution future drop"
    );
    std::fs::remove_dir_all(&work).expect("cancelled job must release test directory");
}
