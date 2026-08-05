//! F22-C1 — the `/goal` slash command: the TUI's Goal CONTROL surface.
//!
//! ## Why this module exists
//!
//! The five `ProtocolCommand::Goal*` variants and the engine-side handler
//! (`TuiEngine::issue_goal_control`) both landed before anything could reach
//! them: the handler had ZERO call sites, so a user sitting at the terminal
//! could observe a durable Goal in the status line and could not move one.
//! This module is the parse half of closing that — it turns a composer line
//! into one of the five typed commands, or into the text to show instead.
//!
//! ## Why parsing lives here and not in the router
//!
//! `Router::dispatch_command` is a 700-line match in a file every lane edits,
//! and a parser buried in one of its arms is only reachable by standing up a
//! `Router`. [`parse_goal_line`] is a pure function of
//! `(line, session id, held projections, request id)` returning what to do, so
//! every refusal below — including the ones this surface decides locally,
//! which Core never sees — is directly testable, negative controls included.
//!
//! ## What this surface decides, and what it refuses to decide
//!
//! It decides only what it can decide from the line and from state the event
//! stream already wrote: is the verb known, is the arity right, is a cursor
//! held for this Goal. Everything else — does the Goal exist, is the version
//! current, is the session live, is the cursor stale — is Core's, and reaching
//! Core is the point. This module NEVER synthesizes an acceptance and never
//! invents a cursor: `advance` and `cancel` carry the cursor from the last
//! projection the user actually saw, and when no projection is held the answer
//! is "run `/goal resync` first", not a zero cursor that Core would then have
//! to reject.

use std::collections::BTreeMap;

use wcore_protocol::commands::{
    GoalAdvanceCommand, GoalCancelCommand, GoalDeclareTaskCommand, GoalOpenCommand,
    GoalResyncCommand, ProtocolCommand,
};
use wcore_protocol::goal::{GOAL_PROTOCOL_VERSION, GoalProjection};
use wcore_types::goal::GoalStrategy;

/// The token ceiling `/goal open` REQUESTS. Core intersects it with the
/// session's own envelope and records the intersection, so this is a request
/// and not an authority — the same posture `GoalOpenCommand::max_tokens`
/// documents on the wire. It matches the CLI's `--parent-max-tokens` default
/// so a Goal opened from the TUI and one opened from `wayland-core goal open`
/// resolve identically.
const TUI_GOAL_MAX_TOKENS: u64 = 1_000_000;

/// What the router should do with a `/goal …` line.
#[derive(Debug, PartialEq)]
pub enum GoalDispatch {
    /// Issue this typed command against the live engine. `note` is the line to
    /// show immediately, so the user sees that the command left the terminal —
    /// the ANSWER arrives asynchronously through the protocol event stream
    /// (`goal_snapshot` or `goal_control_refused`) and is rendered from there.
    Issue {
        command: Box<ProtocolCommand>,
        note: String,
    },
    /// Render this text as a system turn and issue nothing. Usage, the local
    /// listing, and the refusals this surface can settle without Core.
    Say(String),
}

/// The `/goal` usage block — one line per verb, consequence first.
#[must_use]
pub fn usage() -> String {
    "Usage:\n\
     \x20 /goal                             list durable Goals in this session\n\
     \x20 /goal resync [<goal-id>]          pull a fresh snapshot (all Goals when omitted)\n\
     \x20 /goal open <id> <strategy> <n> <objective…>\n\
     \x20                                   authorize a Goal for n iterations\n\
     \x20 /goal task <goal-id> <task-id> [--after <id>[,<id>…]]\n\
     \x20                                   declare one task in the Goal's ledger\n\
     \x20 /goal advance <goal-id>           consume one authorized iteration\n\
     \x20 /goal cancel <goal-id>            terminate the Goal as `cancelled`\n\
     \n\
     Strategies: direct · forge-flows · fleet · council · anvil\n\
     `advance` and `cancel` bind to the state you last saw — run `/goal resync <id>` \
     if the Goal is not listed above."
        .to_string()
}

