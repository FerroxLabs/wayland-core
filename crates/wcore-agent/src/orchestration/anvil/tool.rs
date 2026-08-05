//! `Forge` — the session-level Anvil tool (A1.9): natural language in, a
//! machine-stamped receipt out.
//!
//! This is the smart-loop front door. The session model is the intent
//! detector: the tool description carries the routing law, so "make sure this
//! is right / iterate until it's green / it must be verified" reaches for
//! Forge the same way file edits reach for Edit — no new syntax for the user.
//!
//! Approval posture: the ONE human decision happens at this tool's boundary
//! (interactive sessions approve the Forge call like any Exec tool). Inside,
//! the climb runs in the trusted posture (spec §5) — forked builders have no
//! approval channel, and the gate machinery, not the model, decides success.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_config::anvil::AnvilConfig;
use wcore_config::config::Config;
use wcore_protocol::events::ToolCategory;
use wcore_sandbox::SandboxRegistry;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolResult};

use super::engine::LandingReport;
use super::forge::drive_climb_full;
use crate::output::OutputSink;
use crate::spawner::AgentSpawner;

/// The session-level gated-forge tool.
pub struct ForgeTool {
    anvil: AnvilConfig,
    session_cfg: Config,
    egress_policy: wcore_egress::SharedPolicy,
    session_spawner: Arc<AgentSpawner>,
    output: Arc<dyn OutputSink>,
}

impl ForgeTool {
    /// Build the tool from the merged `[anvil]` block + the resolved session
    /// config (used to materialize the driver seat lazily, per call — key
    /// state is read when the forge actually runs, not at registration).
    #[must_use]
    pub fn new(
        anvil: AnvilConfig,
        session_cfg: Config,
        egress_policy: wcore_egress::SharedPolicy,
        session_spawner: Arc<AgentSpawner>,
        output: Arc<dyn OutputSink>,
    ) -> Self {
        Self {
            anvil,
            session_cfg,
            egress_policy,
            session_spawner,
            output,
        }
    }

    async fn execute_with_sandbox(
        &self,
        input: Value,
        sandbox: Arc<SandboxRegistry>,
        task_id: String,
    ) -> ToolResult {
        let Some(task) = input.get("task").and_then(Value::as_str) else {
            return ToolResult {
                content: "Forge requires a `task` string.".into(),
                is_error: true,
            };
        };

        // Materialize the driver seat lazily — key state as of NOW.
        let seat = match super::seat::materialize_driver_seat(
            &self.anvil,
            &self.session_cfg,
            Arc::clone(&self.egress_policy),
            &self.session_spawner,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                return ToolResult {
                    content: format!("Forge could not build a driver seat: {e}"),
                    is_error: true,
                };
            }
        };

        let workspace = match std::env::current_dir() {
            Ok(w) => w,
            Err(e) => {
                return ToolResult {
                    content: format!("Forge could not resolve the workspace: {e}"),
                    is_error: true,
                };
            }
        };

        // Valve seat (spec §6.4): the session/frontier model, read-only, one
        // diagnostic turn on a stall. Best-effort — a forge without a valve
        // is still a forge.
        let valve_seat = super::seat::materialize_valve_seat(
            &self.session_cfg,
            Arc::clone(&self.egress_policy),
            &self.session_spawner,
        )
        .await
        .ok();
        let valve_spawner = valve_seat
            .as_ref()
            .map(|s| &s.spawner as &dyn wcore_types::spawner::Spawner);

        let session_id = match self.output.current_session_id() {
            Some(session_id) => session_id,
            None => match self.session_spawner.durable_session_id() {
                Ok(session_id) => session_id,
                Err(error) => {
                    return ToolResult {
                        content: format!("Forge has no canonical session authority: {error}"),
                        is_error: true,
                    };
                }
            },
        };
        let run_id = uuid::Uuid::new_v4().to_string();

