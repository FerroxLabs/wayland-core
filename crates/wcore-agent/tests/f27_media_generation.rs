//! F27-C3 — hermetic media-generation fixture and the four presentation shapes.
//!
//! # What Phase 27 left open
//!
//! The phase verdict on Criterion 3 reads, verbatim: *"None of the four
//! generation shapes was exercised. No MCP media-tool fixture was built, so
//! MCP-only, late-MCP and combined were never reachable."* The only way media
//! generation had ever been exercised in this repo was against a **live,
//! billable** provider account, which is why it was almost never exercised.
//!
//! This file closes that: [`MediaFixture`] is a loopback-only server that
//! speaks **both** wire protocols the product uses to reach a media capability:
//!
//! * `POST /v1/images/generations` — the OpenAI-wire endpoint the **built-in**
//!   `image_generate` backend calls. Real HTTP, real `DalleBackend`, real
//!   `ImageGenerationTool`, no money.
//! * `POST /mcp` — MCP streamable-HTTP, exposing a media tool through the real
//!   `McpManager` client. This is the fixture whose absence made the MCP-only,
//!   late-MCP and combined shapes unreachable.
//!
//! Nothing here reaches the network: the listener binds `127.0.0.1:0`.
//!
//! # Why the fixture is configurable rather than canned
//!
//! Every assertion in this file is about whether an observable **varies with
//! the work done**. A fixture that always answers the same way cannot
//! distinguish "the product recorded what happened" from "the product emits a
//! constant" — which is the exact defect class this programme keeps finding.
//! So the fixture takes a [`MediaFixtureMode`] and each test changes **one
//! variable** against it.
//!
//! In particular [`MediaFixtureMode::cost_header`] exists to be the
//! **known-positive control** for the unpriced assertions. Without it, a test
//! asserting "this call is recorded unpriced" would also pass on an
//! implementation that is *incapable* of ever recording a price.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use wcore_config::config::{McpServerConfig, TransportType};
use wcore_mcp::manager::McpManager;
use wcore_tools::Tool;
use wcore_tools::image_generation_tool::ImageGenerationTool;
use wcore_tools::media_cost::{MediaCostLedger, MediaRateCard};

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// A 1x1 transparent PNG, base64. Deterministic and tiny: the tests are about
/// accounting and discovery, not about pixels.
const TINY_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

/// Prompt sentinel that makes the fixture answer with an upstream safety
/// refusal, so the failure path is exercised on the real wire rather than by
/// constructing an error value in-process.
const REJECT_SENTINEL: &str = "TRIGGER_REJECT";
/// Prompt sentinel that makes the fixture answer HTTP 402.
const CREDITS_SENTINEL: &str = "TRIGGER_402";

/// The bearer token the fixture accepts. Any other value gets a 401, so the
/// credential dimension of F27-C3 is measurable rather than assumed.
const FIXTURE_TOKEN: &str = "fixture-media-token";

#[derive(Debug, Clone, Default)]
pub struct MediaFixtureMode {
    /// When `Some`, the image endpoint answers with an `x-flux-cost-usd`
    /// header carrying this value.
    ///
    /// The default is `None`, which mirrors what Phase 27 **measured** against
    /// a live FluxRouter account: the image endpoint returns no cost in any
    /// channel. `Some` is the control proving the header path is live code.
    pub cost_header_usd: Option<f64>,
    /// Name the MCP server advertises its media tool under. Set to
    /// `image_generate` to reproduce the **combined** shape, where an
    /// MCP-supplied tool collides with the built-in of the same name
    /// (threat T-27-03-08).
    pub mcp_tool_name: String,
    /// When true the MCP media tool refuses any call that does not carry an
    /// `api_key` argument, so the MCP shape's credential behaviour can be
    /// compared with the built-in's.
    pub mcp_requires_credential: bool,
}

impl MediaFixtureMode {
    fn silent() -> Self {
        Self {
            cost_header_usd: None,
            mcp_tool_name: "mcp_image_generate".to_string(),
            mcp_requires_credential: false,
        }
    }

    fn with_cost_header(usd: f64) -> Self {
        Self {
            cost_header_usd: Some(usd),
            ..Self::silent()
        }
    }
}

