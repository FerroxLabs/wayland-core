//! Bounded verbatim recent tail — the messages a fold must NOT summarise.
//!
//! Before this, Core kept exactly one thing byte-identical across a
//! compaction boundary: the live user turn (AUDIT A4/A7). Everything else,
//! including the tool call you issued thirty seconds ago and its result,
//! became prose. The recent past is where the execution frontier lives, and
//! prose about a command is not the command's output.
//!
//! Three independent implementations converged on roughly the same number
//! before this one: Codex, Kimi and openclaw each keep on the order of 20,000
//! tokens of recent history verbatim across a compaction. None of them
//! publishes a measurement behind it; what they agree on is the ORDER of
//! magnitude, which is why `compact.keep_recent_tokens` defaults to 20,000 and
//! why that number lives in `wcore-config` as an operator-settable field
//! rather than as a constant buried in this file.
//!
//! Two hard structural rules, both cheap:
//!
//! 1. **Never split a tool call from its result.** The cut point is always an
//!    Assistant message. A `tool_result` only ever appears after the
//!    Assistant `tool_use` that provoked it, so cutting at an Assistant
//!    boundary means every result inside the tail has its call inside the
//!    tail too. (Kimi's `canSplitAfter` predicate — `strategy.ts:232-256` —
//!    encodes the same rule the long way round.)
//! 2. **Say that the messages are not adjacent.** The summary and the first
//!    verbatim message are separated by an unknown amount of destroyed
//!    history; a model that thinks they are adjacent will mis-read the
//!    ordering. [`TAIL_NOTICE`] is spliced into the summary message to say so.
//!
//! This is emphatically NOT Codex's `should_keep_compacted_history_item`
//! filter (`compact_remote.rs:341-370`), which retains a message only if it is
//! neither a `FunctionCall`, a `FunctionCallOutput`, a `Reasoning` record nor
//! a `ToolSearch` record — i.e. it drops exactly the frontier. That filter is
//! the single most-reported compaction failure in their tracker. The tail here
//! is a contiguous, type-blind suffix: whatever the last N tokens were, they
//! survive unaltered.

use wcore_types::message::{Message, Role};

use super::estimate;

/// Splice marker telling the model the summary and the tail are not adjacent.
pub const TAIL_NOTICE: &str = "\n\n[Older history above was summarised. The messages that follow \
                               this one are the most recent part of the conversation, kept \
                               VERBATIM and unmodified — they are not adjacent to the summary.]";

