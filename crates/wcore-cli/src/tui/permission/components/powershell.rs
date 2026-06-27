//! The PowerShell permission component (v0.9.2 W3, SPEC §2 #13 — the Bash
//! variant of the Core 4 shells).
//!
//! Pure projection of a pending `PowerShell` tool call into the inline
//! approval card. Structurally mirrors [`BashComponent`](super::bash): the
//! command text rides on `card.summary` (the protocol bridge copies the
//! `command` arg there verbatim) and the same shared `shell_common`
//! helpers (`infer_shell_prefix`, `destructive_warning`) drive the
//! always-allow prefix and the danger guard.
//!
//! PowerShell-specific framing:
//!  * Icon `❯` (the shell-prompt glyph, shared with Bash) and the title
//!    `Run a PowerShell command`.
//!  * No POSIX `> file` redirection stripping or `sed -i` re-route — those
//!    are Bash-shell idioms. PowerShell cmdlets (`Remove-Item`,
//!    `Format-Volume`) carry their own destructive surface, flagged by the
//!    shared [`destructive_warning`].
//!
//! Special behaviors (SPEC §2 #13, shared with Bash):
//!  * Editable always-allow prefix via §1D/W0. [`infer_shell_prefix`] seeds
//!    the `[a] always for <prefix>` row; when the card is in prefix-edit
//!    mode (`ctx.editable_prefix.is_some()`) the body renders the editable
//!    buffer and the key row swaps to `[enter] commit prefix   [esc] back`.
//!    The commit goes through W0's `AlwaysPrefix` scope (the workspace
//!    sub-mode gates on `Bash|PowerShell` per S-W3a) — never a
//!    category-`Always` (audit BLOCKER).
//!  * Destructive warning. [`destructive_warning`] paints a `theme.error`-
//!    bold line above the keys for `Remove-Item -Recurse -Force`,
//!    `Format-Volume`, etc. — the real guard against a reflexive Enter
//!    (AGENTS §0 #3).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::shell_common::{destructive_warning, infer_shell_prefix};
use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `PowerShell` shell tool.
pub struct PowerShellComponent;

