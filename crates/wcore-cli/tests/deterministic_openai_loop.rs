use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wcore_egress::{BoundedEgressRecorder, EgressClient, EgressOutcome};
use wcore_eval_scenarios::artifact::{
    ArtifactExpectation, SealedBinaryArtifact, seal_binary, verify_artifact_digest,
};
use wcore_eval_scenarios::assertions::Assertion;
use wcore_eval_scenarios::fixtures::manifest::CompositeFixtureManifest;
use wcore_eval_scenarios::fixtures::mcp::{McpHttpFixture, McpHttpMode};
use wcore_eval_scenarios::fixtures::openai::{
    OpenAiFixtureObservation, OpenAiFixtureScript, OpenAiStep,
};
use wcore_eval_scenarios::fixtures::remote_execution::{
    FixtureArtifact, OutputChannel, RemoteExecutionFixture, RemoteExecutionScript, RemoteTask,
    ResourceBudget, ScriptedOutcome, ScriptedOutputEvent,
};
use wcore_eval_scenarios::fixtures::repository::{SeededRepository, repository_tree_sha256};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::receipt::{
    Evidence, EvidenceReceiptV1, ReceiptMetadataV1, milestone_evidence_gaps,
};
use wcore_eval_scenarios::runner::{
    Failure, ScenarioResult, run_with_binary, run_with_binary_in_environment,
};
use wcore_eval_scenarios::scenario::{ApprovalPolicy, Category, Scenario, Turn};
use wcore_eval_scenarios::tempenv::{self, TempEnvOptions};

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
        .with_known_free_cost()
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

async fn run_script_with_timeout(
    name: &'static str,
    steps: impl IntoIterator<Item = OpenAiStep>,
) -> (ScenarioResult, OpenAiFixtureObservation) {
    let fixture = OpenAiFixtureScript::new(steps)
        .start()
        .await
        .expect("start timeout fixture");
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_known_free_cost()
        .with_base_url(fixture.base_url());
    let env = tempenv::build_with(
        &provider,
        &TempEnvOptions {
            provider_read_timeout_ms: Some(75),
            ..TempEnvOptions::default()
        },
    )
    .expect("build timeout environment");
    let scenario = Scenario::new(name, Category::Hardening)
        .max_total_time(Duration::from_secs(12))
        .approval(ApprovalPolicy::Yolo)
        .turn(
            Turn::new("Return the deterministic fixture answer.").max_time(Duration::from_secs(8)),
        );

    let result = run_with_binary_in_environment(
        &scenario,
        &provider,
        Path::new(env!("CARGO_BIN_EXE_wayland-core")),
        &env,
    )
    .await
    .expect("packaged timeout run");
    let observation = fixture.shutdown().await.expect("timeout fixture shutdown");
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
    assert_eq!(result.execution.provider_attempts, Some(3));
    assert_eq!(result.execution.provider_retries, Some(2));
    assert_eq!(result.execution.provider_typed_failures, ["http_503"]);
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
    assert_eq!(result.execution.provider_attempts, Some(2));
    assert_eq!(result.execution.provider_retries, Some(1));
    assert_eq!(result.execution.provider_typed_failures, ["http_429"]);
    let delay_ms = observation.inter_request_delays_ms()[0];
    assert!(delay_ms >= 8, "retry ignored the 10 ms hint: {delay_ms} ms");
    // The ceiling is DERIVED, and it has to be: the bound this replaces was a
    // flat 1_000 ms, and honouring the hint can legitimately take longer than
    // that. `retry_delay` adds `RETRY_AFTER_JITTER` — U[0, 1 s], additive, so
    // a herd handed the same hint does not return in the same millisecond —
    // on top of the value the server sent, so the honoured-hint path spans
    // 10 ms to 1_010 ms before any scheduling overhead. Measured on a loaded
    // box: 1_164 ms on a high draw, 3 failures in 9 matched runs, and 0 in 9
    // on v0.13.5 where that jitter did not yet exist. The test was failing on
    // a correct delay.
    //
    // What it actually wants to assert is "the hint was honoured, NOT the
    // fallback curve". Those are separated by a wide gap rather than by a
    // magic number: with no usable hint a 429 floors at
    // `DEFAULT_RETRY_AFTER_MS`, so any fallback delay is >= 5_000 ms while
    // the honoured hint cannot exceed 1_010 ms. Bound it at the fallback
    // floor and the assertion becomes both un-flaky and strictly stronger
    // than a hand-tuned ceiling — it moves with the constants.
    let fallback_floor_ms = u128::from(wcore_providers::retry::DEFAULT_RETRY_AFTER_MS);
    assert!(
        u128::from(delay_ms) < fallback_floor_ms,
        "retry used a fallback delay instead of the fixture hint: {delay_ms} ms \
         (a hintless 429 floors at {fallback_floor_ms} ms; an honoured 10 ms hint \
         cannot exceed 10 ms + RETRY_AFTER_JITTER)"
    );
}

