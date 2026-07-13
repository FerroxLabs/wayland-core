//! FIFO-scripted OpenAI-compatible loopback fixture.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::workspace_evidence;

const FIXTURE_PROTOCOL_VERSION: u32 = 1;
const MAX_SCRIPT_STEPS: usize = 256;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_STALL_MS: u64 = 60_000;
const MAX_RETRY_AFTER_MS: u64 = 60_000;
const MAX_TOOL_IDENTIFIER_BYTES: usize = 256;
const MAX_TOOL_ARGUMENT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiFixtureScript {
    protocol_version: u32,
    steps: Vec<OpenAiStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenAiStep {
    Text {
        text: String,
    },
    HttpError {
        status: u16,
    },
    RateLimited {
        retry_after_ms: u64,
    },
    Truncated {
        text: String,
    },
    DuplicateText {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    TextThenStall {
        text: String,
        delay_ms: u64,
    },
    StallBeforeHeaders {
        delay_ms: u64,
    },
}

impl OpenAiStep {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn http_error(status: u16) -> Self {
        Self::HttpError { status }
    }

    pub fn rate_limited(retry_after_ms: u64) -> Self {
        Self::RateLimited { retry_after_ms }
    }

    pub fn truncated(text: impl Into<String>) -> Self {
        Self::Truncated { text: text.into() }
    }

    pub fn duplicate_text(text: impl Into<String>) -> Self {
        Self::DuplicateText { text: text.into() }
    }

    pub fn tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self::ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }

    pub fn text_then_stall(text: impl Into<String>, delay_ms: u64) -> Self {
        Self::TextThenStall {
            text: text.into(),
            delay_ms,
        }
    }

    pub fn stall_before_headers(delay_ms: u64) -> Self {
        Self::StallBeforeHeaders { delay_ms }
    }

    fn failure_code(&self) -> Option<String> {
        match self {
            Self::HttpError { status } => Some(format!("http_{status}")),
            Self::RateLimited { .. } => Some("rate_limited".to_string()),
            Self::Truncated { .. } => Some("truncated_stream".to_string()),
            Self::StallBeforeHeaders { .. } => Some("response_timeout".to_string()),
            Self::Text { .. }
            | Self::DuplicateText { .. }
            | Self::ToolCall { .. }
            | Self::TextThenStall { .. } => None,
        }
    }
}

impl OpenAiFixtureScript {
    pub fn new(steps: impl IntoIterator<Item = OpenAiStep>) -> Self {
        Self {
            protocol_version: FIXTURE_PROTOCOL_VERSION,
            steps: steps.into_iter().collect(),
        }
    }

    pub async fn start(&self) -> Result<RunningOpenAiFixture, OpenAiFixtureError> {
        self.start_with_workspace(None).await
    }

    /// Start a fixture whose content and observation identities normalize the
    /// caller-owned temporary workspace while retaining every other value.
    pub async fn start_for_workspace(
        &self,
        workspace: &Path,
    ) -> Result<RunningOpenAiFixture, OpenAiFixtureError> {
        self.start_with_workspace(Some(workspace)).await
    }

    async fn start_with_workspace(
        &self,
        workspace: Option<&Path>,
    ) -> Result<RunningOpenAiFixture, OpenAiFixtureError> {
        self.validate()?;
        let canonical = serde_json::to_vec(self)
            .map_err(|error| OpenAiFixtureError::InvalidScript(error.to_string()))?;
        let fixture_sha256 = match workspace {
            Some(workspace) => {
                workspace_evidence::semantic_sha256(b"openai-fixture-script", &canonical, workspace)
                    .map_err(|error| OpenAiFixtureError::InvalidScript(error.to_string()))?
            }
            None => format!("{:x}", Sha256::digest(canonical)),
        };
        let state = Arc::new(Mutex::new(FixtureState {
            steps: self.steps.clone(),
            workspace_root: workspace.map(Path::to_path_buf),
            cursor: 0,
            requests: Vec::new(),
            request_instants: Vec::new(),
            violations: Vec::new(),
        }));
        let app = Router::new()
            .route("/v1/chat/completions", post(handle_chat_completion))
            .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(OpenAiFixtureError::Bind)?;
        let address = listener.local_addr().map_err(OpenAiFixtureError::Bind)?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
        });

        Ok(RunningOpenAiFixture {
            base_url: format!("http://{address}"),
            fixture_sha256,
            state,
            shutdown_tx: Some(shutdown_tx),
            server,
        })
    }

    fn validate(&self) -> Result<(), OpenAiFixtureError> {
        if self.protocol_version != FIXTURE_PROTOCOL_VERSION {
            return Err(OpenAiFixtureError::InvalidScript(
                "unsupported fixture protocol version".to_string(),
            ));
        }
        if self.steps.is_empty() || self.steps.len() > MAX_SCRIPT_STEPS {
            return Err(OpenAiFixtureError::InvalidScript(format!(
                "script must contain 1..={MAX_SCRIPT_STEPS} steps"
            )));
        }
        for step in &self.steps {
            match step {
                OpenAiStep::Text { text }
                | OpenAiStep::Truncated { text }
                | OpenAiStep::DuplicateText { text } => {
                    if text.len() > MAX_TEXT_BYTES {
                        return Err(OpenAiFixtureError::InvalidScript(format!(
                            "response text exceeds {MAX_TEXT_BYTES} bytes"
                        )));
                    }
                }
                OpenAiStep::HttpError { status } => {
                    if !(400..=599).contains(status) {
                        return Err(OpenAiFixtureError::InvalidScript(
                            "HTTP error status must be between 400 and 599".to_string(),
                        ));
                    }
                }
                OpenAiStep::RateLimited { retry_after_ms } => {
                    if *retry_after_ms == 0 || *retry_after_ms > MAX_RETRY_AFTER_MS {
                        return Err(OpenAiFixtureError::InvalidScript(format!(
                            "retry-after must be between 1 and {MAX_RETRY_AFTER_MS} milliseconds"
                        )));
                    }
                }
                OpenAiStep::ToolCall {
                    id,
                    name,
                    arguments,
                } => {
                    if id.is_empty()
                        || name.is_empty()
                        || id.len() > MAX_TOOL_IDENTIFIER_BYTES
                        || name.len() > MAX_TOOL_IDENTIFIER_BYTES
                    {
                        return Err(OpenAiFixtureError::InvalidScript(
                            "tool id and name must contain 1..=256 bytes".to_string(),
                        ));
                    }
                    let arguments = serde_json::to_vec(arguments)
                        .map_err(|error| OpenAiFixtureError::InvalidScript(error.to_string()))?;
                    if arguments.len() > MAX_TOOL_ARGUMENT_BYTES {
                        return Err(OpenAiFixtureError::InvalidScript(format!(
                            "tool arguments exceed {MAX_TOOL_ARGUMENT_BYTES} bytes"
                        )));
                    }
                }
                OpenAiStep::TextThenStall { text, delay_ms } => {
                    if text.len() > MAX_TEXT_BYTES {
                        return Err(OpenAiFixtureError::InvalidScript(format!(
                            "response text exceeds {MAX_TEXT_BYTES} bytes"
                        )));
                    }
                    if *delay_ms == 0 || *delay_ms > MAX_STALL_MS {
                        return Err(OpenAiFixtureError::InvalidScript(format!(
                            "stall must be between 1 and {MAX_STALL_MS} milliseconds"
                        )));
                    }
                }
                OpenAiStep::StallBeforeHeaders { delay_ms } => {
                    if *delay_ms == 0 || *delay_ms > MAX_STALL_MS {
                        return Err(OpenAiFixtureError::InvalidScript(format!(
                            "stall must be between 1 and {MAX_STALL_MS} milliseconds"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FixtureRequestRecord {
    pub sequence: u64,
    pub method: String,
    pub path: String,
    pub body_sha256: String,
    pub semantic_body_sha256: String,
    /// Per-leaf semantic hashes for diagnosing a repeatability mismatch
    /// without retaining or printing request content.
    pub semantic_leaf_sha256: BTreeMap<String, String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenAiFixtureObservation {
    pub requests: Vec<FixtureRequestRecord>,
    pub consumed_steps: usize,
    pub expected_steps: usize,
    pub violations: Vec<String>,
    injected_faults: Vec<String>,
    inter_request_delays_ms: Vec<u64>,
}

impl OpenAiFixtureObservation {
    pub fn complete(&self) -> bool {
        self.consumed_steps == self.expected_steps && self.violations.is_empty()
    }

    pub fn attempts(&self) -> u64 {
        self.requests.len() as u64
    }

    /// Fault modes supplied by the fixture script. These describe test input,
    /// not provider failures or retries observed from Core.
    pub fn injected_faults(&self) -> &[String] {
        &self.injected_faults
    }

    /// Real elapsed time between request arrivals, used only to prove retry
    /// hint behavior. It is intentionally excluded from fixture identity.
    pub fn inter_request_delays_ms(&self) -> &[u64] {
        &self.inter_request_delays_ms
    }

    /// Hash request semantics and fixture outcomes while excluding ports,
    /// elapsed retry timing, and the caller-owned temporary workspace.
    pub fn behavior_sha256(&self) -> Result<String, serde_json::Error> {
        let projection = OpenAiObservationBehavior {
            protocol_version: FIXTURE_PROTOCOL_VERSION,
            requests: self
                .requests
                .iter()
                .map(|request| FixtureRequestBehavior {
                    sequence: request.sequence,
                    method: &request.method,
                    path: &request.path,
                    semantic_body_sha256: &request.semantic_body_sha256,
                    model: request.model.as_deref(),
                })
                .collect(),
            consumed_steps: self.consumed_steps,
            expected_steps: self.expected_steps,
            violations: &self.violations,
            injected_faults: &self.injected_faults,
        };
        serde_json::to_vec(&projection).map(|bytes| format!("{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Serialize)]
struct OpenAiObservationBehavior<'a> {
    protocol_version: u32,
    requests: Vec<FixtureRequestBehavior<'a>>,
    consumed_steps: usize,
    expected_steps: usize,
    violations: &'a [String],
    injected_faults: &'a [String],
}

#[derive(Serialize)]
struct FixtureRequestBehavior<'a> {
    sequence: u64,
    method: &'a str,
    path: &'a str,
    semantic_body_sha256: &'a str,
    model: Option<&'a str>,
}

pub struct RunningOpenAiFixture {
    base_url: String,
    fixture_sha256: String,
    state: Arc<Mutex<FixtureState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server: JoinHandle<std::io::Result<()>>,
}

impl RunningOpenAiFixture {
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn fixture_sha256(&self) -> &str {
        &self.fixture_sha256
    }

    pub fn observation(&self) -> OpenAiFixtureObservation {
        let state = self.state.lock().expect("OpenAI fixture state lock");
        let injected_faults = state
            .steps
            .iter()
            .take(state.cursor)
            .filter_map(OpenAiStep::failure_code)
            .collect();
        let inter_request_delays_ms = state
            .request_instants
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]).as_millis() as u64)
            .collect();
        OpenAiFixtureObservation {
            requests: state.requests.clone(),
            consumed_steps: state.cursor,
            expected_steps: state.steps.len(),
            violations: state.violations.clone(),
            injected_faults,
            inter_request_delays_ms,
        }
    }

    pub async fn shutdown(mut self) -> Result<OpenAiFixtureObservation, OpenAiFixtureError> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.server.abort();
        match (&mut self.server).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(OpenAiFixtureError::Serve(error)),
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(OpenAiFixtureError::Join(error.to_string())),
        }
        Ok(self.observation())
    }
}

impl Drop for RunningOpenAiFixture {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.server.abort();
    }
}

#[derive(Debug)]
struct FixtureState {
    steps: Vec<OpenAiStep>,
    workspace_root: Option<PathBuf>,
    cursor: usize,
    requests: Vec<FixtureRequestRecord>,
    request_instants: Vec<tokio::time::Instant>,
    violations: Vec<String>,
}

async fn handle_chat_completion(
    State(state): State<Arc<Mutex<FixtureState>>>,
    body: Bytes,
) -> Response {
    let body_sha256 = format!("{:x}", Sha256::digest(&body));
    let workspace_root = state
        .lock()
        .expect("OpenAI fixture state lock")
        .workspace_root
        .clone();
    let semantic_body_sha256 = match workspace_root.as_deref() {
        Some(workspace) => {
            workspace_evidence::semantic_sha256(b"openai-request-body", &body, workspace)
        }
        None => Ok(body_sha256.clone()),
    };
    let semantic_leaf_sha256 = semantic_leaf_hashes(&body, workspace_root.as_deref());
    let model = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });
    let step = {
        let mut state = state.lock().expect("OpenAI fixture state lock");
        let semantic_body_sha256 = semantic_body_sha256.unwrap_or_else(|error| {
            state
                .violations
                .push(format!("workspace_evidence_error:{error}"));
            body_sha256.clone()
        });
        let semantic_leaf_sha256 = semantic_leaf_sha256.unwrap_or_else(|error| {
            state
                .violations
                .push(format!("semantic_leaf_evidence_error:{error}"));
            BTreeMap::new()
        });
        let sequence = state.requests.len() as u64 + 1;
        state.requests.push(FixtureRequestRecord {
            sequence,
            method: "POST".to_string(),
            path: "/v1/chat/completions".to_string(),
            body_sha256,
            semantic_body_sha256,
            semantic_leaf_sha256,
            model,
        });
        state.request_instants.push(tokio::time::Instant::now());
        if state.cursor >= state.steps.len() {
            state.violations.push("unexpected_request".to_string());
            None
        } else {
            let step = state.steps[state.cursor].clone();
            state.cursor += 1;
            Some(step)
        }
    };

    let Some(step) = step else {
        return json_response(
            StatusCode::CONFLICT,
            json!({"error":{"code":"unexpected_request"}}).to_string(),
        );
    };
    match step {
        OpenAiStep::Text { text } => sse_response(complete_text_sse(&text, false)),
        OpenAiStep::HttpError { status } => {
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            json_response(
                status,
                json!({"error":{"code":"fixture_http_error","status":status.as_u16()}}).to_string(),
            )
        }
        OpenAiStep::RateLimited { retry_after_ms } => json_response(
            StatusCode::TOO_MANY_REQUESTS,
            json!({
                "retry_after_ms": retry_after_ms,
                "error": {
                    "code": "fixture_rate_limited",
                }
            })
            .to_string(),
        ),
        OpenAiStep::Truncated { text } => sse_response(text_delta_frame(&text)),
        OpenAiStep::DuplicateText { text } => sse_response(complete_text_sse(&text, true)),
        OpenAiStep::ToolCall {
            id,
            name,
            arguments,
        } => sse_response(tool_call_sse(&id, &name, &arguments)),
        OpenAiStep::TextThenStall { text, delay_ms } => {
            stalling_sse_response(text_delta_frame(&text), delay_ms)
        }
        OpenAiStep::StallBeforeHeaders { delay_ms } => {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            sse_response(complete_text_sse("released", false))
        }
    }
}

fn semantic_leaf_hashes(
    body: &[u8],
    workspace: Option<&Path>,
) -> Result<BTreeMap<String, String>, String> {
    let Some(workspace) = workspace else {
        return Ok(BTreeMap::new());
    };
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|error| error.to_string())?;
    let mut hashes = BTreeMap::new();
    collect_semantic_leaf_hashes("", &value, workspace, &mut hashes)?;
    Ok(hashes)
}

fn collect_semantic_leaf_hashes(
    pointer: &str,
    value: &serde_json::Value,
    workspace: &Path,
    hashes: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    match value {
        serde_json::Value::Array(values) if !values.is_empty() => {
            for (index, value) in values.iter().enumerate() {
                collect_semantic_leaf_hashes(
                    &format!("{pointer}/{index}"),
                    value,
                    workspace,
                    hashes,
                )?;
            }
        }
        serde_json::Value::Object(values) if !values.is_empty() => {
            for (key, value) in values {
                let key = key.replace('~', "~0").replace('/', "~1");
                collect_semantic_leaf_hashes(
                    &format!("{pointer}/{key}"),
                    value,
                    workspace,
                    hashes,
                )?;
            }
        }
        serde_json::Value::String(text) if text.contains('\n') => {
            for (line, text) in text.split('\n').enumerate() {
                insert_semantic_leaf_hash(
                    &format!("{pointer}#line={line}"),
                    &serde_json::Value::String(text.to_string()),
                    workspace,
                    hashes,
                )?;
            }
        }
        _ => {
            insert_semantic_leaf_hash(pointer, value, workspace, hashes)?;
        }
    }
    Ok(())
}

fn insert_semantic_leaf_hash(
    pointer: &str,
    value: &serde_json::Value,
    workspace: &Path,
    hashes: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let encoded = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let digest = workspace_evidence::semantic_sha256(b"openai-request-leaf", &encoded, workspace)
        .map_err(|error| error.to_string())?;
    hashes.insert(pointer.to_string(), digest);
    Ok(())
}

fn text_delta_frame(text: &str) -> String {
    let frame = json!({
        "id": "fixture-completion",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "fixture-chat-v1",
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant", "content": text},
            "finish_reason": null
        }]
    });
    format!("data: {frame}\n\n")
}

