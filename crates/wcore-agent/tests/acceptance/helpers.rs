// Shared helpers for acceptance tests: provider detection and config builders.

use wcore_config::compat::ProviderCompat;
#[cfg(feature = "live-bedrock")]
use wcore_config::config::BedrockConfig;
use wcore_config::config::{Config, ProviderType, SessionConfig, ToolsConfig};

// ---------------------------------------------------------------------------
// Provider detection
// ---------------------------------------------------------------------------

/// Returns the OpenAI API key if set and non-empty.
pub fn openai_api_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

/// Returns true when AWS Bedrock is configured for use.
///
/// M2.6: Gated behind `live-bedrock`. Bedrock acceptance tests must use
/// `--features live-bedrock` to even compile this helper.
#[cfg(feature = "live-bedrock")]
pub fn bedrock_configured() -> bool {
    let has_profile = std::env::var("AWS_PROFILE")
        .ok()
        .filter(|v| !v.is_empty())
        .is_some();
    let bedrock_flag = std::env::var("CLAUDE_CODE_USE_BEDROCK")
        .ok()
        .filter(|v| v == "1")
        .is_some();
    has_profile && bedrock_flag
}

// ---------------------------------------------------------------------------
// Require macros (M2.6: panic-on-missing-key contract)
// ---------------------------------------------------------------------------
//
// Previously these were silent `skip_if_no_*!` macros. M2.6 inverts the
// contract: with the relevant `live-<provider>` feature enabled, a missing
// credential PANICS — the caller asked for live tests and must supply the
// key. With the feature absent, the macro is not compiled and call sites
// must also be `#[cfg(feature = "live-<provider>")]`-gated.

/// Requires OPENAI_API_KEY at runtime; panics if absent.
/// Usage: `require_openai_api_key!();` at the start of a test function.
/// Must be paired with `#[cfg(feature = "live-openai")]` on the caller.
#[cfg(feature = "live-openai")]
#[allow(unused_macros)]
macro_rules! require_openai_api_key {
    () => {
        #[allow(unused_variables)]
        let openai_api_key = $crate::helpers::openai_api_key().expect(
            "[acceptance] OPENAI_API_KEY required when --features live-openai \
             is enabled (wcore-agent/live-openai)",
        );
    };
}

/// Requires Bedrock to be configured at runtime; panics if absent.
/// Usage: `require_bedrock!();` at the start of a test function.
/// Must be paired with `#[cfg(feature = "live-bedrock")]` on the caller.
#[cfg(feature = "live-bedrock")]
#[allow(unused_macros)]
macro_rules! require_bedrock {
    () => {
        if !$crate::helpers::bedrock_configured() {
            panic!(
                "[acceptance] Bedrock not configured (AWS_PROFILE + \
                 CLAUDE_CODE_USE_BEDROCK=1) when --features live-bedrock \
                 is enabled (wcore-agent/live-bedrock)"
            );
        }
    };
}

// ---------------------------------------------------------------------------
// Config builders
// ---------------------------------------------------------------------------

/// Build a Config for the OpenAI provider (gpt-4o-mini, cheap for tests).
pub fn openai_config(api_key: &str) -> Config {
    Config {
        provider: ProviderType::OpenAI,
        provider_label: "openai".to_string(),
        api_key: api_key.to_string(),
        base_url: "https://api.openai.com".to_string(),
        model: crate::common::models::openai_gpt4o_mini(),
        max_tokens: 256,
        max_turns: Some(3),
        system_prompt: Some("You are a helpful assistant. Be concise.".to_string()),
        compat: ProviderCompat::openai_defaults(),
        tools: ToolsConfig {
            auto_approve: true,
            allow_list: vec![],
            skills: wcore_config::config::SkillsPermissionConfig::default(),
            verify_edits: false,
            windows_shell: None,
        },
        session: SessionConfig {
            enabled: false,
            // Cross-platform tempdir — never written because `enabled: false`,
            // but avoids the hardcoded `/tmp/...` literal on Windows.
            directory: std::env::temp_dir()
                .join("wayland-core-acceptance")
                .to_string_lossy()
                .into_owned(),
            max_sessions: 1,
        },
        ..Default::default()
    }
}

/// Build a Config for the AWS Bedrock provider (Claude Haiku).
///
/// M2.6: Gated behind `live-bedrock`. Imports `BedrockConfig` which is only
/// needed when live-bedrock tests are compiled.
#[cfg(feature = "live-bedrock")]
pub fn bedrock_config() -> Config {
    Config {
        provider: ProviderType::Bedrock,
        provider_label: "bedrock".to_string(),
        api_key: String::new(), // Bedrock uses AWS credentials, not API key
        base_url: String::new(),
        model: "us.anthropic.claude-haiku-4-20250514-v1:0".to_string(),
        max_tokens: 256,
        max_turns: Some(3),
        system_prompt: Some("You are a helpful assistant. Be concise.".to_string()),
        compat: ProviderCompat::anthropic_defaults(),
        tools: ToolsConfig {
            auto_approve: true,
            allow_list: vec![],
            skills: wcore_config::config::SkillsPermissionConfig::default(),
            verify_edits: false,
            windows_shell: None,
        },
        session: SessionConfig {
            enabled: false,
            // Cross-platform tempdir — never written because `enabled: false`,
            // but avoids the hardcoded `/tmp/...` literal on Windows.
            directory: std::env::temp_dir()
                .join("wayland-core-acceptance")
                .to_string_lossy()
                .into_owned(),
            max_sessions: 1,
        },
        bedrock: Some(BedrockConfig::default()),
        ..Default::default()
    }
}
