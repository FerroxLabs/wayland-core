//! v0.6.5 Task 2.7 — WASM plugin → `InitializeOutcome` synthesizer.
//!
//! Translates a [`LoadedWasmPlugin`] into the same [`InitializeOutcome`]
//! shape the existing static-plugin discovery path produces, so the
//! downstream `apply_initialize_outcome` pipeline does not branch on
//! plugin runtime. This is the dep-cycle escape per methodology #24:
//! `wcore-plugin-wasm` cannot depend on `wcore-agent` (would cycle), so
//! the synthesis happens HERE inside wcore-agent.
//!
//! ## Surface
//!
//! [`synthesize_initialize_outcome_wasm`] walks the loaded plugin's
//! cached metadata list (populated by the loader after invoking the
//! component's `metadata` export) and wraps each entry in a
//! [`CapturedPluginTool`] whose `execute` closure dispatches back into
//! [`LoadedWasmPlugin::call_tool`].

use std::sync::Arc;

use wcore_plugin_api::tool::{PluginTool, PluginToolInvocation};
use wcore_plugin_wasm::{LoadedWasmPlugin, PluginToolCaps as WasmPluginToolCaps};
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::ToolResult;

use crate::plugins::runner::{CapturedPluginTool, InitializeOutcome};

/// Build an `InitializeOutcome` from a loaded WASM plugin's tool list.
///
/// `plugin_name` is the manifest-declared name (matches
/// [`LoadedWasmPlugin::name`]); `tool_namespace` is the namespace under
/// which `<namespace>::<name>` is computed (typically the plugin name
/// itself unless the host overrides it).
pub fn synthesize_initialize_outcome_wasm(
    loaded: Arc<LoadedWasmPlugin>,
    plugin_name: &str,
    tool_namespace: &str,
    tools: Vec<wcore_plugin_wasm::WasmToolMetadata>,
) -> InitializeOutcome {
    let mut outcome = InitializeOutcome::default();
    for meta in tools {
        let fq_name = format!("{}::{}", tool_namespace, meta.name);
        let tool_name = meta.name.clone();
        let runner = loaded.clone();
        let plugin_id = plugin_name.to_string();

        // PluginTool::execute is `Arc<Fn(PluginToolInvocation) -> BoxFuture<ToolResult>>`
        // — single arg, returns ToolResult (NOT Result<_, _>). Tool-level
        // errors flip `ToolResult::is_error = true`; host-level errors
        // (cancellation, trap, leak) become `is_error = true` content.
        let execute = Arc::new(move |invocation: PluginToolInvocation| {
            let runner = runner.clone();
            let tool_name = tool_name.clone();
            let plugin_id = plugin_id.clone();
            Box::pin(async move {
                let input_string = match serde_json::to_string(&invocation.input) {
                    Ok(s) => s,
                    Err(e) => {
                        return ToolResult {
                            content: format!(
                                "wasm plugin '{plugin_id}' tool '{tool_name}': \
                                 input serialize failed: {e}"
                            ),
                            is_error: true,
                        };
                    }
                };
                // Caps from PluginToolInvocation. The wasm runner uses
                // its own decoupled PluginToolCaps shape (no tokio_util
                // CancellationToken at its public seam).
                let wasm_caps = WasmPluginToolCaps {
                    call_id: invocation.caps.call_id.clone(),
                    source_agent: invocation.caps.source_agent.clone(),
                };
                match runner.call_tool(&tool_name, &input_string, wasm_caps).await {
                    Ok(out) => ToolResult {
                        content: out.stdout,
                        is_error: out.is_error,
                    },
                    Err(e) => ToolResult {
                        content: format!(
                            "wasm plugin '{plugin_id}' tool '{tool_name}': host error: {e}"
                        ),
                        is_error: true,
                    },
                }
            })
                as std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send>>
        });

        outcome.tools.push(CapturedPluginTool {
            plugin: plugin_name.to_string(),
            fq_name,
            tool: PluginTool {
                name: meta.name,
                description: meta.description,
                input_schema: serde_json::from_str(&meta.input_schema)
                    .unwrap_or_else(|_| serde_json::json!({ "type": "object" })),
                category: ToolCategory::Info,
                is_deferred: meta.is_deferred,
                max_result_size: meta.max_result_size as usize,
                execute,
            },
        });
    }
    outcome
}

#[cfg(test)]
mod tests {
    // Wave 6A.1 — the prior "surface compiles" placeholder was deleted
    // when `bootstrap.rs` wired `synthesize_initialize_outcome_wasm` into
    // the on-disk discovery path. End-to-end coverage now lives in
    // `tests/plugin_hybrid_end_to_end.rs`.
}
