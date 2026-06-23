//! End-to-end test of the "Sign in with ChatGPT" (Codex OAuth) path, mocking
//! BOTH OpenAI endpoints with local wiremock servers:
//!
//! 1. `POST /oauth/token` (the `auth.openai.com` token endpoint) — returns a
//!    fresh `access_token` (a fixture JWT carrying `chatgpt_account_id`) +
//!    rotated `refresh_token`.
//! 2. `POST /responses` (the `chatgpt.com/backend-api/codex` inference
//!    endpoint) — returns a Responses-API SSE stream.
//!
//! The wiring exactly mirrors what `wcore-agent/src/bootstrap.rs` does for a
//! `ProviderType::OpenAIChatGpt` config (revision E of the plan): build a
//! [`ChatGptTokenManager`] over an [`OAuthStorage`] rooted at a tempdir, wrap
//! it in an [`AsyncBearerSource`] closure that calls `mgr.get()` per stream,
//! and construct an [`OpenAIChatGptProvider`] directly. We then drive the
//! provider's `stream()` and assert the engine-visible `LlmEvent`s.
//!
//! The real `auth.openai.com` / `chatgpt.com` hosts are never contacted — the
//! token endpoint is overridden via the `#[doc(hidden)]`
//! `ChatGptTokenManager::new_with_flow` seam, and the Codex backend via
//! `OpenAIChatGptProvider::with_base_url`. The live multi-turn / encrypted
//! -reasoning verification (spec §4, D2) is performed by a human against the
//! real backend; this file mocks everything.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::json;
use tempfile::TempDir;

use wcore_agent::oauth::chatgpt::{
    AUTHORIZE_URL, CALLBACK_HOST, CALLBACK_PATH, CALLBACK_PORT, CLIENT_ID, PROVIDER, SCOPES,
};
use wcore_agent::oauth::{
    ChatGptTokenManager, OAuthFlow, OAuthStorage, OAuthTokens, RedirectStrategy,
};

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::openai_chatgpt::{AsyncBearerSource, BearerCreds};
use wcore_providers::{LlmProvider, OpenAIChatGptProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role, StopReason};

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── fixtures ──────────────────────────────────────────────────────────────

/// A 3-segment JWT whose payload decodes to the given ChatGPT account id (and,
/// optionally, a top-level `exp`). The signature segment is a placeholder —
/// `decode_codex_claims` does NOT verify signatures (the token is already
/// trusted; only claims are read).
fn jwt_with_account(account_id: &str) -> String {
    let payload = json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": account_id }
    });
    let seg = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("hdr.{seg}.sig")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn token(access: &str, refresh: Option<&str>, expires_at: Option<u64>) -> OAuthTokens {
    OAuthTokens {
        access_token: access.to_string(),
        refresh_token: refresh.map(str::to_string),
        expires_at_unix_secs: expires_at,
        token_type: "Bearer".into(),
        scope: None,
        id_token: None,
    }
}

/// Build a ChatGPT OAuth flow descriptor whose token endpoint points at a mock
/// server. Mirrors `build_chatgpt_flow` except for the overridden token URL.
fn flow_with_token_url(token_url: &str) -> OAuthFlow {
    OAuthFlow::new(
        CLIENT_ID,
        None,
        AUTHORIZE_URL,
        token_url,
        SCOPES.iter().map(|s| s.to_string()).collect(),
    )
    .with_redirect_strategy(RedirectStrategy::FixedPort(CALLBACK_PORT))
    .with_redirect_uri_parts(CALLBACK_HOST, CALLBACK_PATH)
}

/// The exact `AsyncBearerSource` closure bootstrap builds: it calls
/// `mgr.get()` (load → refresh-if-near-expiry → JWT-decode) per `stream()`,
/// mapping a manager error to `ProviderError::Connection`.
fn bearer_over_manager(mgr: Arc<ChatGptTokenManager>) -> AsyncBearerSource {
    Arc::new(move || {
        let mgr = mgr.clone();
        Box::pin(async move {
            let (access_token, account_id) = mgr.get().await.map_err(ProviderError::Connection)?;
            Ok(BearerCreds {
                access_token,
                account_id,
            })
        })
    })
}

fn make_request() -> LlmRequest {
    LlmRequest {
        model: "gpt-5.5".to_string(),
        system: "You are a test assistant.".to_string(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        )],
        max_tokens: 512,
        ..Default::default()
    }
}

/// Build a Codex Responses SSE body from typed JSON frames. The Responses
/// stream has no `[DONE]` sentinel — the terminal frame IS the success frame.
fn build_responses_sse(frames: &[&str]) -> String {
    let mut body = String::new();
    for f in frames {
        body.push_str("data: ");
        body.push_str(f);
        body.push_str("\n\n");
    }
    body
}

