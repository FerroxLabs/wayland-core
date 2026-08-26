//! FerroxLabs/wayland#863 — the Flux loop-ownership anti-collision contract,
//! CLIENT half, driven end-to-end over real HTTP.
//!
//! These tests deliberately go through `LlmProvider::stream` against a
//! `wiremock` backend and then assert on the request the server ACTUALLY
//! received, rather than calling the header/body helpers directly. A guard that
//! is perfectly unit-tested and then never called is the failure mode this
//! contract already suffered once: #247 shipped a "nested-ladder guard" that
//! turned out to push a note onto a string vector and change no behaviour at
//! all. Asserting on the wire is the only way to prove the wiring.
//!
//! Coverage map — the three translation paths a driver-seat turn can take:
//!   * OpenAI chat completions  (`/v1/chat/completions`) — the Flux route
//!   * OpenAI Responses         (`/v1/responses`)
//!   * Anthropic Messages       (`/v1/messages`)

use serde_json::{Value, json};
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;
use wcore_providers::flux_loop;
use wcore_providers::openai::OpenAIProvider;
use wcore_types::llm::{ANVIL_LOOP_OWNER, FluxLoopIntent, LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A minimal turn. `model` is a CONCRETE model id, not a Flux tier alias —
/// several tests below depend on that, because F2 says Flux honours
/// `loop_owner` regardless of alias.
fn req(model: &str, intent: Option<FluxLoopIntent>, nonce: Option<&str>) -> LlmRequest {
    LlmRequest {
        model: model.to_string(),
        system: "sys".to_string(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        )],
        max_tokens: 64,
        flux_loop_intent: intent,
        flux_turn_nonce: nonce.map(str::to_string),
        ..Default::default()
    }
}

/// An endpoint that speaks the #863 handshake (what `flux_router_defaults`
/// resolves to in production, via `ProviderType::FluxRouter`).
fn flux_compat() -> ProviderCompat {
    ProviderCompat::flux_router_defaults()
}

/// A plain OpenAI endpoint — does NOT speak the handshake.
fn plain_openai_compat() -> ProviderCompat {
    ProviderCompat::openai_defaults()
}

fn ok_sse() -> String {
    let mut b = String::new();
    b.push_str("data: ");
    b.push_str(r#"{"choices":[{"delta":{"content":"ok"},"index":0}]}"#);
    b.push_str("\n\n");
    b.push_str("data: ");
    b.push_str(
        r#"{"choices":[{"delta":{},"index":0,"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
    );
    b.push_str("\n\n");
    b.push_str("data: [DONE]\n\n");
    b
}

async fn mount_ok(server: &MockServer, p: &str) {
    Mock::given(method("POST"))
        .and(path(p))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(ok_sse()),
        )
        .mount(server)
        .await;
}

/// Drain the stream so the request is definitely complete, then return the
/// single request the mock server received: its headers and its JSON body.
async fn captured(server: &MockServer) -> (wiremock::http::HeaderMap, Value) {
    let reqs = server
        .received_requests()
        .await
        .expect("wiremock is recording requests");
    assert!(
        !reqs.is_empty(),
        "the provider must have sent at least one request"
    );
    let r = &reqs[0];
    let body: Value = serde_json::from_slice(&r.body).expect("request body is JSON");
    (r.headers.clone(), body)
}

fn header<'a>(h: &'a wiremock::http::HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

async fn drain(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut out = Vec::new();
    while let Some(e) = rx.recv().await {
        out.push(e);
    }
    out
}

// ---------------------------------------------------------------------------
// F2 — OpenAI chat completions (the Flux route)
// ---------------------------------------------------------------------------

/// The core deliverable, on the wire: a driver-seat turn against an endpoint
/// that speaks the handshake carries `X-Flux-Loop-Owner: anvil` AND
/// `metadata.loop_owner`, on a CONCRETE model id.
///
/// The concrete model id is the point. The #282 `x-wl-*` headers gate on
/// `is_flux_tier_alias`, so a copy of that gate here would emit nothing for
/// this request — and F2 is explicit that Flux honours `loop_owner` regardless
/// of alias. A driver pinned to a concrete upstream is exactly the case where a
/// silent drop makes a collision undetectable from either side.
#[tokio::test]
async fn chat_wire_carries_loop_owner_on_a_concrete_model_id() {
    let server = MockServer::start().await;
    mount_ok(&server, "/chat/completions").await;

    let p = OpenAIProvider::new("k", &server.uri(), flux_compat(), DebugConfig::default());
    let r = req(
        "claude-sonnet-4-5",
        Some(FluxLoopIntent::ClientOwned(ANVIL_LOOP_OWNER.to_string())),
        Some("conv-1:7"),
    );
    let _ = drain(p.stream(&r).await.expect("stream")).await;

    let (h, body) = captured(&server).await;
    assert_eq!(
        header(&h, "x-flux-loop-owner"),
        Some("anvil"),
        "a driver-seat turn must carry the loop-owner header"
    );
    assert_eq!(
        body["metadata"]["loop_owner"],
        json!("anvil"),
        "and the metadata carrier, so a header-stripping proxy cannot silently \
         turn a marked turn into an unmarked one"
    );
    // F3 — per-turn cache variance rides with the marking.
    assert_eq!(body["metadata"]["nonce"], json!("conv-1:7"));
    // F5 — never the opt-in, on driver traffic.
    assert!(header(&h, "x-flux-verify").is_none());
    assert!(body["metadata"].get("flux_verify").is_none());
}

