//! Host-side adapter — wraps a plugin-api `PluginTool` and exposes it as
//! a real `wcore_tools::Tool`.
//!
//! This is the host side of the plugin isolation boundary. `PluginTool`
//! lives in `wcore-plugin-api`, which CANNOT depend on `wcore-tools`
//! (`FORBIDDEN_CORE_IMPORTS`). `wcore-agent` CAN — so `PluginToolAdapter`
//! is the ONLY type that names both `PluginTool` and `wcore_tools::Tool`.
//! It is the generic-tool analogue of `cua_adapter` / `browser_adapter`:
//! plugin-api-local data in, real `Tool` out.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use wcore_plugin_api::tool::{
    PluginTool, PluginToolCaps, PluginToolEffectIdentity, PluginToolEmit, PluginToolInvocation,
};
use wcore_protocol::events::ToolCategory;
use wcore_tools::context::{ToolContext, ToolEffectContext};
use wcore_tools::{NullToolOutputSink, Tool, ToolOutputSink};
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolResult};

/// Buffering `ToolOutputSink` used by `execute_streaming` (no-ctx path).
///
/// `async_trait` boxes the generated future as `Box<dyn Future + Send +
/// 'static>`, so nothing with a non-`'static` lifetime can be captured —
/// that rules out holding a `&dyn ToolOutputSink` reference. Instead we
/// buffer every chunk/progress call while the plugin closure runs, then
/// replay to the real sink *after* `await` returns (still within the
/// caller's borrow of `sink`).
#[derive(Default)]
struct BufferedSink {
    chunks: std::sync::Mutex<Vec<String>>,
    progress: std::sync::Mutex<Vec<(f32, String)>>,
}

impl ToolOutputSink for BufferedSink {
    fn emit_chunk(&self, chunk: &str) {
        self.chunks.lock().unwrap().push(chunk.to_owned());
    }
    fn emit_progress(&self, pct: f32, message: &str) {
        self.progress
            .lock()
            .unwrap()
            .push((pct, message.to_owned()));
    }
}

/// Wraps a plugin-api `PluginTool` and exposes it as a `wcore_tools::Tool`.
pub struct PluginToolAdapter {
    inner: PluginTool,
}

impl PluginToolAdapter {
    pub fn new(inner: PluginTool) -> Self {
        Self { inner }
    }

    /// Build a `PluginToolEmit` whose closures forward into a real
    /// `&dyn ToolOutputSink`. The sink is `Arc`-cloned so the closures
    /// own a `'static` handle.
    fn emit_for(sink: Arc<dyn ToolOutputSink>) -> PluginToolEmit {
        let chunk_sink = sink.clone();
        let progress_sink = sink;
        PluginToolEmit::new(
            Arc::new(move |c: &str| chunk_sink.emit_chunk(c)),
            Arc::new(move |p: f32, m: &str| progress_sink.emit_progress(p, m)),
        )
    }

    fn caps_from_ctx(ctx: &ToolContext, effect: Option<&ToolEffectContext>) -> PluginToolCaps {
        let caps = PluginToolCaps::v1(
            ctx.cancel.clone(),
            ctx.call_id.clone(),
            ctx.source_agent.clone(),
        );
        match effect {
            Some(effect) => caps.with_effect_identity(PluginToolEffectIdentity::v1(
                effect.tool_execution_id.clone(),
                effect.idempotency_key.clone(),
            )),
            None => caps,
        }
    }
}

#[async_trait]
impl Tool for PluginToolAdapter {
    fn name(&self) -> &str {
        &self.inner.name
    }

    fn description(&self) -> &str {
        &self.inner.description
    }

    fn input_schema(&self) -> JsonSchema {
        self.inner.input_schema.clone()
    }

    fn category(&self) -> ToolCategory {
        self.inner.category
    }

    fn is_deferred(&self) -> bool {
        self.inner.is_deferred
    }

    fn max_result_size(&self) -> usize {
        self.inner.max_result_size
    }

