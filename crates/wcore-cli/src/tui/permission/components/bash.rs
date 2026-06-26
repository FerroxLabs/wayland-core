//! The Bash permission component (v0.9.2 W3, SPEC §2 #1 — the largest of
//! the Core 4).
//!
//! Pure projection of a pending `Bash` tool call into the inline approval
//! card. The command text rides on `card.summary` (the protocol bridge's
//! `summarize_args` copies the `command` arg there verbatim — redirections
//! NOT stripped, so this component strips trailing `> file` / `>> file`
//! itself so a filename never reads as part of the command).
//!
//! Special behaviors (SPEC §2 #1):
//!  * Editable always-allow prefix via §1D/W0. The shared
//!    [`infer_shell_prefix`] seeds the `[a] always for <prefix>` row; when
//!    the card is in prefix-edit mode (`ctx.editable_prefix.is_some()`) the
//!    body renders the editable buffer and the key row swaps to
//!    `[enter] commit prefix   [esc] back`. The commit goes through W0's
//!    `AlwaysPrefix` scope — never a category-`Always` (audit BLOCKER).
//!  * Destructive warning. [`destructive_warning`] paints a `theme.error`-
//!    bold line above the keys for `rm -rf`, `git push --force`, etc. — the
//!    real guard against a reflexive Enter (AGENTS §0 #3).
//!  * Sed-edit re-route. [`is_sed_edit`] flags `sed -i … <file>` so the
//!    body notes the file-edit intent instead of presenting the in-place
//!    `sed` as an opaque shell command.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::shell_common::{destructive_warning, infer_shell_prefix, is_sed_edit};
use crate::tui::permission::{PermissionComponent, PermissionContext};

/// Permission projection for the `Bash` shell tool.
pub struct BashComponent;

/// Strip a trailing `> file` / `>> file` redirection from the command
/// label so the destination filename never reads as part of the command
/// (CC `stripBashRedirections`). Only the *trailing* redirection is
/// dropped — a mid-pipeline `>` inside a quoted argument is left alone by
/// only matching the last whitespace-split `>`/`>>` token and everything
/// after it. The remaining command is trimmed of trailing whitespace.
fn strip_trailing_redirection(command: &str) -> &str {
    let trimmed = command.trim_end();
    // Find the last redirection operator that begins a token (preceded by
    // whitespace or at the start). Everything from there is the redirect.
    let bytes = trimmed.as_bytes();
    let mut cut: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'>' {
            // A `>` starts a redirection token only when it sits at a token
            // boundary (start of string or preceded by whitespace).
            let at_boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
            if at_boundary {
                cut = Some(i);
            }
        }
        i += 1;
    }
    match cut {
        Some(idx) => trimmed[..idx].trim_end(),
        None => trimmed,
    }
}

