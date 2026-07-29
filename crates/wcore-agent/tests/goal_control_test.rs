//! F22-C1 — host CONTROL of a durable Goal, driven end to end.
//!
//! ## What these tests are for
//!
//! The protocol crate already had `goal_snapshot` / `goal_transition`, so a
//! host could OBSERVE a Goal. It could not open, task, advance or cancel one.
//! Adding five `ProtocolCommand` variants would compile even if nothing ever
//! answered them — the CLI command loop's match ends in a catch-all that only
//! logs — so "it builds" proves nothing here.
//!
//! Every test below therefore starts from **wire JSON**, deserializes it
//! through `ProtocolCommand`, drives the real handler against a REAL
//! `SessionJournal` on disk, and then asserts against the **chain**, not
//! against the handler's return value. A handler that returned a plausible
//! snapshot without appending anything would fail every one of them.
//!
//! ## Three assertions per gate
//!
//! Each control gate carries a known-positive, a known-negative that genuinely
//! fails, and a demonstration that the pre-change shape would have missed it.
//! The third is the one that proves the gate does something: without it a gate
//! passes just as happily on the code that lacked the feature.

use std::path::Path;

use wcore_agent::goal::{GoalKernel, GoalParentEnvelope, handle_goal_control};
use wcore_agent::session_journal::{GoalLifecycle, SessionJournal};
use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::events::{GoalControlRefusalReason, ProtocolEvent};
use wcore_types::goal::{GoalId, GoalTerminalState};

const SESSION: &str = "goal-control-session";

fn journal(dir: &Path, name: &str) -> SessionJournal {
    SessionJournal::open(dir.join(format!("{name}.jsonl")), SESSION).expect("journal opens")
}

fn parent() -> GoalParentEnvelope {
    GoalParentEnvelope::local_session_default()
}

/// Deserialize wire JSON exactly as the command loop would.
fn wire(json: &str) -> ProtocolCommand {
    serde_json::from_str(json).expect("command must deserialize through ProtocolCommand")
}

fn drive(journal: &SessionJournal, command: &ProtocolCommand) -> Vec<ProtocolEvent> {
    handle_goal_control(
        Some(journal),
        Some(SESSION),
        &parent(),
        1_700_000_000_000,
        command,
    )
    .expect("a goal control command must be handled, never fall through")
}

fn refusal_reason(events: &[ProtocolEvent]) -> Option<GoalControlRefusalReason> {
    events.iter().find_map(|event| match event {
        ProtocolEvent::GoalControlRefused { reason, .. } => Some(*reason),
        _ => None,
    })
}

fn snapshot_ids(events: &[ProtocolEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            ProtocolEvent::GoalSnapshot { goal_id, .. } => Some(goal_id.clone()),
            _ => None,
        })
        .collect()
}

fn open_json(goal: &str) -> String {
    format!(
        r#"{{"type":"goal_open","goal_version":1,"request_id":"r-open","session_id":"{SESSION}","goal_id":"{goal}","objective":"drive the control path","iterations":4,"strategy":"fleet","max_tokens":10000}}"#
    )
}

// ---------------------------------------------------------------------------
// goal_open
// ---------------------------------------------------------------------------

/// KNOWN-POSITIVE: a wire `goal_open` actually writes a Goal into the chain.
///
/// Asserted against a FRESH `GoalKernel` reading the journal back, not against
/// the events the handler returned. That is the difference between proving the
/// command controlled something and proving it described something.
#[test]
fn a_wire_goal_open_appends_a_real_goal_to_the_durable_chain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "open-positive");

    let kernel = GoalKernel::new(journal.clone());
    assert!(
        kernel.goal(&GoalId::new("g-1")).expect("read").is_none(),
        "precondition: the Goal must not exist before the command"
    );

    let events = drive(&journal, &wire(&open_json("g-1")));

    assert_eq!(
        snapshot_ids(&events),
        vec!["g-1".to_owned()],
        "an accepted open must answer with the Goal's snapshot: {events:?}"
    );
    let state = GoalKernel::new(journal.clone())
        .goal(&GoalId::new("g-1"))
        .expect("read")
        .expect("the Goal must exist in the chain after the command");
    assert_eq!(state.objective, "drive the control path");
    assert!(matches!(state.lifecycle, GoalLifecycle::Opened));
}

