//! Selection by published item identity, with a conservation invariant (F26-02).
//!
//! # Selective means the USER chooses
//!
//! Narrowing an import to a chosen set is a user instruction. Quietly dropping
//! an item the user did not exclude — because it was awkward, large, or
//! unfamiliar — is data loss reported as success. So every discovered item ends
//! in exactly ONE [`Outcome`], and the three outcome counts must sum to the
//! discovered total.
//!
//! The conservation property is enforced by TYPE first and asserted as
//! arithmetic second: [`Accounting::record`] stores one outcome per identity in
//! a map keyed by identity, so an item cannot hold two outcomes, and
//! [`Accounting::unaccounted`] names every discovered identity that never got
//! one. [`Accounting::balances`] is then the numeric statement of the same
//! thing, which is what the gates read.
//!
//! # Where a FAILURE goes
//!
//! An item that cannot be imported for a reason other than user exclusion is
//! NOT a fourth bucket and is NOT dropped. It is [`Outcome::Quarantined`] with
//! a [`QuarantineReason::ImportFailed`] carrying the reason text, and it is
//! named in the report. That is the honest placement: a thing that could not be
//! imported is, by definition, not live — and putting it in the same bucket as
//! contained executable content keeps the three-way sum exact instead of
//! inviting a "misc" count nobody reconciles.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

/// Why an item was contained rather than imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuarantineReason {
    /// The item is executable content — see `quarantine::ExecutableReason`.
    Executable(String),
    /// The item could not be imported for a reason the user did not choose.
    /// Named in the report; never silent.
    ImportFailed(String),
}

impl std::fmt::Display for QuarantineReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuarantineReason::Executable(r) => write!(f, "executable: {r}"),
            QuarantineReason::ImportFailed(r) => write!(f, "import failed: {r}"),
        }
    }
}

/// The one outcome a discovered item ends in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Imported,
    Quarantined(QuarantineReason),
    /// The user explicitly excluded it — by naming it in `--exclude`, or by not
    /// naming it in a `--select` that named others.
    Excluded,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SelectError {
    /// A selection named an identity the plan never published. Refused rather
    /// than ignored: a typo that quietly imports nothing is a user telling the
    /// tool to do something and being told it succeeded.
    #[error(
        "no item with identity {0:?} was published by the plan; run the dry run to see the identities you can select"
    )]
    UnknownIdentity(String),
}

/// A user's selection over the identities the plan published.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    include: Option<BTreeSet<String>>,
    exclude: BTreeSet<String>,
}

impl Selection {
    /// Everything the plan published.
    pub fn all() -> Self {
        Self::default()
    }

    /// Only the named identities.
    pub fn including<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            include: Some(ids.into_iter().map(Into::into).collect()),
            exclude: BTreeSet::new(),
        }
    }

    /// Everything except the named identities.
    pub fn excluding<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            include: None,
            exclude: ids.into_iter().map(Into::into).collect(),
        }
    }

    /// Build from the two CLI flags. Both may be given; `--exclude` wins on an
    /// identity named by both, because refusing is the fail-safe reading.
    pub fn from_flags(select: &[String], exclude: &[String]) -> Self {
        Self {
            include: (!select.is_empty()).then(|| select.iter().cloned().collect()),
            exclude: exclude.iter().cloned().collect(),
        }
    }

    pub fn is_narrowed(&self) -> bool {
        self.include.is_some() || !self.exclude.is_empty()
    }

    /// Validate every named identity against what the plan published.
    ///
    /// Returns the set of identities the user asked to import. An identity in
    /// either flag that the plan did not publish is [`SelectError::UnknownIdentity`].
    pub fn resolve(&self, published: &[String]) -> Result<BTreeSet<String>, SelectError> {
        let known: BTreeSet<&str> = published.iter().map(String::as_str).collect();
        for id in self.include.iter().flatten().chain(self.exclude.iter()) {
            if !known.contains(id.as_str()) {
                return Err(SelectError::UnknownIdentity(id.clone()));
            }
        }
        Ok(published
            .iter()
            .filter(|id| self.wants(id))
            .cloned()
            .collect())
    }

    /// Whether the user asked for this identity. Not a validation — use
    /// [`Self::resolve`] for that.
    pub fn wants(&self, id: &str) -> bool {
        if self.exclude.contains(id) {
            return false;
        }
        match &self.include {
            Some(inc) => inc.contains(id),
            None => true,
        }
    }
}

/// One outcome per discovered identity, and the arithmetic over them.
#[derive(Debug, Clone, Default)]
pub struct Accounting {
    discovered: Vec<String>,
    outcomes: BTreeMap<String, Outcome>,
}

impl Accounting {
    /// Start from the identities the plan published. This is the denominator;
    /// nothing else may enlarge it.
    pub fn over(discovered: impl IntoIterator<Item = String>) -> Self {
        let mut discovered: Vec<String> = discovered.into_iter().collect();
        discovered.sort();
        discovered.dedup();
        Self {
            discovered,
            outcomes: BTreeMap::new(),
        }
    }

    /// Record the single outcome for one identity. Recording twice replaces —
    /// it never adds a second count — so double-recording cannot inflate a sum.
    pub fn record(&mut self, id: impl Into<String>, outcome: Outcome) {
        self.outcomes.insert(id.into(), outcome);
    }

    pub fn discovered(&self) -> usize {
        self.discovered.len()
    }

    fn count(&self, pred: impl Fn(&Outcome) -> bool) -> usize {
        self.discovered
            .iter()
            .filter(|id| self.outcomes.get(*id).is_some_and(&pred))
            .count()
    }

