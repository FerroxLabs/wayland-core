//! wayland#559 c5 + c6 — what the agentic SUB-CALLS actually put on the wire.
//!
//! #559 asked two things this file answers with measurement rather than
//! reasoning:
//!
//! - **c5** — "investigate the agentic sub-call count per turn and whether
//!   intermediate context can be trimmed/cached between sub-calls". The count
//!   itself is not the defect: one dispatch per tool round is the minimum an
//!   agentic turn can do, and this file pins it (no hidden extra dispatch).
//!   What made turn 26 cost 4.88M input tokens is that every one of those
//!   sub-calls re-billed the whole context UNCACHED. So the measurement here
//!   is the cacheable one: consecutive sub-calls must be byte-identical up to
//!   the cache write point, which is exactly the condition under which a
//!   sub-call costs its delta instead of the whole prompt.
//!
//! - **c6** — the skill-router hint and `PrePrompt` hook contributions are
//!   injected into a per-turn CLONE of history, so the message they land on is
//!   re-sent WITHOUT them on the next dispatch. Writing a cache entry at that
//!   message produces an entry nothing can ever read; on turn 1 that message
//!   is the entire conversation. Every dispatch here is checked: the tail that
//!   carries a transient contribution is never the write point.
//!
//! Every assertion reads the `LlmRequest` the provider was actually handed.
//! The no-hook control runs the same machinery with nothing injected, so a
//! capture that silently stopped working cannot make a claim pass vacuously.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use wcore_agent::engine::AgentEngine;
use wcore_agent::hooks::HookDispatcher;
use wcore_agent::output::OutputSink;
use wcore_agent::plugins::runner::PluginHook;
use wcore_agent::test_utils::TestSink;
use wcore_plugin_api::registry::hooks::HookPhase;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{
    ContentBlock, FinishReason, Message, MessageCacheHint, StopReason, TokenUsage,
};

use common::{MockTool, test_config};

/// A contribution string a PrePrompt hook returns every time it is asked. It
/// is the same text each dispatch, which is the KINDEST case for the cache —
/// and it still poisons the prefix, because the block lives only in the clone.
const HOOK_TEXT: &str = "PREPROMPT-CONTRIBUTION-9F3A";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct RecordingProvider {
    scripts: Mutex<Vec<Vec<LlmEvent>>>,
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        let mut scripts = self.scripts.lock().unwrap();
        let events = if scripts.len() > 1 {
            scripts.remove(0)
        } else {
            scripts[0].clone()
        };
        drop(scripts);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

struct ConstantDispatcher;

#[async_trait]
impl HookDispatcher for ConstantDispatcher {
    async fn dispatch(&self, _: &str, _: &str, _: HookPhase) -> Option<String> {
        Some(HOOK_TEXT.to_string())
    }
}

fn tool_round(i: usize) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: format!("call-{i}"),
            name: "probe".to_string(),
            input: json!({ "round": i }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage::default(),
        },
    ]
}

fn final_round() -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta("done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        },
    ]
}

/// Run ONE agentic turn that makes `tool_rounds` tool calls, and return every
/// `LlmRequest` the provider was handed, in dispatch order.
async fn dispatches_for_turn(tool_rounds: usize, install_hook: bool) -> Vec<LlmRequest> {
    let mut scripts: Vec<Vec<LlmEvent>> = (0..tool_rounds).map(tool_round).collect();
    scripts.push(final_round());

    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(RecordingProvider {
        scripts: Mutex::new(scripts),
        requests: Arc::clone(&requests),
    });

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(MockTool::new("probe", "probe output", false)));

    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        tools,
        Arc::new(TestSink::new()) as Arc<dyn OutputSink>,
    );
    if install_hook {
        engine.register_plugin_hooks(vec![PluginHook {
            plugin: "test-plugin".to_string(),
            phase: HookPhase::PrePrompt,
            name: "contribute".to_string(),
        }]);
        engine.set_hook_dispatcher(Arc::new(ConstantDispatcher));
    }
    engine
        .run("please use the probe tool", "msg-1")
        .await
        .expect("the scripted provider answers cleanly");

    requests.lock().unwrap().clone()
}

// ---------------------------------------------------------------------------
// Shape helpers, all read off the captured requests
// ---------------------------------------------------------------------------

fn write_point(request: &LlmRequest) -> Option<usize> {
    request
        .messages
        .iter()
        .rposition(|m| m.cache_breakpoint == Some(MessageCacheHint::Breakpoint))
}

fn transient_indices(request: &LlmRequest) -> Vec<usize> {
    request
        .messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.cache_breakpoint == Some(MessageCacheHint::Transient))
        .map(|(i, _)| i)
        .collect()
}

/// Serialize a message's CONTENT only — the cache hint is metadata the
/// provider consumes, not prompt bytes, so it must not affect this comparison.
fn content_bytes(message: &Message) -> String {
    serde_json::to_string(&message.content).expect("messages serialize")
}

fn carries_hook_text(message: &Message) -> bool {
    message.content.iter().any(|b| match b {
        ContentBlock::Text { text } => text.contains(HOOK_TEXT),
        _ => false,
    })
}

// ===========================================================================
// c5 — the sub-call count, and whether it needs reducing
// ===========================================================================

/// The count itself: one dispatch per tool round plus one for the answer, and
/// nothing else. There is no hidden extra full-context sub-call to remove —
/// which is the half of Ask 2 that asks whether the count can come down.
#[tokio::test]
async fn an_agentic_turn_dispatches_exactly_once_per_tool_round_plus_the_answer() {
    for rounds in [0usize, 1, 3, 5] {
        let dispatches = dispatches_for_turn(rounds, false).await;
        assert_eq!(
            dispatches.len(),
            rounds + 1,
            "{rounds} tool rounds must cost exactly {} dispatches",
            rounds + 1
        );
    }
}

