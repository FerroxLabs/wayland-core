//! Static per-model output-token ceilings.
//!
//! The engine sizes each request's `max_tokens` up front (Layer 1) so a normal
//! turn finishes in ONE round — there is NO truncation auto-continue loop, so
//! an undersized turn ends visibly at `finish_reason: length`. To clamp safely
//! we need each model's real **output** ceiling (distinct from its context
//! window) — sending more than the model allows is a hard 400.
//!
//! This table is the *load-bearing* source for that number: live `/models`
//! discovery rarely returns a per-model output cap (most endpoints omit it), so
//! a small, conservative, version-aware static table is the floor. When a model
//! is not in the table (older variant, unknown router alias like `flux-auto`)
//! the lookup returns `None` and the engine falls back to a conservative floor
//! (`size_output_cap`'s `UNKNOWN_CAP` 8192 / `UNKNOWN_REASONING_CAP` 32768) —
//! or, when the user omitted `--max-tokens` and the provider is omit-safe
//! (`ProviderCompat.omit_max_tokens_when_unsized`, #112), OMITS the wire field
//! entirely so the served model's natural ceiling applies. Erring toward
//! `None`/low is safe (an undersize truncates, which is user-visible but
//! recoverable); a too-high entry would 400, so every entry here is at or
//! below the model's documented output ceiling.
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

    // --- Anthropic Claude (4.x/5 era; older 3.x deliberately excluded) ---
    // The 1M-context generation (Opus 4.6/4.7/4.8, Sonnet 4.6, Sonnet 5, Fable 5)
    // serves the full 1,000,000-token window and 128k output BY DEFAULT — no
    // beta header, no long-context premium (verified against docs.anthropic.com,
    // 2026-07-04: "Opus 4.8 serves the full 1M context window by default with no
    // beta header"; the older `context-1m-2025-08-07` beta is retired). Earlier
    // 4.x (Opus 4.0/4.1/4.5, Sonnet 4.0/4.5, Haiku 4.5) stay at 200k. Cross-
    // checked against models.dev (2026-07-04). Match newest-first so a 4.8 id
    // never falls through to the 200k arm.
    if m.contains("opus-4-6") || m.contains("opus-4-7") || m.contains("opus-4-8") {
        return Some((128_000, 1_000_000));
    }
    if m.contains("opus-4-5") {
        return Some((64_000, 200_000));
    }
    if m.contains("opus-4") {
        // Opus 4.0 / 4.1 (and a bare opus-4): 200k window, 32k output.
        return Some((32_000, 200_000));
    }
    if m.contains("sonnet-5") {
        return Some((128_000, 1_000_000));
    }
    if m.contains("sonnet-4-6") {
        // Sonnet 4.6: 1M window, 128k output. Verified against Anthropic's model
        // overview + Codex/Gemini cross-audit; models.dev is stale here at 64k.
        return Some((128_000, 1_000_000));
    }
    if m.contains("sonnet-4") {
        // Sonnet 4.0 / 4.5: 200k window, 64k output.
        return Some((64_000, 200_000));
    }
    if m.contains("haiku-4") {
        // Haiku 4.5: 200k window, real output 64k (was undersized at 8_192).
        return Some((64_000, 200_000));
    }
    if m.contains("fable-5") {
        // Claude Fable 5: 1M window / 128k output (models.dev).
        return Some((128_000, 1_000_000));
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

    // --- OpenAI GPT-5 family ---
    // Fixes #165 (customer: a gpt-5.4 run died at 178,336 tokens against a fake
    // ~177k ceiling). With no entry every gpt-5.x id fell to the 200_000
    // CompactConfig default — a large-context model silently undersized
    // (premature compaction / ceiling death) — while the 128k-window
    // `-codex-spark` tier was simultaneously OVER-claimed by that same default.
    //
    // Windows verified against models.dev raw catalogue AND developers.openai.com
    // docs (2026-07-04); they agree. The family SPLITS by version, so match the
    // large-window tiers explicitly before the general 400k catch:
    //   * gpt-5.4 / gpt-5.4-pro / gpt-5.5 / gpt-5.5-pro → 1,050,000 window.
    //     (Their `-mini` / `-nano` / `-codex` variants stay at 400k — do NOT let
    //     a bare "gpt-5.4" substring claim 1.05M for gpt-5.4-mini, which is 400k
    //     and would 400 near the top.)
    //   * `-codex-spark` (gpt-5.3-codex-spark) → 128k window (BELOW the default,
    //     so this entry prevents an over-claim).
    //   * `-chat-latest` (gpt-5.1/5.2/5.3-chat-latest) → 128k window.
    //   * everything else in the family (gpt-5, 5.1, 5.2, 5.3, the *-codex,
    //     *-mini, *-nano, *-pro variants) → 400,000 window.
    // Output held at 128k (the family's documented cap; err low per the header —
    // gpt-5-pro documents 272k but 128k is safe). These ids route via the Codex
    // OAuth backend in wayland-core (`--provider openai-chatgpt`); OpenAI serves
    // the model's full window on that path (the 272k figure some tables cite is
    // a PRICING tier boundary — cost.tiers[].tier.size — not a context cap).
    if m.contains("gpt-5") {
        if m.contains("codex-spark") {
            return Some((32_000, 128_000));
        }
        // The VERSIONED chat-latest tiers (5.1/5.2/5.3) are small 128k-window
        // models. The BASE gpt-5-chat-latest is 400k (models.dev) — it must NOT
        // be caught here; it falls through to the 400k arm below (cross-audit
        // Defect 2).
        if m.contains("5.1-chat-latest")
            || m.contains("5.2-chat-latest")
            || m.contains("5.3-chat-latest")
        {
            return Some((16_384, 128_000));
        }
        if (m.contains("gpt-5.4") || m.contains("gpt-5.5"))
            && !m.contains("-mini")
            && !m.contains("-nano")
            && !m.contains("-codex")
        {
            return Some((128_000, 1_050_000));
        }
        return Some((128_000, 400_000));
    }

    // --- xAI Grok 3.x ---
    if m.contains("grok-3") {
        return Some((64_000, 131_072));
    }

    // --- Google Gemini 2.5 (text family) ---
    // #112: with no entry, every native Gemini model fell to the unknown-model
    // floor (8_192 output) despite a real 65_536 ceiling. Verified against
    // models.dev (2026-07-02): gemini-2.5-pro, gemini-2.5-flash, and
    // gemini-2.5-flash-lite all report output 65_536 / context 1_048_576. The
    // specialty variants have MUCH smaller limits (gemini-2.5-flash-image:
    // 32_768/32_768; the -preview-tts variants: 8_192 window; the
    // -native-audio / -live realtime variants: ~8k output) — an over-claim
    // would 400 them, so they are excluded and fail open to the unknown path.
    if (m.contains("gemini-2.5-pro") || m.contains("gemini-2.5-flash"))
        && !m.contains("-image")
        && !m.contains("-tts")
        && !m.contains("-native-audio")
        && !m.contains("-live")
    {
        return Some((65_536, 1_048_576));
    }

    // --- DeepSeek V4-Flash family (1,000,000-token context) ---
    // Fixes #255: with no entry, deepseek-v4-flash fell to the unknown-model
    // floor (8_192 output) and its 1M context window was never consulted.
    // Verified against api-docs.deepseek.com (2026-06-23): deepseek-v4-flash is
    // the canonical id; `deepseek-chat` / `deepseek-reasoner` are its (deprecated)
    // non-thinking / thinking aliases that map to the SAME model, so all three
    // share the 1,000,000 context window. Output ceiling is held at the
    // conservative 8_192 — the documented max is far higher, but this table errs
    // LOW on purpose (undersizing costs a continuation round; over-claiming 400s
    // — see the module header). Exact id checks (not a bare `deepseek` prefix)
    // so `deepseek-v4-pro` / a future `deepseek-v5` won't inherit these limits.
    if m.contains("deepseek-v4-flash") || m == "deepseek-chat" || m == "deepseek-reasoner" {
        return Some((8_192, 1_000_000));
    }

    // --- MiniMax M-series ---
    // #165 audit: the canonical MiniMax ids (MiniMax-M2 / M2.5 / M3) had no entry
    // and fell to the 200k default, undersizing M3's 1M window. Verified against
    // models.dev raw (2026-07-04): M3 = 1,000,000; the M2.x point releases
    // (M2.1 / M2.5 / M2.7) = 204,800; but the BASE MiniMax-M2 = 196,608 — a
    // distinct, SMALLER window (cross-audit Defect 1: claiming 204,800 for the
    // base M2 would 400 a request between 196,609 and 204,800). Match order is
    // longest-substring-first so a point release never falls through to the base
    // arm. Output held conservatively (err LOW per the header).
    if m.contains("minimax-m3") {
        return Some((128_000, 1_000_000));
    }
    if m.contains("minimax-m2.1") || m.contains("minimax-m2.5") || m.contains("minimax-m2.7") {
        return Some((128_000, 204_800));
    }
    if m.contains("minimax-m2") {
        // Base MiniMax-M2: 196,608 window (smaller than the point releases).
        return Some((128_000, 196_608));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_modern_models_return_their_real_output_ceiling() {
        // #165: Opus 4.6+ and Sonnet 4.6/5 serve 1M by default (no beta header).
        assert_eq!(
            model_output_ceiling("anthropic", "claude-opus-4-7"),
            Some((128_000, 1_000_000))
        );
        assert_eq!(
            model_output_ceiling("anthropic", "claude-sonnet-4-6"),
            Some((128_000, 1_000_000))
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
    fn claude_1m_generation_resolves_to_one_million_window() {
        // #165: the 1M-window generation (Opus 4.6/4.7/4.8, Sonnet 4.6, Sonnet 5,
        // Fable 5) serves 1M by default — verified vs docs.anthropic.com +
        // models.dev (2026-07-04). Our DEFAULT opus (claude-opus-4-8) was the
        // headline victim, stuck at the 200k default.
        for id in [
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-fable-5",
            "claude-sonnet-5",
        ] {
            assert_eq!(
                model_output_ceiling("anthropic", id),
                Some((128_000, 1_000_000)),
                "{id} must report the 1,000,000-token window / 128k output"
            );
        }
        // Sonnet 4.6 shares the 1M window and the generation's 128k output cap.
        assert_eq!(
            model_output_ceiling("anthropic", "claude-sonnet-4-6"),
            Some((128_000, 1_000_000))
        );
        // Case-insensitive (the lookup lowercases first).
        assert_eq!(
            model_output_ceiling("anthropic", "Claude-Opus-4-8"),
            Some((128_000, 1_000_000))
        );
    }

    #[test]
    fn older_claude_4x_stays_at_200k() {
        // The pre-4.6 generation is genuinely 200k — must NOT inherit the 1M
        // window (that would 400 near the top).
        assert_eq!(
            model_output_ceiling("anthropic", "claude-opus-4-5"),
            Some((64_000, 200_000))
        );
        assert_eq!(
            model_output_ceiling("anthropic", "claude-opus-4-1"),
            Some((32_000, 200_000))
        );
        assert_eq!(
            model_output_ceiling("anthropic", "claude-opus-4-20250514"),
            Some((32_000, 200_000))
        );
        assert_eq!(
            model_output_ceiling("anthropic", "claude-sonnet-4-5"),
            Some((64_000, 200_000))
        );
        // Haiku 4.5: 200k window, real output 64k (previously undersized 8_192).
        assert_eq!(
            model_output_ceiling("anthropic", "claude-haiku-4-5"),
            Some((64_000, 200_000))
        );
    }

    #[test]
    fn gpt5_family_resolves_to_real_windows() {
        // #165 core: verified vs models.dev raw + developers.openai.com
        // (2026-07-04). The family splits: full 5.4/5.5 = 1.05M; the rest = 400k.
        for id in ["gpt-5.4", "gpt-5.4-pro", "gpt-5.5", "gpt-5.5-pro"] {
            assert_eq!(
                model_output_ceiling("openai-chatgpt", id),
                Some((128_000, 1_050_000)),
                "{id} must report the 1,050,000-token window"
            );
        }
        for id in [
            "gpt-5",
            "gpt-5.1",
            "gpt-5.2",
            "gpt-5.3-codex",
            "gpt-5.4-codex",
            "gpt-5.4-mini",
            "gpt-5.4-nano",
            "gpt-5.5-mini",
            // Base gpt-5-chat-latest is 400k (only the 5.1/5.2/5.3 chat tiers
            // are 128k) — cross-audit Defect 2.
            "gpt-5-chat-latest",
        ] {
            assert_eq!(
                model_output_ceiling("openai-chatgpt", id),
                Some((128_000, 400_000)),
                "{id} must report the 400,000-token window"
            );
        }
        // The 128k-window tiers (below the 200k default → must be explicit to
        // avoid an over-claim).
        assert_eq!(
            model_output_ceiling("openai-chatgpt", "gpt-5.3-codex-spark"),
            Some((32_000, 128_000))
        );
        assert_eq!(
            model_output_ceiling("openai", "gpt-5.2-chat-latest"),
            Some((16_384, 128_000))
        );
        // Case-insensitive.
        assert_eq!(
            model_output_ceiling("openai-chatgpt", "GPT-5.5"),
            Some((128_000, 1_050_000))
        );
    }

    #[test]
    fn gpt5_large_window_does_not_leak_to_mini_nano_codex() {
        // A bare "gpt-5.4" substring must NOT hand the 1.05M window to the
        // 400k-window mini/nano/codex variants (that would 400 near the top).
        for id in ["gpt-5.4-mini", "gpt-5.4-nano", "gpt-5.4-codex"] {
            assert_eq!(
                model_output_ceiling("openai-chatgpt", id),
                Some((128_000, 400_000)),
                "{id} must stay at 400k, not inherit the full-5.4 1.05M window"
            );
        }
    }

    #[test]
    fn minimax_m_series_resolves_to_real_windows() {
        // #165: M3 is a 1M-context model; M2 / M2.5 are 204,800 (verified vs
        // MiniMax platform docs + models.dev, 2026-07-04).
        assert_eq!(
            model_output_ceiling("minimax", "MiniMax-M3"),
            Some((128_000, 1_000_000))
        );
        // The point releases are 204,800...
        for id in ["MiniMax-M2.5", "MiniMax-M2.1", "MiniMax-M2.7"] {
            assert_eq!(
                model_output_ceiling("minimax", id),
                Some((128_000, 204_800)),
                "{id} must report the 204,800-token window"
            );
        }
        // ...but the BASE M2 is a smaller 196,608 window (must NOT inherit the
        // point-release 204,800 — that would 400 near the top). Cross-audit
        // Defect 1.
        assert_eq!(
            model_output_ceiling("minimax", "MiniMax-M2"),
            Some((128_000, 196_608))
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

    #[test]
    fn deepseek_v4_flash_family_uses_1m_context_window() {
        // #255: the canonical id and both deprecated aliases share the 1M window.
        for id in ["deepseek-v4-flash", "deepseek-chat", "deepseek-reasoner"] {
            assert_eq!(
                model_output_ceiling("deepseek", id),
                Some((8_192, 1_000_000)),
                "{id} must report the 1,000,000-token context window"
            );
        }
        // Case-insensitive match (the lookup lowercases first).
        assert_eq!(
            model_output_ceiling("deepseek", "DeepSeek-V4-Flash"),
            Some((8_192, 1_000_000))
        );
    }

    #[test]
    fn gemini_2_5_text_family_returns_its_real_output_ceiling() {
        // #112: native Gemini text models resolve as KNOWN (65_536 output /
        // 1_048_576 window per models.dev) instead of the 8_192 unknown floor.
        for id in [
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "gemini-2.5-flash-lite",
        ] {
            assert_eq!(
                model_output_ceiling("gemini", id),
                Some((65_536, 1_048_576)),
                "{id} must report the Gemini 2.5 text-family limits"
            );
        }
        // Case-insensitive match (the lookup lowercases first).
        assert_eq!(
            model_output_ceiling("gemini", "Gemini-2.5-Pro"),
            Some((65_536, 1_048_576))
        );
    }

    #[test]
    fn gemini_2_5_specialty_variants_fail_open() {
        // The image/TTS variants have far smaller limits (flash-image is
        // 32_768/32_768, the -preview-tts variants an 8_192 window) — claiming
        // the text family's 65_536 would 400 them, so they must return None.
        assert_eq!(
            model_output_ceiling("gemini", "gemini-2.5-flash-image"),
            None
        );
        assert_eq!(
            model_output_ceiling("gemini", "gemini-2.5-pro-preview-tts"),
            None
        );
        assert_eq!(
            model_output_ceiling("gemini", "gemini-2.5-flash-preview-tts"),
            None
        );
        // Realtime variants (~8k real output) must also fail open.
        assert_eq!(
            model_output_ceiling("gemini", "gemini-2.5-flash-native-audio-preview"),
            None
        );
        assert_eq!(
            model_output_ceiling("gemini", "gemini-2.5-flash-live"),
            None
        );
    }

    #[test]
    fn deepseek_unmapped_variants_fail_open() {
        // v4-pro is a distinct model; a future v5 is unknown — neither may
        // inherit v4-flash's limits (the id checks are intentionally specific).
        assert_eq!(model_output_ceiling("deepseek", "deepseek-v4-pro"), None);
        assert_eq!(model_output_ceiling("deepseek", "deepseek-v5"), None);
    }

    /// #165 DRIFT GUARD — the durable prevention. This table is hand-maintained
    /// SEPARATELY from the routing catalog (`wcore_types::model_aliases`), which
    /// is how a shipped frontier model (gpt-5.4, claude-opus-4-8) ended up
    /// SILENTLY falling to the conservative default window: it was added to the
    /// catalog but not here, and the miss produced no error — just a wrong,
    /// too-small window.
    ///
    /// This test closes that loop: EVERY model the routing catalog can serve
    /// MUST resolve to a real window/output here. The moment someone adds a
    /// model to `models_for_provider()` without adding its verified limits above,
    /// CI goes red at that PR — a new model can never again ship undersized in
    /// silence. (Routers with no static catalog — flux-router / groq / sakana —
    /// are intentionally absent from `known_providers()` and so are not checked;
    /// their window comes from the live served-model signal, not this table.)
    #[test]
    fn every_routed_catalog_model_has_a_known_window() {
        use wcore_types::model_aliases::{known_providers, models_for_provider};
        let mut missing = Vec::new();
        for provider in known_providers() {
            for (alias, model_id) in models_for_provider(provider) {
                let Some((_out, window)) = model_output_ceiling(provider, model_id) else {
                    missing.push(format!("{provider} :: {alias} -> {model_id}"));
                    continue;
                };
                assert!(window > 0, "{provider}/{model_id}: window must be positive");
            }
        }
        assert!(
            missing.is_empty(),
            "these routed catalog models have NO context-window entry in \
             model_output_ceiling and would silently fall to the conservative \
             default (#165) — add their verified window/output above:\n  {}",
            missing.join("\n  ")
        );
    }
}