    pub fn imported(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Imported))
    }

    pub fn quarantined(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Quarantined(_)))
    }

    pub fn excluded(&self) -> usize {
        self.count(|o| matches!(o, Outcome::Excluded))
    }

    /// Identities that were discovered but never given an outcome. A non-empty
    /// result is the exact data loss the invariant exists to catch.
    pub fn unaccounted(&self) -> Vec<&str> {
        self.discovered
            .iter()
            .filter(|id| !self.outcomes.contains_key(*id))
            .map(String::as_str)
            .collect()
    }

    /// Identities recorded with an outcome that were never discovered. Also a
    /// defect: it means the sum could balance while addressing the wrong set.
    pub fn undiscovered(&self) -> Vec<&str> {
        let known: BTreeSet<&str> = self.discovered.iter().map(String::as_str).collect();
        self.outcomes
            .keys()
            .filter(|id| !known.contains(id.as_str()))
            .map(String::as_str)
            .collect()
    }

    /// The conservation invariant, as arithmetic.
    pub fn balances(&self) -> bool {
        self.imported() + self.quarantined() + self.excluded() == self.discovered()
            && self.unaccounted().is_empty()
            && self.undiscovered().is_empty()
    }

    /// Every item that failed for a reason the user did not choose, with its
    /// named reason. These are reported, never silent.
    pub fn failures(&self) -> Vec<(&str, &str)> {
        self.outcomes
            .iter()
            .filter_map(|(id, o)| match o {
                Outcome::Quarantined(QuarantineReason::ImportFailed(why)) => {
                    Some((id.as_str(), why.as_str()))
                }
                _ => None,
            })
            .collect()
    }

    /// Quarantined identities with their reason text, for the report.
    pub fn quarantined_with_reasons(&self) -> Vec<(&str, String)> {
        self.discovered
            .iter()
            .filter_map(|id| match self.outcomes.get(id) {
                Some(Outcome::Quarantined(r)) => Some((id.as_str(), r.to_string())),
                _ => None,
            })
            .collect()
    }

    /// `discovered imported quarantined excluded` — the four counts, in the
    /// order the scale measurement prints them.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.discovered(),
            self.imported(),
            self.quarantined(),
            self.excluded(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn published() -> Vec<String> {
        ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn selecting_a_subset_yields_exactly_that_subset() {
        let sel = Selection::including(["a", "c"]);
        let got = sel.resolve(&published()).unwrap();
        assert_eq!(
            got,
            ["a", "c"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>()
        );
        // Positive half: the un-narrowed selection is genuinely wider, so the
        // equality above is not the equality of two empty sets.
        assert_eq!(Selection::all().resolve(&published()).unwrap().len(), 4);
    }

    #[test]
    fn excluding_a_subset_imports_everything_else() {
        let sel = Selection::excluding(["b"]);
        let got = sel.resolve(&published()).unwrap();
        assert_eq!(got.len(), 3);
        assert!(!got.contains("b"));
        assert!(got.contains("a") && got.contains("c") && got.contains("d"));
    }

    #[test]
    fn an_unpublished_identity_is_refused_not_ignored() {
        let err = Selection::including(["a", "typo"])
            .resolve(&published())
            .unwrap_err();
        assert_eq!(err, SelectError::UnknownIdentity("typo".into()));
        // The same rule applies on the exclude side.
        assert_eq!(
            Selection::excluding(["nope"])
                .resolve(&published())
                .unwrap_err(),
            SelectError::UnknownIdentity("nope".into())
        );
        // Positive half: a published identity is accepted, so the refusals
        // above are not a blanket refusal.
        assert!(Selection::including(["a"]).resolve(&published()).is_ok());
    }

    #[test]
    fn conservation_balances_and_names_what_was_lost() {
        let mut acct = Accounting::over(published());
        acct.record("a", Outcome::Imported);
        acct.record(
            "b",
            Outcome::Quarantined(QuarantineReason::Executable("skill shell directive".into())),
        );
        acct.record("c", Outcome::Excluded);
        // "d" deliberately unaccounted.
        assert!(!acct.balances(), "an unaccounted item must NOT balance");
        assert_eq!(acct.unaccounted(), vec!["d"]);
        acct.record(
            "d",
            Outcome::Quarantined(QuarantineReason::ImportFailed("unreadable".into())),
        );
        assert!(acct.balances());
        assert_eq!(acct.counts(), (4, 1, 2, 1));
        assert_eq!(acct.failures(), vec![("d", "unreadable")]);
    }

    #[test]
    fn double_recording_cannot_inflate_a_count() {
        let mut acct = Accounting::over(vec!["a".to_string()]);
        acct.record("a", Outcome::Imported);
        acct.record("a", Outcome::Imported);
        assert_eq!(acct.counts(), (1, 1, 0, 0));
        assert!(acct.balances());
        // Replacing the outcome moves the item, never duplicates it.
        acct.record("a", Outcome::Excluded);
        assert_eq!(acct.counts(), (1, 0, 0, 1));
    }

    #[test]
    fn an_outcome_for_an_undiscovered_item_does_not_balance() {
        let mut acct = Accounting::over(vec!["a".to_string()]);
        acct.record("a", Outcome::Imported);
        acct.record("ghost", Outcome::Imported);
        assert_eq!(acct.undiscovered(), vec!["ghost"]);
        assert!(
            !acct.balances(),
            "a sum that balances over the wrong set is not conservation"
        );
    }

    #[test]
    fn exclude_wins_over_select_for_the_same_identity() {
        let sel = Selection::from_flags(&["a".into(), "b".into()], &["b".into()]);
        let got = sel.resolve(&published()).unwrap();
        assert_eq!(got, ["a".to_string()].into_iter().collect::<BTreeSet<_>>());
    }
}
