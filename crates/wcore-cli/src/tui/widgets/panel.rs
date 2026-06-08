//! Bordered-panel helper — a themed `Block` factory for rails/cards.
//!
//! FROZEN Wave-0 signature; T0.4 fills the body.

use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Padding};

use crate::tui::theme::Theme;

/// Build a themed bordered `Block` with `title` as its heading.
///
/// The block carries the Hearth border color and a `surface`-tinted
/// background; the title renders in the muted-text color so it reads as
/// chrome, not content. Borders are plain single-line — flat, no shadow
/// or gradient emulation (brand §07). Surfaces drop their own content
/// into the block's `inner()` area.
///
/// The block carries one column of horizontal and one row of vertical
/// internal padding so content never crowds the frame. `Block::inner()`
/// already accounts for padding, so callers that lay out into
/// `block.inner(area)` keep working unchanged — they simply get a
/// slightly smaller, breathing-room inset.
///
/// v0.9.1.3 J: border style is `t.border` (neutral dim grey), not
/// `t.orange`. Test agent 8's accent-inflation audit found the rail
/// panel border (Activity) was 1 of 10+ orange surfaces in a single
/// viewport vs. recon §1.4 budget of 2. Chrome rails are structural,
/// not signal; orange is now reserved for the active-tab underline and
/// the user-turn `▌` (load-bearing accents only).
///
/// FROZEN Wave-0 contract.
pub fn panel<'a>(title: &str, t: &Theme) -> Block<'a> {
    // Pad the title with single spaces so the border doesn't kiss the
    // title text — the notch reads as an intentional break in the
    // frame rather than as cramped chrome.
    let padded_title = format!(" {title} ");
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.bg))
        .padding(Padding::symmetric(1, 0))
        .title(Span::styled(
            padded_title,
            Style::default().fg(t.text_muted),
        ))
        .title_style(Style::default().fg(t.text_muted))
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    use super::*;
    use crate::tui::theme::Theme;

    #[test]
    fn panel_renders_a_titled_border() {
        let t = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(20, 5)).expect("test terminal");
        terminal
            .draw(|f| {
                let block = panel("Path map", &t);
                block.render(f.area(), f.buffer_mut());
            })
            .expect("render panel");
        let buf = terminal.backend().buffer();
        // The title text appears on the top border row.
        let top: String = (0..20).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(top.contains("Path map"), "title missing: {top:?}");
        // The corners are the box-drawing glyphs of a single-line border.
        assert_eq!(buf[(0, 0)].symbol(), "┌");
        assert_eq!(buf[(19, 4)].symbol(), "┘");
    }

    #[test]
    fn panel_inner_excludes_the_border_and_padding() {
        let t = Theme::no_color();
        let block = panel("X", &t);
        let area = Rect::new(0, 0, 10, 6);
        let inner = block.inner(area);
        // A single-line border (1) plus one column of horizontal padding
        // (1) on left/right; no vertical padding. Inset: border+h_pad = 2
        // on left/right, border only = 1 on top/bottom.
        assert_eq!(inner, Rect::new(2, 1, 6, 4));
    }

    #[test]
    fn panel_horizontal_padding_keeps_content_off_the_border() {
        // Render the panel, then render a wall of `X` into its inner
        // area. The column just inside each side border must be blank —
        // horizontal padding gives content breathing room.
        let t = Theme::no_color();
        let mut terminal = Terminal::new(TestBackend::new(20, 7)).expect("test terminal");
        terminal
            .draw(|f| {
                let block = panel("Tools", &t);
                let inner = block.inner(f.area());
                block.render(f.area(), f.buffer_mut());
                ratatui::widgets::Paragraph::new("XXXXXXXXXXXXXXXX\nXXXXXXXXXXXXXXXX")
                    .render(inner, f.buffer_mut());
            })
            .expect("render padded panel");
        let buf = terminal.backend().buffer();
        // The column directly inside the left border (x == 1) is padding.
        for y in 1..6 {
            assert_eq!(
                buf[(1, y)].symbol(),
                " ",
                "left padding column should be blank at y={y}"
            );
        }
        // The column directly inside the right border (x == 18) is padding.
        for y in 1..6 {
            assert_eq!(
                buf[(18, y)].symbol(),
                " ",
                "right padding column should be blank at y={y}"
            );
        }
    }
}
