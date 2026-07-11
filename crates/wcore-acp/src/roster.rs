//! Persona-agent roster seam (persona-profiles Phase A).
//!
//! `wcore-acp` is a mid-layer transport crate and MUST NOT depend on the
//! identity sources (`wcore-agents-pack`, `wcore-agent`, `wcore-config`). The
//! roster is reached through this transport-neutral trait, implemented in the
//! CLI layer exactly like [`crate::turn::TurnEngine`] and
//! [`crate::a2a::A2aHandler`]. Keeps the crate dependency-free while the CLI
//! owns enumeration (`CliAgentRoster` lands in PR-3, NOT here).
//!
//! SECURITY (red-team, hard requirements):
//!   * R4 — [`AgentInfo`] carries ONLY an opaque `id` + display `label`
//!     (+ optional operator-authored `description`). An implementation MUST
//!     NEVER surface a persona's `system_prompt`/SOUL, model, provider, API
//!     key, or filesystem paths through this trait. The type makes that the
//!     only representable shape.
//!   * R3 — the roster is AUTHZ-gated: [`AgentRoster::list`] returns ONLY the
//!     agents the calling principal is authorized to select, and
//!     [`AgentRoster::contains`] answers within that same authorized set.
//!     Until per-principal authz exists, an implementation scopes the roster
//!     to the trusted local operator. "Unknown" and "not authorized" are the
//!     same answer (`false`) so existence never leaks.

use async_trait::async_trait;

use crate::error::AcpError;
use crate::protocol::AgentInfo;

/// A transport-neutral, authorization-gated catalog of selectable persona
/// agents.
///
/// Installed on an [`crate::server::AcpServer`] via
/// [`crate::server::AcpServer::with_roster`]. When no roster is installed the
/// server behaves exactly as before the extension: `agents/list` returns `[]`
/// and any `agent` selector at `session/create` resolves to
/// [`crate::protocol::ErrorCode::AgentNotFound`] (feature default-OFF).
#[async_trait]
pub trait AgentRoster: Send + Sync {
    /// The persona agents the calling principal is AUTHORIZED to select (R3).
    ///
    /// The returned [`AgentInfo`]s expose only id/label/description (R4). An
    /// implementation MUST filter to the authorized set here — callers treat
    /// this as the full visible roster.
    async fn list(&self) -> Result<Vec<AgentInfo>, AcpError>;

    /// Whether `id` is present in the caller's AUTHORIZED roster.
    ///
    /// Default implementation answers from [`Self::list`], so a minimal roster
    /// need only implement `list` and inherits authz-correct membership for
    /// free (an id the principal cannot see is not "contained"). Override when
    /// a direct membership check is cheaper than materializing the list.
    async fn contains(&self, id: &str) -> bool {
        match self.list().await {
            Ok(agents) => agents.iter().any(|a| a.id == id),
            // Fail closed: on any enumeration error the id is treated as not
            // present/authorized rather than silently admitted.
            Err(_) => false,
        }
    }

    /// The id of the default agent for this principal, if the roster defines
    /// one. `None` means "no implicit default" — a `session/create` without an
    /// `agent` selector keeps the server's base behaviour (no persona overlay).
    ///
    /// Default implementation returns `None`.
    async fn default_id(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny in-memory roster used to prove the trait's default methods and
    /// the authz-gated membership semantics. Mirrors the fixed-script mock
    /// style used for `TurnEngine`/`A2aHandler` elsewhere in the crate.
    struct MockRoster {
        agents: Vec<AgentInfo>,
        default: Option<String>,
    }

    #[async_trait]
    impl AgentRoster for MockRoster {
        async fn list(&self) -> Result<Vec<AgentInfo>, AcpError> {
            Ok(self.agents.clone())
        }
        async fn default_id(&self) -> Option<String> {
            self.default.clone()
        }
    }

    fn info(id: &str) -> AgentInfo {
        AgentInfo {
            id: id.to_string(),
            label: id.to_string(),
            description: None,
        }
    }

    #[tokio::test]
    async fn contains_default_answers_from_list() {
        let roster = MockRoster {
            agents: vec![info("architect"), info("researcher")],
            default: None,
        };
        assert!(roster.contains("architect").await);
        assert!(roster.contains("researcher").await);
        // An id outside the authorized list is not contained (R3: unknown ==
        // not-authorized == false, no existence leak).
        assert!(!roster.contains("root").await);
    }

    #[tokio::test]
    async fn default_id_reports_configured_default() {
        let roster = MockRoster {
            agents: vec![info("architect")],
            default: Some("architect".to_string()),
        };
        assert_eq!(roster.default_id().await.as_deref(), Some("architect"));

        let none = MockRoster {
            agents: vec![],
            default: None,
        };
        assert_eq!(none.default_id().await, None);
    }
}
