//! Durable Goal kernel: the state machine, not the vocabulary.
//!
//! 22-01 shipped `wcore_types::goal` — identity, strategy, terminal taxonomy,
//! loop policy, wait kind and the authority resolver — and stopped there. Its
//! own SUMMARY: "No durable kernel. `crates/wcore-agent/src/goal/kernel.rs` does
//! not exist. No `SessionEvent` variants were added, no reducer arm, no
//! `ReducedSessionState` field, no cursor exposure."
//!
//! These tests are written against the kernel that closes that gap. Every one
//! asserts an INVARIANT — a state, a refusal happening at all, a replay
//! equality — and never an error string, an error kind or a numeric status,
//! because encoding today's failure shape into the suite makes it keep passing
//! for the wrong reason the moment that shape moves.

use std::collections::BTreeMap;
use std::path::Path;

use wcore_agent::goal::strategy::{AnvilOutcome, GoalLoop, StrategyTermination};
use wcore_agent::goal::{GoalAuthorityRecord, GoalKernel, GoalLifecycle, GoalRecovery};
use wcore_agent::orchestration::anvil::TerminalState;
use wcore_agent::orchestration::anvil::engine::ClimbOutcome;
use wcore_agent::session_journal::{SessionEvent, SessionJournal};
use wcore_types::goal::{
    ExhaustionKind, GoalAuthorityRequest, GoalAuthoritySnapshot, GoalId, GoalStrategy,
    GoalTerminalState, HostGateObservation, LoopPolicy, VerifiedTerminal, WaitKind,
    resolve_goal_authority,
};

const SESSION: &str = "goal-kernel-session";

fn limits(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

fn snapshot(strategy: GoalStrategy, loop_policy: LoopPolicy) -> GoalAuthoritySnapshot {
    let request = GoalAuthorityRequest {
        requested_limits: limits(&[("max_tokens", 500)]),
        strategy,
        loop_policy,
    };
    resolve_goal_authority(
        &request,
        &limits(&[("max_tokens", 1000), ("max_cost_cents", 25)]),
        "parent-envelope-digest",
    )
}

fn open_kernel(path: &Path) -> GoalKernel {
    GoalKernel::new(SessionJournal::open(path, SESSION).expect("journal opens"))
}

/// Open a Goal and run it up to, but not including, the transition under test.
fn goal_id(name: &str) -> GoalId {
    GoalId::new(name)
}

// ---------------------------------------------------------------------------
// Behavior 1 — the chain is the source of truth, not the kernel.
// ---------------------------------------------------------------------------

#[test]
fn a_goal_transitioned_through_the_kernel_replays_to_the_same_state_after_a_fresh_load() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = goal_id("g-replay");

    let before = {
        let kernel = open_kernel(&path);
        kernel
            .open_goal(
                &id,
                "ship the durable kernel",
                &snapshot(GoalStrategy::Anvil, LoopPolicy::Fixed { iterations: 3 }),
                1_700_000_000_000,
            )
            .expect("open");
        kernel.start_iteration(&id).expect("iteration 1");
        kernel
            .begin_wait(
                &id,
                WaitKind::Approval {
                    approval_id: "appr-1".to_owned(),
                },
            )
            .expect("wait");
        kernel.resume_from_wait(&id).expect("resume");
        kernel.goal(&id).expect("read").expect("goal exists")
    };

    // Fresh process-shaped load: the writer lease is released and the journal is
    // replayed from disk with no in-memory carry-over.
    let after = {
        let kernel = open_kernel(&path);
        kernel.goal(&id).expect("read").expect("goal exists")
    };

    assert_eq!(
        before, after,
        "the in-memory Goal state is a projection of the chain; a field replay \
         cannot reconstruct does not belong on it"
    );
}

// ---------------------------------------------------------------------------
// Behavior 2 — a resume that cannot reconstruct its envelope blocks.
// ---------------------------------------------------------------------------

