//! The E5 matrix generator's contract (Phase 28, plan 28-01, requirement F28-01).
//!
//! Every rejection below is asserted TWICE: with a case that TRIPS it and a case that does
//! NOT. A rule tested in one direction only is either vacuous or indiscriminate, and this
//! program's own plan-gate linter shipped the disease it hunts four separate times by
//! testing one direction.
//!
//! The rejections under contract:
//!   * a dimension absent from an OS family fails;
//!   * an unclassified skip fails — and is in fact UNREPRESENTABLE, see the note on
//!     `unclassified_skip_is_unrepresentable`;
//!   * a critical cell carrying any skip fails, since a critical cell has no legal skip;
//!   * an observation-blocked skip with no run-time control fails, including when it cites
//!     a document that reports in the product's favour;
//!   * a sandbox-dimension cell cannot be recorded PASSED without positive activeness;
//!   * the three mandatory cells are present, critical and unskippable;
//!   * generation is stable across runs.

use std::collections::BTreeSet;

use wcore_eval_scenarios::Platform;
use wcore_eval_scenarios::e5_matrix::{
    ActivenessEvidence, ActivenessRequirement, Applicability, ApplicabilityRecord, Cell,
    Criticality, Dimension, MANDATORY_CELLS, Matrix, MatrixError, PRODUCT_WIDE_SURFACE,
    SkipEvidence, Surface, criticality,
};

fn surfaces() -> Vec<Surface> {
    vec![
        Surface::new("cmd:alpha", "wayland-core alpha", 1),
        Surface::new("cmd:beta", "wayland-core beta", 1),
        Surface::new("cmd:alpha/one", "wayland-core alpha one", 2),
    ]
}

fn generated() -> Matrix {
    Matrix::generate(&surfaces(), &[], 1).expect("the clean matrix must generate")
}

/// A cell on a NON-critical dimension, so skip rules can be exercised without colliding
/// with the critical-cell rule.
fn standard_cell(id: &str, applicability: Applicability) -> Cell {
    Cell {
        id: id.to_string(),
        dimension: Dimension::Unicode,
        os: Platform::Linux,
        surface: "cmd:alpha".to_string(),
        criticality: Criticality::Standard,
        applicability,
        activeness: ActivenessRequirement::NotApplicable,
    }
}

fn matrix_with(extra: Cell) -> Matrix {
    let mut m = generated();
    m.cells.push(extra);
    m
}

// =======================================================================================
// The nine dimensions are fixed and verbatim
// =======================================================================================

#[test]
fn nine_f28_01_dimensions_exist_verbatim_and_none_was_renamed_or_merged() {
    assert_eq!(
        Dimension::ALL.len(),
        9,
        "F28-01 names exactly nine dimensions"
    );

    // The requirement's own words, in requirement order. A rename for tidiness makes a
    // dimension unprovable against F28-01's text, so it is pinned here.
    let expected = [
        "sandbox probes",
        "Unicode",
        "long paths",
        "UNC/reparse/symlink cases",
        "process cleanup",
        "suspend/resume",
        "offline",
        "disk-full/read-only",
        "hostile inputs",
    ];
    let actual: Vec<&str> = Dimension::ALL
        .iter()
        .map(|d| d.requirement_text())
        .collect();
    assert_eq!(actual, expected);

    let ids: BTreeSet<&str> = Dimension::ALL.iter().map(|d| d.id()).collect();
    assert_eq!(ids.len(), 9, "dimension wire ids must be unique");
}

#[test]
fn three_os_families_are_fixed() {
    assert_eq!(Platform::ALL.len(), 3);
}

// =======================================================================================
// REJECTION: a dimension absent from an OS family
// =======================================================================================

#[test]
fn a_dimension_missing_from_an_os_family_fails() {
    // TRIPS: strip every `offline` cell on macOS.
    let mut broken = generated();
    broken
        .cells
        .retain(|c| !(c.dimension == Dimension::Offline && c.os == Platform::Macos));
    match broken.validate() {
        Err(MatrixError::DimensionMissingOnOs { dimension, os }) => {
            assert_eq!(dimension, Dimension::Offline);
            assert_eq!(os, Platform::Macos);
        }
        other => panic!("expected DimensionMissingOnOs, got {other:?}"),
    }
}

#[test]
fn a_complete_matrix_does_not_trip_the_missing_dimension_rule() {
    // DOES NOT TRIP: the untouched generated matrix.
    generated().validate().expect("a complete matrix validates");
}

