use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::file_state::{FileState, Provenance};
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::context::ToolContext;
use crate::file_cache::{FileStateCache, file_mtime_ms};
use crate::path_validation::validate_user_path;

/// Stub returned when a file has not changed since the model last read it.
/// Saves tokens by avoiding re-sending identical content.
const FILE_UNCHANGED_STUB: &str = "File unchanged since last read. The content from the earlier Read \
     tool_result in this conversation is still current — refer to that \
     instead of re-reading.";

/// Token-opt (diff-resend): header prefixed to a diff result so the model knows
/// it is reading changed lines anchored to the current file, not the full file.
const DIFF_RESEND_HEADER: &str = "File changed since your last read. Showing only the changed lines \
     (anchored to current line numbers); unchanged regions you already have are elided as `…`. \
     Apply these against the content from your previous Read of this file:";

/// Token-opt (diff-resend): a diff is only emitted when it is at most this
/// fraction of the full numbered content it would replace.
const DIFF_RESEND_MAX_RATIO: f64 = 0.6;

/// Token-opt (semantic slicing): build the Read result for a `symbol=` request.
/// Returns the symbol's line window (numbered, with a header + expansion hint),
/// or a recoverable message when the symbol isn't found / the language has no
/// extractor. Never errors — the model can always re-read without `symbol=`.
fn build_symbol_result(text: &str, path: &Path, symbol: &str) -> ToolResult {
    use crate::symbol_slice::{SymbolSlice, resolve_symbol};

    match resolve_symbol(path, text, symbol) {
        SymbolSlice::Found {
            start,
            end,
            kind,
            multiple,
        } => {
            let lines: Vec<&str> = text.lines().collect();
            let total = lines.len();
            // `resolve_symbol` only returns Found for non-empty files with the
            // window inside bounds; clamp defensively anyway.
            let s = start.clamp(1, total.max(1));
            let e = end.clamp(s, total.max(1));
            let slice = &lines[s - 1..e.min(total)];
            let numbered: Vec<String> = slice
                .iter()
                .enumerate()
                .map(|(i, line)| format!("{:>6}\t{}", s + i, line))
                .collect();
            let mut header = format!(
                "Symbol `{symbol}` ({kind:?}, lines {s}\u{2013}{e} of {total}). Re-read without \
                 symbol= for the full file, or with offset/limit for a different window."
            );
            if multiple {
                header.push_str(&format!(
                    "\n(Multiple symbols named `{symbol}` exist; showing the first.)"
                ));
            }
            ToolResult {
                content: format!("{header}\n{}", numbered.join("\n")),
                is_error: false,
            }
        }
        SymbolSlice::NotFound { available } => {
            let list = if available.is_empty() {
                "(none detected)".to_string()
            } else {
                available.join(", ")
            };
            ToolResult {
                content: format!(
                    "No symbol named `{symbol}` found in {}. Available symbols: {list}. Omit \
                     symbol= for the full file, or use offset/limit.",
                    path.display()
                ),
                is_error: false,
            }
        }
        SymbolSlice::Unsupported => ToolResult {
            content: format!(
                "Symbol slicing is only available for Rust / TypeScript / JavaScript files. \
                 Re-read {} without symbol= (or with offset/limit) to view it.",
                path.display()
            ),
            is_error: false,
        },
    }
}

pub struct ReadTool {
    file_cache: Option<Arc<RwLock<FileStateCache>>>,
}

