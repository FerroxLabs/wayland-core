//! Host CONTROL of a durable Goal (F22-C1).
//!
//! ## Why this lives here and not in the CLI command loop
//!
//! The five Goal commands are answered in `wcore-cli/src/main.rs`, which is a
//! file every lane edits and which the frontier programme fences to additive,
//! contiguous edits. Putting the decision logic there would mean a large block
//! in a contended file, and — worse — logic reachable only by standing up a
//! whole protocol session. Here it is a pure function over
//! `(journal, live session id, command)` returning the events to emit, so the
//! refusal table can be tested directly.
//!
//! ## Returning events instead of emitting them
//!
//! [`handle_goal_control`] returns `Option<Vec<ProtocolEvent>>`. `None` means
//! "not a Goal control command" so the caller falls through to its existing
//! arms; `Some` means handled, and the caller emits what it is given. That
//! keeps the emitter out of the decision path, which is what makes the
//! known-negative assertions in this module's tests real rather than mocked.
//!
//! ## Every refusal is typed and emitted
//!
//! There is no path here that returns `Some(vec![])`. A command that is
//! rejected produces a [`ProtocolEvent::GoalControlRefused`] naming why. This
//! is deliberate: the command loop's match ends in a catch-all that only logs,
//! so a silent refusal would be indistinguishable from an accepted no-op —
//! the advertised-but-dead shape this surface exists to close.

use std::collections::BTreeMap;

use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::events::{GoalControlRefusalReason, ProtocolEvent};
use wcore_protocol::goal::GOAL_PROTOCOL_VERSION;
use wcore_types::goal::{
    GoalAuthorityRequest, GoalId, GoalTerminalState, LoopPolicy, TaskId, resolve_goal_authority,
};

use crate::goal::{GoalKernel, GoalLedger};
use crate::session_journal::{GoalLifecycle, GoalState, SessionJournal};

use super::wire::{goal_snapshot_event, goal_state_digest};

/// The parent authority a host-opened Goal is resolved against.
///
/// Supplied by Core, never by the wire. A `parent_max_tokens` field on the
/// command would let an untrusted peer state its own ceiling, which is the
/// authority-minting route `GoalAuthorityWire` was shaped to refuse on the
/// event side.
#[derive(Debug, Clone)]
pub struct GoalParentEnvelope {
    pub digest: String,
    pub effective_limits: BTreeMap<String, u64>,
}

impl GoalParentEnvelope {
    /// The envelope a local session grants its own Goals, matching the CLI's
    /// `--parent-envelope` / `--parent-max-tokens` defaults so a Goal opened
    /// over the protocol and one opened from the CLI resolve identically.
    #[must_use]
    pub fn local_session_default() -> Self {
        Self {
            digest: "wayland-core-goal-fleet/v1".to_owned(),
            effective_limits: [("max_tokens".to_owned(), 1_000_000_u64)]
                .into_iter()
                .collect(),
        }
    }
}

fn refuse(
    request_id: &str,
    session_id: &str,
    goal_id: &str,
    reason: GoalControlRefusalReason,
) -> Vec<ProtocolEvent> {
    vec![ProtocolEvent::GoalControlRefused {
        goal_version: GOAL_PROTOCOL_VERSION,
        request_id: request_id.to_owned(),
        session_id: session_id.to_owned(),
        goal_id: goal_id.to_owned(),
        reason,
    }]
}

/// Shared preamble: version, session identity, journal presence.
///
/// Returns the kernel on success, or the refusal to emit.
fn preflight(
    journal: Option<&SessionJournal>,
    live_session_id: Option<&str>,
    goal_version: u16,
    request_id: &str,
    session_id: &str,
    goal_id: &str,
) -> Result<GoalKernel, Vec<ProtocolEvent>> {
    if goal_version != GOAL_PROTOCOL_VERSION {
        return Err(refuse(
            request_id,
            session_id,
            goal_id,
            GoalControlRefusalReason::UnsupportedVersion,
        ));
    }
    if live_session_id != Some(session_id) {
        return Err(refuse(
            request_id,
            session_id,
            goal_id,
            GoalControlRefusalReason::SessionNotFound,
        ));
    }
    let Some(journal) = journal else {
        return Err(refuse(
            request_id,
            session_id,
            goal_id,
            GoalControlRefusalReason::JournalUnavailable,
        ));
    };
    Ok(GoalKernel::new(journal.clone()))
}

