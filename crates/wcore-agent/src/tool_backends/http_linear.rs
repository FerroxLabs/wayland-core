//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use serde_json::Value;
use wcore_egress::EgressClient as Client;

use super::{build_ssrf_safe_tool_client, error_message, parse_json_or_raw};
use wcore_tools::linear_tool::{LinearBackend, LinearOutcome, LinearRequest};

// ---------------------------------------------------------------------
// Linear (GraphQL).
// ---------------------------------------------------------------------

/// Real Linear GraphQL backend over `reqwest`.
pub struct HttpLinearBackend {
    client: Client,
}

impl HttpLinearBackend {
    /// New backend with the non-streaming HTTP timeout policy (AUDIT B-5)
    /// plus the SSRF-resistant redirect policy (#279) — see
    /// [`build_ssrf_safe_tool_client`].
    pub fn new() -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
        }
    }
}

impl Default for HttpLinearBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LinearBackend for HttpLinearBackend {
    async fn dispatch(&self, request: &LinearRequest) -> LinearOutcome {
        // Linear's API is GraphQL-over-HTTP — always a POST.
        let mut builder = self.client.post(&request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        builder = builder.json(&request.body);
        let response = match builder.send().await {
            Ok(r) => r,
            Err(e) => {
                return LinearOutcome::Err {
                    message: format!("Linear request transport error: {e}"),
                };
            }
        };
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let payload = parse_json_or_raw(&text);
            return LinearOutcome::HttpError {
                status: status.as_u16(),
                message: error_message(
                    &payload,
                    &format!("Linear returned HTTP {}", status.as_u16()),
                ),
            };
        }
        let payload = parse_json_or_raw(&text);
        // GraphQL semantics: a 200 can still carry a non-empty `errors`
        // array. Surface that as a distinct GraphQlError outcome.
        if let Some(errors) = payload.get("errors").and_then(Value::as_array)
            && !errors.is_empty()
        {
            let joined = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("; ");
            let message = if joined.is_empty() {
                "Linear GraphQL returned an errors array".to_string()
            } else {
                joined
            };
            return LinearOutcome::GraphQlError { message };
        }
        LinearOutcome::Ok { payload }
    }
}
