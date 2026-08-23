//! #923 — the RECURRENCE half of the ticket, reproduced end to end.
//!
//! The ticket's own words: "The session is unrecoverable — retrying always
//! hits the same bad message; the only fix is abandoning the session and
//! losing its context. Happened 4+ times in a single working day."
//!
//! v0.13.5 fixed the turn-level half (the provider's error reaches the caller,
//! the request is captured, the session stays closable) and left this half
//! open, for two reasons that are both reproduced below:
//!
//!   1. `is_orphaned_tool_pair_rejection` required the refusal text to name
//!      BOTH `tool_use` and `tool_result`. The refusal in this ticket —
//!      "messages[25]: missing field `tool_call_id`" — names neither, so the
//!      repair-and-retry never engaged on the case that was filed.
//!   2. Even where it did engage, the second send was byte-identical: the
//!      pre-send repairs are idempotent and had already run against that exact
//!      array, so the retry re-earned the same 400.
//!
//! The instrument is a SHAPE-SENSITIVE provider: it refuses, with the exact
//! text from the report, any request whose message array still carries tool
//! blocks. That is what "deterministically re-poisoned" means — the refusal is
//! a function of the array, not of the attempt number — and it makes the
//! recurrence claim measurable instead of asserted.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_egress::{AllowAllPolicy, EgressClient};
use wcore_providers::retry::{builder_send_with_retry, scope_max_retries};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};

use common::{physical_attempt_server, test_config};

/// The refusal EXACTLY as pasted into the issue body (the Flux/DeepSeek route).
const REPORTED_400: &str = "Flux.BadRequestError: DeepseekException - {\"error\":{\"message\":\
     \"Failed to deserialize the JSON body into the target type: messages[25]: missing field \
     `tool_call_id` at line 1 column 114969\"}}";

/// A 400 that must never be re-sent, on the identical history.
const AUTH_400: &str = "invalid x-api-key";

/// A 400 that NAMES the wire field but is a capability refusal, not a pairing
/// fault. Re-sending it would be a blind second bill.
const CAPABILITY_400: &str = "tool_calls is not supported by this model";

// ---------------------------------------------------------------------------
// Shape-sensitive provider.
// ---------------------------------------------------------------------------
struct StrictPairingProvider {
    /// Served whenever the request still carries tool blocks.
    refusal: String,
    seen: Arc<Mutex<Vec<Vec<Message>>>>,
    calls: Arc<AtomicUsize>,
    physical_url: String,
}

fn carries_tool_blocks(messages: &[Message]) -> bool {
    messages.iter().flat_map(|m| &m.content).any(|b| {
        matches!(
            b,
            ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
        )
    })
}

#[async_trait]
impl LlmProvider for StrictPairingProvider {
    async fn stream(&self, r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(r.messages.clone());
        let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
        let _ =
            scope_max_retries(0, builder_send_with_retry(client.get(&self.physical_url))).await?;
        if carries_tool_blocks(&r.messages) {
            return Err(ProviderError::Api {
                status: 400,
                message: self.refusal.clone(),
            });
        }
        let (tx, rx) = mpsc::channel(64);
        for event in [
            LlmEvent::TextDelta("ok".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ] {
            let _ = tx.send(event).await;
        }
        Ok(rx)
    }
}

#[derive(Default)]
struct NullSink {
    infos: Mutex<Vec<String>>,
}

impl OutputSink for NullSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool) {}
    fn emit_info(&self, message: &str) {
        self.infos.lock().unwrap().push(message.to_string());
    }
}

struct Harness {
    engine: AgentEngine,
    seen: Arc<Mutex<Vec<Vec<Message>>>>,
    calls: Arc<AtomicUsize>,
    _server: wiremock::MockServer,
}

impl Harness {
    fn requests(&self) -> Vec<Vec<Message>> {
        self.seen.lock().unwrap().clone()
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

async fn harness(refusal: &str, history: Vec<Message>) -> Harness {
    let server = physical_attempt_server().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(StrictPairingProvider {
        refusal: refusal.to_string(),
        seen: seen.clone(),
        calls: calls.clone(),
        physical_url: server.uri(),
    });
    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        ToolRegistry::new(),
        Arc::new(NullSink::default()) as Arc<dyn OutputSink>,
    );
    engine.load_conversation(history);
    Harness {
        engine,
        seen,
        calls,
        _server: server,
    }
}

fn assistant_tool_use(id: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"path": "config.toml"}),
            extra: None,
        }],
    )
}

fn user_tool_result(id: &str) -> Message {
    Message::new(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: "port = 8080".to_string(),
            is_error: false,
        }],
    )
}

fn user_text(text: &str) -> Message {
    Message::new(
        Role::User,
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    )
}

/// A well-formed history the pre-send repairs have nothing to fix — which is
/// the point. The refusal is not about a shape we can name.
fn tool_bearing_history() -> Vec<Message> {
    vec![
        user_text("read the config"),
        assistant_tool_use("toolu_01ABC"),
        user_tool_result("toolu_01ABC"),
    ]
}

// ===========================================================================
// REPRODUCTION — the reported symptom
// ===========================================================================

