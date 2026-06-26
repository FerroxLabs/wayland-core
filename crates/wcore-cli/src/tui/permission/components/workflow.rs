//! The Workflow permission component (v0.9.2 W4, SPEC §2 #11,
//! feature-gated `workflow`).
//!
//! Projection for the `Workflow` tool: a recycle/loop header (`⟳`), a
//! `Run ForgeFlow {name}` title naming the ForgeFlow, and a body that shows
//! the ForgeFlow name, its step count, and a one-line summary.
//!
//! The whole module — including these tests — only compiles under
//! `#[cfg(feature = "workflow")]` (the gate lives on the `pub mod workflow;`
//! line in `components/mod.rs`), so the default build never pays for it and
//! the dispatcher falls to `Fallback` when the feature is off.
//!
//! Data comes from the card: the structured fields (`name`, `steps`,
//! `summary`) are pulled from the pretty-printed args JSON when present,
//! falling back to the card `summary` so the card is never blank. Pure over
//! `PermissionContext` — no I/O, no state — so it is unit-tested purely on
//! its title/body/keys text.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `Workflow` tool (feature `workflow`).
pub struct WorkflowComponent;

impl WorkflowComponent {
    /// The ForgeFlow name: the `name` field from the pretty-printed args when
    /// present, else the card `summary` (which previews the args). Empty
    /// when neither yields a name (the title degrades to `Run ForgeFlow`).
    fn name(ctx: &PermissionContext) -> String {
        if let Some(name) = arg_str(&ctx.card.input_pretty, "name")
            && !name.trim().is_empty()
        {
            return name.trim().to_string();
        }
        ctx.card.summary.trim().to_string()
    }

    /// The step count parsed from the args `steps` field, accepting both a
    /// JSON number (`"steps": 3`) and a JSON array (`"steps": [..]`) whose
    /// length is the count. `None` when no usable count is present.
    fn step_count(ctx: &PermissionContext) -> Option<usize> {
        let value = arg_value(&ctx.card.input_pretty, "steps")?;
        if let Some(n) = value.as_u64() {
            return Some(n as usize);
        }
        value.as_array().map(|a| a.len())
    }

    /// A one-line summary from the args `summary` field, distinct from the
    /// workflow name. `None` when absent or empty.
    fn summary(ctx: &PermissionContext) -> Option<String> {
        let summary = arg_str(&ctx.card.input_pretty, "summary")?;
        let summary = summary.trim();
        if summary.is_empty() {
            None
        } else {
            Some(summary.to_string())
        }
    }
}

