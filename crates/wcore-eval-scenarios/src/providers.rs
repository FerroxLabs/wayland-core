//! Provider matrix declarations — plan §4.
//!
//! T1/T2 declares the shape; **T4** fills in the per-provider model
//! tables, the API-key availability checks, the cost-rate table for
//! pre-flight estimates, and the strict-mode SKIP/FAIL logic.

use std::ffi::OsStr;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

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

impl FromStr for ProviderId {
    type Err = ResolveError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "deepseek" => Ok(Self::DeepSeek),
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAI),
            other => Err(ResolveError::InvalidProvider(other.to_string())),
        }
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
#[derive(Clone)]
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

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("id", &self.id)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Credential presence snapshot used to plan runs without exposing or copying
/// the credential values themselves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderAvailability {
    pub deepseek: bool,
    pub anthropic: bool,
    pub openai: bool,
}

impl ProviderAvailability {
    pub fn all() -> Self {
        Self {
            deepseek: true,
            anthropic: true,
            openai: true,
        }
    }

    pub fn has(self, provider: ProviderId) -> bool {
        match provider {
            ProviderId::DeepSeek => self.deepseek,
            ProviderId::Anthropic => self.anthropic,
            ProviderId::OpenAI => self.openai,
        }
    }

    /// Snapshot credential presence without copying values into the run plan.
    pub fn from_environment() -> Self {
        Self {
            deepseek: credential_value_present(
                std::env::var_os(ProviderId::DeepSeek.env_var()).as_deref(),
            ),
            anthropic: credential_value_present(
                std::env::var_os(ProviderId::Anthropic.env_var()).as_deref(),
            ),
            openai: credential_value_present(
                std::env::var_os(ProviderId::OpenAI.env_var()).as_deref(),
            ),
        }
    }
}

fn credential_value_present(value: Option<&OsStr>) -> bool {
    value.is_some_and(|value| !value.to_string_lossy().trim().is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSkip {
    pub provider: ProviderId,
    pub missing_key: &'static str,
}

#[derive(Debug, Clone)]
pub struct ProviderResolution {
    pub runnable: Vec<ProviderConfig>,
    pub skipped: Vec<ProviderSkip>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("unknown provider `{0}` (expected deepseek, anthropic, openai, or matrix)")]
    InvalidProvider(String),
    #[error("missing required provider credentials: {providers:?}")]
    MissingCredentials { providers: Vec<ProviderId> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderOverride {
    Provider(ProviderId),
    Matrix,
}

impl FromStr for ProviderOverride {
    type Err = ResolveError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "matrix" {
            Ok(Self::Matrix)
        } else {
            ProviderId::from_str(value).map(Self::Provider)
        }
    }
}

/// Parse provider overrides with the documented CLI-over-environment
/// precedence. Empty values are treated as absent rather than as provider IDs.
pub fn provider_override(
    cli: Option<&str>,
    environment: Option<&str>,
) -> Result<Option<ProviderOverride>, ResolveError> {
    cli.map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| environment.map(str::trim).filter(|value| !value.is_empty()))
        .map(ProviderOverride::from_str)
        .transpose()
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
            .filter(|value| !value.trim().is_empty())
    }
}

/// Resolve a scenario's provider choice into explicit runnable and skipped
/// entries. A CLI/environment override narrows any scenario choice, including
/// `Matrix`, to that provider. Strict mode rejects the entire plan if any
/// selected provider is unavailable.
pub fn resolve(
    choice: ProviderChoice,
    provider_override: Option<ProviderOverride>,
    availability: ProviderAvailability,
    strict: bool,
) -> Result<ProviderResolution, ResolveError> {
    const MATRIX: [ProviderId; 3] = [
        ProviderId::DeepSeek,
        ProviderId::Anthropic,
        ProviderId::OpenAI,
    ];

    let selected: &[ProviderId] = match &provider_override {
        Some(ProviderOverride::Provider(provider)) => std::slice::from_ref(provider),
        Some(ProviderOverride::Matrix) => &MATRIX,
        None => match choice {
            ProviderChoice::Default | ProviderChoice::ForceDeepSeek => {
                std::slice::from_ref(&MATRIX[0])
            }
            ProviderChoice::ForceAnthropic => std::slice::from_ref(&MATRIX[1]),
            ProviderChoice::ForceOpenAI => std::slice::from_ref(&MATRIX[2]),
            ProviderChoice::Matrix => &MATRIX,
        },
    };

    let missing: Vec<ProviderId> = selected
        .iter()
        .copied()
        .filter(|provider| !availability.has(*provider))
        .collect();
    if strict && !missing.is_empty() {
        return Err(ResolveError::MissingCredentials { providers: missing });
    }

    let mut runnable = Vec::with_capacity(selected.len());
    let mut skipped = Vec::with_capacity(missing.len());
    for provider in selected.iter().copied() {
        if availability.has(provider) {
            runnable.push(default_config(provider));
        } else {
            skipped.push(ProviderSkip {
                provider,
                missing_key: provider.env_var(),
            });
        }
    }

    Ok(ProviderResolution { runnable, skipped })
}

