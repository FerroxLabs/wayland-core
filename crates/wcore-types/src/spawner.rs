use async_trait::async_trait;

use crate::message::TokenUsage;

/// Configuration for a sub-agent invocation.
#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    /// Descriptive name for logging
    pub name: String,
    /// The task prompt
    pub prompt: String,
    /// Max turns for this sub-agent (typically lower than main agent)
    pub max_turns: usize,
    /// Max output tokens per response
    pub max_tokens: u32,
    /// Optional system prompt override
    pub system_prompt: Option<String>,
    /// Slice-1 MoP: pin this sub-agent to a named provider (resolved by
    /// `CouncilProviderResolver`). `None` ⇒ inherit the spawner's provider.
    pub provider: Option<String>,
    /// Optional model override applied to the child engine config. `None` ⇒
    /// inherit the (resolved) provider's default model.
    pub model: Option<String>,
    /// Crucible #3: optional sampling temperature applied to the child engine's
    /// requests via `child_config`. `None` ⇒ inherit the base config's
    /// temperature (the engine then omits the field unless the base set one).
    pub temperature: Option<f32>,
}

/// Overrides applied when spawning a fork-mode skill sub-agent.
#[derive(Debug, Clone, Default)]
pub struct ForkOverrides {
    /// Replace the parent's configured model with this one.
    pub model: Option<String>,
    /// Reasoning effort ("low"/"medium"/"high"/"max").
    pub effort: Option<String>,
    /// Restrict registered tools to this list; empty = all built-in tools.
    pub allowed_tools: Vec<String>,
}

/// Result from a completed sub-agent execution.
#[derive(Debug)]
pub struct SubAgentResult {
    pub name: String,
    pub text: String,
    pub usage: TokenUsage,
    pub turns: usize,
    pub is_error: bool,
}

impl SubAgentResult {
    /// Build a terminal error result for a sub-agent that never ran (e.g. its
    /// pinned provider could not be resolved). Zero usage, zero turns,
    /// `is_error = true`.
    pub fn error(name: &str, text: &str) -> Self {
        Self {
            name: name.to_string(),
            text: text.to_string(),
            usage: TokenUsage::default(),
            turns: 0,
            is_error: true,
        }
    }
}

/// Abstraction over fork-mode agent spawning — enables mock implementations in tests.
#[async_trait]
pub trait Spawner: Send + Sync {
    /// Spawn a fork-mode sub-agent with optional overrides and wait for its result.
    async fn spawn_fork(&self, config: SubAgentConfig, overrides: ForkOverrides) -> SubAgentResult;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_agent_config_carries_optional_provider_and_model() {
        let c = SubAgentConfig {
            name: "p".into(),
            prompt: "x".into(),
            max_turns: 1,
            max_tokens: 16,
            system_prompt: None,
            provider: Some("openai".into()),
            model: Some("gpt-5.5".into()),
            temperature: None,
        };
        assert_eq!(c.provider.as_deref(), Some("openai"));
        assert_eq!(c.model.as_deref(), Some("gpt-5.5"));
    }
}
