// AWS Bedrock provider for Claude models.
// Uses AWS SigV4 authentication and AWS event stream binary framing.

use async_trait::async_trait;
use aws_credential_types::Credentials;
use aws_sigv4::http_request::{
    self as sigv4_http, PayloadChecksumKind, SignableBody, SignableRequest, SignatureLocation,
    SigningSettings,
};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::time::SystemTime;
use tokio::sync::mpsc;

use base64::Engine as _;

use wcore_types::llm::{LlmEvent, LlmRequest, ThinkingConfig};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use super::anthropic_shared;
use crate::retry::{DEFAULT_MAX_RETRIES, with_retry};
use crate::{
    LlmProvider, ProviderError, dump_request_body, dump_response_chunk, reset_response_dump,
};
use wcore_config::compat::{self, ProviderCompat};
use wcore_config::debug::DebugConfig;

pub struct BedrockProvider {
    client: wcore_egress::EgressClient,
    region: String,
    credentials: AwsCredentials,
    cache_enabled: bool,
    compat: ProviderCompat,
    debug: DebugConfig,
    /// Override the Bedrock endpoint base URL — used by integration tests to
    /// redirect requests to a local wiremock server without real AWS network
    /// access. `None` in all production code paths.
    endpoint_override: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AwsCredentials {
    Explicit {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
    Profile(String),
    Environment,
}

impl BedrockProvider {
    pub fn new(
        region: &str,
        credentials: AwsCredentials,
        cache_enabled: bool,
        compat: ProviderCompat,
        debug: DebugConfig,
    ) -> Self {
        Self {
            client: crate::http_client::build(),
            region: region.to_string(),
            credentials,
            cache_enabled,
            compat,
            debug,
            endpoint_override: None,
        }
    }

    /// Override the Bedrock endpoint base URL with an arbitrary URL. Used
    /// by integration tests to redirect requests to a local wiremock server
    /// without real AWS credentials or network access. Never call this in
    /// production code — pass `None` through `new()` instead.
    ///
    /// The provider still builds and sends SigV4-signed headers; wiremock
    /// ignores them, so `AwsCredentials::Explicit` with dummy values works.
    #[doc(hidden)]
    pub fn new_with_endpoint_override(
        region: &str,
        credentials: AwsCredentials,
        cache_enabled: bool,
        compat: ProviderCompat,
        debug: DebugConfig,
        endpoint_override: &str,
    ) -> Self {
        Self {
            client: crate::http_client::build(),
            region: region.to_string(),
            credentials,
            cache_enabled,
            compat,
            debug,
            endpoint_override: Some(endpoint_override.to_string()),
        }
    }

    /// Build the request body for `request`, dispatching on model family.
    ///
    /// AWS Bedrock fronts several model families under one runtime endpoint,
    /// but each family has a distinct request JSON schema. Mistral and Cohere
    /// ids route to their own builders; everything else uses the
    /// Anthropic-on-Bedrock schema.
    // NOTE(v0.6.4): native Bedrock event-stream parsing for Mistral/Cohere.
    fn build_request_body(&self, request: &LlmRequest) -> Value {
        if mistral::is_mistral_model(&request.model) {
            // Mistral on Bedrock has no `temperature`/`top_p` source on
            // `LlmRequest` today — pass `None`, letting Bedrock apply its
            // defaults. Wire those through if `LlmRequest` gains the fields.
            return mistral::build_mistral_request_body(
                &request.system,
                &request.messages,
                request.max_tokens,
                None,
                None,
            );
        }
        if cohere::is_cohere_model(&request.model) {
            return cohere::build_cohere_request_body(request);
        }
        self.build_anthropic_request_body(request)
    }

    /// Build the Anthropic-on-Bedrock request body (`anthropic_version` +
    /// Anthropic-shaped `system`/`messages`/`tools`).
    fn build_anthropic_request_body(&self, request: &LlmRequest) -> Value {
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
            "anthropic_version": "bedrock-2023-05-31",
            "max_tokens": request.max_tokens,
            "system": system,
            "messages": anthropic_shared::build_messages(&request.messages, &self.compat)
        });

        if !request.tools.is_empty() {
            let mut tools = anthropic_shared::build_tools(&request.tools);
            if self.compat.sanitize_schema() {
                for tool in &mut tools {
                    if let Some(schema) = tool.get("input_schema").cloned() {
                        tool["input_schema"] = compat::sanitize_json_schema(&schema);
                    }
                }
            }
            if self.cache_enabled
                && let Some(last) = tools.last_mut()
            {
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

        body
    }

    /// AWS event-stream endpoint — used by the Anthropic family.
    fn build_url(&self, model: &str) -> String {
        if let Some(base) = &self.endpoint_override {
            return format!("{}/model/{}/invoke-with-response-stream", base, model);
        }
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/invoke-with-response-stream",
            self.region, model
        )
    }

    /// Non-streaming `invoke` endpoint — used by the Mistral and Cohere
    /// families, whose responses are buffered then re-emitted as a terminal
    /// `LlmEvent` sequence on `stream()`'s channel.
    fn build_invoke_url(&self, model: &str) -> String {
        if let Some(base) = &self.endpoint_override {
            return format!("{}/model/{}/invoke", base, model);
        }
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/invoke",
            self.region, model
        )
    }

    fn resolve_credentials(&self) -> Result<Credentials, ProviderError> {
        match &self.credentials {
            AwsCredentials::Explicit {
                access_key_id,
                secret_access_key,
                session_token,
            } => Ok(Credentials::new(
                access_key_id,
                secret_access_key,
                session_token.clone(),
                None,
                "wayland-core",
            )),
            AwsCredentials::Profile(profile) => Self::credentials_from_sdk(Some(profile.clone())),
            AwsCredentials::Environment => Self::credentials_from_sdk(None),
        }
    }

    fn credentials_from_sdk(profile: Option<String>) -> Result<Credentials, ProviderError> {
        // Use a short-lived tokio runtime to resolve credentials synchronously.
        // This is called once per LLM request so the overhead is acceptable.
        let rt = tokio::runtime::Handle::try_current();

        let resolve = async move {
            let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
            if let Some(p) = profile {
                loader = loader.profile_name(p);
            }
            let config = loader.load().await;
            let provider = config.credentials_provider().ok_or_else(|| {
                ProviderError::Connection(
                    "No AWS credentials found. Set AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY, \
                     AWS_PROFILE, or configure credentials in ~/.aws/credentials"
                        .into(),
                )
            })?;

            use aws_credential_types::provider::ProvideCredentials;
            let creds = provider
                .provide_credentials()
                .await
                .map_err(|e| ProviderError::Connection(format!("AWS credential error: {}", e)))?;

            Ok(Credentials::new(
                creds.access_key_id(),
                creds.secret_access_key(),
                creds.session_token().map(|s| s.to_string()),
                creds.expiry(),
                "wayland-core-sdk",
            ))
        };

        match rt {
            Ok(_handle) => {
                // Already inside a tokio runtime — use spawn_blocking to avoid nested block_on
                std::thread::scope(|s| {
                    s.spawn(|| {
                        tokio::runtime::Runtime::new()
                            .map_err(|e| {
                                ProviderError::Connection(format!("Runtime error: {}", e))
                            })?
                            .block_on(resolve)
                    })
                    // SAFETY: `join()` on a `std::thread::JoinHandle`
                    // returns `Err` only on a thread panic. The spawned
                    // closure above only awaits `resolve` inside a
                    // freshly-built runtime; the only panic surface
                    // would be `Runtime::new()` returning `Err` which
                    // is handled with `?` and produces an `Err`
                    // return, not a panic.
                    .join()
                    .unwrap()
                })
            }
            Err(_) => {
                // No runtime — safe to create one
                tokio::runtime::Runtime::new()
                    .map_err(|e| ProviderError::Connection(format!("Runtime error: {}", e)))?
                    .block_on(resolve)
            }
        }
    }

