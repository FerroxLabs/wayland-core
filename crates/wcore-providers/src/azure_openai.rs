//! Azure OpenAI provider — OpenAI chat-completions wire format, Azure routing.
//!
//! Azure OpenAI is a corporate-gated variant of OpenAI. It speaks the exact
//! same `/chat/completions` request/response wire format, so the request body
//! builder and the SSE decoder are reused verbatim from [`OpenAIProvider`].
//! Only two things differ from vanilla OpenAI:
//!
//! 1. **Endpoint shape.** Azure routes by *deployment name*, not by the model
//!    name carried in the request body:
//!    `https://{resource}.openai.azure.com/openai/deployments/{deployment}/chat/completions?api-version={version}`.
//! 2. **Auth header.** Azure uses `api-key: {key}` (API-key mode) rather than
//!    `Authorization: Bearer {key}`. AAD/OAuth bearer tokens are an advanced
//!    mode — see the TODO below.
//!
//! Because the auth header and the `?api-version=` query param cannot be
//! expressed through [`ProviderCompat`] (which only customizes the path
//! suffix), this provider owns a small self-contained `stream()`. It does NOT
//! fork the request pipeline: the request body comes from
//! [`OpenAIProvider::build_request_body`] and the response is decoded by the
//! shared [`process_sse_stream`] used by every OpenAI-compatible provider.
//!
//! Register via [`register_azure_openai_in`] against a [`ProviderRegistry`].
//! The id is lowercased to `"azure-openai"`.
//
// v0.6.4 Task 3.1: AAD bearer mode lands as `AzureAuth::AadBearer`. The
// token-acquisition + refresh path is intentionally kept OUT of this crate —
// callers supply a `TokenSource` closure that returns the current bearer
// token. This keeps `azure-identity` (or any heavy AAD SDK) out of the wcore
// dep tree, and lets tests inject a deterministic mock token.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use tokio::sync::mpsc;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_types::llm::{LlmEvent, LlmRequest};

use crate::key_rotation::{KeyPool, split_keys};
use crate::openai::{OpenAIProvider, process_sse_stream};
use crate::registry::{ProviderFactory, ProviderRegistry, RegistryError};
use crate::retry::builder_send_with_retry;
use crate::{LlmProvider, ProviderError, dump_request_body, reset_response_dump};

/// Default Azure OpenAI API version. Azure pins the wire contract to a dated
/// `api-version`; this is a recent stable value and can be overridden per call.
pub const AZURE_OPENAI_DEFAULT_API_VERSION: &str = "2024-10-21";

/// Pluggable bearer-token source for [`AzureAuth::AadBearer`].
///
/// Each call returns the *current* bearer token (no `Bearer ` prefix). The
/// implementation owns its own refresh policy: a real Entra ID integration
/// caches a near-expiry token and refreshes ahead of expiry; tests return a
/// fixed string. Returning a `Result<String, ProviderError>` lets a failed
/// token acquisition surface as a normal provider error instead of a panic.
pub type AzureTokenSource = Arc<dyn Fn() -> Result<String, ProviderError> + Send + Sync + 'static>;

/// Azure OpenAI authentication mode.
///
/// v0.6.4 Task 3.1. Mirrors [`wcore_config::config::AzureAuthMode`] but
/// carries the runtime token source for `AadBearer` (which a `Deserialize`
/// enum cannot). The config layer selects the variant; this crate carries
/// the closure.
#[derive(Clone)]
pub enum AzureAuth {
    /// Static `api-key: {key}` header (the v0.6.3 default). The string may carry
    /// multiple comma/whitespace-separated keys; they are split into a rotation
    /// [`KeyPool`] at construction. A single key yields a one-element pool —
    /// behavior identical to the pre-rotation path.
    ApiKey(Arc<Mutex<KeyPool>>),
    /// `Authorization: Bearer {token}` header. The token is acquired on each
    /// request via the supplied [`AzureTokenSource`] closure.
    AadBearer(AzureTokenSource),
}

impl std::fmt::Debug for AzureAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AzureAuth::ApiKey(_) => f.debug_tuple("ApiKey").field(&"<redacted>").finish(),
            AzureAuth::AadBearer(_) => f.debug_tuple("AadBearer").field(&"<token-source>").finish(),
        }
    }
}

