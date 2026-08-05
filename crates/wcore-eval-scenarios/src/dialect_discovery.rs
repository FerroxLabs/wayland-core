//! Dialect discovery: capture a harness's OWN declared tool schema off the wire.
//!
//! # Why this is a separate instrument
//!
//! The shared loopback meter `crate::fixtures::openai` records `body_sha256`,
//! `semantic_body_sha256` and per-leaf hashes — **it does not retain bodies** (SR-30-1), so the
//! declared `tools` array cannot be recovered from it. It is also a hard scope fence for every
//! Phase 30 lane, gate-checked untouched, because editing the meter mid-phase changes what every
//! earlier measurement meant.
//!
//! So discovery gets its own server. The frozen meter stays byte-identical and every 30-02 number
//! keeps meaning exactly what it meant.
//!
//! # What it retains, and what it deliberately does not
//!
//! It retains **only the `tools` declaration**: name, description, JSON Schema. It never retains
//! `messages`, never retains a system prompt, never retains any argument value. That is not a
//! nicety — a per-trial canary lives in the workspace of the very runs this instrument observes,
//! and an instrument that hoovered up message bodies would become a new exfiltration surface in the
//! middle of a benchmark whose security dimension is about exfiltration.
//! [`DiscoveryCapture::corpus`] is asserted canary-free by this module's tests.
//!
//! # This is an UNSCORED pass
//!
//! Discovery produces the compiler's input. It scores nothing, and no measurement taken here
//! enters any comparative.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::dialect::{CORPUS_VERSION, DeclaredToolV1, ToolSchemaCorpusV1};

const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
/// Guard rails on what a harness may declare. A harness that exceeds these is recorded as such
/// rather than silently truncated.
const MAX_DECLARED_TOOLS: usize = 512;
const MAX_DESCRIPTION_BYTES: usize = 8 * 1024;

/// Identity lives HERE, never inside [`ToolSchemaCorpusV1`]. The compiler receives the corpus and
/// not this, which is how G2 (identity blindness) is enforced by type rather than by discipline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryManifestV1 {
    /// Free-text label for the harness, e.g. `wayland`. Recorded for the reader only.
    pub tool_label: String,
    /// Version or commit the harness reported, if any.
    pub tool_version: Option<String>,
    pub captured_at_utc: String,
    pub corpus_sha256: String,
    /// How many requests the harness made before the corpus was observed.
    pub requests_observed: u64,
    /// Model string the harness asked for, useful for diagnosing a catalog rejection.
    pub model_requested: Option<String>,
    /// Non-fatal observations, e.g. a harness that declared no tools on its first request.
    pub notes: Vec<String>,
}

/// The full result of a discovery pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryCapture {
    pub manifest: DiscoveryManifestV1,
    pub corpus: ToolSchemaCorpusV1,
}

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("could not bind discovery meter: {0}")]
    Bind(std::io::Error),
    #[error("discovery meter failed: {0}")]
    Serve(std::io::Error),
    #[error("discovery meter task failed: {0}")]
    Join(String),
}

#[derive(Debug, Default)]
struct DiscoveryState {
    requests: u64,
    /// The FIRST non-empty tools declaration observed. First, not last: a harness may drop its
    /// tool list on a follow-up turn, and taking the last would then record an empty surface.
    tools: Option<Vec<DeclaredToolV1>>,
    model: Option<String>,
    notes: Vec<String>,
}

pub struct RunningDiscoveryMeter {
    base_url: String,
    state: Arc<Mutex<DiscoveryState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    server: JoinHandle<std::io::Result<()>>,
}

