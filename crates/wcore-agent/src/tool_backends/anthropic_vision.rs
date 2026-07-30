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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use wcore_tools::media_cost::{MediaCostLedger, PriceSource, UnpricedReason};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A realistic Anthropic Messages reply, with the `usage` block a real one
    /// carries.
    fn messages_body() -> serde_json::Value {
        serde_json::json!({
            "content": [{ "type": "text", "text": "a red bicycle against a wall" }],
            "usage": { "input_tokens": 1842, "output_tokens": 310 }
        })
    }

    async fn analyze_against(template: ResponseTemplate) -> (Arc<MediaCostLedger>, VisionOutcome) {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(template)
            .mount(&server)
            .await;
        let ledger = MediaCostLedger::shared();
        let backend = AnthropicVisionBackend::new("sk-ant-test".to_string())
            .with_endpoint(format!("{}/v1/messages", server.uri()))
            .with_accounting(MediaAccounting::new(
                Arc::clone(&ledger),
                Default::default(),
            ));
        let outcome = backend
            .analyze("image/png", b"\x89PNG\r\n", "describe")
            .await;
        (ledger, outcome)
    }

    /// **Both directions in one test.** A billed vision call must record a
    /// non-zero provider figure; the SAME call served without the cost header
    /// must record `unpriced`, never `$0.00`.
    ///
    /// The negative half alone would pass on a backend that records nothing at
    /// all, and the positive half alone would pass on one that cannot tell
    /// "free" from "unknown". Together they pin both.
    #[tokio::test]
    async fn billed_vision_call_records_a_nonzero_provider_cost() {
        // --- direction 1: provider reports a cost -> it is recorded, non-zero
        let (ledger, outcome) = analyze_against(
            ResponseTemplate::new(200)
                .insert_header("x-flux-cost-usd", "0.004210")
                .set_body_json(messages_body()),
        )
        .await;
        assert!(
            matches!(outcome, VisionOutcome::Ok { .. }),
            "the mocked 200 must succeed: {outcome:?}"
        );
        let records = ledger.snapshot();
        assert_eq!(records.len(), 1, "exactly one billable call was made");
        let r = &records[0];
        assert_eq!(
            r.cost_usd,
            Some(0.004_210),
            "the provider's figure must land"
        );
        assert!(
            r.cost_usd.expect("priced") > 0.0,
            "a billed call must not record as zero"
        );
        assert!(
            r.price_source.is_provider_reported(),
            "must be labelled provider-reported, got {:?}",
            r.price_source
        );
        // The units the provider actually reported, not a placeholder.
        assert_eq!(r.units.input_tokens, Some(1842));
        assert_eq!(r.units.output_tokens, Some(310));
        assert!(r.units.is_token_billed(), "vision is token-billed");
        assert_eq!(r.units.images, 0, "a vision call produces no artifact");
        assert_eq!(ledger.summary().total_usd, 0.004_210);

        // --- direction 2: same call, no cost header -> unpriced, NOT $0.00
        let (ledger2, _) =
            analyze_against(ResponseTemplate::new(200).set_body_json(messages_body())).await;
        let r2 = &ledger2.snapshot()[0];
        assert_eq!(
            r2.cost_usd, None,
            "absent header must yield no figure, not zero"
        );
        assert_eq!(
            r2.price_source,
            PriceSource::Unpriced {
                reason: UnpricedReason::ProviderReportsNoCost
            }
        );
        assert!(
            !r2.summary_line().contains("$0.00"),
            "{}",
            r2.summary_line()
        );
        // ...but the units are still recorded, which is the whole point of
        // separating units from price.
        assert_eq!(r2.units.input_tokens, Some(1842));
        assert_eq!(
            ledger2.summary().unpriced_calls,
            1,
            "an unpriced call must be counted, not dropped"
        );
    }

    /// A rejected call is billable-unknown, never free. Providers do charge for
    /// rejected prompts.
    #[tokio::test]
    async fn vision_http_failure_records_billing_unknown_not_zero() {
        let (ledger, outcome) = analyze_against(
            ResponseTemplate::new(429).set_body_string("{\"error\":\"rate limited\"}"),
        )
        .await;
        assert!(matches!(outcome, VisionOutcome::Err { .. }));
        let records = ledger.snapshot();
        assert_eq!(records.len(), 1, "a reached-and-rejected call is recorded");
        let r = &records[0];
        assert_eq!(r.cost_usd, None);
        assert_eq!(
            r.price_source,
            PriceSource::Unpriced {
                reason: UnpricedReason::CallFailedBillingUnknown
            }
        );
        assert_eq!(
            r.outcome,
            MediaOutcome::Failed {
                category: "http_429".to_string()
            }
        );
        assert!(
            r.summary_line().contains("unknown whether the provider"),
            "{}",
            r.summary_line()
        );
    }

    /// Known-negative for the ledger wiring itself: with no accounting bound
    /// the call still works and records nowhere. Without this, every assertion
    /// above could be satisfied by a backend that records unconditionally into
    /// some global.
    #[tokio::test]
    async fn unbound_accounting_records_nothing_but_still_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-flux-cost-usd", "0.004210")
                    .set_body_json(messages_body()),
            )
            .mount(&server)
            .await;
        // A ledger that is deliberately NOT handed to the backend.
        let unbound = MediaCostLedger::new();
        let backend = AnthropicVisionBackend::new("sk-ant-test".to_string())
            .with_endpoint(format!("{}/v1/messages", server.uri()));
        let outcome = backend
            .analyze("image/png", b"\x89PNG\r\n", "describe")
            .await;
        assert!(matches!(outcome, VisionOutcome::Ok { .. }));
        assert_eq!(
            unbound.summary().calls,
            0,
            "an unbound ledger must stay empty"
        );
    }
}
