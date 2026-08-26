//! Operator verbs over the existing session substrate (F23-02).
//!
//! This module composes [`crate::session::SessionManager`] and the durable
//! [`crate::session_journal`]; it does not reimplement either. Every verb here
//! is something a *human* does to a session — search, inspect, fork, retry,
//! export, retain, reconcile, cancel — and each is reachable from the shipped
//! `wayland-core session` subcommand.
//!
//! Two design rules are load-bearing and are asserted by tests:
//!
//! 1. **The export envelope carries no free text.** `wcore-protocol`'s
//!    `RecoveryTurnSnapshot` deliberately carries opaque identifiers and typed
//!    state and never transcript text, prompts, tool arguments, tool output,
//!    paths or provider payloads. The export envelope honours the same rule.
//!    That is what makes "a run-time nonce planted in the session is absent
//!    from the exported bytes" true *by construction* rather than by a
//!    shape-matching filter, which could never satisfy it for an arbitrary
//!    value. Message content is represented by a SHA-256 digest and a byte
//!    length, which is sufficient for F26-03 to detect divergence on migration.
//!
//! 2. **Reconcile reuses the states the product already defines.** An
//!    outstanding item is a tool execution the reducer left in
//!    `ToolEffectState::Unknown`; resolving one appends the existing
//!    `SessionEvent::ToolExecutionResolved` with
//!    `ToolResolutionSource::Operator`. No ninth `RecoveryReconcileReason`
//!    variant is introduced — that enum is CI-checked against the Desktop
//!    contract corpus.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::session::{Session, SessionManager, SessionMeta};
use crate::session_journal::{
    ExternalEffectState, HookPhaseState, JournalError, ReducedSessionState, SessionEvent,
    SessionJournal, ToolEffectState, ToolResolution, ToolResolutionSource, TurnCompletion,
};
use wcore_types::tool::ToolEffectKind;

/// Key under which a fork records its parent in [`Session::extra`].
///
/// `extra` is the schema's forward-compatibility overflow bucket: it is
/// preserved verbatim across a save/load round trip, which is exactly the
/// property additive operator metadata needs. Using it keeps `Session` itself
/// unmodified.
const LINEAGE_PARENT_KEY: &str = "wayland_lineage_parent";
/// Key under which [`retain`] records the retain-until instant.
const RETAIN_UNTIL_KEY: &str = "wayland_retain_until";

/// Errors from an operator verb. Public API, so `thiserror`-structured, and
/// every variant that concerns a file names that file.
#[derive(Debug, thiserror::Error)]
pub enum SessionLifecycleError {
    #[error("session '{id}' was not found")]
    NotFound { id: String },
    #[error("session file '{path}' is unreadable or corrupt: {source}")]
    CorruptSession {
        path: PathBuf,
        #[source]
        source: anyhow::Error,
    },
    #[error("session store at '{path}' could not be read: {source}")]
    Store {
        path: PathBuf,
        #[source]
        source: anyhow::Error,
    },
    #[error("journal '{path}' could not be read: {source}")]
    Journal {
        path: PathBuf,
        #[source]
        source: JournalError,
    },
    #[error("refused by session authority: {reason}")]
    RefusedByAuthority { reason: String },
    /// Carries the blocking items themselves, not just how many there are.
    /// A count cannot be turned into a remedy, and the operator surface has to
    /// print the exact command that clears each one — the defect this replaced
    /// was a refusal that named no way out.
    #[error("session '{id}' has {} outstanding reconcile item(s)", .items.len())]
    OutstandingReconcile {
        id: String,
        items: Vec<ReconcileItem>,
    },
    #[error("io error at '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

type Result<T> = std::result::Result<T, SessionLifecycleError>;

/// One hit from [`search`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSearchHit {
    pub id: String,
    /// How many stored messages in this session matched the query.
    pub match_count: usize,
    pub updated_at: DateTime<Utc>,
}

/// Retention verdict for one session.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum RetentionState {
    /// No retain-until has been set.
    Unbounded,
    /// A retain-until is set and has not yet passed.
    Retained { until: DateTime<Utc> },
    /// The retain-until has passed. The session is **reported** as expired and
    /// is never deleted as a side effect of reporting.
    Expired { until: DateTime<Utc> },
}

/// Which class of turn descendant an outstanding item belongs to.
///
/// The reducer refuses `TurnCancelled` while ANY descendant of the turn is
/// nonterminal, and it checks five classes, not one
/// (`require_turn_descendants_terminal`). Projecting only tool executions —
/// as the first cut of this module did — made `reconcile` report
/// `outstanding=0` on a session `cancel` then refused, which reproduces the
/// exact dead end defect D2 describes. The projection therefore covers every
/// class the reducer gates on, even where the CLI cannot yet resolve one.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileKind {
    ToolExecution,
    ProviderAttempt,
    Approval,
    HookPhase,
    Child,
}

impl ReconcileKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolExecution => "tool_execution",
            Self::ProviderAttempt => "provider_attempt",
            Self::Approval => "approval",
            Self::HookPhase => "hook_phase",
            Self::Child => "child",
        }
    }
}

/// One outstanding nonterminal item blocking a turn from reaching a terminal
/// state, and therefore blocking `cancel` and `--continue`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconcileItem {
    pub kind: ReconcileKind,
    /// Journal id of the descendant: a tool execution id, provider attempt id,
    /// approval id, hook phase id or child id.
    pub tool_execution_id: String,
    pub turn_id: String,
    /// Tool name for a tool execution; the descendant class otherwise. Never
    /// tool arguments or output.
    pub tool: String,
    /// The reducer's own typed reason for the nonterminal state.
    pub reason: String,
    /// Whether `reconcile --resolve` can dispose of this item today. An item
    /// this surface cannot resolve is still REPORTED, because a silent
    /// omission is what made the original defect undiagnosable.
    pub operator_resolvable: bool,
}

/// What [`inspect`] reports for one session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInspection {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub provider: String,
    pub model: String,
    pub message_count: usize,
    /// Number of turns the durable journal recorded. `None` when the session
    /// predates the journal or has none on disk.
    pub journal_turn_count: Option<usize>,
    /// Number of journal turns with no terminal completion — the turns that
    /// make `--continue` refuse.
    pub interrupted_turn_count: usize,
    pub lineage_parent: Option<String>,
    pub retention: RetentionState,
    pub outstanding_reconcile: Vec<ReconcileItem>,
}

/// One blocker `cancel` settled without asking, and what settled it.
#[derive(Debug, Clone)]
pub struct AutoResolved {
    pub item: ReconcileItem,
    pub determined_by: DeterminedBy,
}

/// What [`cancel`] did. The auto-resolved list is reported rather than
/// swallowed: the command wrote durable receipts, and an operator is entitled
/// to see every one it wrote on their behalf.
#[derive(Debug, Clone, Default)]
pub struct CancelOutcome {
    pub auto_resolved: Vec<AutoResolved>,
    pub cancelled_turns: Vec<String>,
}

