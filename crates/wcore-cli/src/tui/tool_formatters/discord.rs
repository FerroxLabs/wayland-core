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
use super::{join_facts, opt_str, opt_u64};
use crate::tui::theme::Theme;

/// Max lines of the posted message echoed in the detail view.
const MAX_MESSAGE_LINES: usize = 10;

pub struct DiscordFormatter;

impl ToolResultFormatter for DiscordFormatter {
    // UAT-T3. `DiscordServerTool` is a PASSTHROUGH of the Discord API
    // response (`wcore-tools/src/discord_tool.rs`:
    // `DiscordOutcome::Ok { payload } -> content: payload.to_string()`).
    // Measured: `channel_name` and `chars` appear ZERO times in that file
    // (control: `channel_id` appears 20), so this rendered
    // `Posted to #? · 0 chars` for every call regardless of outcome — and it
    // said "Posted" even for read-only actions like listing channels.
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        if let Some(err) = opt_str(payload, "error") {
            return err.to_string();
        }
        let mut facts: Vec<String> = Vec::new();
        // Discord returns the channel id, and only sometimes a name.
        if let Some(name) = opt_str(payload, "name") {
            facts.push(format!("#{name}"));
        } else if let Some(id) = opt_str(payload, "channel_id") {
            facts.push(format!("channel {id}"));
        }
        // The posted body comes back as `content` on a message object.
        if let Some(body) = opt_str(payload, "content").or_else(|| opt_str(payload, "message")) {
            facts.push(format!("{} chars", body.chars().count()));
        } else if let Some(n) = opt_u64(payload, "chars") {
            facts.push(format!("{n} chars"));
        }
        if facts.is_empty()
            && let Some(a) = payload.as_array()
        {
            let unit = if a.len() == 1 { "item" } else { "items" };
            facts.push(format!("{} {unit}", a.len()));
        }
        join_facts(&facts)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let msg = opt_str(payload, "content")
            .or_else(|| opt_str(payload, "message"))
            .unwrap_or("");
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

    // UAT-T3: these cases previously asserted payload shapes the tool has
    // never emitted, and in several places asserted the FABRICATED output as
    // though it were the specification. They are not weakened here — the
    // fixtures are replaced with the shapes read out of the tool source, and
    // every "renders `?` when the field is missing" case is INVERTED, because
    // rendering `?` as though it were a fact is the defect.

    /// The real shape: `DiscordServerTool` passes the Discord API response
    /// through verbatim (`discord_tool.rs`), so a posted message comes back
    /// as a message object with `channel_id` and `content`.
    #[test]
    fn discord_summary_reads_the_real_api_response() {
        let f = DiscordFormatter;
        let payload = json!({
            "id": "1234567890",
            "channel_id": "9876543210",
            "content": "hello there",
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert!(s.contains("9876543210"), "channel lost: {s}");
        assert!(s.contains("11 chars"), "wrong length: {s}");
    }

    #[test]
    fn discord_surfaces_an_api_error_instead_of_claiming_a_post() {
        let f = DiscordFormatter;
        let s = f.summary_line(
            &json!({ "error": "Missing Permissions" }),
            Duration::from_secs(1),
        );
        assert_eq!(s, "Missing Permissions");
        assert!(
            !s.contains("Posted"),
            "claimed a post that never happened: {s}"
        );
    }

    /// INVERTED. Was `assert_eq!(s, "Posted to #? · 10 chars")`.
    #[test]
    fn discord_never_renders_a_question_mark_channel() {
        let f = DiscordFormatter;
        let s = f.summary_line(&json!({}), Duration::from_secs(1));
        assert!(!s.contains('?'), "fabricated a channel: {s}");
        assert!(!s.contains("Posted"), "claimed a post with no payload: {s}");
    }
}
