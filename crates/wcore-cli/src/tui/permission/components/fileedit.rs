//! The FileEdit permission component (v0.9.2 W3, SPEC §2 #2).
//!
//! Projection for the `Edit` / `FileEdit` tool: a brand-glyph header (`✎`),
//! a `Make this edit to {basename}` title with the dim cwd-relative path,
//! and the inline diff. `Edit` carries its preview as a `DiffModel` (built
//! by `protocol_bridge::edit_preview_from_args` — `old`/`new` from the
//! `old_string`/`new_string` args), rendered via the shared
//! `widgets::diff_lines` extract so the inline card reads identically to the
//! full diff widget (sign column + syntect highlight). The body is clamped
//! to [`EDIT_CLAMP`] lines with a `… (N more lines · ctrl+f to expand)` tail,
//! and the key row advertises `[ctrl+f] expand`. When no preview is present
//! the body renders a single dim `(no preview)` line — never a raw JSON wall.
//!
//! Pure over `PermissionContext` — no I/O, no state — so it is unit-tested
//! purely on its title/body/keys text.

use std::path::Path;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::permission::{PermissionComponent, PermissionContext};
use crate::tui::widgets::diff_lines;

/// Permission projection for the `Edit` / `FileEdit` tool.
pub struct FileEditComponent;

/// Max diff lines before the `ctrl+f` expand affordance kicks in. An edit is
/// usually a small hunk, but a large refactor edit can be long — the clamp
/// keeps it from flooding the transcript.
const EDIT_CLAMP: usize = 12;

impl PermissionComponent for FileEditComponent {
    fn icon(&self) -> &'static str {
        "✎"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let path = edit_path(ctx);
        let base = basename(&path);
        Line::from(vec![
            Span::styled(
                format!("Make this edit to {base}"),
                Style::default()
                    .fg(ctx.theme.text)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", cwd_relative(&path)),
                Style::default().fg(ctx.theme.text_dim),
            ),
        ])
    }

    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>> {
        // Preferred path: the shared diff widget's line-builder renders the
        // hunk (sign column + syntect highlight); clamp to EDIT_CLAMP lines.
        if let Some(diff) = &ctx.card.edit_preview {
            let lines = diff_lines(diff, ctx.width, ctx.theme);
            return clamp(lines, ctx);
        }

        // Fallback: no preview model — a single dim note, never a JSON wall.
        vec![Line::from(Span::styled(
            "(no preview)".to_string(),
            Style::default().fg(ctx.theme.text_muted),
        ))]
    }

    fn keys(&self, ctx: &PermissionContext) -> Line<'static> {
        let always = if ctx.always_allow_available {
            "   [a] always for this tool"
        } else {
            ""
        };
        let expand = if ctx.expanded {
            "[ctrl+f] collapse"
        } else {
            "[ctrl+f] expand"
        };
        Line::from(Span::styled(
            format!("[enter/y] approve{always}   [n] deny   [esc] cancel   {expand}"),
            Style::default().fg(ctx.theme.text_muted),
        ))
    }
}

/// Clamp a diff to [`EDIT_CLAMP`] lines, appending a muted
/// `… (N more lines · ctrl+f to expand)` tail when there is more.
fn clamp(lines: Vec<Line<'static>>, ctx: &PermissionContext) -> Vec<Line<'static>> {
    // When the user has toggled `ctrl+f`, show the full diff unclamped.
    if ctx.expanded {
        return lines;
    }
    let total = lines.len();
    if total <= EDIT_CLAMP {
        return lines;
    }
    let mut out: Vec<Line<'static>> = lines.into_iter().take(EDIT_CLAMP).collect();
    let remaining = total - EDIT_CLAMP;
    out.push(Line::from(Span::styled(
        format!("… ({remaining} more lines · ctrl+f to expand)"),
        Style::default().fg(ctx.theme.text_muted),
    )));
    out
}

/// The edit target path: the preview's `path` when present, else the
/// `file_path` pulled from the pretty-printed args.
fn edit_path(ctx: &PermissionContext) -> String {
    if let Some(diff) = &ctx.card.edit_preview
        && !diff.path.is_empty()
    {
        return diff.path.clone();
    }
    arg_field(&ctx.card.input_pretty, "file_path").unwrap_or_default()
}

/// The trailing path component (the file's display name). Falls back to the
/// whole path when there is no separator, and to `file` for an empty path.
fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if path.is_empty() {
                "file".to_string()
            } else {
                path.to_string()
            }
        })
}

/// The path relative to the current working directory, so the dim subtitle
/// reads `src/foo.rs` rather than a long absolute path. Falls back to the
/// raw path when it is not under the cwd (or the cwd is unknown).
fn cwd_relative(path: &str) -> String {
    let abs = Path::new(path);
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(rel) = abs.strip_prefix(&cwd)
        && let Some(s) = rel.to_str()
        && !s.is_empty()
    {
        return s.to_string();
    }
    path.to_string()
}

