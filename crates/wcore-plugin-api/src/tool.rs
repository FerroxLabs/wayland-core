//! `PluginTool` — the plugin-api-native tool contract.
//!
//! A plugin delivers a tool as a [`PluginTool`]: plugin-api-allowed
//! metadata plus an execution closure typed entirely in plugin-api-allowed
//! terms. The host (`wcore-agent`, which CAN depend on `wcore-tools`)
//! wraps each `PluginTool` in a `PluginToolAdapter` to obtain a real
//! `wcore_tools::Tool`. **Nothing in this module names `wcore-tools`** —
//! that crate is in `FORBIDDEN_CORE_IMPORTS` (see `build.rs`).
//!
//! This is the exact spec/data-contract shape `BrowserToolSpec` /
//! `CuaToolSpec` already use, generalized to carry arbitrary tool
//! *behavior* via a closure rather than describing one fixed tool kind.

use std::sync::Arc;

use futures::future::BoxFuture;
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

/// Closure type for the plugin tool execution path.
pub type ExecuteFn =
    Arc<dyn Fn(PluginToolInvocation) -> BoxFuture<'static, ToolResult> + Send + Sync>;

/// Closure type for the streaming chunk emitter.
pub type ChunkFn = Arc<dyn Fn(&str) + Send + Sync>;

/// Closure type for the bounded-progress emitter (pct 0.0..=1.0 + message).
pub type ProgressFn = Arc<dyn Fn(f32, &str) + Send + Sync>;

/// A plugin-supplied tool, expressed entirely in plugin-api-allowed
/// types. The host (`wcore-agent`) wraps this in a `PluginToolAdapter`
/// to obtain a real `wcore_tools::Tool`. NOTHING here names `wcore-tools`.
#[derive(Clone)]
pub struct PluginTool {
    // --- metadata (maps onto Tool::name/description/input_schema/category/...) ---
    /// Bare (pre-namespace) tool name. `ScopedToolRegistry` computes the
    /// fully-qualified `"<namespace>::<name>"` from this; the host adapter
    /// echoes this bare name verbatim from `Tool::name()`.
    pub name: String,
    pub description: String,
    /// JSON Schema for the input. `JsonSchema` is a `wcore-types` alias
    /// for `serde_json::Value` — both allowed deps.
    pub input_schema: JsonSchema,
    /// `ToolCategory` lives in `wcore-protocol` — an allowed dep.
    pub category: ToolCategory,
    /// Maps onto `Tool::is_deferred()`.
    pub is_deferred: bool,
    /// Maps onto `Tool::max_result_size()`.
    pub max_result_size: usize,

    // --- behavior ---
    /// Execution closure. `Fn` (NOT `FnOnce`) — a tool is invoked many
    /// times over a session. `Arc`-wrapped so `PluginTool` stays `Clone`
    /// and the closure can be shared into the adapter. `Send + Sync`
    /// because the adapter is held inside a `Send + Sync` `Tool` object.
    /// Returns a `BoxFuture` (object-safe async without `async-trait`
    /// on a bare closure).
    pub execute: ExecuteFn,
}

impl PluginTool {
    /// Construct a `PluginTool` whose behavior is delivered by the host
    /// (or an MCP server) rather than an in-process closure.
    ///
    /// Browser/CUA plugins claim a tool namespace through
    /// `ScopedToolRegistry` purely for the `NamespaceLedger` duplicate
    /// protection — their real tool is reified host-side from a
    /// `BrowserToolSpec` / `CuaToolSpec`. IJFW's `ijfw_run` /
    /// `ijfw_update_apply` likewise execute via the IJFW MCP server's
    /// tool proxy, not an in-process body. For all of these the
    /// `PluginTool` carries honest metadata and a closure that returns
    /// an error if it is ever invoked directly — the host-side path
    /// supersedes it before that can happen.
    pub fn host_delegated(
        name: impl Into<String>,
        description: impl Into<String>,
        category: ToolCategory,
    ) -> Self {
        let name = name.into();
        let err_name = name.clone();
        Self {
            name,
            description: description.into(),
            input_schema: serde_json::json!({ "type": "object" }),
            category,
            is_deferred: false,
            max_result_size: 50_000, // inert — closure never runs for host-delegated tools
            execute: Arc::new(move |_inv| {
                let n = err_name.clone();
                Box::pin(async move {
                    ToolResult {
                        content: format!(
                            "tool `{n}` is host-delegated and must be invoked \
                             through the host-reified tool path, not the \
                             plugin closure"
                        ),
                        is_error: true,
                    }
                })
            }),
        }
    }
}