#[derive(Debug, Default)]
struct FixtureLog {
    /// Every `(path, rpc_method_or_none)` the fixture served, so a test can
    /// prove the call actually crossed the wire rather than being short
    /// circuited somewhere in the product.
    hits: Vec<String>,
    /// Prompts the image endpoint received.
    image_prompts: Vec<String>,
    /// Sizes the image endpoint received.
    image_sizes: Vec<String>,
}

struct FixtureState {
    mode: MediaFixtureMode,
    log: Mutex<FixtureLog>,
}

pub struct MediaFixture {
    addr: SocketAddr,
    state: Arc<FixtureState>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl MediaFixture {
    async fn start(mode: MediaFixtureMode) -> Self {
        let state = Arc::new(FixtureState {
            mode,
            log: Mutex::new(FixtureLog::default()),
        });
        let app = Router::new()
            .route("/v1/images/generations", post(images_generations))
            .route("/mcp", post(mcp_endpoint))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind loopback fixture");
        let addr = listener.local_addr().expect("fixture local addr");
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        Self {
            addr,
            state,
            shutdown: Some(tx),
        }
    }

    /// The `/v1` API root, in the shape `openai_wire_media_base` resolves.
    fn api_base(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    fn mcp_url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    fn hits(&self) -> Vec<String> {
        self.state.log.lock().hits.clone()
    }

    fn image_prompts(&self) -> Vec<String> {
        self.state.log.lock().image_prompts.clone()
    }

    fn image_sizes(&self) -> Vec<String> {
        self.state.log.lock().image_sizes.clone()
    }
}

impl Drop for MediaFixture {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// OpenAI-wire image generation. Deliberately mirrors the real endpoint's
/// contract: bearer auth, `prompt`/`size`/`n` in the body, `data[0].b64_json`
/// out, and the error statuses the product's `map_http_error` classifies on.
async fn images_generations(
    State(state): State<Arc<FixtureState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state.log.lock().hits.push("/v1/images/generations".into());

    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v == format!("Bearer {FIXTURE_TOKEN}"))
        .unwrap_or(false);
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            r#"{"error":{"message":"invalid api key"}}"#,
        )
            .into_response();
    }

    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let prompt = parsed
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let size = parsed
        .get("size")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    {
        let mut log = state.log.lock();
        log.image_prompts.push(prompt.clone());
        log.image_sizes.push(size);
    }

    if prompt.contains(REJECT_SENTINEL) {
        // 400 + "safety" is what the product classifies as PromptRejected.
        return (
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"blocked by safety policy"}}"#,
        )
            .into_response();
    }
    if prompt.contains(CREDITS_SENTINEL) {
        return (
            StatusCode::PAYMENT_REQUIRED,
            r#"{"error":{"message":"insufficient credits"}}"#,
        )
            .into_response();
    }

    let payload = json!({
        "created": 1,
        "data": [{"b64_json": TINY_PNG_B64}]
    })
    .to_string();

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(usd) = state.mode.cost_header_usd {
        builder = builder.header("x-flux-cost-usd", format!("{usd}"));
    }
    builder
        .body(axum::body::Body::from(payload))
        .expect("valid fixture response")
}

