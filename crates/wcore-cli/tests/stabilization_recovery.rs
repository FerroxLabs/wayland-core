//! W04 C4 acceptance: deterministic, instrumented process cuts on real ACP REST.
//!
//! The child is this test executable, hosting the production ACP adapter and
//! engine. Journal rendezvous exist only under wcore-agent/test-utils. Ordinary
//! packaged-process controls remain in f14_sigkill_recovery; byte/snapshot cuts
//! remain in session_journal_crash_matrix_test. No test claims external rollback.

#![cfg(unix)]

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::time::timeout;
use wcore_acp::server::AcpServer;
use wcore_acp::transport::http::HttpSseTransport;
use wcore_agent::recovery::{RecoveryDisposition, RecoveryPlan};
use wcore_agent::session_journal::{JournalError, SessionJournal, ToolEffectState};
use wcore_config::config::{CliArgs, Config};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::tempenv::{self, TempEnv};
use wcore_providers::anthropic::AnthropicProvider;

#[path = "support/mock_llm.rs"]
mod mock_llm;
#[path = "support/owned_tree.rs"]
mod owned_tree;
#[path = "support/vault.rs"]
mod vault;

use mock_llm::MockLlm;
use owned_tree::OwnedTree;

const DEADLINE: Duration = Duration::from_secs(30);
const CHILD: &str = "w04_instrumented_acp_host";
const REOPEN: &str = "w04_fresh_recovery_process";

fn child_config() -> Config {
    let workspace = PathBuf::from(std::env::var("W04_WORKSPACE").expect("workspace"));
    let mut config = Config::resolve(&CliArgs {
        provider: Some("anthropic".into()),
        api_key: Some("fixture-no-real-key".into()),
        model: Some("claude-mock".into()),
        base_url: Some(std::env::var("W04_PROVIDER").expect("fixture URL")),
        project_dir: Some(workspace),
        ..Default::default()
    })
    .expect("resolve isolated child config and unlock test vault");
    config.session.enabled = true;
    config.session.require_durability = true;
    config.session.directory = PathBuf::from(std::env::var("WAYLAND_HOME").expect("home"))
        .join("sessions")
        .to_string_lossy()
        .into_owned();
    config.memory.enabled = false;
    config.builtin_tools.defer_cold.enabled = false;
    config
}

fn provider(config: &Config) -> Arc<AnthropicProvider> {
    Arc::new(
        AnthropicProvider::new(
            "fixture-no-real-key",
            &std::env::var("W04_PROVIDER").expect("fixture URL"),
            wcore_config::compat::ProviderCompat::anthropic_defaults(),
            config.debug.clone(),
        )
        .with_cache(false),
    )
}

// This ignored helper is invoked explicitly in a subprocess. Running it in the
// main test process would make vault/config environment and crash ownership race.
#[ignore]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w04_instrumented_acp_host() {
    let config = child_config();
    let force = std::env::var("W04_APPROVAL").as_deref() != Ok("1");
    let engine = wcore_cli::acp_engine::EngineTurnEngine::with_provider(
        config.clone(),
        std::env::var("W04_WORKSPACE").expect("workspace"),
        provider(&config),
    )
    .force_tools(force);
    let server = Arc::new(AcpServer::new().with_turn_engine(Arc::new(engine)));
    let router = HttpSseTransport::new(server)
        .router()
        .layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
                let deleting = request.method() == axum::http::Method::DELETE;
                let response = next.run(request).await;
                if deleting && response.status() == axum::http::StatusCode::NO_CONTENT {
                    // Handler success is established, but no response bytes have
                    // reached the socket. This wrapper belongs to the test host.
                    tokio::task::spawn_blocking(|| {
                        wcore_agent::session_journal::stabilization_test_barrier(
                            "delete_before_response",
                        );
                    })
                    .await
                    .expect("DELETE response barrier");
                }
                response
            },
        ));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ACP listener");
    let mut ready = TcpStream::connect(std::env::var("W04_READY").expect("ready address"))
        .await
        .expect("ready connection");
    ready
        .write_all(format!("http://{}\n", listener.local_addr().expect("ACP address")).as_bytes())
        .await
        .expect("publish ACP address");
    drop(ready);
    axum::serve(listener, router).await.expect("serve ACP");
}

