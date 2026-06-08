//! v0.8.1 U3 — representations endpoint round-trip + mock coverage.
//!
//! Two surfaces under test:
//! 1. The `HonchoClient::mock()` + `seed_mock_representations` pair —
//!    the deterministic in-RAM path adapter tests rely on.
//! 2. The live HTTP path via `HonchoClient::from_spec` aimed at a
//!    wiremock server. Exercises JSON parsing and the 404→empty Vec
//!    fallback without burning live API credit.

use serde_json::json;
use wayland_honcho::{DialecticInference, HonchoClient};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn mock_representations_empty_for_unknown_user() {
    let client = HonchoClient::mock();
    let infs = client.representations("ghost").await.unwrap();
    assert!(infs.is_empty());
}

#[tokio::test]
async fn mock_representations_round_trips_seeded_inferences() {
    let client = HonchoClient::mock();
    let seeded = vec![
        DialecticInference {
            kind: "preference".into(),
            subject: "code_style".into(),
            value: "terse".into(),
            confidence: 0.82,
            evidence_count: 4,
        },
        DialecticInference {
            kind: "expertise".into(),
            subject: "rust".into(),
            value: "expert".into(),
            confidence: 0.91,
            evidence_count: 12,
        },
    ];
    let seeded_ok = client.seed_mock_representations("alice", seeded.clone());
    assert!(seeded_ok, "mock seeding should succeed on a mock client");
    let infs = client.representations("alice").await.unwrap();
    assert_eq!(infs, seeded);
}

/// Build a live client pointed at an in-process wiremock server. Uses a
/// unique env-var name per test so parallel test runs don't race.
async fn live_client_against(server: &MockServer, env_var: &str) -> HonchoClient {
    // SAFETY: env mutation is process-wide. Per-test unique var names
    // (callers pass `env_var = "WAYLAND_HONCHO_TEST_KEY_<test_name>"`)
    // keep parallel tests isolated. We do not unset; tests in this file
    // each own their own env var.
    unsafe {
        std::env::set_var(env_var, "test-token");
    }
    HonchoClient::from_spec(Some(server.uri().as_str()), Some(env_var))
        .expect("live client should construct against wiremock")
}

#[tokio::test]
async fn live_representations_parses_inference_array() {
    let server = MockServer::start().await;
    let body = json!([
        {
            "kind": "preference",
            "subject": "code_style",
            "value": "terse",
            "confidence": 0.82,
            "evidence_count": 4
        },
        {
            "kind": "expertise",
            "subject": "rust",
            "value": "expert",
            "confidence": 0.91,
            "evidence_count": 12
        }
    ]);
    Mock::given(method("GET"))
        .and(path("/v1/users/alice/representations"))
        .and(header("authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let client = live_client_against(&server, "WAYLAND_HONCHO_TEST_KEY_PARSE").await;
    let infs = client.representations("alice").await.unwrap();
    assert_eq!(infs.len(), 2);
    assert_eq!(infs[0].subject, "code_style");
    assert_eq!(infs[1].kind, "expertise");
    assert!((infs[1].confidence - 0.91).abs() < 1e-4);
    assert_eq!(infs[1].evidence_count, 12);
}

#[tokio::test]
async fn live_representations_404_returns_empty_vec() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/users/new-user/representations"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let client = live_client_against(&server, "WAYLAND_HONCHO_TEST_KEY_404").await;
    let infs = client.representations("new-user").await.unwrap();
    assert!(
        infs.is_empty(),
        "404 should degrade to empty Vec, not an error"
    );
}

#[tokio::test]
async fn live_representations_500_surfaces_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/users/bob/representations"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let client = live_client_against(&server, "WAYLAND_HONCHO_TEST_KEY_500").await;
    let err = client
        .representations("bob")
        .await
        .expect_err("5xx must NOT degrade silently");
    assert!(
        format!("{err:?}").contains("Api"),
        "expected HonchoError::Api, got {err:?}"
    );
}
