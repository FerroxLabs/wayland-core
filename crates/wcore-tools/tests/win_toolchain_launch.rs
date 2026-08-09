//! Windows stage 2 — can the DEFAULT sandbox posture actually launch an
//! external toolchain?
//!
//! The product's default for an untrusted workspace is
//! [`WorkspacePolicy::contained`], whose read grant set is the workspace root,
//! the scratch dirs, and `minimal_toolchain_read_dirs()` (`~/.rustup`,
//! `~/.cargo/bin`). Under that posture the measured behaviour on SeanDesktop
//! was: `cmd` builtins run, and every external toolchain fails —
//! git / cargo / rustc with `0xC0000142 STATUS_DLL_INIT_FAILED`, node / python
//! with "not recognized" because their install directories are in no grant set.
//!
//! This binary is the measurement. It drives the REAL product surface —
//! `BashTool::execute_with_ctx` with a `ToolContext` carrying the contained
//! policy and the real `AppContainerBackend` — and grades from world state
//! (did the file appear on disk) as well as exit code.
//!
//! Gating follows `wcore-sandbox/tests/live_cwd_verbatim.rs`: `#![cfg(windows)]`,
//! `#[ignore]`, and an assert (not an early return) on
//! `WAYLAND_SANDBOX_LIVE_WINDOWS=1`, so an unset variable fails rather than
//! silently passing.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use wcore_sandbox::SandboxRegistry;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

fn require_live() {
    assert_eq!(
        std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Ok("1"),
        "native toolchain acceptance requires WAYLAND_SANDBOX_LIVE_WINDOWS=1"
    );
    assert!(
        AppContainerBackend::new().is_available(),
        "native toolchain acceptance requires an available AppContainer backend"
    );
}

/// Quietness guard. A SECOND wayland-core-family process makes every sandboxed
/// call fail on the machine-wide 15 s ACL mutex, which voided an entire earlier
/// measurement. This binary is itself the only process that should hold it.
fn assert_quiet() {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq wayland-core.exe", "/NH"])
        .output()
        .expect("tasklist");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.to_ascii_lowercase().contains("wayland-core.exe"),
        "a wayland-core process is running; the machine-wide AppContainer ACL \
         mutex makes every measurement void. tasklist said: {text}"
    );
}

struct Probe {
    exit_code: i32,
    stdout: String,
    stderr: String,
    raw: String,
}

fn run(ctx: &ToolContext, command: &str) -> Probe {
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt")
        .block_on(BashTool.execute_with_ctx(json!({ "command": command }), ctx));
    parse(&result.content)
}

fn parse(content: &str) -> Probe {
    // `output_to_result` renders "Exit code: N\nSTDOUT:\n…\nSTDERR:\n…".
    // A refusal or an execution failure has no such prefix; encode that as
    // a distinguishable sentinel rather than pretending it is an exit code.
    let Some(rest) = content.strip_prefix("Exit code: ") else {
        return Probe {
            exit_code: i32::MIN,
            stdout: String::new(),
            stderr: String::new(),
            raw: content.to_owned(),
        };
    };
    let (code, rest) = rest.split_once('\n').unwrap_or((rest, ""));
    let exit_code = code.trim().parse::<i32>().unwrap_or(i32::MIN);
    let body = rest.strip_prefix("STDOUT:\n").unwrap_or(rest);
    let (stdout, stderr) = body.split_once("\nSTDERR:\n").unwrap_or((body, ""));
    Probe {
        exit_code,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        raw: content.to_owned(),
    }
}

/// A workspace on a real disk (NOT `%TEMP%`), because the grant set and the
/// verbatim-prefix path both behave differently for a canonicalized root.
fn workspace(tag: &str) -> PathBuf {
    let base = std::env::var_os("WAYLAND_WIN_LANE_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("wayland-win-launch"));
    let dir = base.join(format!("{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("workspace");
    std::fs::canonicalize(&dir).expect("canonicalize workspace")
}

fn contained_ctx(root: &Path) -> (Arc<WorkspacePolicy>, ToolContext) {
    let policy = Arc::new(WorkspacePolicy::contained(root));
    let ctx = ToolContext::test_default()
        .with_workspace(Arc::clone(&policy))
        .with_sandbox(Arc::new(SandboxRegistry::new(Arc::new(
            AppContainerBackend::new(),
        ))));
    (policy, ctx)
}

/// The positive control: a `cmd` builtin under the default posture. Every other
/// case in this file is void if this one fails, so it runs first and is also
/// re-run by each acceptance case.
fn control(ctx: &ToolContext, root: &Path, tag: &str) {
    let marker = format!("control-{tag}.txt");
    let p = run(ctx, &format!("echo hello> {marker}"));
    assert_eq!(
        p.exit_code, 0,
        "POSITIVE CONTROL FAILED at {tag} — every measurement after the last \
         passing control is VOID. raw: {}",
        p.raw
    );
    let landed = root.join(&marker);
    assert!(
        landed.is_file(),
        "control claimed exit 0 but {} is not on disk",
        landed.display()
    );
}

/// THE MEASUREMENT. Prints one row per toolchain and asserts nothing about the
/// toolchains themselves — it exists to produce the table, with the control
/// asserted before, between and after so a wedge point is visible.
#[test]
#[ignore = "explicit native Windows toolchain measurement"]
fn measure_default_posture_toolchain_launch() {
    require_live();
    assert_quiet();
    let root = workspace("measure");
    let (policy, ctx) = contained_ctx(&root);

    println!("WORKSPACE {}", root.display());
    println!("TRUST {:?}", policy.trust());
    for r in policy.readable_roots() {
        println!("READ-GRANT {}", r.display());
    }
    for w in policy.writable_roots() {
        println!("WRITE-GRANT {}", w.display());
    }

    control(&ctx, &root, "start");

    for (name, command) in [
        ("where-git", "where git"),
        ("git", "git --version"),
        ("where-node", "where node"),
        ("node", "node --version"),
        ("where-python", "where python"),
        ("python", "python --version"),
        ("where-cargo", "where cargo"),
        ("cargo", "cargo --version"),
        ("where-rustc", "where rustc"),
        ("rustc", "rustc --version"),
    ] {
        let p = run(&ctx, command);
        println!(
            "ROW name={name} cmd={command:?} exit={} stdout={:?} stderr={:?}",
            p.exit_code,
            p.stdout.trim(),
            p.stderr.trim()
        );
        control(&ctx, &root, name);
    }

    control(&ctx, &root, "end");
    let _ = std::fs::remove_dir_all(&root);
}
