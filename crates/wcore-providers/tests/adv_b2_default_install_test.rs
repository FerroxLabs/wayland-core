//! ADVERSARIAL (B-2 round 2): the open-circuit posture change, proven on the
//! DEFAULT INSTALL PATH.
//!
//! Round 1's breaker bypass was reviewed by reading the condition
//! `fallbacks.is_empty()` and calling it narrow. It was universal, because
//! `create_provider` always passes `Vec::new()`. Round 2's tests all build a
//! `ResilientProvider` by hand, so the same reading gap is still open: nothing
//! drives `create_provider(&Config)` itself.
//!
//! These tests go through `wcore_providers::create_provider` — the constructor
//! `AgentEngine::new`, `bootstrap` and every rebind use — and assert the
//! posture by OBSERVED OUTCOME, never by inspecting the guard.

use std::sync::Arc;
use std::time::Duration;

use wcore_config::config::{Config, ProviderType};
use wcore_providers::{LlmProvider, ProviderError, create_provider};
use wcore_types::llm::LlmRequest;
use wcore_types::message::{ContentBlock, Message, Role};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn request() -> LlmRequest {
    LlmRequest {
        messages: vec![Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
        )],
        ..Default::default()
    }
}

/// Exactly the shape `create_provider` sees on a machine where the user typed
/// one API key into onboarding: one provider, no `fallback_models`.
fn default_install(base_url: String) -> Config {
    Config {
        provider: ProviderType::OpenAI,
        provider_label: "openai".to_string(),
        api_key: "sk-adversarial-not-a-real-key".to_string(),
        base_url,
        model: "gpt-probe".to_string(),
        ..Config::default()
    }
}

async fn drive(p: &Arc<dyn LlmProvider>, n: usize) -> Vec<Result<(), ProviderError>> {
    let mut out = Vec::new();
    for _ in 0..n {
        out.push(p.stream(&request()).await.map(|_| ()));
    }
    out
}

/// A momentary outage (503) on a default install: once the breaker opens the
/// caller must still REACH the provider. If this returns `NotAttempted` the
/// B-2 repair does not exist on the path real users run.
#[tokio::test(flavor = "multi_thread")]
async fn default_install_momentary_outage_still_reaches_the_provider() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).set_body_string("{\"error\":\"overloaded\"}"))
        .mount(&server)
        .await;

    let provider = create_provider(&default_install(server.uri()));
    let results = drive(&provider, 6).await;

    let not_attempted = results
        .iter()
        .filter(|r| matches!(r, Err(ProviderError::NotAttempted { .. })))
        .count();
    assert_eq!(
        not_attempted, 0,
        "a default install must keep reaching the provider through a momentary \
         outage; saw {not_attempted} refusals in {results:?}"
    );
    assert!(
        server.received_requests().await.unwrap().len() >= 6,
        "every call must have produced at least one physical send"
    );
}

/// The other direction, same path: a REJECTED request (403) must still get
/// hard fail-fast once the circuit opens. This is the half of E-H2 the round-2
/// change claims to preserve.
#[tokio::test(flavor = "multi_thread")]
async fn default_install_rejected_request_still_fails_fast() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403).set_body_string("{\"error\":\"forbidden\"}"))
        .mount(&server)
        .await;

    let provider = create_provider(&default_install(server.uri()));
    let results = drive(&provider, 6).await;

    let not_attempted = results
        .iter()
        .filter(|r| matches!(r, Err(ProviderError::NotAttempted { .. })))
        .count();
    assert!(
        not_attempted > 0,
        "a 403 must still open the circuit into refusal on a default install; \
         saw {results:?}"
    );
}

