//! Agent-level hook engine: Rust-native hooks composed with the
//! shell-hook executor from `wcore_config::hooks`.
//!
//! See `docs/superpowers/specs/2026-05-14-wcore-super-agent-design.md`
//! §4.4 (F1) for the design contract.
//!
//! W9 cycle-break: the `Hook` trait + `HookAction` + `TurnContext` +
//! `TurnResult` + `SessionEndSummary` value types were lifted into
//! `wcore-config::hooks` so `wcore-skills` (a dep of `wcore-agent`) can
//! implement them without closing a dependency cycle. The `HookEngine`
//! orchestrator stays here. Existing call sites that import from
//! `wcore_agent::hooks` keep compiling via the re-exports below.

// W8b D.1: SelfCorrectionHook — post_tool_use subscriber that classifies
// tool errors and injects a correction prompt for the next turn.
pub mod self_correction;
pub mod verify_write;

pub use self_correction::{ErrorClass, SelfCorrectMode, SelfCorrectionHook};
pub use verify_write::VerifyWriteHook;

use serde_json::Value;
use wcore_config::hooks::{HookError, HooksConfig, ShellHooks};
use wcore_plugin_api::registry::hooks::HookPhase;
use wcore_types::message::Message;

use crate::plugins::runner::PluginHook;

// Re-exports for backward compatibility: wcore-agent's local hook types
// now live in wcore-config::hooks (W9 cycle-break).
pub use wcore_config::hooks::{Hook, HookAction, SessionEndSummary, TurnContext, TurnResult};

/// Aggregated outcome from running every hook in a phase. Replaces the
/// raw `Result<(), HookError>` / `Vec<String>` return shapes so the
/// orchestration layer can observe Block / ModifyInput / Inject /
/// SwitchModel without losing the shell-hook semantics.
#[derive(Debug, Default, Clone)]
pub struct HookOutcome {
    /// Set when any Rust hook returned `Block`. First Block wins.
    pub block: Option<String>,
    /// Effective tool input after all `ModifyInput` actions (last wins).
    /// `None` means no Rust hook modified the input.
    pub modified_input: Option<Value>,
    /// Messages injected by `InjectMessage`, in registration order.
    pub injected_messages: Vec<Message>,
    /// Last `SwitchModel` target, if any.
    pub switch_model: Option<String>,
    /// Human-readable log lines emitted by shell hooks (post_tool_use, stop)
    /// and by Rust hooks whose action is not honoured at the current phase.
    pub log_lines: Vec<String>,
    /// v0.9.1.2 F10: hook lifecycle telemetry — plugin-hook fire lines and
    /// rust-hook "action ignored at phase X" lines. These are diagnostics
    /// for `/doctor` and log files, not transcript content. Drain sites in
    /// the orchestration + engine layers route this Vec to `tracing::debug!`
    /// only — never to `emit_info` or `eprintln!` — so the TUI transcript
    /// stays clean. Previously these lines lived in `log_lines`, which
    /// caused `[plugin-hook:wayland-ijfw:...] post_tool_use fired for ...`
    /// to leak into the transcript on every tool call (see audit
    /// `.planning/audits/2026-05-27-v0.9.1.2-findings-f10-hook-leak-bypass.md`).
    pub hook_trace: Vec<String>,
}

/// Composing engine. Wraps the existing shell hook executor plus a
/// `Vec<Box<dyn Hook>>` of Rust-native hooks and, from Task 1.3, a
/// `Vec<PluginHook>` of plugin-contributed name-only hooks.
///
/// The shell side keeps every contract today's callers depend on.
pub struct HookEngine {
    rust_hooks: Vec<Box<dyn Hook>>,
    shell: ShellHooks,
    /// Task 1.3 — plugin hooks registered via `register_plugin_hook`.
    /// Stored separately from shell hooks (different phase set, no command).
    plugin_hooks: Vec<PluginHook>,
}

impl HookEngine {
    /// Build with only shell hooks (the v0.1.x compatibility shape).
    /// Every existing call site `HookEngine::new(config.hooks.clone())`
    /// compiles unchanged after the import path is updated.
    pub fn new(config: HooksConfig) -> Self {
        Self {
            rust_hooks: Vec::new(),
            shell: ShellHooks::new(config),
            plugin_hooks: Vec::new(),
        }
    }

    /// Register a Rust hook. Registration order = execution order
    /// within each phase. Rust hooks run BEFORE shell hooks.
    pub fn register_rust_hook(&mut self, hook: Box<dyn Hook>) {
        self.rust_hooks.push(hook);
    }

    /// Task 1.3 — register a plugin-contributed hook. Stored separately from
    /// shell hooks (different phase set, no shell command). Phase 1: firing
    /// emits a tracing log line; no arbitrary behaviour is executed.
    ///
    /// Note: `SessionStart`, `PrePrompt`, and `PreCompact` have no current
    /// firing entrypoint — hooks registered at those phases are stored and
    /// visible via `plugin_hooks()` but are not yet fired.
    pub fn register_plugin_hook(&mut self, hook: PluginHook) {
        self.plugin_hooks.push(hook);
    }

