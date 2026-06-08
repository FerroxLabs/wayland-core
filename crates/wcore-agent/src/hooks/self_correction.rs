//! W8b D.1 — `SelfCorrectionHook`.
//!
//! Subscribes to `post_tool_use` and, when a tool call returns an error,
//! classifies the failure mode (compile / test-fail / permission /
//! network / parse / generic) and injects a correction prompt as a user
//! message for the next turn. The agent then sees a concise "you just
//! got <class>, try X next" nudge without the host having to interrupt.
//!
//! Modes (per design §4.4):
//!   * `Off`        — never injects; the hook is effectively a no-op.
//!   * `Enabled`    — injects on `is_error == true` only.
//!   * `Aggressive` — injects on every `post_tool_use`, regardless of
//!     success. Useful for evaluation harnesses and reflection-heavy
//!     experimental workflows; off by default because it doubles
//!     prompt volume.
//!
//! The classifier is intentionally a small allowlist of substring
//! matches against `output`. Brittle by design — the prompt asks the
//! agent to verify rather than acting on the classification — but
//! cheap, deterministic, and easy to extend.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use wcore_types::message::{ContentBlock, Message, Role};

use crate::hooks::{Hook, HookAction};

/// Operating mode for the hook. Driven by Agent YAML
/// (`agent.self_correct: off|enabled|aggressive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelfCorrectMode {
    Off,
    #[default]
    Enabled,
    Aggressive,
}

/// Coarse classification of tool output text. Used only to pick a
/// correction-prompt template; the agent is the one that decides what
/// to actually do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Compile,
    TestFail,
    Permission,
    Network,
    Parse,
    Generic,
}

/// Subscriber that nudges the agent toward self-correction.
pub struct SelfCorrectionHook {
    mode: SelfCorrectMode,
}

impl SelfCorrectionHook {
    pub fn new(mode: SelfCorrectMode) -> Self {
        Self { mode }
    }

    pub fn mode(&self) -> SelfCorrectMode {
        self.mode
    }
}

impl Default for SelfCorrectionHook {
    fn default() -> Self {
        Self::new(SelfCorrectMode::default())
    }
}

/// Classify the failure mode by scanning `output` for known phrases.
/// First-match wins, deterministic across runs.
pub fn classify_error(output: &str) -> ErrorClass {
    let lower = output.to_lowercase();
    // Compile errors first — these are usually language tooling output
    // and benefit most from the structured prompt.
    if lower.contains("could not compile")
        || lower.contains("error: cannot find")
        || lower.contains("error[e")
        || lower.contains("compilation failed")
        || lower.contains("syntaxerror")
        || lower.contains("unresolved")
    {
        return ErrorClass::Compile;
    }
    if lower.contains("test failed")
        || lower.contains("tests failed")
        || lower.contains("assertion failed")
        || lower.contains("expected")
            && (lower.contains("but got") || lower.contains("got") || lower.contains("found"))
    {
        return ErrorClass::TestFail;
    }
    if lower.contains("permission denied")
        || lower.contains("eacces")
        || lower.contains("operation not permitted")
        || lower.contains("forbidden")
    {
        return ErrorClass::Permission;
    }
    if lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("network unreachable")
        || lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("dns")
    {
        return ErrorClass::Network;
    }
    if lower.contains("parse error")
        || lower.contains("invalid json")
        || lower.contains("unexpected token")
        || lower.contains("malformed")
    {
        return ErrorClass::Parse;
    }
    ErrorClass::Generic
}

/// Build the correction prompt the hook will inject. Includes the tool
/// name + a compact snippet of the input so the next turn doesn't need
/// to re-read the failed call.
pub fn build_correction_prompt(class: ErrorClass, tool: &str, input: &Value) -> String {
    let input_snippet = serde_json::to_string(input)
        .map(|s| {
            if s.len() > 200 {
                format!("{}…", &s[..200])
            } else {
                s
            }
        })
        .unwrap_or_else(|_| "<unprintable>".to_string());
    let hint = match class {
        ErrorClass::Compile => {
            "The previous tool call surfaced a compile error. Read the error \
             carefully, narrow the fix to the smallest change that compiles, \
             then re-run."
        }
        ErrorClass::TestFail => {
            "A test failed. Read the failure message in full, identify which \
             assertion fired, and inspect the code under test before changing \
             the test itself."
        }
        ErrorClass::Permission => {
            "Permission was denied. Check the path's ownership/mode, or pick \
             a writable location (e.g. tempdir, project root) instead of \
             retrying with elevated privileges."
        }
        ErrorClass::Network => {
            "A network operation failed. Verify the endpoint URL, confirm \
             connectivity, and consider an offline-capable alternative \
             before retrying."
        }
        ErrorClass::Parse => {
            "Output failed to parse. Inspect the raw bytes (use a smaller \
             slice if it's large) and adjust the parser or the producer."
        }
        ErrorClass::Generic => {
            "The previous tool call errored. Read the full error text before \
             your next action and avoid silently retrying the same input."
        }
    };
    format!(
        "Self-correction (W8b D.1): tool `{tool}` returned an error.\n\
         {hint}\n\
         Failing input snippet: {input_snippet}"
    )
}

#[async_trait]
impl Hook for SelfCorrectionHook {
    fn name(&self) -> &str {
        "self_correction"
    }

    async fn post_tool_use(
        &self,
        tool: &str,
        _call_id: &str,
        input: &Value,
        output: &str,
        is_error: bool,
    ) -> HookAction {
        let should_fire = match self.mode {
            SelfCorrectMode::Off => false,
            SelfCorrectMode::Enabled => is_error,
            SelfCorrectMode::Aggressive => true,
        };
        if !should_fire {
            return HookAction::Continue;
        }
        let class = classify_error(output);
        let prompt = build_correction_prompt(class, tool, input);
        HookAction::InjectMessage(Message::new(
            Role::User,
            vec![ContentBlock::Text { text: prompt }],
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classify_compile_error() {
        let out = "error[E0432]: could not compile foo";
        assert_eq!(classify_error(out), ErrorClass::Compile);
    }

    #[test]
    fn classify_test_failure() {
        let out = "test result: FAILED. assertion failed at line 12";
        assert_eq!(classify_error(out), ErrorClass::TestFail);
    }

    #[test]
    fn classify_permission_denied() {
        let out = "/etc/hosts: Permission denied";
        assert_eq!(classify_error(out), ErrorClass::Permission);
    }

    #[test]
    fn classify_network_timeout() {
        let out = "connection refused while reaching example.com";
        assert_eq!(classify_error(out), ErrorClass::Network);
    }

    #[test]
    fn classify_parse_error() {
        let out = "parse error: unexpected token";
        assert_eq!(classify_error(out), ErrorClass::Parse);
    }

    #[test]
    fn classify_fallback_to_generic() {
        let out = "something weird";
        assert_eq!(classify_error(out), ErrorClass::Generic);
    }

    #[test]
    fn prompt_includes_tool_name_and_input_snippet() {
        let p = build_correction_prompt(
            ErrorClass::Compile,
            "Bash",
            &json!({"command":"cargo test"}),
        );
        assert!(p.contains("Bash"));
        assert!(p.contains("cargo test"));
        assert!(p.contains("compile"));
    }

    #[test]
    fn prompt_truncates_oversized_inputs() {
        let big = "x".repeat(2000);
        let p = build_correction_prompt(ErrorClass::Generic, "Read", &json!({"file_path": big}));
        // Snippet portion should be capped at the 200-char policy.
        assert!(p.len() < 1000);
        assert!(p.ends_with("…\"}") || p.contains("…"));
    }
}
