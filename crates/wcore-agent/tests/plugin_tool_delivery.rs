//! v0.6.4 Task 1.1 — end-to-end proof of the corrected plugin-tool
//! delivery path.
//!
//! Proves the full capture-then-reify wire:
//!   1. a `PluginTool` registered via `ScopedToolRegistry::register_tool`
//!      is captured into `HostToolRegistrar` with its provenance, and
//!   2. a `PluginToolAdapter` wrapping that `PluginTool` exposes it as a
//!      working `wcore_tools::Tool` — `name()` echoes `PluginTool.name`
//!      verbatim and `Tool::execute` runs the closure and returns its
//!      `ToolResult`.
//!
//! `wcore-agent` may name both `PluginTool` (plugin-api) and
//! `wcore_tools::Tool` — the adapter is the only legal bridge.

use std::sync::{Arc, Mutex};

use wcore_agent::plugins::adapters::tool_registrar::HostToolRegistrar;
use wcore_agent::plugins::{CapturedPluginTool, PluginToolAdapter, ReifiedTool};
use wcore_plugin_api::PluginManifest;
use wcore_plugin_api::registry::tools::ScopedToolRegistry;
use wcore_plugin_api::tool::{PluginTool, PluginToolInvocation};
use wcore_protocol::events::ToolCategory;
use wcore_tools::{Tool, ToolOutputSink};
use wcore_types::tool::ToolResult;

/// A plugin manifest claiming the `ijfw` tool namespace.
fn tool_manifest() -> PluginManifest {
    PluginManifest::from_toml_str(
        r#"
[plugin]
name = "wayland-toolful"
version = "1.0.0"
description = "delivers a real tool"
entry = "builtin:t"
authors = ["t"]
license = "MIT"
[permissions]
register_tools = true
tool_namespace = "ijfw"
"#,
    )
    .expect("manifest parses")
}

/// A `PluginTool` with a real execution closure that uppercases its
/// `text` input field — a behavior the test can observe end-to-end.
fn shouting_tool() -> PluginTool {
    PluginTool {
        name: "shout".into(),
        description: "uppercases the `text` input field".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"],
        }),
        category: ToolCategory::Info,
        is_deferred: false,
        max_result_size: 4_096,
        execute: Arc::new(|inv: PluginToolInvocation| {
            Box::pin(async move {
                let text = inv
                    .input
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_uppercase();
                ToolResult {
                    content: text,
                    is_error: false,
                }
            })
        }),
    }
}

/// A capturing `ToolOutputSink` that records every chunk emitted.
#[derive(Default, Clone)]
struct CaptureSink {
    chunks: Arc<Mutex<Vec<String>>>,
}

impl ToolOutputSink for CaptureSink {
    fn emit_chunk(&self, chunk: &str) {
        self.chunks.lock().unwrap().push(chunk.to_owned());
    }
}

/// A `PluginTool` that emits one chunk per call and returns a summary.
fn chunking_tool() -> PluginTool {
    PluginTool {
        name: "chunker".into(),
        description: "emits a streaming chunk then returns ok".into(),
        input_schema: serde_json::json!({ "type": "object" }),
        category: ToolCategory::Info,
        is_deferred: false,
        max_result_size: 4_096,
        execute: Arc::new(|inv: PluginToolInvocation| {
            Box::pin(async move {
                inv.emit.chunk("hello from plugin");
                ToolResult {
                    content: "done".into(),
                    is_error: false,
                }
            })
        }),
    }
}

/// Verifies that `execute_streaming` (no ctx) wires the passed sink so
/// streaming chunks emitted by the plugin closure reach the caller.
/// This is the regression test for the Fix 2 bug: the default impl fell
/// through to `execute()` which built a `NullToolOutputSink`, silently
/// dropping all chunks even though `supports_streaming()` returns `true`.
#[tokio::test]
async fn execute_streaming_delivers_chunks_to_no_ctx_sink() {
    let adapter = PluginToolAdapter::new(chunking_tool());
    assert!(
        adapter.supports_streaming(),
        "adapter must claim streaming support"
    );

    let sink = CaptureSink::default();
    let result = adapter
        .execute_streaming(serde_json::json!({}), &sink)
        .await;

    assert!(!result.is_error);
    assert_eq!(result.content, "done");

    let captured = sink.chunks.lock().unwrap();
    assert_eq!(
        *captured,
        vec!["hello from plugin"],
        "chunk must reach the sink via execute_streaming (no ctx)"
    );
}

#[tokio::test]
async fn plugin_tool_is_captured_then_reified_into_a_working_tool() {
    let manifest = tool_manifest();

    // --- Stage 1: register through ScopedToolRegistry → HostToolRegistrar.
    let mut host = HostToolRegistrar::default();
    {
        let mut capture = host.capture_for_plugin("wayland-toolful");
        let mut scoped =
            ScopedToolRegistry::new(&manifest, &mut capture).expect("scoped registry builds");
        scoped
            .register_tool(shouting_tool())
            .expect("register_tool succeeds");
    }

    // The host captured the (plugin, fq_name, PluginTool) triple.
    assert_eq!(host.registered.len(), 1, "exactly one tool captured");
    let (plugin, fq_name, _tool) = &host.registered[0];
    assert_eq!(plugin, "wayland-toolful", "provenance plugin name stamped");
    assert_eq!(
        fq_name, "ijfw::shout",
        "ScopedToolRegistry computes the FQ name from tool.name"
    );

    // --- Stage 2: model the InitializeOutcome.tools surface — each
    // capture becomes a CapturedPluginTool (still data, not a Tool).
    let captured: Vec<CapturedPluginTool> = std::mem::take(&mut host.registered)
        .into_iter()
        .map(|(plugin, fq_name, tool)| CapturedPluginTool {
            plugin,
            fq_name,
            tool,
        })
        .collect();
    assert_eq!(captured.len(), 1);
    let captured = captured.into_iter().next().unwrap();
    assert_eq!(captured.plugin, "wayland-toolful");
    assert_eq!(captured.fq_name, "ijfw::shout");
    // The bare name on the PluginTool is the pre-namespace name.
    assert_eq!(captured.tool.name, "shout");

    // --- Stage 3: reify — wrap the PluginTool in a PluginToolAdapter to
    // obtain a real wcore_tools::Tool.
    let plugin_name = captured.tool.name.clone();
    let adapter = PluginToolAdapter::new(captured.tool.clone());

    // The adapter echoes PluginTool.name verbatim (Task 1.1 contract).
    assert_eq!(
        adapter.name(),
        plugin_name,
        "PluginToolAdapter::name() == PluginTool.name"
    );
    assert_eq!(adapter.name(), "shout");
    assert_eq!(adapter.category(), ToolCategory::Info);
    assert_eq!(adapter.max_result_size(), 4_096);

    // Tool::execute runs the closure and returns its ToolResult.
    let result = adapter
        .execute(serde_json::json!({ "text": "hello" }))
        .await;
    assert!(!result.is_error);
    assert_eq!(result.content, "HELLO", "the closure body ran");

    // --- Stage 4: the ReifiedTool helper produces a Box<dyn Tool> too.
    let reified = ReifiedTool::from_captured(captured);
    assert_eq!(reified.plugin, "wayland-toolful");
    assert_eq!(reified.fq_name, "ijfw::shout");
    let boxed: Box<dyn Tool> = reified.tool;
    assert_eq!(boxed.name(), "shout");
    let result = boxed
        .execute(serde_json::json!({ "text": "wayland" }))
        .await;
    assert_eq!(result.content, "WAYLAND");
}
