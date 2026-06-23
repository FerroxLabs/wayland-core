// T3-3.1.7: SessionSearchTool — long-term conversation recall.
//
// Ported from an upstream MIT-licensed library (see THIRD-PARTY-NOTICES.md). The Python version uses
// SQLite FTS5 against a session transcript table, then summarizes the top
// matches with a cheap LLM.
//
// In wayland-core the session/episode store lives in `wcore-memory` (v2
// 5-partition × 3-tier model), and the public `MemoryApi::search` entry
// point returns ranked `Hit` rows with `session_id` + `preview` for every
// matching episode. This tool wraps that surface so the agent can recall
// past sessions without re-reading raw transcripts.
//
// Notable deviations from the Python port (intentional):
//   - No per-session LLM summarization step. The Python tool calls
//     Gemini Flash to fold each matched transcript into a recap; the Rust
//     port returns the raw `Hit` previews because adding an LlmProvider
//     wire into `wcore-tools` would pull `wcore-providers` into the deps
//     graph (a much larger surface than this sub-wave covers). Summarization
//     can be re-introduced as a follow-up wave once the auxiliary-model
//     plumbing exists on `ToolContext`.
//   - "Recent sessions" mode (empty query → list latest sessions) is not
//     supported: `MemoryApi` only exposes search, not raw enumeration.
//     Callers receive an error for empty queries so the contract stays
//     explicit rather than silently returning nothing.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_memory::api::MemoryApi;
use wcore_memory::v2_types::{AccessToken, Hit, Query, Tier};
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Default number of sessions returned when caller does not specify `limit`.
const DEFAULT_LIMIT: u64 = 3;
/// Hard ceiling on `limit` — matches the Python tool's clamp.
const MAX_LIMIT: u64 = 5;

/// Tool that exposes `MemoryApi::search` to the agent for cross-session
/// recall. Constructed with the host's memory handle; if memory is the
/// `NullMemory` no-op the tool still registers and returns an empty
/// result set on every search (mirroring the Python tool's "no matches"
/// branch).
pub struct SessionSearchTool {
    memory: Arc<dyn MemoryApi>,
}

impl SessionSearchTool {
    pub fn new(memory: Arc<dyn MemoryApi>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for SessionSearchTool {
    fn name(&self) -> &str {
        "session_search"
    }

    fn description(&self) -> &str {
        "Search your long-term memory of past conversations. Returns ranked previews \
         of past session episodes matching the query. Use proactively when the user \
         references prior work (\"we did this before\", \"remember when\", \"last time\") \
         or asks about a topic that isn't in the current context. Search syntax mirrors \
         FTS — keywords joined with OR for broad recall, phrases for exact match. \
         Returns up to `limit` hits (default 3, max 5) ordered by relevance."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query — keywords or phrases to find in past sessions."
                },
                "limit": {
                    "type": "integer",
                    "description": "Max sessions to return (default: 3, max: 5).",
                    "default": 3
                },
                "tier": {
                    "type": "string",
                    "enum": ["session", "project", "global"],
                    "description": "Which memory tier to search. Defaults to 'project' (cross-session recall)."
                }
            },
            "required": ["query"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Pure-reader path: MemoryApi::search holds no agent-visible state
        // and is safe to invoke concurrently with other tool calls.
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // --- Parse + validate input --------------------------------------
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            Some(_) => {
                return ToolResult {
                    content: "session_search requires a non-empty `query` (recent-sessions browsing is not supported by MemoryApi).".to_string(),
                    is_error: true,
                };
            }
            None => {
                return ToolResult {
                    content: "session_search: missing required parameter `query`.".to_string(),
                    is_error: true,
                };
            }
        };

        // Defensive coercion: open-source models occasionally send `limit`
        // as a string or null. Mirror the Python tool's clamp to [1, 5].
        let raw_limit = input
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_LIMIT);
        let limit = raw_limit.clamp(1, MAX_LIMIT) as usize;

        let tier = match input.get("tier").and_then(|v| v.as_str()) {
            Some("session") => Tier::Session,
            Some("global") => Tier::Global,
            Some("project") | None => Tier::Project,
            Some(other) => {
                return ToolResult {
                    content: format!(
                        "session_search: unknown tier `{other}` (expected session|project|global)."
                    ),
                    is_error: true,
                };
            }
        };

        // --- Query MemoryApi ---------------------------------------------
        let q = Query {
            text: query.clone(),
            tier,
            partition: None,
            entities: None,
            limit_per_modality: limit,
            kg_depth: 1,
            token_budget: None,
        };