#[tokio::test]
async fn packaged_core_recovers_after_a_real_read_timeout() {
    let (result, observation) = run_script_with_timeout(
        "packaged_openai_timeout_retry",
        [
            OpenAiStep::stall_before_headers(250),
            OpenAiStep::text("recovered after timeout"),
        ],
    )
    .await;

    assert!(result.passed, "unexpected failures: {:?}", result.failures);
    assert_eq!(result.final_text, "recovered after timeout");
    assert_eq!(observation.requests.len(), 2);
    assert_eq!(result.execution.provider_attempts, Some(2));
    assert_eq!(result.execution.provider_retries, Some(1));
    assert_eq!(result.execution.provider_typed_failures, ["timeout"]);
}

#[tokio::test]
async fn packaged_core_exhausts_a_real_read_timeout() {
    // Budget STATED, not inherited. This scenario scripts exactly three
    // stalling provider steps and asserts the run gives up; the shipped
    // default of 10 retries cannot be exhausted inside the 12 s scenario cap,
    // so at the default the script ran out of stalls and a fourth request went
    // through. What is under test is timeout EXHAUSTION, not the budget size.
    let _retry_budget = wcore_eval_scenarios::tempenv::ScenarioRetryBudget::pin(2);
    let (result, observation) = run_script_with_timeout(
        "packaged_openai_timeout_exhausted",
        std::iter::repeat_with(|| OpenAiStep::stall_before_headers(250)).take(3),
    )
    .await;

    assert!(!result.passed, "terminal timeout must fail the scenario");
    assert_eq!(observation.requests.len(), 3);
    assert_eq!(result.execution.provider_attempts, Some(3));
    assert_eq!(result.execution.provider_retries, Some(2));
    assert_eq!(result.execution.provider_typed_failures, ["timeout"]);
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
    assert_eq!(result.execution.provider_attempts, Some(2));
    assert_eq!(result.execution.provider_retries, Some(1));
    assert_eq!(
        result.execution.provider_typed_failures,
        ["stream_truncated"]
    );
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
    let seed_provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_known_free_cost()
        .with_base_url("http://127.0.0.1:1");
    let env = tempenv::build(&seed_provider).expect("build retained eval workspace");
    let target = env.path().join("approved.txt");
    let fixture = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "call-approved-write",
            "Write",
            serde_json::json!({
                "file_path": target.to_string_lossy(),
                "content": "APPROVED"
            }),
        ),
        OpenAiStep::text("approved write completed"),
    ])
    .start()
    .await
    .expect("start OpenAI fixture");
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_known_free_cost()
        .with_base_url(fixture.base_url());
    let scenario = Scenario::new("packaged_openai_approval_allow", Category::Hardening)
        .max_total_time(Duration::from_secs(20))
        .approval(ApprovalPolicy::ApproveAll)
        .turn(
            Turn::new("Return the deterministic fixture answer.")
                .max_time(Duration::from_secs(10))
                .assert(Assertion::Contains("approved write completed")),
        );
    let result = run_with_binary_in_environment(
        &scenario,
        &provider,
        Path::new(env!("CARGO_BIN_EXE_wayland-core")),
        &env,
    )
    .await
    .expect("packaged Core run");
    let observation = fixture.shutdown().await.expect("fixture shutdown");

    assert!(result.passed, "unexpected failures: {:?}", result.failures);
    assert!(observation.complete(), "observation: {observation:?}");
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

/// How long the fixture holds the response stream open after the first text
/// delta. Everything below is scaled against this: the run must finish because
/// Core CANCELLED the stream, never because the provider ran out of things to
/// say. Deliberately 60s (the value `f14_sigkill_recovery` already uses) so the
/// margin over the measured ~1ms cancellation is unambiguous.
const CANCELLATION_STALL_MS: u64 = 60_000;
const CANCELLATION_STALL: Duration = Duration::from_millis(CANCELLATION_STALL_MS);

