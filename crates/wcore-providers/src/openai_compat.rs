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
pub fn wants_max_completion_tokens(model: &str) -> bool {
    let m = lower(model);
    is_o_series(&m) || is_gpt5(&m)
}

/// True when the model accepts a `reasoning_effort` field. Mirrors
/// `wants_max_completion_tokens` — same set of families.
pub fn accepts_reasoning_effort(model: &str) -> bool {
    let m = lower(model);
    is_o_series(&m) || is_gpt5(&m)
}

/// True when the model accepts an explicit `temperature`. False for the
/// `o1*` / `o3*` reasoning families (which fix `temperature` at `1.0`),
/// true everywhere else — including `gpt-5*`, which still honors it.
pub fn accepts_temperature(model: &str) -> bool {
    let m = lower(model);
    !is_o_series(&m)
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
}
