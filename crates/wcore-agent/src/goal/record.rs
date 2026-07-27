//! The durable form of a Goal's authority and budget envelope.
//!
//! ## Why this type exists beside `GoalAuthoritySnapshot`
//!
//! `wcore_types::goal::GoalAuthoritySnapshot` is deliberately output-only: it
//! implements `Serialize` but NOT `Deserialize`, so an untrusted host or child
//! payload has no route to an effective envelope and must pass
//! `resolve_goal_authority` first. That property is load-bearing and this module
//! does not weaken it.
//!
//! But a durable record must be readable to be replayable, and replay is
//! deserialization. Making the snapshot itself `Deserialize` to solve that would
//! hand every untrusted payload the authority-widening route the snapshot type
//! exists to close.
//!
//! So the durable record is a SEPARATE type that carries a digest over its own
//! fields. It deserializes freely — a journal frame must — but it cannot be
//! turned back into an effective envelope unless its fields still hash to the
//! digest recorded when the kernel wrote it. A same-UID writer who widens
//! `effective_limits` in the file produces a record that refuses to reconstruct
//! rather than one that resumes under a wider envelope.
//!
//! ## What this does and does not defend against, stated precisely
//!
//! The chain checksum already makes an edited journal frame detectable at
//! replay, and that is the primary defense. This digest is the second one, and
//! it covers the case the chain does not: a record that is byte-valid in the
//! chain but whose envelope no longer matches the parent it was resolved
//! against — the drift that a resume would otherwise paper over by
//! re-deriving. It is NOT a secret-keyed MAC and does not claim to stop an
//! attacker who can rewrite the whole chain.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wcore_types::goal::{GoalAuthoritySnapshot, GoalStrategy, LoopPolicy};

use crate::session_journal::state_payload_digest;

/// A resume could not reconstruct the recorded envelope exactly.
///
/// This is always a refusal, never a downgrade: the caller's only durable
/// response is [`wcore_types::goal::GoalTerminalState::AuthorityUnreconstructable`].
#[derive(Debug, Clone, thiserror::Error)]
#[error("goal authority envelope cannot be reconstructed: {detail}")]
pub struct AuthorityUnreconstructable {
    pub detail: String,
}

/// The durable, replayable form of a [`GoalAuthoritySnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalAuthorityRecord {
    /// Effective limits after intersection with the parent envelope.
    pub effective_limits: BTreeMap<String, u64>,
    /// The strategy that was actually authorized.
    pub strategy: GoalStrategy,
    /// The loop policy that was actually authorized.
    pub loop_policy: LoopPolicy,
    /// Digest of the parent envelope this was resolved against.
    pub parent_envelope_digest: String,
    /// Digest over the four fields above, computed when the kernel wrote the
    /// record. Reconstruction recomputes and compares.
    pub snapshot_digest: String,
}

impl GoalAuthorityRecord {
    /// Record an effective envelope durably.
    #[must_use]
    pub fn from_snapshot(snapshot: &GoalAuthoritySnapshot) -> Self {
        let mut record = Self {
            effective_limits: snapshot.effective_limits.clone(),
            strategy: snapshot.strategy,
            loop_policy: snapshot.loop_policy.clone(),
            parent_envelope_digest: snapshot.parent_envelope_digest.clone(),
            snapshot_digest: String::new(),
        };
        record.snapshot_digest = record.compute_digest();
        record
    }

    /// Digest over the envelope fields, excluding the digest itself.
    ///
    /// Uses the journal's own canonical-JSON digest so there is exactly one
    /// canonicalization in this crate rather than a second one that could drift
    /// from it.
    fn compute_digest(&self) -> String {
        let material = serde_json::json!({
            "effective_limits": self.effective_limits,
            "strategy": self.strategy,
            "loop_policy": self.loop_policy,
            "parent_envelope_digest": self.parent_envelope_digest,
        });
        // The canonicalizer only fails on non-finite floats; this material is
        // strings, integers and closed enums, so it cannot produce one.
        state_payload_digest(&material).unwrap_or_default()
    }

