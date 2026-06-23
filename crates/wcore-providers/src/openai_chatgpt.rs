//! "Sign in with ChatGPT" (OpenAI Codex OAuth) inference provider.
//!
//! Routes turns through the ChatGPT **Codex** backend
//! (`https://chatgpt.com/backend-api/codex/responses`) using the OpenAI
//! **Responses API** wire format, authenticated by a per-request OAuth bearer
//! rather than a static API key. The provider is intentionally free of any
//! `wcore-agent`/OAuth dependency: the OAuth token manager lives in
//! `wcore-agent` and is injected here as an **async** closure
//! ([`AsyncBearerSource`]). The closure is awaited at the top of [`stream`]
//! (where the OAuth refresh round-trip can happen), then its credentials are
//! folded into the request headers.
//!
//! Everything below the bearer seam is reused verbatim from the existing
//! Responses transport: [`build_responses_body`](crate::openai_responses::build_responses_body)
//! produces the body, [`process_responses_sse_stream`](crate::openai::process_responses_sse_stream)
//! parses the SSE stream into [`LlmEvent`]s. The only provider-local body
//! adjustments are the Codex-specific deltas documented on [`stream`].
//!
//! ## Reference
//!
//! Wire details mirrored from the OpenAI-maintained Codex client.
//!
//! [`stream`]: OpenAIChatGptProvider::stream

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::openai::process_responses_sse_stream;
use crate::openai_responses::build_responses_body;
use crate::retry::builder_send_with_retry;
use crate::{LlmProvider, ProviderError};

/// The OAuth-resolved credentials a single Codex request needs: the bearer
/// access token and the ChatGPT account id (decoded from the access-token JWT
/// upstream in `wcore-agent`).
#[derive(Clone)]
pub struct BearerCreds {
    pub access_token: String,
    pub account_id: String,
}

/// An async source of [`BearerCreds`].
///
/// Async because an OAuth refresh is a network round-trip and the sync Azure
/// `AzureTokenSource` pattern cannot `await`. The closure is invoked at the top
/// of [`OpenAIChatGptProvider::stream`] — before headers are built — so a
/// near-expiry token is refreshed transparently on every turn.
pub type AsyncBearerSource = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<BearerCreds, ProviderError>> + Send>>
        + Send
        + Sync,
>;

/// The ChatGPT Codex inference backend base URL. The `/responses` path is
/// appended by [`OpenAIChatGptProvider::responses_url`].
pub const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// Default `instructions` injected when a request carries no system prompt.
///
/// Codex always sends a non-empty
/// `instructions` field; `build_responses_body` omits it when `request.system`
/// is empty. D4: ensure it is always present.
const DEFAULT_INSTRUCTIONS: &str = "You are a helpful assistant.";

/// OAuth-authenticated provider for the ChatGPT Codex Responses backend.
pub struct OpenAIChatGptProvider {
    client: wcore_egress::EgressClient,
    bearer: AsyncBearerSource,
    /// Defaults to [`CODEX_BASE_URL`]; overridable in tests via
    /// [`OpenAIChatGptProvider::with_base_url`].
    base_url: String,
    compat: ProviderCompat,
    debug: DebugConfig,
}

impl OpenAIChatGptProvider {
    /// Build a provider over an async OAuth bearer source. Uses the shared
    /// streaming HTTP client policy (30s connect, 300s between-bytes read,
    /// redirects disabled).
    pub fn new(bearer: AsyncBearerSource, compat: ProviderCompat, debug: DebugConfig) -> Self {
        Self {
            client: crate::http_client::build(),
            bearer,
            base_url: CODEX_BASE_URL.to_string(),
            compat,
            debug,
        }
    }

    /// Test-only override of the Codex base URL so a mock SSE server can stand
    /// in for `chatgpt.com`. Production always uses [`CODEX_BASE_URL`].
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// The Codex `responses` endpoint: `<base>/responses`.
    pub(crate) fn responses_url(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }

