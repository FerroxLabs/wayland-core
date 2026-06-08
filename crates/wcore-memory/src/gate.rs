// M2 — MemoryAccessGate.
//
// Deny-by-default access control across the 5×3 grid.
// - SystemToken: full access (bootstrap, consolidation, user-model writes).
// - MainAgentToken: P1-P4 read+write within valid tier cells; P5 read+write
//   denied (user_model is system-only).
// - SubAgentToken: deny-by-default; AccessPolicy enumerates explicit
//   read/write scopes per agent.

use std::collections::HashMap;
use std::sync::Arc;

use crate::audit::{AuditEntry, AuditLog, now_secs};
use crate::error::{MemoryError, Result};
use crate::tier::TierResolver;
use crate::v2_types::{AccessToken, Partition, Tier};

/// Per-(partition, tier) scope name. Agent YAMLs declare scopes like
/// `project_episodes` (P2 project read).
#[derive(Debug, Clone, Default)]
pub struct AccessPolicy {
    /// (agent_name, op) -> set of (partition, tier) pairs that are allowed.
    /// op is "read" or "write".
    pub allow: HashMap<(String, String), Vec<(Partition, Tier)>>,
}

impl AccessPolicy {
    pub fn empty() -> Self {
        Self::default()
    }

    /// Convenience: grant read access to (partition, tier) for agent.
    pub fn grant_read(&mut self, agent: &str, p: Partition, t: Tier) {
        self.allow
            .entry((agent.to_string(), "read".into()))
            .or_default()
            .push((p, t));
    }

    /// Convenience: grant write access to (partition, tier) for agent.
    pub fn grant_write(&mut self, agent: &str, p: Partition, t: Tier) {
        self.allow
            .entry((agent.to_string(), "write".into()))
            .or_default()
            .push((p, t));
    }
}

pub struct MemoryAccessGate {
    audit: Arc<AuditLog>,
    policy: AccessPolicy,
}

impl MemoryAccessGate {
    pub fn new(audit: Arc<AuditLog>, policy: AccessPolicy) -> Self {
        Self { audit, policy }
    }

    /// Read gate: returns Ok if allowed; otherwise records a denial and
    /// returns AccessDenied.
    pub fn check_read(&self, token: &AccessToken, p: Partition, t: Tier) -> Result<()> {
        // First: invalid (partition, tier) cell is always denied.
        if let Err(e) = TierResolver::validate(p, t) {
            self.record(token, p, t, "read", "deny", "invalid cell");
            return Err(e);
        }

        let allowed = self.read_allowed(token, p, t);
        if allowed {
            self.record(token, p, t, "read", "allow", "policy");
            Ok(())
        } else {
            self.record(token, p, t, "read", "deny", "no read scope");
            Err(MemoryError::AccessDenied {
                partition: p.to_string(),
                tier: t.to_string(),
                reason: "no read scope".into(),
            })
        }
    }

    pub fn check_write(&self, token: &AccessToken, p: Partition, t: Tier) -> Result<()> {
        if let Err(e) = TierResolver::validate(p, t) {
            self.record(token, p, t, "write", "deny", "invalid cell");
            return Err(e);
        }

        // P5 write: SystemToken only.
        if p == Partition::Core && !matches!(token, AccessToken::System) {
            self.record(token, p, t, "write", "deny", "P5 system-only");
            return Err(MemoryError::AccessDenied {
                partition: p.to_string(),
                tier: t.to_string(),
                reason: "P5 user_model requires SystemToken".into(),
            });
        }

        let allowed = self.write_allowed(token, p, t);
        if allowed {
            self.record(token, p, t, "write", "allow", "policy");
            Ok(())
        } else {
            self.record(token, p, t, "write", "deny", "no write scope");
            Err(MemoryError::AccessDenied {
                partition: p.to_string(),
                tier: t.to_string(),
                reason: "no write scope".into(),
            })
        }
    }

    fn read_allowed(&self, token: &AccessToken, p: Partition, t: Tier) -> bool {
        match token {
            AccessToken::System => true,
            AccessToken::MainAgent => p != Partition::Core, // P5 read = system-only
            AccessToken::SubAgent { agent_name } => self
                .policy
                .allow
                .get(&(agent_name.clone(), "read".into()))
                .map(|v| v.contains(&(p, t)))
                .unwrap_or(false),
        }
    }

    fn write_allowed(&self, token: &AccessToken, p: Partition, t: Tier) -> bool {
        match token {
            AccessToken::System => true,
            AccessToken::MainAgent => p != Partition::Core,
            AccessToken::SubAgent { agent_name } => self
                .policy
                .allow
                .get(&(agent_name.clone(), "write".into()))
                .map(|v| v.contains(&(p, t)))
                .unwrap_or(false),
        }
    }

    fn record(
        &self,
        token: &AccessToken,
        p: Partition,
        t: Tier,
        op: &str,
        decision: &str,
        reason: &str,
    ) {
        let entry = AuditEntry {
            ts: now_secs(),
            token_kind: token.kind().to_string(),
            agent_name: token.agent_name().map(|s| s.to_string()),
            partition: p,
            tier: t,
            op: op.to_string(),
            decision: decision.to_string(),
            reason: reason.to_string(),
        };
        // Audit failures shouldn't break the gate decision — log and move on.
        let _ = self.audit.record(entry);
    }

    pub fn audit(&self) -> Arc<AuditLog> {
        self.audit.clone()
    }
}
