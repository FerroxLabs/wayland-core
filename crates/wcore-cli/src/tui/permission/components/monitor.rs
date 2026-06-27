//! The Monitor permission component (v0.9.2 W4, SPEC §2 #12,
//! feature-gated `monitor`).
//!
//! Projection for the `Monitor` tool: a filled-circle header (`◉`), a
//! `Monitor {target}` title naming the watched target, and a body that
//! shows the target plus the polling cadence.
//!
//! The whole module — including these tests — only compiles under
//! `#[cfg(feature = "monitor")]` (the gate lives on the `pub mod monitor;`
//! line in `components/mod.rs`), so the default build never pays for it and
//! the dispatcher falls to `Fallback` when the feature is off.
//!
//! Data comes from the card: `target` and `cadence` are pulled from the
//! pretty-printed args JSON when present, with the card `summary` as the
//! target fallback so the card is never blank. Pure over
//! `PermissionContext` — no I/O, no state — so it is unit-tested purely on
//! its title/body/keys text.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `Monitor` tool (feature `monitor`).
pub struct MonitorComponent;

impl MonitorComponent {
    /// The monitored target: the `target` field from the pretty-printed args
    /// when present, else the card `summary` (which previews the args). Empty
    /// when neither yields a target (the title degrades to `Monitor`).
    fn target(ctx: &PermissionContext) -> String {
        if let Some(target) = arg_str(&ctx.card.input_pretty, "target")
            && !target.trim().is_empty()
        {
            return target.trim().to_string();
        }
        ctx.card.summary.trim().to_string()
    }

    /// The polling cadence from the args `cadence` field. `None` when absent
    /// or empty.
    fn cadence(ctx: &PermissionContext) -> Option<String> {
        let cadence = arg_str(&ctx.card.input_pretty, "cadence")?;
        let cadence = cadence.trim();
        if cadence.is_empty() {
            None
        } else {
            Some(cadence.to_string())
        }
    }
}

impl PermissionComponent for MonitorComponent {
    fn icon(&self) -> &'static str {
        "◉"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let target = Self::target(ctx);
        let text = if target.is_empty() {
            "Monitor".to_string()
        } else {
            format!("Monitor {target}")
        };
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(ctx.theme.text)
                .add_modifier(Modifier::BOLD),
        ))
    }

    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        // The target in primary text — what is being watched.
        let target = Self::target(ctx);
        if !target.is_empty() {
            lines.push(Line::from(Span::styled(
                target,
                Style::default().fg(ctx.theme.text),
            )));
        }

        // The cadence as a dim note (`every 30s`, `on change`, etc.).
        if let Some(cadence) = Self::cadence(ctx) {
            lines.push(Line::from(Span::styled(
                cadence,
                Style::default().fg(ctx.theme.text_muted),
            )));
        }

        lines
    }

    fn keys(&self, ctx: &PermissionContext) -> Line<'static> {
        let _ = ctx;
        Line::from(Span::styled(
            "[enter/y] approve   [a] always for this tool   [n] deny   [esc] cancel",
            Style::default(),
        ))
    }
}

/// Pull a top-level string field out of the pretty-printed args JSON.
/// Returns `None` when the args are not the expected JSON shape.
fn arg_str(input_pretty: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(input_pretty)
        .ok()?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::permission::ApprovalAction;
    use crate::tui::theme::Theme;

    fn card(input_pretty: &str, summary: &str) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "Monitor".into(),
            summary: summary.into(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: input_pretty.into(),
            approval_reason: String::new(),
            plan_body: None,
            crucible_plan: None,
        }
    }

    fn ctx<'a>(c: &'a ToolCardModel, t: &'a Theme) -> PermissionContext<'a> {
        PermissionContext {
            card: c,
            theme: t,
            width: 80,
            always_allow_available: true,
            editable_prefix: None,
            selected_choice: 0,
            expanded: false,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn icon_is_the_filled_circle() {
        assert_eq!(MonitorComponent.icon(), "◉");
    }

    #[test]
    fn title_carries_the_target_from_args() {
        let t = Theme::hearth();
        let c = card(r#"{"target":"build.log"}"#, "");
        let comp = MonitorComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Monitor build.log");
    }

    #[test]
    fn title_falls_back_to_summary_when_no_target_field() {
        let t = Theme::hearth();
        let c = card("not json", "deploy pipeline");
        let comp = MonitorComponent;
        assert_eq!(
            line_text(&comp.title(&ctx(&c, &t))),
            "Monitor deploy pipeline"
        );
    }

    #[test]
    fn title_degrades_gracefully_when_empty() {
        let t = Theme::hearth();
        let c = card("not json", "");
        let comp = MonitorComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Monitor");
    }

    #[test]
    fn body_shows_target_and_cadence() {
        let t = Theme::hearth();
        let c = card(r#"{"target":"build.log","cadence":"every 30s"}"#, "");
        let comp = MonitorComponent;
        let body = comp.body(&ctx(&c, &t));
        assert_eq!(body.len(), 2);
        assert_eq!(line_text(&body[0]), "build.log");
        assert_eq!(line_text(&body[1]), "every 30s");
        // The cadence is a muted/dim note, not primary text.
        assert_eq!(body[1].spans[0].style.fg, Some(t.text_muted));
    }

    #[test]
    fn body_omits_a_missing_cadence() {
        let t = Theme::hearth();
        let c = card(r#"{"target":"pid 4242"}"#, "");
        let comp = MonitorComponent;
        let body = comp.body(&ctx(&c, &t));
        assert_eq!(body.len(), 1);
        assert_eq!(line_text(&body[0]), "pid 4242");
    }

    #[test]
    fn keys_offer_approve_always_deny_and_cancel() {
        let t = Theme::hearth();
        let c = card(r#"{"target":"x"}"#, "");
        let comp = MonitorComponent;
        let keys = line_text(&comp.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"));
        assert!(keys.contains("always"));
        assert!(keys.contains("deny"));
        assert!(keys.contains("cancel"));
    }

    #[test]
    fn default_action_is_approve_once() {
        assert_eq!(
            MonitorComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
