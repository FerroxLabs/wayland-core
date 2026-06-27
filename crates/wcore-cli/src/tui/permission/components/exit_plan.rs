//! The ExitPlanMode permission component (v0.9.2 W4, SPEC §2 #8).
//!
//! Projection for the `ExitPlanMode` tool: a brand-glyph header (`◶`), an
//! `Approve this plan and begin` title, and the captured plan body rendered
//! as markdown (via the shared [`render_markdown`](crate::tui::render::markdown::render_markdown)
//! renderer), clamped to [`PLAN_CLAMP`] lines with a `… (ctrl+f)` tail.
//!
//! CRITICAL (SPEC §2 #8 audit-HIGH — PLAN-BODY CAPTURE): the live `app.plan`
//! is cleared by the same event that creates this card, so the plan text is
//! snapshotted onto [`ToolCardModel::plan_body`] at card-creation time and
//! read from there — NEVER live. The `protocol_bridge` capture that fills
//! `plan_body` is a separate integration step; until it lands, every card
//! arrives with `plan_body: None` and this component renders a dim
//! `(plan body unavailable)` fallback rather than an empty card.
//!
//! The default action is approve-once (`[enter/y]`), and the key row offers a
//! `[n] keep planning` path so the user can bounce back into planning instead
//! of a flat deny.
//!
//! Pure over `PermissionContext` — no I/O, no state — so it is unit-tested
//! purely on its title/body/keys text.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::permission::{ApprovalAction, PermissionComponent, PermissionContext};
use crate::tui::render::markdown::render_markdown;

/// Permission projection for the `ExitPlanMode` tool.
pub struct ExitPlanModeComponent;

/// Max plan-body lines before the `ctrl+f` expand affordance kicks in. A plan
/// is usually a tidy numbered list, but a verbose plan can run long — the
/// clamp keeps the inline card from flooding the transcript.
const PLAN_CLAMP: usize = 14;

impl PermissionComponent for ExitPlanModeComponent {
    fn icon(&self) -> &'static str {
        "◶"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        Line::from(Span::styled(
            "Approve this plan and begin".to_string(),
            Style::default()
                .fg(ctx.theme.text)
                .add_modifier(Modifier::BOLD),
        ))
    }

    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>> {
        // Preferred path: the captured plan body, markdown-rendered so the
        // numbered steps / headers read like the assistant's prose, then
        // clamped to PLAN_CLAMP lines. Read from the card snapshot, NOT the
        // live `app.plan` (SPEC §2 #8 — the plan is cleared on capture).
        if let Some(plan) = &ctx.card.plan_body
            && !plan.trim().is_empty()
        {
            let (lines, _links) = render_markdown(plan, ctx.theme);
            return clamp(lines, ctx);
        }

        // Fallback: capture has not run (or produced an empty body) — a single
        // dim note, never a blank card.
        vec![Line::from(Span::styled(
            "(plan body unavailable)".to_string(),
            Style::default().fg(ctx.theme.text_muted),
        ))]
    }

    fn keys(&self, ctx: &PermissionContext) -> Line<'static> {
        Line::from(Span::styled(
            "[enter/y] approve plan   [n] keep planning   [esc] cancel".to_string(),
            Style::default().fg(ctx.theme.text_muted),
        ))
    }

    fn default_action(&self) -> ApprovalAction {
        ApprovalAction::ApproveOnce
    }
}

/// Clamp the rendered plan to [`PLAN_CLAMP`] lines, appending a muted
/// `… (ctrl+f)` tail when there is more so the full body is one keystroke away.
fn clamp(lines: Vec<Line<'static>>, ctx: &PermissionContext) -> Vec<Line<'static>> {
    if lines.len() <= PLAN_CLAMP {
        return lines;
    }
    let mut out: Vec<Line<'static>> = lines.into_iter().take(PLAN_CLAMP).collect();
    out.push(Line::from(Span::styled(
        "… (ctrl+f)".to_string(),
        Style::default().fg(ctx.theme.text_muted),
    )));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::theme::Theme;

    fn card(plan_body: Option<String>) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "ExitPlanMode".into(),
            summary: String::new(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: "{}".into(),
            approval_reason: String::new(),
            plan_body,
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
    fn icon_is_the_plan_glyph() {
        assert_eq!(ExitPlanModeComponent.icon(), "◶");
    }

    #[test]
    fn title_is_approve_this_plan_and_begin() {
        let t = Theme::hearth();
        let c = card(None);
        let title = line_text(&ExitPlanModeComponent.title(&ctx(&c, &t)));
        assert_eq!(title, "Approve this plan and begin");
    }

    #[test]
    fn body_renders_plan_markdown_when_present() {
        let t = Theme::hearth();
        let c = card(Some("# Plan\n\n1. Read the file\n2. Make the edit".into()));
        let body = ExitPlanModeComponent.body(&ctx(&c, &t));
        assert!(!body.is_empty(), "plan body should render lines");
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            joined.contains("Read the file"),
            "plan text missing: {joined}"
        );
        assert!(
            joined.contains("Make the edit"),
            "plan text missing: {joined}"
        );
    }

    #[test]
    fn body_clamps_a_long_plan_and_appends_the_tail() {
        let t = Theme::hearth();
        let plan: String = (0..40).map(|i| format!("- step {i}\n")).collect();
        let c = card(Some(plan));
        let body = ExitPlanModeComponent.body(&ctx(&c, &t));
        assert_eq!(
            body.len(),
            PLAN_CLAMP + 1,
            "should clamp to {PLAN_CLAMP} + tail"
        );
        let tail = line_text(body.last().unwrap());
        assert!(tail.contains("ctrl+f"), "expand affordance: {tail}");
    }

    #[test]
    fn body_shows_unavailable_note_when_plan_is_none() {
        let t = Theme::hearth();
        let c = card(None);
        let body = ExitPlanModeComponent.body(&ctx(&c, &t));
        assert_eq!(body.len(), 1);
        assert!(
            line_text(&body[0]).contains("plan body unavailable"),
            "expected unavailable note"
        );
    }

    #[test]
    fn body_shows_unavailable_note_for_an_empty_plan() {
        let t = Theme::hearth();
        let c = card(Some("   \n  ".into()));
        let body = ExitPlanModeComponent.body(&ctx(&c, &t));
        assert_eq!(body.len(), 1);
        assert!(
            line_text(&body[0]).contains("plan body unavailable"),
            "whitespace-only plan should fall back"
        );
    }

    #[test]
    fn keys_offer_approve_keep_planning_and_cancel() {
        let t = Theme::hearth();
        let c = card(None);
        let keys = line_text(&ExitPlanModeComponent.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve plan"), "keys: {keys}");
        assert!(
            keys.contains("keep planning"),
            "keep-planning path missing: {keys}"
        );
        assert!(keys.contains("cancel"), "keys: {keys}");
    }

    #[test]
    fn default_action_is_approve_once() {
        assert_eq!(
            ExitPlanModeComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
