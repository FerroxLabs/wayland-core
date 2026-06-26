//! The AskUserQuestion permission component (v0.9.2 W4, SPEC §2 #10).
//!
//! Unlike every other component this is NOT a yes/no approval — it is an
//! interactive Q&A. The agent asks a question with a set of answer choices;
//! the user arrow-picks one and Enter sends it back. The chosen answer
//! rides the *existing* approval envelope (`Approve { feedback: Some(..) }`)
//! so this component adds no new `SurfaceAction` variant — it only RENDERS
//! the question + the selectable choices list. The highlighted choice is
//! driven by `ctx.selected_choice`, which `workspace.rs` threads from the
//! live `approval_sel` index that the arrow keys move — so the rendered
//! marker tracks the keys (the v0.9.6 phantom-affordance fix).
//!
//! The question + choices come from `ctx.card.input_pretty`, the
//! pretty-printed JSON of the `AskUserQuestion` tool args (built by
//! `protocol_bridge::pretty_input`). The shape is inferred defensively
//! because CC's real `AskUserQuestion` request is the 81KB monster; v0.9.2
//! ships a clean minimal projection. We accept several plausible shapes:
//!
//!   - top-level `{ "question": "...", "choices": [...] }`
//!   - top-level `{ "question": "...", "options": [...] }`
//!   - CC-style `{ "questions": [ { "question"/"header": "...",
//!     "options": [...] } ] }` (first question only)
//!
//! Each choice is either a bare string, or an object carrying `label`
//! (preferred) / `header` for the choice text and an optional `description`
//! rendered dim beneath it. When no choices parse we fall back to a single
//! dim free-text note so the card is never blank and never a JSON wall.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// The selection-marker prefix on the highlighted choice. The non-selected
/// rows get an equal-width blank so the list stays left-aligned. The live
/// arrow-nav moves `ctx.selected_choice` (threaded from `workspace.rs`),
/// and `body()` paints the marker on that row.
const SELECTED_MARKER: &str = "▸ ";
const UNSELECTED_MARKER: &str = "  ";

/// One parsed answer choice: the picked text plus an optional dim note.
struct Choice {
    label: String,
    description: Option<String>,
}

/// Permission projection for the `AskUserQuestion` tool.
pub struct AskUserQuestionComponent;

impl AskUserQuestionComponent {
    /// Parse `ctx.card.input_pretty` into the question text and the choice
    /// list. Returns `(question, choices)`; either may be empty when the
    /// args do not parse, in which case the callers fall back gracefully.
    fn parse(ctx: &PermissionContext) -> (String, Vec<Choice>) {
        let raw = ctx.card.input_pretty.trim();
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            // Not JSON (or empty): nothing to project. The summary may still
            // carry a human preview, so offer it as the question.
            return (ctx.card.summary.trim().to_string(), Vec::new());
        };

        // The args object holding `question` + `choices`/`options`. For the
        // CC-style `{ "questions": [ {...} ] }` shape we descend into the
        // first question; otherwise we read the top-level object.
        let scope = value
            .get("questions")
            .and_then(Value::as_array)
            .and_then(|q| q.first())
            .unwrap_or(&value);

        let question = scope
            .get("question")
            .and_then(Value::as_str)
            .or_else(|| scope.get("header").and_then(Value::as_str))
            .or_else(|| scope.get("prompt").and_then(Value::as_str))
            .unwrap_or("")
            .trim()
            .to_string();

        let choices = scope
            .get("choices")
            .or_else(|| scope.get("options"))
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(Self::parse_choice).collect())
            .unwrap_or_default();

        (question, choices)
    }

    /// Project one JSON array element into a `Choice`. Strings map directly;
    /// objects prefer `label`, then `header`, then `value` for the picked
    /// text, and carry an optional `description`. Anything else is skipped.
    fn parse_choice(item: &Value) -> Option<Choice> {
        if let Some(s) = item.as_str() {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            return Some(Choice {
                label: s.to_string(),
                description: None,
            });
        }
        if let Some(obj) = item.as_object() {
            let label = obj
                .get("label")
                .and_then(Value::as_str)
                .or_else(|| obj.get("header").and_then(Value::as_str))
                .or_else(|| obj.get("value").and_then(Value::as_str))
                .unwrap_or("")
                .trim()
                .to_string();
            if label.is_empty() {
                return None;
            }
            let description = obj
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string);
            return Some(Choice { label, description });
        }
        None
    }
}