/// The half that matters: each sub-call's prompt is the previous sub-call's
/// CACHED PREFIX plus new content, byte for byte. That is the condition under
/// which sub-call N costs its delta rather than the whole context — so the
/// count does not need reducing, the caching did. Measured on the real
/// dispatched requests, at every consecutive pair.
#[tokio::test]
async fn every_sub_call_extends_the_previous_sub_calls_cached_prefix() {
    let dispatches = dispatches_for_turn(5, false).await;
    assert_eq!(dispatches.len(), 6, "harness must produce 6 sub-calls");

    let mut previous_write_point = 0usize;
    for pair in dispatches.windows(2) {
        let (earlier, later) = (&pair[0], &pair[1]);
        let wp = write_point(earlier).expect("every sub-call must have a cache write point");
        assert!(
            wp >= previous_write_point,
            "the write point must advance, never retreat: {wp} after {previous_write_point}"
        );
        previous_write_point = wp;

        assert!(
            later.messages.len() > wp,
            "the next sub-call must still contain the cached prefix"
        );
        for i in 0..=wp {
            assert_eq!(
                content_bytes(&earlier.messages[i]),
                content_bytes(&later.messages[i]),
                "message {i} inside the cached prefix changed between sub-calls — \
                 everything from here on re-bills at full price"
            );
        }
    }
}

// ===========================================================================
// c6 — the transient tail is never a cache write point
// ===========================================================================

/// The precondition, stated as its own assertion so nothing below can pass
/// vacuously: the hook contribution really is injected into every dispatch,
/// and really is absent from that same message on the next one.
#[tokio::test]
async fn the_transient_contribution_is_present_then_gone_on_the_next_sub_call() {
    let dispatches = dispatches_for_turn(3, true).await;
    assert!(dispatches.len() >= 2);

    for (n, request) in dispatches.iter().enumerate() {
        assert!(
            carries_hook_text(request.messages.last().expect("non-empty")),
            "dispatch {n} must carry the PrePrompt contribution on its tail"
        );
    }
    for pair in dispatches.windows(2) {
        let (earlier, later) = (&pair[0], &pair[1]);
        let tail = earlier.messages.len() - 1;
        assert!(
            !carries_hook_text(&later.messages[tail]),
            "message {tail} must lose the transient block on the next dispatch — \
             that disappearance is the whole defect"
        );
    }
}

/// Turn 1 of the reported session: the first dispatch's tail IS the head of
/// the conversation. No cache entry may be written at it.
#[tokio::test]
async fn the_first_dispatch_writes_no_cache_entry_at_a_transient_head() {
    let dispatches = dispatches_for_turn(2, true).await;
    let first = &dispatches[0];

    assert_eq!(
        first.messages.len(),
        1,
        "precondition: turn 1's tail and head are the same message"
    );
    assert_eq!(transient_indices(first), vec![0]);
    assert_eq!(
        write_point(first),
        None,
        "no message may be a cache write point when the only one is transient"
    );
}

/// Every later dispatch: the write point exists and is strictly behind the
/// transient tail.
#[tokio::test]
async fn no_dispatch_ever_writes_a_cache_entry_at_a_transient_message() {
    let dispatches = dispatches_for_turn(4, true).await;

    for (n, request) in dispatches.iter().enumerate() {
        let tail = request.messages.len() - 1;
        assert_eq!(
            transient_indices(request),
            vec![tail],
            "dispatch {n}: the tail, and only the tail, is transient"
        );
        if let Some(wp) = write_point(request) {
            assert!(
                wp < tail,
                "dispatch {n}: the write point ({wp}) must be strictly behind the \
                 transient tail ({tail})"
            );
        }
    }
}

/// Even with a transient tail on every dispatch, the cached prefix still grows
/// and stays byte-stable — the c5 property must survive the c6 fix.
#[tokio::test]
async fn the_cached_prefix_still_extends_with_a_transient_tail() {
    let dispatches = dispatches_for_turn(4, true).await;

    let mut seen_a_write_point = false;
    for pair in dispatches.windows(2) {
        let (earlier, later) = (&pair[0], &pair[1]);
        let Some(wp) = write_point(earlier) else {
            continue;
        };
        seen_a_write_point = true;
        for i in 0..=wp {
            assert_eq!(
                content_bytes(&earlier.messages[i]),
                content_bytes(&later.messages[i]),
                "message {i} inside the cached prefix changed between sub-calls"
            );
        }
    }
    assert!(
        seen_a_write_point,
        "at least one dispatch must have had a write point, or this proves nothing"
    );
}

/// NEGATIVE CONTROL. With nothing injected, the tail is still the write point
/// and nothing is stamped transient. Without this, every assertion above would
/// pass on a change that simply stopped marking messages at all.
#[tokio::test]
async fn without_a_transient_contribution_the_tail_is_still_the_write_point() {
    let dispatches = dispatches_for_turn(3, false).await;

    for (n, request) in dispatches.iter().enumerate() {
        let tail = request.messages.len() - 1;
        assert!(
            transient_indices(request).is_empty(),
            "dispatch {n}: nothing was injected, so nothing may be stamped transient"
        );
        assert_eq!(
            write_point(request),
            Some(tail),
            "dispatch {n}: the tail must hold the write point on a clean turn"
        );
    }
}
