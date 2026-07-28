//! Phase 30 (F30-04) — the claims register, the checker that refuses an unsupported claim,
//! and the renderer that is the ONLY code path producing a published document.
//!
//! ## Why this is a program and not a review
//!
//! Asking *"can this claim be supported?"* once per claim, dozens of times, is a judgement,
//! and judgements degrade. That is how one hundred and seventy-four self-passing gates in
//! this program survived three rounds of adversarial human reading. So the published claim
//! set is DATA, the checker is a program, the gate runs the program, and the published prose
//! is RENDERED from the verified data by that same program. There is no path from an
//! unverified claim to a published sentence, because publication is a code path and not a
//! writing task.
//!
//! ## What bounds a comparative claim, and why it is not "an interval" alone
//!
//! `.planning/intel/COMPETITIVE-LEDGER.md` publishes sentences like *"Core architectural
//! lead, operationally unproven"* and *"it would be a lead if proven"*. Those are comparative
//! AND correct, so a banned-words list would flag this program's own honest writing. They
//! also carry no interval, because they are a census of two pinned source trees rather than
//! a sample — there is no sampling variance to put bounds around.
//!
//! So a comparative claim must be BOUNDED, and scope decides which kind of bound is required:
//!
//! | evidence scope | bound required |
//! |---|---|
//! | `SCRIPTED_HARNESS` / `LIVE_PROVIDER` | a real interval, and the directional rule applies |
//! | `STATIC_SOURCE` | an explicit unproven-qualifier in the claim text |
//!
//! The obvious loophole — relabel a measured comparative `STATIC_SOURCE` and escape the
//! interval — is closed by SCOPE CONTAINMENT rather than by trust: a claim citing scripted
//! evidence cannot declare a scope that evidence does not contain. The two rules interlock,
//! and neither is sufficient alone.
//!
//! ## The directional rule is CALLED, never copied
//!
//! [`frontier_trials::direction_for`] already refuses a direction on an interval containing
//! zero. This module calls it. Two copies of a permissive-drift-prone rule is two chances for
//! one of them to drift while the tests point at the other.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::frontier_trials::{IntervalV1, ScopeV1, direction_for, protocol_sha256};
use crate::receipt::Evidence;

/// The tie band 30-02's frozen protocol declares. A directional verdict must clear it
/// entirely, which necessarily excludes zero.
pub const TIE_BAND_DEFAULT: f64 = 0.05;

pub const CLAIMS_SCHEMA: &str = "wayland.eval.claims";
pub const CLAIMS_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// The lexicons — these CLASSIFY, they do not ban
// ---------------------------------------------------------------------------

/// Comparison markers. A sentence matching one of these IS a comparison, whatever its
/// author declared. Matching is word-boundaried, so `leading` does not fire on `misleading`.
///
/// This list being finite is a stated limit of the mechanism, not a hidden one: a
/// sufficiently creative sentence can compare without matching it. That is precisely why
/// scope containment and the evidence-pointer requirement — which apply to EVERY claim
/// regardless of class — do more work than this list does.
pub const COMPARATIVE_LEXICON: &[&str] = &[
    "ahead",
    "advantage",
    "behind",
    "beats",
    "best",
    "better",
    "clearest",
    "comparable",
    "competitive",
    "exceeds",
    "faster",
    "fastest",
    "inferior",
    "indistinguishable",
    "lead",
    "leads",
    "leading",
    "less reliable",
    "matches",
    "more reliable",
    "more reliably",
    "no counterpart",
    "on par",
    "only party",
    "outperform",
    "outperforms",
    "slower",
    "slowest",
    "stronger",
    "strongest",
    "superior",
    "surpass",
    "surpasses",
    "than",
    "unique",
    "unmatched",
    "unrivalled",
    "weaker",
    "weakest",
    "wins",
    "worse",
    "worst",
];

/// The strict subset that asserts ONE SIDE IS BETTER. Only these trigger the directional
/// rule, because `INCONCLUSIVE` and `PRACTICALLY_INDISTINGUISHABLE` are refusals to claim
/// and must stay available.
///
/// Derived from the text rather than from a declared boolean field ON PURPOSE: a field
/// saying "this does not assert a direction" is a field an author can simply set.
pub const DIRECTIONAL_LEXICON: &[&str] = &[
    "ahead",
    "beats",
    "best",
    "better",
    "behind",
    "clearest",
    "exceeds",
    "faster",
    "fastest",
    "inferior",
    "lead",
    "leads",
    "leading",
    "less reliable",
    "more reliable",
    "more reliably",
    "outperform",
    "outperforms",
    "slower",
    "slowest",
    "stronger",
    "strongest",
    "superior",
    "surpass",
    "surpasses",
    "unmatched",
    "unrivalled",
    "weaker",
    "weakest",
    "wins",
    "worse",
    "worst",
];

/// The qualifiers that WITHHOLD a static-source comparison. This is what makes
/// *"Core architectural lead, operationally unproven"* publishable and
/// *"Core is architecturally superior"* not, at identical scope with identical evidence.
pub const UNPROVEN_QUALIFIER_LEXICON: &[&str] = &[
    "if proven",
    "would be a lead if proven",
    "operationally unproven",
    "unproven",
    "not proven",
    "outcome proof needed",
    "proof needed",
    "runtime certification required",
    "certification required",
    "not yet measured",
    "not measured",
    "unverified",
];

