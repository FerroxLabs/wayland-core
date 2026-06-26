//! The ReviewArtifact permission component (v0.9.2 W4, SPEC §2 #14,
//! feature-gated `review_artifact`).
//!
//! Projection for the `ReviewArtifact` tool: a document header (`▤`), a
//! `Review artifact {name}` title naming the artifact, and a body that
//! shows the artifact name, its type, and its size.
//!
//! The whole module — including these tests — only compiles under
//! `#[cfg(feature = "review_artifact")]` (the gate lives on the
//! `pub mod review_artifact;` line in `components/mod.rs`), so the default
//! build never pays for it and the dispatcher falls to `Fallback` when the
//! feature is off.
//!
//! Data comes from the card: `name`, `type`, and `size` are pulled from the
//! pretty-printed args JSON when present, with the card `summary` as the
//! name fallback so the card is never blank. The `size` field accepts both a
//! JSON number of bytes (rendered human-readable) and a pre-formatted
//! string. Pure over `PermissionContext` — no I/O, no state — so it is
//! unit-tested purely on its title/body/keys text.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `ReviewArtifact` tool (feature
/// `review_artifact`).
pub struct ReviewArtifactComponent;

impl ReviewArtifactComponent {
    /// The artifact name: the `name` field from the pretty-printed args when
    /// present, else the card `summary` (which previews the args). Empty when
    /// neither yields a name (the title degrades to `Review artifact`).
    fn name(ctx: &PermissionContext) -> String {
        if let Some(name) = arg_str(&ctx.card.input_pretty, "name")
            && !name.trim().is_empty()
        {
            return name.trim().to_string();
        }
        ctx.card.summary.trim().to_string()
    }

    /// The artifact type from the args `type` field (e.g. `pdf`, `report`).
    /// `None` when absent or empty.
    fn artifact_type(ctx: &PermissionContext) -> Option<String> {
        let ty = arg_str(&ctx.card.input_pretty, "type")?;
        let ty = ty.trim();
        if ty.is_empty() {
            None
        } else {
            Some(ty.to_string())
        }
    }

    /// The artifact size: a human-readable string from the args `size`
    /// field. A JSON number is treated as a byte count and formatted; a
    /// non-empty string is passed through verbatim. `None` when absent.
    fn size(ctx: &PermissionContext) -> Option<String> {
        let value = arg_value(&ctx.card.input_pretty, "size")?;
        if let Some(bytes) = value.as_u64() {
            return Some(human_bytes(bytes));
        }
        let s = value.as_str()?.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    }
}

impl PermissionComponent for ReviewArtifactComponent {
    fn icon(&self) -> &'static str {
        "▤"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let name = Self::name(ctx);
        let text = if name.is_empty() {
            "Review artifact".to_string()
        } else {
            format!("Review artifact {name}")
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

        // The artifact name in primary text — what is being reviewed.
        let name = Self::name(ctx);
        if !name.is_empty() {
            lines.push(Line::from(Span::styled(
                name,
                Style::default().fg(ctx.theme.text),
            )));
        }

        // The type and size as dim metadata notes.
        if let Some(ty) = Self::artifact_type(ctx) {
            lines.push(Line::from(Span::styled(
                ty,
                Style::default().fg(ctx.theme.text_muted),
            )));
        }
        if let Some(size) = Self::size(ctx) {
            lines.push(Line::from(Span::styled(
                size,
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

/// Render a byte count as a compact human-readable size (`512 B`, `1.5 KB`,
/// `3.0 MB`). Binary units (1024-based) to match how files report on disk.
fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

/// Pull a top-level string field out of the pretty-printed args JSON.
/// Returns `None` when the args are not the expected JSON shape.
fn arg_str(input_pretty: &str, key: &str) -> Option<String> {
    arg_value(input_pretty, key)?.as_str().map(str::to_string)
}

/// Pull a top-level field (any JSON value) out of the pretty-printed args.
fn arg_value(input_pretty: &str, key: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(input_pretty)
        .ok()?
        .get(key)
        .cloned()
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
            tool_name: "ReviewArtifact".into(),
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
    fn icon_is_the_document_glyph() {
        assert_eq!(ReviewArtifactComponent.icon(), "▤");
    }

    #[test]
    fn title_carries_the_artifact_name_from_args() {
        let t = Theme::hearth();
        let c = card(r#"{"name":"Q3-report.pdf"}"#, "");
        let comp = ReviewArtifactComponent;
        assert_eq!(
            line_text(&comp.title(&ctx(&c, &t))),
            "Review artifact Q3-report.pdf"
        );
    }

    #[test]
    fn title_falls_back_to_summary_when_no_name_field() {
        let t = Theme::hearth();
        let c = card("not json", "diagram.svg");
        let comp = ReviewArtifactComponent;
        assert_eq!(
            line_text(&comp.title(&ctx(&c, &t))),
            "Review artifact diagram.svg"
        );
    }

    #[test]
    fn title_degrades_gracefully_when_empty() {
        let t = Theme::hearth();
        let c = card("not json", "");
        let comp = ReviewArtifactComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Review artifact");
    }

    #[test]
    fn body_shows_name_type_and_size() {
        let t = Theme::hearth();
        let c = card(r#"{"name":"report.pdf","type":"pdf","size":2048}"#, "");
        let comp = ReviewArtifactComponent;
        let body = comp.body(&ctx(&c, &t));
        assert_eq!(body.len(), 3);
        assert_eq!(line_text(&body[0]), "report.pdf");
        assert_eq!(line_text(&body[1]), "pdf");
        // 2048 bytes → 2.0 KB (binary units).
        assert_eq!(line_text(&body[2]), "2.0 KB");
        // Metadata lines are muted/dim, not primary text.
        assert_eq!(body[1].spans[0].style.fg, Some(t.text_muted));
        assert_eq!(body[2].spans[0].style.fg, Some(t.text_muted));
    }

    #[test]
    fn size_passes_through_a_preformatted_string() {
        let t = Theme::hearth();
        let c = card(r#"{"name":"x","size":"4.2 MB"}"#, "");
        let comp = ReviewArtifactComponent;
        let body = comp.body(&ctx(&c, &t));
        assert!(body.iter().any(|l| line_text(l) == "4.2 MB"));
    }

    #[test]
    fn small_byte_counts_render_in_bytes() {
        let t = Theme::hearth();
        let c = card(r#"{"name":"x","size":512}"#, "");
        let comp = ReviewArtifactComponent;
        let body = comp.body(&ctx(&c, &t));
        assert!(body.iter().any(|l| line_text(l) == "512 B"));
    }

    #[test]
    fn body_omits_missing_type_and_size() {
        let t = Theme::hearth();
        let c = card("not json", "notes.txt");
        let comp = ReviewArtifactComponent;
        let body = comp.body(&ctx(&c, &t));
        assert_eq!(body.len(), 1);
        assert_eq!(line_text(&body[0]), "notes.txt");
    }

    #[test]
    fn keys_offer_approve_always_deny_and_cancel() {
        let t = Theme::hearth();
        let c = card(r#"{"name":"x"}"#, "");
        let comp = ReviewArtifactComponent;
        let keys = line_text(&comp.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"));
        assert!(keys.contains("always"));
        assert!(keys.contains("deny"));
        assert!(keys.contains("cancel"));
    }

    #[test]
    fn default_action_is_approve_once() {
        assert_eq!(
            ReviewArtifactComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
