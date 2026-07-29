//! The adapter surface: one canonical Goal terminal transition over all five
//! loop owners (F22C, Phase 22 Success Criterion 3).
//!
//! > *"Direct, ForgeFlows, Fleet, Council, and Anvil terminate through one
//! > canonical Goal transition with no nested verification/retry owner."*
//!
//! Every test here asserts an INVARIANT — a durable state, a refusal happening
//! at all, an exhaustive mapping — never an error string or a numeric status,
//! for the reason `goal_kernel_test.rs` states: encoding today's failure shape
//! into the suite makes it keep passing for the wrong reason the moment that
//! shape moves.
//!
//! Three of the eight behaviors 22-02 Task 3 lists are COMPILE-time properties
//! and cannot be asserted from a runtime test at all:
//!
//! * a strategy cannot terminate without passing through the canonical
//!   transition (the closure's return type is `StrategyTermination`);
//! * no generic retry wrapper can sit around an Anvil climb (`LoopOwner` is
//!   moved into the adapter);
//! * a `LoopOwner<CouncilTag>` cannot be handed to the Anvil evidence path.
//!
//! Those live as `compile_fail` doctests on `goal::strategy`. **`cargo nextest`
//! does not run doctests** — they need `cargo test --doc -p wcore-agent`, and
//! the executed count must be read back. A suite that silently ran zero of them
//! would report the same green as one that ran all of them.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use wcore_agent::goal::strategy::{
    AnvilOutcome, CouncilRunOutcome, DirectOutcome, FleetOutcome, GoalLoop, GoalLoopError,
    StrategyTermination, strategy_tag_name,
};
use wcore_agent::goal::{GoalKernel, GoalLifecycle};
use wcore_agent::orchestration::anvil::TerminalState;
use wcore_agent::orchestration::anvil::engine::{ClimbOutcome, EngineError};
use wcore_agent::orchestration::workflow::runner::{
    StageResult, WorkflowRunError, WorkflowRunResult,
};
use wcore_agent::session_journal::{SessionEvent, SessionJournal};
use wcore_swarm::fleet::{FleetError, ShardSummary};
use wcore_types::goal::{
    ExhaustionKind, GoalAuthorityRequest, GoalAuthoritySnapshot, GoalId, GoalStrategy,
    GoalTerminalState, HostGateObservation, LoopPolicy, resolve_goal_authority,
};

const SESSION: &str = "goal-strategy-session";
/// The stability bar every Anvil case in this file is graded against.
const REQUIRED_STABILITY: u32 = 3;