/// The endpoint gate. The SAME marked request against a plain OpenAI endpoint
/// emits nothing at all: no header, and a body with no `metadata` key. Core
/// does not leak its internal loop state to an endpoint that has no contract to
/// honour it, and a strict OpenAI-compatible server (Ollama, llama.cpp, vLLM)
/// never sees an unknown top-level field it could 400 on.
#[tokio::test]
async fn non_declaring_endpoint_receives_nothing() {
    let server = MockServer::start().await;
    mount_ok(&server, "/v1/chat/completions").await;

    let p = OpenAIProvider::new(
        "k",
        &server.uri(),
        plain_openai_compat(),
        DebugConfig::default(),
    );
    let r = req(
        "gpt-4o",
        Some(FluxLoopIntent::ClientOwned(ANVIL_LOOP_OWNER.to_string())),
        Some("conv-1:7"),
    );
    let _ = drain(p.stream(&r).await.expect("stream")).await;

    let (h, body) = captured(&server).await;
    assert!(
        header(&h, "x-flux-loop-owner").is_none(),
        "a non-declaring endpoint must not receive the loop-owner header"
    );
    assert!(
        header(&h, "x-flux-verify").is_none(),
        "nor the verify opt-in"
    );
    assert!(
        body.get("metadata").is_none(),
        "nor any metadata object: the body must be byte-identical to today, got {body}"
    );
}

// ---------------------------------------------------------------------------
// F2 — OpenAI Responses translation
// ---------------------------------------------------------------------------

/// Desktop's explicit ask, half one: the marking survives the **Responses**
/// translation. `build_responses_body` is a completely separate body builder
/// from `build_request_body`, so this is a genuinely different code path and
/// not a re-test of the one above.
#[tokio::test]
async fn responses_translation_carries_loop_owner() {
    let server = MockServer::start().await;
    mount_ok(&server, "/responses").await;

    let mut compat = flux_compat();
    compat.uses_responses_api = Some(true);
    let p = OpenAIProvider::new("k", &server.uri(), compat, DebugConfig::default());
    let r = req(
        "some-reasoner",
        Some(FluxLoopIntent::ClientOwned(ANVIL_LOOP_OWNER.to_string())),
        Some("conv-2:3"),
    );
    let _ = drain(p.stream(&r).await.expect("stream")).await;

    let (h, body) = captured(&server).await;
    assert_eq!(
        header(&h, "x-flux-loop-owner"),
        Some("anvil"),
        "the Responses surface shares `try_send`, so the header must ride it too"
    );
    assert_eq!(
        body["metadata"]["loop_owner"],
        json!("anvil"),
        "the Responses body builder must emit the metadata carrier as well; got {body}"
    );
    assert_eq!(body["metadata"]["nonce"], json!("conv-2:3"));
}

// ---------------------------------------------------------------------------
// F2 — Anthropic Messages translation
// ---------------------------------------------------------------------------

/// Desktop's explicit ask, half two: the marking survives the **Anthropic**
/// translation.
///
/// HEADER ONLY, and that is the design, not a gap: the Anthropic Messages
/// `metadata` object accepts `user_id` and rejects arbitrary keys, so the
/// metadata carrier the OpenAI-wire paths use is not wire-legal here. The
/// header carrier is legal on every wire shape, which is why the F2 contract
/// names it. This test pins BOTH facts so a later change cannot quietly start
/// sending an illegal body.
#[tokio::test]
async fn anthropic_translation_carries_loop_owner_header_only() {
    let server = MockServer::start().await;
    mount_ok(&server, "/v1/messages").await;

    // An Anthropic-wire endpoint that declares the handshake.
    let mut compat = ProviderCompat::anthropic_defaults();
    compat.flux_loop_provenance = Some(true);
    let p = AnthropicProvider::new("k", &server.uri(), compat, DebugConfig::default());
    let r = req(
        "claude-sonnet-4-5",
        Some(FluxLoopIntent::ClientOwned(ANVIL_LOOP_OWNER.to_string())),
        Some("conv-3:1"),
    );
    let _ = drain(p.stream(&r).await.expect("stream")).await;

    let (h, body) = captured(&server).await;
    assert_eq!(
        header(&h, "x-flux-loop-owner"),
        Some("anvil"),
        "the Anthropic translation must not drop the marking"
    );
    assert!(
        body.get("metadata").is_none(),
        "and must NOT invent an Anthropic `metadata` object, which only accepts \
         `user_id` and would 400 on arbitrary keys; got {body}"
    );
}