/// MCP streamable-HTTP endpoint exposing one media tool.
async fn mcp_endpoint(State(state): State<Arc<FixtureState>>, body: Bytes) -> Response {
    let request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    state.log.lock().hits.push(format!("/mcp:{method}"));

    // Notifications carry no id and get no response, per JSON-RPC.
    let Some(id) = request.get("id").cloned() else {
        return StatusCode::ACCEPTED.into_response();
    };

    let tool_name = state.mode.mcp_tool_name.clone();
    let result = match method.as_str() {
        "initialize" => json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "f27-media-fixture", "version": "1"}
        }),
        "tools/list" => json!({
            "tools": [{
                "name": tool_name,
                "description":
                    "Generate an image from a text prompt (F27 hermetic media fixture).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prompt": {"type": "string"},
                        "aspect_ratio": {
                            "type": "string",
                            "enum": ["landscape", "square", "portrait"]
                        },
                        "api_key": {"type": "string"}
                    },
                    "required": ["prompt"]
                }
            }]
        }),
        "tools/call" => {
            let prompt = request
                .pointer("/params/arguments/prompt")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let key = request
                .pointer("/params/arguments/api_key")
                .and_then(Value::as_str);
            if state.mode.mcp_requires_credential && key != Some(FIXTURE_TOKEN) {
                // MCP signals a TOOL-level failure as a successful call with
                // isError set — not a transport error. Modelling that
                // faithfully is the point: it is exactly where the MCP shape's
                // failure semantics diverge from the built-in's.
                json!({
                    "content": [{
                        "type": "text",
                        "text": json!({
                            "success": false,
                            "errorCategory": "no_provider_configured",
                            "error": "media fixture requires an api_key argument"
                        }).to_string()
                    }],
                    "isError": true
                })
            } else if prompt.contains(REJECT_SENTINEL) {
                json!({
                    "content": [{
                        "type": "text",
                        "text": json!({
                            "success": false,
                            "errorCategory": "prompt_rejected",
                            "error": "blocked by safety policy"
                        }).to_string()
                    }],
                    "isError": true
                })
            } else {
                let aspect = request
                    .pointer("/params/arguments/aspect_ratio")
                    .and_then(Value::as_str)
                    .unwrap_or("landscape");
                let (w, h) = match aspect {
                    "square" => (1024, 1024),
                    "portrait" => (1024, 1536),
                    _ => (1536, 1024),
                };
                json!({
                    "content": [{
                        "type": "text",
                        "text": json!({
                            "success": true,
                            "image": format!("data:image/png;base64,{TINY_PNG_B64}"),
                            "usedProvider": "f27-media-fixture",
                            "width": w,
                            "height": h
                        }).to_string()
                    }],
                    "isError": false
                })
            }
        }
        _ => {
            return axum::Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "method not found"}
            }))
            .into_response();
        }
    };
    axum::Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the real production `image_generate` tool pointed at the fixture:
/// the real `DalleBackend`, the real SSRF-guarded egress client, real HTTP.
/// Only the endpoint is substituted.
fn builtin_tool(
    fixture: &MediaFixture,
    rate_card: MediaRateCard,
) -> (ImageGenerationTool, Arc<MediaCostLedger>) {
    // `None` compat model: this fixture is a bare OpenAI-wire endpoint with no
    // provider identity, so the resolver's global fallback applies. F-27C3-04's
    // per-provider defaults are asserted in `image_gen`'s own unit module,
    // where a real `Config` (and therefore a real `ProviderCompat`) exists.
    let backend = wcore_agent::tool_backends::image_gen::DalleBackend::new(
        FIXTURE_TOKEN.to_string(),
        &fixture.api_base(),
        None,
    );
    let ledger = MediaCostLedger::shared();
    let tool = ImageGenerationTool::with_backend(Arc::new(backend))
        .with_rate_card(rate_card)
        .with_cost_ledger(Arc::clone(&ledger));
    (tool, ledger)
}

fn parse(content: &str) -> Value {
    serde_json::from_str(content).expect("tool output must be valid JSON")
}

/// Assert the call really crossed the fixture's wire. Every accounting claim
/// below is worthless if the product short-circuited before the HTTP call, and
/// an assertion about a record is not an assertion that work happened.
fn assert_reached_image_endpoint(fixture: &MediaFixture, expected_calls: usize) {
    let hits: Vec<_> = fixture
        .hits()
        .into_iter()
        .filter(|h| h == "/v1/images/generations")
        .collect();
    assert_eq!(
        hits.len(),
        expected_calls,
        "expected {expected_calls} real HTTP image call(s), fixture saw: {:?}",
        fixture.hits()
    );
}

// ---------------------------------------------------------------------------
// Built-in shape
// ---------------------------------------------------------------------------