fn limits(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

fn snapshot(strategy: GoalStrategy) -> GoalAuthoritySnapshot {
    resolve_goal_authority(
        &GoalAuthorityRequest {
            requested_limits: limits(&[("max_tokens", 500)]),
            strategy,
            loop_policy: LoopPolicy::Once,
        },
        &limits(&[("max_tokens", 1000)]),
        "parent-envelope-digest",
    )
}

fn open_loop(path: &Path) -> GoalLoop {
    GoalLoop::new(GoalKernel::new(
        SessionJournal::open(path, SESSION).expect("journal opens"),
    ))
}

/// Open a Goal authorized for `strategy` and hand back the driver.
fn opened(path: &Path, id: &GoalId, strategy: GoalStrategy) -> GoalLoop {
    let driver = open_loop(path);
    driver
        .kernel()
        .open_goal(id, "close criterion 3", &snapshot(strategy), 1_700_000_000)
        .expect("goal opens");
    driver
}

fn terminal_of(driver: &GoalLoop, id: &GoalId) -> GoalTerminalState {
    match driver
        .kernel()
        .goal(id)
        .expect("goal reduces")
        .expect("goal exists")
        .lifecycle
    {
        GoalLifecycle::Terminated { terminal } => terminal,
        other => panic!("goal is not terminal: {other:?}"),
    }
}

fn climb(terminal: TerminalState, observation: Option<HostGateObservation>) -> ClimbOutcome {
    ClimbOutcome {
        terminal,
        stamp: String::new(),
        checks_passed: 0,
        checks_total: 0,
        iterations: 1,
        valve_fires: 0,
        winner: None,
        best_worktree: None,
        gate_observation: observation,
        landing: None,
    }
}

fn stage(is_error: bool) -> StageResult {
    StageResult {
        node_id: "n".to_owned(),
        text: String::new(),
        is_error,
        turns: 1,
    }
}

fn workflow(stages: &[bool]) -> WorkflowRunResult {
    WorkflowRunResult {
        final_state: serde_json::Value::Null,
        stage_results: stages.iter().map(|e| stage(*e)).collect(),
    }
}

fn shard(successes: usize, failures: usize) -> ShardSummary {
    ShardSummary {
        shard_id: 0,
        agent_count: successes + failures,
        successes,
        failures,
        payload: serde_json::Value::Null,
    }
}

// ---------------------------------------------------------------------------
// Behavior 1 — each strategy produces EXACTLY ONE canonical transition.
// Not zero, not two.
// ---------------------------------------------------------------------------

/// Count `GoalLoopOwnerFinished` records for `goal_id` by replaying the raw
/// chain. Counting terminal RECORDS rather than reading the reduced lifecycle is
/// the point: the reduced state cannot tell one terminal from two, so a test
/// that read it would pass for a Goal that terminated twice.
fn canonical_transitions(path: &Path, goal_id: &GoalId) -> usize {
    SessionJournal::replay(path)
        .expect("chain replays")
        .into_iter()
        .filter(|envelope| {
            matches!(
                &envelope.event,
                SessionEvent::GoalLoopOwnerFinished { goal_id: id, .. } if id == goal_id.as_str()
            )
        })
        .count()
}

#[tokio::test]
async fn each_of_the_five_strategies_produces_exactly_one_canonical_transition() {
    // Driven to a DIFFERENT terminal category per engine so the count is not an
    // artifact of one shared happy path.
    for strategy in GoalStrategy::ALL {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.journal");
        let id = GoalId::new(format!("g-one-{}", strategy_tag_name(strategy)));
        let driver = opened(&path, &id, strategy);

        match strategy {
            GoalStrategy::Direct => {
                driver
                    .run_direct(&id, |owner| async move {
                        StrategyTermination::from_direct(owner, DirectOutcome::Completed)
                    })
                    .await
                    .expect("direct terminates");
            }
            GoalStrategy::ForgeFlows => {
                driver
                    .run_forgeflows(&id, |owner| async move {
                        StrategyTermination::from_forgeflows(owner, Ok(&workflow(&[false, false])))
                    })
                    .await
                    .expect("forgeflows terminates");
            }
            GoalStrategy::Fleet => {
                driver
                    .run_fleet(&id, |owner| async move {
                        StrategyTermination::from_fleet(
                            owner,
                            FleetOutcome::Failed(&FleetError::Timeout(Duration::from_secs(1))),
                        )
                    })
                    .await
                    .expect("fleet terminates");
            }
            GoalStrategy::Council => {
                driver
                    .run_council(&id, |owner| async move {
                        StrategyTermination::from_council(
                            owner,
                            CouncilRunOutcome::Failed(&wcore_agent::orchestration::council::run::CouncilError::UnpriceableRoster),
                        )
                    })
                    .await
                    .expect("council terminates");
            }
            GoalStrategy::Anvil => {
                driver
                    .run_anvil(&id, |owner| async move {
                        StrategyTermination::from_anvil(
                            owner,
                            AnvilOutcome::EngineFailed(&EngineError::Gate(
                                "sandbox refused".to_owned(),
                            )),
                            REQUIRED_STABILITY,
                        )
                    })
                    .await
                    .expect("anvil terminates");
            }
        }

        assert_eq!(
            canonical_transitions(&path, &id),
            1,
            "{strategy:?} must produce exactly one canonical terminal transition"
        );
        // And it is a real terminal, so "exactly one" is not "exactly one of nothing".
        let _ = terminal_of(&driver, &id);
    }
}

#[tokio::test]
async fn a_goal_cannot_be_terminated_twice_through_the_canonical_transition() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-twice");
    let driver = opened(&path, &id, GoalStrategy::Direct);

    driver
        .run_direct(&id, |owner| async move {
            StrategyTermination::from_direct(owner, DirectOutcome::Completed)
        })
        .await
        .expect("first run terminates");

    let second = driver
        .run_direct(&id, |owner| async move {
            StrategyTermination::from_direct(owner, DirectOutcome::Cancelled)
        })
        .await;
    assert!(second.is_err(), "a terminal Goal admits no second run");
    assert_eq!(canonical_transitions(&path, &id), 1);
}

