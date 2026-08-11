//! `wayland-core session` — the operator surface for F23-02 (Phase 23B).
//!
//! Success Criterion 2 is a list of verbs a *human* performs on a session. This
//! module is where a human performs them. Each operation prints one stable,
//! greppable token plus the identifier it acted on to **STDOUT**, so a driver
//! script can observe the outcome with a plain redirect and without parsing
//! prose.
//!
//! ## Exit-code map
//!
//! These codes are contract, asserted by
//! `crates/wcore-cli/tests/session_operator_lifecycle.rs`:
//!
//! | Code | Meaning |
//! |-----:|---------|
//! | `0`  | success |
//! | `1`  | failure the operator cannot classify (corrupt store, i/o, journal) |
//! | `3`  | the named session, turn or checkpoint was not found |
//! | `4`  | refused by session authority (e.g. a destination outside the workspace root) |
//! | `5`  | the operation is blocked by outstanding reconcile items |
//!
//! ## Divergence from `--list-sessions`
//!
//! The pre-existing root `--list-sessions` flag prints its table to STDERR
//! (`main.rs:1510-1525`). This subcommand prints to STDOUT. The divergence is
//! deliberate: a driver must be able to capture the result with `> log`, and
//! changing the older flag would break anything already parsing its stderr.
//! Recorded as backlog item 23B-M1.

use std::path::PathBuf;
use std::process::ExitCode;

use chrono::{DateTime, Duration, Utc};
use clap::{Args, Subcommand};

use wcore_agent::session::SessionManager;
use wcore_agent::session_lifecycle::{
    OperatorResolution, ReconcileItem, ResolutionAuthority, RetryOutcome, SessionLifecycleError,
    cancel, export, fork, inspect, list, reconcile_list, reconcile_resolve, retain, retry, search,
    session_file_digest,
};

use crate::tui::checkpoint::{CheckpointError, CheckpointId, CheckpointStore};

/// Exit code: the named entity was not found.
pub const EXIT_NOT_FOUND: u8 = 3;
/// Exit code: refused by session authority.
pub const EXIT_REFUSED: u8 = 4;
/// Exit code: blocked by outstanding reconcile items.
pub const EXIT_OUTSTANDING_RECONCILE: u8 = 5;

#[derive(Args, Debug)]
pub struct SessionArgs {
    /// Directory holding saved sessions. Defaults to the resolved config's
    /// session directory, or `$WAYLAND_HOME/sessions` when no config resolves.
    #[arg(long, global = true)]
    pub dir: Option<PathBuf>,

    /// Workspace root that checkpoint capture and restore are confined to.
    /// Defaults to the current directory.
    #[arg(long, global = true)]
    pub workspace: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: SessionCmd,
}