/// The production shape measured live in Phase 27: the provider returns no
/// cost in any channel. The product must record the call, record its units,
/// and say `unpriced` — not `$0.00`, and not nothing at all.
#[tokio::test]
async fn builtin_shape_records_units_and_reports_unpriced_when_provider_is_silent() {
    let fixture = MediaFixture::start(MediaFixtureMode::silent()).await;
    let (tool, ledger) = builtin_tool(&fixture, MediaRateCard::default());

    let result = tool
        .execute(json!({"prompt": "a lighthouse", "aspect_ratio": "landscape"}))
        .await;
    assert!(
        !result.is_error,
        "fixture call should succeed: {}",
        result.content
    );
    assert_reached_image_endpoint(&fixture, 1);

    let out = parse(&result.content);
    let acct = &out["accounting"];
    assert_eq!(acct["tool"], "image_generate");
    assert_eq!(acct["units"]["images"], 1);
    assert_eq!(acct["units"]["width"], 1536);
    assert_eq!(acct["units"]["height"], 1024);
    assert_eq!(acct["price_source"]["kind"], "unpriced");
    assert_eq!(acct["price_source"]["reason"], "provider_reports_no_cost");
    assert!(
        acct.get("cost_usd").is_none(),
        "a silent provider must yield no dollar figure at all, got: {acct}"
    );

    // The ledger must be able to say "this session spent on media, and I
    // cannot tell you how much" — which is different from "$0".
    let summary = ledger.summary();
    assert_eq!(summary.calls, 1);
    assert_eq!(summary.unpriced_calls, 1);
    assert_eq!(summary.priced_calls, 0);
    assert_eq!(summary.total_usd, 0.0);
    assert_eq!(summary.images, 1);
}

/// **Known-positive control for the test above.** Change exactly one variable
/// — the provider now returns a cost header — and the same code path must
/// produce a dollar figure stamped `provider_header`.
///
/// Without this test, `..._reports_unpriced_when_provider_is_silent` would
/// pass just as happily on an implementation that can never price anything,
/// which is precisely the invariant-observable failure this record exists to
/// avoid.
#[tokio::test]
async fn builtin_shape_reads_a_provider_reported_cost_from_the_response_header() {
    let fixture = MediaFixture::start(MediaFixtureMode::with_cost_header(0.0125)).await;
    let (tool, ledger) = builtin_tool(&fixture, MediaRateCard::default());

    let result = tool.execute(json!({"prompt": "a lighthouse"})).await;
    assert!(!result.is_error, "{}", result.content);
    assert_reached_image_endpoint(&fixture, 1);

    let acct = parse(&result.content)["accounting"].clone();
    assert_eq!(acct["price_source"]["kind"], "provider_header");
    assert_eq!(acct["price_source"]["header"], "x-flux-cost-usd");
    assert_eq!(acct["cost_usd"], 0.0125);

    let summary = ledger.summary();
    assert_eq!(summary.priced_calls, 1);
    assert_eq!(summary.unpriced_calls, 0);
    assert!((summary.total_usd - 0.0125).abs() < 1e-12);
}

/// The record must change when the requested work changes. Three aspect
/// ratios through one fixture, one variable moved each time.
#[tokio::test]
async fn builtin_shape_record_varies_with_the_requested_work() {
    let fixture = MediaFixture::start(MediaFixtureMode::silent()).await;
    let (tool, ledger) = builtin_tool(&fixture, MediaRateCard::default());

    for aspect in ["landscape", "square", "portrait"] {
        let result = tool
            .execute(json!({"prompt": format!("scene {aspect}"), "aspect_ratio": aspect}))
            .await;
        assert!(!result.is_error, "{}", result.content);
    }
    assert_reached_image_endpoint(&fixture, 3);

    let records = ledger.snapshot();
    assert_eq!(records.len(), 3);
    let dims: Vec<(Option<u32>, Option<u32>)> = records
        .iter()
        .map(|r| (r.units.width, r.units.height))
        .collect();
    assert_eq!(
        dims,
        vec![
            (Some(1536), Some(1024)),
            (Some(1024), Some(1024)),
            (Some(1024), Some(1536))
        ]
    );

    let mp: Vec<Option<f64>> = records.iter().map(|r| r.units.megapixels()).collect();
    assert!(
        mp[0] != mp[1],
        "landscape and square must not record identical megapixels: {mp:?}"
    );

    // ...and the variation reached the wire, not just the record: the fixture
    // itself saw three different sizes. Without this, the record could be
    // varying while the actual request was constant.
    let sizes = fixture.image_sizes();
    assert_eq!(sizes.len(), 3);
    assert_eq!(
        sizes
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "fixture should have received three distinct sizes, got {sizes:?}"
    );
    let prompts = fixture.image_prompts();
    assert_eq!(
        prompts,
        vec!["scene landscape", "scene square", "scene portrait"],
        "each call must have carried its own prompt to the wire"
    );
}

