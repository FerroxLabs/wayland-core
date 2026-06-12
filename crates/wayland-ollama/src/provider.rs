//! `OllamaProvider` — implements `wcore_plugin_api::PluginProvider` AND
//! `wcore_providers::LlmProvider`.
//!
//! Wave OL (closes the W8c.3.D longest-standing stub): the host-side
//! downcast — `Arc<dyn PluginProvider>` → `Arc<dyn LlmProvider>` — happens
//! in `wcore-cli`'s startup, which calls `PluginProvider::as_any` and
//! recovers the concrete `OllamaProvider`. The CLI then hands it to
//! `AgentBootstrap::provider(...)` as the engine's real provider, so a
//! `--model ollama:<name>` invocation routes a turn through this code.
//!
//! Wire format: NDJSON over `POST /api/chat` with `"stream": true`. Each
//! line is a JSON object with shape:
//!
//! ```text
//! {"model":"...","created_at":"...","message":{"role":"assistant","content":"..."},"done":false}
//! ...
//! {"model":"...","created_at":"...","message":{"role":"assistant","content":""},"done":true,
//!  "total_duration":..., "prompt_eval_count":N, "eval_count":M}
//! ```
//!
//! See https://github.com/ollama/ollama/blob/main/docs/api.md#generate-a-chat-completion

use std::any::Any;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::mpsc;
use wcore_plugin_api::registry::providers::PluginProvider;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};

/// Stream channel buffer size. Matches `wcore-providers` siblings (Anthropic
/// uses 32; we use the same so the engine's drain loop never blocks).
const STREAM_CHANNEL_BUFFER: usize = 32;

/// HTTP timeout — Ollama's local generation can be slow on first model
/// load (cold cache). 120s matches the previous `send_chat_blocking`
/// timeout and is well below any reasonable user-perceived ceiling.
const OLLAMA_HTTP_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Error)]
pub enum OllamaError {
    #[error("ollama HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("ollama egress error: {0}")]
    Egress(#[from] wcore_egress::EgressError),
    #[error("ollama returned non-2xx status: {status} body={body}")]
    Status { status: u16, body: String },
    #[error("ollama response missing `message.content`: {0}")]
    ShapeError(Value),
    #[error(
        "ollama provider does not support tool-calling: a {0} content block was \
         encountered but Ollama's /api/chat tools API is not implemented yet"
    )]
    ToolUseUnsupported(&'static str),
}

impl From<OllamaError> for ProviderError {
    fn from(e: OllamaError) -> Self {
        match e {
            OllamaError::Http(err) => ProviderError::Http(err),
            OllamaError::Egress(err) => ProviderError::Egress(err),
            OllamaError::Status { status, body } => ProviderError::Api {
                status,
                message: body,
            },
            OllamaError::ShapeError(v) => ProviderError::Parse(v.to_string()),
            e @ OllamaError::ToolUseUnsupported(_) => ProviderError::Parse(e.to_string()),
        }
    }
}

pub struct OllamaProvider {
    base_url: String,
    model: String,
    http: wcore_egress::EgressClient,
}

impl OllamaProvider {
    /// `base_url` is the full chat endpoint, e.g. `http://localhost:11434/api/chat`.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        // SAFETY: `reqwest::Client::builder().build()` only fails when
        // DNS resolver init or TLS backend setup fails. The default
        // profile builds successfully on every platform we ship.
        let http = wcore_egress::EgressClient::builder()
            .timeout(OLLAMA_HTTP_TIMEOUT)
            .build()
            .expect("reqwest client builder");
        Self {
            base_url: base_url.into(),
            model: model.into(),
            http,
        }
    }

    /// Override the endpoint after construction. Used by tests to point a
    /// pre-built provider at a wiremock server URL.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// One-shot helper, kept for the W8a wiremock unit test. Real engine
    /// turns go through `LlmProvider::stream` instead.
    pub async fn send_chat_blocking(&self, messages: &[Message]) -> Result<String, OllamaError> {
        let mapped: Vec<OllamaChatMessage> = messages
            .iter()
            .map(|m| {
                Ok(OllamaChatMessage {
                    role: ollama_role(&m.role),
                    content: extract_text(m)?,
                })
            })
            .collect::<Result<_, OllamaError>>()?;
        let body = json!({
            "model": self.model,
            "messages": mapped,
            "stream": false,
        });
        let resp = self.http.post(&self.base_url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(OllamaError::Status {
                status: status.as_u16(),
                body: text,
            });
        }
        let value: Value = resp.json().await?;
        value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .ok_or(OllamaError::ShapeError(value))
    }
}

