//! Contradiction resolution for the semantic partition.
//!
//! When `semantic.rs::assert` is about to write a new `Fact` whose
//! `(subject, predicate, tier)` already matches an existing un-superseded
//! `Fact` with a different `object`, the conflict is routed through
//! [`ContradictionResolver::resolve`]. The resolver chooses between three
//! outcomes — supersede the existing fact, keep the existing fact, or let
//! both coexist with a reduced-confidence stamp on the newcomer.
//!
//! This is a direct port of the Forge `ContradictionResolver.ts:71-100`
//! algorithm. Confidence values are weighted by a 1.2× recency bias on the
//! new fact before comparison.
//!
//! See `.blackboard/v0.6.4-memory-depth-design.md` §6.3 for the locked
//! algorithm and golden test values.
//!
//! Greenfield in v0.6.4 Task 6.3.

/// One side of a (existing, new) pair being evaluated for contradiction.
///
/// Borrowed fields keep callers from having to clone fact strings out of
/// SQLite rows just to run the resolver.
#[derive(Debug, Clone, Copy)]
pub struct ContradictionCandidate<'a> {
    pub existing_relation_id: &'a str,
    pub existing_fact: &'a str,
    pub existing_confidence: f64,
    pub new_fact: &'a str,
    pub new_confidence: f64,
}

/// Resolution outcome — what the caller should do with the conflicting pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContradictionResolution {
    /// Mark the existing fact as superseded, write the new fact at its
    /// original confidence.
    Supersede,
    /// Discard the new fact, leave the existing fact untouched.
    KeepExisting,
    /// Write the new fact alongside the existing one, but at a
    /// reduced confidence (×0.8).
    Coexist,
}

/// What the resolver decided plus the confidence the caller should stamp
/// onto the new fact (if it is written at all). `reason` is a static
/// human-readable string for audit logs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolutionResult {
    pub resolution: ContradictionResolution,
    pub adjusted_confidence: f64,
    pub reason: &'static str,
}

/// Stateless resolver. Held as a zero-sized type so callers can keep one
/// on the `Memory` struct without worrying about lifetimes or shared state.
#[derive(Debug, Default, Clone, Copy)]
pub struct ContradictionResolver;

impl ContradictionResolver {
    pub const fn new() -> Self {
        Self
    }

    /// Decide how to handle a (existing, new) contradiction pair.
    ///
    /// Direct port of Forge `ContradictionResolver.ts:71-100`. The new
    /// confidence is multiplied by 1.2 (recency bias) before comparison.
    /// Strictly greater (`>`) is required for [`Supersede`], so exact ties
    /// (after bias) fall through to the [`Coexist`] branch.
    ///
    /// [`Supersede`]: ContradictionResolution::Supersede
    /// [`Coexist`]: ContradictionResolution::Coexist
    pub fn resolve(&self, c: &ContradictionCandidate<'_>) -> ResolutionResult {
        let adjusted_new = c.new_confidence * 1.2;
        if adjusted_new > c.existing_confidence {
            return ResolutionResult {
                resolution: ContradictionResolution::Supersede,
                adjusted_confidence: c.new_confidence,
                reason: "Newer fact with higher adjusted confidence supersedes existing",
            };
        }
        if c.existing_confidence - adjusted_new < 0.1 {
            return ResolutionResult {
                resolution: ContradictionResolution::Coexist,
                adjusted_confidence: c.new_confidence * 0.8,
                reason: "Similar confidence — both stored, new at reduced confidence",
            };
        }
        ResolutionResult {
            resolution: ContradictionResolution::KeepExisting,
            adjusted_confidence: c.new_confidence * 0.8,
            reason: "Existing fact has significantly higher confidence",
        }
    }
}

