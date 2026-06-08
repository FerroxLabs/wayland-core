// M4 — Tier resolver.
//
// Maps (Partition, ResolverContext) to a Tier. Used by the dispatcher when
// the caller doesn't explicitly pin a tier, and to reject invalid
// (Partition, Tier) combinations before any DB I/O.

use std::path::PathBuf;

use crate::error::{MemoryError, Result};
use crate::v2_types::{Partition, Tier, is_valid};

#[derive(Debug, Clone, Default)]
pub struct ResolverContext {
    pub session_id: Option<String>,
    pub project_root: Option<PathBuf>,
}

pub struct TierResolver;

impl TierResolver {
    /// Pick the design-doc default tier for a partition. Does NOT consult
    /// the context — pure dispatch table.
    pub fn resolve_default(p: Partition) -> Tier {
        p.default_tier()
    }

    /// Resolve a tier honoring the caller's context: if a session_id is
    /// present, P1/P2 prefer Session; otherwise fall back to default.
    pub fn resolve_with_context(p: Partition, ctx: &ResolverContext) -> Tier {
        match p {
            Partition::Working => Tier::Session, // P1 is always Session
            Partition::Episodic if ctx.session_id.is_some() => Tier::Session,
            Partition::Episodic if ctx.project_root.is_some() => Tier::Project,
            other => Self::resolve_default(other),
        }
    }

    /// Validate a (partition, tier) pair against the design's allowed cells.
    pub fn validate(p: Partition, t: Tier) -> Result<()> {
        if is_valid(p, t) {
            Ok(())
        } else {
            Err(MemoryError::AccessDenied {
                partition: p.to_string(),
                tier: t.to_string(),
                reason: "invalid (partition, tier) combination".into(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tier_matches_design() {
        assert_eq!(
            TierResolver::resolve_default(Partition::Working),
            Tier::Session
        );
        assert_eq!(
            TierResolver::resolve_default(Partition::Episodic),
            Tier::Project
        );
        assert_eq!(
            TierResolver::resolve_default(Partition::Semantic),
            Tier::Project
        );
        assert_eq!(
            TierResolver::resolve_default(Partition::Procedural),
            Tier::Project
        );
        assert_eq!(TierResolver::resolve_default(Partition::Core), Tier::Global);
    }

    #[test]
    fn p1_always_session() {
        let ctx = ResolverContext::default();
        assert_eq!(
            TierResolver::resolve_with_context(Partition::Working, &ctx),
            Tier::Session
        );
    }

    #[test]
    fn p2_uses_session_when_available() {
        let ctx = ResolverContext {
            session_id: Some("s1".into()),
            project_root: None,
        };
        assert_eq!(
            TierResolver::resolve_with_context(Partition::Episodic, &ctx),
            Tier::Session
        );
    }

    #[test]
    fn p2_uses_project_when_no_session() {
        let ctx = ResolverContext {
            session_id: None,
            project_root: Some("/proj".into()),
        };
        assert_eq!(
            TierResolver::resolve_with_context(Partition::Episodic, &ctx),
            Tier::Project
        );
    }

    #[test]
    fn p5_always_global() {
        let ctx = ResolverContext {
            session_id: Some("s".into()),
            project_root: Some("/p".into()),
        };
        assert_eq!(
            TierResolver::resolve_with_context(Partition::Core, &ctx),
            Tier::Global
        );
    }

    #[test]
    fn validate_accepts_valid_combos() {
        for &(p, t) in crate::v2_types::valid_combinations() {
            assert!(TierResolver::validate(p, t).is_ok(), "rejected {p:?},{t:?}");
        }
    }

    #[test]
    fn validate_rejects_p1_project() {
        let err = TierResolver::validate(Partition::Working, Tier::Project).unwrap_err();
        assert!(matches!(err, MemoryError::AccessDenied { .. }));
    }

    #[test]
    fn validate_rejects_p5_session() {
        let err = TierResolver::validate(Partition::Core, Tier::Session).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("core"), "{s}");
        assert!(s.contains("session"), "{s}");
    }
}
