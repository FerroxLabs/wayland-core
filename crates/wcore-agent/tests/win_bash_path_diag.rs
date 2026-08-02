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

// ---------------------------------------------------------------------------
// Second bisect: the four layers above are GREEN on a real Windows host while
// the engine path is RED on the same host, in the same test binary shape. So
// the loss is downstream of the tool context. This walks the engine path in
// the SAME single-threaded runtime flavour the failing test uses
// (`#[tokio::test]` defaults to current_thread), printing:
//
//   L4  BashTool with a hand-built context, BEFORE any bootstrap runs
//   ..  the sandbox-relevant host env and shell prefix, before and after
//   L5  BashTool with the ENGINE'S OWN registry policy + sandbox runtime
//   L6  the engine dispatching the same Bash call end to end
// ---------------------------------------------------------------------------

mod common;

use common::{
    MockLlmProvider, RECOVERY_TEST_KEY, configure_persisted_test_session, physical_attempt_server,
};
use std::sync::Mutex;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_types::execution_policy::{ApprovalPolicy, PolicySource};
use wcore_types::message::FinishReason;

#[derive(Default)]
struct DiagSink {
    tool_results: Mutex<Vec<(String, bool, String)>>,
}

impl OutputSink for DiagSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        self.tool_results
            .lock()
            .unwrap()
            .push((name.to_string(), is_error, content.to_string()));
    }
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool) {}
    fn emit_info(&self, _: &str) {}
}

/// Every host variable the sandbox env builder can forward, so a bootstrap that
/// mutates one is visible rather than inferred.
fn sandbox_relevant_env() -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(name, _)| {
            [
                "PATH",
                "SYSTEMROOT",
                "WINDIR",
                "COMSPEC",
                "TEMP",
                "TMP",
                "TMPDIR",
                "HOME",
                "USERPROFILE",
                "LOCALAPPDATA",
                "APPDATA",
                "PATHEXT",
                "WAYLAND_HOME",
                "WAYLAND_BASH_SHELL",
                "WAYLAND_SANDBOX",
            ]
            .iter()
            .any(|k| k.eq_ignore_ascii_case(name))
        })
        .collect();
    vars.sort();
    vars
}

async fn run_bash_with_registry(
    label: &str,
    policy: Arc<WorkspacePolicy>,
    sandbox: Arc<SandboxRegistry>,
) {
    banner(&format!("L{label} BASHTOOL via supplied registry"));
    println!("  backend        = {}", sandbox.backend_name());
    println!("  policy.root    = {:?}", policy.root());
    println!("  policy.trust   = {:?}", policy.trust());
    println!("  policy.network = {:?}", policy.network());
    println!("  writable_roots = {:?}", policy.writable_roots());
    let ctx = ToolContext::new(
        "win-bash-path-diag",
        CancellationToken::new(),
        Arc::new(wcore_tools::vfs::RealFs),
        None,
        Arc::new(wcore_tools::NullToolOutputSink),
    )
    .with_workspace(policy)
    .with_sandbox(sandbox);
    let started = std::time::Instant::now();
    let result = BashTool
        .execute_with_ctx(serde_json::json!({ "command": COMMAND }), &ctx)
        .await;
    println!("  elapsed  = {:?}", started.elapsed());
    println!("  content  = {:?}", result.content);
    println!(
        "  VERDICT  = {}",
        if result.content.contains(NEEDLE) {
            "NEEDLE PRESENT"
        } else {
            "NEEDLE MISSING"
        }
    );
}

fn diag_config() -> Config {
    let mut config = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 64,
        max_turns: Some(2),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    config.tools.allow_list.clear();
    config
}

#[tokio::test]
#[ignore = "Windows diagnostic; run explicitly with --ignored --nocapture"]
async fn windows_engine_path_bisect() {
    let workspace = tempfile::tempdir().expect("workspace");

    banner("BEFORE BOOTSTRAP");
    println!(
        "  bash_shell_argv_prefix = {:?}",
        wcore_config::shell::bash_shell_argv_prefix()
    );
    for (key, value) in sandbox_relevant_env() {
        println!("  env {key} = {value:?}");
    }

    // L4 — the control, in THIS runtime flavour, before any bootstrap state.
    let control_policy = Arc::new(WorkspacePolicy::contained(workspace.path()));
    let control_sandbox = Arc::new(
        SandboxRegistry::required_for_session(None).expect("required_for_session must resolve"),
    );
    run_bash_with_registry("4-precontrol", control_policy, control_sandbox).await;

    let physical = physical_attempt_server().await;
    let mut config = diag_config();
    config.tools.auto_approve = false;
    configure_persisted_test_session(&mut config, workspace.path());
    let capture = Arc::new(DiagSink::default());
    let sink: Arc<dyn OutputSink> = capture.clone();
    let mut result = AgentBootstrap::new(config, workspace.path().to_string_lossy(), sink)
        .with_smart_execution_policy(ApprovalPolicy::Bypass, PolicySource::LocalCliLaunch)
        .provider(Arc::new(
            MockLlmProvider::with_tool_use(
                "typed-bash",
                "Bash",
                serde_json::json!({ "command": COMMAND }),
            )
            .with_physical_url(physical.uri()),
        ))
        .without_channels(true)
        .build()
        .await
        .expect("bootstrap must succeed");
    result
        .engine
        .init_session("openai", &workspace.path().to_string_lossy(), None)
        .expect("session");
    result.engine.use_recovery_test_key(&RECOVERY_TEST_KEY);

    banner("AFTER BOOTSTRAP");
    println!(
        "  bash_shell_argv_prefix = {:?}",
        wcore_config::shell::bash_shell_argv_prefix()
    );
    for (key, value) in sandbox_relevant_env() {
        println!("  env {key} = {value:?}");
    }

    // L5 — the engine's OWN policy + sandbox runtime, driven directly.
    let engine_registry = result.engine.tools();
    let engine_policy = engine_registry
        .workspace_policy()
        .expect("bootstrap installs a workspace policy");
    let engine_sandbox = engine_registry.sandbox_runtime();
    run_bash_with_registry("5-engine-registry", engine_policy, engine_sandbox).await;

    // L6 — the engine end to end, the shape that is red.
    banner("L6 ENGINE RUN");
    let started = std::time::Instant::now();
    let run = result
        .engine
        .run("run the harmless proof command", "typed-bypass-bash")
        .await;
    println!("  elapsed = {:?}", started.elapsed());
    println!("  run_ok  = {}", run.is_ok());
    let results = capture.tool_results.lock().unwrap().clone();
    println!("  tool_results = {results:?}");
    println!(
        "  VERDICT = {}",
        if results
            .iter()
            .any(|(name, _, content)| name == "Bash" && content.contains(NEEDLE))
        {
            "NEEDLE PRESENT"
        } else {
            "NEEDLE MISSING"
        }
    );
}
