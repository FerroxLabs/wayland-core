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

/// Billable units for one OpenAI-wire vision call, read from the
/// chat-completions `usage` block. A count the provider omitted stays `None`.
fn units_from_response(parsed: &serde_json::Value) -> MediaUnits {
    let tokens = |p: &str| {
        parsed
            .pointer(p)
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
    };
    MediaUnits::tokens(
        tokens("/usage/prompt_tokens"),
        tokens("/usage/completion_tokens"),
    )
}

/// OpenAI-**compatible** vision backend. Drives native OpenAI (GPT-4o) and
/// our FluxRouter, since both serve the same chat-completions API shape with
/// an `image_url` content block carrying a base64 `data:` URL.
///
/// **The `endpoint` field is the #310-class fix for vision** (`BL-F24-C3-H7`).
/// This backend previously hardcoded `https://api.openai.com/v1/chat/completions`
/// while accepting a caller-supplied `api_key`, so configuring a non-OpenAI
/// key was not merely unsupported — it would have **sent that credential to a
/// third party** instead of failing closed. Key and host are now always
/// resolved together, by the same resolver, from the same source.
pub struct OpenAiVisionBackend {
    client: Client,
    api_key: String,
    endpoint: String,
    model: String,
    backend_id: &'static str,
    /// 27-C3. Ledger + operator price list. Unbound by default.
    ///
    /// This backend is the one vision arm measured to receive a real
    /// provider-reported figure: it serves FluxRouter as well as native
    /// OpenAI, and FluxRouter returns `x-flux-cost-usd` (and `usage.cost_usd`)
    /// on chat completions, which a vision call is.
    accounting: MediaAccounting,
}

impl OpenAiVisionBackend {
    /// Native OpenAI at `api.openai.com`. Preserves the pre-existing
    /// behaviour of the `OPENAI_API_KEY` env arm exactly.
    pub fn new(api_key: String) -> Self {
        let model = std::env::var("WAYLAND_VISION_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
        Self::with_endpoint(
            api_key,
            super::shared::join_openai_endpoint(super::shared::OPENAI_API_BASE, "chat/completions"),
            model,
            "openai",
        )
    }

    /// Explicit endpoint + model, for a resolver that derived both from
    /// `Config` (mirrors [`super::openai_compat_whisper::OpenAiCompatWhisperBackend::new`]).
    pub fn with_endpoint(
        api_key: String,
        endpoint: String,
        model: String,
        backend_id: &'static str,
    ) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint,
            model,
            backend_id,
            accounting: MediaAccounting::default(),
        }
    }

    /// 27-C3. Bind the session ledger and operator price list.
    pub fn with_accounting(mut self, accounting: MediaAccounting) -> Self {
        self.accounting = accounting;
        self
    }

    /// Resolved request endpoint. Exposed so the resolver wiring is
    /// unit-assertable **without a network round-trip** — the property that
    /// matters here is "the key never leaves with the wrong host", and that is
    /// checkable offline.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Model sent in the request body.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Backend label used in log lines and error messages.
    pub fn backend_id(&self) -> &str {
        self.backend_id
    }
}

#[async_trait]
impl VisionBackend for OpenAiVisionBackend {
    async fn analyze(&self, mime: &'static str, bytes: &[u8], prompt: &str) -> VisionOutcome {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let data_url = format!("data:{mime};base64,{b64}");
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image_url", "image_url": { "url": data_url } },
                    { "type": "text", "text": prompt }
                ]
            }]
        });
        let resp = match self
            .client
            .post(&self.endpoint)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(std::time::Duration::from_secs(60))
            .body(body.to_string())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return VisionOutcome::Err {
                    message: format!("{} vision request failed: {e}", self.backend_id),
                };
            }
        };
        let status = resp.status();
        // 27-C3: read the cost header BEFORE `resp.text()` consumes the
        // response. FluxRouter reports the real figure here and it was being
        // discarded.
        let header_cost = reported_cost(resp.headers(), None);
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            // Provider reached and the call rejected; whether it billed is
            // unknown, so this is never $0.00.
            self.accounting.account(MediaCostRecord::for_failure(
                "vision_analyze",
                self.backend_id,
                &self.model,
                MediaUnits::tokens(None, None),
                format!("http_{}", status.as_u16()),
            ));
            return VisionOutcome::Err {
                message: format!(
                    "{} vision returned HTTP {}: {}",
                    self.backend_id,
                    status.as_u16(),
                    txt.chars().take(400).collect::<String>()
                ),
            };
        }
        let parsed: serde_json::Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                // HTTP 200 — billed, but unreadable.
                self.accounting.account(MediaCostRecord::for_failure(
                    "vision_analyze",
                    self.backend_id,
                    &self.model,
                    MediaUnits::tokens(None, None),
                    "response_parse_failed",
                ));
                return VisionOutcome::Err {
                    message: format!("{} vision JSON parse failed: {e}", self.backend_id),
                };
            }
        };
        let record = MediaCostRecord::for_success(
            "vision_analyze",
            self.backend_id,
            &self.model,
            units_from_response(&parsed),
            header_cost.or_else(|| super::shared::cost_from_body(&parsed)),
            &self.accounting.rate_card,
        );
        let analysis = parsed
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        if analysis.is_empty() {
            self.accounting
                .account(record.with_outcome(MediaOutcome::Failed {
                    category: "empty_response".to_string(),
                }));
            return VisionOutcome::Err {
                message: format!("{} vision returned no text content", self.backend_id),
            };
        }
        self.accounting.account(record);
        VisionOutcome::Ok { analysis }
    }
}
