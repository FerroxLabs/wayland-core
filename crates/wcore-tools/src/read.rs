use std::path::Path;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::file_state::{FileState, Provenance};
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolEffectKind, ToolResult};

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

/// Ceiling on what Read will INGEST, decided from the bytes actually read
/// rather than from a stat it does not read from.
///
/// Distinct from [`ReadTool::max_result_size`] (100_000), which caps what Read
/// RETURNS and used to fire only AFTER the whole file had been slurped,
/// line-split, numbered into a `Vec<String>` and joined — 5.4x the file's size
/// in RSS to deliver 100 KB.
///
/// This MUST stay a compile-time constant with no config or per-call override.
/// `tool_output_limits.rs` reads `max_bytes` out of tool INPUT (model
/// controlled) and project config is untrusted
/// (BL-UNTRUSTED-RESOURCE-LIMITS), so wiring this to either surface would
/// convert a DoS fix into a DoS switch. Any future tunability must be
/// operator-owned and may only LOWER it.
pub const MAX_READ_INGEST_BYTES: u64 = 20 * 1024 * 1024;

/// Default line window when the caller gives no `limit`. Previously
/// `lines.len()` — i.e. "every line in the file", so the render was sized by
/// the file rather than by what the tool can return.
const DEFAULT_READ_LINE_LIMIT: usize = 2_000;

/// Reserved out of [`ReadTool::max_result_size`] for the continuation marker,
/// so the marker itself can never be what pushes the result into
/// `orchestration::truncate_result`'s head/tail elision.
const MARKER_RESERVE: usize = 256;

/// What Read returns at most. The orchestrator applies the same number, but
/// the TOOL owning it is the point: a generic middle-elider knows nothing
/// about line numbering, so eliding a numbered body makes line numbers jump
/// discontinuously and the tail half start mid-line.
const READ_MAX_RESULT_SIZE: usize = 100_000;

/// One bounded pass over the requested window.
struct RenderedWindow {
    /// Numbered lines, marker-FREE. This is what the dedup/diff cache stores;
    /// a marker line has no `\t`, so `strip_line_numbers` would keep it
    /// verbatim and it would become a phantom content line in every future
    /// diff.
    body: String,
    /// How many lines were actually emitted.
    lines: usize,
    /// The window stopped early in a way the CALLER did not ask for: it hit
    /// the byte budget, or it hit the DEFAULT line cap because no explicit
    /// `limit` was given. An explicit `limit` honoured exactly is not a
    /// truncation and must not be annotated — a caller that asked for 3 lines
    /// and got 3 lines was not short-changed.
    truncated: bool,
}

/// Render `text`'s window as numbered lines, stopping at the FIRST of
/// `limit` lines or `byte_budget` accumulated bytes.
///
/// Single pass, one `String`: no `Vec<&str>` of every line in the file, no
/// `Vec<String>` of every numbered line, no `join`.
fn render_numbered_window(
    text: &str,
    offset: usize,
    limit: Option<usize>,
    byte_budget: usize,
) -> RenderedWindow {
    let max_lines = limit.unwrap_or(DEFAULT_READ_LINE_LIMIT);
    let mut body = String::new();
    let mut lines = 0usize;
    let mut truncated = false;
    let mut it = text.lines().skip(offset);

    while lines < max_lines {
        let Some(line) = it.next() else {
            return RenderedWindow {
                body,
                lines,
                truncated,
            };
        };
        let entry = format!("{:>6}\t{}", offset + lines + 1, line);
        if body.is_empty() {
            if entry.len() > byte_budget {
                // A single line wider than the whole budget. Emit a
                // byte-bounded prefix rather than blowing the cap or
                // returning nothing at all.
                let mut cut = byte_budget;
                while cut > 0 && !entry.is_char_boundary(cut) {
                    cut -= 1;
                }
                body.push_str(&entry[..cut]);
                return RenderedWindow {
                    body,
                    lines: 1,
                    truncated: true,
                };
            }
            body.push_str(&entry);
        } else {
            if body.len() + 1 + entry.len() > byte_budget {
                truncated = true;
                break;
            }
            body.push('\n');
            body.push_str(&entry);
        }
        lines += 1;
    }
    // Hit the line cap with more of the file left. Only report it when the
    // cap was OURS (no explicit `limit`), not the caller's.
    if !truncated && limit.is_none() && it.next().is_some() {
        truncated = true;
    }
    RenderedWindow {
        body,
        lines,
        truncated,
    }
}

