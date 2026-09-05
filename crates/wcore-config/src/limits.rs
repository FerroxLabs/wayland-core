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
#[cfg(test)]
mod passthrough;

use catalogue::CATALOGUE_CEILINGS;

/// The endpoint identities operated by each open-weights family's vendor, as
/// `ProviderCompat::provider_type()` spells them -- the only string
/// `model_output_ceiling` ever sees.
///
/// WHY A SET, AND WHY PREFIXES. The first cut of #1232 keyed each gated arm on
/// ONE provider name, and that shape cannot be right: a vendor reaches this
/// function under several identities at once, and every identity it is not
/// spelled as loses the arm and takes the 47x cut (`UNKNOWN_CAP` 8,192 output /
/// `UNVERIFIED_CONTEXT_WINDOW` 32,768 window) that #1157 was filed to remove.
/// DeepSeek V4 is the worked example, and all four routes below are real:
///
///   * `deepseek` -- `ProviderType::Deepseek`, the vendor's own API;
///   * `qwen` -- `parse_builtin_provider` maps `"qwen" | "alibaba" |
///     "dashscope"` onto `ProviderType::Qwen`, whose slug is `"qwen"`.
///     Builtins resolve BEFORE the bundled catalog, so `--provider alibaba` --
///     the DEFAULT DashScope route -- arrives here spelled `qwen` and never as
///     `alibaba` at all;
///   * `alibaba-*` -- `alibaba-cn`, `alibaba-token-plan`, `alibaba-coding-plan`
///     and `alibaba-coding-plan-cn` are bundled catalog ids, and
///     `from_catalog_entry` stamps `provider_type` = the catalog id verbatim;
///   * tenant spellings models.dev publishes that the catalog has not adopted
///     yet (`alibaba-token-plan-cn` appeared between two pulls of this table).
///
/// That last bullet is why the members are PREFIXES matched at a `-` boundary
/// rather than exact names: a vendor adds a region/plan tenant far more often
/// than this table is edited, and an exact list silently drops each new one.
/// The `-` boundary is what keeps `minimaxproxy` and `deepseekproxy` out -- a
/// reseller whose name merely STARTS with the vendor's is not the vendor.
///
/// MiniMax needs no second member: its tenants (`minimax-cn`,
/// `minimax-coding-plan`, `minimax-cn-coding-plan`) all sit under the `minimax`
/// prefix, and `provider_type_slug(ProviderType::MiniMax)` is `"minimax"` too.
/// That is luck, not design -- which is exactly why DeepSeek's first-party
/// tenants, branded `alibaba*`, were the ones that broke.
///
/// GRADED AGAINST ARTEFACTS THIS ONE DOES NOT WRITE:
/// `vendor_operated_catalog_endpoints_keep_their_arm` re-derives the answer
/// from `providers.toml` base-URL hosts, and `check-model-limits-freshness.py`
/// re-derives it live from models.dev. A test that reads this const for both
/// the question AND the answer cannot catch a wrong member here, and the one
/// that did read it that way did not.
const VENDOR_OPERATED_ENDPOINTS: &[(&str, &[&str])] = &[
    ("deepseek", &["deepseek", "qwen", "alibaba"]),
    ("minimax", &["minimax"]),
];

/// The DNS suffixes each family's vendor serves its own API from. Used only by
/// `vendor_operated_catalog_endpoints_keep_their_arm`, to re-derive the
/// endpoint set from `providers.toml` rather than from
/// [`VENDOR_OPERATED_ENDPOINTS`].
#[cfg(test)]
const VENDOR_API_DOMAINS: &[(&str, &[&str])] = &[
    ("deepseek", &["deepseek.com", "aliyuncs.com"]),
    ("minimax", &["minimax.io", "minimaxi.com"]),
];

