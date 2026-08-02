//! TEMPORARY Windows diagnostic for the `typed_bypass_executes_bash_inside_required_sandbox`
//! red: the sandboxed child is reported as exiting 0 with EMPTY stdout on the
//! wcore-agent bootstrap path while the byte-identical command through
//! `wayland-core sandbox exec` returns its text.
//!
//! Both paths call `BashTool::execute_with_ctx`, so the difference is in the
//! inputs, not the function. This test takes the SAME command down four
//! progressively-more-wrapped layers and prints every observation, so the layer
//! where the bytes disappear is identified rather than guessed:
//!
//!   L0  the sandbox backend directly, no policy, no cwd
//!   L1  the sandbox backend directly, cwd = workspace
//!   L2  BashTool + `WorkspacePolicy::contained(canonicalized)`   (= `sandbox exec`)
//!   L3  BashTool + `WorkspacePolicy::contained(raw tempdir path)` (= bootstrap)
//!
//! `#[ignore]` so it never runs in the normal suite; the diagnostic workflow
//! runs it with `--ignored --nocapture`.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use wcore_sandbox::{SandboxCommand, SandboxManifest, SandboxRegistry};
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

const NEEDLE: &str = "typed-bypass-bash-succeeded";
const COMMAND: &str = "echo typed-bypass-bash-succeeded";

fn banner(label: &str) {
    println!("\n===== {label} =====");
}

fn report_env() {
    banner("HOST ENVIRONMENT");
    for key in [
        "USERNAME",
        "USERPROFILE",
        "LOCALAPPDATA",
        "APPDATA",
        "TEMP",
        "TMP",
        "COMSPEC",
        "SESSIONNAME",
        "WAYLAND_BASH_SHELL",
        "WAYLAND_SANDBOX",
    ] {
        println!("  {key} = {:?}", std::env::var(key).ok());
    }
    println!("  std::env::temp_dir() = {:?}", std::env::temp_dir());
    println!("  current_dir = {:?}", std::env::current_dir().ok());
}

fn report_policy(label: &str, policy: &WorkspacePolicy) {
    banner(&format!("POLICY {label}"));
    println!("  root            = {:?}", policy.root());
    println!("  trust           = {:?}", policy.trust());
    println!("  network         = {:?}", policy.network());
    println!("  writable_roots  = {:?}", policy.writable_roots());
    println!("  readable_roots  = {:?}", policy.readable_roots());
    println!("  cache_env       = {:?}", policy.cache_env());
}

async fn run_backend_direct(label: &str, cwd: Option<std::path::PathBuf>) {
    banner(&format!("L{label} BACKEND DIRECT cwd={cwd:?}"));
    let backend = wcore_sandbox::default_for_platform();
    println!("  backend.name()               = {}", backend.name());
    println!(
        "  backend.is_available()       = {}",
        backend.is_available()
    );
    let manifest = SandboxManifest::default();
    let argv = if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), COMMAND.into()]
    } else {
        vec!["/bin/sh".to_string(), "-c".into(), COMMAND.into()]
    };
    let cmd = SandboxCommand { argv, cwd };
    let started = std::time::Instant::now();
    match backend.execute(&manifest, cmd).await {
        Ok(out) => {
            println!("  elapsed    = {:?}", started.elapsed());
            println!("  exit_code  = {}", out.exit_code);
            println!("  stdout     = {:?}", String::from_utf8_lossy(&out.stdout));
            println!("  stderr     = {:?}", String::from_utf8_lossy(&out.stderr));
            println!(
                "  VERDICT    = {}",
                if String::from_utf8_lossy(&out.stdout).contains(NEEDLE) {
                    "NEEDLE PRESENT"
                } else {
                    "NEEDLE MISSING"
                }
            );
        }
        Err(error) => {
            println!("  elapsed    = {:?}", started.elapsed());
            println!("  ERROR      = {error}");
            println!("  VERDICT    = NEEDLE MISSING (execute returned Err)");
        }
    }
}

async fn run_bash_tool(label: &str, workspace: &std::path::Path) {
    banner(&format!("L{label} BASHTOOL workspace={workspace:?}"));
    let registry = match SandboxRegistry::required_for_session(None) {
        Ok(registry) => Arc::new(registry),
        Err(error) => {
            println!("  required_for_session ERROR = {error}");
            return;
        }
    };
    println!(
        "  registry.backend_name()      = {}",
        registry.backend_name()
    );
    println!(
        "  registry.is_available()      = {}",
        registry.is_available()
    );
    println!(
        "  registry.bypasses_containment= {}",
        registry.bypasses_containment()
    );
    println!("  registry.env_passthrough()   = {:?}", {
        let mut names: Vec<_> = registry.env_passthrough().iter().cloned().collect();
        names.sort();
        names
    });

    let policy = Arc::new(WorkspacePolicy::contained(workspace));
    report_policy(label, &policy);

    let ctx = ToolContext::new(
        "win-bash-path-diag",
        CancellationToken::new(),
        Arc::new(wcore_tools::vfs::RealFs),
        None,
        Arc::new(wcore_tools::NullToolOutputSink),
    )
    .with_workspace(policy)
    .with_sandbox(registry);

    let started = std::time::Instant::now();
    let result = BashTool
        .execute_with_ctx(serde_json::json!({ "command": COMMAND }), &ctx)
        .await;
    println!("  elapsed   = {:?}", started.elapsed());
    println!("  is_error  = {}", result.is_error);
    println!("  content   = {:?}", result.content);
    println!(
        "  VERDICT   = {}",
        if result.content.contains(NEEDLE) {
            "NEEDLE PRESENT"
        } else {
            "NEEDLE MISSING"
        }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Windows diagnostic; run explicitly with --ignored --nocapture"]
async fn windows_bash_path_differential() {
    report_env();

    let workspace = tempfile::tempdir().expect("workspace");
    let raw = workspace.path().to_path_buf();
    let canonical = std::fs::canonicalize(&raw).unwrap_or_else(|_| raw.clone());
    println!("\nraw workspace       = {raw:?}");
    println!("canonical workspace = {canonical:?}");

    run_backend_direct("0", None).await;
    run_backend_direct("1", Some(raw.clone())).await;
    run_bash_tool("2-canonical", &canonical).await;
    run_bash_tool("3-raw", &raw).await;
}