/// Read a Goal, mapping "absent" and "journal broke" to distinct refusals.
fn load_goal(
    kernel: &GoalKernel,
    goal_id: &GoalId,
    request_id: &str,
    session_id: &str,
) -> Result<GoalState, Vec<ProtocolEvent>> {
    match kernel.goal(goal_id) {
        Ok(Some(state)) => Ok(state),
        Ok(None) => Err(refuse(
            request_id,
            session_id,
            goal_id.as_str(),
            GoalControlRefusalReason::GoalNotFound,
        )),
        Err(_) => Err(refuse(
            request_id,
            session_id,
            goal_id.as_str(),
            GoalControlRefusalReason::JournalError,
        )),
    }
}

/// The snapshot answering an accepted control command.
///
/// A control command is answered with the SAME `goal_snapshot` shape the
/// producer stream emits, rather than a second "control result" shape. One
/// authoritative Goal content shape was the whole point of the projection; a
/// bespoke ack carrying a subset would be a second one that can disagree.
fn snapshot_for(
    kernel: &GoalKernel,
    session_id: &str,
    goal_id: &GoalId,
    request_id: &str,
) -> Vec<ProtocolEvent> {
    match kernel.goal(goal_id) {
        Ok(Some(state)) => match goal_snapshot_event(session_id, &state) {
            Ok(event) => vec![event],
            Err(_) => refuse(
                request_id,
                session_id,
                goal_id.as_str(),
                GoalControlRefusalReason::JournalError,
            ),
        },
        _ => refuse(
            request_id,
            session_id,
            goal_id.as_str(),
            GoalControlRefusalReason::JournalError,
        ),
    }
}

/// Whether the host's cursor still matches the Goal's committed cursor.
fn cursor_matches(state: &GoalState, supplied: &wcore_protocol::events::RecoveryCursor) -> bool {
    let current = state.cursor();
    current.journal_sequence == supplied.journal_sequence
        && current.journal_digest == supplied.journal_digest
}

/// Answer a host Goal control command.
///
/// `None` means the command is not one of the five and the caller should fall
/// through to its own arms.
#[must_use]
pub fn handle_goal_control(
    journal: Option<&SessionJournal>,
    live_session_id: Option<&str>,
    parent: &GoalParentEnvelope,
    now_unix_ms: u64,
    command: &ProtocolCommand,
) -> Option<Vec<ProtocolEvent>> {
    match command {
        ProtocolCommand::GoalOpen(open) => Some(handle_open(
            journal,
            live_session_id,
            parent,
            now_unix_ms,
            open,
        )),
        ProtocolCommand::GoalDeclareTask(task) => {
            Some(handle_declare_task(journal, live_session_id, task))
        }
        ProtocolCommand::GoalAdvance(advance) => {
            Some(handle_advance(journal, live_session_id, advance))
        }
        ProtocolCommand::GoalCancel(cancel) => Some(handle_cancel(journal, live_session_id, cancel)),
        ProtocolCommand::GoalResync(resync) => Some(handle_resync(journal, live_session_id, resync)),
        _ => None,
    }
}

fn handle_open(
    journal: Option<&SessionJournal>,
    live_session_id: Option<&str>,
    parent: &GoalParentEnvelope,
    now_unix_ms: u64,
    open: &wcore_protocol::commands::GoalOpenCommand,
) -> Vec<ProtocolEvent> {
    let kernel = match preflight(
        journal,
        live_session_id,
        open.goal_version,
        &open.request_id,
        &open.session_id,
        &open.goal_id,
    ) {
        Ok(kernel) => kernel,
        Err(refusal) => return refusal,
    };
    if open.goal_id.trim().is_empty() || open.iterations == 0 {
        return refuse(
            &open.request_id,
            &open.session_id,
            &open.goal_id,
            GoalControlRefusalReason::Malformed,
        );
    }
    let goal_id = GoalId::new(&open.goal_id);
    match kernel.goal(&goal_id) {
        Ok(Some(_)) => {
            return refuse(
                &open.request_id,
                &open.session_id,
                &open.goal_id,
                GoalControlRefusalReason::GoalAlreadyExists,
            );
        }
        Ok(None) => {}
        Err(_) => {
            return refuse(
                &open.request_id,
                &open.session_id,
                &open.goal_id,
                GoalControlRefusalReason::JournalError,
            );
        }
    }

    // `Once` for a bound of 1, `Fixed` above it — the same mapping
    // `goal open --iterations` uses. There is no unbounded spelling.
    let request = GoalAuthorityRequest {
        requested_limits: [("max_tokens".to_owned(), open.max_tokens)]
            .into_iter()
            .collect(),
        strategy: open.strategy,
        loop_policy: if open.iterations == 1 {
            LoopPolicy::Once
        } else {
            LoopPolicy::Fixed {
                iterations: open.iterations,
            }
        },
    };
    // The intersection, never the request.
    let snapshot = resolve_goal_authority(&request, &parent.effective_limits, parent.digest.clone());
    match kernel.open_goal(&goal_id, &open.objective, &snapshot, now_unix_ms) {
        Ok(_) => snapshot_for(&kernel, &open.session_id, &goal_id, &open.request_id),
        Err(_) => refuse(
            &open.request_id,
            &open.session_id,
            &open.goal_id,
            GoalControlRefusalReason::JournalError,
        ),
    }
}

