use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolResult};

use crate::Tool;
use crate::context::ToolContext;
use crate::file_cache::{FileStateCache, update_cache_after_write};
use crate::path_validation::validate_user_path;
use crate::unsaved_work::{Mode, UnsavedWorkGuard, Verdict};

pub struct WriteTool {
    file_cache: Option<Arc<RwLock<FileStateCache>>>,
    /// INV-2: session-scoped record of the user's unsaved work, so a
    /// whole-file overwrite can never silently delete it. Shared with the
    /// Edit tool and with every sub-agent's tools, so one baseline and one
    /// agent-authored set govern both write surfaces.
    unsaved: Arc<UnsavedWorkGuard>,
}

impl WriteTool {
    /// Create a WriteTool with optional file state cache.
    ///
    /// When cache is `Some`, the tool updates the cache after each successful
    /// write so that subsequent Edit/Read calls see the latest content and mtime.
    ///
    /// No "must Read first" guard: Write is intended for creating new files
    /// or complete rewrites.
    ///
    /// P2: a complete rewrite still may not destroy work the user has not
    /// saved. Every overwrite is checked against
    /// [`crate::unsaved_work::UnsavedWorkGuard`] and refused when it would
    /// drop a line that is on disk but in no commit. This guard is always
    /// on — it needs no cache and no wiring.
    ///
    /// Pass `None` to disable cache integration (legacy behavior).
    pub fn new(file_cache: Option<Arc<RwLock<FileStateCache>>>) -> Self {
        Self {
            file_cache,
            unsaved: UnsavedWorkGuard::shared(),
        }
    }

    /// Use `guard` instead of the process-wide one.
    ///
    /// The shared guard writes its recovery snapshots under the real profile
    /// home, which a test process must never touch; a test hands in a guard
    /// rooted in its own temporary directory. A host that runs several
    /// independent sessions in one process can use it for the same reason.
    pub fn with_unsaved_guard(mut self, guard: Arc<UnsavedWorkGuard>) -> Self {
        self.unsaved = guard;
        self
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "Write"
    }

    fn description(&self) -> &str {
        "Writes content to a file, creating parent directories if needed.\n\n\
         Usage:\n\
         - This tool overwrites the existing file completely (not append).\n\
         - If the file already exists, you must use Read first to see its current content.\n\
         - Prefer Edit over Write for modifying existing files — Edit only sends the diff.\n\
         - Use Write only for creating new files or complete rewrites.\n\
         - A rewrite that would delete a line present on disk but absent from the \
         file's last commit is refused: that is unsaved user work. Reproduce those \
         lines in the content you write."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(file_path) = input["file_path"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };
        let Some(content) = input["content"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: content".to_string(),
                is_error: true,
            };
        };

