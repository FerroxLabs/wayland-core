//! The dual-surface hostile-child corpus harness — Phase 21, plan 21-02.
//!
//! ## What this file is
//!
//! One corpus, defined once as data in `child_authority_corpus/cases.rs`,
//! executed by FOUR drivers across the cross product of two axes:
//!
//! | | in-process | live |
//! |---|---|---|
//! | **standalone** | the real spawn seam and the real mechanisms | the real `wayland-core` binary, `-p`/`--no-tui` or the bare binary on a PTY |
//! | **host-protocol** | the real session/turn authority the protocol front-end binds | the real `wayland-core --json-stream` |
//!
//! The SURFACE axis is Success Criterion 3's actual proof. The MODE axis is the
//! one that catches a lying suite: if the in-process driver reports REFUSED and
//! the live binary reports ALLOWED, the integration test was vouching for a
//! restriction the shipped product does not enforce, and that divergence is a
//! more serious finding than either result alone.
//!
//! ## Why the harness iterates the table rather than each surface enumerating
//!
//! Two independently-authored surface suites cannot prove equivalence — they
//! can only drift, and the drift stays invisible until one of them is quietly
//! weaker. Here every corpus entry is executed by iterating the COMBINATION set
//! over the table, so authoring a case that runs on only one surface is
//! structurally impossible rather than merely discouraged. The completeness
//! invariant makes that a checked fact: recorded executions must equal entries
//! times combinations, with any combination a platform cannot drive recorded as
//! a counted UNAVAILABLE outcome carrying its reason. Nothing is ever skipped.
//! The Phase 20A audit found 283 tests with no execution evidence, and every one
//! of them looked fine from a distance.
//!
//! ## What this harness asserts, and what it records
//!
//! This plan PROVES; plan 21-03 REPAIRS. The split is deliberate and is stated
//! here so nobody mistakes a recorded red for an ignored one:
//!
//! * **Asserted** — the corpus's own integrity. Every census dimension has an
//!   entry; every entry runs in every combination; every live run proved the
//!   mode it landed in before its verdict was recorded; the two surfaces agree;
//!   in-process and live agree; and — the enforcement assertion — no dimension
//!   the census recorded ENFORCED is found widenable. A contradiction between
//!   what the source appears to do and what it does is a NEW finding and fails
//!   here, immediately.
//! * **Recorded** — the enforcement verdict for the dimensions the census
//!   already recorded VACUOUS or ABSENT. Those are the four HIGH findings the
//!   census already routed to 21-03 with a bounded repair budget. Failing on
//!   them here would not make them any more fixed; it would only make this
//!   plan's own gates unpassable while its output — the severity-classified red
//!   list — is exactly what 21-03 consumes. Every one of them is written to the
//!   per-case ledger and carried into `21-02-CORPUS-RESULTS.md`.
//!
//! No assertion anywhere names an error string, error kind, error variant or
//! numeric status. Every verdict is on what the child obtained.
//!
//! ## Layout
//!
//! One `#[test]` per dimension, so every case id appears in the run transcript
//! and a case that did not execute cannot be mistaken for one that passed.
//! `cargo nextest` runs each in its own process, which is why the completeness
//! invariant is asserted per entry (this entry ran every combination) rather
//! than through shared mutable state that would not survive the process model.

#[path = "child_authority_corpus/cases.rs"]
mod cases;
#[path = "child_authority_corpus/live.rs"]
mod live;
#[path = "support/mod.rs"]
mod support;
#[path = "child_authority_corpus/surfaces.rs"]
mod surfaces;

use std::path::PathBuf;

use cases::{CENSUS_DIMENSIONS, CensusVerdict, CorpusEntry, Dimension, Expectation};
use live::{HostProtocolLive, StandaloneLive};
use surfaces::{
    CorpusExecutor, Execution, HostProtocolInProcess, Mode, Outcome, StandaloneInProcess, Surface,
};

/// The full cross product. A combination is one surface crossed with one mode.
/// Four, always: a combination a platform cannot drive is recorded as an
/// UNAVAILABLE outcome with its reason, never removed from the set, because a
/// missing row and a declared limitation look identical from a distance.
const COMBINATIONS: [(Surface, Mode); 4] = [
    (Surface::Standalone, Mode::InProcess),
    (Surface::HostProtocol, Mode::InProcess),
    (Surface::Standalone, Mode::Live),
    (Surface::HostProtocol, Mode::Live),
];

