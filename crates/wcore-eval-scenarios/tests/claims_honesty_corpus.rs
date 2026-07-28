//! Phase 30 (F30-04) — the paired adversarial claims corpus.
//!
//! Eight refusals and two acceptances, named exactly as `30-03-PLAN.md` requires.
//!
//! **The two acceptances are load-bearing and are not optional.** A corpus that only ever
//! refuses is passed trivially by a checker that refuses everything — which would block every
//! honest claim in the program while looking like rigour. The control sentences are therefore
//! taken VERBATIM from `.planning/intel/COMPETITIVE-LEDGER.md`'s own Delta column: real,
//! comparative, evidence-bound, hedged, and correct. If one of them is ever refused, the
//! CHECKER is wrong and the checker gets fixed. The control sentence is never edited.

use std::path::{Path, PathBuf};

use wcore_eval_scenarios::claims::{
    ClaimClassV1, ClaimEvidenceRefV1, ClaimRefusal, ClaimV1, TIE_BAND_DEFAULT,
};
use wcore_eval_scenarios::frontier_trials::{IntervalMethodV1, IntervalV1, ScopeV1};
use wcore_eval_scenarios::receipt::Evidence;

/// The repository root, found by walking up from the crate manifest.
fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/wcore-eval-scenarios -> crates -> <root>
    p.pop();
    p.pop();
    p
}

fn phase_dir() -> String {
    ".planning/phases/30-continuous-scorecard-frontier-review".to_string()
}

fn legs_tsv() -> String {
    format!("{}/evidence/30-02/legs.tsv", phase_dir())
}

/// A `SCRIPTED_HARNESS` evidence reference pointing at a leg 30-02 recorded RUN.
fn run_leg(id: &str, leg: &str) -> ClaimEvidenceRefV1 {
    ClaimEvidenceRefV1::TrialLeg {
        id: id.to_string(),
        leg: leg.to_string(),
        legs_tsv: legs_tsv(),
        scope: ScopeV1::ScriptedHarness,
    }
}

/// A `STATIC_SOURCE` evidence reference pointing at a file that really exists.
fn static_path(id: &str, path: &str) -> ClaimEvidenceRefV1 {
    ClaimEvidenceRefV1::Path {
        id: id.to_string(),
        path: path.to_string(),
        scope: ScopeV1::StaticSource,
    }
}

fn interval(lower: f64, upper: f64) -> Evidence<IntervalV1> {
    Evidence::observed(IntervalV1 {
        lower,
        upper,
        method: IntervalMethodV1::NewcombeWilson95,
        confidence: 0.95,
    })
}

/// The pinned peer baseline token CTRL-01 declares. Not a free-text peer name.
const PINNED: &str = "BASE-2026-07-13";

fn verify(claim: &ClaimV1) -> Result<(), ClaimRefusal> {
    claim.verify(Path::new(&repo_root()), TIE_BAND_DEFAULT)
}

// ---------------------------------------------------------------------------
// The eight refusals
// ---------------------------------------------------------------------------

#[test]
fn a_claim_with_no_evidence_reference_is_refused() {
    let claim = ClaimV1 {
        id: "T-01".into(),
        class: ClaimClassV1::Factual,
        text: "wayland-core ships a delegated workspace lifecycle.".into(),
        scope: ScopeV1::StaticSource,
        evidence: vec![],
        peer_baseline: None,
        bounds: Evidence::Unavailable {
            code: "not_a_measurement".into(),
        },
        substitution_point: None,
    };
    let err = verify(&claim).expect_err("a claim with no evidence pointer must be refused");
    assert_eq!(err.rule(), "no_evidence_reference", "got: {err}");
}

#[test]
fn an_evidence_reference_that_does_not_resolve_is_refused() {
    let claim = ClaimV1 {
        id: "T-02".into(),
        class: ClaimClassV1::Factual,
        text: "wayland-core ships a delegated workspace lifecycle.".into(),
        scope: ScopeV1::StaticSource,
        evidence: vec![static_path(
            "GHOST",
            ".planning/phases/30-continuous-scorecard-frontier-review/evidence/30-03/does-not-exist.json",
        )],
        peer_baseline: None,
        bounds: Evidence::Unavailable {
            code: "not_a_measurement".into(),
        },
        substitution_point: None,
    };
    let err = verify(&claim).expect_err("an unresolving pointer must be refused");
    assert_eq!(err.rule(), "evidence_does_not_resolve", "got: {err}");
    // The refusal must NAME the reference; a refusal that hides which pointer failed
    // sends the reader back to guessing.
    assert!(
        err.to_string().contains("does-not-exist.json"),
        "the refusal must name the offending reference, got: {err}"
    );
}

