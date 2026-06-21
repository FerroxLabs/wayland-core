//! S9 — BashTool routes all 4 execute paths through the sandbox backend.
//!
//! These tests exercise BashTool with the `NoSandboxBackend` active
//! (`WAYLAND_SANDBOX=none`) and assert that every execute path —
//! `execute`, `execute_streaming`, `execute_with_ctx`,
//! `execute_streaming_with_ctx` — produces the same observable output it
//! produced before S9. The four paths funnel through
//! `SandboxBackend::execute` / `SandboxBackend::execute_streaming`; if any
//! one were not routed it would be a sandbox-escape hole (audit B-C1).
//!
//! `WAYLAND_SANDBOX` is process-global, so every test in this file is
//! serialized with `#[serial]` and sets the env var itself.

use std::sync::Mutex;

use serde_json::json;
use serial_test::serial;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::{Tool, ToolOutputSink};

/// Force the NoSandbox backend so behaviour is byte-identical to the
/// pre-S9 direct-exec path, regardless of what the host platform has.
///
/// `WAYLAND_SANDBOX=none` selects the NoSandbox backend, but the security
/// hardening makes that backend *refuse to run* ("sandbox UNAVAILABLE and
/// unsandboxed execution is not permitted") unless the operator explicitly
/// accepts running with no isolation via `WAYLAND_ALLOW_NO_SANDBOX=1`. These
/// are routing tests (do all 4 execute paths funnel through the backend?),
/// not isolation tests, so the no-isolation opt-in is exactly the intended
/// way to exercise the NoSandbox path — set both.
fn force_no_sandbox() {
    // SAFETY: test-only env mutation; every test in this file is
    // `#[serial]` so no other thread races this write.
    unsafe {
        std::env::set_var("WAYLAND_SANDBOX", "none");
        std::env::set_var("WAYLAND_ALLOW_NO_SANDBOX", "1");
    }
}

/// Collects streamed chunks for assertions.
struct CapSink(Mutex<Vec<String>>);

impl ToolOutputSink for CapSink {
    fn emit_chunk(&self, chunk: &str) {
        self.0.lock().unwrap().push(chunk.to_string());
    }
}

impl CapSink {
    fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }
    // Only called from the `#[cfg(unix)]` streaming assertions below
    // (printf is portable Unix; Windows clippy with `-D warnings`
    // would otherwise flag this as dead_code on the Windows target).
    #[allow(dead_code)]
    fn chunks(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// Path 1 of 4: `execute` — buffered, routed through `SandboxBackend::execute`.
#[tokio::test]
#[serial]
async fn execute_routes_through_sandbox_and_returns_stdout() {
    force_no_sandbox();
    let tool = BashTool;
    let result = tool.execute(json!({"command": "echo hello_sandbox"})).await;
    assert!(!result.is_error, "unexpected error: {}", result.content);
    assert!(
        result.content.contains("hello_sandbox"),
        "stdout must reach the result: {}",
        result.content
    );
    assert!(
        result.content.contains("Exit code: 0"),
        "exit 0 must be reported: {}",
        result.content
    );
}

/// `execute` of a failing command must still report a non-zero exit and
/// flag `is_error` — exit-code semantics unchanged by sandbox routing.
#[tokio::test]
#[serial]
async fn execute_failing_command_reports_error() {
    force_no_sandbox();
    let tool = BashTool;
    let result = tool.execute(json!({"command": "exit 3"})).await;
    assert!(result.is_error, "exit 3 must flag is_error");
    assert!(
        result.content.contains("Exit code: 3"),
        "exit code must be preserved: {}",
        result.content
    );
}

/// Path 2 of 4: `execute_streaming` — routed through
/// `SandboxBackend::execute_streaming`, yields chunks + a terminal exit.
#[tokio::test]
#[serial]
async fn execute_streaming_routes_through_sandbox_and_yields_chunks() {
    force_no_sandbox();
    let tool = BashTool;
    let sink = CapSink::new();
    // printf is portable on Unix; gate the chunk assertion on cfg(unix).
    #[cfg(unix)]
    {
        let result = tool
            .execute_streaming(json!({"command": "printf 'x\\ny\\nz\\n'"}), &sink)
            .await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        let chunks = sink.chunks();
        assert!(
            !chunks.is_empty(),
            "streaming path must emit chunks via the sink; got {chunks:?}"
        );
        assert!(
            result.content.contains('x') && result.content.contains('z'),
            "all streamed output must land in the result: {}",
            result.content
        );
        assert!(
            result.content.contains("Exit code: 0"),
            "terminal exit must be reported: {}",
            result.content
        );
    }
    #[cfg(windows)]
    {
        let result = tool
            .execute_streaming(json!({"command": "echo hello_stream"}), &sink)
            .await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("hello_stream"));
    }
}

