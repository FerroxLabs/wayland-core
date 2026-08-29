//! #998 c6 — the ACP backend's MCP surface, and the per-tool switches acting
//! on it.
//!
//! Core-side enforcement of the operator's per-tool selection has existed since
//! #998 c1-c4, but ONLY where a selection could reach it: the config file and
//! the json-stream `AddMcpServer` command. `wcore-acp` had no MCP surface at
//! all — no route, no type, no field — so the Desktop MCP Library's switches
//! were inert against every ACP-backed session. An operator who switched a
//! destructive tool off still believed they had disabled it.
//!
//! These cases grade the WIRING end to end, the way `acp_tool_selection.rs`
//! does for the builtin selection: a real MCP server is dialled, a real engine
//! is built, and the assertion is read off the `tools[]` array of the request
//! that actually reached the provider — the only place "what the model was
//! offered" is observable. A denied tool is absent from the REGISTRY too, so a
//! hallucinated call cannot reach it either.

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::StreamExt;
use serde_json::json;
use support::mock_llm::{MockLlm, received_requests};
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

use wcore_acp::protocol::{McpToolSelection, SessionCreateRequest, MessageSendRequest};
use wcore_acp::server::AcpServer;
use wcore_acp::transport::http::HttpHandler;
use wcore_acp::turn::{TurnEngine, TurnRequest};
use wcore_cli::acp_engine::EngineTurnEngine;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{CliArgs, Config, McpServerConfig, TransportType};
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;

/// The MCP server the operator configured. It advertises one harmless tool and
/// one destructive one — the shape the MCP Library's switches exist for.
const SAFE_TOOL: &str = "safe_read";
const DANGER_TOOL: &str = "danger_delete";

async fn two_tool_mcp_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("\"method\":\"initialize\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "library", "version": "1.0.0"}
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains(
            "\"method\":\"notifications/initialized\"",
        ))
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("\"method\":\"tools/list\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": 10,
            "result": {"tools": [
                {
                    "name": SAFE_TOOL,
                    "description": "read a document",
                    "inputSchema": {"type": "object", "properties": {}}
                },
                {
                    "name": DANGER_TOOL,
                    "description": "delete everything",
                    "inputSchema": {"type": "object", "properties": {}}
                }
            ]}
        })))
        .mount(&server)
        .await;
    server
}

fn test_config(mcp_url: &str, configured_allowlist: Option<Vec<String>>) -> Config {
    let mut config = Config::resolve(&CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("sk-ant-harness-not-real-key".to_string()),
        base_url: None,
        model: Some("claude-mock".to_string()),
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: true,
        project_dir: None,
    })
    .expect("resolve a default config");
    // Hermetic, for the same reason `acp_tool_selection.rs` says: `Config::resolve`
    // reads the invoking account's real config.toml, and durable sessions are
    // irrelevant to what these cases measure.
    config.session.enabled = false;
    // Layer D1 cold-deferral folds every non-hot tool - MCP tools included -
    // into ONE name-only catalog line inside ToolSearch's description, so an
    // MCP tool never appears as its own entry in `tools[]` and the array could
    // not tell "denied" apart from "deferred". Turn it off so the array is a
    // direct readout of the registry. It does not weaken the claim: a denied
    // tool is absent from the REGISTRY, which is upstream of both.
    config.builtin_tools.defer_cold.enabled = false;
    // The operator's own MCP declaration. This is the ONLY place a server is
    // ever declared — the ACP selection can narrow it and nothing else.
    let mut servers = HashMap::new();
    servers.insert(
        "library".to_string(),
        McpServerConfig {
            transport: TransportType::StreamableHttp,
            command: None,
            args: None,
            env: None,
            url: Some(mcp_url.to_string()),
            headers: None,
            deferred: Some(false),
            allow_local: true,
            only_for_assistant: None,
            allowed_tools: configured_allowlist,
        },
    );
    config.mcp.servers = servers;
    config
}

fn provider_against(base_url: &str) -> Arc<dyn LlmProvider> {
    Arc::new(
        AnthropicProvider::new(
            "sk-ant-harness-not-real-key",
            base_url,
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        )
        .with_cache(false),
    )
}

fn cwd() -> String {
    std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string()
}

