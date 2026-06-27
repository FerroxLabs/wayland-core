// Google Vertex AI provider — native multi-publisher path.
//
// v0.8.1 U8b: extended to support both publishers Vertex actually fronts:
//   - `publishers/anthropic` — Claude on Vertex (existing path,
//      `:streamRawPredict` with Anthropic-shape body & SSE).
//   - `publishers/google` — Gemini on Vertex (new, `:streamGenerateContent`
//      with Gemini-shape body & SSE). Body builder + SSE parser are reused
//      verbatim from `gemini.rs` — Vertex Gemini speaks the SAME wire
//      protocol as the public Generative Language API; only the URL and
//      auth header differ.
//
// Auth is the same RS256 + OAuth2 chain for both publishers:
//   1. `GOOGLE_APPLICATION_CREDENTIALS` → service-account JSON (RS256 JWT
//      assertion → access token).
//   2. `~/.config/gcloud/application_default_credentials.json` (ADC refresh
//      token flow).
//   3. GCE / Cloud Run metadata server (`metadata.google.internal`).
// Tokens are cached in-process until 60s before their reported expiry.

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use wcore_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};

use super::anthropic_shared;
use super::gemini::{self, SafetySetting};
use crate::retry::builder_send_with_retry;
use crate::{LlmProvider, ProviderError, dump_request_body, reset_response_dump};
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;

pub struct VertexProvider {
    client: wcore_egress::EgressClient,
    project_id: String,
    region: String,
    auth: GcpAuth,
    cache_enabled: bool,
    compat: ProviderCompat,
    debug: DebugConfig,
    /// Optional Gemini safety overrides. Empty ⇒ Vertex defaults apply.
    /// Only consulted on the Gemini publisher path.
    safety_settings: Vec<SafetySetting>,
    /// Cached access token
    cached_token: Mutex<Option<CachedToken>>,
}

#[derive(Debug, Clone)]
pub enum GcpAuth {
    ServiceAccount { key_file: String },
    ApplicationDefault,
    MetadataServer,
}

/// Which Vertex publisher hosts the requested model. Auto-detected from
/// the model identifier in `LlmRequest::model` (case-insensitive prefix
/// match). Vertex's URL path and request body differ per publisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexPublisher {
    /// Claude on Vertex — `publishers/anthropic/.../:streamRawPredict`.
    /// Body shape = native Anthropic Messages API.
    Anthropic,
    /// Gemini on Vertex — `publishers/google/.../:streamGenerateContent?alt=sse`.
    /// Body shape = native Generative Language API.
    Google,
}

impl VertexPublisher {
    /// Pick a publisher from a model identifier. Defaults to `Anthropic`
    /// (preserves the v0.7.0 behavior for unrecognized names) so existing
    /// `[vertex]` configs keep working without explicit migration.
    pub fn for_model(model: &str) -> Self {
        let m = model.to_ascii_lowercase();
        if m.starts_with("gemini") || m.contains("/gemini") || m.contains("models/gemini") {
            VertexPublisher::Google
        } else {
            VertexPublisher::Anthropic
        }
    }

    /// URL path segment that follows `/publishers/`.
    fn url_segment(self) -> &'static str {
        match self {
            VertexPublisher::Anthropic => "anthropic",
            VertexPublisher::Google => "google",
        }
    }

    /// Method name appended after the model identifier in the Vertex URL.
    fn method(self) -> &'static str {
        match self {
            VertexPublisher::Anthropic => ":streamRawPredict",
            VertexPublisher::Google => ":streamGenerateContent?alt=sse",
        }
    }
}

struct CachedToken {
    token: String,
    expires_at: u64,
}

impl VertexProvider {
    pub fn new(
        project_id: &str,
        region: &str,
        auth: GcpAuth,
        cache_enabled: bool,
        compat: ProviderCompat,
        debug: DebugConfig,
    ) -> Self {
        Self {
            client: crate::http_client::build(),
            project_id: project_id.to_string(),
            region: region.to_string(),
            auth,
            cache_enabled,
            compat,
            debug,
            safety_settings: Vec::new(),
            cached_token: Mutex::new(None),
        }
    }

