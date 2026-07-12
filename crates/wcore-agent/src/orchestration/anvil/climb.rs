//! Anvil climb — the DECISION CORE of the gated-forge loop (spec §6.2/§6.3).
//!
//! This slice (A1.5a) is the pure, deterministic heart of the climb: given the
//! per-check results of a gate run, it answers the two questions the loop turns
//! on — *is this candidate an acceptable step?* (fail-set acceptance discipline,
//! spec §6.3, audit consensus HIGH) and *which of several candidates is best?*
//! (the deterministic total order, spec §6.2). It also clusters checks that
//! co-flip so the surgical stage can repair them jointly (§6.3).
//!
//! It is deliberately free of async, spawning, sandbox, and filesystem I/O: the
//! acceptance rule and the candidate order are exactly the places a subtle bug
//! silently ships a false `verified`, so they are isolated here where every edge
//! is unit-tested. The async engine loop (`run_climb` over the builder/gate
//! seams), the append-only journal, the per-workspace lease, and per-candidate
//! worktree isolation build on top of this core in the next slice (A1.5b), and
//! the real spawner/gate wiring + receipt emission land in A1.6. The public
//! `super::drive_climb` facade stays kill-switched until then.
//!
//! Spec: `docs/design/2026-07-12-anvil-native-gated-forge-design.md` (v2) §6.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use sha2::{Digest, Sha256};

use super::gates::BoundedGateOutput;

/// Severity class of a single gate check. The ordering is load-bearing: a climb
/// step is judged on the *worst* failure it leaves standing (see
/// [`evaluate_acceptance`]), and `Ord` here (declaration order, ascending) is
/// that judgement. `Safety` is the untradeable class — a regression that breaks
/// a safety-class check is never accepted, however many cosmetic checks it makes
/// pass (spec §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Formatting, lint style, docs — never blocks correctness.
    Cosmetic,
    /// A real but low-stakes failure (a narrow unit test, a warning-as-error).
    Minor,
    /// A broad correctness failure (build break, a core test suite).
    Major,
    /// Safety/security-class: data loss, auth, memory safety, a migration guard.
    /// A regression here is never tradeable (spec §6.3).
    Safety,
}

impl Severity {
    /// Whether this is the untradeable safety class.
    #[must_use]
    pub fn is_safety_class(self) -> bool {
        matches!(self, Severity::Safety)
    }
}

/// Stable identifier of a single gate check (a test name, lint id, build target).
/// Ordered so the fail-set, its digest, and the candidate order are all
/// deterministic regardless of the order the gate emitted its checks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CheckId(String);

impl CheckId {
    /// Wrap a raw check identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CheckId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for CheckId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl fmt::Display for CheckId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The result of one check within a gate run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    /// Which check this is.
    pub id: CheckId,
    /// Whether it passed on this run.
    pub passed: bool,
    /// How much a failure of this check matters (drives acceptance, spec §6.3).
    pub severity: Severity,
}

impl CheckOutcome {
    /// Convenience constructor.
    pub fn new(id: impl Into<CheckId>, passed: bool, severity: Severity) -> Self {
        Self {
            id: id.into(),
            passed,
            severity,
        }
    }
}

/// The per-check outcome of running the pinned gate against one candidate — the
/// granular view the climb reasons over. (The whole-suite `exit_code` from
/// [`super::gates::GateClosure::probe_baseline`] is retained for the receipt, but
/// acceptance needs the *set* of failing checks, not one aggregate bit.)
#[derive(Debug, Clone)]
pub struct GateReport {
    /// Every check the gate reported, in the order the gate emitted them.
    pub checks: Vec<CheckOutcome>,
    /// The gate process's overall exit code (0 == whole suite green).
    pub exit_code: i32,
    /// Bounded, control-stripped diagnostic tail — the only gate output allowed
    /// to reach a model prompt (injection fencing, spec §5).
    pub diagnostics: BoundedGateOutput,
}

