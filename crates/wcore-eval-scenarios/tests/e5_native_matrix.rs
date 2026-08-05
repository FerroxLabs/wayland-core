//! Contract tests for the Phase 28 E5 probe set (F28-01).
//!
//! These bind three artifacts that must not drift apart:
//!
//!   1. `crates/wcore-eval-scenarios/src/e5_cases.rs` — the canonical probe table.
//!   2. `scripts/f28-native-matrix.mjs` — the executor and marker verifier, which is
//!      what actually runs on a host with no cargo.
//!   3. `.planning/phases/28-native-cross-platform-certification/evidence/28-01/matrix.tsv`
//!      — the generated cell set the probes must cover.
//!
//! A probe table that agrees with nothing is a list of intentions. Every assertion here
//! is written so that it can FAIL: each one is stated against a value read from a file
//! on disk, never against a constant this test also defines.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use wcore_eval_scenarios::Platform;
use wcore_eval_scenarios::e5_cases::{Harness, PROBES, ProbeSpec, dimension_probes, probe_for};
use wcore_eval_scenarios::e5_matrix::{Criticality, Dimension, MANDATORY_CELLS};

// ---------------------------------------------------------------------------------------
// Repository paths
// ---------------------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/wcore-eval-scenarios`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repository root is two levels above the crate manifest")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------------------
// 1. The nine dimensions, verbatim and fixed
// ---------------------------------------------------------------------------------------

#[test]
fn the_nine_f28_01_dimensions_exist_verbatim_as_probes() {
    // The requirement text is the authority. Read the nine names out of REQUIREMENTS.md
    // rather than restating them here, so a divergence between the requirement and the
    // code is what fails rather than a divergence between the code and this test.
    let requirements = read(&repo_root().join(".planning/REQUIREMENTS.md"));
    let line = requirements
        .lines()
        .find(|l| l.contains("F28-01") && l.contains("E5 matrices cover"))
        .expect("F28-01 must state the dimension list in REQUIREMENTS.md");

    for dimension in Dimension::ALL {
        assert!(
            line.contains(dimension.requirement_text()),
            "F28-01 does not contain the requirement text `{}` for dimension `{}`; the \
             dimension list is fixed and a renamed dimension is an unprovable one",
            dimension.requirement_text(),
            dimension.id()
        );
        let covering: Vec<&ProbeSpec> = dimension_probes()
            .filter(|p| p.dimension == dimension)
            .collect();
        assert_eq!(
            covering.len(),
            1,
            "dimension `{}` has {} dimension probes, expected exactly 1",
            dimension.id(),
            covering.len()
        );
    }
    assert_eq!(Dimension::ALL.len(), 9);
}

// ---------------------------------------------------------------------------------------
// 2. The executor mirrors the canonical table, entry for entry
// ---------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct MirrorEntry {
    id: String,
    dimension: String,
    families: Vec<String>,
    cell_id: Option<String>,
    harness: String,
    emits_activeness: bool,
}

fn rust_entries() -> Vec<MirrorEntry> {
    let mut out: Vec<MirrorEntry> = PROBES
        .iter()
        .map(|p| MirrorEntry {
            id: p.id.to_string(),
            dimension: p.dimension.id().to_string(),
            families: p.families.iter().map(|f| f.to_string()).collect(),
            cell_id: p.cell_id.map(str::to_string),
            harness: if p.harness.is_black_box() {
                "black-box".to_string()
            } else {
                "harness-bound".to_string()
            },
            emits_activeness: p.emits_activeness,
        })
        .collect();
    out.sort();
    out
}

