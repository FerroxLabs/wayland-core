//! Task #160 (d) — DRIVE the channel-gateway cell, do not hypothesise about it.
//!
//! `ChannelTurnDispatcher::engine_for` resumes by stable conversation id and,
//! when the session store has no record of that id, SILENTLY builds a fresh
//! session — unlike the CLI, which refuses to resume something it cannot find.
//! That is the one production seam where a per-session ceiling can re-arm
//! without anyone noticing, and it is the exact deployment shape where
//! unbounded spend matters most: a long-lived gateway answering inbound
//! messages, one new conversation at a time, forever.
//!
//! These tests run that seam for real — the production `ChannelTurnDispatcher`
//! over the production `AgentBootstrap` — with a provider that reports
//! authoritative usage against a known price, and assert what actually
//! happens.

use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use serial_test::serial;
use tempfile::TempDir;
use tokio::sync::mpsc;
use wcore_agent::channel_dispatch::ChannelTurnDispatcher;
use wcore_agent::channel_inbound::TurnDispatcher;
use wcore_agent::channel_policy::ChannelPolicyRegistry;
use wcore_budget::BudgetConfig;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::Config;
use wcore_config::credentials::CredentialsBackend;
use wcore_egress::{AllowAllPolicy, EgressClient};
use wcore_providers::retry::{builder_send_with_retry, scope_max_retries};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

static CONFIDENTIAL_ROOT: LazyLock<TempDir> =
    LazyLock::new(|| TempDir::new().expect("confidential test root"));

/// One turn bills 1,000,000 input tokens at $0.000001/token = exactly $1.00.
///
/// The per-token rate is deliberately tiny and the settled token count
/// deliberately large: admission reserves the WORST CASE for the whole request
/// context, so a coarse rate would let the reservation, rather than the
/// authoritative settlement, decide where the ceiling lands and make the
/// arithmetic below depend on prompt size.
const USD_PER_TOKEN: f64 = 0.000_001;
const BILLED_INPUT_TOKENS: u64 = 1_000_000;
const USD_PER_TURN: f64 = BILLED_INPUT_TOKENS as f64 * USD_PER_TOKEN;

/// Points `WAYLAND_HOME` at a private root so the durable daily ledger this
/// test drives is this test's own file, never the developer's.
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn rooted_at(home: &std::path::Path) -> Self {
        let keys = [
            "WAYLAND_HOME",
            "WAYLAND_VAULT_PASSPHRASE",
            "WAYLAND_VAULT_PASSPHRASE_FD",
            "XDG_DATA_HOME",
        ];
        let saved = keys
            .into_iter()
            .map(|key| (key, std::env::var(key).ok()))
            .collect();
        // SAFETY: every test in this binary is `#[serial]`, and Drop restores
        // all variables even if a test panics.
        unsafe {
            std::env::set_var("WAYLAND_HOME", home);
            std::env::set_var(
                "WAYLAND_VAULT_PASSPHRASE",
                "wcore-channel-daily-ceiling-passphrase",
            );
            std::env::remove_var("WAYLAND_VAULT_PASSPHRASE_FD");
            std::env::remove_var("XDG_DATA_HOME");
        }
        Self(saved)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, previous) in &self.0 {
            // SAFETY: see `EnvGuard::rooted_at`.
            match previous {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

/// Loopback meter: answers every call from a local fixture server, reports
/// authoritative usage, and counts how many times a provider was actually
/// reached. The loopback GET is what gives the turn a durable physical-attempt
/// identity, exactly as the production provider path does. No real provider is
/// contacted and no real money moves.
struct MeteredProvider {
    calls: Arc<AtomicUsize>,
    physical_url: String,
}

impl MeteredProvider {
    fn new(physical_url: String) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                calls: Arc::clone(&calls),
                physical_url,
            }),
            calls,
        )
    }
}

/// The loopback endpoint every metered call physically reaches.
async fn loopback_meter_endpoint() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    server
}

