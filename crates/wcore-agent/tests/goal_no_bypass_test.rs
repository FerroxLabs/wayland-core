//! F22C criterion 3, the COMPLETENESS half: an engine verdict is sayable only
//! by the Goal's loop owner — for each of the five owners, separately.
//!
//! > *"Direct, ForgeFlows, Fleet, Council, and Anvil terminate through one
//! > canonical Goal transition with no nested verification/retry owner."*
//!
//! ## What was already true, and what was not
//!
//! `goal_strategy_test.rs` proves the SOUNDNESS half: once a Goal has a loop
//! owner, exactly one canonical termination is possible. That half was closed
//! by construction — `LoopOwner` is not `Clone`, the five adapters consume it
//! by value, `StrategyTermination` has no other constructor, `finish_loop_owner`
//! is `pub(crate)`, and `SessionJournal::append` refuses every `Goal*` variant.
//!
//! The completeness half was NOT closed, and this file is where it is measured.
//! Attaching a Goal to an engine was a caller's choice. An engine could run a
//! Goal to completion without ever claiming, then record a full engine verdict
//! straight down the plain `GoalKernel::terminate` path. The reducer waved that
//! through, on the reasoning that a Goal with no claim had had no engine run it
//! — which is false exactly when attachment is opt-in, i.e. always.
//!
//! ## The three assertions, per owner
//!
//! LANE-BRIEF §3.2 and §3b-i: a gate that cannot fail proves nothing, and an
//! absence claim is the easiest thing in the world to pass by accident. So every
//! one of the five cases below carries all three, and none of them is shared
//! across the set:
//!
//! 1. **known-positive** — the engine's REAL outcome, through its REAL adapter,
//!    reaches the durable record. If this fails the refusal is over-broad and
//!    has broken the product.
//! 2. **known-negative** — the SAME terminal category, written on the plain path
//!    with no owner, is refused, and the Goal is left live and resumable.
//! 3. **the old shape would have missed it** — at the moment of the refusal,
//!    `loop_owner` is `None`. That is precisely the condition the previous guard
//!    tested (`if let Some(owner) = &goal.loop_owner`), so the old code would
//!    have ACCEPTED this write. Without this third assertion the file would pass
//!    identically against the unfixed reducer.

use std::collections::BTreeMap;

use wcore_agent::goal::strategy::{
    AnvilOutcome, CouncilRunOutcome, DirectOutcome, FleetOutcome, GoalLoop, StrategyTermination,
};
use wcore_agent::goal::{GoalKernel, GoalLifecycle};
use wcore_agent::orchestration::anvil::TerminalState;
use wcore_agent::orchestration::anvil::engine::ClimbOutcome;
use wcore_agent::orchestration::workflow::runner::{StageResult, WorkflowRunResult};
use wcore_agent::session_journal::SessionJournal;
use wcore_swarm::fleet::ShardSummary;
use wcore_types::goal::{
    GoalAuthorityRequest, GoalAuthoritySnapshot, GoalId, GoalStrategy, GoalTerminalState,
    LoopPolicy, resolve_goal_authority,
};

const SESSION: &str = "goal-no-bypass";

fn snapshot(strategy: GoalStrategy) -> GoalAuthoritySnapshot {
    resolve_goal_authority(
        &GoalAuthorityRequest {
            requested_limits: BTreeMap::new(),
            strategy,
            loop_policy: LoopPolicy::Once,
        },
        &BTreeMap::new(),
        "parent-envelope-digest",
    )
}

fn kernel_at(path: &std::path::Path) -> GoalKernel {
    GoalKernel::new(SessionJournal::open(path, SESSION).expect("journal opens"))
}

fn open(kernel: &GoalKernel, id: &GoalId, strategy: GoalStrategy) {
    kernel
        .open_goal(
            id,
            "close criterion 3",
            &snapshot(strategy),
            1_700_000_000_000,
        )
        .expect("goal opens");
}

fn terminal_of(kernel: &GoalKernel, id: &GoalId) -> GoalTerminalState {
    match kernel
        .goal(id)
        .expect("read")
        .expect("goal exists")
        .lifecycle
    {
        GoalLifecycle::Terminated { terminal } => terminal,
        other => panic!("expected a terminal lifecycle, got {other:?}"),
    }
}

