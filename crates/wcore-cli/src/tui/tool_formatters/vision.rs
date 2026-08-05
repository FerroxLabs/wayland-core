//! `vision` (image analysis) tool formatter.
//!
//! Expected payload shape:
//! ```json
//! { "width": 1024, "height": 768, "provider": "anthropic",
//!   "analysis": "long form description..." }
//! ```

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{fmt_duration, join_facts, opt_str, opt_u64};
use crate::tui::theme::Theme;

/// Max lines of the analysis text shown in the expanded view.
const MAX_ANALYSIS_LINES: usize = 25;

pub struct VisionFormatter;

impl ToolResultFormatter for VisionFormatter {
    // UAT-T3. `VisionAnalyzeTool` returns `{success, analysis, mime, bytes}`
    // (`wcore-tools/src/vision_tools.rs`) — there is no `width`, `height` or
    // `provider`, so this rendered `Analyzed image 0x0 · ? · 0.0s` on every
    // successful call. Only `analysis` was ever real. Report what exists.
    fn summary_line(&self, payload: &Value, duration: Duration) -> String {
        let mut facts = vec!["Analyzed image".to_string()];
        if let (Some(w), Some(h)) = (opt_u64(payload, "width"), opt_u64(payload, "height")) {
            facts.push(format!("{w}x{h}"));
        }
        if let Some(m) = opt_str(payload, "mime") {
            facts.push(m.to_string());
        }
        if let Some(b) = opt_u64(payload, "bytes") {
            facts.push(format!("{b} bytes"));
        }
        if let Some(p) = opt_str(payload, "provider") {
            facts.push(p.to_string());
        }
        if !duration.is_zero() {
            facts.push(fmt_duration(duration));
        }
        join_facts(&facts)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let text = payload
            .get("analysis")
            .and_then(Value::as_str)
            .unwrap_or("");
        let style = Style::default().fg(theme.text);
        text.lines()
            .take(MAX_ANALYSIS_LINES)
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

    /// The real shape: `{success, analysis, mime, bytes}` — there is no
    /// `width`, `height` or `provider`, so the old summary rendered
    /// `Analyzed image 0x0 · ? · 0.0s` on every successful call.
    #[test]
    fn vision_summary_reads_the_real_payload() {
        let f = VisionFormatter;
        let payload = json!({
            "success": true,
            "analysis": "A scenic view.",
            "mime": "image/png",
            "bytes": 20480,
        });
        let s = f.summary_line(&payload, Duration::from_secs_f64(1.4));
        assert!(s.contains("image/png"), "mime lost: {s}");
        assert!(s.contains("20480 bytes"), "size lost: {s}");
        assert!(!s.contains("0x0"), "fabricated dimensions: {s}");
        assert!(!s.contains('?'), "fabricated a provider: {s}");
    }

    #[test]
    fn vision_detail_includes_analysis() {
        let f = VisionFormatter;
        let payload = json!({ "analysis": "Line one\nLine two" });
        let lines = f.detail_lines(&payload, &Theme::hearth());
        assert_eq!(lines.len(), 2);
        let l0: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(l0, "Line one");
    }

    #[test]
    fn vision_never_renders_an_unmeasured_duration() {
        let f = VisionFormatter;
        let s = f.summary_line(&json!({ "analysis": "x" }), Duration::ZERO);
        assert!(!s.contains("0.0s"), "rendered a placeholder duration: {s}");
    }
}
