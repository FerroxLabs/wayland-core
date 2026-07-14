//! Status-bar widget — the single bottom chrome row.
//!
//! v0.9.2 W5 (SPEC §3 S17 variant D + S18): the dense curated default
//! carries `model │ mode │ ctx-bar │ $cost │ duration`. Cost is
//! always-visible (IJFW discipline) and the context bar is a 4-threshold
//! gauge with 9-level sub-character fill (§3 S18). CPU/RAM stay out of the
//! chrome — those are dev info, gated to `Diagnostics`.
//!
//! The model identity drops a `vendor/` prefix (`anthropic/claude-opus-4-7`
//! renders as `claude-opus-4-7`) so the bar reads clean.
//!
//! ## statusLine override (SPEC §6)
//!
//! When the user has configured `statusLine.command`, the bar renders the
//! command's last-good cached output as the WHOLE row instead of the
//! curated default. The command is NEVER executed here — that would freeze
//! every frame. A background sampler ([`crate::tui::statusline`]) runs it
//! off-thread and publishes a sanitized one-line string into a shared
//! cache; this widget only READS that string as plain data. See §6 for the
//! security boundary (settings-file-only, no model write path).
//!
//! ## Toast (SPEC §3 S7)
//!
//! Demoted status events (e.g. `McpReady`) are surfaced as a transient
//! toast in this row, NOT as a transcript turn. The toast is EMITTED
//! elsewhere (the protocol bridge sets `App.toast`/`App.toast_at`); this
//! widget only RENDERS it and auto-dismisses it once `TOAST_DWELL` has
//! elapsed.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::App;
use crate::tui::theme::Theme;
use crate::tui::widgets::SystemSample;

/// Number of cells in the context-usage bar. Twenty cells × the 9-level
/// sub-char fill gives ~160 resolution steps across the bar.
const CTX_CELLS: usize = 20;

/// Sub-character fill glyphs — 9 levels of horizontal block from empty
/// (a space) to full (`█`). The partially-filled cell at the fill
/// boundary renders the matching fractional glyph.
const SUBCELL: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

/// How long a toast stays on screen before it auto-dismisses. The status
/// bar stops drawing it once `App.toast_at` is older than this; the run loop
/// clears the field through `App::dismiss_expired_toast` (same dwell) so the
/// revision bumps and the redraw-skip repaints promptly (audit M2).
pub(crate) const TOAST_DWELL: std::time::Duration = std::time::Duration::from_secs(2);

/// Below this width the session-cost and duration readouts are dropped,
/// leaving just the model + mode + context bar.
const COST_SEGMENT_MIN_WIDTH: u16 = 64;

/// Strip a `vendor/` prefix from a model slug so the bar reads clean.
/// `anthropic/claude-opus-4-7` → `claude-opus-4-7`.
fn display_model(model: &str) -> &str {
    model.split_once('/').map(|(_, m)| m).unwrap_or(model)
}

/// Color the context gauge by four thresholds (S18 variant A):
/// green `<50%`, yellow `<80%`, orange `<95%`, red `>=95%`.
pub fn ctx_bar_color(pct: f64, t: &Theme) -> Color {
    if pct >= 0.95 {
        t.error
    } else if pct >= 0.80 {
        t.orange
    } else if pct >= 0.50 {
        t.warning
    } else {
        t.success
    }
}

/// Render a `cells`-wide context bar for fraction `pct` (`0.0..=1.0`) with
/// sub-character fractional fill. Each cell is divided into eighths; the
/// cell straddling the fill boundary renders the matching [`SUBCELL`]
/// glyph, cells before it render full (`█`), cells after it render empty.
/// The whole bar is painted in the threshold color from
/// [`ctx_bar_color`]. Always returns exactly `cells` spans.
pub fn ctx_bar(pct: f64, cells: usize, t: &Theme) -> Vec<Span<'static>> {
    let color = ctx_bar_color(pct, t);
    let total_eighths = (pct.clamp(0.0, 1.0) * cells as f64 * 8.0).round() as usize;
    (0..cells)
        .map(|i| {
            // Eighths of fill that land in cell `i` (0..=8).
            let cell_eighths = total_eighths.saturating_sub(i * 8).min(8);
            let glyph = SUBCELL[cell_eighths];
            // Empty cells: render the boundary glyph in the dim chrome
            // token so the unfilled track is still visible; filled cells
            // (any fill) carry the threshold color.
            let fg = if cell_eighths == 0 {
                t.surface_hover
            } else {
                color
            };
            Span::styled(glyph.to_string(), Style::default().bg(t.bg).fg(fg))
        })
        .collect()
}

