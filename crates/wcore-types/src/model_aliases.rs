//! Canonical model identifiers used by both production code paths and tests.
//!
//! **Why this module exists.** Hard-coded model literals are a maintenance
//! treadmill: every upstream deprecation breaks scattered call sites. This
//! module is the single source of truth — update HERE when models deprecate,
//! and every dependent fixes itself.
//!
//! **Two API shapes for the same data.** Each role exposes two forms:
//!
//! - `pub const FOO: &str = "..."` — for use in struct literals, `Some(FOO)`
//!   positions, and assertions where a `&'static str` is required.
//! - `pub fn foo() -> String` — for use in code paths where the model name
//!   can be overridden at runtime via a matching env var (`E2E_FOO`). Used
//!   by live e2e tests so CI can pin a model without source edits.
//!
//! The functions wrap the consts: there is exactly one default per role.
//!
//! **Boundary rule.** Entries here MUST reference real provider models.
//! Tests that exercise model-agnostic behaviour (e.g. config-inheritance
//! tests where the point is "string A overrides string B" regardless of
//! value) keep their literal strings inline. Routing those through
//! synthetic accessors here would obscure the test's intent. Such sites
//! carry a one-line comment pointing back to this module.

#![allow(dead_code)]

fn from_env_or(default: &'static str, env_var: &str) -> String {
    std::env::var(env_var).unwrap_or_else(|_| default.to_string())
}

// ── Anthropic ──────────────────────────────────────────────────────────────

/// Anthropic Haiku — cheapest live-API model.
///
/// Current pin: Haiku 4.5. The prior pin `claude-haiku-4-20250514` was
/// deprecated upstream and caused two `anthropic::test_anthropic_*` e2e
/// failures cited in BD-audit; the fix that introduced this module.
pub const ANTHROPIC_HAIKU: &str = "claude-haiku-4-5-20251001";
pub fn anthropic_haiku() -> String {
    from_env_or(ANTHROPIC_HAIKU, "E2E_ANTHROPIC_HAIKU")
}

/// Anthropic Sonnet — mid-tier model.
pub const ANTHROPIC_SONNET: &str = "claude-sonnet-4-6";
pub fn anthropic_sonnet() -> String {
    from_env_or(ANTHROPIC_SONNET, "E2E_ANTHROPIC_SONNET")
}

/// Anthropic Opus — strongest reasoning tier, used in skills metadata
/// fixtures across `wcore-skills` and `wcore-agent`.
pub const ANTHROPIC_OPUS: &str = "claude-opus-4-8";
pub fn anthropic_opus() -> String {
    from_env_or(ANTHROPIC_OPUS, "E2E_ANTHROPIC_OPUS")
}

// ── OpenAI ─────────────────────────────────────────────────────────────────

/// OpenAI gpt-4o — flagship model, used as default in some config tests.
pub const OPENAI_GPT4O: &str = "gpt-4o";
pub fn openai_gpt4o() -> String {
    from_env_or(OPENAI_GPT4O, "E2E_OPENAI_GPT4O")
}

/// OpenAI gpt-4o-mini — cheapest OpenAI live-API model.
pub const OPENAI_GPT4O_MINI: &str = "gpt-4o-mini";
pub fn openai_gpt4o_mini() -> String {
    from_env_or(OPENAI_GPT4O_MINI, "E2E_OPENAI_GPT4O_MINI")
}

/// OpenAI gpt-4.1-mini — used in compaction acceptance tests.
pub const OPENAI_GPT4_1_MINI: &str = "gpt-4.1-mini";
pub fn openai_gpt4_1_mini() -> String {
    from_env_or(OPENAI_GPT4_1_MINI, "E2E_OPENAI_GPT4_1_MINI")
}

// ── OpenAI Codex ("Sign in with ChatGPT") ───────────────────────────────────
//
// These model ids route through the ChatGPT Codex backend
// (`chatgpt.com/backend-api/codex`) under OAuth subscription auth, NOT the
// API-key `api.openai.com` path. They are distinct from the `OPENAI_*` ids
// above and only valid for `--provider openai-chatgpt`.

/// Codex headline model — the default for `--provider openai-chatgpt`.
pub const OPENAI_CODEX_GPT55: &str = "gpt-5.5";
pub fn openai_codex_gpt55() -> String {
    from_env_or(OPENAI_CODEX_GPT55, "E2E_OPENAI_CODEX_GPT55")
}