/// Normalize to ` token token ` form so lexicon matching is word-boundaried and
/// punctuation-insensitive, and multi-word phrases still match.
fn normalized(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    let mut prev_space = true;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

/// The lexicon term that fired, or `None`. Returns the term so a refusal can NAME it.
fn lexicon_hit(text: &str, lexicon: &[&'static str]) -> Option<&'static str> {
    let hay = normalized(text);
    lexicon
        .iter()
        .copied()
        .find(|t| hay.contains(&normalized(t)))
}

/// Whether the sentence compares at all.
pub fn is_comparative(text: &str) -> bool {
    lexicon_hit(text, COMPARATIVE_LEXICON).is_some()
}

/// Whether the sentence asserts that one side is better.
pub fn is_directional(text: &str) -> bool {
    lexicon_hit(text, DIRECTIONAL_LEXICON).is_some()
}

/// Whether the sentence explicitly withholds its own assertion.
pub fn carries_unproven_qualifier(text: &str) -> bool {
    lexicon_hit(text, UNPROVEN_QUALIFIER_LEXICON).is_some()
}

// ---------------------------------------------------------------------------
// Closed vocabularies
// ---------------------------------------------------------------------------

/// What kind of statement a claim is. CLOSED — an invented class fails at deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimClassV1 {
    /// A statement about Wayland alone.
    Factual,
    /// A statement setting Wayland against a peer.
    Comparative,
    /// A statement that something could NOT be measured. Renders to LIMITATIONS, never
    /// to ALLOWED, so it can never carry a superiority assertion into publication.
    Limitation,
}

impl ClaimClassV1 {
    pub fn token(self) -> &'static str {
        match self {
            Self::Factual => "factual",
            Self::Comparative => "comparative",
            Self::Limitation => "limitation",
        }
    }
}

/// Where a claim's evidence points, and at what scope it was gathered. The scope travels
/// WITH the evidence rather than with the claim, because containment compares the two.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClaimEvidenceRefV1 {
    /// A path relative to the repository root. Resolved by a stat, never by reading a
    /// document that asserts it exists.
    Path {
        id: String,
        path: String,
        scope: ScopeV1,
    },
    /// A leg of 30-02's trial accounting. Resolving means the leg EXISTS, is recorded
    /// `RUN` rather than `UNPROVEN`, and its own capture file is present.
    TrialLeg {
        id: String,
        leg: String,
        legs_tsv: String,
        scope: ScopeV1,
    },
    /// A CTRL-01 evidence ID, resolved against 30-01's OWN resolution table rather than
    /// against the ledger sentence that cites it.
    ///
    /// 30-01 filed a HIGH finding that `PEER-PROBE-2026-07-26` names no openable artifact
    /// while carrying half the Delta column in six families, and concluded: *"Any 30-03
    /// claim resting on a peer comparison inherits it."* That finding is made MECHANICAL
    /// here instead of advisory — a claim citing an ID 30-01 recorded `UNRESOLVED` is
    /// refused, exactly as a claim resting on an `UNPROVEN` leg is refused, and for the
    /// same reason: the citation cannot be checked by a reader.
    LedgerEvidenceId {
        id: String,
        evidence_id: String,
        resolution_tsv: String,
        scope: ScopeV1,
    },
}

/// A leg whose measurement is subject to a recorded INSTRUMENT defect.
///
/// 30-02 found, by running its own frozen protocol, that the canonical script emits a tool
/// call named `write_file` — a name only Hermes exposes — and that **OpenClaw also scored
/// 0/30 on the identical script**. Two of three harnesses failing one script is evidence
/// about the script's dialect, not about two products.
///
/// A leg like that is not UNPROVEN: it ran, and its number is real. But the number does not
/// measure what the dimension is named after, so a DIRECTIONAL claim built on it would be
/// the single most misleading sentence this phase could publish. It is refused mechanically
/// rather than left to an author's restraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfoundV1 {
    pub leg: String,
    pub defect: String,
    pub evidence: String,
    pub substitution_point: String,
}

impl ClaimEvidenceRefV1 {
    pub fn id(&self) -> &str {
        match self {
            Self::Path { id, .. }
            | Self::TrialLeg { id, .. }
            | Self::LedgerEvidenceId { id, .. } => id,
        }
    }

    pub fn scope(&self) -> ScopeV1 {
        match self {
            Self::Path { scope, .. }
            | Self::TrialLeg { scope, .. }
            | Self::LedgerEvidenceId { scope, .. } => *scope,
        }
    }

    /// The leg this reference names, if it names one. Used by the confound rule.
    pub fn leg(&self) -> Option<&str> {
        match self {
            Self::TrialLeg { leg, .. } => Some(leg),
            _ => None,
        }
    }