impl GateReport {
    /// The set of failing checks (a check failing in ANY listed outcome is a
    /// fail — a duplicate passing outcome never clears it). This is the value the
    /// acceptance discipline compares.
    #[must_use]
    pub fn fail_set(&self) -> FailSet {
        let mut fails = BTreeMap::new();
        for c in &self.checks {
            if !c.passed {
                // A check id that fails on any outcome is failing; keep the
                // highest severity seen for it so a fail is never under-weighted.
                fails
                    .entry(c.id.clone())
                    .and_modify(|s: &mut Severity| *s = (*s).max(c.severity))
                    .or_insert(c.severity);
            }
        }
        FailSet(fails)
    }

    /// Number of checks that passed on this run.
    #[must_use]
    pub fn passed(&self) -> usize {
        self.checks.iter().filter(|c| c.passed).count()
    }

    /// Total number of checks the gate reported.
    #[must_use]
    pub fn total(&self) -> usize {
        self.checks.len()
    }

    /// Whether every reported check passed. An EMPTY report is never green — a
    /// gate that ran no checks has verified nothing (spec §2 honesty).
    #[must_use]
    pub fn all_green(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.passed)
    }

    /// The climb score = the count of passing checks. Higher is better; it is the
    /// primary key of the candidate order (spec §6.2).
    #[must_use]
    pub fn score(&self) -> u32 {
        // A gate cannot plausibly emit more than u32::MAX checks; saturate rather
        // than risk a wrap on a pathological input.
        u32::try_from(self.passed()).unwrap_or(u32::MAX)
    }
}

/// The set of checks failing on a run, keyed by id with their severity. Cheap to
/// compare; the unit the acceptance rule (spec §6.3) operates on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FailSet(BTreeMap<CheckId, Severity>);

impl FailSet {
    /// Build a fail-set directly from `(id, severity)` pairs.
    pub fn from_pairs(pairs: impl IntoIterator<Item = (CheckId, Severity)>) -> Self {
        Self(pairs.into_iter().collect())
    }

    /// Whether nothing is failing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many checks are failing.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether `id` is in the failing set.
    #[must_use]
    pub fn contains(&self, id: &CheckId) -> bool {
        self.0.contains_key(id)
    }

    /// The failing check ids (sorted).
    pub fn ids(&self) -> impl Iterator<Item = &CheckId> {
        self.0.keys()
    }

    /// Whether every check failing in `self` is also failing in `other` — i.e.
    /// `self` introduces no failure `other` did not already have. This is the
    /// "new fail-set ⊆ old fail-set" test at the core of acceptance (spec §6.3).
    #[must_use]
    pub fn is_subset_of(&self, other: &FailSet) -> bool {
        self.0.keys().all(|id| other.0.contains_key(id))
    }

    /// The severities of the failing checks, sorted WORST-FIRST (descending).
    /// Comparing two of these vectors lexicographically answers "whose worst
    /// standing failure is less bad" — the basis of the severity-Pareto trade in
    /// [`evaluate_acceptance`].
    #[must_use]
    pub fn severity_vector(&self) -> Vec<Severity> {
        let mut v: Vec<Severity> = self.0.values().copied().collect();
        v.sort_unstable_by(|a, b| b.cmp(a));
        v
    }

    /// Checks failing in `self` but not in `other` — the regressions a step
    /// introduces relative to `other`. Ordered by id (deterministic).
    #[must_use]
    pub fn introduced_vs(&self, other: &FailSet) -> Vec<(CheckId, Severity)> {
        self.0
            .iter()
            .filter(|(id, _)| !other.0.contains_key(id))
            .map(|(id, sev)| (id.clone(), *sev))
            .collect()
    }

    /// A stable 64-bit digest of the failing-id SET (independent of insertion
    /// order — the keys are already sorted). Used purely as a deterministic
    /// tie-break in the candidate order so the winner never depends on the order
    /// candidates were produced.
    #[must_use]
    pub fn fail_hash(&self) -> u64 {
        let mut hasher = Sha256::new();
        for id in self.0.keys() {
            hasher.update(id.as_str().as_bytes());
            // NUL separator so ["ab","c"] and ["a","bc"] never collide.
            hasher.update([0u8]);
        }
        let digest = hasher.finalize();
        let mut first8 = [0u8; 8];
        first8.copy_from_slice(&digest[..8]);
        u64::from_be_bytes(first8)
    }
}