    /// Build the Codex request headers from the OAuth credentials.
    ///
    /// Beyond the standard `Authorization`/`Content-Type`, Codex requires
    /// `chatgpt-account-id` (from the access-token JWT), `OpenAI-Beta:
    /// responses=experimental`, our honest `originator: wayland` attribution,
    /// a wayland `User-Agent`, and (D3) `Accept: text/event-stream` so the edge
    /// content-negotiates the SSE stream.
    pub(crate) fn build_headers(
        &self,
        creds: &BearerCreds,
    ) -> Result<reqwest::header::HeaderMap, ProviderError> {
        use reqwest::header::{
            ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT,
        };
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", creds.access_token))
                .map_err(|e| ProviderError::Connection(format!("bad bearer: {e}")))?,
        );
        h.insert(
            "chatgpt-account-id",
            HeaderValue::from_str(&creds.account_id)
                .map_err(|e| ProviderError::Connection(format!("bad account id: {e}")))?,
        );
        h.insert(
            "OpenAI-Beta",
            HeaderValue::from_static("responses=experimental"),
        );
        h.insert("originator", HeaderValue::from_static("wayland"));
        h.insert(
            USER_AGENT,
            HeaderValue::from_static(concat!("wayland-core/", env!("CARGO_PKG_VERSION"))),
        );
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        Ok(h)
    }

    /// Build the Codex Responses body: the shared builder plus the
    /// Codex-specific adjustments (kept here so the shared builder stays
    /// provider-neutral).
    ///
    /// * A2 — strip `max_output_tokens` (the Codex backend rejects it).
    /// * D2 — we deliberately do NOT request
    ///   `include: ["reasoning.encrypted_content"]`. Codex seals each
    ///   `reasoning` item to the `function_call` that follows it; if we
    ///   requested encrypted reasoning we would have to replay that item ahead
    ///   of the paired function_call on the next turn, or the backend rejects
    ///   turn 2 of a tool loop with `missing_following_item`. We don't yet
    ///   round-trip reasoning (`ContentBlock::Thinking` is a bare string and
    ///   `push_assistant_items` drops reasoning), so NOT requesting it keeps the
    ///   multi-turn history self-consistent. Full reasoning round-trip (carry
    ///   the encrypted blob paired with each function_call) is a follow-up.
    /// * D4 — ensure `instructions` is always present.
    /// * D5 — when tools are present, send `tool_choice: "auto"` +
    ///   `parallel_tool_calls: true` (matches the reference client).
    fn build_codex_body(&self, request: &LlmRequest) -> serde_json::Value {
        let mut body = build_responses_body(request, &self.compat);
        if let Some(obj) = body.as_object_mut() {
            // A2: Codex rejects max_output_tokens on this backend.
            obj.remove("max_output_tokens");
            // D4: instructions is unconditional on Codex.
            if !obj.contains_key("instructions") {
                obj.insert("instructions".to_string(), json!(DEFAULT_INSTRUCTIONS));
            }
            // D5: tool routing flags ride alongside a non-empty tools array.
            if !request.tools.is_empty() {
                obj.insert("tool_choice".to_string(), json!("auto"));
                obj.insert("parallel_tool_calls".to_string(), json!(true));
            }
        }
        body
    }
}