/// Azure OpenAI provider — OpenAI chat-completions wire format over Azure's
/// deployment-routed endpoint. Auth is selectable: static `api-key` (default)
/// or AAD `Authorization: Bearer` via a caller-supplied token source.
pub struct AzureOpenAIProvider {
    client: wcore_egress::EgressClient,
    /// Authentication mode (api-key vs AAD bearer). v0.6.4 Task 3.1.
    auth: AzureAuth,
    /// Azure resource name — the `{resource}` in `{resource}.openai.azure.com`.
    resource: String,
    /// Deployment name — Azure routes by this, not by the body `model` field.
    deployment: String,
    /// Azure `api-version` query parameter.
    api_version: String,
    /// Body builder + SSE decode are reused from the OpenAI provider.
    inner: OpenAIProvider,
    debug: DebugConfig,
}

impl AzureOpenAIProvider {
    /// Construct an Azure OpenAI provider.
    ///
    /// `resource` is the Azure resource name (the `{resource}` subdomain),
    /// `deployment` is the model deployment name, and `api_version` is the
    /// Azure `api-version` query parameter (use
    /// [`AZURE_OPENAI_DEFAULT_API_VERSION`] for a sane default).
    pub fn new(
        api_key: &str,
        resource: &str,
        deployment: &str,
        api_version: &str,
        compat: ProviderCompat,
        debug: DebugConfig,
    ) -> Self {
        Self::with_auth(
            AzureAuth::ApiKey(Arc::new(Mutex::new(KeyPool::new(split_keys(api_key))))),
            resource,
            deployment,
            api_version,
            compat,
            debug,
        )
    }

    /// Construct with the default Azure API version (API-key auth).
    pub fn with_defaults(
        api_key: &str,
        resource: &str,
        deployment: &str,
        compat: ProviderCompat,
        debug: DebugConfig,
    ) -> Self {
        Self::new(
            api_key,
            resource,
            deployment,
            AZURE_OPENAI_DEFAULT_API_VERSION,
            compat,
            debug,
        )
    }

    /// v0.6.4 Task 3.1: construct with an explicit [`AzureAuth`] mode.
    ///
    /// Use [`AzureAuth::AadBearer`] with a closure that returns a current
    /// AAD bearer token to enable OAuth / Entra-ID auth. The closure is
    /// invoked on every request, so it owns its own caching policy.
    pub fn with_auth(
        auth: AzureAuth,
        resource: &str,
        deployment: &str,
        api_version: &str,
        compat: ProviderCompat,
        debug: DebugConfig,
    ) -> Self {
        // The inner OpenAIProvider is used only for `build_request_body`; its
        // own base_url is never hit (Azure's `stream()` builds the real URL)
        // and its credential is never sent on the wire (Azure builds the
        // `api-key` / AAD bearer header itself in `build_headers`). An empty
        // placeholder key is therefore safe for BOTH auth modes — the real
        // Azure ApiKey rotation lives in the `AzureAuth::ApiKey` pool below.
        let inner = OpenAIProvider::new("", "", compat, debug.clone());
        Self {
            client: crate::http_client::build(),
            auth,
            resource: resource.to_string(),
            deployment: deployment.to_string(),
            api_version: api_version.to_string(),
            inner,
            debug,
        }
    }

    /// Build the deployment-routed chat-completions URL with the
    /// `api-version` query parameter.
    fn build_url(&self) -> String {
        format!(
            "https://{}.openai.azure.com/openai/deployments/{}/chat/completions?api-version={}",
            self.resource, self.deployment, self.api_version
        )
    }

    /// Select the API key to authenticate the next request, when in API-key
    /// mode. Delegates to [`KeyPool::next_key`] (prefers the last-good key,
    /// rotates round-robin on failure, skips keys in cooldown). Returns
    /// `Ok(None)` for AAD bearer mode (no static key to rotate), and
    /// [`ProviderError::MissingApiKey`] when API-key mode is configured but the
    /// pool is empty or every key is cooling.
    fn select_key(&self) -> Result<Option<String>, ProviderError> {
        match &self.auth {
            AzureAuth::ApiKey(pool) => {
                let mut pool = pool.lock().expect("key pool mutex poisoned");
                pool.next_key()
                    .map(|k| Some(k.to_string()))
                    .ok_or(ProviderError::MissingApiKey)
            }
            AzureAuth::AadBearer(_) => Ok(None),
        }
    }