/// Assertions 2 and 3, run against a Goal that has NEVER claimed an owner.
///
/// Returns the refusal message so each caller can show its own verbatim.
fn refuse_without_owner(
    kernel: &GoalKernel,
    id: &GoalId,
    engine_verdict: GoalTerminalState,
    owner_label: &str,
) -> String {
    // Assertion 3 FIRST, because it is the one that establishes this test is
    // not self-passing. The previous guard was `if let Some(owner) =
    // &goal.loop_owner { refuse }` — so with no claim held, the old reducer
    // reached `apply_goal_terminal` and committed the write. Everything below
    // is therefore measuring behaviour that did not exist before.
    let before = kernel.goal(id).expect("read").expect("goal exists");
    assert!(
        before.loop_owner.is_none(),
        "{owner_label}: this case only distinguishes the new rule from the old one when NO claim \
         is live; with a claim held, the old reducer refused too and this test would pass against \
         the unfixed code"
    );
    assert!(
        !before.is_terminal(),
        "{owner_label}: the goal must still be live, or the refusal below proves nothing"
    );
    assert!(
        engine_verdict.requires_loop_owner(),
        "{owner_label}: {engine_verdict:?} is not an engine verdict, so this case would be \
         testing the control-plane path by mistake"
    );

    // Assertion 2 — the known-negative.
    let refused = kernel.terminate(id, engine_verdict.clone());
    let error = refused.expect_err(&format!(
        "{owner_label}: {engine_verdict:?} must not be reachable without the goal's loop owner"
    ));

    // A refusal that half-applied would be worse than none: the Goal must be
    // left exactly as it was, still resumable by a real owner.
    let after = kernel.goal(id).expect("read").expect("goal exists");
    assert!(
        !after.is_terminal(),
        "{owner_label}: a refused terminal must leave the goal live and resumable"
    );
    error.to_string()
}

// ── Owner 1 — Direct ────────────────────────────────────────────────────────

#[tokio::test]
async fn direct_terminates_through_its_owner_and_nowhere_else() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let kernel = kernel_at(&path);

    // Assertion 2 + 3 — Direct's own verdict, refused with no owner.
    let bypass = GoalId::new("direct-bypass");
    open(&kernel, &bypass, GoalStrategy::Direct);
    let message = refuse_without_owner(
        &kernel,
        &bypass,
        GoalTerminalState::NeedsEscalation,
        "Direct",
    );
    println!("DIRECT_REFUSAL={message}");

    // Assertion 1 — the real adapter still reaches the durable record.
    let ok = GoalId::new("direct-ok");
    open(&kernel, &ok, GoalStrategy::Direct);
    GoalLoop::new(kernel.clone())
        .run_direct(&ok, |owner| async move {
            StrategyTermination::from_direct(owner, DirectOutcome::Completed)
        })
        .await
        .expect("direct terminates canonically");
    assert_eq!(
        terminal_of(&kernel, &ok),
        GoalTerminalState::NeedsEscalation
    );
}

// ── Owner 2 — ForgeFlows ────────────────────────────────────────────────────

#[tokio::test]
async fn forgeflows_terminates_through_its_owner_and_nowhere_else() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let kernel = kernel_at(&path);

    let verdict = GoalTerminalState::PartiallyCompleted {
        completed: 1,
        failed: 1,
    };

    let bypass = GoalId::new("forgeflows-bypass");
    open(&kernel, &bypass, GoalStrategy::ForgeFlows);
    let message = refuse_without_owner(&kernel, &bypass, verdict.clone(), "ForgeFlows");
    println!("FORGEFLOWS_REFUSAL={message}");

    let ok = GoalId::new("forgeflows-ok");
    open(&kernel, &ok, GoalStrategy::ForgeFlows);
    let run = WorkflowRunResult {
        final_state: serde_json::Value::Null,
        stage_results: vec![stage(false), stage(true)],
    };
    GoalLoop::new(kernel.clone())
        .run_forgeflows(&ok, |owner| async move {
            StrategyTermination::from_forgeflows(owner, Ok(&run))
        })
        .await
        .expect("forgeflows terminates canonically");
    assert_eq!(terminal_of(&kernel, &ok), verdict);
}

// ── Owner 3 — Fleet ─────────────────────────────────────────────────────────

#[tokio::test]
async fn fleet_terminates_through_its_owner_and_nowhere_else() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let kernel = kernel_at(&path);

    let verdict = GoalTerminalState::PartiallyCompleted {
        completed: 11,
        failed: 3,
    };

    let bypass = GoalId::new("fleet-bypass");
    open(&kernel, &bypass, GoalStrategy::Fleet);
    let message = refuse_without_owner(&kernel, &bypass, verdict.clone(), "Fleet");
    println!("FLEET_REFUSAL={message}");

    let ok = GoalId::new("fleet-ok");
    open(&kernel, &ok, GoalStrategy::Fleet);
    let shards = [ShardSummary {
        shard_id: 0,
        agent_count: 14,
        successes: 11,
        failures: 3,
        payload: serde_json::Value::Null,
    }];
    GoalLoop::new(kernel.clone())
        .run_fleet(&ok, |owner| async move {
            StrategyTermination::from_fleet(owner, FleetOutcome::Dispatched(&shards))
        })
        .await
        .expect("fleet terminates canonically");
    assert_eq!(terminal_of(&kernel, &ok), verdict);
}

// ── Owner 4 — Council ───────────────────────────────────────────────────────

