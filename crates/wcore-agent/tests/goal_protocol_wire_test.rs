//! F22-C1 — a durable Goal driven through the REAL kernel becomes observable
//! over the host protocol.
//!
//! `goal/wire.rs`'s own unit tests build a `GoalState` by hand. That proves the
//! projection maps fields correctly and proves nothing about whether the kernel
//! actually produces a chain the projection can read. These tests never
//! construct a `GoalState`: every one drives `GoalKernel`, writes real journal
//! frames, replays them, and asserts on the wire the replay produced.
//!
//! Every assertion here is a COUNT or an ordering, not the presence of a
//! substring, because "goals appear on the protocol" is a claim about how many
//! and in what order — and a projection that emitted one event for a Goal with
//! six transitions would pass any presence check.

use std::collections::BTreeMap;
use std::path::Path;

use wcore_agent::goal::{
    GoalKernel, event_line, goal_projection, goal_snapshot_event, goal_state_digest, goal_stream,
};
use wcore_agent::session_journal::SessionJournal;
use wcore_protocol::goal::{GoalLifecycleWire, GoalTransitionKind};
use wcore_types::goal::{
    GoalAuthorityRequest, GoalAuthoritySnapshot, GoalId, GoalStrategy, GoalTerminalState,
    LoopPolicy, WaitKind, resolve_goal_authority,
};

const SESSION: &str = "goal-wire-session";

