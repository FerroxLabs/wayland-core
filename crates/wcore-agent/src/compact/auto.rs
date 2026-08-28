//! Autocompact: watermark-triggered LLM summarization.
//!
//! When the token watermark exceeds the configured threshold, this module
//! calls the LLM to produce a structured summary of the conversation,
//! then replaces the full history with a compact boundary marker and the
//! summary.  A circuit breaker prevents runaway retries.

use tokio::sync::mpsc;
use wcore_config::compact::CompactConfig;
use wcore_providers::{LlmProvider, ProviderError, flux_loop};
use wcore_types::compact::{CompactMetadata, CompactTrigger};
use wcore_types::llm::{FluxLoopIntent, LlmEvent, LlmRequest, ThinkingConfig};
use wcore_types::message::{ContentBlock, Message, Role, TokenUsage};

use super::prompt::{
    COMPACT_MAX_OUTPUT_TOKENS, COMPACT_SYSTEM_PROMPT, build_compact_prompt, build_summary_content,
    format_compact_summary,
};
use super::state::CompactState;

/// Maximum number of prompt-too-long retries.
const MAX_PTL_RETRIES: u32 = 2;

/// #863 F2/F3 - the loop-ownership provenance a compaction turn carries.
///
/// Compaction is a real provider turn on the SAME task the surrounding loop is
/// climbing, so it must be marked the way that loop's ordinary turns are. Sent
/// unmarked it reaches a Flux router as anonymous traffic: cacheable across
/// builders, and eligible for the router's OWN server-side Elevation ladder on
/// mid-loop material - exactly the two-ladder collision #863 exists to
/// prevent. The engine therefore threads the live session's intent down here
/// instead of the request field being hardcoded `None`.
///
/// `Default` is the unmarked case: an ordinary session, or any caller that owns
/// no loop. It keeps the request byte-identical to the pre-#863 shape, so a
/// non-Flux endpoint is unaffected.
#[derive(Debug, Clone, Default)]
pub struct CompactLoopProvenance {
    /// Who owns the outer loop for this turn - `ClientOwned("anvil")` on an
    /// Anvil builder fork. `None` leaves the compaction request unmarked.
    pub intent: Option<FluxLoopIntent>,
    /// F3 per-turn cache variance. Rides only alongside `intent`; the provider
    /// drops a nonce on unmarked traffic.
    pub nonce: Option<String>,
}

/// Content prefix for the compact boundary marker message.
pub const BOUNDARY_PREFIX: &str = "[Conversation compacted]";

// ── Public types ────────────────────────────────────────────────────────────

/// Result of a successful autocompact operation.
#[derive(Debug, Clone)]
pub struct CompactResult {
    /// Post-compact messages that replace the original conversation.
    /// Contains a boundary marker and a summary message.
    pub messages: Vec<Message>,
    /// How many original messages were summarized.
    pub messages_summarized: usize,
    /// Input token count before compaction (from the last API call).
    pub pre_compact_tokens: u64,
}

/// Errors specific to autocompact.
#[derive(Debug, thiserror::Error)]
pub enum CompactError {
    #[error("LLM provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("Prompt too long after {attempts} retries")]
    PromptTooLong { attempts: u32 },
    #[error("Empty response from LLM")]
    EmptyResponse,
    #[error("Stream error: {0}")]
    StreamError(String),
    #[error("Circuit breaker tripped after {failures} consecutive failures")]
    CircuitBroken { failures: u32 },
    /// #863 F2 — the router ran its OWN server-side Elevation ladder on a turn
    /// this session declared it owns. Both ladders climbed the same task, so
    /// the summary that came back is contaminated mid-loop material. Carries
    /// the shared `flux_loop::collision_message` text verbatim.
    #[error("{0}")]
    LoopCollision(String),
}

// ── Trigger check ───────────────────────────────────────────────────────────

/// Check if autocompact should trigger based on the token watermark.
///
/// Returns `true` when `last_input_tokens` >= the autocompact threshold:
/// `threshold = effective_context_window - output_reserve - autocompact_buffer`
///
/// `provider` / `model` are the POST-swap effective pair — see
/// [`autocompact_threshold`].
pub fn should_autocompact(
    last_input_tokens: u64,
    config: &CompactConfig,
    provider: &str,
    model: &str,
) -> bool {
    if !config.enabled {
        return false;
    }
    last_input_tokens as usize >= autocompact_threshold(config, provider, model)
}

