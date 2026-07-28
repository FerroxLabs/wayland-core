//! TEMPORARY diagnostic for F-WR-01. Not an acceptance test.
//!
//! `live_future_drop_reaps_descendant_job_tree` aborts with the sandboxed
//! command exiting 1 on `Access is denied.`, before any descendant exists, so
//! the reap assertion it was written for is never reached. This binary isolates
//! WHICH element of that test's setup is denied, by running the same manifest
//! against a ladder of commands that differ in exactly one property each.
//!
//! Every probe reports exit code, stdout and stderr. Nothing is asserted; the
//! point is to read the ladder and find the first rung that fails.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::Duration;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

fn live() -> bool {
    std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref() == Ok("1")
}

fn work_dir(tag: &str) -> PathBuf {
    let public = std::env::var("PUBLIC").unwrap_or_else(|_| r"C:\Users\Public".into());
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("epoch")
        .as_nanos();
    let dir = PathBuf::from(public).join(format!(
        "wcore-kr01probe-{}-{tag}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create probe dir");
    dir
}

fn manifest(work: &Path) -> SandboxManifest {
    SandboxManifest {
        fs_read_allow: vec![work.to_path_buf()],
        fs_write_allow: vec![work.to_path_buf()],
        timeout: Some(Duration::from_secs(20)),
        ..Default::default()
    }
}

async fn probe(name: &str, work: &Path, cwd: Option<PathBuf>, argv: Vec<String>) {
    let backend = AppContainerBackend::new();
    let m = manifest(work);
    println!("PROBE {name}");
    println!("  argv = {argv:?}");
    println!("  cwd  = {cwd:?}");
    let started = std::time::Instant::now();
    let result = backend.execute(&m, SandboxCommand { argv, cwd }).await;
    let elapsed = started.elapsed().as_millis();
    match result {
        Ok(out) => {
            println!("  RESULT exit_code={} ms={elapsed}", out.exit_code);
            println!("  stdout={:?}", String::from_utf8_lossy(&out.stdout));
            println!("  stderr={:?}", String::from_utf8_lossy(&out.stderr));
            println!(
                "  VERDICT {name} = {}",
                if out.exit_code == 0 { "OK" } else { "NONZERO" }
            );
        }
        Err(e) => {
            println!("  RESULT err={e:?} ms={elapsed}");
            println!("  VERDICT {name} = ERR");
        }
    }
}

fn s(v: &str) -> String {
    v.to_string()
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "temporary F-WR-01 diagnostic; explicit native Windows only"]
async fn kr01_denial_ladder() {
    if !live() {
        println!("KR01PROBE_SKIPPED (WAYLAND_SANDBOX_LIVE_WINDOWS != 1)");
        return;
    }
    println!("KR01PROBE_BEGIN");

    let work = work_dir("ladder");
    let script = work.join("heartbeat.cmd");
    let target = work.join("heartbeat.txt");
    // Byte-for-byte the script live_integrity.rs writes.
    std::fs::write(
        &script,
        format!(
            "@echo off\r\n:loop\r\necho x>>{}\r\nchoice.exe /t 1 /d y /n >nul\r\ngoto loop\r\n",
            target.display()
        ),
    )
    .expect("write script");
    // A bounded variant that uses the cmd BUILTIN hold primitive instead of
    // choice.exe, which live_fs_acl.rs records as non-functional under this
    // sandbox (console/DLL deps fail to load under the Low-IL restricted token).
    let builtin_script = work.join("heartbeat_builtin.cmd");
    std::fs::write(
        &builtin_script,
        format!(
            "@echo off\r\n:loop\r\necho x>>{}\r\nfor /L %%i in (1,1,4000000) do @rem\r\ngoto loop\r\n",
            target.display()
        ),
    )
    .expect("write builtin script");

    println!("WORK={}", work.display());
    println!("SCRIPT={}", script.display());

    // Rung 1: can the sandbox READ the script out of the granted directory?
    probe(
        "1-read-script",
        &work,
        None,
        vec![
            s("cmd.exe"),
            s("/d"),
            s("/s"),
            s("/c"),
            format!("type \"{}\"", script.display()),
        ],
    )
    .await;

    // Rung 2: can it WRITE into the granted directory?
    probe(
        "2-write-into-workdir",
        &work,
        None,
        vec![
            s("cmd.exe"),
            s("/d"),
            s("/s"),
            s("/c"),
            format!("echo probe>>\"{}\"", work.join("probe_write.txt").display()),
        ],
    )
    .await;

    // Rung 3: cwd set to the granted directory, trivial builtin. Isolates cwd
    // from everything else -- no other passing live test sets cwd at all.
    probe(
        "3-cwd-only",
        &work,
        Some(work.clone()),
        vec![s("cmd.exe"), s("/d"), s("/s"), s("/c"), s("echo cwd-ok")],
    )
    .await;

    // Rung 4: EXECUTE the script directly, no nesting, no cwd.
    probe(
        "4-exec-script-nocwd",
        &work,
        None,
        vec![
            s("cmd.exe"),
            s("/d"),
            s("/s"),
            s("/c"),
            format!("\"{}\"", script.display()),
        ],
    )
    .await;

    // Rung 5: the EXACT shape live_integrity.rs uses -- nested cmd, cwd set,
    // no /s, unquoted path.
    probe(
        "5-exact-live-integrity-shape",
        &work,
        Some(work.clone()),
        vec![
            s("cmd.exe"),
            s("/d"),
            s("/c"),
            format!("cmd.exe /d /c {}", script.display()),
        ],
    )
    .await;

    // Rung 6: same nested shape but WITHOUT cwd, to attribute rung 5's result.
    probe(
        "6-nested-nocwd",
        &work,
        None,
        vec![
            s("cmd.exe"),
            s("/d"),
            s("/c"),
            format!("cmd.exe /d /c {}", script.display()),
        ],
    )
    .await;

    // Rung 7: does choice.exe run under this sandbox at all? live_fs_acl.rs
    // records that it does not. If so, the heartbeat loop's sleep primitive is
    // unusable and the script is a busy spin even when it does execute.
    probe(
        "7-choice-exe",
        &work,
        None,
        vec![
            s("cmd.exe"),
            s("/d"),
            s("/s"),
            s("/c"),
            s("choice.exe /t 1 /d y /n"),
        ],
    )
    .await;

    // Rung 8: the builtin-hold variant of the heartbeat, nested + cwd.
    probe(
        "8-builtin-heartbeat-nested",
        &work,
        Some(work.clone()),
        vec![
            s("cmd.exe"),
            s("/d"),
            s("/c"),
            format!("cmd.exe /d /c {}", builtin_script.display()),
        ],
    )
    .await;

    println!(
        "HEARTBEAT_EXISTS={} len={:?}",
        target.exists(),
        std::fs::metadata(&target).map(|m| m.len()).ok()
    );
    println!("KR01PROBE_END");
    let _ = std::fs::remove_dir_all(&work);
}
