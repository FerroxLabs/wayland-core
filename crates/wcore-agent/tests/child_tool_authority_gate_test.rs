//! F21-02-03 regression — a spawned child cannot invoke a tool its parent
//! session was narrowed out of.
//!
//! Phase 21 recorded `PolicyGate` as unreachable: `set_policy_gate` had zero
//! callers workspace-wide and both production engine constructors hard-coded
//! `policy_gate: None`, so the parent's tool authority was never carried into
//! any child. A child's registry is built from its own requested
//! `allowed_tools` (defaulting to the read-only set `Read`/`Grep`/`Glob`) and
//! was never intersected with what the parent actually holds — so a session
//! deliberately narrowed away from `Read` still produced children that could
//! `Read`.
//!
//! These tests drive the real production seam: `AgentBootstrap::build` →
//! `BootstrapResult::host_children` → `AgentSpawner::execute_resolved_launch`.
//! Nothing is stubbed except the LLM, which is scripted to make one tool call
//! and then echo the tool result back as its final text so the child's actual
//! dispatch outcome is observable in `SubAgentResult::text`.
//!
//! RED-before evidence: with the `engine.set_policy_gate(..)` install in
//! `AgentSpawner::execute_resolved_launch` removed, `child_cannot_read_when_
//! parent_session_was_narrowed_away_from_read` fails with the sentinel file
//! contents in the child's text — i.e. the child really did read a file its
//! parent could not.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use serial_test::serial;
use tokio::sync::mpsc;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::null_sink::NullSink;
use wcore_agent::spawner::SubAgentConfig;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, StopReason, TokenUsage};

/// Contents written into the probe file. If this string reaches the child's
/// final text, the child successfully invoked `Read`.
const SENTINEL: &str = "f21-02-03-parent-only-secret";

/// Scripted provider: first turn issues one tool call; every later turn emits
/// the most recent `ToolResult` content it can see in the conversation as the
/// assistant's text. That makes the child's dispatch outcome — the real file
/// contents, or the gate's denial string — observable in `SubAgentResult`.
struct EchoToolResultProvider {
    tool_name: String,
    tool_input: serde_json::Value,
    issued: Mutex<bool>,
}

impl EchoToolResultProvider {
    fn new(tool_name: &str, tool_input: serde_json::Value) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            tool_input,
            issued: Mutex::new(false),
        }
    }
}

