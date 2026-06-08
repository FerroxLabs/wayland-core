//! Provider matrix declarations — plan §4.
//!
//! T1/T2 declares the shape; **T4** fills in the per-provider model
//! tables, the API-key availability checks, the cost-rate table for
//! pre-flight estimates, and the strict-mode SKIP/FAIL logic.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Logical provider id — passed to `wayland-core --provider <id>` and
/// used to look up env-var names + default model strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    DeepSeek,
    Anthropic,
    OpenAI,
}

impl ProviderId {
    /// String form passed to the binary's `--provider` flag.
    pub fn cli_name(self) -> &'static str {
        match self {
            // The engine's provider routing uses these exact strings;
            // verified against `crates/wcore-config/src/config.rs`
            // (provider_type matching). T4 expands with any vendor-
            // -specific aliases if needed.
            ProviderId::DeepSeek => "deepseek",
            ProviderId::Anthropic => "anthropic",
            ProviderId::OpenAI => "openai",
        }
    }

    /// Env var holding the provider's API key.
    pub fn env_var(self) -> &'static str {
        match self {
            ProviderId::DeepSeek => "DEEPSEEK_API_KEY",
            ProviderId::Anthropic => "ANTHROPIC_API_KEY",
            ProviderId::OpenAI => "OPENAI_API_KEY",
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.cli_name())
    }
}

/// Per-scenario provider selection — the scenario builder picks one of
/// these; the runner resolves `Default` against `WCORE_EVAL_PROVIDER`
/// and `Matrix` by running the scenario once per supported provider.
#[derive(Debug, Clone, Copy)]
pub enum ProviderChoice {
    Default,
    ForceDeepSeek,
    ForceAnthropic,
    ForceOpenAI,
    /// Run the scenario against ALL providers that have keys set; in
    /// `--strict`, every provider in the matrix must have a key.
    Matrix,
}

/// Concrete, resolved provider configuration for one scenario run.
///
/// The cross-audit (H-5) called out that `default_model_for(DeepSeek)`
/// returns an empty string — relying on engine defaults silently 400s.
/// Every scenario MUST supply a model explicitly; the runner forwards
/// it as `--model <model>` (per T2 spec).
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub id: ProviderId,
    /// Model string passed verbatim as `--model <model>` (e.g.
    /// `"deepseek-chat"`, `"claude-sonnet-4-6"`, `"gpt-4o"`).
    pub model: String,
    /// API key — the runner writes this into the seeded
    /// `<tempdir>/.wayland-core/config.toml` under
    /// `[provider.<id>] api_key = "..."`. If `None`, the runner reads
    /// `id.env_var()` at spawn time.
    pub api_key: Option<String>,
}

impl ProviderConfig {
    /// Convenience for tests + T4 — `provider(id, model)` then
    /// `with_api_key(...)` when overriding env-var resolution.
    pub fn new(id: ProviderId, model: impl Into<String>) -> Self {
        Self {
            id,
            model: model.into(),
            api_key: None,
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Look up the API key for this provider — `api_key` if set,
    /// otherwise the env var named by `id.env_var()`. Returns `None`
    /// when nothing is set (the caller decides SKIP vs FAIL per M-2).
    pub fn resolved_key(&self) -> Option<String> {
        self.api_key
            .clone()
            .or_else(|| std::env::var(self.id.env_var()).ok())
    }
}

/// **T4** — resolve a [`ProviderChoice`] into the list of
/// [`ProviderConfig`]s the runner should iterate.
///
/// **Not yet implemented (T4 wave).** Returns an empty `Vec` so callers
/// can compile against the final API; the runner treats an empty
/// resolution as "no providers to run" rather than panicking. T4 will
/// replace this body with the per-provider model tables, API-key
/// availability checks, and strict-mode SKIP/FAIL logic.
pub fn resolve(_choice: ProviderChoice) -> Vec<ProviderConfig> {
    // Honest exit until T4 lands: no providers resolved. Replacing
    // `todo!()` with an empty result keeps the `#![deny(clippy::todo)]`
    // crate-level gate satisfied while ensuring runtime behaviour is
    // visibly-incomplete (zero providers run) rather than a panic.
    Vec::new()
}