impl ReadTool {
    /// Create a ReadTool with optional file state cache for dedup.
    ///
    /// Pass `None` to disable caching (all reads return full content).
    pub fn new(file_cache: Option<Arc<RwLock<FileStateCache>>>) -> Self {
        Self { file_cache }
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> &str {
        "Reads a file from the local filesystem. Returns content with line numbers.\n\n\
         Usage:\n\
         - The file_path parameter must be an absolute path, not a relative path.\n\
         - By default, it reads the entire file. Use offset and limit for partial reads on large files.\n\
         - To read just one definition from a large Rust/TypeScript/JavaScript file, pass symbol=\"name\" \
         (a function, struct, enum, trait, impl, class, or interface). Returns only that symbol's lines \
         plus a hint for expanding back to the full file. Saves tokens when you only need one definition.\n\
         - Results are returned with line numbers (1-based) followed by a tab and the line content.\n\
         - Binary files return \"(binary file, N bytes)\" instead of content.\n\
         - This tool can only read files, not directories. To list a directory, use Bash with ls."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-based)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read"
                },
                "symbol": {
                    "type": "string",
                    "description": "Return only this named symbol (function/struct/enum/trait/impl/class/interface) from a Rust/TS/JS file, instead of the whole file. Ignored if the file type is unsupported or the symbol is not found (a recoverable message lists available names)."
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

        // Wave SD SECURITY MAJOR #14: validate the LLM-supplied path
        // before any filesystem touch. Refuses relative paths, traversal,
        // null bytes, and a deny-list of obvious system secrets.
        let validated = match validate_user_path(Path::new(file_path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to read {file_path}: {e}"),
                    is_error: true,
                };
            }
        };

        let offset = input["offset"].as_u64().map(|v| v as usize);
        let limit = input["limit"].as_u64().map(|v| v as usize);
        // Token-opt (semantic slicing): an explicit symbol request bypasses the
        // dedup/diff cache — it's a targeted view, computed fresh from the file.
        let symbol = input["symbol"].as_str().filter(|s| !s.is_empty());

        // Get file mtime for dedup and cache.
        let mtime_ms = file_mtime_ms(&validated);

        // Dedup check: if cache has the same file with matching offset/limit and mtime,
        // return a short stub instead of full content.
        //
        // The `ReadResult` provenance guard is load-bearing: after an Edit/Write,
        // `update_cache_after_write` refreshes this entry to the post-write content
        // AND mtime (provenance `WriteEcho`). Without the guard a verify-read would
        // see mtime-equality and emit "file unchanged, refer to the earlier Read" —
        // but the earlier Read in the transcript is the *pre-edit* content. Only a
        // `ReadResult` entry is something the model has actually seen as a read.
        if symbol.is_none()
            && let (Some(cache_arc), Some(current_mtime)) = (&self.file_cache, mtime_ms)
            && let Ok(mut cache) = cache_arc.write()
            && let Some(cached) = cache.get(&validated)
            && cached.offset == offset
            && cached.limit == limit
            && cached.mtime_ms == current_mtime
            && cached.provenance == Provenance::ReadResult
        {
            return ToolResult {
                content: FILE_UNCHANGED_STUB.to_string(),
                is_error: false,
            };
        }

