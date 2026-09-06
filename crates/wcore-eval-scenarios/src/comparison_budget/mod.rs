//! Shared W16 Responses admission. This initial surface is deliberately fixture-only.
//!
//! Live admission is unavailable: token-count endpoint billing/account access and
//! OS separation of peers from proxy credentials must be proved first. A custom
//! base URL and an environment filter alone do NOT protect a same-UID key from
//! filesystem or /proc reads. No live key is accepted, read, or stored here.
//! Unsupported compaction/memory/hosted-tool requests are refused, never emulated.
mod ledger;
pub use ledger::{BudgetReport, CallReceipt, Ledger, OUTPUT_CAP};

use anyhow::{Result, anyhow, ensure};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use futures::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

pub const MAX_BODY_BYTES: usize = 1024 * 1024;
const MODEL: &str = "gpt-6-astra";
const UPSTREAM_FIXTURE_KEY: &str = "w16-fake-upstream-only";

struct Shared {
    ledger: Arc<Ledger>,
    upstream: String,
    leg: String,
    peer_key: String,
    client: reqwest::Client,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

/// A separate listener binds one leg to one fake bearer; peers cannot select
/// another leg by supplying an ID in their request body.
pub struct FixtureBudgetProxy {
    pub base_url: String,
    pub peer_key: String,
    shared: Arc<Shared>,
    stop: Option<oneshot::Sender<()>>,
    server: Option<JoinHandle<()>>,
}

impl FixtureBudgetProxy {
    pub fn live_admission_blocker() -> &'static str {
        "live admission disabled: verify counting endpoint billing/access and isolate proxy credentials from peer filesystem/proc/network authority"
    }

    /// Accept only numeric loopback HTTP fixtures, never a real provider or key.
    #[allow(clippy::disallowed_methods)] // Existing eval fixture HTTP boundary; URL is checked below.
    pub async fn start(ledger: Arc<Ledger>, leg: String, upstream: &str) -> Result<Self> {
        let url = reqwest::Url::parse(upstream)?;
        let loopback = url
            .host_str()
            .and_then(|h| h.trim_matches(['[', ']']).parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        ensure!(
            url.scheme() == "http"
                && loopback
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/v1",
            "only explicit numeric loopback /v1 fixtures are admitted"
        );
        ensure!(
            !leg.is_empty() && leg.len() <= 128,
            "invalid leg identifier"
        );
        let peer_key = format!("w16-fake-peer-{:032x}", rand::random::<u128>());
        let shared = Arc::new(Shared {
            ledger,
            upstream: upstream.trim_end_matches('/').into(),
            leg,
            peer_key: peer_key.clone(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .http1_only()
                .pool_max_idle_per_host(0)
                .timeout(Duration::from_secs(10))
                .build()?,
            workers: Mutex::new(vec![]),
        });
        let app = Router::new()
            .route("/v1/responses", post(handle))
            .fallback(|| async { refusal(StatusCode::FORBIDDEN, "uncapped_or_unknown_endpoint") })
            .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
            .with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let base_url = format!("http://{}/v1", listener.local_addr()?);
        let (stop, stopped) = oneshot::channel();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await;
        });
        Ok(Self {
            base_url,
            peer_key,
            shared,
            stop: Some(stop),
            server: Some(server),
        })
    }

    pub async fn close(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(mut server) = self.server.take() {
            if tokio::time::timeout(Duration::from_secs(12), &mut server)
                .await
                .is_err()
            {
                server.abort();
                let _ = server.await;
            }
        }
        let workers = std::mem::take(
            &mut *self
                .shared
                .workers
                .lock()
                .map_err(|_| anyhow!("worker ownership poisoned"))?,
        );
        for worker in workers {
            worker.await?;
        }
        Ok(())
    }
}
impl Drop for FixtureBudgetProxy {
    fn drop(&mut self) {
        // Detached bounded workers retain their claims and finish settlement;
        // dropping a client/server is never proof that the upstream did no work.
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn refusal(status: StatusCode, code: &str) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"error":{"code":code,"message":code}}).to_string(),
        ))
        .expect("static response")
}

fn normalize(mut body: Value) -> Result<Value> {
    ensure!(body.is_object(), "request must be an object");
    ensure!(body["model"] == MODEL, "model must be Astra");
    ensure!(
        body.pointer("/reasoning/effort").and_then(Value::as_str) == Some("medium"),
        "effort must be medium"
    );
    ensure!(
        body.get("service_tier").is_none_or(|v| v == "default"),
        "only Standard processing is admitted"
    );
    ensure!(
        body.get("max_output_tokens")
            .is_none_or(|v| v.as_u64() == Some(OUTPUT_CAP)),
        "both peers require the same output cap"
    );
    ensure!(
        !body
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "background calls are not bounded by this worker"
    );
    ensure!(
        body.get("context_management").is_none() && body.get("conversation").is_none(),
        "server compaction and mutable conversation inputs need separate budget authority"
    );
    if let Some(tools) = body.get("tools") {
        let tools = tools.as_array().ok_or_else(|| anyhow!("invalid tools"))?;
        ensure!(
            tools
                .iter()
                .all(|tool| matches!(tool["type"].as_str(), Some("function" | "custom"))),
            "hosted paid tools are not admitted"
        );
    }
    body["max_output_tokens"] = json!(OUTPUT_CAP);
    body["service_tier"] = json!("default");
    Ok(body)
}

