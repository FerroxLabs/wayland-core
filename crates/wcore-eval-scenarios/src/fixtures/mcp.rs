//! Loopback-only MCP HTTP fixtures for the real client transports.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use futures::stream;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
use tokio::task::JoinHandle;

const FIXTURE_PROTOCOL_VERSION: u32 = 1;
const EXPECTED_METHODS: [&str; 4] = [
    "initialize",
    "notifications/initialized",
    "tools/list",
    "tools/call",
];

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpHttpMode {
    DirectJson,
    SseResponse,
    LegacySse,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpHttpRequestRecord {
    pub sequence: u64,
    pub method: String,
    pub path: String,
    pub rpc_method: Option<String>,
    pub body_sha256: String,
}

#[derive(Debug, Clone)]
pub struct McpHttpObservation {
    pub requests: Vec<McpHttpRequestRecord>,
    pub violations: Vec<String>,
}

impl McpHttpObservation {
    pub fn methods(&self) -> Vec<&str> {
        self.requests
            .iter()
            .filter_map(|request| request.rpc_method.as_deref())
            .collect()
    }

    pub fn complete(&self) -> bool {
        self.methods() == EXPECTED_METHODS && self.violations.is_empty()
    }
}

pub struct McpHttpFixture {
    url: String,
    fixture_sha256: String,
    state: Arc<FixtureState>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server: JoinHandle<std::io::Result<()>>,
}

impl McpHttpFixture {
    pub async fn start(mode: McpHttpMode) -> Result<Self, McpHttpFixtureError> {
        let state = Arc::new(FixtureState {
            mode,
            requests: Mutex::new(Vec::new()),
            violations: Mutex::new(Vec::new()),
            legacy_sender: AsyncMutex::new(None),
        });
        let app = Router::new()
            .route("/mcp", post(streamable_post))
            .route("/sse", get(legacy_get))
            .route("/messages", post(legacy_post))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(McpHttpFixtureError::Bind)?;
        let address = listener.local_addr().map_err(McpHttpFixtureError::Bind)?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
        });
        let path = match mode {
            McpHttpMode::LegacySse => "sse",
            McpHttpMode::DirectJson | McpHttpMode::SseResponse => "mcp",
        };
        let fixture_sha256 = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(FIXTURE_PROTOCOL_VERSION, mode))
                    .expect("MCP fixture identity is serializable")
            )
        );

        Ok(Self {
            url: format!("http://{address}/{path}"),
            fixture_sha256,
            state,
            shutdown_tx: Some(shutdown_tx),
            server,
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn fixture_sha256(&self) -> &str {
        &self.fixture_sha256
    }

    pub fn observation(&self) -> McpHttpObservation {
        McpHttpObservation {
            requests: self
                .state
                .requests
                .lock()
                .expect("MCP fixture request lock")
                .clone(),
            violations: self
                .state
                .violations
                .lock()
                .expect("MCP fixture violation lock")
                .clone(),
        }
    }

    pub async fn shutdown(mut self) -> Result<McpHttpObservation, McpHttpFixtureError> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.server.abort();
        match (&mut self.server).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(McpHttpFixtureError::Serve(error)),
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(McpHttpFixtureError::Join(error.to_string())),
        }
        Ok(self.observation())
    }
}

impl Drop for McpHttpFixture {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.server.abort();
    }
}

struct FixtureState {
    mode: McpHttpMode,
    requests: Mutex<Vec<McpHttpRequestRecord>>,
    violations: Mutex<Vec<String>>,
    legacy_sender: AsyncMutex<Option<mpsc::Sender<Result<Bytes, Infallible>>>>,
}

async fn streamable_post(State(state): State<Arc<FixtureState>>, body: Bytes) -> Response {
    state.record("POST", "/mcp", &body);
    if matches!(state.mode, McpHttpMode::LegacySse) {
        state.violation("streamable_post_in_legacy_mode");
        return status_response(StatusCode::CONFLICT);
    }
    let Some(response) = rpc_response(&body) else {
        return status_response(StatusCode::ACCEPTED);
    };
    match state.mode {
        McpHttpMode::DirectJson => typed_response("application/json", response.to_string()),
        McpHttpMode::SseResponse => typed_response(
            "text/event-stream",
            format!("event: message\ndata: {response}\n\n"),
        ),
        McpHttpMode::LegacySse => unreachable!("legacy mode returned above"),
    }
}

async fn legacy_get(State(state): State<Arc<FixtureState>>) -> Response {
    if !matches!(state.mode, McpHttpMode::LegacySse) {
        state.violation("legacy_get_in_streamable_mode");
        return status_response(StatusCode::CONFLICT);
    }
    state.record("GET", "/sse", &[]);
    let (tx, rx) = mpsc::channel(16);
    *state.legacy_sender.lock().await = Some(tx.clone());
    let endpoint = Bytes::from_static(b"event: endpoint\ndata: /messages\n\n");
    if tx.send(Ok(endpoint)).await.is_err() {
        state.violation("legacy_endpoint_delivery_failed");
    }
    let body_stream = stream::unfold(rx, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(body_stream))
        .expect("valid legacy SSE response")
}

async fn legacy_post(State(state): State<Arc<FixtureState>>, body: Bytes) -> Response {
    state.record("POST", "/messages", &body);
    if !matches!(state.mode, McpHttpMode::LegacySse) {
        state.violation("legacy_post_in_streamable_mode");
        return status_response(StatusCode::CONFLICT);
    }
    if let Some(response) = rpc_response(&body) {
        let frame = Bytes::from(format!("event: message\ndata: {response}\n\n"));
        let sender = state.legacy_sender.lock().await.clone();
        match sender {
            Some(sender) if sender.send(Ok(frame)).await.is_ok() => {}
            Some(_) => state.violation("legacy_response_delivery_failed"),
            None => state.violation("legacy_post_before_sse_get"),
        }
    }
    status_response(StatusCode::ACCEPTED)
}

impl FixtureState {
    fn record(&self, method: &str, path: &str, body: &[u8]) {
        let rpc_method = serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|value| value.get("method")?.as_str().map(str::to_string));
        let mut requests = self.requests.lock().expect("MCP fixture request lock");
        let sequence = requests.len() as u64 + 1;
        requests.push(McpHttpRequestRecord {
            sequence,
            method: method.to_string(),
            path: path.to_string(),
            rpc_method,
            body_sha256: format!("{:x}", Sha256::digest(body)),
        });
    }

    fn violation(&self, code: &str) {
        self.violations
            .lock()
            .expect("MCP fixture violation lock")
            .push(code.to_string());
    }
}

fn rpc_response(body: &[u8]) -> Option<Value> {
    let request: Value = serde_json::from_slice(body).ok()?;
    let id = request.get("id")?.clone();
    let method = request.get("method")?.as_str()?;
    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "wcore-eval-fixture", "version": "1"}
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "fixture_echo",
                "description": "Return the supplied text",
                "inputSchema": {
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"]
                }
            }]
        }),
        "tools/call" => {
            let text = request
                .pointer("/params/arguments/text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            json!({"content": [{"type": "text", "text": text}], "isError": false})
        }
        _ => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "method not found"}
            }));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn typed_response(content_type: &'static str, body: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .expect("valid fixture response")
}

fn status_response(status: StatusCode) -> Response {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .expect("valid empty fixture response")
}

#[derive(Debug, Error)]
pub enum McpHttpFixtureError {
    #[error("could not bind MCP fixture: {0}")]
    Bind(std::io::Error),
    #[error("MCP fixture server failed: {0}")]
    Serve(std::io::Error),
    #[error("MCP fixture task failed: {0}")]
    Join(String),
}