/// Result of [`fork`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkOutcome {
    pub parent_id: String,
    pub child_id: String,
    pub messages_copied: usize,
    /// SHA-256 of the parent's on-disk bytes, taken *after* the fork. The
    /// caller compares it against the pre-fork digest to prove the parent was
    /// untouched.
    pub parent_digest_after: String,
}

/// Per-message provenance in an export. Digest and length only — never text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportedMessage {
    pub index: usize,
    pub role: String,
    pub content_sha256: String,
    pub content_bytes: usize,
}

/// The redacted, portable export envelope. F26-03 consumes this shape.
///
/// Deliberately carries **no** transcript text, tool arguments, tool output,
/// prompts, filesystem paths or provider payloads. See the module docs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExportEnvelope {
    pub envelope_version: u32,
    pub source_session_id: String,
    pub exported_at: DateTime<Utc>,
    /// Build identity of the binary that produced this envelope.
    pub exporting_build: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub provider: String,
    pub model: String,
    pub message_count: usize,
    pub lineage_parent: Option<String>,
    pub retention: RetentionState,
    pub messages: Vec<ExportedMessage>,
    /// Typed journal state: turn id to completion kind. No user or assistant
    /// text.
    pub turn_completions: BTreeMap<String, String>,
    pub outstanding_reconcile: Vec<ReconcileItem>,
}

/// Current envelope version. Bump when a required field is added.
pub const SESSION_EXPORT_ENVELOPE_VERSION: u32 = 1;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn load_session(manager: &SessionManager, id: &str) -> Result<Session> {
    // `load` maps both "absent" and "corrupt" onto anyhow. Distinguish them by
    // asking the index first, so a missing session is NotFound (a normal
    // operator outcome) and a present-but-unreadable one is CorruptSession
    // naming the file (a defect the operator must see).
    let known = manager
        .list()
        .map_err(|source| SessionLifecycleError::Store {
            path: manager.directory().to_path_buf(),
            source,
        })?
        .into_iter()
        .any(|meta| meta.id == id);
    match manager.load(id) {
        Ok(session) => Ok(session),
        Err(source) if !known => {
            let _ = source;
            Err(SessionLifecycleError::NotFound { id: id.to_owned() })
        }
        Err(source) => Err(SessionLifecycleError::CorruptSession {
            path: manager.session_file_path(id),
            source,
        }),
    }
}

/// Read the durable journal without taking the writer lease.
///
/// Returns `Ok(None)` when the session has no journal on disk, which is the
/// normal case for a session created before the journal existed.
fn read_journal_state(manager: &SessionManager, id: &str) -> Result<Option<ReducedSessionState>> {
    let path = manager.journal_path(id);
    if !path.exists() {
        return Ok(None);
    }
    SessionJournal::recovered_state(&path)
        .map(Some)
        .map_err(|source| SessionLifecycleError::Journal { path, source })
}

/// Did this provider attempt durably record any streamed provider bytes?
///
/// If it did, a terminal receipt must carry a response digest recomputed from
/// those exact bytes, which is engine-only knowledge. If it did not, the
/// reducer accepts a `Cancelled` terminal with no digest.
fn attempt_has_stream_events(state: &ReducedSessionState, attempt_id: &str) -> bool {
    state
        .streams
        .values()
        .filter(|stream| stream.attempt_id == attempt_id)
        .any(|stream| stream.batches.iter().any(|batch| !batch.is_empty()))
}

/// Project every nonterminal turn descendant the reducer gates `TurnCancelled`
/// on, across all five classes it checks. Anything reported here blocks
/// `cancel` and therefore blocks `--continue`.
fn outstanding_items(state: &ReducedSessionState) -> Vec<ReconcileItem> {
    let mut items = Vec::new();

    for (id, tool) in &state.tools {
        let nonterminal = matches!(
            tool.effect,
            ToolEffectState::Prepared | ToolEffectState::Running | ToolEffectState::Unknown { .. }
        );
        if !nonterminal {
            continue;
        }
        items.push(ReconcileItem {
            kind: ReconcileKind::ToolExecution,
            tool_execution_id: id.clone(),
            turn_id: tool.turn_id.clone(),
            tool: tool.tool.clone(),
            reason: match &tool.effect {
                ToolEffectState::Unknown { reason, .. } => format!("{reason:?}"),
                other => format!("{other:?}"),
            },
            // Only an Unknown tool takes `ToolExecutionResolved`; the reducer's
            // `require_tool_unknown` refuses it from Prepared or Running. A
            // RUNNING tool is still resolvable here because [`reconcile_resolve`]
            // first records the interruption that the crash left implicit — see
            // the comment there for why that is an operator-writable fact and
            // not an engine-only receipt.
            operator_resolvable: matches!(
                tool.effect,
                ToolEffectState::Running | ToolEffectState::Unknown { .. }
            ),
        });
    }

    for (id, attempt) in &state.provider_attempts {
        if !matches!(
            attempt.effect,
            ExternalEffectState::Prepared | ExternalEffectState::Unknown
        ) {
            continue;
        }
        items.push(ReconcileItem {
            kind: ReconcileKind::ProviderAttempt,
            tool_execution_id: id.clone(),
            turn_id: attempt.turn_id.clone(),
            tool: "provider_attempt".to_owned(),
            reason: format!("{:?}", attempt.effect),
            // An attempt is operator-resolvable when a terminal receipt can be
            // written WITHOUT knowledge only the engine holds. The reducer
            // accepts a `Cancelled` terminal — V1 or dispatch-correlated V2 —
            // as long as the attempt has no durable stream events, because
            // then no response digest has to be reproduced. An attempt that
            // DID stream bytes needs a digest recomputed from those bytes, so
            // it stays engine-only and is reported as such rather than hidden.
            operator_resolvable: !attempt_has_stream_events(state, id),
        });
    }

    for (id, approval) in &state.approvals {
        if approval.resolution.is_some() {
            continue;
        }
        items.push(ReconcileItem {
            kind: ReconcileKind::Approval,
            tool_execution_id: id.clone(),
            turn_id: String::new(),
            tool: "approval".to_owned(),
            reason: "PendingApproval".to_owned(),
            operator_resolvable: false,
        });
    }

    for (id, phase) in &state.hook_phases {
        if matches!(
            phase.state,
            HookPhaseState::Consumed { .. }
                | HookPhaseState::NotStarted { .. }
                | HookPhaseState::NotApplicable
                | HookPhaseState::AbandonedUnknown
        ) {
            continue;
        }
        items.push(ReconcileItem {
            kind: ReconcileKind::HookPhase,
            tool_execution_id: id.clone(),
            turn_id: phase.turn_id.clone(),
            tool: "hook_phase".to_owned(),
            reason: format!("{:?}", phase.state),
            operator_resolvable: false,
        });
    }

    for (id, child) in &state.children {
        if child.durable.is_some()
            || !matches!(
                child.effect,
                ExternalEffectState::Prepared | ExternalEffectState::Unknown
            )
        {
            continue;
        }
        items.push(ReconcileItem {
            kind: ReconcileKind::Child,
            tool_execution_id: id.clone(),
            turn_id: child.turn_id.clone(),
            tool: "child".to_owned(),
            reason: format!("{:?}", child.effect),
            operator_resolvable: false,
        });
    }

    items
}