        match drive_climb_full(
            task,
            &self.anvil,
            &workspace,
            &seat.spawner,
            valve_spawner,
            &self.output,
            &session_id,
            &run_id,
            &task_id,
            sandbox,
        )
        .await
        {
            Ok(outcome) => {
                let worktree = outcome
                    .best_worktree
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(none kept)".to_string());
                let notes = if seat.notes.is_empty() {
                    String::new()
                } else {
                    format!("\nnotes: {}", seat.notes.join("; "))
                };
                // Surface the ACTUAL parent-owned landing outcome (not a pre-written
                // "retained for landing" placeholder): the winner was already driven
                // through the landing lifecycle inside the climb. The user's working
                // tree is never touched; a landed candidate waits on the
                // Wayland-owned integration clone for a Desktop-mediated accept.
                let landing_line = match &outcome.landing {
                    Some(LandingReport::Landed {
                        landed_commit,
                        target_ref,
                        integration_checkout,
                    }) => {
                        let short = landed_commit.get(..12).unwrap_or(landed_commit.as_str());
                        format!(
                            "Landed {short} onto {target_ref} in the Wayland-owned integration \
                             clone at {}; accept it from Wayland Desktop to fast-forward your \
                             branch. Your working tree was not modified.",
                            integration_checkout.display()
                        )
                    }
                    Some(LandingReport::Conflict { detail }) => format!(
                        "Landing conflict — nothing was landed; your tree was not modified: {detail}"
                    ),
                    Some(LandingReport::Incomplete { detail }) => format!(
                        "Landing incomplete — the integration ref advanced but projection did not \
                         complete; recovery may be required: {detail}"
                    ),
                    Some(LandingReport::RolledBack { detail }) => format!(
                        "Landing was rolled back — no change remains on the integration branch: \
                         {detail}"
                    ),
                    Some(LandingReport::RecoveryRequired { detail }) => format!(
                        "Landing needs explicit recovery before it can complete; your tree was not \
                         modified: {detail}"
                    ),
                    Some(LandingReport::Failed { detail }) => format!(
                        "Landing did not run to completion (the climb result stands); your tree was \
                         not modified: {detail}"
                    ),
                    None => "No candidate was landable.".to_string(),
                };
                ToolResult {
                    content: format!(
                        "Forged: {stamp} · {passed}/{total} checks · {iters} iteration(s) · \
                         {fires} valve fire(s) · driver seat {seat_label}\nterminal: \
                         {terminal:?}\ncandidate worktree: {worktree}\nreceipt: emitted as an \
                         authoritative top-level Core event; this summary is inert{notes}\n\n\
                         {landing_line}",
                        stamp = outcome.stamp,
                        passed = outcome.checks_passed,
                        total = outcome.checks_total,
                        iters = outcome.iterations,
                        fires = outcome.valve_fires,
                        seat_label = seat.label,
                        terminal = outcome.terminal,
                    ),
                    is_error: false,
                }
            }
            Err(e) => ToolResult {
                content: format!(
                    "Forge refused or failed: {e}\n(seat: {}{})",
                    seat.label,
                    if seat.notes.is_empty() {
                        String::new()
                    } else {
                        format!("; {}", seat.notes.join("; "))
                    }
                ),
                is_error: true,
            },
        }
    }
}

#[async_trait]
impl Tool for ForgeTool {
    fn name(&self) -> &str {
        "Forge"
    }

    fn description(&self) -> &str {
        "Iterate-until-verified forge for work with a REAL executable gate \
         (tests, build, typecheck). Use when the user wants work driven to a \
         provable finish line — \"make sure it's right\", \"iterate until \
         it's green/done\", \"this must be verified\" — and the workspace has \
         (or the task implies) a checkable gate.\n\n\
         - Forks a builder into an isolated git worktree; your tree is never \
         touched. The winning change lands on a candidate branch for review.\n\
         - Runs the gate sandboxed (network-denied) every round; only a \
         passing gate earns the `verified` stamp. Returns a receipt: terminal \
         state, checks, iterations, cost.\n\
         - The gate comes from `[anvil] gate` config or is auto-detected \
         (cargo/npm/go/pytest/just/make).\n\
         - For judgment work with NO checkable reward (naming, prose, \
         architecture opinions), do NOT use Forge — that is Crucible council \
         territory.\n\
         - Long-running: a climb can take several minutes."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The change to forge, stated as intent with its finish line \
                                    (e.g. \"fix the failing auth tests\", \"make `cargo test -p x` pass \
                                    after adding retry backoff\"). Use paths RELATIVE to the repo root \
                                    only — never absolute paths: the forge builds in its own isolated \
                                    worktree and supplies the working directory itself."
                }
            },
            "required": ["task"]
        })
    }

    fn category(&self) -> ToolCategory {
        // Exec: the 600s dispatch-timeout class — a climb legitimately runs
        // minutes. Interactive sessions require approval for Exec tools,
        // which is exactly the single human decision the forge wants.
        ToolCategory::Exec
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false // one climb per workspace (the lease enforces it anyway)
    }

    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        // Forge spans child agents, processes, and repository state without reconciliation.
        ToolEffectContract::default()
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let sandbox = match SandboxRegistry::required_for_session(None) {
            Ok(runtime) => Arc::new(runtime),
            Err(e) => {
                return ToolResult {
                    content: format!("Forge refused: invalid sandbox selection: {e}"),
                    is_error: true,
                };
            }
        };
        self.execute_with_sandbox(input, sandbox, uuid::Uuid::new_v4().to_string())
            .await
    }

    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let task_id = if ctx.call_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            ctx.call_id.clone()
        };
        self.execute_with_sandbox(input, Arc::clone(&ctx.sandbox), task_id)
            .await
    }
}
