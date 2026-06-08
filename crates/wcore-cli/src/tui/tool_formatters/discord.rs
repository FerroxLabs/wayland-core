//! `discord` (channel send) tool formatter.
//!
//! Expected payload shape:
//! ```json
//! { "channel_name": "general", "chars": 42, "message": "..." }
//! ```

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{str_or, u64_or};
use crate::tui::theme::Theme;

/// Max lines of the posted message echoed in the detail view.
const MAX_MESSAGE_LINES: usize = 10;

pub struct DiscordFormatter;

impl ToolResultFormatter for DiscordFormatter {
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let channel = str_or(payload, "channel_name", "?");
        let chars = u64_or(payload, "chars", 0);
        format!("Posted to #{} · {} chars", channel, chars)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let msg = str_or(payload, "message", "");
        let style = Style::default().fg(theme.text);
        msg.lines()
            .take(MAX_MESSAGE_LINES)
            .map(|s| Line::from(Span::styled(s.to_string(), style)))
            .collect()
    }

    /// v0.9.1.1 B4-hunt: render Discord send-message args as
    /// `#channel · "message excerpt"` instead of raw JSON.
    fn format_args(&self, args: &Value) -> Option<String> {
        let channel = args
            .get("channel_name")
            .or_else(|| args.get("channel"))
            .and_then(Value::as_str)?;
        let message = args
            .get("message")
            .or_else(|| args.get("text"))
            .or_else(|| args.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return Some(format!("#{}", channel));
        }
        let chars: Vec<char> = trimmed.chars().collect();
        let preview: String = chars.iter().take(40).collect();
        let suffix = if chars.len() > 40 { "…" } else { "" };
        Some(format!("#{} · \"{}{}\"", channel, preview, suffix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn discord_summary_format() {
        let f = DiscordFormatter;
        let payload = json!({ "channel_name": "general", "chars": 42 });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "Posted to #general · 42 chars");
    }

    #[test]
    fn discord_summary_missing_channel() {
        let f = DiscordFormatter;
        let payload = json!({ "chars": 10 });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "Posted to #? · 10 chars");
    }
}