/// A rate card prices the very call the silent-provider test leaves unpriced,
/// and stamps it as a local estimate rather than as the provider's number.
#[tokio::test]
async fn builtin_shape_rate_card_prices_an_otherwise_unpriced_call() {
    let fixture = MediaFixture::start(MediaFixtureMode::silent()).await;
    // Keyed on the family prefix, not on one exact model id: the resolved
    // model comes from `OPENAI_IMAGE_MODEL` and must not make this assertion
    // depend on the environment it runs in.
    let card = MediaRateCard::new([("OpenAI".to_string(), 0.08)].into_iter().collect());
    let (tool, _ledger) = builtin_tool(&fixture, card);

    let result = tool.execute(json!({"prompt": "a lighthouse"})).await;
    assert!(!result.is_error, "{}", result.content);

    let acct = parse(&result.content)["accounting"].clone();
    assert_eq!(acct["cost_usd"], 0.08);
    assert_eq!(acct["price_source"]["kind"], "local_rate_card");
    assert_eq!(acct["price_source"]["entry"], "OpenAI");
    assert_ne!(
        acct["price_source"]["kind"], "provider_header",
        "an operator estimate must never be labelled provider-reported"
    );
}

/// A refused generation is accounted, and explicitly NOT as `$0.00` — the
/// provider may still have billed for the rejected prompt.
#[tokio::test]
async fn builtin_shape_refusal_is_accounted_as_billing_unknown_not_zero() {
    let fixture = MediaFixture::start(MediaFixtureMode::silent()).await;
    let (tool, ledger) = builtin_tool(&fixture, MediaRateCard::default());

    let result = tool
        .execute(json!({"prompt": format!("{REJECT_SENTINEL} please")}))
        .await;
    assert!(result.is_error, "refusal must surface as an error result");
    assert_reached_image_endpoint(&fixture, 1);

    let out = parse(&result.content);
    assert_eq!(
        out["errorCategory"], "prompt_rejected",
        "failure must carry a comparable category, got: {out}"
    );
    let acct = &out["accounting"];
    assert_eq!(acct["outcome"]["status"], "failed");
    assert_eq!(acct["outcome"]["category"], "prompt_rejected");
    assert_eq!(acct["price_source"]["kind"], "unpriced");
    assert_eq!(
        acct["price_source"]["reason"], "call_failed_billing_unknown",
        "a refused call must not be recorded as free"
    );
    assert!(acct.get("cost_usd").is_none());

    // The failed call still appears in the ledger. A user asking "what did
    // this session cost" must see that a billable attempt was made.
    assert_eq!(ledger.summary().calls, 1);
}

/// The 402 family maps to its own category, so "out of credit" is
/// distinguishable from "prompt refused" in the record.
#[tokio::test]
async fn builtin_shape_insufficient_credits_has_its_own_category() {
    let fixture = MediaFixture::start(MediaFixtureMode::silent()).await;
    let (tool, _ledger) = builtin_tool(&fixture, MediaRateCard::default());

    let result = tool
        .execute(json!({"prompt": format!("{CREDITS_SENTINEL} please")}))
        .await;
    assert!(result.is_error);
    let out = parse(&result.content);
    assert_eq!(out["errorCategory"], "insufficient_credits");
    assert_eq!(
        out["accounting"]["outcome"]["category"],
        "insufficient_credits"
    );
}