    /// Apply Gemini-publisher safety overrides. No-op for Claude models.
    pub fn with_safety_settings(mut self, settings: Vec<SafetySetting>) -> Self {
        self.safety_settings = settings;
        self
    }

    /// Construct the full Vertex URL for `model`, picking the publisher path
    /// from the model name. Kept as a convenience for tests + future callers;
    /// the streaming path uses `build_url_for` directly to share the
    /// pre-resolved publisher with the SSE dispatch.
    #[cfg(test)]
    fn build_url(&self, model: &str) -> String {
        self.build_url_for(model, VertexPublisher::for_model(model))
    }

    fn build_url_for(&self, model: &str, publisher: VertexPublisher) -> String {
        format!(
            "https://{}-aiplatform.googleapis.com/v1/projects/{}/locations/{}/publishers/{}/models/{}{}",
            self.region,
            self.project_id,
            self.region,
            publisher.url_segment(),
            model,
            publisher.method(),
        )
    }

    fn build_request_body(&self, request: &LlmRequest) -> Value {
        match VertexPublisher::for_model(&request.model) {
            VertexPublisher::Anthropic => self.build_anthropic_body(request),
            VertexPublisher::Google => {
                // Vertex Gemini = same body shape as the public Generative
                // Language API; reuse the gemini.rs builder verbatim. Lift
                // `request.system` (Anthropic-on-Vertex convention) into
                // the Gemini `systemInstruction` so it isn't silently lost
                // for callers that don't use `Role::System` messages.
                gemini::build_gemini_body(
                    request,
                    &self.compat,
                    &self.safety_settings,
                    &request.system,
                )
            }
        }
    }