impl PermissionComponent for PowerShellComponent {
    fn icon(&self) -> &'static str {
        "❯"
    }

    fn title(&self, _ctx: &PermissionContext) -> Line<'static> {
        Line::from(Span::styled(
            "Run a PowerShell command",
            Style::default()
                .fg(_ctx.theme.text)
                .add_modifier(Modifier::BOLD),
        ))
    }

    fn body(&self, ctx: &PermissionContext) -> Vec<Line<'static>> {
        let command = ctx.card.summary.trim();
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Prefix-edit mode: the body is the editable always-allow buffer,
        // not the command. The buffer goes through W0's `AlwaysPrefix`
        // scope on commit (SAFETY: never category-`Always`).
        if let Some(prefix) = ctx.editable_prefix {
            lines.push(Line::from(Span::styled(
                "always allow commands starting with:",
                Style::default().fg(ctx.theme.text_muted),
            )));
            // The live buffer, in code style with a trailing cursor block so
            // the user sees an editable input rather than static text.
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {prefix}"),
                    Style::default()
                        .fg(ctx.theme.text)
                        .bg(ctx.theme.surface_hover),
                ),
                Span::styled(
                    "█",
                    Style::default()
                        .fg(ctx.theme.orange)
                        .bg(ctx.theme.surface_hover),
                ),
            ]));
            return lines;
        }

        // The command, in `surface_hover`-bg code style. PowerShell does not
        // use the POSIX `> file` trailing-redirection idiom, so the command
        // is shown verbatim.
        lines.push(Line::from(Span::styled(
            format!(" {command} "),
            Style::default()
                .fg(ctx.theme.text)
                .bg(ctx.theme.surface_hover),
        )));

        // Destructive-command warning above the keys (the dialog chrome
        // inserts the blank + key row after the body). Covers the PowerShell
        // cmdlet hazards (`Remove-Item -Recurse -Force`, `Format-Volume`)
        // via the shared `shell_common::destructive_warning`.
        if destructive_warning(command).is_some() {
            lines.push(Line::from(Span::styled(
                "destructive command — review carefully before approving",
                Style::default()
                    .fg(ctx.theme.error)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        lines
    }

    fn keys(&self, ctx: &PermissionContext) -> Line<'static> {
        // Prefix-edit mode: the row commits or backs out of the buffer.
        if ctx.editable_prefix.is_some() {
            return Line::from("[enter] commit prefix   [esc] back");
        }

        // Normal mode. The `[a] always` affordance carries the inferred
        // prefix so the user sees the scope a category-allow would cover
        // BEFORE entering edit mode — and is suppressed entirely when the
        // managed-rules gate is closed (`always_allow_available`).
        if ctx.always_allow_available {
            let prefix = infer_shell_prefix(ctx.card.summary.trim());
            Line::from(format!(
                "[enter/y] approve once   [a] always for {prefix}  [n] deny   [esc] cancel"
            ))
        } else {
            Line::from("[enter/y] approve once   [n] deny   [esc] cancel")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::theme::Theme;

    fn card(command: &str) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "PowerShell".into(),
            summary: command.into(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: String::new(),
            approval_reason: String::new(),
            plan_body: None,
            crucible_plan: None,
        }
    }

    fn ctx<'a>(
        c: &'a ToolCardModel,
        t: &'a Theme,
        editable_prefix: Option<&'a str>,
    ) -> PermissionContext<'a> {
        PermissionContext {
            card: c,
            theme: t,
            width: 80,
            always_allow_available: true,
            editable_prefix,
            selected_choice: 0,
            expanded: false,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn body_text(lines: &[Line<'_>]) -> String {
        lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn icon_is_the_shell_prompt() {
        assert_eq!(PowerShellComponent.icon(), "❯");
    }

    #[test]
    fn title_reads_run_a_powershell_command() {
        let t = Theme::hearth();
        let c = card("Get-ChildItem");
        let title = line_text(&PowerShellComponent.title(&ctx(&c, &t, None)));
        assert_eq!(title, "Run a PowerShell command");
    }

    #[test]
    fn body_shows_the_command_text() {
        let t = Theme::hearth();
        let c = card("Get-Process | Sort-Object CPU");
        let body = PowerShellComponent.body(&ctx(&c, &t, None));
        assert!(
            body_text(&body).contains("Get-Process | Sort-Object CPU"),
            "command missing from body: {}",
            body_text(&body)
        );
    }

    #[test]
    fn destructive_remove_item_recurse_force_shows_warning_line() {
        let t = Theme::hearth();
        let c = card("Remove-Item -Recurse -Force C:\\tmp\\build");
        let body = PowerShellComponent.body(&ctx(&c, &t, None));
        assert!(
            body_text(&body).contains("destructive command"),
            "expected destructive warning: {}",
            body_text(&body)
        );
        // The warning is painted in the error color (not a plain dim note).
        let warn = body
            .iter()
            .find(|l| line_text(l).contains("destructive command"))
            .expect("warning line present");
        assert_eq!(warn.spans[0].style.fg, Some(t.error));
    }

    #[test]
    fn destructive_format_volume_shows_warning_line() {
        let t = Theme::hearth();
        let c = card("Format-Volume -DriveLetter D");
        let body = PowerShellComponent.body(&ctx(&c, &t, None));
        assert!(
            body_text(&body).contains("destructive command"),
            "expected destructive warning: {}",
            body_text(&body)
        );
    }

    #[test]
    fn safe_command_has_no_warning_line() {
        let t = Theme::hearth();
        let c = card("Get-ChildItem");
        let body = PowerShellComponent.body(&ctx(&c, &t, None));
        assert!(
            !body_text(&body).contains("destructive command"),
            "safe command must not warn: {}",
            body_text(&body)
        );
    }

    #[test]
    fn normal_keys_show_always_for_inferred_prefix() {
        let t = Theme::hearth();
        let c = card("Get-ChildItem -Path .");
        let keys = line_text(&PowerShellComponent.keys(&ctx(&c, &t, None)));
        assert!(keys.contains("approve once"), "keys: {keys}");
        assert!(
            keys.contains("[a] always for Get-ChildItem "),
            "expected inferred prefix in keys: {keys}"
        );
        assert!(keys.contains("deny"), "keys: {keys}");
        assert!(keys.contains("cancel"), "keys: {keys}");
    }

    #[test]
    fn keys_omit_always_when_managed_rules_gate_closed() {
        let t = Theme::hearth();
        let c = card("Get-ChildItem");
        let mut context = ctx(&c, &t, None);
        context.always_allow_available = false;
        let keys = line_text(&PowerShellComponent.keys(&context));
        assert!(keys.contains("approve once"), "keys: {keys}");
        assert!(!keys.contains("always"), "always must be hidden: {keys}");
        assert!(keys.contains("deny"), "keys: {keys}");
    }

    #[test]
    fn prefix_edit_mode_renders_the_buffer_and_swaps_keys() {
        let t = Theme::hearth();
        let c = card("Get-ChildItem -Path .");
        let context = ctx(&c, &t, Some("Get-ChildItem "));
        // Body shows the editable buffer, not the command.
        let body = PowerShellComponent.body(&context);
        let text = body_text(&body);
        assert!(
            text.contains("always allow commands starting with"),
            "expected prefix-edit prompt: {text}"
        );
        assert!(
            text.contains("Get-ChildItem "),
            "expected the buffer in body: {text}"
        );
        // Key row swaps to commit/back.
        let keys = line_text(&PowerShellComponent.keys(&context));
        assert_eq!(keys, "[enter] commit prefix   [esc] back");
    }

    #[test]
    fn default_action_is_approve_once() {
        use crate::tui::permission::ApprovalAction;
        assert_eq!(
            PowerShellComponent.default_action(),
            ApprovalAction::ApproveOnce
        );
    }
}
