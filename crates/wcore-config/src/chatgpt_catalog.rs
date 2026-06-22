//! Conservative model-catalog filtering for the ChatGPT-subscription OAuth path
//! (`--provider openai-chatgpt`), addressing issue #158: a ChatGPT-subscription
//! login lists Codex models the account's plan tier cannot run, which then error
//! on use.
//!
//! # Cardinal constraint — never over-filter
//!
//! Hiding a model the user CAN run is strictly worse than the pre-#158 annoyance
//! of showing one they cannot. So this filter is a *conservative subtraction* of
//! models we can PROVE the plan tier cannot run — never a whitelist. Concretely:
//!
//! - When the plan tier is unknown / missing (`None`), nothing is filtered.
//! - When the plan tier is recognised but a model has no gating entry, the model
//!   is SHOWN.
//! - Only a model whose id appears in the gating table for the *specific*
//!   recognised plan tier is hidden.
//!
//! # Why the gating table is (almost) empty
//!
//! There is no authoritative entitled-model list anywhere in the OAuth token /
//! claims (the JWT exposes only `chatgpt_plan_type`, a free-form string such as
//! `"plus"` / `"pro"`), and the repository contains NO evidence grounding most
//! tier→model gating. Per the #158 brief, a guessed whitelist must not ship.
//!
//! The single grounded exception is the `-pro` model: `gpt-5.5-pro` needs a
//! ChatGPT **Pro** subscription, so it is hidden for the one recognised paid
//! tier we can prove cannot run it — `plus`. Every other tier (including
//! unknown / `None` / `free`) still SHOWS it, and the reactive error
//! (see [`is_model_available_for_plan`] callers in the provider) names the model
//! and the plan when the backend rejects a model the plan cannot run. So the
//! predictive hide here is a conservative single subtraction; the reactive
//! fallback catches everything else.
//!
//! This module lives in `wcore-config` (the data / compat layer) rather than
//! inline in provider code, per AGENTS.md "No Hardcoded Provider Quirks": the
//! tier→model gating is config-layer DATA, consulted by the provider, not a
//! scatter of `if model.contains(...)` conditionals.
//!
//! It deals in model-id strings only: `ModelInfo` lives in the higher
//! `wcore-providers` layer (config sits below it), so the provider maps its
//! `ModelInfo` catalog through [`is_model_available_for_plan`] and reuses the
//! [`decode_plan_type`] / [`filter_model_ids`] helpers here.

/// One conservative gating rule: a recognised plan tier and the model ids that
/// tier provably CANNOT run. The tier match is case-insensitive (the JWT claim
/// casing is not contractually guaranteed). A model id present here is removed
/// from the catalog *only* for an exact (case-insensitive) tier match.
///
/// Add an entry ONLY with evidence that the named tier cannot run the named
/// models. Absent evidence, leave the model unlisted (shown) — showing a
/// runnable model is the safe default; hiding one is the failure mode #158
/// forbids.
struct PlanGate {
    /// The `chatgpt_plan_type` claim value, lowercased (e.g. `"free"`).
    plan_tier: &'static str,
    /// Resolved model ids (as in `model_aliases`, e.g. `"gpt-5.5-pro"`) the
    /// tier cannot run.
    gated_model_ids: &'static [&'static str],
}

/// Conservative, evidence-grounded tier→unavailable-models table.
///
/// One grounded entry: `gpt-5.5-pro` requires ChatGPT **Pro**, so it is hidden
/// for the `plus` tier (the one recognised paid tier we can prove cannot run
/// it). It is deliberately NOT listed for `free` — for free/unknown/None we
/// still SHOW it and let the reactive backend error explain (the no-over-filter
/// rule: over-filtering is the cardinal sin). Add further entries ONLY with the
/// same kind of evidence; absent that, leave a model unlisted (shown) and rely
/// on the reactive fallback.
const PLAN_GATED_MODELS: &[PlanGate] = &[
    // The `-pro` Codex model needs a ChatGPT Pro subscription; `plus` cannot run
    // it. This is the one grounded gate — everything else is shown and the
    // reactive fallback (clear error from the provider) catches the rest.
    PlanGate {
        plan_tier: "plus",
        gated_model_ids: &["gpt-5.5-pro"],
    },
];

/// True iff `model_id` is available (runnable) on `plan_tier`.
///
/// This is the predicate the ChatGPT provider applies to each catalog entry.
/// Conservative: returns `true` (i.e. "show it") for a missing/unknown tier, an
/// unrecognised model, or any case the gating table does not explicitly cover.
/// Only an exact (case-insensitive) tier match WITH the model listed as gated
/// yields `false`.
pub fn is_model_available_for_plan(plan_tier: Option<&str>, model_id: &str) -> bool {
    let Some(plan_tier) = plan_tier else {
        // Unknown / missing plan → no information to filter on → show.
        return true;
    };
    let tier = plan_tier.trim().to_ascii_lowercase();
    let gated = PLAN_GATED_MODELS
        .iter()
        .filter(|g| g.plan_tier == tier)
        .any(|g| g.gated_model_ids.contains(&model_id));
    !gated
}

