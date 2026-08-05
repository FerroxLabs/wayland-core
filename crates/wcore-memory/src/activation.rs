//! 23B-C3 — memory ACTIVATION: seeing and controlling what memory was put
//! into the prompt on this turn.
//!
//! # Why this module exists
//!
//! Criterion 3 opens with "see and control memory/user-model **activation**",
//! and that was the one clause of the seven with no surface of any kind.
//! `AgentEngine::recall_relevant_facts` searched durable memory on the first
//! user turn of every session, built a `<system-reminder>` block, pushed it
//! into the outbound provider request — and reported the fact to nobody except
//! a `tracing::debug!` line nobody reads. There was no way to ask what had
//! been injected, and no way to stop it being injected.
//!
//! The distinction from [`crate::provenance`] is worth stating because they
//! look similar. Provenance answers "**if** I search for X, where would the
//! results come from" — it is a property of a retrieval a user asks for.
//! Activation answers "what did the agent put in my prompt **just now**,
//! without my asking" — a record of something that already happened on the
//! user's behalf. The second is the privacy-relevant one, and it was missing.
//!
//! # The one invariant
//!
//! [`ActivationLog::record`] is called by the engine at the point of
//! injection, with the items it actually pushed. It is not a second
//! computation of what "would" be recalled, so it cannot drift from what was
//! sent — the same discipline `search_basic_with_provenance` uses to keep a
//! provenance record honest.

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::provenance::RecallExclusion;
use crate::v2_types::{Partition, Tier};

/// One item the agent placed into the prompt on the user's behalf.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivatedItem {
    pub id: String,
    pub partition: Partition,
    pub tier: Tier,
    /// The text as it went into the prompt. Stored so a user can identify the
    /// item to correct or forget it without having to run a second search and
    /// hope it returns the same thing.
    pub preview: String,
}

/// What durable memory was activated into one turn's prompt, and what was
/// withheld from it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallActivation {
    pub at: i64,
    /// The user message the recall was run against.
    pub query: String,
    /// False when the user has switched auto-recall off. Recorded rather than
    /// inferred from an empty `injected` list, because "nothing matched" and
    /// "you turned this off" are different answers and only one of them is
    /// actionable.
    pub enabled: bool,
    pub injected: Vec<ActivatedItem>,
    /// Cells and items a privacy scope or retention bound kept out. This is
    /// the half a user cannot otherwise observe at all.
    pub excluded: Vec<RecallExclusion>,
}

impl RecallActivation {
    /// The record written when the user has auto-recall switched off. Carries
    /// `enabled: false` and an empty injection list.
    #[must_use]
    pub fn disabled(at: i64, query: impl Into<String>) -> Self {
        Self {
            at,
            query: query.into(),
            enabled: false,
            injected: Vec::new(),
            excluded: Vec::new(),
        }
    }
}

/// The see-and-control surface for activation. Shared by `Arc` between the
/// engine that writes it and the `/memory activation` command that reads it.
#[derive(Debug)]
pub struct ActivationLog {
    enabled: AtomicBool,
    last: Mutex<Option<RecallActivation>>,
}

impl Default for ActivationLog {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivationLog {
    #[must_use]
    pub fn new() -> Self {
        Self {
            // On by default: cross-session recall is the feature, and shipping
            // it off would be a silent behaviour change dressed as a control.
            enabled: AtomicBool::new(true),
            last: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Turn automatic memory injection on or off. Returns the previous state
    /// so a caller can report what actually changed instead of echoing the
    /// request back as an unconditional success.
    pub fn set_enabled(&self, enabled: bool) -> bool {
        self.enabled.swap(enabled, Ordering::SeqCst)
    }

    /// Record what was injected. Called by the engine AT the injection site
    /// with the items it really pushed — never recomputed.
    pub fn record(&self, activation: RecallActivation) {
        *self.last.lock() = Some(activation);
    }

    /// The most recent activation, or `None` if no turn has run a recall yet.
    /// `None` is deliberately distinguishable from "a turn ran and injected
    /// nothing": a surface that rendered both as "nothing in your prompt"
    /// would be unable to tell a user whether the feature had run at all.
    #[must_use]
    pub fn last(&self) -> Option<RecallActivation> {
        self.last.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> ActivatedItem {
        ActivatedItem {
            id: id.into(),
            partition: Partition::Semantic,
            tier: Tier::Project,
            preview: format!("preview of {id}"),
        }
    }

    #[test]
    fn no_turn_yet_is_distinguishable_from_a_turn_that_injected_nothing() {
        let log = ActivationLog::new();
        assert!(
            log.last().is_none(),
            "before any turn, last() must be None and not an empty activation"
        );
        log.record(RecallActivation {
            at: 100,
            query: "q".into(),
            enabled: true,
            injected: Vec::new(),
            excluded: Vec::new(),
        });
        let last = log.last().expect("a turn has now run");
        assert!(
            last.injected.is_empty(),
            "this turn injected nothing, which is NOT the same as no turn having run"
        );
        assert!(
            last.enabled,
            "and it was not because the user switched it off"
        );
    }

    #[test]
    fn the_off_switch_reports_its_previous_state_and_is_recorded_distinctly() {
        let log = ActivationLog::new();
        assert!(log.enabled(), "auto-recall is on by default");
        assert!(
            log.set_enabled(false),
            "set_enabled must return the PREVIOUS state, not the new one"
        );
        assert!(!log.enabled());
        assert!(
            !log.set_enabled(false),
            "a second off must report it was already off"
        );

        log.record(RecallActivation::disabled(200, "q"));
        let last = log.last().unwrap();
        assert!(
            !last.enabled,
            "a disabled activation must say so, so a user can tell 'nothing matched' from \
             'you turned this off'"
        );
        assert!(last.injected.is_empty());
    }

    /// `record` overwrites rather than appends, so the surface always describes
    /// the **latest** turn — never a stale one.
    #[test]
    fn record_overwrites_so_the_surface_describes_the_latest_turn() {
        let log = ActivationLog::new();
        log.record(RecallActivation {
            at: 1,
            query: "first".into(),
            enabled: true,
            injected: vec![item("a")],
            excluded: Vec::new(),
        });
        log.record(RecallActivation {
            at: 2,
            query: "second".into(),
            enabled: true,
            injected: vec![item("b")],
            excluded: Vec::new(),
        });
        let last = log.last().unwrap();
        assert_eq!(last.query, "second");
        assert_eq!(last.injected.len(), 1);
        assert_eq!(
            last.injected[0].id, "b",
            "a stale activation record would tell a user their prompt contains something it \
             does not"
        );
    }
}