#[async_trait]
impl LlmProvider for EchoToolResultProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let first = {
            let mut issued = self.issued.lock().expect("issued flag");
            let was_first = !*issued;
            *issued = true;
            was_first
        };

        let events = if first {
            vec![
                LlmEvent::ToolUse {
                    id: "call-1".to_string(),
                    name: self.tool_name.clone(),
                    input: self.tool_input.clone(),
                    extra: None,
                },
                LlmEvent::Done {
                    stop_reason: StopReason::ToolUse,
                    finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
                    usage: TokenUsage::default(),
                },
            ]
        } else {
            let echoed = request
                .messages
                .iter()
                .rev()
                .flat_map(|message| message.content.iter().rev())
                .find_map(|block| match block {
                    ContentBlock::ToolResult { content, .. } => Some(content.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "<no tool result in conversation>".to_string());
            vec![
                LlmEvent::TextDelta(echoed),
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                    usage: TokenUsage::default(),
                },
            ]
        };

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

fn base_config(sessions: &std::path::Path) -> Config {
    let mut config = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(3),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    config.session.directory = sessions.to_string_lossy().into_owned();
    config.memory.enabled = false;
    // Without this the child's `ToolConfirmer` denies every call because the
    // test process has no terminal — which would make the assertions below
    // pass for the wrong reason. This is the exact vacuity that made Phase
    // 21's first corpus run produce twelve meaningless REFUSED verdicts.
    config.tools.auto_approve = true;
    config
}

/// Pin plugin discovery at an empty dir so a plugin installed on the host or
/// CI runner cannot contribute extra tools and change the parent's authority.
fn isolated_plugins() -> (tempfile::TempDir, Vec<(&'static str, Option<String>)>) {
    let dir = tempfile::TempDir::new().expect("plugins dir");
    let saved = vec![(
        "WAYLAND_PLUGINS_DIR",
        std::env::var("WAYLAND_PLUGINS_DIR").ok(),
    )];
    // SAFETY: every test in this file is `#[serial]`; nothing else mutates
    // process env concurrently and `restore_env` puts the prior value back.
    unsafe { std::env::set_var("WAYLAND_PLUGINS_DIR", dir.path()) };
    (dir, saved)
}

fn restore_env(saved: Vec<(&'static str, Option<String>)>) {
    for (key, prev) in saved {
        // SAFETY: see `isolated_plugins`.
        match prev {
            Some(value) => unsafe { std::env::set_var(key, value) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
}

/// Spawn one host child from a bootstrapped session and return its text.
///
/// `parent_tools` is the persona-style allowlist the parent session is
/// narrowed to; `None` leaves the parent's full toolset intact.
async fn child_text_after_reading(parent_tools: Option<Vec<String>>) -> String {
    let workdir = tempfile::TempDir::new().expect("workdir");
    let sessions = tempfile::TempDir::new().expect("sessions");
    // The probe MUST live inside the session workspace. A path outside it is
    // refused by the child's workspace sandbox before the tool runs, which
    // would make the deny assertion below pass without the policy gate ever
    // being consulted — a vacuous green.
    let probe = workdir.path().join("probe.txt");
    std::fs::write(&probe, SENTINEL).expect("write probe");
    let provider = Arc::new(EchoToolResultProvider::new(
        "Read",
        serde_json::json!({ "file_path": probe.to_string_lossy() }),
    ));

    let mut bootstrap = AgentBootstrap::new(
        base_config(sessions.path()),
        workdir.path().to_string_lossy(),
        Arc::new(NullSink) as Arc<dyn wcore_agent::output::OutputSink>,
    )
    .provider(provider)
    .without_channels(true)
    .defer_config_mcp(true);
    if let Some(allowed) = parent_tools {
        bootstrap = bootstrap.tool_allowlist(allowed);
    }
    let mut result = bootstrap.build().await.expect("production bootstrap");
    // Bind the durable session; the host-child control plane refuses to launch
    // against an unbound session authority.
    result
        .engine
        .init_session(
            "test-provider",
            &workdir.path().to_string_lossy(),
            Some("f2102030"),
        )
        .expect("bind durable session");

    result
        .host_children
        .spawn_child(SubAgentConfig {
            name: "probe-child".into(),
            prompt: "read the probe file".into(),
            max_turns: 3,
            max_tokens: 512,
            system_prompt: None,
            provider: None,
            model: None,
            temperature: None,
        })
        .await
        .text
}

/// THE REGRESSION. A session narrowed to `Grep`/`Glob` has no `Read`
/// authority. Its child's own registry still contains `Read` (the default
/// read-only child set), so before the parent gate was carried into the child
/// engine the child read a file the parent could not — a strict widening of
/// the parent's tool authority.
#[tokio::test]
#[serial]
async fn child_cannot_read_when_parent_session_was_narrowed_away_from_read() {
    let (_plugins, saved) = isolated_plugins();

    let text = child_text_after_reading(Some(vec!["Grep".to_string(), "Glob".to_string()])).await;

    restore_env(saved);

    assert!(
        !text.contains(SENTINEL),
        "child read a file its parent session had no Read authority for — the \
         parent's tool restriction was widened by the child. Child text: {text}"
    );
    assert!(
        text.contains("Denied by policy"),
        "expected the inherited parent tool authority to deny Read. Child text: {text}"
    );
}

/// The other half of the contract: a session that declared no narrowing must
/// keep working exactly as before. The inherited authority is the parent's own
/// registry, so it grants `Read` and the child reads normally. If this fails,
/// the fix is over-restricting real sessions.
#[tokio::test]
#[serial]
async fn child_still_reads_when_parent_session_holds_read() {
    let (_plugins, saved) = isolated_plugins();

    let text = child_text_after_reading(None).await;

    restore_env(saved);

    assert!(
        !text.contains("Denied by policy"),
        "an unrestricted parent must not deny its child a tool it holds itself. \
         Child text: {text}"
    );
    assert!(
        text.contains(SENTINEL),
        "child should have read the probe file. Child text: {text}"
    );
}
