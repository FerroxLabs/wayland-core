//! The FileWrite permission component (v0.9.2 W3, SPEC §2 #3).
//!
//! Projection for the `Write` / `FileWrite` tool: a brand-glyph header
//! (`⬇`), a `Write {basename}` title with the dim cwd-relative path, and a
//! clamped content preview. `Write` carries its preview as a full-add
//! `DiffModel` (built by `protocol_bridge::edit_preview_from_args` —
//! `old = ""`, `new = content`), so when `edit_preview` is present the body
//! renders its `new` content as `+`-prefixed add rows — an all-additions
//! diff (S-W3a). When it is absent (e.g. the args never produced a model)
//! the body falls back to the raw `content` lifted from the pretty-printed
//! args. Either way the preview is clamped to [`WRITE_CLAMP`] lines with a
//! `… (N more lines)` tail, and the key row advertises `[ctrl+f] expand`.
//!
//! NOTE (S-W3a): the SPEC names `widgets::diff::diff_lines` as the body
//! builder for the syntect-highlighted variant, but that fn lives in the
//! private `mod diff` (only `diff_view` is re-exported from
//! `widgets/mod.rs`), so a permission component cannot reach it without a
//! one-line re-export there — outside this component's write-zone. Because a
//! `Write` preview is a pure full-add (`old = ""`), the body builds the add
//! rows directly from `DiffModel.new` here; swap to `diff_lines` once it is
//! exported to pick up syntax highlighting, no other change required.
//!
//! Pure over `PermissionContext` — no I/O, no state — so it is unit-tested
//! purely on its title/body/keys text.

use std::path::Path;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `Write` / `FileWrite` tool.
pub struct FileWriteComponent;

/// Max preview lines before the `ctrl+f` expand affordance kicks in. The
/// card shows the first `WRITE_CLAMP` lines of the new file then a single
/// `… (N more lines)` tail — a write is frequently a whole-file body, so
/// the clamp keeps a large new file from flooding the transcript.
const WRITE_CLAMP: usize = 12;