    /// A human-readable locator, used so a refusal can name the offender.
    pub fn locator(&self) -> String {
        match self {
            Self::Path { path, .. } => path.clone(),
            Self::TrialLeg { leg, legs_tsv, .. } => format!("{leg} in {legs_tsv}"),
            Self::LedgerEvidenceId {
                evidence_id,
                resolution_tsv,
                ..
            } => format!("{evidence_id} in {resolution_tsv}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals — each a distinct typed error naming its offender
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ClaimRefusal {
    #[error("claim `{claim}` carries no evidence reference")]
    NoEvidenceReference { claim: String },

    #[error("claim `{claim}` cites `{reference}` which does not resolve: {detail}")]
    EvidenceDoesNotResolve {
        claim: String,
        reference: String,
        detail: String,
    },

    #[error("claim `{claim}` rests on `{leg}`, which 30-02 recorded UNPROVEN: {blocker}")]
    EvidenceLegUnproven {
        claim: String,
        leg: String,
        blocker: String,
    },

    #[error(
        "claim `{claim}` cites CTRL-01 evidence ID `{evidence_id}`, which 30-01 recorded \
         `{outcome}`: {detail}"
    )]
    EvidenceIdUnresolved {
        claim: String,
        evidence_id: String,
        outcome: String,
        detail: String,
    },

    #[error(
        "claim `{claim}` compares ({why}) on `{leg}`, whose measurement 30-02 recorded as \
         confounded by an instrument defect: {defect}"
    )]
    ConfoundedLegSupportsNoComparison {
        claim: String,
        why: String,
        leg: String,
        defect: String,
    },

    #[error("comparative claim `{claim}` names no pinned peer baseline")]
    ComparativeWithoutPinnedBaseline { claim: String },

    #[error(
        "comparative claim `{claim}` rests on a {scope} measurement but carries no interval \
         ({detail}); a point estimate is not an interval"
    )]
    ComparativeWithoutInterval {
        claim: String,
        scope: &'static str,
        detail: String,
    },

    #[error(
        "claim `{claim}` asserts a direction (`{term}`) on delta interval [{lower}, {upper}], \
         which frontier_trials::direction_for entails `{entailed}` rather than a direction"
    )]
    DirectionalOnIntervalContainingZero {
        claim: String,
        term: &'static str,
        lower: f64,
        upper: f64,
        entailed: &'static str,
    },

    #[error(
        "claim `{claim}` is declared `{declared}` but its text is comparative (`{term}`); \
         relabelling does not dodge the classifier"
    )]
    Misclassification {
        claim: String,
        declared: &'static str,
        term: &'static str,
    },

    #[error(
        "claim `{claim}` is scoped `{claim_scope}` but cites `{reference}` gathered at \
         `{evidence_scope}`, which does not contain it"
    )]
    ScopeNotContained {
        claim: String,
        claim_scope: &'static str,
        evidence_scope: &'static str,
        reference: String,
    },

    #[error(
        "comparative claim `{claim}` asserts superiority (`{term}`) from a STATIC_SOURCE \
         census with neither an interval nor an explicit unproven-qualifier"
    )]
    UnboundedSuperiority { claim: String, term: &'static str },

    #[error("limitation `{claim}` names no substitution point")]
    LimitationWithoutSubstitutionPoint { claim: String },

    #[error(
        "claim `{claim}` was filed as ATTEMPTED (expected to be refused) but it verifies; \
         it belongs in the allowed set, not the prohibited one"
    )]
    AttemptedClaimUnexpectedlyVerifies { claim: String },
}