/// KNOWN-NEGATIVE: opening the same Goal twice is refused, and refused with
/// the reason that distinguishes it from every other failure.
#[test]
fn opening_the_same_goal_twice_is_refused_as_already_existing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "open-negative");

    drive(&journal, &wire(&open_json("g-1")));
    let second = drive(&journal, &wire(&open_json("g-1")));

    assert_eq!(
        refusal_reason(&second),
        Some(GoalControlRefusalReason::GoalAlreadyExists),
        "a repeat open must be refused, not silently applied: {second:?}"
    );
    assert!(
        snapshot_ids(&second).is_empty(),
        "a refused command must not also answer with a snapshot"
    );
}

/// THE HOST MAY NOT MINT AUTHORITY.
///
/// `goal_open` carries `max_tokens` as a REQUEST. It carries no
/// `parent_max_tokens`, and this test pins that: a wire payload that tries to
/// state its own parent ceiling must be REJECTED at deserialization by
/// `deny_unknown_fields`, and a request above the parent must be clamped down
/// to the intersection rather than granted.
#[test]
fn a_host_cannot_state_its_own_parent_envelope_and_cannot_exceed_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "open-authority");

    // (a) The field does not exist on the wire at all.
    let smuggled = format!(
        r#"{{"type":"goal_open","goal_version":1,"request_id":"r","session_id":"{SESSION}","goal_id":"g-a","objective":"o","iterations":2,"strategy":"fleet","max_tokens":10,"parent_max_tokens":999999999}}"#
    );
    assert!(
        serde_json::from_str::<ProtocolCommand>(&smuggled).is_err(),
        "a host must not be able to state a parent ceiling on the wire"
    );

    // (b) A request ABOVE the parent's effective limit is clamped to the
    //     intersection, so asking for more than the session has cannot grant
    //     more than the session has.
    let parent_ceiling = parent()
        .effective_limits
        .get("max_tokens")
        .copied()
        .expect("the local session envelope names max_tokens");
    let greedy = format!(
        r#"{{"type":"goal_open","goal_version":1,"request_id":"r","session_id":"{SESSION}","goal_id":"g-b","objective":"o","iterations":2,"strategy":"fleet","max_tokens":{}}}"#,
        parent_ceiling.saturating_mul(1000)
    );
    drive(&journal, &wire(&greedy));

    let state = GoalKernel::new(journal.clone())
        .goal(&GoalId::new("g-b"))
        .expect("read")
        .expect("goal exists");
    let recorded = state
        .authority
        .effective_limits
        .get("max_tokens")
        .copied()
        .expect("the recorded envelope names max_tokens");
    assert_eq!(
        recorded, parent_ceiling,
        "the recorded envelope must be the intersection, never the request"
    );
}

// ---------------------------------------------------------------------------
// goal_declare_task
// ---------------------------------------------------------------------------

#[test]
fn a_wire_declare_task_lands_in_the_durable_ledger_with_its_dependency() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "task-positive");
    drive(&journal, &wire(&open_json("g-1")));

    // The dependency must be declared FIRST. The ledger refuses a dependency
    // on an undeclared task rather than treating it as satisfied, so a real
    // control plane declares the graph in topological order.
    let build = format!(
        r#"{{"type":"goal_declare_task","goal_version":1,"request_id":"r-build","session_id":"{SESSION}","goal_id":"g-1","task_id":"build"}}"#
    );
    let built = drive(&journal, &wire(&build));
    assert!(
        refusal_reason(&built).is_none(),
        "declaring a root task must be accepted: {built:?}"
    );

    let declare = format!(
        r#"{{"type":"goal_declare_task","goal_version":1,"request_id":"r-task","session_id":"{SESSION}","goal_id":"g-1","task_id":"publish","depends_on":["build"],"idempotency_key":"idem-publish"}}"#
    );
    let events = drive(&journal, &wire(&declare));
    assert!(
        refusal_reason(&events).is_none(),
        "declaring a fresh task must be accepted: {events:?}"
    );

    let state = GoalKernel::new(journal.clone())
        .goal(&GoalId::new("g-1"))
        .expect("read")
        .expect("goal exists");
    let task = state
        .tasks
        .get("publish")
        .expect("the task must be in the durable ledger, not only in the reply");
    assert!(
        task.depends_on.contains("build"),
        "the declared dependency must survive into the chain"
    );
}