// ---------------------------------------------------------------------------
// Behavior 2 — the bypass is closed. While a loop owner is live, the plain
// terminate path is refused, so the canonical transition is the ONLY route.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_plain_terminate_path_is_refused_while_a_loop_owner_is_live() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-bypass");
    let driver = opened(&path, &id, GoalStrategy::Direct);

    let bypassed = driver
        .run_direct(&id, |owner| {
            let driver = driver.clone();
            let id = id.clone();
            async move {
                // Inside the engine, holding the claim: try to route around the
                // canonical transition using the public kernel API.
                let bypass = driver
                    .kernel()
                    .terminate(&id, GoalTerminalState::CriteriaChecked);
                assert!(
                    bypass.is_err(),
                    "a live loop owner must close the plain terminate path"
                );
                StrategyTermination::from_direct(owner, DirectOutcome::Completed)
            }
        })
        .await;
    assert!(bypassed.is_ok(), "the canonical transition still succeeds");

    // The Goal terminated through the canonical route, not the bypass.
    assert_eq!(canonical_transitions(&path, &id), 1);
    assert!(matches!(
        terminal_of(&driver, &id),
        GoalTerminalState::NeedsEscalation
    ));
}

// ---------------------------------------------------------------------------
// Behavior 3 — set completeness. A sixth strategy fails loudly.
// ---------------------------------------------------------------------------

#[test]
fn the_strategy_set_is_complete_and_every_member_has_a_distinct_tag() {
    // The compile-time half is the wildcard-free match inside `strategy_tag_name`:
    // a sixth `GoalStrategy` variant makes it a non-exhaustive-match error. This
    // is the runtime half — it also catches a copy-paste that pointed two
    // strategies at one tag, which would compile.
    let names: Vec<&'static str> = GoalStrategy::ALL
        .into_iter()
        .map(strategy_tag_name)
        .collect();
    assert_eq!(names.len(), 5);
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 5, "two strategies share a tag");
}

#[tokio::test]
async fn a_strategy_the_durable_record_did_not_authorize_is_refused() {
    // Behavior 8: strategy selection is read from the durable Goal record, never
    // inferred at dispatch time.
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-mismatch");
    let driver = opened(&path, &id, GoalStrategy::Anvil);

    let wrong = driver
        .run_council(&id, |owner| async move {
            StrategyTermination::from_council(
                owner,
                CouncilRunOutcome::Ran(
                    &wcore_agent::orchestration::council::driver::CouncilRunResult::Cancelled,
                ),
            )
        })
        .await;
    assert!(
        matches!(wrong, Err(GoalLoopError::StrategyMismatch { .. })),
        "a Goal authorized for Anvil must refuse a Council loop owner"
    );
    // The refusal did not terminate the Goal, and did not consume a claim.
    assert_eq!(canonical_transitions(&path, &id), 0);
    assert!(
        !driver
            .kernel()
            .goal(&id)
            .expect("reduces")
            .expect("exists")
            .is_terminal()
    );
}

