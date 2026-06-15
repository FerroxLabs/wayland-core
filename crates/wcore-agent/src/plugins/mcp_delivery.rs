//! v0.6.4 Task 1.5 — MCP-server delivery for plugin-supplied servers.
//!
//! Two deliverables:
//!
//! 1. `translate_mcp_server_spec` — pure function mapping a plugin-api
//!    `McpServerSpec` (with its `McpTransport` enum) into the engine's
//!    `wcore_mcp::config::McpServerConfig` / `TransportType`.
//!
//! 2. `connect_plugin_mcp_servers` — standalone async function that runs
//!    the *second* `connect_all` + `register_mcp_tools` pass for
//!    plugin-supplied servers. Returns `None` when the spec list is empty
//!    (no network I/O, no side effects). Task 1.7 calls this from bootstrap
//!    after the first (config-file) MCP pass.
//!
//! **Scope boundary:** this module does NOT touch bootstrap.rs or the
//! existing first MCP pass. It only delivers the translation + the
//! second-pass function that Task 1.7 will wire in.

use std::collections::HashMap;
use std::sync::Arc;

use wcore_mcp::config::{McpServerConfig, TransportType};
use wcore_plugin_api::{McpServerSpec, McpTransport};

// ---------------------------------------------------------------------------
// 1. Translation — pure, no I/O
// ---------------------------------------------------------------------------

/// Translate a plugin-api `McpServerSpec` into the engine's `McpServerConfig`.
///
/// Transport mapping:
/// - `McpTransport::Stdio { command, args }` → `TransportType::Stdio`
///   with `command`, `args`, and `env` populated.
/// - `McpTransport::Sse { url }` → `TransportType::Sse` with `url`.
/// - `McpTransport::Http { url }` → `TransportType::StreamableHttp` with
///   `url` (the engine's HTTP MCP transport is the streamable variant).
///
/// The `deferred` field is left `None` — the engine default (deferred = true)
/// applies, matching the conservative default for plugin-supplied servers.
/// `headers` is left `None`; plugin-api `McpServerSpec` has no header field.
pub fn translate_mcp_server_spec(spec: &McpServerSpec) -> McpServerConfig {
    match &spec.transport {
        McpTransport::Stdio { command, args } => McpServerConfig {
            transport: TransportType::Stdio,
            command: Some(command.clone()),
            args: Some(args.clone()),
            env: if spec.env.is_empty() {
                None
            } else {
                Some(spec.env.clone())
            },
            url: None,
            headers: None,
            deferred: None,
        },
        McpTransport::Sse { url } => McpServerConfig {
            transport: TransportType::Sse,
            command: None,
            args: None,
            env: None,
            url: Some(url.clone()),
            headers: None,
            deferred: None,
        },
        McpTransport::Http { url } => McpServerConfig {
            transport: TransportType::StreamableHttp,
            command: None,
            args: None,
            env: None,
            url: Some(url.clone()),
            headers: None,
            deferred: None,
        },
    }
}

// ---------------------------------------------------------------------------
// 2. Second-pass connector — async, returns Option<Arc<McpManager>>
// ---------------------------------------------------------------------------

/// Run the second `connect_all` + `register_mcp_tools` pass for
/// plugin-supplied MCP servers.
///
/// - Returns `None` immediately when `specs` is empty (no network I/O).
/// - On `connect_all` failure, logs the error and returns `None` — one bad
///   plugin cannot crash boot, matching the "non-fatal" pattern in bootstrap.
/// - On success, registers MCP tools into `tool_registry` via
///   `wcore_mcp::tool_proxy::register_mcp_tools` and returns the
///   `Arc<McpManager>` so Task 1.7 can push it into `mcp_managers`.
///
/// `builtin_names` is the snapshot of built-in tool names taken *before*
/// the first MCP pass (bootstrap.rs:392). It is forwarded to
/// `register_mcp_tools` for collision detection, identical to the first pass.
pub async fn connect_plugin_mcp_servers(
    specs: &[McpServerSpec],
    tool_registry: &mut wcore_tools::registry::ToolRegistry,
    builtin_names: &[String],
) -> Option<Arc<wcore_mcp::manager::McpManager>> {
    if specs.is_empty() {
        return None;
    }

    // Build the HashMap<name, McpServerConfig> that connect_all expects.
    let configs: HashMap<String, McpServerConfig> = specs
        .iter()
        .map(|s| (s.name.clone(), translate_mcp_server_spec(s)))
        .collect();

    // Plugin MCP servers are third-party code from installed marketplace
    // plugins; a broken one must not dominate boot. Give them a tighter
    // connect budget than config-declared servers (which keep the 30s default
    // in `connect_all`) so a wedged plugin server caps the boot delay at 8s
    // instead of 30s. The cause of any skip is still preserved in the
    // manager's health() map (surfaced in /doctor).
    const PLUGIN_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
    match wcore_mcp::manager::McpManager::connect_all_with_connect_timeout(
        &configs,
        PLUGIN_CONNECT_TIMEOUT,
    )
    .await
    {
        Ok(mgr) => {
            let mgr = Arc::new(mgr);
            wcore_mcp::tool_proxy::register_mcp_tools(tool_registry, &mgr, builtin_names, &configs);
            Some(mgr)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "plugin MCP second-pass connect_all failed — skipping plugin MCP servers"
            );
            None
        }
    }
}
