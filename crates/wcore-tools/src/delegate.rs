//! T3-3.1.3 — `DelegateTool` ported from the prior Wayland Python engine.
//!
//! The predecessor's `delegate_task` spawns one or more child `AIAgent`
//! instances with isolated context, restricted toolsets, and their own
//! terminal sessions; the parent only sees the final summary. Wayland already
//! exposes the same primitive via [`wcore_types::spawner::Spawner`],
//! whose concrete implementation `AgentSpawner` lives in `wcore-agent`
//! (one layer above this crate). The trait is intentionally hosted in
//! `wcore-types` so a tool — which must stay below `wcore-agent` in the
//! dep graph — can bridge to the real spawner via dynamic dispatch
//! without inverting the graph.
//!
//! `DelegateTool` therefore holds an `Arc<dyn Spawner>` injected at
//! construction time (the CLI bootstrap wires the concrete `AgentSpawner`
//! through). The tool itself owns:
//!   * input parsing (single `goal`/`context` mode and batch `tasks` mode),
//!   * the focused child-prompt template (mirrors the predecessor's
//!     `_build_child_system_prompt`),
//!   * fan-out via `futures::future::join_all` (mirrors the predecessor's
//!     `ThreadPoolExecutor`), and
//!   * the JSON result envelope returned to the parent agent.
//!
//! The Wayland `Spawner` trait exposes a single-task `spawn_fork`
//! entry point; batch parallelism is implemented here by joining one
//! call per task. The blocked-tools / depth-limit / credential-pool
//! machinery from the predecessor does NOT cross the boundary in this port —
//! those concerns are already enforced inside `wcore-agent`
//! (`SpawnTool` + `AgentSpawner`) so duplicating them here would be
//! redundant and risk drift.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::spawner::{ForkOverrides, Spawner, SubAgentConfig, SubAgentResult};
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Hard upper bound on parallel children per call. Matches the
/// `MAX_SUB_AGENTS` cap in `wcore-agent::spawn_tool` so behaviour is
/// consistent across the two surfaces.
pub const DELEGATE_MAX_TASKS: usize = 5;

/// Default per-child conversation turn budget (mirrors the predecessor's
/// `DEFAULT_MAX_ITERATIONS = 50`).
pub const DELEGATE_DEFAULT_MAX_TURNS: usize = 50;

/// Default per-child max output tokens. Matches the value used by
/// `wcore-agent::spawn_tool` so a delegated child has the same token
/// budget regardless of which surface dispatched it.
pub const DELEGATE_DEFAULT_MAX_TOKENS: u32 = 4096;

/// Built-in delegate tool. Wires LLM-supplied delegation requests to
/// the engine's `Spawner` trait.
///
/// The tool stays callable even when no spawner is wired (e.g. tests
/// that construct the tool without an engine) — invocations in that
/// state return an `is_error: true` `ToolResult` whose `content`
/// surfaces the configuration gap, matching the predecessor's
/// "delegate_task requires a parent agent context" error shape.
pub struct DelegateTool {
    spawner: Option<Arc<dyn Spawner>>,
}

impl DelegateTool {
    /// Build the tool with a live `Spawner` (production path).
    pub fn new(spawner: Arc<dyn Spawner>) -> Self {
        Self {
            spawner: Some(spawner),
        }
    }

    /// Build the tool without a spawner. Useful for early registration
    /// + dispatcher tests; execute() returns the configuration-gap
    ///   error variant.
    pub fn unwired() -> Self {
        Self { spawner: None }
    }
}

/// Parsed delegation request. One entry per child to spawn.
#[derive(Debug, Clone)]
struct Task {
    goal: String,
    context: Option<String>,
    /// Reserved for future per-task tool whitelisting — currently
    /// surfaced via `ForkOverrides::allowed_tools` when set.
    toolsets: Vec<String>,
}

