//! W4 F13 audit fix HIGH-2: pre-decided dispatch shape for ScriptTool.
//!
//! `ToolDispatcher` is a 1-method trait. `ToolRegistry` implements it via
//! the existing `get()`+`execute()` flow. `ScriptTool` holds an
//! `Arc<dyn ToolDispatcher>`, which lets the live registry serve as the
//! dispatch surface without requiring every call site in wcore-agent to
//! switch to `Arc<ToolRegistry>`.

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use serde_json::Value;
use wcore_config::circuit_breaker::BreakerState;
use wcore_types::tool::ToolResult;

/// Dispatch a tool call by name. Implementations look the tool up in their
/// registered set and run `Tool::execute`. Returning an `is_error: true`
/// `ToolResult` for unknown names is OK — the caller (ScriptTool) treats
/// any `is_error` as a short-circuit.
#[async_trait]
pub trait ToolDispatcher: Send + Sync {
    async fn dispatch(&self, tool: &str, input: Value) -> ToolResult;

    /// W8b.2.A — like `dispatch`, but propagates the caller's
    /// `ToolContext` so the child tool inherits `vfs`, `cancel`,
    /// `file_write_notifier`, etc.
    ///
    /// Default impl falls through to `dispatch(tool, input)` so all
    /// existing implementors (every plugin-side ToolDispatcher) stay
    /// byte-identical until they opt in. ScriptTool routes through this
    /// entry point so its sub-steps observe the parent context.
    async fn dispatch_with_ctx(
        &self,
        tool: &str,
        input: Value,
        _ctx: &crate::context::ToolContext,
    ) -> ToolResult {
        self.dispatch(tool, input).await
    }

    /// H2-R5: Returns the circuit-breaker state for a named tool.
    ///
    /// Default returns `None` — only `ToolRegistry` (which owns the
    /// breakers) overrides this. Existing `ClosureDispatcher` and
    /// plugin-side dispatchers stay byte-identical.
    fn breaker_state(&self, _tool: &str) -> Option<BreakerState> {
        None
    }
}

/// Boxed closure type used by `ClosureDispatcher`. Captures whatever shared
/// state the host needs (typically `Arc<RwLock<ToolRegistry>>`).
pub type DispatchFn =
    Box<dyn Fn(String, Value) -> Pin<Box<dyn Future<Output = ToolResult> + Send>> + Send + Sync>;

/// W8b.2.A — ctx-aware closure shape. Captures the same shared state as
/// `DispatchFn` but additionally receives a `ToolContext` reference so
/// the closure body can call `Tool::execute_with_ctx` rather than the
/// legacy `Tool::execute`. The reference is borrowed for the lifetime
/// of the await, hence the `for<'a>` HRTB on the function pointer.
pub type DispatchWithCtxFn = Box<
    dyn for<'a> Fn(
            String,
            Value,
            &'a crate::context::ToolContext,
        ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>
        + Send
        + Sync,
>;

/// Generic adapter wrapping a `DispatchFn`. Lets the bootstrap path build
/// a dispatcher from a closure that captures the live registry rather
/// than handing `ScriptTool` a typed `Arc<ToolRegistry>` directly.
///
/// Two construction paths:
/// * `ClosureDispatcher::new(f)` — legacy `DispatchFn` (no ctx). The
///   trait default for `dispatch_with_ctx` falls through to `dispatch`,
///   so sub-tools see a fresh `ToolContext::test_default()` instead of
///   the caller's. Existing callers stay byte-identical.
/// * `ClosureDispatcher::new_with_ctx(f)` — ctx-aware
///   `DispatchWithCtxFn`. The dispatcher routes both `dispatch` (with a
///   `test_default` ctx) and `dispatch_with_ctx` (with the caller's
///   ctx) through the ctx-aware closure so ScriptTool sub-steps inherit
///   the parent's vfs / cancel / file_write_notifier.
pub struct ClosureDispatcher {
    legacy: Option<DispatchFn>,
    ctx_aware: Option<DispatchWithCtxFn>,
}

impl ClosureDispatcher {
    pub fn new(f: DispatchFn) -> Self {
        Self {
            legacy: Some(f),
            ctx_aware: None,
        }
    }

    /// W8b.2.A — build a ctx-propagating dispatcher. Both `dispatch`
    /// and `dispatch_with_ctx` route through this closure; the
    /// non-ctx path mints a `ToolContext::test_default()` so the
    /// closure body can always assume a real ctx reference.
    pub fn new_with_ctx(f: DispatchWithCtxFn) -> Self {
        Self {
            legacy: None,
            ctx_aware: Some(f),
        }
    }
}

#[async_trait]
impl ToolDispatcher for ClosureDispatcher {
    async fn dispatch(&self, tool: &str, input: Value) -> ToolResult {
        if let Some(f) = self.legacy.as_ref() {
            return f(tool.to_string(), input).await;
        }
        if let Some(f) = self.ctx_aware.as_ref() {
            let ctx = crate::context::ToolContext::test_default();
            return f(tool.to_string(), input, &ctx).await;
        }
        ToolResult {
            content: "ClosureDispatcher built with no backing function".to_string(),
            is_error: true,
        }
    }

    async fn dispatch_with_ctx(
        &self,
        tool: &str,
        input: Value,
        ctx: &crate::context::ToolContext,
    ) -> ToolResult {
        if let Some(f) = self.ctx_aware.as_ref() {
            return f(tool.to_string(), input, ctx).await;
        }
        // Fall back to the legacy closure (drops ctx). Existing callers
        // that built via `::new` preserve their pre-W8b.2.A behaviour.
        if let Some(f) = self.legacy.as_ref() {
            return f(tool.to_string(), input).await;
        }
        ToolResult {
            content: "ClosureDispatcher built with no backing function".to_string(),
            is_error: true,
        }
    }
}
