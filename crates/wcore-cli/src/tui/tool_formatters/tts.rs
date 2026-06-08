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
use super::{str_or, u64_or};
use crate::tui::theme::Theme;

pub struct TtsFormatter;

impl ToolResultFormatter for TtsFormatter {
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let chars = u64_or(payload, "chars", 0);
        let provider = str_or(payload, "provider", "?");
        let path = str_or(payload, "path", "");
        let basename = basename(path);
        format!(
            "Synthesized {} chars · {} · → {}",
            chars, provider, basename
        )
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let path = str_or(payload, "path", "");
        if path.is_empty() {
            return Vec::new();
        }
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

    #[test]
    fn tts_summary_format() {
        let f = TtsFormatter;
        let payload = json!({
            "chars": 320,
            "provider": "elevenlabs",
            "path": "/tmp/output/abc.wav",
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "Synthesized 320 chars · elevenlabs · → abc.wav");
    }

    #[test]
    fn tts_summary_missing_path_is_question_mark() {
        let f = TtsFormatter;
        let payload = json!({ "chars": 50, "provider": "openai" });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "Synthesized 50 chars · openai · → ?");
    }
}
