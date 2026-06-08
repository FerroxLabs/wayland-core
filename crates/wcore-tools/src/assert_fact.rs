// Memory write tool: `assert_fact` — lets the agent record a durable semantic
// fact as a (subject, predicate, object) triple into P3 semantic memory.
// Companion to `record_episode` (events) and `session_search` (recall); all
// wrap the `wcore-memory` v2 `MemoryApi`.
//
// NullMemory-safe: with the no-op backend the write returns success so the
// tool name stays visible to the model.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_memory::api::MemoryApi;
use wcore_memory::v2_types::{AccessToken, Fact, FactId, Tier};
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

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

/// Required non-empty string field accessor.
fn required_str(input: &Value, key: &str) -> Result<String, String> {
    match input.get(key).and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        _ => Err(format!("missing or empty required parameter `{key}`")),
    }
}

/// Tool exposing `MemoryApi::assert_fact` to the agent.
pub struct AssertFactTool {
    memory: Arc<dyn MemoryApi>,
}

impl AssertFactTool {
    pub fn new(memory: Arc<dyn MemoryApi>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for AssertFactTool {
    fn name(&self) -> &str {
        "assert_fact"
    }

    fn description(&self) -> &str {
        "Record a durable semantic fact as a (subject, predicate, object) triple into \
         long-term memory. Use when a new lasting truth emerges that should outlive this \
         session — e.g. subject='user', predicate='prefers', object='tabs over spaces', or \
         subject='project', predicate='deploys_to', object='Vercel'. A later fact about the \
         same subject+predicate supersedes the old one automatically. Defaults to the \
         project tier. Do NOT assert ephemeral or uncertain claims."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "subject":   { "type": "string", "description": "The entity the fact is about (e.g. 'user', 'project', a name)." },
                "predicate": { "type": "string", "description": "The relation/attribute (e.g. 'prefers', 'uses', 'deploys_to')." },
                "object":    { "type": "string", "description": "The value of the relation (e.g. 'dark mode', 'Rust')." },
                "confidence": {
                    "type": "number",
                    "description": "0.0–1.0 confidence in the fact. Defaults to 0.9.",
                    "default": 0.9
                },
                "tier": {
                    "type": "string",
                    "enum": ["session", "project", "global"],
                    "description": "Memory tier. 'global' for user-wide truths, 'project' (default) for project-scoped."
                }
            },
            "required": ["subject", "predicate", "object"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let subject = match required_str(&input, "subject") {
            Ok(s) => s,
            Err(e) => {
                return ToolResult {
                    content: format!("assert_fact: {e}."),
                    is_error: true,
                };
            }
        };
        let predicate = match required_str(&input, "predicate") {
            Ok(s) => s,
            Err(e) => {
                return ToolResult {
                    content: format!("assert_fact: {e}."),
                    is_error: true,
                };
            }
        };
        let object = match required_str(&input, "object") {
            Ok(s) => s,
            Err(e) => {
                return ToolResult {
                    content: format!("assert_fact: {e}."),
                    is_error: true,
                };
            }
        };
        let tier = match parse_tier(&input) {
            Ok(t) => t,
            Err(e) => {
                return ToolResult {
                    content: format!("assert_fact: {e}."),
                    is_error: true,
                };
            }
        };
        // Clamp confidence to [0,1]; default 0.9.
        let confidence = input
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.9)
            .clamp(0.0, 1.0);

        let fact = Fact {
            id: FactId::new(),
            tier,
            ts: now_secs(),
            subject: subject.clone(),
            predicate: predicate.clone(),
            object: object.clone(),
            confidence,
            source_episode: None,
            superseded_by: None,
        };

        match self.memory.assert_fact(fact, AccessToken::MainAgent).await {
            Ok(id) => ToolResult {
                content: json!({
                    "success": true,
                    "fact_id": id.0.to_string(),
                    "triple": format!("{subject} {predicate} {object}"),
                })
                .to_string(),
                is_error: false,
            },
            Err(e) => ToolResult {
                content: format!("assert_fact: memory backend error: {e}"),
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
    async fn missing_fields_rejected() {
        let tool = AssertFactTool::new(Arc::new(NullMemory));
        assert!(tool.execute(json!({})).await.is_error);
        assert!(
            tool.execute(json!({ "subject": "user", "predicate": "likes" }))
                .await
                .is_error,
            "missing object must be rejected"
        );
    }

    #[tokio::test]
    async fn happy_path_returns_triple() {
        let tool = AssertFactTool::new(Arc::new(NullMemory));
        let res = tool
            .execute(json!({
                "subject": "user",
                "predicate": "prefers",
                "object": "tabs over spaces",
                "confidence": 0.95
            }))
            .await;
        assert!(!res.is_error, "got error: {}", res.content);
        let v: Value = serde_json::from_str(&res.content).unwrap();
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["triple"], json!("user prefers tabs over spaces"));
    }
}