    /// Promote `key` to last-good after a successful (2xx) response. No-op for
    /// AAD bearer mode.
    fn mark_key_success(&self, key: &str) {
        if let AzureAuth::ApiKey(pool) = &self.auth {
            pool.lock()
                .expect("key pool mutex poisoned")
                .mark_success(key);
        }
    }

    /// Demote `key` for the cooldown window after an auth/rate-limit failure
    /// (401/403/429). No-op for AAD bearer mode.
    fn mark_key_failure(&self, key: &str) {
        if let AzureAuth::ApiKey(pool) = &self.auth {
            pool.lock()
                .expect("key pool mutex poisoned")
                .mark_failure(key);
        }
    }

    /// Build the Azure auth headers.
    ///
    /// v0.6.4 Task 3.1: dispatches on [`AzureAuth`]. `ApiKey` mode emits the
    /// legacy `api-key: {key}` header using the `selected_key` chosen by
    /// [`Self::select_key`]; `AadBearer` mode emits a standard
    /// `Authorization: Bearer {token}` header sourced from the configured
    /// token closure (and ignores `selected_key`).
    fn build_headers(&self, selected_key: Option<&str>) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();
        match &self.auth {
            AzureAuth::ApiKey(_) => {
                // The caller resolved the key via `select_key`; in API-key mode
                // it is always `Some`.
                let api_key = selected_key.ok_or(ProviderError::MissingApiKey)?;
                let key = HeaderValue::from_str(api_key).map_err(|e| {
                    ProviderError::Connection(format!("Invalid api-key header: {e}"))
                })?;
                headers.insert(HeaderName::from_static("api-key"), key);
            }
            AzureAuth::AadBearer(source) => {
                let token = source()?;
                let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| {
                    ProviderError::Connection(format!("Invalid AAD bearer token: {e}"))
                })?;
                headers.insert(reqwest::header::AUTHORIZATION, value);
            }
        }
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(headers)
    }
}

#[async_trait]
impl LlmProvider for AzureOpenAIProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = self.build_url();
        // Reuse the OpenAI body builder verbatim — Azure's wire body is
        // identical to OpenAI's; only routing and auth differ.
        let body = self.inner.build_request_body(request);

        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        let selected_key = self.select_key()?;
        let response = builder_send_with_retry(
            self.client
                .post(&url)
                .headers(self.build_headers(selected_key.as_deref())?)
                .json(&body),
        )
        .await?;

        // TODO(http-error-class): wiremock tests pending for azure_openai HTTP
        // error class (400/401/403/429/500). The status check is correct — tests
        // are missing. See fix/providers-http-error-class for the pattern used
        // on openai / anthropic / gemini / bedrock.
        let status = response.status();
        if !status.is_success() {
            // Demote this key on auth / rate-limit failures so the next request
            // rotates to another key in the pool (no-op for single key / AAD).
            if matches!(status.as_u16(), 401 | 403 | 429)
                && let Some(key) = selected_key.as_deref()
            {
                self.mark_key_failure(key);
            }
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

        // 2xx: this key works — make it sticky for subsequent requests.
        if let Some(key) = selected_key.as_deref() {
            self.mark_key_success(key);
        }

        let (tx, rx) = mpsc::channel(64);
        let debug = self.debug.clone();

        tokio::spawn(async move {
            if let Err(e) = process_sse_stream(response, &tx, &debug).await {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });

        Ok(rx)
    }
}