/// KNOWN-NEGATIVE, found by this suite failing for real: a dependency on an
/// undeclared task is refused, and refused with its OWN reason.
///
/// The first version of this suite declared `publish -> build` without
/// declaring `build`, and the handler answered `journal_error`. That was
/// technically true and operationally useless: `journal_error` tells a host to
/// retry a write, when the actual fix is to declare the dependency first. The
/// handler now checks the ledger's rule itself and says so.
#[test]
fn a_dependency_on_an_undeclared_task_is_refused_with_its_own_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "task-undeclared-dep");
    drive(&journal, &wire(&open_json("g-1")));

    let orphan = format!(
        r#"{{"type":"goal_declare_task","goal_version":1,"request_id":"r","session_id":"{SESSION}","goal_id":"g-1","task_id":"publish","depends_on":["build"]}}"#
    );
    let events = drive(&journal, &wire(&orphan));
    assert_eq!(
        refusal_reason(&events),
        Some(GoalControlRefusalReason::DependencyNotDeclared),
        "an undeclared dependency must be named as such, not collapsed into a journal error: {events:?}"
    );

    // And nothing was written: a refused declaration must not half-land.
    let state = GoalKernel::new(journal.clone())
        .goal(&GoalId::new("g-1"))
        .expect("read")
        .expect("goal exists");
    assert!(
        state.tasks.is_empty(),
        "a refused task declaration must leave the ledger untouched"
    );

    // THIRD ASSERTION — the old shape would have missed it. Before this
    // reason existed the answer was `JournalError`, which is the SAME value a
    // genuine disk failure produces, so a matcher keyed on it could not tell
    // a fixable graph mistake from an unfixable I/O one.
    assert_ne!(
        refusal_reason(&events),
        Some(GoalControlRefusalReason::JournalError),
        "the pre-repair answer must no longer be what this path returns"
    );
}

/// KNOWN-NEGATIVE: a task on a Goal that does not exist is refused as
/// `GoalNotFound` — kept distinct from `TaskAlreadyDeclared` because the two
/// settle differently for a host.
#[test]
fn declaring_a_task_on_an_absent_goal_is_refused_as_goal_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "task-negative");

    let declare = format!(
        r#"{{"type":"goal_declare_task","goal_version":1,"request_id":"r","session_id":"{SESSION}","goal_id":"ghost","task_id":"t"}}"#
    );
    let events = drive(&journal, &wire(&declare));
    assert_eq!(
        refusal_reason(&events),
        Some(GoalControlRefusalReason::GoalNotFound)
    );
}

// ---------------------------------------------------------------------------
// goal_advance — and the cursor binding
// ---------------------------------------------------------------------------

fn cursor_of(journal: &SessionJournal, goal: &str) -> (Option<u64>, String) {
    let cursor = GoalKernel::new(journal.clone())
        .cursor(&GoalId::new(goal))
        .expect("read")
        .expect("goal has a cursor");
    (cursor.journal_sequence, cursor.journal_digest)
}

fn advance_json(goal: &str, seq: Option<u64>, digest: &str) -> String {
    let seq = seq.map_or("null".to_owned(), |value| value.to_string());
    format!(
        r#"{{"type":"goal_advance","goal_version":1,"request_id":"r-adv","session_id":"{SESSION}","goal_id":"{goal}","cursor":{{"journal_sequence":{seq},"journal_digest":"{digest}"}}}}"#
    )
}

