//! Live filesystem-ACL verification for R61 (Windows AppContainer DACL grants).
//!
//! Proves, on real Windows hardware through an explicit ignored-test run with
//! `WAYLAND_SANDBOX_LIVE_WINDOWS=1`, that `fs_read_allow`/`fs_write_allow` are
//! actually wired to AppContainer DACLs:
//!   1. WITHOUT a grant, a sandboxed `cmd /c type <file>` is DENIED.
//!   2. WITH `fs_read_allow`, the same command SUCCEEDS and reads the content.
//!   3. AFTER the run, the grant is REVOKED — the AppContainer SID
//!      (`S-1-15-2-…`) / profile name is gone from the file's DACL, so the fix
//!      leaves no permanent grant on the host filesystem.
//!
//! Test files live under `%PUBLIC%` (shallow, AppContainer-traversable ancestor
//! chain; writable without elevation) so the positive read exercises the grant
//! and not an unrelated ancestor-traversal denial.

#![cfg(windows)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

const MARKER: &str = "HEADROOM_R61_GRANT_OK";
const NATIVE_ACCEPTANCE_CASES: usize = 11;

fn require_live_acceptance() {
    assert_eq!(
        std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Ok("1"),
        "native acceptance requires WAYLAND_SANDBOX_LIVE_WINDOWS=1"
    );
    assert!(
        AppContainerBackend::new().is_available(),
        "explicit native acceptance requires an available AppContainer backend"
    );
}

#[test]
#[ignore = "zero-execution guard for explicit native Windows acceptance"]
fn native_acceptance_gate_marker() {
    require_live_acceptance();
    assert_eq!(NATIVE_ACCEPTANCE_CASES, 11);
}

/// Seed a unique test dir under `%PUBLIC%` holding a file containing [`MARKER`].
/// `tag` keeps concurrent tests from colliding even under a shared-process runner.
fn seed_file(tag: &str) -> (PathBuf, PathBuf) {
    let public = std::env::var("PUBLIC").unwrap_or_else(|_| r"C:\Users\Public".into());
    let dir = PathBuf::from(public).join(format!("wcore-r61-{}-{}", std::process::id(), tag));
    std::fs::create_dir_all(&dir).expect("create test dir");
    let file = dir.join("granted.txt");
    std::fs::write(&file, MARKER).expect("write test file");
    (dir, file)
}

/// `icacls <path>` output (unsandboxed), for asserting on the path's DACL.
fn icacls(path: &Path) -> String {
    let out = std::process::Command::new("icacls")
        .arg(path)
        .output()
        .expect("run icacls");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn has_appcontainer_ace(path: &Path) -> bool {
    // Match ONLY the raw AppContainer package-SID prefix `s-1-15-2-`. The former
    // `wcore-` substring disjunct was unconditionally true: `icacls <path>`
    // echoes the seed-dir path (`wcore-r61-<pid>-<tag>`, produced by `seed_file`)
    // in every line of its output, so every absent/revoke check built on this
    // detector was unfalsifiable. Matching only the raw package SID mirrors the
    // belt-and-braces present/absent assertions at lines 240/297/613 (which
    // negate both `S-1-15-2-` and the resolved `wcoresandbox` profile rendering).
    // While a grant is live the ephemeral package SID renders as raw
    // `S-1-15-2-…`, so present-checks still pass.
    let acl = icacls(path).to_ascii_lowercase();
    acl.contains("s-1-15-2-")
}

fn lease_profiles() -> BTreeSet<String> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return BTreeSet::new();
    };
    let directory = PathBuf::from(local)
        .join("Wayland")
        .join("Core")
        .join("AppContainerLeases")
        .join("v1");
    let Ok(entries) = std::fs::read_dir(directory) else {
        return BTreeSet::new();
    };
    entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|value| value.to_str()) == Some("toml"))
                .then(|| path.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect()
}

async fn wait_until(mut predicate: impl FnMut() -> bool, message: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {message}");
}

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

