//! The Crucible permission component (Stage 4a).
//!
//! Projection for the `Crucible` tool: a council/scale header (`⚖`), a
//! `Crucible — cross-vendor council` title (with the focus lens appended
//! when present), and a body rendered from the typed
//! [`CruciblePlan`](wcore_types::crucible::CruciblePlan) snapshotted onto the
//! card by `protocol_bridge` at `ApprovalRequired` time.
//!
//! CRITICAL (Stage 2 contract): every number is read from the typed plan, not
//! re-parsed from prose, so the TUI shows exactly what the Assembler certified.
//! Money is rendered to 4 decimals (matching the CLI `render_card`), and an
//! unpriceable roster shows `price unknown` — NEVER `$0`. The component is pure
//! over [`PermissionContext`] — no I/O, no state — so it is unit-tested purely
//! on its title/body/keys text.
//!
//! Stage 4a is render + capture only. The `[e]` edit / `[p]` premium
//! affordances and the decision routing they drive land in a later stage; the
//! key row here is the no-charge `[Enter] run / [Esc] cancel` pair.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use wcore_types::crucible::{CouncilRole, MICROCENTS_PER_USD};

use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `Crucible` tool.
pub struct CrucibleComponent;

/// Render `microcents` as `$X.XXXX` (4 decimals, matching the CLI card).
fn usd(microcents: u64) -> String {
    format!("${:.4}", microcents as f64 / MICROCENTS_PER_USD)
}