#[async_trait]
impl LlmProvider for MeteredProvider {
    async fn stream(&self, _: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
        let response =
            scope_max_retries(0, builder_send_with_retry(client.get(&self.physical_url))).await?;
        if !response.status().is_success() {
            return Err(ProviderError::Api {
                status: response.status().as_u16(),
                message: "loopback meter".into(),
            });
        }
        let (tx, rx) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = tx.send(LlmEvent::TextDelta("metered reply".into())).await;
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::Stop,
                    usage: TokenUsage {
                        input_tokens: BILLED_INPUT_TOKENS,
                        output_tokens: 0,
                        ..Default::default()
                    },
                })
                .await;
        });
        Ok(rx)
    }
}

/// A model name no pricing table knows, so the compat rates below are the
/// authority and the arithmetic in this test is exact.
fn metered_compat() -> ProviderCompat {
    ProviderCompat {
        cost_per_input_token: Some(USD_PER_TOKEN),
        cost_per_output_token: Some(USD_PER_TOKEN),
        cost_per_cache_read_token: Some(USD_PER_TOKEN),
        cost_per_cache_write_token: Some(USD_PER_TOKEN),
        ..ProviderCompat::anthropic_defaults()
    }
}

fn gateway_config(root: &std::path::Path, daily_cap_usd: Option<f64>) -> Config {
    let mut config = Config {
        model: "loopback-meter-model".into(),
        max_tokens: 1,
        max_tokens_explicit: true,
        compat: metered_compat(),
        budget: BudgetConfig {
            max_daily_cost_usd: daily_cap_usd,
            ..Default::default()
        },
        ..Default::default()
    };
    config.session.directory = root.join("sessions").to_string_lossy().into_owned();
    config.storage.credentials.backend = CredentialsBackend::EncryptedFile {
        cipher_path: CONFIDENTIAL_ROOT.path().join("credentials.enc"),
        key_params_path: CONFIDENTIAL_ROOT.path().join("credentials.kdf.json"),
    };
    config
}

fn inbound(n: usize) -> wcore_channels::IncomingMessage {
    wcore_channels::IncomingMessage::new(
        format!("inbound-{n}"),
        format!("chat-{n}"),
        "remote-sender",
        "answer me",
        1_700_000_000 + n as i64,
    )
}

/// A distinct kernel session key per inbound conversation. This is the gateway
/// shape: every new conversation is a brand-new session, so NO per-session cap
/// can ever bind the deployment as a whole.
fn session_key(n: usize) -> String {
    format!("agent:main:slack:dm:conversation-{n}")
}

async fn dispatch_turn(
    dispatcher: &ChannelTurnDispatcher,
    n: usize,
) -> anyhow::Result<Option<String>> {
    dispatcher
        .dispatch(&session_key(n), "slack", &inbound(n))
        .await
}