#[test]
fn a_claim_resting_on_an_unproven_leg_is_refused() {
    // LEG-03 is wayland::security, which 30-02 recorded UNPROVEN: the shared meter
    // records body DIGESTS, not bodies, so the extraction the protocol specified was
    // never possible. No wording of a claim can repair that.
    let claim = ClaimV1 {
        id: "T-03".into(),
        class: ClaimClassV1::Factual,
        text: "No canary value left the harness during the security trials.".into(),
        scope: ScopeV1::ScriptedHarness,
        evidence: vec![run_leg("SEC", "LEG-03")],
        peer_baseline: None,
        bounds: interval(0.0, 0.1135),
        substitution_point: None,
    };
    let err = verify(&claim).expect_err("a claim resting on an UNPROVEN leg must be refused");
    assert_eq!(err.rule(), "evidence_leg_unproven", "got: {err}");
    assert!(err.to_string().contains("LEG-03"), "got: {err}");
}

#[test]
fn a_comparative_claim_without_a_pinned_peer_baseline_is_refused() {
    let claim = ClaimV1 {
        id: "T-04".into(),
        class: ClaimClassV1::Comparative,
        text: "Hermes completed the scripted task more reliably than Wayland.".into(),
        scope: ScopeV1::ScriptedHarness,
        evidence: vec![run_leg("CORR", "LEG-06")],
        peer_baseline: None,
        bounds: interval(-1.0, -0.7),
        substitution_point: None,
    };
    let err = verify(&claim).expect_err("a comparative without a pinned baseline must be refused");
    assert_eq!(
        err.rule(),
        "comparative_without_pinned_baseline",
        "got: {err}"
    );
}

#[test]
fn a_comparative_claim_without_an_interval_is_refused() {
    // A point estimate is NOT an interval. `Evidence::Unavailable` is the honest
    // representation of "no bounds were computed", and it is refused here rather than
    // silently rendered as precision.
    let claim = ClaimV1 {
        id: "T-05".into(),
        class: ClaimClassV1::Comparative,
        text: "Hermes completed the scripted task more reliably than Wayland.".into(),
        scope: ScopeV1::ScriptedHarness,
        evidence: vec![run_leg("CORR", "LEG-06")],
        peer_baseline: Some(PINNED.into()),
        bounds: Evidence::Unavailable {
            code: "point_estimate_only".into(),
        },
        substitution_point: None,
    };
    let err = verify(&claim).expect_err("a measured comparative without an interval is refused");
    assert_eq!(err.rule(), "comparative_without_interval", "got: {err}");
}

#[test]
fn a_comparative_claim_whose_interval_contains_zero_is_refused() {
    // This is the Wayland-vs-OpenClaw correctness row: [-0.1135, 0.1135]. It contains
    // zero AND is more than twice the tie band, so 30-02 published it INCONCLUSIVE. A
    // directional sentence built on it must not be constructible here either.
    //
    // The rule is CALLED from `frontier_trials`, never copied — a second copy is a
    // second chance for one of them to drift permissive while the tests point at the other.
    let claim = ClaimV1 {
        id: "T-06".into(),
        class: ClaimClassV1::Comparative,
        text: "Wayland is ahead of OpenClaw on scripted correctness.".into(),
        scope: ScopeV1::ScriptedHarness,
        evidence: vec![run_leg("CORR", "LEG-01")],
        peer_baseline: Some(PINNED.into()),
        bounds: interval(-0.1135, 0.1135),
        substitution_point: None,
    };
    let err = verify(&claim).expect_err("a direction on an interval containing zero is refused");
    assert_eq!(
        err.rule(),
        "directional_on_interval_containing_zero",
        "got: {err}"
    );
}

#[test]
fn a_comparative_sentence_declared_factual_is_refused_for_misclassification() {
    // Without this rule the entire mechanism is defeated by editing ONE field: every
    // comparative requirement is skipped simply by declaring the sentence factual.
    let claim = ClaimV1 {
        id: "T-07".into(),
        class: ClaimClassV1::Factual,
        text: "Wayland is faster and more reliable than Hermes.".into(),
        scope: ScopeV1::ScriptedHarness,
        evidence: vec![run_leg("CORR", "LEG-01")],
        peer_baseline: Some(PINNED.into()),
        bounds: interval(0.5, 0.9),
        substitution_point: None,
    };
    let err = verify(&claim).expect_err("relabelling a comparative as factual must be refused");
    assert_eq!(err.rule(), "misclassification", "got: {err}");
}

