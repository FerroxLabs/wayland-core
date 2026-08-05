//! M5.bootstrap-wiring — end-to-end smoke for the three coupled fixes:
//!
//! 1. `Config { session_cap: Some(BudgetConfig { .. }), ..Default::default() }`
//!    proves `impl Default for Config` + the new `session_cap` field
//!    compose under spread syntax.
//!
//! 2. `AgentBootstrap::with_span_sink(...)` plumbs an `Arc<dyn SpanSink>`
//!    into the boot pipeline; the `ObservabilityBudgetEventBridge` then
//!    forwards `BudgetEvent::Charge` into the JSON span channel — the
//!    M3.3-style bridge that previously had no production install
//!    point.
//!
//! 3. The installed durable budget authority enforces the configured cap.
//!    A separate test asserts the Smart Default also installs a finite
//!    authority when no legacy `session_cap` block is present.

use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use serde_json::Value;
use serial_test::serial;
use tempfile::TempDir;
use tokio::sync::mpsc;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_budget::BudgetConfig;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::Config;
use wcore_config::credentials::CredentialsBackend;
use wcore_egress::{AllowAllPolicy, EgressClient};
use wcore_observability::sink::SpanSink;
use wcore_providers::retry::{builder_send_with_retry, scope_max_retries};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

static CONFIDENTIAL_ROOT: LazyLock<TempDir> =
    LazyLock::new(|| TempDir::new().expect("confidential test root"));

/// SpanSink that captures every emitted JSON value into a shared buffer.
/// Distinct from `wcore_observability::sink::InMemorySink` only in that
/// we want the buffer handle separately addressable from the trait
/// object so the test can assert on collected events without re-cloning
/// the sink.
struct CollectingSink {
    events: Arc<Mutex<Vec<Value>>>,
}

struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn confidential_vault() -> Self {
        let keys = [
            "WAYLAND_HOME",
            "WAYLAND_VAULT_PASSPHRASE",
            "WAYLAND_VAULT_PASSPHRASE_FD",
        ];
        let saved = keys
            .into_iter()
            .map(|key| (key, std::env::var(key).ok()))
            .collect();
        // SAFETY: every test in this binary is `#[serial]`, and Drop restores
        // all variables even if a test panics.
        unsafe {
            std::env::set_var("WAYLAND_HOME", CONFIDENTIAL_ROOT.path());
            std::env::set_var(
                "WAYLAND_VAULT_PASSPHRASE",
                "wcore-bootstrap-budget-test-passphrase",
            );
            std::env::remove_var("WAYLAND_VAULT_PASSPHRASE_FD");
        }
        Self(saved)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, previous) in &self.0 {
            // SAFETY: see `EnvGuard::confidential_vault`.
            match previous {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

impl SpanSink for CollectingSink {
    fn emit(&self, trace: &Value) {
        if let Ok(mut g) = self.events.lock() {
            g.push(trace.clone());
        }
    }
}

fn null_output() -> Arc<dyn OutputSink> {
    Arc::new(NullSink)
}

struct CountingProvider {
    calls: Arc<AtomicUsize>,
    usage: TokenUsage,
    physical_url: String,
}

impl CountingProvider {
    fn new(usage: TokenUsage, physical_url: String) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                calls: Arc::clone(&calls),
                usage,
                physical_url,
            }),
            calls,
        )
    }
}

#[async_trait]
impl LlmProvider for CountingProvider {
    async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
        let response =
            scope_max_retries(0, builder_send_with_retry(client.get(&self.physical_url))).await?;
        if !response.status().is_success() {
            return Err(ProviderError::Api {
                status: response.status().as_u16(),
                message: "fixture response".into(),
            });
        }
        let (tx, rx) = mpsc::channel(2);
        let usage = self.usage.clone();
        tokio::spawn(async move {
            let _ = tx.send(LlmEvent::TextDelta("done".into())).await;
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::Stop,
                    usage,
                })
                .await;
        });
        Ok(rx)
    }
}

fn priced_config(session_cap: Option<BudgetConfig>) -> Config {
    Config {
        model: "claude-haiku-4-5".into(),
        max_tokens: 1,
        max_tokens_explicit: true,
        compat: ProviderCompat::anthropic_defaults(),
        session_cap,
        ..Default::default()
    }
}

fn configure_persisted_test_storage(config: &mut Config, root: &std::path::Path) {
    config.session.directory = root.join("sessions").to_string_lossy().into_owned();
    config.storage.credentials.backend = CredentialsBackend::EncryptedFile {
        cipher_path: CONFIDENTIAL_ROOT.path().join("credentials.enc"),
        key_params_path: CONFIDENTIAL_ROOT.path().join("credentials.kdf.json"),
    };
}

fn output_ceiling_usage(output_tokens: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: 1,
        output_tokens,
        ..Default::default()
    }
}

fn bind_persisted_session(result: &mut wcore_agent::bootstrap::BootstrapResult, workspace: &str) {
    result
        .engine
        .init_session("anthropic", workspace, None)
        .expect("persisted session must bind the durable budget authority");
}