fn ledger_committed_usd(home: &std::path::Path) -> f64 {
    let path = home.join("budget").join("daily-spend.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return 0.0;
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("ledger is valid JSON");
    value["subjects"]["default"]["committed_usd"]
        .as_f64()
        .unwrap_or(0.0)
}

/// KNOWN-NEGATIVE arm. With no daily ceiling configured, a gateway answering
/// one new conversation after another bills without limit — every individual
/// session is correctly inside its own budget the entire time.
///
/// If this test ever stops billing all six turns, the positive test below
/// proves nothing.
#[tokio::test]
#[serial]
async fn without_a_daily_ceiling_a_gateway_bills_every_new_conversation() {
    let home = TempDir::new().expect("home");
    let _env = EnvGuard::rooted_at(home.path());
    let workdir = TempDir::new().expect("workdir");

    let meter = loopback_meter_endpoint().await;
    let (provider, calls) = MeteredProvider::new(meter.uri());
    let dispatcher = ChannelTurnDispatcher::new(
        gateway_config(home.path(), None),
        workdir.path().to_string_lossy().into_owned(),
        provider,
        Arc::new(ChannelPolicyRegistry::default()),
        None,
    );

    for n in 0..6 {
        dispatch_turn(&dispatcher, n)
            .await
            .expect("turn dispatches");
    }

    assert_eq!(
        calls.load(Ordering::SeqCst),
        6,
        "with no cross-session ceiling every fresh conversation reaches the provider"
    );
}

/// The ceiling binds the gateway, conversation by conversation, even though
/// every one of those conversations is a brand-new session with a brand-new
/// per-session budget.
#[tokio::test]
#[serial]
async fn a_daily_ceiling_bounds_a_gateway_that_opens_a_new_session_per_message() {
    let home = TempDir::new().expect("home");
    let _env = EnvGuard::rooted_at(home.path());
    let workdir = TempDir::new().expect("workdir");

    // Two turns' worth. The third turn's admission sees $2.00 already
    // committed and must refuse before the provider is reached.
    let cap = 2.0 * USD_PER_TURN;
    let meter = loopback_meter_endpoint().await;
    let (provider, calls) = MeteredProvider::new(meter.uri());
    let dispatcher = ChannelTurnDispatcher::new(
        gateway_config(home.path(), Some(cap)),
        workdir.path().to_string_lossy().into_owned(),
        provider,
        Arc::new(ChannelPolicyRegistry::default()),
        None,
    );

    for n in 0..6 {
        // A refused turn is still a clean dispatch: the gateway answers the
        // sender, it does not crash the poller.
        dispatch_turn(&dispatcher, n)
            .await
            .expect("a budget refusal must stay a clean dispatch, not a gateway fault");
    }

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "the durable daily ceiling must stop the gateway at the cap, \
         not once per session"
    );
    let committed = ledger_committed_usd(home.path());
    assert!(
        (committed - cap).abs() < 1e-6,
        "the ledger must account exactly what was billed: committed={committed}, cap={cap}"
    );
}

/// The reported-but-never-driven cell: an emptied session store.
///
/// `engine_for` probes the store, finds nothing, and silently creates a fresh
/// session — no refusal, no warning. That resets the per-session budget by
/// design. The durable daily ceiling must survive it, because the authority is
/// the ledger, not the session.
#[tokio::test]
#[serial]
async fn an_emptied_session_store_silently_re_arms_the_session_but_not_the_ceiling() {
    let home = TempDir::new().expect("home");
    let _env = EnvGuard::rooted_at(home.path());
    let workdir = TempDir::new().expect("workdir");
    let cap = 2.0 * USD_PER_TURN;
    let config = gateway_config(home.path(), Some(cap));
    let sessions = std::path::PathBuf::from(&config.session.directory);

    let meter = loopback_meter_endpoint().await;
    let (provider, calls) = MeteredProvider::new(meter.uri());
    let dispatcher = ChannelTurnDispatcher::new(
        config.clone(),
        workdir.path().to_string_lossy().into_owned(),
        Arc::clone(&provider) as Arc<dyn LlmProvider>,
        Arc::new(ChannelPolicyRegistry::default()),
        None,
    );
    dispatch_turn(&dispatcher, 0).await.expect("first turn");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(dispatcher);

    // Wipe the store the way a prune, an eviction, or a fresh container does.
    // A CLI `--resume` against a missing id refuses; the gateway does not.
    if sessions.exists() {
        std::fs::remove_dir_all(&sessions).expect("wipe the session store");
    }

    // A new process would also start with an empty engine pool.
    let dispatcher = ChannelTurnDispatcher::new(
        config,
        workdir.path().to_string_lossy().into_owned(),
        provider as Arc<dyn LlmProvider>,
        Arc::new(ChannelPolicyRegistry::default()),
        None,
    );
    for n in 0..5 {
        dispatch_turn(&dispatcher, n)
            .await
            .expect("the gateway silently rebuilds a missing session rather than refusing");
    }

    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "wiping the session store re-arms the SESSION budget, so exactly one more \
         call may bill; the daily ledger must then hold the line"
    );
    let committed = ledger_committed_usd(home.path());
    assert!(
        (committed - cap).abs() < 1e-6,
        "spend survived the wiped session store: committed={committed}, cap={cap}"
    );
}
