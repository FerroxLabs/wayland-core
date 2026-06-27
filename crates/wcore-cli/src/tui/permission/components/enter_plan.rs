//! The EnterPlanMode permission component (v0.9.2 W4, SPEC §2 #7).
//!
//! This is the lightweight *transition* card the agent shows when it asks to
//! enter plan mode — it announces the mode switch, nothing more. The actual
//! plan content lives on the `ExitPlanMode` card, which the agent surfaces
//! once it has a plan to approve. So this component stays minimal:
//!
//!   - icon  → `◷` (a clock face: "pause and think before acting")
//!   - title → `Enter plan mode`
//!   - body  → a dim explanation line, plus an optional plan title when the
//!     tool call already carries one in `summary`/`input_pretty`.
//!
//! Default action is approve-once (the frozen trait default): entering plan
//! mode is harmless, so a bare Enter just lets the agent proceed.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `EnterPlanMode` tool.
pub struct EnterPlanModeComponent;

impl EnterPlanModeComponent {
    /// An optional plan title supplied with the call. `summary` carries the
    /// salient field when present; fall back to the pretty-printed args.
    /// Returns `None` when neither is set so the body stays just the
    /// explanation line.
    fn plan_title(ctx: &PermissionContext) -> Option<String> {
        let summary = ctx.card.summary.trim();
        if !summary.is_empty() {
            return Some(summary.to_string());
        }
        let pretty = ctx.card.input_pretty.trim();
        if !pretty.is_empty() {
            return Some(pretty.to_string());
        }
        None
    }
}

impl PermissionComponent for EnterPlanModeComponent {
    fn icon(&self) -> &'static str {
        "◷"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let _ = ctx;
        Line::from("Enter plan mode")
    }

    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        // The dim explanation line: this is a transition card, so frame what
        // the mode switch means rather than showing any plan content.
        lines.push(Line::from(Span::styled(
            "the agent will plan before acting",
            Style::default().fg(ctx.theme.text_dim),
        )));
        // If the call already names a plan, echo it in muted text so the user
        // sees what is about to be planned. The plan BODY is ExitPlanMode's
        // job — we only surface the title here.
        if let Some(title) = Self::plan_title(ctx) {
            lines.push(Line::from(Span::styled(
                title,
                Style::default().fg(ctx.theme.text_muted),
            )));
        }
        lines
    }

    fn keys(&self, ctx: &PermissionContext) -> Line<'static> {
        let _ = ctx;
        Line::from(Span::styled(
            "[enter/y] approve   [a] always   [n] deny   [esc] cancel",
            Style::default(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::permission::ApprovalAction;
    use crate::tui::theme::Theme;

    fn card(summary: &str) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "EnterPlanMode".into(),
            summary: summary.into(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: String::new(),
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
    fn title_is_the_enter_plan_mode_framing() {
        let t = Theme::hearth();
        let c = card("");
        assert_eq!(
            line_text(&EnterPlanModeComponent.title(&ctx(&c, &t))),
            "Enter plan mode"
        );
    }

    #[test]
    fn icon_is_the_clock_face() {
        assert_eq!(EnterPlanModeComponent.icon(), "◷");
    }

    #[test]
    fn body_leads_with_the_dim_explanation_line() {
        let t = Theme::hearth();
        let c = card("");
        let body = EnterPlanModeComponent.body(&ctx(&c, &t));
        // No plan title supplied → just the explanation line.
        assert_eq!(body.len(), 1);
        assert_eq!(line_text(&body[0]), "the agent will plan before acting");
        // The explanation is dim, not the primary text color.
        assert_eq!(body[0].spans[0].style.fg, Some(t.text_dim));
    }

    #[test]
    fn body_echoes_the_plan_title_from_summary_when_present() {
        let t = Theme::hearth();
        let c = card("Refactor the auth module");
        let body = EnterPlanModeComponent.body(&ctx(&c, &t));
        // Explanation line + plan title.
        assert_eq!(body.len(), 2);
        assert_eq!(line_text(&body[0]), "the agent will plan before acting");
        assert_eq!(line_text(&body[1]), "Refactor the auth module");
        // The title is muted, below the dim explanation.
        assert_eq!(body[1].spans[0].style.fg, Some(t.text_muted));
    }

    #[test]
    fn body_plan_title_falls_back_to_input_pretty() {
        let t = Theme::hearth();
        let mut c = card("");
        c.input_pretty = "Plan the migration".into();
        let body = EnterPlanModeComponent.body(&ctx(&c, &t));
        assert_eq!(body.len(), 2);
        assert_eq!(line_text(&body[1]), "Plan the migration");
    }

    #[test]
    fn keys_offer_approve_always_deny_and_cancel() {
        let t = Theme::hearth();
        let c = card("");
        let keys = line_text(&EnterPlanModeComponent.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"));
        assert!(keys.contains("always"));
        assert!(keys.contains("deny"));
        assert!(keys.contains("cancel"));
    }

    #[test]
    fn default_action_is_approve_once() {
        assert_eq!(
            EnterPlanModeComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