impl PermissionComponent for WorkflowComponent {
    fn icon(&self) -> &'static str {
        "⟳"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let name = Self::name(ctx);
        let text = if name.is_empty() {
            "Run ForgeFlow".to_string()
        } else {
            format!("Run ForgeFlow {name}")
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

        // The ForgeFlow name in primary text — the headline of what runs.
        let name = Self::name(ctx);
        if !name.is_empty() {
            lines.push(Line::from(Span::styled(
                name,
                Style::default().fg(ctx.theme.text),
            )));
        }

        // The step count as a dim, pluralized note.
        if let Some(count) = Self::step_count(ctx) {
            let unit = if count == 1 { "step" } else { "steps" };
            lines.push(Line::from(Span::styled(
                format!("{count} {unit}"),
                Style::default().fg(ctx.theme.text_muted),
            )));
        }

        // The one-line summary in the shared "code" style (dim fg on the
        // hover surface), matching the other arg-rendering cards.
        if let Some(summary) = Self::summary(ctx) {
            lines.push(Line::from(Span::styled(
                summary,
                Style::default()
                    .fg(ctx.theme.text_dim)
                    .bg(ctx.theme.surface_hover),
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
            tool_name: "Workflow".into(),
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
    fn icon_is_the_loop_glyph() {
        assert_eq!(WorkflowComponent.icon(), "⟳");
    }

    #[test]
    fn title_carries_the_workflow_name_from_args() {
        let t = Theme::hearth();
        let c = card(r#"{"name":"nightly-deploy"}"#, "");
        let comp = WorkflowComponent;
        assert_eq!(
            line_text(&comp.title(&ctx(&c, &t))),
            "Run ForgeFlow nightly-deploy"
        );
    }

    #[test]
    fn title_falls_back_to_summary_when_no_name_field() {
        let t = Theme::hearth();
        let c = card("not json", "release");
        let comp = WorkflowComponent;
        assert_eq!(
            line_text(&comp.title(&ctx(&c, &t))),
            "Run ForgeFlow release"
        );
    }

    #[test]
    fn title_degrades_gracefully_when_empty() {
        let t = Theme::hearth();
        let c = card("not json", "");
        let comp = WorkflowComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Run ForgeFlow");
    }

    #[test]
    fn body_shows_name_step_count_and_summary() {
        let t = Theme::hearth();
        let c = card(
            r#"{"name":"deploy","steps":3,"summary":"build, test, push"}"#,
            "",
        );
        let comp = WorkflowComponent;
        let body = comp.body(&ctx(&c, &t));
        assert_eq!(body.len(), 3);
        assert_eq!(line_text(&body[0]), "deploy");
        assert_eq!(line_text(&body[1]), "3 steps");
        assert_eq!(line_text(&body[2]), "build, test, push");
        // The summary renders in the shared code style.
        assert_eq!(body[2].spans[0].style.bg, Some(t.surface_hover));
    }

    #[test]
    fn body_renders_engine_proposal_with_cost_summary() {
        // The live B6 confirm gate sends `steps` as an integer agent count
        // and folds the cost into `summary` ("~N agents / ~$X.XX"). The body
        // must surface the name, the integer count, and the cost string.
        let t = Theme::hearth();
        let c = card(
            r#"{"name":"deploy","steps":4,"summary":"~4 agents / ~$0.06"}"#,
            "",
        );
        let comp = WorkflowComponent;
        let body = comp.body(&ctx(&c, &t));
        let texts: Vec<String> = body.iter().map(line_text).collect();
        assert!(
            texts.iter().any(|l| l == "deploy"),
            "name missing: {texts:?}"
        );
        assert!(
            texts.iter().any(|l| l == "4 steps"),
            "int step count missing: {texts:?}"
        );
        assert!(
            texts.iter().any(|l| l == "~4 agents / ~$0.06"),
            "cost summary missing: {texts:?}"
        );
        // The cost summary renders in the shared code style.
        let cost = body
            .iter()
            .find(|l| line_text(l) == "~4 agents / ~$0.06")
            .expect("cost line present");
        assert_eq!(cost.spans[0].style.bg, Some(t.surface_hover));
    }

    #[test]
    fn keys_present_approve_deny_chord_for_engine_proposal() {
        // The approve/deny chord must stay intact on the live proposal card.
        let t = Theme::hearth();
        let c = card(
            r#"{"name":"deploy","steps":4,"summary":"~4 agents / ~$0.06"}"#,
            "",
        );
        let comp = WorkflowComponent;
        let keys = line_text(&comp.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"));
        assert!(keys.contains("deny"));
    }

    #[test]
    fn step_count_accepts_an_array_of_steps() {
        let t = Theme::hearth();
        let c = card(r#"{"name":"x","steps":["a","b"]}"#, "");
        let comp = WorkflowComponent;
        let body = comp.body(&ctx(&c, &t));
        assert!(body.iter().any(|l| line_text(l) == "2 steps"));
    }

    #[test]
    fn single_step_is_not_pluralized() {
        let t = Theme::hearth();
        let c = card(r#"{"name":"x","steps":1}"#, "");
        let comp = WorkflowComponent;
        let body = comp.body(&ctx(&c, &t));
        assert!(body.iter().any(|l| line_text(l) == "1 step"));
    }

    #[test]
    fn body_omits_missing_step_count_and_summary() {
        let t = Theme::hearth();
        // Only a name, from the summary fallback.
        let c = card("not json", "cleanup");
        let comp = WorkflowComponent;
        let body = comp.body(&ctx(&c, &t));
        assert_eq!(body.len(), 1);
        assert_eq!(line_text(&body[0]), "cleanup");
    }

    #[test]
    fn keys_offer_approve_always_deny_and_cancel() {
        let t = Theme::hearth();
        let c = card(r#"{"name":"x"}"#, "");
        let comp = WorkflowComponent;
        let keys = line_text(&comp.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"));
        assert!(keys.contains("always"));
        assert!(keys.contains("deny"));
        assert!(keys.contains("cancel"));
    }

    #[test]
    fn default_action_is_approve_once() {
        assert_eq!(
            WorkflowComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