#[ignore]
#[tokio::test]
async fn w04_fresh_recovery_process() {
    let config = child_config();
    let id = std::env::var("W04_SESSION").expect("session");
    let manager =
        wcore_agent::session::SessionManager::new(PathBuf::from(&config.session.directory), 64);
    let active = manager
        .load_for_run(&id)
        .expect("fresh process acquires writer lease");
    let mut engine = wcore_agent::bootstrap::AgentBootstrap::new(
        config.clone(),
        std::env::var("W04_WORKSPACE").expect("workspace"),
        Arc::new(wcore_agent::output::null_sink::NullSink),
    )
    .provider(provider(&config))
    .resume(active)
    .build()
    .await
    .expect("rebuild engine")
    .engine;
    let plan = engine.recovery_plan().expect("recovery projection");
    let mut refused = false;
    if let RecoveryDisposition::ReconciliationRequired { turn_id, .. } = &plan.disposition {
        refused = engine
            .resume_interrupted_turn(turn_id, &plan.cursor(), "unsafe-replay-probe")
            .await
            .is_err();
        assert!(refused, "uncertain physical tool effect must not resume");
    }
    std::fs::write(
        std::env::var("W04_REPORT").expect("report path"),
        serde_json::to_vec(&json!({"refused": refused, "budget": plan.budget,
            "disposition": format!("{:?}", plan.disposition)}))
        .expect("recovery report"),
    )
    .expect("write recovery report");
}

struct Host {
    child: OwnedTree<Child>,
    base: String,
    stderr: PathBuf,
}

fn command(env: &TempEnv, provider_url: &str, test: &str) -> (Command, vault::VaultGuard) {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args(["--exact", test, "--ignored", "--nocapture"])
        .current_dir(env.path())
        .env("HOME", env.path())
        .env("WAYLAND_HOME", env.home())
        .env("W04_WORKSPACE", env.path())
        .env("W04_PROVIDER", provider_url)
        .env_remove("WAYLAND_W04_CUT")
        .env_remove("WAYLAND_W04_BARRIER")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true);
    let guard = vault::configure_process(command.as_std_mut());
    (command, guard)
}

impl Host {
    async fn start(
        env: &TempEnv,
        provider_url: &str,
        cut: &str,
        barrier: &TcpListener,
        approval: bool,
    ) -> Self {
        let ready = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("ready listener");
        let stderr = env.path().join("w04-host-stderr.log");
        let (mut command, guard) = command(env, provider_url, CHILD);
        command
            .env(
                "W04_READY",
                ready.local_addr().expect("ready address").to_string(),
            )
            .env("WAYLAND_W04_CUT", cut)
            .env(
                "WAYLAND_W04_BARRIER",
                barrier.local_addr().expect("barrier address").to_string(),
            )
            .env("W04_APPROVAL", if approval { "1" } else { "0" })
            .stderr(Stdio::from(
                std::fs::File::create(&stderr).expect("host stderr"),
            ));
        let child = OwnedTree::new(command.spawn().expect("spawn instrumented ACP host"));
        drop(guard);
        let base = timeout(DEADLINE, async {
            let (socket, _) = ready.accept().await.expect("host ready connection");
            let mut line = String::new();
            BufReader::new(socket)
                .read_line(&mut line)
                .await
                .expect("read ACP URL");
            line.trim().to_owned()
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "host ready deadline: {}",
                std::fs::read_to_string(&stderr).unwrap_or_default()
            )
        });
        Self {
            child,
            base,
            stderr,
        }
    }

    async fn kill(mut self) {
        let pid = self.child.child_mut().id().expect("live child");
        // SAFETY: only the exact still-owned child PID is signalled.
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) }, 0);
        let status = timeout(DEADLINE, self.child.wait())
            .await
            .expect("SIGKILL deadline")
            .expect("reap child");
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}