async fn physical_attempt_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
#[serial]
async fn bootstrap_with_session_cap_enforces_authority_and_emits_budget_events() {
    let _env = EnvGuard::confidential_vault();
    let tmp = TempDir::new().expect("workdir");
    let workspace = tmp.path().to_str().expect("workdir utf-8").to_string();

    let buffer = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink: Arc<dyn SpanSink> = Arc::new(CollectingSink {
        events: Arc::clone(&buffer),
    });

    // Acceptance criterion 1: spread syntax over `Default` with the new
    // `session_cap` field carries through cleanly. If `Config` ever loses
    // `Default` or grows another mandatory field, this stops compiling
    // — that's the regression guard.
    let mut cfg = priced_config(Some(BudgetConfig {
        max_cost_usd: Some(10.0),
        max_tokens_out: Some(1),
        ..Default::default()
    }));
    configure_persisted_test_storage(&mut cfg, tmp.path());
    let physical = physical_attempt_server().await;
    let (provider, calls) = CountingProvider::new(output_ceiling_usage(1), physical.uri());

    let mut result = AgentBootstrap::new(cfg, &workspace, null_output())
        .provider(provider)
        .with_span_sink(Arc::clone(&sink))
        .build()
        .await
        .expect("bootstrap should succeed");
    bind_persisted_session(&mut result, &workspace);

    let first = result
        .engine
        .run("first", "msg-1")
        .await
        .expect("the first provider call must fit the configured cap");
    assert_eq!(first.stop_reason, StopReason::EndTurn);
    let denied = result
        .engine
        .run("second", "msg-2")
        .await
        .expect("budget denial is a clean terminal result");
    assert_eq!(denied.stop_reason, StopReason::MaxTurns);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the configured cap must deny the second provider dispatch"
    );

    // The first settlement and second admission denial must both cross the
    // production observability bridge owned by the durable authority.
    let events = buffer.lock().expect("buffer lock");
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|v| v.get("kind").and_then(|k| k.as_str()))
        .collect();
    assert!(
        kinds.contains(&"charge"),
        "expected a BudgetEvent::Charge in the captured span events, got {kinds:?}"
    );
    assert!(
        kinds.contains(&"cap_block"),
        "expected a BudgetEvent::CapBlock for the rejected charge, got {kinds:?}"
    );
}

#[tokio::test]
#[serial]
async fn bootstrap_without_session_cap_installs_finite_smart_default_authority() {
    let _env = EnvGuard::confidential_vault();
    let tmp = TempDir::new().expect("workdir");
    let workspace = tmp.path().to_str().expect("workdir utf-8").to_string();

    // No session_cap and no span sink — F11 still installs Smart Default's
    // 1,000,000-token output ceiling.
    let mut cfg = priced_config(None);
    configure_persisted_test_storage(&mut cfg, tmp.path());
    let physical = physical_attempt_server().await;
    let (provider, calls) = CountingProvider::new(output_ceiling_usage(1_000_000), physical.uri());
    let mut result = AgentBootstrap::new(cfg, &workspace, null_output())
        .provider(provider)
        .build()
        .await
        .expect("bootstrap should succeed");
    bind_persisted_session(&mut result, &workspace);

    let first = result
        .engine
        .run("first", "msg-1")
        .await
        .expect("the first provider call must fit the Smart default");
    assert_eq!(first.stop_reason, StopReason::EndTurn);
    let denied = result
        .engine
        .run("second", "msg-2")
        .await
        .expect("Smart budget denial is a clean terminal result");
    assert_eq!(denied.stop_reason, StopReason::MaxTurns);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the finite Smart default must deny the second provider dispatch"
    );
}

#[tokio::test]
#[serial]
async fn partial_session_cap_keeps_smart_token_ceiling() {
    let _env = EnvGuard::confidential_vault();
    let tmp = TempDir::new().expect("workdir");
    let workspace = tmp.path().to_str().expect("workdir utf-8").to_string();
    let mut cfg = priced_config(Some(BudgetConfig {
        max_cost_usd: Some(50.0),
        ..Default::default()
    }));
    configure_persisted_test_storage(&mut cfg, tmp.path());
    let physical = physical_attempt_server().await;
    let (provider, calls) = CountingProvider::new(output_ceiling_usage(1_000_000), physical.uri());

    let mut result = AgentBootstrap::new(cfg, &workspace, null_output())
        .provider(provider)
        .build()
        .await
        .expect("bootstrap should succeed");
    bind_persisted_session(&mut result, &workspace);
    let first = result
        .engine
        .run("first", "msg-1")
        .await
        .expect("the first provider call must fit the inherited Smart ceiling");
    assert_eq!(first.stop_reason, StopReason::EndTurn);
    let denied = result
        .engine
        .run("second", "msg-2")
        .await
        .expect("inherited Smart budget denial is a clean terminal result");
    assert_eq!(denied.stop_reason, StopReason::MaxTurns);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "omitted token fields must retain the finite Smart ceiling"
    );
}