impl PermissionComponent for AskUserQuestionComponent {
    fn icon(&self) -> &'static str {
        "?"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let (question, _) = Self::parse(ctx);
        let text = if question.is_empty() {
            "Answer a question".to_string()
        } else {
            question
        };
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(ctx.theme.text)
                .add_modifier(Modifier::BOLD),
        ))
    }

    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>> {
        let (_, choices) = Self::parse(ctx);
        let mut lines: Vec<Line<'static>> = Vec::new();

        if choices.is_empty() {
            // No structured choices: a dim free-text note rather than a
            // blank card or a raw-JSON wall. (workspace.rs wires the
            // free-text entry; here we just signal the affordance.)
            lines.push(Line::from(Span::styled(
                "type your answer, then [enter] to send",
                Style::default().fg(ctx.theme.text_muted),
            )));
            return lines;
        }

        // Render each choice on its own line. The highlighted choice tracks
        // the live `ctx.selected_choice` index (moved by the workspace
        // arrow-nav and clamped to the choice count); the rest are
        // blank-prefixed so the labels stay column-aligned.
        let selected_idx = ctx.selected_choice.min(choices.len().saturating_sub(1));
        for (i, choice) in choices.iter().enumerate() {
            let selected = i == selected_idx;
            let marker = if selected {
                SELECTED_MARKER
            } else {
                UNSELECTED_MARKER
            };
            // The selected row is brand-accented + bold; the rest are plain
            // text so the highlighted answer reads at a glance.
            let label_style = if selected {
                Style::default()
                    .fg(ctx.theme.orange)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(ctx.theme.text)
            };
            lines.push(Line::from(vec![
                Span::styled(marker.to_string(), Style::default().fg(ctx.theme.orange)),
                Span::styled(choice.label.clone(), label_style),
            ]));
            // An optional dim description sits indented under its choice.
            if let Some(desc) = &choice.description {
                lines.push(Line::from(Span::styled(
                    format!("{UNSELECTED_MARKER}{desc}"),
                    Style::default().fg(ctx.theme.text_muted),
                )));
            }
        }
        lines
    }

    fn keys(&self, ctx: &PermissionContext) -> Line<'static> {
        let _ = ctx;
        // Q&A keys, NOT approve/deny: the user picks an answer.
        Line::from(Span::styled(
            "[↑/↓] choose   [enter] answer   [esc] cancel",
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

    fn card(input_pretty: &str) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "AskUserQuestion".into(),
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
        ctx_sel(c, t, 0)
    }

    /// Build a context with an explicit live selection index — the seam the
    /// arrow-nav drives. The render-reflection tests use this to prove the
    /// marker follows `selected_choice` (not a hardcoded row).
    fn ctx_sel<'a>(
        c: &'a ToolCardModel,
        t: &'a Theme,
        selected_choice: usize,
    ) -> PermissionContext<'a> {
        PermissionContext {
            card: c,
            theme: t,
            width: 80,
            always_allow_available: true,
            editable_prefix: None,
            selected_choice,
            expanded: false,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn icon_is_the_question_mark() {
        assert_eq!(AskUserQuestionComponent.icon(), "?");
    }

    #[test]
    fn title_is_the_question_text() {
        let t = Theme::hearth();
        let c = card(r#"{ "question": "Which database?", "choices": ["Postgres", "SQLite"] }"#);
        let comp = AskUserQuestionComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Which database?");
    }

    #[test]
    fn title_falls_back_when_no_question_parses() {
        let t = Theme::hearth();
        let c = card("not json at all");
        let comp = AskUserQuestionComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Answer a question");
    }

    #[test]
    fn title_reads_cc_style_questions_array() {
        let t = Theme::hearth();
        let c = card(
            r#"{ "questions": [ { "header": "Pick a deploy target", "options": ["staging", "prod"] } ] }"#,
        );
        let comp = AskUserQuestionComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Pick a deploy target");
    }

    #[test]
    fn body_lists_string_choices_each_on_its_own_line() {
        let t = Theme::hearth();
        let c = card(r#"{ "question": "Which?", "choices": ["Alpha", "Beta", "Gamma"] }"#);
        let comp = AskUserQuestionComponent;
        let body = comp.body(&ctx(&c, &t));
        assert_eq!(body.len(), 3);
        assert!(line_text(&body[0]).contains("Alpha"));
        assert!(line_text(&body[1]).contains("Beta"));
        assert!(line_text(&body[2]).contains("Gamma"));
    }

    #[test]
    fn body_marks_the_first_choice_as_selected() {
        let t = Theme::hearth();
        let c = card(r#"{ "question": "Which?", "choices": ["Alpha", "Beta"] }"#);
        let comp = AskUserQuestionComponent;
        let body = comp.body(&ctx(&c, &t));
        // First row carries the selection marker; second does not.
        assert!(line_text(&body[0]).starts_with(SELECTED_MARKER));
        assert!(line_text(&body[1]).starts_with(UNSELECTED_MARKER));
        assert!(!line_text(&body[1]).starts_with(SELECTED_MARKER));
        // The selected label is brand-accented (orange).
        assert_eq!(body[0].spans[1].style.fg, Some(t.orange));
    }

    #[test]
    fn body_marker_follows_the_live_selected_choice_index() {
        // THE regression test for the v0.9.6 phantom-affordance bug: the
        // rendered marker MUST track `ctx.selected_choice`, not a hardcoded
        // row 0. The old code asserted only that `approval_sel` moved; it
        // never asserted the painted marker moved — so a render that ignored
        // the index passed CI while the arrow keys looked dead on screen.
        let t = Theme::hearth();
        let c = card(r#"{ "question": "Which?", "choices": ["Alpha", "Beta", "Gamma"] }"#);
        let comp = AskUserQuestionComponent;

        // selected_choice = 1 → Beta is marked, Alpha/Gamma are not.
        let body = comp.body(&ctx_sel(&c, &t, 1));
        assert!(
            line_text(&body[1]).starts_with(SELECTED_MARKER),
            "the marker must move to the live selected row (Beta)"
        );
        assert!(!line_text(&body[0]).starts_with(SELECTED_MARKER));
        assert!(!line_text(&body[2]).starts_with(SELECTED_MARKER));
        assert_eq!(body[1].spans[1].style.fg, Some(t.orange));

        // selected_choice = 2 → Gamma is marked.
        let body = comp.body(&ctx_sel(&c, &t, 2));
        assert!(line_text(&body[2]).starts_with(SELECTED_MARKER));
        assert!(!line_text(&body[0]).starts_with(SELECTED_MARKER));

        // Out-of-range index clamps to the last row rather than vanishing.
        let body = comp.body(&ctx_sel(&c, &t, 99));
        assert!(
            line_text(&body[2]).starts_with(SELECTED_MARKER),
            "an over-large index clamps to the last choice, never an unmarked list"
        );
    }

    #[test]
    fn body_reads_object_choices_with_label_and_description() {
        let t = Theme::hearth();
        let c = card(
            r#"{
                "question": "Which framework?",
                "options": [
                    { "label": "React", "description": "component model" },
                    { "label": "Svelte" }
                ]
            }"#,
        );
        let comp = AskUserQuestionComponent;
        let body = comp.body(&ctx(&c, &t));
        // React label + its description line + Svelte label = 3 lines.
        assert_eq!(body.len(), 3);
        assert!(line_text(&body[0]).contains("React"));
        assert_eq!(line_text(&body[1]).trim(), "component model");
        // The description is dim/muted, not primary text.
        assert_eq!(body[1].spans[0].style.fg, Some(t.text_muted));
        assert!(line_text(&body[2]).contains("Svelte"));
    }

    #[test]
    fn body_prefers_label_then_header_then_value_for_object_choices() {
        let t = Theme::hearth();
        let c = card(
            r#"{ "question": "q", "choices": [ { "header": "from-header" }, { "value": "from-value" } ] }"#,
        );
        let comp = AskUserQuestionComponent;
        let body = comp.body(&ctx(&c, &t));
        assert!(line_text(&body[0]).contains("from-header"));
        assert!(line_text(&body[1]).contains("from-value"));
    }

    #[test]
    fn body_falls_back_to_free_text_note_when_no_choices() {
        let t = Theme::hearth();
        let c = card(r#"{ "question": "What is your name?" }"#);
        let comp = AskUserQuestionComponent;
        let body = comp.body(&ctx(&c, &t));
        assert_eq!(body.len(), 1);
        assert!(line_text(&body[0]).contains("type your answer"));
        // The note is muted, never the primary color.
        assert_eq!(body[0].spans[0].style.fg, Some(t.text_muted));
    }

    #[test]
    fn body_skips_empty_and_non_string_object_choices() {
        let t = Theme::hearth();
        let c = card(r#"{ "question": "q", "choices": ["", "Keep", { "nope": 1 }, 42] }"#);
        let comp = AskUserQuestionComponent;
        let body = comp.body(&ctx(&c, &t));
        // Only "Keep" survives.
        assert_eq!(body.len(), 1);
        assert!(line_text(&body[0]).contains("Keep"));
    }

    #[test]
    fn body_never_dumps_raw_json() {
        let t = Theme::hearth();
        let c = card(r#"{ "question": "q", "choices": ["A", "B"] }"#);
        let comp = AskUserQuestionComponent;
        let body = comp.body(&ctx(&c, &t));
        for line in &body {
            let text = line_text(line);
            assert!(!text.contains('{'), "body leaked JSON: {text}");
            assert!(!text.contains("\":"), "body leaked JSON: {text}");
        }
    }

    #[test]
    fn keys_offer_arrow_pick_answer_and_cancel() {
        let t = Theme::hearth();
        let c = card(r#"{ "question": "q", "choices": ["A"] }"#);
        let comp = AskUserQuestionComponent;
        let keys = line_text(&comp.keys(&ctx(&c, &t)));
        assert!(keys.contains("choose"));
        assert!(keys.contains("answer"));
        assert!(keys.contains("cancel"));
        // Arrow affordance present; this is NOT a yes/no approve card.
        assert!(keys.contains('↑') && keys.contains('↓'));
        assert!(!keys.contains("approve"));
        assert!(!keys.contains("deny"));
    }

    #[test]
    fn default_action_stays_approve_once() {
        // The chosen answer rides the existing approval envelope — no new
        // SurfaceAction variant, so the default action is unchanged.
        assert_eq!(
            AskUserQuestionComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