/// Why one outstanding item needs no human judgement.
///
/// `reconcile --resolve` used to take `--as-outcome not-started` BY DEFAULT,
/// which asks the operator whether an effect landed — precisely the thing they
/// are reporting they do not know — and then silently answered it for them.
/// Two of the three common shapes do not need the question asked at all, and
/// the journal is where the answer already is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeterminedBy {
    /// The reducer's own state fixes the receipt. A PREPARED provider attempt
    /// was never dispatched; an UNKNOWN one was dispatched and its outcome was
    /// never observed. `reconcile_resolve` has always ignored `--as-outcome`
    /// for this class — the flag was decoration over a decision the journal
    /// had already made.
    ProviderAttemptState,
    /// The tool declared [`ToolEffectKind::RepeatSafe`] AND named a
    /// reconciler this build registers: by a contract the product recognises,
    /// the invocation cannot have created an external effect, so there is no
    /// landed effect for anyone to have an opinion about.
    RepeatSafeContract,
}

impl DeterminedBy {
    pub fn as_str(self) -> &'static str {
        match self {
            DeterminedBy::ProviderAttemptState => "provider_attempt_state",
            DeterminedBy::RepeatSafeContract => "repeat_safe_effect_contract",
        }
    }
}

/// Who decided one resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionAuthority {
    /// A human asserted it with an explicit `--as-outcome`.
    Operator(OperatorResolution),
    /// The product read it out of the journal — see [`DeterminedBy`].
    Journal(DeterminedBy),
}

/// The refusal shown when the product genuinely cannot tell what happened.
///
/// This is the branch that must not fake certainty. It names why the answer is
/// unavailable, and then spells out what each disposition COMMITS the session
/// to, so the operator is guessing with the consequences in front of them
/// rather than picking a word.
fn unanswerable_reason(item: &ReconcileItem) -> String {
    format!(
        "the journal cannot tell whether the effect of `{tool}` ({id}) landed. It was \
         interrupted in state {state}, its effect contract is `opaque` — no receipt to \
         re-read, and repeating it is not safe — so only you can say what happened. Re-run \
         with an explicit disposition:\n  \
         --as-outcome not-started   the work never began; nothing it would have changed was \
         changed. The turn records no result and nothing re-runs it.\n  \
         --as-outcome succeeded     the work completed. The turn records success with a NULL \
         result — no output is invented — and nothing re-runs it.\n  \
         --as-outcome failed        the work began and failed. The turn records the failure.\n\
         All three are durable and a later resume will not revisit them, so if you cannot \
         tell, inspect the workspace before choosing",
        tool = item.tool,
        id = item.tool_execution_id,
        state = item.reason,
    )
}

/// Can the product settle this item from the journal alone?
///
/// `None` means it genuinely cannot, and the honest response is to say so and
/// ask — never to pick a default. The class that reaches `None` in practice is
/// a tool whose effect contract is `Opaque` (`Write`, `Edit`, and every `Bash`
/// command outside the read-only classifier): the journal records that the
/// tool STARTED and nothing after it, there is no receipt to compare, and
/// repeating it is not safe. No amount of reading the journal turns that into
/// knowledge.
pub fn determined_disposition(
    state: &ReducedSessionState,
    item: &ReconcileItem,
) -> Option<DeterminedBy> {
    if !item.operator_resolvable {
        return None;
    }
    match item.kind {
        ReconcileKind::ProviderAttempt => Some(DeterminedBy::ProviderAttemptState),
        ReconcileKind::ToolExecution => {
            let tool = state.tools.get(&item.tool_execution_id)?;
            // The NAME is load-bearing, not the kind. `reconciler: None` is
            // documented on the field as "no automatic reconciler is
            // available", and an unregistered name says the same thing — so a
            // tool cannot obtain a receipt written on the operator's behalf by
            // declaring a repeat-safe kind and inventing an identifier. This
            // is the same gate the engine's own recovery applies, and the two
            // must not disagree about the same effect.
            let reconciler = tool.effect_contract.reconciler.as_deref()?;
            (tool.effect_contract.kind == ToolEffectKind::RepeatSafe
                && wcore_types::tool::repeat_safe_reconciler_is_registered(reconciler))
            .then_some(DeterminedBy::RepeatSafeContract)
        }
        _ => None,
    }
}

/// The interruption a crash left implicit, recorded before any receipt.
///
/// The journal's last word on a crashed tool is `Running`, and only `Unknown`
/// accepts a resolution. This asserts nothing about the outcome — it is an
/// admission of ignorance, and the exclusive writer lease the caller holds is
/// the proof that no engine still owns the execution.
fn record_tool_interruption(
    journal: &SessionJournal,
    state: &ReducedSessionState,
    path: &std::path::Path,
    tool_execution_id: &str,
    recorded_by: &str,
) -> Result<()> {
    if !matches!(
        state.tools.get(tool_execution_id).map(|tool| &tool.effect),
        Some(ToolEffectState::Running)
    ) {
        return Ok(());
    }
    journal
        .append(SessionEvent::ToolExecutionUnknown {
            tool_execution_id: tool_execution_id.to_owned(),
            reason: crate::session_journal::ToolUnknownReason::Interrupted,
            evidence: serde_json::json!({
                "recovery": "wayland-core session reconcile",
                "prior_state": "running",
                "operator_id": recorded_by,
            }),
        })
        .map(|_| ())
        .map_err(|source| SessionLifecycleError::Journal {
            path: path.to_path_buf(),
            source,
        })
}

