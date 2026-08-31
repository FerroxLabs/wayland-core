//! FerroxLabs/wayland#1266 c3 — the child's own failure category survives the
//! relay onto the frame the design calls AUTHORITATIVE.
//!
//! WHY THIS FILE EXISTS AND `issue_1266_c3_subagent_relay_test.rs` IS NOT
//! ENOUGH. That file is well built — a real spawned child, both halves of c3,
//! a differ-guard, a measured red arm — and it asserts on the DIAGNOSTIC
//! frame: the child's own `emit_error`, relayed on the best-effort stream. The
//! 0.13.12 re-grade found that `ChannelSink::relay_terminal` — whose own doc
//! says "the single authoritative terminal ... a terminal that can reorder
//! behind diagnostics is not authoritative evidence" — hardcoded
//! `FailureCategory::Unknown` on its `Failed` arm, and that deleting the
//! category from THAT frame changed nothing the existing file observes. So a
//! child dying on a context limit reached the parent's host as `unknown` on
//! the one frame the host is told to trust.
//!
//! This file grades that frame. It uses `ChannelSink::new_with_terminal` — the
//! constructor production relay paths use — so the assertion is made on the
//! dedicated `SubAgentTerminalRelay` lane rather than on the diagnostic
//! stream, and it drives `AgentSpawner::spawn_parallel_with_per_task_extras`,
//! the production spawn path, so a real child `AgentEngine` classifies its own
//! failure at its own call site.
//!
//! Both halves of c3 are asserted, because c3 asks for both and a remap can
//! serve only one:
//!   * a child that died on a context ceiling arrives as `context_limit`;
//!   * a child that died on an opaque upstream arrives as `unknown`, NOT
//!     upgraded to a plausible-looking `tool_runtime` — the guess #1237 c4
//!     forbids, made on the child's behalf.
//!
//! RED ARM (re-runnable): in `crates/wcore-agent/src/spawner.rs`, replace
//! `let failure_category = error.failure_category();` in
//! `execute_resolved_launch`'s `Err(error)` arm with
//! `let failure_category = wcore_protocol::events::FailureCategory::Unknown;`
//! — the value the authoritative frame carried before this change. `touch` the
//! file, confirm `cargo check -p wcore-agent --tests` RC=0 so the red is
//! behaviour and not a build break, and rebuild.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{bound_test_spawner_arc, test_config};
use tokio::sync::mpsc;
use wcore_agent::agents::channel_sink::{
    CHANNEL_CAPACITY, ChannelSink, SubAgentRelay, SubAgentTerminalRelay,
};
use wcore_agent::spawner::{AgentSpawner, SpawnExtras, SubAgentConfig};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};

type Outcome = Result<Vec<LlmEvent>, ProviderError>;
/// `ProviderError` is not `Clone`, so the repeating tail is a FACTORY rather
/// than a stored value: a child that retries cannot run off the end of the
/// script and change the exit under test.
type OutcomeFactory = Arc<dyn Fn() -> Outcome + Send + Sync>;

