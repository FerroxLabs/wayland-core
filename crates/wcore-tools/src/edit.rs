use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::context::ToolContext;
use crate::file_cache::{FileStateCache, file_mtime_ms, update_cache_after_write};
use crate::fuzzy_match::fuzzy_find_and_replace;
use crate::path_validation::validate_user_path;

pub struct EditTool {
    file_cache: Option<Arc<RwLock<FileStateCache>>>,
    /// Rank 41: opt-in fuzzy fallback. When `true`, an exact-match failure
    /// (`old_string` not found, or found multiple times without
    /// `replace_all`) retries through [`fuzzy_find_and_replace`]'s 9-strategy
    /// chain. Default `false` so existing behavior and error messages are
    /// byte-identical on the happy path.
    fuzzy_fallback: bool,
}

impl EditTool {
    /// Create an EditTool with optional file state cache.
    ///
    /// When cache is `Some`, the tool enforces:
    /// - "Must Read first" guard (file must be in cache before editing)
    /// - Staleness detection (disk mtime must match cached mtime)
    /// - Post-write cache update (mtime + content refreshed after edit)
    ///
    /// Pass `None` to disable all cache-related guards (legacy behavior).
    ///
    /// The fuzzy fallback is OFF by default; opt in via
    /// [`EditTool::with_fuzzy_fallback`].
    pub fn new(file_cache: Option<Arc<RwLock<FileStateCache>>>) -> Self {
        Self {
            file_cache,
            fuzzy_fallback: false,
        }
    }

    /// Rank 41: enable (or disable) the fuzzy find-and-replace fallback.
    ///
    /// When enabled, an exact-match failure falls back to the 9-strategy
    /// [`fuzzy_find_and_replace`] chain (whitespace / indentation / Unicode
    /// drift tolerance). Builder-style so the existing one-arg `new()`
    /// signature stays back-compatible for every call site.
    pub fn with_fuzzy_fallback(mut self, enabled: bool) -> Self {
        self.fuzzy_fallback = enabled;
        self
    }

