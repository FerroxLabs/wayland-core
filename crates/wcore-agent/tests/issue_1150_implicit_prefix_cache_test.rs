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
    // Registered and never called. `admit_hydrated_tools` hydrates on FIRST USE,
    // per tool, so this one stays a deferred stub for the whole session — which
    // is what makes the hydration break below bounded by the TOOL count rather
    // than by the turn count.
    tools.register(Box::new(MockTool::new("idle", "never called", false)));

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

/// The prompt an implicit-cache endpoint actually keys on, as the ordered list
/// of segments it matches: the system prompt, then the tool schemas, then one
/// segment per message.
///
/// System and tools are segments 0 and 1 because that is where they sit on an
/// OpenAI-shaped body — ahead of the entire conversation. The first cut of this
/// file compared `messages` alone, which is blind to the single most expensive
/// way to lose the cache: a prompt whose FIRST segment differs on every
/// dispatch reuses nothing at all, at token 0, while every message still
/// matches. A per-dispatch nonce in the system prompt, a tool list rebuilt in a
/// different order, a re-labelled message — all of them are a total cache miss
/// on the wire and all of them were invisible here.
///
/// `role` travels inside a message segment alongside `content` for the same
/// reason: a re-labelled message is different bytes even when its text is
/// identical. The cache HINT deliberately stays out — it is metadata a provider
/// consumes, not prompt bytes.
fn prefix_segments(request: &LlmRequest) -> Vec<String> {
    let tools: Vec<serde_json::Value> = request
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.input_schema,
                "deferred": t.deferred,
            })
        })
        .collect();
    let mut segments = vec![
        serde_json::to_string(&request.system).expect("the system prompt serializes"),
        serde_json::to_string(&tools).expect("tool schemas serialize"),
    ];
    segments.extend(
        request
            .messages
            .iter()
            .map(|m| serde_json::to_string(&(m.role, &m.content)).expect("messages serialize")),
    );
    segments
}

/// What segment `i` is, so a failure names the part of the prompt that moved
/// instead of an index.
fn segment_label(i: usize) -> String {
    match i {
        0 => "the SYSTEM PROMPT (token 0 - nothing at all is reused)".to_string(),
        1 => "the TOOL SCHEMAS (nothing after the system prompt is reused)".to_string(),
        n => format!("message {}", n - 2),
    }
}

/// How many leading prefix segments of `a` and `b` are byte-identical — the
/// length of the prefix an implicit cache can serve from.
fn shared_prefix(a: &LlmRequest, b: &LlmRequest) -> usize {
    let (x, y) = (prefix_segments(a), prefix_segments(b));
    let n = x.len().min(y.len());
    let mut i = 0;
    while i < n && x[i] == y[i] {
        i += 1;
    }
    i
}