    /// Turn the durable record back into an effective envelope, or refuse.
    ///
    /// Returns `Err` — never a permissive default — when the recorded fields no
    /// longer hash to the recorded digest.
    pub fn reconstruct(&self) -> Result<GoalAuthoritySnapshot, AuthorityUnreconstructable> {
        let recomputed = self.compute_digest();
        if recomputed.is_empty() || recomputed != self.snapshot_digest {
            return Err(AuthorityUnreconstructable {
                detail: "recorded envelope does not match its committed digest".to_owned(),
            });
        }
        Ok(GoalAuthoritySnapshot {
            effective_limits: self.effective_limits.clone(),
            strategy: self.strategy,
            loop_policy: self.loop_policy.clone(),
            parent_envelope_digest: self.parent_envelope_digest.clone(),
        })
    }

    /// Reconstruct only if the envelope was resolved against the parent
    /// envelope this process can actually produce.
    ///
    /// A Goal whose parent envelope has moved is NOT resumable under the old
    /// one: re-deriving would silently resume under whatever the parent happens
    /// to be now, which is the amplification route Phase 21 closed on the live
    /// seams and which this kernel must not reopen on the durable one.
    pub fn reconstruct_against_parent(
        &self,
        parent_envelope_digest: &str,
    ) -> Result<GoalAuthoritySnapshot, AuthorityUnreconstructable> {
        if self.parent_envelope_digest != parent_envelope_digest {
            return Err(AuthorityUnreconstructable {
                detail: "parent envelope digest moved since the Goal was authorized".to_owned(),
            });
        }
        self.reconstruct()
    }

    /// The iteration ceiling this envelope's loop policy authorizes, if any.
    ///
    /// `Manual` has no numeric ceiling because each advance is itself an
    /// explicit operator action; every other policy carries its own bound and
    /// `Dynamic` structurally cannot be written with only one.
    #[must_use]
    pub fn iteration_ceiling(&self) -> Option<u32> {
        match &self.loop_policy {
            LoopPolicy::Once => Some(1),
            LoopPolicy::Fixed { iterations } => Some(*iterations),
            LoopPolicy::Dynamic { max_iterations, .. } => Some(*max_iterations),
            LoopPolicy::EventDriven { max_deliveries } => Some(*max_deliveries),
            LoopPolicy::Manual => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_types::goal::{GoalAuthorityRequest, resolve_goal_authority};

    fn snapshot() -> GoalAuthoritySnapshot {
        let request = GoalAuthorityRequest {
            requested_limits: BTreeMap::new(),
            strategy: GoalStrategy::Direct,
            loop_policy: LoopPolicy::Once,
        };
        let parent: BTreeMap<String, u64> =
            [("max_tokens".to_owned(), 10_u64)].into_iter().collect();
        resolve_goal_authority(&request, &parent, "parent-digest")
    }

    #[test]
    fn an_honest_record_round_trips_to_the_same_envelope() {
        let original = snapshot();
        let record = GoalAuthorityRecord::from_snapshot(&original);
        let restored = record.reconstruct().expect("reconstructs");
        assert_eq!(restored.effective_limits, original.effective_limits);
        assert_eq!(restored.strategy, original.strategy);
        assert_eq!(
            restored.parent_envelope_digest,
            original.parent_envelope_digest
        );
    }

    #[test]
    fn widening_a_limit_in_the_durable_record_refuses_to_reconstruct() {
        let mut record = GoalAuthorityRecord::from_snapshot(&snapshot());
        record
            .effective_limits
            .insert("max_tokens".to_owned(), 999_999);
        assert!(record.reconstruct().is_err());
    }

    #[test]
    fn a_moved_parent_envelope_refuses_to_reconstruct_rather_than_re_deriving() {
        let record = GoalAuthorityRecord::from_snapshot(&snapshot());
        assert!(record.reconstruct_against_parent("parent-digest").is_ok());
        assert!(
            record
                .reconstruct_against_parent("a-different-parent")
                .is_err()
        );
    }

    #[test]
    fn every_loop_policy_except_manual_carries_a_numeric_iteration_ceiling() {
        let policies = [
            (LoopPolicy::Once, Some(1)),
            (LoopPolicy::Fixed { iterations: 7 }, Some(7)),
            (
                LoopPolicy::Dynamic {
                    max_iterations: 3,
                    max_wall_millis: 1000,
                },
                Some(3),
            ),
            (LoopPolicy::EventDriven { max_deliveries: 5 }, Some(5)),
            (LoopPolicy::Manual, None),
        ];
        for (policy, expected) in policies {
            let mut record = GoalAuthorityRecord::from_snapshot(&snapshot());
            record.loop_policy = policy.clone();
            assert_eq!(record.iteration_ceiling(), expected, "{policy:?}");
        }
    }
}