/// The streaming receiver must deliver a terminal `Exit` chunk: a failing
/// command streamed through the sandbox still reports its non-zero code.
#[tokio::test]
#[serial]
async fn execute_streaming_failing_command_reports_exit() {
    force_no_sandbox();
    let tool = BashTool;
    let sink = CapSink::new();
    let result = tool
        .execute_streaming(json!({"command": "exit 7"}), &sink)
        .await;
    assert!(result.is_error, "exit 7 must flag is_error");
    assert!(
        result.content.contains("Exit code: 7"),
        "streaming exit code must be preserved: {}",
        result.content
    );
}

/// Path 3 of 4: `execute_with_ctx` — cancel-aware buffered path. Delegates
/// to `execute`, so it too routes through `SandboxBackend::execute`.
#[tokio::test]
#[serial]
async fn execute_with_ctx_routes_through_sandbox() {
    force_no_sandbox();
    let tool = BashTool;
    let ctx = ToolContext::test_default();
    let result = tool
        .execute_with_ctx(json!({"command": "echo ctx_buffered"}), &ctx)
        .await;
    assert!(!result.is_error, "unexpected error: {}", result.content);
    assert!(
        result.content.contains("ctx_buffered"),
        "ctx path output must route through sandbox: {}",
        result.content
    );
}

/// `execute_with_ctx` with an already-cancelled token must short-circuit
/// to the cancelled result — cancellation semantics unchanged by S9.
#[tokio::test]
#[serial]
async fn execute_with_ctx_honors_cancellation() {
    force_no_sandbox();
    let tool = BashTool;
    let ctx = ToolContext::test_default();
    ctx.cancel.cancel();
    let result = tool
        .execute_with_ctx(json!({"command": "echo should_not_run"}), &ctx)
        .await;
    assert!(result.is_error, "cancelled call must flag is_error");
    assert!(
        result.content.contains("cancelled"),
        "cancelled result shape must be preserved: {}",
        result.content
    );
}

/// Path 4 of 4: `execute_streaming_with_ctx` — cancel-aware streaming path.
/// Delegates to `execute_streaming`, routing through
/// `SandboxBackend::execute_streaming`.
#[tokio::test]
#[serial]
async fn execute_streaming_with_ctx_routes_through_sandbox() {
    force_no_sandbox();
    let tool = BashTool;
    let ctx = ToolContext::test_default();
    let sink = CapSink::new();
    let result = tool
        .execute_streaming_with_ctx(json!({"command": "echo ctx_streamed"}), &ctx, &sink)
        .await;
    assert!(!result.is_error, "unexpected error: {}", result.content);
    assert!(
        result.content.contains("ctx_streamed"),
        "ctx streaming output must route through sandbox: {}",
        result.content
    );
}

/// Curated env + cwd through the sandbox path.
///
/// D.1 Round 1 (HIGH-2): BashTool no longer copies the engine's *entire*
/// host env into the child — it builds a curated allowlist via
/// `env_passthrough::build_sandboxed_env`. This test asserts the new
/// contract:
/// - an allowlisted toolchain var (`PATH`) reaches the child;
/// - a skill/config-registered passthrough var reaches the child;
/// - an arbitrary, non-allowlisted, non-registered var does NOT;
/// - cwd is still inherited.
#[tokio::test]
#[serial]
async fn env_and_cwd_are_honored_through_sandbox() {
    force_no_sandbox();
    // A var that is neither on the base allowlist nor registered — it must
    // be STRIPPED from the sandboxed child (secret-confinement fix).
    // SAFETY: test-only; `#[serial]` serializes env access in this file.
    unsafe {
        std::env::set_var("S9_BASH_ENV_PROBE", "probe_value_42");
    }
    // A var registered via the passthrough allowlist — it MUST reach the
    // child even though it is not on the base allowlist.
    wcore_tools::env_passthrough::register_env_passthrough(["S9_PASSTHROUGH_PROBE"]);
    unsafe {
        std::env::set_var("S9_PASSTHROUGH_PROBE", "passed_through_99");
    }
    let tool = BashTool;

    #[cfg(unix)]
    {
        // PATH is on the base allowlist — it must reach the child so the
        // shell can find binaries.
        let result = tool.execute(json!({"command": "echo \"$PATH\""})).await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains('/'),
            "PATH must reach the sandboxed child: {}",
            result.content
        );

        // The registered passthrough var must reach the child.
        let result = tool
            .execute(json!({"command": "echo \"$S9_PASSTHROUGH_PROBE\""}))
            .await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            result.content.contains("passed_through_99"),
            "a registered passthrough var must reach the child: {}",
            result.content
        );

        // The arbitrary, unregistered var must NOT reach the child — this
        // is the HIGH-2 secret-confinement guarantee.
        let result = tool
            .execute(json!({"command": "echo \"[$S9_BASH_ENV_PROBE]\""}))
            .await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(
            !result.content.contains("probe_value_42"),
            "a non-allowlisted host var must be stripped from the child: {}",
            result.content
        );

        // Cwd: `pwd` reflects the directory the command runs in. BashTool
        // does not set an explicit cwd, so the child inherits the engine's
        // working directory — `pwd` must succeed and emit a path.
        let result = tool.execute(json!({"command": "pwd"})).await;
        assert!(!result.is_error, "pwd failed: {}", result.content);
        assert!(
            result.content.contains('/'),
            "pwd must emit an absolute path: {}",
            result.content
        );
    }
    #[cfg(windows)]
    {
        let result = tool
            .execute(json!({"command": "echo %S9_PASSTHROUGH_PROBE%"}))
            .await;
        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("passed_through_99"));
    }

    // SAFETY: test cleanup.
    unsafe {
        std::env::remove_var("S9_BASH_ENV_PROBE");
        std::env::remove_var("S9_PASSTHROUGH_PROBE");
    }
    wcore_tools::env_passthrough::clear_env_passthrough();
}