/// The tools still travelling as deferred stubs. `openai.rs:622` truncates a
/// deferred tool's description on the wire, so this flag is not bookkeeping —
/// it decides the bytes of the tool block on the reporter's own route.
fn deferred_names(request: &LlmRequest) -> Vec<&str> {
    request
        .tools
        .iter()
        .filter(|t| t.deferred)
        .map(|t| t.name.as_str())
        .collect()
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
/// every dispatch repeats the previous dispatch's WHOLE prompt prefix verbatim
/// - system prompt, tool schemas, then messages - and appends. That is exactly
/// the condition under which an OpenAI-compatible
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

    // NON-VACUITY for the two segments ahead of the conversation: comparing an
    // empty system prompt against another empty one, or an empty tool list
    // against another empty one, proves nothing about either.
    assert!(
        !dispatches[0].system.is_empty(),
        "this route sent no system prompt, so segment 0 compares empty to empty"
    );
    assert_eq!(
        dispatches[0].tools.len(),
        2,
        "this route must carry both fixture tools, or segment 1 measures nothing"
    );

    // THE SYSTEM PROMPT admits no exception at all. It sits at token 0, so a
    // prompt whose first segment moves reuses NOTHING — not one message, on any
    // dispatch, for the whole session. This is the assertion the messages-only
    // instrument could not make, and a per-dispatch nonce in the system prompt
    // passed straight through it.
    let system0 = prefix_segments(&dispatches[0])[0].clone();
    for (n, d) in dispatches.iter().enumerate() {
        assert_eq!(
            prefix_segments(d)[0],
            system0,
            "dispatch {n} changed the system prompt, so every dispatch from the first \
             one on re-bills its entire context at full price"
        );
    }

    for (n, pair) in dispatches.windows(2).enumerate() {
        assert!(
            pair[1].messages.len() >= pair[0].messages.len(),
            "dispatch {} dropped messages ({} -> {}): a shortened prompt is a cache miss \
             from the first divergence",
            n + 1,
            pair[0].messages.len(),
            pair[1].messages.len()
        );
    }

    // Every divergence in the whole session, as (dispatch, first diverging
    // segment).
    let breaks: Vec<(usize, usize)> = dispatches
        .windows(2)
        .enumerate()
        .filter_map(|(n, pair)| {
            let expected = prefix_segments(&pair[0]).len();
            let shared = shared_prefix(&pair[0], &pair[1]);
            (shared < expected).then_some((n + 1, shared))
        })
        .collect();

    // MEASURED, not chosen, and stated as an exception SET because the absolute
    // form is false and this file's first cut was merely blind to it.
    //
    // Exactly one break in sixteen dispatches, and it is the TOOL-DEFERRAL
    // HYDRATION: `probe` is dispatched once as a deferred stub, used, and
    // admitted by `admit_hydrated_tools`, which un-defers it and moves it to the
    // tail of `tools[]`. Both halves change the tool block's bytes, and the tool
    // block sits ahead of the whole conversation.
    //
    // What makes that affordable is the SHAPE of the bound: it is one break per
    // distinct tool FIRST USED, never one per turn. #1150's reported failure is
    // the per-turn shape, and a per-turn shape would show up here as fifteen
    // entries rather than one.
    let labelled: Vec<String> = breaks
        .iter()
        .map(|(dispatch, segment)| format!("dispatch {dispatch} at {}", segment_label(*segment)))
        .collect();
    assert_eq!(
        breaks,
        vec![(1usize, 1usize)],
        "the implicit prefix cache must break exactly once in this session — at \
         dispatch 1, on the tool-schema segment, for the deferral hydration — and be \
         frozen either side of it. Each entry is (dispatch, first diverging segment); \
         segment 0 is the system prompt, segment 1 the tool schemas, and segment n+2 \
         message n: {breaks:?} = {labelled:?}"
    );

    // ...and the break is pinned to that cause rather than merely coinciding
    // with it. Hydration is PER TOOL, on first use: `idle` is registered and
    // never called, so it is still a stub on the last dispatch of the session.
    // That is why the bound above is by tool count — a session that first-uses
    // K deferred tools pays K of these, on K different dispatches.
    assert_eq!(
        deferred_names(&dispatches[0]),
        vec!["probe", "idle"],
        "both tools must start deferred, or the hydration this break is attributed to \
         never happened"
    );
    for (n, d) in dispatches.iter().enumerate().skip(1) {
        assert_eq!(
            deferred_names(d),
            vec!["idle"],
            "dispatch {n}: after `probe`'s first use exactly one stub must remain"
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
    cfg.compact.tool_results.epoch_results = 6;
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
    // Split by CAUSE. Segments 0 and 1 are the system prompt and the tool
    // schemas; the ceiling rewrites MESSAGES, so only a divergence at segment 2
    // or later is its doing. The one-off tool-deferral hydration is measured and
    // pinned by `every_dispatch_extends_the_previous_dispatchs_byte_prefix`
    // above; folding it in here would move this bound for a reason that has
    // nothing to do with epoch quantization.
    let diverge = |pair: &[LlmRequest]| {
        let expected = prefix_segments(&pair[0]).len();
        let shared = shared_prefix(&pair[0], &pair[1]);
        (shared < expected).then_some(shared)
    };
    let breaks: Vec<usize> = dispatches
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| diverge(pair).is_some_and(|shared| shared >= 2))
        .map(|(n, _)| n + 1)
        .collect();

    // ...and the split is not a place to hide one. Everything ahead of the
    // messages must be the single hydration break and nothing else, or this
    // filter would quietly absorb a new system-prompt or tool-schema churn.
    let ahead_of_the_conversation: Vec<usize> = dispatches
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| diverge(pair).is_some_and(|shared| shared < 2))
        .map(|(n, _)| n + 1)
        .collect();
    assert_eq!(
        ahead_of_the_conversation,
        vec![1usize],
        "the only divergence ahead of the conversation may be `probe`'s one-off \
         deferral hydration at dispatch 1: {ahead_of_the_conversation:?}"
    );
    // MEASURED, and the bound is set from the measurement rather than chosen:
    // with `epoch_results = 6` this session breaks the prefix on 5 of its 17
    // dispatch pairs; with the quantization removed (`let epoch = 1`) the same
    // session breaks 8. A bound that both numbers satisfy would be a gate that
    // cannot fail, so it is pinned at the quantized figure.
    assert!(
        breaks.len() <= 5,
        "the ceiling broke the prefix on {} of {} dispatch pairs; the epoch-quantized \
         boundary measures 5 and an unquantized one measures 8, so the boundary is \
         advancing more often than `epoch_results` allows and the cache never settles \
         between ticks: {breaks:?}",
        breaks.len(),
        dispatches.len() - 1
    );
}
