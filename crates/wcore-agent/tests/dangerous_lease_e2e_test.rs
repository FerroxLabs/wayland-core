//! Unix production-chain proofs for expiring Dangerous sessions.

#![cfg(unix)]

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use common::{
    MockLlmProvider, RECOVERY_TEST_KEY, configure_persisted_test_session, physical_attempt_server,
};
use serde_json::json;
use tokio::sync::{Notify, mpsc};
use wcore_agent::bootstrap::{AgentBootstrap, BootstrapResult};
use wcore_agent::engine::AgentError;
use wcore_agent::output::OutputSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_egress::{AllowAllPolicy, EgressClient};
use wcore_providers::retry::{builder_send_with_retry, scope_max_retries};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::execution_policy::{
    ApprovalPolicy, BaselineExecutionPolicy, DangerousLaunchRequest, PolicySource,
    resolve_dangerous_launch,
};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

fn bootstrap_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 64,
        max_turns: Some(1),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

#[derive(Default)]
struct StreamingSink {
    chunks: AtomicUsize,
}

impl OutputSink for StreamingSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool) {}
    fn emit_info(&self, _: &str) {}
    fn emit_tool_chunk(&self, _: &str, _: &str, _: &str, _: &str) {
        self.chunks.fetch_add(1, Ordering::Relaxed);
    }
    fn streaming_tools_advertised(&self) -> bool {
        true
    }
}

fn dangerous_grant(activation_id: &str) -> wcore_types::execution_policy::DangerousSessionGrant {
    resolve_dangerous_launch(
        &BaselineExecutionPolicy::smart(ApprovalPolicy::Prompt, PolicySource::Default),
        DangerousLaunchRequest::cli(3, activation_id),
        0,
    )
    .expect("trusted local launch must resolve")
}

fn process_running(pid: u32) -> bool {
    // SAFETY: signal 0 only checks process existence.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        && let Some((_, fields)) = stat.rsplit_once(") ")
        && fields.starts_with('Z')
    {
        return false;
    }
    true
}

async fn read_pid(path: &std::path::Path) -> u32 {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(raw) = std::fs::read_to_string(path)
                && let Ok(pid) = raw.trim().parse()
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Dangerous Bash must publish its PID before expiry")
}

async fn wait_gone(pid: u32) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while process_running(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("lease expiry must terminate every in-flight process-group member");
}

#[tokio::test]
async fn dangerous_expiry_cancels_production_streaming_bash_process_tree() {
    let workspace = tempfile::tempdir().unwrap();
    let physical = physical_attempt_server().await;
    let shell_pid_file = workspace.path().join("shell.pid");
    let child_pid_file = workspace.path().join("child.pid");
    let script = format!(
        "echo streaming-proof; echo $$ > '{}'; sleep 30 & echo $! > '{}'; wait",
        shell_pid_file.display(),
        child_pid_file.display()
    );
    let provider = Arc::new(
        MockLlmProvider::with_tool_use("bash-lease", "Bash", json!({ "command": script }))
            .with_physical_url(physical.uri()),
    );
    let streaming_sink = Arc::new(StreamingSink::default());
    let sink: Arc<dyn OutputSink> = streaming_sink.clone();
    let mut config = bootstrap_config();
    configure_persisted_test_session(&mut config, workspace.path());
    let mut result = AgentBootstrap::new(config, workspace.path().to_string_lossy(), sink)
        .provider(provider)
        .without_channels(true)
        .with_dangerous_grant(dangerous_grant("lease-bash-e2e"))
        .build()
        .await
        .expect("Dangerous bootstrap must finish inside its one-shot lease");
    result
        .engine
        .init_session("openai", &workspace.path().to_string_lossy(), None)
        .expect("persisted session must bind the production budget authority");
    result.engine.use_recovery_test_key(&RECOVERY_TEST_KEY);
    let BootstrapResult {
        mut engine,
        cancel_root,
        ..
    } = result;
    let started = Instant::now();
    let run = tokio::spawn(async move {
        let outcome = engine.run("run the requested command", "").await;
        (engine, outcome)
    });
    let shell_pid = read_pid(&shell_pid_file).await;
    let child_pid = read_pid(&child_pid_file).await;
    assert!(process_running(shell_pid));
    assert!(process_running(child_pid));

    let (mut engine, outcome) = tokio::time::timeout(Duration::from_secs(4), run)
        .await
        .expect("lease expiry must stop the production Bash dispatch promptly")
        .expect("engine task must join");
    assert!(
        matches!(outcome, Err(AgentError::UserAborted)),
        "Dangerous expiry must surface UserAborted, got {outcome:?}"
    );
    assert!(matches!(
        engine.recovery_plan().unwrap().disposition,
        wcore_agent::recovery::RecoveryDisposition::ReconciliationRequired { .. }
    ));
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(cancel_root.is_cancelled());
    assert!(
        streaming_sink.chunks.load(Ordering::Relaxed) > 0,
        "the production dispatcher must select Bash's streaming path"
    );
    wait_gone(shell_pid).await;
    wait_gone(child_pid).await;

    let replacement = tokio_util::sync::CancellationToken::new();
    engine.set_cancel_token(replacement.clone());
    assert!(
        replacement.is_cancelled(),
        "expired bootstrapped session must reject replacement turns"
    );
}