/// Codex high-reasoning tier.
pub const OPENAI_CODEX_GPT55_PRO: &str = "gpt-5.5-pro";
pub fn openai_codex_gpt55_pro() -> String {
    from_env_or(OPENAI_CODEX_GPT55_PRO, "E2E_OPENAI_CODEX_GPT55_PRO")
}

/// Codex prior-generation general model.
pub const OPENAI_CODEX_GPT54: &str = "gpt-5.4";
pub fn openai_codex_gpt54() -> String {
    from_env_or(OPENAI_CODEX_GPT54, "E2E_OPENAI_CODEX_GPT54")
}

/// Codex prior-generation code-specialised model.
pub const OPENAI_CODEX_GPT54_CODEX: &str = "gpt-5.4-codex";
pub fn openai_codex_gpt54_codex() -> String {
    from_env_or(OPENAI_CODEX_GPT54_CODEX, "E2E_OPENAI_CODEX_GPT54_CODEX")
}

/// Codex 5.3-generation code-specialised model.
pub const OPENAI_CODEX_GPT53_CODEX: &str = "gpt-5.3-codex";
pub fn openai_codex_gpt53_codex() -> String {
    from_env_or(OPENAI_CODEX_GPT53_CODEX, "E2E_OPENAI_CODEX_GPT53_CODEX")
}

/// Codex 5.3-generation fast/cheap code model.
pub const OPENAI_CODEX_GPT53_CODEX_SPARK: &str = "gpt-5.3-codex-spark";
pub fn openai_codex_gpt53_codex_spark() -> String {
    from_env_or(
        OPENAI_CODEX_GPT53_CODEX_SPARK,
        "E2E_OPENAI_CODEX_GPT53_CODEX_SPARK",
    )
}

// ── AWS Bedrock ────────────────────────────────────────────────────────────
//
// Bedrock IDs follow `<vendor>.<model-id>-<release-date>-v<N>:<M>`. Dates and
// `v1:0` suffix mirror the upstream Anthropic version pinned above (e.g.
// `claude-sonnet-4-6` here ⇒ `anthropic.claude-sonnet-4-6-20251015-v1:0` on
// Bedrock). Live verification against AWS Bedrock requires creds and is
// out of scope for the W8 dispatch (code-level test only).

/// Bedrock Haiku — cheapest Bedrock-hosted Claude.
pub const BEDROCK_HAIKU: &str = "anthropic.claude-haiku-4-5-20251001-v1:0";
pub fn bedrock_haiku() -> String {
    from_env_or(BEDROCK_HAIKU, "E2E_BEDROCK_HAIKU")
}

/// Bedrock Sonnet — mid-tier Bedrock-hosted Claude.
pub const BEDROCK_SONNET: &str = "anthropic.claude-sonnet-4-6-20251015-v1:0";
pub fn bedrock_sonnet() -> String {
    from_env_or(BEDROCK_SONNET, "E2E_BEDROCK_SONNET")
}

/// Bedrock Opus — strongest reasoning tier on Bedrock.
pub const BEDROCK_OPUS: &str = "anthropic.claude-opus-4-6-20251015-v1:0";
pub fn bedrock_opus() -> String {
    from_env_or(BEDROCK_OPUS, "E2E_BEDROCK_OPUS")
}

// ── Google Vertex AI ───────────────────────────────────────────────────────
//
// Vertex IDs follow `<model-id>@<release-date>` for Anthropic-on-Vertex and
// `<model-id>` (no `@`) for first-party Google models. Dates mirror upstream
// Anthropic version pinned above. Live verification against Vertex requires
// GCP OAuth2 creds and is out of scope for the W8 dispatch.

/// Vertex Haiku — cheapest Vertex-hosted Claude.
pub const VERTEX_HAIKU: &str = "claude-haiku-4-5@20251001";
pub fn vertex_haiku() -> String {
    from_env_or(VERTEX_HAIKU, "E2E_VERTEX_HAIKU")
}

/// Vertex Sonnet — mid-tier Vertex-hosted Claude.
pub const VERTEX_SONNET: &str = "claude-sonnet-4-6@20251015";
pub fn vertex_sonnet() -> String {
    from_env_or(VERTEX_SONNET, "E2E_VERTEX_SONNET")
}

/// Vertex Opus — strongest reasoning tier on Vertex.
pub const VERTEX_OPUS: &str = "claude-opus-4-6@20251015";
pub fn vertex_opus() -> String {
    from_env_or(VERTEX_OPUS, "E2E_VERTEX_OPUS")
}