    fn sign_request(
        &self,
        method: &str,
        url: &str,
        headers: &HeaderMap,
        body: &[u8],
        credentials: &Credentials,
    ) -> Result<HeaderMap, ProviderError> {
        let mut signing_settings = SigningSettings::default();
        signing_settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;
        signing_settings.signature_location = SignatureLocation::Headers;

        let identity = credentials.clone().into();
        let signing_params = aws_sigv4::sign::v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name("bedrock")
            .time(SystemTime::now())
            .settings(signing_settings)
            .build()
            .map_err(|e| ProviderError::Connection(format!("SigV4 params error: {}", e)))?;

        // Build header pairs for signing
        let header_pairs: Vec<(&str, &str)> = headers
            .iter()
            .filter_map(|(name, value)| value.to_str().ok().map(|v| (name.as_str(), v)))
            .collect();

        let signable_request = SignableRequest::new(
            method,
            url,
            header_pairs.into_iter(),
            SignableBody::Bytes(body),
        )
        .map_err(|e| ProviderError::Connection(format!("Signable request error: {}", e)))?;

        let (signing_instructions, _signature) =
            sigv4_http::sign(signable_request, &signing_params.into())
                .map_err(|e| ProviderError::Connection(format!("SigV4 signing error: {}", e)))?
                .into_parts();

        let mut signed_headers = headers.clone();
        for (name, value) in signing_instructions.headers() {
            signed_headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes())
                    .map_err(|e| ProviderError::Connection(format!("Header name error: {}", e)))?,
                HeaderValue::from_str(value)
                    .map_err(|e| ProviderError::Connection(format!("Header value error: {}", e)))?,
            );
        }

        Ok(signed_headers)
    }

    /// Drive a Mistral / Cohere request through the non-streaming Bedrock
    /// `invoke` endpoint: build the family request body, POST it, buffer the
    /// complete JSON response, parse it with the family parser, and emit the
    /// result as a terminal `LlmEvent` sequence on a channel matching the
    /// shape `stream()` returns for the native Anthropic path.
    ///
    /// `family` MUST be `Mistral` or `Cohere`; `Anthropic` never reaches here.
    async fn invoke_buffered(
        &self,
        request: &LlmRequest,
        family: BedrockFamily,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let url = self.build_invoke_url(&request.model);
        let body = self.build_request_body(request);

        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Connection(format!("JSON serialize error: {}", e)))?;

        let credentials = self.resolve_credentials()?;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let signed_headers =
            self.sign_request("POST", &url, &headers, &body_bytes, &credentials)?;

        let response = with_retry(DEFAULT_MAX_RETRIES, || {
            let client = &self.client;
            let url = &url;
            let signed_headers = signed_headers.clone();
            let body_bytes = body_bytes.clone();
            async move {
                client
                    .post(url)
                    .headers(signed_headers)
                    .body(body_bytes)
                    .send()
                    .await
                    .map_err(crate::retry::provider_error_from_egress)
            }
        })
        .await?;

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
            let message = format_bedrock_error(status.as_u16(), &body_text);
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message,
            });
        }

        let raw = response.text().await.map_err(|e| {
            ProviderError::Connection(format!("Bedrock response read error: {}", e))
        })?;
        dump_response_chunk(&self.debug, &raw);

        let events = decode_buffered_response(family, &raw)?;

        // Re-emit the buffered result on a channel so the caller sees the
        // same `mpsc::Receiver<LlmEvent>` shape the native streaming path
        // returns. Channel is sized to the (small) event count.
        let (tx, rx) = mpsc::channel(events.len().max(1));
        for event in events {
            // Receiver is live (we just created it) — send cannot fail here,
            // but stay graceful if the caller drops rx before draining.
            if tx.send(event).await.is_err() {
                break;
            }
        }

        Ok(rx)
    }
}

/// Decode a buffered Bedrock `invoke` JSON response into the engine's
/// `LlmEvent` sequence, dispatching on model family.
///
/// Cohere's `parse_cohere_response` already yields `Vec<LlmEvent>`. Mistral's
/// `parse_mistral_response` yields a struct, converted here into a
/// `TextDelta` + terminal `Done` pair so both families produce the identical
/// terminal-sequence shape.
fn decode_buffered_response(
    family: BedrockFamily,
    raw: &str,
) -> Result<Vec<LlmEvent>, ProviderError> {
    match family {
        BedrockFamily::Cohere => cohere::parse_cohere_response(raw),
        BedrockFamily::Mistral => {
            let value: Value = serde_json::from_str(raw).map_err(|e| {
                ProviderError::Connection(format!("Mistral response JSON parse error: {}", e))
            })?;
            let parsed =
                mistral::parse_mistral_response(&value).map_err(|e| ProviderError::Api {
                    status: 400,
                    message: format!("Mistral Bedrock response: {}", e),
                })?;
            let mut events = Vec::with_capacity(2);
            if !parsed.text.is_empty() {
                events.push(LlmEvent::TextDelta(parsed.text));
            }
            events.push(LlmEvent::Done {
                stop_reason: parsed.stop_reason,
                finish_reason: parsed.finish_reason,
                usage: parsed.usage,
            });
            Ok(events)
        }
        // The Anthropic family never uses the buffered path — it streams.
        BedrockFamily::Anthropic => Err(ProviderError::Connection(
            "decode_buffered_response called for the Anthropic family (streaming-only)".into(),
        )),
    }
}

/// Which AWS Bedrock model family a request routes to. The family decides
/// both the request/response JSON schema and the runtime endpoint
/// (`invoke-with-response-stream` for the native-streaming Anthropic family,
/// `invoke` for the buffered Mistral/Cohere families).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BedrockFamily {
    Anthropic,
    Mistral,
    Cohere,
}

impl BedrockFamily {
    /// Classify a Bedrock model id into its family.
    fn classify(model: &str) -> Self {
        if mistral::is_mistral_model(model) {
            BedrockFamily::Mistral
        } else if cohere::is_cohere_model(model) {
            BedrockFamily::Cohere
        } else {
            BedrockFamily::Anthropic
        }
    }
}

#[async_trait]
impl LlmProvider for BedrockProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        // v0.8.1 U8a: expand short aliases (e.g. `claude-3-5-sonnet`) to the
        // canonical Bedrock model id before the family classifier runs. Full
        // ids pass through unchanged; an unknown short name is left as-is so
        // it surfaces as a Bedrock 404 rather than a silent substitution.
        let resolved_model = resolve_model_id(&request.model);
        let request = if resolved_model == request.model {
            // Common case: no alias substitution, avoid the clone.
            std::borrow::Cow::Borrowed(request)
        } else {
            let mut owned = request.clone();
            owned.model = resolved_model;
            std::borrow::Cow::Owned(owned)
        };
        let request: &LlmRequest = &request;
        let family = BedrockFamily::classify(&request.model);

        // Mistral / Cohere: non-streaming `invoke` endpoint. Buffer the full
        // JSON response and re-emit it as a terminal LlmEvent sequence — the
        // S9 buffered-then-emitted pattern. Output is byte-identical to a
        // native stream; only intra-response timing differs.
        // NOTE(v0.6.4): native Bedrock event-stream parsing for Mistral/Cohere
        // is a documented future enhancement — these families work now.
        if family != BedrockFamily::Anthropic {
            return self.invoke_buffered(request, family).await;
        }

        let url = self.build_url(&request.model);
        let body = self.build_request_body(request);

        dump_request_body(&self.debug, &body);
        reset_response_dump(&self.debug);

        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Connection(format!("JSON serialize error: {}", e)))?;

        let credentials = self.resolve_credentials()?;

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let signed_headers =
            self.sign_request("POST", &url, &headers, &body_bytes, &credentials)?;

        // Bedrock uses SigV4-signed headers + raw body bytes. We can't use
        // `builder_send_with_retry` (body is not Clone on RequestBuilder), so
        // we clone body_bytes per attempt and re-use the already-signed headers.
        let response = with_retry(DEFAULT_MAX_RETRIES, || {
            let client = &self.client;
            let url = &url;
            let signed_headers = signed_headers.clone();
            let body_bytes = body_bytes.clone();
            async move {
                client
                    .post(url)
                    .headers(signed_headers)
                    .body(body_bytes)
                    .send()
                    .await
                    .map_err(crate::retry::provider_error_from_egress)
            }
        })
        .await?;

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
            let message = format_bedrock_error(status.as_u16(), &body_text);
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message,
            });
        }

        let (tx, rx) = mpsc::channel(64);
        let debug = self.debug.clone();

        // AWS event stream uses binary framing
        tokio::spawn(async move {
            if let Err(e) = process_aws_event_stream(response, &tx, &debug).await {
                let _ = tx.send(LlmEvent::Error(e.to_string())).await;
            }
        });

        Ok(rx)
    }
}