    /// Compute the post-edit content and replacement count, shared by both
    /// the legacy `execute` and the vfs-aware `execute_with_ctx` paths.
    ///
    /// The exact-match behavior and its error messages are preserved
    /// byte-for-byte on the happy path. Only when the exact attempt FAILS
    /// (`old_string` not found, or found multiple times without
    /// `replace_all`) AND `self.fuzzy_fallback` is on do we retry through
    /// the 9-strategy [`fuzzy_find_and_replace`] chain.
    ///
    /// Returns `Ok((new_content, match_count))` on success or `Err(message)`
    /// carrying the existing error string.
    fn compute_edit(
        &self,
        content: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> Result<(String, usize), String> {
        let match_count = content.matches(old_string).count();

        // Exact-match happy path — identical behavior to the original.
        if match_count == 1 || (match_count > 1 && replace_all) {
            let new_content = if replace_all {
                content.replace(old_string, new_string)
            } else {
                content.replacen(old_string, new_string, 1)
            };
            return Ok((new_content, match_count));
        }

        // CRLF reconciliation (issue #257). The Read tool emits file content via
        // `str::lines()`, which strips the trailing `\r` of every CRLF line, so the
        // model's `old_string` is LF-only even when the file on disk is CRLF. An
        // LF pattern can never exact-match `\r\n` content, producing a spurious
        // "old_string not found" loop on Windows-authored files. When the file is
        // CRLF and the supplied pattern is LF-only, retry the match against a
        // CRLF-translated copy of the pattern. The replacement is translated the
        // same way so the file keeps its original line endings; we operate on the
        // raw `content` (never re-normalized) so the surrounding bytes are intact.
        if old_string.contains('\n') && !old_string.contains('\r') && content.contains("\r\n") {
            let crlf_old = old_string.replace('\n', "\r\n");
            let crlf_match_count = content.matches(crlf_old.as_str()).count();
            if crlf_match_count == 1 || (crlf_match_count > 1 && replace_all) {
                // Mirror the model's LF intent into CRLF for the replacement text
                // so inserted/edited lines match the file's existing endings.
                let crlf_new = if new_string.contains('\n') && !new_string.contains('\r') {
                    new_string.replace('\n', "\r\n")
                } else {
                    new_string.to_string()
                };
                let new_content = if replace_all {
                    content.replace(crlf_old.as_str(), &crlf_new)
                } else {
                    content.replacen(crlf_old.as_str(), &crlf_new, 1)
                };
                return Ok((new_content, crlf_match_count));
            }
            // Multiple CRLF matches without `replace_all`: mirror the exact-path
            // "multiple matches" error rather than falling through to the
            // misleading "old_string not found" (the LF `match_count` is 0).
            if crlf_match_count > 1 {
                return Err(format!(
                    "Multiple matches found ({crlf_match_count}). Use replace_all or provide more context."
                ));
            }
        }

        // Exact match failed. Fall back to fuzzy only when the gate is on.
        if self.fuzzy_fallback {
            let res = fuzzy_find_and_replace(content, old_string, new_string, replace_all);
            if res.error.is_none() && res.match_count > 0 {
                return Ok((res.content, res.match_count));
            }
        }

        // Preserve the original error messages on the exact path.
        if match_count == 0 {
            Err("old_string not found in file".to_string())
        } else {
            Err(format!(
                "Multiple matches found ({match_count}). Use replace_all or provide more context."
            ))
        }
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn description(&self) -> &str {
        "Performs exact string replacements in files.\n\n\
         Usage:\n\
         - You must use the Read tool first before editing a file.\n\
         - The old_string must be unique in the file. If multiple matches exist, \
         the edit will fail. Provide more surrounding context to make it unique, \
         or use replace_all to change every occurrence.\n\
         - Use replace_all for renaming variables or replacing all instances of a string.\n\
         - Prefer Edit over Write for modifying existing files — Edit only sends the diff.\n\
         - When matching text from Read output, preserve the exact indentation (tabs/spaces)."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to modify"
                },
                "old_string": {
                    "type": "string",
                    "description": "The text to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default false)"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
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
        let Some(old_string) = input["old_string"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: old_string".to_string(),
                is_error: true,
            };
        };
        let Some(new_string) = input["new_string"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: new_string".to_string(),
                is_error: true,
            };
        };
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        // Wave SD SECURITY MAJOR #14 — validate before any cache lookup
        // OR filesystem touch. Refuses relative paths, traversal, null
        // bytes, and a deny-list of obvious system secrets.
        let validated = match validate_user_path(Path::new(file_path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to edit {file_path}: {e}"),
                    is_error: true,
                };
            }
        };
        let path = validated.as_path();