/// Vertex Gemini Pro — flagship Google-first-party model on Vertex.
pub const VERTEX_GEMINI_PRO: &str = "gemini-2.5-pro";
pub fn vertex_gemini_pro() -> String {
    from_env_or(VERTEX_GEMINI_PRO, "E2E_VERTEX_GEMINI_PRO")
}

/// Vertex Gemini Flash — cheaper Google-first-party model on Vertex.
pub const VERTEX_GEMINI_FLASH: &str = "gemini-2.5-flash";
pub fn vertex_gemini_flash() -> String {
    from_env_or(VERTEX_GEMINI_FLASH, "E2E_VERTEX_GEMINI_FLASH")
}

// ── Short-form alias resolver ──────────────────────────────────────────────
//
// Closes debt-register B.4 (HC-3-followup). The `--model bedrock:sonnet`
// shorthand is parsed and expanded to the full Bedrock ID *here* — the
// engine's provider layer always sees the literal canonical string, never
// the short form. Anthropic and OpenAI short-forms are included for
// symmetry even though their default routing already lands on the same
// canonical string via `default_model_for`.

/// Try to expand a `<provider>:<role>` short-form into a canonical model
/// identifier. Returns `None` if `model` does not match a known short-form
/// pair, in which case the caller should pass the string through verbatim
/// (it may be a fully-qualified literal already).
///
/// The match is exact, case-sensitive on the role token, and only fires
/// when `model` contains exactly one `:` separator and the prefix is a
/// recognised provider name. Anything else — including unknown roles
/// like `bedrock:gemini` — returns `None` so the literal flows through
/// unchanged and the eventual provider request surfaces the upstream
/// error.
pub fn expand_short_form(model: &str) -> Option<&'static str> {
    let (provider, role) = model.split_once(':')?;
    match (provider, role) {
        ("anthropic", "haiku") => Some(ANTHROPIC_HAIKU),
        ("anthropic", "sonnet") => Some(ANTHROPIC_SONNET),
        ("anthropic", "opus") => Some(ANTHROPIC_OPUS),
        ("openai", "gpt4o") | ("openai", "gpt-4o") => Some(OPENAI_GPT4O),
        ("openai", "gpt4o-mini") | ("openai", "gpt-4o-mini") => Some(OPENAI_GPT4O_MINI),
        ("openai", "gpt4.1-mini") | ("openai", "gpt-4.1-mini") => Some(OPENAI_GPT4_1_MINI),
        ("bedrock", "haiku") => Some(BEDROCK_HAIKU),
        ("bedrock", "sonnet") => Some(BEDROCK_SONNET),
        ("bedrock", "opus") => Some(BEDROCK_OPUS),
        ("vertex", "haiku") => Some(VERTEX_HAIKU),
        ("vertex", "sonnet") => Some(VERTEX_SONNET),
        ("vertex", "opus") => Some(VERTEX_OPUS),
        ("vertex", "gemini-pro") => Some(VERTEX_GEMINI_PRO),
        ("vertex", "gemini-flash") => Some(VERTEX_GEMINI_FLASH),
        // W11: native Gemini provider short-forms. The model IDs are the
        // same as the Vertex Gemini entries (only the API surface differs);
        // re-using the existing constants keeps a single source of truth.
        ("gemini", "pro") => Some(VERTEX_GEMINI_PRO),
        ("gemini", "flash") => Some(VERTEX_GEMINI_FLASH),
        // "Sign in with ChatGPT" — Codex backend model roles. The role token
        // is the bare model id minus the `gpt-` prefix (e.g. `5.5`, `5.4-codex`)
        // so the `/model` picker can offer compact handles.
        ("openai-chatgpt", "5.5") => Some(OPENAI_CODEX_GPT55),
        ("openai-chatgpt", "5.5-pro") => Some(OPENAI_CODEX_GPT55_PRO),
        ("openai-chatgpt", "5.4") => Some(OPENAI_CODEX_GPT54),
        ("openai-chatgpt", "5.4-codex") | ("openai-chatgpt", "codex") => {
            Some(OPENAI_CODEX_GPT54_CODEX)
        }
        ("openai-chatgpt", "5.3-codex") => Some(OPENAI_CODEX_GPT53_CODEX),
        ("openai-chatgpt", "5.3-codex-spark") => Some(OPENAI_CODEX_GPT53_CODEX_SPARK),
        // MiniMax — Anthropic-wire endpoint. Short handles for the `/model`
        // picker; the role token is the bare generation id (`m2`, `m2.5`, `m3`).
        ("minimax", "m2") => Some(MINIMAX_M2),
        ("minimax", "m2.5") => Some(MINIMAX_M2_5),
        ("minimax", "m3") => Some(MINIMAX_M3),
        // Sakana ("Fugu") carries NO hardcoded catalog — it relies on the live
        // `/v1/models` fetch (like flux-router/groq), so there is nothing here
        // to drift out of date.
        _ => None,
    }
}