impl PermissionComponent for BashComponent {
    fn icon(&self) -> &'static str {
        "❯"
    }

    fn title(&self, _ctx: &PermissionContext) -> Line<'static> {
        Line::from(Span::styled(
            "Run a shell command",
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

        // Sed-edit re-route: an in-place `sed -i … <file>` is a file edit
        // wearing a shell command's clothes — note the intent rather than
        // present the raw `sed` as opaque (SPEC §2 #1 sed sub-case).
        if let Some(target) = is_sed_edit(command) {
            lines.push(Line::from(Span::styled(
                format!("in-place edit of {}", target.display()),
                Style::default()
                    .fg(ctx.theme.text)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                "this `sed -i` rewrites the file directly",
                Style::default().fg(ctx.theme.text_muted),
            )));
        }

        // The command, in `surface_hover`-bg code style, with any trailing
        // `> file` / `>> file` redirection stripped from the label.
        let label = strip_trailing_redirection(command);
        lines.push(Line::from(Span::styled(
            format!(" {label} "),
            Style::default()
                .fg(ctx.theme.text)
                .bg(ctx.theme.surface_hover),
        )));

        // Destructive-command warning above the keys (the dialog chrome
        // inserts the blank + key row after the body).
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
            tool_name: "Bash".into(),
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
        assert_eq!(BashComponent.icon(), "❯");
    }

    #[test]
    fn title_reads_run_a_shell_command() {
        let t = Theme::hearth();
        let c = card("ls -la");
        let title = line_text(&BashComponent.title(&ctx(&c, &t, None)));
        assert_eq!(title, "Run a shell command");
    }

    #[test]
    fn body_shows_the_command_text() {
        let t = Theme::hearth();
        let c = card("cargo test --lib");
        let body = BashComponent.body(&ctx(&c, &t, None));
        assert!(
            body_text(&body).contains("cargo test --lib"),
            "command missing from body: {}",
            body_text(&body)
        );
    }

    #[test]
    fn body_strips_trailing_redirection_from_the_label() {
        let t = Theme::hearth();
        let c = card("echo hi > out.txt");
        let body = BashComponent.body(&ctx(&c, &t, None));
        let text = body_text(&body);
        assert!(text.contains("echo hi"), "command head missing: {text}");
        assert!(
            !text.contains("out.txt"),
            "redirection target should be stripped: {text}"
        );
        // The append form `>>` is stripped too.
        let c2 = card("cat log >> all.log");
        let body2 = BashComponent.body(&ctx(&c2, &t, None));
        let text2 = body_text(&body2);
        assert!(text2.contains("cat log"), "command head missing: {text2}");
        assert!(
            !text2.contains("all.log"),
            "append target should be stripped: {text2}"
        );
    }

    #[test]
    fn destructive_rm_rf_shows_warning_line() {
        let t = Theme::hearth();
        let c = card("rm -rf /tmp/build");
        let body = BashComponent.body(&ctx(&c, &t, None));
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
    fn safe_command_has_no_warning_line() {
        let t = Theme::hearth();
        let c = card("cargo build");
        let body = BashComponent.body(&ctx(&c, &t, None));
        assert!(
            !body_text(&body).contains("destructive command"),
            "safe command must not warn: {}",
            body_text(&body)
        );
    }

    #[test]
    fn sed_inplace_edit_reroutes_to_file_edit_note() {
        let t = Theme::hearth();
        let c = card("sed -i s/a/b/ Cargo.toml");
        let body = BashComponent.body(&ctx(&c, &t, None));
        let text = body_text(&body);
        assert!(
            text.contains("in-place edit of Cargo.toml"),
            "expected sed file-edit note: {text}"
        );
    }

    #[test]
    fn normal_keys_show_always_for_inferred_prefix() {
        let t = Theme::hearth();
        let c = card("cargo test --lib");
        let keys = line_text(&BashComponent.keys(&ctx(&c, &t, None)));
        assert!(keys.contains("approve once"), "keys: {keys}");
        assert!(
            keys.contains("[a] always for cargo test "),
            "expected inferred prefix in keys: {keys}"
        );
        assert!(keys.contains("deny"), "keys: {keys}");
        assert!(keys.contains("cancel"), "keys: {keys}");
    }

    #[test]
    fn keys_omit_always_when_managed_rules_gate_closed() {
        let t = Theme::hearth();
        let c = card("cargo build");
        let mut context = ctx(&c, &t, None);
        context.always_allow_available = false;
        let keys = line_text(&BashComponent.keys(&context));
        assert!(keys.contains("approve once"), "keys: {keys}");
        assert!(!keys.contains("always"), "always must be hidden: {keys}");
        assert!(keys.contains("deny"), "keys: {keys}");
    }

    #[test]
    fn prefix_edit_mode_renders_the_buffer_and_swaps_keys() {
        let t = Theme::hearth();
        let c = card("cargo test --lib");
        let context = ctx(&c, &t, Some("cargo "));
        // Body shows the editable buffer, not the command.
        let body = BashComponent.body(&context);
        let text = body_text(&body);
        assert!(
            text.contains("always allow commands starting with"),
            "expected prefix-edit prompt: {text}"
        );
        assert!(
            text.contains("cargo "),
            "expected the buffer in body: {text}"
        );
        // Key row swaps to commit/back.
        let keys = line_text(&BashComponent.keys(&context));
        assert_eq!(keys, "[enter] commit prefix   [esc] back");
    }

    #[test]
    fn default_action_is_approve_once() {
        use crate::tui::permission::ApprovalAction;
        assert_eq!(BashComponent.default_action(), ApprovalAction::ApproveOnce);
    }
}
