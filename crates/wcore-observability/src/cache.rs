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
/// directly on the system block and the last tool entry). This helper adds the
/// third marker: the **last message in `req.messages`** — typically the latest
/// user turn or tool-result turn. Combined with the existing two markers, every
/// turn after the first benefits from a long cacheable prefix.
///
/// No-op when `compat.cache_message_breakpoints()` returns false.
pub fn mark_cache_boundaries(req: &mut LlmRequest, compat: &ProviderCompat) {
    if !compat.cache_message_breakpoints() {
        return;
    }
    // Clear any breakpoint set by a previous call so we don't accumulate.
    for msg in &mut req.messages {
        msg.cache_breakpoint = None;
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
        }
    }

    #[test]
    fn marks_last_message_when_compat_enables_breakpoints() {
        let mut req = request_with(vec![user_msg("first"), user_msg("second")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat);

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

        mark_cache_boundaries(&mut req, &compat);

        assert!(
            req.messages[0].cache_breakpoint.is_none(),
            "openai compat must not place any breakpoint"
        );
    }

    #[test]
    fn idempotent_repeated_invocation_keeps_at_most_one_marker() {
        let mut req = request_with(vec![user_msg("a"), user_msg("b"), user_msg("c")]);
        let compat = ProviderCompat::anthropic_defaults();

        mark_cache_boundaries(&mut req, &compat);
        mark_cache_boundaries(&mut req, &compat);
        mark_cache_boundaries(&mut req, &compat);

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

        mark_cache_boundaries(&mut req, &compat);
        assert!(req.messages[0].cache_breakpoint.is_some());

        req.messages.push(user_msg("turn2"));
        mark_cache_boundaries(&mut req, &compat);

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
        mark_cache_boundaries(&mut req, &compat);
        // No panic; nothing to mark.
        assert!(req.messages.is_empty());
    }
}
