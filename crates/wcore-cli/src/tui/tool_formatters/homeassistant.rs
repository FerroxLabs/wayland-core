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

    // UAT-T3: these cases previously asserted payload shapes the tool has
    // never emitted, and in several places asserted the FABRICATED output as
    // though it were the specification. They are not weakened here — the
    // fixtures are replaced with the shapes read out of the tool source, and
    // every "renders `?` when the field is missing" case is INVERTED, because
    // rendering `?` as though it were a fact is the defect.

    /// The real shape: every success is wrapped as
    /// `{"success": true, "result": <payload>}` by `ok_result`.
    #[test]
    fn ha_summary_reads_the_wrapped_result() {
        let f = HomeAssistantFormatter;
        let payload = json!({
            "success": true,
            "result": {
                "service": "light.turn_on",
                "affected_entities": ["light.kitchen", "light.den"],
            }
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert!(s.contains("light.turn_on"), "service lost: {s}");
        assert!(s.contains("2 entities"), "entity count lost: {s}");
    }

    #[test]
    fn ha_detail_lines_read_the_wrapped_entities() {
        let f = HomeAssistantFormatter;
        let payload = json!({
            "success": true,
            "result": { "affected_entities": ["light.kitchen"] }
        });
        let lines = f.detail_lines(&payload, &Theme::hearth());
        assert_eq!(lines.len(), 1);
        let l0: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(l0, "light.kitchen");
    }

    /// INVERTED. Was `assert_eq!(s, "Called automation.reload on 0 entities")`
    /// for a payload with no entities key — a `list_entities` call renders
    /// through here too, and it calls no service at all.
    #[test]
    fn ha_never_claims_a_service_call_it_cannot_see() {
        let f = HomeAssistantFormatter;
        let s = f.summary_line(
            &json!({ "success": true, "result": {} }),
            Duration::from_secs(1),
        );
        assert!(!s.contains('?'), "fabricated a domain/service: {s}");
        assert!(!s.contains("Called"), "claimed a service call: {s}");
        assert!(!s.contains("0 entities"), "fabricated an entity count: {s}");
    }
}