        // Wave SD SECURITY MAJOR #14: validate the LLM-supplied path
        // before touching the disk. Refuses relative paths, traversal,
        // null bytes, and a deny-list of obvious system secrets.
        let validated = match validate_user_path(Path::new(file_path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to write {file_path}: {e}"),
                    is_error: true,
                };
            }
        };
        let path = validated.as_path();
        let existed = path.exists();

        // P2: never let a whole-file rewrite delete work the user has not
        // saved. Checked before any disk mutation so a refusal leaves the file
        // exactly as it was. On a create there is nothing to lose, and
        // recording the empty baseline now is what keeps the agent's own later
        // rewrites of its own file free.
        let previous = if existed {
            std::fs::read_to_string(path).unwrap_or_default()
        } else {
            String::new()
        };
        let mut unsaved_note = String::new();
        if existed {
            match self
                .unsaved
                .assess(path, file_path, &previous, content, Mode::Rewrite)
            {
                Verdict::Proceed => {}
                Verdict::ProceedWithSnapshot(note) => unsaved_note = note,
                Verdict::Refuse(refusal) => {
                    return ToolResult {
                        content: refusal,
                        is_error: true,
                    };
                }
            }
        }

        // Create parent directories
        if let Some(parent) = path.parent().filter(|p| !p.exists()) {
            match std::fs::create_dir_all(parent) {
                Ok(()) => {}
                Err(e) => {
                    return ToolResult {
                        content: format!("Failed to create directories: {}", e),
                        is_error: true,
                    };
                }
            }
        }

        // Write atomically: write to temp file, then rename.
        let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
        if let Err(e) = std::fs::write(&tmp_path, content) {
            return ToolResult {
                content: format!("Failed to write file: {}", e),
                is_error: true,
            };
        }

        if let Err(e) = std::fs::rename(&tmp_path, path) {
            // Fallback: direct write if rename fails (cross-device)
            let _ = std::fs::remove_file(&tmp_path);
            if let Err(e) = std::fs::write(path, content) {
                return ToolResult {
                    content: format!("Failed to write file: {}", e),
                    is_error: true,
                };
            }
            if let Some(cache_arc) = &self.file_cache {
                update_cache_after_write(cache_arc, path, content);
            }
            self.unsaved.note_written(path, &previous, content);

            return ToolResult {
                content: format!(
                    "Updated {} (rename failed: {}, used direct write){}",
                    file_path, e, unsaved_note
                ),
                is_error: false,
            };
        }

        if let Some(cache_arc) = &self.file_cache {
            update_cache_after_write(cache_arc, path, content);
        }
        self.unsaved.note_written(path, &previous, content);

        let line_count = content.lines().count();
        let action = if existed { "Updated" } else { "Created" };
        ToolResult {
            content: format!("{action} {file_path} ({line_count} lines){unsaved_note}"),
            is_error: false,
        }
    }

    /// W8b — vfs-aware variant. Routes the write through `ctx.vfs`
    /// (RealFs at top-level, SandboxedFs for sub-agents) so sandbox
    /// enforcement applies. Wave SD adds the same `validate_user_path`
    /// shape check as the legacy entry so a top-level (non-sandboxed)
    /// ctx can't be used as a bypass for the path discipline.
    /// Trades the legacy tmp+rename atomicity for VFS-trait portability;
    /// the `RealFs::write` impl still creates parent dirs.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let Some(file_path) = input["file_path"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };
        let Some(content) = input["content"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: content".to_string(),
                is_error: true,
            };
        };

        let validated = match validate_user_path(Path::new(file_path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to write {file_path}: {e}"),
                    is_error: true,
                };
            }
        };
        let path = validated.as_path();
        let existed = ctx.vfs.exists(path).await.unwrap_or(false);

        // INV-2: same unsaved-work assessment as the legacy path, before the
        // write. The pre-image is read back through the SAME vfs this call
        // will write to, so a sandboxed sub-agent is judged against the bytes
        // it can actually see rather than against the real filesystem. A path
        // the sandbox would reject never reaches the guard at all — the
        // sandbox denial, not a message quoting that file's lines, is the
        // right answer there.
        let previous = if existed {
            ctx.vfs
                .read(path)
                .await
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let mut unsaved_note = String::new();
        if existed {
            match self
                .unsaved
                .assess(path, file_path, &previous, content, Mode::Rewrite)
            {
                Verdict::Proceed => {}
                Verdict::ProceedWithSnapshot(note) => unsaved_note = note,
                Verdict::Refuse(refusal) => {
                    return ToolResult {
                        content: refusal,
                        is_error: true,
                    };
                }
            }
        }

        // W8b.2.A D.4 — mark this write as engine-originated BEFORE the
        // actual write so an upstream FileWatcher can debounce its own
        // change event and not feed it back as an "external edit".
        // Skipped when no notifier is wired (the test-default case).
        if let Some(n) = ctx.file_write_notifier.as_ref() {
            n.note_self_originated_write(path).await;
        }

        if let Err(e) = ctx.vfs.write(path, content.as_bytes()).await {
            return ToolResult {
                content: format!("Failed to write file: {e}"),
                is_error: true,
            };
        }

        if let Some(cache_arc) = &self.file_cache {
            update_cache_after_write(cache_arc, path, content);
        }
        self.unsaved.note_written(path, &previous, content);

        let line_count = content.lines().count();
        let action = if existed { "Updated" } else { "Created" };
        ToolResult {
            content: format!("{action} {file_path} ({line_count} lines){unsaved_note}"),
            is_error: false,
        }
    }

    fn max_result_size(&self) -> usize {
        10_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        ToolEffectContract::default()
    }

    fn describe(&self, input: &Value) -> String {
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        format!("Write to {}", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    use crate::Tool;
    use crate::file_cache::file_mtime_ms;
    use wcore_config::file_cache::FileCacheConfig;

    /// A Write tool whose recovery snapshots land in a throwaway directory
    /// rather than the real `~/.wayland`.
    fn tool(cache: Option<Arc<RwLock<FileStateCache>>>) -> WriteTool {
        WriteTool::new(cache).with_unsaved_guard(Arc::new(
            crate::unsaved_work::UnsavedWorkGuard::with_snapshot_root(
                std::env::temp_dir().join("wcore-tools-test-unsaved-snapshots"),
            ),
        ))
    }

    fn make_cache() -> Arc<RwLock<FileStateCache>> {
        let config = FileCacheConfig {
            max_entries: 100,
            max_size_bytes: 25 * 1024 * 1024,
            enabled: true,
        };
        Arc::new(RwLock::new(FileStateCache::new(&config)))
    }

    // -- Legacy tests (no cache) --

    #[tokio::test]
    async fn test_write_new_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("hello.txt");

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "hello world"
        });

        let tool = tool(None);
        let result = tool.execute(input).await;

        assert!(
            !result.is_error,
            "expected success, got: {}",
            result.content
        );
        assert!(file_path.exists(), "file should exist after write");
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("subdir/nested/file.txt");

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "nested content"
        });

        let tool = tool(None);
        let result = tool.execute(input).await;

        assert!(
            !result.is_error,
            "expected success, got: {}",
            result.content
        );
        assert!(
            file_path.parent().unwrap().exists(),
            "parent dirs should be created"
        );
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "nested content"
        );
    }

    #[tokio::test]
    async fn test_write_overwrite_existing() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("overwrite.txt");

        let tool = tool(None);

        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "original"
        });
        let result1 = tool.execute(input1).await;
        assert!(!result1.is_error);
        assert!(result1.content.contains("Created"));

        let input2 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "replaced"
        });
        let result2 = tool.execute(input2).await;
        assert!(!result2.is_error);
        assert!(result2.content.contains("Updated"));

        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "replaced");
    }

    #[tokio::test]
    async fn test_write_file_content_matches() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("exact.txt");

        let content = "line 1\nline 2\nline 3\n";
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": content
        });

        let tool = tool(None);
        let result = tool.execute(input).await;

        assert!(
            !result.is_error,
            "expected success, got: {}",
            result.content
        );

        let read_back = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            read_back, content,
            "read-back content must exactly match written content"
        );
    }

    // -- Cache integration tests --

    #[tokio::test]
    async fn write_populates_cache() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("cached.txt");

        let cache = make_cache();
        let tool = tool(Some(cache.clone()));

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "cached content"
        });
        let result = tool.execute(input).await;
        assert!(!result.is_error, "write failed: {}", result.content);

        // Cache should have an entry with correct mtime.
        let disk_mtime = file_mtime_ms(&file_path).unwrap();
        let mut c = cache.write().unwrap();
        let cached = c
            .get(&file_path)
            .expect("file should be in cache after write");
        assert_eq!(cached.mtime_ms, disk_mtime);
        assert!(cached.content.contains("cached content"));
    }

    #[tokio::test]
    async fn write_then_edit_succeeds() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("write_edit.txt");

        let cache = make_cache();
        let write_tool = tool(Some(cache.clone()));
        let edit_tool = crate::edit::EditTool::new(Some(cache)).with_unsaved_guard(Arc::new(
            crate::unsaved_work::UnsavedWorkGuard::with_snapshot_root(
                std::env::temp_dir().join("wcore-tools-test-unsaved-snapshots"),
            ),
        ));

        // Write creates the file and populates cache.
        let write_input = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "hello world"
        });
        let wr = write_tool.execute(write_input).await;
        assert!(!wr.is_error, "write failed: {}", wr.content);

        // Edit should succeed without needing a separate Read.
        let edit_input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye"
        });
        let er = edit_tool.execute(edit_input).await;
        assert!(!er.is_error, "edit after write failed: {}", er.content);
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "goodbye world"
        );
    }

    #[tokio::test]
    async fn write_overwrite_updates_cache_mtime() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("overwrite_cache.txt");

        let cache = make_cache();
        let tool = tool(Some(cache.clone()));

        // First write.
        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "v1"
        });
        tool.execute(input1).await;

        let mtime1 = {
            let mut c = cache.write().unwrap();
            c.get(&file_path).unwrap().mtime_ms
        };

        // Brief delay to ensure mtime changes.
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Second write.
        let input2 = json!({
            "file_path": file_path.to_str().unwrap(),
            "content": "v2"
        });
        tool.execute(input2).await;

        let mtime2 = {
            let mut c = cache.write().unwrap();
            c.get(&file_path).unwrap().mtime_ms
        };

        assert!(
            mtime2 >= mtime1,
            "cache mtime should update after overwrite"
        );
    }
}
