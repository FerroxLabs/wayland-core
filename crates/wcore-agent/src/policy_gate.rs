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
//! ## F21-02-03 — how a gate reaches production
//!
//! Through v0.12 the gate was opt-in *and had no opt-in*:
//! `AgentEngine::set_policy_gate` had zero callers workspace-wide, and
//! both production engine constructors hard-coded `policy_gate: None`.
//! The mechanism could not run outside a test, so the "a child cannot
//! widen the parent's tool authority" property was satisfied only by the
//! absence of any enforcement — the exact shape Phase 21 indicts.
//!
//! It is now installed on the **child** side of every spawn, without any
//! new operator knob. `AgentSpawner::execute_resolved_launch` reads the
//! spawner's `ParentToolAuthority` — the shared, narrow-only cell every
//! production seam declares (F21-02-01) — turns that one snapshot into a
//! gate via [`PolicyGate::from_parent_tools`], and installs it on the
//! child engine beside the egress policy it already inherits. A child
//! that requests a tool its parent does not hold is denied at dispatch,
//! no matter what its own `allowed_tools` says.
//!
//! ## Why the authority cell, and not a second one
//!
//! F21-02-03 was originally authored against a separate
//! `Arc<OnceLock<PolicyGate>>` published by `AgentBootstrap::build`. That
//! shape had two defects the reconciliation removes:
//!
//!  1. It was wired at the bootstrap seam ONLY. The transient
//!     (`govern_transient_spawner`) and standalone
//!     (`govern_standalone_spawner`) seams left the cell empty, so five of
//!     the six production spawner construction sites installed no gate at
//!     all — the same guard-at-one-of-five-doors shape that sank the first
//!     attempt at F21-02-01.
//!  2. It derived a second authority from the same registry at the same
//!     line as F21-02-01's, so the two could drift apart under any later
//!     edit that touched one and not the other.
//!
//! Reading `ParentToolAuthority` instead gives the gate all three
//! declaring seams, and `spawner_authority_enumeration`'s coverage, for
//! free — and leaves exactly one answer to "what may this child invoke".
//!
//! ## Layering — this is Layer 2, not a duplicate of Layer 1
//!
//! `build_tool_registry` intersects the same authority at CONSTRUCTION, so
//! a denied tool is never built and never advertised to the child's model.
//! That is the primary control. This gate runs at DISPATCH, ahead of the
//! registry lookup, and therefore also covers tool names that reach the
//! child from anywhere other than `build_tool_registry`. No such source
//! exists today — which is precisely why the layer must be kept: the day
//! one appears (child-side MCP, plugins, a widened table), Layer 1 is
//! bypassed silently and this is the only remaining check. It is
//! fail-closed there on purpose.
//!
//! The parent engine itself is deliberately NOT gated: its registry is
//! already the authority on what it can invoke, and a snapshot would go
//! stale against deferred MCP connection (`wayland#551`), turning a
//! late-registered server's tools into spurious denials.
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

    /// F21-02-03 — the parent session's tool authority, as an inheritable
    /// floor for the children it spawns.
    ///
    /// `names` is one snapshot of the spawner's `ParentToolAuthority` — the
    /// shared, narrow-only cell each production seam declares after every
    /// narrowing has run (`channel_tool_posture`'s `apply_posture`, the
    /// persona `allowed_tools` retain, and any conditional built-in
    /// registration). Reading that cell rather than reconstructing the
    /// declarations keeps ONE source of truth for both child-authority
    /// layers, so a future narrowing composes into the child floor instead
    /// of silently escaping it.
    ///
    /// Takes an iterator rather than a slice because the caller holds a
    /// `BTreeSet` snapshot and must not have to allocate a `Vec` to hand it
    /// over — the snapshot it passes here has to be the SAME value it built
    /// the child registry from.
    ///
    /// One `Invoke` grant per surviving tool, for the same actor the gate
    /// resolves top-level calls to, so a child's dispatch — which passes
    /// `source_agent = None` — is checked against exactly the parent's
    /// set. A tool the parent does not hold is therefore denied to every
    /// descendant, which is the non-widening property Phase 21 requires.
    pub fn from_parent_tools<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let actor = Actor::User("default".into());
        let mut engine = PolicyEngine::new();
        for name in names {
            engine.grant(wcore_permissions::Permission {
                actor: actor.clone(),
                resource: Resource::Tool(name.as_ref().to_owned()),
                action: Action::Invoke,
            });
        }
        Self::new(Arc::new(engine), actor)
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