/// The verdict on whether a climb step may be kept (spec §6.3 acceptance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Acceptance {
    /// Keep the candidate. `strict` is true when it strictly improved — it
    /// eliminated at least one failure with no regression, or it reduced the
    /// worst standing severity. A non-strict accept is a lateral move (the same
    /// fail-set), kept because it regressed nothing but counted toward plateau
    /// detection by the loop (A1.5b).
    Accept {
        /// Whether the step made strict progress (vs. a lateral, no-regression move).
        strict: bool,
    },
    /// Reject the candidate; the reason is logged (spec §6.3 "rejected trade-ups
    /// are logged").
    Reject(RejectReason),
}

/// Why a candidate step was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// A safety-class check that was NOT failing in the base now fails. Never
    /// tradeable, regardless of what else the step fixed (spec §6.3).
    SafetyRegression(CheckId),
    /// The step introduced non-safety regressions and its worst-first severity
    /// profile is not strictly better than the base's — so it is not a valid
    /// severity trade.
    NotAnImprovement,
}

/// Decide whether `candidate`'s fail-set is an acceptable step from `base`'s
/// (spec §6.3, audit consensus HIGH). The rule, in order:
///
/// 1. **Subset (the common case).** If the candidate introduces NO new failure
///    (its fail-set ⊆ the base's), keep it. It is `strict` when it also cleared
///    at least one failure; equal fail-sets are a lateral, non-strict accept.
/// 2. **Safety is untradeable.** If the candidate introduces a *safety-class*
///    failure the base did not have, reject — no amount of other progress buys
///    a safety regression.
/// 3. **Severity trade (worst-first).** Otherwise the candidate introduced only
///    non-safety regressions; accept it only if its severity profile, compared
///    WORST-FAILURE-FIRST, is strictly better than the base's — i.e. it may take
///    on lower-severity failures only in exchange for eliminating a strictly
///    higher-severity one. (This is the deterministic reading of the spec's
///    "Pareto improvement on the severity vector": the max standing severity must
///    strictly drop. A transient regression that is NOT such an improvement — a
///    mid-migration compile break — is handled by the loop's bounded multi-step
///    transaction path, §6.3, not by single-step acceptance.)
#[must_use]
pub fn evaluate_acceptance(base: &FailSet, candidate: &FailSet) -> Acceptance {
    let introduced = candidate.introduced_vs(base);

    if introduced.is_empty() {
        // candidate ⊆ base: no new failures. Strict iff it cleared something.
        let cleared_something = candidate.len() < base.len();
        return Acceptance::Accept {
            strict: cleared_something,
        };
    }

    // Regressions exist. A safety-class regression ends it immediately.
    if let Some((id, _)) = introduced.iter().find(|(_, sev)| sev.is_safety_class()) {
        return Acceptance::Reject(RejectReason::SafetyRegression(id.clone()));
    }

    // Non-safety regressions only: allow the trade iff the worst-first severity
    // profile strictly improves (lexicographic compare of the descending vectors;
    // shorter — fewer fails — wins on an otherwise-equal prefix).
    if candidate.severity_vector() < base.severity_vector() {
        Acceptance::Accept { strict: true }
    } else {
        Acceptance::Reject(RejectReason::NotAnImprovement)
    }
}

/// Identifier of a candidate build within a climb (a builder/attempt id).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CandidateId(String);

impl CandidateId {
    /// Wrap a raw candidate identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CandidateId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// One candidate build and how it did against the gate.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Which builder/attempt produced it.
    pub id: CandidateId,
    /// Its per-check gate result.
    pub report: GateReport,
    /// What it cost to produce (microcents) — the last tie-break in the order.
    pub cost_microcents: u64,
}

impl Candidate {
    /// The deterministic rank key (spec §6.2). See [`RankKey`].
    #[must_use]
    pub fn rank_key(&self) -> RankKey {
        let fails = self.report.fail_set();
        RankKey {
            neg_score: Reverse(self.report.score()),
            fail_count: fails.len(),
            severity_desc: fails.severity_vector(),
            fail_hash: fails.fail_hash(),
            cost_microcents: self.cost_microcents,
            id: self.id.clone(),
        }
    }
}

