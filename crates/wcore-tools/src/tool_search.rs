use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolDef, ToolResult};

use crate::Tool;
use crate::context::ToolContext;

/// Wave RA — check `ctx.cancel` every N items so a `ToolSearch` against a
/// huge registry returns within ~500ms of a cancel signal instead of
/// running to completion. 100 is a balance between cancel-latency and the
/// overhead of repeated `is_cancelled()` calls on a `CancellationToken`.
const CANCEL_CHECK_INTERVAL: usize = 100;

/// Built-in tool that searches for deferred tools and loads their full schema.
/// Core tool (never deferred itself) — always available to the LLM.
pub struct ToolSearchTool {
    /// Snapshot of all tool definitions (taken at construction time).
    tool_defs: Vec<ToolDef>,
}

impl ToolSearchTool {
    pub fn new(tool_defs: Vec<ToolDef>) -> Self {
        Self { tool_defs }
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "ToolSearch"
    }

    fn description(&self) -> &str {
        "Search for deferred tools and load their full schema. \
         Use this ONCE before calling a deferred tool: a match makes that tool \
         immediately callable by name on your next step. Call it directly — do \
         NOT search for the same tool again, because repeating the search \
         returns the identical result and makes no further progress."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Tool name or keyword to search for"
                }
            },
            "required": ["query"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        self.execute_with_ctx(input, &ToolContext::test_default())
            .await
    }

    /// Wave RA RELIABILITY MAJOR #3 — periodic cancel check so a search
    /// against a large tool registry doesn't run to completion after the
    /// agent cancelled. Iterates `tool_defs` manually so we can poll
    /// `ctx.cancel.is_cancelled()` every [`CANCEL_CHECK_INTERVAL`] items.
    /// Cancel returns an `is_error: true` ToolResult with a cancellation
    /// message — matches BashTool / McpToolProxy / BrowserTool shape.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        if query.is_empty() {
            return ToolResult {
                content: "Error: query is required".to_string(),
                is_error: true,
            };
        }

        let query_lower = query.to_lowercase();

        // TOKENS, not one substring. `contains(&query_lower)` required the
        // ENTIRE query to appear verbatim in a single name or description, so
        // any natural multi-word query could only ever miss: "tvcontrol
        // TradingView chart" has to be present literally, and no tool
        // description will ever contain it. Measured in this repo's own live
        // runs: `tiny_ping` matched, `tiny_ping tool` returned "No deferred
        // tools matching" — same tool, one extra word.
        //
        // AND over tokens (every token must appear somewhere in name or
        // description) rather than OR, because OR over a chatty query matches
        // most of the catalogue and buries the tool the caller meant. A
        // single-token query behaves exactly as before, so existing callers and
        // their expectations are unchanged.
        //
        // Credited to the Wayland Desktop lane, which found and proved this
        // independently.
        let tokens: Vec<&str> = query_lower.split_whitespace().collect();
        if tokens.is_empty() {
            // Whitespace-only: non-empty by the guard above, but no token to
            // match on. Matching everything here would be worse than useless.
            return ToolResult {
                content: "Error: query is required".to_string(),
                is_error: true,
            };
        }

        let mut matches: Vec<Value> = Vec::new();

        for (idx, def) in self.tool_defs.iter().enumerate() {
            if idx % CANCEL_CHECK_INTERVAL == 0 && ctx.cancel.is_cancelled() {
                return ToolResult {
                    content: "ToolSearch cancelled by cancellation token".to_string(),
                    is_error: true,
                };
            }
            if !def.deferred {
                continue;
            }
            let name_l = def.name.to_lowercase();
            let desc_l = def.description.to_lowercase();
            if tokens
                .iter()
                .all(|t| name_l.contains(t) || desc_l.contains(t))
            {
                // `status` is the repair for a MEASURED no-progress loop, not
                // decoration. A match hydrates the tool engine-side and it is
                // genuinely callable on the next turn (measured: after one
                // search, `tiny_ping` appears in the model's own callable list).
                // But this tool answers from a CONSTRUCTION-TIME snapshot that
                // still marks it deferred, so a second search returns the
                // byte-identical result — which the model reads as "the schema
                // still has not loaded" and searches again. Observed against a
                // real MCP server: ten identical searches, no call ever
                // attempted, and the run killed by the engine's own repeated
                // -tool-call guard. Every MCP tool was unreachable this way,
                // not merely a large server's.
                //
                // Until the snapshot itself becomes hydration-aware, the result
                // must carry the one fact the snapshot cannot express: this
                // tool is now callable, so stop searching and call it.
                matches.push(json!({
                    "name": def.name,
                    "description": def.description,
                    "parameters": def.input_schema,
                    "status": "LOADED — this tool is now callable by name. Call it directly on your next step; searching for it again returns this same result and makes no progress.",
                }));
            }
        }

        // Final cancel check before producing output; lets a cancel that
        // fired in the tail of a fast iteration still be observed.
        if ctx.cancel.is_cancelled() {
            return ToolResult {
                content: "ToolSearch cancelled by cancellation token".to_string(),
                is_error: true,
            };
        }

        if matches.is_empty() {
            return ToolResult {
                content: format!("No deferred tools matching \"{}\" found.", query),
                is_error: false,
            };
        }

        ToolResult {
            content: serde_json::to_string_pretty(&matches).unwrap_or_default(),
            is_error: false,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_tool_defs() -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: "Read".into(),
                description: "Read a file".into(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                deferred: false,
                server: None,
            },
            ToolDef {
                name: "SpawnTool".into(),
                description: "Spawn sub-agents".into(),
                input_schema: json!({"type": "object", "properties": {"agents": {"type": "array"}}}),
                deferred: true,
                server: None,
            },
            ToolDef {
                name: "EnterPlanMode".into(),
                description: "Enter plan mode".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                deferred: true,
                server: None,
            },
        ]
    }

    #[tokio::test]
    async fn search_by_exact_name() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "SpawnTool"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("SpawnTool"));
        assert!(result.content.contains("Spawn sub-agents"));
        assert!(result.content.contains("parameters"));
    }

    /// A match must tell the caller the tool is now CALLABLE, not merely
    /// describe it.
    ///
    /// Without this the result is indistinguishable from "still deferred", and
    /// a model that has just hydrated a tool searches for it again instead of
    /// calling it. Measured against a real MCP server: ten byte-identical
    /// searches, no call ever attempted, the run terminated by the engine's own
    /// repeated-tool-call guard — with EVERY MCP tool unreachable that way, on
    /// a two-tool server as much as a hundred-tool one.
    ///
    /// MUTANT: delete the `"status"` field from the pushed match object and
    /// this fails. It asserts the callability signal specifically, not merely
    /// that some JSON came back — `search_by_exact_name` above already covers
    /// name/description/parameters and stayed green throughout the outage.
    #[tokio::test]
    async fn a_match_states_that_the_tool_is_now_callable() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "SpawnTool"})).await;
        assert!(!result.is_error);

        let parsed: serde_json::Value =
            serde_json::from_str(&result.content).expect("a match set must be a JSON array");
        let first = parsed
            .get(0)
            .expect("SpawnTool must match its own exact name");
        let status = first
            .get("status")
            .and_then(|s| s.as_str())
            .expect("a match must carry a `status` saying the tool is now callable");
        assert!(
            status.contains("callable"),
            "status must state callability, got: {status}"
        );

        // The engine's hydration recorder parses this same body and reads
        // `.name` off each element, so the array shape must survive.
        assert_eq!(
            first.get("name").and_then(|n| n.as_str()),
            Some("SpawnTool"),
            "the array shape `record_hydrated_tools` parses must be preserved"
        );
    }

    /// TEST B from the Wayland Desktop handoff, 2026-08-04. Their defect,
    /// their call, reproduced here.
    ///
    /// A multi-word query must match a tool whose description contains those
    /// words. It could not: the matcher asked whether the WHOLE query appeared
    /// verbatim as one substring, so "sub agents parallel" had to be present
    /// letter-for-letter in a single description. Real callers write queries
    /// like "tvcontrol TradingView chart"; none of them could ever match.
    ///
    /// MUTANT: restore `name_l.contains(&query_lower) ||
    /// desc_l.contains(&query_lower)` and this fails while every other test in
    /// this module stays green — which is exactly what happened in production.
    #[tokio::test]
    async fn a_multi_word_query_matches_words_scattered_through_a_description() {
        let tool = ToolSearchTool::new(build_tool_defs());

        // SpawnTool's description is "Spawn sub-agents". Both words are present
        // but REVERSED relative to the description, so the phrase "agents spawn"
        // appears nowhere verbatim — which is precisely what the old
        // whole-query substring compare demanded.
        let result = tool.execute(json!({"query": "agents spawn"})).await;
        assert!(!result.is_error);
        assert!(
            result.content.contains("SpawnTool"),
            "a multi-word query whose words all appear must match; got: {}",
            result.content
        );

        // AND, not OR: a query carrying a token that matches nothing must not
        // drag the tool back in, or a chatty query returns the whole catalogue.
        let result = tool
            .execute(json!({"query": "agents spawn zzzznotpresent"}))
            .await;
        assert!(!result.is_error);
        assert!(
            !result.content.contains("SpawnTool"),
            "every token must match; got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn search_case_insensitive() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "spawntool"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("SpawnTool"));
    }

    #[tokio::test]
    async fn search_by_description_keyword() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "plan"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("EnterPlanMode"));
    }

    #[tokio::test]
    async fn search_excludes_non_deferred() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "Read"})).await;
        // "Read" is not deferred, should not appear in results
        assert!(
            !result.content.contains("\"name\": \"Read\"")
                || result.content.contains("No deferred tools")
        );
    }

    #[tokio::test]
    async fn search_no_match() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": "nonexistent"})).await;
        assert!(!result.is_error);
        assert!(result.content.contains("No deferred tools"));
    }

    #[tokio::test]
    async fn search_empty_query_returns_error() {
        let tool = ToolSearchTool::new(build_tool_defs());
        let result = tool.execute(json!({"query": ""})).await;
        assert!(result.is_error);
    }
}
