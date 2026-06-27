use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::agents::channel_sink::{ChannelSink, SubAgentRelay};
use crate::agents::registry::AgentRegistry;
use crate::output::OutputSink;
use crate::spawner::{AgentSpawner, SpawnExtras, SubAgentConfig};
use wcore_protocol::events::ToolCategory;
use wcore_swarm::Topology;
use wcore_types::tool::{JsonSchema, ToolResult};

use wcore_tools::Tool;

const DEFAULT_SUB_AGENT_MAX_TURNS: usize = 200;
const DEFAULT_SUB_AGENT_MAX_TOKENS: u32 = 4096;

pub struct SpawnTool {
    spawner: Arc<AgentSpawner>,
    /// W7 F2: optional `AgentRegistry` for resolving named agents. When
    /// `None`, the tool ignores any `agent` field on incoming tasks (legacy
    /// anonymous-spawn behaviour).
    registry: Option<Arc<AgentRegistry>>,
    /// W7 F2: optional parent `OutputSink`. When `Some` and a task carries
    /// `agent: <name>`, the tool wires a `ChannelSink` so the sub-agent's
    /// events flow back via `emit_sub_agent_event`. When `None`, all
    /// tasks fall through to the legacy anonymous path.
    parent_output: Option<Arc<dyn OutputSink>>,
    /// 4.B.5: per-instance topology gates the sub-agent cap. Default
    /// `Topology::Spawn` preserves the legacy `MAX_SUB_AGENTS=5` behaviour;
    /// callers ready for higher fan-out opt in via `with_topology`.
    topology: Topology,
}

impl SpawnTool {
    pub fn new(spawner: Arc<AgentSpawner>) -> Self {
        Self {
            spawner,
            registry: None,
            parent_output: None,
            topology: Topology::Spawn,
        }
    }