struct SpawnThenBlockProvider {
    calls: AtomicUsize,
    child_entered: Notify,
    held_senders: Mutex<Vec<mpsc::Sender<LlmEvent>>>,
    physical_url: String,
}

impl SpawnThenBlockProvider {
    fn new(physical_url: String) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            child_entered: Notify::new(),
            held_senders: Mutex::new(Vec::new()),
            physical_url,
        }
    }
}

#[async_trait]
impl LlmProvider for SpawnThenBlockProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
        let response =
            scope_max_retries(0, builder_send_with_retry(client.get(&self.physical_url))).await?;
        if !response.status().is_success() {
            return Err(ProviderError::Api {
                status: response.status().as_u16(),
                message: "fixture response".into(),
            });
        }
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(4);
        if call == 0 {
            tokio::spawn(async move {
                let _ = tx
                    .send(LlmEvent::ToolUse {
                        id: "spawn-lease".into(),
                        name: "Spawn".into(),
                        input: json!({
                            "tasks": [{"name": "leased-child", "prompt": "wait"}]
                        }),
                        extra: None,
                    })
                    .await;
                let _ = tx
                    .send(LlmEvent::Done {
                        stop_reason: StopReason::ToolUse,
                        finish_reason: FinishReason::Stop,
                        usage: TokenUsage::default(),
                    })
                    .await;
            });
        } else {
            self.held_senders.lock().unwrap().push(tx);
            self.child_entered.notify_one();
        }
        Ok(rx)
    }
}

#[tokio::test]
async fn dangerous_expiry_reaches_bootstrapped_spawn_child() {
    let workspace = tempfile::tempdir().unwrap();
    let physical = physical_attempt_server().await;
    let provider = Arc::new(SpawnThenBlockProvider::new(physical.uri()));
    let sink: Arc<dyn OutputSink> = Arc::new(StreamingSink::default());
    let mut config = bootstrap_config();
    configure_persisted_test_session(&mut config, workspace.path());
    let mut result = AgentBootstrap::new(config, workspace.path().to_string_lossy(), sink)
        .provider(provider.clone())
        .without_channels(true)
        .with_dangerous_grant(dangerous_grant("lease-spawn-e2e"))
        .build()
        .await
        .expect("Dangerous bootstrap must finish inside its one-shot lease");
    result
        .engine
        .init_session("openai", &workspace.path().to_string_lossy(), None)
        .expect("persisted session must bind the production budget authority");
    result.engine.use_recovery_test_key(&RECOVERY_TEST_KEY);
    let BootstrapResult {
        mut engine,
        cancel_root,
        ..
    } = result;
    assert!(engine.tool_names().iter().any(|name| name == "Spawn"));
    let run = tokio::spawn(async move {
        let outcome = engine.run("delegate this task", "").await;
        (engine, outcome)
    });

    tokio::time::timeout(Duration::from_secs(2), provider.child_entered.notified())
        .await
        .expect("production Spawn tool must start the child provider before expiry");
    let (mut engine, outcome) = tokio::time::timeout(Duration::from_secs(4), run)
        .await
        .expect("lease expiry must stop the production child promptly")
        .expect("engine task must join");
    assert!(
        matches!(outcome, Err(AgentError::UserAborted)),
        "Dangerous expiry must surface UserAborted, got {outcome:?}"
    );
    assert!(matches!(
        engine.recovery_plan().unwrap().disposition,
        wcore_agent::recovery::RecoveryDisposition::ReconciliationRequired { .. }
    ));
    assert!(cancel_root.is_cancelled());
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        2,
        "one parent tool-use turn and one real child turn must execute"
    );

    let replacement = tokio_util::sync::CancellationToken::new();
    engine.set_cancel_token(replacement.clone());
    assert!(
        replacement.is_cancelled(),
        "expired session must remain terminal after child cancellation"
    );
}
