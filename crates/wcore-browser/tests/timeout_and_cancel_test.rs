//! Wave RC — Browser tool timeout + cancel cooperation tests.
//!
//! Reproduces the "Browser tool hangs the agent for 10 minutes" bug
//! reported 2026-05-22:
//!
//!   1. The Browser tool was classified as `ToolCategory::Exec` (600s
//!      dispatch-timeout) — wrong for a network/IO operation.
//!   2. The Camoufox HTTP backend's `reqwest::Client` had NO timeouts —
//!      a stalled sidecar wedged every operation until the 600s outer
//!      backstop.
//!   3. A user Esc fires `ctx.cancel`, which `BrowserTool::dispatch_inner`
//!      DOES race in a `select!` — but a per-op deadline was missing, so
//!      a never-cancelled operation against a hung sidecar inherited
//!      only the dispatcher's 600s.
//!
//! These tests assert the locked behaviour:
//!   * Browser is classified as `ToolCategory::Mcp` (120s) — not `Exec`.
//!   * A pre-cancelled `ctx.cancel` returns within ~500ms even against a
//!     never-responding backend.
//!   * A live cancel fired mid-flight reaches the tool within ~500ms.
//!   * The per-op timeout caps a hung navigate at ~60s without needing
//!     the dispatcher backstop (here we use a much shorter custom op
//!     timeout via the test fixture — see `HangBackend`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::json;
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;

use wcore_browser::aria::AriaSnapshot;
use wcore_browser::op::BrowserOp;
use wcore_browser::policy::BrowserPolicy;
use wcore_browser::provider::{
    BrowserOpError, BrowserProvider, BrowserSession, OpResult, SessionCtx,
};
use wcore_browser::supervisor::BrowserSupervisor;
use wcore_browser::tool::BrowserTool;

/// Backend that NEVER responds — simulates a hung sidecar / network
/// drop / chromium process that accepted the navigate but never paged.
/// `open_session` returns instantly; `dispatch` parks forever.
struct HangBackend;

#[async_trait]
impl BrowserProvider for HangBackend {
    async fn open_session(
        &self,
        persistent_profile: bool,
    ) -> Result<BrowserSession, BrowserOpError> {
        Ok(BrowserSession {
            ctx: SessionCtx::for_test("hang-sess"),
            persistent_profile,
        })
    }
    async fn close_session(&self, _ctx: &SessionCtx) -> Result<(), BrowserOpError> {
        Ok(())
    }
    async fn dispatch(
        &self,
        _ctx: &SessionCtx,
        _op: BrowserOp,
    ) -> Result<OpResult, BrowserOpError> {
        // Pretend to be a wedged sidecar — never resolve.
        std::future::pending::<()>().await;
        unreachable!()
    }
    fn backend_name(&self) -> &'static str {
        "hang"
    }
}

/// Backend that completes a Snapshot quickly so the success path is
/// covered when no cancel/timeout fires.
struct InstantBackend;

#[async_trait]
impl BrowserProvider for InstantBackend {
    async fn open_session(
        &self,
        persistent_profile: bool,
    ) -> Result<BrowserSession, BrowserOpError> {
        Ok(BrowserSession {
            ctx: SessionCtx::for_test("instant-sess"),
            persistent_profile,
        })
    }
    async fn close_session(&self, _ctx: &SessionCtx) -> Result<(), BrowserOpError> {
        Ok(())
    }
    async fn dispatch(
        &self,
        _ctx: &SessionCtx,
        _op: BrowserOp,
    ) -> Result<OpResult, BrowserOpError> {
        Ok(OpResult::Snapshot {
            snapshot: AriaSnapshot::empty(),
        })
    }
    fn backend_name(&self) -> &'static str {
        "instant"
    }
}

fn tool_with(provider: Arc<dyn BrowserProvider>) -> BrowserTool {
    BrowserTool::new(
        provider,
        BrowserPolicy::default(),
        Arc::new(BrowserSupervisor::new()),
    )
}

#[test]
fn browser_tool_is_classified_as_mcp_not_exec() {
    // Locked decision (2026-05-22): Browser is a network/IO tool, not an
    // interactive shell. The dispatcher's per-category timeout is 120s
    // for Mcp and 600s for Exec — Exec is too generous for browser ops.
    let tool = tool_with(Arc::new(InstantBackend));
    assert_eq!(
        tool.category(),
        ToolCategory::Mcp,
        "Browser tool must be Mcp (120s budget), not Exec (600s) — see Wave RC"
    );
}

#[tokio::test]
async fn cancel_token_fires_returns_within_500ms_against_hung_backend() {
    // Decisive test: user presses Esc, ctx.cancel.cancel() fires,
    // BrowserTool must observe the cancel and return within ~500ms,
    // NOT wait for the per-op timeout (60s) or the dispatcher backstop
    // (120s after the Mcp re-categorisation).
    let tool = tool_with(Arc::new(HangBackend));
    let ctx = ToolContext::test_default();
    let cancel = ctx.cancel.clone();

    // Fire cancel after 100ms — simulates the user's Esc.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });

    let started = Instant::now();
    let result = tool
        .execute_with_ctx(json!({ "op": { "kind": "snapshot" } }), &ctx)
        .await;
    let elapsed = started.elapsed();

    assert!(
        result.is_error,
        "cancelled op must surface as error, got: {}",
        result.content
    );
    assert!(
        result.content.to_lowercase().contains("cancel"),
        "expected cancel message, got: {}",
        result.content
    );
    assert!(
        elapsed < Duration::from_millis(700),
        "cancel must take effect within ~500ms (locked policy), took {elapsed:?}"
    );
}

#[tokio::test]
async fn per_op_timeout_bounds_a_hung_backend_without_cancel() {
    // The dispatcher's outer timeout is 120s (Mcp). For tighter UX a
    // per-op deadline lives INSIDE the BrowserTool: a hung navigate /
    // snapshot / screenshot / etc. must fail with a typed timeout error
    // well inside the outer budget.
    //
    // We use a custom-config tool with a 250ms per-op timeout so the
    // test runs fast; production defaults are 60s nav / 30s screenshot.
    let tool = BrowserTool::with_op_timeout(
        Arc::new(HangBackend),
        BrowserPolicy::default(),
        Arc::new(BrowserSupervisor::new()),
        Duration::from_millis(250),
    );
    let ctx = ToolContext::test_default();
    let started = Instant::now();
    let result = tool
        .execute_with_ctx(json!({ "op": { "kind": "snapshot" } }), &ctx)
        .await;
    let elapsed = started.elapsed();

    assert!(result.is_error, "hung op must error out");
    assert!(
        result.content.to_lowercase().contains("timed out")
            || result.content.to_lowercase().contains("timeout"),
        "expected timeout message, got: {}",
        result.content
    );
    // 250ms deadline + some slack.
    assert!(
        elapsed < Duration::from_millis(800),
        "per-op timeout must bound the await, took {elapsed:?}"
    );
}

#[tokio::test]
async fn instant_backend_succeeds_well_under_per_op_timeout() {
    // Sanity: the per-op timeout doesn't trip a fast happy-path op.
    let tool = BrowserTool::with_op_timeout(
        Arc::new(InstantBackend),
        BrowserPolicy::default(),
        Arc::new(BrowserSupervisor::new()),
        Duration::from_secs(5),
    );
    let ctx = ToolContext::test_default();
    let result = tool
        .execute_with_ctx(json!({ "op": { "kind": "snapshot" } }), &ctx)
        .await;
    assert!(
        !result.is_error,
        "instant backend must succeed; got: {}",
        result.content
    );
}
