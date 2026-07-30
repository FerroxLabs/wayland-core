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
use super::{join_facts, opt_str};
use crate::tui::theme::Theme;

/// The affected/listed entities, under whichever key the payload carries.
/// The HA tool's own mock and its service responses use `affected_entities`;
/// this formatter only ever looked for `entities`.
fn entity_list(v: &Value) -> Option<&Vec<Value>> {
    v.get("affected_entities")
        .or_else(|| v.get("entities"))
        .and_then(Value::as_array)
}

pub struct HomeAssistantFormatter;

impl ToolResultFormatter for HomeAssistantFormatter {
    // UAT-T3. `HomeAssistantTool` wraps every successful response as
    // `{"success": true, "result": <backend payload>}`
    // (`wcore-tools/src/homeassistant_tool.rs::ok_result`), so the fields this
    // formatter reads are one level DOWN, not at the top. Reading the top
    // level found nothing and rendered `Called ?.? on 0 entities` for every
    // call — including `list_entities` and `get_state`, which call no service
    // at all.
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let inner = payload.get("result").unwrap_or(payload);
        if let Some(err) = opt_str(payload, "error") {
            return err.to_string();
        }
        let mut facts: Vec<String> = Vec::new();
        match (opt_str(inner, "domain"), opt_str(inner, "service")) {
            (Some(d), Some(s)) => facts.push(format!("Called {d}.{s}")),
            // HA also returns a bare `service` like "light.turn_on".
            (None, Some(s)) => facts.push(format!("Called {s}")),
            _ => {}
        }
        if let Some(n) = entity_list(inner).map(Vec::len) {
            let unit = if n == 1 { "entity" } else { "entities" };
            facts.push(format!("{n} {unit}"));
        }
        if let Some(state) = opt_str(inner, "state") {
            facts.push(format!("state {state}"));
        }
        join_facts(&facts)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let style = Style::default().fg(theme.text_dim);
        let inner = payload.get("result").unwrap_or(payload);
        let entities = match entity_list(inner) {
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
