//! `tts` (text-to-speech) tool formatter.
//!
//! Expected payload shape:
//! ```json
//! { "chars": 320, "provider": "elevenlabs", "path": "/tmp/abc.wav" }
//! ```

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{join_facts, opt_str, opt_u64};
use crate::tui::theme::Theme;

/// The written audio path, under whichever key the payload carries.
fn out_path(payload: &Value) -> Option<&str> {
    opt_str(payload, "file_path").or_else(|| opt_str(payload, "path"))
}

pub struct TtsFormatter;

impl ToolResultFormatter for TtsFormatter {
    // UAT-T3. `TextToSpeechTool` returns
    // `{success, file_path, provider, format, bytes_written, voice_compatible}`
    // (`wcore-tools/src/tts_tool.rs`). `provider` matched; `chars` does not
    // exist (rendered `0`) and the output path is `file_path`, not `path`, so
    // the arrow pointed at nothing and the detail view was empty.
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let mut facts = vec!["Synthesized".to_string()];
        if let Some(n) = opt_u64(payload, "chars") {
            facts.push(format!("{n} chars"));
        }
        if let Some(b) = opt_u64(payload, "bytes_written") {
            facts.push(format!("{b} bytes"));
        }
        if let Some(p) = opt_str(payload, "provider") {
            facts.push(p.to_string());
        }
        if let Some(f) = opt_str(payload, "format") {
            facts.push(f.to_string());
        }
        if let Some(p) = out_path(payload) {
            facts.push(format!("\u{2192} {}", basename(p)));
        }
        join_facts(&facts)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let Some(path) = out_path(payload) else {
            return Vec::new();
        };
        let style = Style::default().fg(theme.text_dim);
        vec![Line::from(Span::styled(path.to_string(), style))]
    }

    /// v0.9.1.1 B4-hunt: render TTS args as a quoted excerpt of the
    /// `text` field instead of the raw JSON dump that previously leaked
    /// into the inline approval card.
    fn format_args(&self, args: &Value) -> Option<String> {
        let text = args.get("text").and_then(Value::as_str)?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        // Clamp to a 50-char excerpt so the approval header stays on one
        // line. The full text is still in the engine — this is just the
        // human-readable preview.
        let chars: Vec<char> = trimmed.chars().collect();
        let preview: String = chars.iter().take(50).collect();
        let suffix = if chars.len() > 50 { "…" } else { "" };
        Some(format!("\"{}{}\"", preview, suffix))
    }
}

/// Last path segment — `/tmp/abc.wav` → `abc.wav`. Empty input → `?`.
fn basename(path: &str) -> String {
    if path.is_empty() {
        return "?".to_string();
    }
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // UAT-T3: these cases previously asserted payload shapes the tool has
    // never emitted, and in several places asserted the FABRICATED output as
    // though it were the specification. They are not weakened here — the
    // fixtures are replaced with the shapes read out of the tool source, and
    // every "renders `?` when the field is missing" case is INVERTED, because
    // rendering `?` as though it were a fact is the defect.

    /// The real shape: `{success, file_path, provider, format, bytes_written,
    /// voice_compatible}`. The output path is `file_path`, not `path`.
    #[test]
    fn tts_summary_reads_the_real_payload() {
        let f = TtsFormatter;
        let payload = json!({
            "success": true,
            "file_path": "/tmp/output/abc.wav",
            "provider": "elevenlabs",
            "format": "wav",
            "bytes_written": 40960,
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert!(s.contains("abc.wav"), "output path lost: {s}");
        assert!(s.contains("elevenlabs"), "provider lost: {s}");
        assert!(s.contains("40960 bytes"), "size lost: {s}");
    }

    /// INVERTED. The old case was literally named
    /// `tts_summary_missing_path_is_question_mark` and asserted
    /// `"Synthesized 50 chars · openai · → ?"` — an arrow pointing at a file
    /// that does not exist, plus a `chars` count the tool never emits.
    #[test]
    fn tts_never_points_an_arrow_at_a_file_it_cannot_name() {
        let f = TtsFormatter;
        let s = f.summary_line(&json!({ "provider": "openai" }), Duration::from_secs(1));
        assert!(!s.contains('?'), "fabricated an output path: {s}");
        assert!(!s.contains("\u{2192}"), "arrow with no destination: {s}");
        assert!(!s.contains("0 chars"), "fabricated a character count: {s}");
    }
}