async fn reach(barrier: &TcpListener, cut: &str, host: &Host) -> TcpStream {
    timeout(DEADLINE, async {
        let (socket, _) = barrier.accept().await.expect("accept cut");
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read cut marker");
        assert_eq!(line.trim(), cut);
        reader.into_inner()
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "cut {cut} deadline: {}",
            std::fs::read_to_string(&host.stderr).unwrap_or_default()
        )
    })
}

async fn create(client: &wcore_egress::EgressClient, host: &Host) -> String {
    let response: Value = client
        .post(format!("{}/sessions", host.base))
        .json(&json!({"tools": [], "mcp_servers": []}))
        .send()
        .await
        .expect("create REST session")
        .error_for_status()
        .expect("session created")
        .json()
        .await
        .expect("session JSON");
    response["session_id"]
        .as_str()
        .expect("session id")
        .to_owned()
}

fn journal_path(env: &TempEnv, id: &str) -> PathBuf {
    env.home().join("sessions").join(format!("{id}.journal"))
}

async fn reopen(env: &TempEnv, provider_url: &str, id: &str) -> Value {
    let report = env.path().join("w04-recovery-report.json");
    let (mut command, guard) = command(env, provider_url, REOPEN);
    command
        .env("W04_SESSION", id)
        .env("W04_REPORT", &report)
        .stderr(Stdio::piped());
    let mut child = OwnedTree::new(command.spawn().expect("spawn fresh recovery process"));
    drop(guard);
    let stderr = child.stderr.take().expect("recovery stderr");
    let capture = tokio::spawn(async move {
        let mut text = String::new();
        BufReader::new(stderr)
            .read_to_string(&mut text)
            .await
            .expect("read recovery stderr");
        text
    });
    let status = timeout(DEADLINE, child.wait())
        .await
        .expect("fresh recovery deadline")
        .expect("reap recovery");
    let diagnostics = capture.await.expect("recovery capture");
    assert!(status.success(), "fresh recovery failed: {diagnostics}");
    serde_json::from_slice(&std::fs::read(report).expect("fresh recovery report"))
        .expect("report JSON")
}

fn effect_count(path: &Path) -> usize {
    match std::fs::read_to_string(path) {
        Ok(text) => text.lines().count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("read effect counter: {error}"),
    }
}