        // Cache guard: "must Read first" + staleness detection.
        if let Some(cache_arc) = &self.file_cache
            && let Ok(mut cache) = cache_arc.write()
        {
            let cached = cache.get(path);
            if cached.is_none() {
                return ToolResult {
                    content: format!(
                        "You must Read {} before editing. Use the Read tool first \
                         so the file content is loaded into context.",
                        file_path
                    ),
                    is_error: true,
                };
            }
            // Staleness check: compare cached mtime with current disk mtime.
            let cached_mtime = cached.map(|s| s.mtime_ms);
            let disk_mtime = file_mtime_ms(path);
            if let (Some(cached_mt), Some(disk_mt)) = (cached_mtime, disk_mtime)
                && cached_mt != disk_mt
            {
                return ToolResult {
                    content: format!(
                        "File {} has been modified externally since last read. \
                         Read the file again to see the current content before editing.",
                        file_path
                    ),
                    is_error: true,
                };
            }
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to read file {}: {}", file_path, e),
                    is_error: true,
                };
            }
        };

        let (new_content, match_count) =
            match self.compute_edit(&content, old_string, new_string, replace_all) {
                Ok(v) => v,
                Err(msg) => {
                    return ToolResult {
                        content: msg,
                        is_error: true,
                    };
                }
            };

        if let Err(e) = wcore_config::atomic_write(path, new_content.as_bytes()) {
            return ToolResult {
                content: format!("Failed to write file: {}", e),
                is_error: true,
            };
        }

        // Post-write cache update: refresh mtime and content.
        if let Some(cache_arc) = &self.file_cache {
            update_cache_after_write(cache_arc, path, &new_content);
        }

        ToolResult {
            content: format!(
                "Edited {}: replaced {} occurrence(s)",
                file_path, match_count
            ),
            is_error: false,
        }
    }

    /// W8b — vfs-aware variant. Reads and writes through `ctx.vfs`
    /// (sandbox-aware for sub-agents). The "must Read first" cache
    /// guard + staleness check still consult the FileStateCache and
    /// `file_mtime_ms` directly because those are engine-level
    /// invariants, not VFS-level facts.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let Some(file_path) = input["file_path"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };
        let Some(old_string) = input["old_string"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: old_string".to_string(),
                is_error: true,
            };
        };
        let Some(new_string) = input["new_string"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: new_string".to_string(),
                is_error: true,
            };
        };
        let replace_all = input["replace_all"].as_bool().unwrap_or(false);

        // Wave SD — single validation primitive for both entry paths.
        let validated = match validate_user_path(Path::new(file_path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to edit {file_path}: {e}"),
                    is_error: true,
                };
            }
        };
        let path = validated.as_path();

        // Cache guard: "must Read first" + staleness detection.
        if let Some(cache_arc) = &self.file_cache
            && let Ok(mut cache) = cache_arc.write()
        {
            let cached = cache.get(path);
            if cached.is_none() {
                return ToolResult {
                    content: format!(
                        "You must Read {file_path} before editing. Use the Read tool first \
                         so the file content is loaded into context."
                    ),
                    is_error: true,
                };
            }
            let cached_mtime = cached.map(|s| s.mtime_ms);
            let disk_mtime = file_mtime_ms(path);
            if let (Some(cached_mt), Some(disk_mt)) = (cached_mtime, disk_mtime)
                && cached_mt != disk_mt
            {
                return ToolResult {
                    content: format!(
                        "File {file_path} has been modified externally since last read. \
                         Read the file again to see the current content before editing."
                    ),
                    is_error: true,
                };
            }
        }

        let bytes = match ctx.vfs.read(path).await {
            Ok(b) => b,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to read file {file_path}: {e}"),
                    is_error: true,
                };
            }
        };
        let content = String::from_utf8_lossy(&bytes).into_owned();

        let (new_content, match_count) =
            match self.compute_edit(&content, old_string, new_string, replace_all) {
                Ok(v) => v,
                Err(msg) => {
                    return ToolResult {
                        content: msg,
                        is_error: true,
                    };
                }
            };

        // W8b.2.A D.4 — mark this write as engine-originated BEFORE the
        // actual write so an upstream FileWatcher can debounce its own
        // change event. See the matching block in WriteTool::execute_with_ctx.
        if let Some(n) = ctx.file_write_notifier.as_ref() {
            n.note_self_originated_write(path).await;
        }

        if let Err(e) = ctx.vfs.write(path, new_content.as_bytes()).await {
            return ToolResult {
                content: format!("Failed to write file: {e}"),
                is_error: true,
            };
        }

        if let Some(cache_arc) = &self.file_cache {
            update_cache_after_write(cache_arc, path, &new_content);
        }

        ToolResult {
            content: format!("Edited {file_path}: replaced {match_count} occurrence(s)"),
            is_error: false,
        }
    }

    fn max_result_size(&self) -> usize {
        10_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    fn describe(&self, input: &Value) -> String {
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        format!("Edit {}", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    use crate::file_cache::update_cache_after_write;
    use wcore_config::file_cache::FileCacheConfig;

    fn make_cache() -> Arc<RwLock<FileStateCache>> {
        let config = FileCacheConfig {
            max_entries: 100,
            max_size_bytes: 25 * 1024 * 1024,
            enabled: true,
        };
        Arc::new(RwLock::new(FileStateCache::new(&config)))
    }

    /// Simulate a Read by inserting a cache entry for the given file path.
    fn simulate_read(cache: &Arc<RwLock<FileStateCache>>, path: &Path) {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        update_cache_after_write(cache, path, &content);
    }

    // -- Legacy tests (no cache) --

    #[tokio::test]
    async fn test_edit_replace_block() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye"
        });

        let result = tool.execute(input).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[tokio::test]
    async fn test_edit_old_string_not_found() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "nonexistent",
            "new_string": "replacement"
        });

        let result = tool.execute(input).await;

        assert!(result.is_error);
        assert!(
            result.content.contains("not found"),
            "expected 'not found' in error message, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_edit_preserves_surrounding() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "aaa\nbbb\nccc\n").unwrap();

        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "bbb",
            "new_string": "XXX"
        });

        let result = tool.execute(input).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "aaa\nXXX\nccc\n");
    }

    #[tokio::test]
    async fn test_edit_nonexistent_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("does_not_exist.txt");

        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "anything",
            "new_string": "replacement"
        });

        let result = tool.execute(input).await;

        assert!(result.is_error);
        assert!(
            result.content.contains("Failed to read file"),
            "expected read failure message, got: {}",
            result.content
        );
    }

    // -- Cache guard tests --

    #[tokio::test]
    async fn edit_without_read_returns_error() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("unread.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let cache = make_cache();
        let tool = EditTool::new(Some(cache));

        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "bye"
        });

        let result = tool.execute(input).await;

        assert!(result.is_error);
        assert!(
            result.content.contains("must Read"),
            "expected 'must Read' in error: {}",
            result.content
        );
        // File must be unchanged.
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "hello");
    }

    #[tokio::test]
    async fn edit_after_read_succeeds() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("read_then_edit.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let cache = make_cache();
        simulate_read(&cache, &file_path);

        let tool = EditTool::new(Some(cache));
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye"
        });

        let result = tool.execute(input).await;

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "goodbye world"
        );
    }

    #[tokio::test]
    async fn edit_detects_external_modification() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("stale.txt");
        std::fs::write(&file_path, "original").unwrap();

        let cache = make_cache();
        simulate_read(&cache, &file_path);

        // External modification: change file after caching.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&file_path, "externally changed").unwrap();

        let tool = EditTool::new(Some(cache));
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "original",
            "new_string": "new"
        });

        let result = tool.execute(input).await;

        assert!(result.is_error);
        assert!(
            result.content.contains("modified externally"),
            "expected staleness error: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn edit_then_edit_succeeds_via_cache_update() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("double_edit.txt");
        std::fs::write(&file_path, "aaa bbb ccc").unwrap();

        let cache = make_cache();
        simulate_read(&cache, &file_path);

        let tool = EditTool::new(Some(cache));

        // First edit.
        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "aaa",
            "new_string": "AAA"
        });
        let r1 = tool.execute(input1).await;
        assert!(!r1.is_error, "first edit failed: {}", r1.content);

        // Second edit should succeed because first edit updated the cache.
        let input2 = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "bbb",
            "new_string": "BBB"
        });
        let r2 = tool.execute(input2).await;
        assert!(!r2.is_error, "second edit failed: {}", r2.content);
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "AAA BBB ccc");
    }

    #[tokio::test]
    async fn no_cache_edit_bypasses_guard() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("nocache.txt");
        std::fs::write(&file_path, "hello").unwrap();

        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "bye"
        });

        let result = tool.execute(input).await;
        assert!(
            !result.is_error,
            "expected success without cache: {}",
            result.content
        );
        assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "bye");
    }

    // -- Rank 41: fuzzy fallback gate --

    #[tokio::test]
    async fn fuzzy_fallback_on_succeeds_on_whitespace_mismatch() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("fuzzy_on.txt");
        // On-disk line has trailing whitespace the LLM-supplied old_string lacks.
        std::fs::write(&file_path, "def foo():   \n    pass\n").unwrap();

        let tool = EditTool::new(None).with_fuzzy_fallback(true);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            // No EXACT match: the on-disk line has trailing spaces before the
            // newline that this multi-line old_string lacks, so the span isn't
            // found by substring search — only the whitespace-tolerant fuzzy
            // path matches.
            "old_string": "def foo():\n    pass",
            "new_string": "def bar():\n    pass"
        });

        let result = tool.execute(input).await;

        assert!(
            !result.is_error,
            "fuzzy fallback should have matched: {}",
            result.content
        );
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(
            content.starts_with("def bar():"),
            "expected fuzzy replacement, got: {content:?}"
        );
    }

    #[tokio::test]
    async fn fuzzy_fallback_off_errors_on_whitespace_mismatch() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("fuzzy_off.txt");
        std::fs::write(&file_path, "def foo():   \n    pass\n").unwrap();

        // Default tool: fuzzy fallback OFF.
        let tool = EditTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            // Same real (whitespace) mismatch as the on-test: exact search can't
            // find it, and with fuzzy off there is no fallback, so it must error.
            "old_string": "def foo():\n    pass",
            "new_string": "def bar():\n    pass"
        });

        let result = tool.execute(input).await;

        assert!(result.is_error, "exact match should fail with fuzzy off");
        assert!(
            result.content.contains("not found"),
            "expected exact-path error message, got: {}",
            result.content
        );
        // File must be unchanged.
        assert_eq!(
            std::fs::read_to_string(&file_path).unwrap(),
            "def foo():   \n    pass\n"
        );
    }

    #[tokio::test]
    async fn replace_all_updates_cache() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("replaceall.txt");
        std::fs::write(&file_path, "a-a-a").unwrap();

        let cache = make_cache();
        simulate_read(&cache, &file_path);

        let tool = EditTool::new(Some(cache.clone()));
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "old_string": "a",
            "new_string": "b",
            "replace_all": true
        });

        let result = tool.execute(input).await;
        assert!(!result.is_error, "replace_all failed: {}", result.content);

        // Verify cache was updated: mtime should match current disk mtime.
        let disk_mtime = file_mtime_ms(&file_path).unwrap();
        let mut c = cache.write().unwrap();
        let cached = c.get(&file_path).expect("file should be in cache");
        assert_eq!(cached.mtime_ms, disk_mtime);
    }

    // ── CRLF reconciliation (issue #257) ─────────────────────────────────
    // The Read tool normalizes CRLF to LF via `str::lines()`, so the model's
    // `old_string` is LF-only against a CRLF file on disk. compute_edit must
    // still match, and must preserve the file's CRLF endings.

    #[test]
    fn compute_edit_matches_lf_pattern_against_crlf_file() {
        let tool = EditTool::new(None);
        let content = "line one\r\nline two\r\nline three\r\n";
        // old_string as the Read tool would have shown it: LF-only.
        let (out, count) = tool
            .compute_edit(content, "line one\nline two", "line ONE\nline TWO", false)
            .expect("LF pattern must match the CRLF file");
        assert_eq!(count, 1);
        assert_eq!(out, "line ONE\r\nline TWO\r\nline three\r\n");
    }

    #[test]
    fn compute_edit_lf_file_unaffected_by_crlf_retry() {
        let tool = EditTool::new(None);
        let (out, count) = tool
            .compute_edit("a\nb\nc\n", "a\nb", "X\nY", false)
            .expect("LF happy path");
        assert_eq!(count, 1);
        assert_eq!(out, "X\nY\nc\n");
    }

    #[test]
    fn compute_edit_crlf_missing_pattern_still_errors() {
        let tool = EditTool::new(None);
        let err = tool
            .compute_edit("a\r\nb\r\n", "zzz\nyyy", "q", false)
            .expect_err("genuinely-absent pattern must still error");
        assert!(err.contains("not found"), "got: {err}");
    }

    #[test]
    fn compute_edit_crlf_replace_all_multi() {
        let tool = EditTool::new(None);
        let content = "x\r\ny\r\nx\r\ny\r\n";
        let (out, count) = tool
            .compute_edit(content, "x\ny", "p\nq", true)
            .expect("replace_all through the CRLF path");
        assert_eq!(count, 2);
        assert_eq!(out, "p\r\nq\r\np\r\nq\r\n");
        // Same input without replace_all: the CRLF path reports multiple
        // matches (not the misleading "old_string not found").
        let err = tool
            .compute_edit(content, "x\ny", "p\nq", false)
            .expect_err("multiple CRLF matches without replace_all must error");
        assert!(err.contains("Multiple matches"), "got: {err}");
    }
}