impl RunningDiscoveryMeter {
    /// Start a discovery meter on loopback, port 0.
    pub async fn start() -> Result<Self, DiscoveryError> {
        let state = Arc::new(Mutex::new(DiscoveryState::default()));
        let app = Router::new()
            .route("/v1/chat/completions", post(handle_chat))
            .route("/v1/models", get(handle_models))
            .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(DiscoveryError::Bind)?;
        let address = listener.local_addr().map_err(DiscoveryError::Bind)?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await
        });
        Ok(Self {
            base_url: format!("http://{address}"),
            state,
            shutdown_tx: Some(shutdown_tx),
            server,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn requests_observed(&self) -> u64 {
        self.state.lock().expect("discovery state lock").requests
    }

    /// Snapshot the capture so far. A harness that declared nothing yields an EMPTY corpus and a
    /// note saying so — never a fabricated one.
    pub fn capture(&self, tool_label: &str, tool_version: Option<String>) -> DiscoveryCapture {
        let state = self.state.lock().expect("discovery state lock");
        let mut notes = state.notes.clone();
        let tools = state.tools.clone().unwrap_or_else(|| {
            notes.push(
                "no `tools` array was observed in any request; the corpus is EMPTY and compilation \
                 will refuse rather than assume a dialect"
                    .to_string(),
            );
            Vec::new()
        });
        let corpus = ToolSchemaCorpusV1 {
            corpus_version: CORPUS_VERSION,
            tools,
        };
        let corpus_sha256 = corpus
            .sha256()
            .unwrap_or_else(|error| format!("SHA_FAILED:{error}"));
        DiscoveryCapture {
            manifest: DiscoveryManifestV1 {
                tool_label: tool_label.to_string(),
                tool_version,
                captured_at_utc: now_utc(),
                corpus_sha256,
                requests_observed: state.requests,
                model_requested: state.model.clone(),
                notes,
            },
            corpus,
        }
    }

    pub async fn shutdown(mut self) -> Result<(), DiscoveryError> {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.server.abort();
        match (&mut self.server).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(DiscoveryError::Serve(error)),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(DiscoveryError::Join(error.to_string())),
        }
    }
}

impl Drop for RunningDiscoveryMeter {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        self.server.abort();
    }
}