async fn crash_tool_cut(cut: &str, effects: usize) {
    let provider_config = ProviderConfig::new(ProviderId::Anthropic, "claude-mock")
        .with_api_key("fixture-no-real-key")
        .with_known_free_cost()
        .with_base_url("http://127.0.0.1:1");
    let env = tempenv::build(&provider_config).expect("isolated profile");
    let marker = env.path().join("effect-count.txt");
    let quoted = format!("'{}'", marker.to_string_lossy().replace('\'', "'\\''"));
    let fixture = MockLlm::new()
        .tool_use(
            "Bash",
            json!({"command": format!("printf 'effect\\n' >> {quoted}"), "timeout": 10000}),
        )
        .text("completed once")
        .start()
        .await;
    let barrier = TcpListener::bind("127.0.0.1:0").await.expect("barrier");
    let host = Host::start(&env, &fixture.uri(), cut, &barrier, false).await;
    let client = wcore_egress::EgressClient::builder()
        .timeout(DEADLINE)
        .build()
        .expect("REST client");
    let id = create(&client, &host).await;
    let response = client
        .post(format!("{}/sessions/{id}/messages", host.base))
        .json(&json!({"text": "Run the exact fixture tool once.", "tools": []}))
        .send()
        .await
        .expect("send REST message")
        .error_for_status()
        .expect("admit message");
    let frames = Arc::new(std::sync::Mutex::new(String::new()));
    let captured = frames.clone();
    let drain = tokio::spawn(async move {
        let mut bytes = response.bytes_stream();
        while let Some(Ok(chunk)) = bytes.next().await {
            captured
                .lock()
                .expect("SSE capture")
                .push_str(&String::from_utf8_lossy(&chunk));
        }
    });
    let socket = reach(&barrier, cut, &host).await;
    assert_eq!(effect_count(&marker), effects, "physical side of {cut}");
    let path = journal_path(&env, &id);
    assert!(
        matches!(
            SessionJournal::open(&path, &id),
            Err(JournalError::AlreadyOwned { .. })
        ),
        "live writer lease positive control"
    );
    assert!(
        !frames.lock().expect("SSE capture").contains("event: done"),
        "wire terminal escaped cut {cut}"
    );
    host.kill().await;
    drop(socket);
    timeout(DEADLINE, drain)
        .await
        .expect("SSE drain deadline")
        .expect("SSE task");
    let journal = SessionJournal::open(&path, &id).expect("fresh writer after SIGKILL");
    let state = journal.state().expect("recover state");
    let tool = state.tools.values().next();
    match cut {
        "before_intent" => assert!(tool.is_none()),
        "intent_before_dispatch" => assert!(matches!(
            tool.map(|t| &t.effect),
            Some(ToolEffectState::Prepared)
        )),
        "physical_before_receipt" => {
            assert!(matches!(
                tool.map(|t| &t.effect),
                Some(ToolEffectState::Running)
            ));
            assert!(matches!(
                RecoveryPlan::from_journal(&journal)
                    .expect("plan")
                    .disposition,
                RecoveryDisposition::ReconciliationRequired { .. }
            ));
        }
        "receipt_before_settlement" | "settlement_before_terminal" => assert!(matches!(
            tool.map(|t| &t.effect),
            Some(ToolEffectState::Succeeded)
        )),
        _ => panic!("unknown test cut"),
    }
    drop(journal);
    let before = fixture
        .received_requests()
        .await
        .expect("fixture request counter")
        .len();
    let first = reopen(&env, &fixture.uri(), &id).await;
    let second = reopen(&env, &fixture.uri(), &id).await;
    if cut == "physical_before_receipt" {
        assert_eq!(first["refused"], true);
        assert_eq!(second["refused"], true);
    }
    assert_eq!(
        first["budget"], second["budget"],
        "restart must not settle a receipt twice"
    );
    assert_eq!(
        effect_count(&marker),
        effects,
        "recovery duplicated physical effect"
    );
    assert_eq!(
        fixture
            .received_requests()
            .await
            .expect("fixture request counter")
            .len(),
        before,
        "recovery redispatched provider"
    );
}

macro_rules! tool_cut {
    ($name:ident, $cut:literal, $effects:literal) => {
        #[tokio::test]
        async fn $name() {
            crash_tool_cut($cut, $effects).await;
        }
    };
}
tool_cut!(w04_before_intent_append, "before_intent", 0);
tool_cut!(
    w04_after_intent_before_physical_dispatch,
    "intent_before_dispatch",
    0
);
tool_cut!(
    w04_physical_completion_before_receipt,
    "physical_before_receipt",
    1
);
tool_cut!(
    w04_receipt_before_settlement,
    "receipt_before_settlement",
    1
);
tool_cut!(
    w04_settlement_before_wire_terminal,
    "settlement_before_terminal",
    1
);