/// The terminal receipt a crashed provider attempt takes, decided by the state
/// the crash left rather than by anyone's opinion.
///
/// NOTE the fidelity limit, recorded rather than hidden: unlike
/// `ToolExecutionResolved`, the provider-attempt receipts carry no `source`
/// field, so the journal does not record who asserted this outcome. Filed as
/// 23B-M3.
fn provider_attempt_receipt(
    state: &ReducedSessionState,
    id: &str,
    attempt_id: &str,
    reason: &str,
) -> Result<SessionEvent> {
    let attempt =
        state
            .provider_attempts
            .get(attempt_id)
            .ok_or_else(|| SessionLifecycleError::NotFound {
                id: format!("{id}/{attempt_id}"),
            })?;
    Ok(match (&attempt.effect, attempt.dispatch_id.as_ref()) {
        (ExternalEffectState::Prepared, None) => SessionEvent::ProviderAttemptNotStarted {
            attempt_id: attempt_id.to_owned(),
            reason: crate::session_journal::ProviderAttemptNotStartedReason::Cancelled {
                reason: reason.to_owned(),
            },
        },
        (ExternalEffectState::Prepared, Some(dispatch_id)) => {
            SessionEvent::ProviderAttemptNotStartedV2 {
                attempt_id: attempt_id.to_owned(),
                dispatch_id: dispatch_id.clone(),
                reason: crate::session_journal::ProviderAttemptNotStartedReason::Cancelled {
                    reason: reason.to_owned(),
                },
            }
        }
        (_, None) => SessionEvent::ProviderAttemptFinished {
            attempt_id: attempt_id.to_owned(),
            outcome: crate::session_journal::CompletionOutcome::Cancelled,
            response_digest: None,
        },
        (_, Some(dispatch_id)) => SessionEvent::ProviderAttemptFinishedV2 {
            attempt_id: attempt_id.to_owned(),
            dispatch_id: dispatch_id.clone(),
            outcome: crate::session_journal::CompletionOutcome::Cancelled,
            response_digest: None,
        },
    })
}

fn interrupted_turns(state: &ReducedSessionState) -> Vec<String> {
    state
        .turns
        .iter()
        .filter(|(_, turn)| turn.completion.is_none())
        .map(|(id, _)| id.clone())
        .collect()
}

fn retention_of(session: &Session, now: DateTime<Utc>) -> RetentionState {
    let Some(raw) = session.extra.get(RETAIN_UNTIL_KEY).and_then(|v| v.as_str()) else {
        return RetentionState::Unbounded;
    };
    match DateTime::parse_from_rfc3339(raw) {
        Ok(until) => {
            let until = until.with_timezone(&Utc);
            if until <= now {
                RetentionState::Expired { until }
            } else {
                RetentionState::Retained { until }
            }
        }
        // An unparseable retain-until is reported as unbounded rather than
        // silently treated as expired: expiry must never be inferred from
        // damage.
        Err(_) => RetentionState::Unbounded,
    }
}

fn lineage_parent_of(session: &Session) -> Option<String> {
    session
        .extra
        .get(LINEAGE_PARENT_KEY)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

fn message_text(message: &serde_json::Value) -> String {
    // Messages are stored as provider-neutral JSON. Concatenate every string
    // leaf so search sees the same bytes the store holds, without depending on
    // one content-block shape.
    fn walk(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::String(s) => {
                out.push_str(s);
                out.push('\n');
            }
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            serde_json::Value::Object(map) => map.values().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    let mut out = String::new();
    walk(message, &mut out);
    out
}

/// Full-text search over persisted sessions, scoped to this manager's
/// directory (and therefore to the caller's active profile).
///
/// Reads the persisted transcript through the manager rather than maintaining
/// a second index, so a hit can never describe content the store does not
/// hold. A query matching nothing is a successful empty result, not an error.
pub fn search(manager: &SessionManager, query: &str) -> Result<Vec<SessionSearchHit>> {
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    for meta in manager
        .list()
        .map_err(|source| SessionLifecycleError::Store {
            path: manager.directory().to_path_buf(),
            source,
        })?
    {
        // A corrupt session must not abort the whole search; it is skipped and
        // remains individually diagnosable through `inspect`.
        let Ok(session) = manager.load(&meta.id) else {
            continue;
        };
        let match_count = session
            .messages
            .iter()
            .filter(|message| {
                serde_json::to_value(message)
                    .map(|v| message_text(&v).to_lowercase().contains(&needle))
                    .unwrap_or(false)
            })
            .count();
        if match_count > 0 {
            hits.push(SessionSearchHit {
                id: meta.id,
                match_count,
                updated_at: meta.updated_at,
            });
        }
    }
    // Most recently updated first.
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.updated_at));
    Ok(hits)
}

