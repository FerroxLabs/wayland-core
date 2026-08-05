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
use super::{fmt_duration, join_facts, opt_str, opt_u64};
use crate::tui::theme::Theme;

/// Max chars of a `data:` URI shown in `detail_lines` before truncating.
const DATA_URI_PREVIEW: usize = 80;

/// The generated image reference, under whichever key the payload carries.
/// The tool emits `image`; this formatter historically read only `url`.
fn image_ref(payload: &Value) -> Option<&str> {
    opt_str(payload, "image").or_else(|| opt_str(payload, "url"))
}

pub struct ImageGenFormatter;

impl ToolResultFormatter for ImageGenFormatter {
    // UAT-T3. `ImageGenerationTool` returns
    // `{success, image, freeFallbackUsed, usedProvider, width, height, accounting}`
    // (`wcore-tools/src/image_generation_tool.rs`). `width`/`height` matched;
    // the provider is `usedProvider` (so `?` was printed for every call) and
    // the image is `image`, not `url` — which also meant `extract_urls`
    // returned nothing and the Sources block never carried the image.
    fn summary_line(&self, payload: &Value, duration: Duration) -> String {
        let mut facts = vec!["Generated image".to_string()];
        if let Some(p) = opt_str(payload, "usedProvider").or_else(|| opt_str(payload, "provider")) {
            facts.push(p.to_string());
        }
        if let (Some(w), Some(h)) = (opt_u64(payload, "width"), opt_u64(payload, "height")) {
            facts.push(format!("{w}x{h}"));
        }
        if payload
            .get("freeFallbackUsed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            facts.push("free fallback".to_string());
        }
        if !duration.is_zero() {
            facts.push(fmt_duration(duration));
        }
        join_facts(&facts)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let Some(url) = image_ref(payload) else {
            return Vec::new();
        };
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
        match image_ref(payload) {
            Some(u) if !u.starts_with("data:") => vec![u.to_string()],
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
