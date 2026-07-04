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
use wcore_types::message::{ContentBlock, Message, Role};

/// Stable substring embedded in every [`truncate_head_tail`] marker. Rung 2's
/// pass 1 skips any block already containing it so a truncated block is never
/// re-truncated on a later pass — mirroring rung 1's [`PERSISTED_OUTPUT_TAG`]
/// skip. This keeps rung 2 idempotent: a resumed/re-degraded session re-emits
/// the identical prefix instead of churning the Anthropic prompt cache.
const TRUNC_MARKER: &str = "chars truncated";

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

/// #646 — graceful context-overflow degradation (rung 2): conversation-heavy
/// overflow that rung 1 cannot touch.
///
/// When the tool-output shed ([`shed_tool_outputs_until_under`]) still leaves
/// the request over the ceiling — because the bulk is in a plain `Text` (a
/// pasted log/file) or `Thinking` block, not a tool result — degrade the
/// non-tool content in two passes, mutating `messages` in place so a persisted/
/// resumed session heals rather than re-aborting turn 1:
///
/// 1. **Truncate** every oversized non-tool block (`Text`/`Thinking`) to a
///    head+tail preview with a `[N chars truncated]` marker. This alone rescues
///    the dominant real-world case (one big paste).
/// 2. **Drop-oldest** sliding window: if still over, remove the oldest
///    non-essential message until under the ceiling. Pairing-safe by
///    construction — it never removes the system prompt, never removes the most
///    recent turn (the last message), and never removes a message carrying a
///    `ToolUse`/`ToolResult`, so no `tool_use` is ever orphaned (no 400).
///
/// Returns `true` if anything was truncated or dropped.
pub fn degrade_conversation_overflow(
    messages: &mut Vec<Message>,
    ceiling: u64,
    per_block_budget_chars: usize,
    estimate: impl Fn(&[Message]) -> u64,
) -> bool {
    if estimate(messages) < ceiling {
        return false;
    }
    let mut changed = false;

    // Pass 1: truncate every oversized non-tool block. Done in one sweep (no
    // per-block `estimate` recompute, which would conflict with the `iter_mut`
    // borrow); the ceiling re-check happens after the sweep.
    for msg in messages.iter_mut() {
        for block in msg.content.iter_mut() {
            // Skip blocks that already carry the truncation marker: a truncated
            // block sits at ~budget chars and would otherwise re-qualify on every
            // later pass, re-truncating and churning the cached prefix.
            match block {
                ContentBlock::Text { text }
                    if text.chars().count() > per_block_budget_chars
                        && !text.contains(TRUNC_MARKER) =>
                {
                    let truncated = truncate_head_tail(text, per_block_budget_chars);
                    if truncated.chars().count() < text.chars().count() {
                        *text = truncated;
                        changed = true;
                    }
                }
                ContentBlock::Thinking { thinking }
                    if thinking.chars().count() > per_block_budget_chars
                        && !thinking.contains(TRUNC_MARKER) =>
                {
                    let truncated = truncate_head_tail(thinking, per_block_budget_chars);
                    if truncated.chars().count() < thinking.chars().count() {
                        *thinking = truncated;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }
    if estimate(messages) < ceiling {
        return changed;
    }

    // Pass 2: drop the oldest non-essential message until under the ceiling or
    // nothing safe is left to drop. Re-scan from the front each iteration so the
    // oldest droppable message goes first; each pass removes exactly one message
    // (finite → terminates).
    //
    // Pairing is safe by construction (a `ToolUse` lives on an assistant message
    // and its `ToolResult` on a separate user message; both are protected by the
    // `matches!` guard below, so neither half of a pair can be dropped). Dropping
    // the oldest turns can leave the remaining history assistant-leading; that is
    // repaired downstream by Anthropic's default-on `ensure_message_alternation`
    // (anthropic_shared.rs), which prepends a user filler. Residual edge: a
    // provider configured with `ensure_alternation:false` AND no surviving user
    // message could send an assistant-leading history — acceptable for v1 since
    // the last message here is always the current (user-role) turn.
    loop {
        if estimate(messages) < ceiling {
            break;
        }
        let last = messages.len().saturating_sub(1);
        let victim = messages.iter().enumerate().position(|(i, m)| {
            i != last
                && m.role != Role::System
                && !m.content.iter().any(|b| {
                    matches!(
                        b,
                        ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                    )
                })
        });
        match victim {
            Some(idx) => {
                messages.remove(idx);
                changed = true;
            }
            // Nothing left that is safe to drop — the caller's final ceiling
            // check will terminate the run cleanly.
            None => break,
        }
    }
    changed
}

/// Truncate `s` to a head+tail preview totalling about `budget` chars with a
/// `[N chars truncated]` marker between. Char-boundary safe.
///
/// Guarantees a NET REDUCTION: for a block only marginally over budget, the
/// head+tail+marker framing can be longer than the original, which would grow
/// the request instead of shrinking it. When the candidate is not strictly
/// shorter, return `s` unchanged so the caller's `changed`/`estimate` logic
/// never registers a phantom shrink.
fn truncate_head_tail(s: &str, budget: usize) -> String {
    let total = s.chars().count();
    if total <= budget {
        return s.to_string();
    }
    let half = budget / 2;
    let head: String = s.chars().take(half).collect();
    let tail: String = s.chars().skip(total - half).collect();
    let dropped = total - 2 * half;
    let candidate = format!("{head}\n\n... [{dropped} {TRUNC_MARKER}] ...\n\n{tail}");
    if candidate.chars().count() < total {
        candidate
    } else {
        s.to_string()
    }
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

    // ── #646 rung 2: conversation-heavy (non-tool) overflow ─────────────────

    fn text_msg(role: Role, content: &str) -> Message {
        Message::new(
            role,
            vec![ContentBlock::Text {
                text: content.to_string(),
            }],
        )
    }

    /// Chars-based estimator counting Text + Thinking + ToolResult content, so
    /// the rung-2 tests can drive the ceiling with plain-text bulk.
    fn all_content_estimator(messages: &[Message]) -> u64 {
        let mut total = 0u64;
        for m in messages {
            for b in &m.content {
                match b {
                    ContentBlock::Text { text } => total += text.chars().count() as u64,
                    ContentBlock::Thinking { thinking } => total += thinking.chars().count() as u64,
                    ContentBlock::ToolResult { content, .. } => {
                        total += content.chars().count() as u64
                    }
                    _ => {}
                }
            }
        }
        total
    }

    #[test]
    fn truncate_head_tail_keeps_head_and_tail() {
        let s = format!(
            "{}{}{}",
            "H".repeat(100),
            "M".repeat(1_000),
            "T".repeat(100)
        );
        let out = truncate_head_tail(&s, 40);
        assert!(out.starts_with("HHHH"), "keeps head: {out}");
        assert!(out.ends_with("TTTT"), "keeps tail: {out}");
        assert!(out.contains("chars truncated"), "has marker: {out}");
        assert!(
            out.chars().count() < s.chars().count(),
            "shorter than input"
        );
    }

    #[test]
    fn truncates_oversized_text_block_and_continues() {
        // Rung-2 test 1/2: the overflow bulk is a single big Text block (a paste),
        // not a tool result — truncation must bring it under the ceiling.
        let mut messages = vec![
            text_msg(Role::User, &"P".repeat(100_000)), // the big paste
            text_msg(Role::Assistant, "ok"),
        ];
        let ceiling = 40_000u64;
        // Per-block budget below the ceiling so the truncated block plus the
        // other content lands under it (this fake estimator counts chars 1:1;
        // the real engine's estimator is tokens = chars/4, so it passes the
        // ceiling itself as the char budget).
        let changed = degrade_conversation_overflow(
            &mut messages,
            ceiling,
            20_000, // per-block budget < ceiling
            all_content_estimator,
        );
        assert!(changed, "an oversized text block must be degraded");
        assert!(
            all_content_estimator(&messages) < ceiling,
            "truncation must bring the request under the ceiling"
        );
        // The block was truncated with a marker, not dropped.
        assert_eq!(messages.len(), 2, "truncation must not drop the message");
        let ContentBlock::Text { text } = &messages[0].content[0] else {
            panic!("expected text");
        };
        assert!(text.contains("chars truncated"), "head+tail marker: {text}");
    }

    #[test]
    fn drops_oldest_nonessential_and_preserves_system_and_latest() {
        // Many small text messages, none individually oversized — pass 1 can't
        // help, so drop-oldest must remove the oldest non-essential messages
        // while preserving the system prompt and the most recent turn.
        let mut messages = vec![
            text_msg(Role::System, &"S".repeat(1_000)),
            text_msg(Role::User, &"1".repeat(20_000)), // oldest droppable
            text_msg(Role::Assistant, &"2".repeat(20_000)),
            text_msg(Role::User, &"3".repeat(20_000)), // latest — preserved
        ];
        let ceiling = 35_000u64;
        let changed =
            degrade_conversation_overflow(&mut messages, ceiling, 100_000, all_content_estimator);
        assert!(changed, "drop-oldest must fire");
        assert!(all_content_estimator(&messages) < ceiling, "under ceiling");
        // System prompt survives.
        assert_eq!(messages[0].role, Role::System, "system preserved");
        // The most recent turn (the "3..." block) survives.
        let last = messages.last().unwrap();
        let ContentBlock::Text { text } = &last.content[0] else {
            panic!("expected text");
        };
        assert!(
            text.starts_with("333"),
            "latest turn preserved: got {}",
            &text[..3]
        );
    }

    #[test]
    fn dropoldest_never_orphans_tool_pairs() {
        // A tool_use/tool_result pair must never be split by drop-oldest, even
        // when it is the oldest content. Only the pure-text message is dropped.
        let mut messages = vec![
            tool_use("a"),
            tool_result("a", "small"),
            text_msg(Role::User, &"B".repeat(60_000)), // droppable bulk
            text_msg(Role::Assistant, "latest"),
        ];
        // Force the estimator to count everything; ceiling below the text bulk.
        let ceiling = 20_000u64;
        degrade_conversation_overflow(&mut messages, ceiling, 100_000, all_content_estimator);
        // The tool_use and its result both survive (pairing intact).
        assert!(
            messages
                .iter()
                .any(|m| matches!(&m.content[0], ContentBlock::ToolUse { id, .. } if id == "a")),
            "tool_use must survive drop-oldest"
        );
        assert!(
            messages.iter().any(|m| matches!(
                &m.content[0],
                ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "a"
            )),
            "paired tool_result must survive drop-oldest"
        );
    }

    #[test]
    fn returns_false_when_already_under_ceiling() {
        let mut messages = vec![text_msg(Role::User, "hi")];
        let changed =
            degrade_conversation_overflow(&mut messages, 10_000, 5_000, all_content_estimator);
        assert!(!changed, "no work when already under the ceiling");
    }

    #[test]
    fn truncate_head_tail_never_grows_marginal_block() {
        // A block only a few chars over budget: head+tail+marker framing would be
        // LONGER than the input. `truncate_head_tail` must return it unchanged so
        // truncation is always a net reduction, never a phantom "shrink" that
        // actually grows the request.
        let s = "A".repeat(50);
        let out = truncate_head_tail(&s, 48);
        assert_eq!(out, s, "a marginally-oversized block must not be grown");
    }

    #[test]
    fn truncation_pass_is_idempotent() {
        // Rung-2 pass 1 must be idempotent: a block that already carries the
        // truncation marker must NOT be re-truncated even when it is still over
        // the per-block budget, because a truncated block sits at ~budget chars
        // and would otherwise re-qualify on every later pass — churning the
        // Anthropic prompt cache on resume.
        //
        // Craft a single already-marked, still-oversized block as the sole
        // (latest, drop-oldest-protected) message so the call is forced past the
        // early ceiling guard into pass 1: pass 1 must skip it, and drop-oldest
        // has no eligible victim, so nothing changes.
        let marked = format!(
            "{}\n\n... [50000 {TRUNC_MARKER}] ...\n\n{}",
            "H".repeat(15_000),
            "T".repeat(15_000)
        );
        let mut messages = vec![text_msg(Role::User, &marked)];
        let snapshot = marked.clone();
        let ceiling = 10_000u64; // well under the ~30k marked block
        let changed = degrade_conversation_overflow(
            &mut messages,
            ceiling,
            20_000, // block (~30k) exceeds budget, but marker must guard it
            all_content_estimator,
        );
        let ContentBlock::Text { text } = &messages[0].content[0] else {
            panic!("expected text");
        };
        assert_eq!(
            *text, snapshot,
            "already-marked block must not be re-truncated"
        );
        assert!(!changed, "marker-guarded pass must report no change");
    }
}
