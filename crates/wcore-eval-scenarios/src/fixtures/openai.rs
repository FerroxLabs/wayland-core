//! FIFO-scripted OpenAI-compatible loopback fixture.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const FIXTURE_PROTOCOL_VERSION: u32 = 1;
const MAX_SCRIPT_STEPS: usize = 256;
const MAX_TEXT_BYTES: usize = 64 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_STALL_MS: u64 = 60_000;
const MAX_RETRY_AFTER_MS: u64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiFixtureScript {
    protocol_version: u32,
    steps: Vec<OpenAiStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenAiStep {
    Text { text: String },
    HttpError { status: u16 },
    RateLimited { retry_after_ms: u64 },
    Truncated { text: String },
    DuplicateText { text: String },
    StallBeforeHeaders { delay_ms: u64 },
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

    pub fn stall_before_headers(delay_ms: u64) -> Self {
        Self::StallBeforeHeaders { delay_ms }
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
        self.validate()?;
        let canonical = serde_json::to_vec(self)
            .map_err(|error| OpenAiFixtureError::InvalidScript(error.to_string()))?;
        let fixture_sha256 = format!("{:x}", Sha256::digest(canonical));
        let state = Arc::new(Mutex::new(FixtureState {
            steps: self.steps.clone(),
            cursor: 0,
            requests: Vec::new(),
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
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenAiFixtureObservation {
    pub requests: Vec<FixtureRequestRecord>,
    pub consumed_steps: usize,
    pub expected_steps: usize,
    pub violations: Vec<String>,
}

impl OpenAiFixtureObservation {
    pub fn complete(&self) -> bool {
        self.consumed_steps == self.expected_steps && self.violations.is_empty()
    }
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
        OpenAiFixtureObservation {
            requests: state.requests.clone(),
            consumed_steps: state.cursor,
            expected_steps: state.steps.len(),
            violations: state.violations.clone(),
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
    cursor: usize,
    requests: Vec<FixtureRequestRecord>,
    violations: Vec<String>,
}

async fn handle_chat_completion(
    State(state): State<Arc<Mutex<FixtureState>>>,
    body: Bytes,
) -> Response {
    let body_sha256 = format!("{:x}", Sha256::digest(&body));
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
        let sequence = state.requests.len() as u64 + 1;
        state.requests.push(FixtureRequestRecord {
            sequence,
            method: "POST".to_string(),
            path: "/v1/chat/completions".to_string(),
            body_sha256,
            model,
        });
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
                "error": {
                    "code": "fixture_rate_limited",
                    "retry_after_ms": retry_after_ms
                }
            })
            .to_string(),
        ),
        OpenAiStep::Truncated { text } => sse_response(text_delta_frame(&text)),
        OpenAiStep::DuplicateText { text } => sse_response(complete_text_sse(&text, true)),
        OpenAiStep::StallBeforeHeaders { delay_ms } => {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            sse_response(complete_text_sse("released", false))
        }
    }
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

fn sse_response(body: String) -> Response {
    let mut response = Response::new(Body::from(body));
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