/// Register an Azure OpenAI factory in the given registry under the lowercased
/// id `"azure-openai"`. The factory captures the provided `api_key`,
/// `resource`, `deployment`, `api_version`, `compat`, and `debug` and
/// constructs a fresh provider per call.
///
/// Returns [`RegistryError::EmptyId`] if `resource` or `deployment` is empty
/// after trimming — Azure routes by deployment name, so a missing resource or
/// deployment can never produce a working endpoint. We surface that
/// misconfiguration loudly at registration time rather than letting it become
/// a silent connect-to-`https://.openai.azure.com/...` failure at request
/// time. (We reuse `EmptyId` rather than widening the error surface; the
/// message in context is unambiguous.)
pub fn register_azure_openai_in<R: ProviderRegistry>(
    registry: &mut R,
    api_key: String,
    resource: String,
    deployment: String,
    api_version: String,
    compat: ProviderCompat,
    debug: DebugConfig,
) -> Result<(), RegistryError> {
    if resource.trim().is_empty() || deployment.trim().is_empty() {
        return Err(RegistryError::EmptyId);
    }
    let factory: ProviderFactory = Arc::new(move || {
        Arc::new(AzureOpenAIProvider::new(
            &api_key,
            &resource,
            &deployment,
            &api_version,
            compat.clone(),
            debug.clone(),
        )) as Arc<dyn LlmProvider>
    });
    registry.register("azure-openai", factory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::WaylandProviderRegistry;
    use wcore_types::llm::LlmRequest;

    fn req(model: &str) -> LlmRequest {
        LlmRequest {
            model: model.into(),
            system: String::new(),
            messages: vec![],
            tools: vec![],
            max_tokens: 16,
            thinking: None,
            reasoning_effort: None,
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
        }
    }

    fn provider() -> AzureOpenAIProvider {
        AzureOpenAIProvider::with_defaults(
            "az-key",
            "my-resource",
            "gpt-4o-deploy",
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        )
    }

    #[test]
    fn register_uses_lowercase_id() {
        let mut r = WaylandProviderRegistry::new();
        register_azure_openai_in(
            &mut r,
            "az-key".into(),
            "my-resource".into(),
            "gpt-4o-deploy".into(),
            AZURE_OPENAI_DEFAULT_API_VERSION.into(),
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        )
        .unwrap();
        // Provider id is lowercase and correct.
        assert!(r.get("azure-openai").is_some());
        assert!(r.get("Azure-OpenAI").is_none());
        assert!(r.get("azure_openai").is_none());
    }

    #[test]
    fn build_url_uses_deployment_routing_and_api_version() {
        // Azure routes by deployment name in the path, not by the body model,
        // and pins the contract with an `api-version` query parameter.
        let p = AzureOpenAIProvider::new(
            "az-key",
            "my-resource",
            "gpt-4o-deploy",
            "2024-10-21",
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        );
        let url = p.build_url();
        assert_eq!(
            url,
            "https://my-resource.openai.azure.com/openai/deployments/\
             gpt-4o-deploy/chat/completions?api-version=2024-10-21"
        );
    }

    #[test]
    fn build_url_default_api_version() {
        // with_defaults wires AZURE_OPENAI_DEFAULT_API_VERSION into the query.
        let p = provider();
        let url = p.build_url();
        assert!(url.contains("/openai/deployments/gpt-4o-deploy/chat/completions"));
        assert!(url.ends_with(&format!("api-version={AZURE_OPENAI_DEFAULT_API_VERSION}")));
    }

    #[test]
    fn aad_bearer_auth_emits_authorization_header_from_mock_token_source() {
        // v0.6.4 Task 3.1: AadBearer mode must produce a standard
        // `Authorization: Bearer {token}` header sourced from the supplied
        // closure, and must NOT emit the legacy `api-key` header.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_closure = calls.clone();
        let token_source: AzureTokenSource = Arc::new(move || {
            calls_for_closure.fetch_add(1, Ordering::SeqCst);
            Ok("mock-aad-token-xyz".to_string())
        });

        let p = AzureOpenAIProvider::with_auth(
            AzureAuth::AadBearer(token_source),
            "my-resource",
            "gpt-4o-deploy",
            AZURE_OPENAI_DEFAULT_API_VERSION,
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        );

        // AAD bearer mode has no static key to rotate — select_key returns None.
        assert!(p.select_key().unwrap().is_none());
        let headers = p.build_headers(None).unwrap();
        assert_eq!(
            headers.get(reqwest::header::AUTHORIZATION).unwrap(),
            "Bearer mock-aad-token-xyz"
        );
        assert!(headers.get("api-key").is_none());
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/json");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "token source should be invoked exactly once per build_headers call"
        );
    }

    #[test]
    fn aad_bearer_propagates_token_source_failure() {
        // A failing token acquisition must surface as a ProviderError, not a panic.
        let token_source: AzureTokenSource = Arc::new(|| {
            Err(ProviderError::Connection(
                "AAD token endpoint unreachable".into(),
            ))
        });
        let p = AzureOpenAIProvider::with_auth(
            AzureAuth::AadBearer(token_source),
            "my-resource",
            "gpt-4o-deploy",
            AZURE_OPENAI_DEFAULT_API_VERSION,
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        );
        let err = p
            .build_headers(None)
            .expect_err("token failure must propagate");
        assert!(matches!(err, ProviderError::Connection(_)));
    }

    #[test]
    fn auth_header_is_api_key_not_bearer() {
        // Azure authenticates via `api-key`, never `Authorization: Bearer`.
        let p = provider();
        let key = p.select_key().unwrap().expect("api-key mode yields a key");
        let headers = p.build_headers(Some(&key)).unwrap();
        assert_eq!(headers.get("api-key").unwrap(), "az-key");
        assert!(headers.get(reqwest::header::AUTHORIZATION).is_none());
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/json");
    }

    /// Multi-key rotation in Azure API-key mode: a demoted key rotates to the
    /// other key; a succeeded key sticks. AAD mode and single key are unchanged.
    #[test]
    fn multi_key_rotation_demotes_failing_key_then_succeeds() {
        let p = AzureOpenAIProvider::with_defaults(
            "key-a, key-b",
            "my-resource",
            "gpt-4o-deploy",
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        );
        let first = p.select_key().unwrap().expect("a key is available");
        assert!(first == "key-a" || first == "key-b");

        p.mark_key_failure(&first);
        let second = p
            .select_key()
            .unwrap()
            .expect("rotation finds the other key");
        assert_ne!(second, first);

        p.mark_key_success(&second);
        assert_eq!(p.select_key().unwrap().unwrap(), second);

        // Single-key + empty (MissingApiKey) cases.
        let solo = AzureOpenAIProvider::with_defaults(
            "solo",
            "my-resource",
            "gpt-4o-deploy",
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        );
        assert_eq!(solo.select_key().unwrap().unwrap(), "solo");
        let empty = AzureOpenAIProvider::with_defaults(
            "",
            "my-resource",
            "gpt-4o-deploy",
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        );
        assert!(matches!(
            empty.select_key(),
            Err(ProviderError::MissingApiKey)
        ));
    }

    #[test]
    fn request_body_matches_openai_shape() {
        // The wire body is identical to OpenAI's: model + messages + stream.
        let p = provider();
        let body = p.inner.build_request_body(&req("gpt-4o"));
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["stream"], true);
        assert!(body["messages"].is_array());
        assert_eq!(body["max_tokens"], 16);
    }

    #[test]
    fn register_rejects_missing_resource_or_deployment() {
        // Azure routes by deployment name — a missing resource or deployment
        // can never produce a working endpoint, so registration must fail
        // loudly rather than yielding a connect-to-nothing provider.
        let mut r = WaylandProviderRegistry::new();

        let no_resource = register_azure_openai_in(
            &mut r,
            "az-key".into(),
            "   ".into(),
            "gpt-4o-deploy".into(),
            AZURE_OPENAI_DEFAULT_API_VERSION.into(),
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        );
        assert!(matches!(no_resource, Err(RegistryError::EmptyId)));

        let no_deployment = register_azure_openai_in(
            &mut r,
            "az-key".into(),
            "my-resource".into(),
            String::new(),
            AZURE_OPENAI_DEFAULT_API_VERSION.into(),
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        );
        assert!(matches!(no_deployment, Err(RegistryError::EmptyId)));

        // The registry stays clean — no factory registered.
        assert!(r.get("azure-openai").is_none());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut r = WaylandProviderRegistry::new();
        register_azure_openai_in(
            &mut r,
            "az-key".into(),
            "my-resource".into(),
            "gpt-4o-deploy".into(),
            AZURE_OPENAI_DEFAULT_API_VERSION.into(),
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        )
        .unwrap();
        let second = register_azure_openai_in(
            &mut r,
            "az-other".into(),
            "other-resource".into(),
            "gpt-4o-deploy".into(),
            AZURE_OPENAI_DEFAULT_API_VERSION.into(),
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        );
        assert!(matches!(second, Err(RegistryError::DuplicateId(_))));
    }

    #[tokio::test]
    async fn stream_errors_on_unreachable_host() {
        // Unresolvable Azure resource — stream() must surface a ProviderError,
        // not panic and not hang.
        let p = AzureOpenAIProvider::with_defaults(
            "az-key",
            "nonexistent-resource-wcore-test",
            "gpt-4o-deploy",
            ProviderCompat::openai_defaults(),
            DebugConfig::default(),
        );
        let result = p.stream(&req("gpt-4o")).await;
        assert!(result.is_err(), "expected error from unreachable host");
    }
}
