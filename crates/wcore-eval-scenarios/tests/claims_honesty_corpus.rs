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

// ---------------------------------------------------------------------------
// The paired adversarial corpus (Task 2)
// ---------------------------------------------------------------------------
//
// Built from what this program would ACTUALLY want to say about its own trial
// results, not from invented straw sentences.
//
// Every case is a PAIR carried by one value: a pristine claim the checker accepts and a
// mutation of it the checker refuses. `AttackCase::new` takes both, so a case missing its
// pristine control CANNOT BE CONSTRUCTED — the pairing is enforced by the data structure
// rather than by a reviewer remembering to check for it a dozen times. That is the same
// structural device 29-04 used for its tamper corpus, for the same reason.

use std::fs;

use wcore_eval_scenarios::claims::ConfoundV1;

struct AttackCase {
    id: &'static str,
    what: &'static str,
    pristine: ClaimV1,
    mutation: ClaimV1,
    expected_rule: &'static str,
    confounds: Vec<ConfoundV1>,
}

impl AttackCase {
    /// Both halves are required positionally. There is no constructor that takes only a
    /// mutation, so a refusal-only case is not expressible.
    fn new(
        id: &'static str,
        what: &'static str,
        pristine: ClaimV1,
        mutation: ClaimV1,
        expected_rule: &'static str,
        confounds: Vec<ConfoundV1>,
    ) -> Self {
        Self {
            id,
            what,
            pristine,
            mutation,
            expected_rule,
            confounds,
        }
    }
}

fn evidence_dir() -> PathBuf {
    repo_root().join(".planning/phases/30-continuous-scorecard-frontier-review/evidence/30-03")
}

/// A factual, non-comparative claim resting on a resolving static path.
fn pristine_factual() -> ClaimV1 {
    ClaimV1 {
        id: "P".into(),
        class: ClaimClassV1::Factual,
        text: "30-02's trial accounting records fifteen legs, each naming a capture file.".into(),
        scope: ScopeV1::StaticSource,
        evidence: vec![static_path("LEG-ACCOUNTING", &legs_tsv())],
        peer_baseline: None,
        bounds: Evidence::Unavailable {
            code: "a_census_is_not_a_sampled_quantity".into(),
        },
        substitution_point: None,
    }
}

/// A hedged static-source comparative, i.e. the shape this program already publishes.
fn pristine_hedged_comparative() -> ClaimV1 {
    ClaimV1 {
        id: "P".into(),
        class: ClaimClassV1::Comparative,
        text: "Sandbox/egress: Core architectural lead, operationally unproven".into(),
        scope: ScopeV1::StaticSource,
        evidence: vec![static_path(
            "CTRL01-LEDGER",
            ".planning/intel/COMPETITIVE-LEDGER.md",
        )],
        peer_baseline: Some(PINNED.into()),
        bounds: Evidence::Unavailable {
            code: "static_source_census_has_no_sampling_variance".into(),
        },
        substitution_point: None,
    }
}

