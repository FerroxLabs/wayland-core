//! The NotebookEdit permission component (v0.9.2 W4, SPEC §2 #6).
//!
//! Projection for the `NotebookEdit` tool: a pencil glyph (`✎`), an
//! `Edit notebook {basename}` title, and a per-cell diff breakdown. Unlike
//! `Edit`, NotebookEdit carries no `DiffModel` on the card — the cells live
//! in the pretty-printed args JSON, so this component parses them itself.
//!
//! Two arg shapes are accepted (both seen in the wild):
//!   * a single cell on the top-level object — `cell_number`/`cell_id` plus
//!     `new_source`/`source`, optional `cell_type`;
//!   * a `cells` array of those same per-cell objects.
//!
//! Each changed cell renders as a sub-block: a `Cell N (type)` header line
//! followed by its new source as `diff_lines` (built from an empty→new
//! `DiffModel` so the rendering reads identically to the shared diff
//! widget — sign column + syntect highlight). The whole body is clamped to
//! [`NOTEBOOK_CLAMP`] lines with a `… (N more · ctrl+f to expand)` tail, and
//! the key row advertises `[ctrl+f] expand`. When the args do not parse into
//! any cell the body is a single dim `(no preview)` line — never a raw JSON
//! wall.
//!
//! Pure over `PermissionContext` — no I/O, no state — so it is unit-tested
//! purely on its title/body/keys text.

use std::path::Path;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::app::DiffModel;
use crate::tui::permission::{PermissionComponent, PermissionContext};
use crate::tui::widgets::diff_lines;

/// Permission projection for the `NotebookEdit` tool.
pub struct NotebookEditComponent;

/// Max body lines (across all cell sub-blocks) before the `ctrl+f` expand
/// affordance kicks in. A notebook edit can touch several cells, each of
/// which can be long — the clamp keeps the inline card from flooding the
/// transcript.
const NOTEBOOK_CLAMP: usize = 12;

/// One parsed cell edit pulled from the args JSON.
struct CellEdit {
    /// 1-based label the user sees (`cell_number`, else the array position).
    label: String,
    /// The cell kind, defaulting to `code`.
    kind: String,
    /// The new source for the cell.
    source: String,
}

