//! Per-request OpenAI model-family parameter compatibility detector.
//!
//! OpenAI's reasoning families (`o1*`, `o3*`) and the `gpt-5*` family
//! diverge from the classic Chat Completions request shape:
//!
//! * They require `max_completion_tokens` instead of `max_tokens` in the
//!   request body — sending `max_tokens` returns a 400 with
//!   `Unsupported parameter: 'max_tokens' is not supported with this model.
//!   Use 'max_completion_tokens' instead`.
//! * They accept a `reasoning_effort` field (`low`/`medium`/`high`); the
//!   classic chat families (`gpt-4o`, `gpt-4.x`, etc.) 400 on it.
//! * The `o1*` and `o3*` reasoning families fix `temperature` at `1.0` and
//!   reject any explicit value; the `gpt-5*` family still accepts it.
//!
//! `OpenAIProvider` reuses one HTTP client + compat block across every
//! model it serves in a session — so the family selection must be
//! per-request, not baked into the provider at construction time.
//!
//! All predicates do **case-insensitive prefix matching** on the model
//! string the caller hands to `LlmRequest::model`.

/// Lower-case the input once so the family checks are case-insensitive.
fn lower(model: &str) -> String {
    model.to_ascii_lowercase()
}

/// True when the request body must use `max_completion_tokens` instead of
/// `max_tokens`. Matches the OpenAI reasoning families (`o1*`, `o3*`) and
/// the `gpt-5*` family.
///
/// This is the model-family **prefix heuristic** default. Callers that have a
/// `ProviderCompat` should prefer [`max_completion_tokens_override`], which
/// threads the `[compat] uses_max_completion_tokens` flag over this default —
/// keeping the provider-quirk decision in config rather than hardcoded here.
pub fn wants_max_completion_tokens(model: &str) -> bool {
    let m = lower(model);
    is_o_series(&m) || is_gpt5(&m)
}

/// F27: resolve the `max_completion_tokens`-vs-`max_tokens` decision, honoring
/// an optional per-deployment `ProviderCompat.uses_max_completion_tokens`
/// override before falling back to the model-family default in
/// [`wants_max_completion_tokens`].
///
/// `Some(true)` / `Some(false)` force `max_completion_tokens` / `max_tokens`
/// respectively; `None` defers to the prefix heuristic. Mirrors
/// [`responses_api_override`].
pub fn max_completion_tokens_override(model: &str, override_flag: Option<bool>) -> bool {
    match override_flag {
        Some(forced) => forced,
        None => wants_max_completion_tokens(model),
    }
}

/// True when the model accepts a `reasoning_effort` field. R78: forwards to the
/// single canonical predicate in `wcore-config` instead of re-implementing the
/// prefix logic here — the two copies could otherwise silently drift.
pub fn accepts_reasoning_effort(model: &str) -> bool {
    wcore_config::config::openai_model_accepts_effort(model)
}

/// True when the model must be served via the OpenAI **Responses API**
/// (`POST /v1/responses`) instead of Chat Completions
/// (`POST /v1/chat/completions`).
///
/// The `gpt-5*` family is **rejected** at `/v1/chat/completions` with
/// `unsupported_api_for_model` — it accepts ONLY the Responses surface,
/// which uses a different request body (`input` instead of `messages`,
/// `instructions` for the system prompt, Responses-format `tools`,
/// `reasoning.effort`, `max_output_tokens`) and a different streaming
/// event shape. `OpenAIProvider::stream` consults this predicate per
/// request to pick the endpoint + parser; everything else (`gpt-4o`,
/// `o1*`, `o3*`, third-party openai-compat models) keeps using Chat
/// Completions.
///
/// This is intentionally a SEPARATE predicate from `is_gpt5` even though
/// they currently coincide: the o-series can still be served via Chat
/// Completions today, and a future model could need Responses without the
/// `gpt-5` prefix. Keeping the routing decision in its own named function
/// (mirroring the other model-family predicates here) is the single,
/// documented seam for the chat-vs-responses API-surface choice.
///
/// Callers that need to FORCE one surface (e.g. an openai-compat gateway
/// that proxies `gpt-5*` over Chat Completions, or a deployment that
/// requires Responses for a non-`gpt-5` model) should prefer
/// [`crate::openai_compat::responses_api_override`], which threads a
/// `ProviderCompat` flag over this default.
pub fn model_uses_responses_api(model: &str) -> bool {
    let m = lower(model);
    is_gpt5(&m)
}

