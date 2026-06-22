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
//! # Why the gating table is empty
//!
//! There is no authoritative entitled-model list anywhere in the OAuth token /
//! claims (the JWT exposes only `chatgpt_plan_type`, a free-form string such as
//! `"plus"` / `"pro"`), and the repository contains NO evidence grounding which
//! Codex models a given plan tier may run. Per the #158 brief, a guessed
//! whitelist must not ship. The mechanism is therefore wired end-to-end but seeds
//! [`PLAN_GATED_MODELS`] EMPTY: today it hides nothing (byte-identical to the
//! pre-#158 catalog for every tier), and a grounded entry can be added later
//! without any code change — only data.
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
/// models. Absent evidence, leave this empty — showing a runnable model is the
/// safe default; hiding one is the failure mode #158 forbids.
struct PlanGate {
    /// The `chatgpt_plan_type` claim value, lowercased (e.g. `"free"`).
    plan_tier: &'static str,
    /// Resolved model ids (as in `model_aliases`, e.g. `"gpt-5.5-pro"`) the
    /// tier cannot run.
    gated_model_ids: &'static [&'static str],
}

/// Conservative, evidence-grounded tier→unavailable-models table.
///
/// EMPTY by design — see the module docs. No repo evidence grounds any
/// tier→Codex-model gating, so shipping any entry here would be a guess, which
/// #158 explicitly forbids. Populate only with grounded entries; the filter then
/// activates with no further code change.
const PLAN_GATED_MODELS: &[PlanGate] = &[
    // Example shape (commented out — NOT grounded, do not enable as-is):
    // PlanGate { plan_tier: "free", gated_model_ids: &["gpt-5.5-pro"] },
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
        .any(|g| g.gated_model_ids.iter().any(|m| *m == model_id));
    !gated
}

/// Filter a list of resolved model ids to those `plan_tier` can run.
///
/// `plan_tier` is the `chatgpt_plan_type` JWT claim (`None` when absent or
/// undecodable). Preserves order; removes only *provably unavailable* models for
/// a *recognised* tier. With the current empty [`PLAN_GATED_MODELS`] this is the
/// identity function for every input — see the module docs.
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
    fn known_plan_with_empty_table_shows_full_catalog() {
        // With the conservative empty gating table, even a "recognised"-looking
        // tier hides nothing — the no-over-filter guarantee.
        for plan in ["free", "plus", "pro", "team"] {
            let out = filter_model_ids(Some(plan), CATALOG);
            assert_eq!(
                out, CATALOG,
                "plan {plan} must not filter while the gating table is empty"
            );
        }
    }

    #[test]
    fn gating_is_conservative_and_data_driven() {
        // The shipped table is empty, so nothing is gated for any tier/model.
        assert!(
            is_model_available_for_plan(Some("free"), "gpt-5.5-pro"),
            "shipped table is empty: nothing is gated"
        );
        // Case-insensitive tier handling + unknown model = show.
        assert!(is_model_available_for_plan(Some("FREE"), "gpt-5.5"));
        assert!(is_model_available_for_plan(Some(""), "gpt-5.5"));
        // Missing plan = show.
        assert!(is_model_available_for_plan(None, "gpt-5.5-pro"));
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