async fn delete_cut(cut: &str, inject_failure: bool) {
    let config = ProviderConfig::new(ProviderId::Anthropic, "claude-mock")
        .with_api_key("fixture-no-real-key")
        .with_known_free_cost()
        .with_base_url("http://127.0.0.1:1");
    let env = tempenv::build(&config).expect("isolated delete profile");
    let marker = env.path().join("forbidden-write.txt");
    let fixture = MockLlm::new()
        .tool_use(
            "Write",
            json!({"file_path": marker, "content": "must never execute"}),
        )
        .text("unexpected provider replay")
        .start()
        .await;
    let barrier = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("DELETE barrier");
    let host = Host::start(&env, &fixture.uri(), cut, &barrier, true).await;
    let client = wcore_egress::EgressClient::builder()
        .timeout(DEADLINE)
        .build()
        .expect("REST client");
    let id = create(&client, &host).await;
    let mut response = client
        .post(format!("{}/sessions/{id}/messages", host.base))
        .json(&json!({"text": "Run fixture Write.", "tools": []}))
        .send()
        .await
        .expect("REST message")
        .error_for_status()
        .expect("admit turn");
    timeout(DEADLINE, async {
        let mut wire = String::new();
        while let Some(chunk) = response.chunk().await.expect("SSE approval") {
            wire.push_str(&String::from_utf8_lossy(&chunk));
            if wire.contains("event: approval_required") {
                return;
            }
            assert!(!wire.contains("event: error"), "{wire}");
        }
        panic!("stream ended before approval: {wire}");
    })
    .await
    .expect("approval deadline");
    let url = format!("{}/sessions/{id}", host.base);
    let deleting = tokio::spawn(async move { client.delete(url).send().await });
    let mut socket = reach(&barrier, cut, &host).await;
    assert!(
        !deleting.is_finished(),
        "DELETE response must still be withheld"
    );
    let path = journal_path(&env, &id);
    if cut == "delete_before_response" {
        let journal = SessionJournal::open(&path, &id)
            .expect("successful DELETE already relinquished writer");
        assert!(
            journal
                .state()
                .expect("state")
                .turns
                .values()
                .all(|turn| turn.completion.is_some())
        );
    }
    if inject_failure {
        socket
            .write_all(b"F")
            .await
            .expect("inject journal failure during cleanup");
        let result = timeout(DEADLINE, deleting)
            .await
            .expect("failed DELETE deadline")
            .expect("DELETE task")
            .expect("failed DELETE response");
        assert_eq!(
            result.status(),
            reqwest::StatusCode::SERVICE_UNAVAILABLE,
            "cleanup failure must not acknowledge successful deletion"
        );
    } else {
        host.kill().await;
        drop(socket);
        assert!(
            timeout(DEADLINE, deleting)
                .await
                .expect("severed DELETE deadline")
                .expect("DELETE task")
                .is_err(),
            "killed host cannot deliver receipt"
        );
        let first = reopen(&env, &fixture.uri(), &id).await;
        let second = reopen(&env, &fixture.uri(), &id).await;
        assert_eq!(first["budget"], second["budget"]);
        assert!(!marker.exists());
        assert_eq!(
            fixture.received_requests().await.expect("requests").len(),
            1
        );
        return;
    }
    host.kill().await;
    let journal =
        SessionJournal::open(&path, &id).expect("cleanup failure retains readable durable history");
    let state = journal.state().expect("cleanup state");
    assert!(
        state.turns.values().any(|turn| turn.completion.is_none()),
        "failed terminal append must not fabricate a durable completion"
    );
    assert!(!marker.exists());
    drop(journal);
    let first = reopen(&env, &fixture.uri(), &id).await;
    let second = reopen(&env, &fixture.uri(), &id).await;
    assert_eq!(first["budget"], second["budget"]);
    assert!(!marker.exists());
    assert_eq!(
        fixture.received_requests().await.expect("requests").len(),
        1
    );
}

#[tokio::test]
async fn w04_finalizer_during_delete() {
    delete_cut("delete_finalizer", false).await;
}
#[tokio::test]
async fn w04_journal_failure_during_cleanup() {
    delete_cut("delete_finalizer", true).await;
}
#[tokio::test]
async fn w04_process_death_after_delete_before_receipt_delivery() {
    delete_cut("delete_before_response", false).await;
}