/// Resolve the chat-vs-responses routing decision, honoring an optional
/// per-deployment `ProviderCompat.uses_responses_api` override before
/// falling back to the model-family default in [`model_uses_responses_api`].
///
/// `Some(true)` / `Some(false)` force the Responses / Chat surface
/// respectively (for gateways that proxy `gpt-5*` over Chat Completions, or
/// custom endpoints that require Responses for an unrecognized model id);
/// `None` defers to the model-family predicate.
pub fn responses_api_override(model: &str, override_flag: Option<bool>) -> bool {
    match override_flag {
        Some(forced) => forced,
        None => model_uses_responses_api(model),
    }
}

/// True when the model accepts an explicit `temperature`. False for the
/// `o1*` / `o3*` reasoning families (which fix `temperature` at `1.0`) and
/// Anthropic's Opus 4.x reasoning models (which reject an explicit value),
/// true everywhere else — including `gpt-5*`, which still honors it.
pub fn accepts_temperature(model: &str) -> bool {
    let m = lower(model);
    !is_o_series(&m) && !is_temperature_locked(&m)
}

/// Models that reject an explicit `temperature` outside the OpenAI o-series.
/// Anthropic's Opus 4.x family returns HTTP 400 "temperature is deprecated
/// for this model" when a value is sent, so it must be omitted for them.
fn is_temperature_locked(lower_model: &str) -> bool {
    lower_model.contains("opus-4")
}

/// Crucible #3: emit `body["temperature"]` for the request, but only when ALL
/// hold:
/// 1. the request carries an explicit `temperature` (`Some`),
/// 2. the provider hasn't opted out via `compat.supports_temperature == false`,
/// 3. the model accepts an explicit temperature (`accepts_temperature` excludes
///    the OpenAI `o1*`/`o3*` reasoning families, which fix it at 1.0).
///
/// Centralizing the gate here keeps the o-series exclusion + the provider switch
/// in one place so every provider body builder emits temperature identically —
/// no hardcoded `base_url.contains(...)` quirks (AGENTS.md). The field name is
/// `"temperature"` for every current provider.
pub fn emit_temperature(
    body: &mut serde_json::Value,
    request: &wcore_types::llm::LlmRequest,
    compat: &wcore_config::compat::ProviderCompat,
) {
    if let Some(t) = request.temperature
        && compat.supports_temperature()
        && accepts_temperature(&request.model)
    {
        body["temperature"] = serde_json::json!(t);
    }
}

/// True when the model accepts caller-supplied tool-calling parameters
/// (the `tools` array) on the Chat Completions request.
///
/// Groq's **Compound** / **Compound Mini** are agentic models that perform
/// their own internal tool use and REJECT a caller `tools` array with a 400
/// `` `tool calling` is not supported ``, which kills the whole turn. Every
/// other model served over the OpenAI-compatible surface accepts tools, so
/// default to true and special-case only the Groq Compound family. Like the
/// sibling predicates this is per-request (one `OpenAIProvider` serves many
/// models in a session) and case-insensitive.
pub fn model_supports_tool_calling(model: &str) -> bool {
    !is_groq_compound(&lower(model))
}

/// `o1`, `o1-mini`, `o1-preview`, `o3`, `o3-mini`, ... — match exactly the
/// `o<digit>` prefix so we don't accidentally catch unrelated model names
/// like `octo-7b`.
fn is_o_series(lower_model: &str) -> bool {
    let bytes = lower_model.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'o' {
        return false;
    }
    if !bytes[1].is_ascii_digit() {
        return false;
    }
    // `o1`, `o3`, `o1-mini`, `o3-mini-2024-09-12` all fine. `o4` etc.
    // (future reasoning families) ride the same shape.
    true
}

/// `gpt-5`, `gpt-5.5-preview`, `gpt-5-turbo`, `gpt-5o-mini`, ... — the
/// shared property is the literal prefix `gpt-5`. We deliberately do NOT
/// match `gpt-5.x` via a regex; a simple prefix check is enough and stays
/// correct as long as future `gpt-5*` releases keep the family shape.
fn is_gpt5(lower_model: &str) -> bool {
    lower_model.starts_with("gpt-5")
}

