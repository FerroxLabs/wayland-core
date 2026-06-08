//! End-to-end Script tool against the real Read/Grep/Bash tools.

use std::fs;
use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use tokio::sync::RwLock;
use wcore_tools::Tool;
use wcore_tools::dispatcher::{ClosureDispatcher, ToolDispatcher};
use wcore_tools::registry::ToolRegistry;
use wcore_tools::script::ScriptTool;
use wcore_types::tool::ToolResult;

fn dispatcher_with_builtins() -> Arc<dyn ToolDispatcher> {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(wcore_tools::read::ReadTool::new(None)));
    reg.register(Box::new(wcore_tools::grep::GrepTool));
    reg.register(Box::new(wcore_tools::glob::GlobTool));
    reg.register(Box::new(wcore_tools::bash::BashTool));
    let shared = Arc::new(RwLock::new(reg));
    Arc::new(ClosureDispatcher::new(Box::new(move |tool, input| {
        let reg = Arc::clone(&shared);
        Box::pin(async move {
            let guard = reg.read().await;
            match guard.get(&tool) {
                Some(t) => t.execute(input).await,
                None => ToolResult {
                    content: format!("not in registry: {tool}"),
                    is_error: true,
                },
            }
        })
    })))
}

#[tokio::test]
async fn script_grep_then_read_against_real_files() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("lib.rs");
    fs::write(&target, "fn alpha() {}\nfn beta() {}\n").unwrap();

    let disp = dispatcher_with_builtins();
    let tool = ScriptTool::new(Arc::clone(&disp));
    let input = json!({
        "steps": [
            {"id": "s1", "tool": "Grep", "input": {
                "pattern": "fn alpha",
                "path": tmp.path().to_string_lossy()
            }},
            {"id": "s2", "tool": "Read", "input": {"file_path": target.to_string_lossy()}}
        ],
        "max_output_lines": 50
    });
    let result = tool.execute(input).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(result.content.contains("fn alpha"));
    assert!(result.content.contains("fn beta"));
    assert!(result.content.contains("s1"));
    assert!(result.content.contains("s2"));
}

#[tokio::test]
#[serial_test::serial]
async fn script_bash_step_returns_stdout_in_transcript() {
    // The Bash sub-step routes through wcore-sandbox, which fails closed when
    // no real backend can spawn (bwrap can't make user namespaces in an
    // unprivileged CI container). Opt into the documented no-sandbox degraded
    // mode so the step actually runs and emits stdout.
    // SAFETY: test-only env mutation; `#[serial]` prevents env races.
    unsafe {
        std::env::set_var("WAYLAND_SANDBOX", "none");
        std::env::set_var("WAYLAND_ALLOW_NO_SANDBOX", "1");
    }
    let disp = dispatcher_with_builtins();
    let tool = ScriptTool::new(Arc::clone(&disp));
    let input = json!({
        "steps": [
            {"id": "s1", "tool": "Bash", "input": {"command": "echo hello-from-script"}}
        ]
    });
    let result = tool.execute(input).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(result.content.contains("hello-from-script"));
}

// --- W8b.2.A: parent ctx propagation through Script sub-steps ----------

/// Builds a registry-backed dispatcher using the new ctx-aware closure
/// shape so ScriptTool sub-steps run via `Tool::execute_with_ctx`.
fn ctx_aware_dispatcher() -> Arc<dyn ToolDispatcher> {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(wcore_tools::read::ReadTool::new(None)));
    reg.register(Box::new(wcore_tools::write::WriteTool::new(None)));
    let shared = Arc::new(RwLock::new(reg));
    Arc::new(ClosureDispatcher::new_with_ctx(Box::new(
        move |tool, input, ctx| {
            let reg = Arc::clone(&shared);
            Box::pin(async move {
                let guard = reg.read().await;
                match guard.get(&tool) {
                    Some(t) => t.execute_with_ctx(input, ctx).await,
                    None => ToolResult {
                        content: format!("not in registry: {tool}"),
                        is_error: true,
                    },
                }
            })
        },
    )))
}

#[tokio::test]
async fn script_step_inherits_parent_vfs_sandbox() {
    use wcore_tools::context::ToolContext;
    use wcore_tools::vfs::{RealFs, SandboxedFs};

    // Sandboxed vfs rooted at `inside`. `outside` is a sibling tempdir
    // that the sandbox MUST reject when a Script step tries to write
    // there. Using a ctx-aware ClosureDispatcher so the parent ctx
    // flows into each sub-step.
    let inside = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();

    let vfs = SandboxedFs::new(RealFs, inside.path().to_path_buf());
    let ctx = ToolContext {
        call_id: String::new(),
        cancel: tokio_util::sync::CancellationToken::new(),
        vfs: Arc::new(vfs),
        source_agent: None,
        sink: Arc::new(wcore_tools::NullToolOutputSink),
        file_write_notifier: None,
    };

    let disp = ctx_aware_dispatcher();
    let tool = ScriptTool::new(Arc::clone(&disp));

    // Step 1 writes inside the sandbox — must succeed.
    let inside_target = inside.path().join("a.txt");
    let input_ok = json!({
        "steps": [
            {"id": "s1", "tool": "Write", "input": {
                "file_path": inside_target.to_string_lossy(),
                "content": "from-script"
            }}
        ]
    });
    let r = tool.execute_with_ctx(input_ok, &ctx).await;
    assert!(
        !r.is_error,
        "in-sandbox write should succeed: {}",
        r.content
    );
    let bytes = std::fs::read(&inside_target).expect("file should exist");
    assert_eq!(bytes, b"from-script");

    // Step 2 writes outside the sandbox — sandbox rejection must fire,
    // proving the SandboxedFs vfs propagated through the Script
    // dispatcher into the child WriteTool.
    let outside_target = outside.path().join("escape.txt");
    let input_escape = json!({
        "steps": [
            {"id": "s2", "tool": "Write", "input": {
                "file_path": outside_target.to_string_lossy(),
                "content": "should-be-blocked"
            }}
        ]
    });
    let r2 = tool.execute_with_ctx(input_escape, &ctx).await;
    assert!(
        r2.is_error,
        "out-of-sandbox write must be rejected via inherited vfs, got: {}",
        r2.content
    );
    assert!(
        !outside_target.exists(),
        "rejected write must not land on disk"
    );
}
