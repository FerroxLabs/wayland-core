//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use base64::Engine as _;
use wcore_tools::vision_tools::{VisionBackend, VisionOutcome};

/// Gemini vision backend. Uses the `generateContent` endpoint with an
/// `inline_data` block. Free tier covers ~1500 requests/day on
/// `gemini-2.5-flash` at v0.6.
pub struct GeminiVisionBackend {
    client: Client,
    api_key: String,
    model: String,
    /// Endpoint base, up to and including `/v1beta/models`. Overridable in
    /// tests so the error path can be exercised against a mock host.
    endpoint_base: String,
}

const GEMINI_VISION_ENDPOINT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

impl GeminiVisionBackend {
    pub fn new(api_key: String) -> Self {
        let model = std::env::var("WAYLAND_VISION_MODEL")
            .unwrap_or_else(|_| "gemini-2.5-flash".to_string());
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            model,
            endpoint_base: GEMINI_VISION_ENDPOINT_BASE.to_string(),
        }
    }

    #[cfg(test)]
    fn with_endpoint_base(api_key: String, model: String, endpoint_base: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            model,
            endpoint_base,
        }
    }
}

#[async_trait]
impl VisionBackend for GeminiVisionBackend {
    async fn analyze(&self, mime: &'static str, bytes: &[u8], prompt: &str) -> VisionOutcome {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        // SECRETS-27: the API key rides in the `x-goog-api-key` header, NOT
        // the URL query string. A key in `?key=…` leaks into the reqwest
        // error's `Display` (which carries the URL) on any transport failure,
        // and that error becomes the tool result fed back into model context
        // and persisted to the session transcript.
        let url = format!("{}/{}:generateContent", self.endpoint_base, self.model);
        let body = serde_json::json!({
            "contents": [{
                "role": "user",
                "parts": [
                    { "inline_data": { "mime_type": mime, "data": b64 } },
                    { "text": prompt }
                ]
            }]
        });
        let resp = match self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(std::time::Duration::from_secs(60))
            .body(body.to_string())
            .send()
            .await
        {
            Ok(r) => r,
            // SECRETS-27: strip the URL from the error before formatting.
            // Even with the key out of the URL this avoids leaking the full
            // endpoint into model/log context; defense in depth.
            Err(e) => {
                return VisionOutcome::Err {
                    message: format!("gemini vision request failed: {}", e.redacted()),
                };
            }
        };
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return VisionOutcome::Err {
                message: format!(
                    "gemini vision returned HTTP {}: {}",
                    status.as_u16(),
                    txt.chars().take(400).collect::<String>()
                ),
            };
        }
        let parsed: serde_json::Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                return VisionOutcome::Err {
                    message: format!("gemini vision JSON parse failed: {e}"),
                };
            }
        };
        let analysis = parsed
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        if analysis.is_empty() {
            return VisionOutcome::Err {
                message: "gemini vision returned no text content".to_string(),
            };
        }
        VisionOutcome::Ok { analysis }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET_KEY: &str = "AIzaSyTEST_secrets27_leak_canary_value";

    /// SECRETS-27 regression: a transport failure during a vision call must
    /// NOT echo the API key (or a `key=` query param) into the returned
    /// `VisionOutcome::Err` message, since that message becomes the tool
    /// result fed back into model context and persisted to the transcript.
    #[tokio::test]
    async fn vision_send_error_message_omits_api_key() {
        // TEST-NET-1 (192.0.2.0/24, RFC 5737) is reserved for documentation
        // and guaranteed not to route — the POST fails fast with a transport
        // error whose `Display` historically carried the URL.
        let backend = GeminiVisionBackend::with_endpoint_base(
            SECRET_KEY.to_string(),
            "gemini-2.5-flash".to_string(),
            "http://192.0.2.1:9/v1beta/models".to_string(),
        );
        let outcome = backend
            .analyze("image/png", b"\x89PNG\r\n", "describe")
            .await;
        let message = match outcome {
            VisionOutcome::Err { message } => message,
            VisionOutcome::Ok { .. } => panic!("unreachable host must produce an error"),
        };
        assert!(
            !message.contains(SECRET_KEY),
            "error message leaked the API key: {message}"
        );
        assert!(
            !message.contains("key="),
            "error message leaked a key= query param: {message}"
        );
    }
}
