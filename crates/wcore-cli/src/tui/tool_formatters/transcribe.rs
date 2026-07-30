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
use super::{join_facts, opt_str, opt_u64};
use crate::tui::theme::Theme;

/// Max lines of transcript shown in the expanded view.
const MAX_TEXT_LINES: usize = 25;

pub struct TranscribeFormatter;

impl ToolResultFormatter for TranscribeFormatter {
    // UAT-T3. `TranscribeAudioTool` returns
    // `{success, transcript, mime, bytes, language, segments: [ … ]}`
    // (`wcore-tools/src/transcription_tools.rs`). Three mismatches, each
    // producing a fabricated value:
    //   * `seconds` does not exist        -> rendered `0s`
    //   * `segments` is an ARRAY, not an  -> `as_u64` failed, rendered `0`
    //     integer                            even though the count was right there
    //   * the text is `transcript`, not   -> detail view always empty
    //     `text`
    // Only `language` was ever read correctly.
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let mut facts = vec!["Transcribed".to_string()];
        if let Some(s) = payload.get("seconds").and_then(Value::as_f64) {
            facts.push(format!("{s:.0}s"));
        }
        // Count the array; fall back to a stated integer if a future payload
        // provides one. Absent entirely -> no clause, not "0 segments".
        let segments = payload
            .get("segments")
            .and_then(Value::as_array)
            .map(|a| a.len() as u64)
            .or_else(|| opt_u64(payload, "segments"));
        if let Some(n) = segments {
            let unit = if n == 1 { "segment" } else { "segments" };
            facts.push(format!("{n} {unit}"));
        }
        if let Some(l) = opt_str(payload, "language") {
            facts.push(l.to_string());
        }
        if let Some(b) = opt_u64(payload, "bytes") {
            facts.push(format!("{b} bytes"));
        }
        join_facts(&facts)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let text = opt_str(payload, "transcript")
            .or_else(|| opt_str(payload, "text"))
            .unwrap_or("");
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