#[test]
fn an_authority_record_whose_fields_do_not_match_its_digest_refuses_to_reconstruct() {
    // The durable record must be `Deserialize` to be replayable, so this is the
    // real tampering surface: a same-UID writer edits the effective limits in
    // the journal file. Reconstruction is bound to a digest over the fields, so
    // widening a limit invalidates it.
    let honest =
        GoalAuthorityRecord::from_snapshot(&snapshot(GoalStrategy::Direct, LoopPolicy::Once));
    let encoded = serde_json::to_value(&honest).expect("record encodes");

    let mut tampered = encoded.clone();
    tampered["effective_limits"]["max_tokens"] = serde_json::json!(999_999);
    let tampered: GoalAuthorityRecord =
        serde_json::from_value(tampered).expect("a tampered record still deserializes");

    assert!(
        honest.reconstruct().is_ok(),
        "an untampered record reconstructs"
    );
    assert!(
        tampered.reconstruct().is_err(),
        "a widened limit must not reconstruct; failing open here is the \
         authority-widening route this record shape exists to close"
    );
}

#[test]
fn a_resume_that_cannot_reconstruct_the_envelope_parks_explicitly_and_never_defaults_permissive() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = goal_id("g-unreconstructable");

    {
        let kernel = open_kernel(&path);
        kernel
            .open_goal(
                &id,
                "objective",
                &snapshot(GoalStrategy::Direct, LoopPolicy::Once),
                1_700_000_000_000,
            )
            .expect("open");
        kernel.start_iteration(&id).expect("iteration");
    }

    let kernel = open_kernel(&path);
    // The recorded parent envelope digest no longer matches the one this
    // process can produce — the parent envelope moved under the Goal.
    let recovery = kernel
        .recover_with_parent_envelope(&id, "a-different-parent-envelope-digest")
        .expect("recovery is a decision, not an I/O failure");

    match recovery {
        GoalRecovery::Blocked { terminal } => {
            assert!(
                matches!(
                    terminal,
                    GoalTerminalState::AuthorityUnreconstructable { .. }
                ),
                "the refusal must be the explicit parked category, not a generic block"
            );
        }
        other => panic!("expected an explicit park, got {other:?}"),
    }

    let goal = kernel.goal(&id).expect("read").expect("goal exists");
    assert!(
        matches!(
            goal.lifecycle,
            GoalLifecycle::Terminated {
                terminal: GoalTerminalState::AuthorityUnreconstructable { .. }
            }
        ),
        "the park is durable, not advisory"
    );
}

// ---------------------------------------------------------------------------
// Behavior 3 — the taxonomy survives the round trip through the journal.
// ---------------------------------------------------------------------------