    fn build_anthropic_body(&self, request: &LlmRequest) -> Value {
        let system = if self.cache_enabled {
            json!([{
                "type": "text",
                "text": &request.system,
                "cache_control": { "type": "ephemeral" }
            }])
        } else {
            json!(&request.system)
        };

        let mut body = json!({
            "anthropic_version": "vertex-2023-10-16",
            "max_tokens": request.max_tokens,
            "system": system,
            "messages": anthropic_shared::build_messages(&request.messages, &self.compat),
            "stream": true
        });

        if !request.tools.is_empty() {
            let mut tools = anthropic_shared::build_tools(&request.tools);
            if let Some(last) = tools.last_mut().filter(|_| self.cache_enabled) {
                last["cache_control"] = json!({ "type": "ephemeral" });
            }
            body["tools"] = json!(tools);
        }

        if let Some(ThinkingConfig::Enabled { budget_tokens }) = &request.thinking {
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget_tokens
            });
        }

        // Crucible #3: emit an explicit `temperature` when set, gated by the
        // provider's `supports_temperature` flag + the per-model exclusion (see
        // `openai_compat::emit_temperature`). Anthropic-on-Vertex accepts it.
        crate::openai_compat::emit_temperature(&mut body, request, &self.compat);

        body
    }

    async fn get_access_token(&self) -> Result<String, ProviderError> {
        // Check cache first
        {
            let cached = self.cached_token.lock().map_err(|_| {
                ProviderError::Connection("Vertex token cache lock poisoned".to_string())
            })?;
            if let Some(token) = cached.as_ref() {
                // SAFETY: SystemTime::now() returns the current wall
                // clock; it can only be before UNIX_EPOCH if the
                // system clock is set to a pre-1970 value, which is
                // not a supported configuration on any platform we
                // build against.
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                if token.expires_at > now + 60 {
                    return Ok(token.token.clone());
                }
            }
        }

        let (token, expires_in) = match &self.auth {
            GcpAuth::ServiceAccount { key_file } => {
                self.get_service_account_token(key_file).await?
            }
            GcpAuth::ApplicationDefault => self.get_adc_token().await?,
            GcpAuth::MetadataServer => self.get_metadata_token().await?,
        };

        // Cache the token
        // SAFETY: same as the cache-check above — system clock cannot
        // be before UNIX_EPOCH on supported platforms.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut cached = self.cached_token.lock().map_err(|_| {
            ProviderError::Connection("Vertex token cache lock poisoned".to_string())
        })?;
        *cached = Some(CachedToken {
            token: token.clone(),
            expires_at: now + expires_in,
        });

        Ok(token)
    }

    async fn get_service_account_token(
        &self,
        key_file: &str,
    ) -> Result<(String, u64), ProviderError> {
        let key_json = std::fs::read_to_string(key_file)
            .map_err(|e| ProviderError::Connection(format!("Failed to read key file: {}", e)))?;

        let sa: ServiceAccountKey = serde_json::from_str(&key_json)
            .map_err(|e| ProviderError::Connection(format!("Failed to parse key file: {}", e)))?;

        // SAFETY: same as above — system clock can't be before
        // UNIX_EPOCH on supported platforms.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let claims = JwtClaims {
            iss: sa.client_email.clone(),
            scope: "https://www.googleapis.com/auth/cloud-platform".to_string(),
            aud: sa.token_uri.clone(),
            iat: now,
            exp: now + 3600,
        };

        let encoding_key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
            .map_err(|e| ProviderError::Connection(format!("Invalid RSA key: {}", e)))?;

        let header = Header::new(Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&header, &claims, &encoding_key)
            .map_err(|e| ProviderError::Connection(format!("JWT encode error: {}", e)))?;

        // Exchange JWT for access token
        let resp = self
            .client
            .post(&sa.token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Connection(format!("Token exchange error: {}", e)))?;

        let token_resp: GoogleTokenResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Connection(format!("Token parse error: {}", e)))?;

        Ok((token_resp.access_token, token_resp.expires_in))
    }

    async fn get_adc_token(&self) -> Result<(String, u64), ProviderError> {
        // Read Application Default Credentials
        let adc_path = dirs::home_dir()
            .ok_or_else(|| ProviderError::Connection("Cannot determine home dir".into()))?
            .join(".config/gcloud/application_default_credentials.json");

        let adc_json = std::fs::read_to_string(&adc_path).map_err(|e| {
            ProviderError::Connection(format!(
                "Failed to read ADC at {}: {}. Run 'gcloud auth application-default login'.",
                adc_path.display(),
                e
            ))
        })?;

        let adc: AdcCredentials = serde_json::from_str(&adc_json)
            .map_err(|e| ProviderError::Connection(format!("Failed to parse ADC: {}", e)))?;

        // Use refresh token to get access token
        let resp = self
            .client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", adc.client_id.as_str()),
                ("client_secret", adc.client_secret.as_str()),
                ("refresh_token", adc.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|e| ProviderError::Connection(format!("ADC token refresh error: {}", e)))?;

        let token_resp: GoogleTokenResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Connection(format!("Token parse error: {}", e)))?;

        Ok((token_resp.access_token, token_resp.expires_in))
    }

    async fn get_metadata_token(&self) -> Result<(String, u64), ProviderError> {
        let resp = self
            .client
            .get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token")
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| ProviderError::Connection(format!("Metadata server error: {}", e)))?;

        let token_resp: GoogleTokenResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Connection(format!("Token parse error: {}", e)))?;

        Ok((token_resp.access_token, token_resp.expires_in))
    }
}

