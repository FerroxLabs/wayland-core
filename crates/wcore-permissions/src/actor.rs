//! v0.8.0 Task I — actor attribution for the 1.D.3 sub-agent ACL pre-filter.
//!
//! `CallActor` distinguishes the root (user / top-level) session from
//! delegated sub-agents at the tool-dispatch boundary. The runtime uses
//! this to decide whether to apply the [`crate::LearnedPolicy`] pre-filter
//! BEFORE the approval path:
//!
//! - [`CallActor::Root`] — bypasses the sub-agent pre-filter. The
//!   approval path applies as before. This preserves byte-identical
//!   behaviour for every existing call site (the field defaults to
//!   `Root`).
//! - [`CallActor::SubAgent`] — subject to the pre-filter when a
//!   [`LearnedPolicy`] is configured. A deny short-circuits before the
//!   approval path; an allow or "ask" falls through normally.
//!
//! This type is intentionally separate from [`crate::Actor`] (which
//! identifies callers in the explicit-grant ACL `PolicyEngine`). The two
//! solve different problems:
//!
//! - [`crate::Actor`] = identity for `(actor, resource, action)` grants.
//! - [`CallActor`] = caller-class flag for the learned-policy gate.
//!
//! Keeping them separate avoids overloading the existing `Actor` enum
//! and means downstream code that constructs grants does not change.
//!
//! [`LearnedPolicy`]: crate::LearnedPolicy

use serde::{Deserialize, Serialize};

/// Caller class for the sub-agent ACL pre-filter (v0.8.0 task I, 1.D.3).
///
/// `Root` is the default for every constructor that doesn't explicitly
/// set the field — preserving the v0.7.0 dispatch surface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallActor {
    /// The user / top-level session. Bypasses the sub-agent ACL
    /// pre-filter — the approval path applies instead.
    #[default]
    Root,
    /// A delegated sub-agent. Subject to the ACL pre-filter when a
    /// [`crate::LearnedPolicy`] is configured.
    SubAgent {
        /// Unique identifier for this sub-agent instance (e.g.
        /// `worker-3`).
        id: String,
        /// Identifier of the agent that spawned this sub-agent, when
        /// known. `None` for top-level dispatches that spawn directly.
        parent_id: Option<String>,
    },
}

impl CallActor {
    /// True if this call originates from a delegated sub-agent.
    pub fn is_sub_agent(&self) -> bool {
        matches!(self, Self::SubAgent { .. })
    }

    /// The sub-agent id, if any. `None` for `Root`.
    pub fn sub_agent_id(&self) -> Option<&str> {
        match self {
            Self::SubAgent { id, .. } => Some(id),
            Self::Root => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_not_sub_agent() {
        assert!(!CallActor::Root.is_sub_agent());
    }

    #[test]
    fn sub_agent_is_sub_agent() {
        let a = CallActor::SubAgent {
            id: "worker-1".into(),
            parent_id: Some("main".into()),
        };
        assert!(a.is_sub_agent());
    }

    #[test]
    fn default_is_root() {
        assert_eq!(CallActor::default(), CallActor::Root);
    }

    #[test]
    fn sub_agent_id_round_trip() {
        let a = CallActor::SubAgent {
            id: "worker-7".into(),
            parent_id: None,
        };
        assert_eq!(a.sub_agent_id(), Some("worker-7"));
        assert_eq!(CallActor::Root.sub_agent_id(), None);
    }
}