/// Ceiling on the interval this test actually exists to bound: from the moment
/// the harness sends `stop` (on the first text delta) to the moment the turn
/// ends. Measured on hetzner-dsm, idle and under 48-core load: `stop_sent` and
/// `stream_end` land in the SAME MILLISECOND of the turn trace when idle, and
/// the worst observed turn_end under load sat 49ms after the stream started.
/// One second is a large allowance against that, and a 60x discrimination
/// against `CANCELLATION_STALL`.
///
/// Proven able to fail: with a `sleep(3s)` inserted before the engine honours
/// `Stop`, this assertion fires — `cancellation did not abort the active stream
/// promptly … (3.002120937s later)` — while `result.failures` was still exactly
/// `[CostMissing]`, i.e. every OTHER assertion in this test passed. That is the
/// three-assertion self-test LANE-BRIEF 6b-ii asks for: the old instrument
/// would have missed a three-second cancellation stall entirely.
const CANCELLATION_LATENCY_CEILING: Duration = Duration::from_secs(1);

#[tokio::test]
async fn packaged_core_cancels_an_active_stream() {
    let fixture = OpenAiFixtureScript::new([OpenAiStep::text_then_stall(
        "before cancellation",
        CANCELLATION_STALL_MS,
    )])
    .start()
    .await
    .expect("start OpenAI fixture");
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_known_free_cost()
        .with_base_url(fixture.base_url());
    // The turn and scenario budgets bound the WHOLE turn, most of which is the
    // engine's pre-stream work — NOT the thing under test. They are set in line
    // with this file's other packaged scenarios (10s turn / 20s scenario) and
    // still sit far below CANCELLATION_STALL, so a stream that is not cancelled
    // is caught by them rather than mistaken for a pass.
    //
    // They were 3s / 5s until 2026-07-29, and that was the entire cause of CI
    // run 30434804220's `packaged_core_cancels_an_active_stream` failure —
    // `OverTime { observed_secs: 3.000722258, budget_secs: 3.0 }`, `TRY 3 FAIL`.
    // Measured on hetzner-dsm at 1097cfb3 with `WCORE_EVAL_TURN_TRACE=1`:
    //
    //   passing run:  t=0.000 prompt_sent … t=1.889 stream_start,
    //                 t=1.930 text_delta, t=1.930 stop_sent,
    //                 t=1.931 stream_end, t=1.931 turn_end
    //   failing run:  t=3.001 TURN_TIMEOUT stop_pending=TRUE
    //
    // i.e. ~1.9-2.1s of engine work happens BEFORE the provider stream even
    // starts, leaving under a second of slack; and every failure timed out with
    // the stop still PENDING — the budget expired before the harness had a
    // first token to cancel on. Under 48 busy cores that reproduced 15 times in
    // 30 (`TOTAL pass=15 fail=15`), against the ~2.5% quoted in the old comment.
    // Not one observation in 20 traced runs showed a stop that was sent and not
    // honoured, so the old budget never measured cancellation at all.
    let scenario = Scenario::new("packaged_openai_cancellation", Category::Hardening)
        .max_total_time(Duration::from_secs(20))
        .turn(
            Turn::new("Start a response and wait.")
                .max_time(Duration::from_secs(15))
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

    // THE assertion of this test, and the one the old 3s budget was standing in
    // for: the turn ended because `stop` was honoured, not because the provider
    // finished. `first_token_time` is when the harness saw the delta it cancels
    // on; the turn's wall time is when the turn ended. The difference is the
    // cancellation latency, and it is bounded independently of how long the
    // engine took to reach the stream.
    let turn = result
        .turn_results
        .first()
        .expect("the cancellation scenario has exactly one turn");
    let first_token = result
        .execution
        .first_token_time
        .expect("a text delta must arrive — it is what the mid-turn stop triggers on");
    let cancellation_latency = turn.wall_time.saturating_sub(first_token);
    assert!(
        cancellation_latency < CANCELLATION_LATENCY_CEILING,
        "cancellation did not abort the active stream promptly: stop was sent on the \
         first text delta at {first_token:?} and the turn did not end until {:?} \
         ({cancellation_latency:?} later), while the fixture still had up to \
         {CANCELLATION_STALL:?} of stream to deliver. failures: {:?}",
        turn.wall_time,
        result.failures
    );
    assert!(
        result.wall_time < CANCELLATION_STALL,
        "the run must finish because the stream was CANCELLED, not because the \
         provider stopped stalling: wall_time {:?} vs a {CANCELLATION_STALL:?} stall",
        result.wall_time
    );
    // The predicate is unchanged; only the diagnostic is added. A bare
    // `assert!(matches!(..))` prints nothing but the source line, so the one run
    // that goes red is the only chance to see what it actually got. Without the
    // payload every red costs another reproduction loop — and it is what made
    // the budget diagnosis above possible.
    assert!(
        matches!(result.failures.as_slice(), [Failure::CostMissing]),
        "expected exactly [CostMissing] after a mid-turn cancellation, got {:?}. \
         wall_time {:?}, cancellation_requested {}, final_text {:?}",
        result.failures,
        result.wall_time,
        result.execution.cancellation_requested,
        result.final_text
    );
    assert_eq!(result.final_text, "before cancellation");
    // NOTE: this one cannot fail. `cancellation_requested` is set from
    // `scenario.turns.iter().any(|t| t.stop_mid_turn)` on the normal path
    // (runner.rs) and hardcoded `true` on both failure paths, so it echoes this
    // test's own configuration rather than observing the engine. Kept because it
    // documents the intent; the load-bearing evidence is the latency assertion
    // above.
    assert!(result.execution.cancellation_requested);
    assert_eq!(
        result.execution.cleanup_verified, result.execution.containment_authoritative,
        "cleanup authority mismatch in {:?}; failures: {:?}",
        result.execution, result.failures
    );
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
        .with_known_free_cost()
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
            .any(|entry| entry.output.contains("CORE-MCP-ROUNDTRIP")),
        "trace did not retain MCP output: {:?}",
        result.trace
    );
    assert_eq!(openai_observation.requests.len(), 2);
    assert!(openai_observation.complete());
    assert!(mcp_observation.complete(), "{mcp_observation:?}");
}