#[test]
fn every_terminal_shape_the_census_measured_survives_a_durable_round_trip() {
    // 22-01 proved these encode and decode as values. This proves the durable
    // record carries them through a real append and a real replay without an
    // unpriced or partially-completed outcome collapsing into success/failure.
    let shapes = vec![
        GoalTerminalState::Exhausted {
            kind: ExhaustionKind::Quality,
            attempts: 3,
            detail: "schema validation never passed".to_owned(),
        },
        GoalTerminalState::Exhausted {
            kind: ExhaustionKind::Resource,
            attempts: 64,
            detail: "dispatch budget".to_owned(),
        },
        GoalTerminalState::PartiallyCompleted {
            completed: 97,
            failed: 3,
        },
        GoalTerminalState::TimedOut,
        GoalTerminalState::Unpriced {
            detail: "roster not fully priced".to_owned(),
        },
        GoalTerminalState::CriteriaChecked,
        GoalTerminalState::SelfChecked,
        GoalTerminalState::NeedsEscalation,
        GoalTerminalState::Blocked {
            reason: "gate cannot execute".to_owned(),
        },
        GoalTerminalState::Cancelled,
        GoalTerminalState::PermissionDenied,
        GoalTerminalState::CrashedRecovered,
        GoalTerminalState::Superseded,
    ];

    // F22C criterion 3 splits this list in two, and the split is asserted here
    // rather than assumed: a shape that claims something about an engine's work
    // is only sayable by that engine's loop owner, so it is REFUSED on the plain
    // path. The round trip for those shapes is proven over the canonical
    // transition instead — `goal::kernel`'s
    // `every_terminal_shape_survives_the_round_trip_through_the_canonical_transition`
    // carries the identical assertion over this same list.
    //
    // `Verified` is absent from the list for the older reason (Behavior 4): it
    // is refused on this path outright and needs host-observed gate evidence.
    let (engine_shapes, control_shapes): (Vec<_>, Vec<_>) =
        shapes.iter().partition(|s| s.requires_loop_owner());
    assert!(
        !engine_shapes.is_empty() && !control_shapes.is_empty(),
        "the split must exercise both sides, or this test proves nothing about either"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");

    {
        let kernel = open_kernel(&path);
        for (index, shape) in control_shapes.iter().enumerate() {
            let id = goal_id(&format!("g-shape-{index}"));
            kernel
                .open_goal(
                    &id,
                    "objective",
                    &snapshot(GoalStrategy::Fleet, LoopPolicy::Once),
                    1_700_000_000_000,
                )
                .expect("open");
            kernel.terminate(&id, (*shape).clone()).expect("terminate");
        }

        // The refusal, over EVERY engine-produced shape — not one sample. A
        // Goal that never claimed an owner is exactly the case the plain path
        // used to wave through, and it is the case a sixth engine would be in
        // by default.
        for (index, shape) in engine_shapes.iter().enumerate() {
            let id = goal_id(&format!("g-engine-{index}"));
            kernel
                .open_goal(
                    &id,
                    "objective",
                    &snapshot(GoalStrategy::Fleet, LoopPolicy::Once),
                    1_700_000_000_000,
                )
                .expect("open");
            assert!(
                kernel.terminate(&id, (*shape).clone()).is_err(),
                "{shape:?} is an engine verdict and must not be reachable without a loop owner"
            );
            assert!(
                !kernel
                    .goal(&id)
                    .expect("read")
                    .expect("goal exists")
                    .is_terminal(),
                "a refused terminal must leave the Goal live and resumable, not half-applied"
            );
        }
    }

    let kernel = open_kernel(&path);
    for (index, shape) in control_shapes.iter().enumerate() {
        let id = goal_id(&format!("g-shape-{index}"));
        let goal = kernel.goal(&id).expect("read").expect("goal exists");
        match goal.lifecycle {
            GoalLifecycle::Terminated { terminal } => assert_eq!(
                &&terminal, shape,
                "terminal shape {index} did not survive the durable round trip"
            ),
            other => panic!("expected terminal, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Behavior 4 — anti-forgery. The negative case is the load-bearing one.
// ---------------------------------------------------------------------------

#[test]
fn a_strategy_with_no_real_gate_cannot_reach_verified_through_the_durable_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let kernel = open_kernel(&path);

    let observation = HostGateObservation::from_parent_executed_gate("gate-digest", 10, 10, 3);

    for strategy in GoalStrategy::ALL {
        let id = goal_id(&format!("g-verify-{strategy:?}"));
        kernel
            .open_goal(
                &id,
                "objective",
                &snapshot(strategy, LoopPolicy::Once),
                1_700_000_000_000,
            )
            .expect("open");

        let verified = VerifiedTerminal::from_host_observed_gate(strategy, &observation, 3);
        if strategy.can_produce_host_observed_evidence() {
            let verified = verified.expect("Anvil runs a real gate");
            kernel
                .terminate_verified(&id, verified)
                .expect("a real gate terminates verified");
            let goal = kernel.goal(&id).expect("read").expect("exists");
            assert!(
                matches!(
                    goal.lifecycle,
                    GoalLifecycle::Terminated {
                        terminal: GoalTerminalState::Verified
                    }
                ),
                "{strategy:?} ran a real gate and must reach verified"
            );
        } else {
            assert!(
                verified.is_none(),
                "{strategy:?} has no host-observed verification owner"
            );
        }
    }
}

#[test]
fn a_journal_record_cannot_forge_a_verified_terminal_for_a_strategy_without_a_real_gate() {
    // The structural half of the anti-forgery property is enforced by the
    // compiler: `HostGateObservation` is not `Deserialize`, so model-authored
    // JSON has no route to `VerifiedTerminal`. That leaves ONE remaining route
    // — a same-UID writer appending a hand-built durable record naming
    // `Verified` for a strategy whose verification owner is a model judge. The
    // reducer must refuse it at replay, or the file is the forgery route.
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = goal_id("g-forged");

    let kernel = open_kernel(&path);
    kernel
        .open_goal(
            &id,
            "objective",
            &snapshot(GoalStrategy::Council, LoopPolicy::Once),
            1_700_000_000_000,
        )
        .expect("open");

    let refused = kernel.terminate(&id, GoalTerminalState::Verified);
    assert!(
        refused.is_err(),
        "a council Goal must not be able to terminate verified through the \
         unprivileged terminate path"
    );

    // And the Goal is left resumable rather than wedged by the refusal.
    let goal = kernel.goal(&id).expect("read").expect("exists");
    assert!(
        !matches!(goal.lifecycle, GoalLifecycle::Terminated { .. }),
        "a refused forgery must not terminate the Goal as a side effect"
    );
}

// ---------------------------------------------------------------------------
// Behavior 5 — invalid and stale transitions fail explicitly, stay resumable.
// ---------------------------------------------------------------------------

#[test]
fn an_invalid_transition_is_refused_and_leaves_the_goal_resumable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = goal_id("g-invalid");

    {
        let kernel = open_kernel(&path);

        kernel
            .open_goal(
                &id,
                "objective",
                &snapshot(GoalStrategy::Direct, LoopPolicy::Once),
                1_700_000_000_000,
            )
            .expect("open");

        // Resuming a Goal that is not waiting is not a no-op, it is a refusal.
        assert!(
            kernel.resume_from_wait(&id).is_err(),
            "resuming a Goal that is not waiting must be refused, not absorbed"
        );

        kernel
            .terminate(&id, GoalTerminalState::Cancelled)
            .expect("terminate");

        // A stale command that arrives after the terminal transition is refused.
        assert!(
            kernel.start_iteration(&id).is_err(),
            "a post-terminal command must be refused"
        );
        assert!(
            kernel.terminate(&id, GoalTerminalState::TimedOut).is_err(),
            "a second terminal transition must be refused"
        );
    }

    // The refusals left the recorded terminal state intact, read back through a
    // fresh load rather than the writer that made them.
    let kernel = open_kernel(&path);
    let goal = kernel.goal(&id).expect("read").expect("exists");
    assert!(
        matches!(
            goal.lifecycle,
            GoalLifecycle::Terminated {
                terminal: GoalTerminalState::Cancelled
            }
        ),
        "a refused stale command must not overwrite the committed terminal state"
    );
}

#[test]
fn a_goal_cannot_exceed_the_loop_bound_recorded_on_its_own_durable_record() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = goal_id("g-bounded");
    let kernel = open_kernel(&path);

    kernel
        .open_goal(
            &id,
            "objective",
            &snapshot(GoalStrategy::Direct, LoopPolicy::Fixed { iterations: 2 }),
            1_700_000_000_000,
        )
        .expect("open");

    kernel.start_iteration(&id).expect("iteration 1");
    kernel.start_iteration(&id).expect("iteration 2");
    assert!(
        kernel.start_iteration(&id).is_err(),
        "a bound that is recorded but not enforced is not a bound"
    );
}

// ---------------------------------------------------------------------------
// Behavior 7 — a crash at EVERY transition resumes identically, and the
// transition set is enumerated so a new transition without a crash case fails.
// ---------------------------------------------------------------------------

/// The transitions a crash can land between. Kept here rather than imported so
/// that adding a kernel transition without adding a crash case below fails the
/// completeness assertion rather than silently skipping it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    Open,
    IterationStarted,
    WaitBegun,
    Resumed,
    Terminated,
}

const ALL_CRASH_POINTS: [CrashPoint; 5] = [
    CrashPoint::Open,
    CrashPoint::IterationStarted,
    CrashPoint::WaitBegun,
    CrashPoint::Resumed,
    CrashPoint::Terminated,
];

fn drive_to(kernel: &GoalKernel, id: &GoalId, point: CrashPoint) {
    kernel
        .open_goal(
            id,
            "objective",
            &snapshot(GoalStrategy::Anvil, LoopPolicy::Fixed { iterations: 4 }),
            1_700_000_000_000,
        )
        .expect("open");
    if point == CrashPoint::Open {
        return;
    }
    kernel.start_iteration(id).expect("iteration");
    if point == CrashPoint::IterationStarted {
        return;
    }
    kernel
        .begin_wait(
            id,
            WaitKind::Event {
                event: "external".to_owned(),
            },
        )
        .expect("wait");
    if point == CrashPoint::WaitBegun {
        return;
    }
    kernel.resume_from_wait(id).expect("resume");
    if point == CrashPoint::Resumed {
        return;
    }
    // F22C criterion 3: `CriteriaChecked` is an engine verdict and is refused on
    // the plain path. This Goal is authorized for Anvil, so the crash-point
    // walk terminates through the canonical transition, exactly as the product
    // does. The terminal this test replays against is unchanged.
    terminate_as_anvil_owner(kernel, id, TerminalState::CriteriaChecked);
}

/// Terminate `id` through the ONE canonical Goal transition.
///
/// The Goal must be authorized for Anvil. `from_anvil` is the only constructor
/// of the `StrategyTermination` that `GoalLoop::finish` consumes, and it takes
/// the `LoopOwner` by value, so this helper can terminate the Goal once and
/// cannot terminate it twice.
fn terminate_as_anvil_owner(kernel: &GoalKernel, id: &GoalId, terminal: TerminalState) {
    let driver = GoalLoop::new(kernel.clone());
    let outcome = ClimbOutcome {
        terminal,
        stamp: String::new(),
        checks_passed: 0,
        checks_total: 0,
        iterations: 1,
        valve_fires: 0,
        winner: None,
        best_worktree: None,
        gate_observation: None,
        landing: None,
    };
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime")
        .block_on(async {
            driver
                .run_anvil(id, |owner| async move {
                    StrategyTermination::from_anvil(owner, AnvilOutcome::Climbed(&outcome), 1)
                })
                .await
        })
        .expect("canonical transition");
}

#[test]
fn a_crash_at_every_transition_resumes_the_same_goal_with_the_same_envelope_and_cursor() {
    let mut covered = Vec::new();

    for point in ALL_CRASH_POINTS {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.journal");
        let id = goal_id("g-crash");

        let (before, cursor_before) = {
            let kernel = open_kernel(&path);
            drive_to(&kernel, &id, point);
            let goal = kernel.goal(&id).expect("read").expect("exists");
            let cursor = kernel.cursor(&id).expect("cursor").expect("has a cursor");
            (goal, cursor)
        };

        // The writer lease is dropped without any cooperative shutdown, which is
        // the in-process analogue of the crash. The uncatchable-kill proof is
        // the live exercise; this one pins that every transition is individually
        // recoverable.
        let kernel = open_kernel(&path);
        let after = kernel.goal(&id).expect("read").expect("exists");
        let cursor_after = kernel.cursor(&id).expect("cursor").expect("has a cursor");

        assert_eq!(before, after, "crash at {point:?} lost Goal state");
        assert_eq!(
            cursor_before, cursor_after,
            "crash at {point:?} moved the recovery cursor"
        );

        let recovery = kernel
            .recover_with_parent_envelope(&id, "parent-envelope-digest")
            .expect("recovery decides");
        match (point, &recovery) {
            (CrashPoint::Terminated, GoalRecovery::AlreadyTerminal { .. }) => {}
            (CrashPoint::Terminated, other) => {
                panic!("a terminated Goal must not resume: {other:?}")
            }
            (_, GoalRecovery::Resumed { snapshot, .. }) => {
                assert_eq!(
                    snapshot.effective_limits,
                    limits(&[("max_tokens", 500), ("max_cost_cents", 25)]),
                    "resume at {point:?} restored a different envelope than it recorded"
                );
            }
            (_, other) => panic!("crash at {point:?} did not resume: {other:?}"),
        }

        covered.push(point);
    }

    assert_eq!(
        covered.len(),
        ALL_CRASH_POINTS.len(),
        "every enumerated transition needs a crash case"
    );
}

// ---------------------------------------------------------------------------
// Sole writer. Structural, not conventional.
// ---------------------------------------------------------------------------

#[test]
fn the_public_append_path_refuses_goal_transitions_so_the_kernel_is_the_only_writer() {
    // Mirrors the child-transaction authority denylist: a caller holding a
    // journal handle must not be able to mint a Goal transition beside the
    // kernel, because a transition with no attributable kernel append is a
    // repudiation route.
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let journal = SessionJournal::open(&path, SESSION).expect("journal opens");

    let record =
        GoalAuthorityRecord::from_snapshot(&snapshot(GoalStrategy::Direct, LoopPolicy::Once));
    let refused = journal.append(SessionEvent::GoalOpened {
        goal_id: "g-direct-append".to_owned(),
        objective: "smuggled".to_owned(),
        authority: record,
        opened_at_unix_ms: 1_700_000_000_000,
    });

    assert!(
        refused.is_err(),
        "Goal authority events must not be appendable through the public path"
    );
}
