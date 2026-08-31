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

// ---------------------------------------------------------------------------
// #1230 c3 -- the SERVED slot, stated by the endpoint before we send to it.
// ---------------------------------------------------------------------------

/// Derive the Ollama native `/api/ps` URL from the OpenAI-wire `base_url`,
/// with the same normalization [`ollama_show_url`] applies.
pub(crate) fn ollama_ps_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end();
    let no_slash = trimmed.strip_suffix('/').unwrap_or(trimmed);
    let no_v1 = no_slash.strip_suffix("/v1").unwrap_or(no_slash);
    let root = no_v1.strip_suffix('/').unwrap_or(no_v1);
    format!("{root}/api/ps")
}

/// Read the SERVED context slot for `model` out of an Ollama `/api/ps` body.
///
/// # Why `/api/ps` and not `/api/show`
///
/// They answer different questions and #1172 measured them disagreeing by 10x
/// on the same box. `/api/show` reports what the MODEL advertises
/// (`qwen3.context_length = 40960`); `/api/ps` reports the slot the RUNNING
/// instance was actually loaded with (`context_length = 4096` on a stock
/// service with no `OLLAMA_CONTEXT_LENGTH`). Only the second one binds, and
/// only the second one is what silently discards the head of an oversized
/// prompt. Reading the advertised figure here would be worse than reading
/// nothing: it would manufacture confidence in a window that does not exist.
///
/// # Fail-open
///
/// Returns `None` for every ambiguity -- no such model loaded, absent or
/// non-numeric `context_length`, a zero. `None` means "unknown", and an
/// unknown slot must never be the reason a run is refused.
pub(crate) fn parse_served_window(ps_response: &Value, model: &str) -> Option<u64> {
    let running = ps_response.get("models")?.as_array()?;
    running
        .iter()
        .find(|entry| {
            ["name", "model"].iter().any(|key| {
                entry
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|value| value == model)
            })
        })
        .and_then(|entry| entry.get("context_length"))
        .and_then(Value::as_u64)
        .filter(|slot| *slot > 0)
}

/// Ask a local Ollama backend what context slot `model` is currently loaded
/// with, or `None` when it will not say.
///
/// # Why this probe is allowed where two earlier ones were backed out
///
/// [`wcore_config::context_window::ServedWindowTracker`] records that reaching
/// for this figure was tried and reverted twice, because "probing the endpoint
/// means deciding WHICH endpoints to probe" and every mock server in this
/// workspace binds `127.0.0.1`, so loopback cannot separate a real self-hosted
/// server from a test fixture. That objection is about SNIFFING, and it is
/// already answered elsewhere in this file: [`probe_ollama_tool_support`] is
/// gated on `ProviderCompat::provider_type() == "ollama"`, which is what the
/// OPERATOR declared their endpoint to be, not something inferred from an
/// address. This probe is gated the same way and inherits the same answer.
///
/// The tracker keeps its job. It learns from `usage` after the fact and needs
/// no cooperation from the endpoint; it just cannot answer BEFORE the first
/// request, which is what #1230 c4 asks for. The two are complementary: this
/// one is consulted first and the tracker remains the fallback for every
/// endpoint that does not answer.
///
/// Best-effort and advisory, exactly like the tool-capability probe: one GET,
/// no retry, 2s ceiling, and every failure mode resolves to `None`.
pub async fn probe_ollama_served_window(
    client: &EgressClient,
    base_url: &str,
    model: &str,
) -> Option<u64> {
    let url = ollama_ps_url(base_url);
    let probe = async {
        let response = client.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let body: Value = response.json().await.ok()?;
        parse_served_window(&body, model)
    };
    match tokio::time::timeout(std::time::Duration::from_secs(2), probe).await {
        Ok(result) => result,
        Err(_) => {
            tracing::debug!(url = %url, "Ollama served-window probe timed out");
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

    // --- #1230 c3: the served-slot probe -------------------------------

    #[test]
    fn ps_url_normalizes_the_same_way_show_does() {
        for base in [
            "http://localhost:11434/v1",
            "http://localhost:11434/v1/",
            "http://localhost:11434/",
            "http://localhost:11434",
        ] {
            assert_eq!(ollama_ps_url(base), "http://localhost:11434/api/ps");
        }
    }

    /// The shape a real Ollama 0.30.7 returns, captured on hetzner-dsm from a
    /// private instance on port 21434 with qwen3:8b loaded at the stock slot.
    #[test]
    fn served_window_reads_the_loaded_slot() {
        let body = json!({
            "models": [{
                "name": "qwen3:8b",
                "model": "qwen3:8b",
                "size": 5225388164u64,
                "context_length": 4096,
            }]
        });
        assert_eq!(parse_served_window(&body, "qwen3:8b"), Some(4_096));
    }

    /// Every ambiguity is `None`. A probe that guessed here would refuse runs
    /// against endpoints that are perfectly healthy -- the wrong-refusal
    /// failure, which is worse than the truncation it is trying to prevent.
    #[test]
    fn served_window_fails_open_on_every_ambiguity() {
        // No model loaded at all -- the state a cold server is in, and the
        // state a stock server returns to after its 5-minute idle unload.
        assert_eq!(
            parse_served_window(&json!({"models": []}), "qwen3:8b"),
            None
        );
        // A DIFFERENT model is loaded; its slot says nothing about ours.
        let other = json!({"models": [{"name": "llama3:8b", "context_length": 8192}]});
        assert_eq!(parse_served_window(&other, "qwen3:8b"), None);
        // Field absent (an older Ollama), non-numeric, or zero.
        for entry in [
            json!({"name": "qwen3:8b"}),
            json!({"name": "qwen3:8b", "context_length": "4096"}),
            json!({"name": "qwen3:8b", "context_length": 0}),
        ] {
            let body = json!({ "models": [entry] });
            assert_eq!(parse_served_window(&body, "qwen3:8b"), None);
        }
        // Not an Ollama response at all.
        assert_eq!(
            parse_served_window(&json!({"object": "list"}), "qwen3:8b"),
            None
        );
    }

    /// POSITIVE CONTROL for the test above: the same helper, on the same
    /// document shape, DOES answer when the answer is unambiguous. Without
    /// this, a `parse_served_window` that returned `None` unconditionally
    /// would pass every fail-open assertion.
    #[test]
    fn served_window_control_answers_when_unambiguous() {
        let body = json!({
            "models": [
                {"name": "llama3:8b", "context_length": 8192},
                {"name": "qwen3:8b", "context_length": 4096},
            ]
        });
        assert_eq!(parse_served_window(&body, "llama3:8b"), Some(8_192));
        assert_eq!(parse_served_window(&body, "qwen3:8b"), Some(4_096));
    }
}
