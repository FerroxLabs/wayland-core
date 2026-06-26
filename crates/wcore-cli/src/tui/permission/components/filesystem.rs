//! The Filesystem permission component (v0.9.2 W3, SPEC §2 #4).
//!
//! ONE shared component for the three read-access tools — `Read`, `Glob`,
//! `Grep` — routed here by the dispatcher. The icon is the same magnifier
//! (`🔎`) for all three; only the title branches on `ctx.card.tool_name`:
//!
//!   - `Read`  → `Read {basename}`            (the file being read)
//!   - `Glob`  → `Search files matching {pattern}`
//!   - `Grep`  → `Grep for {pattern}`
//!
//! The body is the path/pattern rendered in the surface-hover "code" style
//! shared with the Fallback card. `Read` is the low-risk case, so it also
//! carries a dim `read-only` note under its path.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `Read` / `Glob` / `Grep` tools.
pub struct FilesystemComponent;

impl FilesystemComponent {
    /// The path/pattern the tool acts on. The card's `summary` already
    /// carries the salient field per tool (file_path for `Read`, pattern
    /// for `Glob`/`Grep`); fall back to the pretty-printed args when the
    /// summary is empty so the body is never blank.
    fn value(ctx: &PermissionContext) -> String {
        let summary = ctx.card.summary.trim();
        if !summary.is_empty() {
            return summary.to_string();
        }
        ctx.card.input_pretty.trim().to_string()
    }

    /// The trailing path component of `path`, handling both `/` and `\`
    /// separators. Trailing separators are ignored. Falls back to the
    /// whole string when there is no separator (already a basename) and to
    /// the original input when it is empty after trimming separators.
    fn basename(path: &str) -> String {
        let trimmed = path.trim_end_matches(['/', '\\']);
        if trimmed.is_empty() {
            return path.to_string();
        }
        trimmed
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(trimmed)
            .to_string()
    }
}

impl PermissionComponent for FilesystemComponent {
    fn icon(&self) -> &'static str {
        "🔎"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let value = Self::value(ctx);
        let text = match ctx.card.tool_name.as_str() {
            "Read" => format!("Read {}", Self::basename(&value)),
            "Glob" => format!("Search files matching {value}"),
            "Grep" => format!("Grep for {value}"),
            // Defensive: the dispatcher only routes the three tools above,
            // but never panic on an unexpected name.
            other => format!("{other} {value}"),
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
        // The path/pattern in the shared "code" style (dim fg on the
        // hover surface), matching the Fallback card's arg rendering.
        lines.push(Line::from(Span::styled(
            Self::value(ctx),
            Style::default()
                .fg(ctx.theme.text_dim)
                .bg(ctx.theme.surface_hover),
        )));
        // `Read` is the low-risk case: a dim `read-only` note reassures
        // the user this touches nothing on disk.
        if ctx.card.tool_name == "Read" {
            lines.push(Line::from(Span::styled(
                "read-only",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::permission::ApprovalAction;
    use crate::tui::theme::Theme;

    fn card(tool: &str, summary: &str) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: tool.into(),
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
    fn read_title_uses_basename_of_the_path() {
        let t = Theme::hearth();
        let c = card("Read", "/Users/sean/dev/wayland/src/lib.rs");
        let comp = FilesystemComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Read lib.rs");
    }

    #[test]
    fn read_title_basename_handles_windows_separators() {
        let t = Theme::hearth();
        let c = card("Read", r"C:\Users\sean\notes.md");
        let comp = FilesystemComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Read notes.md");
    }

    #[test]
    fn glob_title_frames_the_pattern_as_a_file_search() {
        let t = Theme::hearth();
        let c = card("Glob", "**/*.rs");
        let comp = FilesystemComponent;
        assert_eq!(
            line_text(&comp.title(&ctx(&c, &t))),
            "Search files matching **/*.rs"
        );
    }

    #[test]
    fn grep_title_frames_the_pattern_as_a_content_search() {
        let t = Theme::hearth();
        let c = card("Grep", "TODO");
        let comp = FilesystemComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Grep for TODO");
    }

    #[test]
    fn body_renders_the_path_or_pattern_in_code_style() {
        let t = Theme::hearth();
        let c = card("Glob", "src/**/*.toml");
        let comp = FilesystemComponent;
        let body = comp.body(&ctx(&c, &t));
        // Pattern line only — no read-only note for non-Read tools.
        assert_eq!(body.len(), 1);
        assert_eq!(line_text(&body[0]), "src/**/*.toml");
        // The code-style background matches the Fallback arg convention.
        assert_eq!(body[0].spans[0].style.bg, Some(t.surface_hover));
    }

    #[test]
    fn read_body_appends_a_dim_read_only_note() {
        let t = Theme::hearth();
        let c = card("Read", "/etc/hosts");
        let comp = FilesystemComponent;
        let body = comp.body(&ctx(&c, &t));
        // Path line + read-only note.
        assert_eq!(body.len(), 2);
        assert_eq!(line_text(&body[0]), "/etc/hosts");
        assert_eq!(line_text(&body[1]), "read-only");
        // The note is muted/dim, not the primary text color.
        assert_eq!(body[1].spans[0].style.fg, Some(t.text_muted));
    }

    #[test]
    fn grep_and_glob_carry_no_read_only_note() {
        let t = Theme::hearth();
        let comp = FilesystemComponent;
        for tool in ["Grep", "Glob"] {
            let c = card(tool, "x");
            let body = comp.body(&ctx(&c, &t));
            assert_eq!(body.len(), 1, "{tool} should have no read-only note");
        }
    }

    #[test]
    fn icon_is_the_magnifier() {
        assert_eq!(FilesystemComponent.icon(), "🔎");
    }

    #[test]
    fn keys_offer_approve_always_deny_and_cancel() {
        let t = Theme::hearth();
        let c = card("Read", "/x");
        let comp = FilesystemComponent;
        let keys = line_text(&comp.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"));
        assert!(keys.contains("always"));
        assert!(keys.contains("deny"));
        assert!(keys.contains("cancel"));
    }

    #[test]
    fn default_action_is_approve_once() {
        assert_eq!(
            FilesystemComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }

    #[test]
    fn value_falls_back_to_input_pretty_when_summary_empty() {
        let t = Theme::hearth();
        let mut c = card("Read", "");
        c.input_pretty = "/fallback/path.rs".into();
        let comp = FilesystemComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Read path.rs");
    }
}