fn parse_input(input: &Value) -> Result<(Vec<Task>, usize), String> {
    // Batch mode wins when both are present (matches the predecessor contract).
    let max_turns = input
        .get("max_iterations")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(DELEGATE_DEFAULT_MAX_TURNS);

    if let Some(arr) = input.get("tasks").and_then(|v| v.as_array()) {
        if arr.is_empty() {
            return Err("No tasks provided.".to_string());
        }
        if arr.len() > DELEGATE_MAX_TASKS {
            return Err(format!(
                "Too many tasks: {} provided, but the per-call limit is {}.",
                arr.len(),
                DELEGATE_MAX_TASKS
            ));
        }
        let mut tasks = Vec::with_capacity(arr.len());
        for (i, t) in arr.iter().enumerate() {
            let goal = t
                .get("goal")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("Task {i} is missing a non-empty 'goal'."))?
                .to_string();
            let context = t
                .get("context")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let toolsets = t
                .get("toolsets")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            tasks.push(Task {
                goal,
                context,
                toolsets,
            });
        }
        return Ok((tasks, max_turns));
    }

    if let Some(goal) = input
        .get("goal")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let context = input
            .get("context")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let toolsets = input
            .get("toolsets")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        return Ok((
            vec![Task {
                goal: goal.to_string(),
                context,
                toolsets,
            }],
            max_turns,
        ));
    }

    Err("Provide either 'goal' (single task) or 'tasks' (batch).".to_string())
}

/// Mirror of the predecessor's `_build_child_system_prompt`. Kept intentionally
/// terse — the persona / workspace-hint extensions live in
/// `wcore-agent` where the engine knows the real session cwd.
///
/// Exposed (`pub`) so the `Spawn` tool in `wcore-agent` can reuse the
/// same focused-subagent prompt instead of letting its children inherit
/// the parent's full framework system prompt (intro + tool guidance +
/// AGENTS.md + memory + skills index). Sharing this one helper keeps the
/// two delegation surfaces (Delegate / Spawn) on an identical trimmed
/// prompt and avoids duplicating prompt-assembly logic across crates.
pub fn build_child_prompt(goal: &str, context: Option<&str>) -> String {
    let mut parts = Vec::with_capacity(4);
    parts.push("You are a focused subagent working on a specific delegated task.".to_string());
    parts.push(String::new());
    parts.push(format!("YOUR TASK:\n{}", goal));
    if let Some(ctx) = context
        && !ctx.trim().is_empty()
    {
        parts.push(format!("\nCONTEXT:\n{}", ctx));
    }
    parts.push(
        "\nComplete this task using the tools available to you. When finished, \
         provide a clear, concise summary of what you did, what you found, any \
         files you modified, and any issues encountered. Be thorough but \
         concise — your response is returned to the parent agent as a summary."
            .to_string(),
    );
    parts.join("\n")
}

fn task_to_config(task: &Task, max_turns: usize) -> (SubAgentConfig, ForkOverrides) {
    let cfg = SubAgentConfig {
        name: format!("delegate-{}", first_word(&task.goal)),
        prompt: task.goal.clone(),
        max_turns,
        max_tokens: DELEGATE_DEFAULT_MAX_TOKENS,
        system_prompt: Some(build_child_prompt(&task.goal, task.context.as_deref())),
        provider: None,
        model: None,
        temperature: None,
    };
    let overrides = ForkOverrides {
        model: None,
        effort: None,
        allowed_tools: task.toolsets.clone(),
    };
    (cfg, overrides)
}

fn first_word(s: &str) -> String {
    s.split_whitespace()
        .next()
        .map(|w| w.chars().take(24).collect::<String>())
        .unwrap_or_else(|| "task".to_string())
}

fn render_results(results: &[SubAgentResult]) -> Value {
    let entries: Vec<Value> = results
        .iter()
        .enumerate()
        .map(|(idx, r)| {
            json!({
                "task_index": idx,
                "name": r.name,
                "status": if r.is_error { "error" } else { "completed" },
                "summary": r.text,
                "turns": r.turns,
                "tokens": {
                    "input": r.usage.input_tokens,
                    "output": r.usage.output_tokens,
                },
            })
        })
        .collect();
    json!({ "results": entries })
}

