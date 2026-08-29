//! wayland#1150 c4, second half — "prompt/KV cache is reused where possible",
//! measured on the provider shape the reporter is actually on.
//!
//! #559's `prompt_cache_prefix_test.rs` measures cache reuse through the
//! `MessageCacheHint::Breakpoint` a request carries. That instrument reads
//! nothing on an OpenAI-shaped endpoint: `mark_cache_boundaries`
//! (`wcore-observability::cache`) returns before writing any breakpoint when
//! `compat.cache_message_breakpoints()` is false, which it is for every
//! OpenAI-compatible route. #559 c6's own recorded residual says as much — "on
//! implicit-cache providers there is no write point to move".
//!
//! #1150's reporter is on LM Studio over an OpenAI-compatible endpoint. There,
//! reuse is not a marker the client places; it is a property of the BYTES. The
//! endpoint caches on an exact leading-prefix match, so a dispatch costs its
//! delta if and only if it repeats the previous dispatch's messages verbatim
//! and appends. That is the claim this file measures, end to end, off the real
//! `LlmRequest` the provider was handed by the real engine loop.
//!
//! It also measures the interaction c4's two halves create. The first half is a
//! CEILING that rewrites old tool results in place
//! (`compact::micro::bound_accumulated_tool_results`). A rewrite inside the
//! prefix invalidates an implicit cache from that point on, so "not re-sent
//! whole" and "cache is reused" pull against each other. The ceiling's answer
//! is epoch quantization — `ToolResultsConfig::epoch_results`, whose own doc
//! says it exists "so the provider's contiguous prefix cache holds end to end".
//! Whether it does was never measured on the wire. Both tests below are on the
//! same engine, the same loop and the same provider trait the product uses.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::test_utils::TestSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, Message, MessageCacheHint, StopReason, TokenUsage};

#[path = "common/mod.rs"]
mod common;
use common::MockTool;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Replays one script per dispatch, in order, and keeps every request.
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