/// Initial confidence for a freshly-asserted fact, keyed by source.
///
/// Direct port of Forge's source weighting. Used by callers that produce
/// a `Fact` without an upstream-supplied confidence value.
pub fn initial_confidence(source: &str) -> f64 {
    match source {
        "user" => 0.95,
        "system" => 0.85,
        "agent" => 0.70,
        "delegated_agent" => 0.40,
        _ => 0.5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for f64 equality on the pinned golden values. The
    /// algorithm only uses multiplications by `1.2`, `0.8`, and a
    /// subtraction — well within a few ULPs.
    const EPS: f64 = 1e-9;

    fn cand<'a>(
        existing_id: &'a str,
        existing_fact: &'a str,
        existing_conf: f64,
        new_fact: &'a str,
        new_conf: f64,
    ) -> ContradictionCandidate<'a> {
        ContradictionCandidate {
            existing_relation_id: existing_id,
            existing_fact,
            existing_confidence: existing_conf,
            new_fact,
            new_confidence: new_conf,
        }
    }

    // -- pinned golden values from design doc §6.3 -------------------

    #[test]
    fn clear_supersede() {
        // existing=0.50, new=0.80, adjusted_new=0.96 > 0.50 → Supersede @ 0.80
        let r = ContradictionResolver::new().resolve(&cand("r1", "old", 0.50, "new", 0.80));
        assert_eq!(r.resolution, ContradictionResolution::Supersede);
        assert!((r.adjusted_confidence - 0.80).abs() < EPS);
        assert_eq!(
            r.reason,
            "Newer fact with higher adjusted confidence supersedes existing"
        );
    }

    #[test]
    fn clear_keep_existing() {
        // existing=0.95, new=0.20, adjusted_new=0.24, diff=0.71 ≥ 0.1 → KeepExisting @ 0.16
        let r = ContradictionResolver::new().resolve(&cand("r2", "old", 0.95, "new", 0.20));
        assert_eq!(r.resolution, ContradictionResolution::KeepExisting);
        assert!((r.adjusted_confidence - 0.16).abs() < EPS);
        assert_eq!(
            r.reason,
            "Existing fact has significantly higher confidence"
        );
    }

    #[test]
    fn narrow_coexist() {
        // existing=0.85, new=0.70, adjusted_new=0.84, diff=0.01 < 0.1 → Coexist @ 0.56
        let r = ContradictionResolver::new().resolve(&cand("r3", "old", 0.85, "new", 0.70));
        assert_eq!(r.resolution, ContradictionResolution::Coexist);
        assert!((r.adjusted_confidence - 0.56).abs() < EPS);
        assert_eq!(
            r.reason,
            "Similar confidence — both stored, new at reduced confidence"
        );
    }

    #[test]
    fn boundary_at_0_1() {
        // existing=0.95, new=0.70, adjusted_new=0.84, diff=0.11 ≥ 0.1 → KeepExisting @ 0.56
        let r = ContradictionResolver::new().resolve(&cand("r4", "old", 0.95, "new", 0.70));
        assert_eq!(r.resolution, ContradictionResolution::KeepExisting);
        assert!((r.adjusted_confidence - 0.56).abs() < EPS);
    }

    #[test]
    fn exact_tie() {
        // existing=0.60, new=0.50, adjusted_new=0.60, diff=0 → Coexist (because `>` not `>=`)
        let r = ContradictionResolver::new().resolve(&cand("r5", "old", 0.60, "new", 0.50));
        assert_eq!(r.resolution, ContradictionResolution::Coexist);
        assert!((r.adjusted_confidence - 0.40).abs() < EPS);
    }

    // -- initial_confidence pinned table -----------------------------

    #[test]
    fn initial_confidence_user() {
        assert!((initial_confidence("user") - 0.95).abs() < EPS);
    }

    #[test]
    fn initial_confidence_system() {
        assert!((initial_confidence("system") - 0.85).abs() < EPS);
    }

    #[test]
    fn initial_confidence_agent() {
        assert!((initial_confidence("agent") - 0.70).abs() < EPS);
    }

    #[test]
    fn initial_confidence_delegated_agent() {
        assert!((initial_confidence("delegated_agent") - 0.40).abs() < EPS);
    }

    #[test]
    fn initial_confidence_unknown() {
        assert!((initial_confidence("unknown") - 0.50).abs() < EPS);
        // any non-matching string falls through to 0.5
        assert!((initial_confidence("") - 0.50).abs() < EPS);
        assert!((initial_confidence("anything_else") - 0.50).abs() < EPS);
    }
}