#[tokio::test]
async fn council_terminates_through_its_owner_and_nowhere_else() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let kernel = kernel_at(&path);

    let verdict = GoalTerminalState::Blocked {
        reason: "no proposer answered".to_owned(),
    };

    let bypass = GoalId::new("council-bypass");
    open(&kernel, &bypass, GoalStrategy::Council);
    let message = refuse_without_owner(&kernel, &bypass, verdict.clone(), "Council");
    println!("COUNCIL_REFUSAL={message}");

    let ok = GoalId::new("council-ok");
    open(&kernel, &ok, GoalStrategy::Council);
    GoalLoop::new(kernel.clone())
        .run_council(&ok, |owner| async move {
            StrategyTermination::from_council(
                owner,
                CouncilRunOutcome::DriverFailed {
                    detail: "no proposer answered".to_owned(),
                },
            )
        })
        .await
        .expect("council terminates canonically");
    assert_eq!(terminal_of(&kernel, &ok), verdict);
}

// ── Owner 5 — Anvil ─────────────────────────────────────────────────────────

#[tokio::test]
async fn anvil_terminates_through_its_owner_and_nowhere_else() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let kernel = kernel_at(&path);

    let bypass = GoalId::new("anvil-bypass");
    open(&kernel, &bypass, GoalStrategy::Anvil);
    let message = refuse_without_owner(&kernel, &bypass, GoalTerminalState::SelfChecked, "Anvil");
    println!("ANVIL_REFUSAL={message}");

    let ok = GoalId::new("anvil-ok");
    open(&kernel, &ok, GoalStrategy::Anvil);
    let outcome = climb(TerminalState::SelfChecked);
    GoalLoop::new(kernel.clone())
        .run_anvil(&ok, |owner| async move {
            StrategyTermination::from_anvil(owner, AnvilOutcome::Climbed(&outcome), 3)
        })
        .await
        .expect("anvil terminates canonically");
    assert_eq!(terminal_of(&kernel, &ok), GoalTerminalState::SelfChecked);
}

// ── The set, closed ─────────────────────────────────────────────────────────

/// The five cases above are one per `GoalStrategy`, and this fails to compile
/// if a sixth is added without a case.
///
/// A per-engine suite that silently covered four of five would be the
/// "advertised-but-dead" shape one level up, so the completeness of the SET is
/// asserted rather than eyeballed.
#[test]
fn every_strategy_has_a_no_bypass_case() {
    for strategy in GoalStrategy::ALL {
        let covered = match strategy {
            GoalStrategy::Direct => "direct_terminates_through_its_owner_and_nowhere_else",
            GoalStrategy::ForgeFlows => "forgeflows_terminates_through_its_owner_and_nowhere_else",
            GoalStrategy::Fleet => "fleet_terminates_through_its_owner_and_nowhere_else",
            GoalStrategy::Council => "council_terminates_through_its_owner_and_nowhere_else",
            GoalStrategy::Anvil => "anvil_terminates_through_its_owner_and_nowhere_else",
        };
        assert!(!covered.is_empty(), "{strategy:?} has no no-bypass case");
    }
}

/// Every engine-produced category is refused on the plain path — not just the
/// five this file happens to sample.
///
/// The per-owner cases each pick ONE verdict. A rule keyed on a hand-written
/// list of variants could pass all five and still let a sixth category through,
/// so the split itself is swept here.
#[test]
fn no_engine_produced_category_is_reachable_without_an_owner() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let kernel = kernel_at(&path);

    let engine_verdicts = [
        GoalTerminalState::CriteriaChecked,
        GoalTerminalState::SelfChecked,
        GoalTerminalState::PartiallyCompleted {
            completed: 1,
            failed: 0,
        },
        GoalTerminalState::Exhausted {
            kind: wcore_types::goal::ExhaustionKind::Quality,
            attempts: 2,
            detail: "never validated".to_owned(),
        },
        GoalTerminalState::NeedsEscalation,
        GoalTerminalState::Unpriced {
            detail: "roster not priced".to_owned(),
        },
        GoalTerminalState::Blocked {
            reason: "gate cannot execute".to_owned(),
        },
        GoalTerminalState::TimedOut,
    ];
    for (index, verdict) in engine_verdicts.iter().enumerate() {
        let id = GoalId::new(format!("sweep-{index}"));
        open(&kernel, &id, GoalStrategy::Fleet);
        refuse_without_owner(&kernel, &id, verdict.clone(), "sweep");
    }

    // The other side of the split still works, or the refusal is over-broad and
    // has broken the operator's ability to cancel.
    let control = GoalId::new("control-cancel");
    open(&kernel, &control, GoalStrategy::Fleet);
    kernel
        .terminate(&control, GoalTerminalState::Cancelled)
        .expect("an operator cancel is not an engine verdict and must still work");
    assert_eq!(terminal_of(&kernel, &control), GoalTerminalState::Cancelled);
}

// ── Fixtures ────────────────────────────────────────────────────────────────

fn climb(terminal: TerminalState) -> ClimbOutcome {
    ClimbOutcome {
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