/// Everything one tool call receives. Built by the host adapter per
/// invocation; the plugin closure never constructs it.
pub struct PluginToolInvocation {
    /// The LLM-supplied tool input (`serde_json::Value` — allowed).
    pub input: serde_json::Value,
    /// Streaming / progress sink. Host-supplied; a plugin that does not
    /// stream simply ignores it.
    pub emit: PluginToolEmit,
    /// Versioned capability handle (cancellation, call id, agent id).
    pub caps: PluginToolCaps,
}

/// Plugin-api-local mirror of `wcore_tools::ToolOutputSink`. The host
/// adapter wires this to the real `&dyn ToolOutputSink` it is handed.
/// Mirror pattern: same as `BrowserToolSpec` mirroring a forbidden type.
#[derive(Clone)]
pub struct PluginToolEmit {
    /// Host-installed chunk emitter. Boxed closure so the plugin-api
    /// crate need not name the host's sink trait.
    chunk: ChunkFn,
    /// Host-installed bounded-progress emitter (pct 0.0..=1.0 + message).
    progress: ProgressFn,
}

impl PluginToolEmit {
    /// Constructed by the host adapter only.
    pub fn new(chunk: ChunkFn, progress: ProgressFn) -> Self {
        Self { chunk, progress }
    }

    /// Emit one streaming output chunk.
    pub fn chunk(&self, c: &str) {
        (self.chunk)(c)
    }

    /// Emit a bounded-progress signal.
    pub fn progress(&self, pct: f32, message: &str) {
        (self.progress)(pct, message)
    }
}

/// Versioned capability handle. EXPLICIT and versioned so future
/// capabilities (vfs handle, budget view, …) are added as new fields
/// under a bumped `version` without breaking the closure signature.
/// This is a capability *value type*, never a raw host reference.
#[non_exhaustive]
pub struct PluginToolCaps {
    /// Capability-handle schema version. Phase 1 ships `1`. A plugin
    /// MAY check this to feature-detect; the host always sets it.
    pub version: u32,
    /// Cooperative cancellation. `tokio_util::sync::CancellationToken`
    /// is an external crate, allowed by the boundary, and
    /// `ToolContext.cancel` is already that exact type, so no mapping.
    pub cancel: tokio_util::sync::CancellationToken,
    /// Stable in-flight tool-call id (matches `ToolContext.call_id`).
    pub call_id: String,
    /// Originating sub-agent name; `None` = main agent.
    pub source_agent: Option<String>,
}

impl PluginToolCaps {
    /// Construct a Phase-1 (`version = 1`) capability handle. Used by the
    /// host adapter; plugins receive a fully-built `PluginToolCaps`.
    pub fn v1(
        cancel: tokio_util::sync::CancellationToken,
        call_id: impl Into<String>,
        source_agent: Option<String>,
    ) -> Self {
        Self {
            version: 1,
            cancel,
            call_id: call_id.into(),
            source_agent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn host_delegated_tool_carries_metadata_and_errors_on_direct_call() {
        let t = PluginTool::host_delegated("execute", "host-side tool", ToolCategory::Exec);
        assert_eq!(t.name, "execute");
        assert_eq!(t.category, ToolCategory::Exec);
        assert!(!t.is_deferred);

        let inv = PluginToolInvocation {
            input: serde_json::json!({}),
            emit: PluginToolEmit::new(Arc::new(|_| {}), Arc::new(|_, _| {})),
            caps: PluginToolCaps::v1(tokio_util::sync::CancellationToken::new(), "", None),
        };
        let result = (t.execute)(inv).await;
        assert!(result.is_error);
        assert!(result.content.contains("host-delegated"));
    }

    #[tokio::test]
    async fn execute_closure_runs_and_returns_tool_result() {
        let t = PluginTool {
            name: "echo".into(),
            description: "echoes the input back".into(),
            input_schema: serde_json::json!({ "type": "object" }),
            category: ToolCategory::Info,
            is_deferred: false,
            max_result_size: 1_000,
            execute: Arc::new(|inv: PluginToolInvocation| {
                Box::pin(async move {
                    ToolResult {
                        content: inv.input.to_string(),
                        is_error: false,
                    }
                })
            }),
        };
        let inv = PluginToolInvocation {
            input: serde_json::json!({ "k": "v" }),
            emit: PluginToolEmit::new(Arc::new(|_| {}), Arc::new(|_, _| {})),
            caps: PluginToolCaps::v1(tokio_util::sync::CancellationToken::new(), "c1", None),
        };
        let result = (t.execute)(inv).await;
        assert!(!result.is_error);
        assert_eq!(result.content, r#"{"k":"v"}"#);
    }

    #[test]
    fn plugin_tool_is_clone() {
        let t = PluginTool::host_delegated("execute", "d", ToolCategory::Exec);
        let _c = t.clone();
    }
}