/// The total order on candidates (spec §6.2): `(score, |fails|, severity-weighted
/// fail vector, fail-id hash, cost)`. `Ord` is defined so the LEAST key is the
/// BEST candidate — [`best`] selects the minimum. It is a genuine *total* order:
/// score/fail-count/severity are the substance, and the fail-id hash, cost, and
/// finally the candidate id break every remaining tie, so the winner never
/// depends on the order candidates were generated.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RankKey {
    /// Higher score is better, so it is reversed: a smaller key wins.
    neg_score: Reverse<u32>,
    /// Fewer failing checks is better.
    fail_count: usize,
    /// Worst-first severities; a worse top severity sorts later (loses).
    severity_desc: Vec<Severity>,
    /// Deterministic tie-break across distinct fail SETS of equal shape.
    fail_hash: u64,
    /// Cheaper is better (only reached when the fail profile is identical).
    cost_microcents: u64,
    /// Ultimate tie-break so identical reports still order deterministically.
    id: CandidateId,
}

/// Select the single best candidate under the deterministic total order (spec
/// §6.2). `None` for an empty slate. Because [`RankKey`] is a total order, the
/// choice is independent of the input order.
#[must_use]
pub fn best(candidates: &[Candidate]) -> Option<&Candidate> {
    candidates.iter().min_by_key(|c| c.rank_key())
}