/// Real api.anthropic.com — `anthropic_defaults` does not declare the
/// handshake, so a marked turn reaches it byte-identical.
#[tokio::test]
async fn anthropic_default_endpoint_receives_nothing() {
    let server = MockServer::start().await;
    mount_ok(&server, "/v1/messages").await;

    let p = AnthropicProvider::new(
        "k",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    );
    let r = req(
        "claude-sonnet-4-5",
        Some(FluxLoopIntent::ClientOwned(ANVIL_LOOP_OWNER.to_string())),
        None,
    );
    let _ = drain(p.stream(&r).await.expect("stream")).await;

    let (h, body) = captured(&server).await;
    assert!(header(&h, "x-flux-loop-owner").is_none());
    assert!(body.get("metadata").is_none());
}

// ---------------------------------------------------------------------------
// F1 / F5 — the explicit opt-in, and its mutual exclusion
// ---------------------------------------------------------------------------

/// The opt-in surface Flux shipped (their C2 decision): `flux_verify` lets
/// Elevation run on `flux-auto`. It must be expressible...
#[tokio::test]
async fn server_verify_emits_the_opt_in_and_never_a_loop_owner() {
    let server = MockServer::start().await;
    mount_ok(&server, "/chat/completions").await;

    let p = OpenAIProvider::new("k", &server.uri(), flux_compat(), DebugConfig::default());
    let r = req("flux-auto", Some(FluxLoopIntent::ServerVerify), None);
    let _ = drain(p.stream(&r).await.expect("stream")).await;

    let (h, body) = captured(&server).await;
    assert_eq!(header(&h, "x-flux-verify"), Some("true"));
    assert_eq!(body["metadata"]["flux_verify"], json!(true));
    assert!(
        header(&h, "x-flux-loop-owner").is_none(),
        "opting into the server ladder must never also claim Core owns the loop"
    );
    assert!(body["metadata"].get("loop_owner").is_none());
}

/// ...and it must be UNREACHABLE from driver traffic. This is F5.
///
/// The strong form of that guarantee is structural: `FluxLoopIntent` has no
/// representable state carrying both arms, so no amount of engine code can set
/// `flux_verify` on a turn that declared `loop_owner`. This test pins the wire
/// consequence of that type choice, which is what a reviewer can actually check.
#[test]
fn client_owned_and_server_verify_are_mutually_exclusive_by_construction() {
    let owned = FluxLoopIntent::ClientOwned(ANVIL_LOOP_OWNER.to_string());
    assert_eq!(owned.owner(), Some("anvil"));
    assert!(
        !owned.is_server_verify(),
        "a client-owned turn can never also be a server-verify turn"
    );

    let verify = FluxLoopIntent::ServerVerify;
    assert!(verify.is_server_verify());
    assert_eq!(
        verify.owner(),
        None,
        "a server-verify turn can never carry a loop owner"
    );
}

// ---------------------------------------------------------------------------
// F3 — the nonce rides with the marking, and only with it
// ---------------------------------------------------------------------------

/// An UNMARKED turn gets no nonce even when one is present on the request.
///
/// This is a cost guard, not a nicety: a nonce on ordinary traffic would vary
/// the semantic cache for every turn in the workspace, which is a bill, not a
/// fix. F3 only asks for variance on loop traffic.
#[tokio::test]
async fn nonce_rides_only_with_the_marking() {
    let server = MockServer::start().await;
    mount_ok(&server, "/chat/completions").await;

    let p = OpenAIProvider::new("k", &server.uri(), flux_compat(), DebugConfig::default());
    let r = req("gpt-4o", None, Some("conv-9:9"));
    let _ = drain(p.stream(&r).await.expect("stream")).await;

    let (_h, body) = captured(&server).await;
    assert!(
        body.get("metadata").is_none(),
        "an unmarked turn must carry no metadata object at all; got {body}"
    );
}

// ---------------------------------------------------------------------------
// F2 — the runtime collision detector
// ---------------------------------------------------------------------------

