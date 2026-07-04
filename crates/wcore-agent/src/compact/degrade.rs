//! #636 — graceful context-overflow degradation (rung 1).
//!
//! Before the run loop aborts a turn with `FinishReason::Length` (the pre-flight
//! context-window guard in `engine.rs`), shed the largest tool-result outputs
//! from history so the assembled request drops back under the model's context
//! ceiling and the run can CONTINUE instead of dying.
//!
//! This is the mechanical rung of the degradation ladder — no LLM call. Full
//! content is spilled to disk (recoverable via the emitted `<persisted-output>`
//! path that names the file); only a bounded preview stays in-context.
//!
//! ## Why this rung is pairing-safe
//!
//! It rewrites `ToolResult` *content* in place and never adds or removes blocks,
//! so every `tool_use` keeps its matching `tool_result`. There is no orphaned-
//! `tool_use` 400 risk — the reason the riskier drop-oldest sliding window is
//! deferred to a later phase.
//!
//! ## Idempotent, terminating, no hot-loop
//!
//! `maybe_persist_tool_result` skips any block already carrying the
//! [`PERSISTED_OUTPUT_TAG`], so a spilled block is excluded from every later
//! pass. Each iteration therefore either drops under the ceiling and returns, or
//! tags exactly one more block — the sheddable set strictly shrinks, so the loop
//! always terminates. When every oversized result is already spilled and the
//! request is *still* over the ceiling (a conversation-heavy context, not
//! tool-heavy), the pass makes no progress and returns `0`, leaving the caller
//! to terminate cleanly with a session whose tool outputs are already shed (so a
//! resume heals rather than re-aborting turn 1).

use wcore_tools::tool_result_storage::{
    BUDGET_TOOL_NAME, BudgetConfig, PERSISTED_OUTPUT_TAG, StorageDir, maybe_persist_tool_result,
};
use wcore_types::message::{ContentBlock, Message};

