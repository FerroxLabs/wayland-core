use std::path::Path;
use std::time::Duration;

use wcore_eval_scenarios::assertions::Assertion;
use wcore_eval_scenarios::fixtures::openai::{OpenAiFixtureScript, OpenAiStep};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::run_with_binary;
use wcore_eval_scenarios::scenario::{Category, Scenario, Turn};

#[tokio::test]
async fn packaged_core_completes_a_scripted_openai_turn() {
    let fixture = OpenAiFixtureScript::new([OpenAiStep::text("fixture answer")])
        .start()
        .await
        .expect("start OpenAI fixture");
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_base_url(fixture.base_url());
    let scenario = Scenario::new("packaged_openai_turn", Category::Hardening)
        .max_total_time(Duration::from_secs(20))
        .turn(
            Turn::new("Return the deterministic fixture answer.")
                .max_time(Duration::from_secs(10))
                .assert(Assertion::Contains("fixture answer")),
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
    assert!(result.final_text.contains("fixture answer"));
    assert!(observation.complete(), "observation: {observation:?}");
    assert_eq!(observation.requests.len(), 1);
    assert_eq!(
        observation.requests[0].model.as_deref(),
        Some("fixture-chat-v1")
    );
}