/// The platform this run is recording for, as the results table spells it.
fn platform() -> &'static str {
    if cfg!(windows) { "windows" } else { "linux" }
}

/// Run one entry in every combination.
fn run_entry(entry: &CorpusEntry) -> Vec<Execution> {
    COMBINATIONS
        .iter()
        .map(|(surface, mode)| match (surface, mode) {
            (Surface::Standalone, Mode::InProcess) => StandaloneInProcess.execute(entry),
            (Surface::HostProtocol, Mode::InProcess) => HostProtocolInProcess.execute(entry),
            (Surface::Standalone, Mode::Live) => StandaloneLive.execute(entry),
            (Surface::HostProtocol, Mode::Live) => HostProtocolLive.execute(entry),
        })
        .collect()
}

/// The aggregate outcome recorded for a case on this platform. ALLOWED
/// dominates — a widening observed in any single combination is a widening —
/// then NOT-EXPRESSIBLE, then UNAVAILABLE, and only a wholly consistent set of
/// refusals reports REFUSED or NO-CHANNEL.
fn aggregate(executions: &[Execution]) -> Outcome {
    if executions.iter().any(|e| e.outcome == Outcome::Allowed) {
        return Outcome::Allowed;
    }
    if executions
        .iter()
        .any(|e| e.outcome == Outcome::NotExpressible)
    {
        return Outcome::NotExpressible;
    }
    if executions.iter().all(|e| e.outcome == Outcome::Unavailable) {
        return Outcome::Unavailable;
    }
    if executions
        .iter()
        .filter(|e| e.outcome.is_decisive())
        .all(|e| e.outcome == Outcome::NoChannel)
    {
        return Outcome::NoChannel;
    }
    Outcome::Refused
}

/// Where the per-case machine-readable rows land, so the results artifact is
/// assembled from measurement rather than from recollection.
fn ledger_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("child-authority-corpus");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Emit the machine-readable rows for one case, to stdout and to the ledger.
fn record(entry: &CorpusEntry, executions: &[Execution]) {
    let case = entry.dimension.case_id();
    let test_name = format!("corpus_{case}");
    let mut rows = Vec::new();

    rows.push(format!(
        "CASE :: {test_name} :: {} :: {} :: {}",
        entry.dimension.census_name(),
        platform(),
        aggregate(executions).label()
    ));
    rows.push(format!(
        "CENSUS :: {test_name} :: {} :: {} :: expectation {} :: seam {} :: canary {} :: \
         standalone-live-surface {}",
        entry.dimension.census_name(),
        entry.census_verdict.label(),
        entry.expectation.label(),
        entry.seam.label(),
        entry.no_channel_canary,
        entry.standalone_live_mode.label()
    ));

    for execution in executions {
        rows.push(format!(
            "COMBINATION :: {test_name} :: {} :: {} :: {} :: {} :: obtained {} :: {}",
            platform(),
            execution.surface.label(),
            execution.mode.label(),
            execution.outcome.label(),
            execution.obtained,
            execution.detail
        ));
        if let Some(live) = &execution.live {
            rows.push(format!(
                "LIVE :: {test_name} :: {} :: {} :: {} :: {} :: {}",
                platform(),
                live.asserted_mode,
                live.invocation,
                live.observable,
                execution.outcome.label()
            ));
        }
    }

    let body = rows.join("\n");
    println!("{body}");
    let _ = std::fs::write(
        ledger_dir().join(format!("{case}.rows")),
        format!("{body}\n"),
    );
}

/// The whole per-dimension flow: run every combination, record, then assert the
/// corpus's own integrity.
fn drive(dimension: Dimension) {
    let entry = cases::entry(dimension);
    let executions = run_entry(entry);
    record(entry, &executions);

    assert_completeness(entry, &executions);
    assert_live_runs_proved_their_mode(entry, &executions);
    assert_surface_equivalence(entry, &executions);
    assert_mode_equivalence(entry, &executions);
    assert_no_new_widening_against_the_census(entry, &executions);
}