/// GRADES the ticket's headline: "the session is unrecoverable — retrying
/// always hits the same bad message".
///
/// Two consecutive turns against a provider that refuses this array. On
/// v0.13.5 both turns fail with the same 400 and every send is byte-identical:
/// the session is dead and the only remedy is abandoning it. After the fix the
/// first turn recovers itself and the second turn is clean.
#[tokio::test]
async fn repro_a_second_turn_after_the_400_still_reaches_the_provider() {
    let mut h = harness(REPORTED_400, tool_bearing_history()).await;

    let first = h.engine.run("carry on", "").await;
    let second = h.engine.run("carry on again", "").await;

    let reqs = h.requests();
    // CONTROL: the instrument really did see the tool blocks on send #1, so
    // the "no tool blocks" assertions below cannot pass vacuously.
    assert!(
        carries_tool_blocks(&reqs[0]),
        "CONTROL BROKEN: the seeded tool pair never reached the provider at \
         all (sends: {}, first array: {:?})",
        reqs.len(),
        reqs[0]
    );
    assert!(
        first.is_ok(),
        "#923: the turn died on a tool-pairing 400 with no repair that changes \
         the array — sends: {}, arrays carried tool blocks: {:?}",
        h.call_count(),
        reqs.iter()
            .map(|m| carries_tool_blocks(m))
            .collect::<Vec<_>>()
    );
    assert!(
        second.is_ok(),
        "#923: the NEXT turn hit the same bad message — the session is \
         unrecoverable, which is the reported symptom. sends: {}",
        h.call_count()
    );
    let reqs = h.requests();
    assert!(
        reqs.len() >= 2,
        "expected a repair-and-retry send; got {} send(s)",
        reqs.len()
    );
    assert_ne!(
        serde_json::to_string(&reqs[0]).unwrap(),
        serde_json::to_string(&reqs[1]).unwrap(),
        "#923: the second send was BYTE-IDENTICAL to the first — a retry that \
         cannot change the array can only re-earn the same 400"
    );
    assert!(
        !carries_tool_blocks(reqs.last().unwrap()),
        "#923: the last send still carried tool blocks: {:?}",
        reqs.last().unwrap()
    );
    // The context must SURVIVE the repair — the ticket's complaint is that the
    // only fix available was losing it.
    let carried = reqs
        .last()
        .unwrap()
        .iter()
        .flat_map(|m| &m.content)
        .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("port = 8080")));
    assert!(
        carried,
        "#923: the tool result's content was lost rather than demoted to text: \
         {:?}",
        reqs.last().unwrap()
    );
}

/// The recurrence itself, stated as history state rather than as a send: the
/// repair must persist into the conversation, or every later turn rebuilds the
/// refused array from it.
#[tokio::test]
async fn repro_b_the_repair_persists_into_the_conversation() {
    let mut h = harness(REPORTED_400, tool_bearing_history()).await;
    // CONTROL: the seeded history really does carry tool blocks to begin with.
    assert!(
        carries_tool_blocks(h.engine.conversation_messages()),
        "CONTROL BROKEN: the seeded history carried no tool blocks"
    );
    let _ = h.engine.run("carry on", "").await;
    assert!(
        !carries_tool_blocks(h.engine.conversation_messages()),
        "#923: the conversation still holds the tool blocks the provider \
         refused, so the next turn rebuilds the same refused array"
    );
}

// ===========================================================================
// NEGATIVE CONTROLS — the gate must stay narrow
// ===========================================================================

/// An auth 400 on the IDENTICAL history must be terminal on the first send.
#[tokio::test]
async fn control_a_auth_400_is_never_repaired_or_resent() {
    let mut h = harness(AUTH_400, tool_bearing_history()).await;
    let result = h.engine.run("carry on", "").await;
    assert!(result.is_err(), "an auth 400 must fail the turn");
    assert_eq!(
        h.call_count(),
        1,
        "#923: an auth 400 was re-sent — every client error is now billed twice"
    );
    assert!(
        carries_tool_blocks(h.engine.conversation_messages()),
        "#923: an auth 400 demoted the conversation's tool blocks — the \
         escalation fired on a refusal that has nothing to do with pairing"
    );
}

/// A refusal that names `tool_calls` but is a CAPABILITY complaint, not a
/// pairing fault. This is the false positive the widened gate could introduce.
#[tokio::test]
async fn control_b_a_capability_400_naming_tool_calls_is_not_resent() {
    let mut h = harness(CAPABILITY_400, tool_bearing_history()).await;
    let result = h.engine.run("carry on", "").await;
    assert!(result.is_err(), "a capability 400 must fail the turn");
    assert_eq!(
        h.call_count(),
        1,
        "#923: a capability refusal that merely names `tool_calls` was \
         re-sent — the widened gate is not narrow enough"
    );
}

/// The gate matches, but there is nothing to demote. A second send would be
/// byte-identical, so it must not be spent.
#[tokio::test]
async fn control_c_a_toolless_conversation_is_not_resent() {
    // This provider refuses only arrays carrying tool blocks, so to test the
    // "nothing to repair" branch it must refuse unconditionally. A history
    // with no tool blocks + the reported refusal is exactly that case.
    let server = physical_attempt_server().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    struct AlwaysRefuse {
        seen: Arc<Mutex<Vec<Vec<Message>>>>,
        calls: Arc<AtomicUsize>,
        physical_url: String,
    }
    #[async_trait]
    impl LlmProvider for AlwaysRefuse {
        async fn stream(&self, r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push(r.messages.clone());
            let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
            let _ = scope_max_retries(0, builder_send_with_retry(client.get(&self.physical_url)))
                .await?;
            Err(ProviderError::Api {
                status: 400,
                message: REPORTED_400.to_string(),
            })
        }
    }
    let provider = Arc::new(AlwaysRefuse {
        seen: seen.clone(),
        calls: calls.clone(),
        physical_url: server.uri(),
    });
    let mut engine = AgentEngine::new_with_provider(
        provider,
        test_config(),
        ToolRegistry::new(),
        Arc::new(NullSink::default()) as Arc<dyn OutputSink>,
    );
    engine.load_conversation(vec![user_text("no tools were ever used here")]);
    let result = engine.run("carry on", "").await;
    assert!(result.is_err(), "the refusal is terminal");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "#923: a conversation with no tool blocks was re-sent unchanged — the \
         retry cost a second bill for a byte-identical request"
    );
}
