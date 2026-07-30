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

    // UAT-T3: these cases previously asserted payload shapes the tool has
    // never emitted, and in several places asserted the FABRICATED output as
    // though it were the specification. They are not weakened here — the
    // fixtures are replaced with the shapes read out of the tool source, and
    // every "renders `?` when the field is missing" case is INVERTED, because
    // rendering `?` as though it were a fact is the defect.

    /// The real shape: `{success, transcript, mime, bytes, language, segments: [...]}`.
    /// `segments` is an ARRAY — `as_u64` on it returned None, so the old code
    /// rendered `0 segments` while the true count sat right there.
    #[test]
    fn transcribe_summary_reads_the_real_payload() {
        let f = TranscribeFormatter;
        let payload = json!({
            "success": true,
            "transcript": "hello world",
            "mime": "audio/wav",
            "bytes": 4096,
            "language": "en",
            "segments": [{"text": "hello"}, {"text": "world"}],
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert!(s.contains("2 segments"), "segment count lost: {s}");
        assert!(s.contains("en"), "language lost: {s}");
        assert!(!s.contains("0 segments"), "still fabricating zero: {s}");
    }

    #[test]
    fn transcribe_detail_reads_transcript_not_text() {
        let f = TranscribeFormatter;
        let payload = json!({ "transcript": "line one\nline two" });
        let lines = f.detail_lines(&payload, &Theme::hearth());
        assert_eq!(lines.len(), 2, "transcript was never displayed before");
        let l0: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(l0, "line one");
    }

    /// INVERTED. Was `assert_eq!(s, "Transcribed 0s · 0 segments · ?")` —
    /// three fabrications asserted as the specification.
    #[test]
    fn transcribe_never_fabricates_missing_fields() {
        let f = TranscribeFormatter;
        let s = f.summary_line(&json!({}), Duration::from_secs(1));
        assert!(!s.contains('?'), "fabricated a language: {s}");
        assert!(!s.contains("0s"), "fabricated a duration: {s}");
        assert!(!s.contains("0 segments"), "fabricated a segment count: {s}");
    }
}