impl PluginProvider for OllamaProvider {
    fn provider_name(&self) -> &str {
        "ollama"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Real streaming `LlmProvider` impl over Ollama's NDJSON `/api/chat`.
///
/// The model field in `LlmRequest.model` may carry the engine-side `ollama:`
/// prefix (e.g. `ollama:llama3`). We strip it before sending — Ollama itself
/// only knows the bare model name.
#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let model = strip_ollama_prefix(&request.model)
            .unwrap_or(&self.model)
            .to_string();

        let mut mapped: Vec<OllamaChatMessage> = Vec::with_capacity(request.messages.len() + 1);
        if !request.system.is_empty() {
            mapped.push(OllamaChatMessage {
                role: "system",
                content: request.system.clone(),
            });
        }
        for m in &request.messages {
            mapped.push(OllamaChatMessage {
                role: ollama_role(&m.role),
                // Fail loudly on tool blocks rather than silently dropping
                // them and letting the model loop with no error.
                content: extract_text(m).map_err(ProviderError::from)?,
            });
        }

        let body = json!({
            "model": model,
            "messages": mapped,
            "stream": true,
        });

        let resp = self
            .http
            .post(&self.base_url)
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::Egress)?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body,
            });
        }

        let (tx, rx) = mpsc::channel(STREAM_CHANNEL_BUFFER);

        tokio::spawn(async move {
            let mut byte_stream = resp.bytes_stream();
            let mut buffer = Vec::<u8>::new();
            let mut input_tokens: u64 = 0;
            let mut output_tokens: u64 = 0;
            let mut stop_reason = StopReason::EndTurn;
            let mut finish_reason = FinishReason::Stop;
            let mut saw_done = false;

            while let Some(chunk) = byte_stream.next().await {
                let bytes = match chunk {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = tx
                            .send(LlmEvent::Error(format!("ollama stream error: {e}")))
                            .await;
                        return;
                    }
                };
                buffer.extend_from_slice(&bytes);

                // Drain complete NDJSON lines. Lines are `\n`-terminated;
                // partial trailing data stays in the buffer.
                while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buffer.drain(..=pos).collect();
                    let line_str = std::str::from_utf8(&line[..line.len().saturating_sub(1)])
                        .unwrap_or("")
                        .trim();
                    if line_str.is_empty() {
                        continue;
                    }
                    let parsed: Value = match serde_json::from_str(line_str) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = tx
                                .send(LlmEvent::Error(format!(
                                    "ollama NDJSON parse error: {e} (line: {line_str})"
                                )))
                                .await;
                            return;
                        }
                    };

                    // Text delta arrives under message.content on each
                    // non-terminal line. Terminal line has done:true and
                    // (typically) an empty content + the usage counters.
                    if let Some(content) = parsed
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_str())
                        && !content.is_empty()
                    {
                        output_tokens = output_tokens.saturating_add(1);
                        if tx
                            .send(LlmEvent::TextDelta(content.to_string()))
                            .await
                            .is_err()
                        {
                            // Receiver dropped — stop early.
                            return;
                        }
                    }

                    if parsed.get("done").and_then(|d| d.as_bool()) == Some(true) {
                        saw_done = true;
                        // Token counts: prefer Ollama's `prompt_eval_count`
                        // / `eval_count` when present, else fall back to
                        // the delta count we maintained ourselves.
                        if let Some(n) = parsed.get("prompt_eval_count").and_then(|v| v.as_u64()) {
                            input_tokens = n;
                        }
                        if let Some(n) = parsed.get("eval_count").and_then(|v| v.as_u64()) {
                            output_tokens = n;
                        }
                        if let Some(reason) = parsed.get("done_reason").and_then(|v| v.as_str()) {
                            (stop_reason, finish_reason) = map_done_reason(reason);
                        }
                        break;
                    }
                }
                if saw_done {
                    break;
                }
            }

            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason,
                    finish_reason,
                    usage: TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 0,
                    },
                })
                .await;
        });

        Ok(rx)
    }
}