#[tokio::test]
async fn packaged_core_satisfies_a_hidden_repository_outcome() {
    let repository = SeededRepository::new([
        ("README.md", "fixture repository\n"),
        ("src/settings.toml", "port = 8080\nmode = \"legacy\"\n"),
    ])
    .expect("valid repository fixture");
    let seed_provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_known_free_cost()
        .with_base_url("http://127.0.0.1:1");
    let env = tempenv::build_with(
        &seed_provider,
        &TempEnvOptions {
            budget_max_cost_usd: Some(0.10),
            ..TempEnvOptions::default()
        },
    )
    .expect("prepare hermetic repository environment");
    let settings_path = env.path().join("src").join("settings.toml");
    let openai = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "call-seeded-read",
            "Read",
            serde_json::json!({
                "file_path": settings_path.to_string_lossy()
            }),
        ),
        OpenAiStep::tool_call(
            "call-seeded-edit",
            "Edit",
            serde_json::json!({
                "file_path": settings_path.to_string_lossy(),
                "old_string": "port = 8080",
                "new_string": "port = 9090"
            }),
        ),
        OpenAiStep::text("Repository update completed"),
    ])
    .start()
    .await
    .expect("start OpenAI fixture");
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_known_free_cost()
        .with_base_url(openai.base_url());
    let scenario = Scenario::new("packaged_seeded_repository", Category::Hardening)
        .max_total_time(Duration::from_secs(20))
        .setup(move |cwd| repository.materialize(cwd).map_err(Into::into))
        .turn(
            Turn::new("Apply the requested repository update and report completion.")
                .max_time(Duration::from_secs(10))
                .expect_tool("Read")
                .expect_tool("Edit")
                .assert(Assertion::Contains("Repository update completed"))
                .assert(Assertion::FileContains {
                    path: "src/settings.toml",
                    needle: "port = 9090",
                }),
        );

    let result = run_with_binary_in_environment(
        &scenario,
        &provider,
        Path::new(env!("CARGO_BIN_EXE_wayland-core")),
        &env,
    )
    .await
    .expect("packaged seeded-repository run");
    let observation = openai.shutdown().await.expect("fixture shutdown");

    assert!(result.passed, "unexpected failures: {:?}", result.failures);
    assert_eq!(result.trace.count("Edit"), 1);
    assert_eq!(observation.requests.len(), 3);
    assert!(observation.complete());
}

struct SealedRun {
    workspace: PathBuf,
    repository_sha256: String,
    openai_behavior_sha256: String,
    openai_request_sha256: Vec<String>,
    openai_request_leaves: Vec<BTreeMap<String, String>>,
    fixture_manifest: CompositeFixtureManifest,
    receipt: EvidenceReceiptV1,
}

#[derive(Clone, Serialize)]
struct HiddenOutcomeContract {
    kind: &'static str,
    path: &'static str,
    needle: &'static str,
    expected_repository_sha256: String,
}

impl HiddenOutcomeContract {
    fn assertion(&self) -> Assertion {
        Assertion::FileContains {
            path: self.path,
            needle: self.needle,
        }
    }

    fn artifact_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&self).expect("hidden outcome contract serialization")
    }
}