/// The completeness invariant. Recorded executions must equal the combination
/// count, and every combination must be present exactly once. This is what
/// makes a single-surface case impossible to write: a driver cannot be omitted
/// without the count going wrong.
fn assert_completeness(entry: &CorpusEntry, executions: &[Execution]) {
    assert_eq!(
        executions.len(),
        COMBINATIONS.len(),
        "{}: recorded {} executions but the declared combination set has {}. Every entry must \
         execute in every combination; a missing combination is a coverage hole, never an \
         implicit pass.",
        entry.dimension.census_name(),
        executions.len(),
        COMBINATIONS.len()
    );
    for (surface, mode) in COMBINATIONS {
        let found = executions
            .iter()
            .filter(|e| e.surface == surface && e.mode == mode)
            .count();
        assert_eq!(
            found,
            1,
            "{}: combination ({}, {}) was recorded {found} times; it must be recorded exactly \
             once.",
            entry.dimension.census_name(),
            surface.label(),
            mode.label()
        );
    }
    for execution in executions {
        assert!(
            !execution.obtained.trim().is_empty(),
            "{} ({}, {}): recorded no statement of what the child obtained, so the row is not \
             evidence.",
            entry.dimension.census_name(),
            execution.surface.label(),
            execution.mode.label()
        );
    }
}

/// Every live execution must carry its four evidence fields, and any live run
/// that recorded a decisive verdict must have PROVED the mode it landed in. A
/// piped subprocess silently falls through from the TUI to the line REPL; a
/// verdict from such a run would be a verdict about a surface that was never
/// exercised.
fn assert_live_runs_proved_their_mode(entry: &CorpusEntry, executions: &[Execution]) {
    for execution in executions.iter().filter(|e| e.mode == Mode::Live) {
        let live = execution.live.as_ref().unwrap_or_else(|| {
            panic!(
                "{} ({} live): a live execution recorded no invocation, asserted mode or \
                 observable. A live row missing any of those is not evidence.",
                entry.dimension.census_name(),
                execution.surface.label()
            )
        });
        assert!(
            live.invocation.contains("wayland-core"),
            "{} ({} live): the recorded invocation does not name the real binary: {}",
            entry.dimension.census_name(),
            execution.surface.label(),
            live.invocation
        );
        assert!(
            !live.observable.trim().is_empty(),
            "{} ({} live): no observable was recorded, so nothing distinguished an enforced \
             restriction from a widened one.",
            entry.dimension.census_name(),
            execution.surface.label()
        );
        if execution.outcome.is_decisive() {
            assert!(
                !live.asserted_mode.ends_with("-UNPROVEN"),
                "{} ({} live): a verdict of {} was recorded from a run that never proved which \
                 mode it landed in ({}).",
                entry.dimension.census_name(),
                execution.surface.label(),
                execution.outcome.label(),
                live.asserted_mode
            );
        }
    }
}

/// Success Criterion 3's actual proof: the two surfaces must reach the same
/// verdict. When they do not, the weaker path is a bypass of the stronger and
/// the property is false overall, so the failure names the entry, the dimension
/// and both outcomes — the drift is diagnosable from this text alone.
fn assert_surface_equivalence(entry: &CorpusEntry, executions: &[Execution]) {
    for mode in [Mode::InProcess, Mode::Live] {
        let standalone = pick(executions, Surface::Standalone, mode);
        let protocol = pick(executions, Surface::HostProtocol, mode);
        if !standalone.outcome.is_decisive() || !protocol.outcome.is_decisive() {
            continue;
        }
        assert_eq!(
            standalone.outcome,
            protocol.outcome,
            "SURFACE-EQUIVALENCE FAILURE :: corpus_{} :: dimension {} :: mode {} :: standalone \
             {} (obtained: {}) against host-protocol {} (obtained: {}). One surface enforces and \
             the other does not, so the weaker path is a bypass of the stronger.",
            entry.dimension.case_id(),
            entry.dimension.census_name(),
            mode.label(),
            standalone.outcome.label(),
            standalone.obtained,
            protocol.outcome.label(),
            protocol.obtained
        );
    }
}

