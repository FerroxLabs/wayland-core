//! v0.6.4 Task 2.5: integration test for `PolicyGateAdapter`.
//!
//! Asserts that an `McpServer` built with a real `PolicyGateAdapter`
//! wrapping a configured `wcore_permissions::PolicyEngine`:
//!   (a) **denies** a `tools/call` whose tool name lacks a matching grant,
//!       returning the `POLICY_DENIED` JSON-RPC error code.
//!   (b) **allows** a `tools/call` whose tool name has an explicit
//!       `Permission::Invoke` grant on `Resource::Tool(name)`.
//!
//! No network. No filesystem. Pure in-process JSON-RPC dispatch through
//! `McpServer::handle_request`.
//!
//! Why this test lives in `wcore-cli`: the adapter must reference both
//! `wcore_mcp::PolicyCheck` (transport-layer trait) and
//! `wcore_agent::policy_gate::PolicyGate` (orchestration-layer gate).
//! `wcore-mcp` cannot depend on `wcore-agent` without a dep cycle, so the
//! adapter lives in `wcore-cli` — the only crate that already sees both.

use std::sync::Arc;

use serde_json::json;

use wcore_agent::policy_gate::PolicyGate;
use wcore_cli::policy_gate_adapter::PolicyGateAdapter;
use wcore_mcp::server::error_code;
use wcore_mcp::{McpServer, ServerJsonRpcRequest, ServerToolSpec};
use wcore_permissions::{Action, Actor, Permission, PolicyEngine, Resource};

/// Build an `McpServer` advertising `Read` + `Write`, gated by a
/// `PolicyGate` that **only** grants `Read` to the default actor.
fn server_with_read_only_grant() -> McpServer {
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::User("mcp-serve".into()),
        resource: Resource::Tool("Read".into()),
        action: Action::Invoke,
    });
    let gate = PolicyGate::new(Arc::new(engine), Actor::User("mcp-serve".into()));
    let adapter = PolicyGateAdapter::new(gate);

    let specs = vec![
        ServerToolSpec {
            name: "Read".into(),
            description: "read tool".into(),
            schema_json: json!({"type": "object"}),
        },
        ServerToolSpec {
            name: "Write".into(),
            description: "write tool".into(),
            schema_json: json!({"type": "object"}),
        },
    ];

    McpServer::new(specs, Box::new(adapter))
}

#[tokio::test]
async fn policy_gate_adapter_denies_ungranted_tool() {
    let server = server_with_read_only_grant();

    let req = ServerJsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(1)),
        method: "tools/call".into(),
        params: Some(json!({ "name": "Write", "arguments": {} })),
    };
    let resp = server.handle_request(req).await;

    let err = resp
        .error
        .expect("PolicyGateAdapter must deny ungranted tool with a JSON-RPC error");
    assert_eq!(
        err.code,
        error_code::POLICY_DENIED,
        "expected POLICY_DENIED ({}), got {}: {}",
        error_code::POLICY_DENIED,
        err.code,
        err.message
    );
}

#[tokio::test]
async fn policy_gate_adapter_allows_granted_tool() {
    let server = server_with_read_only_grant();

    let req = ServerJsonRpcRequest {
        jsonrpc: "2.0".into(),
        id: Some(json!(2)),
        method: "tools/call".into(),
        params: Some(json!({ "name": "Read", "arguments": {} })),
    };
    let resp = server.handle_request(req).await;

    // The granted tool must pass the policy gate. `Read` is a stub at the
    // MCP layer (no executor wired in v0.6.4 Task 2.5), so the server
    // returns NOT_IMPLEMENTED rather than POLICY_DENIED — that's the
    // signal we want: the gate let the call through to dispatch.
    if let Some(err) = resp.error {
        assert_ne!(
            err.code,
            error_code::POLICY_DENIED,
            "PolicyGateAdapter wrongly denied a granted tool: {}",
            err.message
        );
    }
}