#[test]
fn every_dimension_appears_on_every_os_family() {
    let m = generated();
    for dimension in Dimension::ALL {
        for os in Platform::ALL {
            assert!(
                m.cells
                    .iter()
                    .any(|c| c.dimension == dimension && c.os == os),
                "{dimension} is missing on {os}"
            );
        }
    }
}

// =======================================================================================
// REJECTION: an unclassified skip
// =======================================================================================

#[test]
fn unclassified_skip_is_unrepresentable_and_every_class_carries_its_evidence() {
    // There is no way to construct `Applicability::Skipped` without a `SkipEvidence`
    // variant, and every variant IS a class carrying that class's required evidence. The
    // unclassified skip is therefore not merely rejected — it cannot be written. This test
    // pins the property so a future edit that adds a classless variant fails here.
    let all = [
        SkipEvidence::PlatformInapplicability {
            fact: "UNC paths are a Windows concept".into(),
            observable: "std::path prefix parsing".into(),
        },
        SkipEvidence::ObservationBlocked {
            control_ref: "control:x@h:1".into(),
        },
        SkipEvidence::ArchitecturalImpossibility {
            impossibility_check: "bash_under_appcontainer_is_fail_closed".into(),
        },
        SkipEvidence::UnresolvedSurface {
            phase: "26".into(),
            req_disposition: "deferred".into(),
        },
    ];
    let classes: BTreeSet<&str> = all.iter().map(|e| e.class().id()).collect();
    assert_eq!(classes.len(), 4, "exactly four classes, and no fifth");
}

#[test]
fn a_classified_skip_with_empty_evidence_fails() {
    // TRIPS
    let broken = matrix_with(standard_cell(
        "empty-evidence",
        Applicability::Skipped(SkipEvidence::UnresolvedSurface {
            phase: "26".into(),
            req_disposition: "   ".into(),
        }),
    ));
    assert!(matches!(
        broken.validate(),
        Err(MatrixError::SkipEvidenceEmpty { .. })
    ));

    // DOES NOT TRIP
    let ok = matrix_with(standard_cell(
        "full-evidence",
        Applicability::Skipped(SkipEvidence::UnresolvedSurface {
            phase: "26".into(),
            req_disposition: "deferred to phase 30".into(),
        }),
    ));
    ok.validate().expect("a fully evidenced skip validates");
}

// =======================================================================================
// REJECTION: a critical cell carrying any skip
// =======================================================================================

#[test]
fn a_critical_cell_carrying_any_skip_fails() {
    // TRIPS: even the architectural-impossibility class, the strongest evidence there is.
    let mut cell = standard_cell(
        "critical-skipped",
        Applicability::Skipped(SkipEvidence::ArchitecturalImpossibility {
            impossibility_check: "proved_by_executable_check".into(),
        }),
    );
    cell.criticality = Criticality::Critical;
    assert!(matches!(
        matrix_with(cell).validate(),
        Err(MatrixError::CriticalCellSkipped { .. })
    ));
}

#[test]
fn the_same_skip_on_a_standard_cell_does_not_trip_it() {
    // DOES NOT TRIP: identical evidence, standard criticality.
    let ok = matrix_with(standard_cell(
        "standard-skipped",
        Applicability::Skipped(SkipEvidence::ArchitecturalImpossibility {
            impossibility_check: "proved_by_executable_check".into(),
        }),
    ));
    ok.validate()
        .expect("a standard cell may carry an evidenced skip");
}

#[test]
fn a_declared_applicability_skip_on_a_critical_dimension_is_refused_at_generation() {
    // TRIPS: sandbox-probes is critical on every family, so it has no legal skip at all.
    let record = ApplicabilityRecord {
        dimension: Dimension::SandboxProbes,
        os: Platform::Windows,
        evidence: SkipEvidence::PlatformInapplicability {
            fact: "inconvenient".into(),
            observable: "n/a".into(),
        },
    };
    assert!(matches!(
        Matrix::generate(&surfaces(), &[record], 1),
        Err(MatrixError::CriticalCellSkipped { .. })
    ));

    // DOES NOT TRIP: the same record on a standard dimension generates cleanly.
    let record = ApplicabilityRecord {
        dimension: Dimension::UncReparseSymlink,
        os: Platform::Macos,
        evidence: SkipEvidence::PlatformInapplicability {
            fact: "UNC is a Windows-only path form".into(),
            observable: "no UNC prefix exists in the macOS path grammar".into(),
        },
    };
    let m = Matrix::generate(&surfaces(), &[record], 1)
        .expect("a standard dimension may carry a declared skip");
    assert!(
        m.cells
            .iter()
            .any(|c| matches!(c.applicability, Applicability::Skipped(_))),
        "the declared skip must actually appear on its cells"
    );
}