/// Groq's agentic Compound family: `compound-beta`, `compound-beta-mini`, and
/// the namespaced `groq/compound*` catalog ids. Strip any `provider/` prefix and
/// match by leading `compound` so we don't catch unrelated models that merely
/// contain the substring (e.g. `octo-compound-7b`). These models reject the
/// `tools` parameter.
fn is_groq_compound(lower_model: &str) -> bool {
    let id = lower_model.rsplit('/').next().unwrap_or(lower_model);
    id.starts_with("compound")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- wants_max_completion_tokens --------------------------------------

    #[test]
    fn wants_max_completion_tokens_gpt4o_is_false() {
        assert!(!wants_max_completion_tokens("gpt-4o"));
    }

    #[test]
    fn wants_max_completion_tokens_gpt4o_dated_is_false() {
        assert!(!wants_max_completion_tokens("gpt-4o-2024-08-06"));
    }

    #[test]
    fn wants_max_completion_tokens_gpt5_is_true() {
        assert!(wants_max_completion_tokens("gpt-5"));
    }

    #[test]
    fn wants_max_completion_tokens_gpt55_preview_is_true() {
        assert!(wants_max_completion_tokens("gpt-5.5-preview"));
    }

    #[test]
    fn wants_max_completion_tokens_gpt5_turbo_is_true() {
        assert!(wants_max_completion_tokens("gpt-5-turbo"));
    }

    #[test]
    fn wants_max_completion_tokens_o1_is_true() {
        assert!(wants_max_completion_tokens("o1"));
    }

    #[test]
    fn wants_max_completion_tokens_o1_mini_is_true() {
        assert!(wants_max_completion_tokens("o1-mini"));
    }

    #[test]
    fn wants_max_completion_tokens_o3_mini_is_true() {
        assert!(wants_max_completion_tokens("o3-mini"));
    }

    #[test]
    fn wants_max_completion_tokens_case_insensitive() {
        assert!(wants_max_completion_tokens("GPT-5"));
        assert!(wants_max_completion_tokens("O1-Mini"));
    }

    #[test]
    fn wants_max_completion_tokens_octo_is_false() {
        // Sanity: `o`-prefixed non-OpenAI models must NOT trip the
        // o-series predicate.
        assert!(!wants_max_completion_tokens("octo-7b"));
        assert!(!wants_max_completion_tokens("ollama-llama3"));
    }

    // --- max_completion_tokens_override (F27) -----------------------------

    #[test]
    fn max_completion_tokens_override_none_uses_prefix_heuristic() {
        // None defers to the model-family default — behavior identical to the
        // bare heuristic.
        assert!(max_completion_tokens_override("gpt-5", None));
        assert!(max_completion_tokens_override("o1-mini", None));
        assert!(!max_completion_tokens_override("gpt-4o", None));
    }

    #[test]
    fn max_completion_tokens_override_forces_field() {
        // Some(true) forces max_completion_tokens even for a classic chat model;
        // Some(false) forces max_tokens even for a reasoning-family model.
        assert!(max_completion_tokens_override("gpt-4o", Some(true)));
        assert!(!max_completion_tokens_override("gpt-5", Some(false)));
        assert!(!max_completion_tokens_override("o1-mini", Some(false)));
    }

    // --- accepts_reasoning_effort -----------------------------------------

    #[test]
    fn accepts_reasoning_effort_gpt4o_is_false() {
        assert!(!accepts_reasoning_effort("gpt-4o"));
    }

    #[test]
    fn accepts_reasoning_effort_gpt4o_dated_is_false() {
        assert!(!accepts_reasoning_effort("gpt-4o-2024-08-06"));
    }

    #[test]
    fn accepts_reasoning_effort_gpt5_is_true() {
        assert!(accepts_reasoning_effort("gpt-5"));
    }

    #[test]
    fn accepts_reasoning_effort_gpt55_preview_is_true() {
        assert!(accepts_reasoning_effort("gpt-5.5-preview"));
    }

    #[test]
    fn accepts_reasoning_effort_gpt5_turbo_is_true() {
        assert!(accepts_reasoning_effort("gpt-5-turbo"));
    }

    #[test]
    fn accepts_reasoning_effort_o1_is_true() {
        assert!(accepts_reasoning_effort("o1"));
    }

    #[test]
    fn accepts_reasoning_effort_o1_mini_is_true() {
        assert!(accepts_reasoning_effort("o1-mini"));
    }

    #[test]
    fn accepts_reasoning_effort_o3_mini_is_true() {
        assert!(accepts_reasoning_effort("o3-mini"));
    }

    #[test]
    fn accepts_reasoning_effort_case_insensitive() {
        assert!(accepts_reasoning_effort("GPT-5"));
        assert!(accepts_reasoning_effort("O1-Mini"));
    }

    // --- model_uses_responses_api -----------------------------------------

    #[test]
    fn model_uses_responses_api_gpt5_is_true() {
        assert!(model_uses_responses_api("gpt-5"));
        assert!(model_uses_responses_api("gpt-5.5-preview"));
        assert!(model_uses_responses_api("gpt-5-turbo"));
        assert!(model_uses_responses_api("gpt-5o-mini"));
    }

    #[test]
    fn model_uses_responses_api_gpt4o_is_false() {
        assert!(!model_uses_responses_api("gpt-4o"));
        assert!(!model_uses_responses_api("gpt-4o-2024-08-06"));
        assert!(!model_uses_responses_api("gpt-4.1"));
    }

    #[test]
    fn model_uses_responses_api_o_series_is_false() {
        // o-series stays on Chat Completions today.
        assert!(!model_uses_responses_api("o1"));
        assert!(!model_uses_responses_api("o3-mini"));
    }

    #[test]
    fn model_uses_responses_api_case_insensitive() {
        assert!(model_uses_responses_api("GPT-5"));
        assert!(!model_uses_responses_api("GPT-4o"));
    }

    #[test]
    fn model_uses_responses_api_non_openai_is_false() {
        assert!(!model_uses_responses_api("octo-7b"));
        assert!(!model_uses_responses_api("ollama-llama3"));
    }

    // --- responses_api_override -------------------------------------------

    #[test]
    fn responses_api_override_none_defers_to_family() {
        assert!(responses_api_override("gpt-5", None));
        assert!(!responses_api_override("gpt-4o", None));
    }

    #[test]
    fn responses_api_override_forces_surface() {
        // Force a gpt-5 model back onto Chat Completions (gateway proxy).
        assert!(!responses_api_override("gpt-5", Some(false)));
        // Force a non-gpt-5 model onto Responses (custom endpoint).
        assert!(responses_api_override("custom-model", Some(true)));
    }

    // --- accepts_temperature ----------------------------------------------

    #[test]
    fn accepts_temperature_gpt4o_is_true() {
        assert!(accepts_temperature("gpt-4o"));
    }

    #[test]
    fn accepts_temperature_gpt4o_dated_is_true() {
        assert!(accepts_temperature("gpt-4o-2024-08-06"));
    }

    #[test]
    fn accepts_temperature_gpt5_is_true() {
        // gpt-5 still honors explicit temperature.
        assert!(accepts_temperature("gpt-5"));
    }

    #[test]
    fn accepts_temperature_gpt55_preview_is_true() {
        assert!(accepts_temperature("gpt-5.5-preview"));
    }

    #[test]
    fn accepts_temperature_gpt5_turbo_is_true() {
        assert!(accepts_temperature("gpt-5-turbo"));
    }

    #[test]
    fn accepts_temperature_o1_is_false() {
        assert!(!accepts_temperature("o1"));
    }

    // Anthropic Opus 4.x rejects an explicit temperature (HTTP 400).
    #[test]
    fn accepts_temperature_opus_4x_is_false() {
        assert!(!accepts_temperature("claude-opus-4-7"));
        assert!(!accepts_temperature("claude-opus-4-6"));
        assert!(!accepts_temperature("claude-opus-4-8"));
        assert!(!accepts_temperature("anthropic/claude-opus-4-7"));
    }

    // Sonnet/Haiku still honor temperature — only Opus 4.x is locked.
    #[test]
    fn accepts_temperature_non_opus_claude_is_true() {
        assert!(accepts_temperature("claude-sonnet-4-6"));
        assert!(accepts_temperature("claude-haiku-4-5"));
    }

    #[test]
    fn accepts_temperature_o1_mini_is_false() {
        assert!(!accepts_temperature("o1-mini"));
    }

    #[test]
    fn accepts_temperature_o3_mini_is_false() {
        assert!(!accepts_temperature("o3-mini"));
    }

    #[test]
    fn accepts_temperature_case_insensitive() {
        assert!(!accepts_temperature("O1"));
        assert!(accepts_temperature("GPT-4o"));
    }

    // --- model_supports_tool_calling (Groq Compound) ----------------------

    #[test]
    fn compound_family_rejects_tool_calling() {
        // Groq's agentic Compound ids: bare, beta, mini, and namespaced.
        assert!(!model_supports_tool_calling("compound-beta"));
        assert!(!model_supports_tool_calling("compound-beta-mini"));
        assert!(!model_supports_tool_calling("compound"));
        assert!(!model_supports_tool_calling("compound-mini"));
        assert!(!model_supports_tool_calling("groq/compound-beta"));
    }

    #[test]
    fn compound_predicate_is_case_insensitive() {
        assert!(!model_supports_tool_calling("Compound-Beta"));
        assert!(!model_supports_tool_calling("GROQ/COMPOUND"));
    }

    #[test]
    fn normal_models_support_tool_calling() {
        // Plain Groq + every other openai-compat model accept tools.
        assert!(model_supports_tool_calling("openai/gpt-oss-120b"));
        assert!(model_supports_tool_calling("llama-3.3-70b-versatile"));
        assert!(model_supports_tool_calling("gpt-4o"));
        assert!(model_supports_tool_calling("gpt-5"));
        assert!(model_supports_tool_calling("deepseek-chat"));
    }

    #[test]
    fn compound_predicate_is_not_overgreedy() {
        // A model that merely CONTAINS "compound" but doesn't lead with it must
        // keep tools — only the Compound family is special-cased.
        assert!(model_supports_tool_calling("octo-compound-7b"));
        assert!(model_supports_tool_calling("acme/super-compound"));
    }
}
