//! Live check for the relaxed Windows posture (trusted_local): with
//! WAYLAND_WINDOWS_RELAXED_SANDBOX=1 the sandbox runs msys (ls/pwd), git init,
//! npm, and PowerShell — the toolchain the AppContainer blocks — while still
//! going through the restricted-token + Job Object path.
//!
//! Gated behind WAYLAND_SANDBOX_LIVE_WINDOWS (like the other live tests).

#![cfg(windows)]

use std::time::Duration;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

fn live() -> bool {
    std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").is_ok()
}

async fn run(ws: &std::path::Path, argv: &[&str]) -> (i32, String) {
    let backend = AppContainerBackend::new();
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(20)),
        fs_read_allow: vec![ws.to_path_buf()],
        fs_write_allow: vec![ws.to_path_buf()],
        ..Default::default()
    };
    let cmd = SandboxCommand {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        cwd: Some(ws.to_path_buf()),
    };
    let out = backend.execute(&manifest, cmd).await.expect("spawn");
    (out.exit_code, String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn relaxed_posture_runs_the_toolchain() {
    if !live() {
        return;
    }
    // SAFETY: single-threaded test intent; set the posture for this process.
    unsafe { std::env::set_var("WAYLAND_WINDOWS_RELAXED_SANDBOX", "1") };

    let ws = std::env::temp_dir().join(format!("wcore-relaxed-{}", std::process::id()));
    std::fs::create_dir_all(&ws).unwrap();
    let ws = std::fs::canonicalize(&ws).unwrap();

    // msys tools (git-bash) — blocked by AppContainer, must work here.
    let (ls_code, ls_out) = run(&ws, &["cmd", "/C", "ls", "-la"]).await;
    assert_eq!(ls_code, 0, "msys ls must run under relaxed posture; out={ls_out:?}");

    let (pwd_code, _) = run(&ws, &["cmd", "/C", "pwd"]).await;
    assert_eq!(pwd_code, 0, "msys pwd must run");

    // git init — real repo op that failed under AppContainer.
    let (gi_code, gi_out) = run(&ws, &["cmd", "/C", "git", "init"]).await;
    assert_eq!(gi_code, 0, "git init must run; out={gi_out:?}");
    assert!(gi_out.to_lowercase().contains("initialized"), "git init output: {gi_out:?}");

    // npm — node script, failed under AppContainer.
    let (npm_code, npm_out) = run(&ws, &["cmd", "/C", "npm", "--version"]).await;
    assert_eq!(npm_code, 0, "npm must run; out={npm_out:?}");

    // PowerShell — bare shell, downgraded/blocked under AppContainer.
    let (ps_code, ps_out) = run(
        &ws,
        &["powershell", "-NoProfile", "-Command", "Write-Output PS_RELAXED_OK"],
    )
    .await;
    assert_eq!(ps_code, 0, "powershell must run; out={ps_out:?}");
    assert!(ps_out.contains("PS_RELAXED_OK"), "powershell output: {ps_out:?}");

    // Native tools still work.
    let (git_code, _) = run(&ws, &["cmd", "/C", "git", "--version"]).await;
    assert_eq!(git_code, 0, "git --version must run");

    eprintln!("relaxed posture: ls/pwd/git-init/npm/powershell all ran");
    let _ = std::fs::remove_dir_all(&ws);
}