/// Render the bottom status bar.
///
/// Curated dense default: `model │ mode │ ctx-bar │ $cost │ duration`. When
/// the user has configured a `statusLine.command`, its last-good cached
/// output replaces the whole row instead (the command is run off-thread by
/// the background sampler — never here).
///
/// `sample` is accepted for router signature compatibility; the cpu/ram
/// readouts are no longer rendered (dev info, not chrome).
///
/// On a narrow terminal the cost + duration segments drop first so the
/// model and context bar always survive.
pub fn status_bar(f: &mut Frame, area: Rect, app: &App, t: &Theme, _sample: SystemSample) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let bar_style = Style::default().bg(t.bg).fg(t.text_dim);

    // While the Ctrl+C quit guard is armed the status bar is given over
    // entirely to the confirm prompt — it must be impossible to miss.
    if app.quit_armed {
        let hint = Paragraph::new(Line::from(Span::styled(
            " Press Ctrl+C again to exit ",
            Style::default()
                .bg(t.bg)
                .fg(t.warning)
                .add_modifier(Modifier::BOLD),
        )))
        .style(bar_style);
        f.render_widget(hint, area);
        return;
    }

    // statusLine override (SPEC §6): when a user command is configured and
    // the background sampler has published a (sanitized, one-line) result,
    // paint that as the whole row. The string is plain data — already
    // ANSI/OSC-stripped by the sampler — so it can never inject escapes.
    // The cache is a process-global owned by the `statusline` module (one
    // status bar + one sampler per process); this widget only READS it.
    if let Some(line) = crate::tui::statusline::cached_line() {
        let custom = Paragraph::new(Line::from(Span::styled(
            format!(" {line} "),
            Style::default().bg(t.bg).fg(t.text),
        )))
        .style(bar_style);
        f.render_widget(custom, area);
        return;
    }

    let display = if app.config.model.is_empty() {
        "no model".to_string()
    } else {
        display_model(&app.config.model).to_string()
    };

    // Truncate with `…` on a tiny terminal so the bar never shows a
    // ragged mid-token cut. Budget half the available width for the
    // model segment.
    let model_budget = (area.width / 2).saturating_sub(2) as usize;
    let model = if display.chars().count() <= model_budget {
        display
    } else {
        let mut truncated: String = display
            .chars()
            .take(model_budget.saturating_sub(1))
            .collect();
        truncated.push('…');
        truncated
    };

    let mut spans: Vec<Span> = Vec::new();

    // Left: model name in primary text (bold). Demoted from `t.orange`
    // (v0.9.1.3 J accent-inflation finding) — bold-text-on-bg reads as
    // informational without spending the brand budget.
    spans.push(Span::styled(
        format!(" {model} "),
        Style::default()
            .bg(t.bg)
            .fg(t.text)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(divider(t));

    // Session mode label (the engine's real SessionMode).
    spans.push(Span::styled(
        format!(" {} ", mode_label(&app.mode)),
        bar_style,
    ));
    if matches!(&app.mode, wcore_protocol::commands::SessionMode::Force) {
        spans.push(Span::styled(
            " · FORCE ",
            Style::default()
                .bg(t.bg)
                .fg(t.warning)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(divider(t));

    // Context-usage bar (4-threshold + 9-level sub-char fill, S18).
    spans.push(Span::styled(" ctx ", bar_style));
    let pct = app.context.pct();
    spans.extend(ctx_bar(pct, CTX_CELLS, t));
    spans.push(Span::styled(
        format!(" {:>3}% ", (pct * 100.0).round() as u32),
        bar_style,
    ));

    // Session cost + duration — dropped first when the row gets tight.
    // Cost is always present (never gated on a "small enough to hide"
    // heuristic) — IJFW discipline: the user always sees the spend.
    if area.width >= COST_SEGMENT_MIN_WIDTH {
        spans.push(divider(t));
        // Real-or-nothing: before any spend is recorded `app.cost` is `None` —
        // show an em-dash, never a fabricated `$0.00` (matches the `/config`
        // health line and Sean's cost = real-or-nothing rule). A recorded zero
        // (Some(0.0)) is honest data and still prints `$0.00`.
        let cost_str = match app.cost.as_ref() {
            Some(c) => format_cost(c.total_cost_usd),
            None => "—".to_string(),
        };
        spans.push(Span::styled(
            format!(" {cost_str} "),
            Style::default().bg(t.bg).fg(t.text_dim),
        ));
        spans.push(divider(t));
        spans.push(Span::styled(
            format!(
                " {} ",
                format_duration(app.session.turn_started_at.elapsed())
            ),
            Style::default().bg(t.bg).fg(t.text_dim),
        ));
    }

    // Toast (SPEC §3 S7): a transient demoted-status message, RENDERED here
    // (emitted by the protocol bridge). Auto-dismisses after `TOAST_DWELL`.
    // We read `App` immutably in the render path, so we cannot clear the
    // field here — instead we simply stop showing it once it has expired;
    // the bridge / a future tick overwrites or clears the stale field.
    if let (Some(msg), Some(at)) = (app.toast.as_ref(), app.toast_at)
        && at.elapsed() < TOAST_DWELL
    {
        spans.push(divider(t));
        spans.push(Span::styled(
            format!(" ⚡ {msg} "),
            Style::default()
                .bg(t.bg)
                .fg(t.orange)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let line = Line::from(spans);
    let bar = Paragraph::new(line).style(bar_style);
    f.render_widget(bar, area);
}

/// A thin vertical divider between status segments.
fn divider(t: &Theme) -> Span<'static> {
    Span::styled("│", Style::default().bg(t.bg).fg(t.border))
}

/// Format a session cost as an Intl-style currency string with 3–4
/// significant figures: tiny per-turn costs keep four decimals
/// (`$0.0234`), larger spends round to two (`$1.23`).
fn format_cost(usd: f64) -> String {
    if usd > 0.0 && usd < 0.1 {
        format!("${usd:.4}")
    } else {
        format!("${usd:.2}")
    }
}

/// Format an elapsed session duration compactly: `12s`, `3m04s`, `1h02m`.
fn format_duration(d: std::time::Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Human-readable label for the session mode. The engine's `SessionMode`
/// is not `Display`, so map its variants explicitly.
fn mode_label(mode: &wcore_protocol::commands::SessionMode) -> &'static str {
    use wcore_protocol::commands::SessionMode;
    match mode {
        SessionMode::Default => "Default",
        SessionMode::AutoEdit => "Auto-edit",
        SessionMode::Force => "Force",
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::app::{App, ContextView, SessionCostView};
    use crate::tui::theme::Theme;

    /// A fixed sample so status-bar render tests are deterministic — no
    /// live `sysinfo` probe.
    fn fixed_sample() -> SystemSample {
        SystemSample {
            cpu_pct: 37.0,
            used_mem: 8 * 1024 * 1024 * 1024,
            total_mem: 16 * 1024 * 1024 * 1024,
        }
    }

    fn render(app: &App, t: &Theme, w: u16) -> String {
        let sample = fixed_sample();
        let mut terminal = Terminal::new(TestBackend::new(w, 1)).expect("test terminal");
        terminal
            .draw(|f| status_bar(f, f.area(), app, t, sample))
            .expect("render status bar");
        let buf = terminal.backend().buffer();
        (0..w).map(|x| buf[(x, 0)].symbol()).collect()
    }

    /// Count context-bar cells painted in a non-empty (filled) color.
    /// Cells use the threshold color when filled and `surface_hover` when
    /// empty, so a filled cell is one whose fg is NOT `surface_hover`.
    fn ctx_fill(app: &App, t: &Theme, w: u16) -> usize {
        let sample = fixed_sample();
        let mut terminal = Terminal::new(TestBackend::new(w, 1)).expect("test terminal");
        terminal
            .draw(|f| status_bar(f, f.area(), app, t, sample))
            .expect("render status bar");
        let buf = terminal.backend().buffer();
        let fill_glyphs = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
        (0..w)
            .filter(|&x| {
                let cell = &buf[(x, 0)];
                let sym: Vec<char> = cell.symbol().chars().collect();
                sym.len() == 1 && fill_glyphs.contains(&sym[0]) && cell.fg != t.surface_hover
            })
            .count()
    }

    // ── ctx_bar / ctx_bar_color: 4-threshold + sub-char fill (Task 5.1) ──

    #[test]
    fn ctx_bar_color_uses_four_thresholds() {
        let t = Theme::hearth();
        assert_eq!(ctx_bar_color(0.30, &t), t.success); // green <50
        assert_eq!(ctx_bar_color(0.70, &t), t.warning); // yellow <80
        assert_eq!(ctx_bar_color(0.90, &t), t.orange); // orange <95
        assert_eq!(ctx_bar_color(0.97, &t), t.error); // red >=95
    }

    #[test]
    fn ctx_bar_color_thresholds_are_inclusive_at_the_boundary() {
        let t = Theme::hearth();
        // Exactly on a boundary takes the higher band.
        assert_eq!(ctx_bar_color(0.50, &t), t.warning);
        assert_eq!(ctx_bar_color(0.80, &t), t.orange);
        assert_eq!(ctx_bar_color(0.95, &t), t.error);
        // Just below 50% is still green.
        assert_eq!(ctx_bar_color(0.4999, &t), t.success);
    }

    #[test]
    fn ctx_bar_uses_sub_character_fill_for_partial_cells() {
        let t = Theme::hearth();
        // 20 cells, ~47% → 9.4 cells: full cells + one partial sub-char.
        let spans = ctx_bar(0.47, 20, &t);
        let rendered: String = spans.iter().map(|s| s.content.clone()).collect();
        assert!(
            rendered
                .chars()
                .any(|c| ['▏', '▎', '▍', '▌', '▋', '▊', '▉'].contains(&c)),
            "expected a partial sub-char glyph in {rendered:?}"
        );
        assert_eq!(spans.len(), 20, "bar must always be exactly `cells` wide");
    }

    #[test]
    fn ctx_bar_is_all_empty_at_zero_and_all_full_at_one() {
        let t = Theme::hearth();
        let empty = ctx_bar(0.0, 20, &t);
        assert!(empty.iter().all(|s| s.content == " "), "0% must be blank");
        let full = ctx_bar(1.0, 20, &t);
        assert!(full.iter().all(|s| s.content == "█"), "100% must be solid");
        assert_eq!(empty.len(), 20);
        assert_eq!(full.len(), 20);
    }

    #[test]
    fn ctx_bar_clamps_out_of_range_input() {
        let t = Theme::hearth();
        // >1.0 clamps to full, <0.0 clamps to empty — never panics or
        // produces more than `cells` of fill.
        assert!(ctx_bar(1.5, 10, &t).iter().all(|s| s.content == "█"));
        assert!(ctx_bar(-0.5, 10, &t).iter().all(|s| s.content == " "));
    }

    // ── status_bar dense default (Task 5.2) ──

    #[test]
    fn status_bar_dense_default_shows_model_mode_ctx_cost() {
        let mut app = App::new();
        app.config.provider = "anthropic".into();
        app.config.model = "claude-opus-4-7".into();
        app.context = ContextView {
            used_tokens: 500,
            window_size: 1000,
        };
        app.cost = Some(SessionCostView {
            session_id: "s1".into(),
            total_cost_usd: 0.0234,
            per_turn: Vec::new(),
        });
        let line = render(&app, &Theme::hearth(), 120);
        assert!(line.contains("claude-opus-4-7"), "model missing: {line:?}");
        assert!(
            !line.contains("anthropic/"),
            "vendor prefix leaked: {line:?}"
        );
        assert!(line.contains("Default"), "mode missing: {line:?}");
        assert!(line.contains("ctx"), "ctx label missing: {line:?}");
        assert!(line.contains("$0.0234"), "cost missing: {line:?}");
        // Sub-char fill present in the ctx bar at 50%.
        assert!(
            line.chars().any(|c| c == '█'),
            "ctx fill glyph missing: {line:?}"
        );
    }

    #[test]
    fn status_bar_strips_vendor_prefix_from_a_slashed_slug() {
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-7".into();
        let line = render(&app, &Theme::hearth(), 100);
        assert!(
            line.contains("claude-opus-4-7"),
            "stripped model missing: {line:?}"
        );
        assert!(
            !line.contains("anthropic/"),
            "vendor prefix leaked: {line:?}"
        );
    }

    #[test]
    fn context_bar_is_empty_at_zero_percent() {
        let mut app = App::new();
        app.context = ContextView {
            used_tokens: 0,
            window_size: 1000,
        };
        assert_eq!(ctx_fill(&app, &Theme::hearth(), 120), 0);
        let line = render(&app, &Theme::hearth(), 120);
        assert!(line.contains("0%"), "0% label missing: {line:?}");
    }

    #[test]
    fn context_bar_is_full_at_one_hundred_percent() {
        let mut app = App::new();
        app.context = ContextView {
            used_tokens: 1000,
            window_size: 1000,
        };
        // All 20 cells filled at 100%.
        assert_eq!(ctx_fill(&app, &Theme::hearth(), 120), CTX_CELLS);
        let line = render(&app, &Theme::hearth(), 120);
        assert!(line.contains("100%"), "100% label missing: {line:?}");
    }

    #[test]
    fn status_bar_shows_duration_segment_on_a_wide_row() {
        // The dense default appends a session-duration readout (e.g. `0s`).
        let mut app = App::new();
        app.config.model = "local".into();
        let line = render(&app, &Theme::hearth(), 120);
        // A fresh session is <1s old → `0s`.
        assert!(line.contains("0s"), "duration segment missing: {line:?}");
    }

    #[test]
    fn status_bar_handles_a_missing_model() {
        let app = App::new();
        let line = render(&app, &Theme::hearth(), 100);
        assert!(line.contains("no model"), "fallback missing: {line:?}");
    }

    #[test]
    fn status_bar_shows_cost_always_when_present() {
        let mut app = App::new();
        app.cost = Some(SessionCostView {
            session_id: "s1".into(),
            total_cost_usd: 1.2345,
            per_turn: Vec::new(),
        });
        let line = render(&app, &Theme::hearth(), 120);
        // >= 0.1 rounds to two decimals (Intl-style).
        assert!(line.contains("$1.23"), "session cost missing: {line:?}");
        assert!(!line.contains("cpu"), "cpu leaked: {line:?}");
        assert!(!line.contains("ram"), "ram leaked: {line:?}");
    }

    #[test]
    fn status_bar_shows_em_dash_when_no_cost_recorded() {
        // Real-or-nothing: at boot `app.cost` is None — the status bar must
        // show an em-dash, never a fabricated `$0.00`.
        let app = App::new();
        assert!(app.cost.is_none(), "precondition: no cost at boot");
        let line = render(&app, &Theme::hearth(), 120);
        assert!(
            !line.contains("$0.00"),
            "must not fabricate $0.00 with no spend: {line:?}"
        );
        assert!(
            line.contains('—'),
            "spend must read an em-dash when unrecorded: {line:?}"
        );
    }

    #[test]
    fn status_bar_keeps_model_and_bar_on_a_very_narrow_row() {
        // Below the cost threshold only model + mode + ctx bar remain.
        let mut app = App::new();
        app.config.model = "local".into();
        app.cost = Some(SessionCostView {
            session_id: "s1".into(),
            total_cost_usd: 1.23,
            per_turn: Vec::new(),
        });
        let line = render(&app, &Theme::hearth(), 56);
        assert!(line.contains("local"), "model must survive: {line:?}");
        assert!(line.contains("ctx"), "ctx must survive: {line:?}");
        assert!(!line.contains('$'), "cost should be dropped: {line:?}");
    }

    #[test]
    fn status_bar_shows_the_quit_confirm_hint_while_armed() {
        let mut app = App::new();
        app.quit_armed = true;
        let line = render(&app, &Theme::hearth(), 100);
        assert!(
            line.contains("Press Ctrl+C again to exit"),
            "quit hint missing while armed: {line:?}"
        );
    }

    #[test]
    fn status_bar_hides_the_quit_hint_when_disarmed() {
        let app = App::new();
        let line = render(&app, &Theme::hearth(), 100);
        assert!(
            !line.contains("Ctrl+C"),
            "quit hint leaked while disarmed: {line:?}"
        );
    }

    #[test]
    fn status_bar_renders_the_force_badge_for_active_force_mode() {
        let mut app = App::new();
        app.config.provider = "anthropic".into();
        app.config.model = "sonnet-4-6".into();
        app.mode = wcore_protocol::commands::SessionMode::Force;
        let line = render(&app, &Theme::hearth(), 100);
        assert!(
            line.contains("FORCE"),
            "force badge missing while Force is active:\n{line}"
        );
    }

    #[test]
    fn status_bar_hides_stale_force_badge_after_deescalation() {
        let mut app = App::new();
        app.config.model = "local".into();
        app.config.force = true;
        app.mode = wcore_protocol::commands::SessionMode::Default;
        let line = render(&app, &Theme::hearth(), 100);
        assert!(
            !line.contains("FORCE"),
            "launch authority leaked a stale FORCE badge after de-escalation:\n{line}"
        );
    }

    #[test]
    fn status_bar_renders_with_no_color_theme() {
        let mut app = App::new();
        app.config.model = "local".into();
        let line = render(&app, &Theme::no_color(), 100);
        assert!(line.contains("local"));
    }

    // ── toast RENDER (Task 5.6 render-half; emit owned by WIRE-BRIDGE) ──

    #[test]
    fn status_bar_renders_a_fresh_toast() {
        let mut app = App::new();
        app.config.model = "local".into();
        app.toast = Some("filesystem ready · 12 tools".into());
        app.toast_at = Some(std::time::Instant::now());
        let line = render(&app, &Theme::hearth(), 120);
        assert!(
            line.contains("filesystem ready"),
            "fresh toast must render: {line:?}"
        );
    }

    #[test]
    fn status_bar_hides_an_expired_toast() {
        let mut app = App::new();
        app.config.model = "local".into();
        app.toast = Some("stale toast".into());
        // Set the timestamp older than the dwell window.
        app.toast_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(5));
        let line = render(&app, &Theme::hearth(), 120);
        assert!(
            !line.contains("stale toast"),
            "expired toast must not render: {line:?}"
        );
    }

    // ── statusLine override (Task 5.2 + 5.4 read path) ──

    #[test]
    fn status_bar_renders_the_curated_default_when_no_statusline_cache() {
        // No statusLine command configured → curated default (model etc.).
        // The global cache is empty in a default test process.
        let mut app = App::new();
        app.config.model = "local".into();
        assert_eq!(crate::tui::statusline::cached_line(), None);
        let line = render(&app, &Theme::hearth(), 120);
        assert!(line.contains("local"), "curated default expected: {line:?}");
        assert!(line.contains("ctx"), "curated default expected: {line:?}");
    }
}
