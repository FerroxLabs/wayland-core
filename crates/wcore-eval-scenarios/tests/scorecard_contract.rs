//! Phase 30 scorecard contract suite (F30-01, F30-02).
//!
//! Every refusal test first ACCEPTS a pristine control and only then applies the
//! mutation, because a rejection-only suite passes just as happily against a
//! verifier that rejects everything — which would block the honest path while
//! looking rigorous.

use std::path::{Path, PathBuf};

use wcore_eval_scenarios::scorecard::{
    ALL_MATURITY_STATES, CriterionV1, CriterionVerdictV1, EvidenceLocatorV1, EvidenceRefV1,
    MaturityTruthV1, MaturityV1, MeasurementStateV1, ScorecardDocumentV1, SurfaceRowV1, TruthV1,
    parse_subcommands, render_surfaces_tsv, walk_command_tree,
};

/// The repository root, derived from the crate manifest rather than from the
/// process cwd, which nextest does not guarantee.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/<crate>/ has a grandparent")
        .to_path_buf()
}

/// A path that certainly exists in this repository, used as resolving evidence.
const REAL_PATH: &str = "crates/wcore-eval-scenarios/Cargo.toml";

fn resolving_evidence(id: &str) -> EvidenceRefV1 {
    EvidenceRefV1 {
        id: id.to_string(),
        locator: EvidenceLocatorV1::Path {
            path: REAL_PATH.to_string(),
        },
        measurement: MeasurementStateV1::Proven,
    }
}

fn pristine_met_criterion() -> CriterionV1 {
    CriterionV1 {
        id: "SC-1".to_string(),
        statement: "a criterion whose evidence really resolves".to_string(),
        grade: CriterionVerdictV1::Met,
        evidence: vec![resolving_evidence("EV-1")],
    }
}

fn pristine_surface_row_json() -> serde_json::Value {
    serde_json::json!({
        "id": "SURF-01",
        "command_path": "session",
        "versioned_activation": { "state": "measured", "value": "0.12.25" },
        "operator_completeness": { "state": "unproven",
                                   "would_be_measured_by": "a three-platform operator journey" },
        "maturity": { "state": "measured", "value": "REACHED" },
        "security_authority_owner": "core",
        "evidence": [],
        "peer_delta": { "state": "unproven",
                        "would_be_measured_by": "the 30-02 comparative trial" },
        "last_refreshed_phase": "30-01"
    })
}

// ---------------------------------------------------------------------------
// Closed enums — refusal at DESERIALIZATION, before any logic runs
// ---------------------------------------------------------------------------

#[test]
fn an_invented_verdict_grade_fails_to_deserialize() {
    // CONTROL FIRST: every real grade is accepted, so this is not a parser that
    // rejects everything.
    for token in [
        "MET",
        "MET_WITH_STATED_EXCEPTIONS",
        "PARTIAL",
        "NOT_MET",
        "UNPROVEN",
    ] {
        let json = format!("\"{token}\"");
        serde_json::from_str::<CriterionVerdictV1>(&json)
            .unwrap_or_else(|e| panic!("pristine control `{token}` must be accepted: {e}"));
    }

    // THE MUTATION. `ready_for_frontier_positioning` is the exact shape of the
    // move an agent on this program actually made — inventing an extra
    // termination state to dodge an artifact it wrongly believed unobtainable.
    // A sixth grade meaning "ready" without meaning "proved" is the Phase 30
    // version of it, and it must die at the parser.
    let invented = "\"ready_for_frontier_positioning\"";
    let err = serde_json::from_str::<CriterionVerdictV1>(invented);
    assert!(
        err.is_err(),
        "an invented verdict grade must be refused at deserialization, got {err:?}"
    );
}

#[test]
fn a_maturity_state_the_ledger_never_declared_fails_to_deserialize() {
    for state in ALL_MATURITY_STATES {
        let json = format!("\"{}\"", state.token());
        serde_json::from_str::<MaturityV1>(&json).unwrap_or_else(|e| {
            panic!("pristine control `{}` must be accepted: {e}", state.token())
        });
    }

    for invented in ["FRONTIER", "PROVEN", "READY", "packaged_proven", "COMPLETE"] {
        let json = format!("\"{invented}\"");
        assert!(
            serde_json::from_str::<MaturityV1>(&json).is_err(),
            "maturity `{invented}` is not in the ledger's enum and must be refused"
        );
    }

    // Lifting the unmeasured case OUT of the enum must not open a hole back
    // into it: `MaturityTruthV1::Measured` still carries the closed enum, so an
    // undeclared token is refused just as hard one level down.
    let ungraded: MaturityTruthV1 = serde_json::from_value(serde_json::json!({
        "state": "unproven",
        "would_be_measured_by": "a CTRL-01 coverage family claiming this surface"
    }))
    .expect("an ungraded surface must be able to say so");
    assert!(matches!(ungraded, MaturityTruthV1::Unproven { .. }));

    assert!(
        serde_json::from_value::<MaturityTruthV1>(serde_json::json!({
            "state": "measured", "value": "UNPROVEN"
        }))
        .is_err(),
        "UNPROVEN is not a maturity STATE and must not smuggle in as one"
    );
}

