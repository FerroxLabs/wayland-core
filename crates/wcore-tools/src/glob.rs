use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::context::ToolContext;

const MAX_RESULTS: usize = 100;

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> &str {
        "Fast file pattern matching tool that works with any codebase size.\n\n\
         - Supports glob patterns like \"**/*.rs\" or \"src/**/*.ts\".\n\
         - Returns matching file paths sorted by modification time (newest first).\n\
         - Returns at most 100 results. Only returns files, not directories.\n\
         - The path parameter defaults to the current working directory.\n\
         - Use this tool when you need to find files by name or extension patterns."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern, e.g. \"**/*.rs\""
                },
                "path": {
                    "type": "string",
                    "description": "Root directory (default: cwd)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(pattern) = input["pattern"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: pattern".to_string(),
                is_error: true,
            };
        };

        let root = input["path"].as_str().unwrap_or(".");
        let root_path = Path::new(root);

        // Build full glob pattern
        let full_pattern = if pattern.starts_with('/') {
            pattern.to_string()
        } else {
            format!("{}/{}", root_path.display(), pattern)
        };

        let entries = match glob::glob(&full_pattern) {
            Ok(paths) => paths,
            Err(e) => {
                return ToolResult {
                    content: format!("Invalid glob pattern: {}", e),
                    is_error: true,
                };
            }
        };

        let mut files: Vec<(std::time::SystemTime, String)> = Vec::new();

        for entry in entries {
            if files.len() >= MAX_RESULTS {
                break;
            }

            let Ok(path) = entry else {
                continue;
            };
            if !path.is_file() {
                continue;
            }

            let mtime = path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

            // Make path relative to root
            let display_path = path
                .strip_prefix(root_path)
                .unwrap_or(&path)
                .display()
                .to_string();

            files.push((mtime, display_path));
        }

        // Sort by modification time, newest first
        files.sort_by_key(|f| std::cmp::Reverse(f.0));

        if files.is_empty() {
            return ToolResult {
                content: "No files matched the pattern".to_string(),
                is_error: false,
            };
        }

        let result: Vec<String> = files.into_iter().map(|(_, path)| path).collect();
        ToolResult {
            content: result.join("\n"),
            is_error: false,
        }
    }

    /// W8b — vfs-aware variant. Glob walks the disk directly via the
    /// `glob` crate, so we gate the user-supplied `path` argument
    /// through `ctx.vfs.exists()` first. Top-level RealFs is a no-op;
    /// sandboxed sub-agents are clamped to their root.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        // The sandboxed path validates `path` via the VFS, but `execute()`
        // builds the glob from `pattern` directly against the real filesystem.
        // An absolute pattern (`/etc/**`) or one containing `..` escapes the
        // jail and enumerates paths outside it, so reject those shapes here —
        // only on the sandboxed entry, leaving the direct `execute()` path
        // unchanged.
        if let Some(pattern) = input["pattern"].as_str() {
            let pattern_path = Path::new(pattern);
            // Reject any pattern that anchors outside the relative jail.
            // `is_absolute()` alone is insufficient: on Windows a leading
            // `/` or `\` (e.g. `/etc/**`) is NOT absolute (no drive letter),
            // so it would slip past and escape the sandbox. Inspecting the
            // path components catches every anchor shape on every platform:
            //   - `Prefix`     — Windows drive (`C:`) or UNC root
            //   - `RootDir`    — a leading `/` or `\` (yielded on Unix AND
            //                    Windows for root-relative patterns)
            //   - `ParentDir`  — a `..` traversal segment
            // Legitimate relative patterns (`*.rs`, `**/*.rs`, `src/**/*.ts`)
            // contain only `Normal`/`CurDir` components and pass through.
            if pattern_path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::Prefix(_)
                        | std::path::Component::RootDir
                        | std::path::Component::ParentDir
                )
            }) {
                return ToolResult {
                    content: format!(
                        "Glob refused: pattern {pattern:?} is absolute or contains `..` traversal, \
                         which would escape the sandbox"
                    ),
                    is_error: true,
                };
            }
        }

        let path_arg = input["path"].as_str().unwrap_or(".");
        let path = Path::new(path_arg);
        if let Err(e) = ctx.vfs.exists(path).await {
            return ToolResult {
                content: format!("Glob refused: path {path_arg:?} rejected by sandbox: {e}"),
                is_error: true,
            };
        }
        self.execute(input).await
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
        format!("Search for {}", pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    use wcore_types::tool::ToolResult;

    async fn run_glob(pattern: &str, path: &str) -> ToolResult {
        let tool = GlobTool;
        let input = json!({ "pattern": pattern, "path": path });
        tool.execute(input).await
    }

    #[tokio::test]
    async fn test_glob_matches_pattern() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        fs::write(base.join("main.rs"), "fn main() {}").unwrap();
        fs::write(base.join("lib.rs"), "pub mod lib;").unwrap();
        fs::write(base.join("notes.txt"), "some notes").unwrap();
        fs::write(base.join("readme.md"), "# Readme").unwrap();

        let result = run_glob("*.rs", base.to_str().unwrap()).await;

        assert!(!result.is_error, "glob should succeed");
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 2, "should match exactly 2 .rs files");
        for line in &lines {
            assert!(
                line.ends_with(".rs"),
                "each match should be a .rs file, got: {}",
                line
            );
        }
        assert!(
            !result.content.contains("notes.txt"),
            "should not include .txt files"
        );
        assert!(
            !result.content.contains("readme.md"),
            "should not include .md files"
        );
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        fs::write(base.join("main.rs"), "fn main() {}").unwrap();
        fs::write(base.join("lib.rs"), "pub mod lib;").unwrap();

        let result = run_glob("*.xyz", base.to_str().unwrap()).await;

        assert!(!result.is_error, "no-match glob should not be an error");
        assert_eq!(result.content, "No files matched the pattern");
    }

    #[tokio::test]
    async fn test_glob_with_limit() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        for i in 0..5 {
            fs::write(
                base.join(format!("file_{}.txt", i)),
                format!("content {}", i),
            )
            .unwrap();
        }

        let result = run_glob("*.txt", base.to_str().unwrap()).await;

        assert!(!result.is_error, "glob should succeed");
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 5, "all 5 files should be returned");
    }

    #[tokio::test]
    async fn test_glob_rejects_absolute_and_traversal_patterns_under_sandbox() {
        use std::sync::Arc;
        use tokio_util::sync::CancellationToken;

        use crate::context::ToolContext;
        use crate::vfs::{RealFs, SandboxedFs};
        use crate::{NullToolOutputSink, ToolOutputSink};

        let dir = tempdir().unwrap();
        let base = dir.path();
        fs::write(base.join("main.rs"), "fn main() {}").unwrap();

        let vfs = Arc::new(SandboxedFs::new(RealFs, base));
        let ctx = ToolContext::new(
            "test",
            CancellationToken::new(),
            vfs,
            None,
            Arc::new(NullToolOutputSink) as Arc<dyn ToolOutputSink>,
        );
        let tool = GlobTool;

        // Absolute pattern escapes the jail and must be refused.
        let abs = tool
            .execute_with_ctx(
                json!({ "pattern": "/etc/**", "path": base.to_str().unwrap() }),
                &ctx,
            )
            .await;
        assert!(abs.is_error, "absolute pattern must be rejected");
        assert!(abs.content.contains("escape the sandbox"));

        // `..` traversal in the pattern also escapes and must be refused.
        let dotdot = tool
            .execute_with_ctx(
                json!({ "pattern": "../**/*.rs", "path": base.to_str().unwrap() }),
                &ctx,
            )
            .await;
        assert!(dotdot.is_error, "traversal pattern must be rejected");
        assert!(dotdot.content.contains("escape the sandbox"));
    }

    #[tokio::test]
    async fn test_glob_recursive() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        // Create nested directory structure
        let sub_a = base.join("a");
        let sub_b = base.join("a").join("b");
        fs::create_dir_all(&sub_b).unwrap();

        fs::write(base.join("root.txt"), "root level").unwrap();
        fs::write(sub_a.join("mid.txt"), "middle level").unwrap();
        fs::write(sub_b.join("deep.txt"), "deep level").unwrap();
        // Non-matching file
        fs::write(sub_a.join("skip.rs"), "not a txt").unwrap();

        let result = run_glob("**/*.txt", base.to_str().unwrap()).await;

        assert!(!result.is_error, "recursive glob should succeed");
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 3, "should find 3 .txt files across all levels");
        assert!(
            result.content.contains("root.txt"),
            "should include root-level file"
        );
        assert!(
            result.content.contains("mid.txt"),
            "should include mid-level file"
        );
        assert!(
            result.content.contains("deep.txt"),
            "should include deep-level file"
        );
        assert!(
            !result.content.contains("skip.rs"),
            "should not include .rs files"
        );
    }
}
