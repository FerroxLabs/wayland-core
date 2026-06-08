//! v0.6.5 Task 2.7 — Subprocess plugin → `InitializeOutcome` synthesizer.
//!
//! Dep-cycle escape (methodology #24): `wcore-plugin-subprocess` cannot
//! depend on `wcore-agent`, so synthesis lives HERE. Walks the
//! [`LoadedSubprocessPlugin`]'s handshake-supplied tool descriptors and
//! produces a [`CapturedPluginTool`] per entry whose `execute` closure
//! dispatches back into [`LoadedSubprocessPlugin::runner::call_tool`].

use std::sync::Arc;

use wcore_plugin_api::tool::{PluginTool, PluginToolInvocation};
use wcore_plugin_subprocess::LoadedSubprocessPlugin;
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::ToolResult;

use crate::plugins::runner::{CapturedPluginTool, InitializeOutcome};

/// Synthesize an `InitializeOutcome` from a loaded subprocess plugin.
///
/// Holds the runner inside an `Arc` so the closure can outlive this call.
/// The caller must keep the `Arc` alive for the plugin's lifetime —
/// dropping the last reference kills the subprocess via
/// `SubprocessPluginRunner::Drop`.
pub fn synthesize_initialize_outcome_subprocess(
    loaded: Arc<LoadedSubprocessPlugin>,
    plugin_name: &str,
    tool_namespace: &str,
) -> InitializeOutcome {
    let mut outcome = InitializeOutcome::default();
    for tool_desc in &loaded.tools {
        let bare_name = tool_desc.name.clone();
        let fq_name = format!("{}::{}", tool_namespace, bare_name);
        let runner = loaded.clone();
        let plugin_id = plugin_name.to_string();
        let cap_name = bare_name.clone();

        let execute = Arc::new(move |invocation: PluginToolInvocation| {
            let runner = runner.clone();
            let cap_name = cap_name.clone();
            let plugin_id = plugin_id.clone();
            Box::pin(async move {
                match runner.runner.call_tool(&cap_name, invocation.input).await {
                    Ok(out) => ToolResult {
                        content: out.stdout,
                        is_error: out.is_error,
                    },
                    Err(e) => ToolResult {
                        content: format!("subprocess plugin '{plugin_id}' tool '{cap_name}': {e}"),
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
                name: bare_name,
                description: tool_desc.description.clone().unwrap_or_default(),
                input_schema: tool_desc.input_schema.clone(),
                category: ToolCategory::Info,
                is_deferred: false,
                max_result_size: 50_000,
                execute,
            },
        });
    }
    outcome
}

#[cfg(test)]
mod tests {
    // Wave 6A.1 — placeholder removed; the synthesizer now has a real
    // production caller in `bootstrap.rs`.
}