/// Parse the `PROBES` array out of the executor. Deliberately a strict field-by-field
/// parse: a loose one would silently accept a mirror that had lost a field.
fn mjs_entries(source: &str) -> Vec<MirrorEntry> {
    let start = source
        .find("export const PROBES = [")
        .expect("f28-native-matrix.mjs must export a PROBES table");
    let tail = &source[start..];
    let end = tail.find("\n];").expect("PROBES table must be terminated");
    let body = &tail[..end];

    let mut out = Vec::new();
    for raw in body.lines() {
        let line = raw.trim();
        if !line.starts_with("{ id:") {
            continue;
        }
        let field = |name: &str| -> Option<String> {
            let key = format!("{name}: ");
            let at = line.find(&key)? + key.len();
            let rest = &line[at..];
            let stop = rest.find(", ").unwrap_or(rest.len());
            Some(rest[..stop].trim_end_matches([',', ' ', '}']).to_string())
        };
        let unquote = |v: String| v.trim_matches('\'').to_string();
        let families = {
            let at = line
                .find("families: [")
                .expect("mirror entry has no families")
                + "families: [".len();
            let rest = &line[at..];
            let stop = rest.find(']').expect("families list is unterminated");
            rest[..stop]
                .split(',')
                .map(|s| s.trim().trim_matches('\'').to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        };
        let cell_raw = field("cell_id").expect("mirror entry has no cell_id");
        out.push(MirrorEntry {
            id: unquote(field("id").expect("mirror entry has no id")),
            dimension: unquote(field("dimension").expect("mirror entry has no dimension")),
            families,
            cell_id: if cell_raw == "null" {
                None
            } else {
                Some(unquote(cell_raw))
            },
            harness: unquote(field("harness").expect("mirror entry has no harness")),
            emits_activeness: field("emits_activeness")
                .expect("mirror entry has no emits_activeness")
                == "true",
        });
    }
    out.sort();
    out
}

#[test]
fn the_executor_mirrors_the_canonical_probe_table_entry_for_entry() {
    let source = read(&repo_root().join("scripts/f28-native-matrix.mjs"));
    let mirror = mjs_entries(&source);
    let canonical = rust_entries();

    assert!(
        !mirror.is_empty(),
        "no mirror entries were parsed from the executor; a comparison against an empty \
         set passes without proving anything"
    );
    assert_eq!(
        mirror.len(),
        canonical.len(),
        "the executor mirrors {} probes but the canonical table has {}",
        mirror.len(),
        canonical.len()
    );
    for (m, c) in mirror.iter().zip(canonical.iter()) {
        assert_eq!(
            m, c,
            "the executor has drifted from the canonical probe table; the executor is \
             what actually runs on a host with no cargo, so a drift here means the \
             definition and the measurement are different things"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. Black-box means no cargo harness on the host, and that is asserted
// ---------------------------------------------------------------------------------------

#[test]
fn black_box_probes_require_no_cargo_harness() {
    let source = read(&repo_root().join("scripts/f28-native-matrix.mjs"));

    // The claim being asserted: for every probe declared BlackBox, the executor
    // implements a runner for its dimension. If it did not, the probe would in practice
    // need the cargo-built harness it says it does not, and the macOS leg — where cargo
    // cannot run at all — would silently lose that dimension.
    for spec in &PROBES {
        if !spec.harness.is_black_box() {
            continue;
        }
        let needle = format!("'{}'(bin", spec.dimension.id());
        assert!(
            source.contains(&needle),
            "probe `{}` is declared BlackBox but the executor implements no runner for \
             dimension `{}`; a black-box claim the executor cannot honour narrows the \
             macOS leg silently",
            spec.id,
            spec.dimension.id()
        );
    }

    // And the executor must not reach for cargo. A runner that shells out to cargo is
    // harness-bound whatever its declaration says.
    assert!(
        !source.contains("'cargo'") && !source.contains("\"cargo\""),
        "the executor invokes cargo; every probe it runs is then harness-bound in fact, \
         however it is declared"
    );
}

// ---------------------------------------------------------------------------------------
// 4. Coverage of the generated matrix
// ---------------------------------------------------------------------------------------

struct Row {
    cell: String,
    dimension: Dimension,
    os: Platform,
    criticality: Criticality,
}

fn matrix_rows() -> Vec<Row> {
    let path = repo_root()
        .join(".planning/phases/28-native-cross-platform-certification/evidence/28-01/matrix.tsv");
    let text = read(&path);
    let dimensions: BTreeMap<&str, Dimension> =
        Dimension::ALL.iter().map(|d| (d.id(), *d)).collect();
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let p: Vec<&str> = line.split('\t').collect();
        assert_eq!(p.len(), 9, "matrix row has {} columns, expected 9", p.len());
        let os = match p[2] {
            "linux" => Platform::Linux,
            "macos" => Platform::Macos,
            "windows" => Platform::Windows,
            other => panic!("unknown OS family in the matrix: {other}"),
        };
        rows.push(Row {
            cell: p[0].to_string(),
            dimension: *dimensions
                .get(p[1])
                .unwrap_or_else(|| panic!("unknown dimension in the matrix: {}", p[1])),
            os,
            criticality: match p[4] {
                "critical" => Criticality::Critical,
                "standard" => Criticality::Standard,
                other => panic!("unknown criticality: {other}"),
            },
        });
    }
    assert!(
        !rows.is_empty(),
        "the generated matrix declares no cells; a coverage check over an empty set \
         passes without proving anything"
    );
    rows
}

#[test]
fn every_generated_matrix_cell_has_exactly_one_probe() {
    let rows = matrix_rows();
    let mut uncovered = Vec::new();
    for row in &rows {
        if probe_for(&row.cell, row.dimension, row.os).is_none() {
            uncovered.push(row.cell.clone());
        }
    }
    assert!(
        uncovered.is_empty(),
        "{} of {} generated cells have no probe (or are claimed by more than one); the \
         first are {:?}. A cell with no probe fails the suite rather than being reported \
         absent",
        uncovered.len(),
        rows.len(),
        &uncovered[..uncovered.len().min(5)]
    );
}

#[test]
fn a_harness_bound_probe_may_not_be_the_only_cover_of_a_critical_cell_on_macos() {
    // The certification Mac may run the shipped binary and may NOT run cargo. A
    // harness-bound probe is therefore unrunnable there, and a critical cell covered
    // only by one is a coverage hole wearing a declaration.
    for row in matrix_rows() {
        if row.os != Platform::Macos || !row.criticality.is_critical() {
            continue;
        }
        let spec = probe_for(&row.cell, row.dimension, row.os)
            .unwrap_or_else(|| panic!("critical macOS cell `{}` has no probe", row.cell));
        assert!(
            spec.harness.is_black_box(),
            "critical cell `{}` on macOS is covered only by harness-bound probe `{}`; no \
             cargo harness can be built on the certification Mac, so this is a silent \
             narrowing of coverage",
            row.cell,
            spec.id
        );
    }
}

#[test]
fn the_guard_against_harness_bound_macos_coverage_can_actually_fire() {
    // The assertion above passes today because every probe is BlackBox. A guard that
    // has never been shown to reject is indistinguishable from one that cannot.
    let bound = Harness::HarnessBound {
        reason: "needs a cargo-built harness",
    };
    assert!(!bound.is_black_box());
    assert!(Harness::BlackBox.is_black_box());
}

#[test]
fn all_three_mandatory_cells_are_probed_by_their_own_cell_specific_probe() {
    let generated: BTreeSet<String> = matrix_rows().into_iter().map(|r| r.cell).collect();
    for mandatory in MANDATORY_CELLS {
        assert!(
            generated.contains(mandatory.id),
            "mandatory cell `{}` is absent from the generated matrix",
            mandatory.id
        );
        let spec = probe_for(mandatory.id, mandatory.dimension, mandatory.os)
            .unwrap_or_else(|| panic!("mandatory cell `{}` has no probe", mandatory.id));
        assert_eq!(
            spec.cell_id,
            Some(mandatory.id),
            "mandatory cell `{}` is covered by the generic dimension probe `{}` rather \
             than by a probe written for it",
            mandatory.id,
            spec.id
        );
    }
}

// ---------------------------------------------------------------------------------------
// 5. Every probe can go red
// ---------------------------------------------------------------------------------------

#[test]
fn every_probe_declares_the_mutation_that_makes_it_red() {
    // A hostile-input or Unicode probe authored so permissively that it cannot fail
    // adds a green cell that proves nothing. The only way that becomes visible is to
    // require the failing counterpart to be written down next to the probe.
    for spec in &PROBES {
        assert!(
            spec.failing_counterpart.len() > 20,
            "probe `{}` names no substantive failing counterpart",
            spec.id
        );
        assert!(
            spec.red_when.len() > 20,
            "probe `{}` names no substantive red condition",
            spec.id
        );
    }
}

#[test]
fn the_executor_self_test_covers_the_marker_rejections_this_plan_relies_on() {
    // The verifier's own self-test is the proof that it fails closed. Assert the
    // rejections are actually present in it, so the self-test cannot be quietly reduced
    // to the cases that already pass.
    let source = read(&repo_root().join("scripts/f28-native-matrix.mjs"));
    for needle in [
        "missing cell markers",
        "duplicate cell marker",
        "out of order",
        "foreign cell marker",
        "foreign platform marker",
        "final acceptance marker before all cells",
        "cell marker after final",
        "duplicate final acceptance marker",
        "missing final platform acceptance marker",
        "commit drift",
        "tree drift",
        "nonce drift",
        "unrecognized matrix marker",
        "absence of an observed violation",
        "non-empty declared ordering",
    ] {
        assert!(
            source.contains(needle),
            "the executor's self-test no longer exercises `{needle}`; the marker \
             discipline is only worth what its self-test proves"
        );
    }
}