/// Pull a top-level string field out of the pretty-printed args JSON.
fn arg_field(input_pretty: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(input_pretty)
        .ok()?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{DiffModel, ToolCardModel, ToolCardStatus};
    use crate::tui::theme::Theme;

    fn card(input_pretty: &str, preview: Option<DiffModel>) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "Edit".into(),
            summary: String::new(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: preview,
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

    fn expanded_ctx<'a>(c: &'a ToolCardModel, t: &'a Theme) -> PermissionContext<'a> {
        let mut context = ctx(c, t);
        context.expanded = true;
        context
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn icon_is_the_edit_glyph() {
        assert_eq!(FileEditComponent.icon(), "✎");
    }

    #[test]
    fn title_uses_the_basename_and_shows_the_path() {
        let t = Theme::hearth();
        let c = card(r#"{"file_path":"/tmp/proj/src/lib.rs"}"#, None);
        let title = line_text(&FileEditComponent.title(&ctx(&c, &t)));
        assert!(
            title.starts_with("Make this edit to lib.rs"),
            "title: {title}"
        );
        assert!(title.contains("lib.rs"), "path missing: {title}");
    }

    #[test]
    fn title_falls_back_to_file_for_an_empty_path() {
        let t = Theme::hearth();
        let c = card("{}", None);
        let title = line_text(&FileEditComponent.title(&ctx(&c, &t)));
        assert!(
            title.starts_with("Make this edit to file"),
            "title: {title}"
        );
    }

    #[test]
    fn body_renders_the_diff_when_a_preview_is_present() {
        let t = Theme::hearth();
        let preview = DiffModel {
            path: "src/lib.rs".into(),
            old: "fn a() {}\n".into(),
            new: "fn a() {}\nfn b() {}\n".into(),
        };
        let c = card(r#"{"file_path":"src/lib.rs"}"#, Some(preview));
        let body = FileEditComponent.body(&ctx(&c, &t));
        assert!(!body.is_empty(), "diff body should render lines");
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("fn b()"), "added content missing: {joined}");
    }

    #[test]
    fn body_clamps_a_long_diff_and_appends_the_tail() {
        let t = Theme::hearth();
        let new: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let preview = DiffModel {
            path: "big.rs".into(),
            old: String::new(),
            new,
        };
        let c = card(r#"{"file_path":"big.rs"}"#, Some(preview));
        let body = FileEditComponent.body(&ctx(&c, &t));
        assert_eq!(
            body.len(),
            EDIT_CLAMP + 1,
            "should clamp to {EDIT_CLAMP} + tail"
        );
        let tail = line_text(body.last().unwrap());
        assert!(tail.contains("more lines"), "clamp tail: {tail}");
        assert!(tail.contains("ctrl+f"), "expand affordance: {tail}");
    }

    #[test]
    fn body_does_not_clamp_a_long_diff_when_expanded() {
        let t = Theme::hearth();
        let new: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let preview = DiffModel {
            path: "big.rs".into(),
            old: String::new(),
            new,
        };
        let c = card(r#"{"file_path":"big.rs"}"#, Some(preview));
        let body = FileEditComponent.body(&expanded_ctx(&c, &t));
        // Expanded: full diff, more than the clamp, and no truncation tail.
        assert!(
            body.len() > EDIT_CLAMP,
            "expanded body should exceed {EDIT_CLAMP}, got {}",
            body.len()
        );
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            !joined.contains("more lines"),
            "expanded body must not append a clamp tail: {joined}"
        );
    }

    #[test]
    fn keys_show_collapse_when_expanded() {
        let t = Theme::hearth();
        let c = card(r#"{"file_path":"x.rs"}"#, None);
        let keys = line_text(&FileEditComponent.keys(&expanded_ctx(&c, &t)));
        assert!(keys.contains("ctrl+f"), "keys: {keys}");
        assert!(keys.contains("collapse"), "collapse hint missing: {keys}");
        assert!(!keys.contains("expand"), "should not offer expand: {keys}");
    }

    #[test]
    fn body_shows_no_preview_note_without_a_diff() {
        let t = Theme::hearth();
        let c = card(r#"{"file_path":"x.rs"}"#, None);
        let body = FileEditComponent.body(&ctx(&c, &t));
        assert_eq!(body.len(), 1);
        assert!(
            line_text(&body[0]).contains("no preview"),
            "expected no-preview note"
        );
    }

    #[test]
    fn keys_offer_approve_deny_and_ctrl_f_expand() {
        let t = Theme::hearth();
        let c = card(r#"{"file_path":"x.rs"}"#, None);
        let keys = line_text(&FileEditComponent.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"), "keys: {keys}");
        assert!(keys.contains("deny"), "keys: {keys}");
        assert!(keys.contains("ctrl+f"), "expand key missing: {keys}");
    }

    #[test]
    fn keys_hide_always_when_the_gate_is_closed() {
        let t = Theme::hearth();
        let c = card(r#"{"file_path":"x.rs"}"#, None);
        let mut context = ctx(&c, &t);
        context.always_allow_available = false;
        let keys = line_text(&FileEditComponent.keys(&context));
        assert!(!keys.contains("always"), "always must be hidden: {keys}");
    }

    #[test]
    fn default_action_is_approve_once() {
        use crate::tui::permission::ApprovalAction;
        assert_eq!(
            FileEditComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
