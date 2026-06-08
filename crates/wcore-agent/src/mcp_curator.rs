//! F17 MCP tool curation.
//!
//! Default top-K ranking by keyword overlap (user message ↔ tool description)
//! plus a small recency boost from the M2 audit log when available. Always
//! preserves "rescue" tools (Read/Grep/Glob/Edit/Write/Bash) regardless of
//! score so the agent never loses basic file-access affordances.
//!
//! Scoring is intentionally cheap and explainable. F12 GEPA evolution (W10)
//! replaces this with learned weights.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct RankedTool {
    pub server_name: String,
    pub tool_name: String,
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct CurationInput<'a> {
    /// Most recent user message in the turn — the keyword overlap source.
    pub user_message: &'a str,
    /// (server, tool, description) triples — verbatim from
    /// `McpManager::all_tools()` (mapped at the call site).
    pub tools: &'a [(String, String, String)],
    /// `tool_name -> uses-in-last-N-seconds`. From
    /// `wcore_memory::audit::AuditLog::recent_tool_uses` when available;
    /// empty map otherwise (graceful degrade to keyword-only ranking).
    pub recent_usage: &'a HashMap<String, u64>,
}

const RESCUE_TOOLS: &[&str] = &["Read", "Grep", "Glob", "Edit", "Write", "Bash"];

pub struct McpCurator {
    top_k: usize,
}

impl McpCurator {
    pub fn new(top_k: usize) -> Self {
        Self { top_k }
    }

    pub fn curate(&self, input: &CurationInput<'_>) -> Vec<RankedTool> {
        let msg_lower = input.user_message.to_lowercase();
        let msg_terms: Vec<&str> = msg_lower
            .split_whitespace()
            .filter(|t| t.len() > 3)
            .collect();

        let mut ranked: Vec<RankedTool> = input
            .tools
            .iter()
            .map(|(server, tool, desc)| {
                let desc_lower = desc.to_lowercase();
                let overlap = msg_terms.iter().filter(|t| desc_lower.contains(*t)).count() as f64;
                let usage = *input.recent_usage.get(tool).unwrap_or(&0) as f64;
                // Rescue tools get a baseline floor so they always rank.
                let rescue = if RESCUE_TOOLS.contains(&tool.as_str()) {
                    100.0
                } else {
                    0.0
                };
                RankedTool {
                    server_name: server.clone(),
                    tool_name: tool.clone(),
                    score: overlap * 10.0 + usage * 0.5 + rescue,
                }
            })
            .collect();

        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked.truncate(self.top_k);
        ranked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescue_tools_survive_zero_overlap() {
        let curator = McpCurator::new(2);
        let tools = vec![
            ("a".into(), "Read".into(), "completely unrelated".into()),
            (
                "b".into(),
                "OtherTool".into(),
                "very rust bug rust bug".into(),
            ),
            ("c".into(), "ThirdTool".into(), "rust rust rust rust".into()),
        ];
        let out = curator.curate(&CurationInput {
            user_message: "rust bug",
            tools: &tools,
            recent_usage: &Default::default(),
        });
        let names: Vec<&str> = out.iter().map(|r| r.tool_name.as_str()).collect();
        assert!(names.contains(&"Read"));
    }
}