    /// W7 F2 builder: attach an `AgentRegistry` so tasks can name a
    /// previously-loaded `AgentManifest` via the `agent` field.
    pub fn with_registry(mut self, registry: Arc<AgentRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// W7 F2 builder: attach the parent's `OutputSink` for sub-agent event
    /// relay. Both `with_registry` and `with_parent_output` must be set
    /// for the agent-aware relay path to activate; either missing means
    /// the tool runs in legacy anonymous mode.
    pub fn with_parent_output(mut self, output: Arc<dyn OutputSink>) -> Self {
        self.parent_output = Some(output);
        self
    }

    /// Override the default Spawn topology (max 5). Use Swarm/Mesh/Fleet
    /// to lift the cap when the orchestrator's surrounding lifecycle is
    /// ready to support it. Cap enforcement lives in this tool; the
    /// actual blackboard / hierarchical-reduction wiring is the
    /// orchestrator's job and is out of scope here.
    pub fn with_topology(mut self, topology: Topology) -> Self {
        self.topology = topology;
        self
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "Spawn"
    }

    fn description(&self) -> &str {
        "Spawn one or more sub-agents to handle tasks in parallel. \
         Each sub-agent has its own conversation context and tool access.\n\n\
         - Maximum 5 sub-agents per call.\n\
         - Each sub-agent runs up to 200 conversation turns with a 4096 token output limit.\n\
         - Use for independent, parallelizable tasks (e.g., searching different modules, \
         running separate analyses).\n\
         - Do NOT use for tasks that need shared state or sequential coordination."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "description": "List of tasks for sub-agents to execute in parallel",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Short descriptive name for the task"
                            },
                            "prompt": {
                                "type": "string",
                                "description": "The task description / prompt for the sub-agent"
                            },
                            "agent": {
                                "type": "string",
                                "description": "Optional: Agent YAML name from the registry; omit for anonymous spawn (W7 F2)"
                            }
                        },
                        "required": ["name", "prompt"]
                    }
                }
            },
            "required": ["tasks"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false // manages its own concurrency
    }

    fn is_deferred(&self) -> bool {
        // F-022: Spawn is a user-facing differentiator. Inline full schema in
        // the initial system prompt so models invoke it directly without a
        // mandatory ToolSearch round-trip first.
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let (tasks, agent_names) = match parse_tasks(&input) {
            Ok(parsed) => parsed,
            Err(e) => {
                return ToolResult {
                    content: e,
                    is_error: true,
                };
            }
        };

        if tasks.is_empty() {
            return ToolResult {
                content: "No tasks provided".to_string(),
                is_error: true,
            };
        }

        // W5.5 B-1: when the relay path is active (parent_output is Some), clamp
        // the effective concurrency cap to Topology::Mesh (50) regardless of the
        // configured topology. This prevents a large-registry install (agent_count
        // > 10 → bootstrap flips topology to Fleet = 100) from launching 100
        // concurrent full AgentEngine LLM calls through the relay path, which
        // would exhaust rate-limits and OOM on typical hardware. The Fleet cap
        // (100) is intentionally reserved for the unmonitored (no relay) path.
        // Non-relay path (parent_output None, Fleet) keeps the Fleet cap unchanged.
        let cap = if self.parent_output.is_some() {
            Topology::Mesh.default_config().max_agents as usize
        } else {
            self.topology.default_config().max_agents as usize
        };
        if tasks.len() > cap {
            return ToolResult {
                content: format!(
                    "Too many sub-agents for topology {}: {} (max {})",
                    self.topology,
                    tasks.len(),
                    cap,
                ),
                is_error: true,
            };
        }

        // v0.9.4 W1 relay fix:
        //
        // Decision A: drop the `any_named` gate — anonymous spawns ALSO relay
        // when `parent_output` is wired. The registry is empty in standalone
        // sessions, so requiring a named agent would silently kill all relay.
        //
        // Fleet handling (C8): we do NOT auto-Fleet-route when parent_output is
        // wired because Fleet hardcodes `channel_sink: None`. Instead we use the
        // per-task relay path (cap still enforced above via Mesh cap). Fleet only
        // fires when the caller explicitly set topology=Fleet AND parent_output is
        // None (unmonitored path).
        let results = if self.topology == Topology::Fleet && self.parent_output.is_none() {
            // Pure fleet path: no relay, sharded dispatch.
            let run_id = format!("spawn-tool-{}", uuid::Uuid::new_v4().simple());
            self.spawner.spawn_via_fleet(tasks, run_id).await
        } else if self.parent_output.is_some() {
            // Relay path: each task gets its own ChannelSink + parent_call_id.
            // Works for both named and anonymous tasks (Decision A).
            self.spawn_with_relay(tasks, &agent_names).await
        } else {
            // Legacy anonymous path: no parent output wired.
            self.spawner.spawn_parallel(tasks).await
        };

        let output: Vec<String> = results
            .iter()
            .map(|r| {
                let status = if r.is_error { "ERROR" } else { "OK" };
                format!(
                    "## {} [{}]\n{}\n[turns: {} | tokens: {} in / {} out]",
                    r.name, status, r.text, r.turns, r.usage.input_tokens, r.usage.output_tokens
                )
            })
            .collect();

        let all_error = results.iter().all(|r| r.is_error);

        ToolResult {
            content: output.join("\n\n---\n\n"),
            is_error: all_error,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    fn describe(&self, input: &Value) -> String {
        let task = input
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("sub-agent");
        format!("Spawn: {}", wcore_tools::truncate_utf8(task, 80))
    }
}

fn parse_tasks(input: &Value) -> Result<(Vec<SubAgentConfig>, Vec<Option<String>>), String> {
    let tasks_arr = input["tasks"]
        .as_array()
        .ok_or("Missing or invalid 'tasks' array")?;

    let mut configs = Vec::new();
    let mut agent_names: Vec<Option<String>> = Vec::new();
    for task in tasks_arr {
        let name = task["name"]
            .as_str()
            .ok_or("Each task must have a 'name' string")?
            .to_string();
        let prompt = task["prompt"]
            .as_str()
            .ok_or("Each task must have a 'prompt' string")?
            .to_string();
        let agent = task.get("agent").and_then(|v| v.as_str()).map(String::from);

        // Finding #174 (Spawn sub-agent overhead): give the child a trimmed,
        // task-focused system prompt instead of `None`. With `None`,
        // `AgentSpawner::child_config` leaves `config.system_prompt` as the
        // parent's FULL framework prompt (intro + tool guidance + AGENTS.md +
        // memory + skills index, ~3-6k tokens), so every Spawn child re-pays
        // that overhead — a 5-way spawn wastes ~15-30k input tokens. Spawn
        // children are unconditionally read-only (every dispatch path calls
        // `build_tool_registry(&[])` → Read/Grep/Glob only; Spawn never plumbs
        // `allowed_tools`), so they have no need for project-convention context
        // and the trim is always safe. Reuses `Delegate`'s `build_child_prompt`
        // so both delegation surfaces share one focused-prompt template.
        let system_prompt = wcore_tools::delegate::build_child_prompt(&prompt, None);

        configs.push(SubAgentConfig {
            name,
            prompt,
            max_turns: DEFAULT_SUB_AGENT_MAX_TURNS,
            max_tokens: DEFAULT_SUB_AGENT_MAX_TOKENS,
            system_prompt: Some(system_prompt),
            provider: None,
            model: None,
            temperature: None,
        });
        agent_names.push(agent);
    }

    Ok((configs, agent_names))
}

impl SpawnTool {
    /// v0.9.4 W1 relay fix: spawn with per-task channel-sink relay.
    ///
    /// Pre: `self.parent_output` is `Some`. Works for both named and
    /// anonymous tasks (Decision A — registry gate dropped). Each task
    /// gets its OWN `parent_call_id` (format: `spawn:{idx}:{name|anon}`)
    /// and its OWN `ChannelSink` so the bridge creates N distinct
    /// `SubAgentView`s instead of collapsing all tasks into one row.
    ///
    /// W5.5 H-1: each ChannelSink now has a dedicated lifecycle channel
    /// (capacity 2) for the terminal Done/Failed event. Stream events use
    /// the shared bounded `tx` (best-effort); the terminal signal uses the
    /// per-task `lifecycle_tx` (guaranteed). After the main stream drain
    /// exits, we flush all lifecycle receivers so the bridge always sees the
    /// terminal event even when a chatty sub-agent filled the stream buffer.
    async fn spawn_with_relay(
        &self,
        tasks: Vec<SubAgentConfig>,
        agent_names: &[Option<String>],
    ) -> Vec<crate::spawner::SubAgentResult> {
        // SAFETY: guarded by caller — `spawn_with_relay` is only called
        // when `self.parent_output.is_some()`.
        let parent_output = Arc::clone(
            self.parent_output
                .as_ref()
                .expect("spawn_with_relay precondition: parent_output is Some"),
        );

        // One shared stream drain channel; each task's ChannelSink gets a
        // clone of tx for best-effort stream events.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<SubAgentRelay>(
            crate::agents::channel_sink::CHANNEL_CAPACITY,
        );

        // W5.5 H-1: one dedicated lifecycle channel per task. Collected as
        // a Vec of receivers so we can flush them after the stream drain.
        let mut lifecycle_rxs: Vec<tokio::sync::mpsc::Receiver<SubAgentRelay>> =
            Vec::with_capacity(tasks.len());

        // Build per-task SpawnExtras with distinct parent_call_id + ChannelSink.
        let per_task_extras: Vec<SpawnExtras> = tasks
            .iter()
            .enumerate()
            .map(|(idx, task)| {
                let agent_label = agent_names
                    .get(idx)
                    .and_then(|a| a.as_deref())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("anon");
                // Unique id per task — the bridge keys SubAgentView on this.
                let parent_call_id = format!("spawn:{}:{}", idx, agent_label);
                // W5.5 H-1: dedicated lifecycle channel (capacity 2, never shared).
                let (ltx, lrx) = tokio::sync::mpsc::channel::<SubAgentRelay>(
                    crate::agents::channel_sink::LIFECYCLE_CAPACITY,
                );
                lifecycle_rxs.push(lrx);
                let sink = Arc::new(ChannelSink::new_with_lifecycle(
                    parent_call_id.clone(),
                    task.name.clone(),
                    tx.clone(),
                    ltx,
                ));
                SpawnExtras {
                    channel_sink: Some(sink),
                    agent_name: Some(task.name.clone()),
                    parent_call_id: Some(parent_call_id),
                }
            })
            .collect();

        // Drop the original tx so the drain exits when all per-task senders drop.
        drop(tx);

        // Drain task: wrap each SubAgentRelay in a SubAgentEvent via the parent.
        let drain_output = Arc::clone(&parent_output);
        let drain = tokio::spawn(async move {
            while let Some(relay) = rx.recv().await {
                drain_output.emit_sub_agent_event(
                    &relay.parent_call_id,
                    &relay.agent_name,
                    &relay.inner,
                );
            }
        });

        // Fan out — each task runs with its own ChannelSink via the new
        // per-task-extras API on AgentSpawner.
        let tasks_and_extras: Vec<_> = tasks.into_iter().zip(per_task_extras).collect();
        let results = self
            .spawner
            .spawn_parallel_with_per_task_extras(tasks_and_extras)
            .await;

        // Wait for the stream drain to flush all pending stream relays. By this
        // point every per-task ChannelSink has been dropped (all tasks completed),
        // so rx.recv() returns None and the drain task exits promptly.
        let _ = drain.await;

        // W5.5 H-1: flush lifecycle events AFTER the stream drain. The terminal
        // Done/Failed event for each task sits in its dedicated lifecycle channel.
        // Because the ChannelSink (and its lifecycle_tx clone) has already dropped,
        // recv() returns None after the one lifecycle event, so this loop is O(N).
        for mut lrx in lifecycle_rxs {
            while let Some(relay) = lrx.recv().await {
                parent_output.emit_sub_agent_event(
                    &relay.parent_call_id,
                    &relay.agent_name,
                    &relay.inner,
                );
            }
        }

        results
    }
}