#[test]
fn the_critical_dimensions_are_the_two_named_by_the_success_criteria() {
    for os in Platform::ALL {
        assert!(criticality(Dimension::SandboxProbes, os).is_critical());
        assert!(criticality(Dimension::ProcessCleanup, os).is_critical());
        assert!(!criticality(Dimension::Unicode, os).is_critical());
        assert!(!criticality(Dimension::Offline, os).is_critical());
    }
}

// =======================================================================================
// REJECTION: an observation-blocked skip with no run-time control
// =======================================================================================

#[test]
fn an_observation_blocked_skip_without_a_runtime_control_fails() {
    for bad in [
        "the channel was clearly broken",
        "established by control in a previous phase",
        "control:@host:run",
        "",
    ] {
        let m = matrix_with(standard_cell(
            "ob-bad",
            Applicability::Skipped(SkipEvidence::ObservationBlocked {
                control_ref: bad.into(),
            }),
        ));
        assert!(
            matches!(
                m.validate(),
                Err(MatrixError::ObservationBlockedWithoutControl { .. })
                    | Err(MatrixError::ObservationBlockedCitesDocument { .. })
            ),
            "{bad:?} must not be accepted as observation-blocked evidence"
        );
    }
}

#[test]
fn an_observation_blocked_skip_citing_a_document_fails_even_when_it_favours_the_product() {
    // The lore that AppContainer cannot be observed over SSH was REFUTED on the measured
    // host. That refutation is just as unusable as skip evidence as the lore was: a
    // laundering channel does not become sound by pointing it at good news. Only a control
    // measured in the certification environment at run time counts.
    for citation in [
        ".planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md",
        ".planning/intel/APPCONTAINER-SSH-LEASE-WEDGE.md",
        "HANDOFF-2026-07-26-autonomous-execution section 5",
        "as established by 20A-04-SUMMARY",
    ] {
        let m = matrix_with(standard_cell(
            "ob-lore",
            Applicability::Skipped(SkipEvidence::ObservationBlocked {
                control_ref: citation.into(),
            }),
        ));
        assert!(
            matches!(
                m.validate(),
                Err(MatrixError::ObservationBlockedCitesDocument { .. })
                    | Err(MatrixError::ObservationBlockedWithoutControl { .. })
            ),
            "{citation:?} is a document citation and must be refused"
        );
    }
}

#[test]
fn a_measured_runtime_control_does_not_trip_the_observation_blocked_rule() {
    // DOES NOT TRIP — otherwise the class would be unusable and the rule indiscriminate.
    let ok = matrix_with(standard_cell(
        "ob-good",
        Applicability::Skipped(SkipEvidence::ObservationBlocked {
            control_ref: "control:appc-observe@seandesktop:30184651330".into(),
        }),
    ));
    ok.validate()
        .expect("a control measured at run time IS acceptable evidence");
}

// =======================================================================================
// The sandbox activeness rule
// =======================================================================================

#[test]
fn every_sandbox_dimension_cell_requires_activeness_not_only_the_mandatory_one() {
    let m = generated();
    let sandbox: Vec<&Cell> = m
        .cells
        .iter()
        .filter(|c| c.dimension == Dimension::SandboxProbes)
        .collect();
    assert!(!sandbox.is_empty());
    for c in sandbox {
        assert_eq!(
            c.activeness,
            ActivenessRequirement::Required,
            "sandbox cell {} must require activeness evidence",
            c.id
        );
    }
}