#[async_trait]
impl Tool for DelegateTool {
    fn name(&self) -> &str {
        "Delegate"
    }

    fn description(&self) -> &str {
        "Spawn one or more focused subagents to work on delegated tasks in \
         isolated contexts. Each subagent gets its own conversation, tool set, \
         and turn budget; only the final summary is returned to the parent.\n\n\
         Two modes:\n\
         - Single: provide `goal` (+ optional `context`, `toolsets`).\n\
         - Batch:  provide `tasks` array (each entry has its own `goal`, \
         `context`, `toolsets`). Up to 5 run concurrently.\n\n\
         Subagents do not share the parent's conversation history — pass any \
         needed file paths, error messages, or constraints via `context`."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "What the subagent should accomplish. Be specific — the subagent knows nothing about your conversation."
                },
                "context": {
                    "type": "string",
                    "description": "Background info the subagent needs: file paths, error messages, constraints."
                },
                "toolsets": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tool whitelist for this subagent. Empty = read-only (Read, Grep, Glob). To grant Bash/Write/Edit you must name them explicitly; the subagent inherits the parent's approval posture (it does not silently auto-approve destructive tools)."
                },
                "tasks": {
                    "type": "array",
                    "description": "Batch mode: parallel tasks (up to 5). When set, top-level goal/context are ignored.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "goal":     { "type": "string" },
                            "context":  { "type": "string" },
                            "toolsets": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["goal"]
                    }
                },
                "max_iterations": {
                    "type": "integer",
                    "description": "Max conversation turns per subagent (default 50)."
                }
            }
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Owns its own parallelism via join_all over Spawner::spawn_fork.
        false
    }

    fn is_deferred(&self) -> bool {
        // F-022: Delegate is a user-facing differentiator. Inline full schema
        // in the initial system prompt so models invoke it directly without a
        // mandatory ToolSearch round-trip first.
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let (tasks, max_turns) = match parse_input(&input) {
            Ok(parsed) => parsed,
            Err(e) => {
                return ToolResult {
                    content: e,
                    is_error: true,
                };
            }
        };

        let Some(spawner) = self.spawner.as_ref() else {
            return ToolResult {
                content: "Delegate tool has no spawner wired — engine bootstrap did not inject \
                          a Spawner. This is a configuration bug, not user error."
                    .to_string(),
                is_error: true,
            };
        };

        // Build (config, overrides) pairs and fan out concurrently.
        let jobs: Vec<_> = tasks
            .iter()
            .map(|t| {
                let (cfg, overrides) = task_to_config(t, max_turns);
                let s = spawner.clone();
                async move { s.spawn_fork(cfg, overrides).await }
            })
            .collect();

        let results = futures::future::join_all(jobs).await;
        let all_error = !results.is_empty() && results.iter().all(|r| r.is_error);
        let payload = render_results(&results);

        ToolResult {
            content: payload.to_string(),
            is_error: all_error,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    fn describe(&self, input: &Value) -> String {
        if let Some(arr) = input.get("tasks").and_then(|v| v.as_array()) {
            return format!("Delegate: {} parallel task(s)", arr.len());
        }
        let goal = input
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("(no goal)");
        format!("Delegate: {}", crate::truncate_utf8(goal, 80))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wcore_types::message::TokenUsage;

    use crate::dispatcher::ToolDispatcher;
    use crate::registry::ToolRegistry;

    /// Minimal Spawner used to verify the tool drives the trait
    /// correctly. Counts calls and returns a canned `SubAgentResult`
    /// per `prompt` so we can match results back to input.
    struct MockSpawner {
        calls: AtomicUsize,
    }

    impl MockSpawner {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl Spawner for MockSpawner {
        async fn spawn_fork(
            &self,
            config: SubAgentConfig,
            _overrides: ForkOverrides,
        ) -> SubAgentResult {
            self.calls.fetch_add(1, Ordering::SeqCst);
            SubAgentResult {
                name: config.name,
                text: format!("done: {}", config.prompt),
                usage: TokenUsage::default(),
                turns: 1,
                is_error: false,
            }
        }
    }

    /// Test 1: tool registers in the dispatcher and is resolvable by
    /// name from `ToolRegistry`, including its breaker entry.
    #[tokio::test]
    async fn delegate_registers_in_dispatcher() {
        let spawner = Arc::new(MockSpawner::new());
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DelegateTool::new(spawner)));

        assert!(registry.get("Delegate").is_some());
        assert!(registry.tool_names().iter().any(|n| n == "Delegate"));
        // Breaker is auto-installed on register.
        assert!(registry.breaker_state("Delegate").is_some());
        // F-022: Delegate is no longer deferred so its schema inlines in the initial prompt.
        let defs = registry.to_tool_defs();
        let delegate = defs.iter().find(|d| d.name == "Delegate").unwrap();
        assert!(!delegate.deferred);
    }

    /// Test 2: valid delegation requests are parsed and dispatched
    /// through the wired `Spawner`. Verifies single-task mode, batch
    /// mode, and the no-spawner error variant.
    #[tokio::test]
    async fn delegate_spec_and_execute_paths() {
        // 2a: schema accepts a single-task request and reaches the spawner.
        let spawner = Arc::new(MockSpawner::new());
        let tool = DelegateTool::new(spawner.clone());
        let single = tool
            .execute(json!({
                "goal": "summarize foo.rs",
                "context": "path: /tmp/foo.rs"
            }))
            .await;
        assert!(!single.is_error, "single happy-path should not error");
        assert!(single.content.contains("done: summarize foo.rs"));
        assert!(single.content.contains("\"results\""));
        assert_eq!(spawner.calls.load(Ordering::SeqCst), 1);

        // 2b: batch mode fans out one spawn_fork per task.
        let batch = tool
            .execute(json!({
                "tasks": [
                    {"goal": "task A"},
                    {"goal": "task B", "context": "extra ctx"}
                ]
            }))
            .await;
        assert!(!batch.is_error);
        assert!(batch.content.contains("task A"));
        assert!(batch.content.contains("task B"));
        assert_eq!(
            spawner.calls.load(Ordering::SeqCst),
            3,
            "single + batch(2) = 3 spawn_fork calls",
        );

        // 2c: unwired tool surfaces the configuration-gap error
        // (analogue of the predecessor's "requires a parent agent context").
        let unwired = DelegateTool::unwired();
        let result = unwired.execute(json!({"goal": "noop"})).await;
        assert!(result.is_error);
        assert!(result.content.contains("no spawner wired"));
    }

    /// Test 3: invalid input is rejected without touching the spawner.
    #[tokio::test]
    async fn delegate_rejects_invalid_input() {
        let spawner = Arc::new(MockSpawner::new());
        let tool = DelegateTool::new(spawner.clone());

        // 3a: neither goal nor tasks.
        let r = tool.execute(json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Provide either 'goal'"));

        // 3b: empty goal string after trim.
        let r = tool.execute(json!({"goal": "   "})).await;
        assert!(r.is_error);

        // 3c: empty tasks array.
        let r = tool.execute(json!({"tasks": []})).await;
        assert!(r.is_error);
        assert!(r.content.contains("No tasks provided"));

        // 3d: task entry missing 'goal'.
        let r = tool
            .execute(json!({"tasks": [{"context": "no goal here"}]}))
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("missing a non-empty 'goal'"));

        // 3e: too many tasks (>DELEGATE_MAX_TASKS).
        let big = (0..(DELEGATE_MAX_TASKS + 1))
            .map(|i| json!({"goal": format!("t{}", i)}))
            .collect::<Vec<_>>();
        let r = tool.execute(json!({"tasks": big})).await;
        assert!(r.is_error);
        assert!(r.content.contains("Too many tasks"));

        // No invalid path should have reached the spawner.
        assert_eq!(spawner.calls.load(Ordering::SeqCst), 0);
    }
}