fn type_and_hold(file: &Path, seconds: u8) -> SandboxCommand {
    // `type` proves the granted read; ONLY on its success (`&&`) do we hold the
    // process alive so the grant ACE can be observed PRESENT during the run,
    // then force a deterministic exit 0 with `exit /b 0`. The hold is
    // `waitfor.exe`: it blocks waiting for a signal named `wlhold` that never
    // arrives and times out after `{seconds}`. Unlike `choice.exe` — which
    // exits INSTANTLY (~26ms) under the sandbox's NULL stdin, so the intended
    // present-during-run hold never actually held — `waitfor` is independent of
    // stdin/console/network and is present on the target box. NOT `ping
    // 127.0.0.1`: the AppContainer is built with zero network capability
    // (process.rs:544-545), so loopback is unreachable. `exit /b 0` SETS the
    // script exit code to 0, so the exit code reflects the granted READ
    // succeeding, not `waitfor`'s residual timeout ERRORLEVEL. If `type` is
    // denied, `&&` short-circuits and the script exits with type's non-zero
    // code, so an exit-0 assertion genuinely gates on the granted read.
    cmd_script(format!(
        "type \"{}\" && (waitfor.exe /t {seconds} wlhold >nul 2>&1 & exit /b 0)",
        file.display()
    ))
}

fn echo_temp_and_hold(seconds: u8) -> SandboxCommand {
    // `echo %TEMP%` then the stdin-free `waitfor` hold, then `exit /b 0` to force
    // a deterministic exit 0 on success. `waitfor.exe` blocks on a signal named
    // `wlhold` that never arrives and times out after `{seconds}`. Unlike
    // `choice.exe` — which exits INSTANTLY under the sandbox's NULL stdin so the
    // intended hold never held — `waitfor` is independent of
    // stdin/console/network and is present on the box. NOT `ping 127.0.0.1`: the
    // AppContainer has zero network capability (process.rs:544-545), so loopback
    // is unreachable. `exit /b 0` SETS the exit code to 0 so callers that assert
    // exit 0 see the echo's success, not `waitfor`'s residual timeout ERRORLEVEL.
    cmd_script(format!(
        "echo %TEMP% & waitfor.exe /t {seconds} wlhold >nul 2>&1 & exit /b 0"
    ))
}

