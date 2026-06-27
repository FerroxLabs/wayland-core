//! The Fallback permission component (v0.9.2 W2 keystone, SPEC §2 #15).
//!
//! Generic projection for any tool without a bespoke component — MCP
//! tools, plugin tools, future tools. Guarantees a clean card, never a
//! raw-JSON wall. THE keystone: the 15-component matrix is tractable
//! precisely because any unknown tool degrades to this.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Generic projection for any tool without a bespoke component.
pub struct FallbackComponent;

/// Max body lines before the `ctrl+f` expand affordance kicks in.
const FALLBACK_CLAMP: usize = 10;

impl PermissionComponent for FallbackComponent {
    fn icon(&self) -> &'static str {
        "⊘"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        Line::from(Span::styled(
            format!("Allow {}", ctx.card.tool_name),
            Style::default()
                .fg(ctx.theme.text)
                .add_modifier(Modifier::BOLD),
        ))
    }

    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        // Pretty-printed args, clamped — NEVER a raw JSON dump. When the user
        // has toggled `ctrl+f` the args render in full (no clamp, no tail).
        let pretty = ctx.card.input_pretty.trim();
        let total = pretty.lines().count();
        for (shown, raw) in pretty.lines().enumerate() {
            if !ctx.expanded && shown >= FALLBACK_CLAMP {
                let remaining = total - FALLBACK_CLAMP;
                lines.push(Line::from(Span::styled(
                    format!("… ({remaining} more lines · ctrl+f to expand)"),
                    Style::default().fg(ctx.theme.text_muted),
                )));
                break;
            }
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default()
                    .fg(ctx.theme.text_dim)
                    .bg(ctx.theme.surface_hover),
            )));
        }
        // The engine's approval reason, when present, sits below the args.
        if !ctx.card.approval_reason.trim().is_empty() {
            lines.push(Line::from(Span::styled(
                ctx.card.approval_reason.clone(),
                Style::default().fg(ctx.theme.text_muted),
            )));
        }
        lines
    }

    fn keys(&self, ctx: &PermissionContext) -> Line<'static> {
        let expand = if ctx.expanded {
            "[ctrl+f] collapse"
        } else {
            "[ctrl+f] expand"
        };
        Line::from(Span::styled(
            format!(
                "[enter/y] approve   [a] always for this tool   [n] deny   [esc] cancel   {expand}"
            ),
            Style::default(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::theme::Theme;

    fn card(tool: &str, pretty: &str) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: tool.into(),
            summary: String::new(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: pretty.into(),
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

    fn expanded_ctx<'a>(c: &'a ToolCardModel, t: &'a Theme) -> PermissionContext<'a> {
        let mut context = ctx(c, t);
        context.expanded = true;
        context
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn fallback_title_names_the_unknown_tool() {
        let t = Theme::hearth();
        let c = card("mcp__foo__bar", "{}");
        let comp = FallbackComponent;
        let title = line_text(&comp.title(&ctx(&c, &t)));
        assert_eq!(title, "Allow mcp__foo__bar");
    }

    #[test]
    fn fallback_clamps_a_huge_arg_blob_and_never_dumps_raw() {
        let t = Theme::hearth();
        let big = (0..50)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let c = card("mcp__foo__bar", &big);
        let comp = FallbackComponent;
        let body = comp.body(&ctx(&c, &t));
        // 10 clamped lines + 1 "… more" line — never the full 50.
        assert_eq!(body.len(), 11);
        let tail = line_text(body.last().unwrap());
        assert!(
            tail.contains("40 more lines"),
            "expected clamp tail: {tail}"
        );
        assert!(
            tail.contains("ctrl+f"),
            "expected expand affordance: {tail}"
        );
    }

    #[test]
    fn fallback_does_not_clamp_when_expanded() {
        let t = Theme::hearth();
        let big = (0..50)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let c = card("mcp__foo__bar", &big);
        let comp = FallbackComponent;
        let body = comp.body(&expanded_ctx(&c, &t));
        // Expanded: all 50 arg lines, no clamp tail.
        assert_eq!(body.len(), 50, "expanded body should show all rows");
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            !joined.contains("more lines"),
            "expanded body must not append a clamp tail: {joined}"
        );
    }

    #[test]
    fn fallback_keys_show_collapse_when_expanded() {
        let t = Theme::hearth();
        let c = card("mcp__foo__bar", "{}");
        let comp = FallbackComponent;
        let keys = line_text(&comp.keys(&expanded_ctx(&c, &t)));
        assert!(keys.contains("ctrl+f"), "keys: {keys}");
        assert!(keys.contains("collapse"), "collapse hint missing: {keys}");
        assert!(!keys.contains("expand"), "should not offer expand: {keys}");
    }

    #[test]
    fn fallback_appends_reason_when_present() {
        let t = Theme::hearth();
        let mut c = card("mcp__foo__bar", "{\"x\":1}");
        c.approval_reason = "needs network egress".into();
        let comp = FallbackComponent;
        let body = comp.body(&ctx(&c, &t));
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            joined.contains("needs network egress"),
            "reason missing: {joined}"
        );
    }

    #[test]
    fn fallback_keys_offer_approve_deny_and_always() {
        let t = Theme::hearth();
        let c = card("mcp__foo__bar", "{}");
        let comp = FallbackComponent;
        let keys = line_text(&comp.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"));
        assert!(keys.contains("always"));
        assert!(keys.contains("deny"));
        assert!(keys.contains("cancel"));
    }
}