// ---------------------------------------------------------------------------
// Behavior 4 — a nested loop-owner claim is refused, distinguishably, and
// leaves the Goal RESUMABLE.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_nested_loop_owner_claim_is_refused_and_the_goal_stays_resumable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-nested");
    let driver = opened(&path, &id, GoalStrategy::Direct);

    let outer = driver
        .run_direct(&id, |owner| {
            let driver = driver.clone();
            let id = id.clone();
            async move {
                // A second loop owner while the first is live: the exact nesting
                // Criterion 3 forbids.
                let nested = driver
                    .run_direct(&id, |inner| async move {
                        StrategyTermination::from_direct(inner, DirectOutcome::Completed)
                    })
                    .await;
                assert!(nested.is_err(), "a nested loop owner must be refused");

                // The refusal must NOT poison the Goal: it is still live, still
                // owned by the outer claim, and still terminable by it.
                let state = driver.kernel().goal(&id).expect("reduces").expect("exists");
                assert!(
                    !state.is_terminal(),
                    "a refused nesting must not terminate the Goal"
                );
                assert!(
                    state.loop_owner.is_some(),
                    "the outer claim survives the refusal"
                );

                StrategyTermination::from_direct(owner, DirectOutcome::Completed)
            }
        })
        .await;

    assert!(outer.is_ok(), "the outer owner still terminates normally");
    assert_eq!(canonical_transitions(&path, &id), 1);
}

// ---------------------------------------------------------------------------
// Behavior 6 — a verified terminal consumes REAL gate evidence; an adapter's
// summary of that evidence is refused. The negative cases are load-bearing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_anvil_climb_reaches_verified_only_on_real_host_observed_gate_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-verified");
    let driver = opened(&path, &id, GoalStrategy::Anvil);

    let observation =
        HostGateObservation::from_parent_executed_gate("a".repeat(64), 10, 10, REQUIRED_STABILITY);
    driver
        .run_anvil(&id, |owner| async move {
            StrategyTermination::from_anvil(
                owner,
                AnvilOutcome::Climbed(&climb(TerminalState::Verified, Some(observation))),
                REQUIRED_STABILITY,
            )
        })
        .await
        .expect("anvil terminates");

    assert!(
        terminal_of(&driver, &id).is_verified(),
        "a real gate observation clearing the bar earns the reserved stamp"
    );
}

#[tokio::test]
async fn an_anvil_climb_claiming_verified_without_evidence_is_refused_not_downgraded() {
    // The engine says Verified but carries NO observation — which is exactly the
    // shape an adapter paraphrase would have to fabricate around. The result
    // must be a refusal category, never a weaker verified.
    let cases: Vec<(&str, Option<HostGateObservation>)> = vec![
        ("no observation at all", None),
        (
            "a flaky rerun reports zero stability repeats",
            Some(HostGateObservation::from_parent_executed_gate(
                "a".repeat(64),
                10,
                10,
                0,
            )),
        ),
        (
            "stability below the required bar",
            Some(HostGateObservation::from_parent_executed_gate(
                "a".repeat(64),
                10,
                10,
                REQUIRED_STABILITY - 1,
            )),
        ),
        (
            "a partial gate pass",
            Some(HostGateObservation::from_parent_executed_gate(
                "a".repeat(64),
                9,
                10,
                REQUIRED_STABILITY,
            )),
        ),
        (
            "no pinned gate closure",
            Some(HostGateObservation::from_parent_executed_gate(
                "",
                10,
                10,
                REQUIRED_STABILITY,
            )),
        ),
    ];

    for (label, observation) in cases {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.journal");
        let id = GoalId::new("g-unverified");
        let driver = opened(&path, &id, GoalStrategy::Anvil);

        driver
            .run_anvil(&id, |owner| async move {
                StrategyTermination::from_anvil(
                    owner,
                    AnvilOutcome::Climbed(&climb(TerminalState::Verified, observation)),
                    REQUIRED_STABILITY,
                )
            })
            .await
            .expect("anvil terminates");

        let terminal = terminal_of(&driver, &id);
        assert!(!terminal.is_verified(), "{label}: must not reach verified");
        assert!(
            matches!(terminal, GoalTerminalState::NeedsEscalation),
            "{label}: must be an explicit refusal category, not a silent success"
        );
    }
}