/// The autocompact trigger threshold in tokens:
/// `effective_context_window - output_reserve - autocompact_buffer`.
///
/// F23-04 exposes this so the cache/compaction ledger can report token pressure
/// as a fraction of the boundary that actually fires, rather than as a raw
/// watermark a reader has to interpret. Extracted from — not duplicated
/// alongside — [`should_autocompact`], so the number reported to an operator is
/// by construction the number the engine acts on. GH#635 keeps that property
/// intact by making the *denominator* part of the shared function too: the
/// window comes from [`CompactConfig::effective_context_window`], so the
/// ledger cannot report a threshold derived from a different window than the
/// one the trigger enforces.
///
/// `provider` / `model` must be the POST-swap effective pair (the same values
/// fed to `size_output_cap` and the #255 pre-flight guard). Passing a stale
/// pre-swap model is the bug class this parameter exists to prevent.
///
/// Note this ignores `config.enabled`: it is the threshold's VALUE, and a
/// disabled compactor still has one worth showing next to the watermark.
pub fn autocompact_threshold(config: &CompactConfig, provider: &str, model: &str) -> usize {
    config
        .effective_context_window(provider, model)
        .saturating_sub(config.output_reserve)
        .saturating_sub(config.autocompact_buffer)
}

// ── Request sanitation ──────────────────────────────────────────────────────