/// A bad credential must fail closed and be visible as such. This is the
/// credentials dimension of F27-C3 for the built-in shape.
#[tokio::test]
async fn builtin_shape_bad_credential_fails_closed_and_is_categorised() {
    let fixture = MediaFixture::start(MediaFixtureMode::silent()).await;
    let backend = wcore_agent::tool_backends::image_gen::DalleBackend::new(
        "not-the-fixture-token".to_string(),
        &fixture.api_base(),
        None,
    );
    let tool = ImageGenerationTool::with_backend(Arc::new(backend));

    let result = tool.execute(json!({"prompt": "a lighthouse"})).await;
    assert!(result.is_error, "a rejected key must not produce an image");
    let out = parse(&result.content);
    assert!(
        out["error"].as_str().unwrap_or_default().contains("401"),
        "the HTTP status must reach the user, got: {out}"
    );
    assert_reached_image_endpoint(&fixture, 1);
}

// ---------------------------------------------------------------------------
// MCP shapes
// ---------------------------------------------------------------------------

fn mcp_config(fixture: &MediaFixture) -> HashMap<String, McpServerConfig> {
    let mut servers = HashMap::new();
    servers.insert(
        "f27_media".to_string(),
        McpServerConfig {
            transport: TransportType::StreamableHttp,
            command: None,
            args: None,
            env: None,
            url: Some(fixture.mcp_url()),
            headers: None,
            deferred: Some(false),
            // The fixture is the user's own loopback server, which is exactly
            // the case this flag exists for.
            allow_local: true,
            only_for_assistant: None,
            allowed_tools: None,
        },
    );
    servers
}

/// **MCP-only shape.** The media capability exists solely as an MCP tool. It
/// must be discoverable and callable through the real client, and the call
/// must actually cross the wire.
#[tokio::test]
async fn mcp_only_shape_media_tool_is_discoverable_and_callable() {
    let fixture = MediaFixture::start(MediaFixtureMode::silent()).await;
    let manager = McpManager::connect_all(&mcp_config(&fixture))
        .await
        .expect("connect to the media fixture");

    let names: Vec<String> = manager.all_tools().into_iter().map(|(n, _)| n).collect();
    assert!(
        manager.has_tool_name("mcp_image_generate"),
        "media tool not discovered; tools were {names:?}"
    );

    let outcome = manager
        .call_tool(
            "f27_media",
            "mcp_image_generate",
            json!({"prompt": "a lighthouse", "aspect_ratio": "square"}),
        )
        .await
        .expect("transport-level success");
    assert!(!outcome.is_error, "call reported error: {}", outcome.text);
    let payload = parse(&outcome.text);
    assert_eq!(payload["success"], true);
    assert_eq!(payload["width"], 1024);
    assert!(
        payload["image"]
            .as_str()
            .unwrap_or_default()
            .starts_with("data:image/png;base64,"),
        "expected an image payload, got {payload}"
    );

    // Prove it went over the wire and completed the real handshake.
    let hits = fixture.hits();
    assert!(
        hits.iter().any(|h| h == "/mcp:initialize"),
        "hits: {hits:?}"
    );
    assert!(
        hits.iter().any(|h| h == "/mcp:tools/list"),
        "hits: {hits:?}"
    );
    assert!(
        hits.iter().any(|h| h == "/mcp:tools/call"),
        "hits: {hits:?}"
    );
}

/// **MCP credentials dimension.** An MCP media tool with no credential
/// configured must fail visibly rather than silently returning nothing —
/// and the failure arrives as `isError` on a *successful* transport call,
/// which is where the MCP shape genuinely differs from the built-in.
#[tokio::test]
async fn mcp_shape_missing_credential_fails_closed_and_is_visible() {
    let mode = MediaFixtureMode {
        mcp_requires_credential: true,
        ..MediaFixtureMode::silent()
    };
    let fixture = MediaFixture::start(mode).await;
    let manager = McpManager::connect_all(&mcp_config(&fixture))
        .await
        .expect("connect");

    let outcome = manager
        .call_tool("f27_media", "mcp_image_generate", json!({"prompt": "x"}))
        .await
        .expect("transport success even though the tool failed");
    assert!(
        outcome.is_error,
        "an uncredentialed media call must not report success: {}",
        outcome.text
    );
    let payload = parse(&outcome.text);
    assert_eq!(payload["errorCategory"], "no_provider_configured");

    // Control: the SAME server, SAME call, with the credential supplied,
    // succeeds. Without this the assertion above would pass on a fixture that
    // is simply broken.
    let ok = manager
        .call_tool(
            "f27_media",
            "mcp_image_generate",
            json!({"prompt": "x", "api_key": FIXTURE_TOKEN}),
        )
        .await
        .expect("transport success");
    assert!(
        !ok.is_error,
        "credentialed call should succeed: {}",
        ok.text
    );
}