fn type_file(file: &Path) -> SandboxCommand {
    SandboxCommand {
        argv: vec![
            "cmd.exe".into(),
            "/c".into(),
            "type".into(),
            file.display().to_string(),
        ],
        cwd: None,
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn ungranted_path_is_denied_in_sandbox() {
    require_live_acceptance();
    let (dir, file) = seed_file("denied");
    let backend = AppContainerBackend::new();
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };
    let out = backend
        .execute(&manifest, type_file(&file))
        .await
        .expect("execute");
    let _ = std::fs::remove_dir_all(&dir);

    assert_ne!(
        out.exit_code,
        0,
        "an ungranted file must be denied inside the sandbox; got exit {} stdout={:?}",
        out.exit_code,
        String::from_utf8_lossy(&out.stdout)
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn granted_path_is_readable_then_revoked() {
    require_live_acceptance();
    let (dir, file) = seed_file("granted");
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![dir.clone()],
        ..Default::default()
    };
    // Hold the granted read alive so the grant ACE can be proven PRESENT on the
    // host DACL *during* the run, then joined for the read result (exit 0 +
    // content), then proven ABSENT after. Asserting both present-during and
    // absent-after makes the revoke genuinely falsifiable: a grant that never
    // applied fails the present-during wait, and a leaked grant fails the
    // absent-after wait — neither can be masked by an unrelated success.
    let read_file = file.clone();
    let read = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&manifest, type_and_hold(&read_file, 3))
            .await
    });
    wait_until(
        || has_appcontainer_ace(&dir),
        "granted read AppContainer ACE present during run",
    )
    .await;

    let out = read
        .await
        .expect("join granted read")
        .expect("granted read");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        out.exit_code,
        0,
        "a granted file must be readable inside the sandbox; stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(MARKER),
        "sandbox must read the granted file's content; got stdout={stdout:?}"
    );
    // The grant must be revoked once the spawn finished — no permanent
    // AppContainer ACE left on the host.
    wait_until(
        || !has_appcontainer_ace(&dir),
        "granted read grant revoked after run",
    )
    .await;
    // Belt-and-braces: the file's own DACL carries no residual AppContainer ACE.
    // icacls renders the package SID either as the raw `S-1-15-2-…` or a resolved
    // name containing the profile moniker.
    let acl_after = icacls(&file);
    let acl_lower = acl_after.to_lowercase();
    assert!(
        !acl_after.contains("S-1-15-2-") && !acl_lower.contains("wcoresandbox"),
        "AppContainer grant must be revoked after the run (no host ACL leak); icacls:\n{acl_after}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Task 4: A secret file under a granted parent directory is unreadable when
/// its path is in `fs_read_deny` (DENY ACE overrides the parent ALLOW grant),
/// and the DENY ACE is revoked after the spawn completes.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn denied_secret_under_granted_parent_is_unreadable_and_revoked() {
    require_live_acceptance();
    let (dir, _) = seed_file("deny-parent");
    // Place the secret inside the granted parent.
    let secret_file = dir.join("secret.env");
    std::fs::write(&secret_file, "SECRET_TOKEN=supersecret").expect("write secret");

    let backend = AppContainerBackend::new();

    // Grant the PARENT directory (so the AppContainer can traverse it) but
    // deny the specific secret file. The DENY ACE overrides the ALLOW.
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![dir.clone()],
        fs_read_deny: vec![secret_file.clone()],
        ..Default::default()
    };
    let out = backend
        .execute(
            &manifest,
            SandboxCommand {
                argv: vec![
                    "cmd.exe".into(),
                    "/c".into(),
                    "type".into(),
                    secret_file.display().to_string(),
                ],
                cwd: None,
            },
        )
        .await
        .expect("execute");

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // Capture DACL while file still exists, before cleanup.
    let acl_after = icacls(&secret_file);
    let _ = std::fs::remove_dir_all(&dir);

    // The secret bytes must not appear in stdout (access denied → empty / error).
    assert!(
        !stdout.contains("SECRET_TOKEN"),
        "secret bytes must not be readable when path is in fs_read_deny; stdout={stdout:?}"
    );
    // The DENY ACE must be revoked after the run — no permanent host ACL leak.
    let acl_lower = acl_after.to_lowercase();
    assert!(
        !acl_after.contains("S-1-15-2-") && !acl_lower.contains("wcoresandbox"),
        "AppContainer DENY ACE must be revoked after the run; icacls:\n{acl_after}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn one_exit_does_not_remove_another_execution_grant() {
    require_live_acceptance();
    let (b_dir, b_file) = seed_file("overlap-b");
    let (a_dir, a_file) = seed_file("overlap-a");
    let b_manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![b_dir.clone()],
        ..Default::default()
    };
    let b = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&b_manifest, type_and_hold(&b_file, 4))
            .await
    });
    wait_until(
        || has_appcontainer_ace(&b_dir),
        "execution B AppContainer ACE",
    )
    .await;

    let a_manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![a_dir.clone()],
        ..Default::default()
    };
    let a_out = AppContainerBackend::new()
        .execute(&a_manifest, type_file(&a_file))
        .await
        .expect("execution A");
    assert_eq!(a_out.exit_code, 0, "execution A must finish cleanly");
    assert!(
        has_appcontainer_ace(&b_dir),
        "execution A cleanup must preserve execution B's distinct ACE"
    );

    let b_out = b.await.expect("join execution B").expect("execution B");
    assert_eq!(b_out.exit_code, 0, "execution B must remain functional");
    wait_until(
        || !has_appcontainer_ace(&b_dir),
        "execution B exact-SID cleanup",
    )
    .await;
    let _ = std::fs::remove_dir_all(a_dir);
    let _ = std::fs::remove_dir_all(b_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn one_execution_grant_never_leaks_to_another_identity() {
    require_live_acceptance();
    let (dir, file) = seed_file("no-cross-grant");
    let a_manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![dir.clone()],
        ..Default::default()
    };
    let a_file = file.clone();
    let a = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&a_manifest, type_and_hold(&a_file, 3))
            .await
    });
    wait_until(|| has_appcontainer_ace(&dir), "execution A grant").await;

    let b_out = AppContainerBackend::new()
        .execute(
            &SandboxManifest {
                timeout: Some(Duration::from_secs(10)),
                ..Default::default()
            },
            type_file(&file),
        )
        .await
        .expect("execution B");
    assert_ne!(b_out.exit_code, 0, "B must not inherit A's grant");
    assert!(
        !String::from_utf8_lossy(&b_out.stdout).contains(MARKER),
        "B must not read bytes granted only to A"
    );
    // A's script exit code now reflects the granted READ succeeding (exit 0 via
    // `type && … & exit /b 0`), not the `waitfor` hold's residual timeout
    // ERRORLEVEL. Gate on that read: A must exit 0 AND have actually read the
    // granted bytes.
    let a_out = a.await.expect("join execution A").expect("execution A");
    assert_eq!(
        a_out.exit_code, 0,
        "the granting identity's read must succeed (exit 0), not the hold primitive's residual exit code"
    );
    assert!(
        String::from_utf8_lossy(&a_out.stdout).contains(MARKER),
        "the granting identity must actually read the granted bytes"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn concurrent_allow_and_deny_identities_do_not_interfere() {
    require_live_acceptance();
    let (dir, _) = seed_file("allow-deny");
    let secret = dir.join("secret.txt");
    std::fs::write(&secret, MARKER).expect("write secret");
    let allow_manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![dir.clone()],
        ..Default::default()
    };
    let deny_manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![dir.clone()],
        fs_read_deny: vec![secret.clone()],
        ..Default::default()
    };
    let allow_file = secret.clone();
    let allow = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&allow_manifest, type_and_hold(&allow_file, 2))
            .await
    });
    let deny_file = secret.clone();
    let deny = tokio::spawn(async move {
        AppContainerBackend::new()
            .execute(&deny_manifest, type_file(&deny_file))
            .await
    });
    let allow_out = allow.await.expect("join allow").expect("allow execution");
    let deny_out = deny.await.expect("join deny").expect("deny execution");
    assert!(
        String::from_utf8_lossy(&allow_out.stdout).contains(MARKER),
        "ordinary allow identity must retain access"
    );
    assert!(
        !String::from_utf8_lossy(&deny_out.stdout).contains(MARKER),
        "delegated deny identity must not read the secret"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn twenty_concurrent_executions_have_unique_temp_roots() {
    require_live_acceptance();
    let baseline = lease_profiles();
    let mut tasks = Vec::new();
    for _ in 0..20 {
        tasks.push(tokio::spawn(async {
            AppContainerBackend::new()
                .execute(
                    &SandboxManifest {
                        timeout: Some(Duration::from_secs(10)),
                        ..Default::default()
                    },
                    echo_temp_and_hold(2),
                )
                .await
        }));
    }
    wait_until(
        || lease_profiles().difference(&baseline).count() == 20,
        "20 concurrent durable leases",
    )
    .await;
    let live_profiles: BTreeSet<_> = lease_profiles().difference(&baseline).cloned().collect();
    assert_eq!(live_profiles.len(), 20, "every live command needs one SID");
    assert!(
        live_profiles.iter().all(|name| name.len() <= 64),
        "all AppContainer profile names must satisfy the Win32 limit"
    );

    let mut temp_roots = BTreeSet::new();
    for task in tasks {
        let output = task.await.expect("join command").expect("execute command");
        assert_eq!(output.exit_code, 0);
        let temp = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        assert!(temp.to_ascii_lowercase().contains("\\packages\\wcore-"));
        temp_roots.insert(temp);
    }
    assert_eq!(temp_roots.len(), 20, "TEMP must be unique per execution");
    wait_until(
        || lease_profiles().is_subset(&baseline),
        "all concurrent leases to be removed",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn timeout_and_cancellation_remove_their_leases() {
    require_live_acceptance();
    let baseline = lease_profiles();
    let timeout = AppContainerBackend::new()
        .execute(
            &SandboxManifest {
                timeout: Some(Duration::from_millis(150)),
                ..Default::default()
            },
            echo_temp_and_hold(8),
        )
        .await;
    assert!(timeout.is_err(), "timeout path must return an error");
    wait_until(
        || lease_profiles().is_subset(&baseline),
        "timeout lease cleanup",
    )
    .await;

    let task = tokio::spawn(async {
        AppContainerBackend::new()
            .execute(
                &SandboxManifest {
                    timeout: Some(Duration::from_secs(8)),
                    ..Default::default()
                },
                echo_temp_and_hold(8),
            )
            .await
    });
    wait_until(
        || !lease_profiles().is_subset(&baseline),
        "cancellable execution lease",
    )
    .await;
    task.abort();
    let _ = task.await;
    wait_until(
        || lease_profiles().is_subset(&baseline),
        "cancelled execution lease cleanup",
    )
    .await;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn unrelated_acl_survives_exact_sid_cleanup() {
    require_live_acceptance();
    let (dir, file) = seed_file("unrelated-ace");
    let grant = std::process::Command::new("icacls")
        .arg(&file)
        .args(["/grant", "*S-1-1-0:(R)"])
        .output()
        .expect("grant Everyone read ACE");
    assert!(grant.status.success(), "seed unrelated ACE: {grant:?}");
    let output = AppContainerBackend::new()
        .execute(
            &SandboxManifest {
                timeout: Some(Duration::from_secs(10)),
                fs_read_allow: vec![dir.clone()],
                ..Default::default()
            },
            type_file(&file),
        )
        .await
        .expect("sandbox read");
    assert_eq!(output.exit_code, 0);
    let acl = icacls(&file).to_ascii_lowercase();
    assert!(
        acl.contains("everyone") || acl.contains("s-1-1-0"),
        "exact-SID cleanup must preserve unrelated trustees: {acl}"
    );
    assert!(!has_appcontainer_ace(&file));
    let _ = std::fs::remove_dir_all(dir);
}

/// Isolation proof (REQ-native-r2 / CONTEXT D4): an explicit DENY ace still
/// blocks the sandboxed child even when a matching package-SID grant is ALSO
/// present. Windows evaluates DENY aces before ALLOW, so the DENY must win.
///
/// Falsifiable: the file's directory is `fs_read_allow`-granted (so absent the
/// DENY the child WOULD read it, as `granted_path_is_readable_then_revoked`
/// proves), and the file itself is `fs_read_deny`-denied. If dropping the
/// deny-only SIDs in 20-19 had weakened the boundary so a DENY could be
/// bypassed, this read would succeed (exit 0, MARKER present) and the test
/// would FAIL.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn deny_ace_still_blocks_granted_read() {
    require_live_acceptance();
    let (dir, file) = seed_file("deny-wins");
    let backend = AppContainerBackend::new();
    // Same target: a package-SID ALLOW grant (via the granted directory) PLUS an
    // explicit package-SID DENY on the file. The DENY must override the grant.
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![dir.clone()],
        fs_read_deny: vec![file.clone()],
        ..Default::default()
    };
    let out = backend
        .execute(&manifest, type_file(&file))
        .await
        .expect("execute");

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    // Capture the DACL while the file still exists, before cleanup.
    let acl_after = icacls(&file);
    let _ = std::fs::remove_dir_all(&dir);

    assert_ne!(
        out.exit_code, 0,
        "an explicit DENY ace must block the read even with a matching grant; \
         got exit 0 stdout={stdout:?}"
    );
    assert!(
        !stdout.contains(MARKER),
        "a DENY-blocked read must not disclose the file's bytes; stdout={stdout:?}"
    );
    // Both the ALLOW grant and the DENY ace are revoked after the run — no
    // permanent AppContainer ACE (grant or deny) left on the host.
    let acl_lower = acl_after.to_lowercase();
    assert!(
        !acl_after.contains("S-1-15-2-") && !acl_lower.contains("wcoresandbox"),
        "AppContainer grant/deny aces must be revoked after the run; icacls:\n{acl_after}"
    );
}

/// Isolation proof (REQ-native-r2 / CONTEXT D4): a file granted ONLY to a
/// normal SID (`Everyone` / `S-1-1-0`), with NO AppContainer package-SID grant
/// (no `fs_read_allow`), is STILL denied to the sandboxed child.
///
/// This is the load-bearing proof that dropping the deny-only SIDs in 20-19 did
/// not weaken the sandbox: isolation is intrinsic to the AppContainer package-SID
/// access model, which ignores normal SIDs for granting. Falsifiable: if the
/// child could use a normal-SID grant, this read would succeed (exit 0, MARKER)
/// and the test would FAIL — the exact regression the deny-only marking was
/// (redundantly) thought to guard against.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn normal_sid_only_grant_is_denied() {
    require_live_acceptance();
    let (dir, file) = seed_file("normal-sid-only");
    // Grant the file to a NORMAL sid only (Everyone / S-1-1-0). No package-SID
    // grant is issued (no fs_read_allow), so the ONLY ACE the child could try to
    // use is the normal-SID one the AppContainer model must ignore.
    let grant = std::process::Command::new("icacls")
        .arg(&file)
        .args(["/grant", "*S-1-1-0:(R)"])
        .output()
        .expect("grant Everyone read ACE");
    assert!(grant.status.success(), "seed normal-SID grant: {grant:?}");

    let backend = AppContainerBackend::new();
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        // Deliberately NO fs_read_allow: prove denial is intrinsic, not a
        // side effect of withholding a package-SID grant on a shared dir.
        ..Default::default()
    };
    let out = backend
        .execute(&manifest, type_file(&file))
        .await
        .expect("execute");

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&dir);

    assert_ne!(
        out.exit_code, 0,
        "a file granted only to a normal SID must stay denied to the AppContainer \
         child; got exit 0 stdout={stdout:?}"
    );
    assert!(
        !stdout.contains(MARKER),
        "a normal-SID-only grant must not disclose bytes to the sandboxed child; \
         stdout={stdout:?}"
    );
}
