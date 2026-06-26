//! Inline approval widget — the single-surface permission card.
//!
//! v0.9.2 W2 (SPEC §0 #1, §1C): the single-surface criterion. There is
//! exactly ONE approval surface — the inline card rendered into the
//! transcript by the permission dispatcher. The v0.9.1.2 stacked surfaces
//! (the sticky strip `render_approval_strip` + the batch builder
//! `render_approval_inline_batch` + the status-widget label) are DELETED.
//! Visibility — the strip's only job — is now done by per-card
//! scroll-to-pending (`workspace.rs`, re-arming
//! `App::force_scroll_to_pending_approval` on each new head-of-queue card).
//!
//! `render_approval_inline` is now a thin shim that delegates to
//! `permission::render`, which routes the tool to its bespoke
//! `PermissionComponent` (or `FallbackComponent`) and composes the shared
//! `PermissionDialog` chrome (top-edge-only orange rule, paddingX=1,
//! marginTop=1). The chrome owns the visual contract; this shim only adapts
//! the legacy `(card, theme, width)` call shape used by the workspace
//! transcript render path.
//!
//! The widget stays a pure `Vec<Line>` builder, not a `render` fn painting
//! into a `Rect`: the transcript `render_turns` path owns layout and
//! decides where these lines sit in the scrollback, so the approval card
//! composes with the rest of the transcript pipeline (markdown, sources,
//! tool cards) and is testable without a `TestBackend`.

use ratatui::text::Line;

use crate::tui::app::{ToolCardModel, ToolCardStatus};
use crate::tui::theme::Theme;

/// Render the inline approval card for `card`. Returns the lines the
/// transcript should emit. An empty `Vec` is returned for cards not in
/// [`ToolCardStatus::AwaitingApproval`] so callers can pass any card
/// unconditionally — the widget itself decides whether to render.
///
/// v0.9.2 W2: single-surface — delegates to the permission dispatcher via
/// [`crate::tui::permission::render`]. The sticky strip + batch builder are
/// deleted; queue visibility is done by per-card scroll-to-pending in
/// `workspace.rs`. `width` is the transcript content width (drives the
/// chrome's full-width top rule and the body clamp budget).
///
/// The lines use `'static` lifetime spans (owned `String` or `&'static
/// str`) so the caller can stash them in a long-lived buffer.
pub fn render_approval_inline(
    card: &ToolCardModel,
    theme: &Theme,
    width: u16,
) -> Vec<Line<'static>> {
    if card.status != ToolCardStatus::AwaitingApproval {
        return Vec::new();
    }
    let ctx = crate::tui::permission::PermissionContext {
        card,
        theme,
        width,
        always_allow_available: true,
        editable_prefix: None,
        // Static shim (test + non-interactive callers): the live arrow-nav
        // index is threaded directly at the workspace render site instead.
        selected_choice: 0,
        expanded: false,
    };
    crate::tui::permission::render(card, &ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::theme::Theme;

    fn awaiting_card(tool: &str, summary: &str, reason: &str) -> ToolCardModel {
        ToolCardModel {
            call_id: "call-1".to_string(),
            tool_name: tool.to_string(),
            summary: summary.to_string(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: String::new(),
            approval_reason: reason.to_string(),
            plan_body: None,
            crucible_plan: None,
        }
    }

    /// Flatten a `Line` to plain text for substring assertions.
    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn inline_approval_returns_empty_for_non_awaiting_cards() {
        let mut card = awaiting_card("Edit", "src/main.rs", "writes a file");
        card.status = ToolCardStatus::Running;
        let theme = Theme::hearth();
        let lines = render_approval_inline(&card, &theme, 80);
        assert!(
            lines.is_empty(),
            "non-awaiting card must not render an approval prompt"
        );
    }

    #[test]
    fn inline_approval_delegates_to_permission_chrome() {
        // v0.9.2 W2: the shim must emit the permission chrome — a leading
        // full-width orange top-rule of box-drawing chars (line index 1,
        // after the marginTop blank). This is the single-surface proof:
        // no bespoke 6-line ⊘/│ card, just the dispatched dialog.
        let card = awaiting_card("mcp__discord__send", "send · #general", "writes to Discord");
        let theme = Theme::hearth();
        let lines = render_approval_inline(&card, &theme, 40);
        let rule = line_text(&lines[1]);
        assert!(
            !rule.is_empty() && rule.chars().all(|c| c == '─'),
            "expected the orange top-rule at line 1, got {rule:?}"
        );
        assert_eq!(
            lines[1].spans[0].style.fg,
            Some(theme.orange),
            "the top rule must be theme.orange"
        );
    }

    #[test]
    fn inline_approval_unknown_tool_uses_fallback_title() {
        let card = awaiting_card("mcp__foo__bar", "", "");
        let theme = Theme::hearth();
        let joined: String = render_approval_inline(&card, &theme, 60)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("Allow mcp__foo__bar"),
            "fallback title missing from shim output: {joined}"
        );
    }
}
