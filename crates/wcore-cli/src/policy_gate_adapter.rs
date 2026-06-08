//! v0.6.4 Task 2.5: bridge `wcore_mcp::PolicyCheck` ↔ `wcore_agent::PolicyGate`.
//!
//! `wcore-mcp` lives below `wcore-agent` in the workspace dep graph, so it
//! cannot depend on the orchestration-layer [`PolicyGate`] directly without
//! introducing a cycle. The transport crate solves this by exposing a
//! minimal [`PolicyCheck`] trait; this adapter — living one layer up in
//! `wcore-cli`, the only crate that already sees both — wraps a
//! `PolicyGate` and implements the trait.
//!
//! ## Why `Resource::Tool`, not `Resource::McpServer`
//!
//! v0.6.4 Task 2.5 gates **the tool name** the MCP client is asking us to
//! invoke, exactly mirroring the in-process gate in `wcore_agent::engine`.
//! Per-MCP-server gating (one `Resource::McpServer("name")` grant gating
//! the entire server) is a strictly coarser policy and can be layered at
//! the transport-acceptance boundary in a later task. Routing the tool
//! name through `PolicyGate::check_tool` keeps the in-process and
//! over-the-wire authorization stories identical, so a tool that the
//! engine denies internally is also denied over MCP.
//!
//! ## Backwards compatibility
//!
//! [`wcore_mcp::AllowAll`] stays in place for tests and standalone
//! scenarios. The `wayland-core mcp-serve` subcommand swaps it for a
//! `PolicyGateAdapter` so the over-the-wire surface respects the same ACL
//! as the in-process engine.

use wcore_agent::policy_gate::PolicyGate;
use wcore_mcp::PolicyCheck;

/// Adapter implementing [`PolicyCheck`] on top of a workspace [`PolicyGate`].
///
/// Calls flow as:
///   `PolicyCheck::check_tool(name)`
///     → `PolicyGate::check_tool(name, None)`
///     → `PolicyEngine::check(default_actor, Resource::Tool(name), Action::Invoke)`
///
/// `source_agent` is hard-coded to `None` here because the MCP server has
/// no notion of a "spawning sub-agent" — every incoming `tools/call` is
/// attributed to the gate's configured default actor (typically
/// `Actor::User("mcp-serve")` for the subcommand path).
#[derive(Debug, Clone)]
pub struct PolicyGateAdapter {
    gate: PolicyGate,
}

impl PolicyGateAdapter {
    /// Wrap a configured [`PolicyGate`] for use with `McpServer`.
    pub fn new(gate: PolicyGate) -> Self {
        Self { gate }
    }
}

impl PolicyCheck for PolicyGateAdapter {
    fn check_tool(&self, name: &str) -> bool {
        // MCP `tools/call` has no sub-agent attribution — every request is
        // the configured default actor. `is_ok()` collapses the deny
        // reasons into a boolean per the `PolicyCheck` trait contract.
        self.gate.check_tool(name, None).is_ok()
    }
}

#[cfg(test)]
mod tests {
    //! Unit coverage for the adapter glue. Integration coverage that drives
    //! a real `McpServer` lives in `tests/policy_gate_adapter.rs`.

    use std::sync::Arc;

    use super::*;
    use wcore_permissions::{Action, Actor, Permission, PolicyEngine, Resource};

    fn adapter_with_grants(grants: Vec<Permission>) -> PolicyGateAdapter {
        let mut engine = PolicyEngine::new();
        for g in grants {
            engine.grant(g);
        }
        PolicyGateAdapter::new(PolicyGate::new(
            Arc::new(engine),
            Actor::User("mcp-serve".into()),
        ))
    }

    #[test]
    fn empty_engine_denies_every_tool() {
        let adapter = adapter_with_grants(vec![]);
        assert!(!adapter.check_tool("Read"));
        assert!(!adapter.check_tool("Write"));
    }

    #[test]
    fn explicit_grant_allows_only_that_tool() {
        let adapter = adapter_with_grants(vec![Permission {
            actor: Actor::User("mcp-serve".into()),
            resource: Resource::Tool("Read".into()),
            action: Action::Invoke,
        }]);
        assert!(adapter.check_tool("Read"));
        assert!(
            !adapter.check_tool("Write"),
            "grant for Read must not implicitly cover Write"
        );
    }
}