#[derive(Serialize)]
struct EgressFixtureEvidence<'a> {
    schema: &'static str,
    method: &'a str,
    scheme: &'a str,
    host: &'a str,
    path_query_sha256: &'a str,
    request_body_sha256: String,
    outcome: &'static str,
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sealed_f04_binary() -> SealedBinaryArtifact {
    let source_commit = std::env::var("WCORE_F04_SOURCE_COMMIT")
        .unwrap_or_else(|_| env!("WAYLAND_SOURCE_SHA").to_string());
    let path = std::env::var_os("WCORE_EVAL_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_wayland-core")));
    if std::env::var_os("WCORE_F04_REQUIRE_PREBUILT").is_some()
        && std::env::var_os("WCORE_EVAL_BIN").is_none()
    {
        panic!("WCORE_F04_REQUIRE_PREBUILT requires an exact WCORE_EVAL_BIN artifact");
    }
    seal_binary(
        &path,
        ArtifactExpectation {
            version: env!("CARGO_PKG_VERSION"),
            source_commit: &source_commit,
        },
    )
    .expect("seal exact F04 Core artifact")
}

fn assert_request_leaves_equal(
    first: &[BTreeMap<String, String>],
    second: &[BTreeMap<String, String>],
) {
    assert_eq!(first.len(), second.len(), "OpenAI request count diverged");
    for (request_index, (first_request, second_request)) in first.iter().zip(second).enumerate() {
        let pointers = first_request.keys().chain(second_request.keys());
        for pointer in pointers {
            let first_digest = first_request.get(pointer);
            let second_digest = second_request.get(pointer);
            assert_eq!(
                first_digest,
                second_digest,
                "OpenAI request {} diverged at {}",
                request_index + 1,
                pointer
            );
        }
    }
}

fn remote_fixture_artifact(repository_sha256: &str) -> Vec<u8> {
    let limits = ResourceBudget::new(2_000, 64 * 1024 * 1024, 30_000, 1024 * 1024)
        .expect("remote fixture limits");
    let fixture =
        RemoteExecutionFixture::new("fixture-local", "worker-01", "fixture-v1", limits, [23; 32])
            .expect("remote fixture");
    let task = RemoteTask::new(
        "task-001",
        repository_sha256,
        b"verify the materialized repository".to_vec(),
        ResourceBudget::new(500, 1024 * 1024, 5_000, 4096).expect("remote task limits"),
    )
    .expect("remote task");
    let script = RemoteExecutionScript::new(
        [ScriptedOutputEvent::new(
            2,
            OutputChannel::Stdout,
            "repository verified",
        )],
        ScriptedOutcome::success(
            FixtureArtifact::new("dist/result.txt", b"verified\n".to_vec())
                .expect("remote artifact"),
        ),
    );
    let receipt = fixture.execute(&task, &script).expect("remote execution");
    receipt
        .verify(fixture.identity(), &fixture.verifying_key())
        .expect("remote fixture attestation");
    serde_json::to_vec(&receipt).expect("remote fixture receipt serialization")
}

async fn observe_egress_fixture() -> Vec<u8> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("egress fixture listener");
    let address = listener.local_addr().expect("egress fixture address");
    let mut server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("egress fixture accept");
        let mut buffer = [0_u8; 2048];
        let _ = socket.read(&mut buffer).await.expect("egress request read");
        socket
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
            .await
            .expect("egress response write");
    });
    let recorder = Arc::new(BoundedEgressRecorder::new(1));
    let send = EgressClient::tool()
        .with_observer(recorder.clone())
        .post(format!("http://{address}/fixture/status"))
        .body("fixture-request")
        .send();
    let response = match tokio::time::timeout(Duration::from_secs(5), send).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            server.abort();
            panic!("observed egress request failed: {error}");
        }
        Err(_) => {
            server.abort();
            panic!("observed egress request exceeded five seconds");
        }
    };
    assert_eq!(response.status().as_u16(), 204);
    match tokio::time::timeout(Duration::from_secs(2), &mut server).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("egress fixture server failed: {error}"),
        Err(_) => {
            server.abort();
            panic!("egress fixture server did not terminate");
        }
    }
    let observation = recorder.snapshot();
    assert_eq!(observation.dropped_events, 0);
    assert_eq!(observation.events.len(), 1);
    assert_eq!(
        observation.events[0].outcome,
        EgressOutcome::HttpResponse { status: 204 }
    );
    let event = &observation.events[0];
    let outcome = match event.outcome {
        EgressOutcome::HttpResponse { status: 204 } => "http_204",
        _ => panic!("unexpected egress fixture outcome: {:?}", event.outcome),
    };
    serde_json::to_vec(&EgressFixtureEvidence {
        schema: "wayland.eval.f04-egress-fixture.v1",
        method: &event.method,
        scheme: &event.destination.scheme,
        host: &event.destination.host,
        path_query_sha256: &event.destination.path_query_sha256,
        request_body_sha256: sha256(b"fixture-request"),
        outcome,
    })
    .expect("egress fixture evidence serialization")
}