#[test]
fn the_maturity_enum_has_exactly_the_eight_states_the_ledger_declares() {
    // The array type `[MaturityV1; 8]` makes the COMPILER check the count.
    let states: [MaturityV1; 8] = ALL_MATURITY_STATES;
    assert_eq!(states.len(), 8);

    let tokens: Vec<&str> = states.iter().map(|s| s.token()).collect();
    assert_eq!(
        tokens,
        vec![
            "ABSENT",
            "SOURCE",
            "CONFIGURED",
            "CONSTRUCTED",
            "REACHED",
            "EFFECTIVE",
            "OPERATOR_COMPLETE",
            "PACKAGED_PROVEN",
        ],
        "the enum must mirror CTRL-01's declared states in the ledger's own order"
    );

    // No duplicates: eight variants, eight distinct tokens.
    let mut sorted = tokens.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 8, "the eight states must be distinct");
}

// ---------------------------------------------------------------------------
// Seven truths, and unknown fields
// ---------------------------------------------------------------------------

#[test]
fn a_surface_row_missing_a_required_truth_fails_to_deserialize() {
    // CONTROL: the complete row is accepted.
    let pristine = pristine_surface_row_json();
    serde_json::from_value::<SurfaceRowV1>(pristine.clone())
        .expect("the pristine seven-truth row must be accepted");

    // MUTATION: drop each required truth in turn. Every one must be fatal —
    // there is no representation of a partially filled row.
    for truth in [
        "versioned_activation",
        "operator_completeness",
        "maturity",
        "security_authority_owner",
        "evidence",
        "peer_delta",
        "last_refreshed_phase",
    ] {
        let mut mutated = pristine.clone();
        mutated
            .as_object_mut()
            .expect("object")
            .remove(truth)
            .unwrap_or_else(|| panic!("fixture must contain `{truth}` to remove it"));
        assert!(
            serde_json::from_value::<SurfaceRowV1>(mutated).is_err(),
            "a row missing `{truth}` must not deserialize — a half-filled row is unrepresentable"
        );
    }
}

#[test]
fn an_unknown_field_in_a_scorecard_document_is_refused_at_deserialization() {
    let pristine = serde_json::json!({
        "schema": "wayland.scorecard",
        "schema_version": 1,
        "source_sha": "eab69cdbc244cfe90b0a623a9fb15c80da249d24",
        "criteria": [],
        "surfaces": []
    });
    serde_json::from_value::<ScorecardDocumentV1>(pristine.clone())
        .expect("the pristine document must be accepted");

    // A truth that was silently ignored reads exactly like a truth that was
    // supplied, so an unknown field is refused rather than dropped.
    let mut mutated = pristine.clone();
    mutated.as_object_mut().expect("object").insert(
        "overall_verdict".to_string(),
        serde_json::json!("ready_for_frontier_positioning"),
    );
    assert!(
        serde_json::from_value::<ScorecardDocumentV1>(mutated).is_err(),
        "an unknown document field must be refused"
    );

    // The same discipline at the surface-row boundary.
    let mut row = pristine_surface_row_json();
    row.as_object_mut()
        .expect("object")
        .insert("looks_fine".to_string(), serde_json::json!(true));
    assert!(
        serde_json::from_value::<SurfaceRowV1>(row).is_err(),
        "an unknown surface-row field must be refused"
    );

    // And at the evidence-reference boundary.
    let ev = serde_json::json!({
        "id": "EV-1",
        "locator": { "kind": "path", "path": REAL_PATH },
        "measurement": "proven",
        "note": "an extra field"
    });
    assert!(
        serde_json::from_value::<EvidenceRefV1>(ev).is_err(),
        "an unknown evidence-reference field must be refused"
    );
}

// ---------------------------------------------------------------------------
// The asymmetry: MET is expensive, NOT_MET is free
// ---------------------------------------------------------------------------

