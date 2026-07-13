use std::path::Path;
use std::time::Duration;

use wcore_eval_scenarios::assertions::Assertion;
use wcore_eval_scenarios::fixtures::openai::{
    OpenAiFixtureObservation, OpenAiFixtureScript, OpenAiStep,
};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::{ScenarioResult, run_with_binary};
use wcore_eval_scenarios::scenario::{Category, Scenario, Turn};

async fn run_script(
    name: &'static str,
    steps: impl IntoIterator<Item = OpenAiStep>,
    expected: &'static str,
) -> (ScenarioResult, OpenAiFixtureObservation) {
    let fixture = OpenAiFixtureScript::new(steps)
        .start()
        .await
        .expect("start OpenAI fixture");
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_base_url(fixture.base_url());
    let scenario = Scenario::new(name, Category::Hardening)
        .max_total_time(Duration::from_secs(20))
        .turn(
            Turn::new("Return the deterministic fixture answer.")
                .max_time(Duration::from_secs(10))
                .assert(Assertion::Contains(expected)),
        );

    let result = run_with_binary(
        &scenario,
        &provider,
        Path::new(env!("CARGO_BIN_EXE_wayland-core")),
    )
    .await;
    let observation = fixture.shutdown().await.expect("fixture shutdown");
    let result = result.expect("packaged Core run");

    assert!(result.passed, "unexpected failures: {:?}", result.failures);
    assert!(observation.complete(), "observation: {observation:?}");
    assert!(
        observation
            .requests
            .iter()
            .all(|request| request.model.as_deref() == Some("fixture-chat-v1"))
    );
    (result, observation)
}

#[tokio::test]
async fn packaged_core_completes_a_scripted_openai_turn() {
    let (result, observation) = run_script(
        "packaged_openai_turn",
        [OpenAiStep::text("fixture answer")],
        "fixture answer",
    )
    .await;

    assert!(result.final_text.contains("fixture answer"));
    assert_eq!(observation.requests.len(), 1);
}

#[tokio::test]
async fn packaged_core_recovers_after_two_503_responses() {
    let (result, observation) = run_script(
        "packaged_openai_503_retry",
        [
            OpenAiStep::http_error(503),
            OpenAiStep::http_error(503),
            OpenAiStep::text("recovered after 503"),
        ],
        "recovered after 503",
    )
    .await;

    assert_eq!(result.final_text, "recovered after 503");
    assert_eq!(observation.requests.len(), 3);
}

#[tokio::test]
async fn packaged_core_recovers_after_a_bounded_429() {
    let (result, observation) = run_script(
        "packaged_openai_429_retry",
        [
            OpenAiStep::rate_limited(10),
            OpenAiStep::text("recovered after 429"),
        ],
        "recovered after 429",
    )
    .await;

    assert_eq!(result.final_text, "recovered after 429");
    assert_eq!(observation.requests.len(), 2);
}

#[tokio::test]
async fn packaged_core_recovers_after_a_truncated_stream() {
    let (result, observation) = run_script(
        "packaged_openai_truncated_retry",
        [
            OpenAiStep::truncated("discarded partial"),
            OpenAiStep::text("recovered after truncation"),
        ],
        "recovered after truncation",
    )
    .await;

    assert!(result.final_text.ends_with("recovered after truncation"));
    assert_eq!(observation.requests.len(), 2);
}

#[tokio::test]
async fn packaged_core_preserves_declared_duplicate_deltas() {
    let (result, observation) = run_script(
        "packaged_openai_duplicate_delta",
        [OpenAiStep::duplicate_text("repeat")],
        "repeatrepeat",
    )
    .await;

    assert_eq!(result.final_text, "repeatrepeat");
    assert_eq!(observation.requests.len(), 1);
}