/// Everything an operator needs to decide what to do with one session.
pub fn inspect(manager: &SessionManager, id: &str) -> Result<SessionInspection> {
    let session = load_session(manager, id)?;
    let state = read_journal_state(manager, id)?;
    let (journal_turn_count, interrupted, outstanding) = match &state {
        Some(state) => (
            Some(state.turns.len()),
            interrupted_turns(state).len(),
            outstanding_items(state),
        ),
        None => (None, 0, Vec::new()),
    };
    Ok(SessionInspection {
        id: session.id.clone(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        provider: session.provider.clone(),
        model: session.model.clone(),
        message_count: session.messages.len(),
        journal_turn_count,
        interrupted_turn_count: interrupted,
        lineage_parent: lineage_parent_of(&session),
        retention: retention_of(&session, Utc::now()),
        outstanding_reconcile: outstanding,
    })
}

/// List the sessions this manager can see.
pub fn list(manager: &SessionManager) -> Result<Vec<SessionMeta>> {
    manager
        .list()
        .map_err(|source| SessionLifecycleError::Store {
            path: manager.directory().to_path_buf(),
            source,
        })
}

/// Fork a session at its current head.
///
/// The child records the parent in its lineage. The parent is re-read and
/// digested *after* the copy so the caller can prove it was untouched.
pub fn fork(manager: &SessionManager, id: &str) -> Result<ForkOutcome> {
    let parent = load_session(manager, id)?;
    let parent_path = manager.session_file_path(id);

    let mut child = manager
        .create(&parent.provider, &parent.model, &parent.cwd, None)
        .map_err(|source| SessionLifecycleError::Store {
            path: manager.directory().to_path_buf(),
            source,
        })?;
    child.messages.clone_from(&parent.messages);
    child.total_usage = parent.total_usage.clone();
    child.extra = parent.extra.clone();
    child.extra.insert(
        LINEAGE_PARENT_KEY.to_owned(),
        serde_json::Value::String(parent.id.clone()),
    );
    let messages_copied = child.messages.len();
    manager
        .persist_first_message(&child)
        .map_err(|source| SessionLifecycleError::Store {
            path: manager.directory().to_path_buf(),
            source,
        })?;

    let bytes = std::fs::read(&parent_path).map_err(|source| SessionLifecycleError::Io {
        path: parent_path.clone(),
        source,
    })?;
    Ok(ForkOutcome {
        parent_id: parent.id,
        child_id: child.id,
        messages_copied,
        parent_digest_after: sha256_hex(&bytes),
    })
}

/// SHA-256 of a session's on-disk bytes, for a before/after comparison.
pub fn session_file_digest(manager: &SessionManager, id: &str) -> Result<String> {
    let path = manager.session_file_path(id);
    let bytes = std::fs::read(&path).map_err(|source| SessionLifecycleError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(sha256_hex(&bytes))
}

/// Outcome of a retry request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RetryOutcome {
    /// Approval was re-derived under the *current* session authority and the
    /// turn is admissible for re-run. `forked_into` carries the new session
    /// holding the retry, so the original turn is retained rather than
    /// overwritten.
    Admitted {
        turn_id: String,
        forked_into: String,
        reapproved: Vec<String>,
    },
    /// The recorded approval is no longer valid under the current authority.
    /// Reported with the existing `ApprovalExpired` reconcile reason rather
    /// than a new error kind, and the turn is **not** replayed.
    RefusedApprovalExpired {
        turn_id: String,
        approval_ids: Vec<String>,
    },
}

/// Re-run one identified turn.
///
/// Retry never mutates the source session: it forks, so the original turn is
/// retained. Approval is **re-derived** from the live session authority; a
/// recorded approval that is no longer valid is refused with the
/// `ApprovalExpired` disposition and never replayed. That refusal is the
/// difference between a retry and an authority-amplification defect.
pub fn retry(manager: &SessionManager, id: &str, turn_id: &str) -> Result<RetryOutcome> {
    let state = read_journal_state(manager, id)?.ok_or(SessionLifecycleError::NotFound {
        id: format!("{id} (journal)"),
    })?;
    if !state.turns.contains_key(turn_id) {
        return Err(SessionLifecycleError::NotFound {
            id: format!("{id}/{turn_id}"),
        });
    }

    // Every approval recorded against a tool call in this turn.
    let turn_tool_ids: Vec<&String> = state
        .tools
        .iter()
        .filter(|(_, tool)| tool.turn_id == turn_id)
        .map(|(id, _)| id)
        .collect();
    let recorded_approvals: Vec<String> = state
        .approvals
        .keys()
        .filter(|approval_id| {
            turn_tool_ids
                .iter()
                .any(|tool_id| approval_id.contains(tool_id.as_str()))
        })
        .cloned()
        .collect();

    // Re-derivation under current authority. A recorded approval is only
    // admissible if the tool execution it authorised reached a terminal,
    // non-unknown state — an approval attached to an effect whose outcome the
    // product cannot vouch for is time-expired by definition, because the
    // authority that granted it can no longer be reconstructed.
    let expired: Vec<String> = recorded_approvals
        .iter()
        .filter(|approval_id| {
            turn_tool_ids.iter().any(|tool_id| {
                approval_id.contains(tool_id.as_str())
                    && state
                        .tools
                        .get(*tool_id)
                        .is_some_and(|tool| tool.effect.requires_reconciliation())
            })
        })
        .cloned()
        .collect();
    if !expired.is_empty() {
        return Ok(RetryOutcome::RefusedApprovalExpired {
            turn_id: turn_id.to_owned(),
            approval_ids: expired,
        });
    }

    let forked = fork(manager, id)?;
    Ok(RetryOutcome::Admitted {
        turn_id: turn_id.to_owned(),
        forked_into: forked.child_id,
        reapproved: recorded_approvals,
    })
}

/// Build the redacted export envelope for one session.
pub fn export(
    manager: &SessionManager,
    id: &str,
    exporting_build: &str,
) -> Result<SessionExportEnvelope> {
    let session = load_session(manager, id)?;
    let state = read_journal_state(manager, id)?;
    let messages = session
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let value = serde_json::to_value(message).unwrap_or(serde_json::Value::Null);
            let role = value
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown")
                .to_owned();
            let text = message_text(&value);
            ExportedMessage {
                index,
                role,
                content_sha256: sha256_hex(text.as_bytes()),
                content_bytes: text.len(),
            }
        })
        .collect();
    let (turn_completions, outstanding) = match &state {
        Some(state) => (
            state
                .turns
                .iter()
                .map(|(turn_id, turn)| {
                    let kind = match &turn.completion {
                        Some(TurnCompletion::Committed { .. }) => "committed",
                        Some(TurnCompletion::Failed { .. }) => "failed",
                        Some(TurnCompletion::Cancelled) => "cancelled",
                        None => "interrupted",
                    };
                    (turn_id.clone(), kind.to_owned())
                })
                .collect(),
            outstanding_items(state),
        ),
        None => (BTreeMap::new(), Vec::new()),
    };
    Ok(SessionExportEnvelope {
        envelope_version: SESSION_EXPORT_ENVELOPE_VERSION,
        source_session_id: session.id.clone(),
        exported_at: Utc::now(),
        exporting_build: exporting_build.to_owned(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        provider: session.provider.clone(),
        model: session.model.clone(),
        message_count: session.messages.len(),
        lineage_parent: lineage_parent_of(&session),
        retention: retention_of(&session, Utc::now()),
        messages,
        turn_completions,
        outstanding_reconcile: outstanding,
    })
}

/// Record an explicit retain-until for a session and report the resulting
/// state. A session past its retain-until is **reported** as expired; nothing
/// is deleted here.
pub fn retain(manager: &SessionManager, id: &str, until: DateTime<Utc>) -> Result<RetentionState> {
    let mut session = load_session(manager, id)?;
    session.extra.insert(
        RETAIN_UNTIL_KEY.to_owned(),
        serde_json::Value::String(until.to_rfc3339()),
    );
    manager
        .save(&session)
        .map_err(|source| SessionLifecycleError::Store {
            path: manager.session_file_path(id),
            source,
        })?;
    Ok(retention_of(&session, Utc::now()))
}

/// The outstanding unknown-effect items for one session.
pub fn reconcile_list(manager: &SessionManager, id: &str) -> Result<Vec<ReconcileItem>> {
    match read_journal_state(manager, id)? {
        Some(state) => Ok(outstanding_items(&state)),
        None => Ok(Vec::new()),
    }
}

/// How an operator dispositions one outstanding item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorResolution {
    /// The effect did land. Its result is unknown to the product, so it is
    /// recorded as succeeded with operator-supplied evidence and a null
    /// result rather than a fabricated one.
    Succeeded,
    /// The effect did not land.
    NotStarted,
    /// The effect landed and failed.
    Failed,
}