/// #158 reactive fallback: turn a Codex backend rejection that is *clearly* a
/// "your plan can't run this model" refusal into a clear, actionable message.
///
/// Returns `Some(message)` ONLY when the rejection body unambiguously concerns
/// the model/plan (a recognised OpenAI error `code` such as `model_not_found` /
/// `model_not_supported`, or a body phrase naming a model/access/plan problem).
/// In every other case it returns `None` so the caller passes the raw body
/// through unchanged — we do NOT fabricate plan-gate detection out of a generic
/// 4xx, because the Codex backend's plan refusal is not cleanly distinguishable
/// from other client errors by status alone.
///
/// When the plan tier is known (decoded from the OAuth bearer), the message also
/// lists what that plan CAN run, reusing the same conservative catalog +
/// [`is_model_available_for_plan`](wcore_config::chatgpt_catalog::is_model_available_for_plan)
/// gate that drives the predictive hide — so the two never disagree.
fn plan_gate_rejection_message(
    status: u16,
    body: &str,
    model: &str,
    plan_tier: Option<&str>,
) -> Option<String> {
    // Only 4xx client rejections are candidates — a 5xx/transient error is not a
    // plan-gate refusal and must retain its retry semantics.
    if !(400..500).contains(&status) {
        return None;
    }
    let b = body.to_ascii_lowercase();
    // Unambiguous model/plan signals only. OpenAI returns these for an unknown
    // or not-entitled model; the message variants cover the human-readable
    // "model ... does not exist or you do not have access" / plan phrasings.
    let model_plan_signal = b.contains("model_not_found")
        || b.contains("model_not_supported")
        || b.contains("unsupported_model")
        || b.contains("does not exist or you do not have access")
        || b.contains("do not have access to")
        || (b.contains(&model.to_ascii_lowercase())
            && (b.contains("plan") || b.contains("not available") || b.contains("access")));
    if !model_plan_signal {
        return None;
    }

    // Compose the "you CAN run" list from the conservative catalog, filtered by
    // the same plan-gate that drives the predictive hide. With no/unknown plan
    // tier we can't claim a runnable set, so we keep the message model-scoped.
    let catalog = crate::alias_models("openai-chatgpt");
    match plan_tier {
        Some(plan) => {
            let runnable: Vec<&str> = catalog
                .iter()
                .filter(|m| {
                    m.id != model
                        && wcore_config::chatgpt_catalog::is_model_available_for_plan(
                            Some(plan),
                            &m.id,
                        )
                })
                .map(|m| m.id.as_str())
                .collect();
            Some(format!(
                "{model} isn't available on your ChatGPT plan ({plan}). \
                 Models your plan can run: {}.",
                runnable.join(", ")
            ))
        }
        None => Some(format!(
            "{model} isn't available on your current ChatGPT plan. \
             Pick a different model with /model, or upgrade your ChatGPT subscription."
        )),
    }
}

#[async_trait]
impl LlmProvider for OpenAIChatGptProvider {
    fn alias_key(&self) -> &str {
        "openai-chatgpt"
    }

    /// #158 — filter the Codex catalog to what the ChatGPT subscription's plan
    /// tier can actually run, so the `/model` picker (via `engine_bridge`) does
    /// not offer models that 4xx on use.
    ///
    /// Conservative by construction (never over-filters):
    /// - We resolve the plan tier by decoding the live OAuth access token's
    ///   `chatgpt_plan_type` claim. If the bearer fetch fails (offline / not
    ///   signed in) or the claim is absent/undecodable, the tier is `None` and
    ///   NOTHING is filtered — the full alias catalog is returned, matching the
    ///   trait contract that `list_models` must floor to the alias catalog
    ///   rather than error.
    /// - The actual subtraction is driven by the conservative, currently-empty
    ///   `wcore_config::chatgpt_catalog` gating table; an unrecognised tier or
    ///   model is always shown.
    ///
    /// Only this OAuth-subscription provider filters. The API-key OpenAI path
    /// (`OpenAIProvider`) and every other provider are untouched.
    async fn list_models(&self) -> anyhow::Result<Vec<crate::ModelInfo>> {
        let full = crate::alias_models(self.alias_key());
        // Best-effort plan-tier resolution: any failure → None → show everything.
        let plan_tier = (self.bearer)()
            .await
            .ok()
            .and_then(|creds| wcore_config::chatgpt_catalog::decode_plan_type(&creds.access_token));
        Ok(full
            .into_iter()
            .filter(|m| {
                wcore_config::chatgpt_catalog::is_model_available_for_plan(
                    plan_tier.as_deref(),
                    &m.id,
                )
            })
            .collect())
    }

    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        // Acquire (and, if near expiry, refresh) the OAuth bearer first — this
        // is the only async point where the token manager can do its network
        // round-trip before the sync header build.
        let creds = (self.bearer)().await?;