impl PermissionComponent for NotebookEditComponent {
    fn icon(&self) -> &'static str {
        "✎"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let path = notebook_path(ctx);
        let base = basename(&path);
        Line::from(Span::styled(
            format!("Edit notebook {base}"),
            Style::default()
                .fg(ctx.theme.text)
                .add_modifier(Modifier::BOLD),
        ))
    }

    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>> {
        let cells = parse_cells(&ctx.card.input_pretty);
        if cells.is_empty() {
            // Fallback: args carry no cell — a single dim note, never a JSON
            // wall.
            return vec![Line::from(Span::styled(
                "(no preview)".to_string(),
                Style::default().fg(ctx.theme.text_muted),
            ))];
        }

        // One sub-block per changed cell: a `Cell N (type)` header followed
        // by the new source rendered through the shared diff line-builder
        // (empty→new, so every row is an addition with the same sign column
        // + highlight the full diff widget uses).
        let mut lines: Vec<Line<'static>> = Vec::new();
        for cell in &cells {
            lines.push(Line::from(Span::styled(
                format!("Cell {} ({})", cell.label, cell.kind),
                Style::default()
                    .fg(ctx.theme.text_dim)
                    .add_modifier(Modifier::BOLD),
            )));
            let diff = DiffModel {
                path: notebook_path(ctx),
                old: String::new(),
                new: cell.source.clone(),
            };
            lines.extend(diff_lines(&diff, ctx.width, ctx.theme));
        }

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

/// Clamp the body to [`NOTEBOOK_CLAMP`] lines, appending a muted
/// `… (N more · ctrl+f to expand)` tail when there is more.
fn clamp(lines: Vec<Line<'static>>, ctx: &PermissionContext) -> Vec<Line<'static>> {
    // When the user has toggled `ctrl+f`, show the full body unclamped.
    if ctx.expanded {
        return lines;
    }
    let total = lines.len();
    if total <= NOTEBOOK_CLAMP {
        return lines;
    }
    let mut out: Vec<Line<'static>> = lines.into_iter().take(NOTEBOOK_CLAMP).collect();
    let remaining = total - NOTEBOOK_CLAMP;
    out.push(Line::from(Span::styled(
        format!("… ({remaining} more · ctrl+f to expand)"),
        Style::default().fg(ctx.theme.text_muted),
    )));
    out
}

/// The notebook target path: the `notebook_path` arg, falling back to a
/// bare `file_path` for the older arg name.
fn notebook_path(ctx: &PermissionContext) -> String {
    let pretty = &ctx.card.input_pretty;
    arg_field(pretty, "notebook_path")
        .or_else(|| arg_field(pretty, "file_path"))
        .unwrap_or_default()
}

/// Parse the cell edits out of the args JSON. Accepts either a top-level
/// `cells` array or a single cell carried directly on the top-level object.
fn parse_cells(input_pretty: &str) -> Vec<CellEdit> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(input_pretty) else {
        return Vec::new();
    };

    if let Some(arr) = value.get("cells").and_then(|c| c.as_array()) {
        return arr
            .iter()
            .enumerate()
            .filter_map(|(i, cell)| parse_one_cell(cell, i))
            .collect();
    }

    // Single-cell shape: the cell fields sit on the top-level object.
    parse_one_cell(&value, 0).into_iter().collect()
}

/// Parse one cell object. The new source comes from `new_source` (else the
/// older `source`); the label from `cell_number` (else `cell_id`, else the
/// array position); the kind from `cell_type`, defaulting to `code`. Returns
/// `None` when there is no source to show.
fn parse_one_cell(cell: &serde_json::Value, index: usize) -> Option<CellEdit> {
    let source = cell
        .get("new_source")
        .or_else(|| cell.get("source"))
        .and_then(|s| s.as_str())?
        .to_string();

    let label = cell
        .get("cell_number")
        .and_then(value_as_label)
        .or_else(|| cell.get("cell_id").and_then(value_as_label))
        .unwrap_or_else(|| (index + 1).to_string());

    let kind = cell
        .get("cell_type")
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("code")
        .to_string();

    Some(CellEdit {
        label,
        kind,
        source,
    })
}

/// Render a JSON value (number or string) as a cell label.
fn value_as_label(v: &serde_json::Value) -> Option<String> {
    if let Some(n) = v.as_u64() {
        return Some(n.to_string());
    }
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

/// The trailing path component (the notebook's display name). Falls back to
/// the whole path when there is no separator, and to `notebook` for an empty
/// path.
fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if path.is_empty() {
                "notebook".to_string()
            } else {
                path.to_string()
            }
        })
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
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::theme::Theme;

    fn card(input_pretty: &str) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "NotebookEdit".into(),
            summary: String::new(),
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
        assert_eq!(NotebookEditComponent.icon(), "✎");
    }

    #[test]
    fn title_uses_the_notebook_basename() {
        let t = Theme::hearth();
        let c = card(r#"{"notebook_path":"/tmp/proj/analysis.ipynb"}"#);
        let title = line_text(&NotebookEditComponent.title(&ctx(&c, &t)));
        assert_eq!(title, "Edit notebook analysis.ipynb", "title: {title}");
    }

    #[test]
    fn title_falls_back_to_notebook_for_an_empty_path() {
        let t = Theme::hearth();
        let c = card("{}");
        let title = line_text(&NotebookEditComponent.title(&ctx(&c, &t)));
        assert_eq!(title, "Edit notebook notebook", "title: {title}");
    }

    #[test]
    fn body_renders_a_single_cell_with_its_header_and_source() {
        let t = Theme::hearth();
        let c = card(
            r#"{"notebook_path":"a.ipynb","cell_number":3,"cell_type":"code","new_source":"print('hi')"}"#,
        );
        let body = NotebookEditComponent.body(&ctx(&c, &t));
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            joined.contains("Cell 3 (code)"),
            "cell header missing: {joined}"
        );
        assert!(joined.contains("print('hi')"), "source missing: {joined}");
    }

    #[test]
    fn body_renders_two_cells_as_two_cell_headers() {
        let t = Theme::hearth();
        let c = card(
            r##"{"notebook_path":"a.ipynb","cells":[
                {"cell_number":1,"cell_type":"markdown","new_source":"# Title"},
                {"cell_number":2,"cell_type":"code","new_source":"x = 1"}
            ]}"##,
        );
        let body = NotebookEditComponent.body(&ctx(&c, &t));
        let headers: Vec<String> = body
            .iter()
            .map(line_text)
            .filter(|l| l.starts_with("Cell "))
            .collect();
        assert_eq!(headers.len(), 2, "expected two cell headers: {headers:?}");
        assert!(headers[0].contains("Cell 1 (markdown)"), "{headers:?}");
        assert!(headers[1].contains("Cell 2 (code)"), "{headers:?}");
    }

    #[test]
    fn body_clamps_a_long_cell_and_appends_the_tail() {
        let t = Theme::hearth();
        let src: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let c = card(&format!(
            r#"{{"notebook_path":"big.ipynb","cell_number":1,"new_source":{}}}"#,
            serde_json::to_string(&src).unwrap()
        ));
        let body = NotebookEditComponent.body(&ctx(&c, &t));
        assert_eq!(
            body.len(),
            NOTEBOOK_CLAMP + 1,
            "should clamp to {NOTEBOOK_CLAMP} + tail"
        );
        let tail = line_text(body.last().unwrap());
        assert!(tail.contains("more"), "clamp tail: {tail}");
        assert!(tail.contains("ctrl+f"), "expand affordance: {tail}");
    }

    #[test]
    fn body_does_not_clamp_a_long_cell_when_expanded() {
        let t = Theme::hearth();
        let src: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let c = card(&format!(
            r#"{{"notebook_path":"big.ipynb","cell_number":1,"new_source":{}}}"#,
            serde_json::to_string(&src).unwrap()
        ));
        let body = NotebookEditComponent.body(&expanded_ctx(&c, &t));
        // Expanded: full cell body, more than the clamp, and no truncation tail.
        assert!(
            body.len() > NOTEBOOK_CLAMP,
            "expanded body should exceed {NOTEBOOK_CLAMP}, got {}",
            body.len()
        );
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            !joined.contains("more · ctrl+f"),
            "expanded body must not append a clamp tail: {joined}"
        );
    }

    #[test]
    fn keys_show_collapse_when_expanded() {
        let t = Theme::hearth();
        let c = card(r#"{"notebook_path":"a.ipynb"}"#);
        let keys = line_text(&NotebookEditComponent.keys(&expanded_ctx(&c, &t)));
        assert!(keys.contains("ctrl+f"), "keys: {keys}");
        assert!(keys.contains("collapse"), "collapse hint missing: {keys}");
        assert!(!keys.contains("expand"), "should not offer expand: {keys}");
    }

    #[test]
    fn body_shows_no_preview_note_when_args_carry_no_cell() {
        let t = Theme::hearth();
        let c = card(r#"{"notebook_path":"a.ipynb"}"#);
        let body = NotebookEditComponent.body(&ctx(&c, &t));
        assert_eq!(body.len(), 1);
        assert!(
            line_text(&body[0]).contains("no preview"),
            "expected no-preview note"
        );
    }

    #[test]
    fn body_shows_no_preview_note_for_unparseable_args() {
        let t = Theme::hearth();
        let c = card("not json at all {{{");
        let body = NotebookEditComponent.body(&ctx(&c, &t));
        assert_eq!(body.len(), 1);
        assert!(
            line_text(&body[0]).contains("no preview"),
            "expected no-preview note"
        );
    }

    #[test]
    fn keys_offer_approve_deny_and_ctrl_f_expand() {
        let t = Theme::hearth();
        let c = card(r#"{"notebook_path":"a.ipynb"}"#);
        let keys = line_text(&NotebookEditComponent.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"), "keys: {keys}");
        assert!(keys.contains("deny"), "keys: {keys}");
        assert!(keys.contains("ctrl+f"), "expand key missing: {keys}");
    }

    #[test]
    fn keys_hide_always_when_the_gate_is_closed() {
        let t = Theme::hearth();
        let c = card(r#"{"notebook_path":"a.ipynb"}"#);
        let mut context = ctx(&c, &t);
        context.always_allow_available = false;
        let keys = line_text(&NotebookEditComponent.keys(&context));
        assert!(!keys.contains("always"), "always must be hidden: {keys}");
    }

    #[test]
    fn default_action_is_approve_once() {
        use crate::tui::permission::ApprovalAction;
        assert_eq!(
            NotebookEditComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
