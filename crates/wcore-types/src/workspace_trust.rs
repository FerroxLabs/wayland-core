//! Workspace trust and authority-precedence vocabulary.
//!
//! Repository content is data until an independently-owned local decision
//! makes its executable surfaces eligible for normal policy evaluation.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceTrustLevel {
    Untrusted,
    Trusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthoritySource {
    Default,
    Managed,
    User,
    LocalSession,
    Project,
    Skill,
    Hook,
    Mcp,
    Remote,
    Child,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDirective {
    Grant,
    Narrow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceTrustInput {
    pub source: AuthoritySource,
    pub directive: TrustDirective,
}

impl WorkspaceTrustInput {
    pub const fn grant(source: AuthoritySource) -> Self {
        Self {
            source,
            directive: TrustDirective::Grant,
        }
    }

    pub const fn narrow(source: AuthoritySource) -> Self {
        Self {
            source,
            directive: TrustDirective::Narrow,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveWorkspaceTrust {
    level: WorkspaceTrustLevel,
    source: AuthoritySource,
    fingerprint: String,
    explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeveloperCapability {
    pub name: String,
    pub executable: String,
    pub read_only_roots: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSandboxProfile {
    Strict,
    TrustedLocalSmart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspacePolicyReceipt {
    pub trust: EffectiveWorkspaceTrust,
    pub profile: WorkspaceSandboxProfile,
    pub backend: String,
    pub writable_roots: Vec<String>,
    pub readable_roots: Vec<String>,
    pub capabilities: Vec<DeveloperCapability>,
}

impl EffectiveWorkspaceTrust {
    pub fn untrusted(
        source: AuthoritySource,
        fingerprint: impl Into<String>,
        explanation: impl Into<String>,
    ) -> Self {
        Self {
            level: WorkspaceTrustLevel::Untrusted,
            source,
            fingerprint: fingerprint.into(),
            explanation: explanation.into(),
        }
    }

    pub const fn level(&self) -> WorkspaceTrustLevel {
        self.level
    }

    pub const fn source(&self) -> AuthoritySource {
        self.source
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn explanation(&self) -> &str {
        &self.explanation
    }

    pub const fn is_trusted(&self) -> bool {
        matches!(self.level, WorkspaceTrustLevel::Trusted)
    }
}

/// Resolve all workspace-authority inputs monotonically.
///
/// Only an independently-owned user store or an explicit local session can
/// grant trust. Managed, remote and child constraints always narrow. Project,
/// skill, hook and MCP inputs can request narrowing but can never self-grant.
pub fn resolve_workspace_trust(
    fingerprint: impl Into<String>,
    inputs: impl IntoIterator<Item = WorkspaceTrustInput>,
) -> EffectiveWorkspaceTrust {
    let fingerprint = fingerprint.into();
    let mut grant = None;
    let mut narrowing = None;

    for input in inputs {
        match (input.source, input.directive) {
            (AuthoritySource::User | AuthoritySource::LocalSession, TrustDirective::Grant) => {
                grant = Some(input.source);
            }
            (
                AuthoritySource::Managed | AuthoritySource::Remote | AuthoritySource::Child,
                TrustDirective::Narrow,
            ) => {
                let rank = authority_rank(input.source);
                if narrowing
                    .map(|current| rank > authority_rank(current))
                    .unwrap_or(true)
                {
                    narrowing = Some(input.source);
                }
            }
            (_, TrustDirective::Narrow) => {
                narrowing.get_or_insert(input.source);
            }
            // Serialized/repository-controlled sources cannot grant trust.
            _ => {}
        }
    }

    if let Some(source) = narrowing {
        return EffectiveWorkspaceTrust::untrusted(
            source,
            fingerprint,
            format!("{source:?} policy requires the strict workspace profile"),
        );
    }

    if let Some(source) = grant {
        return EffectiveWorkspaceTrust {
            level: WorkspaceTrustLevel::Trusted,
            source,
            fingerprint,
            explanation: "fingerprint-bound local trust decision is current".to_string(),
        };
    }

    EffectiveWorkspaceTrust::untrusted(
        AuthoritySource::Default,
        fingerprint,
        "no current fingerprint-bound local trust decision",
    )
}

const fn authority_rank(source: AuthoritySource) -> u8 {
    match source {
        AuthoritySource::Managed => 3,
        AuthoritySource::Remote => 2,
        AuthoritySource::Child => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOWER_TRUST_SOURCES: [AuthoritySource; 4] = [
        AuthoritySource::Project,
        AuthoritySource::Skill,
        AuthoritySource::Hook,
        AuthoritySource::Mcp,
    ];

    #[test]
    fn repository_controlled_sources_cannot_self_grant() {
        for source in LOWER_TRUST_SOURCES {
            let decision = resolve_workspace_trust("fp", [WorkspaceTrustInput::grant(source)]);
            assert!(!decision.is_trusted(), "{source:?} self-granted trust");
        }
    }

    #[test]
    fn every_strict_constraint_wins_over_every_local_grant_order() {
        for grant in [AuthoritySource::User, AuthoritySource::LocalSession] {
            for narrow in [
                AuthoritySource::Managed,
                AuthoritySource::Remote,
                AuthoritySource::Child,
            ] {
                for inputs in [
                    vec![
                        WorkspaceTrustInput::grant(grant),
                        WorkspaceTrustInput::narrow(narrow),
                    ],
                    vec![
                        WorkspaceTrustInput::narrow(narrow),
                        WorkspaceTrustInput::grant(grant),
                    ],
                ] {
                    let decision = resolve_workspace_trust("fp", inputs);
                    assert!(!decision.is_trusted());
                    assert_eq!(decision.source(), narrow);
                }
            }
        }
    }

    #[test]
    fn local_grants_work_without_a_stricter_constraint() {
        for source in [AuthoritySource::User, AuthoritySource::LocalSession] {
            let decision = resolve_workspace_trust("fp", [WorkspaceTrustInput::grant(source)]);
            assert!(decision.is_trusted());
            assert_eq!(decision.source(), source);
        }
    }
}
