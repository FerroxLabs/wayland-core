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

/// How long the fixtures below lease Dangerous authority for.
///
/// This is NOT an assertion bound — every assertion in this file is stated
/// relative to the resolver's own monotonic deadline, so this value cannot
/// make a failing behaviour pass. It is the budget the FIXTURE has to get a
/// real Bash (or Spawn) child running before the lease it is testing expires
/// underneath it.
///
/// core#337: at 3s it did not fit. The resolver binds the deadline when the
/// grant is created, which is before `AgentBootstrap::build()`, so bootstrap
/// plus the first provider round-trip plus tool dispatch all spend lease. On
/// hetzner-dsm (96 cores, ambient load 45-90) that setup was measured at
/// 0.20s alone but up to 2.94s across 208 samples at 48-, 64- and 96-way
/// parallelism -- 98% of a 3s lease. The tests were racing their own lease,
/// and lost by killing Bash before it could publish its PID.
///
/// 10s is >3x the measured worst case. Re-derive it by timing `grant
/// creation -> Bash tool dispatch` at the parallelism the suite actually
/// runs at; raise it if that ever approaches this value.
const FIXTURE_LEASE_TTL: Duration = Duration::from_secs(10);

/// Anti-hang budget for waits whose CONTENT is the assertion.
///
/// Deliberately far larger than anything measured (worst observed
/// cancellation-to-return was 2.5s at 64-way): these guards exist so a
/// genuine deadlock fails the suite instead of wedging it, not to bound
/// latency. Bounding latency here is what made core#337 flaky.
const ANTI_HANG: Duration = Duration::from_secs(30);

fn dangerous_grant(activation_id: &str) -> wcore_types::execution_policy::DangerousSessionGrant {
    resolve_dangerous_launch(
        &BaselineExecutionPolicy::smart(ApprovalPolicy::Prompt, PolicySource::Default),
        DangerousLaunchRequest::cli(FIXTURE_LEASE_TTL.as_secs(), activation_id),
        0,
    )
    .expect("trusted local launch must resolve")
}

/// The wall-clock instant this grant's authority ends, on the same monotonic
/// clock the runtime arms. Every timing assertion below is stated against
/// this, never against how long fixture setup happened to take.
fn deadline_of(grant: &wcore_types::execution_policy::DangerousSessionGrant) -> Instant {
    Instant::now()
        + grant
            .remaining_ttl()
            .expect("a freshly resolved grant must still be live")
}

/// One of four independent hand-rolled zombie checks this workspace grew, all
/// Linux-only and already disagreeing with each other on the malformed-stat
/// branch. Replaced by the single cross-platform probe; see
/// `.planning/ZOMBIE-PROBE.md`.
fn process_running(pid: u32) -> bool {
    wcore_types::process_liveness::process_is_alive(pid)
}

async fn read_pid(path: &std::path::Path) -> u32 {
    tokio::time::timeout(ANTI_HANG, async {
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
    .expect(
        "Dangerous Bash never published its PID. Either the tool did not run \
         at all, or the lease expired before it could -- check FIXTURE_LEASE_TTL \
         against how long setup is taking under load.",
    )
}

async fn wait_gone(pid: u32) {
    tokio::time::timeout(ANTI_HANG, async {
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
        "echo streaming-proof; echo $$ > '{}'; sleep 600 & echo $! > '{}'; wait",
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
    let bash_grant = dangerous_grant("lease-bash-e2e");
    let deadline = deadline_of(&bash_grant);
    let mut result = AgentBootstrap::new(config, workspace.path().to_string_lossy(), sink)
        .provider(provider)
        .without_channels(true)
        .with_dangerous_grant(bash_grant)
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
    let run = tokio::spawn(async move {
        let outcome = engine.run("run the requested command", "").await;
        (engine, outcome)
    });
    let shell_pid = read_pid(&shell_pid_file).await;
    let child_pid = read_pid(&child_pid_file).await;
    assert!(process_running(shell_pid));
    assert!(process_running(child_pid));
    // Fixture health, stated explicitly so losing this race reads as what it
    // is instead of surfacing later as a confusing timeout: there is nothing
    // to prove about expiry unless a live process tree exists while the lease
    // is still granted.
    assert!(
        Instant::now() < deadline,
        "fixture setup outran the {FIXTURE_LEASE_TTL:?} lease -- the process tree \
         only became observable after the authority it was meant to outlive had \
         already expired"
    );

    let (mut engine, outcome) = tokio::time::timeout(ANTI_HANG, run)
        .await
        .expect("lease expiry must stop the production Bash dispatch")
        .expect("engine task must join");
    assert!(
        matches!(outcome, Err(AgentError::UserAborted)),
        "Dangerous expiry must surface UserAborted, got {outcome:?}"
    );
    assert!(matches!(
        engine.recovery_plan().unwrap().disposition,
        wcore_agent::recovery::RecoveryDisposition::ReconciliationRequired { .. }
    ));
    // The other direction, which the old wall-clock bound could not check at
    // all: authority must not be revoked EARLY. A resolver that bound the
    // deadline to the wrong clock, or an arm that mis-computed the remaining
    // TTL, would abort the turn before this instant.
    assert!(
        Instant::now() >= deadline,
        "the Dangerous session aborted BEFORE its lease deadline"
    );
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
    let spawn_grant = dangerous_grant("lease-spawn-e2e");
    let deadline = deadline_of(&spawn_grant);
    let mut result = AgentBootstrap::new(config, workspace.path().to_string_lossy(), sink)
        .provider(provider.clone())
        .without_channels(true)
        .with_dangerous_grant(spawn_grant)
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

    tokio::time::timeout(ANTI_HANG, provider.child_entered.notified())
        .await
        .expect("production Spawn tool must start the child provider before expiry");
    // Same fixture-health check as the Bash proof above.
    assert!(
        Instant::now() < deadline,
        "fixture setup outran the {FIXTURE_LEASE_TTL:?} lease -- the child engine \
         only started after the authority it was meant to outlive had expired"
    );
    let (mut engine, outcome) = tokio::time::timeout(ANTI_HANG, run)
        .await
        .expect("lease expiry must stop the production child")
        .expect("engine task must join");
    assert!(
        matches!(outcome, Err(AgentError::UserAborted)),
        "Dangerous expiry must surface UserAborted, got {outcome:?}"
    );
    assert!(matches!(
        engine.recovery_plan().unwrap().disposition,
        wcore_agent::recovery::RecoveryDisposition::ReconciliationRequired { .. }
    ));
    assert!(
        Instant::now() >= deadline,
        "the Dangerous session aborted BEFORE its lease deadline"
    );
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