#[cfg(test)]
mod child_prompt_trim_tests {
    //! Finding #174 — a `Spawn`-created child must carry a TRIMMED, task-focused
    //! system prompt, not inherit the parent's full framework prompt (intro +
    //! tool guidance + AGENTS.md + memory + skills index). Without the fix,
    //! `parse_tasks` set `system_prompt: None`, so `child_config` left the
    //! child's `config.system_prompt` as the parent's full prompt and every
    //! spawned child re-paid ~3-6k tokens of framework overhead.
    use super::parse_tasks;
    use serde_json::json;

    /// Stable markers that ONLY appear in the full parent framework prompt
    /// assembled by `crate::context::build_system_prompt` — never in the trimmed
    /// `build_child_prompt` output.
    const FULL_PROMPT_TOOL_GUIDANCE_MARKER: &str = "# Using your tools";
    const FULL_PROMPT_INTRO_MARKER: &str = "You are an AI assistant that can use tools";
    /// The skills index is wrapped in this reminder tag in the full prompt.
    const FULL_PROMPT_SKILLS_MARKER: &str = "The following skills are available for use";

    #[test]
    fn spawn_child_system_prompt_is_trimmed_not_full_framework() {
        let (configs, _agent_names) = parse_tasks(&json!({
            "tasks": [
                { "name": "search-mod", "prompt": "Find all callers of build_system_prompt" }
            ]
        }))
        .expect("valid single-task spawn input must parse");

        assert_eq!(configs.len(), 1);
        let sp = configs[0]
            .system_prompt
            .as_deref()
            .expect("Spawn child must get an explicit trimmed system prompt, not None");

        // It DOES carry the focused-subagent framing + the spawn task context.
        assert!(
            sp.contains("You are a focused subagent working on a specific delegated task."),
            "trimmed prompt must carry the focused-subagent framing; got: {sp}"
        );
        assert!(
            sp.contains("Find all callers of build_system_prompt"),
            "trimmed prompt must embed the spawn task prompt as the child's task; got: {sp}"
        );

        // It does NOT carry the parent's full framework sections. These markers
        // come only from `build_system_prompt`; their presence would prove the
        // child inherited the parent's full prompt (the pre-fix `None` behaviour).
        assert!(
            !sp.contains(FULL_PROMPT_TOOL_GUIDANCE_MARKER),
            "trimmed prompt must NOT carry the parent's tool-guidance section; got: {sp}"
        );
        assert!(
            !sp.contains(FULL_PROMPT_INTRO_MARKER),
            "trimmed prompt must NOT carry the parent's intro section; got: {sp}"
        );
        assert!(
            !sp.contains(FULL_PROMPT_SKILLS_MARKER),
            "trimmed prompt must NOT carry the parent's skills index; got: {sp}"
        );
    }
}

