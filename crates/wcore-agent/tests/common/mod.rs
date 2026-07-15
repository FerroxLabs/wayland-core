// Shared test utilities for integration tests.
#![allow(dead_code)]

pub mod models;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use wcore_agent::confirm::ToolConfirmer;
use wcore_config::config::{Config, ProviderType, SessionConfig, ToolsConfig};
use wcore_egress::{AllowAllPolicy, EgressClient};
use wcore_protocol::events::ToolCategory;
use wcore_providers::retry::{builder_send_with_retry, scope_max_retries};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::Tool;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{StopReason, TokenUsage};
use wcore_types::tool::ToolResult;

// ---------------------------------------------------------------------------
// MockLlmProvider — deterministic LLM for engine / spawn tests
// ---------------------------------------------------------------------------

/// A mock LLM provider that emits a pre-configured sequence of events.
/// Each call to `stream` pops the first sequence from `responses`.
/// When `responses` is empty it falls back to a single EndTurn with empty text.
pub struct MockLlmProvider {
    responses: Mutex<Vec<Vec<LlmEvent>>>,
    physical_url: Option<String>,
}

impl MockLlmProvider {
    /// Create a provider that returns a single text response then ends.
    pub fn with_text_response(text: &str) -> Self {
        let events = vec![
            LlmEvent::TextDelta(text.to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                    StopReason::EndTurn,
                ),
                usage: TokenUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ];
        Self {
            responses: Mutex::new(vec![events]),
            physical_url: None,
        }
    }

    /// Create a provider that returns a single tool_use then ends with ToolUse stop reason.
    pub fn with_tool_use(id: &str, name: &str, input: Value) -> Self {
        let events = vec![
            LlmEvent::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input,
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                    StopReason::ToolUse,
                ),
                usage: TokenUsage {
                    input_tokens: 80,
                    output_tokens: 30,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                },
            },
        ];
        Self {
            responses: Mutex::new(vec![events]),
            physical_url: None,
        }
    }

    /// Create a provider with multiple turns of pre-configured event sequences.
    /// Each call to `stream` consumes the next sequence.
    pub fn with_turns(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            responses: Mutex::new(turns),
            physical_url: None,
        }
    }

    /// Create a provider that returns custom events.
    pub fn with_events(events: Vec<LlmEvent>) -> Self {
        Self {
            responses: Mutex::new(vec![events]),
            physical_url: None,
        }
    }

    /// Route each scripted response through a real provider send boundary.
    /// Persisted-session tests opt in so the journal receives authoritative
    /// physical-attempt identity before the scripted stream becomes visible.
    pub fn with_physical_url(mut self, url: impl Into<String>) -> Self {
        self.physical_url = Some(url.into());
        self
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        if let Some(url) = self.physical_url.as_deref() {
            let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
            let response = scope_max_retries(0, builder_send_with_retry(client.get(url))).await?;
            if !response.status().is_success() {
                return Err(ProviderError::Api {
                    status: response.status().as_u16(),
                    message: "fixture response".into(),
                });
            }
        }

        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                // Fallback: end turn with empty text
                vec![LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                        StopReason::EndTurn,
                    ),
                    usage: TokenUsage::default(),
                }]
            } else {
                responses.remove(0)
            }
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

// ---------------------------------------------------------------------------
// MockTool — deterministic tool for orchestration tests
// ---------------------------------------------------------------------------

/// A simple mock tool that returns a pre-configured result.
pub struct MockTool {
    pub tool_name: String,
    pub tool_description: String,
    pub concurrent_safe: bool,
    pub result: Mutex<ToolResult>,
    pub max_result_size: usize,
}

impl MockTool {
    pub fn new(name: &str, result: &str, is_error: bool) -> Self {
        Self {
            tool_name: name.to_string(),
            tool_description: format!("Mock tool: {}", name),
            concurrent_safe: true,
            result: Mutex::new(ToolResult {
                content: result.to_string(),
                is_error,
            }),
            max_result_size: 50_000,
        }
    }

    pub fn sequential(name: &str, result: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            tool_description: format!("Mock sequential tool: {}", name),
            concurrent_safe: false,
            result: Mutex::new(ToolResult {
                content: result.to_string(),
                is_error: false,
            }),
            max_result_size: 50_000,
        }
    }

    pub fn with_max_result_size(mut self, max_result_size: usize) -> Self {
        self.max_result_size = max_result_size;
        self
    }
}

#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        self.concurrent_safe
    }

    async fn execute(&self, _input: Value) -> ToolResult {
        self.result.lock().unwrap().clone()
    }

    fn max_result_size(&self) -> usize {
        self.max_result_size
    }
}

// ---------------------------------------------------------------------------
// ExecMockTool — mock tool with Exec category (requires approval)
// ---------------------------------------------------------------------------

/// A mock tool that returns a pre-configured result, with Exec category.
pub struct ExecMockTool {
    pub tool_name: String,
    pub result: Mutex<ToolResult>,
}

impl ExecMockTool {
    pub fn new(name: &str, result: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            result: Mutex::new(ToolResult {
                content: result.to_string(),
                is_error: false,
            }),
        }
    }
}

#[async_trait]
impl Tool for ExecMockTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        "Mock exec tool"
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, _input: Value) -> ToolResult {
        self.result.lock().unwrap().clone()
    }
}

// ---------------------------------------------------------------------------
// Helper: build a minimal Config for testing
// ---------------------------------------------------------------------------

pub fn test_config() -> Config {
    Config {
        provider_label: "anthropic".to_string(),
        provider: ProviderType::Anthropic,
        api_key: "test-key".to_string(),
        base_url: "http://localhost:0".to_string(),
        model: "test-model".to_string(),
        max_tokens: 4096,
        max_turns: Some(10),
        system_prompt: Some("You are a test assistant.".to_string()),
        compat: wcore_config::compat::ProviderCompat::anthropic_defaults(),
        tools: ToolsConfig {
            auto_approve: true,
            allow_list: vec![],
            skills: wcore_config::config::SkillsPermissionConfig::default(),
            verify_edits: false,
            windows_shell: None,
            env_passthrough: Vec::new(),
            sandbox: None,
            allow_no_sandbox: None,
        },
        session: SessionConfig {
            enabled: false,
            // Cross-platform tempdir — never written because `enabled: false`,
            // but `std::env::temp_dir()` keeps the literal honest on Windows
            // (no hardcoded `/tmp/...` reaching any future validator).
            directory: std::env::temp_dir()
                .join("wayland-core-test-sessions")
                .to_string_lossy()
                .into_owned(),
            max_sessions: 5,
        },
        ..Default::default()
    }
}

/// Create a ToolConfirmer that auto-approves everything.
pub fn auto_approve_confirmer() -> Arc<Mutex<ToolConfirmer>> {
    Arc::new(Mutex::new(ToolConfirmer::new(true, vec![])))
}
