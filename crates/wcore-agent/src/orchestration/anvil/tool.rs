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
use parking_lot::Mutex;
use serde_json::{Value, json};

use wcore_config::anvil::AnvilConfig;
use wcore_config::config::Config;
use wcore_protocol::events::{ProtocolEvent, ToolCategory};
use wcore_protocol::writer::ProtocolEmitter;
use wcore_sandbox::SandboxRegistry;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_types::tool::{JsonSchema, ToolResult};

use super::forge::drive_climb_full;

/// Captures the climb's protocol events (the `AnvilReceipt`) so the receipt
/// can ride back inside the tool result instead of being written to stdout —
/// a raw JSON line on stdout would corrupt the interactive TUI. Host-stream
/// re-emission of captured receipts is a documented follow-up.
struct CapturedEmitter(Mutex<Vec<String>>);

impl ProtocolEmitter for CapturedEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        if let Ok(line) = serde_json::to_string(event) {
            self.0.lock().push(line);
        }
        Ok(())
    }
}

/// The session-level gated-forge tool.
pub struct ForgeTool {
    anvil: AnvilConfig,
    session_cfg: Config,
}

impl ForgeTool {
    /// Build the tool from the merged `[anvil]` block + the resolved session
    /// config (used to materialize the driver seat lazily, per call — key
    /// state is read when the forge actually runs, not at registration).
    #[must_use]
    pub fn new(anvil: AnvilConfig, session_cfg: Config) -> Self {
        Self { anvil, session_cfg }
    }

    async fn execute_with_sandbox(
        &self,
        input: Value,
        sandbox: Arc<SandboxRegistry>,
    ) -> ToolResult {
        let Some(task) = input.get("task").and_then(Value::as_str) else {
            return ToolResult {
                content: "Forge requires a `task` string.".into(),
                is_error: true,
            };
        };

        // Materialize the driver seat lazily — key state as of NOW.
        let seat = match super::seat::materialize_driver_seat(&self.anvil, &self.session_cfg) {
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
        let valve_seat = super::seat::materialize_valve_seat(&self.session_cfg).ok();
        let valve_spawner = valve_seat
            .as_ref()
            .map(|s| &s.spawner as &dyn wcore_types::spawner::Spawner);

        let captured = Arc::new(CapturedEmitter(Mutex::new(Vec::new())));
        let emitter: Arc<dyn ProtocolEmitter> = captured.clone();

        match drive_climb_full(
            task,
            &self.anvil,
            &workspace,
            &seat.spawner,
            valve_spawner,
            &emitter,
            None,
            sandbox,
        )
        .await
        {
            Ok(outcome) => {
                let receipts = captured.0.lock().join("\n");
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
                ToolResult {
                    content: format!(
                        "Forged: {stamp} · {passed}/{total} checks · {iters} iteration(s) · \
                         {fires} valve fire(s) · driver seat {seat_label}\nterminal: \
                         {terminal:?}\ncandidate worktree: {worktree}\nreceipt: \
                         {receipts}{notes}\n\nIf verified, review the candidate worktree and \
                         merge/cherry-pick its branch; the user's tree was not modified.",
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
        self.execute_with_sandbox(input, sandbox).await
    }

    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        self.execute_with_sandbox(input, Arc::clone(&ctx.sandbox))
            .await
    }
}
