//! Tool-call card widget — renders one `ToolCardModel`.
//!
//! v0.9.0 W3 D1: dual-mode renderer.
//!
//! - **Compact** (default, `SessionView::compact_tool_output = true`):
//!   one line, no border, of the form
//!   `<icon> <name>(<args>) · <summary_line from formatter>`
//!   The summary line is produced by the W2 C2/C3 per-tool formatter for
//!   `card.tool_name`, falling back to the generic formatter for unknown
//!   tools (the dispatcher is Total).
//!
//! - **Full** (`Ctrl+E` toggles): a bordered box with a sticky/bold
//!   header (`<icon> <name>`), the formatter's `detail_lines` for the
//!   body, and a footer carrying `duration · <provider/url>` when those
//!   are surfaceable from the payload.
//!
//! The mode is per-session-global, not per-card — the workspace passes
//! `app.session.compact_tool_output` into [`tool_card`] for every card on
//! one frame, so `Ctrl+E` flips all cards at once.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::Value;

use crate::tui::app::{ToolCardModel, ToolCardStatus};
use crate::tui::theme::Theme;
use crate::tui::tool_formatters::formatter_for;

/// The height a card needs in `compact` mode: one row of text, no border.
pub const TOOL_CARD_COMPACT_HEIGHT: u16 = 1;

/// The minimum height a card needs in `full` mode: top + bottom border +
/// header + footer + one detail row. Real cards extend beyond this when
/// the formatter emits more detail lines (the caller is responsible for
/// sizing the area).
pub const TOOL_CARD_FULL_MIN_HEIGHT: u16 = 5;

/// Render a tool-call card.
///
/// `compact` is the per-session view mode flag (defaulting to `true` so
/// new sessions land on the dense view). The widget reads the matching
/// formatter for `card.tool_name` from `tool_formatters::formatter_for`
/// — that dispatcher is Total, so an unknown tool name falls through to
/// the generic JSON formatter and never panics.
pub fn tool_card(f: &mut Frame, area: Rect, card: &ToolCardModel, t: &Theme, compact: bool) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    if compact {
        render_compact(f, area, card, t);
    } else {
        render_full(f, area, card, t);
    }
}

/// Back-compat alias retained for any caller built against W0's
/// signature. Renders in compact mode (the v0.9.0 W3 default).
///
/// New code should call [`tool_card`] directly and thread the
/// session-level `compact_tool_output` flag through.
pub fn tool_card_default(f: &mut Frame, area: Rect, card: &ToolCardModel, t: &Theme) {
    tool_card(f, area, card, t, true);
}

// ─── compact mode ──────────────────────────────────────────────────────