#[tokio::test]
async fn no_strategy_other_than_anvil_can_reach_verified_even_with_perfect_evidence() {
    // The compiler closes the wrong-tag route (`from_anvil` will not take a
    // `LoopOwner<CouncilTag>` — see the compile_fail doctest). This closes the
    // remaining one: a hand-built durable record. The four non-Anvil strategies
    // have no host-observed verification owner, and the reducer says so.
    for strategy in GoalStrategy::ALL {
        if strategy.can_produce_host_observed_evidence() {
            continue;
        }
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.journal");
        let id = GoalId::new("g-forge");
        let driver = opened(&path, &id, strategy);
        let refused = driver.kernel().terminate(&id, GoalTerminalState::Verified);
        assert!(
            refused.is_err(),
            "{strategy:?} has no host-observed verification owner and must not reach verified"
        );
        assert!(
            !driver
                .kernel()
                .goal(&id)
                .expect("reduces")
                .expect("exists")
                .is_terminal()
        );
    }
}

// ---------------------------------------------------------------------------
// Behavior 7 — engine errors are carried as categories, never swallowed; the
// unpriced and partially-checked categories stay distinguishable from success
// and from failure.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn forgeflows_keeps_quality_and_resource_exhaustion_apart() {
    // The census's headline: SchemaValidationFailed and DispatchBudgetExceeded
    // are both "ran out of attempts" and mean opposite things. An operator fixes
    // the prompt for one and raises the budget for the other; collapsing them
    // destroys the only signal saying which.
    let quality = WorkflowRunError::SchemaValidationFailed {
        stage: "extract".to_owned(),
        attempts: 3,
        message: "not an object".to_owned(),
        partial: Box::new(workflow(&[false])),
    };
    let resource = WorkflowRunError::DispatchBudgetExceeded {
        limit: 64,
        attempted: 65,
        partial: Box::new(workflow(&[false])),
    };

    let mut kinds = Vec::new();
    for error in [&quality, &resource] {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.journal");
        let id = GoalId::new("g-exhaust");
        let driver = opened(&path, &id, GoalStrategy::ForgeFlows);
        driver
            .run_forgeflows(&id, |owner| async move {
                StrategyTermination::from_forgeflows(owner, Err(error))
            })
            .await
            .expect("forgeflows terminates");
        match terminal_of(&driver, &id) {
            GoalTerminalState::Exhausted { kind, .. } => kinds.push(kind),
            other => panic!("expected an exhaustion category, got {other:?}"),
        }
    }
    assert_eq!(
        kinds,
        vec![ExhaustionKind::Quality, ExhaustionKind::Resource]
    );
}

#[tokio::test]
async fn a_forgeflows_stage_failure_reports_its_partial_instead_of_discarding_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-partial");
    let driver = opened(&path, &id, GoalStrategy::ForgeFlows);

    // Four stages ran, one errored. Rounding that to "failed" loses the work.
    let error = WorkflowRunError::StageFailed {
        stage: "c".to_owned(),
        message: "boom".to_owned(),
        partial: Box::new(workflow(&[false, false, false, true])),
    };
    driver
        .run_forgeflows(&id, |owner| async move {
            StrategyTermination::from_forgeflows(owner, Err(&error))
        })
        .await
        .expect("forgeflows terminates");

    assert_eq!(
        terminal_of(&driver, &id),
        GoalTerminalState::PartiallyCompleted {
            completed: 3,
            failed: 1
        }
    );
}

#[tokio::test]
async fn a_fleet_run_is_bound_at_shard_summary_and_keeps_the_success_failure_split() {
    // The census §3 finding: the fleet-level result is a caller-chosen `T`, so
    // an adapter written against it adapts whatever the caller felt like
    // returning. This binds at `ShardSummary`, before the reducer collapses it.
    // 97-of-100 is neither success nor failure.
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-fleet");
    let driver = opened(&path, &id, GoalStrategy::Fleet);

    let shards = vec![shard(50, 2), shard(47, 1)];
    driver
        .run_fleet(&id, |owner| async move {
            StrategyTermination::from_fleet(owner, FleetOutcome::Dispatched(&shards))
        })
        .await
        .expect("fleet terminates");

    assert_eq!(
        terminal_of(&driver, &id),
        GoalTerminalState::PartiallyCompleted {
            completed: 97,
            failed: 3
        }
    );
}

