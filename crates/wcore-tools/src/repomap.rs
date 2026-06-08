//! W3→W4 hand-off: thin Tool adapter over `wcore_repomap::RepoMap`.
//!
//! Design contract §5.6 "Injection" specifies the RepoMap tool with
//! parameters (query, file_limit, symbol_limit). This adapter implements
//! the `Tool` trait so the agent can invoke RepoMap through the same
//! registry surface as Grep/Read/etc.
//!
//! Read-only by construction: `RepoMap::build` walks the directory tree;
//! `render::render_compact` serialises in-memory state. No filesystem
//! writes.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::context::ToolContext;

#[derive(Debug, Deserialize)]
struct RepoMapInput {
    /// Optional query string. When present, render_compact's output is
    /// filtered to lines containing the query (case-insensitive substring).
    /// When absent, the full compact view is returned.
    #[serde(default)]
    query: Option<String>,
    /// Cap the number of files in the rendered output. Default 100.
    #[serde(default)]
    file_limit: Option<usize>,
    /// Cap the number of symbols per file in the rendered output. Default 50.
    #[serde(default)]
    symbol_limit: Option<usize>,
}

pub struct RepoMapTool {
    root: PathBuf,
}

impl RepoMapTool {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

#[async_trait]
impl Tool for RepoMapTool {
    fn name(&self) -> &str {
        "RepoMap"
    }

    fn description(&self) -> &str {
        "Return a compact, queryable index of the repository's symbols \
         (Rust + TypeScript). Parameters: `query` (substring filter), \
         `file_limit` (cap rendered files, default 100), `symbol_limit` \
         (cap symbols per file, default 50). Read-only."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "query":        {"type": "string"},
                "file_limit":   {"type": "integer", "minimum": 1},
                "symbol_limit": {"type": "integer", "minimum": 1}
            }
        })
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let parsed: RepoMapInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => {
                return ToolResult {
                    content: format!("invalid RepoMap input: {e}"),
                    is_error: true,
                };
            }
        };

        // Build is synchronous CPU work — offload via spawn_blocking so we
        // don't stall the tokio runtime on a 5K-file index.
        let root = self.root.clone();
        let map =
            match tokio::task::spawn_blocking(move || wcore_repomap::RepoMap::build(&root)).await {
                Ok(Ok(m)) => m,
                Ok(Err(e)) => {
                    return ToolResult {
                        content: format!("RepoMap::build failed: {e}"),
                        is_error: true,
                    };
                }
                Err(join_err) => {
                    return ToolResult {
                        content: format!("RepoMap task join error: {join_err}"),
                        is_error: true,
                    };
                }
            };

        let rendered = wcore_repomap::render::render_compact(&map);
        let filtered = match &parsed.query {
            Some(q) if !q.is_empty() => {
                let needle = q.to_lowercase();
                rendered
                    .lines()
                    .filter(|line| line.to_lowercase().contains(&needle))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => rendered,
        };

        // Honour file_limit + symbol_limit by truncating the output. The
        // current render shape is one file-block per file (header + symbol
        // lines); we cap by line count derived from limits as a coarse upper
        // bound. Refining the renderer to accept limits natively is a
        // follow-up.
        let file_limit = parsed.file_limit.unwrap_or(100);
        let symbol_limit = parsed.symbol_limit.unwrap_or(50);
        let max_lines = file_limit * (symbol_limit + 1) + 1;
        let trimmed = if filtered.lines().count() > max_lines {
            let head: Vec<&str> = filtered.lines().take(max_lines).collect();
            format!(
                "{}\n... (truncated; raise file_limit/symbol_limit to see more)",
                head.join("\n")
            )
        } else {
            filtered
        };

        ToolResult {
            content: trimmed,
            is_error: false,
        }
    }

    /// W8b — vfs-aware variant. RepoMap walks the disk directly via
    /// `wcore_repomap::RepoMap::build`, so we gate `self.root` through
    /// `ctx.vfs.exists()` first. Sandboxed sub-agents are clamped to
    /// their workspace root.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        if let Err(e) = ctx.vfs.exists(&self.root).await {
            return ToolResult {
                content: format!(
                    "RepoMap refused: root {:?} rejected by sandbox: {e}",
                    self.root
                ),
                is_error: true,
            };
        }
        self.execute(input).await
    }
}