fn cases() -> Vec<AttackCase> {
    let confound = |leg: &str| ConfoundV1 {
        leg: leg.to_string(),
        defect: "The canonical script emits a `write_file` tool call, a name only Hermes \
                 exposes; OpenClaw also scored 0/30 on the identical script."
            .to_string(),
        evidence: ".planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md"
            .to_string(),
        substitution_point: "Per-tool dialect compilation, then a re-run.".to_string(),
    };

    let mut v = Vec::new();

    // ATK-01 — the claim with nothing behind it at all.
    v.push(AttackCase::new(
        "ATK-01",
        "an evidence pointer is removed",
        pristine_factual(),
        ClaimV1 {
            evidence: vec![],
            ..pristine_factual()
        },
        "no_evidence_reference",
        vec![],
    ));

    // ATK-02 — the pointer that LOOKS like evidence but opens nothing. This is the
    // single cheapest defect to write and the hardest to see by reading.
    v.push(AttackCase::new(
        "ATK-02",
        "an evidence pointer is repointed at a path that does not exist",
        pristine_factual(),
        ClaimV1 {
            evidence: vec![static_path("GHOST", "evidence/30-02/does-not-exist.tsv")],
            ..pristine_factual()
        },
        "evidence_does_not_resolve",
        vec![],
    ));

    // ATK-03 — the security claim this program would most like to make. It rests on a
    // leg 30-02 recorded UNPROVEN because the meter records digests, not bodies.
    let leg_pristine = ClaimV1 {
        id: "P".into(),
        class: ClaimClassV1::Factual,
        text: "The wayland correctness leg produced a recorded run of thirty trials.".into(),
        scope: ScopeV1::ScriptedHarness,
        evidence: vec![run_leg("WAYLAND-CORRECTNESS", "LEG-01")],
        peer_baseline: None,
        bounds: Evidence::Unavailable {
            code: "a_trial_count_is_a_census".into(),
        },
        substitution_point: None,
    };
    v.push(AttackCase::new(
        "ATK-03",
        "a claim is moved onto a leg recorded UNPROVEN",
        leg_pristine.clone(),
        ClaimV1 {
            text: "No canary value left the harness during the security trials.".into(),
            evidence: vec![run_leg("WAYLAND-SECURITY", "LEG-03")],
            ..leg_pristine.clone()
        },
        "evidence_leg_unproven",
        vec![],
    ));

    // ATK-04 — 30-01's HIGH finding, made mechanical: PEER-PROBE-2026-07-26 names no
    // openable artifact yet carries half the Delta column in six families.
    let id_pristine = ClaimV1 {
        id: "P".into(),
        class: ClaimClassV1::Factual,
        text: "The F03 receipt is a concrete committed object.".into(),
        scope: ScopeV1::StaticSource,
        evidence: vec![ClaimEvidenceRefV1::LedgerEvidenceId {
            id: "F03".into(),
            evidence_id: "F03-RECEIPT@1c644ccd".into(),
            resolution_tsv: format!("{}/evidence/30-01/evidence-id-resolution.tsv", phase_dir()),
            scope: ScopeV1::StaticSource,
        }],
        peer_baseline: None,
        bounds: Evidence::Unavailable {
            code: "object_existence_is_not_a_sampled_quantity".into(),
        },
        substitution_point: None,
    };
    v.push(AttackCase::new(
        "ATK-04",
        "a claim is rested on the CTRL-01 evidence ID that opens nothing",
        id_pristine.clone(),
        ClaimV1 {
            text: "Structural probes confirm the peer baseline shape.".into(),
            evidence: vec![ClaimEvidenceRefV1::LedgerEvidenceId {
                id: "PEER-PROBE".into(),
                evidence_id: "PEER-PROBE-2026-07-26".into(),
                resolution_tsv: format!(
                    "{}/evidence/30-01/evidence-id-resolution.tsv",
                    phase_dir()
                ),
                scope: ScopeV1::StaticSource,
            }],
            ..id_pristine.clone()
        },
        "evidence_id_unresolved",
        vec![],
    ));

    // ATK-05 — THE most important case in this corpus. A comparison built on a leg whose
    // number is real but does not measure its own dimension.
    v.push(AttackCase::new(
        "ATK-05",
        "a comparison is built on a leg carrying a recorded instrument defect",
        pristine_hedged_comparative(),
        ClaimV1 {
            id: "P".into(),
            class: ClaimClassV1::Comparative,
            text: "wayland-core's cost is practically indistinguishable from both peers.".into(),
            scope: ScopeV1::ScriptedHarness,
            evidence: vec![run_leg("WAYLAND-COST", "LEG-04")],
            peer_baseline: Some(PINNED.into()),
            bounds: interval(0.0, 0.0),
            substitution_point: None,
        },
        "confounded_leg_supports_no_comparison",
        vec![confound("LEG-04")],
    ));

    // ATK-06 — a comparison against an unpinned peer is a comparison against nothing.
    v.push(AttackCase::new(
        "ATK-06",
        "the pinned peer baseline is dropped from a comparative",
        pristine_hedged_comparative(),
        ClaimV1 {
            peer_baseline: None,
            ..pristine_hedged_comparative()
        },
        "comparative_without_pinned_baseline",
        vec![],
    ));

    // ATK-07 — a point estimate offered where an interval is required.
    let measured_pristine = ClaimV1 {
        id: "P".into(),
        class: ClaimClassV1::Comparative,
        text: "Hermes completed the scripted task more reliably than wayland-core.".into(),
        scope: ScopeV1::ScriptedHarness,
        evidence: vec![run_leg("HERMES-CORRECTNESS", "LEG-06")],
        peer_baseline: Some(PINNED.into()),
        bounds: interval(-1.0, -0.7),
        substitution_point: None,
    };
    v.push(AttackCase::new(
        "ATK-07",
        "a measured comparative's interval is replaced by a point estimate",
        measured_pristine.clone(),
        ClaimV1 {
            bounds: Evidence::Unavailable {
                code: "point_estimate_only".into(),
            },
            ..measured_pristine.clone()
        },
        "comparative_without_interval",
        vec![],
    ));

    // ATK-08 — the real Wayland-vs-OpenClaw correctness row: [-0.1135, 0.1135].
    v.push(AttackCase::new(
        "ATK-08",
        "a direction is asserted on an interval that straddles zero",
        measured_pristine.clone(),
        ClaimV1 {
            text: "wayland-core is ahead of OpenClaw on scripted correctness.".into(),
            bounds: interval(-0.1135, 0.1135),
            ..measured_pristine.clone()
        },
        "directional_on_interval_containing_zero",
        vec![],
    ));

    // ATK-09 — the one-field dodge. Without this rule every comparative requirement is
    // skipped simply by declaring the sentence factual.
    v.push(AttackCase::new(
        "ATK-09",
        "a comparative sentence is relabelled factual to skip the comparative rules",
        pristine_hedged_comparative(),
        ClaimV1 {
            class: ClaimClassV1::Factual,
            ..pristine_hedged_comparative()
        },
        "misclassification",
        vec![],
    ));

    // ATK-10 — the generalisation this program is most tempted to make: a scripted
    // measurement restated as a fact about real use.
    v.push(AttackCase::new(
        "ATK-10",
        "a scripted-harness result is restated at real-world scope",
        leg_pristine.clone(),
        ClaimV1 {
            scope: ScopeV1::LiveProvider,
            ..leg_pristine.clone()
        },
        "scope_not_contained",
        vec![],
    ));

    // ATK-11 — TRUNCATING A HEDGE IS ITSELF A WAY TO MANUFACTURE AN UNSUPPORTED CLAIM.
    // The mutation is a real ledger fragment, quoted verbatim but severed from the
    // qualifier its family carries. Same evidence, same scope, no hedge.
    v.push(AttackCase::new(
        "ATK-11",
        "a real ledger sentence is quoted with its unproven-qualifier truncated away",
        pristine_hedged_comparative(),
        ClaimV1 {
            text: "This is Core's clearest unique capability".into(),
            ..pristine_hedged_comparative()
        },
        "unbounded_superiority",
        vec![],
    ));

    // ATK-12 — a limitation that records a gap without saying what would close it is
    // just an apology.
    let lim_pristine = ClaimV1 {
        id: "P".into(),
        class: ClaimClassV1::Limitation,
        text: "The security dimension was not measured: the meter records body digests.".into(),
        scope: ScopeV1::ScriptedHarness,
        evidence: vec![static_path("LEG-ACCOUNTING", &legs_tsv())],
        peer_baseline: None,
        bounds: Evidence::Unavailable {
            code: "meter_records_digests_not_bodies".into(),
        },
        substitution_point: Some("Request-body retention under a redaction policy.".into()),
    };
    v.push(AttackCase::new(
        "ATK-12",
        "a limitation's substitution point is removed",
        lim_pristine.clone(),
        ClaimV1 {
            substitution_point: None,
            ..lim_pristine.clone()
        },
        "limitation_without_substitution_point",
        vec![],
    ));

    v
}