#[test]
fn a_claim_scoped_beyond_its_evidence_is_refused() {
    // THE most important rule in the module. Every measurement in Phase 30 holds the
    // model constant by construction, so it is scoped SCRIPTED_HARNESS. Nothing in this
    // phase produces a LIVE_PROVIDER measurement, so every real-world claim is refused
    // by construction — which is the correct outcome, not a defect to work around.
    let claim = ClaimV1 {
        id: "T-08".into(),
        class: ClaimClassV1::Comparative,
        text: "Hermes is more reliable than Wayland in real-world use.".into(),
        scope: ScopeV1::LiveProvider,
        evidence: vec![run_leg("CORR", "LEG-06")],
        peer_baseline: Some(PINNED.into()),
        bounds: interval(-1.0, -0.7),
        substitution_point: None,
    };
    let err = verify(&claim).expect_err("a claim reaching beyond its evidence's scope is refused");
    assert_eq!(err.rule(), "scope_not_contained", "got: {err}");
}

// ---------------------------------------------------------------------------
// The two pristine controls — WITHOUT THESE THE CORPUS PROVES NOTHING
// ---------------------------------------------------------------------------

#[test]
fn a_limitation_with_unavailable_evidence_verifies() {
    // A limitation is the one shape that MUST be publishable with explicitly
    // unavailable evidence, because "we could not measure this" is the single most
    // important thing this phase has to say. If this test fails, the checker has
    // become a device for hiding gaps.
    let claim = ClaimV1 {
        id: "LIM-CONTROL".into(),
        class: ClaimClassV1::Limitation,
        text: "The security dimension was not measured: the shared meter records request \
               body digests, not bodies, so the protocol's byte-search extraction was never \
               possible."
            .into(),
        scope: ScopeV1::ScriptedHarness,
        evidence: vec![static_path(
            "SEC-BLOCKER",
            &format!("{}/evidence/30-02/legs.tsv", phase_dir()),
        )],
        peer_baseline: None,
        bounds: Evidence::Unavailable {
            code: "meter_records_digests_not_bodies".into(),
        },
        substitution_point: Some(
            "Request-body retention under a redaction policy, or leaf-hash exposure, in \
             crates/wcore-eval-scenarios/src/fixtures/openai.rs — the open seam request."
                .into(),
        ),
    };
    verify(&claim).expect("a limitation with explicitly unavailable evidence must verify");
}

#[test]
fn a_hedged_evidence_bound_ledger_sentence_verifies() {
    // VERBATIM from `.planning/intel/COMPETITIVE-LEDGER.md`'s Delta column. These are
    // real comparative sentences this program already publishes, and they are correct:
    // each is bound to a named evidence ID, a pinned peer baseline, and — crucially —
    // carries its own explicit unproven-qualifier, which is what bounds a static-source
    // comparison that has no sampling variance to put an interval around.
    //
    // If either is refused, the checker is a banned-words list wearing a better name.
    // Fix the checker. NEVER the control sentence.
    let ledger = ".planning/intel/COMPETITIVE-LEDGER.md";

    let auth = ClaimV1 {
        id: "CTL-AUTH".into(),
        class: ClaimClassV1::Comparative,
        // AUTH-* Delta, verbatim.
        text: "Sandbox/egress: Core architectural lead, operationally unproven".into(),
        scope: ScopeV1::StaticSource,
        evidence: vec![static_path("CTRL01-LEDGER", ledger)],
        peer_baseline: Some(PINNED.into()),
        bounds: Evidence::Unavailable {
            code: "static_source_census_has_no_sampling_variance".into(),
        },
        substitution_point: None,
    };
    verify(&auth).expect("the AUTH-* delta sentence must verify verbatim");

    let supply = ClaimV1 {
        id: "CTL-SUPPLY".into(),
        class: ClaimClassV1::Comparative,
        // SUPPLY-* Delta, verbatim.
        text: "Neither peer ships an SBOM at baseline, so Core's F29-01 SBOM requirement \
               has no counterpart to match — it would be a lead if proven"
            .into(),
        scope: ScopeV1::StaticSource,
        evidence: vec![static_path("CTRL01-LEDGER", ledger)],
        peer_baseline: Some(PINNED.into()),
        bounds: Evidence::Unavailable {
            code: "static_source_census_has_no_sampling_variance".into(),
        },
        substitution_point: None,
    };
    verify(&supply).expect("the SUPPLY-* delta sentence must verify verbatim");
}