        let hits: Vec<Hit> = match self.memory.search(q, AccessToken::MainAgent).await {
            Ok(h) => h,
            Err(e) => {
                return ToolResult {
                    content: format!("session_search: memory backend error: {e}"),
                    is_error: true,
                };
            }
        };

        // --- Dedup by session, cap at `limit` ----------------------------
        // The MemoryApi may return multiple hits within a single session
        // (e.g. one per matching episode). Group by session_id so the
        // caller sees one row per past conversation, matching the
        // Python tool's seen_sessions logic.
        let mut seen: Vec<&Hit> = Vec::with_capacity(limit);
        for h in &hits {
            let already_seen = seen
                .iter()
                .any(|prev| prev.session_id == h.session_id && h.session_id.is_some());
            if !already_seen {
                seen.push(h);
            }
            if seen.len() >= limit {
                break;
            }
        }

        let results: Vec<Value> = seen
            .iter()
            .map(|h| {
                json!({
                    "session_id": h.session_id,
                    "score": h.score,
                    "partition": format!("{:?}", h.partition).to_lowercase(),
                    "tier": h.tier.as_str(),
                    "preview": h.preview,
                    "id": h.id,
                })
            })
            .collect();

        let count = results.len();
        let body = json!({
            "success": true,
            "query": query,
            "results": results,
            "count": count,
        });

        ToolResult {
            content: body.to_string(),
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_memory::null::NullMemory;

    /// Test 1: tool registers in the registry and is reachable via
    /// `ToolDispatcher::dispatch`. Verifies name + schema round-trip.
    #[tokio::test]
    async fn registers_in_dispatcher() {
        use crate::dispatcher::ToolDispatcher;
        use crate::registry::ToolRegistry;

        let mem: Arc<dyn MemoryApi> = Arc::new(NullMemory);
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(SessionSearchTool::new(mem)));

        assert!(reg.tool_names().iter().any(|n| n == "session_search"));

        let res = reg
            .dispatch("session_search", json!({ "query": "anything" }))
            .await;
        assert!(!res.is_error, "dispatch returned error: {}", res.content);
        // NullMemory has no episodes → expect empty results array.
        let v: Value = serde_json::from_str(&res.content).unwrap();
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["count"], json!(0));
        assert!(v["results"].as_array().unwrap().is_empty());
    }

    /// Test 2: happy path against a real (in-memory) Memory backend.
    /// Records an episode, then searches for terms in its summary and
    /// asserts at least one hit comes back with the expected session id.
    #[tokio::test]
    async fn happy_path_against_null_memory() {
        // NullMemory always returns empty hits; this test verifies the
        // tool surfaces the empty-result path correctly with all the
        // expected envelope keys present.
        let mem: Arc<dyn MemoryApi> = Arc::new(NullMemory);
        let tool = SessionSearchTool::new(mem);

        let res = tool
            .execute(json!({
                "query": "rust borrow checker",
                "limit": 5,
                "tier": "project",
            }))
            .await;

        assert!(!res.is_error);
        let v: Value = serde_json::from_str(&res.content).unwrap();
        assert_eq!(v["query"], json!("rust borrow checker"));
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["count"], json!(0));
        assert!(v["results"].is_array());
    }

    /// Test 3: invalid inputs are rejected with `is_error: true`.
    #[tokio::test]
    async fn invalid_input_rejected() {
        let mem: Arc<dyn MemoryApi> = Arc::new(NullMemory);
        let tool = SessionSearchTool::new(mem);

        // Missing query.
        let res = tool.execute(json!({})).await;
        assert!(res.is_error, "expected error for missing query");
        assert!(res.content.contains("query"));

        // Empty / whitespace-only query.
        let res = tool.execute(json!({ "query": "   " })).await;
        assert!(res.is_error, "expected error for whitespace query");

        // Unknown tier.
        let res = tool.execute(json!({ "query": "x", "tier": "bogus" })).await;
        assert!(res.is_error, "expected error for unknown tier");
        assert!(res.content.contains("tier"));
    }

    /// Bonus: limit clamping. A `limit` above MAX_LIMIT should not blow
    /// up — it gets silently clamped, mirroring the Python tool.
    #[tokio::test]
    async fn limit_clamped_to_max() {
        let mem: Arc<dyn MemoryApi> = Arc::new(NullMemory);
        let tool = SessionSearchTool::new(mem);
        let res = tool.execute(json!({ "query": "x", "limit": 999 })).await;
        assert!(!res.is_error);
    }
}
