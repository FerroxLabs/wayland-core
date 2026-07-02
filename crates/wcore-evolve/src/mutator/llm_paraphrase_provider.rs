//! Real `LlmParaphraseProvider` — wraps a `wcore_providers::LlmProvider` and
//! collapses its streaming `LlmEvent` channel into the synchronous
//! `ParaphraseProvider::paraphrase_blocking` contract the GEPA loop expects.
//!
//! ## Why sync wrapping async
//!
//! `Generation::run` (W10B Task 2) calls `mutator.mutate(...)` inside
//! `tokio::task::spawn_blocking`, so by the time `paraphrase_blocking` runs
//! we are on a blocking-pool thread *outside* the tokio reactor. The adapter
//! therefore stores a `tokio::runtime::Handle` captured at construction time
//! and uses `Handle::block_on` to drive the async provider call from that
//! blocking thread. This is the standard tokio bridge for sync-from-async-
//! context; it is sound on every `Handle` configuration (current-thread,
//! multi-thread) because the outer thread is not itself a reactor worker.
//!
//! ## Determinism contract
//!
//! Real-provider drift (silent model updates, sampler RNG, batched inference)
//! is **out of contract**. Determinism for Paraphrase is fixture-replay only —
//! see `crates/wcore-evolve/tests/mutator_determinism.rs`. This real provider
//! is used during live evolution runs; the test corpus exercises it through
//! a recorded `LlmProvider` mock under `tests/llm_paraphrase_test.rs`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role};
use wcore_types::tool::ToolDef;

use super::paraphrase::ParaphraseProvider;

/// Default system prompt for paraphrase. Documented in source per Wave PA
/// step 2. Override via `LlmParaphraseProvider::with_system_prompt`.
pub const DEFAULT_PARAPHRASE_SYSTEM_PROMPT: &str = "\
You are a paraphrasing assistant. Given an input text, produce a single \
paraphrase that preserves meaning but varies wording, structure, or \
emphasis. Do not add new information. Do not refuse. Output ONLY the \
paraphrase, no preamble, no quotes.";

/// Default output cap. Skill bodies in the W10A corpus are ~30 lines, so 2k
/// tokens leaves headroom for a full rewrite without inviting unbounded cost.
pub const DEFAULT_MAX_TOKENS: u32 = 2048;

/// Default per-call wall-clock cap for the streaming response. The
/// generation-level timeout in `Generation::run` is the outer guard; this
/// inner cap exists so a stalled provider returns a typed error rather than
/// being killed by the outer timeout (which discards the child silently).
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Async-extension of `ParaphraseProvider` used by the integration tests so
/// they can exercise the streaming-collapse logic without the sync bridge.
/// Production callers go through `ParaphraseProvider::paraphrase_blocking`.
#[async_trait]
pub trait AsyncParaphrase: Send + Sync {
    async fn paraphrase_async(&self, body: &str) -> Result<String, LlmParaphraseError>;
}

/// Real LLM-backed paraphrase provider. Owns an `Arc<dyn LlmProvider>` and
/// converts a `body` string into a paraphrase by:
///   1. Building an `LlmRequest` with the configured model + system prompt
///      and the body as a single user `text` block.
///   2. Streaming the response.
///   3. Concatenating every `LlmEvent::TextDelta` until `Done` / `Error`.
///   4. Returning the assembled string trimmed of trailing whitespace.
pub struct LlmParaphraseProvider {
    provider: Arc<dyn LlmProvider>,
    runtime: tokio::runtime::Handle,
    model: String,
    system_prompt: String,
    max_tokens: u32,
    request_timeout: Duration,
}

impl LlmParaphraseProvider {
    /// Construct using the *current* tokio runtime handle. Must be called from
    /// inside a tokio runtime (e.g. from `#[tokio::main]` or a `Runtime::block_on`).
    pub fn new(provider: Arc<dyn LlmProvider>, model: impl Into<String>) -> Self {
        Self::with_handle(provider, model, tokio::runtime::Handle::current())
    }