/// The assertion that catches the failure this codebase has already shipped
/// once. An in-process REFUSED against a live ALLOWED means the suite was green
/// while the product was ungoverned, and it is called out as its own class
/// rather than folded into whatever widening it accompanies.
fn assert_mode_equivalence(entry: &CorpusEntry, executions: &[Execution]) {
    for surface in [Surface::Standalone, Surface::HostProtocol] {
        let in_process = pick(executions, surface, Mode::InProcess);
        let live_run = pick(executions, surface, Mode::Live);
        if !in_process.outcome.is_decisive() || !live_run.outcome.is_decisive() {
            continue;
        }
        if in_process.outcome != Outcome::Allowed && live_run.outcome == Outcome::Allowed {
            panic!(
                "MODE-EQUIVALENCE FAILURE (in-process vouched for a restriction the shipped \
                 product does not enforce) :: corpus_{} :: dimension {} :: surface {} :: \
                 in-process {} against live ALLOWED (obtained: {}). This is the most serious \
                 result the corpus can produce.",
                entry.dimension.case_id(),
                entry.dimension.census_name(),
                surface.label(),
                in_process.outcome.label(),
                live_run.obtained
            );
        }
        assert_eq!(
            in_process.outcome,
            live_run.outcome,
            "MODE-EQUIVALENCE FAILURE :: corpus_{} :: dimension {} :: surface {} :: in-process {} \
             (obtained: {}) against live {} (obtained: {}).",
            entry.dimension.case_id(),
            entry.dimension.census_name(),
            surface.label(),
            in_process.outcome.label(),
            in_process.obtained,
            live_run.outcome.label(),
            live_run.obtained
        );
    }
}

/// The enforcement assertion, stated as a DELTA against the census.
///
/// A dimension the census recorded ENFORCED that the corpus now finds widenable
/// is a contradiction between what the source appears to do and what it does.
/// That is a NEW finding and it fails here. A widening on a dimension the census
/// already recorded VACUOUS or ABSENT is a CONFIRMATION of a HIGH finding
/// already routed to 21-03 with a bounded repair budget; it is recorded in the
/// ledger and carried into the results table unrepaired, because this plan
/// repairs nothing and failing on it would only make the proof uncommittable.
fn assert_no_new_widening_against_the_census(entry: &CorpusEntry, executions: &[Execution]) {
    if entry.census_verdict != CensusVerdict::Enforced {
        return;
    }
    for execution in executions {
        assert_ne!(
            execution.outcome,
            Outcome::Allowed,
            "NEW WIDENING AGAINST THE CENSUS :: corpus_{} :: dimension {} :: the census recorded \
             this dimension ENFORCED, and the corpus finds the child obtained {} through the {} \
             surface in {} mode. Detail: {}",
            entry.dimension.case_id(),
            entry.dimension.census_name(),
            execution.obtained,
            execution.surface.label(),
            execution.mode.label(),
            execution.detail
        );
    }
}

fn pick(executions: &[Execution], surface: Surface, mode: Mode) -> &Execution {
    executions
        .iter()
        .find(|e| e.surface == surface && e.mode == mode)
        .expect("the completeness invariant guarantees every combination is present")
}

// ===========================================================================
// Table-level invariants — no driver runs for these
// ===========================================================================

#[test]
fn corpus_table_covers_every_census_dimension() {
    for dimension in CENSUS_DIMENSIONS {
        let found = cases::CORPUS
            .iter()
            .filter(|e| e.dimension == *dimension)
            .count();
        assert_eq!(
            found,
            1,
            "census dimension {} appears {found} times in the corpus table; it must appear \
             exactly once. The census is the sole authorised source of cases, so a dimension \
             cannot be dropped and none may be invented.",
            dimension.census_name()
        );
    }
    assert_eq!(
        cases::CORPUS.len(),
        CENSUS_DIMENSIONS.len(),
        "the corpus table has {} entries against the census's {} dimensions. An extra entry is a \
         case the census did not name.",
        cases::CORPUS.len(),
        CENSUS_DIMENSIONS.len()
    );
}

