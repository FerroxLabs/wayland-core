//! T11 (Plan v2 Tier 2B): JSON Lines (`.jsonl`) streaming tool.
//!
//! A large-file-friendly tool for working with newline-delimited JSON.
//! Every operation streams the file line-by-line through a buffered
//! reader ([`BufReader::lines`]) — the whole file is **never** slurped
//! into memory, so a multi-gigabyte log can be sliced/counted/filtered
//! without an OOM.
//!
//! Operations (selected via the `operation` discriminator):
//!
//! - `slice` / `head` — return `limit` lines starting at `offset`
//!   (both 0-based; `head` is `slice` with `offset` defaulting to 0).
//! - `count` — total line count (cheap streaming pass).
//! - `filter` — return lines whose top-level JSON object has
//!   `key == value` (string compare against the JSON value's display
//!   form). Non-object or malformed lines never match.
//! - `validate` — report the 1-based line numbers of lines that are
//!   not well-formed JSON.
//!
//! Output is truncated per [`ToolOutputLimits`] (line count + per-line
//! length) so a pathological file can't blow the model's context.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::path_validation::validate_user_path;
use crate::tool_output_limits::ToolOutputLimits;

/// JSON Lines streaming tool. See the module docs for the operation set.
pub struct JsonlTool {
    limits: ToolOutputLimits,
}

impl Default for JsonlTool {
    fn default() -> Self {
        Self::new(ToolOutputLimits::default())
    }
}

impl JsonlTool {
    /// Build a `JsonlTool` with explicit output-truncation limits.
    pub fn new(limits: ToolOutputLimits) -> Self {
        Self { limits }
    }

    /// Open a validated path as a line-streaming buffered reader.
    fn open(&self, file_path: &str) -> Result<BufReader<File>, String> {
        let validated = validate_user_path(Path::new(file_path))
            .map_err(|e| format!("Refused to read {file_path}: {e}"))?;
        let file =
            File::open(&validated).map_err(|e| format!("Failed to open file {file_path}: {e}"))?;
        Ok(BufReader::new(file))
    }

    /// Cap each line to `max_line_length`, appending an ellipsis marker
    /// when the line was longer than the cap.
    fn clamp_line(&self, line: &str) -> String {
        let max = self.limits.max_line_length;
        if line.len() <= max {
            return line.to_string();
        }
        let mut end = max;
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}... [truncated]", &line[..end])
    }

    /// Join collected output lines, applying the `max_lines` cap with a
    /// trailing notice when lines were dropped.
    fn finish(&self, mut lines: Vec<String>, total_emitted: usize) -> String {
        let max = self.limits.max_lines;
        if total_emitted > max {
            lines.truncate(max);
            lines.push(format!(
                "... [truncated: showing {max} of {total_emitted} lines]"
            ));
        }
        lines.join("\n")
    }
}

#[async_trait]
impl Tool for JsonlTool {
    fn name(&self) -> &str {
        "Jsonl"
    }

    fn description(&self) -> &str {
        "Works with JSON Lines (.jsonl) files — newline-delimited JSON — in a \
         large-file-friendly, streaming way (the file is never loaded whole).\n\n\
         operation:\n\
         - \"slice\": return `limit` lines starting at `offset` (0-based).\n\
         - \"head\": like slice with offset defaulting to 0.\n\
         - \"count\": return the total number of lines.\n\
         - \"filter\": return lines whose top-level JSON object has key == value.\n\
         - \"validate\": report 1-based line numbers of malformed JSON lines.\n\n\
         file_path must be an absolute path."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the .jsonl file"
                },
                "operation": {
                    "type": "string",
                    "enum": ["slice", "head", "count", "filter", "validate"],
                    "description": "Operation to perform (default: head)"
                },
                "offset": {
                    "type": "integer",
                    "description": "slice: 0-based line to start from (default 0)"
                },
                "limit": {
                    "type": "integer",
                    "description": "slice/head: max lines to return (default 100)"
                },
                "key": {
                    "type": "string",
                    "description": "filter: top-level JSON key to match on"
                },
                "value": {
                    "type": "string",
                    "description": "filter: value the key must equal"
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(file_path) = input["file_path"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };

        let operation = input["operation"].as_str().unwrap_or("head");

        let reader = match self.open(file_path) {
            Ok(r) => r,
            Err(e) => {
                return ToolResult {
                    content: e,
                    is_error: true,
                };
            }
        };

        match operation {
            "count" => self.op_count(reader),
            "slice" | "head" => {
                let offset = if operation == "head" {
                    0
                } else {
                    input["offset"].as_u64().unwrap_or(0) as usize
                };
                let limit = input["limit"].as_u64().unwrap_or(100) as usize;
                self.op_slice(reader, offset, limit)
            }
            "filter" => {
                let Some(key) = input["key"].as_str() else {
                    return ToolResult {
                        content: "filter operation requires a 'key' parameter".to_string(),
                        is_error: true,
                    };
                };
                let Some(value) = input["value"].as_str() else {
                    return ToolResult {
                        content: "filter operation requires a 'value' parameter".to_string(),
                        is_error: true,
                    };
                };
                self.op_filter(reader, key, value)
            }
            "validate" => self.op_validate(reader),
            other => ToolResult {
                content: format!(
                    "Unknown operation '{other}'. Use slice, head, count, filter, or validate."
                ),
                is_error: true,
            },
        }
    }