#[test]
fn a_sandbox_cell_cannot_be_recorded_passed_without_positive_activeness() {
    let m = generated();
    let cell = m
        .cells
        .iter()
        .find(|c| c.dimension == Dimension::SandboxProbes)
        .expect("a sandbox cell exists");

    // TRIPS: nothing was observed. This is the silent-disable case, and it is a RED.
    assert!(matches!(
        cell.record_pass(None),
        Err(MatrixError::SandboxPassWithoutActiveness { .. })
    ));

    // TRIPS: the measurement could not be taken. NOT a green and NOT a skip.
    assert!(matches!(
        cell.record_pass(Some(ActivenessEvidence::NotMeasured {
            reason: "AppContainer probe returned unavailable".into(),
        })),
        Err(MatrixError::SandboxPassWithoutActiveness { .. })
    ));

    // TRIPS: an "observation" with nothing in it.
    assert!(matches!(
        cell.record_pass(Some(ActivenessEvidence::Observed {
            probe: String::new(),
            detail: String::new(),
        })),
        Err(MatrixError::SandboxActivenessEmpty { .. })
    ));

    // DOES NOT TRIP: positive evidence the sandbox was active for this cell.
    let pass = cell
        .record_pass(Some(ActivenessEvidence::Observed {
            probe: "appcontainer-sid-match".into(),
            detail: "profile WCore-exec-4f2 active; token SID matches lease".into(),
        }))
        .expect("positive activeness evidence is a legal pass")
        .expect("a sandbox cell yields a SandboxPass");
    assert_eq!(pass.probe(), "appcontainer-sid-match");
}

#[test]
fn a_non_sandbox_cell_passes_without_activeness_evidence() {
    let m = generated();
    let cell = m
        .cells
        .iter()
        .find(|c| c.dimension == Dimension::Unicode)
        .expect("a unicode cell exists");
    assert!(
        cell.record_pass(None)
            .expect("no activeness required")
            .is_none(),
        "a non-sandbox cell yields no SandboxPass"
    );
}

#[test]
fn a_sandbox_cell_stripped_of_its_activeness_requirement_fails_validation() {
    // TRIPS: someone downgrades the requirement to make a cell easier to pass.
    let mut broken = generated();
    for c in &mut broken.cells {
        if c.dimension == Dimension::SandboxProbes {
            c.activeness = ActivenessRequirement::NotApplicable;
        }
    }
    assert!(matches!(
        broken.validate(),
        Err(MatrixError::SandboxPassWithoutActiveness { .. })
    ));
}

// =======================================================================================
// The three mandatory cells
// =======================================================================================

#[test]
fn all_three_mandatory_cells_are_present_critical_and_unskippable() {
    let m = generated();
    assert_eq!(MANDATORY_CELLS.len(), 3);

    for mandatory in MANDATORY_CELLS {
        let cell = m
            .cells
            .iter()
            .find(|c| c.id == mandatory.id)
            .unwrap_or_else(|| panic!("mandatory cell {} is absent", mandatory.id));
        assert_eq!(
            cell.criticality,
            Criticality::Critical,
            "{} must be critical",
            mandatory.id
        );
        assert_eq!(cell.dimension, mandatory.dimension);
        assert_eq!(cell.os, mandatory.os);
        assert_eq!(cell.surface, PRODUCT_WIDE_SURFACE);
        assert!(matches!(cell.applicability, Applicability::Applicable));
        assert!(!mandatory.rationale.is_empty());
    }

    let ids: BTreeSet<&str> = MANDATORY_CELLS.iter().map(|m| m.id).collect();
    assert!(ids.contains("w-sandbox-silent-disable"));
    assert!(ids.contains("w-process-cleanup-descendant-tree"));
    assert!(ids.contains("w-sandbox-observability-control"));
}

#[test]
fn removing_a_mandatory_cell_fails() {
    for mandatory in MANDATORY_CELLS {
        let mut broken = generated();
        broken.cells.retain(|c| c.id != mandatory.id);
        match broken.validate() {
            Err(MatrixError::MandatoryCellAbsent { cell_id }) => {
                assert_eq!(cell_id, mandatory.id);
            }
            other => panic!("removing {} must fail, got {other:?}", mandatory.id),
        }
    }
}

#[test]
fn downgrading_a_mandatory_cell_off_critical_fails() {
    let mut broken = generated();
    for c in &mut broken.cells {
        if c.id == "w-sandbox-silent-disable" {
            c.criticality = Criticality::Standard;
        }
    }
    assert!(matches!(
        broken.validate(),
        Err(MatrixError::MandatoryCellNotCritical {
            cell_id: "w-sandbox-silent-disable"
        })
    ));
}

#[test]
fn a_mandatory_cell_is_emitted_regardless_of_what_surface_resolution_produces() {
    // A candidate exposing a single unrelated surface still gets all three.
    let lone = vec![Surface::new("cmd:zzz", "wayland-core zzz", 1)];
    let m = Matrix::generate(&lone, &[], 1).expect("generates");
    for mandatory in MANDATORY_CELLS {
        assert!(m.cells.iter().any(|c| c.id == mandatory.id));
    }
}

// =======================================================================================
// Structural rules
// =======================================================================================