    /// Plugin tools are conservatively NOT concurrency-safe in Phase 1
    /// (the host cannot prove a closure is reentrant-safe). A later
    /// `PluginTool` field can lift this if needed.
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        // Plugin closures are untrusted effect surfaces with no host reconciler.
        ToolEffectContract::default()
    }

    /// Plugin tools opt INTO streaming — the adapter always routes
    /// through the closure, which receives a live emit sink.
    fn supports_streaming(&self) -> bool {
        true
    }

    /// Non-streaming entry: build an invocation with a `NullToolOutputSink`
    /// emit and an open cancel token.
    async fn execute(&self, input: Value) -> ToolResult {
        let emit = Self::emit_for(Arc::new(NullToolOutputSink));
        let inv = PluginToolInvocation {
            input,
            emit,
            caps: PluginToolCaps::v1(tokio_util::sync::CancellationToken::new(), "", None),
        };
        (self.inner.execute)(inv).await
    }

    /// No-ctx streaming entry: wire the passed `sink` into the emit so
    /// chunks are not silently dropped. The default impl falls back to
    /// `execute()` which builds a null sink, losing all streaming output.
    ///
    /// `async_trait` boxes the generated future as `'static`, so we cannot
    /// capture `&dyn ToolOutputSink` directly. We route through a
    /// `BufferedSink` that collects chunks while the closure runs, then
    /// replay them to the real sink after `await` (still within its borrow).
    async fn execute_streaming(&self, input: Value, sink: &dyn ToolOutputSink) -> ToolResult {
        let buf = Arc::new(BufferedSink::default());
        let emit = Self::emit_for(Arc::clone(&buf) as Arc<dyn ToolOutputSink>);
        let inv = PluginToolInvocation {
            input,
            emit,
            caps: PluginToolCaps::v1(tokio_util::sync::CancellationToken::new(), "", None),
        };
        let result = (self.inner.execute)(inv).await;
        // Replay buffered output to the real sink now that `await` has
        // returned and `sink` is still in scope.
        for chunk in buf.chunks.lock().unwrap().drain(..) {
            sink.emit_chunk(&chunk);
        }
        for (pct, msg) in buf.progress.lock().unwrap().drain(..) {
            sink.emit_progress(pct, &msg);
        }
        result
    }

    /// ctx + streaming entry: wire the real sink + the real `ToolContext`
    /// capabilities into the invocation. This is the path the engine
    /// dispatcher uses for plugin tools.
    async fn execute_streaming_with_ctx(
        &self,
        input: Value,
        ctx: &ToolContext,
        sink: &dyn ToolOutputSink,
    ) -> ToolResult {
        // `ToolContext` already carries an `Arc<dyn ToolOutputSink>`
        // (`ctx.sink`) which the dispatcher set as the canonical sink for
        // this call; the explicitly-passed `sink` is the same channel.
        let _ = sink;
        let emit = Self::emit_for(ctx.sink.clone());
        let inv = PluginToolInvocation {
            input,
            emit,
            caps: Self::caps_from_ctx(ctx, None),
        };
        (self.inner.execute)(inv).await
    }

    /// ctx-only entry (no separate sink): same wiring, sink from ctx.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let emit = Self::emit_for(ctx.sink.clone());
        let inv = PluginToolInvocation {
            input,
            emit,
            caps: Self::caps_from_ctx(ctx, None),
        };
        (self.inner.execute)(inv).await
    }

    async fn execute_with_effect_ctx(
        &self,
        input: Value,
        ctx: &ToolContext,
        effect: Option<&ToolEffectContext>,
    ) -> ToolResult {
        let emit = Self::emit_for(ctx.sink.clone());
        let inv = PluginToolInvocation {
            input,
            emit,
            caps: Self::caps_from_ctx(ctx, effect),
        };
        (self.inner.execute)(inv).await
    }

    async fn execute_streaming_with_effect_ctx(
        &self,
        input: Value,
        ctx: &ToolContext,
        effect: Option<&ToolEffectContext>,
        sink: &dyn ToolOutputSink,
    ) -> ToolResult {
        let _ = sink;
        self.execute_with_effect_ctx(input, ctx, effect).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn echo_tool() -> PluginTool {
        PluginTool {
            name: "echo".into(),
            description: "echoes input".into(),
            input_schema: serde_json::json!({ "type": "object" }),
            category: ToolCategory::Info,
            is_deferred: false,
            max_result_size: 1_234,
            execute: Arc::new(|inv: PluginToolInvocation| {
                Box::pin(async move {
                    ToolResult {
                        content: inv.input.to_string(),
                        is_error: false,
                    }
                })
            }),
        }
    }

    #[test]
    fn adapter_echoes_plugin_tool_metadata() {
        let adapter = PluginToolAdapter::new(echo_tool());
        assert_eq!(adapter.name(), "echo");
        assert_eq!(adapter.description(), "echoes input");
        assert_eq!(adapter.category(), ToolCategory::Info);
        assert_eq!(adapter.max_result_size(), 1_234);
        assert!(adapter.supports_streaming());
        assert!(!adapter.is_concurrency_safe(&serde_json::json!({})));
    }

    #[tokio::test]
    async fn adapter_execute_runs_the_closure() {
        let adapter = PluginToolAdapter::new(echo_tool());
        let result = adapter.execute(serde_json::json!({ "x": 1 })).await;
        assert!(!result.is_error);
        assert_eq!(result.content, r#"{"x":1}"#);
    }

    #[tokio::test]
    async fn adapter_execute_with_ctx_runs_the_closure() {
        let adapter = PluginToolAdapter::new(echo_tool());
        let ctx = ToolContext::test_default();
        let result = adapter
            .execute_with_ctx(serde_json::json!({ "y": 2 }), &ctx)
            .await;
        assert!(!result.is_error);
        assert_eq!(result.content, r#"{"y":2}"#);
    }

    #[tokio::test]
    async fn adapter_reuses_exact_durable_identity_and_legacy_path_has_none() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&seen);
        let tool = PluginTool {
            name: "capture".into(),
            description: "captures caps".into(),
            input_schema: serde_json::json!({ "type": "object" }),
            category: ToolCategory::Info,
            is_deferred: false,
            max_result_size: 1_000,
            execute: Arc::new(move |inv: PluginToolInvocation| {
                capture.lock().unwrap().push(inv.caps.effect.clone());
                Box::pin(async {
                    ToolResult {
                        content: "ok".into(),
                        is_error: false,
                    }
                })
            }),
        };
        let adapter = PluginToolAdapter::new(tool);
        let ctx = ToolContext::test_default();
        let effect = ToolEffectContext {
            tool_execution_id: "tool-execution-7".into(),
            idempotency_key: "stable-key-7".into(),
        };

        adapter
            .execute_with_effect_ctx(serde_json::json!({ "same": true }), &ctx, Some(&effect))
            .await;
        adapter
            .execute_with_effect_ctx(serde_json::json!({ "same": true }), &ctx, Some(&effect))
            .await;
        adapter
            .execute_with_ctx(
                serde_json::json!({ "same": true }),
                &ToolContext::test_default(),
            )
            .await;
        adapter.execute(serde_json::json!({ "same": true })).await;

        let expected = Some(PluginToolEffectIdentity::v1(
            "tool-execution-7",
            "stable-key-7",
        ));
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[expected.clone(), expected, None, None]
        );
    }
}