// ── MiniMax (Anthropic-wire endpoint) ───────────────────────────────────────
//
// MiniMax's `/anthropic` endpoint speaks the native Anthropic wire protocol
// (`x-api-key` auth, `/v1/messages`, `/v1/messages/count_tokens`, `/v1/models`,
// SSE, Anthropic error envelopes), verified live 2026-06-18. The provider
// therefore reuses `AnthropicProvider` rather than a duplicate struct. These
// ids mirror what `GET /anthropic/v1/models` advertises and are the offline
// fallback for the `/model` picker when the live list is unreachable.

/// MiniMax M2 — the documented headline model and the per-provider default.
pub const MINIMAX_M2: &str = "MiniMax-M2";
pub fn minimax_m2() -> String {
    from_env_or(MINIMAX_M2, "E2E_MINIMAX_M2")
}

/// MiniMax M2.5 — mid-generation refresh.
pub const MINIMAX_M2_5: &str = "MiniMax-M2.5";
pub fn minimax_m2_5() -> String {
    from_env_or(MINIMAX_M2_5, "E2E_MINIMAX_M2_5")
}

/// MiniMax M3 — newest generation.
pub const MINIMAX_M3: &str = "MiniMax-M3";
pub fn minimax_m3() -> String {
    from_env_or(MINIMAX_M3, "E2E_MINIMAX_M3")
}

// Sakana ("Fugu") deliberately has NO model-id constants or catalog here: its
// `/model` picker is driven by the live `https://api.sakana.ai/v1/models`
// fetch (`OpenAIProvider::list_models`), the same live-source pattern Hermes /
// OpenClaw / the desktop app use (models.dev) — never a hand-typed list. The
// per-provider default (`fugu`) lives in `wcore_config::default_model_for`.

/// The selectable models for a provider, as `(short_form, resolved_id)`
/// pairs in display order (most-capable first). The single source of truth
/// for the TUI `/model` picker — keeps the catalog from drifting from
/// [`expand_short_form`]. Returns an empty slice for an unknown provider
/// (the user can still type a literal model id).
///
/// `short_form` is the human handle (`anthropic:opus`); `resolved_id` is what
/// the provider request actually carries. Restricted to the given provider
/// because a bare model swap keeps the current provider + compat — switching
/// providers is a separate, heavier operation.
/// The built-in providers with a model catalog, as offered by the `/provider`
/// listing and reachable through `/setup`. Single source of truth: every name
/// here must list models in [`models_for_provider`] (guarded by a no-drift
/// test). A user can still configure other providers via custom aliases.
pub fn known_providers() -> &'static [&'static str] {
    &[
        "anthropic",
        "openai",
        "bedrock",
        "vertex",
        "gemini",
        "openai-chatgpt",
        "minimax",
    ]
}