fn limits(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

fn snapshot(strategy: GoalStrategy, loop_policy: LoopPolicy) -> GoalAuthoritySnapshot {
    resolve_goal_authority(
        &GoalAuthorityRequest {
            requested_limits: limits(&[("max_tokens", 500)]),
            strategy,
            loop_policy,
        },
        &limits(&[("max_tokens", 1000), ("max_cost_cents", 25)]),
        "parent-envelope-digest",
    )
}

fn open_kernel(path: &Path) -> GoalKernel {
    GoalKernel::new(SessionJournal::open(path, SESSION).expect("journal opens"))
}

/// The wire types of a stream, in order, as `(type, transition-or-"snapshot")`.
fn shape(events: &[wcore_protocol::events::ProtocolEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .map(|event| {
            let value: serde_json::Value =
                serde_json::from_str(&event_line(event).expect("serializes")).expect("valid JSON");
            let kind = value
                .get("transition")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("snapshot")
                .to_owned();
            (
                value["type"].as_str().expect("typed event").to_owned(),
                kind,
            )
        })
        .collect()
}

/// THE headline claim, measured rather than asserted.
///
/// Six kernel transitions in, seven wire events out, in the order the chain
/// holds them, terminating in a snapshot. A projection that dropped, reordered
/// or merged a transition fails on the vector, not on a boolean.
#[test]
fn six_kernel_transitions_produce_six_ordered_wire_transitions_and_one_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-stream");

    let kernel = open_kernel(&path);
    kernel
        .open_goal(
            &id,
            "make goals observable",
            &snapshot(GoalStrategy::Anvil, LoopPolicy::Fixed { iterations: 4 }),
            1_721_000_000_000,
        )
        .expect("opens");
    kernel.start_iteration(&id).expect("iteration 1");
    kernel
        .begin_wait(
            &id,
            WaitKind::Approval {
                approval_id: "approval-1".to_owned(),
            },
        )
        .expect("waits");
    kernel.resume_from_wait(&id).expect("wait resolves");
    kernel.start_iteration(&id).expect("iteration 2");
    kernel
        .terminate(&id, GoalTerminalState::Cancelled)
        .expect("terminates");

    let envelopes = SessionJournal::replay(&path).expect("replays");
    let events = goal_stream(SESSION, id.as_str(), &envelopes).expect("projects");

    assert_eq!(
        shape(&events),
        vec![
            ("goal_transition".to_owned(), "opened".to_owned()),
            ("goal_transition".to_owned(), "iteration_started".to_owned()),
            ("goal_transition".to_owned(), "wait_begun".to_owned()),
            ("goal_transition".to_owned(), "wait_resolved".to_owned()),
            ("goal_transition".to_owned(), "iteration_started".to_owned()),
            ("goal_transition".to_owned(), "terminated".to_owned()),
            ("goal_snapshot".to_owned(), "snapshot".to_owned()),
        ],
        "the wire stream must mirror the chain exactly"
    );
}

/// The lifecycle reported with each transition must be the lifecycle the
/// REDUCER holds after it — not one derived from the transition's name.
///
/// `wait_begun` is the discriminating case: its lifecycle carries the wait
/// itself, so a projection that guessed from the kind alone could not produce
/// the approval id and would be caught here.
#[test]
fn each_transition_reports_the_lifecycle_the_reducer_holds_after_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-lifecycle");

    let kernel = open_kernel(&path);
    kernel
        .open_goal(
            &id,
            "prove the fold",
            &snapshot(GoalStrategy::Direct, LoopPolicy::Fixed { iterations: 2 }),
            1_721_000_000_000,
        )
        .expect("opens");
    kernel.start_iteration(&id).expect("iteration");
    kernel
        .begin_wait(
            &id,
            WaitKind::Child {
                child_id: "child-7".to_owned(),
            },
        )
        .expect("waits");

    let envelopes = SessionJournal::replay(&path).expect("replays");
    let events = goal_stream(SESSION, id.as_str(), &envelopes).expect("projects");

    let lifecycles = events
        .iter()
        .filter_map(|event| match event {
            wcore_protocol::events::ProtocolEvent::GoalTransition { lifecycle, .. } => {
                Some(lifecycle.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(lifecycles.len(), 3, "three transitions expected");
    assert_eq!(lifecycles[0], GoalLifecycleWire::Opened);
    assert_eq!(lifecycles[1], GoalLifecycleWire::Running);
    assert_eq!(
        lifecycles[2],
        GoalLifecycleWire::Waiting {
            wait: WaitKind::Child {
                child_id: "child-7".to_owned()
            }
        },
        "the wait itself must survive onto the wire, not just the fact of waiting"
    );
}

/// A Goal that has not terminated must not project a terminal lifecycle, and a
/// terminated one must carry the exact terminal category.
///
/// This is the claim a control plane renders a badge from; a wire that rounded
/// `PartiallyCompleted` to a failure, or lost its counts, would be actively
/// misleading. 22-C3 recorded that exact taxonomy complaint.
#[test]
fn a_partial_completion_reaches_the_wire_with_its_counts_intact() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-partial");

    let kernel = open_kernel(&path);
    kernel
        .open_goal(
            &id,
            "count honestly",
            &snapshot(GoalStrategy::Fleet, LoopPolicy::Once),
            1_721_000_000_000,
        )
        .expect("opens");
    kernel
        .terminate(
            &id,
            GoalTerminalState::PartiallyCompleted {
                completed: 11,
                failed: 3,
            },
        )
        .expect("terminates");

    let state = kernel.goal(&id).expect("reads").expect("exists");
    let projection = goal_projection(&state);
    assert_eq!(
        projection.lifecycle,
        GoalLifecycleWire::Terminated {
            terminal: GoalTerminalState::PartiallyCompleted {
                completed: 11,
                failed: 3
            }
        }
    );

    // And it must survive serialization, not merely the in-memory compare.
    let event = goal_snapshot_event(SESSION, &state).expect("snapshot builds");
    let value: serde_json::Value =
        serde_json::from_str(&event_line(&event).expect("serializes")).expect("valid JSON");
    assert_eq!(value["goal"]["lifecycle"]["terminal"]["completed"], 11);
    assert_eq!(value["goal"]["lifecycle"]["terminal"]["failed"], 3);
}

/// The snapshot's cursor must be the Goal's own cursor from the chain, so a
/// host reconnecting at that cursor resumes where the projection said it was.
#[test]
fn the_snapshot_cursor_is_the_goals_committed_journal_position() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-cursor");

    let kernel = open_kernel(&path);
    kernel
        .open_goal(
            &id,
            "bind the cursor",
            &snapshot(GoalStrategy::Council, LoopPolicy::Fixed { iterations: 3 }),
            1_721_000_000_000,
        )
        .expect("opens");
    let cursor = kernel.start_iteration(&id).expect("iteration");

    let state = kernel.goal(&id).expect("reads").expect("exists");
    assert_eq!(
        goal_projection(&state).cursor,
        cursor,
        "the projection's cursor must be the one the kernel just committed"
    );
    assert!(
        cursor.journal_sequence.is_some(),
        "a committed cursor must carry a sequence"
    );
    assert!(
        !cursor.journal_digest.is_empty(),
        "a committed cursor must carry a digest"
    );
}

/// Two Goals in ONE journal must not bleed into each other's stream.
///
/// The projection filters by goal id; a filter that matched loosely would
/// produce a stream describing a Goal the caller did not ask about, which is
/// worse than emitting nothing.
#[test]
fn one_journal_holding_two_goals_projects_each_one_separately() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let first = GoalId::new("g-alpha");
    let second = GoalId::new("g-alpha-extended");

    let kernel = open_kernel(&path);
    for id in [&first, &second] {
        kernel
            .open_goal(
                id,
                "two goals, one chain",
                &snapshot(GoalStrategy::Fleet, LoopPolicy::Fixed { iterations: 2 }),
                1_721_000_000_000,
            )
            .expect("opens");
    }
    kernel.start_iteration(&second).expect("iteration");

    let envelopes = SessionJournal::replay(&path).expect("replays");

    let alpha = goal_stream(SESSION, first.as_str(), &envelopes).expect("projects");
    let extended = goal_stream(SESSION, second.as_str(), &envelopes).expect("projects");

    // `g-alpha` is a strict prefix of `g-alpha-extended`. A prefix or
    // contains-style match would give the shorter id the longer Goal's events.
    assert_eq!(alpha.len(), 2, "opened + snapshot");
    assert_eq!(extended.len(), 3, "opened + iteration + snapshot");
    for event in &alpha {
        let value: serde_json::Value =
            serde_json::from_str(&event_line(event).expect("serializes")).expect("valid JSON");
        assert_eq!(
            value["goal_id"], "g-alpha",
            "g-alpha's stream leaked another Goal's event"
        );
    }
}

/// Asking for a Goal that is not in the chain must yield nothing, not an empty
/// shell that a host would render as a real Goal.
#[test]
fn an_unknown_goal_projects_no_events_rather_than_an_empty_goal() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-present");

    let kernel = open_kernel(&path);
    kernel
        .open_goal(
            &id,
            "present",
            &snapshot(GoalStrategy::Direct, LoopPolicy::Once),
            1_721_000_000_000,
        )
        .expect("opens");

    let envelopes = SessionJournal::replay(&path).expect("replays");
    assert!(
        goal_stream(SESSION, "g-absent", &envelopes)
            .expect("projects")
            .is_empty()
    );
    assert_eq!(
        goal_stream(SESSION, id.as_str(), &envelopes)
            .expect("projects")
            .len(),
        2,
        "the present Goal must still project, or the negative case proved nothing"
    );
}

