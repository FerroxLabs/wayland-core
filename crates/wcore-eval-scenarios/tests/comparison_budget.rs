//! Real loopback requests only; no operator credentials or live account access.
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use tokio::{sync::Semaphore, task::JoinHandle};
use wcore_eval_scenarios::comparison_budget::{FixtureBudgetProxy, Ledger};

struct Upstream {
    input: AtomicU64,
    output: AtomicU64,
    count_status: StatusCode,
    generation_status: StatusCode,
    usage_present: bool,
    generation_calls: AtomicUsize,
    release: Semaphore,
    bodies: Mutex<Vec<Value>>,
}
struct Fixture {
    state: Arc<Upstream>,
    url: String,
    task: JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new(
        input: u64,
        usage_present: bool,
        count_status: StatusCode,
        generation_status: StatusCode,
    ) -> Self {
        let state = Arc::new(Upstream {
            input: AtomicU64::new(input),
            output: AtomicU64::new(40),
            count_status,
            generation_status,
            usage_present,
            generation_calls: AtomicUsize::new(0),
            release: Semaphore::new(0),
            bodies: Mutex::new(vec![]),
        });
        let app = Router::new()
            .route("/v1/responses/input_tokens", post(count))
            .route("/v1/responses", post(generate))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { state, url, task }
    }
}
async fn count(
    State(state): State<Arc<Upstream>>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    assert_eq!(body["model"], "gpt-6-astra");
    (
        state.count_status,
        Json(
            json!({"object":"response.input_tokens","input_tokens":state.input.load(Ordering::SeqCst)}),
        ),
    )
}
async fn generate(
    State(state): State<Arc<Upstream>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(headers["authorization"], "Bearer w16-fake-upstream-only");
    state.bodies.lock().unwrap().push(body);
    state.generation_calls.fetch_add(1, Ordering::SeqCst);
    let status = state.generation_status;
    let stream = futures::stream::once(async move {
        let permit = state.release.acquire().await.unwrap();
        permit.forget();
        let usage = if state.usage_present {
            json!({"input_tokens":state.input.load(Ordering::SeqCst),"output_tokens":state.output.load(Ordering::SeqCst)})
        } else {
            Value::Null
        };
        Ok::<Bytes, std::io::Error>(Bytes::from(format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"type":"response.completed","response":{"usage":usage}})
        )))
    });
    Response::builder()
        .status(status)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}