/// FerroxLabs/wayland#1232 -- every open-weights id carried by the `if`-chain
/// families, tagged with the FAMILY whose vendor operates it when its hosts
/// disagree too widely for one static figure to be true of all of them. The
/// tag is a key into [`VENDOR_OPERATED_ENDPOINTS`], never a single provider
/// name: "the vendor that operates this family" is a SET of endpoint
/// identities in every case, and naming one member of that set is the defect
/// this const was rewritten to make unrepresentable.
///
/// WHY A GATE AND NOT A DELETION. AGENTS.md's third model-limits rule forbids a
/// static arm for an open-weights id served at wildly different limits by
/// different hosts, and on the 2026-08-30 models.dev pull seven of these span
/// 3.5x-8.2x across 19-64 endpoints. Deleting those arms is NOT free, and not
/// symmetric with the Qwen precedent: an arm revokes `should_omit_max_tokens`,
/// but that omission only exists on an OMIT-SAFE preset (`gemini`,
/// `openrouter`, `flux-router`). DeepSeek's and MiniMax's own endpoints are
/// plain `openai_compat_provider` presets, which are NOT omit-safe -- so on the
/// vendor's own API a missing arm restores no natural ceiling at all. It drops
/// output to `UNKNOWN_CAP` (8,192) and the window to `UNVERIFIED_CONTEXT_WINDOW`
/// (32,768): the 47x cut #1157 was filed to fix, re-introduced.
///
/// So the defect is not the figures, it is the KEY -- #1232's own words, "keyed
/// on the model id alone, with no provider in the key". Scoping the seven to
/// their vendor gives every caller the right answer at once:
///
///   * the vendor's endpoint keeps the vendor's verified figures, unchanged;
///   * an omit-safe reseller route (`openrouter` / `flux-router`) resolves
///     `None`, which RESTORES the omission and lets that host apply its own
///     natural ceiling -- the outcome a deletion was wanted for;
///   * any other host resolves `None` too, so the window falls to
///     `UNVERIFIED_CONTEXT_WINDOW` (32,768, below every measured host) and the
///     output to `UNKNOWN_CAP` (8,192, which IS the measured host floor for
///     both families: nebius serves minimax-m2.5 at 8,192 and deepinfra serves
///     deepseek-v4-pro at 8,192). It errs LOW, where a wrong high number is
///     ceiling death (#165).
///
/// THE FIVE ROWS THAT ARE NOT GATED are the point of measuring instead of
/// exempting the family: `deepseek-v4-flash-vision-exp` (1.05x),
/// `deepseek-v4-pro-0813` (1.05x), `minimax-m2.5-highspeed` (1.0x),
/// `minimax-m2.7` (1.3x) and `minimax-m2.7-highspeed` (1.02x) have hosts that
/// AGREE, so gating them would replace a real ceiling with a low guess for no
/// benefit -- the harm this rule exists to prevent, in the other direction.
///
/// ORDERED LONGEST-FRAGMENT-FIRST, and the ordering is load-bearing: the lookup
/// is a substring match, so `deepseek-v4-flash-vision-exp` must be tested
/// BEFORE `deepseek-v4-flash` or it inherits the gated row's verdict and loses
/// an arm its hosts agree on. `open_weights_rows_are_longest_fragment_first`
/// asserts that structurally rather than trusting this sentence.
///
/// Spreads measured on the 2026-08-30 models.dev pull (`host_spread` over every
/// provider, vendor and third-party alike); control: `claude-opus-5` is
/// 1,000,000 -> 1,000,000 across 31 hosts, i.e. 1.0x, and is not an
/// open-weights id at all.
const OPEN_WEIGHTS_HOST_SPREAD: &[(&str, Option<&str>)] = &[
    // --- hosts AGREE: the arm stays keyed on the id alone. ---
    ("deepseek-v4-flash-vision-exp", None), // 1.05x over 12 hosts
    ("deepseek-v4-pro-0813", None),         // 1.05x over 27 hosts
    ("minimax-m2.5-highspeed", None),       // 1.00x over 11 hosts
    ("minimax-m2.7-highspeed", None),       // 1.02x over 15 hosts
    ("minimax-m2.7", None),                 // 1.30x over 38 hosts
    // --- hosts DISAGREE: vendor-operated providers only. ---
    ("deepseek-v4-flash-0731", Some("deepseek")), // 5.1x over 35 hosts
    ("deepseek-v4-flash", Some("deepseek")),      // 8.0x over 61 hosts
    ("deepseek-v4-pro", Some("deepseek")),        // 8.2x over 64 hosts
    ("minimax-m2.1", Some("minimax")),            // 5.1x over 24 hosts
    ("minimax-m2.5", Some("minimax")),            // 3.5x over 44 hosts
    ("minimax-m3", Some("minimax")),              // 4.0x over 43 hosts
    ("minimax-m2", Some("minimax")),              // 5.1x over 19 hosts
];

/// The open-weights FAMILY `m` belongs to, when that family's hosts disagree
/// widely enough that a globally-keyed arm is forbidden. `None` for every other
/// id, including the open-weights ids whose hosts agree.
///
/// `m` must already be lowercased, as `model_output_ceiling` does.
pub(crate) fn host_variable_open_weights_family(m: &str) -> Option<&'static str> {
    OPEN_WEIGHTS_HOST_SPREAD
        .iter()
        .find(|(fragment, _)| m.contains(fragment))
        .and_then(|&(_, family)| family)
}

/// Every endpoint identity `family`'s vendor operates. Empty for a family that
/// is not in [`VENDOR_OPERATED_ENDPOINTS`], so an unknown family fails CLOSED
/// (`provider_operates` false everywhere, the arm resolves `None`, sizing errs
/// low) rather than open. `every_gated_family_has_vendor_endpoints` makes that
/// state unreachable in the first place.
pub(crate) fn vendor_operated_endpoints(family: &str) -> &'static [&'static str] {
    VENDOR_OPERATED_ENDPOINTS
        .iter()
        .find(|(f, _)| *f == family)
        .map_or(&[], |&(_, endpoints)| endpoints)
}

/// Whether `provider` is one of the identities the family's vendor operates.
///
/// Prefix-matched at a `-` boundary against EVERY member of the family's set,
/// so all of the vendor's tenant spellings keep the arm while a third-party
/// host whose id merely starts with (or contains) a vendor name does not.
fn provider_operates(provider: &str, family: &str) -> bool {
    let p = provider.to_ascii_lowercase();
    vendor_operated_endpoints(family).iter().any(|vendor| {
        p == *vendor
            || p.strip_prefix(vendor)
                .is_some_and(|rest| rest.starts_with('-'))
    })
}

