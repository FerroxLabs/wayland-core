//! v0.6.1 hardening (CRIT-1) — gate tool dispatch through the M5.8
//! `PolicyEngine`.
//!
//! v0.6.0 shipped `wcore-permissions` as orphan code: the crate compiled,
//! tests passed in isolation, but **no consumer in the engine called
//! `PolicyEngine::check`**. ACL grants existed only on paper. v0.6.1
//! installs this gate at the orchestration boundary so a configured
//! `PolicyEngine` is consulted before any tool runs.
//!
//! ## Backwards compatibility
//!
//! `PolicyGate` is **opt-in**. Sessions that do not configure one see
//! identical behaviour to v0.6.0 — every tool runs. The dispatch path
//! treats `Option<&PolicyGate>` as the canonical "is there a policy"
//! signal, so the cost on the unconfigured fast path is one `Option`
//! match per tool call.
//!
//! ## Actor resolution
//!
//! Top-level (main-agent) tool calls use the gate's configured
//! [`Actor`]. Sub-agent calls (where the orchestration layer knows the
//! spawning agent's name) use `Actor::Agent(name)` so a single
//! `PolicyEngine` can grant the main user tools the sub-agents do not
//! get. v0.6.1 keeps actor resolution simple — `Actor::System` (the
//! engine's free bypass) is intentionally not exposed here; tool
//! dispatches are never `System`.
//!
//! ## Threats closed by wiring this in
//!
//! - **T5** (tool path traversal) — `PolicyEngine::check` consults the
//!   already-implemented glob-deny logic.
//! - **T6** (debug leakage of grants) — `PolicyEngine`'s `Debug`
//!   redaction is now reachable by tool-trace consumers.
//! - **T7** (grant audit) — `set_audit_sink` events fire whenever a
//!   grant is added through the live engine.
//! - **T2** (token replay) and the bearer-token revocation path live
//!   one layer up at the session boundary, not here.

use std::sync::Arc;

use wcore_permissions::{Action, Actor, PolicyEngine, PolicyResult, Resource};

/// Wraps a [`PolicyEngine`] with the actor identity for a session.
///
/// Cheap to clone — the underlying `PolicyEngine` is shared by `Arc`
/// and the actor is a small enum.
#[derive(Debug, Clone)]
pub struct PolicyGate {
    engine: Arc<PolicyEngine>,
    /// Identity used when the dispatch path has no sub-agent name. For
    /// CLI sessions this is typically `Actor::User("default")`; hosts
    /// that surface real user identities set it to `Actor::User(name)`.
    default_actor: Actor,
}

impl PolicyGate {
    /// Construct a gate from a shared engine + default actor.
    pub fn new(engine: Arc<PolicyEngine>, default_actor: Actor) -> Self {
        Self {
            engine,
            default_actor,
        }
    }

    /// Check whether the dispatching actor may invoke `tool_name`.
    ///
    /// `source_agent = Some(name)` when the call comes from a spawned
    /// sub-agent; the gate uses `Actor::Agent(name)` in that case so
    /// the grant table can distinguish sub-agent capability from main
    /// agent capability. `None` falls back to the gate's default actor.
    pub fn check_tool(&self, tool_name: &str, source_agent: Option<&str>) -> PolicyResult<()> {
        let actor = match source_agent {
            Some(name) => Actor::Agent(name.to_owned()),
            None => self.default_actor.clone(),
        };
        self.engine.check(
            &actor,
            &Resource::Tool(tool_name.to_owned()),
            Action::Invoke,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_permissions::Permission;

    fn gate_with_grants(grants: Vec<Permission>) -> PolicyGate {
        let mut engine = PolicyEngine::new();
        for g in grants {
            engine.grant(g);
        }
        PolicyGate::new(Arc::new(engine), Actor::User("default".into()))
    }

    #[test]
    fn empty_engine_denies_main_agent() {
        let gate = gate_with_grants(vec![]);
        assert!(gate.check_tool("Read", None).is_err());
    }

    #[test]
    fn explicit_grant_allows_main_agent() {
        let gate = gate_with_grants(vec![Permission {
            actor: Actor::User("default".into()),
            resource: Resource::Tool("Read".into()),
            action: Action::Invoke,
        }]);
        assert!(gate.check_tool("Read", None).is_ok());
        assert!(
            gate.check_tool("Write", None).is_err(),
            "grant for Read must not implicitly cover Write"
        );
    }

    #[test]
    fn sub_agent_uses_agent_actor_not_default() {
        // Grant Read to main agent only; sub-agent named "worker" must
        // be denied unless it has its own grant.
        let gate = gate_with_grants(vec![Permission {
            actor: Actor::User("default".into()),
            resource: Resource::Tool("Read".into()),
            action: Action::Invoke,
        }]);
        assert!(gate.check_tool("Read", Some("worker")).is_err());
    }

    #[test]
    fn sub_agent_grant_allows_named_agent_only() {
        let gate = gate_with_grants(vec![Permission {
            actor: Actor::Agent("worker".into()),
            resource: Resource::Tool("Read".into()),
            action: Action::Invoke,
        }]);
        assert!(gate.check_tool("Read", Some("worker")).is_ok());
        assert!(
            gate.check_tool("Read", Some("other")).is_err(),
            "grant to worker must not transfer to other agents"
        );
        assert!(
            gate.check_tool("Read", None).is_err(),
            "grant to sub-agent must not transfer to main agent"
        );
    }
}