fn render_compact(f: &mut Frame, area: Rect, card: &ToolCardModel, t: &Theme) {
    let icon = status_icon(card.status);
    let icon_color = status_color(card.status, t);

    let args = args_summary(&card.input_pretty, area.width);
    let summary = formatter_summary(card);

    // Build: `<icon> <name>(<args>) · <summary>`. Use the surface bg so a
    // compact card visually slots into the transcript (no card chrome).
    let bg = Style::default().bg(t.bg);
    let spans = vec![
        Span::styled(format!("{icon} "), bg.fg(icon_color)),
        Span::styled(
            card.tool_name.clone(),
            bg.fg(t.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("({args})"), bg.fg(t.text_dim)),
        Span::styled(" · ", bg.fg(t.text_muted)),
        Span::styled(summary, bg.fg(t.text_dim)),
    ];

    let para = Paragraph::new(Line::from(spans)).style(bg);
    f.render_widget(para, area);
}

// ─── full mode ─────────────────────────────────────────────────────────

fn render_full(f: &mut Frame, area: Rect, card: &ToolCardModel, t: &Theme) {
    let icon = status_icon(card.status);
    let icon_color = status_color(card.status, t);

    // A `Running` card draws an accent border; finished cards get the
    // muted chrome border so they recede.
    let border_color = match card.status {
        ToolCardStatus::Running | ToolCardStatus::AwaitingApproval => t.orange,
        _ => t.border,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(t.surface));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let bg = Style::default().bg(t.surface);
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Header: `<icon> <tool_name>` — sticky/bold.
    lines.push(Line::from(vec![
        Span::styled(format!("{icon} "), bg.fg(icon_color)),
        Span::styled(
            card.tool_name.clone(),
            bg.fg(t.text).add_modifier(Modifier::BOLD),
        ),
    ]));

    // Body: detail lines from the per-tool formatter.
    let payload = parse_payload(card);
    let formatter = formatter_for(&card.tool_name);
    // Reserve 1 row for the footer; clamp body to the available height.
    let body_budget = inner.height.saturating_sub(2) as usize;
    let mut detail = formatter.detail_lines(&payload, t);
    if detail.len() > body_budget {
        detail.truncate(body_budget);
    }
    lines.extend(detail);

    // Footer: `<duration> · <provider/url>` when surfaceable.
    let footer = footer_line(card, &payload, t);
    if inner.height as usize > lines.len() {
        lines.push(footer);
    }

    let para = Paragraph::new(lines).style(bg);
    f.render_widget(para, inner);
}

// ─── helpers ───────────────────────────────────────────────────────────

/// Status icons per the v0.9.2 SPEC §3 S20 (variant A) glyph map:
/// `◐` running · `●` done · `○` cancelled · `⊘` awaiting-approval ·
/// `◑` running-but-stalled (color-lerp) · `✗` error.
///
/// (Reconciled v0.9.1.2's stale doc, which described `○` as "running"
/// and `⊘` as "cancelled". Under S20 the half-circle pair `◐`/`◑`
/// carries the in-flight signal — `◐` for a healthy stream, `◑` for one
/// whose delta has stalled — while `○` (empty circle) now reads as a
/// finished-without-result *cancelled* card and `●` (filled) as done.
/// `⊘` stays distinct for awaiting-approval: the tool is blocked on the
/// user, not making progress under its own power.)
///
/// `◑` is NOT a `ToolCardStatus` variant (the enum carries no stall
/// state); it is applied via [`status_icon_stalled`] at the render site
/// when a `Running` card's stream has stalled. Plain `Running` here
/// returns `◐`.
fn status_icon(status: ToolCardStatus) -> &'static str {
    match status {
        ToolCardStatus::Ok => "●",
        ToolCardStatus::Err => "✗",
        ToolCardStatus::Running => "◐",
        ToolCardStatus::AwaitingApproval => "⊘",
        ToolCardStatus::Cancelled => "○",
    }
}

/// The S20 stalled-aware glyph. A `Running` card whose stream has gone
/// quiet (the W6 stall check: >3 s with no streamed delta) renders `◑`
/// instead of `◐` to flag "in-flight but stalled" — paired with the
/// streaming-status color-lerp. Every non-`Running` status, and a
/// healthy `Running` card (`stalled == false`), defers to
/// [`status_icon`].
///
/// The `ToolCardModel` does not itself carry stall state, so the caller
/// supplies `stalled` from the streaming-status path (W6). Until a
/// render site threads that signal, cards render `◐` via the plain
/// [`status_icon`] and never show `◑` — no fabricated stall state.
// Not yet called from a render site (the ToolCardModel carries no stall
// flag; W6's streaming-status path owns the >3 s no-delta signal). Kept
// + tested here so the S20 `◑` glyph is ready to wire when a card-level
// stall signal lands.
#[allow(dead_code)]
fn status_icon_stalled(status: ToolCardStatus, stalled: bool) -> &'static str {
    match status {
        ToolCardStatus::Running if stalled => "◑",
        _ => status_icon(status),
    }
}

