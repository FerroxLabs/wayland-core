//! FerroxLabs/wayland#1266 c3 — a sub-agent's OWN failure category survives
//! the relay to the parent's host.
//!
//! WHY THIS FILE EXISTS. #1266 c3 was graded on lane/f13-protocol against the
//! TYPE — `ChannelSink::emit_error` takes the category as an argument and
//! passes it through, so there is no value left for the relay to substitute —
//! and the ledger recorded, rather than implied away, that a dedicated
//! parent/child integration test spawning a REAL sub-agent was still owed.
//! This is that test.
//!
//! It is not a restatement of the type-level argument. It drives
//! `AgentSpawner::spawn_parallel_with_per_task_extras` — the production spawn
//! path — so a real child `AgentEngine` classifies its own failure at its own
//! call site, hands it to its own `emit_error`, and the assertion is made on
//! the JSON that reaches the parent's drain channel. Every layer between the
//! child's decision and the host's frame is real.
//!
//! Both of c3's halves are asserted, because c3 asks for both and a remap can
//! only serve one:
//!   * a child that died on a context ceiling arrives as `context_limit`;
//!   * a child that died on an opaque upstream arrives as `unknown`, NOT
//!     upgraded to a plausible-looking `tool_runtime` — the guess #1237 c4
//!     forbids, made on the child's behalf.
//!
//! RED ARM (recorded, re-runnable): in
//! `crates/wcore-agent/src/agents/channel_sink.rs`, replace the `category`
//! pass-through in `impl OutputSink for ChannelSink :: emit_error` with
//! `category: wcore_protocol::events::FailureCategory::ToolRuntime` (the
//! #1237 hardcode this criterion replaced), `touch` the file and rebuild.
//! `a_child_that_died_on_a_context_ceiling_reaches_the_parent_as_context_limit`
//! goes RED, and so does the opaque control — which is the point of keeping
//! both: the hardcode is wrong in two directions at once.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{bound_test_spawner_arc, test_config};
use tokio::sync::mpsc;
use wcore_agent::agents::channel_sink::{CHANNEL_CAPACITY, ChannelSink, SubAgentRelay};
use wcore_agent::spawner::{AgentSpawner, SpawnExtras, SubAgentConfig};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};

/// One scripted outcome per `stream()` call; the tail repeats so a child that
/// retries cannot run off the end of the script and change the exit under test.
type Outcome = Result<Vec<LlmEvent>, ProviderError>;
/// `ProviderError` is not `Clone`, so the repeating tail is a FACTORY rather
/// than a stored value.
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

/// Spawn ONE real child through the production spawner and return every
/// `category` string that reached the parent's drain on an `error` frame.
///
/// The categories are read out of the relayed JSON rather than out of a Rust
/// value, because the JSON is what the host actually receives — a category
/// that only exists as an in-process enum is not one any host can branch on.
async fn categories_relayed_from_a_failing_child(
    script: Vec<Outcome>,
    tail: OutcomeFactory,
    mutate: impl FnOnce(&mut wcore_config::config::Config),
) -> Vec<String> {
    let provider = Arc::new(ScriptedProvider {
        script: Mutex::new(script.into_iter().collect()),
        tail,
    });
    let mut config = test_config();
    mutate(&mut config);
    let (spawner, _session_root) = bound_test_spawner_arc(AgentSpawner::new(provider, config));

    let (tx, mut rx) = mpsc::channel::<SubAgentRelay>(CHANNEL_CAPACITY);
    let extras = SpawnExtras {
        channel_sink: Some(Arc::new(ChannelSink::new(
            "spawn:0:child".to_string(),
            "child".to_string(),
            tx.clone(),
        ))),
        agent_name: Some("child".to_string()),
        parent_call_id: Some("spawn:0:child".to_string()),
    };
    drop(tx);

    let relays: Arc<Mutex<Vec<SubAgentRelay>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_clone = Arc::clone(&relays);
    let drain = tokio::spawn(async move {
        while let Some(relay) = rx.recv().await {
            sink_clone.lock().unwrap().push(relay);
        }
    });

    spawner
        .spawn_parallel_with_per_task_extras(vec![(sub_config("child"), extras)])
        .await;
    drain.await.unwrap();

    let relays = relays.lock().unwrap();
    relays
        .iter()
        .filter_map(|relay| {
            let error = relay.inner.get("error")?;
            // Assert the frame really is the sub-agent relay shape while we
            // are here: a category on some OTHER frame would not be evidence.
            assert_eq!(
                error.get("code").and_then(|c| c.as_str()),
                Some("sub_agent_error"),
                "a relayed error frame must still identify itself as a child's: {relay:?}"
            );
            Some(
                error
                    .get("category")
                    .and_then(|c| c.as_str())
                    .unwrap_or("<absent>")
                    .to_string(),
            )
        })
        .collect()
}