fn map_done_reason(reason: &str) -> (StopReason, FinishReason) {
    match reason {
        // Standard Ollama termination — model emitted EOS.
        "stop" => (StopReason::EndTurn, FinishReason::Stop),
        // Hit the model's max context / num_predict cap.
        "length" => (StopReason::MaxTokens, FinishReason::Length),
        // Anything else is non-fatal but we surface as Error so the
        // engine doesn't silently swallow an unfamiliar terminator.
        _ => (StopReason::EndTurn, FinishReason::Error),
    }
}

fn strip_ollama_prefix(model: &str) -> Option<&str> {
    model.strip_prefix("ollama:")
}

fn ollama_role(role: &Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool",
    }
}

fn extract_text(m: &Message) -> Result<String, OllamaError> {
    // Concatenate all text-shaped ContentBlocks. Ollama's tool-use API is
    // model-specific and not implemented yet, so a ToolUse/ToolResult block
    // cannot be represented on the wire. Silently dropping it makes tool
    // output invisible to the model, which then loops with no error — so we
    // fail LOUDLY instead. `Thinking` blocks are non-load-bearing and skipped.
    let mut out = String::new();
    for block in &m.content {
        match block {
            ContentBlock::Text { text } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
            ContentBlock::ToolUse { .. } => {
                return Err(OllamaError::ToolUseUnsupported("tool_use"));
            }
            ContentBlock::ToolResult { .. } => {
                return Err(OllamaError::ToolUseUnsupported("tool_result"));
            }
            ContentBlock::Thinking { .. } => {}
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaChatMessage {
    role: &'static str,
    content: String,
}

/// Internal helper for tests in this crate that need a typed handle to the
/// concrete provider without pulling in a public `as_llm_provider` shim. The
/// host adapter (in `wcore-cli`) uses `PluginProvider::as_any` →
/// `downcast_ref::<OllamaProvider>()` for the same effect.
#[doc(hidden)]
pub fn as_llm_provider(provider: Arc<OllamaProvider>) -> Arc<dyn LlmProvider> {
    provider
}

#[cfg(test)]
mod prefix_tests {
    use super::strip_ollama_prefix;

    #[test]
    fn strips_ollama_prefix() {
        assert_eq!(strip_ollama_prefix("ollama:llama3"), Some("llama3"));
        assert_eq!(strip_ollama_prefix("ollama:llama3:8b"), Some("llama3:8b"));
        assert_eq!(strip_ollama_prefix("llama3"), None);
    }
}

#[cfg(test)]
mod extract_text_tests {
    use super::{OllamaError, extract_text};
    use wcore_types::message::{ContentBlock, Message, Role};

    #[test]
    fn concatenates_text_blocks() {
        let m = Message::new(
            Role::User,
            vec![
                ContentBlock::Text {
                    text: "hello".into(),
                },
                ContentBlock::Text {
                    text: "world".into(),
                },
            ],
        );
        assert_eq!(extract_text(&m).unwrap(), "hello\nworld");
    }

    #[test]
    fn tool_use_block_fails_loudly() {
        // Regression: a ToolUse block was silently dropped, leaving the model
        // looping with no error. It must now surface a visible failure.
        let m = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "Bash".into(),
                input: serde_json::json!({"cmd": "ls"}),
                extra: None,
            }],
        );
        match extract_text(&m) {
            Err(OllamaError::ToolUseUnsupported("tool_use")) => {}
            other => panic!("expected ToolUseUnsupported(tool_use), got {other:?}"),
        }
    }

    #[test]
    fn tool_result_block_fails_loudly() {
        let m = Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".into(),
                content: "output".into(),
                is_error: false,
            }],
        );
        match extract_text(&m) {
            Err(OllamaError::ToolUseUnsupported("tool_result")) => {}
            other => panic!("expected ToolUseUnsupported(tool_result), got {other:?}"),
        }
    }
}
