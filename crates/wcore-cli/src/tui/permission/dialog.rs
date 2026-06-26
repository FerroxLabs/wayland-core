//! The shared PermissionDialog chrome (v0.9.2 W2, SPEC §1C).
//!
//! Reimplements CC's top-edge-only border in ratatui: a single leading
//! horizontal rule in `theme.orange` (brand discipline — CC uses
//! permission-blue, Wayland uses the brand accent orange), `paddingX=1`
//! (one-space indent), `marginTop=1` (one blank line above). Composes the
//! routed component's `icon` + `title` + `body` + a blank + `keys` into a
//! `Vec<Line<'static>>`. REPLACES the body of
//! `widgets::approval_inline::render_approval_inline`.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::{PermissionContext, permission_component_for};
use crate::tui::app::ToolCardModel;

/// Render one pending tool call into the single inline approval card.
/// Top-edge-only orange rule + one-space indent + one blank line above.
pub fn render(card: &ToolCardModel, ctx: &PermissionContext) -> Vec<Line<'static>> {
    let component = permission_component_for(&card.tool_name);
    let mut out: Vec<Line<'static>> = Vec::new();

    // marginTop = 1: one blank line separates the card from prior content.
    out.push(Line::from(""));

    // Top-edge-only rule, full width, in the brand accent. The dialog has
    // NO bottom rule — the key row is the visual close.
    let rule = "─".repeat(ctx.width.max(1) as usize);
    out.push(Line::from(Span::styled(
        rule,
        Style::default().fg(ctx.theme.orange),
    )));

    // Header: ` <icon> <title>` (paddingX = 1).
    let mut header = vec![Span::raw(format!(" {} ", component.icon()))];
    header.extend(component.title(ctx).spans);
    out.push(Line::from(header));

    // Body, each line indented one space (paddingX = 1).
    for line in component.body(ctx) {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }

    // Blank then the key row.
    out.push(Line::from(""));
    let mut keys = vec![Span::raw(" ")];
    keys.extend(component.keys(ctx).spans);
    out.push(Line::from(keys));

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::theme::Theme;

    fn unknown_card() -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "mcp__x__y".into(),
            summary: String::new(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: "{}".into(),
            approval_reason: String::new(),
            plan_body: None,
            crucible_plan: None,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn chrome_has_top_rule_and_no_bottom_rule() {
        let t = Theme::hearth();
        let card = unknown_card();
        let ctx = PermissionContext {
            card: &card,
            theme: &t,
            width: 20,
            always_allow_available: true,
            editable_prefix: None,
            selected_choice: 0,
            expanded: false,
        };
        let lines = render(&card, &ctx);
        // Line 0 is the marginTop blank; line 1 is the top rule of box-
        // drawing chars; the last line is the key row, NOT a rule.
        let rule = line_text(&lines[1]);
        assert!(
            !rule.is_empty() && rule.chars().all(|c| c == '─'),
            "line 1 must be a full box-drawing rule, got {rule:?}"
        );
        let last = line_text(lines.last().unwrap());
        assert!(
            !last.chars().all(|c| c == '─'),
            "last line must be the key row, not a bottom rule: {last:?}"
        );
    }

    #[test]
    fn chrome_rule_is_brand_orange() {
        let t = Theme::hearth();
        let card = unknown_card();
        let ctx = PermissionContext {
            card: &card,
            theme: &t,
            width: 24,
            always_allow_available: true,
            editable_prefix: None,
            selected_choice: 0,
            expanded: false,
        };
        let lines = render(&card, &ctx);
        let rule_span = &lines[1].spans[0];
        assert_eq!(
            rule_span.style.fg,
            Some(t.orange),
            "the top rule must be theme.orange (brand discipline)"
        );
        // Rule spans the full width.
        assert_eq!(rule_span.content.chars().count(), 24);
    }

    #[test]
    fn chrome_renders_fallback_title_for_unknown_tool() {
        let t = Theme::hearth();
        let card = unknown_card();
        let ctx = PermissionContext {
            card: &card,
            theme: &t,
            width: 40,
            always_allow_available: true,
            editable_prefix: None,
            selected_choice: 0,
            expanded: false,
        };
        let joined = render(&card, &ctx)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("Allow mcp__x__y"),
            "fallback title must appear in the chrome: {joined}"
        );
    }
}