struct ScriptedProvider {
    script: Mutex<VecDeque<Outcome>>,
    tail: OutcomeFactory,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(&self, _r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let next = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| (self.tail)());
        let events = next?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

fn sub_config(name: &str) -> SubAgentConfig {
    SubAgentConfig {
        name: name.to_string(),
        prompt: format!("Task for {name}"),
        max_turns: 5,
        max_tokens: 1024,
        system_prompt: None,
        provider: None,
        model: None,
        temperature: None,
    }
}

/// What reached the parent off ONE real spawned child.
struct Terminals {
    /// `category` on every `error` frame that arrived on the AUTHORITATIVE
    /// terminal lane.
    authoritative: Vec<String>,
    /// The same, off the best-effort diagnostic stream. Collected so a test
    /// can prove the two lanes are being told apart rather than assumed to be.
    diagnostic: Vec<String>,
}

/// Spawn one real child through the production spawner and read the categories
/// off the JSON that reaches the parent — not out of a Rust value, because a
/// category that only exists as an in-process enum is not one any host can
/// branch on.
async fn terminals_from_a_failing_child(
    tail: OutcomeFactory,
    mutate: impl FnOnce(&mut wcore_config::config::Config),
) -> Terminals {
    let provider = Arc::new(ScriptedProvider {
        script: Mutex::new(VecDeque::new()),
        tail,
    });
    let mut config = test_config();
    mutate(&mut config);
    let (spawner, _session_root) = bound_test_spawner_arc(AgentSpawner::new(provider, config));

    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(CHANNEL_CAPACITY);
    let (terminal_tx, mut terminal_rx) = mpsc::channel::<SubAgentTerminalRelay>(CHANNEL_CAPACITY);
    let extras = SpawnExtras {
        channel_sink: Some(Arc::new(ChannelSink::new_with_terminal(
            "spawn:0:child".to_string(),
            "child".to_string(),
            tx.clone(),
            terminal_tx.clone(),
        ))),
        agent_name: Some("child".to_string()),
        parent_call_id: Some("spawn:0:child".to_string()),
    };
    drop(tx);
    drop(terminal_tx);

    let stream: Arc<Mutex<Vec<SubAgentRelay>>> = Arc::new(Mutex::new(Vec::new()));
    let stream_clone = Arc::clone(&stream);
    let drain = tokio::spawn(async move {
        while let Some(relay) = rx.recv().await {
            stream_clone.lock().unwrap().push(relay);
        }
    });
    let terminals: Arc<Mutex<Vec<SubAgentTerminalRelay>>> = Arc::new(Mutex::new(Vec::new()));
    let terminals_clone = Arc::clone(&terminals);
    let terminal_drain = tokio::spawn(async move {
        while let Some(relay) = terminal_rx.recv().await {
            terminals_clone.lock().unwrap().push(relay);
        }
    });

    spawner
        .spawn_parallel_with_per_task_extras(vec![(sub_config("child"), extras)])
        .await;
    drain.await.unwrap();
    terminal_drain.await.unwrap();

    let categories = |inner: &serde_json::Value| -> Option<String> {
        let error = inner.get("error")?;
        // The frame must still identify itself as a child's terminal: a
        // category read off some OTHER frame would not be evidence.
        assert_eq!(
            error.get("code").and_then(|c| c.as_str()),
            Some("sub_agent_error"),
            "a relayed child error frame must identify itself as one: {inner:?}"
        );
        Some(
            error
                .get("category")
                .and_then(|c| c.as_str())
                .unwrap_or("<absent>")
                .to_string(),
        )
    };

    Terminals {
        authoritative: terminals
            .lock()
            .unwrap()
            .iter()
            .filter_map(|t| categories(&t.relay.inner))
            .collect(),
        diagnostic: stream
            .lock()
            .unwrap()
            .iter()
            .filter_map(|r| categories(&r.inner))
            .collect(),
    }
}

/// A child refused on an unworkable context window: far below
/// `minimum_workable_window()`, so the child refuses before any provider call
/// and this cannot be flaky on the network.
async fn child_that_hit_a_context_ceiling() -> Terminals {
    terminals_from_a_failing_child(
        Arc::new(|| Ok(vec![LlmEvent::TextDelta("unreachable".to_string())])),
        |config| config.compact.context_window = Some(1_024),
    )
    .await
}

/// A child that died on an opaque non-2xx — the #1184 split core cannot decide
/// from inside this repo.
async fn child_that_hit_an_opaque_upstream() -> Terminals {
    terminals_from_a_failing_child(
        Arc::new(|| {
            Err(ProviderError::Api {
                status: 400,
                message: "{\"error\":{\"message\":\"upstream said no\"}}".to_string(),
            })
        }),
        |_| {},
    )
    .await
}

/// c3, first half, on the authoritative frame.
#[tokio::test]
async fn a_context_ceiling_child_reaches_the_authoritative_terminal_as_context_limit() {
    let seen = child_that_hit_a_context_ceiling().await;

    assert!(
        !seen.authoritative.is_empty(),
        "control: the child's terminal must reach the parent's AUTHORITATIVE \
         lane at all -- if it does not, the assertion below passes vacuously. \
         Diagnostic-lane categories seen: {:?}",
        seen.diagnostic
    );
    for category in &seen.authoritative {
        assert_eq!(
            category, "context_limit",
            "wayland#1266 c3: the CHILD classified this as a context ceiling at \
             its own call site, and `relay_terminal` must carry that to the \
             parent's host instead of the `unknown` it hardcoded. \
             Authoritative: {:?}",
            seen.authoritative
        );
    }
}

/// c3's CONTROL, the half a remap cannot serve — and the wrong-refusal guard
/// for this change: carrying a category through must not start MANUFACTURING
/// one where none is known. A plausible-looking `tool_runtime` here would be
/// the guess #1237 c4 forbids, made on the child's behalf.
#[tokio::test]
async fn an_opaque_upstream_child_still_reaches_the_authoritative_terminal_as_unknown() {
    let seen = child_that_hit_an_opaque_upstream().await;

    assert!(
        !seen.authoritative.is_empty(),
        "control: the child's terminal must reach the parent's AUTHORITATIVE lane at all"
    );
    for category in &seen.authoritative {
        assert_eq!(
            category, "unknown",
            "wayland#1266 c3 control: core cannot tell a provider rate limit \
             from a router failure (#1184) and must not start guessing on a \
             CHILD's behalf either. Authoritative: {:?}",
            seen.authoritative
        );
    }
}

/// The two arms are not the same constant.
///
/// Each test above asserts ONE value, so a relay wired to emit that value
/// unconditionally would satisfy either alone. This asserts two differently
/// failing children produce DIFFERENT categories on the authoritative frame,
/// which no hardcode -- `Unknown` included -- can pass.
#[tokio::test]
async fn two_differently_failing_children_do_not_relay_the_same_authoritative_category() {
    let ceiling = child_that_hit_a_context_ceiling().await;
    let opaque = child_that_hit_an_opaque_upstream().await;

    assert!(
        !ceiling.authoritative.is_empty() && !opaque.authoritative.is_empty(),
        "control: both children reached the authoritative lane"
    );
    assert_ne!(
        ceiling.authoritative[0], opaque.authoritative[0],
        "two differently-failing children relayed the SAME authoritative \
         category, so that frame is carrying a constant rather than the \
         child's own classification"
    );
}

/// The lane under test is the authoritative one, and it is a DIFFERENT lane.
///
/// Without this, a change that accidentally routed the diagnostic stream into
/// the terminal collector would make every assertion above pass while
/// `relay_terminal` itself stayed broken -- which is exactly how the previous
/// grading of c3 came to be made on the wrong frame.
#[tokio::test]
async fn the_authoritative_terminal_is_a_separate_lane_from_the_diagnostics() {
    let seen = child_that_hit_a_context_ceiling().await;

    assert_eq!(
        seen.authoritative.len(),
        1,
        "`relay_terminal` is once-only: exactly one authoritative terminal is \
         owed per child. Got {:?}",
        seen.authoritative
    );
    assert!(
        !seen.diagnostic.is_empty(),
        "positive control for the split: the child also emits at least one \
         DIAGNOSTIC error frame. If this is empty the two lanes are not being \
         told apart -- they are both empty for some other reason."
    );
}
