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
/// `transient_tail` says the LAST message carries per-turn transient content
/// (the skill-router hint, a `PrePrompt` hook contribution) that is injected
/// into the per-turn CLONE of history and will not be there next turn. Such a
/// message is poison as a cache WRITE point — the entry written at it can
/// never be read again — so it is stamped [`MessageCacheHint::Transient`]
/// instead and the write point moves back to the newest message that carries
/// no transient content (wayland#559 c6). With a single message and a
/// transient tail there is no stable message left, so the messages zone gets
/// no breakpoint at all and only the system + tools prefix is cached.
///
/// No-op when `compat.cache_message_breakpoints()` returns false — except that
/// the [`MessageCacheHint::Transient`] stamp is still applied, because it is a
/// PROHIBITION ("never write a cache entry here"), not a request for one, and
/// providers outside the breakpoint family still need to honour it.
pub fn mark_cache_boundaries(
    req: &mut LlmRequest,
    compat: &ProviderCompat,
    anchor_index: Option<usize>,
    transient_tail: bool,
) {
    // Clear any hint set by a previous call so we don't accumulate.
    for msg in &mut req.messages {
        msg.cache_breakpoint = None;
    }
    if transient_tail && let Some(last) = req.messages.last_mut() {
        last.cache_breakpoint = Some(MessageCacheHint::Transient);
    }
    if !compat.cache_message_breakpoints() {
        return;
    }
    // The newest message a cache entry may be written at. One short of the
    // tail when the tail is transient; `None` when that leaves nothing.
    let write_point = if transient_tail {
        req.messages.len().checked_sub(2)
    } else {
        req.messages.len().checked_sub(1)
    };
    let Some(write_point) = write_point else {
        return;
    };
    // Permanent anchor first; skipped when it would collide with the write
    // point (the marker below covers that message already).
    if let Some(idx) = anchor_index
        && idx < write_point
    {
        req.messages[idx].cache_breakpoint = Some(MessageCacheHint::Breakpoint);
    }
    req.messages[write_point].cache_breakpoint = Some(MessageCacheHint::Breakpoint);
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
            flux_loop_intent: None,
            flux_turn_nonce: None,
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
            routed_model_hint: None,
            replay_reasoning_content: false,
        }
    }

    #[test]
    fn marks_last_message_when_compat_enables_breakpoints() {
        let mut req = request_with(vec![user_msg("first"), user_msg("second")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, None, false);

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

        mark_cache_boundaries(&mut req, &compat, None, false);

        assert!(
            req.messages[0].cache_breakpoint.is_none(),
            "openai compat must not place any breakpoint"
        );
    }

    #[test]
    fn idempotent_repeated_invocation_keeps_at_most_one_marker() {
        let mut req = request_with(vec![user_msg("a"), user_msg("b"), user_msg("c")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, None, false);
        mark_cache_boundaries(&mut req, &compat, None, false);
        mark_cache_boundaries(&mut req, &compat, None, false);

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

        mark_cache_boundaries(&mut req, &compat, None, false);
        assert!(req.messages[0].cache_breakpoint.is_some());

        req.messages.push(user_msg("turn2"));
        mark_cache_boundaries(&mut req, &compat, None, false);

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
        mark_cache_boundaries(&mut req, &compat, None, false);
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

        mark_cache_boundaries(&mut req, &compat, Some(1), false);

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

        mark_cache_boundaries(&mut req, &compat, Some(1), false);

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

        mark_cache_boundaries(&mut req, &compat, Some(99), false);

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

        mark_cache_boundaries(&mut req, &compat, Some(0), false);

        assert!(
            req.messages.iter().all(|m| m.cache_breakpoint.is_none()),
            "openai compat must suppress the anchor hint too"
        );
    }
    // --- Transient tail (wayland#559 c6) -------------------------------------

    /// The turn-1 shape from the ticket: ONE message, and it carries the
    /// per-turn transient. No message may be a cache write point, because the
    /// only message there is will not look like this on turn 2.
    #[test]
    fn a_transient_turn_one_tail_is_never_a_cache_write_point() {
        let mut req = request_with(vec![user_msg("hint + the user's first words")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, None, true);

        assert_eq!(
            req.messages[0].cache_breakpoint,
            Some(MessageCacheHint::Transient),
            "the transient tail must be stamped as a prohibition"
        );
        assert!(
            req.messages
                .iter()
                .all(|m| m.cache_breakpoint != Some(MessageCacheHint::Breakpoint)),
            "no cache write point may be placed on turn 1 when the only message is transient"
        );
    }

    /// From turn 2 on there IS a stable message: the write point moves back
    /// one, so the cached prefix ends on bytes every later turn re-sends.
    #[test]
    fn the_write_point_moves_off_a_transient_tail() {
        let mut req = request_with(vec![user_msg("u1"), user_msg("a1"), user_msg("u2 + hint")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, None, true);

        assert_eq!(
            req.messages[1].cache_breakpoint,
            Some(MessageCacheHint::Breakpoint),
            "the newest NON-transient message must hold the write point"
        );
        assert_eq!(
            req.messages[2].cache_breakpoint,
            Some(MessageCacheHint::Transient),
            "the transient tail must not hold a breakpoint"
        );
    }

    /// Negative control: with no transient injected this turn, nothing moves.
    #[test]
    fn a_clean_tail_still_holds_the_write_point() {
        let mut req = request_with(vec![user_msg("u1"), user_msg("a1"), user_msg("u2")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, None, false);

        assert_eq!(
            req.messages[2].cache_breakpoint,
            Some(MessageCacheHint::Breakpoint),
            "without a transient the tail is still the write point"
        );
    }

    /// The anchor must not collide with the moved-back write point either.
    #[test]
    fn anchor_yields_to_a_moved_back_write_point() {
        let mut req = request_with(vec![user_msg("a"), user_msg("b"), user_msg("c")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat, Some(1), true);

        let breakpoints: Vec<usize> = req
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| m.cache_breakpoint == Some(MessageCacheHint::Breakpoint))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            breakpoints,
            vec![1],
            "anchor == write point must not double-mark, got {breakpoints:?}"
        );
    }

    /// The prohibition is not gated on the breakpoint family: OpenAI-shaped
    /// compat gets no breakpoint, but still learns which message is transient.
    #[test]
    fn transient_stamp_survives_a_non_breakpoint_provider() {
        let mut req = request_with(vec![user_msg("u1"), user_msg("u2 + hint")]);
        let compat = ProviderCompat::openai_defaults();

        mark_cache_boundaries(&mut req, &compat, None, true);

        assert_eq!(
            req.messages[1].cache_breakpoint,
            Some(MessageCacheHint::Transient)
        );
        assert!(
            req.messages
                .iter()
                .all(|m| m.cache_breakpoint != Some(MessageCacheHint::Breakpoint))
        );
    }
}