/// Resolve one outstanding unknown-effect item under an operator's authority.
///
/// Appends the existing [`SessionEvent::ToolExecutionResolved`] with
/// [`ToolResolutionSource::Operator`], which the reducer already accepts and
/// which is therefore durable across a restart: the same item is not presented
/// twice. This is the operator half of the `reconcile` verb the engine's
/// interrupted-turn refusal already names but which no command surfaced.
pub fn reconcile_resolve(
    manager: &SessionManager,
    id: &str,
    tool_execution_id: &str,
    resolution: Option<OperatorResolution>,
    operator_id: &str,
) -> Result<ResolutionAuthority> {
    let path = manager.journal_path(id);
    if !path.exists() {
        return Err(SessionLifecycleError::NotFound {
            id: format!("{id} (journal)"),
        });
    }
    // Which class does this id belong to? A crash interrupts a provider
    // dispatch at least as often as a tool call, so resolving only tool
    // executions leaves the common case stuck.
    let state = SessionJournal::recovered_state(&path).map_err(|source| {
        SessionLifecycleError::Journal {
            path: path.clone(),
            source,
        }
    })?;
    let item = outstanding_items(&state)
        .into_iter()
        .find(|item| item.tool_execution_id == tool_execution_id)
        .ok_or_else(|| SessionLifecycleError::NotFound {
            id: format!("{id}/{tool_execution_id}"),
        })?;
    if !item.operator_resolvable {
        return Err(SessionLifecycleError::RefusedByAuthority {
            reason: format!(
                "{} {tool_execution_id} is in state {} — this state has no operator-writable receipt; \
                 only the engine can mint one because the receipt must carry the exact dispatch it proved",
                item.kind.as_str(),
                item.reason
            ),
        });
    }

    // Can the journal settle this without asking? If it can, an absent
    // `--as-outcome` is not a gap to paper over with a default. If it cannot,
    // an absent `--as-outcome` must be REFUSED with the consequences spelled
    // out — never silently answered.
    let determined = determined_disposition(&state, &item);
    let authority = match (resolution, determined) {
        (Some(explicit), _) => ResolutionAuthority::Operator(explicit),
        (None, Some(basis)) => ResolutionAuthority::Journal(basis),
        (None, None) => {
            return Err(SessionLifecycleError::RefusedByAuthority {
                reason: unanswerable_reason(&item),
            });
        }
    };
    let effective = match authority {
        ResolutionAuthority::Operator(explicit) => explicit,
        // A repeat-safe tool created no external effect by its own declared
        // contract, so "the effect did not land" is a reading of the contract,
        // not a guess about the world.
        ResolutionAuthority::Journal(_) => OperatorResolution::NotStarted,
    };

    let journal = SessionJournal::open(&path, id.to_owned()).map_err(|source| {
        SessionLifecycleError::Journal {
            path: path.clone(),
            source,
        }
    })?;

    record_tool_interruption(&journal, &state, &path, tool_execution_id, operator_id)?;

    let event = match item.kind {
        ReconcileKind::ToolExecution => {
            let resolution = match effective {
                OperatorResolution::Succeeded => ToolResolution::Succeeded {
                    result: serde_json::Value::Null,
                },
                // `Cancelled` is the honest reason: the operator is asserting
                // the effect never began, and the cause of record is the
                // interruption they are reconciling — not a policy or budget
                // denial that never happened.
                OperatorResolution::NotStarted => ToolResolution::NotStarted {
                    reason: crate::session_journal::ToolNotStartedReason::Cancelled {
                        reason: format!("resolved as not-started by operator {operator_id}"),
                    },
                },
                OperatorResolution::Failed => ToolResolution::Failed {
                    error: format!("resolved as failed by operator {operator_id}"),
                    result: None,
                },
            };
            // Attribute the receipt to whoever actually decided it. A
            // determination the product made from the tool's own effect
            // contract is a RECONCILER result; recording it as an operator
            // assertion would put words in a human's mouth.
            let source = match authority {
                ResolutionAuthority::Operator(_) => ToolResolutionSource::Operator {
                    operator_id: operator_id.to_owned(),
                },
                ResolutionAuthority::Journal(basis) => ToolResolutionSource::Reconciler {
                    reconciler: basis.as_str().to_owned(),
                },
            };
            SessionEvent::ToolExecutionResolved {
                tool_execution_id: tool_execution_id.to_owned(),
                resolution,
                source,
                evidence: serde_json::json!({
                    "source": "wayland-core session reconcile",
                    "determined_by": match authority {
                        ResolutionAuthority::Operator(_) => "operator",
                        ResolutionAuthority::Journal(basis) => basis.as_str(),
                    },
                }),
            }
        }
        // A provider attempt has two operator-writable terminal receipts, and
        // which one applies is decided by the state the crash left, not by the
        // operator: a PREPARED attempt was never dispatched, so it takes
        // `ProviderAttemptNotStarted`; an UNKNOWN attempt was dispatched and
        // its outcome was never observed, so it takes
        // `ProviderAttemptFinished { Cancelled }`. Claiming Succeeded is
        // impossible here and the reducer refuses it — a successful attempt
        // must have a finished stream, which a crashed dispatch does not have.
        //
        // NOTE the fidelity limit, recorded rather than hidden: unlike
        // `ToolExecutionResolved`, the provider-attempt receipts carry no
        // `source` field, so the journal does not record that a HUMAN, rather
        // than the engine, asserted this outcome. Filed as 23B-M3.
        ReconcileKind::ProviderAttempt => provider_attempt_receipt(
            &state,
            id,
            tool_execution_id,
            &format!("resolved as not-started by operator {operator_id}"),
        )?,
        other => {
            return Err(SessionLifecycleError::RefusedByAuthority {
                reason: format!("{} items are not operator-resolvable", other.as_str()),
            });
        }
    };

    journal
        .append(event)
        .map_err(|source| SessionLifecycleError::Journal {
            path: path.clone(),
            source,
        })?;
    Ok(authority)
}

