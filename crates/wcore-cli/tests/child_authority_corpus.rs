//! The dual-surface hostile-child corpus harness — Phase 21, plan 21-02.
//!
//! ## What this file is
//!
//! One corpus, defined once as data in `child_authority_corpus/cases.rs`,
//! executed by FOUR drivers across the cross product of two axes:
//!
//! | | in-process | live |
//! |---|---|---|
//! | **standalone** | the real spawn seam and the real mechanisms | the real `wayland-core` binary, `--no-tui` headless or the bare binary on a PTY |
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
        if let Some(channel) = &execution.canary_trip {
            rows.push(format!(
                "CANARY :: {test_name} :: {} :: {} :: {} :: TRIPPED :: {channel}",
                platform(),
                execution.surface.label(),
                execution.mode.label()
            ));
        }
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
    // The canary assertion runs BEFORE the equivalence pair on purpose. A
    // request channel appearing is the most specific thing the corpus can
    // observe, and it explains any divergence the equivalence assertions would
    // otherwise report as a surface drift. Checked in the other order, the
    // injected-channel proof fails as "one surface enforces and the other does
    // not" — true, but not the finding.
    assert_no_channel_canaries_stayed_intact(entry, &executions);
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
        assert_eq!(
            execution.dimension,
            entry.dimension,
            "a driver returned an execution stamped {} while running the {} entry; a row \
             attributed to the wrong dimension would land in the results table as evidence about \
             something it never exercised.",
            execution.dimension.census_name(),
            entry.dimension.census_name()
        );
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
/// ENFORCEMENT verdict. When they do not, the weaker path is a bypass of the
/// stronger and the property is false overall, so the failure names the entry,
/// the dimension and both outcomes — the drift is diagnosable from this text
/// alone.
///
/// A JUDGEMENT, stated openly because it is the one place this harness does not
/// compare labels literally. Equivalence is asserted on WIDENED-or-not, not on
/// the outcome label. REFUSED and NO-CHANNEL are both "the child did not
/// obtain": one path refuses a request, the other has no way to make one. That
/// is a difference in MECHANISM, and it is exactly what the census found for
/// the budget family — the seam refuses when a request is forced in process,
/// and no shipped surface lets a child issue the request at all. Failing on
/// that pairing would force one of the two honest answers to be restated as the
/// other to reach green, which is the forgery this plan exists to avoid. The
/// difference is not swallowed: it is printed as a
/// `SURFACE-MECHANISM-DIFFERENCE` row and carried into the results.
fn assert_surface_equivalence(entry: &CorpusEntry, executions: &[Execution]) {
    for mode in [Mode::InProcess, Mode::Live] {
        let standalone = pick(executions, Surface::Standalone, mode);
        let protocol = pick(executions, Surface::HostProtocol, mode);
        if !standalone.outcome.is_decisive() || !protocol.outcome.is_decisive() {
            continue;
        }
        assert_eq!(
            standalone.outcome == Outcome::Allowed,
            protocol.outcome == Outcome::Allowed,
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
        if standalone.outcome != protocol.outcome {
            // Both surfaces enforce, but by different mechanisms: one refuses a
            // request the other has no way to make. That is a MECHANISM
            // difference, not a drift in enforcement, and it is reported rather
            // than asserted on — see the note above `assert_surface_equivalence`.
            println!(
                "SURFACE-MECHANISM-DIFFERENCE :: corpus_{} :: {} :: standalone {} against \
                 host-protocol {} :: neither widened",
                entry.dimension.case_id(),
                mode.label(),
                standalone.outcome.label(),
                protocol.outcome.label()
            );
        }
    }
}

/// The assertion that catches the failure this codebase has already shipped
/// once. An in-process REFUSED against a live ALLOWED means the suite was green
/// while the product was ungoverned, and it is called out as its own class
/// rather than folded into whatever widening it accompanies.
///
/// The same widened-or-not judgement applies here as in
/// `assert_surface_equivalence`, and for the same reason. The serious direction
/// — in-process not-widened against live ALLOWED — is checked first and named
/// explicitly, because it is the most serious result the corpus can produce.
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
            in_process.outcome == Outcome::Allowed,
            live_run.outcome == Outcome::Allowed,
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
        if in_process.outcome != live_run.outcome {
            println!(
                "MODE-MECHANISM-DIFFERENCE :: corpus_{} :: {} :: in-process {} against live {} \
                 :: neither widened",
                entry.dimension.case_id(),
                surface.label(),
                in_process.outcome.label(),
                live_run.outcome.label()
            );
        }
    }
}

