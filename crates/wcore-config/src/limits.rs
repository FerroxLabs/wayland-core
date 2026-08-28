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

mod catalogue;

use catalogue::CATALOGUE_CEILINGS;

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
        // 5.6 joins 5.4/5.5 (models.dev, vendor row `openai`, 2026-08-28):
        // gpt-5.6 and its -luna / -sol / -terra siblings all serve 1,050,000.
        // Without this arm they fell to the 400k catch below and every 5.6 run
        // compacted at 40% of its real window.
        if (m.contains("gpt-5.4") || m.contains("gpt-5.5") || m.contains("gpt-5.6"))
            && !m.contains("-mini")
            && !m.contains("-nano")
            && !m.contains("-codex")
        {
            return Some((128_000, 1_050_000));
        }
        return Some((128_000, 400_000));
    }

    // --- xAI Grok 4.x ---
    // Added 2026-08-28. With no arm the whole 4.x family fell to the
    // `CompactConfig` default and compacted a 1M-window model at 200k.
    //
    // The OUTPUT figures here are the vendor's own, NOT the conservative
    // under-claim this table uses elsewhere, and that is deliberate: adding an
    // arm REVOKES `should_omit_max_tokens`, so until now these ids sent no
    // max_tokens at all and got xAI's natural ceiling. An arm that "errs low"
    // would not be free here — it would be the first thing ever to cut their
    // output. Vendor rows (models.dev provider `xai`, 2026-08-28) are the only
    // source for these ids; no reseller serves them.
    //
    // The split is real: 4.20/4.3 are 1M-window / 30k-output; 4.5 and 4.6 are
    // 500k both ways. Longest-match first so `grok-4.5` never falls into the
    // 4.x arm and loses 470k of output.
    if m.contains("grok-4.5") || m.contains("grok-4.6") {
        return Some((500_000, 500_000));
    }
    if m.contains("grok-4") {
        return Some((30_000, 1_000_000));
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

    // --- Google Gemini 3.x (text family) ---
    // Added 2026-08-28. Google's CURRENT generation had no arm at all, so
    // gemini-3.1-pro, 3.5-flash, 3.6-flash and 3.7-flash each compacted at the
    // 200k default against a real 1,048,576 window.
    //
    // Same numbers and the SAME exclusion list as 2.5, and for the same reason:
    // `google` and `google-vertex` agree at 65_536 / 1_048_576 for every text
    // tier (models.dev, 2026-08-28), while the specialty variants are much
    // smaller and would 400 on an over-claim — `-image` is 32_768/131_072 or
    // less, `-tts` is an 8k window, and the `-live` / live-translate variants
    // are smaller again.
    if m.contains("gemini-3")
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
    // share the 1,000,000 context window.
    //
    // OUTPUT WAS 8_192 AND THAT WAS A 47x CUT (#1157). "Err LOW" is sound while
    // a model has NO arm — `should_omit_max_tokens` then sends no field and the
    // provider applies its own ceiling. It stops being sound the moment an arm
    // EXISTS, because the arm REVOKES that omission and the number here becomes
    // the enforced cap. Measured on models.dev 2026-08-28, unanimous across the
    // three vendor-operated providers that serve it (`deepseek`,
    // `alibaba-token-plan`, `alibaba-cn`): 1,000,000 context / 384,000 output.
    //
    // `deepseek-v4-pro` is now a REAL id, not the hypothetical the old comment
    // guarded against, and the same three vendors publish the same figures for
    // it. Left unmapped it fell to the CompactConfig default and a 1M-context
    // model compacted at 200k — the #165 failure, in the direction that is
    // invisible. A future `deepseek-v5` still inherits nothing.
    if m.contains("deepseek-v4-flash")
        || m.contains("deepseek-v4-pro")
        || m == "deepseek-chat"
        || m == "deepseek-reasoner"
    {
        return Some((384_000, 1_000_000));
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
    // M3's output was 128_000 against a published 512_000 — the same
    // arm-revokes-omission mistake as DeepSeek above, costing 4x. `minimax` is
    // the only vendor-operated provider serving it and reports
    // 1,048,576 / 512,000 (models.dev, 2026-08-28). The context stays at the
    // Both numbers are the vendor's, exactly. The context was 1,000,000 against
    // a published 1,048,576 — only 4.7%, but there is no reason to round DOWN a
    // figure the vendor states unanimously, and rounding it down is what makes a
    // 1M model start compacting before it has to.
    if m.contains("minimax-m3") {
        return Some((512_000, 1_048_576));
    }
    if m.contains("minimax-m2.1") || m.contains("minimax-m2.5") || m.contains("minimax-m2.7") {
        return Some((128_000, 204_800));
    }
    if m.contains("minimax-m2") {
        // Base MiniMax-M2: 196,608 window (smaller than the point releases).
        return Some((128_000, 196_608));
    }

    // --- Catalogue-refresh families (GLM / Qwen / Kimi / Mistral / Llama) ---
    // See [`catalogue::CATALOGUE_CEILINGS`]. Kept as an ordered table in its own
    // module rather than more `if` arms: 33 entries is where a chain stops being
    // auditable, and a table lets `catalogue_table_has_no_shadowed_entries`
    // prove the ordering STRUCTURALLY instead of by spot-check.
    if let Some(&(_, output, context)) = CATALOGUE_CEILINGS
        .iter()
        .find(|(pattern, _, _)| m.contains(pattern))
    {
        return Some((output, context));
    }

    None
}

/// CORE-4 — conservative CONTEXT-WINDOW floor for the four Flux Router tier
/// aliases (`flux-auto` / `flux-fast` / `flux-standard` / `flux-reasoning`).
///
/// Deliberately a SEPARATE table from [`model_output_ceiling`]: the tier
/// aliases must stay UNKNOWN to that lookup so the engine's output sizing
/// keeps its router-alias behavior — `size_output_cap` clamps to the
/// conservative unknown floor (8192 / 32768 reasoning, #426) and
/// `should_omit_max_tokens` keeps omitting the wire max-tokens field on the
/// omit-safe Flux preset (#112), letting the SERVED model's natural output
/// ceiling apply. Listing the aliases in `model_output_ceiling` would silently
/// revoke both contracts.
///
/// What compaction needs is only the INPUT denominator. Flux routes a tier
/// alias to varying backends per request, so the only safe pre-route window is
/// the MINIMUM across each tier's realistic pool. No authoritative per-tier
/// pool manifest exists in this repo, so all four tiers use 128,000 — the safe
/// common denominator: the pools include 128k-class backends (gpt-4o = 128_000,
/// grok-3 = 131_072, the gpt-5.x chat tiers = 128_000), and every other
/// realistic member is larger. Erring LOW is safe here (compaction fires a
/// little early); erring high is the customer-reported wedge — with no window
/// the smart-compact trigger never fired and one session grew to 17M cumulative
/// input before dying at `finish_reason: length`.
///
/// Once Flux signals the real served-model window back (`x-flux-model-window`,
/// #282), the engine prefers THAT over this floor — this value only governs
/// turns before the first signal (or routes that never send one).
///
/// Matched case-insensitively against the four documented aliases, same set as
/// `wcore_providers::is_flux_tier_alias` (which lives downstream of this crate
/// and so cannot be called from here).
pub fn flux_tier_context_window(model: &str) -> Option<u32> {
    match model.to_ascii_lowercase().as_str() {
        "flux-auto" | "flux-fast" | "flux-standard" | "flux-reasoning" => Some(128_000),
        _ => None,
    }
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
            Some((512_000, 1_048_576))
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
        // LOAD-BEARING for CORE-4: the Flux tier aliases must stay UNKNOWN to
        // this OUTPUT-sizing lookup even though they now have a context-window
        // floor in `flux_tier_context_window` — a Some() here would make
        // `size_output_cap` clamp Flux output to a fixed ceiling and flip
        // `should_omit_max_tokens` off (#112/#426 router-alias contracts).
        for alias in ["flux-auto", "flux-fast", "flux-standard", "flux-reasoning"] {
            assert_eq!(model_output_ceiling("flux-router", alias), None);
        }
        assert_eq!(model_output_ceiling("openai", "some-future-model"), None);
        assert_eq!(model_output_ceiling("ollama", "llama3.1"), None);
    }

    #[test]
    fn flux_tier_aliases_resolve_conservative_context_window() {
        // CORE-4: all four tier aliases carry the 128k pool-minimum window so
        // the compaction kernel gets a real denominator (customer evidence:
        // with None the smart trigger never fired and a session wedged at
        // finish_reason=length after 17M cumulative input tokens).
        for alias in ["flux-auto", "flux-fast", "flux-standard", "flux-reasoning"] {
            assert_eq!(
                flux_tier_context_window(alias),
                Some(128_000),
                "{alias} must resolve the conservative 128k floor"
            );
        }
        // Case-insensitive, consistent with model_output_ceiling.
        assert_eq!(flux_tier_context_window("Flux-Reasoning"), Some(128_000));
        // Concrete model ids and non-flux names stay None — the floor is for
        // the four documented tier aliases ONLY (a pinned model resolves its
        // real window via model_output_ceiling).
        assert_eq!(flux_tier_context_window("flux-pinned-gpt-5"), None);
        assert_eq!(flux_tier_context_window("gpt-4o"), None);
        assert_eq!(flux_tier_context_window(""), None);
    }

    #[test]
    fn deepseek_v4_flash_family_uses_1m_context_window() {
        // #255: the canonical id and both deprecated aliases share the 1M window.
        for id in [
            "deepseek-v4-flash",
            "deepseek-v4-pro",
            "deepseek-chat",
            "deepseek-reasoner",
        ] {
            assert_eq!(
                model_output_ceiling("deepseek", id),
                Some((384_000, 1_000_000)),
                "{id} must report the 1,000,000-token context window"
            );
        }
        // Case-insensitive match (the lookup lowercases first).
        assert_eq!(
            model_output_ceiling("deepseek", "DeepSeek-V4-Flash"),
            Some((384_000, 1_000_000))
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
    fn gpt_5_6_gets_the_large_window_and_its_small_siblings_do_not() {
        // Refreshed 2026-08-28: 5.6 serves 1,050,000 exactly as 5.4/5.5 do.
        // Before this arm every 5.6 id fell to the 400k catch and compacted at
        // 40% of its real window.
        for id in ["gpt-5.6", "gpt-5.6-luna", "gpt-5.6-sol", "gpt-5.6-terra"] {
            assert_eq!(
                model_output_ceiling("openai", id),
                Some((128_000, 1_050_000)),
                "{id} must report the 1.05M window"
            );
        }
        // The exclusion list is load-bearing and must apply to 5.6 too: the
        // small variants are 400k, and claiming 1.05M for them would 400 near
        // the top.
        for id in ["gpt-5.6-mini", "gpt-5.6-nano", "gpt-5.6-codex"] {
            assert_eq!(
                model_output_ceiling("openai", id),
                Some((128_000, 400_000)),
                "{id} must stay at the 400k window"
            );
        }
    }

    #[test]
    fn grok_4_family_splits_by_version_and_does_not_shadow_grok_3() {
        // Added 2026-08-28. Vendor rows (models.dev provider `xai`) are the
        // only source for these ids: 4.20/4.3 are 1M-window / 30k-output;
        // 4.5 and 4.6 are 500k both ways.
        for id in ["grok-4.5", "grok-4.6"] {
            assert_eq!(
                model_output_ceiling("xai", id),
                Some((500_000, 500_000)),
                "{id} is 500k both ways"
            );
        }
        // ORDERING IS LOAD-BEARING. `grok-4.5` also contains `grok-4`, so
        // moving the general arm above the 4.5/4.6 one would silently cut
        // their output from 500,000 to 30,000. This assertion is what fails if
        // the arms are reordered.
        for id in [
            "grok-4.3",
            "grok-4.20-0309-reasoning",
            "grok-4.20-multi-agent-0309",
        ] {
            assert_eq!(
                model_output_ceiling("xai", id),
                Some((30_000, 1_000_000)),
                "{id} is the 1M-window / 30k-output tier"
            );
        }
        // The 3.x arm is untouched, and 4.x must not have swallowed it.
        assert_eq!(
            model_output_ceiling("xai", "grok-3"),
            Some((64_000, 131_072))
        );
    }

    #[test]
    fn gemini_3_text_family_resolves_and_its_specialty_variants_fail_open() {
        // Added 2026-08-28: Google's CURRENT generation had no arm at all and
        // compacted at the 200k default against a real 1,048,576 window.
        for id in [
            "gemini-3-flash-preview",
            "gemini-3.1-pro-preview",
            "gemini-3.1-flash-lite",
            "gemini-3.5-flash",
            "gemini-3.6-flash",
            "gemini-3.7-flash",
        ] {
            assert_eq!(
                model_output_ceiling("gemini", id),
                Some((65_536, 1_048_576)),
                "{id} must report the Gemini 3.x text-family limits"
            );
        }
        // Same exclusions as 2.5, and they matter more here: the 3.x image and
        // live tiers are 131_072 or smaller, so an over-claim would 400 them.
        for id in [
            "gemini-3-pro-image",
            "gemini-3.1-flash-image-preview",
            "gemini-3.1-flash-tts-preview",
            "gemini-3.1-flash-live-preview",
            "gemini-3.5-live-translate-preview",
        ] {
            assert_eq!(
                model_output_ceiling("gemini", id),
                None,
                "{id} must fail open rather than inherit the text-family window"
            );
        }
    }

    #[test]
    fn deepseek_unmapped_variants_fail_open() {
        // v4-pro USED to be here. It is now a real, shipped id that the three
        // vendor-operated providers publish at the same 1,000,000 / 384,000 as
        // v4-flash, so failing it open meant a 1M model compacting at the 200k
        // default. A future v5 is still unknown and must inherit nothing —
        // that half of the original claim is what this test still guards.
        assert_eq!(model_output_ceiling("deepseek", "deepseek-v5"), None);
        assert_eq!(model_output_ceiling("deepseek", "deepseek-v3.2"), None);
    }

    /// The output ceilings that were cut by an order of magnitude, pinned
    /// against the value they were cut TO. #1157: an arm revokes
    /// `should_omit_max_tokens`, so a conservative number here is not caution —
    /// it is an enforced cap the provider would not otherwise have applied.
    #[test]
    fn output_ceilings_are_not_the_old_conservative_cuts() {
        assert_ne!(
            model_output_ceiling("deepseek", "deepseek-v4-flash"),
            Some((8_192, 1_000_000)),
            "8,192 against a published 384,000 is a 47x cut, not caution"
        );
        assert_ne!(
            model_output_ceiling("minimax", "MiniMax-M3"),
            Some((128_000, 1_000_000)),
            "128,000 against a published 512,000 is a 4x cut"
        );
        assert_eq!(
            model_output_ceiling("minimax", "MiniMax-M3"),
            Some((512_000, 1_048_576))
        );
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
