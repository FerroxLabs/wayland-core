//! Static per-model output-token ceilings.
//!
//! The engine sizes each request's `max_tokens` up front (Layer 1) so a normal
//! turn finishes in ONE round instead of relying on the truncation
//! auto-continue loop for routine output. To clamp safely we need each model's
//! real **output** ceiling (distinct from its context window) — sending more
//! than the model allows is a hard 400.
//!
//! This table is the *load-bearing* source for that number: live `/models`
//! discovery rarely returns a per-model output cap (most endpoints omit it), so
//! a small, conservative, version-aware static table is the floor. When a model
//! is not in the table (older variant, unknown router alias like `flux-auto`)
//! the lookup returns `None` and the caller **fails open** — it sends the
//! configured value and lets the continuation loop net any truncation. Erring
//! toward `None`/low is safe (an undersize just costs a continuation round); a
//! too-high entry would 400, so every entry here is at or below the model's
//! documented output ceiling.
//!
//! Matching is on **versioned** id fragments on purpose: `claude-3-opus` caps
//! output at 4096 while `claude-opus-4-x` allows 32000, so a bare `"opus"`
//! match would 400 the old model. Only id shapes we are confident about are
//! listed; everything else is `None`.

/// Returns `(max_output_tokens, context_window)` for a known model, or `None`
/// when the model is unknown (caller must fail open).
///
/// `provider` is accepted for future provider-scoped disambiguation; today the
/// model id is distinctive enough to match on alone.
pub fn model_output_ceiling(_provider: &str, model: &str) -> Option<(u32, u32)> {
    let m = model.to_ascii_lowercase();

    // --- Anthropic Claude (4.x era; older 3.x deliberately excluded) ---
    if m.contains("opus-4") {
        return Some((32_000, 200_000));
    }
    if m.contains("sonnet-4") {
        return Some((64_000, 200_000));
    }
    if m.contains("haiku-4") {
        // Conservative: 4.5 may allow more, but undersizing is safe.
        return Some((8_192, 200_000));
    }

    // --- OpenAI ---
    // gpt-4.1 family allows 32768 output; check BEFORE the gpt-4o catch so
    // "gpt-4.1" never falls through to the 4o branch.
    if m.contains("gpt-4.1") {
        return Some((32_768, 1_000_000));
    }
    if m.contains("gpt-4o") {
        return Some((16_384, 128_000));
    }

    // --- xAI Grok 3.x ---
    if m.contains("grok-3") {
        return Some((64_000, 131_072));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_modern_models_return_their_real_output_ceiling() {
        assert_eq!(
            model_output_ceiling("anthropic", "claude-opus-4-7"),
            Some((32_000, 200_000))
        );
        assert_eq!(
            model_output_ceiling("anthropic", "claude-sonnet-4-6"),
            Some((64_000, 200_000))
        );
        assert_eq!(
            model_output_ceiling("openai", "gpt-4o-mini"),
            Some((16_384, 128_000))
        );
        assert_eq!(
            model_output_ceiling("openai", "gpt-4.1"),
            Some((32_768, 1_000_000))
        );
    }

    #[test]
    fn gpt_4_1_does_not_fall_through_to_4o() {
        // "gpt-4.1" must NOT match the gpt-4o branch (substring ordering bug
        // would clamp 4.1 to 16384 and undersize it).
        assert_eq!(
            model_output_ceiling("openai", "gpt-4.1-mini"),
            Some((32_768, 1_000_000))
        );
    }

    #[test]
    fn older_claude_3_is_not_matched_so_it_fails_open() {
        // claude-3-opus caps output at 4096; a bare "opus" match would 400 it.
        // It must return None (fail open), NOT the 4.x ceiling.
        assert_eq!(model_output_ceiling("anthropic", "claude-3-opus"), None);
        assert_eq!(model_output_ceiling("anthropic", "claude-3-5-sonnet"), None);
    }

    #[test]
    fn unknown_and_router_aliases_return_none() {
        assert_eq!(model_output_ceiling("flux-router", "flux-auto"), None);
        assert_eq!(model_output_ceiling("flux-router", "flux-standard"), None);
        assert_eq!(model_output_ceiling("openai", "some-future-model"), None);
        assert_eq!(model_output_ceiling("ollama", "llama3.1"), None);
    }
}