/// The names the outgoing provider request actually offered the model.
async fn offered_tool_names(server: &MockServer) -> Vec<String> {
    let requests = received_requests(server).await;
    let first = requests
        .first()
        .expect("the turn must have reached the provider");
    first
        .body
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn offers(names: &[String], tool: &str) -> bool {
    names.iter().any(|n| n.contains(tool))
}

/// Drive one ACP turn through the engine bridge with the given selection and
/// return what the model was offered.
async fn offered_with(
    session_id: &str,
    configured_allowlist: Option<Vec<String>>,
    selection: Vec<McpToolSelection>,
) -> Vec<String> {
    let mcp = two_tool_mcp_server().await;
    let llm = MockLlm::new().text("ok").start().await;
    let engine = EngineTurnEngine::with_provider(
        test_config(&mcp.uri(), configured_allowlist),
        cwd(),
        provider_against(&llm.uri()),
    );
    let stream = engine
        .run_turn(TurnRequest {
            session_id: session_id.to_string(),
            text: "hi".to_string(),
            tools: Vec::new(),
            agent: None,
            mcp_servers: selection,
        })
        .await
        .expect("run_turn establishes a stream");
    let _: Vec<_> = stream.collect().await;
    offered_tool_names(&llm).await
}

/// CONTROL. Without any selection the operator's configured server contributes
/// BOTH tools. Every assertion below is a claim that something disappeared, and
/// none of them mean anything unless this passes.
#[tokio::test]
async fn control_an_unselected_server_contributes_every_tool() {
    let offered = offered_with(
        "99999999-1111-2222-3333-000000000001",
        None,
        Vec::new(),
    )
    .await;
    assert!(
        offers(&offered, SAFE_TOOL) && offers(&offered, DANGER_TOOL),
        "with no selection the configured MCP server must contribute both of \
         its tools; offered: {offered:?}"
    );
}

/// THE criterion. A tool the operator switched OFF in the MCP Library, sent
/// over ACP, is not offered to the model.
#[tokio::test]
async fn a_switched_off_mcp_tool_is_not_offered_over_acp() {
    let offered = offered_with(
        "99999999-1111-2222-3333-000000000002",
        None,
        vec![McpToolSelection {
            server: "library".to_string(),
            allowed_tools: Some(vec![SAFE_TOOL.to_string()]),
        }],
    )
    .await;
    assert!(
        offers(&offered, SAFE_TOOL),
        "the tool left ON must still be offered; offered: {offered:?}"
    );
    assert!(
        !offers(&offered, DANGER_TOOL),
        "the tool the operator switched OFF must not be offered; offered: {offered:?}"
    );
}

/// "Disable all" is a distinct state from "no selection", and it must survive
/// the whole path. Folding the two together would enable every tool at exactly
/// the moment the operator asked for none.
#[tokio::test]
async fn an_empty_selection_disables_every_tool_on_that_server() {
    let offered = offered_with(
        "99999999-1111-2222-3333-000000000003",
        None,
        vec![McpToolSelection {
            server: "library".to_string(),
            allowed_tools: Some(Vec::new()),
        }],
    )
    .await;
    assert!(
        !offers(&offered, SAFE_TOOL) && !offers(&offered, DANGER_TOOL),
        "an empty selection must disable the server's tools entirely; offered: {offered:?}"
    );
    // ...and only that server's. The builtin registry is untouched.
    assert!(
        offers(&offered, "Read"),
        "disabling an MCP server must not disturb the builtin tools; offered: {offered:?}"
    );
}

/// SECURITY. The selection is authority-REDUCING and nothing else: a client
/// cannot use it to hand itself a tool the operator's own config withheld.
#[tokio::test]
async fn a_selection_can_never_widen_what_the_config_allowed() {
    let offered = offered_with(
        "99999999-1111-2222-3333-000000000004",
        // The operator's config already allows ONLY the safe tool.
        Some(vec![SAFE_TOOL.to_string()]),
        // The client asks for the destructive one.
        vec![McpToolSelection {
            server: "library".to_string(),
            allowed_tools: Some(vec![DANGER_TOOL.to_string()]),
        }],
    )
    .await;
    assert!(
        !offers(&offered, DANGER_TOOL),
        "a client selection must never re-enable a tool the config withheld; \
         offered: {offered:?}"
    );
    assert!(
        !offers(&offered, SAFE_TOOL),
        "and the intersection of two disjoint narrowings is empty, not either \
         side; offered: {offered:?}"
    );
}

/// Naming a server the host has not configured is fail-SAFE: it cannot make a
/// server appear, and it must not disturb the ones that do exist.
#[tokio::test]
async fn a_selection_for_an_unconfigured_server_is_inert() {
    let offered = offered_with(
        "99999999-1111-2222-3333-000000000005",
        None,
        vec![McpToolSelection {
            server: "not-configured-anywhere".to_string(),
            allowed_tools: Some(vec!["whatever".to_string()]),
        }],
    )
    .await;
    assert!(
        offers(&offered, SAFE_TOOL) && offers(&offered, DANGER_TOOL),
        "a selection naming an unknown server must leave the configured one \
         alone; offered: {offered:?}"
    );
    assert!(
        !offers(&offered, "whatever"),
        "and it must never conjure a server into existence; offered: {offered:?}"
    );
}

/// THE SURFACE, end to end. The switch travels on `session/create`, is stored
/// on the session record, is read from THERE on `message/send`, and reaches the
/// engine — which is the whole of what c6 says was missing.
#[tokio::test]
async fn the_switch_travels_from_session_create_to_the_engine() {
    let mcp = two_tool_mcp_server().await;
    let llm = MockLlm::new().text("ok").start().await;
    let server = AcpServer::new().with_turn_engine(Arc::new(EngineTurnEngine::with_provider(
        test_config(&mcp.uri(), None),
        cwd(),
        provider_against(&llm.uri()),
    )));

    // The capability handshake first — a client is required to consult it
    // before sending the key, so a build that does not advertise it must not
    // be relied on.
    let init = server.initialize().await.expect("initialize");
    assert!(
        init.capabilities.mcp_tool_selection,
        "the server must advertise that it applies the selection"
    );

    let created = server
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            mcp_servers: vec![McpToolSelection {
                server: "library".to_string(),
                allowed_tools: Some(vec![SAFE_TOOL.to_string()]),
            }],
        })
        .await
        .expect("create_session");

    // The per-message body carries NO MCP field at all, so this turn's
    // narrowing can only have come from the session record.
    let stream = server
        .send_message(MessageSendRequest {
            session_id: created.session_id.clone(),
            text: "hi".to_string(),
            tools: Vec::new(),
        })
        .await
        .expect("send_message");
    let _: Vec<_> = stream.collect().await;

    let offered = offered_tool_names(&llm).await;
    assert!(
        offers(&offered, SAFE_TOOL),
        "the surviving tool must still be offered; offered: {offered:?}"
    );
    assert!(
        !offers(&offered, DANGER_TOOL),
        "the switch set at session/create must reach the engine; offered: {offered:?}"
    );
}