#[test]
fn every_entry_states_a_request_and_an_invariant_and_names_no_error_shape() {
    // The forbidden vocabulary. An invariant that names any of this is an
    // assertion on today's failure shape, which keeps passing for the wrong
    // reason once the shape changes and keeps passing when the refusal moves to
    // a weaker cause.
    const ERROR_SHAPE_WORDS: [&str; 8] = [
        "error",
        "err(",
        "panic",
        "status",
        "exit code",
        "message \"",
        "returns Err",
        "Result::",
    ];
    for entry in cases::CORPUS {
        assert!(
            !entry.request.trim().is_empty(),
            "{}: no hostile request is described",
            entry.dimension.census_name()
        );
        assert!(
            !entry.invariant.trim().is_empty(),
            "{}: no invariant is stated",
            entry.dimension.census_name()
        );
        let lowered = entry.invariant.to_ascii_lowercase();
        for word in ERROR_SHAPE_WORDS {
            assert!(
                !lowered.contains(word),
                "{}: the invariant names an error shape ({word:?}). Every invariant must state \
                 what the child must NOT have obtained: {}",
                entry.dimension.census_name(),
                entry.invariant
            );
        }
        assert!(
            lowered.contains("must not"),
            "{}: the invariant does not state what the child must NOT have obtained: {}",
            entry.dimension.census_name(),
            entry.invariant
        );
    }
}

#[test]
fn every_vacuous_dimension_carries_a_no_channel_canary() {
    // Census section 8: provider, approval, and the Some(..) legs of
    // depth/time/token/cost are currently protected in part by the absence of a
    // request channel. A corpus that only asserted refusal would stay green
    // forever while enforcement does not exist.
    for entry in cases::CORPUS {
        let must_carry = entry.census_verdict == CensusVerdict::Vacuous
            || matches!(
                entry.dimension,
                Dimension::Depth | Dimension::Time | Dimension::Token | Dimension::Cost
            );
        if must_carry {
            assert!(
                entry.no_channel_canary,
                "{}: the census records this dimension's protection as resting in part on the \
                 absence of a request channel, so the entry must carry a NO-CHANNEL canary that \
                 fails when a channel appears.",
                entry.dimension.census_name()
            );
        }
        if entry.expectation == Expectation::NoChannel {
            assert!(
                entry.no_channel_canary,
                "{}: an entry whose expectation kind is NO-CHANNEL must carry the canary.",
                entry.dimension.census_name()
            );
        }
    }
}

#[test]
fn the_combination_set_is_the_full_cross_product_and_declares_its_unavailability() {
    assert_eq!(
        COMBINATIONS.len(),
        4,
        "the combination set must be the full cross product of two surfaces and two modes"
    );
    for surface in [Surface::Standalone, Surface::HostProtocol] {
        for mode in [Mode::InProcess, Mode::Live] {
            assert!(
                COMBINATIONS.contains(&(surface, mode)),
                "combination ({}, {}) is missing from the declared set",
                surface.label(),
                mode.label()
            );
        }
    }
    // The one declared unavailability, stated as a platform fact rather than
    // discovered at runtime.
    let tui_available = live::LiveTransport::Tui.available_here();
    assert_eq!(
        tui_available,
        !cfg!(windows),
        "the interactive TUI combination must be declared available exactly off Windows: {}",
        live::LiveTransport::Tui.unavailable_reason()
    );
    println!(
        "AVAILABILITY :: {} :: json-stream=true :: headless=true :: tui={tui_available}",
        platform()
    );
}

// ===========================================================================
// The eleven corpus cases. One test per census dimension, so every case id
// appears in the run transcript and a case that did not execute cannot be
// mistaken for one that passed.
// ===========================================================================

#[test]
fn corpus_provider() {
    drive(Dimension::Provider);
}

#[test]
fn corpus_tool() {
    drive(Dimension::Tool);
}

#[test]
fn corpus_filesystem() {
    drive(Dimension::Filesystem);
}

#[test]
fn corpus_egress() {
    drive(Dimension::Egress);
}

#[test]
fn corpus_secret() {
    drive(Dimension::Secret);
}

#[test]
fn corpus_approval() {
    drive(Dimension::Approval);
}

#[test]
fn corpus_depth() {
    drive(Dimension::Depth);
}

#[test]
fn corpus_fan_out() {
    drive(Dimension::FanOut);
}

#[test]
fn corpus_time() {
    drive(Dimension::Time);
}

#[test]
fn corpus_token() {
    drive(Dimension::Token);
}

#[test]
fn corpus_cost() {
    drive(Dimension::Cost);
}