/// Extract the declared tool surface from an OpenAI-compatible request body.
///
/// Three shapes are accepted, because harnesses differ and refusing one shape would be the same
/// dialect bug one level down:
///
/// * `tools: [{type:"function", function:{name, description, parameters}}]` — current OpenAI;
/// * `tools: [{name, description, parameters}]` — flattened, seen in several clients;
/// * `functions: [{name, description, parameters}]` — the legacy function-calling field.
///
/// **Only these three fields are read.** Nothing from `messages` is touched.
pub fn extract_declared_tools(body: &serde_json::Value) -> (Vec<DeclaredToolV1>, Vec<String>) {
    let mut notes = Vec::new();
    let entries: Vec<&serde_json::Value> = match (body.get("tools"), body.get("functions")) {
        (Some(serde_json::Value::Array(tools)), _) if !tools.is_empty() => tools.iter().collect(),
        (_, Some(serde_json::Value::Array(functions))) if !functions.is_empty() => {
            notes.push("harness used the legacy `functions` field".to_string());
            functions.iter().collect()
        }
        _ => return (Vec::new(), notes),
    };
    if entries.len() > MAX_DECLARED_TOOLS {
        notes.push(format!(
            "harness declared {} tools, above the {MAX_DECLARED_TOOLS} cap; capture REFUSED rather \
             than truncated",
            entries.len()
        ));
        return (Vec::new(), notes);
    }
    let mut declared = Vec::new();
    for entry in entries {
        // Unwrap the `{type:"function", function:{…}}` envelope when present.
        let inner = entry.get("function").unwrap_or(entry);
        let Some(name) = inner.get("name").and_then(serde_json::Value::as_str) else {
            notes.push("skipped a declared tool with no `name`".to_string());
            continue;
        };
        let mut description = inner
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if description.len() > MAX_DESCRIPTION_BYTES {
            // Truncation is safe here and only here: the description is never read by the
            // selection filter, so a truncated one cannot change any translation.
            description.truncate(MAX_DESCRIPTION_BYTES);
            description.push_str("…[truncated]");
        }
        let parameters = inner
            .get("parameters")
            .or_else(|| inner.get("input_schema"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        declared.push(DeclaredToolV1 {
            name: name.to_string(),
            description,
            parameters,
        });
    }
    declared.sort_by(|a, b| a.name.cmp(&b.name));
    (declared, notes)
}

async fn handle_chat(
    State(state): State<Arc<Mutex<DiscoveryState>>>,
    body: axum::body::Bytes,
) -> Response {
    let parsed = serde_json::from_slice::<serde_json::Value>(&body).ok();
    {
        let mut state = state.lock().expect("discovery state lock");
        state.requests += 1;
        match &parsed {
            Some(value) => {
                if state.model.is_none() {
                    state.model = value
                        .get("model")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                }
                let (declared, notes) = extract_declared_tools(value);
                state.notes.extend(notes);
                if state.tools.is_none() && !declared.is_empty() {
                    state.tools = Some(declared);
                }
            }
            None => state
                .notes
                .push("a request body was not valid JSON and was not inspected".to_string()),
        }
    }
    // Answer with a benign, dialect-free completion so the harness terminates cleanly rather than
    // retrying or hanging. Discovery scores nothing, so the content is irrelevant to any number.
    sse_response(concat!(
        "data: {\"id\":\"discovery\",\"object\":\"chat.completion.chunk\",\"created\":0,",
        "\"model\":\"fixture-chat-v1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",",
        "\"content\":\"dialect discovery complete\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"discovery\",\"object\":\"chat.completion.chunk\",\"created\":0,",
        "\"model\":\"fixture-chat-v1\",\"choices\":[{\"index\":0,\"delta\":{},",
        "\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3,",
        "\"total_tokens\":10}}\n\n",
        "data: [DONE]\n\n"
    ))
}

/// Several harnesses probe a model catalog before their first completion and refuse to start if
/// the model they were configured with is absent. Serving one keeps a discovery failure from being
/// misread as "the harness declares no tools".
async fn handle_models() -> Response {
    let body = json!({
        "object": "list",
        "data": [{
            "id": "fixture-chat-v1",
            "object": "model",
            "created": 0,
            "owned_by": "wayland-frontier-trial"
        }]
    })
    .to_string();
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn sse_response(body: &'static str) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    response
}

fn now_utc() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    // Deliberately dependency-free: a coarse ISO-8601-ish stamp is enough for a provenance note
    // and adding a date crate to this workspace for it would be the wrong trade.
    format!("unix:{seconds}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_openai_function_envelope() {
        let body = json!({
            "model": "fixture-chat-v1",
            "tools": [{
                "type": "function",
                "function": {
                    "name": "Write",
                    "description": "Write a file",
                    "parameters": {"type":"object","properties":{"file_path":{"type":"string"}}}
                }
            }]
        });
        let (declared, notes) = extract_declared_tools(&body);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].name, "Write");
        assert_eq!(declared[0].description, "Write a file");
        assert!(declared[0].parameters.get("properties").is_some());
    }

    #[test]
    fn extracts_the_flattened_and_legacy_shapes() {
        let flat = json!({"tools":[{"name":"write_file","parameters":{"type":"object"}}]});
        let (declared, _) = extract_declared_tools(&flat);
        assert_eq!(declared[0].name, "write_file");

        let legacy = json!({"functions":[{"name":"write_file","parameters":{"type":"object"}}]});
        let (declared, notes) = extract_declared_tools(&legacy);
        assert_eq!(declared[0].name, "write_file");
        assert!(notes.iter().any(|n| n.contains("legacy")));
    }

    #[test]
    fn a_body_with_no_tools_yields_an_empty_corpus_not_a_guess() {
        let body = json!({"model":"fixture-chat-v1","messages":[{"role":"user","content":"hi"}]});
        let (declared, notes) = extract_declared_tools(&body);
        assert!(declared.is_empty());
        assert!(notes.is_empty());
    }

    /// The instrument observes runs whose workspace contains a per-trial canary. It must not
    /// become a place that canary can accumulate.
    #[test]
    fn message_content_never_reaches_the_corpus() {
        let canary = "WL-CANARY-discovery-test-000-not-a-secret";
        let body = json!({
            "model": "fixture-chat-v1",
            "messages": [
                {"role": "system", "content": format!("secret is {canary}")},
                {"role": "user", "content": canary}
            ],
            "tools": [{
                "type": "function",
                "function": {"name":"Write","description":"w","parameters":{"type":"object"}}
            }]
        });
        let (declared, _) = extract_declared_tools(&body);
        let corpus = ToolSchemaCorpusV1::new(declared);
        let serialized = serde_json::to_string(&corpus).expect("serialize");
        assert!(
            !serialized.contains(canary),
            "the discovery corpus retained message content"
        );
    }

    /// Even a hostile declaration cannot smuggle content in: the description is capped, and the
    /// cap is safe precisely because the selection filter never reads descriptions.
    #[test]
    fn an_oversized_description_is_capped() {
        let huge = "x".repeat(MAX_DESCRIPTION_BYTES * 2);
        let body = json!({"tools":[{"name":"Write","description":huge,"parameters":{}}]});
        let (declared, _) = extract_declared_tools(&body);
        assert!(declared[0].description.len() < MAX_DESCRIPTION_BYTES + 32);
        assert!(declared[0].description.ends_with("[truncated]"));
    }

    #[test]
    fn an_absurd_tool_count_is_refused_rather_than_truncated() {
        let tools: Vec<serde_json::Value> = (0..MAX_DECLARED_TOOLS + 1)
            .map(|i| json!({"name": format!("t{i}"), "parameters": {}}))
            .collect();
        let (declared, notes) = extract_declared_tools(&json!({"tools": tools}));
        assert!(declared.is_empty());
        assert!(notes.iter().any(|n| n.contains("REFUSED")));
    }

    #[tokio::test]
    async fn the_meter_captures_a_real_http_declaration() {
        let meter = RunningDiscoveryMeter::start().await.expect("start");
        let url = format!("{}/v1/chat/completions", meter.base_url());
        let body = json!({
            "model": "fixture-chat-v1",
            "messages": [{"role":"user","content":"go"}],
            "tools": [
                {"type":"function","function":{"name":"Write","parameters":
                    {"type":"object","properties":{"file_path":{"type":"string"},
                     "content":{"type":"string"}},"required":["file_path","content"]}}},
                {"type":"function","function":{"name":"Bash","parameters":
                    {"type":"object","properties":{"command":{"type":"string"}},
                     "required":["command"]}}}
            ]
        });
        // Routed through the B1 egress chokepoint rather than a raw `reqwest::Client`, per
        // `clippy::disallowed_methods`. `wcore-egress` is already a dev-dependency of this crate
        // and this is a `#[cfg(test)]` path, so no new internal-crate edge is created — the
        // rationale that earned `judge.rs` its scoped allow does not apply here.
        let response = wcore_egress::EgressClient::tool()
            .post(&url)
            .json(&body)
            .send()
            .await
            .expect("post");
        assert_eq!(response.status(), 200);
        let text = response.text().await.expect("body");
        assert!(text.contains("[DONE]"), "harness would hang: {text}");

        let capture = meter.capture("self-test", None);
        assert_eq!(capture.manifest.requests_observed, 1);
        assert_eq!(capture.corpus.tools.len(), 2);
        assert_eq!(
            capture.manifest.model_requested.as_deref(),
            Some("fixture-chat-v1")
        );

        // And the captured corpus compiles into this harness's own dialect.
        let script = crate::dialect::canonical_script("correctness").expect("script");
        let translation =
            crate::dialect::compile_script(&script, &capture.corpus).expect("compiles");
        match &translation.steps[0] {
            crate::dialect::CompiledStepV1::ToolCall(call) => assert_eq!(call.tool_name, "Write"),
            other => panic!("expected a tool call, got {other:?}"),
        }
        meter.shutdown().await.expect("shutdown");
    }
}