/// Maximum size the AWS event-stream reassembly buffer may reach before the
/// parser gives up (M4). 8 MiB comfortably exceeds any legitimate Bedrock
/// event-stream message while bounding memory against a hostile prelude.
const MAX_AWS_EVENT_BUFFER_BYTES: usize = 8 * 1024 * 1024;

/// Process the AWS event stream (binary framed) from Bedrock
async fn process_aws_event_stream(
    response: reqwest::Response,
    tx: &mpsc::Sender<LlmEvent>,
    debug: &DebugConfig,
) -> Result<(), ProviderError> {
    use futures::StreamExt;

    let mut state = anthropic_shared::StreamState::new();
    let mut buffer = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ProviderError::Connection(e.to_string()))?;
        buffer.extend_from_slice(&chunk);

        // M4: cap the buffer. A hostile endpoint can declare an enormous
        // `total_len` in an AWS event-stream prelude (a 4-byte field) and
        // `parse_aws_event` would keep waiting for bytes that never come,
        // growing `buffer` without bound. 8 MiB is far above any
        // legitimate Bedrock event-stream message.
        if buffer.len() > MAX_AWS_EVENT_BUFFER_BYTES {
            return Err(ProviderError::Parse(format!(
                "AWS event-stream buffer exceeded {MAX_AWS_EVENT_BUFFER_BYTES} bytes \
                 without a complete message frame"
            )));
        }

        // Parse complete AWS event stream messages from buffer.
        // H-8 / rel-panic-66: `parse_aws_event` now returns `Err` on a
        // malformed frame (bad `total_len`/`headers_len`) instead of
        // underflowing/OOB-slicing. Propagating that `Err` lets the spawned
        // task surface `LlmEvent::Error` rather than silently truncating.
        while let Some((event_data, consumed)) = parse_aws_event(&buffer)? {
            buffer = buffer[consumed..].to_vec();

            if let Some(payload) = event_data {
                // The payload contains an SSE-like structure with "bytes" field
                if let Ok(wrapper) = serde_json::from_slice::<Value>(&payload) {
                    // Bedrock wraps the payload in {"bytes": "base64-encoded-data"}
                    if let Some(b64) = wrapper["bytes"].as_str()
                        && let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64)
                        && let Ok(inner) = String::from_utf8(decoded)
                    {
                        dump_response_chunk(debug, &inner);
                        // Inner payload is JSON with event type hints
                        if let Ok(json_val) = serde_json::from_str::<Value>(&inner) {
                            let event_type = json_val["type"].as_str().unwrap_or("");
                            let events =
                                anthropic_shared::parse_sse_data(event_type, &inner, &mut state);
                            for event in events {
                                if tx.send(event).await.is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // If the inner SSE stream ended without a message_delta carrying a
    // stop_reason, we still need to terminate the agent loop. Map to
    // FinishReason::Error because we have no provider signal — pre-Task-F
    // code defaulted to EndTurn here, masking aborted streams.
    if state.input_tokens > 0 || state.output_tokens > 0 {
        eprintln!(
            "[wcore-providers] bedrock: stream ended without message_delta stop_reason, emitting FinishReason::Error"
        );
        let _ = tx
            .send(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::Error,
                usage: TokenUsage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cache_creation_tokens: state.cache_creation_tokens,
                    cache_read_tokens: state.cache_read_tokens,
                },
            })
            .await;
    }

    Ok(())
}

/// A decoded AWS event-stream frame: `(payload, consumed)` — the optional
/// event payload (`None` for empty/header-only frames) and the number of
/// buffer bytes the frame occupied.
type AwsEventFrame = (Option<Vec<u8>>, usize);

/// Parse one AWS event stream message from the buffer.
///
/// Returns:
/// - `Ok(Some((Some(payload), consumed)))` — a complete message with a payload;
/// - `Ok(Some((None, consumed)))` — a complete message with an empty payload
///   (e.g. the initial-response event);
/// - `Ok(None)` — more bytes are needed to complete the current message;
/// - `Err(ProviderError::Parse(..))` — the frame is malformed and can never be
///   valid (H-8 / rel-panic-66).
///
/// AWS event stream binary format:
/// - Prelude: total_len (4 bytes, big-endian) + headers_len (4 bytes) + prelude_crc (4 bytes)
/// - Headers: variable length
/// - Payload: variable length
/// - Message CRC: 4 bytes
///
/// H-8 / rel-panic-66: every field is validated BEFORE any arithmetic. A frame
/// declaring `total_len ∈ 0..16` or `headers_len > total_len - 16` previously
/// underflowed `total_len - 4` (release: wrap to ~usize::MAX → inverted/OOB
/// `buffer[start..end]` panic; debug: subtraction panic). The parser runs in a
/// bare `tokio::spawn`, so that panic aborted the task and dropped `tx` without
/// an error event — a silent truncation. Now a bad frame is a typed `Err` that
/// the stream loop forwards as `LlmEvent::Error`.
fn parse_aws_event(buffer: &[u8]) -> Result<Option<AwsEventFrame>, ProviderError> {
    if buffer.len() < 12 {
        return Ok(None); // Need at least the prelude
    }

    let total_len = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    let headers_len = u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]) as usize;

    // A valid message is at minimum: 8-byte prelude lengths + 4-byte
    // prelude CRC + 4-byte message CRC = 16 bytes, with zero-length headers
    // and zero-length payload. Anything smaller is structurally impossible.
    if total_len < 16 {
        return Err(ProviderError::Parse(format!(
            "AWS event-stream frame declares total_len={total_len} (< 16-byte minimum)"
        )));
    }

    // `headers_len` must leave room for the 12-byte prelude and the trailing
    // 4-byte message CRC: headers_len <= total_len - 16. Checked arithmetic
    // throughout so a hostile field can never underflow.
    let max_headers = total_len
        .checked_sub(16)
        .expect("total_len >= 16 checked above");
    if headers_len > max_headers {
        return Err(ProviderError::Parse(format!(
            "AWS event-stream frame declares headers_len={headers_len} \
             exceeding total_len-16={max_headers}"
        )));
    }

    if buffer.len() < total_len {
        return Ok(None); // Incomplete message — wait for more bytes.
    }

    // Prelude is 12 bytes; payload starts after prelude + headers.
    let payload_start = 12usize
        .checked_add(headers_len)
        .ok_or_else(|| ProviderError::Parse("AWS event-stream payload_start overflow".into()))?;
    // Payload ends 4 bytes before total_len (message CRC). Both bounds are now
    // provably valid: payload_start = 12 + headers_len <= 12 + (total_len-16)
    // = total_len - 4 = payload_end, so payload_start <= payload_end always.
    let payload_end = total_len
        .checked_sub(4)
        .expect("total_len >= 16 checked above");

    if payload_start < payload_end {
        let payload = buffer[payload_start..payload_end].to_vec();
        Ok(Some((Some(payload), total_len)))
    } else {
        // Empty payload (e.g., initial response event). `payload_start ==
        // payload_end` lands here — a zero-length slice, no copy needed.
        Ok(Some((None, total_len)))
    }
}

/// Format Bedrock error responses with actionable hints
fn format_bedrock_error(status: u16, body: &str) -> String {
    // Try to extract the AWS error type from the response
    let error_type = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        v.get("__type")
            .or_else(|| v.get("type"))
            .and_then(|t| t.as_str().map(String::from))
    });

    let hint = match status {
        403 => Some(
            "Check IAM permissions: the role/user needs bedrock:InvokeModelWithResponseStream. \
             Also verify the model is enabled in the Bedrock console for your account.",
        ),
        404 => Some(
            "Model not found in this region. Verify the model ID and that it's available in \
             your configured AWS region.",
        ),
        400 => {
            if body.contains("schema") || body.contains("Schema") {
                Some(
                    "Request schema validation failed. If using tools, try enabling sanitize_schema=true in [providers.bedrock.compat].",
                )
            } else {
                Some("Bad request — check model parameters and message format.")
            }
        }
        503 | 529 => Some(
            "Service overloaded or throttled. You may have exceeded your provisioned throughput quota. \
             Retry after a moment or request a quota increase.",
        ),
        _ => None,
    };

    let type_info = error_type.map(|t| format!(" [{}]", t)).unwrap_or_default();

    match hint {
        Some(h) => format!("{}{}\nHint: {}", body, type_info, h),
        None => format!("{}{}", body, type_info),
    }
}