fn normalize_workspace_in_json(value: &mut serde_json::Value, workspace: &str) {
    match value {
        serde_json::Value::String(text) => {
            *text = text.replace(workspace, "<WORKSPACE>");
        }
        serde_json::Value::Array(values) => {
            for value in values {
                normalize_workspace_in_json(value, workspace);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                normalize_workspace_in_json(value, workspace);
            }
        }
        _ => {}
    }
}

fn workspace_stable_artifact(value: &impl Serialize, workspace: &Path) -> Vec<u8> {
    let mut value = serde_json::to_value(value).expect("fixture artifact serialization");
    normalize_workspace_in_json(&mut value, workspace.to_string_lossy().as_ref());
    serde_json::to_vec(&value).expect("normalized fixture artifact serialization")
}

/// Does a recorded tool `input`/`output` field still carry the real workspace
/// path?
///
/// A tool call's `input` is a JSON document, so a Windows path's backslashes
/// arrive escaped (`C:\\Users\\...`) and a plain `contains` of the raw path
/// (`C:\Users\...`) can never match — the assertion failed for the encoding, not
/// for a missing path. `output` is plain text, so the raw form has to keep
/// matching too. Accept either encoding of the SAME path; on Unix the two forms
/// are identical, so this is a no-op there and cannot weaken the check on any
/// platform: an absent path matches neither.
fn retains_path(haystack: &str, path: &str) -> bool {
    if haystack.contains(path) {
        return true;
    }
    let encoded = serde_json::Value::String(path.to_owned()).to_string();
    haystack.contains(&encoded[1..encoded.len() - 1])
}