fn handle_declare_task(
    journal: Option<&SessionJournal>,
    live_session_id: Option<&str>,
    task: &wcore_protocol::commands::GoalDeclareTaskCommand,
) -> Vec<ProtocolEvent> {
    let kernel = match preflight(
        journal,
        live_session_id,
        task.goal_version,
        &task.request_id,
        &task.session_id,
        &task.goal_id,
    ) {
        Ok(kernel) => kernel,
        Err(refusal) => return refusal,
    };
    if task.task_id.trim().is_empty() {
        return refuse(
            &task.request_id,
            &task.session_id,
            &task.goal_id,
            GoalControlRefusalReason::Malformed,
        );
    }
    let goal_id = GoalId::new(&task.goal_id);
    let state = match load_goal(&kernel, &goal_id, &task.request_id, &task.session_id) {
        Ok(state) => state,
        Err(refusal) => return refusal,
    };
    if state.tasks.contains_key(&task.task_id) {
        return refuse(
            &task.request_id,
            &task.session_id,
            &task.goal_id,
            GoalControlRefusalReason::TaskAlreadyDeclared,
        );
    }
    // The reducer refuses a dependency on an undeclared task, because treating
    // an unknown dependency as satisfied would release a dependent on a task
    // that never exists. Checked HERE too so the host gets that specific
    // reason: collapsing it into `JournalError` would tell a control plane to
    // retry a write when the actual fix is to declare the dependency first.
    if task.depends_on.contains(&task.task_id)
        || task
            .depends_on
            .iter()
            .any(|dependency| !state.tasks.contains_key(dependency))
    {
        return refuse(
            &task.request_id,
            &task.session_id,
            &task.goal_id,
            GoalControlRefusalReason::DependencyNotDeclared,
        );
    }
    let Some(journal) = journal else {
        return refuse(
            &task.request_id,
            &task.session_id,
            &task.goal_id,
            GoalControlRefusalReason::JournalUnavailable,
        );
    };
    let key = task
        .idempotency_key
        .clone()
        .unwrap_or_else(|| format!("idem-{}", task.task_id));
    let ledger = GoalLedger::new(journal.clone());
    match ledger.declare_task(&goal_id, &TaskId::new(&task.task_id), &task.depends_on, &key) {
        Ok(()) => snapshot_for(&kernel, &task.session_id, &goal_id, &task.request_id),
        Err(_) => refuse(
            &task.request_id,
            &task.session_id,
            &task.goal_id,
            GoalControlRefusalReason::JournalError,
        ),
    }
}

fn handle_advance(
    journal: Option<&SessionJournal>,
    live_session_id: Option<&str>,
    advance: &wcore_protocol::commands::GoalAdvanceCommand,
) -> Vec<ProtocolEvent> {
    let kernel = match preflight(
        journal,
        live_session_id,
        advance.goal_version,
        &advance.request_id,
        &advance.session_id,
        &advance.goal_id,
    ) {
        Ok(kernel) => kernel,
        Err(refusal) => return refusal,
    };
    let goal_id = GoalId::new(&advance.goal_id);
    let state = match load_goal(&kernel, &goal_id, &advance.request_id, &advance.session_id) {
        Ok(state) => state,
        Err(refusal) => return refusal,
    };
    if matches!(state.lifecycle, GoalLifecycle::Terminated { .. }) {
        return refuse(
            &advance.request_id,
            &advance.session_id,
            &advance.goal_id,
            GoalControlRefusalReason::GoalTerminated,
        );
    }
    if !cursor_matches(&state, &advance.cursor) {
        return refuse(
            &advance.request_id,
            &advance.session_id,
            &advance.goal_id,
            GoalControlRefusalReason::CursorStale,
        );
    }
    // The ceiling is checked HERE as well as in the reducer. `Manual` has no
    // numeric ceiling and is therefore never refused on this axis — that is
    // the policy whose whole meaning is that an operator advances it.
    if let Some(ceiling) = state.authority.iteration_ceiling()
        && state.iterations_started >= ceiling
    {
        return refuse(
            &advance.request_id,
            &advance.session_id,
            &advance.goal_id,
            GoalControlRefusalReason::IterationCeilingReached,
        );
    }
    match kernel.start_iteration(&goal_id) {
        Ok(_) => snapshot_for(&kernel, &advance.session_id, &goal_id, &advance.request_id),
        Err(_) => refuse(
            &advance.request_id,
            &advance.session_id,
            &advance.goal_id,
            GoalControlRefusalReason::JournalError,
        ),
    }
}