async fn handle(State(shared): State<Arc<Shared>>, headers: HeaderMap, bytes: Bytes) -> Response {
    let bearer = format!("Bearer {}", shared.peer_key);
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(bearer.as_str()) {
        return refusal(StatusCode::UNAUTHORIZED, "wrong_leg_bearer");
    }
    let body = match serde_json::from_slice::<Value>(&bytes)
        .map_err(anyhow::Error::from)
        .and_then(normalize)
    {
        Ok(body) => body,
        Err(_) => return refusal(StatusCode::BAD_REQUEST, "request_outside_frozen_contract"),
    };
    let (reply, receiver) = oneshot::channel();
    let worker_shared = shared.clone();
    let worker = tokio::spawn(async move {
        run_request(worker_shared, body, reply).await;
    });
    shared
        .workers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(worker);
    receiver
        .await
        .unwrap_or_else(|_| refusal(StatusCode::BAD_GATEWAY, "owned_worker_failed"))
}

#[allow(clippy::disallowed_methods)] // Numeric loopback is enforced by the only constructor.
async fn run_request(shared: Arc<Shared>, body: Value, reply: oneshot::Sender<Response>) {
    // Only the documented count parameters are forwarded. There is no paid
    // generation fallback if counting refuses or its response is malformed.
    let mut count = serde_json::Map::new();
    for key in [
        "model",
        "input",
        "instructions",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "reasoning",
        "text",
        "conversation",
        "previous_response_id",
        "truncation",
    ] {
        if let Some(value) = body.get(key) {
            count.insert(key.into(), value.clone());
        }
    }
    if shared.ledger.record_count().is_err() {
        let _ = reply.send(refusal(
            StatusCode::PAYMENT_REQUIRED,
            "budget_authority_unavailable",
        ));
        return;
    }
    let counted = shared
        .client
        .post(format!("{}/responses/input_tokens", shared.upstream))
        .bearer_auth(UPSTREAM_FIXTURE_KEY)
        .json(&count)
        .send()
        .await;
    let input = match counted {
        Ok(response) if response.status().is_success() => match response.json::<Value>().await {
            Ok(value) if value["object"] == "response.input_tokens" => {
                value["input_tokens"].as_u64()
            }
            _ => None,
        },
        _ => None,
    };
    let Some(input) = input else {
        let _ = reply.send(refusal(
            StatusCode::BAD_GATEWAY,
            "count_not_verified_no_generation_sent",
        ));
        return;
    };
    let digest = format!("{:x}", Sha256::digest(body.to_string().as_bytes()));
    let claim = match shared.ledger.reserve(&shared.leg, input, digest) {
        Ok(claim) => claim,
        Err(_) => {
            let _ = reply.send(refusal(
                StatusCode::PAYMENT_REQUIRED,
                "budget_admission_refused",
            ));
            return;
        }
    };
    let response = shared
        .client
        .post(format!("{}/responses", shared.upstream))
        .bearer_auth(UPSTREAM_FIXTURE_KEY)
        .json(&body)
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            // reqwest's connect failure precedes sending HTTP request bytes.
            if shared
                .ledger
                .settle(claim, None, error.is_connect())
                .is_err()
            {
                shared.ledger.disable();
            }
            let _ = reply.send(refusal(
                StatusCode::BAD_GATEWAY,
                "upstream_transport_failure",
            ));
            return;
        }
    };
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response.headers().get("content-type").cloned();
    let (sender, receiver) = mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let stream = futures::stream::unfold(receiver, |mut rx| async move {
        rx.recv().await.map(|v| (v, rx))
    });
    let mut output = Response::builder().status(status);
    if let Some(content_type) = content_type {
        output = output.header("content-type", content_type);
    }
    let _ = reply.send(
        output
            .body(Body::from_stream(stream))
            .expect("upstream status response"),
    );
    let mut wire = Vec::new();
    let mut intact = true;
    let mut downstream = Some(sender);
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => {
                intact = false;
                break;
            }
        };
        if wire.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            intact = false;
            break;
        }
        wire.extend_from_slice(&chunk);
        if let Some(sender) = &downstream
            && sender.try_send(Ok(chunk)).is_err()
        {
            // A slow/gone reader cannot stall accounting or get a refund.
            downstream = None;
        }
    }
    let usage = if intact && status.is_success() {
        usage(&wire)
    } else {
        None
    };
    if shared.ledger.settle(claim, usage, false).is_err() {
        shared.ledger.disable();
    }
    drop(downstream);
}

fn usage(wire: &[u8]) -> Option<(u64, u64)> {
    fn counters(value: &Value) -> Option<(u64, u64)> {
        let usage = value.get("usage")?;
        Some((
            usage["input_tokens"].as_u64()?,
            usage["output_tokens"].as_u64()?,
        ))
    }
    if let Ok(value) = serde_json::from_slice::<Value>(wire) {
        return counters(&value);
    }
    let text = std::str::from_utf8(wire).ok()?;
    let mut result = None;
    for line in text.lines().filter_map(|line| line.strip_prefix("data: ")) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if matches!(
            value["type"].as_str(),
            Some("response.completed" | "response.incomplete")
        ) {
            if result.is_some() {
                return None;
            }
            result = counters(&value["response"]);
        }
    }
    result
}
