//! `RunRecord` — the captured output of one `run_cell` invocation.
//!
//! Fields are always redacted before storage so credential shapes never leak
//! into test output or assertion messages.
//!
//! Many fields (`exit`, `config_toml`, `requests`) are scaffolded for later
//! Tasks and are not yet read by Task 2's single test.
#![allow(dead_code)]

use std::path::Path;

use super::super::mock_llm::RecordedRequest;

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// Mask credential shapes before storing any string in a `RunRecord`.
///
/// Patterns covered (conservative prefix-match so the redaction is
/// self-describing in output):
/// - `sk-ant-…`     Anthropic API keys
/// - `sk-…`         OpenAI and OpenAI-compatible keys
/// - `xai-…`        xAI / Grok keys
/// - `sk-or-…`      OpenRouter keys
/// - `r8_…`         Replicate tokens
/// - `gsk_…`        Groq keys
/// - `eyJ…`         JWT Bearer tokens (base64 header starts with `eyJ`)
pub fn redact(s: &str) -> String {
    // Simple state-machine redactor: scan for prefix, then replace until the
    // next whitespace or end-of-string.  Using a simple loop instead of a
    // regex dep keeps the implementation self-contained and matches the
    // project's "no new deps for one function" rule.
    const PREFIXES: &[&str] = &["sk-ant-", "sk-or-", "sk-", "xai-", "r8_", "gsk_", "eyJ"];

    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;

    'outer: while i < bytes.len() {
        // Try each prefix at the current position.
        for &prefix in PREFIXES {
            if bytes[i..].starts_with(prefix.as_bytes()) {
                // Emit the redaction label.
                out.push_str("<REDACTED>");
                // Skip until whitespace, end-of-string, or a `"` quote
                // (common in TOML/JSON contexts).
                i += prefix.len();
                while i < bytes.len()
                    && bytes[i] != b' '
                    && bytes[i] != b'\t'
                    && bytes[i] != b'\n'
                    && bytes[i] != b'\r'
                    && bytes[i] != b'"'
                    && bytes[i] != b'\''
                {
                    i += 1;
                }
                continue 'outer;
            }
        }
        // No prefix matched — copy the byte verbatim.
        out.push(bytes[i] as char);
        i += 1;
    }

    out
}

// ---------------------------------------------------------------------------
// RunRecord
// ---------------------------------------------------------------------------

/// The captured outcome of one `run_cell` invocation.
///
/// All string fields are redacted via [`redact`] before storage.
#[derive(Debug)]
pub struct RunRecord {
    /// Terminal screen contents at the point `RunRecord::capture` was called
    /// (after `pty.quit()`). Credential shapes are masked.
    pub final_screen: String,

    /// 0 on clean exit, 1 on any non-zero exit (portable_pty does not expose
    /// the raw code), None if still running.
    pub exit: Option<i32>,

    /// The contents of `<home>/config.toml` at capture time, if it exists.
    /// Credential shapes are masked.
    pub config_toml: Option<String>,

    /// Requests recorded by a `MockLlm` server during the cell run.
    /// Empty when the cell does not wire up a mock (the majority of harness
    /// cells do not drive LLM turns).
    pub requests: Vec<RecordedRequest>,

    /// True iff the `<home>/.dirty-death` sentinel file exists at capture
    /// time.  The binary writes this file when it exits abnormally (panic,
    /// signal, unclean shutdown).  A well-behaved clean-boot cell must never
    /// leave this sentinel behind.
    pub dirty_death: bool,
}

impl RunRecord {
    /// Capture the final outcome of a cell run.
    ///
    /// Call order in `run_cell`:
    /// 1. `final_screen = redact(&pty.screen_text())` — snapshot BEFORE quit
    /// 2. `pty.quit()` — clean shutdown (waits for exit)
    /// 3. `RunRecord::capture_post_quit(home, &mut pty, final_screen)` — reads
    ///    filesystem state now that the process has exited cleanly
    ///
    /// This two-phase approach ensures:
    /// - `final_screen` reflects the script's last UI state (e.g. "Workspace")
    /// - `dirty_death` is checked after the `CrashSentinel` Drop has run, so a
    ///   clean exit correctly shows `false` (the sentinel file is gone)
    #[cfg(unix)]
    pub fn capture_post_quit(
        home: &Path,
        pty: &mut super::super::pty::Pty,
        final_screen: String,
    ) -> Self {
        let config_toml = std::fs::read_to_string(home.join("config.toml"))
            .ok()
            .map(|s| redact(&s));
        // Check after clean exit: the CrashSentinel Drop removes the file on
        // clean shutdown, so this should be false for a well-behaved run.
        let dirty_death = home.join(".dirty-death").exists();

        // Best-effort: process has exited (quit() waited), so try_wait
        // should return immediately with a status.
        let exit = pty
            .wait_for_exit(std::time::Duration::from_millis(100))
            .and_then(|status| {
                if status.success() {
                    Some(0i32)
                } else {
                    Some(1i32)
                }
            });

        Self {
            final_screen,
            exit,
            config_toml,
            requests: Vec::new(),
            dirty_death,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_openai_key() {
        let s = "api_key = \"sk-ant-harness-not-real-key-0000000000\"";
        let r = redact(s);
        assert!(!r.contains("sk-ant-"), "key prefix must be masked");
        assert!(r.contains("<REDACTED>"), "must contain placeholder");
    }

    #[test]
    fn redact_xai_key() {
        let s = "Authorization: Bearer xai-supersecrettoken123";
        let r = redact(s);
        assert!(!r.contains("xai-"), "xai key must be masked");
        assert!(r.contains("<REDACTED>"));
    }

    #[test]
    fn redact_leaves_normal_text_alone() {
        let s = "hello world no credentials here";
        assert_eq!(redact(s), s);
    }

    #[test]
    fn redact_multiple_keys_in_one_string() {
        let s = "key1=sk-abc key2=sk-or-xyz";
        let r = redact(s);
        assert!(!r.contains("sk-abc"), "first key must be masked");
        assert!(!r.contains("sk-or-xyz"), "second key must be masked");
        let count = r.matches("<REDACTED>").count();
        assert_eq!(count, 2, "two placeholders expected; got: {r}");
    }
}