/// Never claim a total the tool did not count — that is the same class of
/// confident-wrong-number this bound exists to stop.
fn continuation_marker(next_offset: usize) -> String {
    format!(
        "\n... [window ends here; more of this file remains — pass offset={next_offset} to \
         continue, or use Grep to find a pattern]"
    )
}

/// Honest refusal for a file over [`MAX_READ_INGEST_BYTES`].
///
/// Refusing beats serving a partial: it is the `pdf_tool::MAX_PDF_INGEST_BYTES`
/// precedent already in the tree, it is one deterministic testable rule, and
/// the model has two working alternatives.
fn oversize_refusal(file_path: &str, size: Option<u64>) -> ToolResult {
    let measured = match size {
        Some(n) => format!("{n} bytes"),
        None => format!("over {MAX_READ_INGEST_BYTES} bytes"),
    };
    ToolResult {
        content: format!(
            "Failed to read file {file_path}: it is {measured}, past Read's \
             {MAX_READ_INGEST_BYTES}-byte ingest limit. Use Grep to search it for a pattern, \
             Bash with `sed -n 'START,ENDp' <file>` to pull a line range, or doc_extract for \
             office documents."
        ),
        is_error: true,
    }
}

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
            // `count()`, not `collect()`: the header needs the total, not a
            // fat pointer per line of the file.
            let total = text.lines().count();
            // `resolve_symbol` only returns Found for non-empty files with the
            // window inside bounds; clamp defensively anyway.
            let s = start.clamp(1, total.max(1));
            let e = end.clamp(s, total.max(1));
            let mut header = format!(
                "Symbol `{symbol}` ({kind:?}, lines {s}\u{2013}{e} of {total}). Re-read without \
                 symbol= for the full file, or with offset/limit for a different window."
            );
            if multiple {
                header.push_str(&format!(
                    "\n(Multiple symbols named `{symbol}` exist; showing the first.)"
                ));
            }
            // The symbol window is rendered under the same byte budget as any
            // other window. It needs its OWN marker: `execute_with_ctx`
            // returns here BEFORE offset/limit are ever consulted, so
            // "pass offset=N to continue" would be false advice while
            // `symbol=` is set.
            let budget = READ_MAX_RESULT_SIZE
                .saturating_sub(MARKER_RESERVE + header.len())
                .max(1);
            let window = render_numbered_window(text, s - 1, Some(e - s + 1), budget);
            let mut content = format!("{header}\n{}", window.body);
            if window.truncated {
                content.push_str(&format!(
                    "\n... [symbol window truncated at the result byte budget; re-read \
                     without symbol= and pass offset={} to continue]",
                    s - 1 + window.lines
                ));
            }
            ToolResult {
                content,
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
         - By default it reads the first 2000 lines (or 100 KB, whichever comes first) and tells \
         you when more remains; use offset and limit to move the window.\n\
         - A file larger than 20 MiB is refused outright — use Grep, or Bash with sed, instead.\n\
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

        // Read file from disk, BOUNDED. The size is taken from the descriptor
        // we opened, not from a separate stat that could name a different
        // object.
        let mut handle = match std::fs::File::open(&validated) {
            Ok(f) => f,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to read file {}: {}", file_path, e),
                    is_error: true,
                };
            }
        };
        let mut content = Vec::new();
        {
            use std::io::Read as _;
            if let Err(e) = handle
                .by_ref()
                .take(MAX_READ_INGEST_BYTES.saturating_add(1))
                .read_to_end(&mut content)
            {
                return ToolResult {
                    content: format!("Failed to read file {}: {}", file_path, e),
                    is_error: true,
                };
            }
        }
        // Refuse BEFORE the binary sniff and before any render. Declared
        // behaviour change: a >20 MiB BINARY file now returns a hard error
        // instead of `(binary file, N bytes)`.
        if content.len() as u64 > MAX_READ_INGEST_BYTES {
            return oversize_refusal(file_path, handle.metadata().ok().map(|m| m.len()));
        }

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

        let effective_offset = offset.unwrap_or(0);
        let window = render_numbered_window(
            text.as_ref(),
            effective_offset,
            limit,
            READ_MAX_RESULT_SIZE.saturating_sub(MARKER_RESERVE),
        );
        // Marker-FREE body: this is what the cache stores and what the dedup
        // comparison keys on.
        let result_content = window.body;
        let response_content = if window.truncated {
            format!(
                "{result_content}{}",
                continuation_marker(effective_offset + window.lines)
            )
        } else {
            result_content.clone()
        };

        // Update cache after successful read.
        if let Some(cache_arc) = &self.file_cache
            && let (Ok(mut cache), Some(mtime)) = (cache_arc.write(), mtime_ms)
        {
            let gen_at_read = cache.compaction_generation();
            cache.insert(
                validated.clone(),
                FileState {
                    content: result_content,
                    mtime_ms: mtime,
                    offset,
                    limit,
                    provenance: Provenance::ReadResult,
                    gen_at_read,
                },
            );
        }

        ToolResult {
            content: response_content,
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
        // `diff_base` is only populated when a diff would be SOUND to emit.
        //
        // #182: this is deliberately NOT gated on the route's `optimize_reads`
        // flag. Diff-resend shrinks THIS agent's own context — it is model-facing
        // ("apply this diff to your previous Read") — so it applies on every
        // route, exactly like the always-on unchanged-stub below. `optimize_reads`
        // (== `input_optimization == "client"`) governs OUTPUT-side opts and
        // wire/billing input dedup, which a server-side router (flux-router,
        // openrouter, …) does upstream; but a router cannot shrink this process's
        // context window, so on router routes a mid-turn re-read of a changed file
        // was re-injecting the FULL file (the #182 bloat). The soundness guards
        // below still hold on all routes:
        //   * this is a full read (offset/limit None) matching the cached window,
        //   * the caller is the main agent (`source_agent` is None) — the cache is
        //     process-wide across sub-agents, so a sibling's read must never seed
        //     a base this transcript never contained,
        //   * the base is a `ReadResult` (something the model actually saw), and
        //   * the base is still visible: the compaction generation has not moved
        //     since it was cached, so the diff's reference content has not been
        //     collapsed/cleared out of the transcript,
        //   * and `build_read_diff` verifies the diff reconstructs the current
        //     content byte-for-byte before it is ever emitted.
        let is_full_read = offset.is_none() && limit.is_none();
        let single_agent = ctx.source_agent.is_none();
        let mut diff_base: Option<String> = None;
        // Token-burn fix: the still-in-transcript cached content (ANY window) for
        // a post-read content-equality dedup. Distinct from `diff_base` (which is
        // exact-window + full-read only): this also catches a narrower/overlapping
        // re-range and mtime churn.
        let mut dedup_base: Option<String> = None;
        let mut current_gen: u64 = 0;

        if symbol.is_none()
            && let (Some(cache_arc), Some(current_mtime)) = (&self.file_cache, mtime_ms)
            && let Ok(mut cache) = cache_arc.write()
        {
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
                if is_full_read
                    && single_agent
                    && matches_window
                    && cached.provenance == Provenance::ReadResult
                    && cached.gen_at_read == current_gen
                {
                    diff_base = Some(cached.content.clone());
                }
                // Token-burn fix: capture the cached content under the SAME
                // soundness guards as the stub (referenced Read still in this
                // transcript: single agent, same compaction generation) but WITHOUT
                // requiring an exact window or matching mtime. A post-read
                // content-equality check then stubs any re-read whose exact numbered
                // lines the model already holds — defeating the mtime churn and
                // varied-range re-reads that the window-exact fast path above misses.
                if cached.provenance == Provenance::ReadResult
                    && single_agent
                    && cached.gen_at_read == current_gen
                {
                    dedup_base = Some(cached.content.clone());
                }
            }
        }

        // Bounded ingest. The single error arm below is unchanged, so every
        // existing refusal wording — including `SecretDenyFs`'s, which
        // `full_posture_secret_jail_test` asserts starts with
        // "Failed to read file " — stays byte-identical.
        let content = match ctx.vfs.read_capped(path, MAX_READ_INGEST_BYTES).await {
            Ok(bytes) => bytes,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to read file {file_path}: {e}"),
                    is_error: true,
                };
            }
        };
        // Refuse BEFORE the binary sniff and before any render. The size for
        // the message comes through the SAME vfs (and therefore the same
        // jails) that produced the bytes — never a raw `std::fs::metadata`,
        // which `SandboxedFs::contain` may have re-rooted away from.
        if content.len() as u64 > MAX_READ_INGEST_BYTES {
            let size = ctx.vfs.metadata(path).await.ok().map(|m| m.size);
            return oversize_refusal(file_path, size.filter(|n| *n > 0));
        }

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

        let effective_offset = offset.unwrap_or(0);
        let window = render_numbered_window(
            text.as_ref(),
            effective_offset,
            limit,
            READ_MAX_RESULT_SIZE.saturating_sub(MARKER_RESERVE),
        );
        // Marker-FREE body. It is what gets cached and what the dedup
        // comparison keys on: a marker line has no `\t`, so
        // `read_diff::strip_line_numbers` would keep it verbatim and it would
        // become a phantom content line in every future diff, failing
        // `build_read_diff`'s byte-exact reconstruction check and silently
        // degrading every re-read to full content (the #182 bloat).
        let result_content = window.body;
        let window_marker = window
            .truncated
            .then(|| continuation_marker(effective_offset + window.lines));

        // Token-burn fix: if the exact numbered lines we would return are already
        // present verbatim in a still-current cached Read of this file, the model
        // already holds them — return the unchanged stub instead of re-injecting,
        // and fall through WITHOUT overwriting the (possibly broader) cached window
        // below, so a narrow re-read never evicts the full-file entry. Every line
        // carries a unique `%6d\t` number prefix, so a substring match is
        // line-aligned and unambiguous; a real edit changes the numbered text and
        // correctly misses, falling through to diff-resend / full content.
        if let Some(base) = &dedup_base
            && !result_content.is_empty()
            && base.contains(&result_content)
        {
            return ToolResult {
                content: FILE_UNCHANGED_STUB.to_string(),
                is_error: false,
            };
        }

        // Token-opt (diff-resend): if we captured a sound base and the content
        // actually changed, try to answer with a line diff. The diff is byte-exact
        // verified to reconstruct the current content before it is emitted
        // (`build_read_diff`); any failure falls back to the full content.
        let mut response_content = match &window_marker {
            Some(marker) => format!("{result_content}{marker}"),
            None => result_content.clone(),
        };
        if let Some(base_numbered) = &diff_base {
            let base_raw = crate::read_diff::strip_line_numbers(base_numbered);
            // Bounded by the window, not by the file.
            let cur_raw: Vec<String> = text
                .lines()
                .skip(effective_offset)
                .take(window.lines)
                .map(|s| s.to_string())
                .collect();
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
        READ_MAX_RESULT_SIZE
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    /// Reading a file mutates nothing outside this process, so there is no
    /// external effect an interrupted invocation could have left behind and no
    /// additional effect a repeat could create. Inheriting the conservative
    /// `Opaque` default made every read error — including a refused malformed
    /// path — an *ambiguous* effect, which is nonterminal and killed the
    /// session (live UAT defect D1).
    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        ToolEffectContract {
            kind: ToolEffectKind::RepeatSafe,
            reconciler: None,
        }
    }

    /// Read cannot mutate anything for any input it accepts: it opens a path
    /// for reading and returns bytes. Safe under `[default] read_only = true`.
    fn read_only_safe(&self, _input: &Value) -> bool {
        true
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

    // ── Read bounds: ingest ceiling + windowed render ─────────────────────
    //
    // ReadTool used to have NO size bound of any kind: it slurped the whole
    // file, allocated a `Vec<&str>` of every line, then a `Vec<String>` of
    // every numbered line, then `join`ed them — and only two crates away did
    // `orchestration::truncate_result` throw 99.9% of that away. A 120 MB file
    // cost ~620 MiB RSS and 93 s of CPU to deliver 100 KB, and the model was
    // then shown a head+tail elision of a NUMBERED body, so it confidently
    // reported a "last line" under a line number that does not exist.

    /// Comfortably over `MAX_READ_INGEST_BYTES` (20 MiB). Spelled as a literal
    /// rather than derived from the constant so this test compiles — and fails
    /// on its assertions rather than on rustc — against the unfixed tree.
    /// `ingest_ceiling_is_the_documented_twenty_mib` pins the two together.
    const OVERSIZE_TARGET_BYTES: usize = 21 * 1024 * 1024;

    const HEAD_CANARY: &str = "WCORE_READ_HEAD_CANARY_9f3a1c";
    const TAIL_CANARY: &str = "WCORE_READ_TAIL_CANARY_7b2e40";

    /// Write `bytes_target` bytes of ASCII by looping a real buffer.
    ///
    /// Deliberately NOT `set_len`: a sparse hole is NUL bytes, which would trip
    /// the binary sniff and make the test pass for the wrong reason.
    fn write_oversized(path: &std::path::Path, bytes_target: usize) -> u64 {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        writeln!(f, "{HEAD_CANARY}").unwrap();
        let filler = format!("{}\n", "f".repeat(4095));
        let mut written = HEAD_CANARY.len() + 1;
        while written < bytes_target {
            f.write_all(filler.as_bytes()).unwrap();
            written += filler.len();
        }
        writeln!(f, "{TAIL_CANARY}").unwrap();
        f.into_inner().unwrap().sync_all().unwrap();
        std::fs::metadata(path).unwrap().len()
    }

    fn head_of(s: &str) -> String {
        s.chars().take(240).collect()
    }

    /// `OVERSIZE_TARGET_BYTES` is spelled as a literal so the oversize test
    /// compiles against the unfixed tree. This keeps the two honest.
    #[test]
    fn ingest_ceiling_is_the_documented_twenty_mib() {
        assert_eq!(MAX_READ_INGEST_BYTES, 20 * 1024 * 1024);
        assert!(
            (OVERSIZE_TARGET_BYTES as u64) > MAX_READ_INGEST_BYTES,
            "the oversize fixture must actually exceed the ceiling"
        );
        assert_eq!(READ_MAX_RESULT_SIZE, ReadTool::new(None).max_result_size());
    }

    /// A file over the ingest ceiling is REFUSED — before the binary sniff,
    /// before any render — and the refusal names the real size.
    ///
    /// Both entry points, with separate fixture files so the second leg cannot
    /// hit the unchanged-stub.
    #[tokio::test]
    async fn oversized_files_are_refused_by_both_entry_points() {
        let dir = tempdir().unwrap();

        for (name, via_ctx) in [("big_legacy.txt", false), ("big_ctx.txt", true)] {
            let file_path = dir.path().join(name);
            let actual = write_oversized(&file_path, OVERSIZE_TARGET_BYTES);

            let tool = ReadTool::new(None);
            let input = json!({ "file_path": file_path.to_str().unwrap() });
            let result = if via_ctx {
                tool.execute_with_ctx(input, &ctx_main()).await
            } else {
                tool.execute(input).await
            };

            assert!(
                result.is_error,
                "{name}: an oversized read must be refused, got: {}",
                head_of(&result.content)
            );
            assert!(
                result.content.contains(&actual.to_string()),
                "{name}: an honest refusal names the size ({actual}): {}",
                head_of(&result.content)
            );
            assert!(
                result.content.contains("limit"),
                "{name}: the refusal must name the limit: {}",
                head_of(&result.content)
            );
            assert!(
                !result.content.contains(HEAD_CANARY),
                "{name}: the file was rendered anyway (head canary present)"
            );
            assert!(
                !result.content.contains(TAIL_CANARY),
                "{name}: the file was rendered anyway (tail canary present)"
            );
        }
    }

    /// A file UNDER the ingest ceiling is served, but the render is windowed:
    /// the tool owns its own bound instead of handing a huge string to a
    /// generic middle-elider that knows nothing about line numbering.
    #[tokio::test]
    async fn an_unqualified_read_returns_a_bounded_window_with_an_honest_marker() {
        const WINDOW_CANARY: &str = "WCORE_READ_WINDOW_CANARY_4d81ff";
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("many_lines.txt");
        let mut body = String::new();
        for i in 0..5_000 {
            body.push_str(&format!("line {i}\n"));
        }
        body.push_str(WINDOW_CANARY);
        body.push('\n');
        std::fs::write(&file_path, &body).unwrap();

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = tool.execute_with_ctx(input, &ctx_main()).await;

        assert!(!result.is_error, "{}", head_of(&result.content));
        assert!(
            result.content.contains("1\tline 0"),
            "the window still starts at the top"
        );
        assert!(
            !result.content.contains(WINDOW_CANARY),
            "the tail was not in the requested window and must not be presented \
             as if it were"
        );
        assert!(
            result.content.contains("pass offset="),
            "the marker must exist and be actionable"
        );
        assert!(
            result.content.len() <= tool.max_result_size(),
            "the TOOL owns its bound, not the orchestrator: {} bytes",
            result.content.len()
        );

        // The marker's advice must be true: the window is movable.
        let tool2 = ReadTool::new(None);
        let far = json!({ "file_path": file_path.to_str().unwrap(), "offset": 4_995 });
        let r2 = tool2.execute_with_ctx(far, &ctx_main()).await;
        assert!(!r2.is_error, "{}", head_of(&r2.content));
        assert!(
            r2.content.contains(WINDOW_CANARY),
            "offset= must actually move the window: {}",
            head_of(&r2.content)
        );
    }

    /// The BYTE half of the render bound.
    ///
    /// Under the ingest ceiling AND under the line cap, but far over
    /// `max_result_size()`. A line-cap-only implementation passes every other
    /// test here and still hands the orchestrator ~1.6 MB to middle-elide —
    /// which is the reported defect.
    #[tokio::test]
    async fn a_few_very_long_lines_still_respect_the_byte_budget() {
        const BYTECAP_CANARY: &str = "WCORE_READ_BYTECAP_CANARY_c1e77a";
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("wide.txt");
        let mut body = String::new();
        for _ in 0..399 {
            body.push_str(&"w".repeat(4096));
            body.push('\n');
        }
        body.push_str(BYTECAP_CANARY);
        body.push('\n');
        std::fs::write(&file_path, &body).unwrap();

        let tool = ReadTool::new(None);
        let input = json!({ "file_path": file_path.to_str().unwrap() });
        let result = tool.execute_with_ctx(input, &ctx_main()).await;

        assert!(!result.is_error, "{}", head_of(&result.content));
        assert!(
            result.content.len() <= tool.max_result_size(),
            "400 x 4 KiB lines is under the 2000-line cap but must still respect \
             the byte budget; got {} bytes",
            result.content.len()
        );
        assert!(
            result.content.contains("pass offset="),
            "a byte-truncated window must still say so"
        );
        assert!(
            !result.content.contains(BYTECAP_CANARY),
            "the last line was not in the window and must not be presented"
        );
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

    /// #182: diff-resend is a context reduction, so it applies on EVERY route —
    /// including router-optimized ones (`optimize_reads` false). A server-side
    /// router optimizes the wire/billing but cannot shrink THIS process's
    /// context, so a mid-turn re-read of an externally-changed file must diff,
    /// not re-inject the full file (the exact bloat #182 reported on flux-router).
    #[tokio::test]
    async fn router_route_still_diffs_changed_reread() {
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 60, "ORIGINAL");

        // Plain cache: optimize_reads stays false (router-optimized route).
        let cache = make_cache();
        let tool = ReadTool::new(Some(cache));
        let ctx = ctx_main();
        let input = json!({ "file_path": file.to_str().unwrap() });

        let r1 = tool.execute_with_ctx(input.clone(), &ctx).await;
        assert!(!r1.content.contains(DIFF_RESEND_HEADER));
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = write_lines(dir.path(), "big.txt", 60, "PATCHED");

        let r2 = tool.execute_with_ctx(input, &ctx).await;
        assert!(!r2.is_error);
        assert!(
            r2.content.contains(DIFF_RESEND_HEADER),
            "#182: a router route must ALSO diff a changed re-read, got: {}",
            r2.content
        );
        assert!(r2.content.contains("PATCHED"));
        assert!(
            r2.content.len() < r1.content.len(),
            "diff must be smaller than the full content"
        );
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

    // -- content-equality dedup tests (token-burn fix) --
    // `make_cache()` leaves optimize_reads OFF, so these isolate the content
    // dedup from the diff-resend path (which only fires for full reads).

    #[tokio::test]
    async fn subrange_reread_of_unchanged_file_returns_stub() {
        // Read the whole file, then re-read a sub-range of the UNCHANGED file.
        // The window differs (so the exact-window fast path misses), but the
        // model already holds those lines from the full read -> stub, not re-inject.
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 60, "ORIGINAL");
        let tool = ReadTool::new(Some(make_cache()));
        let ctx = ctx_main();

        let full = json!({ "file_path": file.to_str().unwrap() });
        let r1 = tool.execute_with_ctx(full, &ctx).await;
        assert!(r1.content.contains("line 59"));

        let sub = json!({ "file_path": file.to_str().unwrap(), "offset": 0, "limit": 10 });
        let r2 = tool.execute_with_ctx(sub, &ctx).await;
        assert_eq!(
            r2.content, FILE_UNCHANGED_STUB,
            "a sub-range already covered by the full read must stub, got: {}",
            r2.content
        );
    }

    #[tokio::test]
    async fn full_reread_after_mtime_churn_with_identical_content_stubs() {
        // Rewrite identical bytes: mtime bumps so the exact-window+mtime fast path
        // misses, but the content is unchanged -> content dedup stubs it (this is
        // the case the ticket calls "mtime churn defeats dedup").
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 60, "ORIGINAL");
        let tool = ReadTool::new(Some(make_cache()));
        let ctx = ctx_main();
        let input = json!({ "file_path": file.to_str().unwrap() });

        tool.execute_with_ctx(input.clone(), &ctx).await;
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = write_lines(dir.path(), "big.txt", 60, "ORIGINAL"); // identical bytes, new mtime

        let r2 = tool.execute_with_ctx(input, &ctx).await;
        assert_eq!(
            r2.content, FILE_UNCHANGED_STUB,
            "mtime churn with identical content must stub, got: {}",
            r2.content
        );
    }

    #[tokio::test]
    async fn changed_content_is_never_stubbed() {
        // Correctness guard: when the bytes actually change, the numbered lines
        // differ, the substring match fails, and the model MUST receive the new
        // content — never a stub pointing at stale data.
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 60, "ORIGINAL");
        let tool = ReadTool::new(Some(make_cache()));
        let ctx = ctx_main();
        let input = json!({ "file_path": file.to_str().unwrap() });

        tool.execute_with_ctx(input.clone(), &ctx).await;
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = write_lines(dir.path(), "big.txt", 60, "PATCHED");

        let r2 = tool.execute_with_ctx(input, &ctx).await;
        assert_ne!(
            r2.content, FILE_UNCHANGED_STUB,
            "changed content must NOT be stubbed"
        );
        assert!(r2.content.contains("PATCHED"));
    }

    #[tokio::test]
    async fn subrange_reread_does_not_evict_full_entry() {
        // A sub-range re-read stubs WITHOUT overwriting the cache, so the broader
        // full-file entry survives (no window thrash). Proven by a subsequent full
        // re-read still stubbing against the surviving full entry.
        let dir = tempdir().unwrap();
        let file = write_lines(dir.path(), "big.txt", 60, "ORIGINAL");
        let tool = ReadTool::new(Some(make_cache()));
        let ctx = ctx_main();
        let full = json!({ "file_path": file.to_str().unwrap() });
        let sub = json!({ "file_path": file.to_str().unwrap(), "offset": 0, "limit": 10 });

        tool.execute_with_ctx(full.clone(), &ctx).await;
        let r_sub = tool.execute_with_ctx(sub, &ctx).await;
        assert_eq!(r_sub.content, FILE_UNCHANGED_STUB);

        let r3 = tool.execute_with_ctx(full, &ctx).await;
        assert_eq!(
            r3.content, FILE_UNCHANGED_STUB,
            "the full entry must survive a sub-range read (no thrash), got: {}",
            r3.content
        );
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