/// The credential-exfiltration denylist must still refuse before any
/// sandbox dispatch — routing through the sandbox does not weaken the
/// pre-spawn Wave SA guard, on either the buffered or streaming path.
#[tokio::test]
#[serial]
async fn denylist_still_refuses_before_sandbox_dispatch() {
    force_no_sandbox();
    let tool = BashTool;
    let buffered = tool.execute(json!({"command": "env"})).await;
    assert!(buffered.is_error, "`env` must be refused on buffered path");
    assert!(buffered.content.contains("denylist"));

    let sink = CapSink::new();
    let streamed = tool
        .execute_streaming(json!({"command": "printenv"}), &sink)
        .await;
    assert!(
        streamed.is_error,
        "`printenv` must be refused on streaming path"
    );
    assert!(streamed.content.contains("denylist"));
}

// ── Task 8 — exec-time capability gate ────────────────────────────────────────
//
// With `WAYLAND_SANDBOX=none` + `WAYLAND_ALLOW_NO_SANDBOX=1`, the active
// backend is `NoSandboxBackend`, which returns `enforces_read_deny()=false`.
// A ctx carrying a `Contained` workspace policy must therefore be refused
// at exec time — the TOCTOU-free boundary in bash.rs.

/// Task 8: `execute_with_ctx` must refuse with a Contained policy when the
/// active backend doesn't enforce secret-read-deny.
#[tokio::test]
#[serial]
async fn exec_time_gate_contained_policy_no_enforcing_backend_refuses() {
    // NoSandboxBackend: enforces_read_deny() = false.
    force_no_sandbox();
    let dir = tempfile::tempdir().unwrap();
    let policy = std::sync::Arc::new(wcore_tools::workspace_policy::WorkspacePolicy::contained(
        dir.path(),
    ));
    let ctx = ToolContext::test_default().with_workspace(policy);
    let tool = BashTool;
    let result = tool
        .execute_with_ctx(json!({"command": "echo hello"}), &ctx)
        .await;
    assert!(
        result.is_error,
        "Contained + non-enforcing backend must refuse; got: {}",
        result.content
    );
    assert!(
        result.content.contains("secret-read-deny"),
        "refusal message must mention secret-read-deny: {}",
        result.content
    );
}

/// Task 8: `execute_streaming_with_ctx` must also refuse with a Contained
/// policy when the active backend doesn't enforce secret-read-deny.
#[tokio::test]
#[serial]
async fn exec_time_gate_streaming_contained_policy_no_enforcing_backend_refuses() {
    force_no_sandbox();
    let dir = tempfile::tempdir().unwrap();
    let policy = std::sync::Arc::new(wcore_tools::workspace_policy::WorkspacePolicy::contained(
        dir.path(),
    ));
    let ctx = ToolContext::test_default().with_workspace(policy);
    let tool = BashTool;
    let sink = CapSink::new();
    let result = tool
        .execute_streaming_with_ctx(json!({"command": "echo hello"}), &ctx, &sink)
        .await;
    assert!(
        result.is_error,
        "Contained + non-enforcing backend must refuse on streaming path; got: {}",
        result.content
    );
    assert!(
        result.content.contains("secret-read-deny"),
        "refusal message must mention secret-read-deny: {}",
        result.content
    );
}

/// Task 8: `execute_with_ctx` with a Trusted policy and non-enforcing backend
/// must NOT refuse — the gate only applies to Contained mode.
#[tokio::test]
#[serial]
async fn exec_time_gate_trusted_policy_passes_through() {
    force_no_sandbox();
    let dir = tempfile::tempdir().unwrap();
    let policy = std::sync::Arc::new(
        wcore_tools::workspace_policy::WorkspacePolicy::trusted_local(dir.path()),
    );
    let ctx = ToolContext::test_default().with_workspace(policy);
    let tool = BashTool;
    // echo is safe and not on the denylist — must reach the shell.
    let result = tool
        .execute_with_ctx(json!({"command": "echo trusted_ok"}), &ctx)
        .await;
    // The result content should NOT be the exec-time refusal.
    assert!(
        !result.content.contains("secret-read-deny"),
        "Trusted policy must NOT trigger the exec-time gate: {}",
        result.content
    );
}