impl PermissionComponent for FileWriteComponent {
    fn icon(&self) -> &'static str {
        "⬇"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let path = write_path(ctx);
        let base = basename(&path);
        Line::from(vec![
            Span::styled(
                format!("Write {base}"),
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
        // Preferred path: `Write` ships a full-add diff (old empty, new =
        // content), so render `new` as `+`-prefixed add rows and clamp it.
        if let Some(diff) = &ctx.card.edit_preview {
            let lines: Vec<Line<'static>> =
                diff.new.lines().map(|raw| add_line(raw, ctx)).collect();
            return clamp(lines, ctx);
        }

        // Fallback: no preview model — lift the raw `content` out of the
        // pretty-printed args and clamp it. Never a raw JSON wall.
        let content = write_content(&ctx.card.input_pretty);
        let lines: Vec<Line<'static>> = content
            .lines()
            .map(|raw| {
                Line::from(Span::styled(
                    raw.to_string(),
                    Style::default().fg(ctx.theme.text_dim),
                ))
            })
            .collect();
        clamp(lines, ctx)
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

/// One preview row of a new file, rendered as an addition: a `+` sign
/// column in the success color, then the line text. Mirrors the add row of
/// the shared diff widget so the inline write preview reads consistently.
fn add_line(text: &str, ctx: &PermissionContext) -> Line<'static> {
    Line::from(vec![
        Span::styled("+ ", Style::default().fg(ctx.theme.success)),
        Span::styled(text.to_string(), Style::default().fg(ctx.theme.text)),
    ])
}

/// Clamp a preview to [`WRITE_CLAMP`] lines, appending a muted
/// `… (N more lines · ctrl+f to expand)` tail when there is more. A preview
/// at or under the cap is returned untouched.
fn clamp(lines: Vec<Line<'static>>, ctx: &PermissionContext) -> Vec<Line<'static>> {
    // When the user has toggled `ctrl+f`, show the full preview unclamped.
    if ctx.expanded {
        return lines;
    }
    let total = lines.len();
    if total <= WRITE_CLAMP {
        return lines;
    }
    let mut out: Vec<Line<'static>> = lines.into_iter().take(WRITE_CLAMP).collect();
    let remaining = total - WRITE_CLAMP;
    out.push(Line::from(Span::styled(
        format!("… ({remaining} more lines · ctrl+f to expand)"),
        Style::default().fg(ctx.theme.text_muted),
    )));
    out
}

/// The write target path: the preview's `path` when present, else the
/// `file_path` pulled from the pretty-printed args.
fn write_path(ctx: &PermissionContext) -> String {
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

/// Extract the `content` string field from a `Write` tool's pretty-printed
/// JSON args. Returns an empty string when the args are not the expected
/// shape (the body then renders nothing rather than a JSON wall).
fn write_content(input_pretty: &str) -> String {
    arg_field(input_pretty, "content").unwrap_or_default()
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
            tool_name: "Write".into(),
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
    fn icon_is_the_write_glyph() {
        assert_eq!(FileWriteComponent.icon(), "⬇");
    }

    #[test]
    fn title_uses_the_basename_and_shows_the_path() {
        let t = Theme::hearth();
        let c = card(
            r#"{"file_path":"/tmp/proj/src/lib.rs","content":"fn x() {}"}"#,
            None,
        );
        let title = line_text(&FileWriteComponent.title(&ctx(&c, &t)));
        // The header names the file by its basename, not the full path.
        assert!(title.starts_with("Write lib.rs"), "title: {title}");
        // The full/relative path still appears in the dim subtitle.
        assert!(title.contains("lib.rs"), "path missing: {title}");
    }

    #[test]
    fn title_falls_back_to_file_for_an_empty_path() {
        let t = Theme::hearth();
        let c = card(r#"{"content":"x"}"#, None);
        let title = line_text(&FileWriteComponent.title(&ctx(&c, &t)));
        assert!(title.starts_with("Write file"), "title: {title}");
    }

    #[test]
    fn body_renders_a_short_write_in_full() {
        let t = Theme::hearth();
        let preview = DiffModel {
            path: "src/lib.rs".into(),
            old: String::new(),
            new: "fn a() {}\nfn b() {}\n".into(),
        };
        let c = card(
            r#"{"file_path":"src/lib.rs","content":"..."}"#,
            Some(preview),
        );
        let body = FileWriteComponent.body(&ctx(&c, &t));
        // Two added lines, no clamp tail.
        assert_eq!(body.len(), 2);
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("fn a()"), "content missing: {joined}");
        assert!(joined.contains("fn b()"), "content missing: {joined}");
        // Each preview row is rendered as an addition.
        assert!(joined.contains("+ fn a()"), "add sign missing: {joined}");
    }

    #[test]
    fn body_clamps_a_long_preview_and_appends_the_tail() {
        let t = Theme::hearth();
        let content: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let preview = DiffModel {
            path: "big.txt".into(),
            old: String::new(),
            new: content,
        };
        let c = card(r#"{"file_path":"big.txt","content":"..."}"#, Some(preview));
        let body = FileWriteComponent.body(&ctx(&c, &t));
        // 12 clamped lines + 1 tail — never the full 40.
        assert_eq!(body.len(), WRITE_CLAMP + 1);
        let tail = line_text(body.last().unwrap());
        assert!(tail.contains("28 more lines"), "clamp tail: {tail}");
        assert!(tail.contains("ctrl+f"), "expand affordance: {tail}");
    }

    #[test]
    fn body_does_not_clamp_a_long_preview_when_expanded() {
        let t = Theme::hearth();
        let content: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let preview = DiffModel {
            path: "big.txt".into(),
            old: String::new(),
            new: content,
        };
        let c = card(r#"{"file_path":"big.txt","content":"..."}"#, Some(preview));
        let body = FileWriteComponent.body(&expanded_ctx(&c, &t));
        // Expanded: all 40 add rows, no clamp tail.
        assert_eq!(body.len(), 40, "expanded body should show all rows");
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            !joined.contains("more lines"),
            "expanded body must not append a clamp tail: {joined}"
        );
    }

    #[test]
    fn keys_show_collapse_when_expanded() {
        let t = Theme::hearth();
        let c = card(r#"{"file_path":"x","content":"y"}"#, None);
        let keys = line_text(&FileWriteComponent.keys(&expanded_ctx(&c, &t)));
        assert!(keys.contains("ctrl+f"), "keys: {keys}");
        assert!(keys.contains("collapse"), "collapse hint missing: {keys}");
        assert!(!keys.contains("expand"), "should not offer expand: {keys}");
    }

    #[test]
    fn body_falls_back_to_raw_content_without_a_preview() {
        let t = Theme::hearth();
        // No edit_preview — must lift `content` out of the args, clamped.
        let big: String = (0..30).map(|i| format!("row {i}\\n")).collect::<String>();
        let pretty = format!(r#"{{"file_path":"x.txt","content":"{big}"}}"#);
        let c = card(&pretty, None);
        let body = FileWriteComponent.body(&ctx(&c, &t));
        assert_eq!(body.len(), WRITE_CLAMP + 1);
        let tail = line_text(body.last().unwrap());
        assert!(tail.contains("more lines"), "clamp tail: {tail}");
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            joined.contains("row 0"),
            "first content row missing: {joined}"
        );
    }

    #[test]
    fn body_is_empty_for_unparseable_args_without_a_preview() {
        let t = Theme::hearth();
        // Garbage args + no preview → empty body, never a raw dump.
        let c = card("not json", None);
        let body = FileWriteComponent.body(&ctx(&c, &t));
        assert!(body.is_empty(), "expected empty body, got {}", body.len());
    }

    #[test]
    fn keys_offer_approve_deny_and_ctrl_f_expand() {
        let t = Theme::hearth();
        let c = card(r#"{"file_path":"x","content":"y"}"#, None);
        let keys = line_text(&FileWriteComponent.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"), "keys: {keys}");
        assert!(keys.contains("deny"), "keys: {keys}");
        assert!(keys.contains("cancel"), "keys: {keys}");
        assert!(keys.contains("ctrl+f"), "expand key missing: {keys}");
    }

    #[test]
    fn keys_hide_always_when_the_gate_is_closed() {
        let t = Theme::hearth();
        let c = card(r#"{"file_path":"x","content":"y"}"#, None);
        let mut context = ctx(&c, &t);
        context.always_allow_available = false;
        let keys = line_text(&FileWriteComponent.keys(&context));
        assert!(!keys.contains("always"), "always must be hidden: {keys}");
        assert!(keys.contains("ctrl+f"), "expand key still present: {keys}");
    }

    #[test]
    fn default_action_is_approve_once() {
        use crate::tui::permission::ApprovalAction;
        assert_eq!(
            FileWriteComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