/// c3, first half. The child engine refuses on an unworkable context window —
/// the same in-band exit #1266's own `context_limit` test uses, but reached
/// here inside a real spawned child. The parent's host must be told
/// `context_limit`, not the `tool_runtime` #1237 hardcoded for every child.
#[tokio::test]
async fn a_child_that_died_on_a_context_ceiling_reaches_the_parent_as_context_limit() {
    let categories = categories_relayed_from_a_failing_child(
        vec![],
        Arc::new(|| Ok(vec![LlmEvent::TextDelta("unreachable".to_string())])),
        |config| {
            // Far below `minimum_workable_window()`: the child refuses before
            // any provider call, so this cannot be flaky on the network.
            config.compact.context_window = Some(1_024);
        },
    )
    .await;

    assert!(
        !categories.is_empty(),
        "control: the child's failure must reach the parent's drain as an error          frame at all -- if it does not, every assertion below passes vacuously"
    );
    assert!(
        categories.iter().any(|c| c == "context_limit"),
        "wayland#1266 c3: the CHILD engine classified this as a context ceiling          at its own call site, and the relay must carry that classification to          the parent's host rather than replacing it. Categories relayed:          {categories:?}"
    );
}

/// c3's CONTROL, the half a remap cannot serve. A child that died on an opaque
/// non-2xx is exactly the #1184 split core cannot decide, and #1237 c4 forbids
/// guessing it. The relay must NOT upgrade it to something plausible-looking
/// on the child's behalf.
#[tokio::test]
async fn a_child_that_died_on_an_opaque_upstream_still_reaches_the_parent_as_unknown() {
    let opaque: OutcomeFactory = Arc::new(|| {
        Err(ProviderError::Api {
            status: 400,
            message: "{\"error\":{\"message\":\"upstream said no\"}}".to_string(),
        })
    });
    let categories = categories_relayed_from_a_failing_child(vec![], opaque, |_| {}).await;

    assert!(
        !categories.is_empty(),
        "control: the child's failure must reach the parent's drain at all"
    );
    for category in &categories {
        assert_eq!(
            category, "unknown",
            "wayland#1266 c3 control: core cannot tell a provider rate limit              from a router failure (#1184) and must not start guessing on a              CHILD's behalf either. Categories relayed: {categories:?}"
        );
    }
}

/// The two arms are not the same constant.
///
/// Each test above asserts ONE value, so a relay wired to emit that value
/// unconditionally would satisfy either of them alone. This asserts the two
/// child failures produce DIFFERENT categories at the parent, which no
/// hardcode -- including the `ToolRuntime` one this criterion replaced -- can
/// pass.
#[tokio::test]
async fn two_differently_failing_children_do_not_relay_the_same_category() {
    let ceiling = categories_relayed_from_a_failing_child(
        vec![],
        Arc::new(|| Ok(vec![LlmEvent::TextDelta("unreachable".to_string())])),
        |config| config.compact.context_window = Some(1_024),
    )
    .await;
    let opaque = categories_relayed_from_a_failing_child(
        vec![],
        Arc::new(|| {
            Err(ProviderError::Api {
                status: 400,
                message: "opaque".to_string(),
            })
        }),
        |_| {},
    )
    .await;

    assert!(
        !ceiling.is_empty() && !opaque.is_empty(),
        "control: both children relayed something"
    );
    assert_ne!(
        ceiling[0], opaque[0],
        "two differently-failing children relayed the SAME category, so the          boundary is carrying a constant rather than the child's own          classification"
    );
}
