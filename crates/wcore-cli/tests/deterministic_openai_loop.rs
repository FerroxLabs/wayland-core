use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wcore_egress::{BoundedEgressRecorder, EgressClient, EgressOutcome};
use wcore_eval_scenarios::assertions::Assertion;
use wcore_eval_scenarios::fixtures::manifest::{CompositeFixtureManifest, FixtureComponents};
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
use wcore_eval_scenarios::receipt::{Evidence, EvidenceReceiptV1, ReceiptMetadataV1};
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
    assert_eq!(result.execution.provider_retries, None);
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
    let delay_ms = observation.inter_request_delays_ms()[0];
    assert!(delay_ms >= 8, "retry ignored the 10 ms hint: {delay_ms} ms");
    assert!(
        delay_ms < 1_000,
        "retry used a fallback delay instead of the fixture hint: {delay_ms} ms"
    );
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
        .with_api_key("fixture-local-token");
    let env = tempenv::build_with(
        &seed_provider,
        &TempEnvOptions {
            budget_max_cost_usd: Some(0.10),
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

    fn fixture_sha256(&self) -> String {
        sha256(&serde_json::to_vec(&self).expect("hidden outcome contract serialization"))
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

fn remote_fixture_sha256(repository_sha256: &str) -> String {
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
    receipt.body_sha256
}

async fn observe_egress_fixture() -> String {
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
    sha256(
        &serde_json::to_vec(&EgressFixtureEvidence {
            schema: "wayland.eval.f04-egress-fixture.v1",
            method: &event.method,
            scheme: &event.destination.scheme,
            host: &event.destination.host,
            path_query_sha256: &event.destination.path_query_sha256,
            request_body_sha256: sha256(b"fixture-request"),
            outcome,
        })
        .expect("egress fixture evidence serialization"),
    )
}

async fn run_sealed_repository_once(run_id: &str) -> SealedRun {
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
        .with_api_key("fixture-local-token");
    let env = tempenv::build_with(
        &seed_provider,
        &TempEnvOptions {
            budget_max_cost_usd: Some(0.10),
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
    let openai = openai_script
        .start_for_workspace(&workspace)
        .await
        .expect("start workspace-aware OpenAI fixture");
    let openai_fixture_sha256 = openai.fixture_sha256().to_string();
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_base_url(openai.base_url());
    let setup_repository = repository.clone();
    let scenario = Scenario::new("packaged_f04_repeatability", Category::Hardening)
        .max_total_time(Duration::from_secs(30))
        .setup(move |cwd| {
            setup_repository.materialize(&cwd.join("repository"))?;
            let config_path = cwd.join(".wayland-core").join("config.toml");
            let mut config = std::fs::read_to_string(&config_path)?;
            config.push_str(&format!(
                "\n[mcp.servers.fixture]\ntransport = \"streamable-http\"\nurl = \"{mcp_url}\"\nallow_local = true\ndeferred = false\n"
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

    let mut result = run_with_binary_in_environment(
        &scenario,
        &provider,
        Path::new(env!("CARGO_BIN_EXE_wayland-core")),
        &env,
    )
    .await
    .expect("packaged F04 seal run");
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
        read.input.contains(workspace_text.as_ref()),
        "Read input did not retain {}: {}",
        workspace.display(),
        read.input
    );
    assert!(
        edit.input.contains(workspace_text.as_ref()),
        "Edit input did not retain {}: {}",
        workspace.display(),
        edit.input
    );
    assert!(
        edit.output.contains(workspace_text.as_ref()),
        "Edit output did not retain {}: {}",
        workspace.display(),
        edit.output
    );
    assert!(mcp_call.input.contains("F04-SEALED"));
    assert!(mcp_call.output.contains("F04-SEALED"));
    result.execution.provider_attempts = Some(openai_observation.attempts());

    let final_repository_sha256 =
        repository_tree_sha256(&repository_root).expect("materialized repository digest");
    assert_eq!(
        final_repository_sha256,
        expected_repository.fixture_sha256(),
        "packaged run produced an unexpected extra or missing repository mutation"
    );
    let hidden_outcome_sha256 = hidden_outcome.fixture_sha256();
    let egress_fixture_sha256 = observe_egress_fixture().await;
    let remote_execution_sha256 = remote_fixture_sha256(repository.fixture_sha256());
    let manifest = CompositeFixtureManifest::new(
        FixtureComponents::new(
            openai_fixture_sha256,
            repository.fixture_sha256(),
            hidden_outcome_sha256,
            mcp_fixture_sha256,
            egress_fixture_sha256,
            remote_execution_sha256,
        )
        .expect("complete fixture identities"),
    );
    let binary_sha256 =
        sha256(&std::fs::read(env!("CARGO_BIN_EXE_wayland-core")).expect("read packaged binary"));
    let source_commit = std::env::var("WCORE_F04_SOURCE_COMMIT").unwrap_or_else(|_| "a".repeat(40));
    let receipt = EvidenceReceiptV1::from_scenario_result(
        ReceiptMetadataV1 {
            run_id: run_id.to_string(),
            source_commit,
            binary_sha256,
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
    }
}