/// The `x-flux-loop-engaged` response echo reaches the engine as
/// `LlmEvent::ProviderMeta.loop_engaged`.
#[tokio::test]
async fn loop_engaged_response_header_surfaces_as_provider_meta() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .insert_header("x-flux-loop-engaged", "elevation")
                .set_body_string(ok_sse()),
        )
        .mount(&server)
        .await;

    let p = OpenAIProvider::new("k", &server.uri(), flux_compat(), DebugConfig::default());
    let r = req(
        "flux-auto",
        Some(FluxLoopIntent::ClientOwned(ANVIL_LOOP_OWNER.to_string())),
        None,
    );
    let events = drain(p.stream(&r).await.expect("stream")).await;

    let engaged = events.iter().find_map(|e| match e {
        LlmEvent::ProviderMeta { loop_engaged, .. } => loop_engaged.clone(),
        _ => None,
    });
    assert_eq!(
        engaged.as_deref(),
        Some("elevation"),
        "the loop-engaged echo must reach the engine; events: {events:?}"
    );
}

/// The predicate itself, as a truth table.
///
/// Note what is deliberately NOT a collision. `cascade` is permitted by F1
/// (single-tier climb-on-failure, per-request, origin-tier billed). An ABSENT
/// header is not a collision either — no non-Flux endpoint sends one, and
/// treating silence as a fault would fail every Anthropic turn in the
/// workspace. Getting this polarity wrong in the safe direction is a dead
/// detector; getting it wrong in the unsafe direction breaks every session.
#[test]
fn collision_truth_table() {
    let owned = Some("anvil");
    let unowned: Option<&str> = None;

    assert!(
        flux_loop::collides(owned, Some("elevation")),
        "owner + elevation is THE collision"
    );
    assert!(
        flux_loop::collides(owned, Some("ELEVATION")),
        "header values are case-insensitive on the wire"
    );
    assert!(
        !flux_loop::collides(owned, Some("cascade")),
        "F1 permits Cascade's single-tier climb-on-failure"
    );
    assert!(!flux_loop::collides(owned, Some("none")));
    assert!(
        !flux_loop::collides(owned, None),
        "a non-Flux endpoint sends no echo; silence must not fault the turn"
    );
    assert!(
        !flux_loop::collides(unowned, Some("elevation")),
        "Elevation on traffic Core does not own is Flux doing its job"
    );
    assert!(!flux_loop::collides(unowned, None));
}

/// `is_collision` reads the same predicate straight off a request, so the
/// provider-side and engine-side forms cannot drift.
#[test]
fn is_collision_matches_collides() {
    let marked = req(
        "flux-auto",
        Some(FluxLoopIntent::ClientOwned(ANVIL_LOOP_OWNER.to_string())),
        None,
    );
    let verify = req("flux-auto", Some(FluxLoopIntent::ServerVerify), None);
    let bare = req("flux-auto", None, None);

    assert!(flux_loop::is_collision(&marked, Some("elevation")));
    assert!(!flux_loop::is_collision(&marked, Some("cascade")));
    assert!(
        !flux_loop::is_collision(&verify, Some("elevation")),
        "a turn that ASKED for the server ladder cannot collide with it"
    );
    assert!(!flux_loop::is_collision(&bare, Some("elevation")));
}

// ---------------------------------------------------------------------------
// The gate must be reachable in production
// ---------------------------------------------------------------------------

/// `ProviderCompat::merge` enumerates its fields by hand. A field left out of
/// it is silently dropped from every merged compat, which would leave this
/// contract's gate permanently false in production while its preset-reading
/// unit tests stayed green — a gate that cannot pass is exactly as useless as
/// one that cannot fail.
#[test]
fn flux_loop_provenance_survives_compat_merge() {
    let merged = ProviderCompat::merge(
        ProviderCompat::flux_router_defaults(),
        ProviderCompat::default(),
    );
    assert!(
        merged.flux_loop_provenance(),
        "the Flux preset's opt-in must survive a merge with an empty user compat"
    );

    // And a user can still turn it off explicitly.
    let off = ProviderCompat {
        flux_loop_provenance: Some(false),
        ..ProviderCompat::default()
    };
    assert!(
        !ProviderCompat::merge(ProviderCompat::flux_router_defaults(), off).flux_loop_provenance()
    );
}

/// Positive/negative control on the preset itself: exactly one preset declares
/// the handshake today.
#[test]
fn only_the_flux_preset_declares_the_handshake() {
    assert!(ProviderCompat::flux_router_defaults().flux_loop_provenance());
    for other in [
        ProviderCompat::openai_defaults(),
        ProviderCompat::anthropic_defaults(),
        ProviderCompat::openrouter_defaults(),
        ProviderCompat::bedrock_defaults(),
        ProviderCompat::vertex_defaults(),
        ProviderCompat::default(),
    ] {
        assert!(
            !other.flux_loop_provenance(),
            "no non-Flux preset may declare the loop-ownership handshake"
        );
    }
}
