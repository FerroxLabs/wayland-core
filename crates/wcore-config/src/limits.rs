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

    // --- Catalogue-refresh families (GLM / Qwen / Kimi / Mistral / Llama) ---
    // See CATALOGUE_CEILINGS below. Kept as an ordered table rather than more
    // `if` arms: 33 entries is where a chain stops being auditable, and a table
    // lets `catalogue_table_has_no_shadowed_entries` prove the ordering
    // STRUCTURALLY instead of by spot-check.
    if let Some(&(_, output, context)) = CATALOGUE_CEILINGS
        .iter()
        .find(|(pattern, _, _)| m.contains(pattern))
    {
        return Some((output, context));
    }

    None
}

/// Ordered `(id fragment, max_output_tokens, context_window)` table for the
/// families added by the 2026-08-27 models.dev catalogue refresh.
///
/// SOURCE OF TRUTH — models.dev `/api.json`, snapshot fetched **2026-08-27**,
/// sha256 `63211863354dac0cf032067b93316cdd2a7940c8788e6fff1028e573077004c1`
/// (4,318,657 bytes, 203 providers). The JSON itself is deliberately NOT
/// committed (4 MB); re-pull it and diff if these figures look stale.
/// `scripts/check-model-limits-freshness.py` mechanises exactly that diff and
/// runs at release time -- see `just check-model-limits-freshness`.
///
/// # Matching is ORDERED and the order is load-bearing
///
/// `find` returns the FIRST fragment contained in the model id, so a fragment
/// that is a substring of a later one must come first: `"glm-5"` matches
/// `glm-5.3`, so `"glm-5.3"` is listed above it. Same for
/// `mistral-large-2411` vs `mistral-large`. This is not a convention to
/// respect -- `catalogue_table_has_no_shadowed_entries` fails the build if any
/// earlier fragment shadows a later one.
///
/// # How these numbers were chosen -- READ BEFORE RAISING ANY OF THEM
///
/// The catalogue disagrees with itself, badly. The same id is reported with
/// different limits by different providers (`glm-5.3` context ranges over
/// 1,000,000 and 1,048,576; `vercel` reports its output as `12800`, a dropped
/// digit from `128000`; `qwen3.8-2.4t-a95b` has a row claiming `out=1010000`).
/// So each row here is a CONSENSUS, resolved by a fixed rule:
///
///   * **context** -- the lowest value reported by any vendor-operated provider
///     (`zhipuai`/`zai` for GLM, `alibaba`/`alibaba-cn` for Qwen, `moonshotai`
///     for Kimi, `mistral` for Mistral, `llama` for Llama). Where the vendors
///     agree, the lowest value reported by two or more independent providers.
///   * **output** -- the lowest NON-DEGENERATE value reported by a
///     vendor-operated provider, rounded down to a round number. "Degenerate"
///     means `output == context`, which is how models.dev encodes "no separate
///     output cap known" and must never be read as a real ceiling.
///
/// **The two error directions are NOT symmetric, and that decides every tie.**
/// A ceiling set too HIGH kills a run mid-flight -- that is #165 verbatim,
/// where a gpt-5.4 run died at 178,336 tokens against a fake ~177k ceiling. A
/// ceiling set too LOW only compacts early: degraded, not fatal. So **when
/// credible sources disagree, take the LOWER value.** Do not "correct" a number
/// here upward to match a vendor page without re-running the freshness check;
/// the gap between this table and the vendor headline figure is usually the
/// deliberate safety margin, not an error.
///
/// # Adding an entry here REVOKES the omitted-max-tokens contract
///
/// `wcore_agent::engine::should_omit_max_tokens` (#112) omits the wire
/// `max_tokens` field ONLY while a model is UNKNOWN to this table, letting the
/// SERVED model's natural output ceiling apply on omit-safe providers (gemini /
/// openrouter / flux-router presets). The moment a model gains an arm here, its
/// turns start sending OUR number instead. So an output figure that is merely
/// "safely low" is not free: on those providers it REPLACES a natural ceiling
/// that may be far higher, and an undersized turn ends visibly at
/// `finish_reason: length` with no auto-continue. That is why the output column
/// tracks the modal NON-DEGENERATE catalogue figure rather than the single
/// lowest row -- a lone outlier (one `alibaba-cn` row reporting 16,384 for
/// `kimi-k2.6` against 40+ rows at 65,536 and above) is noise, and treating it
/// as the floor would truncate real work.
///
/// # What is deliberately NOT here
///
/// Open-weights models whose served window is chosen by the HOST, not the
/// vendor, are excluded: measured in the snapshot, `qwen3.6-27b` is served at
/// 262,144 by `alibaba`, 131,072 by `groq`, 120,000 by `regolo-ai` and 32,768
/// by `pioneer` -- one static number cannot be right, and any number at or
/// above the 200,000 `CompactConfig` default makes the status quo WORSE on the
/// small hosts. So the Qwen dense tier (`qwen3.x-27b/35b/122b/397b`,
/// `qwen3.8-2.4t-a95b`) fails open on purpose. Llama IS listed despite being
/// open weights because its conservative figures (128,000 context / 4,096
/// output) are at or below the status-quo fallback in BOTH dimensions -- adding
/// it is strictly safer than leaving it unknown, which is the bar for including
/// an open-weights id at all.
///
/// Also absent: GLM 4.5/4.6, the Kimi K2.0 ids, `qwen-max`/`qwen-plus`/
/// `qwen-turbo` (floating aliases whose window differs 4x between `alibaba` at
/// 32,768 and `alibaba-cn` at 131,072 for the SAME name), and every audio /
/// vision / embedding variant.
const CATALOGUE_CEILINGS: &[(&str, u32, u32)] = &[
    // -- Zhipu GLM ----------------------------------------------------------
    // Vendor rows (`zhipuai`, `zai`, and both coding-plan tenants) report
    // output 131,072 for everything 4.7 and newer; `alibaba-cn` reports
    // 128,000 for the same ids, so 128,000 is the floor across vendors.
    // Context: 5.3/5.2 are the 1M generation; 5.1/5/4.7 report 200,000-204,800
    // depending on tenant, so 200,000.
    ("glm-5.3", 128_000, 1_000_000),
    ("glm-5.2", 128_000, 1_000_000),
    ("glm-5.1", 128_000, 200_000),
    // Bare `glm-5` LAST in the 5.x group -- it is a substring of every id
    // above. Also catches `glm-5-turbo` / `glm-5v-turbo`, both genuinely
    // 200,000.
    ("glm-5", 128_000, 200_000),
    ("glm-4.7", 128_000, 200_000),
    // -- Alibaba Qwen (API-served tiers only) -------------------------------
    // `alibaba` + `alibaba-cn` + the token/coding plans agree: the `-max` /
    // `-plus` / `-flash` tiers are Alibaba-gated, so every reseller proxies the
    // same limits and the id really is decisive. 3.8-max reports output
    // 131,072; 3.7 and older report 65,536 (and `alibaba-cn` says 64,000 for
    // 3.7-plus, hence 64,000).
    ("qwen3.8-max", 128_000, 1_000_000),
    ("qwen3.7-max", 64_000, 1_000_000),
    ("qwen3.7-plus", 64_000, 1_000_000),
    ("qwen3.7-flash", 64_000, 1_000_000),
    // qwen3.6-max is NOT a 1M model -- `alibaba` reports 262,144 and
    // `alibaba-cn` 245,800, against 1,000,000 for its own -plus/-flash
    // siblings. Listing it is what stops it inheriting a 4x over-claim.
    ("qwen3.6-max", 64_000, 240_000),
    ("qwen3.6-plus", 64_000, 1_000_000),
    ("qwen3.6-flash", 64_000, 1_000_000),
    ("qwen3.5-plus", 64_000, 1_000_000),
    ("qwen3.5-flash", 64_000, 1_000_000),
    // -- Moonshot Kimi ------------------------------------------------------
    // `moonshotai` reports K3 as 1,048,576 / 131,072; eight independent
    // providers report 1,000,000, so 1,000,000 is the floor, and 131,072 is
    // the dominant output reading (38 of 63 rows) -> 128,000.
    //
    // For the K2.x line `moonshotai` reports output == context (degenerate),
    // so output comes from the modal NON-DEGENERATE reading instead: 32,768
    // (8 rows for k2.5, 4 for k2.7-code), not the lone 16,384 outlier -- see
    // the omitted-max-tokens note above for why the lowest row is the wrong
    // floor here.
    ("kimi-k3", 128_000, 1_000_000),
    ("kimi-k2.7", 32_768, 256_000),
    ("kimi-k2.6", 32_768, 256_000),
    ("kimi-k2.5", 32_768, 256_000),
    // -- Mistral ------------------------------------------------------------
    // Version here is a DATE, not a semver, and the old dated ids are much
    // smaller than the `-latest` alias they share a prefix with:
    // `mistral-large-2411` is 131,072 where `mistral-large-latest` is 262,144.
    // Every dated arm therefore precedes its family catch-all.
    //
    // Output splits the same way. The OLD dated ids and magistral state a real
    // 16,384 at the vendor; the modern line (`-latest`, 2512/2603/2604) is
    // degenerate at the vendor and its non-degenerate readings elsewhere are
    // 209,715-262,144, so 65,536 is the conservative figure there. Using
    // 16,384 across the whole family would have cut modern Mistral output by
    // 16x on omit-safe providers -- see the note above.
    ("mistral-large-2411", 16_384, 131_072),
    ("mistral-large", 65_536, 262_144),
    ("mistral-medium-2505", 65_536, 131_072),
    ("mistral-medium", 65_536, 262_144),
    ("mistral-small-2506", 16_384, 128_000),
    ("mistral-small", 65_536, 256_000),
    ("magistral", 16_384, 128_000),
    ("codestral", 4_096, 256_000),
    ("devstral-small-2505", 16_384, 128_000),
    ("devstral-small-2507", 16_384, 128_000),
    ("devstral-medium-2507", 16_384, 128_000),
    // Catch-all at 256,000, not the 262,144 that `devstral-latest` reports:
    // `labs-devstral-small-2512` lands here and is 256,000.
    ("devstral", 65_536, 256_000),
    // -- Meta Llama ---------------------------------------------------------
    // `llama` (llama.com) reports 128,000 / 4,096 for every id, and 4,096 is
    // the point: with no entry these fall to the UNKNOWN_CAP 8,192 output
    // floor, which OVER-claims by 2x and 400s. Included despite being open
    // weights precisely because both figures are below the status-quo
    // fallback. NOTE: the `meta` provider ships `muse-spark-*`, not Llama.
    ("llama-4-maverick", 4_096, 128_000),
    ("llama-4-scout", 4_096, 128_000),
    // Trailing `-` so `llama3.1` and other unversioned names stay unknown.
    ("llama-3.3-", 4_096, 128_000),
];

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

    /// ORDERING GUARANTEE, structural rather than by spot-check.
    ///
    /// `CATALOGUE_CEILINGS` is matched with `find(|(p, ..)| m.contains(p))`, so
    /// if an EARLIER fragment is a substring of a LATER one, the later entry is
    /// unreachable and its model silently inherits the wrong limits. That is
    /// the exact failure this table exists to prevent (`glm-5` matches
    /// `glm-5.3`; `mistral-large` matches `mistral-large-2411`).
    ///
    /// A comment saying "keep these ordered" is not a guarantee. This is: every
    /// pair is checked, so reordering ANY shadowing pair fails the build, not
    /// just the handful a spot-check happened to name.
    #[test]
    fn catalogue_table_has_no_shadowed_entries() {
        for (i, (later, _, _)) in CATALOGUE_CEILINGS.iter().enumerate() {
            for (earlier, _, _) in CATALOGUE_CEILINGS.iter().take(i) {
                assert!(
                    !later.contains(earlier),
                    "entry {i} ({later:?}) is unreachable: the earlier fragment \
                     {earlier:?} is a substring of it, so `find` stops there \
                     first and {later:?} silently inherits {earlier:?}'s \
                     limits. Move {later:?} ABOVE {earlier:?}."
                );
            }
        }
        // Sanity: the table really does contain shadowing pairs, so the loop
        // above is exercising a live hazard and not vacuously passing over a
        // table where no fragment could ever shadow another.
        let shadow_pairs = CATALOGUE_CEILINGS
            .iter()
            .enumerate()
            .filter(|(i, (later, _, _))| {
                CATALOGUE_CEILINGS
                    .iter()
                    .skip(i + 1)
                    .any(|(other, _, _)| other.contains(later) || later.contains(other))
            })
            .count();
        assert!(
            shadow_pairs >= 4,
            "expected the table to still contain prefix-shadowing families \
             (glm-5.x, mistral-large/medium/small, devstral); found \
             {shadow_pairs}. If these were renamed apart, the ordering check \
             above has become vacuous."
        );
    }

    /// The concrete half of the ordering contract: real ids, real figures.
    /// Reordering any newer entry below its older prefix flips these.
    #[test]
    fn newer_catalogue_ids_do_not_fall_through_to_older_arms() {
        // `glm-5` is a substring of `glm-5.3` / `glm-5.2` / `glm-5.1`. If the
        // bare arm ran first, all three would report 200_000 instead of their
        // real windows.
        assert_eq!(
            model_output_ceiling("z-ai", "glm-5.3"),
            Some((128_000, 1_000_000)),
            "glm-5.3 must NOT inherit the bare glm-5 200k window"
        );
        assert_eq!(
            model_output_ceiling("z-ai", "glm-5.2"),
            Some((128_000, 1_000_000))
        );
        assert_eq!(
            model_output_ceiling("z-ai", "glm-5.1"),
            Some((128_000, 200_000))
        );
        assert_eq!(
            model_output_ceiling("z-ai", "glm-5"),
            Some((128_000, 200_000))
        );

        // The Mistral trap runs the other way: the DATED id is the small one,
        // so it must be matched before the family catch-all or a 131k model
        // gets handed a 262k window and 400s near the top.
        assert_eq!(
            model_output_ceiling("mistral", "mistral-large-2411"),
            Some((16_384, 131_072)),
            "the 2411 large is 131k/16k — it must NOT inherit \
             mistral-large-latest's 262k window or its larger output"
        );
        assert_eq!(
            model_output_ceiling("mistral", "mistral-large-latest"),
            Some((65_536, 262_144))
        );
        assert_eq!(
            model_output_ceiling("mistral", "mistral-medium-2505"),
            Some((65_536, 131_072))
        );
        assert_eq!(
            model_output_ceiling("mistral", "mistral-small-2506"),
            Some((16_384, 128_000)),
            "the 2506 small states a real 16,384 output at the vendor"
        );
        assert_eq!(
            model_output_ceiling("mistral", "mistral-small-2603"),
            Some((65_536, 256_000))
        );

        // qwen3.6-max is a 240k model sitting between two 1M siblings.
        assert_eq!(
            model_output_ceiling("alibaba", "qwen3.6-max-preview"),
            Some((64_000, 240_000)),
            "qwen3.6-max is 240k, NOT the 1M its -plus/-flash siblings serve"
        );
        assert_eq!(
            model_output_ceiling("alibaba", "qwen3.6-plus"),
            Some((64_000, 1_000_000))
        );
    }

    #[test]
    fn glm_family_resolves_to_first_party_consensus() {
        // Vendor rows (`zhipuai`, `zai`, both coding-plan tenants) at the
        // 2026-08-27 snapshot. 5.3/5.2 are the 1M generation; 5.1/5/4.7 are
        // 200k-204,800 depending on tenant, so 200,000.
        for id in ["glm-5.3", "glm-5.3-flash", "glm-5.3-highspeed", "glm-5.2"] {
            assert_eq!(
                model_output_ceiling("z-ai", id),
                Some((128_000, 1_000_000)),
                "{id} must report the GLM 1M-generation limits"
            );
        }
        for id in ["glm-5.1", "glm-5", "glm-5-turbo", "glm-5v-turbo", "glm-4.7"] {
            assert_eq!(
                model_output_ceiling("zhipuai", id),
                Some((128_000, 200_000)),
                "{id} must report the GLM 200k-generation limits"
            );
        }
        // Provider-prefixed ids (the shape `google-vertex` and the aggregators
        // use) resolve identically — the fragment match is provider-agnostic.
        assert_eq!(
            model_output_ceiling("google-vertex", "zai-org/glm-4.7-maas"),
            Some((128_000, 200_000))
        );
        // Case-insensitive, consistent with the rest of the lookup.
        assert_eq!(
            model_output_ceiling("z-ai", "GLM-5.3"),
            Some((128_000, 1_000_000))
        );
    }

    #[test]
    fn qwen_api_tiers_resolve_to_alibaba_consensus() {
        assert_eq!(
            model_output_ceiling("alibaba", "qwen3.8-max"),
            Some((128_000, 1_000_000))
        );
        assert_eq!(
            model_output_ceiling("alibaba", "qwen3.8-max-preview"),
            Some((128_000, 1_000_000))
        );
        for id in [
            "qwen3.7-max",
            "qwen3.7-plus",
            "qwen3.7-flash",
            "qwen3.6-plus",
            "qwen3.6-flash",
            "qwen3.5-plus",
            "qwen3.5-flash",
        ] {
            assert_eq!(
                model_output_ceiling("alibaba", id),
                Some((64_000, 1_000_000)),
                "{id} is an Alibaba-gated 1M tier"
            );
        }
    }

    #[test]
    fn kimi_family_resolves_to_moonshot_consensus() {
        for id in ["kimi-k3", "kimi-k3-fast", "kimi-k3-eco", "kimi-k3@eu"] {
            assert_eq!(
                model_output_ceiling("moonshotai", id),
                Some((128_000, 1_000_000)),
                "{id} must report the K3 1M window"
            );
        }
        for id in ["kimi-k2.7-code", "kimi-k2.6", "kimi-k2.5"] {
            assert_eq!(
                model_output_ceiling("moonshotai", id),
                Some((32_768, 256_000)),
                "{id} must report the K2.x 256k window and the modal \
                 non-degenerate 32,768 output (moonshotai reports output == \
                 context, which is models.dev for `unknown`, not a ceiling)"
            );
        }
        assert_eq!(
            model_output_ceiling("moonshotai", "Kimi-K3"),
            Some((128_000, 1_000_000))
        );
    }

    #[test]
    fn llama_family_caps_output_below_the_unknown_floor() {
        // The POINT of these entries: llama.com reports output 4,096, but with
        // no entry these fall to `size_output_cap`'s UNKNOWN_CAP of 8,192 —
        // a 2x OVER-claim, which is the fatal direction (hard 400), not the
        // recoverable one.
        for id in [
            "llama-4-maverick-17b-128e-instruct-fp8",
            "llama-4-scout-17b-16e-instruct-fp8",
            "llama-3.3-70b-instruct",
            "llama-3.3-8b-instruct",
            "cerebras-llama-4-maverick-17b-128e-instruct",
        ] {
            assert_eq!(
                model_output_ceiling("llama", id),
                Some((4_096, 128_000)),
                "{id} must cap output at 4,096 — BELOW the 8,192 unknown floor"
            );
        }
        // The unversioned Ollama-style name must stay unknown (the trailing
        // `-` in the `llama-3.3-` fragment is what guarantees this).
        assert_eq!(model_output_ceiling("ollama", "llama3.1"), None);
        assert_eq!(model_output_ceiling("ollama", "llama3.3"), None);
    }

    #[test]
    fn host_variable_open_weights_stay_unknown() {
        // Measured in the 2026-08-27 snapshot: `qwen3.6-27b` is served at
        // 262,144 (alibaba), 131,072 (groq), 120,000 (regolo-ai) and 32,768
        // (pioneer). No single static window is correct, and anything at or
        // above the 200,000 CompactConfig default makes the small hosts WORSE
        // than the status quo. These must fail open.
        for id in [
            "qwen3.8-27b",
            "qwen3.8-2.4t-a95b",
            "qwen3.6-27b",
            "qwen3.6-35b-a3b",
            "qwen3.5-397b-a17b",
            "qwen3.5-122b-a10b",
        ] {
            assert_eq!(
                model_output_ceiling("alibaba", id),
                None,
                "{id} is host-variable open weights and MUST fail open"
            );
        }
        // Floating aliases whose window differs 4x between alibaba (32,768)
        // and alibaba-cn (131,072) for the SAME name.
        for id in ["qwen-max", "qwen-plus", "qwen-turbo", "qwen-long"] {
            assert_eq!(
                model_output_ceiling("alibaba", id),
                None,
                "{id} must fail open"
            );
        }
        // Out of the refresh's declared scope — must not be half-covered.
        assert_eq!(model_output_ceiling("z-ai", "glm-4.6"), None);
        assert_eq!(model_output_ceiling("z-ai", "glm-4.5"), None);
        assert_eq!(model_output_ceiling("moonshotai", "kimi-k2-thinking"), None);
        assert_eq!(model_output_ceiling("moonshotai", "kimi-latest"), None);
        // Mistral audio/embedding rows report ctx=0 in the catalogue.
        assert_eq!(model_output_ceiling("mistral", "mistral-embed"), None);
        assert_eq!(
            model_output_ceiling("mistral", "voxtral-small-latest"),
            None
        );
        assert_eq!(model_output_ceiling("mistral", "mistral-nemo"), None);
    }

    /// The two lookups must not interfere. The `if` chain runs FIRST, so an
    /// existing arm that happened to match a new-family id would silently win
    /// and never reach the table; and a new fragment that matched an id the
    /// chain owns would be dead code at best.
    #[test]
    fn catalogue_table_does_not_collide_with_the_existing_chain() {
        // Ids owned by the pre-existing chain must resolve to the chain's
        // figures, unchanged by this table.
        assert_eq!(
            model_output_ceiling("anthropic", "claude-opus-4-8"),
            Some((128_000, 1_000_000))
        );
        assert_eq!(
            model_output_ceiling("openai", "gpt-5.4"),
            Some((128_000, 1_050_000))
        );
        assert_eq!(
            model_output_ceiling("minimax", "MiniMax-M2"),
            Some((128_000, 196_608))
        );
        // ...and no catalogue fragment may be a substring of an id the chain
        // owns (which would make the table entry unreachable).
        for chain_id in [
            "claude-opus-4-8",
            "claude-sonnet-4-6",
            "claude-haiku-4-5",
            "claude-fable-5",
            "gpt-4.1",
            "gpt-4o",
            "gpt-5.4-codex-spark",
            "grok-3",
            "gemini-2.5-pro",
            "deepseek-v4-flash",
            "minimax-m3",
            "minimax-m2.5",
        ] {
            for (fragment, _, _) in CATALOGUE_CEILINGS {
                assert!(
                    !chain_id.contains(fragment),
                    "catalogue fragment {fragment:?} also matches chain-owned \
                     id {chain_id:?} — the chain would answer first and the \
                     table entry is unreachable"
                );
            }
        }
        // The Flux tier aliases must STILL be unknown to this lookup (#112 /
        // #426 router-alias contracts, see `flux_tier_context_window`).
        for alias in ["flux-auto", "flux-fast", "flux-standard", "flux-reasoning"] {
            assert_eq!(model_output_ceiling("flux-router", alias), None);
        }
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
