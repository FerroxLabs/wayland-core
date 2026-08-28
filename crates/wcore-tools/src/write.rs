use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolEffectKind, ToolResult};

use crate::Tool;
use crate::context::ToolContext;
use crate::effects::{
    FILESYSTEM_EFFECT_RECONCILER, FilesystemWriteAttempt, PreparedToolEffect, ToolEffectExecution,
    classify_filesystem_execution, prepare_filesystem_effect,
};
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
    /// The process-wide guard pins one baseline per repository for the whole
    /// session and is shared by every sub-agent, so a test that needs its own
    /// pins hands in an isolated guard; a host running several independent
    /// sessions in one process wants one for the same reason. There is no
    /// longer any snapshot directory under the profile home — recovery copies
    /// go to the repository's own object store.
    pub fn with_unsaved_guard(mut self, guard: Arc<UnsavedWorkGuard>) -> Self {
        self.unsaved = guard;
        self
    }

    /// W8b — vfs-aware write body. Routes the write through `ctx.vfs`
    /// (RealFs at top-level, SandboxedFs for sub-agents) so sandbox
    /// enforcement applies. Wave SD adds the same `validate_user_path`
    /// shape check as the legacy entry so a top-level (non-sandboxed)
    /// ctx can't be used as a bypass for the path discipline.
    /// Trades the legacy tmp+rename atomicity for VFS-trait portability;
    /// the `RealFs::write` impl still creates parent dirs.
    async fn write_through_vfs(
        &self,
        input: Value,
        ctx: &ToolContext,
        attempt: &mut FilesystemWriteAttempt,
    ) -> ToolResult {
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
        // ADV-8, vfs side: the same fail-open in the same shape. A read that
        // failed, and bytes that are not UTF-8, both became an empty
        // pre-image that the gate below waved through. Only the vfs saying
        // definitely "there is nothing here" may do that.
        let mut attributable = true;
        let mut judged: Option<Vec<u8>> = None;
        let previous = match ctx.vfs.read(path).await {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => {
                    judged = Some(text.as_bytes().to_vec());
                    text
                }
                Err(e) => {
                    let raw = e.into_bytes();
                    judged = Some(raw.clone());
                    if let Verdict::Refuse(refusal) = self.unsaved.assess_opaque(
                        path,
                        file_path,
                        Some(&raw),
                        "the bytes on disk are not valid UTF-8",
                    ) {
                        return ToolResult {
                            content: refusal,
                            is_error: true,
                        };
                    }
                    attributable = false;
                    String::new()
                }
            },
            Err(e) => match ctx.vfs.exists(path).await {
                // Definitely nothing there: an ordinary create.
                Ok(false) => String::new(),
                _ => {
                    if let Verdict::Refuse(refusal) =
                        self.unsaved
                            .assess_opaque(path, file_path, None, &e.to_string())
                    {
                        return ToolResult {
                            content: refusal,
                            is_error: true,
                        };
                    }
                    attributable = false;
                    String::new()
                }
            },
        };
        let mut unsaved_note = String::new();
        if !previous.is_empty() {
            match self
                .unsaved
                .assess(path, file_path, &previous, content, Mode::Rewrite)
            {
                Verdict::Proceed => {}
                Verdict::ProceedWithNote(note) => unsaved_note = note,
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

        // ADV-7, vfs side: same re-check, through the same vfs the write
        // goes to.
        let still = match ctx.vfs.read(path).await {
            Ok(now) => match judged.as_deref() {
                Some(before) if now == before => Ok(()),
                Some(_) => Err("its contents changed on disk".to_owned()),
                None => Err("something else created it".to_owned()),
            },
            Err(e) if judged.is_none() => match ctx.vfs.exists(path).await {
                Ok(false) => Ok(()),
                _ => Err(format!("it could no longer be read ({e})")),
            },
            Err(e) => Err(format!("it could no longer be read ({e})")),
        };
        if let Err(why) = still {
            return ToolResult {
                content: crate::unsaved_work::changed_under_write(file_path, &why),
                is_error: true,
            };
        }

        // F13: everything above this line left the target untouched, and
        // everything below it may not have. The filesystem cannot be asked
        // afterwards which side a failure fell on.
        *attempt = FilesystemWriteAttempt::Attempted;
        if let Err(e) = ctx.vfs.write(path, content.as_bytes()).await {
            return ToolResult {
                content: format!("Failed to write file: {e}"),
                is_error: true,
            };
        }
        *attempt = FilesystemWriteAttempt::Completed;

        if let Some(cache_arc) = &self.file_cache {
            update_cache_after_write(cache_arc, path, content);
        }
        // Not `previous` when the pre-image was not text: claiming the file
        // was empty would make every line of `content` agent-authored, and so
        // exempt from this guard for the rest of the session.
        let attribution_pre = if attributable {
            previous.as_str()
        } else {
            content
        };
        self.unsaved.note_written(path, attribution_pre, content);

        let line_count = content.lines().count();
        let action = if existed { "Updated" } else { "Created" };
        ToolResult {
            content: format!("{action} {file_path} ({line_count} lines){unsaved_note}"),
            is_error: false,
        }
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
         file's last commit is refused: that is unsaved user work. Carry those lines \
         into the content you write — in their changed form if what you are doing \
         changes them.\n\
         - Never route a rewrite around that refusal. Bash refuses a git command \
         that would throw the work tree away, but a write made through Bash (`sed -i`, \
         `>`, `rm`) is not checked by anything and destroys unsaved work irreversibly; \
         using one to apply a change this tool refused is the single worst thing you \
         can do to the user's file."
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
        // Gated on the pre-image, not on `existed`: a probe that reports
        // "no such file" on an error it could not classify would otherwise
        // wave the overwrite straight through. If there are bytes there, they
        // are judged, whatever `exists` said.
        //
        // ADV-8: and "there are bytes there" is decided by the read, not by
        // the read succeeding. `unwrap_or_default()` turned every failure —
        // a permission denied, a directory in the way, bytes that are not
        // UTF-8 — into an empty pre-image, which the gate below then waved
        // straight through. Measured as uid 65534 against a root-owned 0600
        // file in a writable directory: no refusal, no note, no copy, and the
        // file went `root:root 0600` -> `nobody:nogroup 0644`. Only a
        // definite "no such file" may produce an empty pre-image.
        let mut attributable = true;
        // The exact bytes the assessment below is about to judge, re-checked
        // immediately before the write lands (ADV-7).
        let mut judged: Option<Vec<u8>> = None;
        let previous = match std::fs::read_to_string(path) {
            Ok(text) => {
                judged = Some(text.as_bytes().to_vec());
                text
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                let raw = std::fs::read(path).ok();
                judged = raw.clone();
                if let Verdict::Refuse(refusal) =
                    self.unsaved
                        .assess_opaque(path, file_path, raw.as_deref(), &e.to_string())
                {
                    return ToolResult {
                        content: refusal,
                        is_error: true,
                    };
                }
                // Proven byte-identical to the pinned commit, so nothing on
                // disk is unsaved. Nothing may be claimed as agent-authored
                // either — there is no line model for a pre-image that is not
                // text — so the attribution record below is told this write
                // introduced nothing.
                attributable = false;
                String::new()
            }
        };
        let mut unsaved_note = String::new();
        if !previous.is_empty() {
            match self
                .unsaved
                .assess(path, file_path, &previous, content, Mode::Rewrite)
            {
                Verdict::Proceed => {}
                Verdict::ProceedWithNote(note) => unsaved_note = note,
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

        // ADV-7 / #1155: the assessment took a measured 13.5 ms, and a save
        // that landed inside it was destroyed uncopied while the note claimed
        // the prior contents were preserved. Re-reading the path immediately
        // before the rename only NARROWED that window — measured at 13 losses
        // in 200 with the re-check in place — because the read and the rename
        // are still two operations. `atomic_write_checked` publishes by an
        // atomic exchange, so the pre-image it judges IS the one it displaced.
        let unpublishable =
            match wcore_config::atomic_write_checked(path, content.as_bytes(), |observed| {
                crate::unsaved_work::pre_image_matches(observed, judged.as_deref())
            }) {
                Ok(Ok(())) => None,
                Ok(Err(why)) => {
                    return ToolResult {
                        content: crate::unsaved_work::changed_under_write(file_path, &why),
                        is_error: true,
                    };
                }
                Err(e) => Some(e),
            };

        if let Some(e) = unpublishable {
            // Fallback: direct write if the tempfile round trip fails at all
            // (a cross-device rename, or a directory that will not hold a
            // sibling). The guard above did not run, so this is the one path
            // that publishes unchecked — unchanged from before this fix.
            if let Err(e) = std::fs::write(path, content) {
                return ToolResult {
                    content: format!("Failed to write file: {}", e),
                    is_error: true,
                };
            }
            if let Some(cache_arc) = &self.file_cache {
                update_cache_after_write(cache_arc, path, content);
            }
            // Not `previous` when the pre-image was not text: claiming the file
            // was empty would make every line of `content` agent-authored, and so
            // exempt from this guard for the rest of the session.
            let attribution_pre = if attributable {
                previous.as_str()
            } else {
                content
            };
            self.unsaved.note_written(path, attribution_pre, content);

            return ToolResult {
                content: format!(
                    "Updated {} (atomic write failed: {}, used direct write){}",
                    file_path, e, unsaved_note
                ),
                is_error: false,
            };
        }

        if let Some(cache_arc) = &self.file_cache {
            update_cache_after_write(cache_arc, path, content);
        }
        // Not `previous` when the pre-image was not text: claiming the file
        // was empty would make every line of `content` agent-authored, and so
        // exempt from this guard for the rest of the session.
        let attribution_pre = if attributable {
            previous.as_str()
        } else {
            content
        };
        self.unsaved.note_written(path, attribution_pre, content);

        let line_count = content.lines().count();
        let action = if existed { "Updated" } else { "Created" };
        ToolResult {
            content: format!("{action} {file_path} ({line_count} lines){unsaved_note}"),
            is_error: false,
        }
    }

    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        self.write_through_vfs(input, ctx, &mut FilesystemWriteAttempt::default())
            .await
    }

    fn max_result_size(&self) -> usize {
        10_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    /// F13: a whole-file rewrite is filesystem-transactional *when the
    /// target can be identified before the write*.
    ///
    /// The kind is declared unconditionally because `effect_contract` sees
    /// only the input; whether THIS call actually gets a receipt is decided
    /// by [`Self::prepare_effect`], and orchestration narrows the recorded
    /// contract back to opaque when no receipt was produced.
    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        ToolEffectContract {
            kind: ToolEffectKind::FilesystemTransactional,
            reconciler: Some(FILESYSTEM_EFFECT_RECONCILER.to_string()),
        }
    }

    /// Bind this call to the exact preimage and postimage identities, so a
    /// crash between here and the terminal journal append can be answered by
    /// one read of the target instead of by a human.
    async fn prepare_effect(
        &self,
        input: &Value,
        ctx: &ToolContext,
    ) -> Result<Option<PreparedToolEffect>, ToolResult> {
        // Malformed input stays the ordinary path's error to report, with the
        // wording it already has; preparation only ever declines.
        let (Some(file_path), Some(content)) =
            (input["file_path"].as_str(), input["content"].as_str())
        else {
            return Ok(None);
        };
        let intended = content.as_bytes().to_vec();
        Ok(prepare_filesystem_effect(
            ctx.vfs.as_ref(),
            Path::new(file_path),
            input,
            move |_preimage| Some(intended),
        )
        .await)
    }

    async fn execute_prepared_effect(
        &self,
        prepared: PreparedToolEffect,
        ctx: &ToolContext,
    ) -> ToolEffectExecution {
        let mut attempt = FilesystemWriteAttempt::default();
        let result = self
            .write_through_vfs(prepared.invocation().clone(), ctx, &mut attempt)
            .await;
        classify_filesystem_execution(&prepared, ctx.vfs.as_ref(), attempt, result).await
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
            crate::unsaved_work::UnsavedWorkGuard::new_isolated(),
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
            crate::unsaved_work::UnsavedWorkGuard::new_isolated(),
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