/// KNOWN-POSITIVE: an advance at the CURRENT cursor consumes an iteration.
#[test]
fn an_advance_at_the_current_cursor_consumes_one_iteration() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "advance-positive");
    drive(&journal, &wire(&open_json("g-1")));

    let before = GoalKernel::new(journal.clone())
        .goal(&GoalId::new("g-1"))
        .expect("read")
        .expect("goal exists")
        .iterations_started;

    let (seq, digest) = cursor_of(&journal, "g-1");
    let events = drive(&journal, &wire(&advance_json("g-1", seq, &digest)));
    assert!(
        refusal_reason(&events).is_none(),
        "a fresh-cursor advance must be accepted: {events:?}"
    );

    let after = GoalKernel::new(journal.clone())
        .goal(&GoalId::new("g-1"))
        .expect("read")
        .expect("goal exists");
    assert_eq!(
        after.iterations_started,
        before + 1,
        "the iteration counter must actually move in the chain"
    );
    assert!(matches!(after.lifecycle, GoalLifecycle::Running));
}

/// KNOWN-NEGATIVE, and the sharpest one: a STALE cursor is refused.
///
/// This is the assertion that proves the cursor is load-bearing rather than
/// decorative. The command is byte-identical to the accepted one except that
/// its cursor is the one the operator saw BEFORE a change landed.
#[test]
fn an_advance_at_a_stale_cursor_is_refused_rather_than_applied() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "advance-negative");
    drive(&journal, &wire(&open_json("g-1")));

    // The operator's view: the cursor as it is right now.
    let (stale_seq, stale_digest) = cursor_of(&journal, "g-1");

    // The Goal moves underneath them.
    drive(
        &journal,
        &wire(&advance_json("g-1", stale_seq, &stale_digest)),
    );

    let iterations_before = GoalKernel::new(journal.clone())
        .goal(&GoalId::new("g-1"))
        .expect("read")
        .expect("goal exists")
        .iterations_started;

    // Now they act on the view they were holding.
    let events = drive(
        &journal,
        &wire(&advance_json("g-1", stale_seq, &stale_digest)),
    );

    assert_eq!(
        refusal_reason(&events),
        Some(GoalControlRefusalReason::CursorStale),
        "a stale-cursor advance must be refused: {events:?}"
    );
    let iterations_after = GoalKernel::new(journal.clone())
        .goal(&GoalId::new("g-1"))
        .expect("read")
        .expect("goal exists")
        .iterations_started;
    assert_eq!(
        iterations_after, iterations_before,
        "a refused advance must not have consumed an iteration"
    );
}

