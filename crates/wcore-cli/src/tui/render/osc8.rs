//! OSC 8 hyperlink emission — wrap-survival, mailto-strip, nested guard.
//!
//! v0.9.2 Wave 9 (SPEC §3 S24, decision A). Terminals that honour the
//! OSC 8 escape (`ESC ] 8 ; ; <url> <terminator>`) turn the enclosed text
//! into a Cmd-click / Ctrl-click hyperlink (iTerm2, kitty, GNOME Terminal,
//! Apple Terminal, ghostty, WezTerm, Windows Terminal). Terminals that
//! ignore it render the visible text with zero added visual width.
//!
//! ## Three behaviours this module guarantees (S24)
//!
//! 1. **Wrap-survival.** A hyperlink whose display text is longer than the
//!    viewport must stay clickable on *every* visual row, not just the
//!    first. The OSC 8 state is per-cell in most terminals, but a *soft*
//!    line break inside one logical link is the boundary where some
//!    terminals (and ratatui's own wrap) drop the link. [`wrap_hyperlink`]
//!    re-opens the OSC 8 sequence on each wrapped segment so each row is
//!    independently anchored to the URL.
//! 2. **mailto-strip.** `mailto:` URLs are rendered *plain* — the email
//!    address shows as text with no clickable escape. [`is_plain_only`]
//!    is the predicate; callers skip linkification when it returns true.
//! 3. **Nested-OSC-8 guard.** Text that already contains an OSC 8 opener
//!    must not be wrapped again (double-wrapping confuses every terminal
//!    and corrupts the URL state). [`contains_osc8`] lets callers detect
//!    pre-linked text and skip.
//!
//! ## Terminator choice — BEL, not ST
//!
//! The de-facto OSC 8 spec accepts either BEL (`\x07`) or ST (`\x1b\\`).
//! The rest of the Wayland TUI (the v0.9.1.4 markdown link path) emits
//! BEL, and xterm / VTE / ratatui's ANSI re-emission pipeline handle BEL
//! most uniformly. This module emits BEL for binary-wide consistency. The
//! PLAN's example used ST; BEL is the deliberate reconciliation so a link
//! rendered through markdown and one rendered through this helper carry
//! byte-identical escapes.

/// The OSC 8 sequence terminator. BEL (one byte) over ST (`ESC \`) for
/// binary-wide consistency with the v0.9.1.4 markdown link path.
const TERM: char = '\x07';

/// The OSC 8 *opener* escape for `url`: `ESC ] 8 ; ; <url> BEL`.
///
/// Emitted as its own span by callers so ratatui's width accounting —
/// which counts every char naively, escapes included — sees only the
/// visible text when summing line width. Keeps wrap, table column sums,
/// and code-block padding correct.
pub fn open_seq(url: &str) -> String {
    format!("\x1b]8;;{url}{TERM}")
}

/// The OSC 8 *closer* escape: `ESC ] 8 ; ; BEL` (empty URL = close).
pub fn close_seq() -> String {
    format!("\x1b]8;;{TERM}")
}

/// Wrap an OSC 8 hyperlink around `text` pointing at `url`. The emitted
/// string is `ESC]8;;<url>BEL <text> ESC]8;;BEL`.
///
/// This is the single-string form (display text fits on one row). For
/// text that may soft-wrap, use [`wrap_hyperlink`] so each row stays
/// clickable. `mailto:` URLs should be filtered by the caller via
/// [`is_plain_only`] *before* calling this.
pub fn hyperlink(url: &str, text: &str) -> String {
    format!("{}{text}{}", open_seq(url), close_seq())
}

/// True when the URL should NOT be linkified — `mailto:` is rendered as
/// plain text (the email address), never a clickable escape (S24).
pub fn is_plain_only(url: &str) -> bool {
    url.starts_with("mailto:")
}

/// True when `text` already contains an OSC 8 opener — the nested guard.
/// Callers must skip linkification of text that returns true here, or the
/// terminal sees two overlapping `ESC]8` openers and corrupts both links.
pub fn contains_osc8(text: &str) -> bool {
    text.contains("\x1b]8;;")
}

/// Split `text` into soft-wrap segments of at most `width` *display*
/// columns, then wrap EACH segment in its own OSC 8 hyperlink so the link
/// survives the wrap (S24 wrap-survival). Returns one string per visual
/// row. Each returned string is independently clickable.
///
/// * `width == 0` (or text fitting in one row) yields a single segment.
/// * `mailto:` URLs are returned *plain* (no escapes), one segment per row.
/// * Text that already contains an OSC 8 opener is returned as a single
///   un-rewrapped segment (nested guard) — the caller already linkified it.
///
/// Width is measured in Unicode scalar values, matching the naive char
/// count ratatui uses for its own wrap; this keeps the segment boundaries
/// aligned with where ratatui would itself break the line.
pub fn wrap_hyperlink(url: &str, text: &str, width: usize) -> Vec<String> {
    // Nested guard: do not re-wrap text that already carries OSC 8.
    if contains_osc8(text) {
        return vec![text.to_string()];
    }
    let segments = split_segments(text, width);
    if is_plain_only(url) {
        // mailto → plain: each row is the bare text, no escapes.
        return segments;
    }
    segments
        .into_iter()
        .map(|seg| hyperlink(url, &seg))
        .collect()
}