/// Cancel every interrupted turn in a session.
///
/// This is the verb the engine's own refusal names — "resume, reconcile, or
/// cancel it before starting a new message" — and which no command surfaced,
/// which is why a crash-interrupted session was permanently unresumable.
/// Cancellation is refused while unknown effects remain outstanding: the
/// reducer requires every descendant of a turn to be terminal before
/// `TurnCancelled`, and that ordering is correct — an operator must say what
/// happened to an effect before declaring the turn over.
pub fn cancel(manager: &SessionManager, id: &str) -> Result<CancelOutcome> {
    let path = manager.journal_path(id);
    if !path.exists() {
        return Err(SessionLifecycleError::NotFound {
            id: format!("{id} (journal)"),
        });
    }
    let state = SessionJournal::recovered_state(&path).map_err(|source| {
        SessionLifecycleError::Journal {
            path: path.clone(),
            source,
        }
    })?;
    // The reducer refuses `TurnCancelled` while any descendant is nonterminal,
    // across five classes. Refuse here first, with the blocking items named, so
    // the operator gets exit code 5 and an actionable list rather than an
    // opaque "invalid journal state transition" from deep inside the reducer.
    //
    // Hook phases are excluded from the refusal: they are the engine's own
    // bookkeeping about whether a hook's outcome was applied, not an external
    // effect an operator can have an opinion about, and no verb has ever been
    // able to dispose of one. Counting them made a crash mid-tool-round
    // permanently uncancellable, because a `Finished` pre-tool phase is
    // consumed only by a recovery checkpoint that a crashed run never records.
    // They are abandoned below instead.
    let blocking = outstanding_items(&state)
        .into_iter()
        .filter(|item| item.kind != ReconcileKind::HookPhase)
        .collect::<Vec<_>>();
    // Split the blockers by whether anyone has to be ASKED. An item the
    // journal already settles is not a question, and making the operator type
    // a separate command to answer it — with a flag whose default silently
    // asserted `not-started` — is how a one-command recovery became four.
    let (determinable, unanswerable): (Vec<_>, Vec<_>) = blocking
        .into_iter()
        .partition(|item| determined_disposition(&state, item).is_some());
    if !unanswerable.is_empty() {
        return Err(SessionLifecycleError::OutstandingReconcile {
            id: id.to_owned(),
            items: unanswerable,
        });
    }
    let pending = interrupted_turns(&state);
    if pending.is_empty() && determinable.is_empty() {
        return Ok(CancelOutcome::default());
    }
    let journal = SessionJournal::open(&path, id.to_owned()).map_err(|source| {
        SessionLifecycleError::Journal {
            path: path.clone(),
            source,
        }
    })?;
    // Settle every blocker the journal already answers, before the turns that
    // depend on them are closed. Each receipt records WHAT determined it, so a
    // later reader can tell a product determination from a human assertion.
    let mut auto_resolved = Vec::new();
    for item in &determinable {
        let Some(basis) = determined_disposition(&state, item) else {
            continue;
        };
        let event = match item.kind {
            ReconcileKind::ProviderAttempt => provider_attempt_receipt(
                &state,
                id,
                &item.tool_execution_id,
                "resolved as not-started by wayland-core session cancel (journal-determined)",
            )?,
            ReconcileKind::ToolExecution => {
                record_tool_interruption(
                    &journal,
                    &state,
                    &path,
                    &item.tool_execution_id,
                    basis.as_str(),
                )?;
                SessionEvent::ToolExecutionResolved {
                    tool_execution_id: item.tool_execution_id.clone(),
                    resolution: ToolResolution::NotStarted {
                        reason: crate::session_journal::ToolNotStartedReason::Cancelled {
                            reason: format!(
                                "no external effect is possible for this tool ({})",
                                basis.as_str()
                            ),
                        },
                    },
                    source: ToolResolutionSource::Reconciler {
                        reconciler: basis.as_str().to_owned(),
                    },
                    evidence: serde_json::json!({
                        "source": "wayland-core session cancel",
                        "determined_by": basis.as_str(),
                    }),
                }
            }
            _ => continue,
        };
        journal
            .append(event)
            .map_err(|source| SessionLifecycleError::Journal {
                path: path.clone(),
                source,
            })?;
        auto_resolved.push(AutoResolved {
            item: item.clone(),
            determined_by: basis,
        });
    }

    // Abandon the interrupted turns' nonterminal hook phases first. Each takes
    // the one terminal transition its state admits; `AbandonedUnknown` remains
    // nonterminal for continuation, so this closes the turn without ever
    // claiming a lost hook outcome was applied.
    for (hook_phase_id, phase) in &state.hook_phases {
        if !pending.contains(&phase.turn_id) {
            continue;
        }
        let event = match phase.state {
            HookPhaseState::Prepared => Some(SessionEvent::HookPhaseNotStarted {
                hook_phase_id: hook_phase_id.clone(),
                reason: crate::session_journal::HookPhaseNotStartedReason::CancelledBeforeStart,
            }),
            HookPhaseState::Started { .. } | HookPhaseState::Finished { .. } => {
                Some(SessionEvent::HookPhaseAbandonedUnknown {
                    hook_phase_id: hook_phase_id.clone(),
                })
            }
            HookPhaseState::NotStarted { .. }
            | HookPhaseState::NotApplicable
            | HookPhaseState::AbandonedUnknown
            | HookPhaseState::Consumed { .. } => None,
        };
        if let Some(event) = event {
            journal
                .append(event)
                .map_err(|source| SessionLifecycleError::Journal {
                    path: path.clone(),
                    source,
                })?;
        }
    }
    for turn_id in &pending {
        journal
            .append(SessionEvent::TurnCancelled {
                turn_id: turn_id.clone(),
            })
            .map_err(|source| SessionLifecycleError::Journal {
                path: path.clone(),
                source,
            })?;
    }
    Ok(CancelOutcome {
        auto_resolved,
        cancelled_turns: pending,
    })
}