/// Where the verbatim tail begins in `messages`, or `None` for "summarise
/// everything" (the pre-existing behaviour).
///
/// `budget_tokens` is the operator's `compact.keep_recent_tokens`;
/// `hard_cap_tokens` is the degradation guard described below. A `None` result
/// is always safe: it is exactly what the engine did before the tail existed.
///
/// ### Degradation
///
/// A tail is worthless if it re-triggers compaction on the next turn, and
/// actively harmful if it alone exceeds the window. `hard_cap_tokens` — the
/// engine passes half the autocompact trigger threshold — bounds the tail so
/// the post-fold buffer lands at most halfway back to the watermark no matter
/// how large `keep_recent_tokens` is set. On a small window that shrinks the
/// tail; at the limit it disappears entirely and the fold behaves exactly as
/// it did before.
///
/// ### The prefix must survive
///
/// At least one message must remain OUTSIDE the tail — otherwise the
/// summariser is handed nothing, "compaction" collapses zero messages, and the
/// buffer is unchanged while a provider round-trip was spent. When no
/// Assistant cut point leaves a non-empty prefix, this returns `None`.
pub fn tail_start(
    messages: &[Message],
    budget_tokens: usize,
    hard_cap_tokens: usize,
) -> Option<usize> {
    let budget = budget_tokens.min(hard_cap_tokens) as u64;
    if budget == 0 || messages.len() < 2 {
        return None;
    }

    // Walk backwards accumulating cost; `earliest` is the earliest index that
    // still fits. Costed one message at a time (rather than re-estimating a
    // growing slice) so this stays linear.
    let mut used: u64 = 0;
    let mut earliest = messages.len();
    for idx in (0..messages.len()).rev() {
        let cost = estimate::estimate_tokens_from_messages(&messages[idx..=idx]);
        if used + cost > budget {
            break;
        }
        used += cost;
        earliest = idx;
    }
    if earliest >= messages.len() {
        // Not even the last message fits the budget.
        return None;
    }

    // Snap FORWARD to an Assistant message: the cut must never land between a
    // tool_use and its tool_result, and it must never make the summary
    // message adjacent to another User message. Forward, not backward,
    // because moving the cut backwards would grow the tail past its budget.
    let cut = (earliest..messages.len()).find(|&i| matches!(messages[i].role, Role::Assistant))?;

    // The prefix must be non-empty, or there is nothing to summarise.
    if cut == 0 { None } else { Some(cut) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_types::message::ContentBlock;

    fn msg(role: Role, text: &str) -> Message {
        Message::new(role, vec![ContentBlock::Text { text: text.into() }])
    }

    fn tool_use(id: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.into(),
                name: "Bash".into(),
                input: serde_json::json!({"command": "echo hi"}),
                extra: None,
            }],
        )
    }

    fn tool_result(id: &str, body: &str) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content: body.into(),
                is_error: false,
            }],
        )
    }

    /// A conversation of `pairs` tool round-trips, each result padded to
    /// roughly `pad` characters so the token budget is exercised for real.
    fn conversation(pairs: usize, pad: usize) -> Vec<Message> {
        let mut v = vec![msg(Role::User, "start")];
        for i in 0..pairs {
            v.push(tool_use(&format!("t{i}")));
            v.push(tool_result(&format!("t{i}"), &"z".repeat(pad)));
        }
        v
    }

    #[test]
    fn zero_budget_keeps_the_pre_tail_behaviour() {
        assert_eq!(tail_start(&conversation(5, 100), 0, 100_000), None);
    }

    #[test]
    fn the_cut_is_always_an_assistant_message() {
        let msgs = conversation(8, 400);
        let cut = tail_start(&msgs, 2_000, 100_000).expect("a tail should fit");
        assert!(
            matches!(msgs[cut].role, Role::Assistant),
            "cut landed on {:?} at {cut}",
            msgs[cut].role
        );
    }

    #[test]
    fn no_tool_result_in_the_tail_is_orphaned_from_its_call() {
        let msgs = conversation(8, 400);
        let cut = tail_start(&msgs, 2_000, 100_000).unwrap();
        let calls: Vec<&str> = msgs[cut..]
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        for block in msgs[cut..].iter().flat_map(|m| &m.content) {
            if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                assert!(
                    calls.contains(&tool_use_id.as_str()),
                    "tool_result {tool_use_id} has no call inside the tail (cut={cut})"
                );
            }
        }
    }

    #[test]
    fn an_unbounded_budget_still_leaves_the_summariser_something_to_summarise() {
        // A budget far larger than the whole conversation must not put the cut
        // at 0: that would collapse zero messages and spend a provider
        // round-trip changing nothing.
        let cut = tail_start(&conversation(3, 10), 1_000_000, 1_000_000).unwrap();
        assert!(cut >= 1, "cut={cut} leaves an empty prefix");
    }

    #[test]
    fn a_conversation_opening_on_the_assistant_refuses_rather_than_empty_the_prefix() {
        // The only legal cut point here is index 0, and cutting there would
        // hand the summariser nothing at all.
        let msgs = vec![
            tool_use("t0"),
            tool_result("t0", "small"),
            msg(Role::User, "next"),
        ];
        assert_eq!(tail_start(&msgs, 1_000_000, 1_000_000), None);
    }

    #[test]
    fn a_bigger_budget_never_produces_a_later_cut() {
        let msgs = conversation(10, 400);
        let small = tail_start(&msgs, 2_000, 100_000).unwrap();
        let large = tail_start(&msgs, 8_000, 100_000).unwrap();
        assert!(large <= small, "small={small} large={large}");
    }

    #[test]
    fn the_hard_cap_binds_over_the_operator_budget() {
        let msgs = conversation(10, 400);
        let uncapped = tail_start(&msgs, 20_000, 100_000).unwrap();
        let capped = tail_start(&msgs, 20_000, 1_000).unwrap();
        assert!(
            capped > uncapped,
            "the cap must shrink the tail: capped={capped} uncapped={uncapped}"
        );
    }

    #[test]
    fn a_tail_that_cannot_fit_even_one_message_is_refused() {
        let msgs = conversation(2, 40_000);
        assert_eq!(tail_start(&msgs, 100, 100), None);
    }

    #[test]
    fn the_tail_never_exceeds_its_budget() {
        let msgs = conversation(12, 500);
        let budget = 3_000;
        let cut = tail_start(&msgs, budget, 100_000).unwrap();
        let cost = estimate::estimate_tokens_from_messages(&msgs[cut..]);
        assert!(cost <= budget as u64, "tail cost {cost} > budget {budget}");
    }
}