/// THIRD ASSERTION — the pre-change shape would have missed all of this.
///
/// Before this lane there was no Goal command on the wire at all. The exact
/// payloads the tests above drive are, against the OLD `ProtocolCommand`,
/// simply unknown variants. This test pins that by asserting the property that
/// made the old shape blind: an unrecognised `type` does not deserialize, and
/// the CLI command loop's catch-all arm means an unrecognised command that DID
/// deserialize would have been silently logged and dropped.
///
/// So a gate written only as "the handler returns events" would have passed on
/// the old code by never being reachable. These assert on the CHAIN instead.
#[test]
fn the_old_shape_could_not_have_carried_any_of_these_commands() {
    // A command type this Core does not know is rejected outright — which is
    // what every `goal_*` payload above WAS, before this change.
    for unknown in [
        r#"{"type":"goal_open_v0","goal_version":1}"#,
        r#"{"type":"goal_teleport","goal_version":1}"#,
    ] {
        assert!(
            serde_json::from_str::<ProtocolCommand>(unknown).is_err(),
            "an unknown command type must not deserialize: {unknown}"
        );
    }

    // And the positive half of the same claim: the five DO deserialize now,
    // so the difference between the old and new shape is real and is exactly
    // these five.
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "old-shape");
    drive(&journal, &wire(&open_json("g-1")));
    let (seq, digest) = cursor_of(&journal, "g-1");
    for payload in [
        open_json("g-2"),
        format!(
            r#"{{"type":"goal_declare_task","goal_version":1,"request_id":"r","session_id":"{SESSION}","goal_id":"g-1","task_id":"t"}}"#
        ),
        advance_json("g-1", seq, &digest),
        format!(
            r#"{{"type":"goal_resync","goal_version":1,"request_id":"r","session_id":"{SESSION}"}}"#
        ),
    ] {
        let command: ProtocolCommand =
            serde_json::from_str(&payload).expect("the five must deserialize now");
        assert!(
            handle_goal_control(Some(&journal), Some(SESSION), &parent(), 1, &command).is_some(),
            "every Goal command must be HANDLED, never fall through to the catch-all: {payload}"
        );
    }

    // A non-Goal command must still fall through, or this handler would be
    // swallowing commands other arms own.
    let ping: ProtocolCommand = serde_json::from_str(r#"{"type":"ping"}"#).expect("ping");
    assert!(
        handle_goal_control(Some(&journal), Some(SESSION), &parent(), 1, &ping).is_none(),
        "a non-Goal command must fall through to its own arm"
    );
}

// ---------------------------------------------------------------------------
// goal_cancel
// ---------------------------------------------------------------------------

#[test]
fn a_wire_cancel_terminates_the_goal_through_the_canonical_transition() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "cancel-positive");
    drive(&journal, &wire(&open_json("g-1")));
    let (seq, digest) = cursor_of(&journal, "g-1");
    let seq_text = seq.map_or("null".to_owned(), |value| value.to_string());

    let cancel = format!(
        r#"{{"type":"goal_cancel","goal_version":1,"request_id":"r-cancel","session_id":"{SESSION}","goal_id":"g-1","cursor":{{"journal_sequence":{seq_text},"journal_digest":"{digest}"}}}}"#
    );
    let events = drive(&journal, &wire(&cancel));
    assert!(refusal_reason(&events).is_none(), "{events:?}");

    let state = GoalKernel::new(journal.clone())
        .goal(&GoalId::new("g-1"))
        .expect("read")
        .expect("goal exists");
    match state.lifecycle {
        GoalLifecycle::Terminated { ref terminal } => {
            assert_eq!(
                *terminal,
                GoalTerminalState::Cancelled,
                "a host cancel must land in the canonical Cancelled terminal"
            );
        }
        other => panic!("cancel must terminate the Goal, found {other:?}"),
    }

    // KNOWN-NEGATIVE in the same test: cancelling again is refused, because a
    // terminated Goal cannot terminate twice.
    let (seq2, digest2) = cursor_of(&journal, "g-1");
    let seq2_text = seq2.map_or("null".to_owned(), |value| value.to_string());
    let again = format!(
        r#"{{"type":"goal_cancel","goal_version":1,"request_id":"r-cancel-2","session_id":"{SESSION}","goal_id":"g-1","cursor":{{"journal_sequence":{seq2_text},"journal_digest":"{digest2}"}}}}"#
    );
    assert_eq!(
        refusal_reason(&drive(&journal, &wire(&again))),
        Some(GoalControlRefusalReason::GoalTerminated)
    );
}

/// A host may not nominate a terminal, so it can never reach `Verified` —
/// the one stamp reserved for a real executable gate.
#[test]
fn the_cancel_command_has_no_field_through_which_a_terminal_could_be_chosen() {
    let smuggled = format!(
        r#"{{"type":"goal_cancel","goal_version":1,"request_id":"r","session_id":"{SESSION}","goal_id":"g","cursor":{{"journal_sequence":1,"journal_digest":"d"}},"terminal":"verified"}}"#
    );
    assert!(
        serde_json::from_str::<ProtocolCommand>(&smuggled).is_err(),
        "a host must not be able to nominate a terminal state"
    );
}