async fn collect_events(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

/// Mount the Codex `/responses` SSE mock on `server` and return the
/// provider-ready `OpenAIChatGptProvider` pointed at it, with `bearer`.
fn provider_against(server_uri: String, bearer: AsyncBearerSource) -> OpenAIChatGptProvider {
    OpenAIChatGptProvider::new(bearer, ProviderCompat::default(), DebugConfig::default())
        .with_base_url(server_uri)
}

// ── tests ─────────────────────────────────────────────────────────────────

/// Full path with a FRESH stored token: the bearer closure loads the token
/// from disk (no refresh round-trip), decodes the account id from its JWT, and
/// the provider streams a Codex turn. Asserts `TextDelta` then `Done`, and that
/// the OAuth account id reaches the Codex backend as the `chatgpt-account-id`
/// header. The token endpoint is intentionally NOT mounted — a fresh token
/// must not hit it.
#[tokio::test]
async fn end_to_end_fresh_token_streams_codex_turn() {
    // The stored access_token is a JWT (the account id is decoded from it), so
    // it is ALSO the literal bearer the provider sends. Compute it first so the
    // Codex mock can assert the exact `Authorization` header.
    let jwt = jwt_with_account("acct_e2e_fresh");

    let codex = MockServer::start().await;
    let delta = r#"{"type":"response.output_text.delta","delta":"Hi from Codex"}"#;
    let completed = r#"{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":9,"output_tokens":4}}}"#;
    let sse_body = build_responses_sse(&[delta, completed]);

    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", format!("Bearer {jwt}").as_str()))
        .and(header("chatgpt-account-id", "acct_e2e_fresh"))
        .and(header("openai-beta", "responses=experimental"))
        .and(header("originator", "wayland"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .expect(1)
        .mount(&codex)
        .await;

    // Seed a fresh (non-expired) token whose access_token JWT carries the
    // account id. Storage is rooted at a tempdir, exactly like bootstrap's
    // OAuthStorage but isolated from the real ~/.wayland. No token endpoint is
    // mounted — a fresh token must be served from disk without a refresh.
    let tmp = TempDir::new().unwrap();
    let storage = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
    storage
        .store(
            PROVIDER,
            &token(&jwt, Some("rt-fresh"), Some(now_secs() + 3600)),
        )
        .unwrap();

    let mgr = Arc::new(ChatGptTokenManager::new(storage));
    let provider = provider_against(codex.uri(), bearer_over_manager(mgr));

    let rx = provider
        .stream(&make_request())
        .await
        .expect("stream opens");
    let events = collect_events(rx).await;

    assert_eq!(events.len(), 2, "events: {events:?}");
    match &events[0] {
        LlmEvent::TextDelta(t) => assert_eq!(t, "Hi from Codex"),
        e => panic!("expected TextDelta, got {e:?}"),
    }
    match &events[1] {
        LlmEvent::Done {
            stop_reason, usage, ..
        } => {
            assert_eq!(*stop_reason, StopReason::EndTurn);
            assert_eq!(usage.input_tokens, 9);
            assert_eq!(usage.output_tokens, 4);
        }
        e => panic!("expected Done, got {e:?}"),
    }
}

/// Full path with an EXPIRED stored token: the bearer closure triggers a
/// refresh against the mock `/oauth/token` endpoint, which returns a NEW
/// access_token (a fresh JWT) + rotated refresh_token; the provider then
/// streams the Codex turn carrying the refreshed bearer + decoded account id.
/// Also asserts the rotated refresh token was persisted to disk.
#[tokio::test]
async fn end_to_end_expired_token_refreshes_then_streams() {
    // 1) Mock the token endpoint: an expired stored token forces a refresh.
    let auth = MockServer::start().await;
    let refreshed_jwt = jwt_with_account("acct_e2e_refreshed");
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": refreshed_jwt,
            "refresh_token": "rt-ROTATED",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .expect(1)
        .mount(&auth)
        .await;

    // 2) Mock the Codex backend, expecting the REFRESHED bearer + account id.
    let codex = MockServer::start().await;
    let delta = r#"{"type":"response.output_text.delta","delta":"after refresh"}"#;
    let completed = r#"{"type":"response.completed","response":{"id":"resp_2","status":"completed","usage":{"input_tokens":3,"output_tokens":2}}}"#;
    let sse_body = build_responses_sse(&[delta, completed]);
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header(
            "authorization",
            format!("Bearer {refreshed_jwt}").as_str(),
        ))
        .and(header("chatgpt-account-id", "acct_e2e_refreshed"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .expect(1)
        .mount(&codex)
        .await;

    // 3) Seed an EXPIRED token (exp=0) → manager refreshes on get().
    let tmp = TempDir::new().unwrap();
    let storage = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
    storage
        .store(
            PROVIDER,
            &token(&jwt_with_account("acct_old"), Some("rt-OLD"), Some(0)),
        )
        .unwrap();

    // Build the manager with the token endpoint pointed at the auth mock — the
    // out-of-crate test seam (production uses the real auth.openai.com URL).
    let flow = flow_with_token_url(&format!("{}/oauth/token", auth.uri()));
    let mgr = Arc::new(ChatGptTokenManager::new_with_flow(storage, flow));

    let provider = provider_against(codex.uri(), bearer_over_manager(mgr.clone()));
    let events = collect_events(provider.stream(&make_request()).await.expect("stream")).await;

    // Codex turn streamed using the refreshed credentials.
    assert!(
        matches!(&events[0], LlmEvent::TextDelta(t) if t == "after refresh"),
        "events: {events:?}"
    );
    assert!(
        matches!(events.last(), Some(LlmEvent::Done { .. })),
        "events: {events:?}"
    );

    // The rotated single-use refresh token must have been persisted (C4).
    let on_disk = OAuthStorage::at_root(tmp.path().join("oauth"))
        .unwrap()
        .load(PROVIDER)
        .unwrap()
        .expect("token present");
    assert_eq!(on_disk.refresh_token.as_deref(), Some("rt-ROTATED"));
    assert_eq!(on_disk.access_token, refreshed_jwt);
}

/// The Codex terminal-success alias `response.done` (D1) closes the stream
/// just like `response.completed`, end-to-end through the OAuth path — a Codex
/// turn that terminates on `response.done` must NOT surface as a truncation
/// error.
#[tokio::test]
async fn end_to_end_response_done_terminal_closes_cleanly() {
    let codex = MockServer::start().await;
    let delta = r#"{"type":"response.output_text.delta","delta":"done-frame text"}"#;
    let done = r#"{"type":"response.done","response":{"status":"completed","usage":{"input_tokens":1,"output_tokens":1}}}"#;
    let sse_body = build_responses_sse(&[delta, done]);
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse_body, "text/event-stream"))
        .expect(1)
        .mount(&codex)
        .await;

    let tmp = TempDir::new().unwrap();
    let storage = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
    storage
        .store(
            PROVIDER,
            &token(
                &jwt_with_account("acct_done"),
                Some("rt"),
                Some(now_secs() + 3600),
            ),
        )
        .unwrap();

    let mgr = Arc::new(ChatGptTokenManager::new(storage));
    let provider = provider_against(codex.uri(), bearer_over_manager(mgr));

    let events = collect_events(provider.stream(&make_request()).await.expect("stream")).await;

    assert!(
        !events.iter().any(|e| matches!(e, LlmEvent::Error(_))),
        "stream must close cleanly on response.done: {events:?}"
    );
    assert!(
        matches!(events.last(), Some(LlmEvent::Done { .. })),
        "expected terminal Done: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, LlmEvent::TextDelta(t) if t == "done-frame text")),
        "expected the streamed text delta: {events:?}"
    );
}

/// When no token is stored at all, the bearer closure surfaces the manager's
/// "not signed in" guidance as a `ProviderError::Connection` BEFORE any HTTP
/// request is attempted (the Codex mock is mounted with `expect(0)`).
#[tokio::test]
#[serial_test::serial]
async fn end_to_end_no_token_surfaces_login_guidance() {
    let codex = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string("unreachable"))
        .expect(0)
        .mount(&codex)
        .await;

    let tmp = TempDir::new().unwrap();
    let storage = OAuthStorage::at_root(tmp.path().join("oauth")).unwrap();
    // Nothing stored. Isolate CODEX_HOME to an empty dir so the #293 Codex-CLI
    // fallback can't backfill the empty store from a real host login (CI runners
    // may carry a live ~/.codex/auth.json). Restored before the assertions so a
    // failure can't leak the env var to other serial tests.
    let codex_home = tmp.path().join("codex-empty");
    std::fs::create_dir_all(&codex_home).unwrap();
    let saved_codex_home = std::env::var_os("CODEX_HOME");
    // SAFETY: serial test; reverted below before any assertion.
    unsafe { std::env::set_var("CODEX_HOME", &codex_home) };

    let mgr = Arc::new(ChatGptTokenManager::new(storage));
    let provider = provider_against(codex.uri(), bearer_over_manager(mgr));

    let result = provider.stream(&make_request()).await;

    match saved_codex_home {
        Some(v) => unsafe { std::env::set_var("CODEX_HOME", v) },
        None => unsafe { std::env::remove_var("CODEX_HOME") },
    }

    let err = result.expect_err("missing token must error before any HTTP call");
    match err {
        ProviderError::Connection(msg) => {
            assert!(msg.contains("not signed in"), "msg={msg}");
        }
        other => panic!("expected Connection error, got {other:?}"),
    }
}