#[test]
fn met_is_refused_when_any_evidence_reference_does_not_resolve() {
    let root = repo_root();

    // CONTROL: MET with resolving evidence is ACCEPTED first.
    pristine_met_criterion()
        .verify(&root)
        .expect("MET with resolving evidence must be accepted");

    // MUTATION: repoint one reference at a path that does not exist.
    let mut broken = pristine_met_criterion();
    broken.evidence.push(EvidenceRefV1 {
        id: "EV-BROKEN".to_string(),
        locator: EvidenceLocatorV1::Path {
            path: "crates/wcore-eval-scenarios/THIS-FILE-DOES-NOT-EXIST.toml".to_string(),
        },
        measurement: MeasurementStateV1::Proven,
    });
    let err = broken
        .verify(&root)
        .expect_err("MET must be refused when a reference does not resolve");
    assert!(
        format!("{err}").contains("EV-BROKEN"),
        "the refusal must NAME the reference, got: {err}"
    );

    // And the same for a git object that does not exist.
    let mut bad_object = pristine_met_criterion();
    bad_object.evidence.push(EvidenceRefV1 {
        id: "EV-NO-OBJECT".to_string(),
        locator: EvidenceLocatorV1::GitObject {
            object: "0000000000000000000000000000000000000000".to_string(),
        },
        measurement: MeasurementStateV1::Proven,
    });
    let err = bad_object
        .verify(&root)
        .expect_err("MET must be refused on an unresolvable git object");
    assert!(format!("{err}").contains("EV-NO-OBJECT"), "got: {err}");

    // MET with NO evidence at all is also refused.
    let mut empty = pristine_met_criterion();
    empty.evidence.clear();
    assert!(
        empty.verify(&root).is_err(),
        "MET with an empty evidence set must be refused"
    );
}

#[test]
fn met_is_refused_when_a_referenced_measurement_is_unproven() {
    let root = repo_root();

    // CONTROL: the same reference, marked proven, is accepted.
    pristine_met_criterion()
        .verify(&root)
        .expect("the pristine control must be accepted");

    // MUTATION: flip only the measurement state. The path still resolves —
    // this isolates "unproven" as the single cause of the refusal.
    let mut unproven = pristine_met_criterion();
    unproven.evidence[0].measurement = MeasurementStateV1::Unproven;
    let err = unproven
        .verify(&root)
        .expect_err("MET must be refused when a referenced measurement is unproven");
    assert!(format!("{err}").contains("UNPROVEN"), "got: {err}");
    assert!(
        format!("{err}").contains("EV-1"),
        "must name the reference: {err}"
    );

    // MET_WITH_STATED_EXCEPTIONS pays the same price.
    let mut exceptions = unproven.clone();
    exceptions.grade = CriterionVerdictV1::MetWithStatedExceptions;
    assert!(
        exceptions.verify(&root).is_err(),
        "MET_WITH_STATED_EXCEPTIONS must pay the same evidence price as MET"
    );
}

#[test]
fn not_met_verifies_with_no_evidence_at_all() {
    let root = repo_root();
    let honest = CriterionV1 {
        id: "SC-3".to_string(),
        statement: "graded honestly, and it must cost nothing to say so".to_string(),
        grade: CriterionVerdictV1::NotMet,
        evidence: vec![],
    };
    honest
        .verify(&root)
        .expect("NOT_MET must verify with no evidence at all — the honest grade is the cheap one");

    // PARTIAL is equally free.
    let mut partial = honest.clone();
    partial.grade = CriterionVerdictV1::Partial;
    partial.verify(&root).expect("PARTIAL must also be free");

    // And a NOT_MET carrying a BROKEN reference is still fine: the grade makes
    // no claim the evidence has to support.
    let mut with_junk = honest.clone();
    with_junk.evidence.push(EvidenceRefV1 {
        id: "EV-JUNK".to_string(),
        locator: EvidenceLocatorV1::Path {
            path: "no/such/path.md".to_string(),
        },
        measurement: MeasurementStateV1::Unproven,
    });
    with_junk
        .verify(&root)
        .expect("NOT_MET imposes no evidence requirement whatsoever");
}

#[test]
fn unproven_verifies_with_no_evidence_at_all() {
    let root = repo_root();
    let unproven = CriterionV1 {
        id: "SC-4".to_string(),
        statement: "nobody has measured this yet, and saying so must be free".to_string(),
        grade: CriterionVerdictV1::Unproven,
        evidence: vec![],
    };
    unproven
        .verify(&root)
        .expect("UNPROVEN must verify with no evidence at all");

    // Contrast, in one assertion, with the expensive grade on the same input.
    let mut claimed = unproven.clone();
    claimed.grade = CriterionVerdictV1::Met;
    assert!(
        claimed.verify(&root).is_err(),
        "the SAME empty evidence set must be free for UNPROVEN and fatal for MET"
    );
}