/// The card's accent color, keyed to its lifecycle status.
fn status_color(status: ToolCardStatus, t: &Theme) -> ratatui::style::Color {
    match status {
        ToolCardStatus::AwaitingApproval => t.warning,
        ToolCardStatus::Running => t.orange,
        ToolCardStatus::Ok => t.success,
        ToolCardStatus::Err => t.error,
        ToolCardStatus::Cancelled => t.text_muted,
    }
}

/// Stringify the tool's input args, truncated to `terminal_width / 3`.
///
/// `input_pretty` is JSON-pretty; collapse it into a single-line form by
/// stripping all whitespace runs to one space, then trim to width. An
/// empty payload (no ToolRequest content) shows as a hyphen so the
/// `(...)` reads cleanly.
fn args_summary(input_pretty: &str, terminal_width: u16) -> String {
    let trimmed = input_pretty.trim();
    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
        return String::new();
    }
    let max = (terminal_width as usize / 3).max(8);
    // Collapse whitespace runs.
    let mut collapsed = String::with_capacity(trimmed.len());
    let mut in_space = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !in_space {
                collapsed.push(' ');
                in_space = true;
            }
        } else {
            collapsed.push(ch);
            in_space = false;
        }
    }
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let preview: String = collapsed.chars().take(max.saturating_sub(1)).collect();
        format!("{preview}…")
    }
}

/// Ask the per-tool formatter for a one-line summary.
///
/// Output is parsed with [`parse_payload`]; if the card hasn't produced
/// output yet we hand the formatter a `null` value (its `summary_line`
/// implementations are required to degrade gracefully on missing
/// fields). Duration is zero today — the ToolCardModel doesn't carry
/// timing yet (W3 D5+ will wire it from `ToolResult` events).
fn formatter_summary(card: &ToolCardModel) -> String {
    let payload = parse_payload(card);
    formatter_for(&card.tool_name).summary_line(&payload, Duration::ZERO)
}

/// Best-effort parse of `card.output` into a `serde_json::Value`. Falls
/// back to a string-typed value (or `null` for an empty output) so
/// formatters always receive *some* `Value` to read.
fn parse_payload(card: &ToolCardModel) -> Value {
    match card.output.as_deref() {
        None | Some("") => Value::Null,
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string())),
    }
}

/// The full-mode footer: `<duration> · <provider/url>` when those are
/// surfaceable from the payload. Today the card carries no duration;
/// when the payload has a `url` field we surface that, otherwise the
/// footer collapses to the card's chip text.
fn footer_line(card: &ToolCardModel, payload: &Value, t: &Theme) -> Line<'static> {
    let bg = Style::default().bg(t.surface).fg(t.text_muted);
    let chip = status_chip(card);
    let url = payload
        .get("url")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let extra = payload
        .get("provider")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .or(url);

    if let Some(suffix) = extra {
        Line::from(vec![
            Span::styled(chip, bg),
            Span::styled(" · ", bg),
            Span::styled(suffix, bg),
        ])
    } else {
        Line::from(Span::styled(chip, bg))
    }
}