    /// Returns true when any Rust hooks, shell hooks, or plugin hooks are present.
    pub fn has_hooks(&self) -> bool {
        !self.rust_hooks.is_empty() || self.shell.has_hooks() || !self.plugin_hooks.is_empty()
    }

    /// Skill-hook merge stays on the shell side only.
    pub fn merge_hooks(&mut self, additional: HooksConfig) {
        self.shell.merge_hooks(additional);
    }

    pub async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_input: &Value,
    ) -> Result<HookOutcome, HookError> {
        let mut outcome = HookOutcome::default();
        for hook in &self.rust_hooks {
            match hook.pre_tool_use(tool_name, tool_input).await {
                HookAction::Continue => {}
                HookAction::Block { reason } => {
                    outcome.block = Some(reason);
                    // Short-circuit: skip remaining Rust hooks AND shell hooks.
                    return Ok(outcome);
                }
                HookAction::ModifyInput(v) => outcome.modified_input = Some(v),
                HookAction::InjectMessage(_) => {
                    outcome.hook_trace.push(format!(
                        "[hook:{}] InjectMessage ignored on pre_tool_use (subscribe to on_turn_start)",
                        hook.name()
                    ));
                }
                HookAction::SwitchModel(_) => {
                    outcome.hook_trace.push(format!(
                        "[hook:{}] SwitchModel ignored on pre_tool_use (subscribe to on_turn_start)",
                        hook.name()
                    ));
                }
            }
        }
        // Task 1.3: fire plugin hooks registered at PreToolUse.
        // v0.9.1.2 F10: plugin-hook fire lines are telemetry — route to
        // `hook_trace`, never to `log_lines`, to keep them out of the transcript.
        outcome.hook_trace.extend(self.fire_plugin_hooks(
            HookPhase::PreToolUse,
            "pre_tool_use",
            &format!("for tool \"{tool_name}\""),
        ));
        // Effective input for shell hooks: respect any Rust-side modify.
        let effective_input = outcome.modified_input.as_ref().unwrap_or(tool_input);
        self.shell
            .run_pre_tool_use(tool_name, effective_input)
            .await?;
        Ok(outcome)
    }

    pub async fn run_post_tool_use(
        &self,
        tool_name: &str,
        call_id: &str,
        tool_input: &Value,
        tool_output: &str,
        is_error: bool,
    ) -> HookOutcome {
        let mut outcome = HookOutcome::default();
        for hook in &self.rust_hooks {
            match hook
                .post_tool_use(tool_name, call_id, tool_input, tool_output, is_error)
                .await
            {
                HookAction::Continue => {}
                HookAction::Block { reason } => {
                    outcome.hook_trace.push(format!(
                        "[hook:{}] post-block ignored: {}",
                        hook.name(),
                        reason
                    ));
                }
                HookAction::ModifyInput(_) => {
                    outcome.hook_trace.push(format!(
                        "[hook:{}] ModifyInput ignored on post_tool_use",
                        hook.name()
                    ));
                }
                HookAction::InjectMessage(m) => outcome.injected_messages.push(m),
                HookAction::SwitchModel(s) => outcome.switch_model = Some(s),
            }
        }
        // Task 1.3: fire plugin hooks registered at PostToolUse.
        // v0.9.1.2 F10: route plugin-hook fire lines to `hook_trace` only.
        outcome.hook_trace.extend(self.fire_plugin_hooks(
            HookPhase::PostToolUse,
            "post_tool_use",
            &format!("for tool \"{tool_name}\""),
        ));
        let shell_lines = self
            .shell
            .run_post_tool_use(tool_name, tool_input, tool_output)
            .await;
        outcome.log_lines.extend(shell_lines);
        outcome
    }

    pub async fn run_stop(&self) -> HookOutcome {
        HookOutcome {
            log_lines: self.shell.run_stop().await,
            ..Default::default()
        }
    }

    /// Fire `SessionStart` plugin hooks once, at the start of a session run.
    /// Log-only (Phase 1): no Rust-hook actions are applied for this phase, so
    /// it mirrors the lightweight `fire_plugin_hooks` path the other phases use.
    pub async fn run_session_start(&self) -> HookOutcome {
        let mut outcome = HookOutcome::default();
        outcome.hook_trace.extend(self.fire_plugin_hooks(
            HookPhase::SessionStart,
            "session_start",
            "",
        ));
        outcome
    }

    /// Fire `PrePrompt` plugin hooks once per turn, after the request is
    /// assembled and immediately before it is streamed. Log-only (Phase 1).
    pub async fn run_pre_prompt(&self) -> HookOutcome {
        let mut outcome = HookOutcome::default();
        outcome
            .hook_trace
            .extend(self.fire_plugin_hooks(HookPhase::PrePrompt, "pre_prompt", ""));
        outcome
    }

    /// Fire `PreCompact` plugin hooks once per turn, immediately before the
    /// multi-level compaction pass runs. Log-only (Phase 1).
    pub async fn run_pre_compact(&self, turn: usize, message_count: usize) -> HookOutcome {
        let mut outcome = HookOutcome::default();
        outcome.hook_trace.extend(self.fire_plugin_hooks(
            HookPhase::PreCompact,
            "pre_compact",
            &format!("(turn {turn}, {message_count} messages)"),
        ));
        outcome
    }

    pub async fn on_turn_start(&self, turn: usize, ctx: &TurnContext) -> HookOutcome {
        let mut outcome = HookOutcome::default();
        for hook in &self.rust_hooks {
            match hook.on_turn_start(turn, ctx).await {
                HookAction::Continue => {}
                HookAction::Block { reason } => {
                    outcome.hook_trace.push(format!(
                        "[hook:{}] Block ignored on on_turn_start: {}",
                        hook.name(),
                        reason
                    ));
                }
                HookAction::ModifyInput(_) => {
                    outcome.hook_trace.push(format!(
                        "[hook:{}] ModifyInput ignored on on_turn_start",
                        hook.name()
                    ));
                }
                HookAction::InjectMessage(m) => outcome.injected_messages.push(m),
                HookAction::SwitchModel(s) => outcome.switch_model = Some(s),
            }
        }
        // Task 1.3: fire plugin hooks registered at TurnStart.
        // v0.9.1.2 F10: route plugin-hook fire lines to `hook_trace` only.
        outcome.hook_trace.extend(self.fire_plugin_hooks(
            HookPhase::TurnStart,
            "on_turn_start",
            &format!("(turn {turn})"),
        ));
        outcome
    }

    pub async fn on_turn_end(&self, turn: usize, result: &TurnResult) -> HookOutcome {
        let mut outcome = HookOutcome::default();
        for hook in &self.rust_hooks {
            match hook.on_turn_end(turn, result).await {
                HookAction::Continue => {}
                HookAction::Block { reason } => {
                    outcome.hook_trace.push(format!(
                        "[hook:{}] Block ignored on on_turn_end: {}",
                        hook.name(),
                        reason
                    ));
                }
                HookAction::ModifyInput(_) => {
                    outcome.hook_trace.push(format!(
                        "[hook:{}] ModifyInput ignored on on_turn_end",
                        hook.name()
                    ));
                }
                HookAction::InjectMessage(m) => outcome.injected_messages.push(m),
                HookAction::SwitchModel(s) => outcome.switch_model = Some(s),
            }
        }
        // Task 1.3: fire plugin hooks registered at TurnEnd.
        // v0.9.1.2 F10: route plugin-hook fire lines to `hook_trace` only.
        outcome.hook_trace.extend(self.fire_plugin_hooks(
            HookPhase::TurnEnd,
            "on_turn_end",
            &format!("(turn {turn})"),
        ));
        outcome
    }

    pub async fn on_session_end(&self, summary: &SessionEndSummary) -> HookOutcome {
        let mut outcome = HookOutcome::default();
        for hook in &self.rust_hooks {
            match hook.on_session_end(summary).await {
                HookAction::Continue => {}
                action => {
                    outcome.hook_trace.push(format!(
                        "[hook:{}] {:?} ignored on on_session_end (session is terminating)",
                        hook.name(),
                        std::mem::discriminant(&action)
                    ));
                }
            }
        }
        // Task 1.3: fire plugin hooks registered at SessionEnd.
        // v0.9.1.2 F10: route plugin-hook fire lines to `hook_trace` only.
        outcome.hook_trace.extend(self.fire_plugin_hooks(
            HookPhase::SessionEnd,
            "on_session_end",
            &format!("(turns: {})", summary.turns),
        ));
        outcome
    }

    /// Fire all plugin hooks registered at `phase`. Phase 1: each emits a
    /// tracing log line and a returned entry. `detail` is the phase-specific suffix.
    fn fire_plugin_hooks(&self, phase: HookPhase, verb: &str, detail: &str) -> Vec<String> {
        self.plugin_hooks
            .iter()
            .filter(|h| h.phase == phase)
            .map(|ph| {
                let line = format!(
                    "[plugin-hook:{}:{}] {verb} fired {detail}",
                    ph.plugin, ph.name
                );
                tracing::debug!("{}", line);
                line
            })
            .collect()
    }

    /// Returns the slice of registered plugin hooks. Used by tests to assert
    /// delivery without triggering a full phase fire.
    pub fn plugin_hooks(&self) -> &[PluginHook] {
        &self.plugin_hooks
    }
}
