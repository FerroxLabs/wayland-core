//! v0.6.4 Task 1.5 — MCP server spec translation + second-pass delivery.
//!
//! Tests the pure `translate_mcp_server_spec` function (McpServerSpec →
//! McpServerConfig) and the no-op empty-input path of
//! `connect_plugin_mcp_servers`.

use std::collections::HashMap;

use wcore_agent::plugins::mcp_delivery::{connect_plugin_mcp_servers, translate_mcp_server_spec};
use wcore_config::config::TransportType;
use wcore_plugin_api::{McpServerSpec, McpTransport};

// ---------------------------------------------------------------------------
// translate_mcp_server_spec — transport mapping
// ---------------------------------------------------------------------------

/// Stdio variant maps to TransportType::Stdio with command + args + env.
#[test]
fn translate_stdio_maps_command_args_env() {
    let spec = McpServerSpec {
        name: "my-server".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["--yes".into(), "my-mcp-server".into()],
        },
        env: {
            let mut m = HashMap::new();
            m.insert("API_KEY".into(), "secret".into());
            m
        },
    };

    let cfg = translate_mcp_server_spec(&spec);

    assert_eq!(cfg.transport, TransportType::Stdio);
    assert_eq!(cfg.command.as_deref(), Some("npx"));
    let args = cfg.args.as_ref().expect("args must be populated");
    assert_eq!(args, &["--yes", "my-mcp-server"]);
    let env = cfg.env.as_ref().expect("env must be populated");
    assert_eq!(env.get("API_KEY").map(String::as_str), Some("secret"));
    // url is not relevant for Stdio
    assert!(cfg.url.is_none());
}

/// Stdio with no env produces None or empty env (not a missing field panic).
#[test]
fn translate_stdio_empty_env() {
    let spec = McpServerSpec {
        name: "bare-stdio".into(),
        transport: McpTransport::Stdio {
            command: "echo".into(),
            args: vec![],
        },
        env: HashMap::new(),
    };

    let cfg = translate_mcp_server_spec(&spec);

    assert_eq!(cfg.transport, TransportType::Stdio);
    assert_eq!(cfg.command.as_deref(), Some("echo"));
    assert!(cfg.args.as_deref().unwrap_or(&[]).is_empty());
    // env may be None or Some(empty map) — either is acceptable
    if let Some(env) = &cfg.env {
        assert!(env.is_empty());
    }
}

/// Sse variant maps to TransportType::Sse with url set, command/args None.
#[test]
fn translate_sse_maps_url() {
    let spec = McpServerSpec {
        name: "sse-server".into(),
        transport: McpTransport::Sse {
            url: "https://example.com/mcp/sse".into(),
        },
        env: HashMap::new(),
    };

    let cfg = translate_mcp_server_spec(&spec);

    assert_eq!(cfg.transport, TransportType::Sse);
    assert_eq!(cfg.url.as_deref(), Some("https://example.com/mcp/sse"));
    assert!(cfg.command.is_none());
    assert!(cfg.args.is_none());
}

/// Http variant maps to TransportType::StreamableHttp with url set.
#[test]
fn translate_http_maps_url_to_streamable_http() {
    let spec = McpServerSpec {
        name: "http-server".into(),
        transport: McpTransport::Http {
            url: "https://api.example.com/mcp".into(),
        },
        env: HashMap::new(),
    };

    let cfg = translate_mcp_server_spec(&spec);

    assert_eq!(cfg.transport, TransportType::StreamableHttp);
    assert_eq!(cfg.url.as_deref(), Some("https://api.example.com/mcp"));
    assert!(cfg.command.is_none());
    assert!(cfg.args.is_none());
}

/// `deferred` field is always `None` from translation (callers / bootstrap set
/// it per-server policy; the plugin spec has no deferred flag).
#[test]
fn translate_sets_deferred_none() {
    let spec = McpServerSpec {
        name: "any".into(),
        transport: McpTransport::Stdio {
            command: "true".into(),
            args: vec![],
        },
        env: HashMap::new(),
    };

    let cfg = translate_mcp_server_spec(&spec);

    // deferred=None means the engine's default (true) applies — correct for
    // plugin-supplied servers.
    assert!(cfg.deferred.is_none());
}

// ---------------------------------------------------------------------------
// connect_plugin_mcp_servers — empty-input no-op
// ---------------------------------------------------------------------------

/// When no plugin specs are provided `connect_plugin_mcp_servers` returns
/// immediately without error and produces `None` (no manager spawned).
#[tokio::test]
async fn connect_plugin_mcp_servers_empty_is_noop() {
    use wcore_tools::registry::ToolRegistry;

    let mut registry = ToolRegistry::new();
    let initial_count = registry.tool_names().len();

    let result = connect_plugin_mcp_servers(&[], &mut registry, &[]).await;

    // No MCP specs → no manager → None returned.
    assert!(result.is_none(), "empty specs must return None");
    // Registry untouched.
    assert_eq!(
        registry.tool_names().len(),
        initial_count,
        "registry must not gain tools from an empty spec list"
    );
}
