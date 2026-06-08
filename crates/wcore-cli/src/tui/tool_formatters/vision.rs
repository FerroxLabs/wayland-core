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
use super::{fmt_duration, str_or, u64_or};
use crate::tui::theme::Theme;

/// Max lines of the analysis text shown in the expanded view.
const MAX_ANALYSIS_LINES: usize = 25;

pub struct VisionFormatter;

impl ToolResultFormatter for VisionFormatter {
    fn summary_line(&self, payload: &Value, duration: Duration) -> String {
        let w = u64_or(payload, "width", 0);
        let h = u64_or(payload, "height", 0);
        let provider = str_or(payload, "provider", "?");
        format!(
            "Analyzed image {}x{} · {} · {}",
            w,
            h,
            provider,
            fmt_duration(duration)
        )
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

    #[test]
    fn vision_summary_format() {
        let f = VisionFormatter;
        let payload = json!({
            "width": 1024,
            "height": 768,
            "provider": "anthropic",
            "analysis": "A scenic view.",
        });
        let s = f.summary_line(&payload, Duration::from_secs_f64(1.4));
        assert_eq!(s, "Analyzed image 1024x768 · anthropic · 1.4s");
    }

    #[test]
    fn vision_detail_includes_analysis() {
        let f = VisionFormatter;
        let payload = json!({
            "width": 100,
            "height": 100,
            "provider": "openai",
            "analysis": "Line one\nLine two",
        });
        let theme = Theme::hearth();
        let lines = f.detail_lines(&payload, &theme);
        assert_eq!(lines.len(), 2);
        let l0: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(l0, "Line one");
    }
}
