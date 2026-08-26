//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use super::shared::{http_error_detail, reported_cost};
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
                    http_error_detail("vision", status.as_u16(), &txt)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use wcore_tools::media_cost::{MediaCostLedger, PriceSource};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A FluxRouter chat-completions reply. Flux reports the per-call cost in
    /// `usage.cost_usd` **as well as** in the header — this fixture carries
    /// only the body figure, so it exercises the body channel specifically.
    fn flux_chat_body(cost_usd: Option<f64>) -> serde_json::Value {
        let mut usage = serde_json::json!({
            "prompt_tokens": 1500,
            "completion_tokens": 240
        });
        if let Some(c) = cost_usd {
            usage["cost_usd"] = serde_json::json!(c);
        }
        serde_json::json!({
            "choices": [{ "message": { "content": "a red bicycle" } }],
            "usage": usage
        })
    }

    async fn analyze_against(body: serde_json::Value) -> Arc<MediaCostLedger> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let ledger = MediaCostLedger::shared();
        let backend = OpenAiVisionBackend::with_endpoint(
            "flux-test-key".to_string(),
            format!("{}/v1/chat/completions", server.uri()),
            "gpt-4o".to_string(),
            "flux-router",
        )
        .with_accounting(MediaAccounting::new(
            Arc::clone(&ledger),
            Default::default(),
        ));
        let outcome = backend
            .analyze("image/png", b"\x89PNG\r\n", "describe")
            .await;
        assert!(
            matches!(outcome, VisionOutcome::Ok { .. }),
            "mocked 200 must succeed: {outcome:?}"
        );
        ledger
    }

    /// **Both directions.** FluxRouter's `usage.cost_usd` must be read off the
    /// body and recorded as a real, non-zero, provider-reported figure — and
    /// the same reply without that field must record `unpriced`, not `$0.00`.
    ///
    /// This is the arm that matters most for `video_analyze`: it fans out to
    /// nine of these, and this backend is the one measured to receive a genuine
    /// provider figure.
    #[tokio::test]
    async fn flux_vision_cost_is_read_from_the_response_body() {
        // --- direction 1: body carries a cost
        let ledger = analyze_against(flux_chat_body(Some(0.004_21))).await;
        let records = ledger.snapshot();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.cost_usd, Some(0.004_21), "body figure must be recorded");
        assert!(r.cost_usd.expect("priced") > 0.0, "must be non-zero");
        assert_eq!(
            r.price_source,
            PriceSource::ProviderBody {
                field: "usage.cost_usd".to_string()
            },
            "must name the channel it came from"
        );
        assert!(r.price_source.is_provider_reported());
        assert_eq!(r.units.input_tokens, Some(1500));
        assert_eq!(r.units.output_tokens, Some(240));

        // --- direction 2: identical reply, cost field absent
        let ledger2 = analyze_against(flux_chat_body(None)).await;
        let r2 = &ledger2.snapshot()[0];
        assert_eq!(r2.cost_usd, None, "absent field must not become zero");
        assert!(
            !r2.summary_line().contains("$0.00"),
            "{}",
            r2.summary_line()
        );
        // Units still recorded, so the call is not invisible just because it
        // is unpriced.
        assert_eq!(r2.units.total_tokens(), Some(1740));
    }

    /// The header outranks the body when both are present, and the record says
    /// which one it used. An operator reconciling a bill needs to know.
    #[tokio::test]
    async fn header_outranks_body_and_the_record_names_the_channel() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-flux-cost-usd", "0.009")
                    .set_body_json(flux_chat_body(Some(0.004_21))),
            )
            .mount(&server)
            .await;
        let ledger = MediaCostLedger::shared();
        let backend = OpenAiVisionBackend::with_endpoint(
            "flux-test-key".to_string(),
            format!("{}/v1/chat/completions", server.uri()),
            "gpt-4o".to_string(),
            "flux-router",
        )
        .with_accounting(MediaAccounting::new(
            Arc::clone(&ledger),
            Default::default(),
        ));
        let _ = backend
            .analyze("image/png", b"\x89PNG\r\n", "describe")
            .await;
        let r = &ledger.snapshot()[0];
        assert_eq!(r.cost_usd, Some(0.009), "header wins");
        assert_eq!(
            r.price_source,
            PriceSource::ProviderHeader {
                header: "x-flux-cost-usd".to_string()
            }
        );
    }

    /// #938. A FluxRouter 402 on the vision (chat-completions) endpoint is the
    /// same entitlement signal the image path maps to `PremiumLocked`, and must
    /// read the same way rather than as a wall of provider JSON.
    #[tokio::test]
    async fn flux_402_on_vision_yields_the_typed_entitlement_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
                "error": {
                    "message": "vision is available on paid plans only",
                    "code": "premium_locked"
                }
            })))
            .mount(&server)
            .await;
        let backend = OpenAiVisionBackend::with_endpoint(
            "flux-test-key".to_string(),
            format!("{}/v1/chat/completions", server.uri()),
            "flux-auto".to_string(),
            "flux-router",
        );
        let msg = match backend
            .analyze("image/png", b"\x89PNG\r\n", "describe")
            .await
        {
            VisionOutcome::Err { message } => message,
            other => panic!("a 402 must not succeed: {other:?}"),
        };
        assert!(msg.contains("requires a paid Flux plan"), "got: {msg}");
        assert!(
            !msg.contains("\"code\""),
            "the provider's raw JSON envelope must not reach the user, got: {msg}"
        );
        assert!(
            msg.contains("402"),
            "video_analyze classifies InsufficientCredits by string-matching the \
             status, so it must survive: {msg}"
        );
    }
}