fn handle_cancel(
    journal: Option<&SessionJournal>,
    live_session_id: Option<&str>,
    cancel: &wcore_protocol::commands::GoalCancelCommand,
) -> Vec<ProtocolEvent> {
    let kernel = match preflight(
        journal,
        live_session_id,
        cancel.goal_version,
        &cancel.request_id,
        &cancel.session_id,
        &cancel.goal_id,
    ) {
        Ok(kernel) => kernel,
        Err(refusal) => return refusal,
    };
    let goal_id = GoalId::new(&cancel.goal_id);
    let state = match load_goal(&kernel, &goal_id, &cancel.request_id, &cancel.session_id) {
        Ok(state) => state,
        Err(refusal) => return refusal,
    };
    if matches!(state.lifecycle, GoalLifecycle::Terminated { .. }) {
        return refuse(
            &cancel.request_id,
            &cancel.session_id,
            &cancel.goal_id,
            GoalControlRefusalReason::GoalTerminated,
        );
    }
    if !cursor_matches(&state, &cancel.cursor) {
        return refuse(
            &cancel.request_id,
            &cancel.session_id,
            &cancel.goal_id,
            GoalControlRefusalReason::CursorStale,
        );
    }
    // Always `Cancelled`. The host does not nominate a terminal: letting the
    // wire pick would let an untrusted peer reach for `Verified`, the one
    // stamp reserved for a real executable gate.
    match kernel.terminate(&goal_id, GoalTerminalState::Cancelled) {
        Ok(_) => snapshot_for(&kernel, &cancel.session_id, &goal_id, &cancel.request_id),
        Err(_) => refuse(
            &cancel.request_id,
            &cancel.session_id,
            &cancel.goal_id,
            GoalControlRefusalReason::JournalError,
        ),
    }
}

fn handle_resync(
    journal: Option<&SessionJournal>,
    live_session_id: Option<&str>,
    resync: &wcore_protocol::commands::GoalResyncCommand,
) -> Vec<ProtocolEvent> {
    let named = resync.goal_id.clone().unwrap_or_default();
    let kernel = match preflight(
        journal,
        live_session_id,
        resync.goal_version,
        &resync.request_id,
        &resync.session_id,
        &named,
    ) {
        Ok(kernel) => kernel,
        Err(refusal) => return refusal,
    };
    let Some(journal) = journal else {
        return refuse(
            &resync.request_id,
            &resync.session_id,
            &named,
            GoalControlRefusalReason::JournalUnavailable,
        );
    };

    match &resync.goal_id {
        Some(goal_id) => {
            let goal_id = GoalId::new(goal_id);
            match load_goal(&kernel, &goal_id, &resync.request_id, &resync.session_id) {
                Ok(_) => snapshot_for(&kernel, &resync.session_id, &goal_id, &resync.request_id),
                Err(refusal) => refusal,
            }
        }
        // Every Goal in the session, in the reducer's own (id) order. An empty
        // session yields an EMPTY vec, which is a correct answer and not a
        // refusal: "this session holds no Goals" is a fact, not an error.
        None => match journal.state() {
            Ok(state) => state
                .goals
                .values()
                .filter_map(|goal| goal_snapshot_event(&resync.session_id, goal).ok())
                .collect(),
            Err(_) => refuse(
                &resync.request_id,
                &resync.session_id,
                &named,
                GoalControlRefusalReason::JournalError,
            ),
        },
    }
}

/// Re-exported so a caller can digest a Goal without reaching into `wire`.
#[must_use]
pub fn state_digest(state: &GoalState) -> Option<String> {
    goal_state_digest(state).ok()
}