/// Rate limit (429), default install. REVISED with
/// `resilient::tests::open_circuit_without_fallback_still_probes_a_rate_limited_provider`:
/// this used to assert that a 429 fails fast, on the grounds that re-issuing
/// burns exhausted quota. A 429 is refused before the model runs, the engine
/// waits out the provider's own `Retry-After` between sends, and failing fast
/// here cost the run its retry budget three sends in — measured, both 429
/// shapes stopped at 3 sends against a budget of 10 and reported an open
/// circuit to a user who was over quota.
///
/// `default_install_rejected_request_still_fails_fast` (403) is the control
/// directly above: the genuinely REJECTED classes must still refuse, so this
/// change is scoped to "come back later" and not a removal of fail-fast.
#[tokio::test(flavor = "multi_thread")]
async fn default_install_rate_limit_keeps_probing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).set_body_string("{\"error\":\"rate limit\"}"))
        .mount(&server)
        .await;

    let provider = create_provider(&default_install(server.uri()));
    let results = drive(&provider, 6).await;

    let not_attempted = results
        .iter()
        .filter(|r| matches!(r, Err(ProviderError::NotAttempted { .. })))
        .count();
    assert_eq!(
        not_attempted, 0,
        "an open circuit with nowhere else to route must keep probing a \
         rate-limited provider rather than refusing; saw {results:?}"
    );
    // Known-positive: the drive really did meet 429s, so the assertion above
    // is not passing on an empty result set.
    let rate_limited = results
        .iter()
        .filter(|r| {
            matches!(r, Err(ProviderError::RateLimited { .. }))
                || matches!(r, Err(ProviderError::Api { status: 429, .. }))
        })
        .count();
    assert_eq!(
        rate_limited,
        results.len(),
        "every drive must have come back rate-limited; saw {results:?}"
    );
}

/// Single-flight, on the default install path and with a REAL socket rather
/// than a hand-built provider that parks in Rust. Two concurrent callers meet
/// an open circuit; at most one may hold the probe PERMIT.
///
/// This half of the round-2 claim holds.
#[tokio::test(flavor = "multi_thread")]
async fn default_install_open_circuit_probe_permit_is_single_flight() {
    let (refused, _sent) = overlap_two_callers().await;
    assert_eq!(
        refused, 1,
        "exactly one of two overlapping callers must be refused by the probe lease"
    );
}

/// ADVERSARIAL FINDING. `create_provider`s E-H2 doc says of the open-circuit
/// probe: "exactly one REQUEST is in flight against the open circuit and the
/// rest are refused". Measured on that exact constructor, one probe permit
/// produces SEVERAL physical requests, because the retry ring inside the
/// provider re-sends underneath the breaker. The lease is single-flight in
/// PERMITS, not in requests.
///
/// The engine path happens to be safe — it wraps the call in
/// `scope_max_retries(0)`, so one permit is one send there — but that is a
/// property of the caller, not of the constructor, and the doc makes the
/// claim unconditionally for "however many sessions or sub-agents share the
/// tracker".
#[tokio::test(flavor = "multi_thread")]
async fn default_install_one_probe_permit_emits_more_than_one_physical_send() {
    let (_refused, sent) = overlap_two_callers().await;
    assert!(
        sent > 1,
        "pinning observed reality: a single probe permit produced {sent} \
         physical sends against the open circuit (E-H2 documents exactly 1)"
    );
}

/// Opens the circuit, then overlaps two callers against it.
/// Returns (callers refused, physical sends that reached the open circuit).
async fn overlap_two_callers() -> (usize, usize) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(503)
                .set_delay(Duration::from_secs(3))
                .set_body_string("{\"error\":\"overloaded\"}"),
        )
        .mount(&server)
        .await;

    let provider = create_provider(&default_install(server.uri()));
    // Open the circuit (threshold defaults to 3).
    drive(&provider, 3).await;
    let opened = server.received_requests().await.unwrap().len();

    let a = {
        let p = Arc::clone(&provider);
        tokio::spawn(async move { p.stream(&request()).await.map(|_| ()) })
    };
    tokio::time::sleep(Duration::from_millis(400)).await;
    let b = {
        let p = Arc::clone(&provider);
        tokio::spawn(async move { p.stream(&request()).await.map(|_| ()) })
    };

    let (ra, rb) = (a.await.unwrap(), b.await.unwrap());
    let refused = [&ra, &rb]
        .iter()
        .filter(|r| matches!(r, Err(ProviderError::NotAttempted { .. })))
        .count();
    let sent = server.received_requests().await.unwrap().len() - opened;
    (refused, sent)
}