#[async_trait]
impl LlmProvider for VertexProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let publisher = VertexPublisher::for_model(&request.model);
        let url = self.build_url_for(&request.model, publisher);
        let body = self.build_request_body(request);

        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        let access_token = self.get_access_token().await?;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", access_token))
                .map_err(|e| ProviderError::Connection(format!("Header error: {}", e)))?,
        );

        // TODO(http-error-class): wiremock tests pending for vertex HTTP error
        // class (400/401/403/429/500). The status check is correct — tests are
        // missing. See fix/providers-http-error-class for the pattern used on
        // openai / anthropic / gemini / bedrock.
        let response =
            builder_send_with_retry(self.client.post(&url).headers(headers).json(&body)).await?;

        let status = response.status();
        if !status.is_success() {
            // E-H1 / L3: capture headers before `.text()` consumes the body
            // so a 429 can honour `Retry-After` (header, then nested body).
            let headers = response.headers().clone();
            let body_text = response.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(ProviderError::RateLimited {
                    retry_after_ms: crate::retry::resolve_retry_after_ms(&headers, &body_text),
                });
            }
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        let (tx, rx) = mpsc::channel(64);
        let debug = self.debug.clone();

        // Pick the SSE parser by publisher: Anthropic emits Anthropic SSE,
        // Google emits Gemini SSE. Both stream over `data: {...}` frames but
        // the JSON payload shape differs.
        match publisher {
            VertexPublisher::Anthropic => {
                tokio::spawn(async move {
                    if let Err(e) =
                        anthropic_shared::process_sse_stream(response, &tx, &debug).await
                    {
                        let _ = tx.send(LlmEvent::Error(e.to_string())).await;
                    }
                });
            }
            VertexPublisher::Google => {
                tokio::spawn(async move {
                    if let Err(e) = gemini::process_sse_stream(response, &tx, &debug).await {
                        let _ = tx.send(LlmEvent::Error(e.to_string())).await;
                    }
                });
            }
        }

        Ok(rx)
    }

    fn alias_key(&self) -> &str {
        // Live discovery on Vertex (GCP-OAuth + pagination + dual-publisher
        // Anthropic/Google catalogs) is a heavier follow-up; for now the
        // `/model` picker floors to the static Vertex alias catalog.
        "vertex"
    }
}

// --- Internal types ---

#[derive(Debug, Deserialize)]
struct ServiceAccountKey {
    client_email: String,
    private_key: String,
    token_uri: String,
}

#[derive(Debug, Serialize)]
struct JwtClaims {
    iss: String,
    scope: String,
    aud: String,
    iat: u64,
    exp: u64,
}

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

fn default_expires_in() -> u64 {
    3600
}

#[derive(Debug, Deserialize)]
struct AdcCredentials {
    client_id: String,
    client_secret: String,
    refresh_token: String,
}

/// Build GcpAuth from wcore-config's VertexConfig
pub fn auth_from_config(vc: &wcore_config::config::VertexConfig) -> GcpAuth {
    if let Some(creds_file) = &vc.credentials_file {
        GcpAuth::ServiceAccount {
            key_file: creds_file.clone(),
        }
    } else {
        GcpAuth::ApplicationDefault
    }
}

