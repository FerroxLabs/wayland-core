// Memory write tool: `record_episode` — lets the agent deliberately log a
// meaningful event into P2 episodic memory so future sessions can recall it
// via `session_search` / session-start recall. Companion to the read-side
// `SessionSearchTool`; both wrap the `wcore-memory` v2 `MemoryApi`.
//
// Mirrors the `SessionSearchTool` shape (Arc<dyn MemoryApi>, NullMemory-safe).
// With `NullMemory` the write is a no-op that still returns success so the
// tool name is always visible to the model.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_memory::api::MemoryApi;
use wcore_memory::v2_types::{AccessToken, Episode, EpisodeId, EpisodeStatus, Tier};
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Current unix time in seconds (the `Episode.ts` unit). Saturates to 0 if the
/// clock is before the epoch (impossible in practice) rather than panicking.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Parse the optional `tier` field, defaulting to Project (cross-session,
/// project-scoped recall — the most useful default for a durable episode).
fn parse_tier(input: &Value) -> Result<Tier, String> {
    match input.get("tier").and_then(|v| v.as_str()) {
        Some("session") => Ok(Tier::Session),
        Some("global") => Ok(Tier::Global),
        Some("project") | None => Ok(Tier::Project),
        Some(other) => Err(format!(
            "unknown tier `{other}` (expected session|project|global)"
        )),
    }
}

/// Tool exposing `MemoryApi::record_episode` to the agent.
pub struct RecordEpisodeTool {
    memory: Arc<dyn MemoryApi>,
}

impl RecordEpisodeTool {
    pub fn new(memory: Arc<dyn MemoryApi>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for RecordEpisodeTool {
    fn name(&self) -> &str {
        "record_episode"
    }

    fn description(&self) -> &str {
        "Record a meaningful event into your long-term episodic memory so future \
         sessions can recall it. Use when something worth remembering happens — a \
         decision made, a problem solved, a user preference learned, a milestone \
         reached. Provide a concise `summary` (one or two sentences). Episodes are \
         timestamped and retrieved later via memory search; do NOT log routine or \
         trivial turns. Defaults to the project tier (cross-session, this project)."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Concise description of what happened and why it matters."
                },
                "episode_type": {
                    "type": "string",
                    "description": "Short category label, e.g. 'decision', 'bugfix', 'preference', 'milestone'. Defaults to 'note'."
                },
                "atomic_facts": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of discrete facts extracted from the event."
                },
                "tier": {
                    "type": "string",
                    "enum": ["session", "project", "global"],
                    "description": "Memory tier. Defaults to 'project' (this project, across sessions)."
                }
            },
            "required": ["summary"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // A write — serialize against other tool calls to avoid interleaving
        // ambiguous concurrent memory mutations within a turn.
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let summary = match input.get("summary").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                return ToolResult {
                    content: "record_episode: missing or empty required parameter `summary`."
                        .to_string(),
                    is_error: true,
                };
            }
        };
        let tier = match parse_tier(&input) {
            Ok(t) => t,
            Err(e) => {
                return ToolResult {
                    content: format!("record_episode: {e}."),
                    is_error: true,
                };
            }
        };
        let episode_type = input
            .get("episode_type")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("note")
            .to_string();
        let atomic_facts: Vec<String> = input
            .get("atomic_facts")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let episode = Episode {
            id: EpisodeId::new(),
            tier,
            ts: now_secs(),
            episode_type,
            summary: summary.clone(),
            atomic_facts,
            source: "agent".to_string(),
            source_product: "wcore-agent".to_string(),
            session_id: None,
            project_root: None,
            decay_score: 0.0,
            status: EpisodeStatus::Active,
        };

        match self
            .memory
            .record_episode(episode, AccessToken::MainAgent)
            .await
        {
            Ok(id) => ToolResult {
                content: json!({
                    "success": true,
                    "episode_id": id.0.to_string(),
                    "summary": summary,
                })
                .to_string(),
                is_error: false,
            },
            Err(e) => ToolResult {
                content: format!("record_episode: memory backend error: {e}"),
                is_error: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_memory::null::NullMemory;

    #[tokio::test]
    async fn missing_summary_is_rejected() {
        let tool = RecordEpisodeTool::new(Arc::new(NullMemory));
        let res = tool.execute(json!({})).await;
        assert!(res.is_error);
        assert!(res.content.contains("summary"));
    }

    #[tokio::test]
    async fn unknown_tier_is_rejected() {
        let tool = RecordEpisodeTool::new(Arc::new(NullMemory));
        let res = tool
            .execute(json!({ "summary": "did a thing", "tier": "bogus" }))
            .await;
        assert!(res.is_error);
        assert!(res.content.contains("tier"));
    }

    #[tokio::test]
    async fn happy_path_returns_success_envelope() {
        let tool = RecordEpisodeTool::new(Arc::new(NullMemory));
        let res = tool
            .execute(json!({
                "summary": "Chose the live-overlay scheduler for predicate pruning",
                "episode_type": "decision",
                "atomic_facts": ["pruned branch marked done", "joins still drain"],
                "tier": "project"
            }))
            .await;
        assert!(!res.is_error, "got error: {}", res.content);
        let v: Value = serde_json::from_str(&res.content).unwrap();
        assert_eq!(v["success"], json!(true));
        assert!(v["episode_id"].as_str().is_some());
    }
}