/// Strip the absolute worktree path out of a recorded line (#1132).
///
/// A refusal names the path it could not resolve, and that path is absolute:
/// it is `repo_root()` plus the reference. Written verbatim, the capture
/// records the machine it ran on rather than the thing under test, and every
/// lane worktree produces a different one.
fn worktree_independent(text: &str, root: &Path) -> String {
    text.replace(&format!("{}", root.display()), "<repo>")
}

/// Runs every pair and RECORDS the outcome. The TSV is a record of something that ran,
/// not a set of lines someone typed.
///
/// **The captures are written OUTSIDE the repository (#1132).** They used to be
/// written over the tracked evidence files under `.planning/`, so every run in
/// every lane worktree left a modified tracked file holding that worktree's
/// absolute path. `git add -A` — the obvious thing to type — then carried an
/// unrelated diff into an unrelated commit, and two lanes running the suite
/// conflicted over a file neither had touched on purpose. A test that mutates
/// tracked evidence is recording its own environment, not the thing under test;
/// the committed captures stay as the phase's evidence and are read, never
/// rewritten.
#[test]
fn the_attack_corpus_pairs_every_case_and_records_what_fired() {
    let root = repo_root();
    let out = tempfile::tempdir().expect("capture output dir");
    let ev = out.path().to_path_buf();
    let caps = ev.join("attack-captures");
    fs::create_dir_all(&caps).expect("capture dir");
    // The guard, not a comment: a capture directory inside the repository is
    // the defect itself. `evidence_dir()` is still what the corpus POINTS at;
    // it is no longer what the corpus WRITES to.
    assert!(
        !ev.starts_with(&root) && ev != evidence_dir(),
        "captures would be written inside the repository ({}), over the tracked evidence \
         tree at {} — that dirties a tracked file on every run (#1132)",
        ev.display(),
        evidence_dir().display()
    );

    let mut tsv = String::new();
    let mut accepted = 0usize;
    let mut refused = 0usize;
    let mut rules: BTreeSet<String> = BTreeSet::new();

    for c in cases() {
        // -- the pristine half MUST be accepted, or the case proves nothing: a checker
        //    that refuses everything would pass a refusal-only corpus.
        let p = c
            .pristine
            .verify_with_confounds(&root, TIE_BAND_DEFAULT, &c.confounds);
        assert!(
            p.is_ok(),
            "{}: the pristine control MUST be accepted, got {:?}",
            c.id,
            p.err().map(|e| e.to_string())
        );
        let pcap = format!("attack-captures/{}-pristine.txt", c.id);
        fs::write(
            ev.join(&pcap),
            worktree_independent(
                &format!(
                    "case: {}\nwhat: {}\nhalf: PRISTINE\nclass: {}\nscope: {}\ntext: {}\noutcome: \
                 ACCEPTED\nrule: NONE\n",
                    c.id,
                    c.what,
                    c.pristine.class.token(),
                    c.pristine.scope.token(),
                    c.pristine.text
                ),
                &root,
            ),
        )
        .expect("write pristine capture");
        let _ = writeln!(tsv, "{}::ACCEPTED::rule=NONE::evidence={}", c.id, pcap);
        accepted += 1;

        // -- the mutated half MUST be refused, by the rule the case is aimed at.
        let m = c
            .mutation
            .verify_with_confounds(&root, TIE_BAND_DEFAULT, &c.confounds)
            .expect_err("the mutation MUST be refused");
        assert_eq!(
            m.rule(),
            c.expected_rule,
            "{}: refused by the wrong rule: {m}",
            c.id
        );
        let mcap = format!("attack-captures/{}-mutation.txt", c.id);
        fs::write(
            ev.join(&mcap),
            worktree_independent(
                &format!(
                    "case: {}\nwhat: {}\nhalf: MUTATION\nclass: {}\nscope: {}\ntext: {}\noutcome: \
                 REFUSED\nrule: {}\nrefusal: {}\nmissing: {}\n",
                    c.id,
                    c.what,
                    c.mutation.class.token(),
                    c.mutation.scope.token(),
                    c.mutation.text,
                    m.rule(),
                    m,
                    m.missing()
                ),
                &root,
            ),
        )
        .expect("write mutation capture");
        let _ = writeln!(
            tsv,
            "{}::REFUSED::rule={}::evidence={}",
            c.id,
            m.rule(),
            mcap
        );
        refused += 1;
        rules.insert(m.rule().to_string());
    }

    fs::write(ev.join("attack-corpus.tsv"), &tsv).expect("write corpus tsv");

    // Every capture must be readable on a machine that is not this one. The
    // absolute worktree path is the one thing a refusal carries that no other
    // host can reproduce, so a capture holding it is not evidence, it is a
    // fingerprint of the box that ran the suite (#1132).
    let root_str = format!("{}", root.display());
    let mut checked = 0usize;
    for entry in fs::read_dir(&caps).expect("read captures") {
        let path = entry.expect("capture entry").path();
        let body = fs::read_to_string(&path).expect("read capture");
        assert!(
            !body.contains(&root_str),
            "{} embeds the absolute worktree path `{root_str}`, so its content differs on \
             every host — #1132",
            path.display()
        );
        checked += 1;
    }
    // The absence above proves nothing if the loop never ran.
    assert!(
        checked >= 16,
        "only {checked} capture(s) were checked for host-specific content; the scan is \
         reading the wrong directory"
    );

    // A corpus that only refuses is passed by a checker that refuses everything; a
    // corpus that only accepts proves nothing at all. Both halves are asserted.
    assert!(accepted >= 8, "too few accepted rows: {accepted}");
    assert!(refused >= 8, "too few refused rows: {refused}");
    assert!(
        rules.len() >= 8,
        "too few DISTINCT rules actually fired: {} ({rules:?})",
        rules.len()
    );
}

use std::collections::BTreeSet;
use std::fmt::Write as _;
