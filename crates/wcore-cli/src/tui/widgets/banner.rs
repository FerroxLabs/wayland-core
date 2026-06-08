//! WAYLAND figlet banner — the full ASCII wordmark hero.
//!
//! The hybrid-branding decision: a compact one-row `header` while working,
//! this full banner on the surfaces with room for it — the onboarding
//! intro and the idle/empty workspace state. Both call [`wayland_banner`]
//! so the wordmark, tagline, and command hint stay identical everywhere.
//!
//! The banner is themed via [`Theme`] (accent for the wordmark, muted for
//! the supporting copy) and centered in whatever `area` it is given. On a
//! terminal too short or too narrow for the full art it degrades to the
//! tagline + hint alone rather than printing a clipped, broken wordmark.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::tui::theme::Theme;

/// The WAYLAND wordmark in figlet ASCII. Six rows, each padded with
/// trailing spaces to the SAME width ([`BANNER_WIDTH`] columns) so that
/// `Alignment::Center` aligns every row at the same X coordinate — without
/// the padding the rows drift relative to one another because Paragraph
/// centers each Line by its own measured width.
const BANNER_ART: [&str; 6] = [
    " __      __  _____ _____.___.____       _____    _______  ________   ",
    "/  \\    /  \\/  _  \\\\__  |   |    |     /  _  \\   \\      \\ \\______ \\  ",
    "\\   \\/\\/   /  /_\\  \\/   |   |    |    /  /_\\  \\  /   |   \\ |    |  \\ ",
    " \\        /    |    \\____   |    |___/    |    \\/    |    \\|    `   \\",
    "  \\__/\\  /\\____|__  / ______|_______ \\____|__  /\\____|__  /_______  /",
    "       \\/         \\/\\/              \\/       \\/         \\/        \\/ ",
];

/// Number of rows in [`BANNER_ART`].
const BANNER_ROWS: u16 = 6;

/// Width of every [`BANNER_ART`] row, in columns. Every row is padded to
/// exactly this width so centering aligns them flush.
const BANNER_WIDTH: u16 = 69;

/// The product tagline shown directly under the wordmark.
const TAGLINE: &str = "the autonomous AI agent";

/// Render the full WAYLAND banner centered inside `area`.
///
/// Lays out the wordmark and the tagline as a single centered block.
/// When `area` cannot fit the full ASCII art (too narrow or too short)
/// only the tagline renders — a clipped wordmark would read as a
/// rendering bug, the degraded form reads as deliberate.
pub fn wayland_banner(f: &mut Frame, area: Rect, t: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    // The wordmark fits only when the area can hold the art plus the
    // tagline row and a separating blank line.
    let fits_art = area.width >= BANNER_WIDTH && area.height >= BANNER_ROWS + 2;
    if fits_art {
        for row in BANNER_ART.iter() {
            lines.push(Line::from(Span::styled(
                row.to_string(),
                Style::default().fg(t.orange).add_modifier(Modifier::BOLD),
            )));
        }
        lines.push(Line::from(""));
    } else {
        // Degraded hero: a bold "WAYLAND" word stands in for the art.
        lines.push(Line::from(Span::styled(
            "WAYLAND",
            Style::default().fg(t.orange).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        TAGLINE,
        Style::default().fg(t.text_dim),
    )));
    // The "type / for commands" hint moved into the composer as ghost
    // placeholder text (workspace.rs render_composer) so it sits where
    // typing actually happens, not floating below the banner.

    // Vertically center the block: pad the top with blank rows so the
    // banner sits in the middle of `area` rather than at its top edge.
    let content_rows = lines.len() as u16;
    let top_pad = area.height.saturating_sub(content_rows) / 2;
    let mut padded: Vec<Line> = Vec::with_capacity(lines.len() + top_pad as usize);
    for _ in 0..top_pad {
        padded.push(Line::from(""));
    }
    padded.extend(lines);

    let para = Paragraph::new(Text::from(padded)).alignment(Alignment::Center);
    f.render_widget(para, area);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::theme::Theme;

    /// Render the banner and flatten the `TestBackend` buffer to one
    /// string for substring assertions.
    fn render(w: u16, h: u16) -> String {
        let t = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal
            .draw(|f| wayland_banner(f, f.area(), &t))
            .expect("render banner");
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
    fn banner_renders_the_full_ascii_art_when_it_fits() {
        // A wide, tall area shows the figlet wordmark — the distinctive
        // `\__/\  /` strut from the bottom rows is a reliable fingerprint.
        let out = render(90, 16);
        assert!(out.contains("\\__/\\  /"), "ascii wordmark missing:\n{out}");
        assert!(
            out.contains("the autonomous AI agent"),
            "tagline missing:\n{out}"
        );
        // The "type / for commands" hint is now the composer placeholder,
        // not part of the banner — must NOT leak back in here.
        assert!(
            !out.contains("type / for commands"),
            "hint leaked into banner:\n{out}"
        );
    }

    #[test]
    fn banner_degrades_to_a_word_on_a_narrow_area() {
        // Too narrow for the 69-column art — the bold WAYLAND word stands
        // in, and the tagline still renders.
        let out = render(40, 12);
        assert!(out.contains("WAYLAND"), "degraded wordmark missing:\n{out}");
        assert!(
            out.contains("the autonomous AI agent"),
            "tagline missing on narrow area:\n{out}"
        );
        // The full ASCII art's strut must NOT appear — it cannot fit.
        assert!(
            !out.contains("\\__/\\  /"),
            "ascii art rendered in too-narrow area:\n{out}"
        );
    }

    #[test]
    fn banner_rows_have_uniform_width() {
        // Every art row is padded to BANNER_WIDTH so centering aligns
        // them at the same X — the row drift that motivated the padding
        // would have shown up here.
        for (i, row) in BANNER_ART.iter().enumerate() {
            assert_eq!(
                row.chars().count() as u16,
                BANNER_WIDTH,
                "banner row {i} has width {} expected {BANNER_WIDTH}",
                row.chars().count()
            );
        }
    }

    #[test]
    fn banner_does_not_panic_on_a_tiny_area() {
        // A 1×1 area must clamp, not panic.
        let _ = render(1, 1);
        let _ = render(10, 2);
    }

    #[test]
    fn banner_renders_with_the_no_color_theme() {
        let t = Theme::no_color();
        let mut terminal = Terminal::new(TestBackend::new(90, 16)).expect("test terminal");
        terminal
            .draw(|f| wayland_banner(f, f.area(), &t))
            .expect("render banner uncolored");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..16 {
            for x in 0..90 {
                out.push_str(buf[(x, y)].symbol());
            }
        }
        assert!(out.contains("the autonomous AI agent"));
    }
}