#[derive(Subcommand, Debug)]
pub enum SessionCmd {
    /// List saved sessions (STDOUT, unlike the root `--list-sessions` flag).
    List,
    /// Full-text search across saved sessions.
    Search {
        /// Term to look for in stored message content.
        query: String,
    },
    /// Show one session's metadata, lineage, retention and reconcile state.
    Show {
        /// Session id.
        id: String,
    },
    /// Capture a workspace checkpoint over the named files.
    Checkpoint {
        /// Free-form label recorded with the checkpoint.
        #[arg(long, default_value = "session checkpoint")]
        label: String,
        /// Files to snapshot. A file that does not exist is captured as
        /// absent, and restore deletes it back to absence.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Restore a previously captured checkpoint.
    Rewind {
        /// Checkpoint id, as printed by `checkpoint`.
        id: String,
    },
    /// Re-run one turn. Approval is re-derived; an expired one is refused.
    Retry {
        /// Session id.
        id: String,
        /// Turn id, as shown by `show`.
        turn_id: String,
    },
    /// Fork a session, leaving the parent byte-identical.
    Fork {
        /// Session id.
        id: String,
    },
    /// Write the redacted export envelope.
    Export {
        /// Session id.
        id: String,
        /// Destination file. Defaults to STDOUT when omitted.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Set a retain-until bound and report the resulting retention state.
    Retain {
        /// Session id.
        id: String,
        /// Days from now. A negative value sets a bound in the past, which
        /// reports the session as expired without deleting it.
        #[arg(long, allow_negative_numbers = true)]
        days: i64,
    },
    /// List or resolve outstanding unknown-effect items.
    Reconcile {
        /// Session id.
        id: String,
        /// Resolve this tool execution instead of listing.
        #[arg(long)]
        resolve: Option<String>,
        /// Disposition for `--resolve`: what actually happened to the
        /// interrupted effect.
        ///
        /// Deliberately has NO default. It used to default to `not-started`,
        /// which answered "did the effect land?" — the one thing the operator
        /// is reporting they do not know — on their behalf, and recorded the
        /// answer durably. Omit it and the product resolves the item itself
        /// where the journal determines the outcome, and refuses with the
        /// consequence of each choice where it does not.
        #[arg(long, value_enum)]
        as_outcome: Option<ResolveAs>,
        /// Operator identity recorded in the journal alongside the resolution.
        #[arg(long, default_value = "cli-operator")]
        operator: String,
    },
    /// Cancel every interrupted turn so the session becomes resumable.
    ///
    /// This is the verb the engine's own refusal names — "resume, reconcile,
    /// or cancel it before starting a new message" — and which no command
    /// surfaced before Phase 23B (live Windows UAT defect D2).
    Cancel {
        /// Session id.
        id: String,
    },
}

#[derive(Copy, Clone, Debug, clap::ValueEnum)]
pub enum ResolveAs {
    /// The effect never began.
    NotStarted,
    /// The effect landed successfully.
    Succeeded,
    /// The effect landed and failed.
    Failed,
}

impl From<ResolveAs> for OperatorResolution {
    fn from(value: ResolveAs) -> Self {
        match value {
            ResolveAs::NotStarted => OperatorResolution::NotStarted,
            ResolveAs::Succeeded => OperatorResolution::Succeeded,
            ResolveAs::Failed => OperatorResolution::Failed,
        }
    }
}

/// Map a lifecycle error onto the documented exit code, reporting the message
/// on stderr so stdout stays machine-parseable.
///
/// A refusal that names no remedy is the defect this surface was built to
/// close and then reproduced: `cancel` exited 5 with ZERO bytes on stdout and
/// a single stderr line counting items the user had no command to clear. Every
/// refusal that HAS a remedy now prints it — on stdout, where the operator's
/// `> log` redirect can see it, in the same shape the missing-API-key error
/// already uses: the exact command, with the real identifier substituted.
fn report(error: &SessionLifecycleError) -> ExitCode {
    if let SessionLifecycleError::OutstandingReconcile { id, items } = error {
        print_reconcile_remedy(id, items);
    }
    eprintln!("wayland-core session: {error}");
    match error {
        SessionLifecycleError::NotFound { .. } => ExitCode::from(EXIT_NOT_FOUND),
        SessionLifecycleError::RefusedByAuthority { .. } => ExitCode::from(EXIT_REFUSED),
        SessionLifecycleError::OutstandingReconcile { .. } => {
            ExitCode::from(EXIT_OUTSTANDING_RECONCILE)
        }
        _ => ExitCode::FAILURE,
    }
}

/// Print, on STDOUT, the exact command that clears each blocking item.
fn print_reconcile_remedy(id: &str, items: &[ReconcileItem]) {
    println!(
        "F23_SESSION=cancel_blocked id={id} outstanding={}",
        items.len()
    );
    for item in items {
        println!(
            "F23_SESSION=cancel_blocked_item id={id} kind={} ref={} tool={} resolvable={}",
            item.kind.as_str(),
            item.tool_execution_id,
            item.tool,
            item.operator_resolvable
        );
    }
    println!();
    println!(
        "Cancel is blocked: {} interrupted effect(s) have no recorded outcome, and a turn \
         cannot be declared over while the product would be guessing about what it did.",
        items.len()
    );
    for item in items {
        println!();
        if !item.operator_resolvable {
            println!(
                "  {} {} ({}) is in state {} — no operator-writable receipt exists for that \
                 state, so only a live engine can close it. Resume the session instead of \
                 cancelling it.",
                item.kind.as_str(),
                item.tool,
                item.tool_execution_id,
                item.reason
            );
            continue;
        }
        println!(
            "  `{}` was interrupted in state {} and only you can say what happened to it. \
             Run ONE of:",
            item.tool, item.reason
        );
        for (outcome, consequence) in [
            (
                "not-started",
                "the work never began; nothing it would have changed was changed",
            ),
            (
                "succeeded",
                "the work completed; the turn records success with a null result, and no \
                 output is invented",
            ),
            (
                "failed",
                "the work began and failed; the turn records the failure",
            ),
        ] {
            println!(
                "    wayland-core session reconcile {id} --resolve {} --as-outcome {outcome}",
                item.tool_execution_id
            );
            println!("        {consequence}");
        }
    }
    println!();
    println!("Then re-run: wayland-core session cancel {id}");
}

/// Resolve the session directory without requiring a provider API key.
///
/// Listing and searching sessions must work for a first-run user with no
/// credentials — the same contract the root `--list-sessions` flag already
/// honours by resolving with a fallback to defaults.
fn resolve_manager(args: &SessionArgs) -> SessionManager {
    if let Some(dir) = args.dir.clone() {
        return SessionManager::new(dir, 50);
    }
    let config = wcore_config::config::Config::resolve(&wcore_config::config::CliArgs {
        provider: None,
        api_key: None,
        base_url: None,
        model: None,
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: None,
    })
    .unwrap_or_default();
    SessionManager::new(
        config.session.directory.clone().into(),
        config.session.max_sessions,
    )
}

fn workspace_root(args: &SessionArgs) -> PathBuf {
    args.workspace
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn checkpoint_store(args: &SessionArgs, manager: &SessionManager) -> CheckpointStore {
    // The same store the TUI `/rewind` handler uses, rooted beside the session
    // directory, so a checkpoint taken in the TUI is restorable from the shell
    // and the reverse.
    CheckpointStore::new(
        manager.directory().join("checkpoints"),
        workspace_root(args),
    )
}

/// Entry point. Synchronous: no verb here performs a provider call.
pub fn run(args: SessionArgs) -> anyhow::Result<ExitCode> {
    let manager = resolve_manager(&args);
    Ok(match &args.cmd {
        SessionCmd::List => match list(&manager) {
            Ok(sessions) => {
                for meta in &sessions {
                    println!(
                        "F23_SESSION=list id={} messages={} model={} updated={}",
                        meta.id,
                        meta.message_count,
                        meta.model,
                        meta.updated_at.to_rfc3339()
                    );
                }
                println!("F23_SESSION=list_total count={}", sessions.len());
                ExitCode::SUCCESS
            }
            Err(error) => report(&error),
        },

        SessionCmd::Search { query } => match search(&manager, query) {
            Ok(hits) => {
                for hit in &hits {
                    println!(
                        "F23_SESSION=search id={} matches={}",
                        hit.id, hit.match_count
                    );
                }
                // A query matching nothing is a successful empty result. The
                // total line is always emitted so a driver can distinguish
                // "ran and found nothing" from "did not run".
                println!(
                    "F23_SESSION=search_total query={} count={}",
                    query,
                    hits.len()
                );
                ExitCode::SUCCESS
            }
            Err(error) => report(&error),
        },

        SessionCmd::Show { id } => match inspect(&manager, id) {
            Ok(report_) => {
                println!(
                    "F23_SESSION=show id={} messages={} turns={} interrupted={} parent={} retention={} reconcile_items={}",
                    report_.id,
                    report_.message_count,
                    report_
                        .journal_turn_count
                        .map_or_else(|| "none".to_owned(), |n| n.to_string()),
                    report_.interrupted_turn_count,
                    report_.lineage_parent.as_deref().unwrap_or("none"),
                    serde_json::to_string(&report_.retention).unwrap_or_default(),
                    report_.outstanding_reconcile.len()
                );
                for item in &report_.outstanding_reconcile {
                    println!(
                        "F23_SESSION=show_reconcile_item id={} kind={} ref={} tool={} turn={} reason={} resolvable={}",
                        report_.id,
                        item.kind.as_str(),
                        item.tool_execution_id,
                        item.tool,
                        item.turn_id,
                        item.reason,
                        item.operator_resolvable
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => report(&error),
        },

        SessionCmd::Checkpoint { label, files } => {
            let store = checkpoint_store(&args, &manager);
            match store.capture(label.clone(), files.iter()) {
                Ok(id) => {
                    println!("F23_SESSION=checkpoint id={id} files={}", files.len());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("wayland-core session: {error}");
                    ExitCode::FAILURE
                }
            }
        }

        SessionCmd::Rewind { id } => {
            let store = checkpoint_store(&args, &manager);
            match store.restore(&CheckpointId(id.clone())) {
                Ok(()) => {
                    println!("F23_SESSION=rewind id={id} restored=true");
                    ExitCode::SUCCESS
                }
                Err(CheckpointError::NotFound(missing)) => {
                    eprintln!("wayland-core session: no checkpoint with id `{missing}`");
                    ExitCode::from(EXIT_NOT_FOUND)
                }
                // A recorded destination outside the workspace root is an
                // authority refusal, not an i/o failure, and nothing was
                // written when it is raised.
                Err(error @ CheckpointError::DestinationEscapesRoot { .. }) => {
                    eprintln!("wayland-core session: {error}");
                    ExitCode::from(EXIT_REFUSED)
                }
                Err(error) => {
                    eprintln!("wayland-core session: {error}");
                    ExitCode::FAILURE
                }
            }
        }

        SessionCmd::Retry { id, turn_id } => match retry(&manager, id, turn_id) {
            Ok(RetryOutcome::Admitted {
                turn_id,
                forked_into,
                reapproved,
            }) => {
                println!(
                    "F23_SESSION=retry id={id} turn={turn_id} forked_into={forked_into} reapproved={}",
                    reapproved.len()
                );
                ExitCode::SUCCESS
            }
            Ok(RetryOutcome::RefusedApprovalExpired {
                turn_id,
                approval_ids,
            }) => {
                println!(
                    "F23_SESSION=retry id={id} turn={turn_id} refused=approval_expired approvals={}",
                    approval_ids.len()
                );
                eprintln!(
                    "wayland-core session: retry refused — the recorded approval is no longer valid under current session authority (ApprovalExpired)"
                );
                ExitCode::from(EXIT_REFUSED)
            }
            Err(error) => report(&error),
        },

        SessionCmd::Fork { id } => {
            // Digest before, so the caller can prove non-mutation without a
            // second command.
            let before = session_file_digest(&manager, id).ok();
            match fork(&manager, id) {
                Ok(outcome) => {
                    println!(
                        "F23_SESSION=fork id={} child={} messages={} parent_unchanged={}",
                        outcome.parent_id,
                        outcome.child_id,
                        outcome.messages_copied,
                        before
                            .as_deref()
                            .is_some_and(|d| d == outcome.parent_digest_after)
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => report(&error),
            }
        }

        SessionCmd::Export { id, out } => {
            // Same identity string `--build-info` prints, so an export can be
            // tied to the exact binary that produced it.
            let build = format!(
                "wayland-core {} (source {})",
                env!("CARGO_PKG_VERSION"),
                env!("WAYLAND_SOURCE_SHA")
            );
            match export(&manager, id, &build) {
                Ok(envelope) => {
                    let json = match serde_json::to_string_pretty(&envelope) {
                        Ok(json) => json,
                        Err(error) => {
                            eprintln!("wayland-core session: {error}");
                            return Ok(ExitCode::FAILURE);
                        }
                    };
                    let bytes = json.len();
                    match out {
                        Some(path) => {
                            if let Err(error) = std::fs::write(path, json.as_bytes()) {
                                eprintln!(
                                    "wayland-core session: writing {}: {error}",
                                    path.display()
                                );
                                return Ok(ExitCode::FAILURE);
                            }
                            println!(
                                "F23_SESSION=export id={id} path={} bytes={bytes}",
                                path.display()
                            );
                        }
                        None => {
                            println!("{json}");
                            println!("F23_SESSION=export id={id} path=- bytes={bytes}");
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => report(&error),
            }
        }

        SessionCmd::Retain { id, days } => {
            let until: DateTime<Utc> = Utc::now() + Duration::days(*days);
            match retain(&manager, id, until) {
                Ok(state) => {
                    println!(
                        "F23_SESSION=retain id={id} state={}",
                        serde_json::to_string(&state).unwrap_or_default()
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => report(&error),
            }
        }

        SessionCmd::Reconcile {
            id,
            resolve,
            as_outcome,
            operator,
        } => match resolve {
            None => match reconcile_list(&manager, id) {
                Ok(items) => {
                    for item in &items {
                        println!(
                            "F23_SESSION=reconcile_item id={id} kind={} ref={} tool={} turn={} reason={} resolvable={}",
                            item.kind.as_str(),
                            item.tool_execution_id,
                            item.tool,
                            item.turn_id,
                            item.reason,
                            item.operator_resolvable
                        );
                    }
                    println!("F23_SESSION=reconcile id={id} outstanding={}", items.len());
                    ExitCode::SUCCESS
                }
                Err(error) => report(&error),
            },
            Some(tool_execution_id) => match reconcile_resolve(
                &manager,
                id,
                tool_execution_id,
                as_outcome.map(|outcome| outcome.into()),
                operator,
            ) {
                Ok(authority) => {
                    let by = match authority {
                        ResolutionAuthority::Operator(_) => "operator".to_owned(),
                        ResolutionAuthority::Journal(basis) => basis.as_str().to_owned(),
                    };
                    println!(
                        "F23_SESSION=reconcile_resolved id={id} tool_execution={tool_execution_id} operator={operator} determined_by={by}"
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => report(&error),
            },
        },

        SessionCmd::Cancel { id } => match cancel(&manager, id) {
            Ok(outcome) => {
                // Report every receipt written on the operator's behalf. An
                // automatic disposition they cannot see is one they cannot
                // audit or disagree with.
                for resolved in &outcome.auto_resolved {
                    println!(
                        "F23_SESSION=cancel_auto_resolved id={id} kind={} ref={} tool={} determined_by={}",
                        resolved.item.kind.as_str(),
                        resolved.item.tool_execution_id,
                        resolved.item.tool,
                        resolved.determined_by.as_str()
                    );
                }
                for turn_id in &outcome.cancelled_turns {
                    println!("F23_SESSION=cancel_turn id={id} turn={turn_id}");
                }
                println!(
                    "F23_SESSION=cancel id={id} cancelled={} auto_resolved={}",
                    outcome.cancelled_turns.len(),
                    outcome.auto_resolved.len()
                );
                if !outcome.cancelled_turns.is_empty() {
                    println!();
                    println!(
                        "The session is resumable again. Continue it with:\n  \
                         wayland-core --resume {id} \"your next message\""
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => report(&error),
        },
    })
}
