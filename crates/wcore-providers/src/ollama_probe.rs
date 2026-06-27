//! Best-effort probe of a local Ollama backend's `/api/show` endpoint to learn
//! whether a given model advertises tool / function-calling support.
//!
//! ## Why this exists
//!
//! Ollama is wired into wayland-core as an OpenAI-compatible provider, not a
//! distinct provider type. Many local models served by Ollama (and llama.cpp
//! style backends) do **not** support function calling, and sending a Chat
//! Completions request that carries a `tools` array to such a model returns a
//! hard `400`. The cheapest way to avoid that round-trip failure is to ask the
//! backend up front: Ollama's native `POST /api/show` returns a JSON document
//! whose `"capabilities"` array lists strings such as `"completion"`,
//! `"tools"`, and `"vision"`. A model that supports function calling lists
//! `"tools"`. Callers use this probe to strip `tools` *before* dispatching a
//! request that would otherwise 400.
//!
//! ## Best-effort contract
//!
//! The probe is purely advisory and **fail-open / optimistic**: every failure
//! mode (request error, non-success status, unparseable body, missing or
//! malformed `capabilities`) resolves to `None`, meaning "unknown — leave the
//! caller's behavior unchanged". Only an unambiguous answer from the backend
//! yields `Some(true)` / `Some(false)`. A probe must never be the reason tools
//! get blocked for a model that actually supports them.
//!
//! ## Testing
//!
//! NOTE: the pure helpers [`ollama_show_url`] and [`parse_tool_capability`] are
//! exhaustively unit-tested below. The async wrapper
//! [`probe_ollama_tool_support`] performs a live HTTP request, so it is not
//! unit-tested here (no mock server in this crate's unit scope) — it is covered
//! by manual / live testing against a running Ollama instance.

use serde_json::Value;
use wcore_egress::EgressClient;

/// Derive the Ollama native `/api/show` URL from the OpenAI-wire `base_url`
/// the provider is configured with.
///
/// Ollama's OpenAI-compatible surface is typically configured as
/// `http://localhost:11434/v1` (with or without a trailing slash, and
/// occasionally without the `/v1` segment at all). The native `/api/show`
/// endpoint lives at the host root, *not* under `/v1`, so we normalize back to
/// the root before appending it:
///
/// 1. trim trailing whitespace,
/// 2. strip one trailing `/`,
/// 3. strip a trailing `/v1` segment if present,
/// 4. strip any `/` the previous step exposed,
/// 5. append `/api/show`.
pub(crate) fn ollama_show_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end();
    let no_slash = trimmed.strip_suffix('/').unwrap_or(trimmed);
    let no_v1 = no_slash.strip_suffix("/v1").unwrap_or(no_slash);
    let root = no_v1.strip_suffix('/').unwrap_or(no_v1);
    format!("{root}/api/show")
}

/// Interpret an Ollama `/api/show` response body for tool support.
///
/// Returns:
/// * `Some(true)`  — `capabilities` is an array containing `"tools"`,
/// * `Some(false)` — `capabilities` is a present array that does *not* contain
///   `"tools"`,
/// * `None`        — `capabilities` is absent or not an array (unknown; the
///   caller stays optimistic).
///
/// The `"tools"` match is case-insensitive for robustness against backend
/// capitalization quirks.
pub(crate) fn parse_tool_capability(show_response: &Value) -> Option<bool> {
    let capabilities = show_response.get("capabilities")?.as_array()?;
    let has_tools = capabilities
        .iter()
        .filter_map(Value::as_str)
        .any(|cap| cap.eq_ignore_ascii_case("tools"));
    Some(has_tools)
}