/// Returns `(max_output_tokens, context_window)` for a known model, or `None`
/// when the model is unknown (caller must fail open).
///
/// `provider` disambiguates the open-weights families whose hosts disagree --
/// see [`OPEN_WEIGHTS_HOST_SPREAD`]. Every other family is served by its vendor
/// alone (or by resellers that republish the vendor's figures), so the model id
/// is distinctive enough to match on alone and `provider` is not consulted.
pub fn model_output_ceiling(provider: &str, model: &str) -> Option<(u32, u32)> {
    let m = model.to_ascii_lowercase();

    // FerroxLabs/wayland#1232 -- the provider-scoped gate, ahead of EVERY arm
    // below so no later arm can hand one of these figures to a host that does
    // not serve it. `None` here is the honest answer, not a fallback: the
    // caller then fails open exactly as it does for any unlisted model.
    if let Some(family) = host_variable_open_weights_family(&m)
        && !provider_operates(provider, family)
    {
        return None;
    }

    // --- Anthropic Claude (4.x/5 era; older 3.x deliberately excluded) ---
    // The 1M-context generation (Opus 4.6/4.7/4.8, Opus 5, Sonnet 4.6, Sonnet 5,
    // Fable 5)
    // serves the full 1,000,000-token window and 128k output BY DEFAULT — no
    // beta header, no long-context premium (verified against docs.anthropic.com,
    // 2026-07-04: "Opus 4.8 serves the full 1M context window by default with no
    // beta header"; the older `context-1m-2025-08-07` beta is retired). Earlier
    // 4.x (Opus 4.0/4.1/4.5, Sonnet 4.0/4.5, Haiku 4.5) stay at 200k. Cross-
    // checked against models.dev (2026-07-04). Match newest-first so a 4.8 id
    // never falls through to the 200k arm.
    // `opus-5` joins the 1M arm (models.dev, 2026-08-28). It had NO arm, and a
    // missing arm is NOT the 200k default: `known_context_window` returns None,
    // compaction substitutes `UNVERIFIED_CONTEXT_WINDOW` (32,768) and the
    // non-omit-safe `anthropic_defaults()` preset clamps output to `UNKNOWN_CAP`
    // (8,192) — a 30x undersize on Anthropic's current flagship. The three
    // vendor-operated providers agree exactly: `anthropic` claude-opus-5,
    // `google-vertex` claude-opus-5@default and `amazon-bedrock`
    // anthropic.claude-opus-5 all report 1,000,000 / 128,000. The `opus-5`
    // fragment also covers `claude-opus-5-fast`, which every provider that
    // serves it lists at the same figures.
    if m.contains("opus-4-6")
        || m.contains("opus-4-7")
        || m.contains("opus-4-8")
        || m.contains("opus-5")
    {
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
    // The MAY-2024 gpt-4o snapshot is the one dated id that is NOT 16,384: its
    // real output cap is 4,096 (models.dev 2026-08-28 — vendor row `openai`
    // gpt-4o-2024-05-13 out=4096, plus kilo / merge-gateway / openrouter /
    // orcarouter, all 4,096). This is the only OVER-claim in this table that
    // bites at default settings: `size_output_cap` sends
    // min(config_max 64_000, ceiling, room), so the generic arm put 16,384 on
    // the wire against a 4,096 cap — a hard 400 mid-run, not a truncation. Must
    // be matched BEFORE the generic arm. The sibling snapshots `-2024-08-06`
    // and `-2024-11-20` are genuinely 16,384 and do not contain this fragment.
    if m.contains("gpt-4o-2024-05-13") {
        return Some((4_096, 128_000));
    }
    if m.contains("gpt-4o") {
        return Some((16_384, 128_000));
    }

    // --- OpenAI GPT-6 ---
    // models.dev vendor row `openai`, 2026-09-04: gpt-6-astra serves a
    // 1,050,000 window with a 128,000 output cap, and the -pro / -fast
    // siblings report the identical pair on every endpoint that carries
    // them, so one substring covers all three at figures none of them
    // disagree with.
    //
    // Matched on `gpt-6-astra`, NOT a bare `gpt-6`: the 5.x family SPLITS
    // (-mini / -nano / -codex stay at 400k), so a family-wide claim would
    // over-claim for siblings nobody has seen yet -- the same trap the
    // gpt-5.4 arm below guards against. An unknown gpt-6 id keeps the
    // CompactConfig default until models.dev shows its real figures.
    if m.contains("gpt-6-astra") {
        return Some((128_000, 1_050_000));
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
    // OUTPUT CAVEAT (4.5 / 4.6): the 500,000 output figure below is UNSOURCED.
    // The only vendor-operated row for these ids (models.dev provider `xai`,
    // re-pulled 2026-08-28) is DEGENERATE — out == ctx == 500,000 — which by
    // this file's own rule means the vendor published "unknown", never a
    // ceiling. `amazon-bedrock` (xai.grok-4.6) repeats the same degenerate pair,
    // so it is not an independent source. No vendor-operated, non-degenerate
    // output figure exists for grok-4.5/4.6, and the reseller rows disagree
    // wildly (github-copilot 128,000; kilo / openrouter 450,000; ofox 65,536;
    // abacus 32,768), so no defensible replacement is available and none has
    // been invented. The value is left as-is deliberately: an arm REVOKES
    // `should_omit_max_tokens`, so lowering it to a guess would be the first
    // thing ever to cut xAI's natural ceiling. The CONTEXT 500,000 is
    // well-supported (every non-junk row agrees) and is not in doubt.
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

    // --- Google Gemini rolling `-latest` aliases ---
    // Added 2026-08-28. `gemini-flash-latest` / `gemini-flash-lite-latest` carry
    // no version fragment, so they matched neither the 2.5 nor the 3.x arm and
    // had no entry at all — 32,768 substituted for a 1,048,576 window, a 32x
    // undersize. Both Google-operated providers agree exactly (`google` and
    // `google-vertex`, models.dev 2026-08-28): 1,048,576 context / 65,536 output.
    //
    // Adding the arm REVOKES `should_omit_max_tokens` on the omit-safe gemini
    // preset, so 65,536 becomes an enforced cap — but it IS the vendor ceiling,
    // so nothing truncates. Matched on the full alias (not a bare `-latest`) so
    // no other rolling id is caught.
    if m.contains("gemini-flash-latest") || m.contains("gemini-flash-lite-latest") {
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
    // (M2.1 / M2.5 / M2.7) = 204,800. The base-M2 arm below still returns
    // 196,608, but its ORIGINAL premise is stale: re-pulled 2026-08-28, the
    // vendor (`minimax`, and its `minimax-cn` / `minimax-coding-plan` tenants)
    // reports 204,800 for the BASE MiniMax-M2 as well, and 196,608 now survives
    // only on `alibaba-coding-plan` / `alibaba-token-plan` rows for a DIFFERENT
    // model (M2.5). So the "distinct, smaller window" claim is wrong — the entry
    // is a harmless 4% UNDER-claim, not the 400-avoidance it was documented as.
    // Match order is longest-substring-first so a point release never falls
    // through to the base arm. Output held conservatively (err LOW per header).
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

    /// #1176 DRIFT GUARD -- the other half of #165, and the half both guards
    /// were blind to.
    ///
    /// `every_routed_catalog_model_has_a_known_window` walks ROUTED ALIASES
    /// only, and the release-time freshness script grades only the ordered
    /// `CATALOGUE_CEILINGS` table -- it says in its own output that the
    /// `if`-chain families above cannot be evaluated by a text parser. An id
    /// that is in an `if` chain AND reaches users through provider-native
    /// `--model` passthrough was therefore graded by nothing but a hand check,
    /// and the hand check is what found `claude-opus-5` (no arm at all: 32,768
    /// substituted for a 1,000,000 window), `gpt-4o-2024-05-13` (output
    /// over-claimed 4x into a hard 400) and the `gemini-*-latest` aliases (32x
    /// undersized).
    ///
    /// This walks the vendor catalogue instead. It is the half a text parser
    /// could never do -- the chain evaluates ITSELF -- and it runs on every
    /// PR, not once per release.
    #[test]
    fn every_passthrough_vendor_model_resolves_its_arm() {
        use super::passthrough::PASSTHROUGH_VENDOR_MODELS;

        let mut wrong = Vec::new();
        for &(id, want_out, want_ctx) in PASSTHROUGH_VENDOR_MODELS {
            // #1232 -- the host-variable open-weights ids are provider-scoped,
            // so probe them under EVERY identity the family's vendor operates,
            // not just one. Probing a single name is what let the first cut of
            // the scoping pass while `qwen` and the four `alibaba-*` tenants --
            // DeepSeek V4's own first-party endpoints -- all resolved `None`.
            let probes: Vec<&str> =
                match super::host_variable_open_weights_family(&id.to_ascii_lowercase()) {
                    Some(family) => super::vendor_operated_endpoints(family).to_vec(),
                    None => vec!["passthrough"],
                };
            assert!(
                !probes.is_empty(),
                "{id} is gated to a family with no vendor endpoints, so its arm \
                 is dead on every route"
            );
            for probe in probes {
                match model_output_ceiling(probe, id) {
                    Some((out, ctx)) if (out, ctx) == (want_out, want_ctx) => {}
                    Some((out, ctx)) => wrong.push(format!(
                        "{id} on `{probe}`: the chain resolves output {out} / \
                         context {ctx}, but the vendor figure is output \
                         {want_out} / context {want_ctx}"
                    )),
                    None => wrong.push(format!(
                        "{id} on `{probe}`: NO ARM AT ALL. A missing arm is not a \
                         safe default -- `known_context_window` substitutes \
                         UNVERIFIED_CONTEXT_WINDOW (32,768) and a non-omit-safe \
                         preset clamps output to UNKNOWN_CAP (8,192). Expected \
                         output {want_out} / context {want_ctx}"
                    )),
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "these provider-native passthrough ids no longer resolve their \
             verified vendor limits (#1176). Add or correct the arm in \
             `model_output_ceiling`, or -- if the VENDOR changed the figure -- \
             update `limits/passthrough.rs` and say which vendor rows you \
             checked:\n  {}",
            wrong.join("\n  ")
        );
    }

    /// #1232 -- the lookup is a substring match, so the ORDER of
    /// `OPEN_WEIGHTS_HOST_SPREAD` is semantics, not tidiness: a fragment that
    /// contains another must be tested first or the longer id can never reach
    /// its own row and silently inherits the shorter one's verdict. Asserted
    /// structurally so the claim in that const's doc cannot quietly go false --
    /// this is the exact shape that let `deepseek-v4-flash-vision-exp` be
    /// mistaken for `deepseek-v4-flash` in the first draft of the gate.
    #[test]
    fn open_weights_rows_are_longest_fragment_first() {
        let rows = super::OPEN_WEIGHTS_HOST_SPREAD;
        let mut shadowed = Vec::new();
        for (i, (long, _)) in rows.iter().enumerate() {
            for (j, (short, _)) in rows.iter().enumerate() {
                if i == j || !long.contains(short) {
                    continue;
                }
                if i > j {
                    shadowed.push(format!(
                        "`{long}` contains `{short}` but is listed after it, so \
                         `{long}` can never reach its own row"
                    ));
                }
            }
        }
        assert!(shadowed.is_empty(), "{}", shadowed.join("\n  "));
    }

    /// #1232 -- the seven open-weights ids whose hosts disagree resolve their
    /// vendor's figures ON THE VENDOR'S OWN ENDPOINT and nothing anywhere else.
    ///
    /// This is the test the issue's third acceptance box asks for: it fails if
    /// a later change re-globalises one of the seven (the third-party arm
    /// stops being `None`) AND it fails if a later change deletes one outright
    /// (the vendor arm stops resolving). Both directions matter, because
    /// deletion is the obvious reading of AGENTS.md rule 3 and it is the wrong
    /// one here -- DeepSeek's and MiniMax's own presets are not omit-safe, so a
    /// deleted arm gives their users `UNKNOWN_CAP` (8,192), not a natural
    /// ceiling.
    #[test]
    fn host_variable_open_weights_arms_are_provider_scoped() {
        use super::passthrough::PASSTHROUGH_VENDOR_MODELS;
        use std::collections::BTreeMap;

        let vendor_figures: BTreeMap<&str, (u32, u32)> = PASSTHROUGH_VENDOR_MODELS
            .iter()
            .map(|&(id, out, ctx)| (id, (out, ctx)))
            .collect();

        let gated: Vec<&str> = super::OPEN_WEIGHTS_HOST_SPREAD
            .iter()
            .filter(|(_, vendor)| vendor.is_some())
            .map(|&(fragment, _)| fragment)
            .collect();
        let ungated: Vec<&str> = super::OPEN_WEIGHTS_HOST_SPREAD
            .iter()
            .filter(|(_, vendor)| vendor.is_none())
            .map(|&(fragment, _)| fragment)
            .collect();

        // NON-VACUITY. Both arms of this test loop over a list; an empty list
        // asserts nothing, and a gate that scoped EVERYTHING would pass the
        // first loop while destroying the five rows the second loop protects.
        assert_eq!(
            gated.len(),
            7,
            "the 2026-08-30 pull measured SEVEN rule-3 violations: {gated:?}"
        );
        assert_eq!(
            ungated.len(),
            5,
            "five open-weights rows have hosts that AGREE and must stay \
             globally keyed: {ungated:?}"
        );

        // Third-party shapes: two omit-safe reseller routes (where `None`
        // restores `should_omit_max_tokens` and the host's own ceiling), and
        // three non-omit-safe ones (where `None` errs low instead of high).
        const THIRD_PARTY: [&str; 5] = [
            "openrouter",
            "flux-router",
            "openai-compat",
            "nebius",
            "deepinfra",
        ];

        for id in &gated {
            let family = super::host_variable_open_weights_family(id)
                .expect("a gated row reports its family");
            let endpoints = super::vendor_operated_endpoints(family);
            assert!(
                !endpoints.is_empty(),
                "{id} names family `{family}`, which has no VENDOR_OPERATED_ENDPOINTS \
                 entry -- the arm would be dead on every route including the vendor's"
            );
            let want = *vendor_figures
                .get(id)
                .unwrap_or_else(|| panic!("{id} must have a PASSTHROUGH_VENDOR_MODELS row"));
            for vendor in endpoints {
                assert_eq!(
                    model_output_ceiling(vendor, id),
                    Some(want),
                    "{id}: `{vendor}` is a {family} vendor endpoint and must keep \
                     its verified figures -- losing the arm hands its users \
                     UNKNOWN_CAP (8,192), because that preset is not omit-safe"
                );
                // Tenant spellings the vendor adds without asking us. These are
                // exactly the rows the single-name key dropped, so they are
                // asserted rather than trusted to the prefix's doc comment.
                for suffix in ["-cn", "-coding-plan", "-token-plan-cn"] {
                    let tenant = format!("{vendor}{suffix}");
                    assert_eq!(
                        model_output_ceiling(&tenant, id),
                        Some(want),
                        "{id}: `{tenant}` is a {family} vendor tenant and must keep \
                         the arm -- the `-` boundary match exists for this case"
                    );
                }
            }
            for host in THIRD_PARTY {
                assert_eq!(
                    model_output_ceiling(host, id),
                    None,
                    "{id} resolved an arm on `{host}`, which does not operate \
                     it. AGENTS.md rule 3: this id is served from {} to {} \
                     across dozens of endpoints, so one static figure reaches \
                     hosts that serve neither",
                    "its floor",
                    "its ceiling"
                );
            }
        }

        // CONTROL 1 -- the five whose hosts AGREE are untouched on every
        // provider. Without this the test would pass just as well against a
        // gate that scoped the whole family, which is the over-correction
        // AGENTS.md's rule warns about in its second half.
        for id in &ungated {
            let want = *vendor_figures
                .get(id)
                .unwrap_or_else(|| panic!("{id} must have a PASSTHROUGH_VENDOR_MODELS row"));
            for host in [
                "deepseek",
                "minimax",
                "openrouter",
                "openai-compat",
                "nebius",
            ] {
                assert_eq!(
                    model_output_ceiling(host, id),
                    Some(want),
                    "{id} lost its arm on `{host}`, but its hosts agree within \
                     1.3x -- gating it replaces a real ceiling with a low guess"
                );
            }
        }

        // CONTROL 2 -- a vendor-only id is outside this gate entirely, on any
        // provider string. Proves the gate is keyed on the id, not on the
        // provider argument suddenly mattering everywhere.
        for host in ["anthropic", "openrouter", "nebius", "passthrough"] {
            assert_eq!(
                model_output_ceiling(host, "claude-opus-5"),
                Some((128_000, 1_000_000)),
                "claude-opus-5 is not open-weights; `{host}` must not change it"
            );
        }

        // CONTROL 3 -- vendor tenants keep the arm; a host whose NAME merely
        // contains the vendor's does not. `provider_operates` is a prefix match
        // for exactly this reason.
        assert_eq!(
            model_output_ceiling("minimax-coding-plan", "minimax-m2.5"),
            Some((128_000, 204_800)),
            "the vendor's own tenant rows publish the vendor's figures"
        );
        assert_eq!(
            model_output_ceiling("minimaxproxy", "minimax-m2.5"),
            None,
            "a reseller whose id merely starts with the vendor's name must not \
             inherit the vendor's ceiling"
        );
    }

    /// #1232 -- a gated row may not name a family that has no endpoint set.
    ///
    /// This is the state that makes an arm dead on EVERY route, vendor
    /// included: `vendor_operated_endpoints` returns `&[]`, `provider_operates`
    /// is false for every provider string, and the id falls to UNKNOWN_CAP
    /// (8,192) / UNVERIFIED_CONTEXT_WINDOW (32,768) for everybody. Failing
    /// closed at runtime is the right default; making the state unreachable is
    /// better, and that is what this test does.
    #[test]
    fn every_gated_family_has_vendor_endpoints() {
        let mut dangling = Vec::new();
        let mut used: Vec<&str> = Vec::new();
        for &(fragment, family) in super::OPEN_WEIGHTS_HOST_SPREAD {
            let Some(family) = family else { continue };
            used.push(family);
            let endpoints = super::vendor_operated_endpoints(family);
            if endpoints.is_empty() {
                dangling.push(format!(
                    "`{fragment}` is gated to family `{family}`, which has no \
                     VENDOR_OPERATED_ENDPOINTS entry"
                ));
            }
            for e in endpoints {
                assert!(
                    !e.is_empty() && *e == e.to_ascii_lowercase() && !e.ends_with('-'),
                    "{family}: `{e}` is not a usable provider-identity prefix \
                     (`provider_type()` is compared lowercased and the `-` \
                     boundary is appended by the matcher)"
                );
            }
        }
        assert!(dangling.is_empty(), "{}", dangling.join("\n  "));

        // NON-VACUITY, both directions: the loop above asserts nothing over an
        // empty table, and a family nobody references is dead weight that will
        // be trusted by the next reader.
        assert_eq!(
            used.len(),
            7,
            "the seven gated rows must each name a family: {used:?}"
        );
        for &(family, _) in super::VENDOR_OPERATED_ENDPOINTS {
            assert!(
                used.contains(&family),
                "VENDOR_OPERATED_ENDPOINTS names `{family}`, which no gated row \
                 uses -- delete it or gate the row that needs it"
            );
        }
    }

    /// Every endpoint name `family`'s vendor is known to answer on, derived
    /// from `providers.toml` base-URL hosts plus the family key itself -- and
    /// never from [`VENDOR_OPERATED_ENDPOINTS`], so a test built on this can
    /// still fail when that const is wrong.
    fn vendor_endpoint_names(family: &str) -> Vec<String> {
        use crate::catalog::ProviderCatalog;

        let catalog = ProviderCatalog::bundled().expect("the bundled catalog parses");
        let domains = super::VENDOR_API_DOMAINS
            .iter()
            .find(|(f, _)| *f == family)
            .map_or(&[] as &[&str], |&(_, d)| d);
        let mut names = vec![family.to_string()];
        for e in &catalog.providers {
            let host = e
                .base_url
                .split_once("://")
                .map_or(e.base_url.as_str(), |(_, rest)| rest)
                .split('/')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if domains
                .iter()
                .any(|d| host == *d || host.ends_with(&format!(".{d}")))
                && !names.contains(&e.id)
            {
                names.push(e.id.clone());
            }
        }
        names
    }

    /// #1232 -- the identity axis a catalog-shaped test cannot see: a builtin
    /// provider ALIAS that resolves onto a DIFFERENT canonical slug.
    ///
    /// `--provider alibaba` is the default DashScope route, and it never
    /// reaches `model_output_ceiling` spelled `alibaba` at all:
    /// `parse_builtin_provider` folds `qwen | alibaba | dashscope` onto
    /// `ProviderType::Qwen`, whose slug is `qwen`, and builtins resolve BEFORE
    /// the bundled catalog. `vendor_operated_catalog_endpoints_keep_their_arm`
    /// grades catalog ids, so it is structurally blind to this route -- and
    /// this is the N+1: fixing the catalog ids alone would still have left the
    /// commonest first-party DeepSeek route on `UNKNOWN_CAP`.
    ///
    /// The oracle is the product's OWN resolver, not a list: every vendor
    /// endpoint name is pushed through `parse_builtin_provider` ->
    /// `provider_type_slug` and the resulting slug must keep the arm. A new
    /// alias added to the resolver is graded the day it lands.
    #[test]
    fn builtin_alias_routes_to_a_vendor_endpoint_keep_their_arm() {
        let gated: Vec<(&str, &str)> = super::OPEN_WEIGHTS_HOST_SPREAD
            .iter()
            .filter_map(|&(frag, family)| family.map(|f| (frag, f)))
            .collect();
        assert_eq!(gated.len(), 7, "the gated set changed shape: {gated:?}");

        let mut respelled: Vec<String> = Vec::new();
        let mut checked = 0usize;
        for family in ["deepseek", "minimax"] {
            for name in vendor_endpoint_names(family) {
                let Some(kind) = crate::config::parse_builtin_provider(&name) else {
                    continue;
                };
                let slug = crate::config::provider_type_slug(kind);
                if slug != name {
                    respelled.push(format!("{name} -> {slug}"));
                }
                for &(fragment, gated_family) in &gated {
                    if gated_family != family {
                        continue;
                    }
                    checked += 1;
                    assert!(
                        model_output_ceiling(slug, fragment).is_some(),
                        "`--provider {name}` resolves to the builtin slug \
                         `{slug}`, which is the ONLY string \
                         `model_output_ceiling` ever sees on that route -- and \
                         `{fragment}` resolves NO arm there, so a first-party \
                         user gets UNKNOWN_CAP (8,192) output and a 32,768 \
                         window. Add `{slug}` to VENDOR_OPERATED_ENDPOINTS."
                    );
                }
            }
        }

        // NON-VACUITY. Without a RE-SPELLING this test is just a slower copy of
        // the catalog one: the whole class it exists for is the alias whose
        // slug differs from the endpoint's own name.
        assert!(
            !respelled.is_empty(),
            "no vendor endpoint name was re-spelled by `parse_builtin_provider`, \
             so this test graded nothing it exists for (`alibaba` -> `qwen` is \
             the route that broke)"
        );
        assert!(
            checked >= 7,
            "only {checked} (endpoint, id) pairs graded, and {respelled:?} \
             re-spelled -- the derivation stopped finding the vendor's routes"
        );
    }

    /// #1232 -- the check that does NOT read `VENDOR_OPERATED_ENDPOINTS` for
    /// its answer, and the reason the first cut of the scoping shipped.
    ///
    /// The verifier's point stands and is the whole design of this test: a test
    /// whose "correct" provider is read from the artefact under test cannot
    /// discover that the artefact names the wrong provider. So the oracle here
    /// is `providers.toml` -- a different file, curated from models.dev by the
    /// catalog process -- and specifically its `base_url` host. An endpoint
    /// served from the vendor's own DNS is the vendor's endpoint, whatever it
    /// is branded: that is how `alibaba-cn`, `alibaba-token-plan`,
    /// `alibaba-coding-plan` and `alibaba-coding-plan-cn` are identified here
    /// as DeepSeek V4 first-party routes without `deepseek` appearing anywhere
    /// in their ids.
    ///
    /// Run against the commit that introduced the scoping, this test fails on
    /// all five Alibaba rows.
    #[test]
    fn vendor_operated_catalog_endpoints_keep_their_arm() {
        use crate::catalog::ProviderCatalog;
        use std::collections::BTreeMap;

        let catalog = ProviderCatalog::bundled().expect("the bundled catalog parses");
        let host_of = |base_url: &str| -> String {
            base_url
                .split_once("://")
                .map_or(base_url, |(_, rest)| rest)
                .split('/')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase()
        };

        // family -> the catalog ids served from that vendor's own DNS.
        let mut derived: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for &(family, domains) in super::VENDOR_API_DOMAINS {
            for e in &catalog.providers {
                let host = host_of(&e.base_url);
                if domains
                    .iter()
                    .any(|d| host == *d || host.ends_with(&format!(".{d}")))
                {
                    derived.entry(family).or_default().push(&e.id);
                }
            }
        }

        // NON-VACUITY. The derivation must actually find the rows that broke;
        // if the catalog drops them this test must fail loudly rather than
        // quietly assert nothing. `minimax*` is deliberately NOT in the catalog
        // (it speaks the Anthropic wire, see the file header), so its endpoint
        // identities come from `ProviderType::MiniMax` and are covered by
        // `host_variable_open_weights_arms_are_provider_scoped` instead.
        let deepseek_ids = derived.get("deepseek").cloned().unwrap_or_default();
        for must in [
            "deepseek",
            "alibaba",
            "alibaba-cn",
            "alibaba-coding-plan",
            "alibaba-coding-plan-cn",
            "alibaba-token-plan",
        ] {
            assert!(
                deepseek_ids.contains(&must),
                "providers.toml no longer yields `{must}` as a DeepSeek-family \
                 vendor endpoint, so this test has stopped grading the rows it \
                 exists for. Derived: {deepseek_ids:?}"
            );
        }

        let mut lost = Vec::new();
        for &(fragment, family) in super::OPEN_WEIGHTS_HOST_SPREAD {
            let Some(family) = family else { continue };
            for id in derived.get(family).into_iter().flatten() {
                if model_output_ceiling(id, fragment).is_none() {
                    lost.push(format!(
                        "`{id}` serves {family} from the vendor's own DNS \
                         ({}), but `{fragment}` resolves NO arm there -- that \
                         endpoint's users get UNKNOWN_CAP (8,192) and a 32,768 \
                         window, the #1157 cut",
                        catalog.get(id).map(|e| e.base_url.as_str()).unwrap_or("?")
                    ));
                }
            }
        }
        assert!(
            lost.is_empty(),
            "vendor-operated endpoints lost their arm -- add the missing \
             identity prefix to VENDOR_OPERATED_ENDPOINTS:\n  {}",
            lost.join("\n  ")
        );

        // CONTROL -- a catalog id that is NOT on a vendor domain must still
        // resolve `None`, or this test would pass just as well against a table
        // with no scoping at all.
        let third_party: Vec<&str> = catalog
            .providers
            .iter()
            .map(|e| e.id.as_str())
            .filter(|id| !deepseek_ids.contains(id))
            .collect();
        assert!(
            third_party.len() > 50,
            "control set is too small to mean anything"
        );
        for id in third_party {
            assert_eq!(
                model_output_ceiling(id, "deepseek-v4-pro"),
                None,
                "`{id}` is not served from a DeepSeek-family vendor domain but \
                 resolved the vendor's arm"
            );
        }
    }

    /// The table records one canonical spelling per model because the lookup
    /// is a substring match. That claim is load-bearing -- it is why 83 rows
    /// cover the ~160 provider-specific spellings models.dev publishes -- so
    /// it is asserted, not assumed.
    #[test]
    fn provider_specific_spellings_resolve_through_the_same_arm() {
        for (canonical, dressed) in [
            ("claude-opus-5", "us.anthropic.claude-opus-5"),
            ("claude-opus-5", "claude-opus-5@default"),
            (
                "claude-sonnet-4-5-20250929",
                "anthropic.claude-sonnet-4-5-20250929-v1:0",
            ),
            ("claude-opus-4-6", "eu.anthropic.claude-opus-4-6-v1"),
            ("grok-4.3", "xai.grok-4.3"),
            ("gemini-flash-latest", "google/gemini-flash-latest"),
        ] {
            assert_eq!(
                model_output_ceiling("passthrough", canonical),
                model_output_ceiling("passthrough", dressed),
                "{dressed} must resolve through the same arm as {canonical}"
            );
            assert!(
                model_output_ceiling("passthrough", dressed).is_some(),
                "{dressed} must resolve at all"
            );
        }
    }

    /// A shadowed or duplicated row would make the release script's
    /// containment lookup disagree with the chain, which is the drift this
    /// whole guard exists to prevent.
    #[test]
    fn passthrough_table_is_populated_and_free_of_duplicates() {
        use super::passthrough::PASSTHROUGH_VENDOR_MODELS;
        assert!(
            PASSTHROUGH_VENDOR_MODELS.len() >= 50,
            "the table parsed to {} rows -- a table this small is not covering \
             the vendor catalogue",
            PASSTHROUGH_VENDOR_MODELS.len()
        );
        let mut seen = std::collections::BTreeSet::new();
        for &(id, _, _) in PASSTHROUGH_VENDOR_MODELS {
            assert!(
                id.chars().all(|c| c.is_ascii_lowercase()
                    || c.is_ascii_digit()
                    || matches!(c, '.' | '-' | '_' | '@' | ':' | '/')),
                "{id} is not a lowercased model id"
            );
            assert!(seen.insert(id), "{id} appears twice in the table");
        }
    }

    /// The PREMISE, asserted rather than asserted-about: these ids really are
    /// invisible to the routed-alias drift guard. If the routing catalogue
    /// ever gains one, this test says so and the row moves.
    #[test]
    fn the_passthrough_table_covers_ids_the_routed_guard_cannot_see() {
        use super::passthrough::PASSTHROUGH_VENDOR_MODELS;
        use wcore_types::model_aliases::{known_providers, models_for_provider};

        let mut routed = Vec::new();
        for provider in known_providers() {
            for (_alias, model_id) in models_for_provider(provider) {
                routed.push(model_id.to_ascii_lowercase());
            }
        }
        let unseen: Vec<&str> = PASSTHROUGH_VENDOR_MODELS
            .iter()
            .map(|&(id, _, _)| id)
            .filter(|id| !routed.iter().any(|r| r.contains(*id)))
            .collect();

        // The three ids the hand check caught last cycle. Each must be in the
        // passthrough table AND absent from the routed catalogue -- that
        // combination is precisely the gap #1176 reports.
        for id in ["claude-opus-5", "gpt-4o-2024-05-13", "gemini-flash-latest"] {
            assert!(
                PASSTHROUGH_VENDOR_MODELS.iter().any(|&(t, _, _)| t == id),
                "{id} cost a real defect last cycle and must stay in the table"
            );
            assert!(
                unseen.contains(&id),
                "{id} is now in the routed catalogue too -- good, but move it \
                 out of this list so the premise stays honest"
            );
        }
        assert!(
            unseen.len() >= 20,
            "only {} passthrough ids are outside the routed catalogue; if the \
             two catalogues have converged, say so here rather than leaving a \
             guard that grades nothing new",
            unseen.len()
        );
    }

    #[test]
    fn claude_opus_5_resolves_to_the_one_million_window() {
        // Anthropic's current flagship. With NO arm it fell through every
        // Claude branch to `None`, which is NOT the 200k default: the compaction
        // kernel substitutes `UNVERIFIED_CONTEXT_WINDOW` (32,768) and the
        // non-omit-safe `anthropic_defaults()` preset clamps output to
        // `UNKNOWN_CAP` (8,192) — a 30x undersize on a 1M model.
        //
        // Vendor-operated rows agree unanimously (models.dev, 2026-08-28):
        //   anthropic        claude-opus-5              ctx=1000000 out=128000
        //   google-vertex    claude-opus-5@default      ctx=1000000 out=128000
        //   amazon-bedrock   anthropic.claude-opus-5    ctx=1000000 out=128000
        for id in [
            "claude-opus-5",
            "claude-opus-5-fast",
            "anthropic.claude-opus-5",
            "claude-opus-5@default",
        ] {
            assert_eq!(
                model_output_ceiling("anthropic", id),
                Some((128_000, 1_000_000)),
                "{id} must report the 1,000,000-token window / 128k output"
            );
        }
        // Case-insensitive, consistent with the rest of the table.
        assert_eq!(
            model_output_ceiling("anthropic", "Claude-Opus-5"),
            Some((128_000, 1_000_000))
        );
    }

    #[test]
    fn gpt_4o_may_2024_snapshot_caps_output_at_4096() {
        // The ONLY over-claim in this table that bites at default settings:
        // `size_output_cap` computes min(config_max 64_000, ceiling, room), so
        // the generic gpt-4o arm put 16_384 on the wire against a real 4_096
        // cap — a hard HTTP 400 mid-run, not a truncation.
        //
        // models.dev 2026-08-28, vendor row `openai`:
        //   gpt-4o-2024-05-13  ctx=128000 out=4096
        //   gpt-4o-2024-08-06  ctx=128000 out=16384
        //   gpt-4o-2024-11-20  ctx=128000 out=16384
        assert_eq!(
            model_output_ceiling("openai", "gpt-4o-2024-05-13"),
            Some((4_096, 128_000))
        );
        assert_eq!(
            model_output_ceiling("openai", "openai/gpt-4o-2024-05-13"),
            Some((4_096, 128_000))
        );
        assert_eq!(
            model_output_ceiling("openai", "GPT-4o-2024-05-13"),
            Some((4_096, 128_000))
        );
    }

    #[test]
    fn gpt_4o_sibling_snapshots_keep_the_16k_output() {
        // The narrow May-2024 carve-out must not catch the siblings, which are
        // GENUINELY 16,384 (vendor row `openai`, 2026-08-28) — clamping them to
        // 4,096 would cut real output 4x.
        for id in [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4o-2024-08-06",
            "gpt-4o-2024-11-20",
            "gpt-4o-mini-2024-07-18",
        ] {
            assert_eq!(
                model_output_ceiling("openai", id),
                Some((16_384, 128_000)),
                "{id} must keep the 16,384 output cap"
            );
        }
    }

    #[test]
    fn gemini_latest_aliases_resolve_the_full_window() {
        // The rolling `-latest` aliases match neither the `gemini-2.5-*` nor the
        // `gemini-3*` arm, so they had no entry at all and fell to
        // `UNVERIFIED_CONTEXT_WINDOW` (32,768) — a 32x undersize.
        //
        // Both Google-operated providers agree exactly (models.dev, 2026-08-28):
        //   google         gemini-flash-latest       ctx=1048576 out=65536
        //   google         gemini-flash-lite-latest  ctx=1048576 out=65536
        //   google-vertex  gemini-flash-latest       ctx=1048576 out=65536
        //   google-vertex  gemini-flash-lite-latest  ctx=1048576 out=65536
        //
        // 65,536 IS the vendor ceiling, so revoking output omission on the
        // omit-safe gemini preset truncates nothing.
        for id in [
            "gemini-flash-latest",
            "gemini-flash-lite-latest",
            "google/gemini-flash-latest",
        ] {
            assert_eq!(
                model_output_ceiling("google", id),
                Some((65_536, 1_048_576)),
                "{id} must report the 1,048,576-token window / 65,536 output"
            );
        }
    }
}