pub fn models_for_provider(provider: &str) -> &'static [(&'static str, &'static str)] {
    match provider {
        "anthropic" => &[
            ("anthropic:opus", ANTHROPIC_OPUS),
            ("anthropic:sonnet", ANTHROPIC_SONNET),
            ("anthropic:haiku", ANTHROPIC_HAIKU),
        ],
        "openai" => &[
            ("openai:gpt4o", OPENAI_GPT4O),
            ("openai:gpt4.1-mini", OPENAI_GPT4_1_MINI),
            ("openai:gpt4o-mini", OPENAI_GPT4O_MINI),
        ],
        "bedrock" => &[
            ("bedrock:opus", BEDROCK_OPUS),
            ("bedrock:sonnet", BEDROCK_SONNET),
            ("bedrock:haiku", BEDROCK_HAIKU),
        ],
        "vertex" => &[
            ("vertex:opus", VERTEX_OPUS),
            ("vertex:sonnet", VERTEX_SONNET),
            ("vertex:haiku", VERTEX_HAIKU),
            ("vertex:gemini-pro", VERTEX_GEMINI_PRO),
            ("vertex:gemini-flash", VERTEX_GEMINI_FLASH),
        ],
        "gemini" => &[
            ("gemini:pro", VERTEX_GEMINI_PRO),
            ("gemini:flash", VERTEX_GEMINI_FLASH),
        ],
        // "Sign in with ChatGPT" — Codex backend catalog, most-capable first.
        "openai-chatgpt" => &[
            ("openai-chatgpt:5.5", OPENAI_CODEX_GPT55),
            ("openai-chatgpt:5.5-pro", OPENAI_CODEX_GPT55_PRO),
            ("openai-chatgpt:5.4", OPENAI_CODEX_GPT54),
            ("openai-chatgpt:5.4-codex", OPENAI_CODEX_GPT54_CODEX),
            ("openai-chatgpt:5.3-codex", OPENAI_CODEX_GPT53_CODEX),
            (
                "openai-chatgpt:5.3-codex-spark",
                OPENAI_CODEX_GPT53_CODEX_SPARK,
            ),
        ],
        // MiniMax — Anthropic-wire endpoint. Most-capable / newest first.
        "minimax" => &[
            ("minimax:m3", MINIMAX_M3),
            ("minimax:m2.5", MINIMAX_M2_5),
            ("minimax:m2", MINIMAX_M2),
        ],
        // NOTE: Sakana is intentionally absent — live `/v1/models` is its
        // catalog (no hand-typed fallback to drift). Same for flux-router/groq.
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Format invariants ──────────────────────────────────────────────
    //
    // These assertions guard against typos in the const literals above:
    // a Bedrock ID without the `vN:M` suffix or a Vertex ID without the
    // `@date` separator would silently 404 at runtime against AWS/GCP.
    // Catch it at compile-test time instead.

    #[test]
    fn models_for_provider_entries_resolve_and_dont_drift() {
        // Every short_form the /model picker offers MUST resolve through
        // expand_short_form to the resolved id it advertises — the catalog
        // and the resolver can't silently diverge.
        for provider in [
            "anthropic",
            "openai",
            "bedrock",
            "vertex",
            "gemini",
            "openai-chatgpt",
            "minimax",
        ] {
            let models = models_for_provider(provider);
            assert!(!models.is_empty(), "{provider} must list models");
            for (short, resolved) in models {
                assert_eq!(
                    expand_short_form(short),
                    Some(*resolved),
                    "`{short}` must resolve to `{resolved}`"
                );
            }
        }
        assert!(
            models_for_provider("nonesuch").is_empty(),
            "an unknown provider lists nothing"
        );
    }

    #[test]
    fn chatgpt_provider_has_codex_models() {
        let m = models_for_provider("openai-chatgpt");
        // The headline Codex model is offered.
        assert!(
            m.iter().any(|(_, id)| *id == "gpt-5.5"),
            "openai-chatgpt must offer gpt-5.5"
        );
        // It is a known provider so /provider and /model can reach it.
        assert!(known_providers().contains(&"openai-chatgpt"));
        // Every advertised short-form resolves (drift guard, provider-scoped).
        for (short, resolved) in m {
            assert_eq!(expand_short_form(short), Some(*resolved));
        }
        // The `codex` convenience alias resolves to the code-specialised model.
        assert_eq!(
            expand_short_form("openai-chatgpt:codex"),
            Some(OPENAI_CODEX_GPT54_CODEX)
        );
    }

    #[test]
    fn known_providers_all_have_a_model_catalog() {
        // The `/provider` listing and the `/model` catalog can't diverge: every
        // advertised provider must actually carry models.
        for p in known_providers() {
            assert!(
                !models_for_provider(p).is_empty(),
                "known provider `{p}` has no models"
            );
        }
    }

    #[test]
    fn minimax_is_a_known_provider() {
        // Regression guard (deep-sweep F41): MiniMax is a registered built-in
        // provider with a model catalog, but was omitted from known_providers()
        // — which the /provider and /model TUI pickers iterate — so it never
        // appeared in the picker. Every built-in with model aliases must be
        // listed here to be reachable from the UI.
        assert!(
            known_providers().contains(&"minimax"),
            "minimax must be in known_providers() or it is invisible in the pickers"
        );
        assert!(!models_for_provider("minimax").is_empty());
    }

    fn assert_bedrock_format(id: &str) {
        assert!(!id.is_empty(), "bedrock id empty");
        assert!(
            id.contains('.'),
            "bedrock id `{id}` missing vendor `.` separator"
        );
        // `vN:M` suffix — `:` is the trailing version revision marker.
        let suffix = id.rsplit_once(':').unwrap_or_else(|| {
            panic!("bedrock id `{id}` missing `:M` revision suffix");
        });
        assert!(
            suffix.0.contains("-v"),
            "bedrock id `{id}` missing `vN` version prefix"
        );
    }

    fn assert_vertex_anthropic_format(id: &str) {
        assert!(!id.is_empty(), "vertex id empty");
        assert!(
            id.contains('@'),
            "vertex anthropic id `{id}` missing `@<date>` separator"
        );
        let (_name, date) = id.rsplit_once('@').unwrap();
        assert_eq!(
            date.len(),
            8,
            "vertex anthropic id `{id}` date must be YYYYMMDD"
        );
        assert!(
            date.chars().all(|c| c.is_ascii_digit()),
            "vertex anthropic id `{id}` date must be all digits"
        );
    }

    #[test]
    fn bedrock_consts_well_formed() {
        assert_bedrock_format(BEDROCK_HAIKU);
        assert_bedrock_format(BEDROCK_SONNET);
        assert_bedrock_format(BEDROCK_OPUS);
    }

    #[test]
    fn vertex_anthropic_consts_well_formed() {
        assert_vertex_anthropic_format(VERTEX_HAIKU);
        assert_vertex_anthropic_format(VERTEX_SONNET);
        assert_vertex_anthropic_format(VERTEX_OPUS);
    }

    #[test]
    fn vertex_gemini_consts_non_empty() {
        // Gemini IDs don't carry `@<date>` — they're plain
        // `gemini-<version>-<role>` strings. Non-empty + sane prefix
        // is the only invariant we can assert without hitting the API.
        assert!(VERTEX_GEMINI_PRO.starts_with("gemini-"));
        assert!(VERTEX_GEMINI_FLASH.starts_with("gemini-"));
    }

    // ── Short-form expansion ───────────────────────────────────────────

    #[test]
    fn expand_short_form_bedrock_roles() {
        assert_eq!(expand_short_form("bedrock:haiku"), Some(BEDROCK_HAIKU));
        assert_eq!(expand_short_form("bedrock:sonnet"), Some(BEDROCK_SONNET));
        assert_eq!(expand_short_form("bedrock:opus"), Some(BEDROCK_OPUS));
    }

    #[test]
    fn expand_short_form_vertex_roles() {
        assert_eq!(expand_short_form("vertex:haiku"), Some(VERTEX_HAIKU));
        assert_eq!(expand_short_form("vertex:sonnet"), Some(VERTEX_SONNET));
        assert_eq!(expand_short_form("vertex:opus"), Some(VERTEX_OPUS));
        assert_eq!(
            expand_short_form("vertex:gemini-pro"),
            Some(VERTEX_GEMINI_PRO)
        );
        assert_eq!(
            expand_short_form("vertex:gemini-flash"),
            Some(VERTEX_GEMINI_FLASH)
        );
    }

    #[test]
    fn expand_short_form_anthropic_and_openai_for_symmetry() {
        assert_eq!(
            expand_short_form("anthropic:sonnet"),
            Some(ANTHROPIC_SONNET)
        );
        assert_eq!(expand_short_form("openai:gpt-4o"), Some(OPENAI_GPT4O));
    }

    #[test]
    fn expand_short_form_gemini_native_roles() {
        // W11: native Gemini reuses the canonical Vertex Gemini IDs.
        assert_eq!(expand_short_form("gemini:pro"), Some(VERTEX_GEMINI_PRO));
        assert_eq!(expand_short_form("gemini:flash"), Some(VERTEX_GEMINI_FLASH));
    }

    #[test]
    fn expand_short_form_passes_through_literals() {
        // Full Bedrock literal must not be re-mapped (no `:` *role* match).
        assert_eq!(
            expand_short_form("anthropic.claude-sonnet-4-6-20251015-v1:0"),
            None,
            "literal Bedrock ID must pass through (None ⇒ caller uses it verbatim)"
        );
        // Full Vertex literal contains `@`, no provider-prefix.
        assert_eq!(expand_short_form("claude-sonnet-4-6@20251015"), None);
        // Unknown role: literal flows through, provider returns the error.
        assert_eq!(expand_short_form("bedrock:gemini"), None);
        // No prefix at all.
        assert_eq!(expand_short_form("claude-sonnet-4-6"), None);
        // Plugin-style prefix is reserved for plugin routing.
        assert_eq!(expand_short_form("ollama:llama-4"), None);
    }
}