/// THE NO-CHANNEL CANARY ASSERTION — FINDING F-V4.
///
/// `21-04-PHASE-VERDICT.md` calls this canary class the phase's single most
/// important inheritance, "worth more than any currently-green assertion in the
/// phase". Verification found that only one of the three was actually
/// fail-closed:
///
/// * the budget canary returned a `String` that nothing asserted on, so it
///   could trip silently forever;
/// * the approval canary failed only VIA mode-equivalence, and in the exact
///   scenario it exists for — a channel appears AND is live-exploitable — both
///   legs read `Allowed`, mode- and surface-equivalence both passed, and
///   `assert_no_new_widening_against_the_census` returned early because the
///   census verdict for approval is `Vacuous`. The suite stayed green on a
///   fully realised approval widening.
///
/// That early return is the bug this assertion closes. A census verdict is a
/// measurement taken BEFORE the corpus ran; it can explain why a widening was
/// already known, and it can excuse failing on a KNOWN red — but it cannot
/// excuse a widening on a dimension whose entire protection was the absence of
/// a request channel. When the channel appears, the absence is gone, and the
/// only honest response is red.
///
/// Two independent triggers, because a channel can appear in two ways:
///
/// 1. `canary_trip` — a structural canary measured a production request channel
///    that did not exist when the census ran. This fires on EVERY entry,
///    canary-carrying or not, because a new channel is news wherever it appears.
/// 2. `Outcome::Allowed` on an entry whose census protection rests on the
///    absence of a channel. This is the realised-widening case, and it fires
///    regardless of census verdict, surface or mode.
fn assert_no_channel_canaries_stayed_intact(entry: &CorpusEntry, executions: &[Execution]) {
    for execution in executions {
        if let Some(channel) = &execution.canary_trip {
            panic!(
                "NO-CHANNEL CANARY TRIPPED :: corpus_{} :: dimension {} :: surface {} :: mode {} \
                 :: {channel}. The census recorded this dimension's protection as resting in part \
                 on the absence of a request channel. A channel now exists, so the absence no \
                 longer protects anything and nothing was put in its place.",
                entry.dimension.case_id(),
                entry.dimension.census_name(),
                execution.surface.label(),
                execution.mode.label()
            );
        }
        if entry.no_channel_canary {
            assert_ne!(
                execution.outcome,
                Outcome::Allowed,
                "NO-CHANNEL CANARY TRIPPED (realised widening) :: corpus_{} :: dimension {} :: \
                 the child obtained {} through the {} surface in {} mode. This dimension carries a \
                 NO-CHANNEL canary because the census recorded its protection as resting on the \
                 absence of a request channel; a widening observed here means the channel exists \
                 AND is exploitable. The census verdict ({}) is a measurement taken before this \
                 run and does not excuse it. Detail: {}",
                entry.dimension.case_id(),
                entry.dimension.census_name(),
                execution.obtained,
                execution.surface.label(),
                execution.mode.label(),
                entry.census_verdict.label(),
                execution.detail
            );
        }
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
    // The declared unavailabilities, stated as platform facts rather than
    // discovered at runtime. Both PTY-backed transports share one gate.
    let tui_available = live::LiveTransport::Tui.available_here();
    let headless_pty_available = live::LiveTransport::HeadlessPty.available_here();
    assert_eq!(
        tui_available,
        !cfg!(windows),
        "the interactive TUI combination must be declared available exactly off Windows: {}",
        live::LiveTransport::Tui.unavailable_reason()
    );
    assert_eq!(
        headless_pty_available,
        !cfg!(windows),
        "the PTY-backed headless transport must be declared available exactly off Windows: {}",
        live::LiveTransport::HeadlessPty.unavailable_reason()
    );
    // The approval-channel fact that F-V2 turned on. A transport with no
    // approval channel cannot execute a gated delegation at all, so no child
    // can ever act on it — and that is a property of the SHIPPED confirmer, not
    // of this harness.
    assert!(
        !live::LiveTransport::Headless.has_approval_channel(),
        "the piped headless transport must be declared to have NO approval channel: {}",
        live::LiveTransport::Headless.approval_channel_reason()
    );
    for transport in [
        live::LiveTransport::JsonStream,
        live::LiveTransport::HeadlessPty,
        live::LiveTransport::Tui,
    ] {
        assert!(
            transport.has_approval_channel(),
            "{} must be declared to have an approval channel",
            transport.label()
        );
    }
    println!(
        "AVAILABILITY :: {} :: json-stream=true :: headless=true :: \
         headless-pty={headless_pty_available} :: tui={tui_available}",
        platform()
    );
}

// ===========================================================================
// Proof that the NO-CHANNEL canary can actually fail — FINDING F-V4
//
// "A canary that has never been seen to fail is not a canary." These three
// tests construct the exact scenario the verification report describes, show
// that every OTHER assertion in the harness stays green on it — which is why
// the hole survived two plans — and then show that the canary assertion goes
// red on it and green again once the scenario is removed.
//
// The scenario is built as data rather than by mutating the product, so it is a
// PERMANENT executable proof rather than a one-off manual demonstration: it
// re-runs on every CI pass and fails the day someone re-weakens the assertion.
// ===========================================================================

/// One synthetic execution row for the canary proofs.
fn row(surface: Surface, mode: Mode, outcome: Outcome) -> Execution {
    Execution {
        dimension: Dimension::Approval,
        surface,
        mode,
        outcome,
        obtained: "a child-sourced approval bypass".to_owned(),
        detail: "synthetic row constructed by the F-V4 canary proof".to_owned(),
        live: None,
        canary_trip: None,
    }
}

/// The F-V4 scenario, verbatim: a child-sourced approval request channel has
/// appeared AND is live-exploitable, so every leg reads ALLOWED.
fn realised_approval_widening() -> Vec<Execution> {
    vec![
        row(Surface::Standalone, Mode::InProcess, Outcome::Allowed),
        row(Surface::HostProtocol, Mode::InProcess, Outcome::Allowed),
        row(Surface::Standalone, Mode::Live, Outcome::Allowed),
        row(Surface::HostProtocol, Mode::Live, Outcome::Allowed),
    ]
}

/// The same dimension with the scenario removed.
fn intact_approval_absence() -> Vec<Execution> {
    vec![
        row(Surface::Standalone, Mode::InProcess, Outcome::NoChannel),
        row(Surface::HostProtocol, Mode::InProcess, Outcome::Refused),
        row(Surface::Standalone, Mode::Live, Outcome::NotExpressible),
        row(Surface::HostProtocol, Mode::Live, Outcome::NoChannel),
    ]
}

fn panicked(body: impl FnOnce()) -> bool {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    std::panic::set_hook(previous);
    outcome.is_err()
}

#[test]
fn every_other_assertion_stays_green_on_a_realised_approval_widening() {
    // This is the finding, reproduced. None of these four is capable of
    // reporting a fully realised approval widening, and the reasons differ:
    // equivalence compares WIDENED-or-not and both sides are widened, so they
    // agree; the census assertion returns early because approval's census
    // verdict is VACUOUS. Without the canary assertion the suite is green.
    let entry = cases::entry(Dimension::Approval);
    assert_eq!(entry.census_verdict, CensusVerdict::Vacuous);
    assert!(entry.no_channel_canary);
    let executions = realised_approval_widening();

    assert!(!panicked(|| assert_completeness(entry, &executions)));
    assert!(!panicked(|| assert_surface_equivalence(entry, &executions)));
    assert!(!panicked(|| assert_mode_equivalence(entry, &executions)));
    assert!(
        !panicked(|| assert_no_new_widening_against_the_census(entry, &executions)),
        "the census assertion is expected to return early on a VACUOUS dimension — that early \
         return is exactly the hole F-V4 names, and this test pins it so the canary assertion is \
         never mistaken for redundant"
    );
}

#[test]
fn the_no_channel_canary_goes_red_on_a_realised_approval_widening() {
    let entry = cases::entry(Dimension::Approval);
    let executions = realised_approval_widening();
    assert!(
        panicked(|| assert_no_channel_canaries_stayed_intact(entry, &executions)),
        "the NO-CHANNEL canary assertion did NOT fail on a fully realised approval widening. The \
         canary that the phase verdict calls its most important inheritance would once again be \
         incapable of reporting the exact event it exists for."
    );
}

#[test]
fn the_no_channel_canary_passes_once_the_widening_is_removed() {
    // The other half of the proof. A canary that fails on everything is as
    // useless as one that fails on nothing.
    let entry = cases::entry(Dimension::Approval);
    let executions = intact_approval_absence();
    assert!(
        !panicked(|| assert_no_channel_canaries_stayed_intact(entry, &executions)),
        "the NO-CHANNEL canary assertion failed on a dimension whose protection is intact"
    );
}

#[test]
fn a_structural_canary_trip_goes_red_on_any_dimension() {
    // The budget canary's trip is a `canary_trip` rather than an `Allowed`,
    // because a production caller starting to forward a child-supplied budget
    // override is news even while the ancestor rollup still refuses. Before
    // this repair its trip was a `String` interpolated into display text and
    // nothing asserted on it.
    let entry = cases::entry(Dimension::Cost);
    let mut executions = intact_approval_absence();
    for execution in &mut executions {
        execution.dimension = Dimension::Cost;
        execution.outcome = Outcome::Refused;
    }
    assert!(!panicked(|| assert_no_channel_canaries_stayed_intact(
        entry,
        &executions
    )));

    executions[0].canary_trip = Some(
        "a production file now forwards a Some(..) budget override into sub_budget".to_owned(),
    );
    assert!(
        panicked(|| assert_no_channel_canaries_stayed_intact(entry, &executions)),
        "a tripped structural canary did not fail the suite"
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