        // Read file from disk.
        let content = match std::fs::read(&validated) {
            Ok(bytes) => bytes,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to read file {}: {}", file_path, e),
                    is_error: true,
                };
            }
        };

        // Check if binary.
        if content.iter().take(8192).any(|&b| b == 0) {
            return ToolResult {
                content: format!("(binary file, {} bytes)", content.len()),
                is_error: false,
            };
        }

        let text = String::from_utf8_lossy(&content);

        // Token-opt (semantic slicing): targeted symbol view, not cached.
        if let Some(sym) = symbol {
            return build_symbol_result(text.as_ref(), &validated, sym);
        }

        let lines: Vec<&str> = text.lines().collect();

        let effective_offset = offset.unwrap_or(0);
        let effective_limit = limit.unwrap_or(lines.len());

        let end = (effective_offset + effective_limit).min(lines.len());
        let slice = &lines[effective_offset.min(lines.len())..end];

        let numbered: Vec<String> = slice
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", effective_offset + i + 1, line))
            .collect();

        let result_content = numbered.join("\n");

        // Update cache after successful read.
        if let Some(cache_arc) = &self.file_cache
            && let (Ok(mut cache), Some(mtime)) = (cache_arc.write(), mtime_ms)
        {
            let gen_at_read = cache.compaction_generation();
            cache.insert(
                validated.clone(),
                FileState {
                    content: result_content.clone(),
                    mtime_ms: mtime,
                    offset,
                    limit,
                    provenance: Provenance::ReadResult,
                    gen_at_read,
                },
            );
        }

        ToolResult {
            content: result_content,
            is_error: false,
        }
    }

    /// W8b — vfs-aware variant. Routes the read through `ctx.vfs`
    /// (RealFs / SandboxedFs / InMemoryFs). Wave SD adds the same
    /// `validate_user_path` shape check as the legacy entry, so a
    /// top-level (non-sandboxed) ctx can't be used as a bypass for
    /// the path discipline. The dedup cache + mtime staleness check
    /// still consult the real disk via `file_mtime_ms` because the
    /// VFS trait doesn't expose mtime today; this is acceptable for
    /// the migration (the staleness check is a hint, not a security
    /// boundary). Sandboxed sub-agents reading through this path are
    /// additionally clamped to their root by SandboxedFs.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let Some(file_path) = input["file_path"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: file_path".to_string(),
                is_error: true,
            };
        };

        // Wave SD — single validation primitive for both entry paths.
        let validated = match validate_user_path(Path::new(file_path)) {
            Ok(p) => p,
            Err(e) => {
                return ToolResult {
                    content: format!("Refused to read {file_path}: {e}"),
                    is_error: true,
                };
            }
        };

        let offset = input["offset"].as_u64().map(|v| v as usize);
        let limit = input["limit"].as_u64().map(|v| v as usize);
        // Token-opt (semantic slicing): a symbol request is a targeted view,
        // computed fresh — it skips the dedup stub and diff-resend entirely.
        let symbol = input["symbol"].as_str().filter(|s| !s.is_empty());

        let path = validated.as_path();
        let mtime_ms = file_mtime_ms(path);

        // Single locked pass over the cache: serve the unchanged stub, and (if
        // not stubbing) capture a base for a possible diff. See `execute()` for
        // the `ReadResult` guard rationale.
        //
        // `diff_base` is only populated when a diff would be SOUND to emit:
        //   * the route opted into client-side optimization (`optimize_reads`),
        //   * this is a full read (offset/limit None) matching the cached window,
        //   * the caller is the main agent (`source_agent` is None) — the cache is
        //     process-wide across sub-agents, so a sibling's read must never seed
        //     a base this transcript never contained,
        //   * the base is a `ReadResult` (something the model actually saw), and
        //   * the base is still visible: the compaction generation has not moved
        //     since it was cached, so the diff's reference content has not been
        //     collapsed/cleared out of the transcript.
        let is_full_read = offset.is_none() && limit.is_none();
        let single_agent = ctx.source_agent.is_none();
        let mut diff_base: Option<String> = None;
        let mut current_gen: u64 = 0;

        if symbol.is_none()
            && let (Some(cache_arc), Some(current_mtime)) = (&self.file_cache, mtime_ms)
            && let Ok(mut cache) = cache_arc.write()
        {
            let optimize_reads = cache.optimize_reads();
            current_gen = cache.compaction_generation();
            if let Some(cached) = cache.get(path) {
                let matches_window = cached.offset == offset && cached.limit == limit;
                // The stub tells the model "you already saw this content earlier";
                // that claim is only sound when the earlier Read is STILL in this
                // transcript. Gate it the same way as the diff path below:
                //   * `single_agent` — the cache is process-wide, so a sibling's
                //     read must not make the main agent claim it saw content its
                //     own transcript never contained, and
                //   * `gen_at_read == current_gen` — if compaction has advanced
                //     since the cache entry was seeded, the referenced Read has
                //     been collapsed/cleared, so the stub would point the model at
                //     gone content (a hallucination seed). On a stale generation
                //     fall through to a fresh full read.
                if matches_window
                    && cached.mtime_ms == current_mtime
                    && cached.provenance == Provenance::ReadResult
                    && single_agent
                    && cached.gen_at_read == current_gen
                {
                    return ToolResult {
                        content: FILE_UNCHANGED_STUB.to_string(),
                        is_error: false,
                    };
                }
                if optimize_reads
                    && is_full_read
                    && single_agent
                    && matches_window
                    && cached.provenance == Provenance::ReadResult
                    && cached.gen_at_read == current_gen
                {
                    diff_base = Some(cached.content.clone());
                }
            }
        }

        let content = match ctx.vfs.read(path).await {
            Ok(bytes) => bytes,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to read file {file_path}: {e}"),
                    is_error: true,
                };
            }
        };

        if content.iter().take(8192).any(|&b| b == 0) {
            return ToolResult {
                content: format!("(binary file, {} bytes)", content.len()),
                is_error: false,
            };
        }

        let text = String::from_utf8_lossy(&content);

        // Token-opt (semantic slicing): targeted symbol view, not cached.
        if let Some(sym) = symbol {
            return build_symbol_result(text.as_ref(), &validated, sym);
        }

        let lines: Vec<&str> = text.lines().collect();

        let effective_offset = offset.unwrap_or(0);
        let effective_limit = limit.unwrap_or(lines.len());

        let end = (effective_offset + effective_limit).min(lines.len());
        let slice = &lines[effective_offset.min(lines.len())..end];

        let numbered: Vec<String> = slice
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", effective_offset + i + 1, line))
            .collect();

        let result_content = numbered.join("\n");

        // Token-opt (diff-resend): if we captured a sound base and the content
        // actually changed, try to answer with a line diff. The diff is byte-exact
        // verified to reconstruct the current content before it is emitted
        // (`build_read_diff`); any failure falls back to the full content.
        let mut response_content = result_content.clone();
        if let Some(base_numbered) = &diff_base {
            let base_raw = crate::read_diff::strip_line_numbers(base_numbered);
            let cur_raw: Vec<String> = slice.iter().map(|s| s.to_string()).collect();
            if base_raw != cur_raw
                && let Some(diff_body) = crate::read_diff::build_read_diff(
                    &base_raw,
                    &cur_raw,
                    result_content.len(),
                    DIFF_RESEND_MAX_RATIO,
                )
            {
                response_content = format!("{DIFF_RESEND_HEADER}\n{diff_body}");
            }
        }

        // Cache the FULL current content as the new ReadResult base, stamped with
        // the current generation. Even when we emitted a diff, the model now
        // effectively holds the full current content (visible base + diff), so a
        // future re-read diffs against it correctly.
        if let Some(cache_arc) = &self.file_cache
            && let (Ok(mut cache), Some(mtime)) = (cache_arc.write(), mtime_ms)
        {
            cache.insert(
                validated.clone(),
                FileState {
                    content: result_content,
                    mtime_ms: mtime,
                    offset,
                    limit,
                    provenance: Provenance::ReadResult,
                    gen_at_read: current_gen,
                },
            );
        }

        ToolResult {
            content: response_content,
            is_error: false,
        }
    }

    fn max_result_size(&self) -> usize {
        100_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let path = input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        format!("Read {}", path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;

    use wcore_config::file_cache::FileCacheConfig;

    fn make_cache() -> Arc<RwLock<FileStateCache>> {
        let config = FileCacheConfig {
            max_entries: 100,
            max_size_bytes: 25 * 1024 * 1024,
            enabled: true,
        };
        Arc::new(RwLock::new(FileStateCache::new(&config)))
    }

    // -- Basic read tests (no cache) --

    #[tokio::test]
    async fn test_read_file_full() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "line one").unwrap();
        writeln!(file, "line two").unwrap();
        writeln!(file, "line three").unwrap();
        drop(file);

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = tool.execute(input).await;

        assert!(!result.is_error);
        assert!(result.content.contains("1\tline one"));
        assert!(result.content.contains("2\tline two"));
        assert!(result.content.contains("3\tline three"));
    }

    #[tokio::test]
    async fn test_read_file_with_offset_and_limit() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("lines.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        for i in 1..=10 {
            writeln!(file, "line {}", i).unwrap();
        }
        drop(file);

        let tool = ReadTool::new(None);
        let input = json!({
            "file_path": file_path.to_str().unwrap(),
            "offset": 2,
            "limit": 3
        });
        let result = tool.execute(input).await;

        assert!(!result.is_error);
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("3\tline 3"));
        assert!(lines[1].contains("4\tline 4"));
        assert!(lines[2].contains("5\tline 5"));
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        // Use a real tempdir for a platform-agnostic absolute path
        // (Windows wants C:\..., Linux/mac /tmp/...). The file inside
        // is never created — we want the read to fail with "Failed to
        // read file", not the path-validation "NotAbsolute" branch
        // (CI run 25955535226 caught this — original /tmp/... wasn't
        // absolute on Windows).
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent_file_abc123.txt");
        let tool = ReadTool::new(None);
        let input = json!({ "file_path": path.to_str().unwrap() });
        let result = tool.execute(input).await;

        assert!(result.is_error);
        assert!(result.content.contains("Failed to read file"));
    }

    #[tokio::test]
    async fn test_read_empty_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        std::fs::File::create(&file_path).unwrap();

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = tool.execute(input).await;

        assert!(!result.is_error);
        assert!(result.content.is_empty());
    }

    #[tokio::test]
    async fn test_read_large_file_truncation() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("large.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        for i in 1..=200 {
            writeln!(file, "line number {}", i).unwrap();
        }
        drop(file);

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = tool.execute(input).await;

        assert!(!result.is_error);
        let lines: Vec<&str> = result.content.lines().collect();
        assert_eq!(lines.len(), 200);
        assert!(lines[0].contains("1\tline number 1"));
        assert!(lines[199].contains("200\tline number 200"));
    }

    // -- Dedup tests (with cache) --

    #[tokio::test]
    async fn dedup_returns_stub_on_unchanged_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("dedup.txt");
        std::fs::write(&file_path, "hello\n").unwrap();

        let cache = make_cache();
        let tool = ReadTool::new(Some(cache));

        let input = json!({ "file_path": file_path.to_str().unwrap() });

        // First read: full content.
        let r1 = tool.execute(input.clone()).await;
        assert!(!r1.is_error);
        assert!(r1.content.contains("hello"));

        // Second read: dedup stub.
        let r2 = tool.execute(input).await;
        assert!(!r2.is_error);
        assert_eq!(r2.content, FILE_UNCHANGED_STUB);
    }

    #[tokio::test]
    async fn dedup_returns_new_content_after_modification() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("modified.txt");
        std::fs::write(&file_path, "version1\n").unwrap();

        let cache = make_cache();
        let tool = ReadTool::new(Some(cache));

        let input = json!({ "file_path": file_path.to_str().unwrap() });

        let r1 = tool.execute(input.clone()).await;
        assert!(r1.content.contains("version1"));

        // Modify the file — ensure mtime changes.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&file_path, "version2\n").unwrap();

        let r2 = tool.execute(input).await;
        assert!(!r2.is_error);
        assert!(r2.content.contains("version2"));
    }

    #[tokio::test]
    async fn dedup_different_offset_limit_returns_full() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("multi.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        for i in 1..=20 {
            writeln!(file, "line {}", i).unwrap();
        }
        drop(file);

        let cache = make_cache();
        let tool = ReadTool::new(Some(cache));

        let input1 = json!({
            "file_path": file_path.to_str().unwrap(),
            "offset": 0,
            "limit": 10
        });
        let r1 = tool.execute(input1).await;
        assert!(!r1.is_error);
        assert!(r1.content.contains("line 1"));

        // Different range: should return full content, not stub.
        let input2 = json!({
            "file_path": file_path.to_str().unwrap(),
            "offset": 10,
            "limit": 10
        });
        let r2 = tool.execute(input2).await;
        assert!(!r2.is_error);
        assert!(r2.content.contains("line 11"));
        assert!(!r2.content.contains(FILE_UNCHANGED_STUB));
    }

    #[tokio::test]
    async fn no_cache_always_returns_full_content() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("nocache.txt");
        std::fs::write(&file_path, "data\n").unwrap();

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file_path.to_str().unwrap() });

        let r1 = tool.execute(input.clone()).await;
        assert!(r1.content.contains("data"));

        let r2 = tool.execute(input).await;
        assert!(r2.content.contains("data"));
        assert_ne!(r2.content, FILE_UNCHANGED_STUB);
    }

    #[tokio::test]
    async fn nonexistent_file_not_cached() {
        let cache = make_cache();
        let tool = ReadTool::new(Some(cache.clone()));

        let input = json!({ "file_path": "/tmp/nonexistent_xyz_789.txt" });
        let r = tool.execute(input).await;
        assert!(r.is_error);

        // Cache should be empty.
        let c = cache.read().unwrap();
        assert!(c.is_empty());
    }

    #[tokio::test]
    async fn dedup_empty_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        std::fs::File::create(&file_path).unwrap();

        let cache = make_cache();
        let tool = ReadTool::new(Some(cache));

        let input = json!({ "file_path": file_path.to_str().unwrap() });

        let r1 = tool.execute(input.clone()).await;
        assert!(!r1.is_error);

        let r2 = tool.execute(input).await;
        assert!(!r2.is_error);
        assert_eq!(r2.content, FILE_UNCHANGED_STUB);
    }

    #[tokio::test]
    async fn read_after_write_returns_full_content_not_stub() {
        // Regression: the Read dedup keyed on mtime-equality alone would false-stub
        // a post-write verify-read. `update_cache_after_write` refreshes the entry
        // to the new content AND mtime, so a verify-read sees mtime-equality and
        // (pre-fix) returned "file unchanged, refer to the earlier Read" — but the
        // earlier Read in the transcript is the PRE-edit content. The `WriteEcho`
        // provenance guard must force full current content instead.
        use crate::file_cache::update_cache_after_write;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("verify.txt");
        std::fs::write(&file_path, "version1\n").unwrap();

        let cache = make_cache();
        let tool = ReadTool::new(Some(cache.clone()));
        let input = json!({ "file_path": file_path.to_str().unwrap() });

        // Model reads version1.
        let r1 = tool.execute(input.clone()).await;
        assert!(r1.content.contains("version1"));

        // A tool writes version2 (Edit/Write path): cache entry becomes WriteEcho
        // with the new on-disk mtime.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(&file_path, "version2\n").unwrap();
        update_cache_after_write(&cache, &file_path, "version2\n");

        // Verify-read: mtime matches the WriteEcho entry, but the model never saw
        // version2 as a read — must return full content, NOT the misleading stub.
        let r2 = tool.execute(input.clone()).await;
        assert!(!r2.is_error);
        assert_ne!(
            r2.content, FILE_UNCHANGED_STUB,
            "post-write verify-read must not emit the unchanged stub"
        );
        assert!(r2.content.contains("version2"));

        // The verify-read re-cached version2 as a genuine ReadResult, so an
        // immediate unchanged re-read now correctly stubs.
        let r3 = tool.execute(input).await;
        assert_eq!(r3.content, FILE_UNCHANGED_STUB);
    }

    // -- diff-resend tests (execute_with_ctx, optimize_reads enabled) --

    fn opt_cache() -> Arc<RwLock<FileStateCache>> {
        let c = make_cache();
        c.write().unwrap().set_optimize_reads(true);
        c
    }

    fn ctx_main() -> ToolContext {
        ToolContext::test_default()
    }

    fn ctx_sub() -> ToolContext {
        // A sub-agent context: source_agent is Some, so diff-resend must not fire
        // (the process-wide cache must not seed a base this transcript lacks).
        use crate::vfs::RealFs;
        ToolContext::new(
            String::new(),
            tokio_util::sync::CancellationToken::new(),
            Arc::new(RealFs),
            Some("sub-agent".to_string()),
            Arc::new(crate::NullToolOutputSink),
        )
    }

    /// Write `n` numbered lines, return the file path.
    fn write_lines(
        dir: &std::path::Path,
        name: &str,
        n: usize,
        marker: &str,
    ) -> std::path::PathBuf {
        let p = dir.join(name);
        let body: String = (0..n)
            .map(|i| {
                if i == n / 2 {
                    format!("line {i} {marker}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect();
        std::fs::write(&p, body).unwrap();
        p
    }

    #[tokio::test]
    async fn external_change_full_read_returns_diff() {
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 60, "ORIGINAL");

        let cache = opt_cache();
        let tool = ReadTool::new(Some(cache));
        let ctx = ctx_main();
        let input = json!({ "file_path": file.to_str().unwrap() });

        // First read: full content (model sees ORIGINAL).
        let r1 = tool.execute_with_ctx(input.clone(), &ctx).await;
        assert!(r1.content.contains("ORIGINAL"));
        assert!(!r1.content.contains(DIFF_RESEND_HEADER));

        // External change (NOT via Edit/Write tool): one line differs, mtime bumps.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = write_lines(dir.path(), "big.txt", 60, "PATCHED");

        // Re-read: must return a diff, not full content.
        let r2 = tool.execute_with_ctx(input, &ctx).await;
        assert!(!r2.is_error);
        assert!(
            r2.content.contains(DIFF_RESEND_HEADER),
            "re-read of an externally-changed file should diff, got: {}",
            r2.content
        );
        assert!(r2.content.contains("PATCHED"));
        assert!(
            r2.content.len() < r1.content.len(),
            "diff must be smaller than the full content"
        );
    }

    #[tokio::test]
    async fn route_disabled_returns_full_content_not_diff() {
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 60, "ORIGINAL");

        // Plain cache: optimize_reads stays false (router-optimized route).
        let cache = make_cache();
        let tool = ReadTool::new(Some(cache));
        let ctx = ctx_main();
        let input = json!({ "file_path": file.to_str().unwrap() });

        tool.execute_with_ctx(input.clone(), &ctx).await;
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = write_lines(dir.path(), "big.txt", 60, "PATCHED");

        let r2 = tool.execute_with_ctx(input, &ctx).await;
        assert!(!r2.content.contains(DIFF_RESEND_HEADER));
        assert!(r2.content.contains("PATCHED"));
        // Full content has every line numbered.
        assert!(r2.content.contains("line 59"));
    }

    #[tokio::test]
    async fn subagent_read_never_diffs() {
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 60, "ORIGINAL");

        let cache = opt_cache();
        let tool = ReadTool::new(Some(cache));
        let ctx = ctx_sub();
        let input = json!({ "file_path": file.to_str().unwrap() });

        tool.execute_with_ctx(input.clone(), &ctx).await;
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = write_lines(dir.path(), "big.txt", 60, "PATCHED");

        let r2 = tool.execute_with_ctx(input, &ctx).await;
        assert!(
            !r2.content.contains(DIFF_RESEND_HEADER),
            "sub-agent reads must return full content, never a diff"
        );
        assert!(r2.content.contains("PATCHED"));
    }

    #[tokio::test]
    async fn compaction_generation_bump_invalidates_diff_base() {
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 60, "ORIGINAL");

        let cache = opt_cache();
        let tool = ReadTool::new(Some(cache.clone()));
        let ctx = ctx_main();
        let input = json!({ "file_path": file.to_str().unwrap() });

        tool.execute_with_ctx(input.clone(), &ctx).await;

        // A compaction pass runs: the base read may no longer be visible.
        cache.write().unwrap().bump_compaction_generation();

        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = write_lines(dir.path(), "big.txt", 60, "PATCHED");

        let r2 = tool.execute_with_ctx(input, &ctx).await;
        assert!(
            !r2.content.contains(DIFF_RESEND_HEADER),
            "a generation bump must force full content (stale base), got a diff"
        );
        assert!(r2.content.contains("PATCHED"));
    }

    #[tokio::test]
    async fn partial_read_never_diffs() {
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 60, "ORIGINAL");

        let cache = opt_cache();
        let tool = ReadTool::new(Some(cache));
        let ctx = ctx_main();
        let input = json!({
            "file_path": file.to_str().unwrap(),
            "offset": 0,
            "limit": 40
        });

        tool.execute_with_ctx(input.clone(), &ctx).await;
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = write_lines(dir.path(), "big.txt", 60, "PATCHED");

        let r2 = tool.execute_with_ctx(input, &ctx).await;
        assert!(
            !r2.content.contains(DIFF_RESEND_HEADER),
            "partial reads must never be answered with a diff"
        );
    }

    #[tokio::test]
    async fn unchanged_reread_still_stubs_with_optimize_on() {
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 20, "X");

        let cache = opt_cache();
        let tool = ReadTool::new(Some(cache));
        let ctx = ctx_main();
        let input = json!({ "file_path": file.to_str().unwrap() });

        tool.execute_with_ctx(input.clone(), &ctx).await;
        // No change at all: the stub still fires (mtime + ReadResult match).
        let r2 = tool.execute_with_ctx(input, &ctx).await;
        assert_eq!(r2.content, FILE_UNCHANGED_STUB);
    }

    #[tokio::test]
    async fn compaction_generation_bump_invalidates_stub() {
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 20, "X");

        let cache = opt_cache();
        let tool = ReadTool::new(Some(cache.clone()));
        let ctx = ctx_main();
        let input = json!({ "file_path": file.to_str().unwrap() });

        tool.execute_with_ctx(input.clone(), &ctx).await;
        // Compaction collapses the earlier Read out of the transcript.
        cache.write().unwrap().bump_compaction_generation();

        // Even with the file unchanged, the stub must NOT fire: it would point
        // the model at content that was just cleared (a hallucination seed).
        // Full content is returned instead.
        let r2 = tool.execute_with_ctx(input, &ctx).await;
        assert_ne!(
            r2.content, FILE_UNCHANGED_STUB,
            "a stale compaction generation must not answer with the unchanged stub"
        );
        assert!(r2.content.contains('X'));
    }

    #[tokio::test]
    async fn subagent_reread_never_stubs() {
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 20, "X");

        let cache = opt_cache();
        let tool = ReadTool::new(Some(cache));
        let input = json!({ "file_path": file.to_str().unwrap() });

        // A sub-agent seeds the cache, then re-reads. The stub must not fire:
        // the process-wide cache must not make a sibling claim it saw content
        // that was never in its own transcript.
        let sub = ctx_sub();
        tool.execute_with_ctx(input.clone(), &sub).await;
        let r2 = tool.execute_with_ctx(input, &sub).await;
        assert_ne!(r2.content, FILE_UNCHANGED_STUB);
        assert!(r2.content.contains('X'));
    }

    // -- semantic slicing (symbol=) tests --

    #[tokio::test]
    async fn symbol_read_returns_only_the_symbol() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("code.rs");
        std::fs::write(
            &file,
            "fn alpha() {\n    let a = 111;\n    a\n}\n\n\
             fn target() {\n    let unique_marker = 222;\n    unique_marker\n}\n\n\
             fn omega() {\n    let z = 333;\n    z\n}\n",
        )
        .unwrap();

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file.to_str().unwrap(), "symbol": "target" });
        let r = tool.execute(input).await;

        assert!(!r.is_error);
        assert!(
            r.content.contains("Symbol `target`"),
            "header present: {}",
            r.content
        );
        assert!(r.content.contains("unique_marker"), "target body present");
        // The other functions' bodies must NOT be included.
        assert!(!r.content.contains("let a = 111"), "alpha body excluded");
        assert!(!r.content.contains("let z = 333"), "omega body excluded");
        // Line numbers are anchored to the real file (target starts at line 6).
        assert!(r.content.contains("     6\tfn target() {"));
    }

    #[tokio::test]
    async fn symbol_not_found_lists_available_names() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("code.rs");
        std::fs::write(&file, "fn alpha() {}\nstruct Beta {}\n").unwrap();

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file.to_str().unwrap(), "symbol": "ghost" });
        let r = tool.execute(input).await;

        assert!(!r.is_error);
        assert!(r.content.contains("No symbol named `ghost`"));
        assert!(r.content.contains("alpha"));
        assert!(r.content.contains("Beta"));
    }

    #[tokio::test]
    async fn symbol_on_unsupported_file_type_is_recoverable() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, "just some prose\nnot code\n").unwrap();

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file.to_str().unwrap(), "symbol": "anything" });
        let r = tool.execute(input).await;

        assert!(!r.is_error);
        assert!(r.content.contains("only available for Rust"));
        // Must NOT dump the file content (that would defeat the token saving).
        assert!(!r.content.contains("just some prose"));
    }

    #[tokio::test]
    async fn empty_symbol_param_falls_back_to_full_read() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("code.rs");
        std::fs::write(&file, "fn alpha() {\n    1\n}\n").unwrap();

        let tool = ReadTool::new(None);
        // An empty symbol string must be treated as "no symbol" → full file.
        let input = json!({ "file_path": file.to_str().unwrap(), "symbol": "" });
        let r = tool.execute(input).await;

        assert!(!r.is_error);
        assert!(r.content.contains("1\tfn alpha"));
        assert!(!r.content.contains("Symbol `"));
    }
}
