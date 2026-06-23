//! T3-3.3.3 — Tool-result persistence helper ported from the prior
//! Wayland Python engine.
//!
//! Defense against context-window overflow operates at three layers:
//!
//! 1. **Per-tool output cap** — each tool pre-truncates its own output.
//!    This is the first line of defense and is not handled here.
//! 2. **Per-result persistence** ([`maybe_persist_tool_result`]) — after a
//!    tool returns, if its output exceeds the registered threshold the
//!    full output is written to a tempdir-backed file
//!    (`$TMPDIR/wayland-results/<tool_use_id>.txt`) and the in-context
//!    content is replaced by a `<persisted-output>` preview + path
//!    reference. The model can then `Read` the file to access the full
//!    output on demand.
//! 3. **Per-turn aggregate budget** ([`enforce_turn_budget`]) — after all
//!    tool results in a single assistant turn are collected, if the total
//!    exceeds [`BudgetConfig::turn_budget`] the largest non-persisted
//!    results are spilled to disk (with `threshold = 0`) until the
//!    aggregate is under budget.
//!
//! ## Differences from the Python original
//!
//! * The Python version writes via an `env.execute(...)` heredoc so the
//!   spill file lands inside the active sandbox (Docker / SSH / Modal).
//!   wcore-tools doesn't yet have a uniform sandbox-env abstraction —
//!   we write through the standard library to a real tempdir
//!   (`std::env::temp_dir().join("wayland-results")`) which matches the
//!   default Python branch (`STORAGE_DIR = "/tmp/wayland-results"`) on
//!   Linux/macOS and uses `%TEMP%\\wayland-results` on Windows. A
//!   future sandbox integration can re-route writes by passing an
//!   alternate [`StorageDir`] override.
//! * The Python `BudgetConfig` is a separate module; here we keep the
//!   defaults inline in [`BudgetConfig`] so the helper is
//!   self-contained. Per-tool thresholds can be supplied via
//!   [`BudgetConfig::with_tool_threshold`].
//! * Heredoc-marker collision handling is unnecessary because we use
//!   `std::fs::write` instead of shell heredocs.
//!
//! Pure-helper module: nothing here registers a tool. Callers that want
//! to plug this into the registry should wire `maybe_persist_tool_result`
//! into their post-tool-execution path and `enforce_turn_budget` into
//! their turn-finalization path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Default per-result preview window (chars).
pub const DEFAULT_PREVIEW_SIZE_CHARS: usize = 2_000;

/// Default per-result persistence threshold (chars). Tools whose output
/// exceeds this are spilled to disk in [`maybe_persist_tool_result`].
pub const DEFAULT_RESULT_THRESHOLD_CHARS: usize = 25_000;

/// Default aggregate turn budget (chars) for [`enforce_turn_budget`].
pub const DEFAULT_TURN_BUDGET_CHARS: usize = 200_000;

/// Opening sentinel for the persisted-output replacement block.
pub const PERSISTED_OUTPUT_TAG: &str = "<persisted-output>";
/// Closing sentinel for the persisted-output replacement block.
pub const PERSISTED_OUTPUT_CLOSING_TAG: &str = "</persisted-output>";

/// Synthetic tool-name used by [`enforce_turn_budget`] when forcing a
/// spill. Matches the Python `_BUDGET_TOOL_NAME` sentinel.
pub const BUDGET_TOOL_NAME: &str = "__budget_enforcement__";

/// Subdirectory under the OS temp dir where spill files live.
pub const STORAGE_SUBDIR: &str = "wayland-results";

/// Threshold sentinel meaning "never persist this tool's output".
pub const THRESHOLD_DISABLED: usize = usize::MAX;

/// Tools whose persistence threshold must NEVER be overridden — the
/// engine pins them to [`THRESHOLD_DISABLED`] regardless of per-tool
/// config or registry hints. Mirrors `PINNED_THRESHOLDS` in the Python
/// `budget_config.py` (T3-3-4).
///
/// Rationale: `read` is the canonical recovery path for a persisted
/// tool result. If `read`'s own output were itself persisted, the
/// caller would have to issue another `read` to recover it — creating
/// an unbounded persist → read → persist loop. Pinning `read` to
/// [`THRESHOLD_DISABLED`] makes that loop unrepresentable.
///
/// Resolution priority (highest to lowest):
/// 1. **Pinned** (this list) — wins unconditionally.
/// 2. Per-call `threshold_override` argument to
///    [`maybe_persist_tool_result`].
/// 3. Per-tool entry in [`BudgetConfig::per_tool_thresholds`].
/// 4. [`BudgetConfig::default_result_threshold`].
pub const PINNED_THRESHOLDS: &[(&str, usize)] = &[("Read", THRESHOLD_DISABLED)];

