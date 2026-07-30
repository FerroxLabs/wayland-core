//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use super::shared::reported_cost;
use base64::Engine as _;
use wcore_tools::media_cost::{MediaAccounting, MediaCostRecord, MediaOutcome, MediaUnits};
use wcore_tools::vision_tools::{VisionBackend, VisionOutcome};

const ANTHROPIC_MESSAGES_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";

/// Billable units for one Anthropic vision call, read from the Messages API
/// `usage` block. A count the provider omitted stays `None` — reporting `0`
/// would claim it processed an empty prompt on a call it charged for.
fn units_from_response(parsed: &serde_json::Value) -> MediaUnits {
    let tokens = |p: &str| {
        parsed
            .pointer(p)
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
    };
    MediaUnits::tokens(
        tokens("/usage/input_tokens"),
        tokens("/usage/output_tokens"),
    )
}

/// Anthropic vision backend. Uses the Messages API with an `image`
/// content block; same `ANTHROPIC_API_KEY` the agent already uses for
/// chat — no separate signup.
pub struct AnthropicVisionBackend {
    client: Client,
    api_key: String,
    model: String,
    endpoint: String,
    /// 27-C3. Ledger + operator price list for the billable call this backend
    /// makes. Defaults to unbound, so every existing construction site is
    /// unchanged and behaviour is identical until a host binds one.
    accounting: MediaAccounting,
}

impl AnthropicVisionBackend {
    pub fn new(api_key: String) -> Self {
        // Default to Sonnet 4.6 — cheaper than Opus for image-look tasks
        // and still very strong at vision. Users can override via
        // `WAYLAND_VISION_MODEL` env var.
        let model = std::env::var("WAYLAND_VISION_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-6".to_string());
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            model,
            endpoint: ANTHROPIC_MESSAGES_ENDPOINT.to_string(),
            accounting: MediaAccounting::default(),
        }
    }

    /// 27-C3. Bind the session ledger and operator price list.
    pub fn with_accounting(mut self, accounting: MediaAccounting) -> Self {
        self.accounting = accounting;
        self
    }

    /// Override the Messages endpoint. Exists so the accounting path can be
    /// exercised against a mock host — the cost header and the `usage` block
    /// only exist on a real response, so asserting on them requires serving
    /// one.
    #[cfg(test)]
    pub(crate) fn with_endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }
}

#[async_trait]
impl VisionBackend for AnthropicVisionBackend {
    async fn analyze(&self, mime: &'static str, bytes: &[u8], prompt: &str) -> VisionOutcome {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": mime,
                            "data": b64,
                        }
                    },
                    { "type": "text", "text": prompt }
                ]
            }]
        });
        let resp = match self
            .client
            .post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(std::time::Duration::from_secs(60))
            .body(body.to_string())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // The request never reached the provider, so nothing was
                // billed and there is no record to make.
                return VisionOutcome::Err {
                    message: format!("anthropic vision request failed: {e}"),
                };
            }
        };
        let status = resp.status();
        // 27-C3: read the cost header BEFORE `resp.text()` consumes the
        // response. This was previously discarded entirely.
        let header_cost = reported_cost(resp.headers(), None);
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            // The provider was reached and rejected the call. Whether it
            // billed is unknown — a rejected request can still be charged —
            // so this records `CallFailedBillingUnknown`, never $0.00.
            self.accounting.account(MediaCostRecord::for_failure(
                "vision_analyze",
                "anthropic",
                &self.model,
                MediaUnits::tokens(None, None),
                format!("http_{}", status.as_u16()),
            ));
            return VisionOutcome::Err {
                message: format!(
                    "anthropic vision returned HTTP {}: {}",
                    status.as_u16(),
                    txt.chars().take(400).collect::<String>()
                ),
            };
        }
        let parsed: serde_json::Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                // HTTP 200 — the provider did the work and billed for it; we
                // simply could not read the reply. Recording this as anything
                // other than billable would understate real spend.
                self.accounting.account(MediaCostRecord::for_failure(
                    "vision_analyze",
                    "anthropic",
                    &self.model,
                    MediaUnits::tokens(None, None),
                    "response_parse_failed",
                ));
                return VisionOutcome::Err {
                    message: format!("anthropic vision JSON parse failed: {e}"),
                };
            }
        };
        let record = MediaCostRecord::for_success(
            "vision_analyze",
            "anthropic",
            &self.model,
            units_from_response(&parsed),
            header_cost.or_else(|| super::shared::cost_from_body(&parsed)),
            &self.accounting.rate_card,
        );
        let analysis = parsed
            .pointer("/content/0/text")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        if analysis.is_empty() {
            // A product-side rejection of work the provider already performed
            // and billed for, so the record keeps its resolved price and only
            // the outcome changes.
            self.accounting
                .account(record.with_outcome(MediaOutcome::Failed {
                    category: "empty_response".to_string(),
                }));
            return VisionOutcome::Err {
                message: "anthropic vision returned no text content".to_string(),
            };
        }
        self.accounting.account(record);
        VisionOutcome::Ok { analysis }
    }
}