/// Shed oversized tool-result outputs from `messages`, largest-first, until the
/// recomputed request size (`estimate`) drops below `ceiling` or no oversized,
/// not-already-spilled tool result remains.
///
/// * `min_shed_chars` — never spill a result smaller than this; spilling a
///   result already near the preview size reclaims little and only churns the
///   cached prefix. Callers MUST pass a value larger than the persisted-output
///   replacement (`preview_size` + a small header), both so every shed is a net
///   reduction AND so a spill-write failure — which replaces content with a
///   preview-sized `InlineTruncated` fallback that carries no
///   [`PERSISTED_OUTPUT_TAG`] — lands below the floor and is never re-selected
///   on a later pass (the only way an untagged block could be revisited).
/// * `estimate` — recomputes the FULL request token count (messages + system +
///   tool schemas) against the shed history after each spill; the loop stops as
///   soon as it is under `ceiling`. Called on a cold path (only when a turn is
///   about to abort), so the per-shed recompute is acceptable.
///
/// Returns the number of tool results rewritten. `0` means the pass could not
/// help — nothing oversized was left to shed.
pub fn shed_tool_outputs_until_under(
    messages: &mut [Message],
    storage: &StorageDir,
    config: &BudgetConfig,
    min_shed_chars: usize,
    ceiling: u64,
    estimate: impl Fn(&[Message]) -> u64,
) -> usize {
    if estimate(messages) < ceiling {
        return 0;
    }
    // Snapshot the sheddable tool-result blocks ONCE — oversized and not already
    // spilled — largest-first. Processing a fixed, finite list (each block at
    // most once) makes the pass provably terminating regardless of how
    // `maybe_persist_tool_result` rewrote the content (spill success adds the
    // tag; a spill-write failure falls back to a preview-sized inline truncation
    // that no longer meets `min_shed_chars`). Indices stay valid because we only
    // rewrite content strings in place — never resize any `content` vec or the
    // `messages` slice. This mirrors `enforce_turn_budget`'s largest-first loop.
    let mut candidates: Vec<(usize, usize, usize)> = Vec::new(); // (msg_idx, block_idx, chars)
    for (mi, msg) in messages.iter().enumerate() {
        for (bi, block) in msg.content.iter().enumerate() {
            if let ContentBlock::ToolResult { content, .. } = block {
                let chars = content.chars().count();
                if chars > min_shed_chars && !content.contains(PERSISTED_OUTPUT_TAG) {
                    candidates.push((mi, bi, chars));
                }
            }
        }
    }
    candidates.sort_by_key(|&(_, _, chars)| std::cmp::Reverse(chars));

    let mut shed = 0usize;
    for (mi, bi, _) in candidates {
        // Stop as soon as the recomputed request drops under the ceiling.
        if estimate(messages) < ceiling {
            break;
        }
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } = &mut messages[mi].content[bi]
        {
            // `BUDGET_TOOL_NAME` is not pinned, so `Some(0)` forces the spill.
            let (replacement, _outcome) = maybe_persist_tool_result(
                content,
                BUDGET_TOOL_NAME,
                tool_use_id.as_str(),
                storage,
                config,
                Some(0),
            );
            *content = replacement;
            shed += 1;
        }
    }
    shed
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use wcore_types::message::{ContentBlock, Message, Role};

    fn tool_result(id: &str, content: &str) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: false,
            }],
        )
    }

    fn tool_use(id: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "read".to_string(),
                input: serde_json::json!({}),
                extra: None,
            }],
        )
    }

    /// Chars-based fake estimator: sum of tool-result content lengths. Lets the
    /// tests drive the ceiling deterministically without the real tokenizer.
    fn chars_estimator(messages: &[Message]) -> u64 {
        let mut total = 0u64;
        for m in messages {
            for b in &m.content {
                if let ContentBlock::ToolResult { content, .. } = b {
                    total += content.chars().count() as u64;
                }
            }
        }
        total
    }

    fn test_storage() -> (TempDir, StorageDir) {
        let dir = TempDir::new().expect("tempdir");
        let storage = StorageDir(dir.path().join("results"));
        (dir, storage)
    }

    #[test]
    fn sheds_largest_first_and_stops_once_under_ceiling() {
        let (_dir, storage) = test_storage();
        let config = BudgetConfig::default();
        let mut messages = vec![
            tool_use("a"),
            tool_result("a", &"x".repeat(50_000)), // biggest
            tool_use("b"),
            tool_result("b", &"y".repeat(30_000)),
            tool_use("c"),
            tool_result("c", &"z".repeat(1_000)), // below min_shed, untouched
        ];
        // Ceiling below the initial 81k but above what shedding the single
        // biggest result leaves, so exactly ONE shed should suffice.
        let ceiling = 40_000u64;
        let shed = shed_tool_outputs_until_under(
            &mut messages,
            &storage,
            &config,
            8_000,
            ceiling,
            chars_estimator,
        );
        assert_eq!(shed, 1, "shedding the single biggest result should suffice");
        assert!(
            chars_estimator(&messages) < ceiling,
            "estimate must be under the ceiling after shedding"
        );
        // The biggest result was spilled (now a persisted-output preview)...
        let a = &messages[1].content[0];
        let ContentBlock::ToolResult { content, .. } = a else {
            panic!("expected tool result");
        };
        assert!(
            content.contains(PERSISTED_OUTPUT_TAG),
            "biggest result should be spilled"
        );
        // ...the second-biggest was left alone (one shed was enough).
        let b = &messages[3].content[0];
        let ContentBlock::ToolResult { content, .. } = b else {
            panic!("expected tool result");
        };
        assert!(
            !content.contains(PERSISTED_OUTPUT_TAG),
            "second result should be untouched once under ceiling"
        );
    }

    #[test]
    fn never_sheds_results_below_min_size() {
        let (_dir, storage) = test_storage();
        let config = BudgetConfig::default();
        let mut messages = vec![
            tool_use("a"),
            tool_result("a", &"x".repeat(5_000)),
            tool_use("b"),
            tool_result("b", &"y".repeat(5_000)),
        ];
        // Ceiling is unreachable by shedding because both results are under the
        // min_shed floor — the pass must make no progress and return 0.
        let shed = shed_tool_outputs_until_under(
            &mut messages,
            &storage,
            &config,
            8_000,
            1_000,
            chars_estimator,
        );
        assert_eq!(shed, 0, "results below min_shed must never be spilled");
    }

    #[test]
    fn idempotent_no_progress_when_all_oversized_already_spilled() {
        let (_dir, storage) = test_storage();
        let config = BudgetConfig::default();
        let mut messages = vec![tool_use("a"), tool_result("a", &"x".repeat(50_000))];
        // First pass spills the block.
        let first = shed_tool_outputs_until_under(
            &mut messages,
            &storage,
            &config,
            8_000,
            1, // impossibly low ceiling → shed everything sheddable
            chars_estimator,
        );
        assert_eq!(first, 1);
        // Second pass on the already-spilled block must make no progress (no
        // hot-loop, no re-spill), even with the same impossible ceiling.
        let second = shed_tool_outputs_until_under(
            &mut messages,
            &storage,
            &config,
            8_000,
            1,
            chars_estimator,
        );
        assert_eq!(second, 0, "already-spilled block must not be re-shed");
    }

    #[test]
    fn preserves_tool_use_result_pairing() {
        let (_dir, storage) = test_storage();
        let config = BudgetConfig::default();
        let mut messages = vec![tool_use("a"), tool_result("a", &"x".repeat(50_000))];
        let before_blocks: usize = messages.iter().map(|m| m.content.len()).sum();
        shed_tool_outputs_until_under(&mut messages, &storage, &config, 8_000, 1, chars_estimator);
        let after_blocks: usize = messages.iter().map(|m| m.content.len()).sum();
        assert_eq!(
            before_blocks, after_blocks,
            "shedding must not add or remove blocks (no orphaned tool_use)"
        );
        // The tool_use and its id-matched tool_result both still present.
        assert!(matches!(
            &messages[0].content[0],
            ContentBlock::ToolUse { id, .. } if id == "a"
        ));
        assert!(matches!(
            &messages[1].content[0],
            ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "a"
        ));
    }

    #[test]
    fn returns_zero_when_already_under_ceiling() {
        let (_dir, storage) = test_storage();
        let config = BudgetConfig::default();
        let mut messages = vec![tool_use("a"), tool_result("a", &"x".repeat(1_000))];
        let shed = shed_tool_outputs_until_under(
            &mut messages,
            &storage,
            &config,
            8_000,
            10_000,
            chars_estimator,
        );
        assert_eq!(shed, 0, "no work when the request is already under ceiling");
    }
}