fn default_config(provider: ProviderId) -> ProviderConfig {
    let model = match provider {
        ProviderId::DeepSeek => "deepseek-chat",
        ProviderId::Anthropic => "claude-sonnet-4-6",
        ProviderId::OpenAI => "gpt-4o",
    };
    ProviderConfig::new(provider, model)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(resolution: &ProviderResolution) -> Vec<ProviderId> {
        resolution.runnable.iter().map(|run| run.id).collect()
    }

    #[test]
    fn override_precedence_is_cli_then_environment() {
        assert_eq!(
            provider_override(Some("openai"), Some("anthropic")),
            Ok(Some(ProviderOverride::Provider(ProviderId::OpenAI)))
        );
        assert_eq!(
            provider_override(None, Some("anthropic")),
            Ok(Some(ProviderOverride::Provider(ProviderId::Anthropic)))
        );
        assert_eq!(provider_override(None, None), Ok(None));
    }

    #[test]
    fn matrix_override_and_blank_values_are_explicit() {
        assert_eq!(
            provider_override(Some("matrix"), Some("openai")),
            Ok(Some(ProviderOverride::Matrix))
        );
        assert_eq!(provider_override(Some("  \t"), None), Ok(None));
    }

    #[test]
    fn whitespace_only_credentials_are_absent() {
        assert!(!credential_value_present(Some(std::ffi::OsStr::new(
            " \t\n"
        ))));
        assert!(credential_value_present(Some(std::ffi::OsStr::new(
            "fixture-key"
        ))));
    }

    #[test]
    fn invalid_override_is_rejected_before_execution() {
        assert_eq!(
            provider_override(Some("not-a-provider"), None),
            Err(ResolveError::InvalidProvider("not-a-provider".into()))
        );
    }

    #[test]
    fn forced_and_overridden_providers_use_explicit_models() {
        let forced = resolve(
            ProviderChoice::ForceAnthropic,
            None,
            ProviderAvailability::all(),
            false,
        )
        .unwrap();
        assert_eq!(ids(&forced), [ProviderId::Anthropic]);
        assert_eq!(forced.runnable[0].model, "claude-sonnet-4-6");

        let overridden = resolve(
            ProviderChoice::ForceAnthropic,
            Some(ProviderOverride::Provider(ProviderId::OpenAI)),
            ProviderAvailability::all(),
            false,
        )
        .unwrap();
        assert_eq!(ids(&overridden), [ProviderId::OpenAI]);
        assert_eq!(overridden.runnable[0].model, "gpt-4o");
    }

    #[test]
    fn matrix_is_stable_and_lenient_mode_records_skips() {
        let resolution = resolve(
            ProviderChoice::Matrix,
            None,
            ProviderAvailability {
                deepseek: true,
                anthropic: false,
                openai: true,
            },
            false,
        )
        .unwrap();

        assert_eq!(ids(&resolution), [ProviderId::DeepSeek, ProviderId::OpenAI]);
        assert_eq!(
            resolution.skipped,
            [ProviderSkip {
                provider: ProviderId::Anthropic,
                missing_key: "ANTHROPIC_API_KEY",
            }]
        );
    }

    #[test]
    fn strict_mode_rejects_any_missing_required_provider() {
        let error = resolve(
            ProviderChoice::Matrix,
            None,
            ProviderAvailability {
                deepseek: true,
                anthropic: false,
                openai: false,
            },
            true,
        )
        .unwrap_err();

        assert_eq!(
            error,
            ResolveError::MissingCredentials {
                providers: vec![ProviderId::Anthropic, ProviderId::OpenAI]
            }
        );
    }

    #[test]
    fn zero_runnable_providers_is_never_an_unexplained_success() {
        let lenient = resolve(
            ProviderChoice::ForceDeepSeek,
            None,
            ProviderAvailability::default(),
            false,
        )
        .unwrap();
        assert!(lenient.runnable.is_empty());
        assert_eq!(lenient.skipped.len(), 1);

        assert!(matches!(
            resolve(
                ProviderChoice::ForceDeepSeek,
                None,
                ProviderAvailability::default(),
                true
            ),
            Err(ResolveError::MissingCredentials { .. })
        ));
    }
}