/// Probe a local Ollama backend to discover whether `model` supports tool /
/// function calling.
///
/// Issues a single best-effort `POST {base_url-root}/api/show` with body
/// `{"model": <model>}` and interprets the response via
/// [`parse_tool_capability`]. There is intentionally **no retry**: a probe is
/// advisory and must stay cheap.
///
/// Returns `None` on any failure (request error, non-success status, body
/// parse failure, timeout, or unknown `capabilities`) so that a failed probe
/// never blocks tool use — the caller stays optimistic.
///
/// A hard 2s wall-clock cap wraps the whole probe. The shared streaming client
/// deliberately has no request-level timeout (only a 300s between-bytes read
/// timeout, tuned for token streaming), so without this cap a wedged or slow
/// `/api/show` could stall the first turn for seconds. A probe is advisory and
/// must stay cheap.
pub(crate) async fn probe_ollama_tool_support(
    client: &EgressClient,
    base_url: &str,
    model: &str,
) -> Option<bool> {
    let url = ollama_show_url(base_url);
    let body = serde_json::json!({ "model": model });

    let probe = async {
        let response = client.post(&url).json(&body).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let value = response.json::<Value>().await.ok()?;
        parse_tool_capability(&value)
    };

    match tokio::time::timeout(std::time::Duration::from_secs(2), probe).await {
        Ok(result) => result,
        Err(_) => {
            tracing::debug!(url = %url, "Ollama tool-capability probe timed out");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn show_url_strips_v1_suffix() {
        assert_eq!(
            ollama_show_url("http://localhost:11434/v1"),
            "http://localhost:11434/api/show"
        );
    }

    #[test]
    fn show_url_strips_v1_with_trailing_slash() {
        assert_eq!(
            ollama_show_url("http://localhost:11434/v1/"),
            "http://localhost:11434/api/show"
        );
    }

    #[test]
    fn show_url_handles_bare_host() {
        assert_eq!(
            ollama_show_url("http://localhost:11434"),
            "http://localhost:11434/api/show"
        );
    }

    #[test]
    fn show_url_handles_bare_host_with_trailing_slash() {
        assert_eq!(
            ollama_show_url("http://localhost:11434/"),
            "http://localhost:11434/api/show"
        );
    }

    #[test]
    fn show_url_preserves_custom_host_and_port() {
        assert_eq!(
            ollama_show_url("http://host:1234/v1"),
            "http://host:1234/api/show"
        );
    }

    #[test]
    fn show_url_trims_trailing_whitespace() {
        assert_eq!(
            ollama_show_url("http://localhost:11434/v1  "),
            "http://localhost:11434/api/show"
        );
    }

    #[test]
    fn parse_returns_true_when_tools_listed() {
        let resp = json!({ "capabilities": ["completion", "tools", "vision"] });
        assert_eq!(parse_tool_capability(&resp), Some(true));
    }

    #[test]
    fn parse_returns_false_when_array_lacks_tools() {
        let resp = json!({ "capabilities": ["completion", "vision"] });
        assert_eq!(parse_tool_capability(&resp), Some(false));
    }

    #[test]
    fn parse_matches_tools_case_insensitively() {
        let resp = json!({ "capabilities": ["Completion", "Tools"] });
        assert_eq!(parse_tool_capability(&resp), Some(true));
    }

    #[test]
    fn parse_returns_none_when_capabilities_absent() {
        let resp = json!({ "model": "llama3", "details": {} });
        assert_eq!(parse_tool_capability(&resp), None);
    }

    #[test]
    fn parse_returns_none_when_capabilities_not_an_array() {
        let resp = json!({ "capabilities": "tools" });
        assert_eq!(parse_tool_capability(&resp), None);
    }

    #[test]
    fn parse_returns_false_for_empty_capabilities_array() {
        let resp = json!({ "capabilities": [] });
        assert_eq!(parse_tool_capability(&resp), Some(false));
    }

    #[test]
    fn parse_ignores_non_string_entries() {
        // A malformed mixed array still resolves the `"tools"` string correctly.
        let resp = json!({ "capabilities": [1, true, "tools", null] });
        assert_eq!(parse_tool_capability(&resp), Some(true));
    }
}
