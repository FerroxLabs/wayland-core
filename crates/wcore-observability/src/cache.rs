//! Prompt-cache discipline (S3).
//!
//! Places `MessageCacheHint::Breakpoint` on the tail of `LlmRequest.messages`
//! when the active provider honours explicit breakpoints (per
//! `ProviderCompat.cache_message_breakpoints()`). Providers translate the hint
//! into provider-native markers in their `build_messages()` step.
//!
//! Idempotent: calling `mark_cache_boundaries` repeatedly on the same request
//! leaves at most one breakpoint at the tail. Safe to call before every API
//! call from the agent loop.

use wcore_config::compat::ProviderCompat;
use wcore_types::llm::LlmRequest;
use wcore_types::message::MessageCacheHint;

/// Mark cache boundaries on a request just before it is sent to the provider.
///
/// **System prompt + tools markers** are still emitted by individual provider
/// `build_request_body()` functions (Anthropic-family puts `cache_control`
/// directly on the system block and the last tool entry). This helper adds
/// up to two more:
///
/// - the **last message in `req.messages`** — typically the latest user turn
///   or tool-result turn; the moving cache-write point;
/// - the **permanent anchor** (`anchor_index`, when `Some` and not the tail)
///   — an immutable already-compacted message picked by
///   `wcore-agent::compact::micro::cache_anchor_index`. Continuous
///   args-compaction transitions one message verbatim→stub inside the
///   previously cached prefix each turn; the anchor breakpoint keeps the
///   long prefix up to the anchor cache-valid across those transitions.
///
/// The provider-side budget (`apply_cache_zones`) counts these hints before
/// spending its own markers, so system + tools + anchor + tail come out at
/// exactly Anthropic's 4-marker limit (the moving previous-boundary marker
/// yields its slot to the anchor).
///
/// No-op when `compat.cache_message_breakpoints()` returns false.
pub fn mark_cache_boundaries(
    req: &mut LlmRequest,
    compat: &ProviderCompat,
    anchor_index: Option<usize>,
) {
    if !compat.cache_message_breakpoints() {
        return;
    }
    // Clear any breakpoint set by a previous call so we don't accumulate.
    for msg in &mut req.messages {
        msg.cache_breakpoint = None;
    }
    // Permanent anchor first; skipped when it would collide with the tail
    // (the tail marker below covers that message already).
    if let Some(idx) = anchor_index
        && idx + 1 < req.messages.len()
    {
        req.messages[idx].cache_breakpoint = Some(MessageCacheHint::Breakpoint);
    }
    if let Some(last) = req.messages.last_mut() {
        last.cache_breakpoint = Some(MessageCacheHint::Breakpoint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_types::message::{ContentBlock, Message, Role};

    fn user_msg(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::Text { text: text.into() }])
    }

    fn request_with(messages: Vec<Message>) -> LlmRequest {
        LlmRequest {
            model: "m".into(),
            system: "s".into(),
            messages,
            tools: vec![],
            max_tokens: 1024,
            thinking: None,
            reasoning_effort: None,
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
            omit_max_tokens: false,
        }
    }

    #[test]
    fn marks_last_message_when_compat_enables_breakpoints() {
        let mut req = request_with(vec![user_msg("first"), user_msg("second")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, None);

        assert!(req.messages[0].cache_breakpoint.is_none());
        assert_eq!(
            req.messages[1].cache_breakpoint,
            Some(MessageCacheHint::Breakpoint),
            "tail message must get the breakpoint when compat allows it"
        );
    }

    #[test]
    fn does_not_mark_when_compat_disables_breakpoints() {
        let mut req = request_with(vec![user_msg("first")]);
        let compat = ProviderCompat::openai_defaults();

        mark_cache_boundaries(&mut req, &compat, None);

        assert!(
            req.messages[0].cache_breakpoint.is_none(),
            "openai compat must not place any breakpoint"
        );
    }

    #[test]
    fn idempotent_repeated_invocation_keeps_at_most_one_marker() {
        let mut req = request_with(vec![user_msg("a"), user_msg("b"), user_msg("c")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, None);
        mark_cache_boundaries(&mut req, &compat, None);
        mark_cache_boundaries(&mut req, &compat, None);

        let count = req
            .messages
            .iter()
            .filter(|m| m.cache_breakpoint.is_some())
            .count();
        assert_eq!(count, 1, "exactly one breakpoint expected after 3 calls");
        assert!(req.messages.last().unwrap().cache_breakpoint.is_some());
    }

    #[test]
    fn marker_moves_forward_when_new_messages_appended() {
        let mut req = request_with(vec![user_msg("turn1")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, None);
        assert!(req.messages[0].cache_breakpoint.is_some());

        req.messages.push(user_msg("turn2"));
        mark_cache_boundaries(&mut req, &compat, None);

        assert!(
            req.messages[0].cache_breakpoint.is_none(),
            "turn1 marker must be cleared when turn2 arrives"
        );
        assert!(
            req.messages[1].cache_breakpoint.is_some(),
            "turn2 must hold the new breakpoint"
        );
    }

    #[test]
    fn no_panic_on_empty_messages() {
        let mut req = request_with(vec![]);
        let compat = ProviderCompat::anthropic_defaults();
        mark_cache_boundaries(&mut req, &compat, None);
        // No panic; nothing to mark.
        assert!(req.messages.is_empty());
    }

    // --- Permanent anchor (gap-1 + gap-2 coupling) ---------------------------

    #[test]
    fn anchor_and_tail_both_marked() {
        let mut req = request_with(vec![
            user_msg("a"),
            user_msg("b"),
            user_msg("c"),
            user_msg("d"),
        ]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, Some(1));

        let marked: Vec<usize> = req
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.cache_breakpoint.is_some())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            marked,
            vec![1, 3],
            "anchor (1) and tail (3) must both carry the breakpoint hint"
        );
    }

    #[test]
    fn anchor_colliding_with_tail_yields_single_marker() {
        let mut req = request_with(vec![user_msg("a"), user_msg("b")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, Some(1));

        let count = req
            .messages
            .iter()
            .filter(|m| m.cache_breakpoint.is_some())
            .count();
        assert_eq!(count, 1, "anchor == tail must not double-mark");
        assert!(req.messages[1].cache_breakpoint.is_some());
    }

    #[test]
    fn out_of_range_anchor_is_ignored() {
        let mut req = request_with(vec![user_msg("a"), user_msg("b")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, Some(99));

        let count = req
            .messages
            .iter()
            .filter(|m| m.cache_breakpoint.is_some())
            .count();
        assert_eq!(count, 1, "an out-of-range anchor must mark only the tail");
    }

    #[test]
    fn anchor_respects_family_gating() {
        let mut req = request_with(vec![user_msg("a"), user_msg("b"), user_msg("c")]);
        let compat = ProviderCompat::openai_defaults();

        mark_cache_boundaries(&mut req, &compat, Some(0));

        assert!(
            req.messages.iter().all(|m| m.cache_breakpoint.is_none()),
            "openai compat must suppress the anchor hint too"
        );
    }
}