/// Returns the pinned threshold for `tool_name`, if any. Pinned values
/// override every other resolution layer — see [`PINNED_THRESHOLDS`].
#[inline]
pub fn pinned_threshold(tool_name: &str) -> Option<usize> {
    PINNED_THRESHOLDS
        .iter()
        .find_map(|(name, t)| (*name == tool_name).then_some(*t))
}

/// Budget configuration shared by [`maybe_persist_tool_result`] and
/// [`enforce_turn_budget`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Default per-result threshold applied when a tool has no
    /// explicit entry in [`Self::per_tool_thresholds`].
    pub default_result_threshold: usize,
    /// Aggregate per-turn budget.
    pub turn_budget: usize,
    /// Preview window size used for spill replacements.
    pub preview_size: usize,
    /// Per-tool overrides. A value of [`THRESHOLD_DISABLED`] means
    /// "never persist". A value of `0` means "always persist".
    pub per_tool_thresholds: HashMap<String, usize>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            default_result_threshold: DEFAULT_RESULT_THRESHOLD_CHARS,
            turn_budget: DEFAULT_TURN_BUDGET_CHARS,
            preview_size: DEFAULT_PREVIEW_SIZE_CHARS,
            per_tool_thresholds: HashMap::new(),
        }
    }
}

impl BudgetConfig {
    /// Builder-style override for a per-tool threshold.
    pub fn with_tool_threshold(mut self, tool: impl Into<String>, threshold: usize) -> Self {
        self.per_tool_thresholds.insert(tool.into(), threshold);
        self
    }

    /// Resolve the effective threshold for `tool_name`.
    ///
    /// Priority: [`PINNED_THRESHOLDS`] (engine invariant) ->
    /// [`Self::per_tool_thresholds`] -> [`Self::default_result_threshold`].
    /// Pinned entries cannot be overridden via config — see
    /// [`PINNED_THRESHOLDS`] for rationale.
    pub fn resolve_threshold(&self, tool_name: &str) -> usize {
        if let Some(pinned) = pinned_threshold(tool_name) {
            return pinned;
        }
        self.per_tool_thresholds
            .get(tool_name)
            .copied()
            .unwrap_or(self.default_result_threshold)
    }
}

/// Where spill files are written. Defaults to
/// `std::env::temp_dir().join("wayland-results")`; tests inject a
/// tempdir-rooted override.
#[derive(Debug, Clone)]
pub struct StorageDir(pub PathBuf);

impl StorageDir {
    /// OS default — `$TMPDIR/wayland-results` (or `/tmp/wayland-results`
    /// on Unix when `TMPDIR` is unset).
    pub fn os_default() -> Self {
        Self(std::env::temp_dir().join(STORAGE_SUBDIR))
    }

    /// Resolve the path for `tool_use_id` underneath this storage dir.
    pub fn path_for(&self, tool_use_id: &str) -> PathBuf {
        self.0.join(format!("{}.txt", sanitize_id(tool_use_id)))
    }
}

impl Default for StorageDir {
    fn default() -> Self {
        Self::os_default()
    }
}

/// Result type returned by [`maybe_persist_tool_result`] so callers can
/// distinguish "nothing happened" from "spilled to disk" from "inline
/// fallback because the write failed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistOutcome {
    /// Output was under threshold — `content` is unchanged.
    Untouched,
    /// Output was spilled to disk; `path` holds the file location and
    /// the returned content is the `<persisted-output>` replacement.
    Persisted { path: PathBuf, original_size: usize },
    /// Output exceeded threshold but the disk write failed; the
    /// returned content is an inline-truncated fallback.
    InlineTruncated { original_size: usize },
}