// ---------------------------------------------------------------------------
// The walk is a measurement, so it must be deterministic
// ---------------------------------------------------------------------------

#[test]
fn two_walks_of_the_same_binary_produce_identical_bytes() {
    // Walk a binary that certainly exists on every platform CI runs: the test
    // runner's own executable. The property under test is DETERMINISM of the
    // walk, which does not depend on which binary is walked.
    let binary = std::env::current_exe().expect("the test binary exists");

    let first = walk_command_tree(&binary).expect("walk must succeed");
    let second = walk_command_tree(&binary).expect("walk must succeed");

    assert_eq!(
        render_surfaces_tsv(&first),
        render_surfaces_tsv(&second),
        "two walks of the same binary must produce identical bytes"
    );

    // Sortedness is what makes that true, so assert it directly rather than
    // relying on the equality above, which two identically-unsorted walks would
    // also satisfy.
    let paths: Vec<&str> = first.iter().map(|n| n.command_path.as_str()).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted, "the walk must emit a sorted table");
}

#[test]
fn the_subcommand_parser_reads_a_clap_commands_block_and_stops_at_its_end() {
    // A real clap help rendering, including the trailing Options block that a
    // naive parser would swallow as subcommands.
    let help = "\
Wayland Core

Usage: wayland-core [OPTIONS] [COMMAND]

Commands:
  session  Manage sessions
  plugin   Manage plugins
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
";
    let cmds = parse_subcommands(help);
    let names: Vec<&str> = cmds.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["plugin", "session"],
        "only real subcommands, sorted, with clap's built-in `help` excluded"
    );
    assert_eq!(cmds[1].1, "Manage sessions");

    // A help output with no Commands block yields no subcommands — the leaf case.
    assert!(parse_subcommands("Usage: x [OPTIONS]\n\nOptions:\n  -h\n").is_empty());
}

// ---------------------------------------------------------------------------
// The committed inventory must survive the real verifier
// ---------------------------------------------------------------------------

#[test]
fn every_surface_row_in_the_committed_inventory_deserializes_and_verifies() {
    let root = repo_root();
    let inventory = root.join(
        ".planning/phases/30-continuous-scorecard-frontier-review/evidence/30-01/\
         surface-truths.tsv",
    );
    let raw = std::fs::read_to_string(&inventory).unwrap_or_else(|e| {
        panic!(
            "the committed inventory must exist at {}: {e}",
            inventory.display()
        )
    });

    let mut rows = 0usize;
    for line in raw.lines() {
        if line.trim().is_empty() || line.starts_with('#') || !line.starts_with("SURF-") {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            f.len(),
            9,
            "every inventory row carries id, path and the seven truths: {line}"
        );
        let truth = |cell: &str, what: &str| -> TruthV1 {
            if cell == "UNPROVEN" {
                TruthV1::Unproven {
                    would_be_measured_by: what.to_string(),
                }
            } else {
                TruthV1::Measured {
                    value: cell.to_string(),
                }
            }
        };
        let row = SurfaceRowV1 {
            id: f[0].to_string(),
            command_path: f[1].to_string(),
            versioned_activation: truth(f[2], "a live activation observation"),
            operator_completeness: truth(f[3], "a three-platform operator journey"),
            maturity: if f[4] == "UNPROVEN" {
                MaturityTruthV1::Unproven {
                    would_be_measured_by: "a CTRL-01 coverage family claiming this surface"
                        .to_string(),
                }
            } else {
                MaturityTruthV1::Measured {
                    value: serde_json::from_str::<MaturityV1>(&format!("\"{}\"", f[4]))
                        .unwrap_or_else(|e| {
                            panic!(
                                "row {} carries an undeclared maturity `{}`: {e}",
                                f[0], f[4]
                            )
                        }),
                }
            },
            security_authority_owner: f[5].to_string(),
            evidence: vec![],
            peer_delta: truth(f[7], "the 30-02 comparative trial"),
            last_refreshed_phase: f[8].to_string(),
        };
        row.verify(&root)
            .unwrap_or_else(|e| panic!("committed inventory row {} does not verify: {e}", f[0]));
        rows += 1;
    }

    assert!(
        rows >= 21,
        "the binary declares at least twenty-one top-level commands; got {rows} inventory rows"
    );
}
