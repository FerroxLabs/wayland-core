//! The Skill permission component (v0.9.2 W4, SPEC §2 #9).
//!
//! The `Skill` tool runs a named slash-skill (a packaged prompt/workflow),
//! optionally namespaced as `plugin:skill`. The card frames it as
//! `Run skill {skill_name}` and shows the skill name plus the invocation
//! summary in the body. When the name is namespaced, a dim
//! `skill from {plugin}` line tells the user which plugin the skill ships
//! with.
//!
//! The skill name and invocation come from the card. `ctx.card.summary`
//! carries the salient field (the skill `name`/`command`); the
//! pretty-printed args (`input_pretty`) are the fallback when the summary
//! is empty, so the body and title are never blank.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `Skill` tool.
pub struct SkillComponent;

impl SkillComponent {
    /// The invocation string the card carries — the skill name (possibly
    /// `plugin:skill`) and any inline args. Falls back to the pretty args
    /// when the summary is empty so the card is never blank.
    fn invocation(ctx: &PermissionContext) -> String {
        let summary = ctx.card.summary.trim();
        if !summary.is_empty() {
            return summary.to_string();
        }
        ctx.card.input_pretty.trim().to_string()
    }

    /// The skill name — the first whitespace-delimited token of the
    /// invocation. The Skill tool's name field is the leading token
    /// (`plugin:skill` or a bare `skill`); trailing args, if any, follow a
    /// space. Empty invocations yield an empty name (the title degrades to
    /// `Run skill`).
    fn skill_name(ctx: &PermissionContext) -> String {
        Self::invocation(ctx)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// The plugin segment of a namespaced `plugin:skill` name, or `None`
    /// for a bare skill name. Only a single leading `plugin:` qualifies;
    /// the plugin segment must be non-empty.
    fn plugin(name: &str) -> Option<String> {
        let (plugin, rest) = name.split_once(':')?;
        if plugin.is_empty() || rest.is_empty() {
            return None;
        }
        Some(plugin.to_string())
    }
}

impl PermissionComponent for SkillComponent {
    fn icon(&self) -> &'static str {
        "◆"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let name = Self::skill_name(ctx);
        let text = if name.is_empty() {
            "Run skill".to_string()
        } else {
            format!("Run skill {name}")
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
        let name = Self::skill_name(ctx);
        let invocation = Self::invocation(ctx);

        // The skill name in primary text — the headline of what runs.
        if !name.is_empty() {
            lines.push(Line::from(Span::styled(
                name.clone(),
                Style::default().fg(ctx.theme.text),
            )));
        }

        // The full invocation summary in the shared "code" style (dim fg on
        // the hover surface), matching the other arg-rendering cards.
        if !invocation.is_empty() {
            lines.push(Line::from(Span::styled(
                invocation,
                Style::default()
                    .fg(ctx.theme.text_dim)
                    .bg(ctx.theme.surface_hover),
            )));
        }

        // A dim provenance line when the skill is plugin-namespaced
        // (`plugin:skill`), so the user sees where the skill comes from.
        if let Some(plugin) = Self::plugin(&name) {
            lines.push(Line::from(Span::styled(
                format!("skill from {plugin}"),
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

    fn card(summary: &str) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "Skill".into(),
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
    fn icon_is_the_diamond() {
        assert_eq!(SkillComponent.icon(), "◆");
    }

    #[test]
    fn title_carries_the_skill_name() {
        let t = Theme::hearth();
        let c = card("brainstorming");
        let comp = SkillComponent;
        assert_eq!(
            line_text(&comp.title(&ctx(&c, &t))),
            "Run skill brainstorming"
        );
    }

    #[test]
    fn title_uses_the_leading_token_when_invocation_has_args() {
        let t = Theme::hearth();
        let c = card("commit --amend off");
        let comp = SkillComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Run skill commit");
    }

    #[test]
    fn title_carries_the_namespaced_skill_name() {
        let t = Theme::hearth();
        let c = card("vercel:deploy prod");
        let comp = SkillComponent;
        assert_eq!(
            line_text(&comp.title(&ctx(&c, &t))),
            "Run skill vercel:deploy"
        );
    }

    #[test]
    fn body_shows_the_name_and_the_invocation_summary() {
        let t = Theme::hearth();
        let c = card("commit --push");
        let comp = SkillComponent;
        let body = comp.body(&ctx(&c, &t));
        // name line + invocation line (bare name → no plugin line).
        assert_eq!(body.len(), 2);
        assert_eq!(line_text(&body[0]), "commit");
        assert_eq!(line_text(&body[1]), "commit --push");
        // The invocation renders in the shared code style.
        assert_eq!(body[1].spans[0].style.bg, Some(t.surface_hover));
    }

    #[test]
    fn namespaced_name_shows_a_dim_plugin_provenance_line() {
        let t = Theme::hearth();
        let c = card("vercel:deploy prod");
        let comp = SkillComponent;
        let body = comp.body(&ctx(&c, &t));
        // name line + invocation line + `skill from vercel`.
        assert_eq!(body.len(), 3);
        assert_eq!(line_text(&body[0]), "vercel:deploy");
        assert_eq!(line_text(&body[1]), "vercel:deploy prod");
        assert_eq!(line_text(&body[2]), "skill from vercel");
        // The provenance line is muted/dim, not primary text.
        assert_eq!(body[2].spans[0].style.fg, Some(t.text_muted));
    }

    #[test]
    fn bare_skill_name_carries_no_plugin_line() {
        let t = Theme::hearth();
        let c = card("brainstorming");
        let comp = SkillComponent;
        let body = comp.body(&ctx(&c, &t));
        // name + invocation, no provenance line.
        assert_eq!(body.len(), 2);
        for line in &body {
            assert!(!line_text(line).contains("skill from"));
        }
    }

    #[test]
    fn invocation_falls_back_to_input_pretty_when_summary_empty() {
        let t = Theme::hearth();
        let mut c = card("");
        c.input_pretty = "code-review:review-pr 1234".into();
        let comp = SkillComponent;
        assert_eq!(
            line_text(&comp.title(&ctx(&c, &t))),
            "Run skill code-review:review-pr"
        );
        let body = comp.body(&ctx(&c, &t));
        assert_eq!(line_text(&body[2]), "skill from code-review");
    }

    #[test]
    fn empty_invocation_title_degrades_gracefully() {
        let t = Theme::hearth();
        let c = card("");
        let comp = SkillComponent;
        assert_eq!(line_text(&comp.title(&ctx(&c, &t))), "Run skill");
        // No name, no invocation, no plugin line — an empty body.
        assert!(comp.body(&ctx(&c, &t)).is_empty());
    }

    #[test]
    fn keys_offer_approve_always_deny_and_cancel() {
        let t = Theme::hearth();
        let c = card("brainstorming");
        let comp = SkillComponent;
        let keys = line_text(&comp.keys(&ctx(&c, &t)));
        assert!(keys.contains("approve"));
        assert!(keys.contains("always"));
        assert!(keys.contains("deny"));
        assert!(keys.contains("cancel"));
    }

    #[test]
    fn default_action_is_approve_once() {
        assert_eq!(SkillComponent.default_action(), ApprovalAction::ApproveOnce);
    }
}