impl ClaimRefusal {
    /// The stable rule name recorded in the attack corpus and the prohibited document.
    /// A rule that never appears in that column is indistinguishable from an absent one.
    pub fn rule(&self) -> &'static str {
        match self {
            Self::NoEvidenceReference { .. } => "no_evidence_reference",
            Self::EvidenceDoesNotResolve { .. } => "evidence_does_not_resolve",
            Self::EvidenceLegUnproven { .. } => "evidence_leg_unproven",
            Self::EvidenceIdUnresolved { .. } => "evidence_id_unresolved",
            Self::ConfoundedLegSupportsNoComparison { .. } => {
                "confounded_leg_supports_no_comparison"
            }
            Self::ComparativeWithoutPinnedBaseline { .. } => "comparative_without_pinned_baseline",
            Self::ComparativeWithoutInterval { .. } => "comparative_without_interval",
            Self::DirectionalOnIntervalContainingZero { .. } => {
                "directional_on_interval_containing_zero"
            }
            Self::Misclassification { .. } => "misclassification",
            Self::ScopeNotContained { .. } => "scope_not_contained",
            Self::UnboundedSuperiority { .. } => "unbounded_superiority",
            Self::LimitationWithoutSubstitutionPoint { .. } => {
                "limitation_without_substitution_point"
            }
            Self::AttemptedClaimUnexpectedlyVerifies { .. } => {
                "attempted_claim_unexpectedly_verifies"
            }
        }
    }

    /// What the claim lacked, for the prohibited document's own column.
    pub fn missing(&self) -> String {
        match self {
            Self::NoEvidenceReference { .. } => "any evidence reference at all".into(),
            Self::EvidenceDoesNotResolve { reference, .. } => {
                format!("a resolving reference (`{reference}` does not exist)")
            }
            Self::EvidenceLegUnproven { leg, .. } => format!("a RUN leg (`{leg}` is UNPROVEN)"),
            Self::EvidenceIdUnresolved {
                evidence_id,
                outcome,
                ..
            } => format!("a resolvable citation (`{evidence_id}` is {outcome})"),
            Self::ConfoundedLegSupportsNoComparison { leg, .. } => {
                format!(
                    "an unconfounded measurement (`{leg}` carries a recorded instrument defect)"
                )
            }
            Self::ComparativeWithoutPinnedBaseline { .. } => "a pinned peer baseline token".into(),
            Self::ComparativeWithoutInterval { .. } => "a real confidence interval".into(),
            Self::DirectionalOnIntervalContainingZero { lower, upper, .. } => {
                format!("separation from zero (interval [{lower}, {upper}] contains it)")
            }
            Self::Misclassification { term, .. } => {
                format!("a class consistent with its own text (`{term}`)")
            }
            Self::ScopeNotContained { evidence_scope, .. } => {
                format!("evidence at its own scope (it has only `{evidence_scope}`)")
            }
            Self::UnboundedSuperiority { .. } => {
                "either an interval or an explicit unproven-qualifier".into()
            }
            Self::LimitationWithoutSubstitutionPoint { .. } => "a named substitution point".into(),
            Self::AttemptedClaimUnexpectedlyVerifies { .. } => "nothing — it verifies".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Scope containment
// ---------------------------------------------------------------------------

/// Does evidence gathered at `evidence` support a claim made at `claim`?
///
/// A live-provider measurement supports the narrower readings as well; a scripted-harness
/// measurement supports ONLY a scripted-harness claim; a static-source observation supports
/// ONLY a static-source claim.
///
/// Nothing in Phase 30 produces a `LIVE_PROVIDER` measurement, so in practice this refuses
/// every real-world claim. That is the correct outcome and not a defect to work around.
pub fn scope_contains(evidence: ScopeV1, claim: ScopeV1) -> bool {
    match evidence {
        ScopeV1::LiveProvider => true,
        ScopeV1::ScriptedHarness => claim == ScopeV1::ScriptedHarness,
        ScopeV1::StaticSource => claim == ScopeV1::StaticSource,
    }
}

// ---------------------------------------------------------------------------
// The claim
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimV1 {
    pub id: String,
    pub class: ClaimClassV1,
    pub text: String,
    pub scope: ScopeV1,
    #[serde(default)]
    pub evidence: Vec<ClaimEvidenceRefV1>,
    #[serde(default)]
    pub peer_baseline: Option<String>,
    /// Explicitly-representable absence, mirroring `receipt::Evidence`: bounds that were
    /// never computed must be SAYABLE, or they get written as a plausible zero.
    pub bounds: Evidence<IntervalV1>,
    #[serde(default)]
    pub substitution_point: Option<String>,
}

impl ClaimV1 {
    /// Check every refusal rule, in an order chosen so the FIRST failure is the most
    /// fundamental one — a reader learns "it points at nothing" before "and also its
    /// scope is wrong".
    pub fn verify(&self, repo_root: &Path, tie_band: f64) -> Result<(), ClaimRefusal> {
        self.verify_with_confounds(repo_root, tie_band, &[])
    }

    /// As [`ClaimV1::verify`], plus the recorded-confound rule. The register always calls
    /// this form; the two-argument form exists for corpus cases that declare no confounds.
    pub fn verify_with_confounds(
        &self,
        repo_root: &Path,
        tie_band: f64,
        confounds: &[ConfoundV1],
    ) -> Result<(), ClaimRefusal> {
        // 1. A claim with no evidence reference is refused, whatever its class.
        if self.evidence.is_empty() {
            return Err(ClaimRefusal::NoEvidenceReference {
                claim: self.id.clone(),
            });
        }

        // 2/3. Every reference must resolve, and must not name an UNPROVEN leg.
        for r in &self.evidence {
            self.resolve_reference(r, repo_root)?;
        }

        // 4. Classification consistency. Checked before the comparative requirements so
        //    relabelling cannot skip them.
        if let Some(term) = lexicon_hit(&self.text, COMPARATIVE_LEXICON) {
            let bad_class = match self.class {
                ClaimClassV1::Factual => true,
                // A limitation may discuss a comparison it could not make, but it may not
                // ASSERT one — that would be a superiority claim hiding in the one class
                // that skips the comparative requirements.
                ClaimClassV1::Limitation => is_directional(&self.text),
                ClaimClassV1::Comparative => false,
            };
            if bad_class {
                return Err(ClaimRefusal::Misclassification {
                    claim: self.id.clone(),
                    declared: self.class.token(),
                    term,
                });
            }
        }

        // 5. A limitation must name what would lift it.
        if self.class == ClaimClassV1::Limitation && self.substitution_point.is_none() {
            return Err(ClaimRefusal::LimitationWithoutSubstitutionPoint {
                claim: self.id.clone(),
            });
        }

        // 6. A COMPARISON resting on a leg with a RECORDED INSTRUMENT DEFECT is refused.
        //    The leg is not UNPROVEN — it ran and its number is real — but the number does
        //    not measure the thing its dimension is named after.
        //
        //    This covers EQUIVALENCE as well as direction, and deliberately so: on this
        //    phase's data an equivalence claim is the more dangerous of the two. All three
        //    tools spent an identical 20.00 cost units, but two of them completed 0/30 of
        //    the task, so "cost is indistinguishable" would read as a positive finding
        //    while actually describing equal spend for unequal work.
        //
        //    A FACTUAL, non-directional statement ABOUT the measurement — "two of the three
        //    harnesses scored 0/30 on the identical script" — stays publishable, because it
        //    describes what was observed rather than comparing the products.
        if !confounds.is_empty() {
            let why = if self.class == ClaimClassV1::Comparative {
                Some("declared comparative".to_string())
            } else {
                lexicon_hit(&self.text, DIRECTIONAL_LEXICON).map(|t| format!("directional `{t}`"))
            };
            if let Some(why) = why {
                for r in &self.evidence {
                    if let Some(leg) = r.leg()
                        && let Some(c) = confounds.iter().find(|c| c.leg == leg)
                    {
                        return Err(ClaimRefusal::ConfoundedLegSupportsNoComparison {
                            claim: self.id.clone(),
                            why,
                            leg: leg.to_string(),
                            defect: c.defect.clone(),
                        });
                    }
                }
            }
        }

        if self.class == ClaimClassV1::Comparative {
            self.verify_comparative(tie_band)?;
        }

        // 8. Scope containment. Limitations are exempt: a limitation asserts nothing about
        //    the world, it records that a measurement is ABSENT, so it makes no reach for
        //    containment to bound. Its directional text is already refused above.
        if self.class != ClaimClassV1::Limitation {
            for r in &self.evidence {
                if !scope_contains(r.scope(), self.scope) {
                    return Err(ClaimRefusal::ScopeNotContained {
                        claim: self.id.clone(),
                        claim_scope: self.scope.token(),
                        evidence_scope: r.scope().token(),
                        reference: r.locator(),
                    });
                }
            }
        }

        Ok(())
    }

    fn verify_comparative(&self, tie_band: f64) -> Result<(), ClaimRefusal> {
        // 6. A pinned peer baseline is required of EVERY comparative, at every scope.
        //    A comparison against an unpinned peer is a comparison against nothing.
        let pinned = self
            .peer_baseline
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if pinned.is_none() {
            return Err(ClaimRefusal::ComparativeWithoutPinnedBaseline {
                claim: self.id.clone(),
            });
        }

        // The measured scopes require an interval; the static-source census requires an
        // explicit qualifier instead, because a census has no sampling variance.
        let measured = self
            .evidence
            .iter()
            .any(|r| matches!(r.scope(), ScopeV1::ScriptedHarness | ScopeV1::LiveProvider));

        match (&self.bounds, measured) {
            (Evidence::Unavailable { code }, true) => {
                return Err(ClaimRefusal::ComparativeWithoutInterval {
                    claim: self.id.clone(),
                    scope: self.scope.token(),
                    detail: code.clone(),
                });
            }
            (Evidence::Unavailable { .. }, false) => {
                // 9. Static-source superiority must withhold itself explicitly.
                if let Some(term) = lexicon_hit(&self.text, DIRECTIONAL_LEXICON)
                    && !carries_unproven_qualifier(&self.text)
                {
                    return Err(ClaimRefusal::UnboundedSuperiority {
                        claim: self.id.clone(),
                        term,
                    });
                }
            }
            (Evidence::Observed { value }, _) => {
                // 7. THE DIRECTIONAL RULE — CALLED from frontier_trials, never copied.
                if let Some(term) = lexicon_hit(&self.text, DIRECTIONAL_LEXICON) {
                    let entailed = direction_for(value, tie_band);
                    if !entailed.is_directional() {
                        return Err(ClaimRefusal::DirectionalOnIntervalContainingZero {
                            claim: self.id.clone(),
                            term,
                            lower: value.lower,
                            upper: value.upper,
                            entailed: entailed.token(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve_reference(
        &self,
        r: &ClaimEvidenceRefV1,
        repo_root: &Path,
    ) -> Result<(), ClaimRefusal> {
        match r {
            ClaimEvidenceRefV1::Path { path, .. } => {
                let full = repo_root.join(path);
                if full.exists() {
                    Ok(())
                } else {
                    Err(ClaimRefusal::EvidenceDoesNotResolve {
                        claim: self.id.clone(),
                        reference: path.clone(),
                        detail: format!("no such path: {}", full.display()),
                    })
                }
            }
            ClaimEvidenceRefV1::TrialLeg { leg, legs_tsv, .. } => {
                let full = repo_root.join(legs_tsv);
                let body = std::fs::read_to_string(&full).map_err(|e| {
                    ClaimRefusal::EvidenceDoesNotResolve {
                        claim: self.id.clone(),
                        reference: legs_tsv.clone(),
                        detail: format!("cannot read {}: {e}", full.display()),
                    }
                })?;
                let prefix = format!("{leg}::");
                let row = body
                    .lines()
                    .find(|l| l.starts_with(&prefix))
                    .ok_or_else(|| ClaimRefusal::EvidenceDoesNotResolve {
                        claim: self.id.clone(),
                        reference: format!("{leg} in {legs_tsv}"),
                        detail: "no such leg in the accounting file".into(),
                    })?;
                let fields: Vec<&str> = row.split("::").collect();
                // LEG-NN::tool::dimension::STATUS::evidence=<path>
                let status = fields.get(3).copied().unwrap_or("");
                let capture = fields
                    .get(4)
                    .and_then(|f| f.strip_prefix("evidence="))
                    .unwrap_or("");
                if status == "UNPROVEN" {
                    return Err(ClaimRefusal::EvidenceLegUnproven {
                        claim: self.id.clone(),
                        leg: leg.clone(),
                        blocker: capture.to_string(),
                    });
                }
                if status != "RUN" {
                    return Err(ClaimRefusal::EvidenceDoesNotResolve {
                        claim: self.id.clone(),
                        reference: format!("{leg} in {legs_tsv}"),
                        detail: format!("leg status `{status}` is neither RUN nor UNPROVEN"),
                    });
                }
                // The leg's OWN capture must exist, or "RUN" is an assertion rather than
                // a record.
                let dir = full.parent().unwrap_or(repo_root);
                let cap = dir.join(capture);
                if capture.is_empty() || !cap.exists() {
                    return Err(ClaimRefusal::EvidenceDoesNotResolve {
                        claim: self.id.clone(),
                        reference: format!("{leg} capture `{capture}`"),
                        detail: format!("no such capture: {}", cap.display()),
                    });
                }
                Ok(())
            }
            ClaimEvidenceRefV1::LedgerEvidenceId {
                evidence_id,
                resolution_tsv,
                ..
            } => {
                let full = repo_root.join(resolution_tsv);
                let body = std::fs::read_to_string(&full).map_err(|e| {
                    ClaimRefusal::EvidenceDoesNotResolve {
                        claim: self.id.clone(),
                        reference: resolution_tsv.clone(),
                        detail: format!("cannot read {}: {e}", full.display()),
                    }
                })?;
                let row = body
                    .lines()
                    .find(|l| l.split('\t').next().map(str::trim) == Some(evidence_id.as_str()))
                    .ok_or_else(|| ClaimRefusal::EvidenceDoesNotResolve {
                        claim: self.id.clone(),
                        reference: format!("{evidence_id} in {resolution_tsv}"),
                        detail: "30-01 recorded no determination for this evidence ID".into(),
                    })?;
                let fields: Vec<&str> = row.split('\t').collect();
                let outcome = fields.get(1).copied().unwrap_or("").trim();
                match outcome {
                    // PARTIAL means the artifact is real and only the citation is
                    // imprecise, so it still supports a claim.
                    "CONFIRMED" | "PARTIAL" => Ok(()),
                    other => Err(ClaimRefusal::EvidenceIdUnresolved {
                        claim: self.id.clone(),
                        evidence_id: evidence_id.clone(),
                        outcome: other.to_string(),
                        detail: fields
                            .get(2)
                            .copied()
                            .unwrap_or("no capture recorded")
                            .to_string(),
                    }),
                }
            }
        }
    }

    fn bounds_cell(&self) -> String {
        match &self.bounds {
            Evidence::Observed { value } => format!(
                "[{}, {}] ({:?}, {})",
                value.lower, value.upper, value.method, value.confidence
            ),
            Evidence::Unavailable { code } => format!("UNAVAILABLE: {code}"),
        }
    }

    fn evidence_cell(&self) -> String {
        self.evidence
            .iter()
            .map(|r| format!("`{}` → {}", r.id(), r.locator()))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

// ---------------------------------------------------------------------------
// The register — the ONLY source of a published sentence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimRegisterV1 {
    pub schema: String,
    pub schema_version: u32,
    pub tie_band: f64,
    /// Claims that must ALL verify. If one does not, publication is refused entirely.
    pub claims: Vec<ClaimV1>,
    /// Claims this program actually tried to make and could not support. Every one MUST
    /// be refused; one that verifies is itself an error, because it belongs in `claims`.
    #[serde(default)]
    pub attempted: Vec<ClaimV1>,
    /// Legs whose measurement carries a recorded instrument defect. Declared in the
    /// register — and therefore digest-bound and published — rather than hidden in code,
    /// so a reader can check each one against 30-02's own findings.
    #[serde(default)]
    pub confounded_legs: Vec<ConfoundV1>,
}

/// One refused claim, as rendered into the prohibited document.
pub struct RefusedClaim<'a> {
    pub claim: &'a ClaimV1,
    pub refusal: ClaimRefusal,
}

impl ClaimRegisterV1 {
    /// Verify every allowed claim, and confirm every attempted claim really is refused.
    /// Publication calls this first and does nothing at all if it fails.
    pub fn verify(&self, repo_root: &Path) -> Result<(), ClaimRefusal> {
        for c in &self.claims {
            c.verify_with_confounds(repo_root, self.tie_band, &self.confounded_legs)?;
        }
        for c in &self.attempted {
            if c.verify_with_confounds(repo_root, self.tie_band, &self.confounded_legs)
                .is_ok()
            {
                return Err(ClaimRefusal::AttemptedClaimUnexpectedlyVerifies {
                    claim: c.id.clone(),
                });
            }
        }
        Ok(())
    }

    /// The refusal set the prohibited document is GENERATED from. Hand-writing that list
    /// would be the same defect as hand-writing the allowed one.
    pub fn refusals(&self, repo_root: &Path) -> Vec<RefusedClaim<'_>> {
        let mut out: Vec<RefusedClaim<'_>> = self
            .attempted
            .iter()
            .filter_map(|c| {
                c.verify_with_confounds(repo_root, self.tie_band, &self.confounded_legs)
                    .err()
                    .map(|refusal| RefusedClaim { claim: c, refusal })
            })
            .collect();
        out.sort_by(|a, b| a.claim.id.cmp(&b.claim.id));
        out
    }

    /// Distinct rule names that actually fired. A rule that never fires is
    /// indistinguishable from a rule that is absent.
    pub fn rules_fired(&self, repo_root: &Path) -> BTreeSet<&'static str> {
        self.refusals(repo_root)
            .into_iter()
            .map(|r| r.refusal.rule())
            .collect()
    }

    fn allowed_sorted(&self) -> Vec<&ClaimV1> {
        let mut v: Vec<&ClaimV1> = self
            .claims
            .iter()
            .filter(|c| c.class != ClaimClassV1::Limitation)
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    fn limitations_sorted(&self) -> Vec<&ClaimV1> {
        let mut v: Vec<&ClaimV1> = self
            .claims
            .iter()
            .filter(|c| c.class == ClaimClassV1::Limitation)
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    // -- Rendering. Deterministic: sorted, stable, NO timestamps. A re-render that
    //    differed for any reason other than content would make the regeneration gate
    //    useless, which is the whole tamper-evidence mechanism.

    pub fn render_allowed(&self, digest: &str) -> String {
        let rows = self.allowed_sorted();
        let mut s = String::new();
        let _ = writeln!(s, "# 30-03 — Claims ALLOWED\n");
        let _ = writeln!(
            s,
            "**Rendered by `wayland-scorecard claims publish`. Do not edit.** Every sentence \
             below is rendered from a register entry that survived the checker. A sentence \
             added here by hand fails the on-hardware re-render diff."
        );
        let _ = writeln!(s, "\n- register sha256: `{digest}`");
        let _ = writeln!(s, "- allowed claims: **{}**", rows.len());
        let _ = writeln!(s, "- tie band: `{}`\n", self.tie_band);
        let _ = writeln!(
            s,
            "The allowed set is whatever the evidence supports. It is SMALL, and that is \
             the honest shape of this phase's evidence rather than a shortfall.\n"
        );
        if rows.is_empty() {
            let _ = writeln!(s, "_No claim in the register survived the checker._\n");
        }
        for c in rows {
            let _ = writeln!(s, "## {} — {}\n", c.id, c.class.token());
            let _ = writeln!(s, "> {}\n", c.text.trim());
            let _ = writeln!(s, "| field | value |");
            let _ = writeln!(s, "|---|---|");
            let _ = writeln!(s, "| scope | `{}` |", c.scope.token());
            let _ = writeln!(
                s,
                "| peer baseline | {} |",
                c.peer_baseline
                    .as_deref()
                    .map(|b| format!("`{b}`"))
                    .unwrap_or_else(|| "n/a".into())
            );
            let _ = writeln!(s, "| bounds | {} |", c.bounds_cell());
            let _ = writeln!(s, "| evidence | {} |\n", c.evidence_cell());
        }
        s
    }

    pub fn render_prohibited(&self, repo_root: &Path, digest: &str) -> String {
        let rows = self.refusals(repo_root);
        let mut s = String::new();
        let _ = writeln!(s, "# 30-03 — Claims PROHIBITED\n");
        let _ = writeln!(
            s,
            "**Generated from the checker's refusal set by `wayland-scorecard claims publish`. \
             Do not edit.** These are not promises about what we will avoid saying — a \
             hand-written list of those is worth nothing. Each entry below is a claim this \
             program actually attempted and the checker refused, with the rule that refused \
             it and the evidence it lacked."
        );
        let _ = writeln!(s, "\n- register sha256: `{digest}`");
        let _ = writeln!(s, "- refused claims: **{}**\n", rows.len());
        for r in &rows {
            let _ = writeln!(s, "## {} — REFUSED by `{}`\n", r.claim.id, r.refusal.rule());
            let _ = writeln!(s, "> {}\n", r.claim.text.trim());
            let _ = writeln!(s, "| field | value |");
            let _ = writeln!(s, "|---|---|");
            let _ = writeln!(s, "| declared class | `{}` |", r.claim.class.token());
            let _ = writeln!(s, "| declared scope | `{}` |", r.claim.scope.token());
            let _ = writeln!(s, "| rule | `{}` |", r.refusal.rule());
            let _ = writeln!(s, "| what it lacked | {} |", r.refusal.missing());
            let _ = writeln!(s, "| refusal | {} |\n", r.refusal);
        }
        s
    }

    pub fn render_limitations(&self, digest: &str) -> String {
        let rows = self.limitations_sorted();
        let mut s = String::new();
        let _ = writeln!(s, "# 30-03 — LIMITATIONS\n");
        let _ = writeln!(
            s,
            "**Rendered by `wayland-scorecard claims publish`. Do not edit.** Every dimension \
             this phase could not measure, with its evidence explicitly unavailable and the \
             exact substitution point that would change it. This is not a gap in the report; \
             this IS the report."
        );
        let _ = writeln!(s, "\n- register sha256: `{digest}`");
        let _ = writeln!(s, "- limitations: **{}**\n", rows.len());
        for c in rows {
            let _ = writeln!(s, "## {}\n", c.id);
            let _ = writeln!(s, "> {}\n", c.text.trim());
            let _ = writeln!(s, "| field | value |");
            let _ = writeln!(s, "|---|---|");
            let _ = writeln!(s, "| scope | `{}` |", c.scope.token());
            let _ = writeln!(s, "| evidence | {} |", c.bounds_cell());
            let _ = writeln!(s, "| references | {} |", c.evidence_cell());
            let _ = writeln!(
                s,
                "| substitution point | {} |\n",
                c.substitution_point.as_deref().unwrap_or("—")
            );
        }
        if !self.confounded_legs.is_empty() {
            let _ = writeln!(s, "## Confounded legs — measured, but not measuring\n");
            let _ = writeln!(
                s,
                "These legs RAN and their numbers are real. They are recorded here because \
                 the number does not measure the thing its dimension is named after, so no \
                 directional claim may rest on one — the checker refuses it by rule \
                 `directional_claim_on_confounded_leg`. This is a stronger statement than \
                 UNPROVEN: an unproven leg produced nothing, whereas a confounded leg \
                 produced something that would be READ WRONGLY.\n"
            );
            let _ = writeln!(s, "| leg | defect | evidence | substitution point |");
            let _ = writeln!(s, "|---|---|---|---|");
            let mut cs: Vec<&ConfoundV1> = self.confounded_legs.iter().collect();
            cs.sort_by(|a, b| a.leg.cmp(&b.leg));
            for c in cs {
                let _ = writeln!(
                    s,
                    "| `{}` | {} | {} | {} |",
                    c.leg, c.defect, c.evidence, c.substitution_point
                );
            }
            let _ = writeln!(s);
        }
        s
    }

    /// The machine-checkable limitations index the completeness gate counts.
    pub fn render_limitations_tsv(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "# LIM-NN::<scope>::<substitution point>::<one-line statement>"
        );
        for c in self.limitations_sorted() {
            let _ = writeln!(
                s,
                "{}::{}::{}::{}",
                c.id,
                c.scope.token(),
                c.substitution_point
                    .as_deref()
                    .unwrap_or("UNSTATED")
                    .replace(['\n', '\r'], " "),
                c.text.replace(['\n', '\r'], " ")
            );
        }
        s
    }
}

/// The digest a published document embeds: sha256 over the register's RAW BYTES, so it
/// matches `shasum -a 256 claims-register.json` computed by a gate that never parses it.
pub fn register_digest(register_bytes: &[u8]) -> String {
    protocol_sha256(register_bytes)
}

/// Publish: refuse outright if the register does not verify, then render deterministically.
/// There is no path from an unverified claim to a published document because there is no
/// other function that writes one.
pub fn publish(
    register_bytes: &[u8],
    repo_root: &Path,
    out_dir: &Path,
) -> Result<PublishedSet, anyhow::Error> {
    let register: ClaimRegisterV1 = serde_json::from_slice(register_bytes)?;
    if register.schema != CLAIMS_SCHEMA || register.schema_version != CLAIMS_SCHEMA_VERSION {
        anyhow::bail!(
            "register declares schema `{}` v{}, expected `{CLAIMS_SCHEMA}` v{CLAIMS_SCHEMA_VERSION}",
            register.schema,
            register.schema_version
        );
    }
    register
        .verify(repo_root)
        .map_err(|e| anyhow::anyhow!("register does not verify, publication refused: {e}"))?;

    let digest = register_digest(register_bytes);
    let set = PublishedSet {
        allowed: register.render_allowed(&digest),
        prohibited: register.render_prohibited(repo_root, &digest),
        limitations: register.render_limitations(&digest),
        limitations_tsv: register.render_limitations_tsv(),
        digest,
    };
    std::fs::create_dir_all(out_dir)?;
    std::fs::write(out_dir.join("30-03-CLAIMS-ALLOWED.md"), &set.allowed)?;
    std::fs::write(out_dir.join("30-03-CLAIMS-PROHIBITED.md"), &set.prohibited)?;
    std::fs::write(out_dir.join("30-03-LIMITATIONS.md"), &set.limitations)?;
    std::fs::write(out_dir.join("limitations.tsv"), &set.limitations_tsv)?;
    Ok(set)
}

pub struct PublishedSet {
    pub allowed: String,
    pub prohibited: String,
    pub limitations: String,
    pub limitations_tsv: String,
    pub digest: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lexicon_is_word_boundaried_not_substring() {
        // `misleading` must NOT fire `leading`, or the classifier becomes a nuisance that
        // gets relaxed — which is how a rule drifts permissive.
        assert!(!is_comparative("This is a misleading summary of the run."));
        assert!(is_comparative("Wayland is leading on this dimension."));
        // `already` must not fire `lead`.
        assert!(!is_comparative("The workspace already exists."));
    }

    #[test]
    fn a_static_source_superiority_without_a_qualifier_is_unbounded() {
        assert!(is_directional("Core is superior to Hermes."));
        assert!(!carries_unproven_qualifier("Core is superior to Hermes."));
        // ...and the ledger's own hedged form is bounded.
        assert!(carries_unproven_qualifier(
            "Sandbox/egress: Core architectural lead, operationally unproven"
        ));
    }

    #[test]
    fn scope_containment_is_asymmetric() {
        assert!(scope_contains(
            ScopeV1::ScriptedHarness,
            ScopeV1::ScriptedHarness
        ));
        // The whole point: scripted evidence cannot support a real-world claim.
        assert!(!scope_contains(
            ScopeV1::ScriptedHarness,
            ScopeV1::LiveProvider
        ));
        assert!(!scope_contains(
            ScopeV1::StaticSource,
            ScopeV1::ScriptedHarness
        ));
        assert!(scope_contains(ScopeV1::LiveProvider, ScopeV1::StaticSource));
    }
}