fn request() -> Value {
    json!({"model":"gpt-6-astra","reasoning":{"effort":"medium"},"input":"private-prompt-canary","stream":true})
}
async fn setup(
    input: u64,
    usage: bool,
    count_status: StatusCode,
    generation_status: StatusCode,
) -> (tempfile::TempDir, Arc<Ledger>, Fixture, FixtureBudgetProxy) {
    let root = tempfile::tempdir().unwrap();
    let ledger = Arc::new(Ledger::open(root.path().join("ledger.json")).unwrap());
    let fixture = Fixture::new(input, usage, count_status, generation_status).await;
    let proxy = FixtureBudgetProxy::start(ledger.clone(), "leg-a".into(), &fixture.url)
        .await
        .unwrap();
    (root, ledger, fixture, proxy)
}
#[allow(clippy::disallowed_methods)]
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}
#[allow(clippy::disallowed_methods)]
async fn send(proxy: &FixtureBudgetProxy, body: Value) -> reqwest::Response {
    client()
        .post(format!("{}/responses", proxy.base_url))
        .bearer_auth(&proxy.peer_key)
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn concurrent_calls_reserve_before_post_and_preserve_fixed_cap_and_privacy() {
    let (_root, ledger, fixture, proxy) = setup(8_000, true, StatusCode::OK, StatusCode::OK).await;
    let responses = futures::future::join_all((0..4).map(|_| send(&proxy, request()))).await;
    assert_eq!(
        responses.iter().filter(|r| r.status().is_success()).count(),
        1
    );
    assert_eq!(fixture.state.generation_calls.load(Ordering::SeqCst), 1);
    fixture.state.release.add_permits(1);
    for response in responses {
        let _ = response.bytes().await.unwrap();
    }
    proxy.close().await.unwrap();
    let report = ledger.report().unwrap();
    assert_eq!(report.calls.len(), 1);
    assert_eq!(report.calls[0].outcome, "usage_reconciled");
    assert_eq!(report.calls[0].output_tokens, Some(40));
    let bodies = fixture.state.bodies.lock().unwrap();
    assert_eq!(bodies[0]["max_output_tokens"], 1024);
    assert_eq!(bodies[0]["service_tier"], "default");
    let receipt = serde_json::to_string(&report).unwrap();
    assert!(
        !receipt.contains("private-prompt-canary")
            && !receipt.contains("fake-peer")
            && !receipt.contains("fake-upstream")
    );
}

#[tokio::test]
async fn missing_usage_and_upstream_error_consume_reservation_without_refund() {
    for (usage, status) in [
        (false, StatusCode::OK),
        (true, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let (_root, ledger, fixture, proxy) = setup(20, usage, StatusCode::OK, status).await;
        fixture.state.release.add_permits(1);
        let response = send(&proxy, request()).await;
        let _ = response.bytes().await.unwrap();
        proxy.close().await.unwrap();
        let report = ledger.report().unwrap();
        assert_eq!(report.calls[0].outcome, "unknown_consumed");
        assert_eq!(
            report.calls[0].charged_nano_usd,
            report.calls[0].reserved_nano_usd
        );
    }
}

#[tokio::test]
async fn client_disconnect_does_not_cancel_the_accounting_worker() {
    let (_root, ledger, fixture, proxy) = setup(20, true, StatusCode::OK, StatusCode::OK).await;
    let response = send(&proxy, request()).await;
    drop(response);
    fixture.state.release.add_permits(1);
    proxy.close().await.unwrap();
    let report = ledger.report().unwrap();
    assert_eq!(report.calls[0].outcome, "usage_reconciled");
    assert!(report.calls[0].charged_nano_usd > 0);
}

#[tokio::test]
async fn usage_overrun_stops_other_legs_in_the_same_programme() {
    let (_root, ledger, fixture, proxy) = setup(20, true, StatusCode::OK, StatusCode::OK).await;
    fixture.state.output.store(1025, Ordering::SeqCst);
    fixture.state.release.add_permits(1);
    let _ = send(&proxy, request()).await.bytes().await.unwrap();
    proxy.close().await.unwrap();
    assert!(ledger.report().unwrap().halted);
    let other = FixtureBudgetProxy::start(ledger.clone(), "other-leg".into(), &fixture.url)
        .await
        .unwrap();
    assert_eq!(
        send(&other, request()).await.status(),
        StatusCode::PAYMENT_REQUIRED
    );
    assert_eq!(fixture.state.generation_calls.load(Ordering::SeqCst), 1);
    other.close().await.unwrap();
}

#[tokio::test]
#[allow(clippy::disallowed_methods)]
async fn invalid_count_routes_body_and_contract_never_dispatch_generation() {
    let (_root, ledger, fixture, proxy) =
        setup(20, true, StatusCode::BAD_REQUEST, StatusCode::OK).await;
    assert_eq!(
        send(&proxy, request()).await.status(),
        StatusCode::BAD_GATEWAY
    );
    for endpoint in ["responses/compact", "memories/trace_summarize", "anything"] {
        let response = client()
            .post(format!("{}/{endpoint}", proxy.base_url))
            .bearer_auth(&proxy.peer_key)
            .json(&request())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let mut hosted = request();
    hosted["tools"] = json!([{"type":"web_search"}]);
    assert_eq!(send(&proxy, hosted).await.status(), StatusCode::BAD_REQUEST);
    let mut cap = request();
    cap["max_output_tokens"] = json!(2048);
    assert_eq!(send(&proxy, cap).await.status(), StatusCode::BAD_REQUEST);
    let mut oversized = request();
    oversized["input"] = json!("x".repeat(1024 * 1024));
    assert_eq!(
        send(&proxy, oversized).await.status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(fixture.state.generation_calls.load(Ordering::SeqCst), 0);
    assert!(ledger.report().unwrap().calls.is_empty());
    assert!(
        FixtureBudgetProxy::start(ledger, "live".into(), "https://api.openai.com/v1")
            .await
            .is_err()
    );
    assert!(FixtureBudgetProxy::live_admission_blocker().contains("proc"));
    proxy.close().await.unwrap();
}