fn complete_text_sse(text: &str, duplicate: bool) -> String {
    let delta = text_delta_frame(text);
    let finish = json!({
        "id": "fixture-completion",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "fixture-chat-v1",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    let usage = json!({
        "id": "fixture-completion",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "fixture-chat-v1",
        "choices": [],
        "usage": {
            "prompt_tokens": 7,
            "completion_tokens": 3,
            "total_tokens": 10,
            "prompt_tokens_details": {"cached_tokens": 0}
        }
    });
    let repeated = if duplicate { delta.as_str() } else { "" };
    format!("{delta}{repeated}data: {finish}\n\ndata: {usage}\n\ndata: [DONE]\n\n")
}

fn tool_call_sse(id: &str, name: &str, arguments: &serde_json::Value) -> String {
    let call = json!({
        "id": "fixture-completion",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "fixture-chat-v1",
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments.to_string()
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish = json!({
        "id": "fixture-completion",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "fixture-chat-v1",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}],
        "usage": {
            "prompt_tokens": 7,
            "completion_tokens": 3,
            "total_tokens": 10,
            "prompt_tokens_details": {"cached_tokens": 0}
        }
    });
    format!("data: {call}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

fn sse_response(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    response
}

fn stalling_sse_response(prefix: String, delay_ms: u64) -> Response {
    let prefix = futures::stream::once(async move { Ok::<Bytes, Infallible>(Bytes::from(prefix)) });
    let stall = futures::stream::once(async move {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        Ok::<Bytes, Infallible>(Bytes::new())
    });
    let mut response = Response::new(Body::from_stream(prefix.chain(stall)));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    response
}

fn json_response(status: StatusCode, body: String) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

#[derive(Debug, Error)]
pub enum OpenAiFixtureError {
    #[error("invalid OpenAI fixture script: {0}")]
    InvalidScript(String),
    #[error("could not bind OpenAI fixture: {0}")]
    Bind(std::io::Error),
    #[error("OpenAI fixture server failed: {0}")]
    Serve(std::io::Error),
    #[error("OpenAI fixture task failed: {0}")]
    Join(String),
}