async fn run_sealed_repository_once(run_id: &str) -> SealedRun {
    let artifact = sealed_f04_binary();
    let repository = SeededRepository::new([
        ("README.md", "fixture repository\n"),
        ("src/settings.toml", "port = 8080\nmode = \"legacy\"\n"),
    ])
    .expect("valid repository fixture");
    let expected_repository = SeededRepository::new([
        ("README.md", "fixture repository\n"),
        ("src/settings.toml", "port = 9090\nmode = \"legacy\"\n"),
    ])
    .expect("valid expected repository outcome");
    let hidden_outcome = HiddenOutcomeContract {
        kind: "file_contains",
        path: "repository/src/settings.toml",
        needle: "port = 9090",
        expected_repository_sha256: expected_repository.fixture_sha256().to_string(),
    };
    let seed_provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_known_free_cost()
        .with_base_url("http://127.0.0.1:1");
    let env = tempenv::build_with(
        &seed_provider,
        &TempEnvOptions {
            budget_max_cost_usd: Some(0.10),
            ..TempEnvOptions::default()
        },
    )
    .expect("prepare hermetic seal environment");
    let workspace = env.path().to_path_buf();
    let repository_root = workspace.join("repository");
    let settings_path = repository_root.join("src").join("settings.toml");
    let mcp = McpHttpFixture::start(McpHttpMode::SseResponse)
        .await
        .expect("start MCP fixture");
    let mcp_url = mcp.url().to_string();
    let mcp_fixture_sha256 = mcp.fixture_sha256().to_string();
    let openai_script = OpenAiFixtureScript::new([
        OpenAiStep::tool_call(
            "call-seal-read",
            "Read",
            serde_json::json!({"file_path": settings_path.to_string_lossy()}),
        ),
        OpenAiStep::tool_call(
            "call-seal-edit",
            "Edit",
            serde_json::json!({
                "file_path": settings_path.to_string_lossy(),
                "old_string": "port = 8080",
                "new_string": "port = 9090"
            }),
        ),
        OpenAiStep::tool_call(
            "call-seal-mcp",
            "fixture_echo",
            serde_json::json!({"text": "F04-SEALED"}),
        ),
        OpenAiStep::text("Repository and MCP verification completed"),
    ]);
    let openai_script_artifact = workspace_stable_artifact(&openai_script, &workspace);
    let openai = openai_script
        .start_for_workspace(&workspace)
        .await
        .expect("start workspace-aware OpenAI fixture");
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_known_free_cost()
        .with_base_url(openai.base_url());
    let setup_repository = repository.clone();
    let scenario = Scenario::new("packaged_f04_repeatability", Category::Hardening)
        .max_total_time(Duration::from_secs(30))
        .setup(move |cwd| {
            setup_repository.materialize(&cwd.join("repository"))?;
            let config_path = cwd.join(".wayland-core").join("config.toml");
            let mut config = std::fs::read_to_string(&config_path)?;
            config.push_str(&format!(
                "\n[memory]\nenabled = false\n\n[observability]\nstructured_traces = true\n\n[mcp.servers.fixture]\ntransport = \"streamable-http\"\nurl = \"{mcp_url}\"\nallow_local = true\ndeferred = false\n"
            ));
            std::fs::write(config_path, config)?;
            Ok(())
        })
        .turn(
            Turn::new("Update the repository, call fixture_echo, then report completion.")
                .max_time(Duration::from_secs(20))
                .expect_tool("Read")
                .expect_tool("Edit")
                .expect_tool("fixture_echo")
                .assert(Assertion::Contains(
                    "Repository and MCP verification completed",
                ))
                .assert(hidden_outcome.assertion()),
        );

    let result = run_with_binary_in_environment(&scenario, &provider, &artifact.path, &env)
        .await
        .expect("packaged F04 seal run");
    verify_artifact_digest(&artifact).expect("F04 Core artifact changed during execution");
    let openai_observation = openai.shutdown().await.expect("OpenAI fixture shutdown");
    let mcp_observation = mcp.shutdown().await.expect("MCP fixture shutdown");
    assert!(result.passed, "unexpected failures: {:?}", result.failures);
    assert_eq!(
        result
            .trace
            .entries
            .iter()
            .map(|entry| entry.tool_name.as_str())
            .collect::<Vec<_>>(),
        ["Read", "Edit", "fixture_echo"]
    );
    assert!(openai_observation.complete());
    assert!(mcp_observation.complete(), "{mcp_observation:?}");
    let workspace_text = workspace.to_string_lossy();
    let read = &result.trace.entries[0];
    let edit = &result.trace.entries[1];
    let mcp_call = &result.trace.entries[2];
    assert!(
        retains_path(&read.input, workspace_text.as_ref()),
        "Read input did not retain {}: {}",
        workspace.display(),
        read.input
    );
    assert!(
        retains_path(&edit.input, workspace_text.as_ref()),
        "Edit input did not retain {}: {}",
        workspace.display(),
        edit.input
    );
    assert!(
        retains_path(&edit.output, workspace_text.as_ref()),
        "Edit output did not retain {}: {}",
        workspace.display(),
        edit.output
    );
    assert!(mcp_call.input.contains("F04-SEALED"));
    assert!(mcp_call.output.contains("F04-SEALED"));
    let final_repository_sha256 =
        repository_tree_sha256(&repository_root).expect("materialized repository digest");
    assert_eq!(
        final_repository_sha256,
        expected_repository.fixture_sha256(),
        "packaged run produced an unexpected extra or missing repository mutation"
    );
    let repository_artifact = repository
        .artifact_bytes()
        .expect("seeded repository artifact serialization");
    assert_eq!(repository.fixture_sha256(), sha256(&repository_artifact));
    let hidden_outcome_artifact = hidden_outcome.artifact_bytes();
    let mcp_artifact = serde_json::to_vec(&(1_u32, McpHttpMode::SseResponse))
        .expect("MCP fixture mode serialization");
    assert_eq!(
        mcp_fixture_sha256,
        sha256(&mcp_artifact),
        "running MCP fixture identity must derive from its live mode"
    );
    let egress_artifact = observe_egress_fixture().await;
    let remote_execution_artifact = remote_fixture_artifact(repository.fixture_sha256());
    let manifest = CompositeFixtureManifest::from_artifacts(
        &openai_script_artifact,
        &repository_artifact,
        &hidden_outcome_artifact,
        &mcp_artifact,
        &egress_artifact,
        &remote_execution_artifact,
    );
    let receipt = EvidenceReceiptV1::from_scenario_result(
        ReceiptMetadataV1 {
            run_id: run_id.to_string(),
            source_commit: artifact.source_commit.clone(),
            binary_sha256: artifact.sha256.clone(),
            fixture_sha256: manifest.fixture_sha256().to_string(),
            model: "fixture-chat-v1".to_string(),
            build: Evidence::Unavailable {
                code: "local_run".to_string(),
            },
        },
        &result,
        0.10,
    )
    .expect("sealed evidence receipt");

    SealedRun {
        workspace,
        repository_sha256: final_repository_sha256,
        openai_behavior_sha256: openai_observation
            .behavior_sha256()
            .expect("OpenAI behavior digest"),
        openai_request_sha256: openai_observation
            .requests
            .iter()
            .map(|request| request.semantic_body_sha256.clone())
            .collect(),
        openai_request_leaves: openai_observation
            .requests
            .iter()
            .map(|request| request.semantic_leaf_sha256.clone())
            .collect(),
        fixture_manifest: manifest,
        receipt,
    }
}