    fn max_result_size(&self) -> usize {
        100_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let op = input
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("head");
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        format!("Jsonl {op} {path}")
    }
}

impl JsonlTool {
    /// `count` — streaming line tally.
    fn op_count(&self, reader: BufReader<File>) -> ToolResult {
        let mut total = 0usize;
        for line in reader.lines() {
            match line {
                Ok(_) => total += 1,
                Err(e) => {
                    return ToolResult {
                        content: format!("Error reading file: {e}"),
                        is_error: true,
                    };
                }
            }
        }
        ToolResult {
            content: total.to_string(),
            is_error: false,
        }
    }

    /// `slice` / `head` — emit `limit` lines starting at `offset`.
    fn op_slice(&self, reader: BufReader<File>, offset: usize, limit: usize) -> ToolResult {
        let mut out: Vec<String> = Vec::new();
        for (idx, line) in reader.lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    return ToolResult {
                        content: format!("Error reading file: {e}"),
                        is_error: true,
                    };
                }
            };
            if idx < offset {
                continue;
            }
            if out.len() >= limit {
                break;
            }
            out.push(self.clamp_line(&line));
        }
        let emitted = out.len();
        ToolResult {
            content: self.finish(out, emitted),
            is_error: false,
        }
    }

    /// `filter` — emit lines whose top-level JSON object has `key == value`.
    fn op_filter(&self, reader: BufReader<File>, key: &str, value: &str) -> ToolResult {
        let mut out: Vec<String> = Vec::new();
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    return ToolResult {
                        content: format!("Error reading file: {e}"),
                        is_error: true,
                    };
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(parsed) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let matches = parsed
                .get(key)
                .map(|v| match v {
                    Value::String(s) => s == value,
                    other => {
                        // Non-string JSON values (numbers, bools, null)
                        // are compared against their JSON display form.
                        let rendered = other.to_string();
                        rendered == value
                    }
                })
                .unwrap_or(false);
            if matches {
                out.push(self.clamp_line(&line));
            }
        }
        if out.is_empty() {
            return ToolResult {
                content: format!("No lines matched {key} == {value}"),
                is_error: false,
            };
        }
        let emitted = out.len();
        ToolResult {
            content: self.finish(out, emitted),
            is_error: false,
        }
    }

    /// `validate` — report 1-based line numbers of malformed JSON.
    fn op_validate(&self, reader: BufReader<File>) -> ToolResult {
        let mut bad: Vec<usize> = Vec::new();
        let mut total = 0usize;
        for (idx, line) in reader.lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    return ToolResult {
                        content: format!("Error reading file: {e}"),
                        is_error: true,
                    };
                }
            };
            total += 1;
            if line.trim().is_empty() {
                continue;
            }
            if serde_json::from_str::<Value>(&line).is_err() {
                bad.push(idx + 1);
            }
        }
        if bad.is_empty() {
            return ToolResult {
                content: format!("All {total} line(s) are valid JSON."),
                is_error: false,
            };
        }
        let shown: Vec<String> = bad
            .iter()
            .take(self.limits.max_lines)
            .map(|n| n.to_string())
            .collect();
        ToolResult {
            content: format!(
                "{} of {} line(s) malformed. Malformed line numbers: {}",
                bad.len(),
                total,
                shown.join(", ")
            ),
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;

    /// Write `contents` to `<dir>/<name>` and return the absolute path.
    fn write_fixture(dir: &Path, name: &str, contents: &str) -> String {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        drop(file);
        path.to_str().unwrap().to_string()
    }

    const SAMPLE: &str = "{\"id\":1,\"role\":\"user\"}\n\
                          {\"id\":2,\"role\":\"admin\"}\n\
                          {\"id\":3,\"role\":\"user\"}\n\
                          {\"id\":4,\"role\":\"guest\"}\n";

    #[tokio::test]
    async fn slice_returns_the_right_line_range() {
        let dir = tempdir().unwrap();
        let path = write_fixture(dir.path(), "data.jsonl", SAMPLE);

        let tool = JsonlTool::default();
        let result = tool
            .execute(json!({
                "file_path": path,
                "operation": "slice",
                "offset": 1,
                "limit": 2
            }))
            .await;

        assert!(!result.is_error);
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"id\":2"));
        assert!(lines[1].contains("\"id\":3"));
    }

    #[tokio::test]
    async fn count_is_correct() {
        let dir = tempdir().unwrap();
        let path = write_fixture(dir.path(), "data.jsonl", SAMPLE);

        let tool = JsonlTool::default();
        let result = tool
            .execute(json!({ "file_path": path, "operation": "count" }))
            .await;

        assert!(!result.is_error);
        assert_eq!(result.content, "4");
    }

    #[tokio::test]
    async fn filter_matches_the_right_lines() {
        let dir = tempdir().unwrap();
        let path = write_fixture(dir.path(), "data.jsonl", SAMPLE);

        let tool = JsonlTool::default();
        let result = tool
            .execute(json!({
                "file_path": path,
                "operation": "filter",
                "key": "role",
                "value": "user"
            }))
            .await;

        assert!(!result.is_error);
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"id\":1"));
        assert!(lines[1].contains("\"id\":3"));
    }

    #[tokio::test]
    async fn validate_flags_a_malformed_line_by_number() {
        let dir = tempdir().unwrap();
        // Line 2 is malformed (unterminated object).
        let contents = "{\"ok\":1}\n{\"broken\":\n{\"ok\":3}\n";
        let path = write_fixture(dir.path(), "mixed.jsonl", contents);

        let tool = JsonlTool::default();
        let result = tool
            .execute(json!({ "file_path": path, "operation": "validate" }))
            .await;

        assert!(!result.is_error);
        assert!(
            result.content.contains("line numbers: 2"),
            "expected line 2 flagged, got: {}",
            result.content
        );
        assert!(result.content.contains("1 of 3"));
    }

    #[tokio::test]
    async fn missing_file_is_an_error() {
        let dir = tempdir().unwrap();
        // Path inside a real tempdir so it is absolute on every OS, but
        // the file is never created.
        let path = dir.path().join("does_not_exist.jsonl");

        let tool = JsonlTool::default();
        let result = tool
            .execute(json!({ "file_path": path.to_str().unwrap() }))
            .await;

        assert!(result.is_error);
        assert!(result.content.contains("Failed to open file"));
    }

    #[tokio::test]
    async fn head_defaults_to_offset_zero() {
        let dir = tempdir().unwrap();
        let path = write_fixture(dir.path(), "data.jsonl", SAMPLE);

        let tool = JsonlTool::default();
        let result = tool
            .execute(json!({
                "file_path": path,
                "operation": "head",
                "limit": 2
            }))
            .await;

        assert!(!result.is_error);
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"id\":1"));
        assert!(lines[1].contains("\"id\":2"));
    }

    #[tokio::test]
    async fn filter_no_match_reports_cleanly() {
        let dir = tempdir().unwrap();
        let path = write_fixture(dir.path(), "data.jsonl", SAMPLE);

        let tool = JsonlTool::default();
        let result = tool
            .execute(json!({
                "file_path": path,
                "operation": "filter",
                "key": "role",
                "value": "nobody"
            }))
            .await;

        assert!(!result.is_error);
        assert!(result.content.contains("No lines matched"));
    }

    #[tokio::test]
    async fn line_count_cap_truncates_output() {
        let dir = tempdir().unwrap();
        // 10 lines, but cap max_lines at 3.
        let mut contents = String::new();
        for i in 0..10 {
            contents.push_str(&format!("{{\"n\":{i}}}\n"));
        }
        let path = write_fixture(dir.path(), "big.jsonl", &contents);

        let limits = ToolOutputLimits {
            max_lines: 3,
            ..ToolOutputLimits::default()
        };
        let tool = JsonlTool::new(limits);
        let result = tool
            .execute(json!({
                "file_path": path,
                "operation": "slice",
                "offset": 0,
                "limit": 100
            }))
            .await;

        assert!(!result.is_error);
        assert!(result.content.contains("truncated: showing 3 of 10"));
    }
}