impl PermissionComponent for CrucibleComponent {
    fn icon(&self) -> &'static str {
        // A balance/scale glyph for the cross-vendor council weighing answers.
        "⚖"
    }

    fn title(&self, ctx: &PermissionContext) -> Line<'static> {
        let mut text = "Crucible — cross-vendor council".to_string();
        if let Some(plan) = &ctx.card.crucible_plan
            && let Some(focus) = plan.focus.as_deref()
            && !focus.trim().is_empty()
        {
            text.push_str(&format!(" ({})", focus.trim()));
        }
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(ctx.theme.text)
                .add_modifier(Modifier::BOLD),
        ))
    }

    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>> {
        // Defensive: a Crucible card should always carry a plan, but if the
        // capture ever misses we render a single dim note rather than a blank
        // card or a crash.
        let Some(plan) = &ctx.card.crucible_plan else {
            return vec![Line::from(Span::styled(
                "Crucible council (plan unavailable)".to_string(),
                Style::default().fg(ctx.theme.text_muted),
            ))];
        };

        let mut lines: Vec<Line<'static>> = Vec::new();

        // One row per member: `<role>  <spec>  (<vendor>)`. The judge row is
        // brand-accented + bold so the independent cross-check reads at a glance.
        for member in &plan.members {
            let (role, is_judge) = match member.role {
                CouncilRole::Proposer => ("proposer", false),
                CouncilRole::Judge => ("judge", true),
            };
            let row = format!("{}  {}  ({})", role, member.spec, member.vendor);
            let style = if is_judge {
                Style::default()
                    .fg(ctx.theme.orange)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(ctx.theme.text)
            };
            lines.push(Line::from(Span::styled(row, style)));
        }

        // The certified ceiling — bold-emphasized like the CLI card. `None`
        // renders "price unknown", NEVER $0.
        let ceiling_line = match plan.ceiling_microcents {
            Some(mc) => format!("You're approving up to {}", usd(mc)),
            None => "ceiling: price unknown".to_string(),
        };
        lines.push(Line::from(Span::styled(
            ceiling_line,
            Style::default()
                .fg(ctx.theme.text)
                .add_modifier(Modifier::BOLD),
        )));

        // The single-model baseline, when priceable.
        if let Some(mc) = plan.single_model_baseline_microcents {
            lines.push(Line::from(Span::styled(
                format!("One model alone ≈ {}", usd(mc)),
                Style::default().fg(ctx.theme.text_muted),
            )));
        }

        // Judge independence — only meaningful when a council convenes.
        if plan.convene {
            let judge_line = if plan.judge_independent {
                "judge: independent (different vendor than every proposer)"
            } else {
                "judge: shares a proposer vendor"
            };
            let style = if plan.judge_independent {
                Style::default().fg(ctx.theme.text_muted)
            } else {
                Style::default().fg(ctx.theme.warning)
            };
            lines.push(Line::from(Span::styled(judge_line.to_string(), style)));
        }

        // The daily envelope — only when a cap genuinely aggregates. Spent
        // defaults to 0.0 when the running tally is absent (a fresh day).
        if let Some(cap_mc) = plan.day_cap_microcents {
            let spent = plan.day_spent_microcents.unwrap_or(0);
            lines.push(Line::from(Span::styled(
                format!("today: {} / {}", usd(spent), usd(cap_mc)),
                Style::default().fg(ctx.theme.text_muted),
            )));
        }

        // The Assembler's decision trace + any budget downshift steps.
        if !plan.reason.trim().is_empty() {
            lines.push(Line::from(Span::styled(
                format!("why: {}", plan.reason.trim()),
                Style::default().fg(ctx.theme.text_muted),
            )));
        }
        for trim in &plan.trims {
            lines.push(Line::from(Span::styled(
                trim.clone(),
                Style::default().fg(ctx.theme.text_muted),
            )));
        }

        lines
    }

    fn keys(&self, ctx: &PermissionContext) -> Line<'static> {
        Line::from(Span::styled(
            "[Enter] run    [Esc] cancel — no charge".to_string(),
            Style::default().fg(ctx.theme.text_muted),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::permission::ApprovalAction;
    use crate::tui::theme::Theme;
    use wcore_types::crucible::{CouncilMemberCard, CruciblePlan};

    /// Build a Crucible card carrying `plan`.
    fn card(plan: Option<CruciblePlan>) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "Crucible".into(),
            summary: String::new(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: String::new(),
            approval_reason: String::new(),
            plan_body: None,
            crucible_plan: plan,
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

    /// A two-member convening council: one proposer + an independent judge,
    /// priced ceiling + baseline, with a daily cap.
    fn convening_plan() -> CruciblePlan {
        CruciblePlan {
            convene: true,
            members: vec![
                CouncilMemberCard {
                    spec: "deepseek:deepseek-v4-pro".into(),
                    vendor: "deepseek".into(),
                    role: CouncilRole::Proposer,
                },
                CouncilMemberCard {
                    spec: "anthropic:claude-opus-4-8".into(),
                    vendor: "anthropic".into(),
                    role: CouncilRole::Judge,
                },
            ],
            stakes: "med".into(),
            focus: Some("c-suite".into()),
            ceiling_microcents: Some(210_000_000),
            single_model_baseline_microcents: Some(45_000_000),
            day_spent_microcents: None,
            day_cap_microcents: Some(2_000_000_000),
            judge_independent: true,
            reason: "diverse cross-vendor".into(),
            trims: vec![],
        }
    }

    #[test]
    fn icon_is_the_council_scale() {
        assert_eq!(CrucibleComponent.icon(), "⚖");
    }

    #[test]
    fn title_names_the_council_and_appends_focus() {
        let t = Theme::hearth();
        let c = card(Some(convening_plan()));
        let title = line_text(&CrucibleComponent.title(&ctx(&c, &t)));
        assert!(title.contains("cross-vendor council"), "title: {title}");
        assert!(title.contains("c-suite"), "focus lens missing: {title}");
    }

    #[test]
    fn body_shows_proposer_judge_ceiling_baseline_and_independence() {
        let t = Theme::hearth();
        let c = card(Some(convening_plan()));
        let body = CrucibleComponent.body(&ctx(&c, &t));
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");

        // Both a proposer row and the judge row render.
        assert!(joined.contains("proposer"), "no proposer row: {joined}");
        assert!(joined.contains("judge"), "no judge row: {joined}");
        assert!(
            joined.contains("deepseek:deepseek-v4-pro"),
            "proposer spec missing: {joined}"
        );
        assert!(
            joined.contains("anthropic:claude-opus-4-8"),
            "judge spec missing: {joined}"
        );

        // The ceiling line carries a `$` amount (4 decimals) — and the judge
        // row is the only one styled with the brand accent (bold).
        assert!(
            joined.contains("You're approving up to $2.1000"),
            "ceiling line: {joined}"
        );
        // The baseline comparison renders.
        assert!(
            joined.contains("One model alone ≈ $0.4500"),
            "baseline line: {joined}"
        );
        // Judge independence is asserted for the convening council.
        assert!(
            joined.contains("judge: independent"),
            "independence line missing: {joined}"
        );

        // The judge row (second member) is brand-accented.
        assert_eq!(body[1].spans[0].style.fg, Some(t.orange));
    }

    #[test]
    fn body_shows_price_unknown_and_never_zero_dollars() {
        let t = Theme::hearth();
        let mut plan = convening_plan();
        plan.ceiling_microcents = None;
        plan.single_model_baseline_microcents = None;
        // Clear the daily cap too: the "today: $0.0000 / …" spent line is a
        // legitimate $0, so leaving it would falsely trip the no-$0-ceiling
        // assertion below. This test is about the unpriceable ceiling only.
        plan.day_cap_microcents = None;
        let c = card(Some(plan));
        let body = CrucibleComponent.body(&ctx(&c, &t));
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(
            joined.contains("price unknown"),
            "unpriceable ceiling must say 'price unknown': {joined}"
        );
        // NEVER a phantom zero ceiling.
        assert!(!joined.contains("$0.0000"), "leaked a $0 ceiling: {joined}");
        assert!(
            !joined.contains("up to $0"),
            "leaked a $0 ceiling: {joined}"
        );
        // With no baseline price, the baseline line is omitted entirely.
        assert!(
            !joined.contains("One model alone"),
            "baseline line should be omitted when unpriceable: {joined}"
        );
    }

    #[test]
    fn body_today_line_appears_only_with_a_day_cap() {
        let t = Theme::hearth();

        // With a cap (and no running spend → 0.0), the today: line shows.
        let with_cap = convening_plan();
        let c = card(Some(with_cap));
        let body = CrucibleComponent.body(&ctx(&c, &t));
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            joined.contains("today: $0.0000 / $20.0000"),
            "today line with cap: {joined}"
        );

        // Without a cap, the today: line is omitted.
        let mut no_cap = convening_plan();
        no_cap.day_cap_microcents = None;
        let c = card(Some(no_cap));
        let body = CrucibleComponent.body(&ctx(&c, &t));
        let joined = body.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(
            !joined.contains("today:"),
            "today line must be omitted with no cap: {joined}"
        );
    }

    #[test]
    fn body_renders_unavailable_note_when_plan_is_none() {
        let t = Theme::hearth();
        let c = card(None);
        let body = CrucibleComponent.body(&ctx(&c, &t));
        assert_eq!(body.len(), 1);
        assert!(
            line_text(&body[0]).contains("plan unavailable"),
            "expected the defensive note"
        );
    }

    #[test]
    fn keys_offer_run_and_cancel_no_charge_without_edit_or_premium() {
        let t = Theme::hearth();
        let c = card(Some(convening_plan()));
        let keys = line_text(&CrucibleComponent.keys(&ctx(&c, &t)));
        assert!(keys.contains("run"), "keys: {keys}");
        assert!(keys.contains("cancel"), "keys: {keys}");
        assert!(keys.contains("no charge"), "keys: {keys}");
        // Stage 4a does NOT advertise edit/premium yet.
        assert!(
            !keys.contains("[e]"),
            "edit affordance is a later stage: {keys}"
        );
        assert!(
            !keys.contains("[p]"),
            "premium affordance is a later stage: {keys}"
        );
    }

    #[test]
    fn default_action_stays_approve_once() {
        assert_eq!(
            CrucibleComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
