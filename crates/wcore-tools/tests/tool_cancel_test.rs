//! W8a A.4 — cancellation propagation tests.
//!
//! Confirms that long-running tools observe `ctx.cancel.cancelled()` and
//! return promptly when fired. Plan's named acceptance test:
//! `bash_tool_kills_child_on_cancel`.

// These imports are only consumed by the cancellation test below, which
// is #[cfg(unix)] (uses `bash sleep 30` — unix-only). The second test
// in this file only needs BashTool + ToolContext + json!, so gate the
// rest to cfg(unix) to keep Windows clippy clean.
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::{Duration, Instant};

use serde_json::json;
#[cfg(unix)]
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use wcore_tools::NullToolOutputSink;
use wcore_tools::Tool; // .execute_with_ctx — used by BOTH tests
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
#[cfg(unix)]
use wcore_tools::vfs::RealFs;

fn no_sandbox_runtime() -> std::sync::Arc<wcore_sandbox::SandboxRegistry> {
    std::sync::Arc::new(wcore_sandbox::SandboxRegistry::new(std::sync::Arc::new(
        wcore_sandbox::backends::no_sandbox::NoSandboxBackend::new(),
    )))
}

#[tokio::test]
#[cfg(unix)]
async fn bash_tool_kills_long_sleep_on_cancel() {
    // This is a cancellation test, so inject a deterministic execution
    // backend instead of depending on host sandbox availability.
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    // Fire the cancel after 100ms.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel2.cancel();
    });
    let ctx = ToolContext::new(
        "test-call-1",
        cancel,
        Arc::new(RealFs),
        None,
        Arc::new(NullToolOutputSink),
    )
    .with_sandbox(no_sandbox_runtime());
    let start = Instant::now();
    let result = BashTool
        .execute_with_ctx(json!({ "command": "sleep 30" }), &ctx)
        .await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(800),
        "expected cancel <800ms, elapsed = {:?}",
        elapsed
    );
    assert!(
        result.is_error,
        "cancellation should produce an error result; got: {}",
        result.content
    );
    assert!(
        result.content.to_lowercase().contains("cancel"),
        "result content should mention cancellation; got: {}",
        result.content
    );
}

#[tokio::test]
async fn bash_tool_ctx_passthrough_runs_normally_when_no_cancel() {
    let ctx = ToolContext::test_default().with_sandbox(no_sandbox_runtime());
    let result = BashTool
        .execute_with_ctx(json!({ "command": "echo bash_ctx_ok" }), &ctx)
        .await;
    assert!(!result.is_error, "unexpected error: {}", result.content);
    assert!(result.content.contains("bash_ctx_ok"));
}
