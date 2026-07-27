//! Command idempotency with receipt replay.
//!
//! Phase 24 Success Criterion 4.
//!
//! # Contract
//!
//! - The same command identity issued twice produces ONE effect and TWO
//!   IDENTICAL receipts.
//! - A DIFFERENT command issued under a used identity is a named CONFLICT —
//!   never silently accepted (two effects) and never silently replayed
//!   (the caller's actual request is discarded and it is handed someone
//!   else's answer).
//!
//! # This deliberately mirrors an existing pattern rather than inventing one
//!
//! `wcore-cli`'s `McpRemovalLedger` already implements exactly this shape,
//! including the conflict case, for the JSON-stream MCP-removal command. This
//! is that shape lifted to a reusable, bounded, generic ledger so the
//! workspace has ONE idempotency story rather than two that can drift.
//!
//! # A full ledger REFUSES a new identity rather than evicting an old one
//!
//! This is the design decision that matters. Eviction looks harmless and is
//! not: evicting an entry converts a future replay into a SECOND EFFECT, and
//! it does so silently, under load, which is exactly when a client is
//! retrying. A refusal is visible, is attributable, and leaves the guarantee
//! intact. See [`LedgerOutcome::Full`].

use std::collections::HashMap;

/// Default number of command receipts retained.
///
/// Finite because the identity is caller-supplied: an unbounded ledger is an
/// allocation a client chooses the size of.
pub const DEFAULT_CAPACITY: usize = 4096;

/// Longest accepted command identity. A caller-supplied key is untrusted
/// input and is bounded before it is ever used as a map key.
pub const MAX_IDENTITY_LEN: usize = 256;

/// What the ledger says about an incoming command.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum LedgerOutcome<R> {
    /// Never seen. The caller performs the effect and then records the receipt.
    Fresh,
    /// Seen, with a byte-identical command. The recorded receipt is returned
    /// and NO effect is performed.
    Replay(R),
    /// The identity is in use by a DIFFERENT command.
    Conflict,
    /// The identity is malformed — empty, or over [`MAX_IDENTITY_LEN`].
    InvalidIdentity,
    /// The ledger is at capacity and this identity is new. The command is
    /// REFUSED rather than admitted by evicting an older guarantee.
    Full,
}

impl<R> LedgerOutcome<R> {
    pub fn is_fresh(&self) -> bool {
        matches!(self, LedgerOutcome::Fresh)
    }
}

/// A bounded command-identity ledger.
///
/// `C` is the command shape and must be comparable, because "is this the same
/// command" is what separates a replay from a conflict. `R` is the receipt.
#[derive(Debug, Clone)]
pub struct CommandLedger<C, R> {
    entries: HashMap<String, (C, R)>,
    capacity: usize,
}

impl<C: PartialEq + Clone, R: Clone> CommandLedger<C, R> {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn identity_ok(identity: &str) -> bool {
        !identity.trim().is_empty() && identity.len() <= MAX_IDENTITY_LEN
    }

    /// Classify an incoming command WITHOUT mutating anything.
    ///
    /// Separated from [`Self::record`] on purpose: the caller must be able to
    /// learn "fresh" and then perform the effect before a receipt exists to
    /// record. Collapsing the two would force the ledger to invent a receipt
    /// for an effect that has not happened.
    pub fn classify(&self, identity: &str, command: &C) -> LedgerOutcome<R> {
        if !Self::identity_ok(identity) {
            return LedgerOutcome::InvalidIdentity;
        }
        match self.entries.get(identity) {
            Some((bound, receipt)) if bound == command => LedgerOutcome::Replay(receipt.clone()),
            Some(_) => LedgerOutcome::Conflict,
            None if self.entries.len() >= self.capacity => LedgerOutcome::Full,
            None => LedgerOutcome::Fresh,
        }
    }

    /// Bind `identity` to `command` and its `receipt`.
    ///
    /// First write wins. A second `record` under the same identity is ignored,
    /// so a caller that records twice by mistake cannot overwrite the receipt a
    /// previous replay already returned — which would make two replays of the
    /// same identity disagree.
    pub fn record(&mut self, identity: &str, command: &C, receipt: &R) -> bool {
        if !Self::identity_ok(identity) {
            return false;
        }
        if !self.entries.contains_key(identity) && self.entries.len() >= self.capacity {
            return false;
        }
        self.entries
            .entry(identity.to_string())
            .or_insert_with(|| (command.clone(), receipt.clone()));
        true
    }
}

