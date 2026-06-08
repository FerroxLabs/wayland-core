//! W8b C.7 — `RollbackTool`.
//!
//! Restores a file to its state N edit-steps ago, where each step
//! corresponds to a snapshot the engine took before a previous
//! `Write`/`Edit` to the same path (via `FileHistory::snapshot`).
//!
//! Lives in `wcore-agent` rather than `wcore-tools` because it consumes
//! the `FileHistory` snapshot store — which itself depends on the
//! engine's root-level `RealFs` (audit F9). `SkillTool` and `SpawnTool`
//! follow the same pattern (agent-level tools that ride on engine
//! state).
//!
//! Conflict handling (suspend semantics): if the live file's current
//! bytes don't match the most-recent snapshot's bytes, the file was
//! changed externally since the engine's last write. Rolling back now
//! would clobber unsaved work. The tool returns an `is_error: true`
//! result tagged `SUSPEND:` so the orchestration layer can surface an
//! `S4 Suspend` to the host instead of silently overwriting.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::file_history::{FileHistory, FileHistoryError, byte_digest};

/// JSON input shape for the Rollback tool.
#[derive(Deserialize)]
struct RollbackInput {
    /// Absolute path to the file to roll back.
    path: PathBuf,
    /// Number of edit-steps to roll back. 0 = restore to the most-recent
    /// pre-edit snapshot.
    #[serde(default)]
    steps: usize,
}

/// Tag prefix in the `content` field that signals an `S4 Suspend` to
/// the orchestration / host layer. Hosts MAY surface this verbatim;
/// engines MUST treat it as an error result.
const SUSPEND_PREFIX: &str = "SUSPEND: ";

/// Rollback meta-tool. Cheap to clone (single Arc field).
pub struct RollbackTool {
    history: Arc<FileHistory>,
}

impl RollbackTool {
    pub fn new(history: Arc<FileHistory>) -> Self {
        Self { history }
    }
}

#[async_trait]
impl Tool for RollbackTool {
    fn name(&self) -> &str {
        "Rollback"
    }

    fn description(&self) -> &str {
        "Restore a file to its state N edit-steps ago.\n\n\
         Usage:\n\
         - path: absolute path to the file to restore.\n\
         - steps: 0 (default) = restore to the most-recent pre-edit \
           snapshot; higher values walk further back. Capped at 10 \
           snapshots/file (FIFO eviction).\n\
         - If the live file has been modified externally since the \
           engine's last write, the tool refuses and returns a Suspend \
           result instead of clobbering unsaved work."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to roll back"
                },
                "steps": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Number of edit-steps to roll back (0 = most recent)"
                }
            },
            "required": ["path"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    /// `execute(input)` does NOT have access to a `ctx.vfs`, so the
    /// legacy entry point falls through to the ctx-aware variant with
    /// a RealFs context. Real callers always go through
    /// `execute_with_ctx` via the dispatcher.
    async fn execute(&self, input: Value) -> ToolResult {
        self.execute_with_ctx(input, &ToolContext::test_default())
            .await
    }

    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let RollbackInput { path, steps } = match serde_json::from_value::<RollbackInput>(input) {
            Ok(v) => v,
            Err(e) => {
                return ToolResult {
                    content: format!("invalid Rollback input: {e}"),
                    is_error: true,
                };
            }
        };

        // External-change guard: if the engine has recorded a post-write
        // digest for this path, the live file must still match it. If it
        // doesn't, the user (or some external process) wrote to the file
        // between the engine's last write and this rollback request —
        // applying the snapshot now would clobber that work. If no
        // engine-write digest is recorded (e.g., first run, or test
        // fixture without an explicit `record_post_write_digest`) the
        // guard skips and rollback proceeds normally.
        if let Some(engine_digest) = self.history.last_engine_write_digest(&path)
            && let Ok(current_bytes) = ctx.vfs.read(&path).await
        {
            let current_digest = byte_digest(&current_bytes);
            if current_digest != engine_digest {
                return ToolResult {
                    content: format!(
                        "{SUSPEND_PREFIX}file {path:?} changed externally since the engine's last \
                         write; rolling back would clobber unsaved work — manual review required."
                    ),
                    is_error: true,
                };
            }
        }

        let snapshot_bytes = match self.history.read_snapshot(&path, steps).await {
            Ok(b) => b,
            Err(FileHistoryError::StepOutOfRange {
                requested,
                available,
                ..
            }) => {
                return ToolResult {
                    content: format!(
                        "only {available} snapshots available for {path:?}; requested {requested}"
                    ),
                    is_error: true,
                };
            }
            Err(FileHistoryError::NoSnapshots { .. }) => {
                return ToolResult {
                    content: format!("no snapshots recorded for {path:?}"),
                    is_error: true,
                };
            }
            Err(e) => {
                return ToolResult {
                    content: format!("rollback read failed: {e}"),
                    is_error: true,
                };
            }
        };

        if let Err(e) = ctx.vfs.write(&path, &snapshot_bytes).await {
            return ToolResult {
                content: format!("rollback write failed: {e}"),
                is_error: true,
            };
        }

        ToolResult {
            content: format!("restored {path:?} to -{steps} state"),
            is_error: false,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    fn describe(&self, input: &Value) -> String {
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("?");
        let steps = input.get("steps").and_then(|v| v.as_u64()).unwrap_or(0);
        format!("Rollback {path} (-{steps})")
    }
}

/// Returns true if `content` carries the `SUSPEND:` marker. Used by
/// orchestration to decide whether to emit an `S4 Suspend` event.
pub fn is_suspend_marker(content: &str) -> bool {
    content.starts_with(SUSPEND_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspend_marker_round_trip() {
        let s = format!("{SUSPEND_PREFIX}something happened");
        assert!(is_suspend_marker(&s));
        assert!(!is_suspend_marker("no marker"));
    }
}
