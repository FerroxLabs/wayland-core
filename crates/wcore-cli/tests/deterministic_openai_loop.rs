use std::path::Path;
use std::time::Duration;

use wcore_eval_scenarios::assertions::Assertion;
use wcore_eval_scenarios::fixtures::mcp::{McpHttpFixture, McpHttpMode};
use wcore_eval_scenarios::fixtures::openai::{
    OpenAiFixtureObservation, OpenAiFixtureScript, OpenAiStep,
};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::{Failure, ScenarioResult, run_with_binary};
use wcore_eval_scenarios::scenario::{ApprovalPolicy, Category, Scenario, Turn};

async fn run_script(
    name: &'static str,
    steps: impl IntoIterator<Item = OpenAiStep>,
    expected: &'static str,
) -> (ScenarioResult, OpenAiFixtureObservation) {
    run_script_with_approval(name, steps, expected, ApprovalPolicy::Yolo).await
}

async fn run_script_with_approval(
    name: &'static str,
    steps: impl IntoIterator<Item = OpenAiStep>,
    expected: &'static str,
    approval: ApprovalPolicy,
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
        .approval(approval)
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
    let mut result = result.expect("packaged Core run");

    assert!(result.passed, "unexpected failures: {:?}", result.failures);
    assert!(observation.complete(), "observation: {observation:?}");
    assert!(
        observation
            .requests
            .iter()
            .all(|request| request.model.as_deref() == Some("fixture-chat-v1"))
    );
    result.execution.provider_attempts = Some(observation.attempts());
    result.execution.provider_retries = Some(observation.retries());
    result.execution.provider_typed_failures = observation.typed_failures().to_vec();
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
    assert_eq!(result.execution.provider_attempts, Some(1));
    assert_eq!(result.execution.provider_retries, Some(0));
    let usage = result
        .execution
        .provider_usage
        .expect("packaged stream_end usage");
    assert_eq!(usage.input_tokens, 7);
    assert_eq!(usage.output_tokens, 3);
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

#[tokio::test]
async fn packaged_core_executes_an_approved_write() {
    let target_dir = tempfile::tempdir().expect("target tempdir");
    let target = target_dir.path().join("approved.txt");
    let (result, observation) = run_script_with_approval(
        "packaged_openai_approval_allow",
        [
            OpenAiStep::tool_call(
                "call-approved-write",
                "Write",
                serde_json::json!({
                    "file_path": target.to_string_lossy(),
                    "content": "APPROVED"
                }),
            ),
            OpenAiStep::text("approved write completed"),
        ],
        "approved write completed",
        ApprovalPolicy::ApproveAll,
    )
    .await;

    assert_eq!(std::fs::read_to_string(&target).unwrap(), "APPROVED");
    assert_eq!(result.approval, ApprovalPolicy::ApproveAll);
    assert_eq!(result.trace.count("Write"), 1);
    assert_eq!(observation.requests.len(), 2);
}

#[tokio::test]
async fn packaged_core_blocks_a_denied_write() {
    let target_dir = tempfile::tempdir().expect("target tempdir");
    let target = target_dir.path().join("denied.txt");
    let (result, observation) = run_script_with_approval(
        "packaged_openai_approval_deny",
        [
            OpenAiStep::tool_call(
                "call-denied-write",
                "Write",
                serde_json::json!({
                    "file_path": target.to_string_lossy(),
                    "content": "DENIED"
                }),
            ),
            OpenAiStep::text("denied write handled"),
        ],
        "denied write handled",
        ApprovalPolicy::DenyAll,
    )
    .await;

    assert!(!target.exists(), "denied tool created {}", target.display());
    assert_eq!(result.approval, ApprovalPolicy::DenyAll);
    assert_eq!(result.trace.count("Write"), 1);
    assert_eq!(observation.requests.len(), 2);
}

#[tokio::test]
async fn packaged_core_cancels_an_active_stream() {
    let started = std::time::Instant::now();
    let fixture =
        OpenAiFixtureScript::new([OpenAiStep::text_then_stall("before cancellation", 10_000)])
            .start()
            .await
            .expect("start OpenAI fixture");
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_base_url(fixture.base_url());
    let scenario = Scenario::new("packaged_openai_cancellation", Category::Hardening)
        .max_total_time(Duration::from_secs(5))
        .turn(
            Turn::new("Start a response and wait.")
                .max_time(Duration::from_secs(3))
                .stop_mid_turn(),
        );
    let result = run_with_binary(
        &scenario,
        &provider,
        Path::new(env!("CARGO_BIN_EXE_wayland-core")),
    )
    .await
    .expect("packaged cancellation run");
    let observation = fixture.shutdown().await.expect("fixture shutdown");

    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(matches!(result.failures.as_slice(), [Failure::CostMissing]));
    assert_eq!(result.final_text, "before cancellation");
    assert!(result.execution.cancellation_requested);
    assert!(result.execution.cleanup_verified);
    assert!(observation.complete(), "observation: {observation:?}");
    assert_eq!(observation.requests.len(), 1);
}

#[tokio::test]
async fn packaged_core_calls_a_streamable_http_mcp_tool() {
    let mcp = McpHttpFixture::start(McpHttpMode::SseResponse)
        .await
        .expect("start MCP fixture");
    let mcp_url = mcp.url().to_string();
    let openai = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "call-mcp-echo",
            "fixture_echo",
            serde_json::json!({"text": "CORE-MCP-ROUNDTRIP"}),
        ),
        OpenAiStep::text("MCP roundtrip completed"),
    ])
    .start()
    .await
    .expect("start OpenAI fixture");
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_base_url(openai.base_url());
    let scenario = Scenario::new("packaged_mcp_roundtrip", Category::Hardening)
        .max_total_time(Duration::from_secs(20))
        .setup(move |cwd| {
            let config_path = cwd.join(".wayland-core").join("config.toml");
            let mut config = std::fs::read_to_string(&config_path)?;
            config.push_str(&format!(
                "\n[mcp.servers.fixture]\ntransport = \"streamable-http\"\nurl = \"{mcp_url}\"\nallow_local = true\ndeferred = false\n"
            ));
            std::fs::write(config_path, config)?;
            Ok(())
        })
        .turn(
            Turn::new("Use fixture_echo with CORE-MCP-ROUNDTRIP, then confirm completion.")
                .max_time(Duration::from_secs(10))
                .expect_tool("fixture_echo")
                .assert(Assertion::Contains("MCP roundtrip completed")),
        );

    let result = run_with_binary(
        &scenario,
        &provider,
        Path::new(env!("CARGO_BIN_EXE_wayland-core")),
    )
    .await
    .expect("packaged MCP run");
    let openai_observation = openai.shutdown().await.expect("OpenAI fixture shutdown");
    let mcp_observation = mcp.shutdown().await.expect("MCP fixture shutdown");

    assert!(result.passed, "unexpected failures: {:?}", result.failures);
    assert_eq!(result.trace.count("fixture_echo"), 1);
    assert!(
        result
            .trace
            .entries
            .iter()
            .any(|entry| entry.output.contains("CORE-MCP-ROUNDTRIP"))
    );
    assert_eq!(openai_observation.requests.len(), 2);
    assert!(openai_observation.complete());
    assert!(mcp_observation.complete(), "{mcp_observation:?}");
}