/// Expand short Bedrock model aliases into full AWS Bedrock model ids.
///
/// AWS Bedrock identifies models by long, version-stamped ids
/// (e.g. `anthropic.claude-3-5-sonnet-20240620-v1:0`). Operators often want to
/// configure a model by its short colloquial name (`claude-3-5-sonnet`) and
/// have the provider expand it to the canonical id at request time. Inputs
/// that don't match an alias pass through unchanged — operators can still
/// configure a full Bedrock id directly.
///
/// v0.8.1 U8a: introduced alongside the native SigV4 + IAM-credential-chain
/// path so short names like `claude-3-5-sonnet` resolve to real model ids.
pub fn resolve_model_id(input: &str) -> String {
    match input {
        // Anthropic on Bedrock
        "claude-3-5-sonnet" => "anthropic.claude-3-5-sonnet-20240620-v1:0".to_string(),
        "claude-3-5-sonnet-v2" => "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
        "claude-3-5-haiku" => "anthropic.claude-3-5-haiku-20241022-v1:0".to_string(),
        "claude-3-opus" => "anthropic.claude-3-opus-20240229-v1:0".to_string(),
        "claude-3-sonnet" => "anthropic.claude-3-sonnet-20240229-v1:0".to_string(),
        "claude-3-haiku" => "anthropic.claude-3-haiku-20240307-v1:0".to_string(),
        // Meta Llama on Bedrock
        "llama-3-1-70b" => "meta.llama3-1-70b-instruct-v1:0".to_string(),
        "llama-3-1-8b" => "meta.llama3-1-8b-instruct-v1:0".to_string(),
        // Mistral on Bedrock
        "mistral-large" => "mistral.mistral-large-2402-v1:0".to_string(),
        "mistral-large-2407" => "mistral.mistral-large-2407-v1:0".to_string(),
        "mixtral-8x7b" => "mistral.mixtral-8x7b-instruct-v0:1".to_string(),
        // Cohere on Bedrock
        "command-r" => "cohere.command-r-v1:0".to_string(),
        "command-r-plus" => "cohere.command-r-plus-v1:0".to_string(),
        // Pass-through for full ids and unknown inputs.
        other => other.to_string(),
    }
}

/// Build AwsCredentials from wcore-config's BedrockConfig
pub fn credentials_from_config(bc: &wcore_config::config::BedrockConfig) -> AwsCredentials {
    if let (Some(key_id), Some(secret)) = (&bc.access_key_id, &bc.secret_access_key) {
        AwsCredentials::Explicit {
            access_key_id: key_id.clone(),
            secret_access_key: secret.clone(),
            session_token: bc.session_token.clone(),
        }
    } else if let Some(profile) = &bc.profile {
        AwsCredentials::Profile(profile.clone())
    } else {
        AwsCredentials::Environment
    }
}

// ===========================================================================
// P7 v0.6.3 — Bedrock Mistral model family
// ===========================================================================
//
// AWS Bedrock hosts multiple model families under one runtime endpoint, but
// each family has a distinct request/response JSON schema. The code above
// targets the Anthropic-on-Bedrock schema (`anthropic_version` + Anthropic
// SSE event-stream passthrough). The `mistral` submodule below adds the
// Mistral-on-Bedrock family: `mistral.*` model ids, the Mistral chat request
// body (`messages` + `max_tokens` / `temperature` / `top_p`), and Mistral's
// response shape (`choices[].message.content` + `stop_reason`).
//
// This block is purely additive — no existing Bedrock code above is touched.
// Source: Forge `bedrock-mistral-provider.ts` (Apache-2.0). Plan v2 Tier 2A P7.
pub mod mistral {
    use serde_json::{Value, json};

    use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};

    /// Catalog of Mistral models available on AWS Bedrock.
    ///
    /// Bedrock identifies models by family-prefixed ids. The Mistral family
    /// uses the `mistral.` prefix. Region-suffixed variants (e.g.
    /// `us.mistral.*` cross-region inference profiles) are recognised by
    /// [`is_mistral_model`] via substring match, so they need no separate
    /// catalog entry.
    pub const MISTRAL_BEDROCK_MODELS: &[&str] = &[
        "mistral.mistral-7b-instruct-v0:2",
        "mistral.mixtral-8x7b-instruct-v0:1",
        "mistral.mistral-large-2402-v1:0",
        "mistral.mistral-large-2407-v1:0",
        "mistral.mistral-small-2402-v1:0",
    ];

    /// Return the list of Mistral-on-Bedrock model ids this provider knows.
    pub fn mistral_models() -> &'static [&'static str] {
        MISTRAL_BEDROCK_MODELS
    }

    /// True if `model` is a Mistral model id served on Bedrock.
    ///
    /// Matches the bare `mistral.` family prefix as well as cross-region
    /// inference-profile ids (`us.mistral.*`, `eu.mistral.*`, …) which embed
    /// the family id after a region prefix.
    pub fn is_mistral_model(model: &str) -> bool {
        model.starts_with("mistral.") || model.contains(".mistral.")
    }

    /// Build the Mistral-on-Bedrock request body.
    ///
    /// Unlike Anthropic-on-Bedrock (which carries `anthropic_version` and a
    /// separate `system` field), the Mistral family uses an OpenAI-style
    /// `messages` array with the system prompt folded in as a leading
    /// `system`-role message, plus flat `max_tokens` / `temperature` /
    /// `top_p` sampling controls.
    pub fn build_mistral_request_body(
        system: &str,
        messages: &[Message],
        max_tokens: u32,
        temperature: Option<f32>,
        top_p: Option<f32>,
    ) -> Value {
        let mut wire: Vec<Value> = Vec::with_capacity(messages.len() + 1);

        if !system.is_empty() {
            wire.push(json!({ "role": "system", "content": system }));
        }

        for msg in messages {
            let role = match msg.role {
                Role::User | Role::Tool => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
            };
            wire.push(json!({ "role": role, "content": flatten_content(&msg.content) }));
        }

        let mut body = json!({
            "messages": wire,
            "max_tokens": max_tokens,
        });

        if let Some(t) = temperature {
            body["temperature"] = json!(t);
        }
        if let Some(p) = top_p {
            body["top_p"] = json!(p);
        }

        body
    }

    /// Collapse a message's content blocks into the plain-text string the
    /// Mistral-on-Bedrock chat schema expects. Tool-use / tool-result blocks
    /// are rendered as readable text since this family is invoked as a
    /// text-completion model on Bedrock.
    fn flatten_content(blocks: &[ContentBlock]) -> String {
        let mut out = String::new();
        for block in blocks {
            match block {
                ContentBlock::Text { text } => out.push_str(text),
                ContentBlock::Thinking { thinking } => out.push_str(thinking),
                ContentBlock::ToolResult { content, .. } => out.push_str(content),
                ContentBlock::ToolUse { name, input, .. } => {
                    out.push_str(&format!("[tool_use: {name} {input}]"));
                }
            }
        }
        out
    }

    /// Parsed result of a Mistral-on-Bedrock invocation.
    ///
    /// No `PartialEq` derive — `TokenUsage` (from `wcore-types`) does not
    /// implement it; callers compare the individual fields instead.
    #[derive(Debug, Clone)]
    pub struct MistralResponse {
        pub text: String,
        pub stop_reason: StopReason,
        pub finish_reason: FinishReason,
        pub usage: TokenUsage,
    }

    /// Map a Mistral-on-Bedrock `stop_reason` / `finish_reason` string to the
    /// engine's protocol-level reasons.
    ///
    /// Mistral on Bedrock emits `"stop"` (clean finish), `"length"`
    /// (max_tokens hit), `"model_length"`, and `"tool_calls"`. Unknown values
    /// map to [`FinishReason::Error`] rather than silently degrading.
    fn map_stop_reason(raw: &str) -> (StopReason, FinishReason) {
        match raw {
            "stop" | "tool_calls" => (StopReason::EndTurn, FinishReason::Stop),
            "length" | "model_length" => (StopReason::MaxTokens, FinishReason::Length),
            _ => (StopReason::EndTurn, FinishReason::Error),
        }
    }

    /// Parse a non-streaming Mistral-on-Bedrock response body.
    ///
    /// The Mistral family returns an OpenAI-style envelope:
    /// `{ "choices": [{ "message": { "content": "…" }, "stop_reason": "stop" }],
    ///    "usage": { "prompt_tokens": N, "completion_tokens": M } }`.
    /// Returns `Err` with a diagnostic string if the envelope is missing the
    /// `choices` array or the first choice's message content.
    pub fn parse_mistral_response(body: &Value) -> Result<MistralResponse, String> {
        let choice = body
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .ok_or_else(|| "Mistral response missing `choices` array".to_string())?;

        let text = choice
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| "Mistral choice missing `message.content`".to_string())?
            .to_string();

        // Mistral on Bedrock surfaces the reason as `stop_reason`; some
        // variants use `finish_reason`. Accept either; default to `"stop"`.
        let raw_reason = choice
            .get("stop_reason")
            .or_else(|| choice.get("finish_reason"))
            .and_then(|r| r.as_str())
            .unwrap_or("stop");
        let (stop_reason, finish_reason) = map_stop_reason(raw_reason);

        let usage = body.get("usage");
        let input_tokens = usage
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let output_tokens = usage
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        Ok(MistralResponse {
            text,
            stop_reason,
            finish_reason,
            usage: TokenUsage {
                input_tokens,
                output_tokens,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        })
    }
}