/// Parse `--strategy`-style spelling into the canonical loop owner.
///
/// An exhaustive match with no `_` arm on the OUTPUT side is not possible here
/// (the input is free text), so the accepted set is written out and a sixth
/// [`GoalStrategy`] would be caught by [`strategy_names`]'s completeness test
/// below rather than silently becoming untypeable from the TUI.
fn parse_strategy(raw: &str) -> Option<GoalStrategy> {
    match raw.to_ascii_lowercase().replace('_', "-").as_str() {
        "direct" => Some(GoalStrategy::Direct),
        "forge-flows" | "forgeflows" => Some(GoalStrategy::ForgeFlows),
        "fleet" => Some(GoalStrategy::Fleet),
        "council" => Some(GoalStrategy::Council),
        "anvil" => Some(GoalStrategy::Anvil),
        _ => None,
    }
}

/// The canonical spelling of every strategy, for the error message and for the
/// completeness test that keeps this module honest when a sixth is added.
fn strategy_names() -> Vec<&'static str> {
    GoalStrategy::ALL
        .iter()
        .map(|strategy| match strategy {
            GoalStrategy::Direct => "direct",
            GoalStrategy::ForgeFlows => "forge-flows",
            GoalStrategy::Fleet => "fleet",
            GoalStrategy::Council => "council",
            GoalStrategy::Anvil => "anvil",
        })
        .collect()
}

/// Render the Goals this session currently holds a projection for.
fn render_goals(goals: &BTreeMap<String, GoalProjection>) -> String {
    if goals.is_empty() {
        return format!(
            "No durable Goal has been reported in this session.\n\n{}",
            usage()
        );
    }
    let mut out = String::from("Durable Goals in this session:\n");
    for goal in goals.values() {
        let lifecycle = serde_json::to_value(&goal.lifecycle)
            .ok()
            .and_then(|value| match value {
                serde_json::Value::String(s) => Some(s),
                serde_json::Value::Object(map) => map.keys().next().cloned(),
                _ => None,
            })
            .unwrap_or_else(|| "unknown".to_string());
        let ceiling = goal
            .iteration_ceiling
            .map_or_else(|| "manual".to_string(), |n| n.to_string());
        out.push_str(&format!(
            "  {}  {}  iterations {}/{}  — {}\n",
            goal.goal_id, lifecycle, goal.iterations_started, ceiling, goal.objective
        ));
    }
    out.push('\n');
    out.push_str(&usage());
    out
}