/// A crash-resume must reach the wire as its own transition and carry the
/// resume count, because a host that cannot see a resume cannot tell a Goal
/// that ran once from one that ran, died and was picked up again.
#[test]
fn a_recovered_goal_projects_its_resume_as_a_transition_and_a_count() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-resume");

    {
        let kernel = open_kernel(&path);
        kernel
            .open_goal(
                &id,
                "survive a restart",
                &snapshot(GoalStrategy::Fleet, LoopPolicy::Fixed { iterations: 3 }),
                1_721_000_000_000,
            )
            .expect("opens");
        kernel.start_iteration(&id).expect("iteration");
    }

    // A genuinely fresh process view: a new journal handle over the same file.
    let kernel = open_kernel(&path);
    kernel
        .recover_with_parent_envelope(&id, "parent-envelope-digest")
        .expect("recovers");

    let envelopes = SessionJournal::replay(&path).expect("replays");
    let events = goal_stream(SESSION, id.as_str(), &envelopes).expect("projects");

    let kinds = events
        .iter()
        .filter_map(|event| match event {
            wcore_protocol::events::ProtocolEvent::GoalTransition { transition, .. } => {
                Some(*transition)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            GoalTransitionKind::Opened,
            GoalTransitionKind::IterationStarted,
            GoalTransitionKind::RunResumed,
        ]
    );

    let state = kernel.goal(&id).expect("reads").expect("exists");
    assert_eq!(goal_projection(&state).resume_count, 1);
}

/// The digest must be over the chain state, so two different chain states
/// cannot share one — including two that a narrowed view renders identically.
#[test]
fn the_state_digest_moves_when_the_chain_moves() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.journal");
    let id = GoalId::new("g-digest");

    let kernel = open_kernel(&path);
    kernel
        .open_goal(
            &id,
            "bind the state",
            &snapshot(GoalStrategy::Fleet, LoopPolicy::Fixed { iterations: 3 }),
            1_721_000_000_000,
        )
        .expect("opens");
    let opened = goal_state_digest(&kernel.goal(&id).expect("reads").expect("exists"))
        .expect("digest computes");

    kernel.start_iteration(&id).expect("iteration");
    let iterated = goal_state_digest(&kernel.goal(&id).expect("reads").expect("exists"))
        .expect("digest computes");

    assert_ne!(opened, iterated, "the digest must track the chain");
    assert!(
        opened.len() > 16 && iterated.len() > 16,
        "a digest that is empty or trivial would make the inequality meaningless"
    );
}
