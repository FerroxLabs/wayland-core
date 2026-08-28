//! The ordered catalogue-refresh ceiling table (GLM / Qwen / Kimi / Mistral /
//! Llama), split out of `limits.rs` when that file crossed 1000 lines.
//!
//! Extraction only -- the entries, their ORDER, and their figures are
//! byte-identical to what `limits.rs` carried. The parent module still owns the
//! version-aware `if` chain and consults this table only after that chain
//! declines, so the two lookups keep the precedence they had.
//!
//! `scripts/check-model-limits-freshness.py` parses THIS file at release time
//! and fails closed if it cannot find the const, so the path is load-bearing;
//! move the table again and update the script's `DEFAULT_LIMITS` with it.

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
pub(super) const CATALOGUE_CEILINGS: &[(&str, u32, u32)] = &[
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
    //
    // 3.8-flash was MISSING and the release-time freshness gate refused the
    // v0.13.9 cut over it: served first-party by `alibaba-token-plan` at
    // 1,000,000 / 131,072, it matched no arm, fell to the `CompactConfig`
    // default and was silently mis-sized. It carries the same 128,000 as its
    // -max sibling rather than the vendor's 131,072 -- the same deliberate
    // few-percent under-claim, which the gate accepts by design.
    ("qwen3.8-max", 128_000, 1_000_000),
    ("qwen3.8-flash", 128_000, 1_000_000),
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

#[cfg(test)]
mod tests {
    use super::CATALOGUE_CEILINGS;
    use crate::limits::model_output_ceiling;

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

        // The id the release gate caught missing. Graded against the DEFAULT
        // it used to fall to, so this fails if the arm is dropped again
        // rather than only if its numbers change.
        assert_eq!(
            model_output_ceiling("alibaba", "qwen3.8-flash"),
            Some((128_000, 1_000_000)),
            "qwen3.8-flash must not inherit the CompactConfig default"
        );
        assert_ne!(
            model_output_ceiling("alibaba", "qwen3.8-flash"),
            model_output_ceiling("alibaba", "qwen3.7-flash"),
            "3.8-flash is a 128k-output tier; 3.7-flash is 64k"
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
}