    /// Construct with an explicit runtime handle. Use this when the provider
    /// is built outside a runtime (e.g. tests or CLIs that build the handle
    /// up front).
    pub fn with_handle(
        provider: Arc<dyn LlmProvider>,
        model: impl Into<String>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            provider,
            runtime,
            model: model.into(),
            system_prompt: DEFAULT_PARAPHRASE_SYSTEM_PROMPT.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Override the system prompt. Default text is
    /// `DEFAULT_PARAPHRASE_SYSTEM_PROMPT`.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Override the output-token cap. Default is `DEFAULT_MAX_TOKENS`.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Override the per-call wall-clock cap. Default is
    /// `DEFAULT_REQUEST_TIMEOUT`.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    fn build_request(&self, body: &str) -> LlmRequest {
        LlmRequest {
            model: self.model.clone(),
            system: self.system_prompt.clone(),
            messages: vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: body.to_string(),
                }],
            )],
            tools: Vec::<ToolDef>::new(),
            max_tokens: self.max_tokens,
            thinking: None,
            reasoning_effort: None,
            cache_tier: None,
            routing_hint: None,
            stop_sequences: Vec::new(),
            web_search: false,
            conversation_id: None,
            client_context_tokens: None,
            temperature: None,
            omit_max_tokens: false,
        }
    }

    async fn run_async(&self, body: &str) -> Result<String, LlmParaphraseError> {
        let request = self.build_request(body);
        let rx = self.provider.stream(&request).await?;
        let collected = tokio::time::timeout(self.request_timeout, collect_text(rx))
            .await
            .map_err(|_| LlmParaphraseError::Timeout(self.request_timeout))??;
        let trimmed = collected.trim().to_string();
        if trimmed.is_empty() {
            return Err(LlmParaphraseError::Empty);
        }
        Ok(trimmed)
    }
}

#[async_trait]
impl AsyncParaphrase for LlmParaphraseProvider {
    async fn paraphrase_async(&self, body: &str) -> Result<String, LlmParaphraseError> {
        self.run_async(body).await
    }
}

impl ParaphraseProvider for LlmParaphraseProvider {
    fn paraphrase_blocking(&self, body: &str, _seed_token: &str) -> Result<String, String> {
        // Bridge sync→async. `Generation::run` calls us inside
        // `spawn_blocking`, so we are off the reactor and `Handle::block_on`
        // is safe (nested-runtime panic is impossible from a blocking-pool
        // thread).
        self.runtime
            .block_on(self.run_async(body))
            .map_err(|e| e.to_string())
    }
}

/// Typed error surface for the real provider. Mapped to
/// `MutationError::LlmUnavailable(String)` at the `Paraphrase` mutator
/// boundary via `Display`.
#[derive(Debug, thiserror::Error)]
pub enum LlmParaphraseError {
    #[error("provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("provider returned an error event: {0}")]
    ProviderEvent(String),

    #[error("paraphrase timed out after {0:?}")]
    Timeout(Duration),

    #[error("stream ended before any Done/Error event")]
    StreamEndedEarly,

    #[error("provider returned an empty paraphrase")]
    Empty,
}

/// Drain a streaming `LlmEvent` channel until `Done` or `Error`, returning
/// the concatenated `TextDelta` payloads. Tool-use, thinking, and other
/// non-text events are ignored — the paraphrase prompt instructs the model
/// to emit text only, and we don't want a stray tool call to bleed into the
/// skill body.
async fn collect_text(
    mut rx: tokio::sync::mpsc::Receiver<LlmEvent>,
) -> Result<String, LlmParaphraseError> {
    let mut buf = String::new();
    while let Some(event) = rx.recv().await {
        match event {
            LlmEvent::TextDelta(chunk) => buf.push_str(&chunk),
            LlmEvent::Done { .. } => return Ok(buf),
            LlmEvent::Error(message) => return Err(LlmParaphraseError::ProviderEvent(message)),
            // Ignore: model may emit thinking deltas (Anthropic), stray
            // tool-use blocks (every provider, on prompt non-compliance), or
            // Flux web-search citations/results — none belong in a paraphrase.
            LlmEvent::ThinkingDelta(_)
            | LlmEvent::ThinkingSubject(_)
            | LlmEvent::ToolUse { .. }
            | LlmEvent::Citations(_)
            | LlmEvent::SearchResults(_)
            | LlmEvent::ProviderMeta { .. } => {}
        }
    }
    Err(LlmParaphraseError::StreamEndedEarly)
}

// Tests live in `tests/llm_paraphrase_test.rs` (integration). The crate-level
// `clippy::unwrap_used` deny makes inline `#[cfg(test)]` modules harder to
// write than they're worth here, and the integration-test surface also
// exercises the public API a user actually consumes.