/// Errors surfaced by the helper. The Python original logs and falls
/// back silently; we keep that behaviour but expose this type for
/// direct disk-write helpers used by tests.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("failed to create storage dir {dir}: {source}")]
    CreateDir {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write spill file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read spill file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Truncate `content` at the last newline within `max_chars`.
///
/// Returns `(preview, has_more)`. When the cut-point is past the
/// halfway mark we snap to the newline; otherwise we keep the raw cut
/// so we don't return a near-empty preview for newline-light inputs.
pub fn generate_preview(content: &str, max_chars: usize) -> (String, bool) {
    if content.chars().count() <= max_chars {
        return (content.to_string(), false);
    }
    // Cut on char boundary, not byte boundary.
    let cut: String = content.chars().take(max_chars).collect();
    if let Some(nl) = cut.rfind('\n')
        && nl > max_chars / 2
    {
        return (cut[..=nl].to_string(), true);
    }
    (cut, true)
}

/// Build the `<persisted-output>` replacement block.
fn build_persisted_message(
    preview: &str,
    has_more: bool,
    original_size: usize,
    file_path: &Path,
) -> String {
    let size_kb = original_size as f64 / 1024.0;
    let size_str = if size_kb >= 1024.0 {
        format!("{:.1} MB", size_kb / 1024.0)
    } else {
        format!("{:.1} KB", size_kb)
    };

    let preview_chars = preview.chars().count();
    let mut msg = String::with_capacity(preview.len() + 256);
    msg.push_str(PERSISTED_OUTPUT_TAG);
    msg.push('\n');
    msg.push_str(&format!(
        "This tool result was too large ({} characters, {}).\n",
        format_with_commas(original_size),
        size_str
    ));
    msg.push_str(&format!("Full output saved to: {}\n", file_path.display()));
    msg.push_str(
        "Use the read_file tool with offset and limit to access specific sections of this output.\n\n",
    );
    msg.push_str(&format!("Preview (first {} chars):\n", preview_chars));
    msg.push_str(preview);
    if has_more {
        msg.push_str("\n...");
    }
    msg.push('\n');
    msg.push_str(PERSISTED_OUTPUT_CLOSING_TAG);
    msg
}

/// `1234567` -> `1,234,567`. Tiny helper used only inside the
/// persisted-message header.
fn format_with_commas(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Sanitize a tool-use id so it's safe to use as a filename. Strips
/// path separators and limits length to 128 chars.
fn sanitize_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.len() > 128 {
        cleaned[..128].to_string()
    } else {
        cleaned
    }
}

/// Write `content` to `path`, creating parent dirs as needed.
pub fn write_spill_file(path: &Path, content: &str) -> Result<(), StorageError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|source| StorageError::CreateDir {
            dir: dir.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, content).map_err(|source| StorageError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// Read a previously-spilled file back from disk. Useful for tests
/// and for callers that want to surface the spill via [`crate::read`].
pub fn read_spill_file(path: &Path) -> Result<String, StorageError> {
    std::fs::read_to_string(path).map_err(|source| StorageError::Read {
        path: path.to_path_buf(),
        source,
    })
}

/// Layer 2: persist oversized result to disk and return a preview +
/// path replacement.
///
/// Returns `(replacement_content, outcome)` so callers can both update
/// the model-visible payload and log/observe what happened.
///
/// `threshold_override` takes precedence over `config.resolve_threshold`.
/// Pass `Some(0)` from [`enforce_turn_budget`] to force a spill.
pub fn maybe_persist_tool_result(
    content: &str,
    tool_name: &str,
    tool_use_id: &str,
    storage: &StorageDir,
    config: &BudgetConfig,
    threshold_override: Option<usize>,
) -> (String, PersistOutcome) {
    // PINNED_THRESHOLDS wins unconditionally — engine invariant against the
    // persist→read→persist loop. Per-call `threshold_override` cannot bypass.
    let effective_threshold = pinned_threshold(tool_name).unwrap_or_else(|| {
        threshold_override.unwrap_or_else(|| config.resolve_threshold(tool_name))
    });

    if effective_threshold == THRESHOLD_DISABLED {
        return (content.to_string(), PersistOutcome::Untouched);
    }

    let len_chars = content.chars().count();
    if len_chars <= effective_threshold {
        return (content.to_string(), PersistOutcome::Untouched);
    }

    let spill_path = storage.path_for(tool_use_id);
    let (preview, has_more) = generate_preview(content, config.preview_size);

    match write_spill_file(&spill_path, content) {
        Ok(()) => {
            tracing::info!(
                tool = tool_name,
                tool_use_id = tool_use_id,
                chars = len_chars,
                path = %spill_path.display(),
                "persisted large tool result"
            );
            let msg = build_persisted_message(&preview, has_more, len_chars, &spill_path);
            (
                msg,
                PersistOutcome::Persisted {
                    path: spill_path,
                    original_size: len_chars,
                },
            )
        }
        Err(err) => {
            tracing::warn!(
                tool = tool_name,
                tool_use_id = tool_use_id,
                error = %err,
                "tool-result spill write failed; falling back to inline truncation"
            );
            let mut msg = preview;
            msg.push_str(&format!(
                "\n\n[Truncated: tool response was {} chars. \
                 Full output could not be saved to disk.]",
                format_with_commas(len_chars)
            ));
            (
                msg,
                PersistOutcome::InlineTruncated {
                    original_size: len_chars,
                },
            )
        }
    }
}

/// A single tool-result message considered by [`enforce_turn_budget`].
/// The `id` is used as the spill filename when persistence is triggered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultMessage {
    pub id: String,
    pub content: String,
}

/// Layer 3: enforce the aggregate per-turn budget.
///
/// If `sum(len(msg.content)) > config.turn_budget`, spill the largest
/// non-persisted results first (with `threshold_override = Some(0)`)
/// until under budget. Already-persisted entries (those whose
/// `content` already contains [`PERSISTED_OUTPUT_TAG`]) are skipped.
///
/// Returns the number of messages that were rewritten in place.
pub fn enforce_turn_budget(
    messages: &mut [ToolResultMessage],
    storage: &StorageDir,
    config: &BudgetConfig,
) -> usize {
    let mut total: usize = messages.iter().map(|m| m.content.chars().count()).sum();
    if total <= config.turn_budget {
        return 0;
    }

    // Collect (index, size) for non-persisted candidates, then sort
    // largest-first so we maximize budget recovery per spill.
    let mut candidates: Vec<(usize, usize)> = messages
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            if m.content.contains(PERSISTED_OUTPUT_TAG) {
                None
            } else {
                Some((i, m.content.chars().count()))
            }
        })
        .collect();
    candidates.sort_by_key(|a| std::cmp::Reverse(a.1));

    let mut rewritten = 0usize;
    for (idx, size) in candidates {
        if total <= config.turn_budget {
            break;
        }
        let msg = &messages[idx];
        let (replacement, outcome) = maybe_persist_tool_result(
            &msg.content,
            BUDGET_TOOL_NAME,
            &msg.id,
            storage,
            config,
            Some(0),
        );
        match outcome {
            PersistOutcome::Persisted { .. } | PersistOutcome::InlineTruncated { .. } => {
                let new_size = replacement.chars().count();
                if new_size != size {
                    total = total.saturating_sub(size).saturating_add(new_size);
                    messages[idx].content = replacement;
                    rewritten += 1;
                    tracing::info!(
                        tool_use_id = %messages[idx].id,
                        original_size = size,
                        "budget enforcement: persisted tool result"
                    );
                }
            }
            PersistOutcome::Untouched => {}
        }
    }
    rewritten
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_storage(tmp: &TempDir) -> StorageDir {
        StorageDir(tmp.path().join("wayland-results"))
    }

    #[test]
    fn round_trip_persists_and_reads_back() {
        let tmp = TempDir::new().unwrap();
        let storage = make_storage(&tmp);
        let config = BudgetConfig::default();

        let big = "x".repeat(config.default_result_threshold + 5_000);
        let (replacement, outcome) =
            maybe_persist_tool_result(&big, "read_file", "tooluse_abc123", &storage, &config, None);

        let path = match outcome {
            PersistOutcome::Persisted {
                path,
                original_size,
            } => {
                assert_eq!(original_size, big.chars().count());
                path
            }
            other => panic!("expected Persisted, got {:?}", other),
        };

        assert!(replacement.starts_with(PERSISTED_OUTPUT_TAG));
        assert!(replacement.contains(&path.display().to_string()));
        assert!(replacement.ends_with(PERSISTED_OUTPUT_CLOSING_TAG));

        let on_disk = read_spill_file(&path).unwrap();
        assert_eq!(on_disk, big);
    }

    #[test]
    fn below_threshold_returns_untouched() {
        let tmp = TempDir::new().unwrap();
        let storage = make_storage(&tmp);
        let config = BudgetConfig::default();

        let small = "hello world\n".repeat(10);
        let (replacement, outcome) =
            maybe_persist_tool_result(&small, "read_file", "id", &storage, &config, None);

        assert_eq!(replacement, small);
        assert_eq!(outcome, PersistOutcome::Untouched);
        // No file should have been created.
        assert!(!storage.path_for("id").exists());
    }

    #[test]
    fn threshold_disabled_skips_persistence() {
        let tmp = TempDir::new().unwrap();
        let storage = make_storage(&tmp);
        let config = BudgetConfig::default().with_tool_threshold("noisy_tool", THRESHOLD_DISABLED);

        let huge = "y".repeat(500_000);
        let (replacement, outcome) =
            maybe_persist_tool_result(&huge, "noisy_tool", "id", &storage, &config, None);

        assert_eq!(replacement.chars().count(), huge.chars().count());
        assert_eq!(outcome, PersistOutcome::Untouched);
    }

    #[test]
    fn per_tool_threshold_zero_always_persists() {
        let tmp = TempDir::new().unwrap();
        let storage = make_storage(&tmp);
        let config = BudgetConfig::default().with_tool_threshold("trace_tool", 0);

        let tiny = "boom";
        let (replacement, outcome) =
            maybe_persist_tool_result(tiny, "trace_tool", "tinyid", &storage, &config, None);

        assert!(matches!(outcome, PersistOutcome::Persisted { .. }));
        assert!(replacement.contains(PERSISTED_OUTPUT_TAG));
        // Round-trip preserves the original bytes on disk.
        let path = storage.path_for("tinyid");
        assert_eq!(read_spill_file(&path).unwrap(), tiny);
    }

    #[test]
    fn preview_snaps_to_last_newline_when_past_halfway() {
        let mut content = String::new();
        for i in 0..100 {
            content.push_str(&format!("line {}\n", i));
        }
        let (preview, has_more) = generate_preview(&content, 50);
        assert!(has_more);
        assert!(preview.ends_with('\n'));
        // The "line 5\nline 6\n..." stream means a newline must exist
        // past max_chars / 2 = 25.
        assert!(preview.len() <= 50 + 8); // small slack for char vs byte
    }

    #[test]
    fn enforce_turn_budget_spills_largest_first() {
        let tmp = TempDir::new().unwrap();
        let storage = make_storage(&tmp);
        let config = BudgetConfig {
            turn_budget: 1_000,
            preview_size: 100,
            default_result_threshold: 10_000,
            per_tool_thresholds: HashMap::new(),
        };

        let mut messages = vec![
            ToolResultMessage {
                id: "small".into(),
                content: "a".repeat(100),
            },
            ToolResultMessage {
                id: "medium".into(),
                content: "b".repeat(400),
            },
            ToolResultMessage {
                id: "huge".into(),
                content: "c".repeat(2_000),
            },
        ];
        let total_before: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        assert!(total_before > config.turn_budget);

        let rewritten = enforce_turn_budget(&mut messages, &storage, &config);
        assert!(rewritten >= 1);

        // The "huge" message (sorted first) is the one that gets
        // spilled; "small" should be untouched.
        assert_eq!(messages[0].content, "a".repeat(100));
        assert!(messages[2].content.contains(PERSISTED_OUTPUT_TAG));

        // Total is now within budget.
        let total_after: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        assert!(
            total_after <= config.turn_budget + 1_000,
            "post-enforcement total {} should be near budget {}",
            total_after,
            config.turn_budget
        );
    }

    #[test]
    fn enforce_turn_budget_skips_already_persisted() {
        let tmp = TempDir::new().unwrap();
        let storage = make_storage(&tmp);
        let config = BudgetConfig {
            turn_budget: 500,
            preview_size: 100,
            default_result_threshold: 10_000,
            per_tool_thresholds: HashMap::new(),
        };

        // Pre-marked persisted block: must not be touched again even
        // though it pushes over budget on its own.
        let pre_persisted = format!(
            "{}\nFull output saved to: /tmp/somewhere\n{}",
            PERSISTED_OUTPUT_TAG, PERSISTED_OUTPUT_CLOSING_TAG
        );
        let mut messages = vec![
            ToolResultMessage {
                id: "already".into(),
                content: pre_persisted.clone(),
            },
            ToolResultMessage {
                id: "fresh".into(),
                content: "z".repeat(900),
            },
        ];

        enforce_turn_budget(&mut messages, &storage, &config);

        // The pre-persisted entry is byte-identical.
        assert_eq!(messages[0].content, pre_persisted);
        // The fresh oversize entry got rewritten.
        assert!(messages[1].content.contains(PERSISTED_OUTPUT_TAG));
    }

    #[test]
    fn pinned_threshold_returns_disabled_for_read() {
        // T3-3-4: `Read` (matches ReadTool::name) is pinned to
        // THRESHOLD_DISABLED to prevent the persist -> read -> persist loop.
        assert_eq!(pinned_threshold("Read"), Some(THRESHOLD_DISABLED));
        assert_eq!(pinned_threshold("read"), None); // lowercase is NOT pinned
        assert_eq!(pinned_threshold("other_tool"), None);
    }

    #[test]
    fn pinned_threshold_matches_read_tool_name() {
        // Wire-up guard: the pin must match the actual `Tool::name()`
        // string `ReadTool` returns. If `ReadTool::name()` ever changes,
        // this test will fail — flagging that the pin needs to track it.
        use crate::Tool;
        let read_tool = crate::read::ReadTool::new(None);
        assert_eq!(
            pinned_threshold(read_tool.name()),
            Some(THRESHOLD_DISABLED),
            "PINNED_THRESHOLDS must key on ReadTool::name() ({}) — got None",
            read_tool.name()
        );
    }

    #[test]
    fn resolve_threshold_honours_pinned_over_overrides() {
        // Even if a caller explicitly sets a finite per-tool threshold
        // for the pinned tool, the pinned invariant wins — the loop-
        // prevention contract must not be defeatable from config.
        let config = BudgetConfig::default().with_tool_threshold("Read", 100);
        assert_eq!(config.resolve_threshold("Read"), THRESHOLD_DISABLED);

        // Non-pinned tools still honour overrides as before.
        let config = BudgetConfig::default().with_tool_threshold("grep", 5_000);
        assert_eq!(config.resolve_threshold("grep"), 5_000);
        assert_eq!(
            config.resolve_threshold("unmapped"),
            DEFAULT_RESULT_THRESHOLD_CHARS
        );
    }

    #[test]
    fn pinned_read_skips_persistence_end_to_end() {
        // End-to-end: large output from `Read` must NOT be spilled,
        // because doing so would force the agent to issue another
        // `Read` to recover — an unbounded loop.
        let tmp = TempDir::new().unwrap();
        let storage = make_storage(&tmp);
        let config = BudgetConfig::default();
        let huge = "r".repeat(DEFAULT_RESULT_THRESHOLD_CHARS * 4);

        let (replacement, outcome) =
            maybe_persist_tool_result(&huge, "Read", "rid", &storage, &config, None);

        assert_eq!(outcome, PersistOutcome::Untouched);
        assert_eq!(replacement.chars().count(), huge.chars().count());
        // No spill file was written.
        assert!(!storage.path_for("rid").exists());
    }

    #[test]
    fn pinned_read_skips_persistence_even_with_threshold_override() {
        // Concern B guard: per-call `threshold_override` must NOT defeat
        // the pin. The docstring claims pinned wins unconditionally; this
        // test enforces that across the orthogonal override channel.
        let tmp = TempDir::new().unwrap();
        let storage = make_storage(&tmp);
        let config = BudgetConfig::default();
        let huge = "r".repeat(DEFAULT_RESULT_THRESHOLD_CHARS * 4);

        // Pass `Some(0)` — the most aggressive override (forces spill on
        // anything). Pinned `Read` MUST still skip persistence.
        let (replacement, outcome) =
            maybe_persist_tool_result(&huge, "Read", "rid", &storage, &config, Some(0));

        assert_eq!(outcome, PersistOutcome::Untouched);
        assert_eq!(replacement.chars().count(), huge.chars().count());
        assert!(!storage.path_for("rid").exists());
    }

    #[test]
    fn read_spill_file_missing_returns_error() {
        let tmp = TempDir::new().unwrap();
        let bogus = tmp.path().join("does_not_exist.txt");
        let err = read_spill_file(&bogus).unwrap_err();
        assert!(matches!(err, StorageError::Read { .. }));
    }

    #[test]
    fn sanitize_id_strips_path_traversal() {
        // Path separators and `..` are replaced; the spill file lands
        // inside the storage dir, not somewhere else.
        let tmp = TempDir::new().unwrap();
        let storage = make_storage(&tmp);
        let evil = "../../etc/passwd";
        let path = storage.path_for(evil);
        assert!(
            path.starts_with(&storage.0),
            "spill path {} must be inside storage dir {}",
            path.display(),
            storage.0.display()
        );
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn format_with_commas_handles_small_and_large() {
        assert_eq!(format_with_commas(0), "0");
        assert_eq!(format_with_commas(999), "999");
        assert_eq!(format_with_commas(1_234), "1,234");
        assert_eq!(format_with_commas(1_234_567), "1,234,567");
    }
}