/// Assert that a checkpoint restore destination lies inside the workspace the
/// session was authorised for.
///
/// The checkpoint store records absolute destination paths and writes those
/// bytes back on restore. A recorded path that escapes the root turns rewind
/// into an arbitrary-file-write primitive, so containment is checked before
/// any byte is written. Comparison is on the *lexically normalised* path so a
/// `..` component cannot walk out, and neither path needs to exist.
#[must_use]
pub fn destination_within_root(root: &Path, destination: &Path) -> bool {
    fn normalise(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    out.pop();
                }
                std::path::Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        out
    }
    if !destination.is_absolute() {
        // A relative destination is resolved against the root, so it is
        // contained iff normalising it does not escape.
        return normalise(&root.join(destination)).starts_with(normalise(root));
    }
    normalise(destination).starts_with(normalise(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_types::message::{ContentBlock, Message, Role};

    fn manager(dir: &tempfile::TempDir) -> SessionManager {
        SessionManager::new(dir.path().to_path_buf(), 50)
    }

    fn seed(manager: &SessionManager, text: &str) -> Session {
        let mut session = manager
            .create("anthropic", "test-model", "/tmp", None)
            .unwrap();
        session.messages.push(Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
        ));
        manager.persist_first_message(&session).unwrap();
        manager.save(&session).unwrap();
        session
    }

    #[test]
    fn search_finds_a_seeded_term_and_returns_empty_for_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let session = seed(&manager, "the aardvark ate the mango");

        let hits = search(&manager, "aardvark").unwrap();
        assert_eq!(hits.len(), 1, "seeded term must match exactly one session");
        assert_eq!(hits[0].id, session.id);

        let misses = search(&manager, "zzz-no-such-term").unwrap();
        assert!(
            misses.is_empty(),
            "a term matching nothing is an empty success, not an error"
        );
    }

    #[test]
    fn search_never_returns_a_session_outside_the_managers_directory() {
        let mine = tempfile::tempdir().unwrap();
        let theirs = tempfile::tempdir().unwrap();
        let my_manager = manager(&mine);
        let their_manager = manager(&theirs);
        seed(&their_manager, "someone-elses-secret");

        let hits = search(&my_manager, "someone-elses-secret").unwrap();
        assert!(
            hits.is_empty(),
            "search must be scoped to the caller's session directory"
        );
    }

    #[test]
    fn inspect_reports_metadata_and_an_unbounded_retention_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let session = seed(&manager, "hello");

        let report = inspect(&manager, &session.id).unwrap();
        assert_eq!(report.id, session.id);
        assert_eq!(report.message_count, 1);
        assert_eq!(report.lineage_parent, None);
        assert_eq!(report.retention, RetentionState::Unbounded);
        assert!(report.outstanding_reconcile.is_empty());
    }

    #[test]
    fn inspect_of_an_absent_session_is_not_found_not_an_empty_session() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let error = inspect(&manager, "no-such-session").unwrap_err();
        assert!(
            matches!(error, SessionLifecycleError::NotFound { .. }),
            "expected NotFound, got {error:?}"
        );
    }

    #[test]
    fn a_corrupt_session_file_is_a_structured_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let session = seed(&manager, "hello");
        let path = manager.session_file_path(&session.id);
        std::fs::write(&path, b"{ this is not json").unwrap();

        let error = inspect(&manager, &session.id).unwrap_err();
        match error {
            SessionLifecycleError::CorruptSession { path: named, .. } => {
                assert_eq!(named, path, "the error must name the offending file");
            }
            other => panic!("expected CorruptSession, got {other:?}"),
        }
    }

    #[test]
    fn fork_records_lineage_and_leaves_the_parent_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let parent = seed(&manager, "parent content");
        let before = session_file_digest(&manager, &parent.id).unwrap();

        let outcome = fork(&manager, &parent.id).unwrap();
        assert_eq!(outcome.parent_id, parent.id);
        assert_ne!(outcome.child_id, parent.id);
        assert_eq!(outcome.messages_copied, 1);
        assert_eq!(
            outcome.parent_digest_after, before,
            "forking must leave the parent's bytes untouched"
        );

        let child = inspect(&manager, &outcome.child_id).unwrap();
        assert_eq!(child.lineage_parent, Some(parent.id));
        assert_eq!(child.message_count, 1);
    }

    #[test]
    fn retain_records_a_bound_and_a_past_bound_reports_expired_without_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let session = seed(&manager, "hello");

        let future = Utc::now() + chrono::Duration::hours(1);
        assert_eq!(
            retain(&manager, &session.id, future).unwrap(),
            RetentionState::Retained { until: future }
        );

        let past = Utc::now() - chrono::Duration::hours(1);
        assert_eq!(
            retain(&manager, &session.id, past).unwrap(),
            RetentionState::Expired { until: past }
        );
        assert!(
            manager.session_file_path(&session.id).exists(),
            "an expired session is reported, never silently deleted"
        );
        assert_eq!(inspect(&manager, &session.id).unwrap().message_count, 1);
    }

    #[test]
    fn the_export_envelope_round_trips_and_carries_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let session = seed(&manager, "hello");

        let envelope = export(&manager, &session.id, "wayland-core test (source abc123)").unwrap();
        assert_eq!(envelope.source_session_id, session.id);
        assert_eq!(
            envelope.exporting_build,
            "wayland-core test (source abc123)"
        );
        assert_eq!(envelope.envelope_version, SESSION_EXPORT_ENVELOPE_VERSION);

        let bytes = serde_json::to_vec(&envelope).unwrap();
        let back: SessionExportEnvelope = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.source_session_id, envelope.source_session_id);
        assert_eq!(back.messages, envelope.messages);
    }

    #[test]
    fn a_run_time_nonce_planted_in_a_session_is_absent_from_the_export() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        // Generated at run time: no shape-matching filter could target it.
        let nonce = format!("nonce-{}", uuid::Uuid::new_v4().simple());
        let session = seed(&manager, &format!("secret value {nonce} end"));

        // Prove the nonce really is in the session before asserting absence —
        // an absence that was always absent proves nothing.
        let stored = std::fs::read_to_string(manager.session_file_path(&session.id)).unwrap();
        assert!(
            stored.contains(&nonce),
            "the nonce must be present in the stored session before the export is tested"
        );

        let envelope = export(&manager, &session.id, "test").unwrap();
        let bytes = serde_json::to_string(&envelope).unwrap();
        assert!(
            !bytes.contains(&nonce),
            "the export envelope must not carry session free text"
        );
    }

    #[test]
    fn reconcile_list_is_empty_for_a_session_with_no_journal() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let session = seed(&manager, "hello");
        assert!(reconcile_list(&manager, &session.id).unwrap().is_empty());
    }

    #[test]
    fn cancel_on_a_session_with_no_interrupted_turn_is_a_successful_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let active = manager
            .create_for_run("anthropic", "test-model", "/tmp", None)
            .unwrap();
        let cancelled = cancel(&manager, &active.session.id).unwrap();
        assert!(cancelled.cancelled_turns.is_empty());
        assert!(cancelled.auto_resolved.is_empty());
    }

    #[test]
    fn cancel_terminates_an_interrupted_turn_and_the_state_is_durable() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let active = manager
            .create_for_run("anthropic", "test-model", "/tmp", None)
            .unwrap();
        let id = active.session.id.clone();
        active
            .journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn-1".to_owned(),
                user_message: "do a thing".to_owned(),
            })
            .unwrap();
        // Drop the writer lease, exactly as a crash would.
        drop(active);

        let path = manager.journal_path(&id);
        let before = SessionJournal::recovered_state(&path).unwrap();
        assert_eq!(
            interrupted_turns(&before),
            vec!["turn-1".to_owned()],
            "the fixture must genuinely have an interrupted turn"
        );

        let cancelled = cancel(&manager, &id).unwrap();
        assert_eq!(cancelled.cancelled_turns, vec!["turn-1".to_owned()]);

        // Re-read from disk: the disposition must survive a restart.
        let after = SessionJournal::recovered_state(&path).unwrap();
        assert!(
            interrupted_turns(&after).is_empty(),
            "a cancelled turn must not be presented as interrupted again"
        );
        assert_eq!(
            after.turns.get("turn-1").unwrap().completion,
            Some(TurnCompletion::Cancelled)
        );
    }

    #[test]
    fn retry_of_an_unknown_turn_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(&dir);
        let active = manager
            .create_for_run("anthropic", "test-model", "/tmp", None)
            .unwrap();
        let id = active.session.id.clone();
        drop(active);
        let error = retry(&manager, &id, "turn-does-not-exist").unwrap_err();
        assert!(matches!(error, SessionLifecycleError::NotFound { .. }));
    }

    #[test]
    fn a_destination_escaping_the_workspace_root_is_refused() {
        let root = Path::new("/workspace/project");
        assert!(destination_within_root(
            root,
            Path::new("/workspace/project/src/main.rs")
        ));
        assert!(!destination_within_root(
            root,
            Path::new("/workspace/project/../../etc/passwd")
        ));
        assert!(!destination_within_root(root, Path::new("/etc/passwd")));
        assert!(
            !destination_within_root(root, Path::new("/workspace/project-evil/x")),
            "a sibling sharing a name prefix must not count as contained"
        );
    }
}