/// Break `text` into chunks of at most `width` chars (by scalar count).
/// `width == 0` or text shorter than `width` yields the whole string as
/// one chunk. Pure, no escapes — the OSC 8 wrapping is layered on top by
/// [`wrap_hyperlink`].
fn split_segments(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return vec![text.to_string()];
    }
    chars
        .chunks(width)
        .map(|c| c.iter().collect::<String>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyperlink_wraps_and_closes_the_osc8_sequence() {
        let s = hyperlink("https://x.test", "click");
        assert!(s.starts_with("\x1b]8;;https://x.test\x07"));
        assert!(s.ends_with("\x1b]8;;\x07"));
        assert!(s.contains("click"));
    }

    #[test]
    fn open_and_close_seq_are_byte_compatible_with_markdown_path() {
        // The markdown link path (v0.9.1.4) emits exactly these byte
        // sequences as separate spans; this guards drift.
        assert_eq!(open_seq("https://x.test"), "\x1b]8;;https://x.test\x07");
        assert_eq!(close_seq(), "\x1b]8;;\x07");
    }

    #[test]
    fn mailto_is_plain_only() {
        assert!(is_plain_only("mailto:a@b.com"));
        assert!(!is_plain_only("https://x.test"));
        assert!(!is_plain_only("http://x.test"));
    }

    #[test]
    fn contains_osc8_detects_existing_opener() {
        assert!(contains_osc8("\x1b]8;;https://x.test\x07already linked"));
        assert!(!contains_osc8("plain text with no escape"));
    }

    #[test]
    fn long_link_wraps_into_at_least_two_clickable_segments() {
        // A display text longer than the width must yield ≥2 segments,
        // each independently OSC-8-wrapped (wrap-survival).
        let url = "https://example.com/very/long/path";
        let text = "this-is-a-long-anchor-text-that-must-wrap";
        let segs = wrap_hyperlink(url, text, 10);
        assert!(
            segs.len() >= 2,
            "expected ≥2 wrapped segments; got {}",
            segs.len()
        );
        for seg in &segs {
            assert!(
                seg.starts_with("\x1b]8;;https://example.com/very/long/path\x07"),
                "each wrapped row must re-open the OSC 8 sequence; got: {seg:?}"
            );
            assert!(
                seg.ends_with("\x1b]8;;\x07"),
                "each wrapped row must close the OSC 8 sequence; got: {seg:?}"
            );
        }
    }

    #[test]
    fn wrapped_segments_reconstruct_the_visible_text() {
        // Stripping the escapes from every segment and concatenating must
        // recover the original display text exactly (no chars dropped).
        let url = "https://x.test";
        let text = "abcdefghijklmnop";
        let segs = wrap_hyperlink(url, text, 5);
        let visible: String = segs
            .iter()
            .map(|s| {
                s.replace(&open_seq(url), "")
                    .replace(close_seq().as_str(), "")
            })
            .collect();
        assert_eq!(visible, text);
    }

    #[test]
    fn short_link_is_a_single_segment() {
        let segs = wrap_hyperlink("https://x.test", "ok", 80);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0], hyperlink("https://x.test", "ok"));
    }

    #[test]
    fn zero_width_is_a_single_segment() {
        let segs = wrap_hyperlink("https://x.test", "anything at all", 0);
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn mailto_wraps_plain_with_no_escapes() {
        // mailto across a narrow width: every row is plain text, no OSC 8.
        let segs = wrap_hyperlink("mailto:someone@example.com", "someone@example.com", 6);
        assert!(segs.len() >= 2, "should still split for layout");
        for seg in &segs {
            assert!(
                !contains_osc8(seg),
                "mailto must never carry an OSC 8 escape; got: {seg:?}"
            );
        }
        let joined: String = segs.concat();
        assert_eq!(joined, "someone@example.com");
    }

    #[test]
    fn nested_guard_does_not_double_wrap() {
        // Text that already has an OSC 8 opener is returned untouched —
        // never wrapped a second time.
        let already = hyperlink("https://x.test", "linked");
        let segs = wrap_hyperlink("https://y.test", &already, 4);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0], already);
        // Exactly one opener — proof we did not nest a second.
        let opener_count = segs[0].matches("\x1b]8;;https").count();
        assert_eq!(opener_count, 1, "must not nest a second OSC 8 opener");
    }
}