/// Parse a `/goal …` composer line.
///
/// `session_id` is the LIVE durable session id, `None` when the engine has no
/// durable session at all. `goals` is the projection map the protocol bridge
/// wrote — the only place a cursor for `advance`/`cancel` may come from.
/// `request_id` is supplied by the caller (rather than minted here) so this
/// stays a pure function and its tests can assert the exact command built.
#[must_use]
pub fn parse_goal_line(
    line: &str,
    session_id: Option<&str>,
    goals: &BTreeMap<String, GoalProjection>,
    request_id: &str,
) -> GoalDispatch {
    let mut tokens = line.split_whitespace();
    // The `/goal` word itself.
    let _ = tokens.next();
    let verb = tokens.next();

    // Bare `/goal` is a listing — it needs no live session, because it reads
    // only what the event stream already delivered.
    let Some(verb) = verb else {
        return GoalDispatch::Say(render_goals(goals));
    };

    // Every remaining verb is a CONTROL command, and control needs a durable
    // session to name. Saying so here (rather than sending a command with an
    // empty `session_id` that Core answers with `session_not_found`) names the
    // real cause: durable sessions are off, not "your Goal is missing".
    let Some(session_id) = session_id else {
        return GoalDispatch::Say(
            "This session has no durable journal, so it can hold no Goal. Goals need durable \
             sessions enabled (`[sessions] enabled = true`) and confidential credential storage."
                .to_string(),
        );
    };

    match verb.to_ascii_lowercase().as_str() {
        "resync" => {
            let goal_id = tokens.next().map(str::to_string);
            let note = match &goal_id {
                Some(id) => format!("Resyncing Goal `{id}`…"),
                None => "Resyncing every Goal in this session…".to_string(),
            };
            GoalDispatch::Issue {
                command: Box::new(ProtocolCommand::GoalResync(GoalResyncCommand {
                    goal_version: GOAL_PROTOCOL_VERSION,
                    request_id: request_id.to_string(),
                    session_id: session_id.to_string(),
                    goal_id,
                })),
                note,
            }
        }
        "open" => {
            let (Some(goal_id), Some(strategy_raw), Some(iterations_raw)) =
                (tokens.next(), tokens.next(), tokens.next())
            else {
                return GoalDispatch::Say(format!(
                    "Usage: /goal open <goal-id> <strategy> <iterations> <objective…>\n\n{}",
                    usage()
                ));
            };
            let Some(strategy) = parse_strategy(strategy_raw) else {
                return GoalDispatch::Say(format!(
                    "Unknown strategy `{strategy_raw}`. One of: {}.",
                    strategy_names().join(" · ")
                ));
            };
            let Ok(iterations) = iterations_raw.parse::<u32>() else {
                return GoalDispatch::Say(format!(
                    "`{iterations_raw}` is not a loop bound. Give a whole number of iterations, \
                     at least 1."
                ));
            };
            if iterations == 0 {
                return GoalDispatch::Say(
                    "A Goal must authorize at least 1 iteration. There is no spelling for \
                     unbounded."
                        .to_string(),
                );
            }
            let objective = tokens.collect::<Vec<_>>().join(" ");
            if objective.trim().is_empty() {
                return GoalDispatch::Say(
                    "Give the Goal an objective — the durable record of what it was authorized \
                     to do."
                        .to_string(),
                );
            }
            GoalDispatch::Issue {
                command: Box::new(ProtocolCommand::GoalOpen(GoalOpenCommand {
                    goal_version: GOAL_PROTOCOL_VERSION,
                    request_id: request_id.to_string(),
                    session_id: session_id.to_string(),
                    goal_id: goal_id.to_string(),
                    objective,
                    iterations,
                    strategy,
                    max_tokens: TUI_GOAL_MAX_TOKENS,
                })),
                note: format!("Opening Goal `{goal_id}` ({strategy_raw}, {iterations})…"),
            }
        }
        "task" => {
            let (Some(goal_id), Some(task_id)) = (tokens.next(), tokens.next()) else {
                return GoalDispatch::Say(format!(
                    "Usage: /goal task <goal-id> <task-id> [--after <id>[,<id>…]]\n\n{}",
                    usage()
                ));
            };
            let mut depends_on = std::collections::BTreeSet::new();
            if let Some(flag) = tokens.next() {
                if flag != "--after" {
                    return GoalDispatch::Say(format!(
                        "Unknown option `{flag}`. Only `--after <id>[,<id>…]` is accepted here."
                    ));
                }
                let Some(list) = tokens.next() else {
                    return GoalDispatch::Say(
                        "`--after` needs at least one task id it depends on.".to_string(),
                    );
                };
                for dep in list.split(',').map(str::trim).filter(|d| !d.is_empty()) {
                    depends_on.insert(dep.to_string());
                }
                if depends_on.is_empty() {
                    return GoalDispatch::Say(
                        "`--after` needs at least one task id it depends on.".to_string(),
                    );
                }
            }
            GoalDispatch::Issue {
                command: Box::new(ProtocolCommand::GoalDeclareTask(GoalDeclareTaskCommand {
                    goal_version: GOAL_PROTOCOL_VERSION,
                    request_id: request_id.to_string(),
                    session_id: session_id.to_string(),
                    goal_id: goal_id.to_string(),
                    task_id: task_id.to_string(),
                    depends_on,
                    // Absent means Core derives the same default the CLI does.
                    // Letting the terminal invent one would be a second
                    // dedup vocabulary.
                    idempotency_key: None,
                })),
                note: format!("Declaring task `{task_id}` on Goal `{goal_id}`…"),
            }
        }
        verb @ ("advance" | "cancel") => {
            let Some(goal_id) = tokens.next() else {
                return GoalDispatch::Say(format!("Usage: /goal {verb} <goal-id>"));
            };
            // The cursor binds the decision to the state the operator actually
            // inspected. It comes from the projection the event stream
            // delivered, never from anything this surface derives — so a Goal
            // the terminal has not seen cannot be advanced or cancelled blind.
            let Some(projection) = goals.get(goal_id) else {
                return GoalDispatch::Say(format!(
                    "No projection is held for Goal `{goal_id}`, so there is no state to bind \
                     this {verb} to. Run `/goal resync {goal_id}` first."
                ));
            };
            let cursor = projection.cursor.clone();
            let command = if verb == "advance" {
                ProtocolCommand::GoalAdvance(GoalAdvanceCommand {
                    goal_version: GOAL_PROTOCOL_VERSION,
                    request_id: request_id.to_string(),
                    session_id: session_id.to_string(),
                    goal_id: goal_id.to_string(),
                    cursor,
                })
            } else {
                ProtocolCommand::GoalCancel(GoalCancelCommand {
                    goal_version: GOAL_PROTOCOL_VERSION,
                    request_id: request_id.to_string(),
                    session_id: session_id.to_string(),
                    goal_id: goal_id.to_string(),
                    cursor,
                })
            };
            GoalDispatch::Issue {
                command: Box::new(command),
                note: format!("Sending {verb} for Goal `{goal_id}`…"),
            }
        }
        other => GoalDispatch::Say(format!("Unknown `/goal` verb `{other}`.\n\n{}", usage())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_protocol::events::RecoveryCursor;
    use wcore_protocol::goal::{GoalAuthorityWire, GoalLifecycleWire};

    fn projection(goal_id: &str, sequence: u64, digest: &str) -> GoalProjection {
        GoalProjection {
            goal_id: goal_id.to_string(),
            objective: "ship the thing".to_string(),
            authority: GoalAuthorityWire {
                effective_limits: BTreeMap::new(),
                strategy: GoalStrategy::Direct,
                loop_policy: wcore_types::goal::LoopPolicy::Fixed { iterations: 3 },
                parent_envelope_digest: "parent".to_string(),
                snapshot_digest: "snapshot".to_string(),
            },
            lifecycle: GoalLifecycleWire::Opened,
            iterations_started: 0,
            iteration_ceiling: Some(3),
            resume_count: 0,
            opened_at_unix_ms: 0,
            cursor: RecoveryCursor {
                journal_sequence: Some(sequence),
                journal_digest: digest.to_string(),
            },
            tasks: Vec::new(),
            loop_owner: None,
            loop_owner_epochs: 0,
        }
    }

    fn held(goal_id: &str) -> BTreeMap<String, GoalProjection> {
        let mut map = BTreeMap::new();
        map.insert(goal_id.to_string(), projection(goal_id, 12, "digest-12"));
        map
    }

    fn say(dispatch: &GoalDispatch) -> &str {
        match dispatch {
            GoalDispatch::Say(text) => text.as_str(),
            GoalDispatch::Issue { .. } => panic!("expected a local answer, got an issued command"),
        }
    }

    fn issued(dispatch: &GoalDispatch) -> &ProtocolCommand {
        match dispatch {
            GoalDispatch::Issue { command, .. } => command.as_ref(),
            GoalDispatch::Say(text) => panic!("expected an issued command, got: {text}"),
        }
    }

    // ── positive controls: each verb builds the typed command ─────────────

    #[test]
    fn open_builds_the_typed_command() {
        let dispatch = parse_goal_line(
            "/goal open g1 anvil 3 land the repair",
            Some("sess-1"),
            &BTreeMap::new(),
            "req-1",
        );
        match issued(&dispatch) {
            ProtocolCommand::GoalOpen(open) => {
                assert_eq!(open.goal_id, "g1");
                assert_eq!(open.session_id, "sess-1");
                assert_eq!(open.request_id, "req-1");
                assert_eq!(open.iterations, 3);
                assert_eq!(open.strategy, GoalStrategy::Anvil);
                assert_eq!(open.objective, "land the repair");
                assert_eq!(open.goal_version, GOAL_PROTOCOL_VERSION);
            }
            other => panic!("expected GoalOpen, got {other:?}"),
        }
    }

    #[test]
    fn resync_without_a_goal_id_asks_for_every_goal() {
        let dispatch = parse_goal_line("/goal resync", Some("sess-1"), &BTreeMap::new(), "req-2");
        match issued(&dispatch) {
            ProtocolCommand::GoalResync(resync) => assert!(resync.goal_id.is_none()),
            other => panic!("expected GoalResync, got {other:?}"),
        }
    }

    #[test]
    fn resync_with_a_goal_id_scopes_to_it() {
        let dispatch =
            parse_goal_line("/goal resync g1", Some("sess-1"), &BTreeMap::new(), "req-3");
        match issued(&dispatch) {
            ProtocolCommand::GoalResync(resync) => {
                assert_eq!(resync.goal_id.as_deref(), Some("g1"));
            }
            other => panic!("expected GoalResync, got {other:?}"),
        }
    }

    #[test]
    fn task_collects_dependencies_as_a_set() {
        let dispatch = parse_goal_line(
            "/goal task g1 t2 --after t1,t1,t0",
            Some("sess-1"),
            &held("g1"),
            "req-4",
        );
        match issued(&dispatch) {
            ProtocolCommand::GoalDeclareTask(task) => {
                assert_eq!(task.task_id, "t2");
                // A repeated edge is the same graph — the set collapses it.
                assert_eq!(task.depends_on.len(), 2);
                assert!(task.depends_on.contains("t1"));
                assert!(task.depends_on.contains("t0"));
                assert!(task.idempotency_key.is_none());
            }
            other => panic!("expected GoalDeclareTask, got {other:?}"),
        }
    }

    #[test]
    fn advance_carries_the_cursor_from_the_held_projection() {
        let dispatch = parse_goal_line("/goal advance g1", Some("sess-1"), &held("g1"), "req-5");
        match issued(&dispatch) {
            ProtocolCommand::GoalAdvance(advance) => {
                assert_eq!(advance.cursor.journal_sequence, Some(12));
                assert_eq!(advance.cursor.journal_digest, "digest-12");
            }
            other => panic!("expected GoalAdvance, got {other:?}"),
        }
    }

    #[test]
    fn cancel_carries_the_cursor_from_the_held_projection() {
        let dispatch = parse_goal_line("/goal cancel g1", Some("sess-1"), &held("g1"), "req-6");
        match issued(&dispatch) {
            ProtocolCommand::GoalCancel(cancel) => {
                assert_eq!(cancel.cursor.journal_digest, "digest-12");
                assert_eq!(cancel.goal_id, "g1");
            }
            other => panic!("expected GoalCancel, got {other:?}"),
        }
    }

    #[test]
    fn every_strategy_is_typeable_from_the_terminal() {
        // Completeness: a sixth GoalStrategy makes this go red rather than
        // becoming a loop owner no TUI user can authorize.
        for (name, expected) in strategy_names().into_iter().zip(GoalStrategy::ALL) {
            let dispatch = parse_goal_line(
                &format!("/goal open g1 {name} 1 objective"),
                Some("sess-1"),
                &BTreeMap::new(),
                "req-strategy",
            );
            match issued(&dispatch) {
                ProtocolCommand::GoalOpen(open) => assert_eq!(open.strategy, expected),
                other => panic!("expected GoalOpen for {name}, got {other:?}"),
            }
        }
    }

    // ── negative controls: each refusal is decided here, not sent ─────────

    #[test]
    fn a_bare_goal_lists_and_issues_nothing() {
        let dispatch = parse_goal_line("/goal", Some("sess-1"), &held("g1"), "req-7");
        let text = say(&dispatch);
        assert!(text.contains("g1"), "the held Goal must be listed: {text}");
        assert!(text.contains("ship the thing"));
    }

    #[test]
    fn a_bare_goal_with_no_goals_says_so() {
        let dispatch = parse_goal_line("/goal", Some("sess-1"), &BTreeMap::new(), "req-8");
        assert!(say(&dispatch).contains("No durable Goal has been reported"));
    }

    #[test]
    fn control_without_a_durable_session_names_the_real_cause() {
        let dispatch = parse_goal_line("/goal resync", None, &BTreeMap::new(), "req-9");
        let text = say(&dispatch);
        assert!(
            text.contains("durable"),
            "must name durable sessions, not a missing Goal: {text}"
        );
    }

    #[test]
    fn advance_without_a_held_projection_refuses_locally() {
        let dispatch =
            parse_goal_line("/goal advance ghost", Some("sess-1"), &BTreeMap::new(), "r");
        let text = say(&dispatch);
        assert!(
            text.contains("/goal resync ghost"),
            "must point at the recovery step: {text}"
        );
    }

    #[test]
    fn zero_iterations_is_refused_before_it_reaches_core() {
        let dispatch = parse_goal_line(
            "/goal open g1 direct 0 objective",
            Some("sess-1"),
            &BTreeMap::new(),
            "r",
        );
        assert!(say(&dispatch).contains("at least 1"));
    }

    #[test]
    fn a_non_numeric_loop_bound_is_refused() {
        let dispatch = parse_goal_line(
            "/goal open g1 direct many objective",
            Some("sess-1"),
            &BTreeMap::new(),
            "r",
        );
        assert!(say(&dispatch).contains("not a loop bound"));
    }

    #[test]
    fn an_unknown_strategy_lists_the_real_ones() {
        let dispatch = parse_goal_line(
            "/goal open g1 swarm 1 objective",
            Some("sess-1"),
            &BTreeMap::new(),
            "r",
        );
        let text = say(&dispatch);
        assert!(text.contains("swarm"));
        for name in strategy_names() {
            assert!(text.contains(name), "must list `{name}`: {text}");
        }
    }

    #[test]
    fn an_objective_less_open_is_refused() {
        let dispatch = parse_goal_line(
            "/goal open g1 direct 2",
            Some("sess-1"),
            &BTreeMap::new(),
            "r",
        );
        assert!(say(&dispatch).contains("objective"));
    }

    #[test]
    fn an_unknown_verb_shows_usage() {
        let dispatch = parse_goal_line("/goal frobnicate", Some("sess-1"), &BTreeMap::new(), "r");
        let text = say(&dispatch);
        assert!(text.contains("frobnicate"));
        assert!(text.contains("/goal advance"));
    }

    #[test]
    fn an_unknown_task_option_is_refused() {
        let dispatch = parse_goal_line(
            "/goal task g1 t1 --before t0",
            Some("sess-1"),
            &held("g1"),
            "r",
        );
        assert!(say(&dispatch).contains("--after"));
    }

    #[test]
    fn after_with_no_ids_is_refused() {
        let dispatch = parse_goal_line(
            "/goal task g1 t1 --after ,,",
            Some("sess-1"),
            &held("g1"),
            "r",
        );
        assert!(say(&dispatch).contains("at least one task id"));
    }
}