// ---------------------------------------------------------------------------
// Unit tests — pure-function coverage. The auth + HTTP paths are exercised
// via integration tests under `tests/` when GCP creds are available; here we
// pin publisher selection, URL shape, body shape, and token cache semantics.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_types::llm::LlmRequest;

    fn provider() -> VertexProvider {
        VertexProvider::new(
            "my-proj",
            "us-central1",
            GcpAuth::ApplicationDefault,
            false,
            ProviderCompat::default(),
            DebugConfig::default(),
        )
    }

    fn req(model: &str) -> LlmRequest {
        LlmRequest {
            model: model.to_string(),
            system: "you are helpful".to_string(),
            max_tokens: 1024,
            ..Default::default()
        }
    }

    #[test]
    fn publisher_for_gemini_models_is_google() {
        for m in [
            "gemini-2.0-flash-001",
            "gemini-1.5-pro",
            "Gemini-2.5-Flash",
            "publishers/google/models/gemini-2.0-flash",
        ] {
            assert_eq!(
                VertexPublisher::for_model(m),
                VertexPublisher::Google,
                "expected Google publisher for {m}",
            );
        }
    }

    #[test]
    fn publisher_for_claude_models_defaults_to_anthropic() {
        for m in [
            "claude-3-5-sonnet-v2@20241022",
            "claude-opus-4@20250514",
            "claude-sonnet-4@20250514",
            // Unknown model identifiers must NOT silently flip publishers —
            // preserving the v0.7.0 behavior for unrecognized names.
            "unknown-model",
            "",
        ] {
            assert_eq!(
                VertexPublisher::for_model(m),
                VertexPublisher::Anthropic,
                "expected Anthropic publisher for {m:?}",
            );
        }
    }

    #[test]
    fn build_url_routes_claude_to_anthropic_publisher_streamrawpredict() {
        let p = provider();
        let url = p.build_url("claude-sonnet-4@20250514");
        assert!(
            url.contains("/publishers/anthropic/"),
            "claude URL should target anthropic publisher: {url}",
        );
        assert!(
            url.ends_with(":streamRawPredict"),
            "claude URL should use streamRawPredict: {url}",
        );
        assert!(url.contains("/projects/my-proj/locations/us-central1/"));
        assert!(url.starts_with("https://us-central1-aiplatform.googleapis.com/v1/"));
    }

    #[test]
    fn build_url_routes_gemini_to_google_publisher_streamgeneratecontent() {
        let p = provider();
        let url = p.build_url("gemini-2.0-flash-001");
        assert!(
            url.contains("/publishers/google/"),
            "gemini URL should target google publisher: {url}",
        );
        assert!(
            url.contains(":streamGenerateContent?alt=sse"),
            "gemini URL should use streamGenerateContent with SSE: {url}",
        );
        assert!(url.contains("/models/gemini-2.0-flash-001"));
    }

    #[test]
    fn build_body_for_claude_uses_anthropic_vertex_shape() {
        let p = provider();
        let body = p.build_request_body(&req("claude-sonnet-4@20250514"));
        assert_eq!(body["anthropic_version"], "vertex-2023-10-16");
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["stream"], true);
        // The Gemini-only top-level fields must NOT leak into the
        // Anthropic-publisher body.
        assert!(body.get("contents").is_none());
        assert!(body.get("generationConfig").is_none());
    }

    #[test]
    fn build_body_for_gemini_uses_generative_language_shape() {
        let p = provider();
        let body = p.build_request_body(&req("gemini-2.0-flash-001"));
        // Gemini-shape body has `contents` + `generationConfig`, no
        // `anthropic_version` or top-level `messages`.
        assert!(body.get("contents").is_some(), "expected contents: {body}");
        assert!(
            body.get("generationConfig").is_some(),
            "expected generationConfig: {body}",
        );
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 1024);
        assert!(body.get("anthropic_version").is_none());
        assert!(body.get("messages").is_none());
        // System message lifts to `systemInstruction`.
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "you are helpful",
        );
    }

    #[test]
    fn with_safety_settings_applies_to_gemini_body_only() {
        let p = provider().with_safety_settings(vec![SafetySetting {
            category: "HARM_CATEGORY_HARASSMENT".to_string(),
            threshold: "BLOCK_ONLY_HIGH".to_string(),
        }]);

        let gemini_body = p.build_request_body(&req("gemini-2.0-flash-001"));
        let arr = gemini_body["safetySettings"]
            .as_array()
            .expect("safetySettings present on gemini body");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["category"], "HARM_CATEGORY_HARASSMENT");

        let claude_body = p.build_request_body(&req("claude-sonnet-4@20250514"));
        assert!(
            claude_body.get("safetySettings").is_none(),
            "claude body must not carry safetySettings: {claude_body}",
        );
    }

    #[test]
    fn auth_from_config_with_credentials_file_picks_service_account() {
        let vc = wcore_config::config::VertexConfig {
            credentials_file: Some("/tmp/key.json".into()),
            ..Default::default()
        };
        match auth_from_config(&vc) {
            GcpAuth::ServiceAccount { key_file } => assert_eq!(key_file, "/tmp/key.json"),
            other => panic!("expected ServiceAccount, got {other:?}"),
        }
    }

    #[test]
    fn auth_from_config_without_credentials_file_picks_adc() {
        let vc = wcore_config::config::VertexConfig::default();
        assert!(matches!(auth_from_config(&vc), GcpAuth::ApplicationDefault,));
    }

    #[tokio::test]
    async fn cached_token_is_returned_within_expiry_window() {
        // Pre-seed the cache with a token expiring well in the future and
        // verify `get_access_token` returns it without invoking any auth
        // backend (which would fail in this test environment).
        let p = provider();
        let future = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        {
            let mut cached = p.cached_token.lock().unwrap();
            *cached = Some(CachedToken {
                token: "ya29.fake-bearer-for-test".to_string(),
                expires_at: future,
            });
        }
        let tok = p.get_access_token().await.expect("cached token returned");
        assert_eq!(tok, "ya29.fake-bearer-for-test");
    }
}