#[test]
fn a_matrix_over_no_surfaces_is_refused_rather_than_certifying_nothing() {
    assert!(matches!(
        Matrix::generate(&[], &[], 1),
        Err(MatrixError::NoSurfaces)
    ));
    // A depth filter that excludes everything is the same condition, not an empty matrix.
    let deep = vec![Surface::new("cmd:a/b", "wayland-core a b", 2)];
    assert!(matches!(
        Matrix::generate(&deep, &[], 1),
        Err(MatrixError::NoSurfaces)
    ));
}

#[test]
fn duplicate_cell_ids_are_refused() {
    let mut broken = generated();
    let dup = broken.cells[0].clone();
    broken.cells.push(dup);
    assert!(matches!(
        broken.validate(),
        Err(MatrixError::DuplicateCellId { .. })
    ));
}

#[test]
fn generation_is_stable_across_runs_so_three_hosts_execute_the_same_matrix() {
    let a = generated();
    let b = generated();
    let ids_a: Vec<&str> = a.cells.iter().map(|c| c.id.as_str()).collect();
    let ids_b: Vec<&str> = b.cells.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids_a, ids_b, "cell ids and order must be identical");
    assert_eq!(
        a.to_tsv(),
        b.to_tsv(),
        "the emitted TSV must be byte-stable"
    );

    // Input order must not change the output either — the three hosts may read the
    // candidate ledger in any order.
    let mut shuffled = surfaces();
    shuffled.reverse();
    let c = Matrix::generate(&shuffled, &[], 1).expect("generates");
    assert_eq!(
        a.to_tsv(),
        c.to_tsv(),
        "surface input order must not matter"
    );
}

#[test]
fn the_emitted_matrix_carries_no_observation_blocked_skip_at_construction_time() {
    // No control has run yet. The class becomes usable only when plan 28-02 supplies a
    // measured control, so a generated matrix must contain none.
    let m = generated();
    for c in &m.cells {
        if let Applicability::Skipped(e) = &c.applicability {
            assert_ne!(
                e.class().id(),
                "observation-blocked",
                "cell {} carries an observation-blocked skip before any control ran",
                c.id
            );
        }
    }
}

#[test]
fn the_tsv_has_one_header_and_one_row_per_cell_with_nine_fields() {
    let m = generated();
    let tsv = m.to_tsv();
    let rows: Vec<&str> = tsv
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .collect();
    assert_eq!(rows.len(), m.cells.len());
    for row in rows {
        assert_eq!(
            row.split('\t').count(),
            9,
            "row must carry nine fields: {row}"
        );
    }
}

// =======================================================================================
// Emission — the machine artifact plans 02 and 04 consume
// =======================================================================================

/// Generate the matrix over the REAL resolved candidate and, when `F28_MATRIX_OUT` is set,
/// write it there.
///
/// The TSV is produced BY the generator, never typed, so the committed artifact and the
/// code cannot disagree. Without the env var this still exercises the real candidate.json
/// and asserts the matrix over it is valid.
#[test]
fn generates_over_the_real_resolved_candidate() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..");
    let candidate = repo.join(
        ".planning/phases/28-native-cross-platform-certification/evidence/28-01/candidate.json",
    );
    if !candidate.is_file() {
        // Not skipped silently: the resolver has not run in this checkout. Plan 28-02
        // consumes the committed artifact, and the other tests fully cover the generator.
        eprintln!(
            "F28: candidate.json not present at {}; generator contract is covered by the \
             fixture tests above",
            candidate.display()
        );
        return;
    }

    let json = std::fs::read_to_string(&candidate).expect("read candidate.json");
    let surfaces = Matrix::surfaces_from_candidate_json(&json).expect("parse surfaces");
    assert!(
        !surfaces.is_empty(),
        "the resolved candidate must expose surfaces"
    );

    let matrix = Matrix::generate(&surfaces, &[], 1).expect("the real candidate generates");
    matrix.validate().expect("the real matrix validates");

    let depth1 = surfaces.iter().filter(|s| s.depth <= 1).count();
    assert_eq!(matrix.cells.len(), 9 * 3 * depth1 + 3);

    eprintln!(
        "F28_MATRIX cells={} surfaces_depth1={} mandatory={}",
        matrix.cells.len(),
        depth1,
        MANDATORY_CELLS.len()
    );

    if let Ok(out) = std::env::var("F28_MATRIX_OUT") {
        std::fs::write(&out, matrix.to_tsv()).expect("write matrix tsv");
        eprintln!("F28_MATRIX_WRITTEN {out}");
    }
}