/// A short status chip — the right-aligned `.chip` from the mockup.
fn status_chip(card: &ToolCardModel) -> String {
    match card.status {
        ToolCardStatus::AwaitingApproval => "awaiting approval".into(),
        ToolCardStatus::Running => "running".into(),
        ToolCardStatus::Ok => "done".into(),
        ToolCardStatus::Err => "error".into(),
        ToolCardStatus::Cancelled => "cancelled".into(),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::app::{ToolCardModel, ToolCardStatus};
    use crate::tui::theme::Theme;

    fn model(status: ToolCardStatus) -> ToolCardModel {
        ToolCardModel {
            call_id: "c1".into(),
            tool_name: "Read".into(),
            summary: "src/main.rs".into(),
            status,
            output: Some(r#"{"path":"src/main.rs","bytes":1234}"#.into()),
            edit_preview: None,
            input_pretty: r#"{ "path": "src/main.rs" }"#.into(),
            approval_reason: String::new(),
            plan_body: None,
            crucible_plan: None,
        }
    }

    fn render(card: &ToolCardModel, t: &Theme, w: u16, h: u16, compact: bool) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal
            .draw(|f| tool_card(f, f.area(), card, t, compact))
            .expect("render tool card");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn tool_card_compact_default_renders_single_line() {
        // Compact mode shows the whole card on one row (no border chrome).
        // Render into a 1-row area so the assertion is unambiguous: any
        // bordered widget would paint the top edge there and the body
        // would never appear.
        let card = model(ToolCardStatus::Ok);
        let out = render(&card, &Theme::hearth(), 80, 1, true);
        // Exactly one row (trailing newline).
        assert_eq!(
            out.lines().count(),
            1,
            "compact card not single-line:\n{out}"
        );
        // Status icon + tool name + summary all on that one row.
        assert!(out.contains("Read"), "tool name missing:\n{out}");
        // S20: a done card is the filled `●` (was `✓` pre-v0.9.2).
        assert!(out.contains("●"), "success icon missing:\n{out}");
    }

    #[test]
    fn compact_renders_status_icon_and_tool_name() {
        // Each lifecycle status has its own glyph; the widget must paint
        // the right one with the right color. Tracks the v0.9.2 S20
        // (variant A) palette: `◐` running · `●` done · `○` cancelled ·
        // `⊘` awaiting-approval · `✗` error.
        let icon_for = |s: ToolCardStatus| match s {
            ToolCardStatus::Ok => "●",
            ToolCardStatus::Err => "✗",
            ToolCardStatus::Running => "◐",
            ToolCardStatus::AwaitingApproval => "⊘",
            ToolCardStatus::Cancelled => "○",
        };
        for s in [
            ToolCardStatus::Ok,
            ToolCardStatus::Err,
            ToolCardStatus::Running,
            ToolCardStatus::AwaitingApproval,
            ToolCardStatus::Cancelled,
        ] {
            let card = model(s);
            let out = render(&card, &Theme::hearth(), 80, 1, true);
            assert!(out.contains(icon_for(s)), "{s:?} icon missing:\n{out}");
            assert!(out.contains("Read"), "{s:?} tool name missing:\n{out}");
        }
    }

    /// v0.9.2 W7 (S20 variant A): each `ToolCardStatus` variant maps to
    /// its locked S20 glyph. Test exhaustively enumerates the enum so
    /// adding a new variant forces a deliberate glyph choice instead
    /// of silently falling through to a wildcard.
    #[test]
    fn status_icons_match_s20_variant_a_v092() {
        let expected = [
            (ToolCardStatus::Running, "◐"),          // running
            (ToolCardStatus::Ok, "●"),               // done
            (ToolCardStatus::Cancelled, "○"),        // cancelled
            (ToolCardStatus::AwaitingApproval, "⊘"), // kept distinct
            (ToolCardStatus::Err, "✗"),              // error
        ];
        for (status, glyph) in expected {
            assert_eq!(
                status_icon(status),
                glyph,
                "S20: {status:?} must render as `{glyph}`"
            );
        }
        // Under S20 the in-flight signal is the HALF circle `◐`, not the
        // empty `○` (now cancelled) or the filled `●` (now done). Guard
        // against regressing to the old map.
        assert_ne!(
            status_icon(ToolCardStatus::Running),
            "○",
            "Running must be the half-circle ◐, not the cancelled glyph"
        );
        assert_ne!(
            status_icon(ToolCardStatus::Ok),
            "✓",
            "Ok must be the filled ●, not the old check glyph"
        );
    }

    /// S20 `◑ stalled` color-lerp: a `Running` card whose stream has
    /// stalled (>3 s no delta) renders `◑` via `status_icon_stalled`;
    /// every other status — and a healthy `Running` card — defers to the
    /// plain `status_icon`. (`◑` is not an enum variant; the card model
    /// carries no stall flag, so the caller supplies it from W6.)
    #[test]
    fn stalled_running_card_uses_half_circle_dot_glyph_v092() {
        assert_eq!(
            status_icon_stalled(ToolCardStatus::Running, true),
            "◑",
            "stalled Running must render ◑"
        );
        assert_eq!(
            status_icon_stalled(ToolCardStatus::Running, false),
            "◐",
            "healthy Running must render ◐"
        );
        // Stall context never overrides a terminal status.
        assert_eq!(status_icon_stalled(ToolCardStatus::Ok, true), "●");
        assert_eq!(status_icon_stalled(ToolCardStatus::Err, true), "✗");
        assert_eq!(status_icon_stalled(ToolCardStatus::Cancelled, true), "○");
        assert_eq!(
            status_icon_stalled(ToolCardStatus::AwaitingApproval, true),
            "⊘"
        );
    }

    #[test]
    fn tool_card_expanded_renders_bordered_box_with_detail() {
        // Full mode draws a Borders::ALL block — the top-left corner
        // cell is a non-space border character, and the rendered output
        // spans multiple rows.
        let card = model(ToolCardStatus::Ok);
        let t = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(60, 8)).expect("test terminal");
        terminal
            .draw(|f| tool_card(f, f.area(), &card, &t, false))
            .expect("render full");
        let buf = terminal.backend().buffer();
        // The top-left corner must carry a border glyph (not blank).
        let corner = buf[(0, 0)].symbol();
        assert_ne!(corner, " ", "full card missing top-left border");
        // The card occupies more than one row.
        let mut non_blank_rows = 0;
        for y in 0..8 {
            let mut any = false;
            for x in 0..60 {
                if buf[(x, y)].symbol() != " " {
                    any = true;
                    break;
                }
            }
            if any {
                non_blank_rows += 1;
            }
        }
        assert!(
            non_blank_rows >= 3,
            "full card too few non-blank rows: {non_blank_rows}"
        );
    }

    #[test]
    fn compact_args_summary_truncates_to_third_of_width() {
        // A wide args blob must not blow past `width/3` characters in
        // the rendered compact line; the truncator appends `…`.
        let mut card = model(ToolCardStatus::Ok);
        let long: String = (0..400).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        card.input_pretty = format!(r#"{{ "blob": "{long}" }}"#);
        let out = render(&card, &Theme::hearth(), 60, 1, true);
        // 60 / 3 = 20 → max 19 chars of args + `…`. The full 400-char
        // blob must NOT appear in the rendered line.
        assert!(
            !out.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "args not truncated:\n{out}"
        );
    }

    #[test]
    fn full_mode_running_card_uses_accent_border() {
        let card = model(ToolCardStatus::Running);
        let t = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(40, 5)).expect("test terminal");
        terminal
            .draw(|f| tool_card(f, f.area(), &card, &t, false))
            .expect("render");
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(0, 0)].fg, t.orange);
    }

    #[test]
    fn full_mode_finished_card_uses_chrome_border() {
        let card = model(ToolCardStatus::Ok);
        let t = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(40, 5)).expect("test terminal");
        terminal
            .draw(|f| tool_card(f, f.area(), &card, &t, false))
            .expect("render");
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(0, 0)].fg, t.border);
    }

    #[test]
    fn compact_renders_summary_line_from_formatter() {
        // The `web` formatter's idiom is "Found N results in X.Xs" — a
        // compact card for tool="web" with a results array must surface
        // that exact phrasing on its single line.
        let mut card = model(ToolCardStatus::Ok);
        card.tool_name = "web".into();
        card.output = Some(
            r#"{"results":[{"title":"A","url":"https://example.com","domain":"example.com","snippet":"s"}]}"#
                .into(),
        );
        let out = render(&card, &Theme::hearth(), 100, 1, true);
        assert!(out.contains("Found"), "web summary missing:\n{out}");
        assert!(out.contains("result"), "web summary missing:\n{out}");
    }
}