// ====================================================================
// P8 v0.6.3 — Cohere model family on AWS Bedrock.
//
// Bedrock dispatches request/response bodies per model family. The
// Anthropic family above uses the `anthropic_*` passthrough shape; the
// Cohere Command family uses Cohere's own Chat API schema. Everything
// Cohere-specific is contained in this `mod cohere` block so it composes
// cleanly alongside other Bedrock family additions.
// ====================================================================
pub mod cohere {
    use super::*;
    use wcore_types::message::{ContentBlock, Role};

    /// Bedrock model ids for the Cohere Command family.
    /// Cohere-on-Bedrock ids are always prefixed `cohere.command`.
    pub const COHERE_BEDROCK_MODELS: &[&str] = &[
        "cohere.command-r-v1:0",
        "cohere.command-r-plus-v1:0",
        "cohere.command-text-v14",
        "cohere.command-light-text-v14",
    ];

    /// True when `model` is an AWS Bedrock model id in the Cohere family.
    /// Routing is prefix-based so future `cohere.command-*` ids are picked
    /// up without a registry edit.
    pub fn is_cohere_model(model: &str) -> bool {
        model.starts_with("cohere.command")
    }

    /// The Cohere model catalog exposed for this Bedrock variant.
    pub fn cohere_models() -> Vec<&'static str> {
        COHERE_BEDROCK_MODELS.to_vec()
    }

    /// Flatten a message's content blocks into a single plain-text string.
    /// Cohere's Bedrock Chat API is text-only on the wire; tool_use /
    /// thinking blocks are dropped (Cohere tool calling on Bedrock is not
    /// wired here).
    fn block_text(content: &[ContentBlock]) -> String {
        let mut out = String::new();
        for block in content {
            match block {
                ContentBlock::Text { text } => out.push_str(text),
                ContentBlock::ToolResult { content, .. } => out.push_str(content),
                _ => {}
            }
        }
        out
    }

    /// Build the Cohere-on-Bedrock request body.
    ///
    /// Schema (Cohere Command R Chat API on Bedrock):
    /// `{ message, chat_history?, preamble?, max_tokens, temperature?, p? }`
    /// where `chat_history` entries are `{ role: "USER"|"CHATBOT", message }`.
    pub fn build_cohere_request_body(request: &LlmRequest) -> Value {
        // The final user turn becomes `message`; everything prior becomes
        // `chat_history`. If the conversation does not end on a user turn
        // (shouldn't happen in normal agent loops) `message` is left empty.
        let mut chat_history: Vec<Value> = Vec::new();
        let mut current_message = String::new();

        let last_user_idx = request
            .messages
            .iter()
            .rposition(|m| matches!(m.role, Role::User | Role::Tool));

        for (idx, msg) in request.messages.iter().enumerate() {
            let text = block_text(&msg.content);
            if Some(idx) == last_user_idx {
                current_message = text;
                continue;
            }
            let role = match msg.role {
                Role::User | Role::Tool => "USER",
                Role::Assistant => "CHATBOT",
                Role::System => continue, // system goes in `preamble`
            };
            chat_history.push(json!({ "role": role, "message": text }));
        }

        let mut body = json!({
            "message": current_message,
            "max_tokens": request.max_tokens,
        });

        if !chat_history.is_empty() {
            body["chat_history"] = json!(chat_history);
        }
        if !request.system.is_empty() {
            body["preamble"] = json!(&request.system);
        }

        body
    }

    /// Map a Cohere `finish_reason` string to the protocol `FinishReason`.
    /// Cohere returns `COMPLETE`, `MAX_TOKENS`, `ERROR`, `ERROR_TOXIC`,
    /// `ERROR_LIMIT`, `STOP_SEQUENCE`.
    pub fn map_finish_reason(raw: &str) -> (StopReason, FinishReason) {
        match raw {
            "COMPLETE" | "STOP_SEQUENCE" => (StopReason::EndTurn, FinishReason::Stop),
            "MAX_TOKENS" => (StopReason::MaxTokens, FinishReason::Length),
            _ => (StopReason::EndTurn, FinishReason::Error),
        }
    }

    /// Parse a complete (non-streamed) Cohere-on-Bedrock chat response into
    /// the engine's `LlmEvent` sequence: a `TextDelta` for the body text
    /// followed by a terminal `Done`.
    ///
    /// Response shape:
    /// `{ "text": "...", "finish_reason": "COMPLETE",
    ///    "meta": { "tokens": { "input_tokens": N, "output_tokens": M } } }`
    pub fn parse_cohere_response(raw: &str) -> Result<Vec<LlmEvent>, ProviderError> {
        let value: Value = serde_json::from_str(raw).map_err(|e| {
            ProviderError::Connection(format!("Cohere response JSON parse error: {}", e))
        })?;

        // Cohere/Bedrock error envelope.
        if let Some(msg) = value.get("message").and_then(|m| m.as_str())
            && value.get("text").is_none()
        {
            return Err(ProviderError::Api {
                status: 400,
                message: format!("Cohere Bedrock error: {}", msg),
            });
        }

        let text = value
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string();

        let raw_finish = value
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .unwrap_or("ERROR");
        let (stop_reason, finish_reason) = map_finish_reason(raw_finish);

        let tokens = value.get("meta").and_then(|m| m.get("tokens"));
        let input_tokens = tokens
            .and_then(|t| t.get("input_tokens"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        let output_tokens = tokens
            .and_then(|t| t.get("output_tokens"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0);

        let mut events = Vec::new();
        if !text.is_empty() {
            events.push(LlmEvent::TextDelta(text));
        }
        events.push(LlmEvent::Done {
            stop_reason,
            finish_reason,
            usage: TokenUsage {
                input_tokens,
                output_tokens,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        });
        Ok(events)
    }
}
// ==================== end P8 Cohere family ====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic_shared::{StreamState, parse_sse_data};
    use wcore_types::llm::LlmEvent;

    // -----------------------------------------------------------------------
    // H-8 / rel-panic-66 — AWS event-stream frame validation
    // -----------------------------------------------------------------------

    /// Build a minimal AWS event-stream frame: 4-byte total_len, 4-byte
    /// headers_len, 4-byte (zeroed) prelude CRC, `headers_len` header bytes,
    /// `payload` bytes, 4-byte (zeroed) message CRC. `total_len` is the real
    /// total unless `force_total` overrides it (to forge a malformed frame).
    fn make_frame(headers: &[u8], payload: &[u8], force_total: Option<u32>) -> Vec<u8> {
        let real_total = 12 + headers.len() + payload.len() + 4;
        let total = force_total.unwrap_or(real_total as u32);
        let mut f = Vec::new();
        f.extend_from_slice(&total.to_be_bytes());
        f.extend_from_slice(&(headers.len() as u32).to_be_bytes());
        f.extend_from_slice(&[0u8; 4]); // prelude CRC (unchecked here)
        f.extend_from_slice(headers);
        f.extend_from_slice(payload);
        f.extend_from_slice(&[0u8; 4]); // message CRC (unchecked here)
        f
    }

    #[test]
    fn parse_aws_event_extracts_payload_of_valid_frame() {
        let frame = make_frame(b"hdr", b"PAYLOAD", None);
        let consumed_total = frame.len();
        let out = parse_aws_event(&frame).expect("valid frame must not error");
        let (payload, consumed) = out.expect("a complete frame must parse");
        assert_eq!(consumed, consumed_total);
        assert_eq!(payload.as_deref(), Some(&b"PAYLOAD"[..]));
    }

    #[test]
    fn parse_aws_event_empty_payload_frame_yields_none_payload() {
        // headers present, zero payload → Some((None, total)).
        let frame = make_frame(b"hdronly", b"", None);
        let out = parse_aws_event(&frame)
            .expect("valid frame")
            .expect("complete");
        assert!(
            out.0.is_none(),
            "empty-payload frame must yield None payload"
        );
    }

    #[test]
    fn parse_aws_event_needs_more_bytes_returns_ok_none() {
        // Fewer than 12 bytes — incomplete prelude.
        assert!(parse_aws_event(&[0u8; 8]).unwrap().is_none());
        // Prelude says total_len is large but buffer is short — incomplete.
        let mut buf = Vec::new();
        buf.extend_from_slice(&1000u32.to_be_bytes());
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&[0u8; 4]);
        assert!(parse_aws_event(&buf).unwrap().is_none());
    }

    /// The exploit frame: `total_len ∈ 0..16`. Before the fix this underflowed
    /// `total_len - 4` and OOB-sliced (release) / panicked (debug). Now it is a
    /// clean typed `Err` with NO panic.
    #[test]
    fn parse_aws_event_rejects_tiny_total_len_without_panic() {
        for forged in [0u32, 3, 4, 12, 15] {
            // Pad the buffer to >= 12 bytes so the length guards (not the
            // short-buffer guard) are what reject it.
            let mut buf = Vec::new();
            buf.extend_from_slice(&forged.to_be_bytes());
            buf.extend_from_slice(&0u32.to_be_bytes()); // headers_len
            buf.extend_from_slice(&[0u8; 8]); // pad past 12 bytes
            let err =
                parse_aws_event(&buf).expect_err(&format!("total_len={forged} must be rejected"));
            assert!(matches!(err, ProviderError::Parse(_)), "got {err:?}");
        }
    }

    /// A `headers_len` larger than `total_len - 16` is rejected before any
    /// slicing — guards against an OOB `payload_start`.
    #[test]
    fn parse_aws_event_rejects_oversized_headers_len() {
        // total_len = 20 (room for 4 header bytes max), but declare 100.
        let mut buf = Vec::new();
        buf.extend_from_slice(&20u32.to_be_bytes());
        buf.extend_from_slice(&100u32.to_be_bytes());
        buf.extend_from_slice(&[0u8; 16]); // pad to total_len so length guard passes
        let err = parse_aws_event(&buf).expect_err("oversized headers_len must be rejected");
        assert!(matches!(err, ProviderError::Parse(_)), "got {err:?}");
    }

    /// A malformed frame surfaced through the full stream loop must reach the
    /// caller as `LlmEvent::Error`, not a silent truncation. We drive
    /// `process_aws_event_stream` against a wiremock body that is one hostile
    /// frame and assert an Error event arrives.
    #[tokio::test]
    async fn malformed_frame_surfaces_llm_event_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // total_len=3 (< 16) with enough trailing bytes to pass the 12-byte
        // short-buffer guard — the underflow trigger.
        let mut body = Vec::new();
        body.extend_from_slice(&3u32.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&[0u8; 8]);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(body, "application/vnd.amazon.eventstream"),
            )
            .mount(&server)
            .await;

        let provider = BedrockProvider::new_with_endpoint_override(
            "us-east-1",
            AwsCredentials::Explicit {
                access_key_id: "AKIA_TEST".into(),
                secret_access_key: "secret".into(),
                session_token: None,
            },
            false,
            ProviderCompat::default(),
            DebugConfig::default(),
            &server.uri(),
        );
        let req = LlmRequest {
            model: "anthropic.claude-3-haiku-20240307-v1:0".into(),
            max_tokens: 16,
            messages: vec![wcore_types::message::Message::new(
                wcore_types::message::Role::User,
                vec![wcore_types::message::ContentBlock::Text { text: "hi".into() }],
            )],
            ..Default::default()
        };

        let mut rx = provider.stream(&req).await.expect("stream starts");
        let mut saw_error = false;
        while let Some(ev) = rx.recv().await {
            if let LlmEvent::Error(msg) = ev {
                assert!(
                    msg.contains("total_len"),
                    "expected frame error, got: {msg}"
                );
                saw_error = true;
                break;
            }
        }
        assert!(saw_error, "a malformed frame must surface LlmEvent::Error");
    }

    /// Bedrock's anthropic-passthrough event stream wraps Anthropic SSE
    /// verbatim; parsing is delegated to `anthropic_shared::parse_sse_data`.
    /// This test pins the contract that a `message_delta` carrying
    /// `max_tokens` surfaces as `FinishReason::Length` (the bedrock entry
    /// in the Task F provider-mapping table).
    #[test]
    fn bedrock_max_tokens_maps_to_length_via_anthropic_passthrough() {
        let mut state = StreamState::new();
        let data = r#"{"delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":42}}"#;
        let events = parse_sse_data("message_delta", data, &mut state);
        assert_eq!(events.len(), 1);
        match &events[0] {
            LlmEvent::Done { finish_reason, .. } => {
                assert_eq!(*finish_reason, FinishReason::Length);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn bedrock_end_turn_maps_to_stop_via_anthropic_passthrough() {
        let mut state = StreamState::new();
        let data = r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#;
        let events = parse_sse_data("message_delta", data, &mut state);
        match &events[0] {
            LlmEvent::Done { finish_reason, .. } => {
                assert_eq!(*finish_reason, FinishReason::Stop);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // v0.8.1 U8a — short-alias resolution for Bedrock model ids
    // -----------------------------------------------------------------------

    mod alias_resolution {
        use super::super::resolve_model_id;

        /// Short Anthropic-on-Bedrock aliases expand to the canonical
        /// version-stamped Bedrock model ids.
        #[test]
        fn anthropic_aliases_expand_to_full_ids() {
            assert_eq!(
                resolve_model_id("claude-3-5-sonnet"),
                "anthropic.claude-3-5-sonnet-20240620-v1:0"
            );
            assert_eq!(
                resolve_model_id("claude-3-5-sonnet-v2"),
                "anthropic.claude-3-5-sonnet-20241022-v2:0"
            );
            assert_eq!(
                resolve_model_id("claude-3-5-haiku"),
                "anthropic.claude-3-5-haiku-20241022-v1:0"
            );
            assert_eq!(
                resolve_model_id("claude-3-opus"),
                "anthropic.claude-3-opus-20240229-v1:0"
            );
        }

        /// Short aliases for non-Anthropic families on Bedrock also expand.
        #[test]
        fn cross_family_aliases_expand() {
            assert_eq!(
                resolve_model_id("llama-3-1-70b"),
                "meta.llama3-1-70b-instruct-v1:0"
            );
            assert_eq!(
                resolve_model_id("mistral-large"),
                "mistral.mistral-large-2402-v1:0"
            );
            assert_eq!(
                resolve_model_id("mixtral-8x7b"),
                "mistral.mixtral-8x7b-instruct-v0:1"
            );
            assert_eq!(resolve_model_id("command-r"), "cohere.command-r-v1:0");
            assert_eq!(
                resolve_model_id("command-r-plus"),
                "cohere.command-r-plus-v1:0"
            );
        }

        /// A full Bedrock model id passes through resolution unchanged — the
        /// resolver must not corrupt operator-configured canonical ids.
        #[test]
        fn full_bedrock_id_passes_through_unchanged() {
            let canonical = "anthropic.claude-3-5-sonnet-20240620-v1:0";
            assert_eq!(resolve_model_id(canonical), canonical);

            let mistral = "mistral.mistral-large-2407-v1:0";
            assert_eq!(resolve_model_id(mistral), mistral);
        }

        /// Unknown short names pass through (so an unrecognised alias surfaces
        /// as an honest Bedrock 404 rather than a silent substitution).
        #[test]
        fn unknown_alias_passes_through() {
            assert_eq!(resolve_model_id("no-such-model"), "no-such-model");
            assert_eq!(resolve_model_id(""), "");
        }
    }

    // -----------------------------------------------------------------------
    // P7 v0.6.3 — Bedrock Mistral model family
    // -----------------------------------------------------------------------

    mod mistral_family {
        use super::super::mistral;
        use serde_json::json;
        use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason};

        /// A `mistral.*` Bedrock id is recognised and routes to the Mistral
        /// family, while Anthropic / Titan / Cohere ids are not.
        #[test]
        fn mistral_model_id_recognized_and_routes_to_family() {
            assert!(mistral::is_mistral_model("mistral.mistral-large-2407-v1:0"));
            assert!(mistral::is_mistral_model(
                "mistral.mixtral-8x7b-instruct-v0:1"
            ));
            // Cross-region inference profile (region-prefixed) still routes.
            assert!(mistral::is_mistral_model(
                "us.mistral.mistral-large-2407-v1:0"
            ));
            // Other Bedrock families must NOT route to Mistral.
            assert!(!mistral::is_mistral_model(
                "anthropic.claude-3-5-sonnet-20241022-v2:0"
            ));
            assert!(!mistral::is_mistral_model("cohere.command-r-v1:0"));
            assert!(!mistral::is_mistral_model("amazon.titan-text-express-v1"));
        }

        /// The Mistral request body uses an OpenAI-style `messages` array with
        /// the system prompt folded in as a leading system message, plus flat
        /// `max_tokens` / `temperature` / `top_p` sampling controls.
        #[test]
        fn build_request_body_shape() {
            let messages = vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            )];
            let body = mistral::build_mistral_request_body(
                "you are helpful",
                &messages,
                256,
                Some(0.7),
                Some(0.95),
            );

            assert_eq!(body["max_tokens"], 256);
            // Sampling params are stored as f32, so JSON round-trips them as
            // a slightly-imprecise f64 — compare with a tolerance.
            assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 1e-4);
            assert!((body["top_p"].as_f64().unwrap() - 0.95).abs() < 1e-4);

            let wire = body["messages"].as_array().expect("messages array");
            // Leading system message + the user message.
            assert_eq!(wire.len(), 2);
            assert_eq!(wire[0]["role"], "system");
            assert_eq!(wire[0]["content"], "you are helpful");
            assert_eq!(wire[1]["role"], "user");
            assert_eq!(wire[1]["content"], "hello");

            // No `anthropic_version` — that key belongs to the Anthropic family.
            assert!(body.get("anthropic_version").is_none());
        }

        /// An empty system prompt and unset sampling params are omitted from
        /// the body rather than serialized as empty/null.
        #[test]
        fn build_request_body_omits_empty_system_and_unset_sampling() {
            let messages = vec![Message::new(
                Role::User,
                vec![ContentBlock::Text { text: "hi".into() }],
            )];
            let body = mistral::build_mistral_request_body("", &messages, 64, None, None);

            let wire = body["messages"].as_array().expect("messages array");
            assert_eq!(wire.len(), 1, "no leading system message when system empty");
            assert_eq!(wire[0]["role"], "user");
            assert!(body.get("temperature").is_none());
            assert!(body.get("top_p").is_none());
        }

        /// A well-formed Mistral-on-Bedrock response envelope parses into
        /// text + usage, and `stop` maps to a clean finish.
        #[test]
        fn parse_response_extracts_text_and_usage() {
            let raw = json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "the answer is 42" },
                    "stop_reason": "stop"
                }],
                "usage": { "prompt_tokens": 11, "completion_tokens": 5 }
            });
            let parsed = mistral::parse_mistral_response(&raw).expect("valid envelope");
            assert_eq!(parsed.text, "the answer is 42");
            assert_eq!(parsed.stop_reason, StopReason::EndTurn);
            assert_eq!(parsed.finish_reason, FinishReason::Stop);
            assert_eq!(parsed.usage.input_tokens, 11);
            assert_eq!(parsed.usage.output_tokens, 5);
        }

        /// A `length` stop_reason maps to `FinishReason::Length` (truncation).
        #[test]
        fn parse_response_length_maps_to_finish_length() {
            let raw = json!({
                "choices": [{
                    "message": { "content": "truncated…" },
                    "stop_reason": "length"
                }]
            });
            let parsed = mistral::parse_mistral_response(&raw).expect("valid envelope");
            assert_eq!(parsed.stop_reason, StopReason::MaxTokens);
            assert_eq!(parsed.finish_reason, FinishReason::Length);
        }

        /// A malformed envelope (no `choices`) is a recoverable error, not a
        /// panic.
        #[test]
        fn parse_response_errors_on_missing_choices() {
            let raw = json!({ "usage": { "prompt_tokens": 1 } });
            let err = mistral::parse_mistral_response(&raw)
                .expect_err("missing choices must be an error");
            assert!(
                err.contains("choices"),
                "diagnostic mentions choices: {err}"
            );

            // Present-but-empty choices array is also an error.
            let empty = json!({ "choices": [] });
            assert!(mistral::parse_mistral_response(&empty).is_err());
        }

        /// The Mistral family exposes its model catalog, and every entry
        /// carries the `mistral.` family prefix.
        #[test]
        fn model_catalog_lists_mistral_ids() {
            let models = mistral::mistral_models();
            assert!(!models.is_empty(), "catalog must not be empty");
            for m in models {
                assert!(
                    mistral::is_mistral_model(m),
                    "catalog entry `{m}` must be recognised as a Mistral model"
                );
            }
            assert!(
                models.contains(&"mistral.mistral-large-2407-v1:0"),
                "catalog must list a mistral-large id"
            );
        }
    }

    // ---- P8 v0.6.3: Bedrock Cohere model family ----
    mod cohere_family {
        use super::super::cohere;
        use crate::ProviderError;
        use wcore_types::llm::{LlmEvent, LlmRequest};
        use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason};

        fn req(model: &str, system: &str, messages: Vec<Message>) -> LlmRequest {
            LlmRequest {
                model: model.to_string(),
                system: system.to_string(),
                messages,
                tools: vec![],
                max_tokens: 256,
                thinking: None,
                reasoning_effort: None,
                cache_tier: None,
                routing_hint: None,
                stop_sequences: Vec::new(),
            }
        }

        fn user(text: &str) -> Message {
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
            )
        }

        fn assistant(text: &str) -> Message {
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
            )
        }

        #[test]
        fn cohere_model_id_recognized_and_routes_to_cohere_family() {
            assert!(cohere::is_cohere_model("cohere.command-r-v1:0"));
            assert!(cohere::is_cohere_model("cohere.command-r-plus-v1:0"));
            assert!(cohere::is_cohere_model("cohere.command-text-v14"));
            // Anthropic Bedrock ids must NOT route to the Cohere family.
            assert!(!cohere::is_cohere_model(
                "anthropic.claude-3-5-sonnet-20241022-v2:0"
            ));
            assert!(!cohere::is_cohere_model("amazon.titan-text-v1"));
        }

        #[test]
        fn cohere_request_body_shape() {
            let request = req(
                "cohere.command-r-plus-v1:0",
                "You are helpful.",
                vec![
                    user("first question"),
                    assistant("first answer"),
                    user("follow up"),
                ],
            );
            let body = cohere::build_cohere_request_body(&request);

            // Final user turn becomes `message`.
            assert_eq!(body["message"], "follow up");
            // System prompt becomes `preamble`.
            assert_eq!(body["preamble"], "You are helpful.");
            // max_tokens passed through.
            assert_eq!(body["max_tokens"], 256);
            // Prior turns become chat_history with USER/CHATBOT roles.
            let history = body["chat_history"].as_array().unwrap();
            assert_eq!(history.len(), 2);
            assert_eq!(history[0]["role"], "USER");
            assert_eq!(history[0]["message"], "first question");
            assert_eq!(history[1]["role"], "CHATBOT");
            assert_eq!(history[1]["message"], "first answer");
        }

        #[test]
        fn cohere_request_body_omits_empty_history_and_preamble() {
            let request = req("cohere.command-r-v1:0", "", vec![user("hello")]);
            let body = cohere::build_cohere_request_body(&request);
            assert_eq!(body["message"], "hello");
            assert!(body.get("chat_history").is_none());
            assert!(body.get("preamble").is_none());
        }

        #[test]
        fn cohere_response_parses_text_and_usage() {
            let raw = r#"{
                "text": "Hello from Cohere",
                "finish_reason": "COMPLETE",
                "meta": { "tokens": { "input_tokens": 12, "output_tokens": 5 } }
            }"#;
            let events = cohere::parse_cohere_response(raw).unwrap();
            assert_eq!(events.len(), 2);
            match &events[0] {
                LlmEvent::TextDelta(t) => assert_eq!(t, "Hello from Cohere"),
                other => panic!("expected TextDelta, got {other:?}"),
            }
            match &events[1] {
                LlmEvent::Done {
                    stop_reason,
                    finish_reason,
                    usage,
                } => {
                    assert_eq!(*stop_reason, StopReason::EndTurn);
                    assert_eq!(*finish_reason, FinishReason::Stop);
                    assert_eq!(usage.input_tokens, 12);
                    assert_eq!(usage.output_tokens, 5);
                }
                other => panic!("expected Done, got {other:?}"),
            }
        }

        #[test]
        fn cohere_max_tokens_finish_reason_maps_to_length() {
            let raw = r#"{"text":"truncated","finish_reason":"MAX_TOKENS","meta":{"tokens":{"input_tokens":3,"output_tokens":256}}}"#;
            let events = cohere::parse_cohere_response(raw).unwrap();
            match events.last().unwrap() {
                LlmEvent::Done { finish_reason, .. } => {
                    assert_eq!(*finish_reason, FinishReason::Length);
                }
                other => panic!("expected Done, got {other:?}"),
            }
        }

        #[test]
        fn cohere_error_envelope_surfaces_provider_error() {
            // Bedrock returns an error envelope with `message` and no `text`.
            let raw = r#"{"message":"Malformed input request"}"#;
            let result = cohere::parse_cohere_response(raw);
            assert!(result.is_err());
            match result.unwrap_err() {
                ProviderError::Api { message, .. } => {
                    assert!(message.contains("Malformed input request"));
                }
                other => panic!("expected ProviderError::Api, got {other:?}"),
            }
        }

        #[test]
        fn cohere_models_listed_in_catalog() {
            let catalog = cohere::cohere_models();
            assert!(catalog.contains(&"cohere.command-r-v1:0"));
            assert!(catalog.contains(&"cohere.command-r-plus-v1:0"));
            // Every catalog id must be recognized by the family router.
            for model in &catalog {
                assert!(
                    cohere::is_cohere_model(model),
                    "catalog id {model} not recognized by is_cohere_model"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // v0.6.3 — Bedrock model-family dispatch seam (P7 + P8 follow-up)
    //
    // These tests pin the dispatch wiring: `BedrockProvider::build_request_body`
    // and the `BedrockFamily` classifier route a model id to the correct
    // family schema, and `decode_buffered_response` turns a buffered Bedrock
    // `invoke` body into the right `LlmEvent` sequence.
    // -----------------------------------------------------------------------
    mod family_dispatch {
        use super::super::{
            AwsCredentials, BedrockFamily, BedrockProvider, decode_buffered_response,
        };
        use wcore_config::compat::ProviderCompat;
        use wcore_config::debug::DebugConfig;
        use wcore_types::llm::{LlmEvent, LlmRequest};
        use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason};

        /// A `BedrockProvider` with throwaway explicit credentials — enough to
        /// exercise the pure request-building/dispatch logic without AWS.
        fn provider() -> BedrockProvider {
            BedrockProvider::new(
                "us-east-1",
                AwsCredentials::Explicit {
                    access_key_id: "AKIA_TEST".into(),
                    secret_access_key: "secret_test".into(),
                    session_token: None,
                },
                false,
                ProviderCompat::default(),
                DebugConfig::default(),
            )
        }

        fn req(model: &str) -> LlmRequest {
            LlmRequest {
                model: model.to_string(),
                system: "you are helpful".to_string(),
                messages: vec![Message::new(
                    Role::User,
                    vec![ContentBlock::Text {
                        text: "hello".into(),
                    }],
                )],
                max_tokens: 128,
                ..LlmRequest::default()
            }
        }

        /// The family classifier routes each id to the right `BedrockFamily`.
        #[test]
        fn classify_routes_each_family() {
            assert_eq!(
                BedrockFamily::classify("anthropic.claude-3-5-sonnet-20241022-v2:0"),
                BedrockFamily::Anthropic
            );
            assert_eq!(
                BedrockFamily::classify("mistral.mistral-large-2407-v1:0"),
                BedrockFamily::Mistral
            );
            assert_eq!(
                BedrockFamily::classify("us.mistral.mistral-large-2407-v1:0"),
                BedrockFamily::Mistral
            );
            assert_eq!(
                BedrockFamily::classify("cohere.command-r-plus-v1:0"),
                BedrockFamily::Cohere
            );
        }

        /// A Mistral model id routed through `BedrockProvider` builds the
        /// Mistral request body (OpenAI-style `messages`, no `anthropic_version`).
        #[test]
        fn mistral_id_builds_mistral_body() {
            let body = provider().build_request_body(&req("mistral.mistral-large-2407-v1:0"));
            assert!(
                body.get("anthropic_version").is_none(),
                "Mistral body must not carry anthropic_version"
            );
            let wire = body["messages"].as_array().expect("messages array");
            // Leading system message folded in + the user turn.
            assert_eq!(wire.len(), 2);
            assert_eq!(wire[0]["role"], "system");
            assert_eq!(wire[1]["role"], "user");
            assert_eq!(wire[1]["content"], "hello");
            assert_eq!(body["max_tokens"], 128);
        }

        /// A Cohere model id routed through `BedrockProvider` builds the
        /// Cohere request body (`message` + `preamble`, no `anthropic_version`).
        #[test]
        fn cohere_id_builds_cohere_body() {
            let body = provider().build_request_body(&req("cohere.command-r-v1:0"));
            assert!(
                body.get("anthropic_version").is_none(),
                "Cohere body must not carry anthropic_version"
            );
            assert_eq!(body["message"], "hello");
            assert_eq!(body["preamble"], "you are helpful");
            assert_eq!(body["max_tokens"], 128);
        }

        /// An Anthropic model id still builds the Anthropic-on-Bedrock body —
        /// no regression from adding the dispatch seam.
        #[test]
        fn anthropic_id_still_builds_anthropic_body() {
            let body =
                provider().build_request_body(&req("anthropic.claude-3-5-sonnet-20241022-v2:0"));
            assert_eq!(
                body["anthropic_version"], "bedrock-2023-05-31",
                "Anthropic body must carry anthropic_version"
            );
            assert!(body.get("messages").is_some());
            // Cohere-only / Mistral-only keys must be absent.
            assert!(body.get("preamble").is_none());
        }

        /// A buffered Mistral `invoke` response decodes into a `TextDelta`
        /// followed by a terminal `Done` carrying the parsed usage.
        #[test]
        fn decode_buffered_mistral_response() {
            let raw = r#"{
                "choices": [{
                    "message": { "role": "assistant", "content": "the answer is 42" },
                    "stop_reason": "stop"
                }],
                "usage": { "prompt_tokens": 11, "completion_tokens": 5 }
            }"#;
            let events =
                decode_buffered_response(BedrockFamily::Mistral, raw).expect("valid envelope");
            assert_eq!(events.len(), 2);
            match &events[0] {
                LlmEvent::TextDelta(t) => assert_eq!(t, "the answer is 42"),
                other => panic!("expected TextDelta, got {other:?}"),
            }
            match &events[1] {
                LlmEvent::Done {
                    stop_reason,
                    finish_reason,
                    usage,
                } => {
                    assert_eq!(*stop_reason, StopReason::EndTurn);
                    assert_eq!(*finish_reason, FinishReason::Stop);
                    assert_eq!(usage.input_tokens, 11);
                    assert_eq!(usage.output_tokens, 5);
                }
                other => panic!("expected Done, got {other:?}"),
            }
        }

        /// A buffered Cohere `invoke` response decodes into a `TextDelta` +
        /// terminal `Done` through the family dispatch.
        #[test]
        fn decode_buffered_cohere_response() {
            let raw = r#"{
                "text": "Hello from Cohere",
                "finish_reason": "COMPLETE",
                "meta": { "tokens": { "input_tokens": 12, "output_tokens": 5 } }
            }"#;
            let events =
                decode_buffered_response(BedrockFamily::Cohere, raw).expect("valid envelope");
            assert_eq!(events.len(), 2);
            match &events[0] {
                LlmEvent::TextDelta(t) => assert_eq!(t, "Hello from Cohere"),
                other => panic!("expected TextDelta, got {other:?}"),
            }
            assert!(matches!(events[1], LlmEvent::Done { .. }));
        }

        /// Decoding through the Anthropic family is rejected — that family
        /// uses the native streaming path, never the buffered decoder.
        #[test]
        fn decode_buffered_rejects_anthropic_family() {
            let err = decode_buffered_response(BedrockFamily::Anthropic, "{}")
                .expect_err("Anthropic must not use the buffered decoder");
            assert!(format!("{err:?}").contains("Anthropic"));
        }
    }
}