/// Drop every `tool_use` block that is not answered by a `tool_result` in the
/// IMMEDIATELY following message, and drop any message left with no content.
///
/// **C4L-F1, measured live rather than reasoned about.** Anthropic states the
/// invariant explicitly — *"Each `tool_use` block must have a corresponding
/// `tool_result` block in the next message"* — and enforces it with a 400.
/// Autocompact's trigger is checked while the agent loop is mid-tool-call, so
/// the conversation it is handed normally ends with an assistant turn whose
/// calls are still outstanding. Appending the summary prompt after that turn
/// puts a plain user text message where the provider demands a `tool_result`.
///
/// Driven live on `hetzner-dsm` against `claude-haiku-4-5` (23B-C4), three
/// consecutive autocompact attempts failed with
/// `API error 400 … "tool_use ids were found without tool_result blocks
/// immediately after"`. The ledger recorded `compactions=3 failed=3
/// tokens_reclaimed=0` at `peak_pressure=2.2021` — i.e. **autocompaction never
/// ran at all in a tool-using Anthropic session**, and the session was walking
/// toward the emergency hard stop with no relief available.
///
/// Summarization does not need an in-flight call — it needs a request the
/// provider will accept. Text in the same assistant turn is preserved; only the
/// unanswered `tool_use` blocks go. A `tool_result` message can never be
/// emptied by this pass (it contains no `tool_use` blocks), so dropping an
/// emptied assistant turn cannot orphan anything after it.
pub(crate) fn drop_unanswered_tool_calls(messages: &[Message]) -> Vec<Message> {
    let answered = |idx: usize| -> std::collections::HashSet<&str> {
        messages
            .get(idx + 1)
            .map(|next| {
                next.content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut out = Vec::with_capacity(messages.len());
    for (i, msg) in messages.iter().enumerate() {
        let has_tool_use = msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
        if !has_tool_use {
            out.push(msg.clone());
            continue;
        }
        let answered_ids = answered(i);
        let kept: Vec<ContentBlock> = msg
            .content
            .iter()
            .filter(|b| match b {
                ContentBlock::ToolUse { id, .. } => answered_ids.contains(id.as_str()),
                _ => true,
            })
            .cloned()
            .collect();
        if kept.is_empty() {
            // Every block was an unanswered call — the turn carried nothing
            // else, so there is nothing left to summarize from it.
            continue;
        }
        let mut trimmed = msg.clone();
        trimmed.content = kept;
        out.push(trimmed);
    }
    out
}

// ── Core autocompact ────────────────────────────────────────────────────────

/// Execute autocompact: call LLM to summarize the conversation.
///
/// 1. Build a summary prompt and send conversation + prompt to the LLM.
/// 2. If the prompt is too long, truncate oldest 20% messages and retry
///    (up to [`MAX_PTL_RETRIES`] times).
/// 3. Parse the `<summary>` from the response.
/// 4. Return a [`CompactResult`] with boundary marker + summary messages.
///
/// On failure, increments `state.consecutive_failures`.
/// On success, resets the failure counter.
pub async fn autocompact(
    provider: &dyn LlmProvider,
    messages: &[Message],
    model: &str,
    config: &CompactConfig,
    state: &mut CompactState,
    provenance: &CompactLoopProvenance,
) -> Result<CompactResult, CompactError> {
    // Circuit breaker check
    if state.is_circuit_broken(config) {
        return Err(CompactError::CircuitBroken {
            failures: state.consecutive_failures,
        });
    }

    let pre_compact_tokens = state.last_input_tokens;
    let messages_summarized = messages.len();

    // Summarization is the canonical cheap-model task. When a dedicated
    // compaction model is configured, target it instead of the live
    // (premium) conversation model; otherwise fall back to the live model,
    // preserving prior behavior exactly. The id is a plain provider-served
    // string — no provider is assumed.
    let compact_model = config.compaction_model.as_deref().unwrap_or(model);

    // Build messages for the compact LLM call: conversation + summary prompt.
    //
    // C4L-F1: the conversation must be sanitized FIRST. Autocompact is checked
    // mid-tool-loop, so `messages` routinely ends with an assistant turn whose
    // tool calls are still in flight; appending the summary prompt after that
    // produces a `tool_use` with no `tool_result` in the next message, which
    // Anthropic-wire providers reject outright. See
    // [`drop_unanswered_tool_calls`].
    let prompt = build_compact_prompt();
    let mut conv_messages = drop_unanswered_tool_calls(messages);
    conv_messages.push(Message::new(
        Role::User,
        vec![ContentBlock::Text { text: prompt }],
    ));

    let mut ptl_attempts = 0u32;

    let summary_text = loop {
        let request = LlmRequest {
            flux_loop_intent: provenance.intent.clone(),
            flux_turn_nonce: provenance.nonce.clone(),
            model: compact_model.to_string(),
            system: COMPACT_SYSTEM_PROMPT.to_string(),
            messages: conv_messages.clone(),
            tools: vec![],
            max_tokens: COMPACT_MAX_OUTPUT_TOKENS,
            thinking: Some(ThinkingConfig::Disabled),
            reasoning_effort: None,
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
            omit_max_tokens: false,
        };

        match provider.stream(&request).await {
            Ok(rx) => {
                match collect_stream_text(rx, provenance.intent.as_ref().and_then(|i| i.owner()))
                    .await
                {
                    Ok((text, _usage)) => break text,
                    Err(e) => {
                        state.record_failure();
                        return Err(e);
                    }
                }
            }
            Err(ProviderError::PromptTooLong(_)) if ptl_attempts < MAX_PTL_RETRIES => {
                ptl_attempts += 1;
                // Remove the summary prompt (last msg), truncate, re-add prompt
                let conversation_part = &conv_messages[..conv_messages.len() - 1];
                match truncate_for_retry(conversation_part) {
                    Some(mut truncated) => {
                        truncated.push(Message::new(
                            Role::User,
                            vec![ContentBlock::Text {
                                text: build_compact_prompt(),
                            }],
                        ));
                        conv_messages = truncated;
                    }
                    None => {
                        state.record_failure();
                        return Err(CompactError::PromptTooLong {
                            attempts: ptl_attempts,
                        });
                    }
                }
            }
            Err(ProviderError::PromptTooLong(_)) => {
                state.record_failure();
                return Err(CompactError::PromptTooLong {
                    attempts: ptl_attempts,
                });
            }
            Err(e) => {
                state.record_failure();
                return Err(CompactError::Provider(e));
            }
        }
    };

    if summary_text.trim().is_empty() {
        state.record_failure();
        return Err(CompactError::EmptyResponse);
    }

    // Format and build post-compact messages
    let formatted = format_compact_summary(&summary_text);
    let summary_content = build_summary_content(&formatted, true);

    let metadata = CompactMetadata {
        trigger: CompactTrigger::Auto,
        pre_compact_tokens,
        messages_summarized,
    };

    // SAFETY: `CompactMetadata` is a plain struct of String + u64 +
    // u32 fields with derived Serialize — none of those can ever fail
    // serialization. The `expect` only fires if a future field is
    // added whose Serialize impl returns Err, which CI would catch.
    let boundary_text = format!(
        "{BOUNDARY_PREFIX}\n{}",
        serde_json::to_string(&metadata).expect("CompactMetadata serialization cannot fail")
    );

    let boundary_msg = Message::new(
        Role::User,
        vec![ContentBlock::Text {
            text: boundary_text,
        }],
    );

    let summary_msg = Message::new(
        Role::User,
        vec![ContentBlock::Text {
            text: summary_content,
        }],
    );

    state.record_success();

    Ok(CompactResult {
        messages: vec![boundary_msg, summary_msg],
        messages_summarized,
        pre_compact_tokens,
    })
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Collect all text from a streaming LLM response.
///
/// `loop_owner` is the owner this compaction turn DECLARED (`None` when the
/// session owns no loop, which is the ordinary case).
///
/// #863 F2 — the RUNTIME half of the anti-collision handshake, which until now
/// existed only on the ordinary turn path (`engine.rs`, the `ProviderMeta` arm
/// of the turn loop, which returns `AgentError::ApiError`). Declaring
/// `loop_owner` asks the router not to elevate; `x-flux-loop-engaged:
/// elevation` is it saying it did anyway. Both ladders then climbed the same
/// task and the text that came back is contaminated mid-loop material. On an
/// ordinary turn accepting it costs one answer; on a COMPACTION it replaces
/// the entire conversation history and nothing can restore it — so this is the
/// one path where dropping the signal is irreversible, and it must fault.
async fn collect_stream_text(
    mut rx: mpsc::Receiver<LlmEvent>,
    loop_owner: Option<&str>,
) -> Result<(String, TokenUsage), CompactError> {
    let mut text = String::new();

    while let Some(event) = rx.recv().await {
        match event {
            LlmEvent::TextDelta(delta) => text.push_str(&delta),
            LlmEvent::Done { usage, .. } => return Ok((text, usage)),
            LlmEvent::Error(e) => return Err(CompactError::StreamError(e)),
            LlmEvent::ProviderMeta { loop_engaged, .. } => {
                // Same predicate and same wording as the ordinary turn path, so
                // the two cannot drift apart. Deliberately narrow: `cascade` is
                // not a collision (F1 permits Cascade's single-tier
                // climb-on-failure) and a missing header is not one either — a
                // non-Flux endpoint never sends one, and every Anthropic
                // compaction in the workspace would fault if silence counted.
                if let Some(owner) = loop_owner
                    && flux_loop::collides(Some(owner), loop_engaged.as_deref())
                {
                    let engaged = loop_engaged
                        .as_deref()
                        .unwrap_or(flux_loop::LOOP_ENGAGED_ELEVATION);
                    return Err(CompactError::LoopCollision(flux_loop::collision_message(
                        owner, engaged,
                    )));
                }
            }
            // Ignore thinking deltas and tool calls (shouldn't happen in compact)
            _ => {}
        }
    }

    // Channel closed without a Done event
    Err(CompactError::EmptyResponse)
}

/// True when `msg` carries a tool result — either a dedicated `Role::Tool`
/// message or a user-role message threading `ToolResult` blocks (both shapes
/// occur in the conversation history). Such a message is only valid when its
/// parent assistant `tool_calls` turn precedes it; on its own it is an orphan.
fn is_tool_result(msg: &Message) -> bool {
    msg.role == Role::Tool
        || msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

/// Truncate the oldest ~20% of messages for PTL retry.
///
/// Returns `None` if there are too few messages to truncate meaningfully.
///
/// Tool-pair aware (FerroxLabs/wayland-core#123): the cut never lands between
/// an assistant `tool_calls` turn and its tool results. Dropping the assistant
/// while keeping its result leaves an orphaned `role:"tool"` message that
/// strict OpenAI endpoints (DeepSeek via Flux) reject with HTTP 400. After
/// computing the nominal boundary we advance it forward past any leading
/// tool-result messages so `remaining` always begins at a clean turn boundary.
fn truncate_for_retry(messages: &[Message]) -> Option<Vec<Message>> {
    if messages.len() < 2 {
        return None;
    }

    let mut drop_count = (messages.len() / 5).max(1);

    // Snap the boundary to a turn start: if it would leave a tool result at the
    // front of `remaining` (its parent assistant turn dropped), drop that
    // orphaned result too. Parallel tool calls produce several consecutive
    // results, so advance past the whole run.
    while drop_count < messages.len() && is_tool_result(&messages[drop_count]) {
        drop_count += 1;
    }

    if drop_count >= messages.len() {
        return None;
    }

    let remaining = &messages[drop_count..];
    let mut result = Vec::with_capacity(remaining.len() + 1);

    // Ensure the first message is User role for API compatibility
    if remaining.first().map(|m| m.role) != Some(Role::User) {
        result.push(Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "[earlier conversation truncated for compaction retry]".to_string(),
            }],
        ));
    }

    result.extend_from_slice(remaining);
    Some(result)
}

/// Check if a message is a compact boundary marker.
pub fn is_compact_boundary(message: &Message) -> bool {
    message.content.iter().any(|block| {
        if let ContentBlock::Text { text } = block {
            text.starts_with(BOUNDARY_PREFIX)
        } else {
            false
        }
    })
}

/// Extract [`CompactMetadata`] from a boundary marker message.
pub fn extract_compact_metadata(message: &Message) -> Option<CompactMetadata> {
    for block in &message.content {
        if let ContentBlock::Text { text } = block
            && let Some(json_str) = text.strip_prefix(BOUNDARY_PREFIX)
        {
            let json_str = json_str.trim_start_matches('\n');
            return serde_json::from_str(json_str).ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use wcore_types::compact::CompactTrigger;
    use wcore_types::message::{FinishReason, StopReason};

    fn default_config() -> CompactConfig {
        CompactConfig::default()
    }

    // ── C4L-F1: unanswered tool calls must not reach the compact request ────

    fn tool_use(id: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"file_path": "/tmp/x"}),
            extra: None,
        }
    }

    fn tool_result(id: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: "ok".to_string(),
            is_error: false,
        }
    }

    fn text(t: &str) -> ContentBlock {
        ContentBlock::Text {
            text: t.to_string(),
        }
    }

    /// Anthropic's stated invariant, implemented independently of the code under
    /// test: *"Each `tool_use` block must have a corresponding `tool_result`
    /// block in the next message."* This is the instrument — it is what turns
    /// the tests below into a measurement rather than a restatement of the fix.
    fn violates_anthropic_pairing(messages: &[Message]) -> bool {
        messages.iter().enumerate().any(|(i, m)| {
            m.content.iter().any(|b| match b {
                ContentBlock::ToolUse { id, .. } => !messages.get(i + 1).is_some_and(|next| {
                    next.content.iter().any(|nb| {
                        matches!(nb, ContentBlock::ToolResult { tool_use_id, .. }
                                if tool_use_id == id)
                    })
                }),
                _ => false,
            })
        })
    }

    /// The instrument itself must be able to report BOTH answers, or every
    /// assertion built on it is self-passing.
    #[test]
    fn pairing_instrument_reports_both_answers() {
        let bad = vec![Message::new(Role::Assistant, vec![tool_use("t1")])];
        assert!(
            violates_anthropic_pairing(&bad),
            "instrument must detect an unanswered tool_use"
        );
        let good = vec![
            Message::new(Role::Assistant, vec![tool_use("t1")]),
            Message::new(Role::User, vec![tool_result("t1")]),
        ];
        assert!(
            !violates_anthropic_pairing(&good),
            "instrument must accept a correctly paired tool_use"
        );
    }

    #[test]
    fn unanswered_tail_tool_call_is_dropped_and_the_old_path_would_not_have_been() {
        // The exact live shape: a completed tool round-trip, then an assistant
        // turn whose calls are still in flight when the watermark trips.
        let raw = vec![
            Message::new(Role::User, vec![text("do the thing")]),
            Message::new(Role::Assistant, vec![tool_use("t1")]),
            Message::new(Role::User, vec![tool_result("t1")]),
            Message::new(Role::Assistant, vec![tool_use("t2"), tool_use("t3")]),
        ];

        // Assertion 1 (known-positive): the OLD path — `messages.to_vec()` then
        // push the summary prompt — produces the shape the provider rejects.
        // Without this the test would pass on a no-op sanitizer.
        let mut old_path = raw.clone();
        old_path.push(Message::new(Role::User, vec![text("summarize")]));
        assert!(
            violates_anthropic_pairing(&old_path),
            "the pre-fix request must violate the pairing rule — otherwise this \
             test is not exercising the defect that was measured live"
        );

        // Assertion 2: the sanitized request does not.
        let mut fixed = drop_unanswered_tool_calls(&raw);
        fixed.push(Message::new(Role::User, vec![text("summarize")]));
        assert!(
            !violates_anthropic_pairing(&fixed),
            "sanitized request still violates the pairing rule: {fixed:?}"
        );

        // Assertion 3 (known-negative on over-deletion): the ANSWERED call and
        // its result must survive. A sanitizer that simply dropped every
        // tool_use would pass assertion 2 and destroy the transcript.
        assert_eq!(fixed.len(), 4, "expected 3 kept messages + the prompt");
        assert!(
            matches!(&fixed[1].content[0], ContentBlock::ToolUse { id, .. } if id == "t1"),
            "the answered tool_use must be preserved"
        );
        assert!(
            matches!(&fixed[2].content[0], ContentBlock::ToolResult { tool_use_id, .. }
                if tool_use_id == "t1"),
            "the tool_result must be preserved"
        );
    }

    #[test]
    fn text_alongside_an_unanswered_call_survives() {
        let raw = vec![Message::new(
            Role::Assistant,
            vec![text("I will read the file"), tool_use("t9")],
        )];
        let out = drop_unanswered_tool_calls(&raw);
        assert_eq!(
            out.len(),
            1,
            "the turn carried text, so it must not be dropped"
        );
        assert_eq!(out[0].content.len(), 1);
        assert!(
            matches!(&out[0].content[0], ContentBlock::Text { text } if text == "I will read the file")
        );
    }

    #[test]
    fn a_conversation_with_no_tool_calls_is_returned_unchanged() {
        // Known-negative: the sanitizer must be a no-op on the shape it is not
        // for, or it would silently perturb every non-tool session.
        let raw = vec![
            Message::new(Role::User, vec![text("hello")]),
            Message::new(Role::Assistant, vec![text("hi")]),
        ];
        // `Message` has no `PartialEq` (it lives in wcore-types and this lane
        // does not widen a shared type for a test), so compare structurally.
        let out = drop_unanswered_tool_calls(&raw);
        assert_eq!(out.len(), raw.len(), "no message may be dropped");
        for (a, b) in out.iter().zip(raw.iter()) {
            assert_eq!(a.role, b.role);
            assert_eq!(a.content.len(), b.content.len());
        }
    }

    /// Fake provider that records the model id from the request it is given,
    /// then returns a minimal valid `<summary>` stream so autocompact succeeds.
    struct ModelCapturingProvider {
        seen_model: Arc<Mutex<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for ModelCapturingProvider {
        async fn stream(
            &self,
            request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            *self.seen_model.lock().unwrap() = Some(request.model.clone());
            let (tx, rx) = mpsc::channel(4);
            tx.send(LlmEvent::TextDelta("<summary>ok</summary>".to_string()))
                .await
                .unwrap();
            tx.send(LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::Stop,
                usage: TokenUsage::default(),
            })
            .await
            .unwrap();
            Ok(rx)
        }
    }

    fn sample_messages() -> Vec<Message> {
        vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "earlier question".to_string(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "earlier answer".to_string(),
                }],
            ),
        ]
    }

    /// When `compaction_model` is configured, the compaction LLM request must
    /// carry the configured model, NOT the live conversation model.
    ///
    /// Fails without the fix because `autocompact` hardcodes `model.to_string()`
    /// into the request — it never consults config, so the live model "premium"
    /// would be sent regardless of the configured "cheap" model.
    #[tokio::test]
    async fn uses_configured_compaction_model() {
        let seen = Arc::new(Mutex::new(None));
        let provider = ModelCapturingProvider {
            seen_model: Arc::clone(&seen),
        };
        let config = CompactConfig {
            compaction_model: Some("cheap-model".to_string()),
            ..default_config()
        };
        let mut state = CompactState::new();

        autocompact(
            &provider,
            &sample_messages(),
            "premium-model",
            &config,
            &mut state,
            &Default::default(),
        )
        .await
        .expect("autocompact should succeed");

        assert_eq!(seen.lock().unwrap().as_deref(), Some("cheap-model"));
    }

    /// With `compaction_model` unset (the default), the compaction request must
    /// carry the live model exactly as before — proving zero behavior change for
    /// existing users.
    ///
    /// Fails if a future change made the cheap model the default: the live model
    /// "premium-model" would no longer be the one sent.
    #[tokio::test]
    async fn defaults_to_live_model() {
        let seen = Arc::new(Mutex::new(None));
        let provider = ModelCapturingProvider {
            seen_model: Arc::clone(&seen),
        };
        let config = default_config();
        assert!(config.compaction_model.is_none());
        let mut state = CompactState::new();

        autocompact(
            &provider,
            &sample_messages(),
            "premium-model",
            &config,
            &mut state,
            &Default::default(),
        )
        .await
        .expect("autocompact should succeed");

        assert_eq!(seen.lock().unwrap().as_deref(), Some("premium-model"));
    }

    // ── should_autocompact (TC-2.4-01..03, TC-2.4-14) ──────────────────

    /// A provider/model pair the `wcore_config::limits` registry does NOT
    /// know, so the effective window is the configured fallback and these
    /// cases exercise the arithmetic rather than the registry.
    const UNKNOWN_PROVIDER: &str = "test-provider";
    const UNKNOWN_MODEL: &str = "test-model";

    #[test]
    fn above_threshold_triggers() {
        // threshold = 200k - 20k - 13k = 167k
        let config = default_config();
        assert!(should_autocompact(
            170_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn below_threshold_does_not_trigger() {
        let config = default_config();
        assert!(!should_autocompact(
            160_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn at_exact_threshold_triggers() {
        let config = default_config();
        assert!(should_autocompact(
            167_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn disabled_config_never_triggers() {
        let config = CompactConfig {
            enabled: false,
            ..default_config()
        };
        assert!(!should_autocompact(
            999_999,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn custom_config_threshold() {
        let config = CompactConfig {
            context_window: Some(100_000),
            output_reserve: 10_000,
            autocompact_buffer: 5_000,
            ..default_config()
        };
        // threshold = 100k - 10k - 5k = 85k
        assert!(!should_autocompact(
            80_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
        assert!(should_autocompact(
            85_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
        assert!(should_autocompact(
            90_000,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    #[test]
    fn zero_tokens_does_not_trigger() {
        let config = default_config();
        assert!(!should_autocompact(
            0,
            &config,
            UNKNOWN_PROVIDER,
            UNKNOWN_MODEL
        ));
    }

    // ── GH#635: the threshold follows the MODEL's window ────────────────

    /// A registry-known 1.05M-window model must not autocompact at ~177k.
    /// The pre-fix threshold was 167k on EVERY model; the real one here is
    /// 1_050_000 − 20_000 − 13_000 = 1_017_000.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: change
    /// `config.effective_context_window(provider, model)` back to
    /// `config.context_window` in `autocompact_threshold` (auto.rs) — the
    /// threshold collapses to 167_000 and the 177k assertion below fires.
    #[test]
    fn large_window_model_does_not_compact_at_the_200k_threshold() {
        let config = default_config();
        assert_eq!(
            autocompact_threshold(&config, "openai-chatgpt", "gpt-5.4"),
            1_017_000
        );
        // The customer number: a 177k session on a 1.05M model is nowhere
        // near needing relief.
        assert!(!should_autocompact(
            177_000,
            &config,
            "openai-chatgpt",
            "gpt-5.4"
        ));
        // ...but the real boundary still fires.
        assert!(should_autocompact(
            1_017_000,
            &config,
            "openai-chatgpt",
            "gpt-5.4"
        ));
    }

    /// A registry-known SMALL model must compact EARLIER than the 200k
    /// default, not later — the fix has to cut both ways or it is just a
    /// blanket raise.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: same line as above; with the raw
    /// `config.context_window` the threshold is 167_000 and a 110k gpt-4o
    /// session sails past its real 105k ceiling.
    #[test]
    fn small_window_model_compacts_earlier_than_the_default() {
        let config = default_config();
        // 128_000 − 20_000 − 13_000
        assert_eq!(autocompact_threshold(&config, "openai", "gpt-4o"), 95_000);
        assert!(should_autocompact(100_000, &config, "openai", "gpt-4o"));
    }

    /// An explicitly configured window outranks the registry.
    ///
    /// HOW THIS FAILS IF THE DEFECT RETURNS: delete the
    /// `if let Some(configured) = self.context_window` early return in
    /// `CompactConfig::effective_context_window` (wcore-config/src/compact.rs)
    /// — the threshold jumps to 1_017_000 and the operator's cap is ignored.
    #[test]
    fn explicit_window_outranks_a_known_models_window() {
        let config = CompactConfig {
            context_window: Some(200_000),
            ..default_config()
        };
        assert_eq!(
            autocompact_threshold(&config, "openai-chatgpt", "gpt-5.4"),
            167_000
        );
        assert!(should_autocompact(
            170_000,
            &config,
            "openai-chatgpt",
            "gpt-5.4"
        ));
    }

    // ── truncate_for_retry ──────────────────────────────────────────────

    #[test]
    fn truncate_drops_20_percent() {
        let msgs: Vec<Message> = (0..10)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                Message::new(
                    role,
                    vec![ContentBlock::Text {
                        text: format!("msg-{i}"),
                    }],
                )
            })
            .collect();

        let result = truncate_for_retry(&msgs).unwrap();
        // Drop 20% of 10 = 2 messages, remaining 8
        assert_eq!(result.len(), 8);
    }

    #[test]
    fn truncate_ensures_user_first() {
        let msgs: Vec<Message> = (0..5)
            .map(|i| {
                Message::new(
                    Role::Assistant,
                    vec![ContentBlock::Text {
                        text: format!("msg-{i}"),
                    }],
                )
            })
            .collect();

        let result = truncate_for_retry(&msgs).unwrap();
        assert_eq!(result[0].role, Role::User);
    }

    #[test]
    fn truncate_too_few_returns_none() {
        let msgs = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "only one".to_string(),
            }],
        )];
        assert!(truncate_for_retry(&msgs).is_none());
    }

    #[test]
    fn truncate_empty_returns_none() {
        assert!(truncate_for_retry(&[]).is_none());
    }

    #[test]
    fn truncate_preserves_user_first_without_placeholder() {
        // First remaining message is already User — no placeholder needed
        let msgs: Vec<Message> = (0..10)
            .map(|i| {
                let role = if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                };
                Message::new(
                    role,
                    vec![ContentBlock::Text {
                        text: format!("msg-{i}"),
                    }],
                )
            })
            .collect();

        let result = truncate_for_retry(&msgs).unwrap();
        // msgs[2] (User) should be first; no placeholder prepended
        assert_eq!(result.len(), 8);
        match &result[0].content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "msg-2"),
            _ => panic!("expected Text"),
        }
    }

    /// #123 lock: the nominal 20% boundary lands on a tool result whose parent
    /// assistant turn would be dropped. The cut must advance past the orphan so
    /// `remaining` never starts with a tool result, and a later intact tool
    /// pair (in the kept tail) must survive whole.
    #[test]
    fn truncate_never_splits_a_tool_pair() {
        let tool_use = |id: &str| {
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: id.into(),
                    name: "bash".into(),
                    input: serde_json::json!({}),
                    extra: None,
                }],
            )
        };
        let tool_result = |id: &str| {
            Message::new(
                Role::Tool,
                vec![ContentBlock::ToolResult {
                    tool_use_id: id.into(),
                    content: "out".into(),
                    is_error: false,
                }],
            )
        };
        let text =
            |role: Role, t: &str| Message::new(role, vec![ContentBlock::Text { text: t.into() }]);

        // 12 msgs → nominal drop_count = 2, which is the tool result for tc1
        // (its assistant tool_use is index 1, inside the drop window).
        let msgs = vec![
            text(Role::User, "u0"),
            tool_use("tc1"),    // 1 — dropped
            tool_result("tc1"), // 2 — naive boundary: orphan
            text(Role::User, "u3"),
            text(Role::Assistant, "a4"),
            text(Role::User, "u5"),
            text(Role::Assistant, "a6"),
            text(Role::User, "u7"),
            tool_use("tc2"), // 8 — intact pair, in kept tail
            tool_result("tc2"),
            text(Role::User, "u10"),
            text(Role::Assistant, "a11"),
        ];

        let result = truncate_for_retry(&msgs).unwrap();

        // The cut advanced past the orphaned tc1 result → no leading tool result.
        assert!(
            !is_tool_result(&result[0]),
            "remaining must not start with a tool result: {:?}",
            result[0].role
        );

        // Every surviving tool result has its parent tool_use earlier in the
        // result (no orphans of either id).
        let mut seen_calls = std::collections::HashSet::new();
        for m in &result {
            for b in &m.content {
                match b {
                    ContentBlock::ToolUse { id, .. } => {
                        seen_calls.insert(id.clone());
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        assert!(
                            seen_calls.contains(tool_use_id),
                            "orphaned tool result for id {tool_use_id} survived truncation"
                        );
                    }
                    _ => {}
                }
            }
        }

        // The intact tc2 pair survived whole.
        let has_tc2_result = result.iter().any(|m| {
            m.content.iter().any(
                |b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "tc2"),
            )
        });
        assert!(has_tc2_result, "intact tc2 pair must survive in the tail");
    }

    // ── boundary detection / extraction ─────────────────────────────────

    #[test]
    fn detect_boundary_message() {
        let metadata = CompactMetadata {
            trigger: CompactTrigger::Auto,
            pre_compact_tokens: 150_000,
            messages_summarized: 42,
        };
        let text = format!(
            "{BOUNDARY_PREFIX}\n{}",
            serde_json::to_string(&metadata).unwrap()
        );
        let msg = Message::new(Role::User, vec![ContentBlock::Text { text }]);
        assert!(is_compact_boundary(&msg));
    }

    #[test]
    fn non_boundary_message() {
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
        );
        assert!(!is_compact_boundary(&msg));
    }

    #[test]
    fn extract_metadata_from_boundary() {
        let metadata = CompactMetadata {
            trigger: CompactTrigger::Auto,
            pre_compact_tokens: 150_000,
            messages_summarized: 42,
        };
        let text = format!(
            "{BOUNDARY_PREFIX}\n{}",
            serde_json::to_string(&metadata).unwrap()
        );
        let msg = Message::new(Role::User, vec![ContentBlock::Text { text }]);
        let extracted = extract_compact_metadata(&msg).unwrap();
        assert_eq!(extracted, metadata);
    }

    #[test]
    fn extract_metadata_from_non_boundary_returns_none() {
        let msg = Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "not a boundary".to_string(),
            }],
        );
        assert!(extract_compact_metadata(&msg).is_none());
    }
}
