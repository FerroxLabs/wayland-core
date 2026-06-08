//! `image_gen` (image generation) tool formatter.
//!
//! Expected payload shape:
//! ```json
//! { "provider": "openai", "width": 1024, "height": 1024, "url": "..." }
//! ```
//! `url` may be a remote URL or a `data:` URI (inline base64). We
//! truncate `data:` URIs in `detail_lines` for readability, and we
//! never feed them to the Sources block (Sources is for live links).

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{fmt_duration, str_or, u64_or};
use crate::tui::theme::Theme;

/// Max chars of a `data:` URI shown in `detail_lines` before truncating.
const DATA_URI_PREVIEW: usize = 80;

pub struct ImageGenFormatter;

impl ToolResultFormatter for ImageGenFormatter {
    fn summary_line(&self, payload: &Value, duration: Duration) -> String {
        let provider = str_or(payload, "provider", "?");
        let w = u64_or(payload, "width", 0);
        let h = u64_or(payload, "height", 0);
        format!(
            "Generated image · {} · {}x{} · {}",
            provider,
            w,
            h,
            fmt_duration(duration)
        )
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let url = str_or(payload, "url", "");
        if url.is_empty() {
            return Vec::new();
        }
        let style = Style::default().fg(theme.text_dim);
        let display = if url.starts_with("data:") && url.chars().count() > DATA_URI_PREVIEW {
            let preview: String = url.chars().take(DATA_URI_PREVIEW).collect();
            format!("{}...", preview)
        } else {
            url.to_string()
        };
        vec![Line::from(Span::styled(display, style))]
    }

    fn extract_urls(&self, payload: &Value) -> Vec<String> {
        match payload.get("url").and_then(Value::as_str) {
            Some(u) if !u.is_empty() && !u.starts_with("data:") => vec![u.to_string()],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn image_gen_summary_format() {
        let f = ImageGenFormatter;
        let payload = json!({
            "provider": "openai",
            "width": 1024,
            "height": 1024,
            "url": "https://images.example.com/abc.png",
        });
        let s = f.summary_line(&payload, Duration::from_secs_f64(3.2));
        assert_eq!(s, "Generated image · openai · 1024x1024 · 3.2s");
    }

    #[test]
    fn image_gen_extract_urls_skips_data_uri() {
        let f = ImageGenFormatter;
        let payload = json!({ "url": "data:image/png;base64,iVBORw0KGgo..." });
        assert!(f.extract_urls(&payload).is_empty());
    }

    #[test]
    fn image_gen_extract_urls_returns_http_url() {
        let f = ImageGenFormatter;
        let payload = json!({ "url": "https://img.example.com/a.png" });
        assert_eq!(
            f.extract_urls(&payload),
            vec!["https://img.example.com/a.png".to_string()]
        );
    }

    #[test]
    fn image_gen_detail_truncates_data_uri() {
        let f = ImageGenFormatter;
        let long_data = format!("data:image/png;base64,{}", "A".repeat(500));
        let payload = json!({ "url": long_data });
        let theme = Theme::hearth();
        let lines = f.detail_lines(&payload, &theme);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with("..."));
        // 80 chars preview + "..." = 83 chars total.
        assert_eq!(text.chars().count(), DATA_URI_PREVIEW + 3);
    }
}