/// **MCP failure semantics.** The same refusal sentinel that the built-in
/// shape maps to `prompt_rejected` must be comparably labelled here, or
/// F27-C3's "consistent failures" clause is unanswerable.
#[tokio::test]
async fn mcp_shape_refusal_carries_the_same_failure_category_as_the_builtin() {
    let fixture = MediaFixture::start(MediaFixtureMode::silent()).await;
    let manager = McpManager::connect_all(&mcp_config(&fixture))
        .await
        .expect("connect");

    let outcome = manager
        .call_tool(
            "f27_media",
            "mcp_image_generate",
            json!({"prompt": format!("{REJECT_SENTINEL} please")}),
        )
        .await
        .expect("transport success");
    assert!(outcome.is_error);
    assert_eq!(parse(&outcome.text)["errorCategory"], "prompt_rejected");
}

/// **Combined shape.** An MCP server advertising `image_generate` collides
/// with the built-in tool of that name.
///
/// This test records the CURRENT behaviour rather than asserting a desired
/// one: the MCP client happily advertises the colliding name, and nothing at
/// this layer marks the collision. That is threat T-27-03-08 ("an MCP-supplied
/// media tool shadows a built-in one and the substitution is invisible to the
/// user") observed rather than assumed, and it is reported as an open finding
/// in the lane summary rather than quietly fixed here.
#[tokio::test]
async fn combined_shape_mcp_tool_may_shadow_the_builtin_name_without_a_marker() {
    let mode = MediaFixtureMode {
        mcp_tool_name: "image_generate".to_string(),
        ..MediaFixtureMode::silent()
    };
    let fixture = MediaFixture::start(mode).await;
    let manager = McpManager::connect_all(&mcp_config(&fixture))
        .await
        .expect("connect");

    assert!(
        manager.has_tool_name("image_generate"),
        "the MCP server should be able to advertise the built-in's name"
    );
    let (_server, def) = manager
        .all_tools()
        .into_iter()
        .find(|(_, d)| d.name == "image_generate")
        .expect("colliding tool present");
    // The advertised description is the FIXTURE's, not the built-in's — i.e.
    // an inspector can tell them apart only by reading the description, and
    // nothing in the tool definition itself flags the collision.
    assert!(
        def.description
            .as_deref()
            .unwrap_or_default()
            .contains("F27 hermetic media fixture"),
        "expected the MCP definition, got: {:?}",
        def.description
    );
}

/// **The honest negative, asserted so it cannot change silently.**
///
/// The product's media cost record is produced by the built-in tool. An
/// MCP-served media tool returns opaque text through the MCP proxy, so the
/// engine produces **no `MediaCostRecord` for it at all**. Accounting is
/// therefore NOT consistent across the four shapes, and this test pins that
/// gap rather than leaving it to be rediscovered.
///
/// If a later change teaches the MCP proxy to emit a record, this test fails
/// and must be updated — which is the point.
#[tokio::test]
async fn mcp_shape_produces_no_product_cost_record_today() {
    let fixture = MediaFixture::start(MediaFixtureMode::silent()).await;
    let manager = McpManager::connect_all(&mcp_config(&fixture))
        .await
        .expect("connect");

    let outcome = manager
        .call_tool("f27_media", "mcp_image_generate", json!({"prompt": "x"}))
        .await
        .expect("transport success");
    let payload = parse(&outcome.text);
    assert!(
        payload.get("accounting").is_none(),
        "an MCP media result carries no product cost record today; if it now \
         does, F27-C3's accounting-consistency clause has moved and this test \
         must be rewritten. Payload: {payload}"
    );

    // Control, in the same test, proving the assertion above is not passing
    // because the payload is empty or the call failed: the success fields ARE
    // present. A negative asserted against a dead instrument is worthless.
    assert_eq!(payload["success"], true);
    assert_eq!(payload["usedProvider"], "f27-media-fixture");
}