/// Filter a list of resolved model ids to those `plan_tier` can run.
///
/// `plan_tier` is the `chatgpt_plan_type` JWT claim (`None` when absent or
/// undecodable). Preserves order; removes only *provably unavailable* models for
/// a *recognised* tier. With the current [`PLAN_GATED_MODELS`] this removes only
/// `gpt-5.5-pro` for the `plus` tier; every other input is unchanged — see the
/// module docs.
///
/// Only the ChatGPT-OAuth-subscription provider must call this. The API-key
/// OpenAI path and all other providers MUST NOT — their catalogs are unaffected.
pub fn filter_model_ids<'a>(plan_tier: Option<&str>, model_ids: &'a [&'a str]) -> Vec<&'a str> {
    model_ids
        .iter()
        .copied()
        .filter(|id| is_model_available_for_plan(plan_tier, id))
        .collect()
}

/// Decode the `chatgpt_plan_type` claim from a ChatGPT OAuth access token (a
/// JWT), returning the plan tier string when present and decodable.
///
/// Pure data helper — no OAuth/network. It reads the JWT's second
/// (base64url, no-pad) segment and pulls
/// `["https://api.openai.com/auth"]["chatgpt_plan_type"]`, mirroring
/// `wcore_agent::oauth::chatgpt::decode_codex_claims`. Duplicated here (not
/// shared) deliberately: `wcore-providers` must not depend on `wcore-agent`
/// (layering), and this slice is a handful of lines. Returns `None` on any
/// malformed input so callers degrade to "show everything".
pub fn decode_plan_type(access_token: &str) -> Option<String> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let seg = access_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(seg).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("https://api.openai.com/auth")?
        .get("chatgpt_plan_type")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors the real `openai-chatgpt` alias catalog ids.
    const CATALOG: &[&str] = &[
        "gpt-5.5",
        "gpt-5.5-pro",
        "gpt-5.4",
        "gpt-5.4-codex",
        "gpt-5.3-codex",
        "gpt-5.3-codex-spark",
    ];

    #[test]
    fn unknown_plan_shows_full_catalog() {
        // None plan tier → no filtering, full catalog preserved in order.
        let out = filter_model_ids(None, CATALOG);
        assert_eq!(out, CATALOG, "missing plan must not filter");
    }

    #[test]
    fn unrecognised_plan_shows_full_catalog() {
        // A plan string we have no gating data for → show everything.
        let out = filter_model_ids(Some("enterprise"), CATALOG);
        assert_eq!(out, CATALOG, "unrecognised plan must not filter");
    }

    #[test]
    fn non_plus_paid_or_unknown_tiers_show_full_catalog() {
        // The ONLY grounded gate is `plus` → hide `gpt-5.5-pro`. Every other
        // tier — including the recognised Pro tiers and unknown strings — keeps
        // the full catalog (no over-filter).
        for plan in ["free", "pro", "team", "enterprise"] {
            let out = filter_model_ids(Some(plan), CATALOG);
            assert_eq!(out, CATALOG, "plan {plan} must keep the full catalog");
        }
    }

    #[test]
    fn plus_tier_hides_only_gpt_5_5_pro() {
        // The grounded gate: `gpt-5.5-pro` requires ChatGPT Pro, so `plus`
        // excludes it — but ALL other Codex models remain, in order.
        let out = filter_model_ids(Some("plus"), CATALOG);
        let expected: Vec<&str> = CATALOG
            .iter()
            .copied()
            .filter(|m| *m != "gpt-5.5-pro")
            .collect();
        assert_eq!(out, expected, "plus hides only gpt-5.5-pro");
        assert!(!out.contains(&"gpt-5.5-pro"));
        assert!(out.contains(&"gpt-5.5"));
    }

    #[test]
    fn gating_is_conservative_and_data_driven() {
        // The one gate: `plus` cannot run `gpt-5.5-pro`.
        assert!(
            !is_model_available_for_plan(Some("plus"), "gpt-5.5-pro"),
            "plus is gated off gpt-5.5-pro"
        );
        // Case-insensitive tier match — `PLUS` is the same gate.
        assert!(!is_model_available_for_plan(Some("PLUS"), "gpt-5.5-pro"));
        // free / unknown / None still SHOW gpt-5.5-pro (reactive fallback covers
        // them — over-filtering is the cardinal sin).
        assert!(is_model_available_for_plan(Some("free"), "gpt-5.5-pro"));
        assert!(is_model_available_for_plan(
            Some("enterprise"),
            "gpt-5.5-pro"
        ));
        assert!(is_model_available_for_plan(None, "gpt-5.5-pro"));
        // plus still shows every NON-pro model.
        assert!(is_model_available_for_plan(Some("plus"), "gpt-5.5"));
        assert!(is_model_available_for_plan(Some("plus"), "gpt-5.4-codex"));
        // Case-insensitive tier handling + unknown model = show.
        assert!(is_model_available_for_plan(Some("FREE"), "gpt-5.5"));
        assert!(is_model_available_for_plan(Some(""), "gpt-5.5"));
    }

    #[test]
    fn decode_plan_type_reads_claim() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct_1",
                "chatgpt_plan_type": "pro"
            }
        });
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let jwt = format!("header.{body}.sig");
        assert_eq!(decode_plan_type(&jwt).as_deref(), Some("pro"));
    }

    #[test]
    fn decode_plan_type_tolerates_garbage() {
        assert_eq!(decode_plan_type("not-a-jwt"), None);
        assert_eq!(decode_plan_type("a.b.c"), None);
        // Valid JWT shape but no auth/plan claim → None (→ show everything).
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let body = URL_SAFE_NO_PAD.encode(b"{\"foo\":1}");
        assert_eq!(decode_plan_type(&format!("h.{body}.s")), None);
    }
}