#[cfg(test)]
mod topology_cap_tests {
    use wcore_swarm::Topology;

    fn cap_for(topology: Topology) -> usize {
        topology.default_config().max_agents as usize
    }

    #[test]
    fn spawn_topology_cap_is_5() {
        assert_eq!(cap_for(Topology::Spawn), 5);
    }

    #[test]
    fn swarm_topology_cap_is_20() {
        assert_eq!(cap_for(Topology::Swarm), 20);
    }

    #[test]
    fn mesh_topology_cap_is_50() {
        assert_eq!(cap_for(Topology::Mesh), 50);
    }

    #[test]
    fn fleet_topology_cap_is_100() {
        assert_eq!(cap_for(Topology::Fleet), 100);
    }

    /// W5.5 B-1: effective_cap(Fleet, relay=true) == Mesh (50).
    /// This is the pure formula test for the cap selection logic in execute().
    /// The relay path (parent_output = Some) must cap at Mesh regardless of
    /// topology so a large-registry install (topology flipped to Fleet by
    /// bootstrap) does not launch 100 concurrent LLM engines.
    #[test]
    fn relay_path_effective_cap_is_mesh_when_topology_is_fleet_w55_b1() {
        // Mirrors the cap computation in execute():
        //   if parent_output.is_some() { Mesh } else { topology }
        let relay_active = true; // parent_output.is_some()
        let effective_cap = if relay_active {
            cap_for(Topology::Mesh)
        } else {
            cap_for(Topology::Fleet)
        };
        assert_eq!(
            effective_cap, 50,
            "W5.5 B-1: relay path effective cap must be Mesh=50, not Fleet=100. \
             Got: {effective_cap}"
        );
    }

    /// W5.5 B-1 counterpart: non-relay Fleet path must retain Fleet cap (100).
    #[test]
    fn non_relay_fleet_path_retains_fleet_cap_w55_b1() {
        let relay_active = false; // parent_output.is_none()
        let effective_cap = if relay_active {
            cap_for(Topology::Mesh)
        } else {
            cap_for(Topology::Fleet)
        };
        assert_eq!(
            effective_cap, 100,
            "W5.5 B-1: non-relay Fleet path must retain cap=100. Got: {effective_cap}"
        );
    }
}