        let url = self.responses_url();
        let body = self.build_codex_body(request);
        let headers = self.build_headers(&creds)?;

        let resp =
            builder_send_with_retry(self.client.post(&url).headers(headers).json(&body)).await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            // 401 → the OAuth token is bad/expired beyond refresh; map to
            // MissingApiKey so the CLI nudges `wayland auth login chatgpt`.
            if status.as_u16() == 401 {
                return Err(ProviderError::MissingApiKey);
            }
            // #158 (reactive fallback) — if the backend clearly rejected the
            // model BECAUSE the plan can't run it, replace the raw body with a
            // message that names the model, the plan, and what the plan CAN run.
            // Only fires when the body unambiguously signals a model/plan issue
            // (see `plan_gate_rejection_message`); otherwise the body is passed
            // through unchanged. The plan tier comes from the bearer we already
            // resolved above.
            let plan_tier = wcore_config::chatgpt_catalog::decode_plan_type(&creds.access_token);
            let message = plan_gate_rejection_message(
                status.as_u16(),
                &text,
                &request.model,
                plan_tier.as_deref(),
            )
            .unwrap_or(text);
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message,
            });
        }

        let (tx, rx) = mpsc::channel(64);
        let debug = self.debug.clone();
        tokio::spawn(async move {
            if let Err(e) = process_responses_sse_stream(resp, &tx, &debug).await {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_types::message::{ContentBlock, Message, Role};
    use wcore_types::tool::ToolDef;

    /// A bearer source that must never be invoked (header/url tests don't stream).
    fn unreachable_bearer() -> AsyncBearerSource {
        Arc::new(|| {
            Box::pin(async {
                unreachable!("bearer source must not be called in this test");
            })
        })
    }

    fn provider() -> OpenAIChatGptProvider {
        OpenAIChatGptProvider::new(
            unreachable_bearer(),
            ProviderCompat::default(),
            DebugConfig::default(),
        )
    }

    /// A bearer whose access token is a JWT carrying the given plan claim. Used
    /// by the #158 `list_models` tier-filter tests.
    fn bearer_with_plan(plan: &'static str) -> AsyncBearerSource {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct_test",
                "chatgpt_plan_type": plan
            }
        });
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let jwt = format!("h.{body}.s");
        Arc::new(move || {
            let jwt = jwt.clone();
            Box::pin(async move {
                Ok(BearerCreds {
                    access_token: jwt,
                    account_id: "acct_test".into(),
                })
            })
        })
    }

    /// A bearer that always fails (offline / not signed in). `list_models` must
    /// still floor to the full alias catalog rather than error.
    fn failing_bearer() -> AsyncBearerSource {
        Arc::new(|| Box::pin(async { Err(ProviderError::Connection("offline".into())) }))
    }

    fn request_with_tools(tools: Vec<ToolDef>) -> LlmRequest {
        LlmRequest {
            model: "gpt-5.5".into(),
            system: String::new(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "hi".into() }],
            )],
            max_tokens: 4096,
            tools,
            ..Default::default()
        }
    }

    // --- Task 3.2: headers + url ----------------------------------------

    #[test]
    fn headers_carry_account_id_beta_and_originator() {
        let creds = BearerCreds {
            access_token: "at".into(),
            account_id: "acct_9".into(),
        };
        let h = provider().build_headers(&creds).unwrap();
        assert_eq!(h["authorization"], "Bearer at");
        assert_eq!(h["chatgpt-account-id"], "acct_9");
        assert_eq!(h["openai-beta"], "responses=experimental");
        assert_eq!(h["originator"], "wayland");
        // D3: the SSE Accept header is present.
        assert_eq!(h["accept"], "text/event-stream");
        assert_eq!(h["content-type"], "application/json");
    }

    #[test]
    fn responses_url_targets_codex_backend() {
        assert_eq!(
            provider().responses_url(),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn responses_url_does_not_double_slash_on_trailing_base() {
        let p = provider().with_base_url("http://localhost:1234/");
        assert_eq!(p.responses_url(), "http://localhost:1234/responses");
    }

    // --- Task 3.4 / A2 / D2 / D4 / D5: body adjustments -----------------

    #[test]
    fn body_strips_max_output_tokens() {
        let body = provider().build_codex_body(&request_with_tools(vec![]));
        assert!(
            body.get("max_output_tokens").is_none(),
            "Codex rejects max_output_tokens; it must be stripped: {body}"
        );
    }

    #[test]
    fn body_does_not_request_encrypted_reasoning_until_round_trip_exists() {
        // We must NOT request `include: ["reasoning.encrypted_content"]` while
        // we cannot replay the sealed reasoning item — doing so 400s turn 2 of
        // a tool loop (`missing_following_item`). Lock that decision in.
        let body = provider().build_codex_body(&request_with_tools(vec![]));
        assert!(
            body.get("include").is_none(),
            "include must be absent: {body}"
        );
    }

    #[test]
    fn body_injects_default_instructions_when_system_empty() {
        let body = provider().build_codex_body(&request_with_tools(vec![]));
        assert_eq!(body["instructions"], json!(DEFAULT_INSTRUCTIONS));
    }

    #[test]
    fn body_preserves_caller_instructions_when_system_present() {
        let mut req = request_with_tools(vec![]);
        req.system = "You are Codex.".into();
        let body = provider().build_codex_body(&req);
        assert_eq!(body["instructions"], json!("You are Codex."));
    }

    #[test]
    fn body_adds_tool_routing_flags_only_when_tools_present() {
        // No tools → no tool_choice / parallel_tool_calls (and no tools field).
        let no_tools = provider().build_codex_body(&request_with_tools(vec![]));
        assert!(no_tools.get("tool_choice").is_none());
        assert!(no_tools.get("parallel_tool_calls").is_none());

        // With tools → both flags present alongside the tools array.
        let with_tools = provider().build_codex_body(&request_with_tools(vec![ToolDef {
            name: "read".into(),
            description: "Read a file".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
            deferred: false,
        }]));
        assert_eq!(with_tools["tool_choice"], json!("auto"));
        assert_eq!(with_tools["parallel_tool_calls"], json!(true));
        assert!(with_tools["tools"].is_array());
    }

    // --- #158: plan-tier model-catalog filtering ------------------------

    /// The full Codex alias catalog ids, most-capable first.
    fn full_catalog_ids() -> Vec<String> {
        crate::alias_models("openai-chatgpt")
            .into_iter()
            .map(|m| m.id)
            .collect()
    }

    async fn listed_ids(p: &OpenAIChatGptProvider) -> Vec<String> {
        p.list_models()
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect()
    }

    #[tokio::test]
    async fn list_models_with_pro_plan_keeps_full_catalog() {
        // A signed-in "pro" plan CAN run gpt-5.5-pro, so the full Codex catalog
        // is returned (the gate only subtracts gpt-5.5-pro for `plus`).
        let p = OpenAIChatGptProvider::new(
            bearer_with_plan("pro"),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert_eq!(listed_ids(&p).await, full_catalog_ids());
    }

    #[tokio::test]
    async fn list_models_with_plus_plan_hides_gpt_5_5_pro() {
        // #158 grounded gate: `plus` cannot run gpt-5.5-pro, so the `/model`
        // picker (which calls list_models) drops it — but keeps every other
        // Codex model, in order.
        let p = OpenAIChatGptProvider::new(
            bearer_with_plan("plus"),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        let listed = listed_ids(&p).await;
        assert!(
            !listed.contains(&"gpt-5.5-pro".to_string()),
            "plus must not be offered gpt-5.5-pro: {listed:?}"
        );
        let expected: Vec<String> = full_catalog_ids()
            .into_iter()
            .filter(|id| id != "gpt-5.5-pro")
            .collect();
        assert_eq!(listed, expected, "plus keeps all non-pro models in order");
    }

    #[tokio::test]
    async fn list_models_with_unrecognised_plan_keeps_full_catalog() {
        let p = OpenAIChatGptProvider::new(
            bearer_with_plan("enterprise"),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert_eq!(listed_ids(&p).await, full_catalog_ids());
    }

    // --- #158 Task C: reactive plan-gate rejection message --------------

    #[test]
    fn rejection_with_model_code_and_known_plan_names_runnable_set() {
        // OpenAI's "model not found / no access" envelope + a known plus plan →
        // a clear message that names the model, the plan, and what plus CAN run.
        let body = r#"{"error":{"message":"The model `gpt-5.5-pro` does not exist or you do not have access to it.","type":"invalid_request_error","code":"model_not_found"}}"#;
        let msg =
            plan_gate_rejection_message(404, body, "gpt-5.5-pro", Some("plus")).expect("plan-gate");
        assert!(msg.contains("gpt-5.5-pro"));
        assert!(msg.contains("plus"));
        // plus can run gpt-5.5 but NOT gpt-5.5-pro — the runnable list reflects
        // the same gate as the predictive hide.
        assert!(msg.contains("gpt-5.5"));
        assert!(
            !msg.contains("gpt-5.5-pro,") && !msg.ends_with("gpt-5.5-pro."),
            "the rejected model must not appear in the runnable set: {msg}"
        );
    }

    #[test]
    fn rejection_with_model_code_and_unknown_plan_is_model_scoped() {
        // No plan tier → we can't claim a runnable set, but still improve the
        // message (model-scoped + actionable) since the body clearly signals a
        // model/plan issue.
        let body = r#"{"error":{"code":"model_not_supported","message":"model not supported"}}"#;
        let msg = plan_gate_rejection_message(403, body, "gpt-5.5-pro", None).expect("plan-gate");
        assert!(msg.contains("gpt-5.5-pro"));
        assert!(msg.contains("/model") || msg.contains("upgrade"));
    }

    #[test]
    fn unrelated_4xx_is_left_unchanged() {
        // A generic bad-request body that does NOT clearly concern the model/plan
        // must NOT be rewritten — we don't fabricate plan-gate detection.
        let body = r#"{"error":{"message":"Invalid value for 'temperature'.","type":"invalid_request_error","code":"invalid_value"}}"#;
        assert!(plan_gate_rejection_message(400, body, "gpt-5.5", Some("plus")).is_none());
    }

    #[test]
    fn server_error_is_never_treated_as_plan_gate() {
        // A 5xx (even one that happens to mention the model) is transient and
        // must retain retry semantics — never rewritten as a plan refusal.
        let body = r#"{"error":{"message":"gpt-5.5-pro upstream had a plan glitch"}}"#;
        assert!(plan_gate_rejection_message(503, body, "gpt-5.5-pro", Some("plus")).is_none());
    }

    #[tokio::test]
    async fn list_models_floors_to_full_catalog_when_bearer_fails() {
        // Offline / not signed in: the bearer errors, the tier is unknown, and
        // `list_models` must NOT error — it floors to the full alias catalog.
        let p = OpenAIChatGptProvider::new(
            failing_bearer(),
            ProviderCompat::default(),
            DebugConfig::default(),
        );
        assert_eq!(listed_ids(&p).await, full_catalog_ids());
        assert!(
            !full_catalog_ids().is_empty(),
            "catalog must be non-empty so the picker never renders blank"
        );
    }
}
