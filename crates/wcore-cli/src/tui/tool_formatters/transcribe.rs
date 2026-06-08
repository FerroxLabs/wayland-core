//! `transcribe` (speech-to-text) tool formatter.
//!
//! Expected payload shape:
//! ```json
//! { "seconds": 12.4, "segments": 8, "language": "en", "text": "..." }
//! ```

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{str_or, u64_or};
use crate::tui::theme::Theme;

/// Max lines of transcript shown in the expanded view.
const MAX_TEXT_LINES: usize = 25;

pub struct TranscribeFormatter;

impl ToolResultFormatter for TranscribeFormatter {
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        // `seconds` may arrive as float or integer — prefer float, fall
        // back to integer, default to 0.
        let seconds = payload
            .get("seconds")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let segments = u64_or(payload, "segments", 0);
        let lang = str_or(payload, "language", "?");
        format!(
            "Transcribed {:.0}s · {} segments · {}",
            seconds, segments, lang
        )
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let text = payload.get("text").and_then(Value::as_str).unwrap_or("");
        let style = Style::default().fg(theme.text);
        text.lines()
            .take(MAX_TEXT_LINES)
            .map(|s| Line::from(Span::styled(s.to_string(), style)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transcribe_summary_format() {
        let f = TranscribeFormatter;
        let payload = json!({
            "seconds": 12.4,
            "segments": 8,
            "language": "en",
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "Transcribed 12s · 8 segments · en");
    }

    #[test]
    fn transcribe_summary_handles_missing_fields() {
        let f = TranscribeFormatter;
        let payload = json!({});
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "Transcribed 0s · 0 segments · ?");
    }
}