impl<C: PartialEq + Clone, R: Clone> Default for CommandLedger<C, R> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Cmd {
        verb: String,
        target: String,
    }

    fn cmd(verb: &str, target: &str) -> Cmd {
        Cmd {
            verb: verb.into(),
            target: target.into(),
        }
    }

    #[test]
    fn a_repeated_identity_yields_one_effect_and_two_identical_receipts() {
        let mut ledger: CommandLedger<Cmd, String> = CommandLedger::new();
        let c = cmd("delete", "session-1");

        // Effects are counted here rather than assumed: the contract is about
        // how many times the caller ACTS, not about what the ledger says.
        let mut effects = 0;
        let mut issue = |ledger: &mut CommandLedger<Cmd, String>, effects: &mut i32| match ledger
            .classify("req-1", &c)
        {
            LedgerOutcome::Fresh => {
                *effects += 1;
                let receipt = format!("deleted:{}", c.target);
                ledger.record("req-1", &c, &receipt);
                receipt
            }
            LedgerOutcome::Replay(r) => r,
            other => panic!("unexpected outcome {other:?}"),
        };

        let first = issue(&mut ledger, &mut effects);
        let second = issue(&mut ledger, &mut effects);
        assert_eq!(effects, 1, "the effect must happen exactly once");
        assert_eq!(first, second, "both receipts must be identical");
    }

    #[test]
    fn a_different_command_under_a_used_identity_is_a_named_conflict() {
        // Both wrong answers are worth naming. Accepting it silently performs
        // a second, different effect under a key the caller believes is
        // idempotent. Replaying it silently discards the caller's actual
        // request and hands back someone else's answer.
        let mut ledger: CommandLedger<Cmd, String> = CommandLedger::new();
        let original = cmd("delete", "session-1");
        ledger.record("req-1", &original, &"deleted:session-1".to_string());

        let different = cmd("delete", "session-2");
        assert_eq!(
            ledger.classify("req-1", &different),
            LedgerOutcome::Conflict,
            "a different command under a used identity must be a conflict"
        );
        // Positive control: the ORIGINAL command under the same identity still
        // replays, so the conflict above is caused by the command differing
        // and not by the identity being present at all.
        assert_eq!(
            ledger.classify("req-1", &original),
            LedgerOutcome::Replay("deleted:session-1".to_string())
        );
    }

    #[test]
    fn a_full_ledger_refuses_a_new_identity_instead_of_evicting_a_guarantee() {
        // Eviction converts a future replay into a SECOND EFFECT, silently,
        // under load — which is exactly when clients retry.
        let mut ledger: CommandLedger<Cmd, String> = CommandLedger::with_capacity(2);
        let a = cmd("delete", "a");
        let b = cmd("delete", "b");
        let c = cmd("delete", "c");
        assert!(ledger.record("req-a", &a, &"ra".to_string()));
        assert!(ledger.record("req-b", &b, &"rb".to_string()));

        assert_eq!(ledger.classify("req-c", &c), LedgerOutcome::Full);
        assert!(!ledger.record("req-c", &c, &"rc".to_string()));

        // The existing guarantees are intact — that is the point of refusing.
        assert_eq!(
            ledger.classify("req-a", &a),
            LedgerOutcome::Replay("ra".to_string())
        );
        assert_eq!(ledger.len(), 2);
    }

    #[test]
    fn a_full_ledger_still_replays_a_known_identity() {
        // Capacity must bound NEW identities only. If a full ledger stopped
        // replaying, the exactly-once guarantee would evaporate at exactly the
        // load where it matters.
        let mut ledger: CommandLedger<Cmd, String> = CommandLedger::with_capacity(1);
        let a = cmd("delete", "a");
        ledger.record("req-a", &a, &"ra".to_string());
        assert_eq!(
            ledger.classify("req-a", &a),
            LedgerOutcome::Replay("ra".to_string())
        );
    }

    #[test]
    fn a_malformed_identity_is_refused_rather_than_used_as_a_key() {
        let ledger: CommandLedger<Cmd, String> = CommandLedger::new();
        let c = cmd("delete", "a");
        assert_eq!(ledger.classify("", &c), LedgerOutcome::InvalidIdentity);
        assert_eq!(ledger.classify("   ", &c), LedgerOutcome::InvalidIdentity);
        let long = "x".repeat(MAX_IDENTITY_LEN + 1);
        assert_eq!(ledger.classify(&long, &c), LedgerOutcome::InvalidIdentity);
        // The boundary itself is accepted, so the bound is exact rather than
        // approximately right.
        let exact = "x".repeat(MAX_IDENTITY_LEN);
        assert_eq!(ledger.classify(&exact, &c), LedgerOutcome::Fresh);
    }

    #[test]
    fn recording_twice_under_one_identity_does_not_rewrite_the_receipt() {
        // Otherwise two replays of the same identity can disagree, and the
        // second contradicts an answer the client already acted on.
        let mut ledger: CommandLedger<Cmd, String> = CommandLedger::new();
        let a = cmd("delete", "a");
        ledger.record("req-a", &a, &"first".to_string());
        ledger.record("req-a", &a, &"second".to_string());
        assert_eq!(
            ledger.classify("req-a", &a),
            LedgerOutcome::Replay("first".to_string()),
            "first write wins; a later record must not rewrite a receipt \
             a replay may already have returned"
        );
    }
}
