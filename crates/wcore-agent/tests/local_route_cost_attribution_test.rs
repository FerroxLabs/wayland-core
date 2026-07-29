//! C4-F3 — the cache/cost ledger must record the route that SERVED the turn.
//!
//! `cache_ledger_engine_test.rs` proves a real `AgentEngine::run()` writes a
//! ledger with the right numbers in it. It does not ask whose numbers they
//! are: every config it drives is hand-built with
//! `ProviderCompat::anthropic_defaults()`, so `provider` reads `anthropic`
//! and is correct there by construction.
//!
//! The defect lives one layer up. `make_plugin_provider_router` (wcore-cli)
//! claims every `ollama:`-prefixed model and serves it locally, but
//! `ProviderType` has no Ollama variant, so `Config::resolve` handed that
//! local turn the configured REMOTE provider's compat — and
//! `compat.provider_type()` is the only thing the ledger, `TurnTrace`, the
//! budget reservation and the journalled attempt identity read. Measured live:
//! `ollama:smollm2:135m`, running on local hardware for nothing, recorded as
//! `provider=anthropic` and billed $0.0756 at Anthropic's family rate.
//!
//! So the compat here comes from `Config::resolve`, NOT from a preset written
//! into the test. That is the whole point: a test that constructs
//! `ollama_defaults()` itself proves only that the ledger copies a field, and
//! passes against the unfixed engine. This one goes resolver → engine → the
//! JSON file on disk.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serial_test::serial;
use tempfile::TempDir;
use tokio::sync::mpsc;

use wcore_agent::cache_ledger::CacheLedger;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{CliArgs, Config};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::test_config;

/// Mock provider replaying one scripted round-trip.
struct ScriptedProvider {
    turns: Mutex<VecDeque<Vec<LlmEvent>>>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                usage: TokenUsage::default(),
            }]
        });
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for e in events {
                let _ = tx.send(e).await;
            }
        });
        Ok(rx)
    }
}

fn one_text_turn(input: u64, output: u64) -> Arc<ScriptedProvider> {
    Arc::new(ScriptedProvider {
        turns: Mutex::new(VecDeque::from(vec![vec![
            LlmEvent::TextDelta("done".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                usage: TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                    ..Default::default()
                },
            },
        ]])),
    })
}

/// Read the single ledger the engine wrote into `dir`.
fn read_only_ledger(dir: &std::path::Path) -> CacheLedger {
    let files: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("no ledger directory at {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    assert_eq!(
        files.len(),
        1,
        "expected exactly one ledger in {}, found {files:?}",
        dir.display()
    );
    let raw = std::fs::read(&files[0]).unwrap();
    serde_json::from_slice(&raw).unwrap_or_else(|e| panic!("ledger is not decodable: {e}"))
}

/// Resolve a REAL `Config` the way the CLI does, hermetically, and hand back
/// the compat it produced. Nothing in this test may name a preset.
fn resolved_compat_for(model: &str) -> ProviderCompat {
    let project = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    let _home = HomeGuard::enter(home.path());
    let cli = CliArgs {
        provider: Some("anthropic".into()),
        api_key: Some("test-key-not-a-real-credential".into()),
        base_url: None,
        model: Some(model.into()),
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: Some(project.path().to_path_buf()),
    };
    Config::resolve(&cli)
        .unwrap_or_else(|e| panic!("resolve for {model} failed: {e:#}"))
        .compat
}

async fn ledger_row_provider_for(model: &str) -> (String, f64) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.model = model.to_string();
    config.compat = resolved_compat_for(model);

    let mut engine = AgentEngine::new_with_provider(
        one_text_turn(500, 300),
        config,
        ToolRegistry::new(),
        Arc::new(TerminalSink::new(true)) as Arc<dyn OutputSink>,
    );
    engine.set_cache_ledger_dir(dir.path());
    engine.run("go", "msg-1").await.expect("run should succeed");

    let ledger = read_only_ledger(dir.path());
    assert_eq!(ledger.turns.len(), 1, "one round-trip was scripted");
    (ledger.turns[0].provider.clone(), ledger.turns[0].cost_usd)
}

#[tokio::test]
#[serial]
async fn a_local_turn_is_recorded_under_the_route_that_served_it() {
    let (provider, cost_usd) = ledger_row_provider_for("ollama:smollm2:135m").await;

    assert_eq!(
        provider, "ollama",
        "the ledger row must name the route that served the turn. `{provider}` \
         is the configured compatibility profile — the operator reading this \
         ledger would attribute local inference to a cloud vendor, and the \
         budget path reads the same value."
    );
    assert_eq!(
        cost_usd, 0.0,
        "a local turn spent nothing; the cloud family rate must not reach the \
         ledger (this is the $0.0756 that was billed for free hardware)"
    );
}

/// CONTROL. Without this a build that hard-wired every ledger row to `ollama`
/// would pass the test above while erasing all real cloud spend.
#[tokio::test]
#[serial]
async fn a_remote_turn_is_still_recorded_under_its_own_provider_and_costs_money() {
    let (provider, cost_usd) = ledger_row_provider_for("claude-sonnet-4-6").await;

    assert_eq!(provider, "anthropic");
    assert!(
        cost_usd > 0.0,
        "a remote Anthropic turn of 500 in / 300 out is not free; got {cost_usd}"
    );
}

/// Confines resolution to a throwaway home so the host's own config cannot
/// supply a `[provider.compat]` override. On the shared build box the real
/// `~/.wayland/` is populated, so this is load-bearing, not hygiene.
struct HomeGuard {
    prev: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn enter(path: &std::path::Path) -> Self {
        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe { std::env::set_var("WAYLAND_HOME", path) };
        Self { prev }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
    }
}