#[tokio::test]
async fn a_council_that_could_not_be_priced_is_unpriced_and_not_blocked() {
    // `Unpriced` is the one carrier the census said the lifted taxonomy had to
    // add: folding it into `Blocked` loses that the run never started, and that
    // the reason was pricing.
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-unpriced");
    let driver = opened(&path, &id, GoalStrategy::Council);

    driver
        .run_council(&id, |owner| async move {
            StrategyTermination::from_council(
                owner,
                CouncilRunOutcome::Failed(
                    &wcore_agent::orchestration::council::run::CouncilError::UnpriceableRoster,
                ),
            )
        })
        .await
        .expect("council terminates");

    assert!(
        matches!(
            terminal_of(&driver, &id),
            GoalTerminalState::Unpriced { .. }
        ),
        "an unpriceable roster is refused for a PRICING reason, distinct from Blocked"
    );
}

#[tokio::test]
async fn a_direct_run_that_merely_completed_does_not_claim_any_verification() {
    // Direct has no verification owner at all (census §1). A completed Direct
    // run must not land in a category that claims checks ran.
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-direct");
    let driver = opened(&path, &id, GoalStrategy::Direct);

    driver
        .run_direct(&id, |owner| async move {
            StrategyTermination::from_direct(owner, DirectOutcome::Completed)
        })
        .await
        .expect("direct terminates");

    let terminal = terminal_of(&driver, &id);
    assert!(!terminal.is_verified());
    assert!(
        !matches!(
            terminal,
            GoalTerminalState::CriteriaChecked | GoalTerminalState::SelfChecked
        ),
        "Direct produces no verdict about its own output, so it may not claim one"
    );
}

// ---------------------------------------------------------------------------
// Every Anvil terminal state has a canonical home, and only one is verified.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_anvil_terminal_state_maps_to_exactly_one_canonical_category() {
    let anvil_states = [
        TerminalState::CriteriaChecked,
        TerminalState::SelfChecked,
        TerminalState::NeedsEscalation,
        TerminalState::Blocked("gate cannot execute".to_owned()),
        TerminalState::Cancelled,
        TerminalState::TimedOut,
        TerminalState::PermissionDenied,
        TerminalState::CrashedRecovered,
        TerminalState::Superseded,
    ];

    let mut mapped = Vec::new();
    for state in anvil_states {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("session.journal");
        let id = GoalId::new("g-anvil-map");
        let driver = opened(&path, &id, GoalStrategy::Anvil);
        driver
            .run_anvil(&id, |owner| async move {
                StrategyTermination::from_anvil(
                    owner,
                    AnvilOutcome::Climbed(&climb(state, None)),
                    REQUIRED_STABILITY,
                )
            })
            .await
            .expect("anvil terminates");
        mapped.push(terminal_of(&driver, &id));
    }

    // None of the nine non-verified Anvil states may reach the reserved stamp.
    assert_eq!(mapped.iter().filter(|t| t.is_verified()).count(), 0);
    assert_eq!(mapped.len(), 9);
}

// ---------------------------------------------------------------------------
// Falsification: the gates above must be able to fail.
// ---------------------------------------------------------------------------

#[test]
fn the_tag_completeness_gate_can_fail() {
    // A gate that cannot fail proves nothing. This asserts the DETECTOR, not the
    // property: had two strategies shared a tag, the dedup in
    // `the_strategy_set_is_complete_and_every_member_has_a_distinct_tag` would
    // shrink the set and the assertion would go red. Proven here on a
    // deliberately-collided list rather than by breaking the real one.
    let collided = ["DirectTag", "DirectTag", "FleetTag"];
    let mut unique = collided.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_ne!(
        unique.len(),
        collided.len(),
        "the dedup detector must notice a collision"
    );
}

#[test]
fn the_transition_counter_can_report_a_number_other_than_one() {
    // The same discipline for `canonical_transitions`: a counter that always
    // returned 1 would make every "exactly one" assertion above a tautology. A
    // Goal that never ran a strategy must count ZERO.
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-zero");
    let _driver = opened(&path, &id, GoalStrategy::Direct);
    assert_eq!(
        canonical_transitions(&path, &id),
        0,
        "a Goal with no strategy run has no canonical transition"
    );
}