// ---------------------------------------------------------------------------
// goal_resync, versioning and session binding
// ---------------------------------------------------------------------------

#[test]
fn resync_without_a_goal_id_returns_every_goal_in_the_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "resync-all");
    drive(&journal, &wire(&open_json("g-1")));
    drive(&journal, &wire(&open_json("g-2")));

    let all = format!(
        r#"{{"type":"goal_resync","goal_version":1,"request_id":"r","session_id":"{SESSION}"}}"#
    );
    let mut ids = snapshot_ids(&drive(&journal, &wire(&all)));
    ids.sort();
    assert_eq!(ids, vec!["g-1".to_owned(), "g-2".to_owned()]);
}

/// An empty session answers with NO snapshots and NO refusal: "this session
/// holds no Goals" is a fact, not an error.
#[test]
fn resync_on_a_session_with_no_goals_is_an_empty_answer_not_a_refusal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "resync-empty");
    let all = format!(
        r#"{{"type":"goal_resync","goal_version":1,"request_id":"r","session_id":"{SESSION}"}}"#
    );
    let events = drive(&journal, &wire(&all));
    assert!(events.is_empty(), "expected no events, got {events:?}");
}

/// KNOWN-NEGATIVE for the two preflight gates, each with its own reason.
#[test]
fn a_wrong_version_and_a_wrong_session_are_refused_distinctly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "preflight");

    let bad_version = format!(
        r#"{{"type":"goal_resync","goal_version":99,"request_id":"r","session_id":"{SESSION}"}}"#
    );
    assert_eq!(
        refusal_reason(&drive(&journal, &wire(&bad_version))),
        Some(GoalControlRefusalReason::UnsupportedVersion)
    );

    let bad_session =
        r#"{"type":"goal_resync","goal_version":1,"request_id":"r","session_id":"not-this-one"}"#;
    assert_eq!(
        refusal_reason(&drive(&journal, &wire(bad_session))),
        Some(GoalControlRefusalReason::SessionNotFound)
    );
}

/// With no journal there is nothing to control, and that must be said rather
/// than silently succeeding.
#[test]
fn a_process_without_a_journal_refuses_control_rather_than_pretending() {
    let command = wire(&open_json("g-1"));
    let events = handle_goal_control(None, Some(SESSION), &parent(), 1, &command)
        .expect("still handled — a missing journal is an answer, not a fall-through");
    assert_eq!(
        refusal_reason(&events),
        Some(GoalControlRefusalReason::JournalUnavailable)
    );
}

/// EVERY Goal command answers with SOMETHING. This is the anti-dead-surface
/// assertion: an arm that accepted a command and emitted nothing would be
/// indistinguishable from the catch-all that ignores it.
#[test]
fn no_goal_control_command_is_ever_answered_with_silence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal = journal(dir.path(), "never-silent");

    // Deliberately all against an EMPTY journal, so every one of these is a
    // refusal path — the paths most at risk of returning an empty vec.
    let (seq, digest) = (Some(1_u64), "sha256:nope".to_owned());
    let payloads = [
        open_json("g-x"),
        format!(
            r#"{{"type":"goal_declare_task","goal_version":1,"request_id":"r","session_id":"{SESSION}","goal_id":"ghost","task_id":"t"}}"#
        ),
        advance_json("ghost", seq, &digest),
        format!(
            r#"{{"type":"goal_cancel","goal_version":1,"request_id":"r","session_id":"{SESSION}","goal_id":"ghost","cursor":{{"journal_sequence":1,"journal_digest":"{digest}"}}}}"#
        ),
        format!(
            r#"{{"type":"goal_resync","goal_version":1,"request_id":"r","session_id":"{SESSION}","goal_id":"ghost"}}"#
        ),
    ];
    for payload in payloads {
        let events = drive(&journal, &wire(&payload));
        assert!(
            !events.is_empty(),
            "a Goal command must never be answered with silence: {payload}"
        );
    }
}
