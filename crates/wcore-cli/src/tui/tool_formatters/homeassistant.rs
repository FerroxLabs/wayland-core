//! `homeassistant` (HA service call) tool formatter.
//!
//! Expected payload shape:
//! ```json
//! { "domain": "light", "service": "turn_on",
//!   "entities": ["light.kitchen", "light.den"] }
//! ```
//! `entities` may be missing on a service-level call (no entity_id);
//! we report 0 entities in that case.

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::str_or;
use crate::tui::theme::Theme;

pub struct HomeAssistantFormatter;

impl ToolResultFormatter for HomeAssistantFormatter {
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let domain = str_or(payload, "domain", "?");
        let service = str_or(payload, "service", "?");
        let n = payload
            .get("entities")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        format!("Called {}.{} on {} entities", domain, service, n)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let style = Style::default().fg(theme.text_dim);
        let entities = match payload.get("entities").and_then(Value::as_array) {
            Some(e) => e,
            None => return Vec::new(),
        };
        entities
            .iter()
            .filter_map(Value::as_str)
            .map(|s| Line::from(Span::styled(s.to_string(), style)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ha_summary_format() {
        let f = HomeAssistantFormatter;
        let payload = json!({
            "domain": "light",
            "service": "turn_on",
            "entities": ["light.kitchen", "light.den"],
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "Called light.turn_on on 2 entities");
    }

    #[test]
    fn ha_summary_missing_entities() {
        let f = HomeAssistantFormatter;
        let payload = json!({ "domain": "automation", "service": "reload" });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "Called automation.reload on 0 entities");
    }
}