#[tokio::test]
async fn packaged_f04_run_is_repeatable_and_content_addressed() {
    let first = run_sealed_repository_once("f04-repeat-1").await;
    let second = run_sealed_repository_once("f04-repeat-2").await;

    assert_ne!(first.workspace, second.workspace);
    assert_eq!(first.repository_sha256, second.repository_sha256);
    assert_request_leaves_equal(&first.openai_request_leaves, &second.openai_request_leaves);
    assert_eq!(
        first.openai_request_sha256, second.openai_request_sha256,
        "OpenAI semantic request bodies diverged"
    );
    assert_eq!(first.openai_behavior_sha256, second.openai_behavior_sha256);
    assert_eq!(
        first.fixture_manifest.components(),
        second.fixture_manifest.components()
    );
    assert_eq!(
        first.receipt.body.identity.fixture_sha256,
        second.receipt.body.identity.fixture_sha256
    );
    assert_eq!(
        first.receipt.body.identity.config_sha256,
        second.receipt.body.identity.config_sha256
    );
    assert_eq!(first.receipt.body.tools.len(), 3);
    assert_eq!(
        first.receipt.body.tools.len(),
        second.receipt.body.tools.len()
    );
    for (first_tool, second_tool) in first
        .receipt
        .body
        .tools
        .iter()
        .zip(&second.receipt.body.tools)
    {
        assert_eq!(first_tool.tool_name, second_tool.tool_name);
        assert_eq!(first_tool.request_sha256, second_tool.request_sha256);
        assert_eq!(first_tool.result_sha256, second_tool.result_sha256);
        assert_eq!(first_tool.exit_state, second_tool.exit_state);
    }
    assert_ne!(first.receipt.body_sha256, second.receipt.body_sha256);
    if let Some(directory) = std::env::var_os("WCORE_F04_EVIDENCE_DIR") {
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory).expect("create F04 evidence directory");
        std::fs::write(
            directory.join("repeat-1-receipt.json"),
            serde_json::to_vec_pretty(&first.receipt).expect("serialize first receipt"),
        )
        .expect("write first receipt");
        std::fs::write(
            directory.join("repeat-2-receipt.json"),
            serde_json::to_vec_pretty(&second.receipt).expect("serialize second receipt"),
        )
        .expect("write second receipt");
    }
    let behavior_sha256 = first
        .receipt
        .behavior_sha256()
        .expect("first receipt behavior digest");
    assert_eq!(
        behavior_sha256,
        second
            .receipt
            .behavior_sha256()
            .expect("second receipt behavior digest")
    );

    if let Some(directory) = std::env::var_os("WCORE_F04_EVIDENCE_DIR") {
        let directory = PathBuf::from(directory);
        std::fs::write(
            directory.join("repeatability.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "wayland.eval.f04-repeatability",
                "schema_version": 1,
                "behavior_sha256": behavior_sha256,
                "fixture_sha256": first.receipt.body.identity.fixture_sha256,
                "fixture_manifest": first.fixture_manifest,
                "openai_behavior_sha256": first.openai_behavior_sha256,
                "repository_sha256": first.repository_sha256,
                "runs": 2,
            }))
            .expect("serialize repeatability summary"),
        )
        .expect("write repeatability summary");
        std::fs::write(
            directory.join("fixture-manifest.json"),
            serde_json::to_vec_pretty(&first.fixture_manifest).expect("serialize fixture manifest"),
        )
        .expect("write fixture manifest");
        std::fs::write(
            directory.join("authority-policy-observation.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "wayland.eval.authority-policy-observation",
                "schema_version": 1,
                "config_sha256": first.receipt.body.identity.config_sha256,
                "fixture_sha256": first.receipt.body.identity.fixture_sha256,
                "provider": first.receipt.body.identity.provider,
                "model": first.receipt.body.identity.model,
                "target_os": first.receipt.body.target.os,
                "target_architecture": first.receipt.body.target.architecture,
                "sandbox_backend": first.receipt.body.target.sandbox_backend,
                "policy_posture": first.receipt.body.policy.posture,
                "effective_policy_sha256": first.receipt.body.policy.effective_policy_sha256,
                "required_cells": first.receipt.body.required_cells,
            }))
            .expect("serialize authority policy observation"),
        )
        .expect("write authority policy observation");
        std::fs::write(
            directory.join("milestone-evidence-gaps.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "wayland.eval.milestone-evidence-gaps",
                "schema_version": 1,
                "missing": milestone_evidence_gaps(&first.receipt.body),
            }))
            .expect("serialize milestone evidence gaps"),
        )
        .expect("write milestone evidence gaps");
    }
}