/// Group checks that co-flip across `observations` so the surgical stage repairs
/// them jointly (spec §6.3 coupled-failure clustering). Two checks are coupled
/// when they fail in exactly the same subset of observations — a shared root
/// cause moves them together. Each returned cluster is a set of check ids with an
/// identical failing-observation signature; a check that never fails in any
/// observation is not part of any cluster. Deterministic (ordered by signature).
#[must_use]
pub fn coupled_clusters(observations: &[FailSet]) -> Vec<BTreeSet<CheckId>> {
    // signature (sorted observation indices where the check fails) -> cluster.
    let mut by_signature: BTreeMap<Vec<usize>, BTreeSet<CheckId>> = BTreeMap::new();
    // The universe of checks that failed at least once.
    let mut universe: BTreeSet<&CheckId> = BTreeSet::new();
    for obs in observations {
        universe.extend(obs.ids());
    }
    for id in universe {
        let signature: Vec<usize> = observations
            .iter()
            .enumerate()
            .filter(|(_, obs)| obs.contains(id))
            .map(|(i, _)| i)
            .collect();
        // A check in `universe` failed somewhere, so its signature is non-empty.
        by_signature
            .entry(signature)
            .or_default()
            .insert(id.clone());
    }
    by_signature.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(id: &str, passed: bool, sev: Severity) -> CheckOutcome {
        CheckOutcome::new(id, passed, sev)
    }

    fn report(checks: Vec<CheckOutcome>, exit: i32) -> GateReport {
        GateReport {
            checks,
            exit_code: exit,
            diagnostics: BoundedGateOutput::from_bytes(b""),
        }
    }

    fn failset(pairs: &[(&str, Severity)]) -> FailSet {
        FailSet::from_pairs(pairs.iter().map(|(id, s)| (CheckId::from(*id), *s)))
    }

    // ── Severity / gate model ────────────────────────────────────────────────

    #[test]
    fn severity_orders_cosmetic_below_safety() {
        assert!(Severity::Cosmetic < Severity::Minor);
        assert!(Severity::Minor < Severity::Major);
        assert!(Severity::Major < Severity::Safety);
        assert!(Severity::Safety.is_safety_class());
        assert!(!Severity::Major.is_safety_class());
    }

    #[test]
    fn empty_report_is_never_green() {
        let r = report(vec![], 0);
        assert!(!r.all_green(), "a gate that ran no checks verified nothing");
        assert_eq!(r.score(), 0);
    }

    #[test]
    fn all_green_requires_every_check_pass() {
        let green = report(
            vec![
                out("a", true, Severity::Major),
                out("b", true, Severity::Minor),
            ],
            0,
        );
        assert!(green.all_green());
        assert_eq!(green.score(), 2);

        let red = report(
            vec![
                out("a", true, Severity::Major),
                out("b", false, Severity::Minor),
            ],
            1,
        );
        assert!(!red.all_green());
        assert_eq!(red.score(), 1);
    }

    #[test]
    fn fail_set_keeps_highest_severity_on_duplicate_id() {
        // Same id reported twice failing at different severities → the worst wins.
        let r = report(
            vec![
                out("dup", false, Severity::Minor),
                out("dup", false, Severity::Safety),
                out("ok", true, Severity::Major),
            ],
            1,
        );
        let fs = r.fail_set();
        assert_eq!(fs.len(), 1);
        assert_eq!(fs.severity_vector(), vec![Severity::Safety]);
    }

    #[test]
    fn fail_set_passing_duplicate_never_clears_a_fail() {
        let r = report(
            vec![
                out("x", false, Severity::Major),
                out("x", true, Severity::Major),
            ],
            1,
        );
        assert!(r.fail_set().contains(&CheckId::from("x")));
    }

    // ── Acceptance discipline (spec §6.3) ────────────────────────────────────

    #[test]
    fn subset_with_a_fix_is_a_strict_accept() {
        let base = failset(&[("a", Severity::Major), ("b", Severity::Minor)]);
        let cand = failset(&[("a", Severity::Major)]); // fixed b, no new fails
        assert_eq!(
            evaluate_acceptance(&base, &cand),
            Acceptance::Accept { strict: true }
        );
    }

    #[test]
    fn equal_fail_set_is_a_lateral_non_strict_accept() {
        let base = failset(&[("a", Severity::Major)]);
        let cand = failset(&[("a", Severity::Major)]);
        assert_eq!(
            evaluate_acceptance(&base, &cand),
            Acceptance::Accept { strict: false }
        );
    }

    #[test]
    fn green_candidate_from_failing_base_is_strict_accept() {
        let base = failset(&[("a", Severity::Major)]);
        let cand = FailSet::default(); // all green
        assert_eq!(
            evaluate_acceptance(&base, &cand),
            Acceptance::Accept { strict: true }
        );
    }

    #[test]
    fn introducing_a_safety_regression_is_always_rejected() {
        // Base has two cosmetic fails; candidate clears BOTH but breaks safety.
        let base = failset(&[("fmt1", Severity::Cosmetic), ("fmt2", Severity::Cosmetic)]);
        let cand = failset(&[("auth", Severity::Safety)]);
        assert_eq!(
            evaluate_acceptance(&base, &cand),
            Acceptance::Reject(RejectReason::SafetyRegression(CheckId::from("auth"))),
        );
    }

    #[test]
    fn trading_a_major_for_a_minor_is_a_valid_severity_improvement() {
        // Eliminates the Major, introduces a Minor → worst severity drops.
        let base = failset(&[("build", Severity::Major)]);
        let cand = failset(&[("lint", Severity::Minor)]);
        assert_eq!(
            evaluate_acceptance(&base, &cand),
            Acceptance::Accept { strict: true }
        );
    }

    #[test]
    fn trading_a_minor_for_a_major_is_rejected() {
        // Introduces a strictly WORSE failure — not an improvement.
        let base = failset(&[("lint", Severity::Minor)]);
        let cand = failset(&[("build", Severity::Major)]);
        assert_eq!(
            evaluate_acceptance(&base, &cand),
            Acceptance::Reject(RejectReason::NotAnImprovement),
        );
    }

    #[test]
    fn same_worst_severity_but_more_fails_is_rejected() {
        // Worst stays Major, but the candidate adds a second Major → not better.
        let base = failset(&[("b1", Severity::Major)]);
        let cand = failset(&[("b1", Severity::Major), ("b2", Severity::Major)]);
        assert_eq!(
            evaluate_acceptance(&base, &cand),
            Acceptance::Reject(RejectReason::NotAnImprovement),
        );
    }

    // ── Deterministic total order (spec §6.2) ────────────────────────────────

    fn cand(id: &str, checks: Vec<CheckOutcome>, cost: u64) -> Candidate {
        Candidate {
            id: CandidateId::from(id),
            report: report(checks, 0),
            cost_microcents: cost,
        }
    }

    #[test]
    fn best_prefers_higher_score_then_fewer_fails() {
        let low = cand(
            "low",
            vec![
                out("a", true, Severity::Minor),
                out("b", false, Severity::Minor),
            ],
            10,
        );
        let high = cand(
            "high",
            vec![
                out("a", true, Severity::Minor),
                out("b", true, Severity::Minor),
            ],
            999, // costlier, but score wins first
        );
        let slate = vec![low, high];
        assert_eq!(best(&slate).unwrap().id.as_str(), "high");
    }

    #[test]
    fn best_prefers_lower_worst_severity_at_equal_score() {
        // Both score 1/2 (one fail each) but differ in the failing severity.
        let worse = cand(
            "worse",
            vec![
                out("a", true, Severity::Minor),
                out("b", false, Severity::Safety),
            ],
            5,
        );
        let better = cand(
            "better",
            vec![
                out("a", true, Severity::Minor),
                out("b", false, Severity::Cosmetic),
            ],
            5,
        );
        let slate = vec![worse, better];
        assert_eq!(best(&slate).unwrap().id.as_str(), "better");
    }

    #[test]
    fn best_breaks_a_true_tie_by_cost() {
        // Identical fail profile (same id + severity), differ only in cost.
        let dear = cand("dear", vec![out("a", false, Severity::Minor)], 100);
        let cheap = cand("cheap", vec![out("a", false, Severity::Minor)], 10);
        let slate = vec![dear, cheap];
        assert_eq!(best(&slate).unwrap().id.as_str(), "cheap");
    }

    #[test]
    fn best_is_independent_of_input_order() {
        let mk = || {
            vec![
                cand("c", vec![out("a", true, Severity::Minor)], 3),
                cand("a", vec![out("a", false, Severity::Minor)], 1),
                cand("b", vec![out("a", true, Severity::Minor)], 9),
            ]
        };
        let mut forward = mk();
        let mut reversed = mk();
        reversed.reverse();
        assert_eq!(
            best(&forward).unwrap().id.as_str(),
            best(&reversed).unwrap().id.as_str(),
        );
        // Sorting by the key is likewise stable regardless of starting order.
        forward.sort_by_key(Candidate::rank_key);
        reversed.sort_by_key(Candidate::rank_key);
        let f: Vec<_> = forward.iter().map(|c| c.id.as_str()).collect();
        let r: Vec<_> = reversed.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(f, r);
    }

    #[test]
    fn best_of_empty_slate_is_none() {
        assert!(best(&[]).is_none());
    }

    // ── Coupled-failure clustering (spec §6.3) ───────────────────────────────

    #[test]
    fn checks_that_co_flip_cluster_together() {
        // a & b always fail together; c fails alone in obs 1.
        let observations = vec![
            failset(&[("a", Severity::Major), ("b", Severity::Major)]),
            failset(&[
                ("a", Severity::Major),
                ("b", Severity::Major),
                ("c", Severity::Minor),
            ]),
        ];
        let clusters = coupled_clusters(&observations);
        assert!(
            clusters.contains(&BTreeSet::from([CheckId::from("a"), CheckId::from("b")])),
            "a and b share a signature and must cluster: {clusters:?}",
        );
        assert!(clusters.contains(&BTreeSet::from([CheckId::from("c")])));
        assert_eq!(clusters.len(), 2);
    }

    #[test]
    fn never_failing_checks_are_not_clustered() {
        let observations = vec![failset(&[("a", Severity::Major)]), FailSet::default()];
        let clusters = coupled_clusters(&observations);
        // Only `a` ever failed.
        assert_eq!(clusters, vec![BTreeSet::from([CheckId::from("a")])]);
    }

    #[test]
    fn no_observations_yields_no_clusters() {
        assert!(coupled_clusters(&[]).is_empty());
    }
}