fn tool_round(tag: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: format!("call-{tag}"),
            name: "probe".to_string(),
            input: json!({ "tag": tag }),
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

/// The reporter's route: an OpenAI-compatible endpoint, which is an
/// IMPLICIT-cache provider — no breakpoints, prefix bytes or nothing.
fn openai_shaped_config() -> Config {
    let mut cfg = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "issue-1150-local-unlisted".into(),
        max_tokens: 1024,
        max_turns: Some(20),
        system_prompt: Some("You are a test assistant.".to_string()),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    cfg.tools.auto_approve = true;
    cfg.session.enabled = false;
    cfg
}

/// Drive `turns` user turns, each making `rounds` tool calls, on ONE engine, and
/// return every `LlmRequest` the provider was handed across the whole session.
async fn session_dispatches(
    turns: usize,
    rounds: usize,
    result_bytes: usize,
    cfg: Config,
) -> Vec<LlmRequest> {
    let mut scripts: Vec<Vec<LlmEvent>> = Vec::new();
    for turn in 0..turns {
        for round in 0..rounds {
            scripts.push(tool_round(&format!("t{turn}r{round}")));
        }
        scripts.push(final_round());
    }

    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(RecordingProvider {
        scripts: Mutex::new(scripts),
        requests: Arc::clone(&requests),
    });

    let mut tools = ToolRegistry::new();
    tools.register(Box::new(MockTool::new(
        "probe",
        &"P".repeat(result_bytes),
        false,
    )));

    let mut engine = AgentEngine::new_with_provider(
        provider,
        cfg,
        tools,
        Arc::new(TestSink::new()) as Arc<dyn OutputSink>,
    );
    for turn in 0..turns {
        engine
            .run(
                &format!("turn {turn}: please use the probe tool"),
                &format!("msg-{turn}"),
            )
            .await
            .expect("the scripted provider answers cleanly");
    }
    requests.lock().unwrap().clone()
}

/// Content only. The cache hint is metadata a provider consumes, not prompt
/// bytes, so it must not enter this comparison.
fn content_bytes(message: &Message) -> String {
    serde_json::to_string(&message.content).expect("messages serialize")
}

/// How many leading messages of `a` and `b` are byte-identical — the length of
/// the prefix an implicit cache can serve from.
fn shared_prefix(a: &LlmRequest, b: &LlmRequest) -> usize {
    let n = a.messages.len().min(b.messages.len());
    let mut i = 0;
    while i < n && content_bytes(&a.messages[i]) == content_bytes(&b.messages[i]) {
        i += 1;
    }
    i
}

fn breakpoints(request: &LlmRequest) -> usize {
    request
        .messages
        .iter()
        .filter(|m| m.cache_breakpoint == Some(MessageCacheHint::Breakpoint))
        .count()
}

// ===========================================================================

/// THE PRECONDITION, asserted rather than described: on this route the client
/// places no cache write point at all, so every claim below is a claim about
/// bytes. Without this the two tests could pass while silently measuring the
/// Anthropic breakpoint path #559 already covers.
#[tokio::test]
async fn an_openai_shaped_route_places_no_cache_write_point() {
    let dispatches = session_dispatches(2, 2, 64, openai_shaped_config()).await;
    assert_eq!(dispatches.len(), 6, "2 turns x (2 tool rounds + 1 answer)");
    let total: usize = dispatches.iter().map(breakpoints).sum();
    assert_eq!(
        total, 0,
        "this route emitted {total} cache breakpoints, so it is not the \
         implicit-cache shape #1150's reporter is on and these tests measure \
         the wrong provider class"
    );
}

/// THE CLAIM. Across a whole multi-turn session — not merely within one turn —
/// every dispatch repeats the previous dispatch's messages verbatim and
/// appends. That is exactly the condition under which an OpenAI-compatible
/// endpoint bills dispatch N its delta instead of the whole context, which is
/// what "prompt/KV cache is reused where possible" means where there is no
/// write point to place.
///
/// Measured ACROSS TURNS deliberately: #559 c5's file stops at the sub-calls
/// inside one turn, and the reported session was 26 turns long.
#[tokio::test]
async fn every_dispatch_extends_the_previous_dispatchs_byte_prefix() {
    let dispatches = session_dispatches(4, 3, 64, openai_shaped_config()).await;
    assert_eq!(dispatches.len(), 16, "4 turns x (3 tool rounds + 1 answer)");

    for (n, pair) in dispatches.windows(2).enumerate() {
        let (earlier, later) = (&pair[0], &pair[1]);
        assert!(
            later.messages.len() >= earlier.messages.len(),
            "dispatch {} dropped messages ({} -> {}): a shortened prompt is a cache miss \
             from the first divergence",
            n + 1,
            earlier.messages.len(),
            later.messages.len()
        );
        let shared = shared_prefix(earlier, later);
        assert_eq!(
            shared,
            earlier.messages.len(),
            "dispatch {} diverges from dispatch {} at message {shared} of {}: everything \
             from there on re-bills at full price on an implicit-cache endpoint, every \
             turn, for the rest of the session",
            n + 1,
            n,
            earlier.messages.len()
        );
    }

    // NEGATIVE CONTROL for the assertion above: the prompt must actually be
    // GROWING. A session that appended nothing would satisfy "every dispatch
    // extends the previous prefix" trivially.
    assert!(
        dispatches.last().unwrap().messages.len() > dispatches[0].messages.len() + 8,
        "the session did not accumulate history, so prefix stability is vacuous here"
    );
}

/// The boundary the criterion's "where possible" is doing work at.
///
/// c4's FIRST half rewrites old tool results in place once the conversation's
/// tool-result bytes pass a ceiling. A rewrite inside the prefix IS a cache
/// invalidation, so the two halves of this criterion pull against each other
/// and the honest claim is bounded rather than absolute.
///
/// The bound that makes the cost affordable: each message goes verbatim → stub
/// ONCE and is then frozen. Under that rule an implicit-cache endpoint pays for
/// a message twice over a whole session, however long it runs. Under the
/// failure it would pay every turn, which is the 4.88M-input-token shape #559
/// reported and the "absurd input token size" this ticket is named for.
///
/// This is the wire-level form of `micro.rs::the_ceiling_is_byte_stable_on_a_second_pass`,
/// which asserts the same thing about one pass over one buffer. Here it is
/// asserted across 18 real dispatches of a 6-turn session.
#[tokio::test]
async fn a_bounded_tool_result_is_rewritten_once_and_then_frozen() {
    let mut cfg = openai_shaped_config();
    cfg.compact.tool_results.total_budget_bytes = 4_000;
    cfg.compact.tool_results.keep_recent = 2;
    cfg.compact.tool_results.epoch_results = 2;
    let dispatches = session_dispatches(6, 2, 1_000, cfg).await;
    assert_eq!(dispatches.len(), 18, "6 turns x (2 tool rounds + 1 answer)");

    // For every message index, the sequence of DISTINCT consecutive byte
    // values it takes across the session.
    let width = dispatches.iter().map(|d| d.messages.len()).max().unwrap();
    let mut rewrites: Vec<usize> = Vec::new();
    for i in 0..width {
        let mut forms: Vec<String> = Vec::new();
        for d in &dispatches {
            if let Some(m) = d.messages.get(i) {
                let bytes = content_bytes(m);
                if forms.last() != Some(&bytes) {
                    forms.push(bytes);
                }
            }
        }
        rewrites.push(forms.len().saturating_sub(1));
    }

    // PRECONDITION: the ceiling really engaged. 12 results of 1,000 bytes
    // against a 4,000-byte budget cannot all survive; a session where nothing
    // was ever rewritten does not measure the interaction this test exists for.
    let touched = rewrites.iter().filter(|n| **n > 0).count();
    assert!(
        touched > 0,
        "the tool-result ceiling never rewrote a single message, so this test \
         measures nothing: {rewrites:?}"
    );

    // THE BOUND: once, and then frozen.
    for (i, n) in rewrites.iter().enumerate() {
        assert!(
            *n <= 1,
            "message {i} changed bytes {n} times across the session. A tool result may \
             go verbatim -> stub once; changing again re-invalidates the implicit prefix \
             cache from message {i} onward on every later dispatch. Per-message rewrite \
             counts: {rewrites:?}"
        );
    }

    // ...and the invalidation is EPOCH-QUANTIZED rather than per-dispatch:
    // most consecutive dispatch pairs must share a full prefix, or the cache
    // never gets a chance to settle between ticks.
    let breaks: Vec<usize> = dispatches
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| shared_prefix(&pair[0], &pair[1]) < pair[0].messages.len())
        .map(|(n, _)| n + 1)
        .collect();
    assert!(
        breaks.len() * 2 < dispatches.len() - 1,
        "{} of {} dispatch pairs broke the prefix - the ceiling is invalidating the cache \
         on the majority of dispatches, which is a cost c4's first half was supposed to \
         trade away, not add: {breaks:?}",
        breaks.len(),
        dispatches.len() - 1
    );
}
